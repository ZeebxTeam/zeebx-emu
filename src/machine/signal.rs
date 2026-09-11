//! ISignal e os callbacks: o que o jogo pediu para ser avisado.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Roda os callbacks pendentes na fronteira entre duas chamadas de API."""
    ///
    /// O BREW entrega notificações pelo laço de eventos dele, quando o applet já devolveu o
    /// controle. Nosso equivalente mais próximo é este ponto: a chamada de API terminou, o
    /// resultado já está em `r0` e o guest ainda não retomou. Todo o contexto é salvo e
    /// devolvido, de forma que o chamador não perceba o desvio.
    pub(super) fn run_pending_callbacks(&mut self, budget: u64) -> Result<(), CpuError> {
        self.poll_media()?;
        let idle = self.pending_calls.is_empty()
            && self.pending_probes.is_empty()
            && self.pending_blits.is_empty();
        if idle || self.nesting >= MAX_NESTING {
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        for _ in 0..CALLBACK_ROUNDS {
            for target in std::mem::take(&mut self.pending_probes) {
                self.probe_foreign_surface(target, budget)?;
            }
            for blit in std::mem::take(&mut self.pending_blits) {
                self.blit_into_foreign(blit, budget)?;
            }
            let pending = std::mem::take(&mut self.pending_calls);
            if pending.is_empty() {
                break;
            }
            for call in pending {
                if call.function != 0 {
                    self.call_guest(call.function, call.args, budget)?;
                }
            }
        }
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        Ok(())
    }

    /// Descobre a função a chamar por trás de um par `(pfn, pUser)`.
    ///
    /// O `ISHELL_SetTimerEx` do SDK é uma macro sobre o `SetTimer`, e ela passa **o mesmo
    /// ponteiro** nos dois argumentos:
    ///
    /// ```c
    /// #define ISHELL_SetTimerEx(p,s,pcb) \
    ///     GET_PVTBL(p,IShell)->SetTimer(p, s, (PFNNOTIFY)(void *)pcb, (void *)pcb)
    /// ```
    ///
    /// Quando os dois são iguais, o que chegou é um `AEECallback *`, e quem deve ser chamado
    /// está dentro dele: `pfnNotify` no offset 16 e `pNotifyData` no 20 (`inc/AEECallback.h`).
    /// Um `pfn` de verdade nunca coincide com o seu `pUser` — um é código, o outro é dado.
    pub(super) fn resolve_notify(&mut self, callback: Callback) -> Result<Callback, CpuError> {
        if callback.function == 0 || callback.function != callback.context {
            return Ok(callback);
        }
        let base = callback.function;
        Ok(Callback {
            function: self.cpu.read_u32(base + 16).unwrap_or(0),
            context: self.cpu.read_u32(base + 20).unwrap_or(0),
        })
    }

    /// Entrega os sinais disparados, chamando os callbacks do guest.
    ///
    /// Precisa rodar fora do despacho de uma chamada, quando o guest não está no meio de
    /// outra — daí a fila.
    pub fn deliver_signals(&mut self, budget: u64) -> Result<Vec<Outcome>, CpuError> {
        // A resposta de rede compartilha esta fronteira pelo mesmo motivo dos sinais: entregá-la
        // pede chamar o alocador do jogo, e isso só é seguro fora do despacho.
        self.flush_response()?;
        self.pinta_widgets()?;
        self.desenha_widgets()?;
        self.parte_animacao()?;
        self.skip_wheel_instructions()?;
        self.flush_keys()?;
        let pending = std::mem::take(&mut self.pending_signals);
        let mut outcomes = Vec::new();
        for callback in pending {
            if callback.function == 0 {
                continue;
            }
            outcomes.push(self.call_guest(
                callback.function,
                [callback.context, 0, 0, 0],
                budget,
            )?);
        }
        Ok(outcomes)
    }

    /// Entrega os callbacks enfileirados (som, imagem, o que vier).
    ///
    /// Mesma razão da fila de sinais: eles rodam no guest, e não dá para reentrar no guest no
    /// meio do despacho de uma chamada dele.
    pub fn deliver_callbacks(&mut self, budget: u64) -> Result<Vec<Outcome>, CpuError> {
        let mut outcomes = Vec::new();
        // Um callback pode enfileirar outro, então drena até esvaziar — com teto, para que um
        // ciclo entre callbacks não prenda o emulador.
        for _ in 0..CALLBACK_ROUNDS {
            let pending = std::mem::take(&mut self.pending_calls);
            if pending.is_empty() {
                break;
            }
            for call in pending {
                if call.function == 0 {
                    continue;
                }
                outcomes.push(self.call_guest(call.function, call.args, budget)?);
            }
        }
        Ok(outcomes)
    }

    /// `ISignal`, `ISignalCtl` e `ISignalCBFactory`.
    ///
    /// Um sinal é o aviso "aconteceu alguma coisa". O app cria um pela fábrica, passando uma
    /// função de callback e um contexto, e entrega o `ISignal` a quem for notificá-lo — o
    /// gamepad, por exemplo, via `RegisterForButtonEvent`. Quando o sistema chama
    /// `ISIGNAL_Set`, o callback do app roda.
    ///
    /// Aqui o disparo é adiado em vez de imediato: chamar o guest de dentro do despacho de uma
    /// chamada do guest seria reentrância, e o BREW também não dispara na hora — ele agenda
    /// para o laço de eventos do app. Os sinais pendentes ficam registrados até termos esse
    /// laço.
    pub(super) fn signal_call(&mut self, iface: Interface, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
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
                    self.signals.remove(&this);
                }
                remaining
            }
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                SUCCESS
            }
            // int CreateSignal(ISignalCBFactory *, void (*pfn)(void *pCx), void *pCx,
            //                  ISignal **ppiSig, ISignalCtl **ppiSigCtl)
            "CreateSignal" => {
                let signal = self.new_object(Interface::Signal)?;
                let control = self.new_object(Interface::SignalCtl)?;
                if signal == 0 || control == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                // Os dois objetos falam do mesmo sinal, então compartilham o callback.
                let callback = Callback {
                    function: a1,
                    context: a2,
                };
                self.signals.insert(signal, callback);
                self.signals.insert(control, callback);
                if a3 != 0 {
                    self.cpu.write_u32(a3, signal)?;
                }
                let out_control = self.stack_arg(0)?;
                if out_control != 0 {
                    self.cpu.write_u32(out_control, control)?;
                }
                SUCCESS
            }
            "Set" => {
                if let Some(&callback) = self.signals.get(&this) {
                    self.pending_signals.push(callback);
                }
                SUCCESS
            }
            // `Enable` rearma o sinal e `Detach` o desliga da fonte. Sem laço de eventos ainda,
            // os dois são registro de estado.
            "Enable" => SUCCESS,
            "Detach" => {
                self.signals.remove(&this);
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Entrega um evento ao applet em execução, e devolve o que ele respondeu.
    ///
    /// Só há um applet aqui, então um `cls` que não seja o dele é evento para alguém que não
    /// existe — e a resposta certa nesse caso é "ninguém tratou", que é o que o BREW responde.
    ///
    /// Chamar o guest daqui é reentrância, com o mesmo cuidado do `qsort` e da entrega de
    /// linhas de SQL: salva os registradores, respeita o teto de aninhamento, devolve tudo.
    pub(super) fn send_applet_event(&mut self, cls: u32, evt: u32, w: u16, dw: u32) -> Result<u32, CpuError> {
        // O `current_applet` só é preenchido quando o `EVT_APP_START` é despachado, e há
        // evento antes disso: a Z-Wheel monta o banco de preferências **durante a
        // construção** do applet, e para isso manda um evento para a própria classe. Nesse
        // instante o objeto já existe — o `AEEApplet_New` escreveu o ponteiro de saída antes
        // de o código do jogo rodar —, então lê-lo de lá é o que o console faz: para o shell,
        // o applet passa a existir quando é registrado, não quando é iniciado.
        //
        // Sem isto o evento voltava "ninguém tratou", e a Z-Wheel imprimia
        // `SendEvent to get PrefsDB failed` onze vezes seguidas antes de desistir da
        // configuração inteira.
        let applet = match self.current_applet {
            0 => self.cpu.read_u32(self.module.out_module + 4).unwrap_or(0),
            vivo => vivo,
        };
        if applet == 0 || (cls != 0 && cls != self.applet_class) {
            return Ok(FALSE);
        }
        if self.nesting >= MAX_NESTING {
            self.assumptions
                .insert("um evento de applet não foi entregue por aninhamento profundo");
            return Ok(FALSE);
        }
        let vtable = self.cpu.read_u32(applet)?;
        let handle_event = self.cpu.read_u32(vtable + 2 * 4)?;
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        let outcome = self.call_guest(handle_event, [applet, evt, u32::from(w), dw], QSORT_BUDGET);
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        Ok(match outcome? {
            Outcome::Returned { code } => code,
            // Um tratador que se perde não derruba quem mandou o evento: para ele, ninguém
            // tratou.
            _ => FALSE,
        })
    }
}
