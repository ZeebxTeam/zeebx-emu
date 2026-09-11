//! IThread, IQueue e o heap do BREW: a cooperação e a memória do jogo.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `IThread` (`AEECLSID_THREAD` = `0x01001017`), de `sdk/inc/AEEThread.h`.
    ///
    /// A thread do BREW é cooperativa e se apoia nos callbacks: ela roda até chamar `Suspend`,
    /// e volta quando o `AEECallback` de `GetResumeCBK` é disparado — tipicamente pelo próprio
    /// jogo, via `ISHELL_Resume`. Aqui isso vira salvar e restaurar registradores, com uma
    /// pilha por thread alocada na heap do guest.
    pub(super) fn thread_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Thread.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    if let Some(state) = self.threads.remove(&this) {
                        self.resume_callbacks.remove(&state.resume_cb);
                        self.heap.free(state.resume_cb);
                        self.heap.free(state.stack);
                    }
                }
                remaining
            }
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                SUCCESS
            }
            "Malloc" => self.malloc(a1)?,
            "Free" => {
                self.heap.free(a1);
                SUCCESS
            }
            // O pool de recursos existe para liberar tudo junto no fim da thread; a nossa heap
            // já sobrevive à thread, então segurar e soltar não muda nada.
            "HoldRsc" => SUCCESS,
            "ReleaseRsc" => SUCCESS,
            // int Start(IThread *, int nStackSz, PFNTHREAD pfStart, void *pvStart)
            "Start" => {
                let state = self.threads.entry(this).or_default();
                if state.started {
                    EALREADY
                } else {
                    let stack = self.malloc(a1.max(THREAD_MIN_STACK))?;
                    if stack == 0 {
                        ENOMEMORY
                    } else {
                        // A pilha do ARM cresce para baixo, então o topo do bloco é o `sp`
                        // inicial; a AAPCS pede alinhamento de 8.
                        let top = (stack + a1.max(THREAD_MIN_STACK)) & !7;
                        let state = self.threads.entry(this).or_default();
                        state.started = true;
                        state.stack = stack;
                        state.resume_pc = a2;
                        state.context = [0; 14];
                        state.context[0] = this;
                        state.context[1] = a3;
                        state.context[13] = top;
                        self.pending_threads.push(this);
                        SUCCESS
                    }
                }
            }
            // int Exit(IThread *, int nRv) — a thread acabou; o controle volta a quem a retomou.
            "Exit" => {
                self.finish_thread(this, a1)?;
                self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
                SUCCESS
            }
            // void Join(IThread *, AEECallback *pcb, int *pnRv)
            "Join" => {
                let call = self.resolve_notify(Callback {
                    function: a1,
                    context: a1,
                })?;
                let state = self.threads.entry(this).or_default();
                if state.finished {
                    let (code, joiner) = (state.exit_code, call);
                    if a2 != 0 {
                        self.cpu.write_u32(a2, code)?;
                    }
                    self.queue_call(joiner);
                } else {
                    state.joiners.push((call, a2));
                }
                SUCCESS
            }
            // void Suspend(IThread *) — o único ponto em que a thread devolve o controle.
            //
            // Salvamos os registradores como estão e trocamos o endereço de retorno pelo
            // sentinela: o laço de execução vai parar aí, e quem retomar continua no `lr` que
            // guardamos.
            "Suspend" => {
                let resume_pc = self.cpu.read_reg(Reg::Lr);
                let context = THREAD_REGS.map(|reg| self.cpu.read_reg(reg));
                let state = self.threads.entry(this).or_default();
                state.resume_pc = resume_pc;
                state.context = context;
                state.suspended = true;
                self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
                SUCCESS
            }
            // AEECallback *GetResumeCBK(IThread *) — o mesmo callback a cada chamada, porque é
            // por ele que reconhecemos um `ISHELL_Resume` dirigido à thread.
            "GetResumeCBK" => {
                let existing = self.threads.entry(this).or_default().resume_cb;
                if existing != 0 {
                    existing
                } else {
                    let cb = self.malloc(CALLBACK_SIZE)?;
                    self.threads.entry(this).or_default().resume_cb = cb;
                    if cb != 0 {
                        self.resume_callbacks.insert(cb, this);
                    }
                    cb
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Encerra uma thread: solta a pilha e libera quem esperava por ela.
    pub(super) fn finish_thread(&mut self, thread: u32, code: u32) -> Result<(), CpuError> {
        let Some(state) = self.threads.get_mut(&thread) else {
            return Ok(());
        };
        state.finished = true;
        state.exit_code = code;
        let stack = std::mem::take(&mut state.stack);
        let joiners = std::mem::take(&mut state.joiners);
        self.heap.free(stack);
        self.pending_threads.retain(|&t| t != thread);
        for (call, out) in joiners {
            if out != 0 {
                self.cpu.write_u32(out, code)?;
            }
            self.queue_call(call);
        }
        Ok(())
    }

    /// Dá uma volta a cada thread que pediu para voltar.
    ///
    /// Uma volta por quadro, e só a partir daqui. Uma thread cooperativa cede o controle
    /// esperando ser retomada na próxima passada do laço de eventos, e o laço de eventos deste
    /// emulador é o laço de quadros — retomá-la também a cada fronteira entre chamadas de API
    /// faria o jogo rodar dezenas de quadros internos para cada quadro nosso.
    pub(super) fn run_pending_threads(&mut self, budget: u64) -> Result<(), CpuError> {
        if self.pending_threads.is_empty() || self.current_thread.is_some() {
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        for thread in std::mem::take(&mut self.pending_threads) {
            self.resume_thread(thread, budget)?;
        }
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        Ok(())
    }

    /// Retoma uma thread do ponto em que ela parou.
    pub(super) fn resume_thread(&mut self, thread: u32, budget: u64) -> Result<(), CpuError> {
        let Some(state) = self.threads.get(&thread) else {
            return Ok(());
        };
        if state.finished {
            return Ok(());
        }
        let (context, pc) = (state.context, state.resume_pc);
        for (reg, value) in THREAD_REGS.iter().zip(context) {
            self.cpu.write_reg(*reg, value);
        }
        self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        self.threads.entry(thread).or_default().suspended = false;
        self.current_thread = Some(thread);
        let outcome = self.execute(pc, budget);
        self.current_thread = None;
        // Voltar sem ter passado por `Suspend` significa que a função de entrada retornou: a
        // thread acabou, mesmo sem `Exit`.
        match outcome? {
            Outcome::Returned { code } => {
                let ceded = self
                    .threads
                    .get(&thread)
                    .is_some_and(|t| t.suspended || t.finished);
                if !ceded {
                    self.finish_thread(thread, code)?;
                }
            }
            // Um desfecho que não é retorno — API faltando, falha de memória — precisa chegar
            // ao relatório: é dentro da thread que o jogo passa a maior parte do tempo.
            other => self.stalled = Some(other),
        }
        Ok(())
    }

    /// Enfileira um callback do guest para a próxima fronteira entre chamadas.
    pub(super) fn queue_call(&mut self, call: Callback) {
        if call.function != 0 {
            self.pending_calls.push(GuestCall {
                function: call.function,
                args: [call.context, 0, 0, 0],
            });
        }
    }

    /// `IHeap` (`AEECLSID_HEAP` = `0x01001002`), de `sdk/inc/AEEHeap.h`.
    ///
    /// A alocação é a mesma do `MALLOC` dos helpers — é o mesmo heap do guest, só que
    /// alcançado por outra porta. O que faltava de verdade era o `CheckAvail`: o Double Dragon
    /// pergunta se cabe o que ele quer antes de começar e, sem ninguém para responder,
    /// desenhava "Memory is insufficient" em vez do jogo.
    pub(super) fn heap_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Heap.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => self.objects.release(this),
            "Malloc" => self.malloc(a1 & !ALLOC_NO_ZMEM)?,
            "Realloc" => self.realloc(a1, a2 & !ALLOC_NO_ZMEM)?,
            "Free" => {
                self.heap.free(a1);
                SUCCESS
            }
            "StrDup" => {
                // `AECHAR` é UTF-16: a cópia leva o terminador junto.
                let units = self.read_aechar_units(a1)?;
                let bytes: Vec<u8> = units
                    .iter()
                    .chain(std::iter::once(&0))
                    .flat_map(|u| u.to_le_bytes())
                    .collect();
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr != 0 {
                    self.cpu.write_mem(ptr, &bytes)?;
                }
                ptr
            }
            // `boolean`: cabe se ainda há esse tanto livre no heap do guest.
            "CheckAvail" => u32::from(self.heap_available() >= a1 as u64),
            // A documentação é explícita: o que sai daqui é o total **em uso**, e o total do
            // aparelho vem do `ISHELL_GetDeviceInfo`.
            "GetMemStats" => self.heap.used(),
            "GetModuleMemStats" => {
                // O pico não é acompanhado; devolver o uso corrente nos dois é a resposta
                // honesta mais próxima, e é o que o jogo compara com o que precisa.
                let used = self.heap.used();
                self.write_at(a2, used)?;
                self.write_at(a3, used)?;
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Quantos bytes o heap do guest ainda pode entregar.
    pub(super) fn heap_available(&self) -> u64 {
        (loader::HEAP_SIZE as u64).saturating_sub(u64::from(self.heap.used()))
    }

    /// `realloc`: aloca o novo tamanho e copia o conteúdo antigo.
    pub(super) fn realloc(&mut self, ptr: u32, size: u32) -> Result<u32, CpuError> {
        if ptr == 0 {
            return self.malloc(size);
        }
        let old_size = self.heap.size_of(ptr).unwrap_or(0);
        let new_ptr = self.malloc(size)?;
        if new_ptr != 0 && old_size > 0 {
            self.copy_guest(new_ptr, ptr, old_size.min(size))?;
        }
        self.heap.free(ptr);
        Ok(new_ptr)
    }

    /// `MALLOC` do BREW: devolve memória, ou 0 se não houver espaço.
    ///
    /// O tamanho pode vir com `ALLOC_NO_ZMEM` (`0x80000000`) no bit alto, pedindo memória
    /// **sem** zerar — é o que o `malloc` da libc do SDK faz (`MALLOC(size|ALLOC_NO_ZMEM)`).
    /// Ignorar essa flag faz o emulador tentar alocar dois gigabytes e devolver nulo, e o jogo
    /// conclui, corretamente, que ficou sem memória.
    pub(super) fn malloc(&mut self, size: u32) -> Result<u32, CpuError> {
        let zero = size & ALLOC_NO_ZMEM == 0;
        let size = size & !ALLOC_NO_ZMEM;
        let Some(addr) = self.heap.alloc(size) else {
            return Ok(0);
        };
        // Blocos reaproveitados carregam lixo do dono anterior; zeramos quando pedido.
        if zero && size > 0 {
            self.cpu.write_mem(addr, &vec![0u8; size as usize])?;
        }
        Ok(addr)
    }

    pub fn heap_used(&self) -> u32 {
        self.heap.used()
    }
}
