//! Alocador do heap do guest.
//!
//! Serve o `MALLOC`/`FREE` do BREW. A contabilidade fica toda no host — nenhum cabeçalho é
//! escrito na memória do guest — porque o jogo nunca inspeciona a estrutura interna do heap.

use std::collections::{BTreeMap, HashMap};

/// Alinhamento dos blocos. O ARM exige 4; usamos 8 para não penalizar acessos de 64 bits.
const ALIGN: u32 = 8;

#[derive(Debug)]
pub struct Heap {
    base: u32,
    end: u32,
    /// Primeiro endereço nunca usado.
    next: u32,
    /// Blocos devolvidos por [`Heap::free`], do endereço para o tamanho.
    ///
    /// É mapa ordenado, e não lista, porque o que importa aqui é achar o vizinho de um bloco
    /// para fundir os dois — ver [`Heap::free`].
    free_list: BTreeMap<u32, u32>,
    /// Blocos em uso, para que `free` saiba o tamanho sem o chamador informar.
    live: HashMap<u32, u32>,
}

impl Heap {
    pub fn new(base: u32, size: usize) -> Self {
        Self {
            base,
            end: base + size as u32,
            next: base,
            free_list: BTreeMap::new(),
            live: HashMap::new(),
        }
    }

    /// Reserva `size` bytes e devolve o endereço, ou `None` se o heap encheu.
    ///
    /// Um `size` zero ainda recebe endereço próprio: o BREW devolve ponteiro válido nesse caso,
    /// e devolver o mesmo endereço duas vezes confundiria o `free`.
    /// O primeiro endereço nunca usado.
    ///
    /// É o que delimita o que vale gravar num save state: abaixo dele está tudo o que o jogo
    /// pediu, e acima é zero desde a construção da região.
    pub fn proximo(&self) -> u32 {
        self.next
    }

    pub fn alloc(&mut self, size: u32) -> Option<u32> {
        let total = size.max(1).div_ceil(ALIGN) * ALIGN;

        // Primeiro ajuste entre os blocos livres.
        if let Some((&addr, &len)) = self.free_list.iter().find(|&(_, &len)| len >= total) {
            self.free_list.remove(&addr);
            // O resto do bloco volta para a lista em vez de virar desperdício.
            if len > total {
                self.free_list.insert(addr + total, len - total);
            }
            self.live.insert(addr, total);
            return Some(addr);
        }

        let addr = self.next;
        if addr.checked_add(total)? > self.end {
            return None;
        }
        self.next = addr + total;
        self.live.insert(addr, total);
        Some(addr)
    }

    /// Tamanho de um bloco vivo — usado pelo `realloc` para saber quanto copiar.
    pub fn size_of(&self, ptr: u32) -> Option<u32> {
        self.live.get(&ptr).copied()
    }

    /// Devolve um bloco. Retorna o tamanho liberado, ou `None` se o ponteiro não estava vivo
    /// — inclusive o ponteiro nulo, que `FREE` aceita sem fazer nada.
    ///
    /// **O bloco é fundido com os vizinhos livres.** Sem isso o heap se despedaça: cada troca
    /// de tela do Treino Cerebral devolve centenas de blocos pequenos, nenhum deles volta a
    /// formar uma região grande, e o pedido de um mega que vem na fase seguinte não acha onde
    /// caber mesmo com dezenas de megas livres — o jogo não confere o ponteiro nulo e morre
    /// chamando um método em zero.
    pub fn free(&mut self, ptr: u32) -> Option<u32> {
        let size = self.live.remove(&ptr)?;
        let (mut inicio, mut fim) = (ptr, ptr + size);
        // O vizinho de baixo, se ele termina exatamente onde este começa.
        if let Some((&antes, &len)) = self.free_list.range(..inicio).next_back()
            && antes + len == inicio
        {
            self.free_list.remove(&antes);
            inicio = antes;
        }
        // E o de cima, se começa exatamente onde este termina.
        if let Some(&len) = self.free_list.get(&fim) {
            self.free_list.remove(&fim);
            fim += len;
        }
        // Um bloco que encosta no topo não precisa de lista: o `next` recua e ele vira espaço
        // novo de novo, do tamanho que for pedido depois.
        if fim == self.next {
            self.next = inicio;
        } else {
            self.free_list.insert(inicio, fim - inicio);
        }
        Some(size)
    }

