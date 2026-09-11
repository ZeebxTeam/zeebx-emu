//! A sondagem: descobrir o que uma classe desconhecida responde.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Atende uma chamada num objeto-sonda: registra e responde `SUCCESS`.
    ///
    /// Responder sucesso a tudo é deliberado. A sonda não tenta acertar o comportamento — ela
    /// tenta **fazer o jogo andar** para ver o que ele pede em seguida. Um `EFAILED` honesto
    /// pararia a investigação na primeira chamada.
    pub(super) fn probe_call(&mut self, slot: u32) -> Result<u32, CpuError> {
        let this = self.cpu.read_reg(Reg::R0);
        let clsid = self.probe_objects.get(&this).copied().unwrap_or(0);
        // Registra **todos** os slots, o `AddRef` e o `Release` inclusive: saber que o jogo só
        // criou e soltou o objeto é resposta tão útil quanto saber que ele chamou o slot 7.
        let args = self.args();
        // A contagem é o que separa "o app chamou isto" de "o app está preso nisto": um método
        // com milhões de chamadas é um laço contra uma resposta nossa, não uso normal.
        if let Some(entrada) = self
            .probe_log
            .iter_mut()
            .find(|(c, o, s, _, _, _)| *c == clsid && *o == this && *s == slot)
        {
            entrada.5 += 1;
        } else {
            // O argumento que aponta para texto legível é quase sempre o que interessa — o
            // nome do banco, a instrução SQL. Lê-lo aqui evita ter que descobrir onde o
            // módulo foi mapeado para ir buscar no arquivo.
            let textos = args.map(|arg| self.probe_text(arg));
            self.probe_log.push((clsid, this, slot, args, textos, 1));
        }
        match slot {
            // `AddRef` e `Release` são os dois primeiros em toda interface do BREW, e a
            // contagem precisa valer: sem ela o objeto morre ou vaza no meio da observação.
            0 => Ok(self.objects.add_ref(this)),
            1 => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.probe_objects.remove(&this);
                }
                Ok(restantes)
            }
            _ => {
                // Um método que devolve objeto escreve o ponteiro num argumento de saída, e
                // devolver `SUCCESS` sem escrever nada faz o jogo seguir com lixo e morrer no
                // primeiro uso — foi o que o Z-Wheel fez. Então a sonda **entrega outra sonda**
                // no que parecer um ponteiro de saída: assim o jogo continua e a observação
                // alcança a família inteira de objetos, não só o primeiro.
                for arg in args {
                    if self.looks_like_out_pointer(arg) {
                        let filho = self.new_object(Interface::Probe)?;
                        if filho != 0 {
                            self.probe_objects.insert(filho, clsid);
                            self.cpu.write_u32(arg, filho)?;
                        }
                        break;
                    }
                }
                // A resposta combinada troca só o **valor de retorno**; a entrega do objeto no
                // ponteiro de saída continua valendo. Juntar as duas coisas já me custou uma
                // investigação: combinei "responda 1" e o jogo recebeu 1 com o ponteiro de
                // saída vazio, que é um estado que não existe em lugar nenhum.
                Ok(self
                    .probe_answers
                    .get(&(clsid, slot))
                    .copied()
                    .unwrap_or(SUCCESS))
            }
        }
    }

    /// O texto em `addr`, quando o que está lá é mesmo texto.
    ///
    /// Exige começar com caractere imprimível e ter pelo menos dois deles antes do zero: com
    /// menos que isso, qualquer inteiro pequeno viraria "string" e o registro só teria ruído.
    pub(super) fn probe_text(&self, addr: u32) -> Option<String> {
        if addr == 0 {
            return None;
        }
        let texto = self.cpu.read_cstring(addr, 120);
        let legivel = texto.len() >= 2
            && texto
                .chars()
                .all(|c| c == '\n' || c == '\t' || (' '..='~').contains(&c));
        legivel.then_some(texto)
    }

    /// Se `addr` tem cara de ponteiro de saída: alinhado, na memória do jogo e valendo zero.
    ///
    /// A exigência do zero é o que torna isto seguro de usar: um argumento que já aponta para
    /// algo não é destino de saída, e escrever nele estragaria dado do jogo.
    pub(super) fn looks_like_out_pointer(&self, addr: u32) -> bool {
        let na_memoria = (loader::HEAP_BASE..loader::HEAP_BASE + loader::HEAP_SIZE as u32)
            .contains(&addr)
            || (loader::STACK_BASE..loader::STACK_BASE + loader::STACK_SIZE as u32).contains(&addr);
        na_memoria && addr.is_multiple_of(4) && self.cpu.read_u32(addr).unwrap_or(1) == 0
    }

    /// Manda atender estas classes com um objeto-sonda em vez de recusá-las.
    pub fn probe_classes(&mut self, classes: &[u32]) {
        self.probe_classes.extend(classes);
    }

    /// Combina a resposta de um slot de sonda, para explorar o outro lado de um desvio.
    pub fn probe_answer(&mut self, clsid: u32, slot: u32, value: u32) {
        self.probe_classes.insert(clsid);
        self.probe_answers.insert((clsid, slot), value);
    }

    /// O que os jogos chamaram nas sondas, na ordem em que apareceu.
    pub fn probe_log(&self) -> &[ProbeCall] {
        &self.probe_log
    }
}