    /// Quanto do heap está entregue ao jogo, em bytes.
    ///
    /// Não é `next - base`: o que está na lista de livres já voltou, e contá-lo faria o
    /// `GetRAMFree` mentir para quem pergunta antes de alocar.
    pub fn used(&self) -> u32 {
        let livres: u32 = self.free_list.values().sum();
        (self.next - self.base).saturating_sub(livres)
    }

    /// **O maior bloco que uma alocação única consegue obter**, e não a soma do que sobra.
    ///
    /// A diferença não é acadêmica: `MALLOC` pede um bloco **contíguo**, e responde por dois
    /// caminhos — um buraco da lista de livres do tamanho pedido, ou o ponteiro de avanço. Somar
    /// os buracos com o que resta à frente responde um número que nenhuma alocação consegue.
    ///
    /// Medido em 22/09/2026 com a Z-Wheel: ela pergunta quanto há livre, pede **isso** numa
    /// alocação só (o seu próprio `Free(67001392)` vira um `malloc` de 67001488), e a soma
    /// mentirosa fazia o pedido ser recusado — `check_malloc: Malloc failed in Tectoy.c at line
    /// 570` — com o heap longe de estar cheio. Quem decide se o aparelho dá conta é o jogo,
    /// e ele decide pelo número que nós informamos.
    pub fn maior_bloco(&self) -> u32 {
        let a_frente = self.end.saturating_sub(self.next);
        let buraco = self.free_list.values().copied().max().unwrap_or(0);
        a_frente.max(buraco)
    }

    /// **Como o heap está partido**, e não só quanto sobra.
    ///
    /// [`Heap::maior_bloco`] responde o que uma alocação consegue; este retrato responde *por que*
    /// ela consegue o que consegue. A distância entre o total livre e o maior buraco é a medida da
    /// fragmentação: com a fusão funcionando ela fica pequena mesmo depois de o jogo passar a vida
    /// alocando e liberando blocos de tamanhos diferentes. É o número que distingue "o heap encheu"
    /// de "o heap se despedaçou" — os dois sintomas que o jogo relata como falta de memória.
    pub fn retrato(&self) -> Retrato {
        let na_lista: u32 = self.free_list.values().sum();
        Retrato {
            buracos: self.free_list.len(),
            maior_buraco: self.maior_bloco(),
            livre: na_lista + self.end.saturating_sub(self.next),
            usado: self.used(),
            vivos: self.live.len(),
            teto: self.end - self.base,
        }
    }
}

/// O estado da memória do guest num instante: o que a fragmentação precisa para ser um número.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retrato {
    /// Quantos buracos a lista de livres tem.
    pub buracos: usize,
    /// O maior bloco que uma alocação única obtém.
    pub maior_buraco: u32,
    /// Quanto há livre no total, somados os buracos e o que resta à frente.
    pub livre: u32,
    /// Quanto está entregue ao jogo.
    pub usado: u32,
    /// Quantos blocos vivos.
    pub vivos: usize,
    /// O tamanho da região.
    pub teto: u32,
}

impl Retrato {
    /// A parte do total livre que **não** serve para uma alocação única, em bytes.
    ///
    /// Zero quando existe um buraco do tamanho de tudo o que está livre. Quanto maior, mais
    /// despedaçado está o heap.
    pub fn perdido_em_buracos(&self) -> u32 {
        self.livre.saturating_sub(self.maior_buraco)
    }
}


impl Heap {
    /// Grava com um prefixo de seção.
    ///
    /// O prefixo existe porque a máquina tem **dois** heaps: a memória do jogo e a região das
    /// superfícies. Com nome fixo, o segundo sobrescreveria o primeiro no arquivo — e o defeito
    /// apareceria como "as superfícies voltaram no lugar do heap".
    pub fn grava_com_prefixo(&self, prefixo: &str, destino: &mut crate::save_state::Secoes) {
        destino.poe_u32(&format!("{prefixo}.base"), self.base);
        destino.poe_u32(&format!("{prefixo}.end"), self.end);
        destino.poe_u32(&format!("{prefixo}.next"), self.next);
        destino.poe_mapa(
            &format!("{prefixo}.free_list"),
            self.free_list.iter().map(|(a, t)| (*a, *t)),
        );
        destino.poe_mapa(&format!("{prefixo}.live"), self.live.iter().map(|(a, t)| (*a, *t)));
    }

    /// Restaura de um prefixo, conferindo que o heap é desta máquina e que o conteúdo é coerente.
    pub fn restaura_com_prefixo(
        &mut self,
        prefixo: &str,
        origem: &crate::save_state::Leitor<'_>,
    ) -> Result<(), crate::save_state::Erro> {
        use crate::save_state::Erro;
        let base = origem.u32(&format!("{prefixo}.base"))?;
        let end = origem.u32(&format!("{prefixo}.end"))?;
        if base != self.base || end != self.end {
            return Err(Erro::Secao { nome: prefixo.to_string(), motivo: format!(
                "o estado é de uma região {base:#010x}..{end:#010x} e esta máquina tem {:#010x}..{:#010x}",
                self.base, self.end) });
        }
        let next = origem.u32(&format!("{prefixo}.next"))?;
        let livres = origem.pares(&format!("{prefixo}.free_list"))?;
        let vivos = origem.pares(&format!("{prefixo}.live"))?;
        if let Some((endereco, _)) = vivos.iter().find(|(a, _)| livres.iter().any(|(f, _)| f == a)) {
            return Err(Erro::Secao {
                nome: prefixo.to_string(),
                motivo: format!("o endereço {endereco:#010x} aparece livre e em uso"),
            });
        }
        self.next = next;
        self.free_list = livres.into_iter().collect();
        self.live = vivos.into_iter().collect();
        Ok(())
    }
}

impl crate::save_state::Guardavel for Heap {
    /// Grava os cinco campos. `base` e `end` não mudam depois da construção, e vão junto de
    /// propósito: na volta eles servem de **conferência** de que o estado é desta máquina.
    fn grava(&self, destino: &mut crate::save_state::Secoes) {
        self.grava_com_prefixo("heap", destino);
    }

    /// Restaura, e **recusa** o estado que não for desta máquina ou estiver inconsistente.
    ///
    /// As duas checagens não são enfeite: um heap restaurado com `base` diferente faria todo
    /// ponteiro do guest apontar para o lugar errado, e o sintoma apareceria muito depois, como
    /// acesso inválido em endereço sem relação com o save state.
    fn restaura(&mut self, origem: &crate::save_state::Leitor<'_>) -> Result<(), crate::save_state::Erro> {
        self.restaura_com_prefixo("heap", origem)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **"Quanto há livre" para uma alocação única é o maior bloco contíguo.**
    ///
    /// O teste existe por uma medição: a Z-Wheel pergunta quanto há livre e pede isso numa
    /// alocação só. Somando os buracos ao que resta à frente, a resposta era maior do que qualquer
    /// alocação consegue — o pedido era recusado com o heap longe de estar cheio, e o jogo
    /// concluía, corretamente para o número que ouviu, que não havia memória.
    #[test]
    fn o_livre_de_uma_alocacao_unica_e_o_maior_bloco() {
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(0x100).unwrap();
        let b = heap.alloc(0x100).unwrap();
        let c = heap.alloc(0x100).unwrap();
        heap.free(b);
        // 0x1000 no total, 0x300 em uso: à frente restam 0xD00, e o buraco do meio é 0x100.
        assert_eq!(heap.maior_bloco(), 0xD00, "o buraco não entra na conta");
        // Com o maior bloco menor que o pedido, o `alloc` recusa — e é isso que o `CheckAvail`
        // tem de dizer antes, em vez de somar.
        assert_eq!(heap.alloc(0xD08), None);
        // 0xD00 ainda cabe à frente, e é de lá que sai: o buraco do meio é pequeno demais para
        // este pedido, e o alocador não move blocos vivos.
        assert_eq!(heap.alloc(0xD00), Some(0x1300), "servido pelo ponteiro de avanço");
        // O que passa a valer é só o buraco: um pedido maior que ele não cabe em lugar nenhum.
        assert_eq!(heap.maior_bloco(), 0x100, "resta o buraco do meio");
        assert_eq!(heap.alloc(0x108), None);
        let _ = (a, c);
    }

    /// **A fusão impede o heap de se despedaçar.**
    ///
    /// O padrão é o do relato do Treino Cerebral: blocos pequenos alocados e liberados entre dois
    /// grandes. O teste **esgota o ponteiro de avanço antes de medir** — sem isso o espaço à frente
    /// esconde a fragmentação, e o retrato parece saudável justamente quando não é.
    #[test]
    fn liberar_blocos_pequenos_devolve_uma_regiao_continua() {
        let mut heap = Heap::new(0x1000_0000, 1 << 20);
        let _primeiro = heap.alloc(64 * 1024).unwrap();
        let pequenos: Vec<u32> = (0..64).map(|_| heap.alloc(4 * 1024).unwrap()).collect();
        // Enche o que resta à frente: assim o único livre que sobra é o dos buracos do meio.
        while heap.alloc(4 * 1024).is_some() {}
        let antes = heap.retrato();
        assert!(
            antes.maior_buraco < 64 * 1024,
            "o avanço ainda escondia espaço: o maior bloco é {}",
            antes.maior_buraco
        );
        for p in &pequenos {
            heap.free(*p);
        }
        let depois = heap.retrato();
        assert!(
            depois.maior_buraco >= 256 * 1024,
            "os 64 blocos de 4 KiB deviam voltar a formar 256 KiB; o maior é {}",
            depois.maior_buraco
        );
        assert_eq!(depois.buracos, 1, "e virar um buraco só");
        // O que sobra à frente (menos de 4 KiB) não serve para nada grande, e é a única parte do
        // livre que não está num buraco: a fragmentação fica abaixo disso.
        assert!(
            depois.perdido_em_buracos() < 8 * 1024,
            "livre preso em buracos: {} bytes",
            depois.perdido_em_buracos()
        );
    }

    #[test]
    fn aloca_sequencialmente_e_alinhado() {
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(4).unwrap();
        let b = heap.alloc(4).unwrap();
        assert_eq!(a, 0x1000);
        assert_eq!(b, 0x1008, "blocos devem respeitar o alinhamento de {ALIGN}");
    }

    #[test]
    fn reaproveita_bloco_liberado() {
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(32).unwrap();
        assert_eq!(heap.free(a), Some(32));
        assert_eq!(heap.alloc(16), Some(a), "deveria reusar o bloco livre");
        // O restante do bloco continua disponível.
        assert_eq!(heap.alloc(16), Some(a + 16));
    }

    #[test]
    fn free_de_ponteiro_invalido_e_inofensivo() {
        let mut heap = Heap::new(0x1000, 0x1000);
        assert_eq!(heap.free(0), None);
        assert_eq!(heap.free(0xdead), None);
    }

    #[test]
    fn nao_entrega_dois_ponteiros_iguais_para_tamanho_zero() {
        let mut heap = Heap::new(0x1000, 0x1000);
        assert_ne!(heap.alloc(0).unwrap(), heap.alloc(0).unwrap());
    }

    #[test]
    fn blocos_vizinhos_liberados_voltam_a_ser_um_so() {
        // Sem fundir, três blocos de 32 devolvidos viram três buracos de 32 e um pedido de 96
        // vai para o topo do heap. É assim que o Treino Cerebral esgotava 64 MB.
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(32).unwrap();
        let b = heap.alloc(32).unwrap();
        let c = heap.alloc(32).unwrap();
        let depois = heap.alloc(8).unwrap();
        heap.free(b);
        heap.free(a);
        heap.free(c);
        assert_eq!(heap.alloc(96), Some(a), "os três deveriam ter virado um só");
        assert_eq!(heap.free(depois), Some(8));
    }

    #[test]
    fn o_bloco_do_topo_devolve_o_espaco_ao_heap() {
        let mut heap = Heap::new(0x1000, 64);
        let a = heap.alloc(32).unwrap();
        heap.free(a);
        assert_eq!(heap.used(), 0, "nada continua entregue");
        assert!(heap.alloc(64).is_some(), "o heap inteiro voltou a caber");
    }

    #[test]
    fn o_uso_nao_conta_o_que_ja_voltou() {
        let mut heap = Heap::new(0x1000, 0x1000);
        let a = heap.alloc(64).unwrap();
        let _b = heap.alloc(64).unwrap();
        assert_eq!(heap.used(), 128);
        heap.free(a);
        assert_eq!(heap.used(), 64, "o bloco livre no meio não está entregue");
    }

    /// Nenhum bloco vivo pode encostar em outro: a fusão mexe em vizinhança, e um erro ali
    /// entrega o mesmo endereço duas vezes — o jogo escreve por cima do que era dele.
    #[test]
    fn sob_estresse_nenhum_bloco_vivo_se_sobrepoe() {
        let mut heap = Heap::new(0x1000, 0x8000);
        let mut vivos: Vec<(u32, u32)> = Vec::new();
        let mut semente: u32 = 12345;
        let mut sorteia = move || {
            semente = semente.wrapping_mul(1103515245).wrapping_add(12345);
            semente >> 8
        };
        for passo in 0..4000 {
            let solta = !vivos.is_empty() && (passo % 3 == 0 || vivos.len() > 40);
            if solta {
                let qual = sorteia() as usize % vivos.len();
                let (addr, _) = vivos.swap_remove(qual);
                heap.free(addr);
                continue;
            }
            let tamanho = 1 + sorteia() % 600;
            let Some(addr) = heap.alloc(tamanho) else {
                continue;
            };
            let fim = addr + tamanho;
            for &(outro, outro_tam) in &vivos {
                assert!(
                    fim <= outro || addr >= outro + outro_tam,
                    "bloco {addr:#x}+{tamanho} encosta em {outro:#x}+{outro_tam}"
                );
            }
            vivos.push((addr, tamanho));
        }
    }

    /// **O heap volta do save state exatamente como estava**, e um estado de outra máquina é
    /// recusado em vez de aplicado pela metade.
    #[test]
    fn o_heap_vai_e_volta_no_save_state() {
        use crate::save_state::{Guardavel, Leitor, Secoes};

        let mut antes = Heap::new(0x2000_0000, 4096);
        let a = antes.alloc(64).expect("primeiro bloco");
        let _b = antes.alloc(128).expect("segundo bloco");
        let c = antes.alloc(32).expect("terceiro bloco");
        antes.free(c);
        let _d = antes.alloc(16).expect("reusa o bloco menor");

        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("o estado abriu");

        let mut depois = Heap::new(0x2000_0000, 4096);
        depois.restaura(&leitor).expect("restaurou");

        // **A conta não é minha, é dos dois heaps.** Em vez de eu calcular endereços de cabeça, os
        // dois recebem a mesma sequência de pedidos e têm de responder igual — é a mesma ideia do
        // teste que compara os dois rasterizadores.
        let _ = a;
        let mut esperado = Vec::new();
        let mut obtido = Vec::new();
        for tamanho in [64u32, 16, 256, 32, 1024, 8] {
            esperado.push(antes.alloc(tamanho));
            obtido.push(depois.alloc(tamanho));
        }
        assert_eq!(
            obtido, esperado,
            "o heap restaurado alocou diferente do original"
        );
    }

    #[test]
    fn um_heap_de_outra_maquina_e_recusado() {
        use crate::save_state::{Guardavel, Leitor, Secoes};

        let antes = Heap::new(0x2000_0000, 4096);
        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");

        // Outra máquina: mesmo tamanho, outro endereço. Aplicar isto mandaria todo ponteiro do
        // guest para o lugar errado.
        let mut outra = Heap::new(0x3000_0000, 4096);
        match outra.restaura(&leitor) {
            Err(crate::save_state::Erro::Secao { nome, motivo }) => {
                assert_eq!(nome, "heap");
                assert!(motivo.contains("0x20000000"), "{motivo}");
                assert!(motivo.contains("0x30000000"), "{motivo}");
            }
            outro => panic!("devia recusar o heap de outra máquina, e devolveu {outro:?}"),
        }
    }

    #[test]
    fn recusa_quando_o_heap_enche() {
        let mut heap = Heap::new(0x1000, 16);
        assert!(heap.alloc(8).is_some());
        assert!(heap.alloc(8).is_some());
        assert_eq!(heap.alloc(8), None);
    }
}
