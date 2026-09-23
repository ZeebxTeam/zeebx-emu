//! Objetos que o emulador expõe ao guest.
//!
//! Um objeto BREW é um ponteiro cuja primeira palavra aponta para a vtable. Como as nossas
//! vtables são fixas por interface e vivem em memória só de leitura, cada objeto precisa de
//! apenas uma palavra na memória do guest — o resto do estado fica aqui, no host.

use std::collections::HashMap;

use crate::brew::aee::Interface;

/// Espaço reservado a cada objeto na região do guest.
///
/// A maioria precisa de uma palavra só, o ponteiro da vtable. A exceção é o `IDIB`, que expõe
/// 36 bytes de campos públicos que o jogo lê direto — daí a folga.
const OBJECT_STRIDE: u32 = 64;

#[derive(Debug)]
pub struct ObjectStore {
    end: u32,
    next: u32,
    /// Endereços que já foram soltos e podem ser reusados.
    ///
    /// Sem isto a região de objetos é um bloco que só anda para a frente: são 64 KB a 64 bytes
    /// cada, mil e vinte e quatro objetos, e acabou. Um jogo que crie e solte objetos em laço
    /// esgota isso em segundos — a Z-Wheel, repetindo a abertura em modo de atração, ficava sem
    /// e a partir daí **tudo** falhava: `Unable to create vector model`, formulários com erro 3,
    /// e o jogo girando para sempre porque nada mais podia ser criado.
    ///
    /// Reusar é o que um alocador faz. O ponteiro solto que o jogo guardou por engano passa a
    /// apontar para outro objeto em vez de para lixo — o que é pior de depurar, sim, mas é
    /// exatamente o que acontece no console, e o contrário é um teto que nenhum jogo longo
    /// respeita.
    livres: Vec<u32>,
    /// Interface de cada objeto vivo, indexada pelo ponteiro no guest.
    kinds: HashMap<u32, Interface>,
    /// Contagem de referências, para responder `AddRef`/`Release` com honestidade.
    refs: HashMap<u32, u32>,
}

impl ObjectStore {
    /// O primeiro endereço nunca usado. Mesma função do [`crate::brew::heap::Heap::proximo`].
    pub fn proximo(&self) -> u32 {
        self.next
    }

    pub fn new(base: u32, size: usize) -> Self {
        Self {
            end: base + size as u32,
            next: base,
            livres: Vec::new(),
            kinds: HashMap::new(),
            refs: HashMap::new(),
        }
    }

    /// Reserva o endereço de um novo objeto. Quem chama grava o ponteiro de vtable.
    pub fn create(&mut self, iface: Interface) -> Option<u32> {
        let addr = match self.livres.pop() {
            Some(reusado) => reusado,
            None => {
                let addr = self.next;
                if addr.checked_add(OBJECT_STRIDE)? > self.end {
                    return None;
                }
                self.next = addr + OBJECT_STRIDE;
                addr
            }
        };
        self.kinds.insert(addr, iface);
        self.refs.insert(addr, 1);
        Some(addr)
    }

    /// Marca um endereço já existente como objeto de uma interface — usado para os objetos
    /// que o carregador monta antes de a máquina existir, como o `IShell`.
    pub fn adopt(&mut self, addr: u32, iface: Interface) {
        self.kinds.insert(addr, iface);
        self.refs.insert(addr, 1);
    }

    /// Interface de um objeto vivo. Devolve `None` para ponteiro que não criamos.
    pub fn kind_of(&self, addr: u32) -> Option<Interface> {
        self.kinds.get(&addr).copied()
    }

    /// Quantas referências um objeto tem agora. Zero para quem não existe.
    pub fn contagem(&self, addr: u32) -> u32 {
        self.refs.get(&addr).copied().unwrap_or(0)
    }

    /// Incrementa e devolve a nova contagem.
    ///
    /// **Um endereço já solto não ressuscita.** Ele está na lista de livres; recriar a contagem
    /// dele sem tirá-lo de lá faria o `Release` seguinte pô-lo na lista **outra vez**, e dois
    /// objetos vivos nasceriam no mesmo lugar. Foi assim que o `IGraphics` que a Z-Wheel cria
    /// para desenhar o aviso de lançamento nasceu em cima de um bitmap: o `SetDestination` dele
    /// caía no `IBitmap`, e o jogo escolhido nunca abria.
    pub fn add_ref(&mut self, addr: u32) -> u32 {
        if !self.refs.contains_key(&addr) && self.livres.contains(&addr) {
            return 0;
        }
        let count = self.refs.entry(addr).or_insert(0);
        *count += 1;
        *count
    }

    /// Decrementa e devolve a contagem restante. Chegando a zero, o objeto é esquecido.
    pub fn release(&mut self, addr: u32) -> u32 {
        let Some(count) = self.refs.get_mut(&addr) else {
            return 0;
        };
        *count = count.saturating_sub(1);
        let remaining = *count;
        if remaining == 0 {
            self.refs.remove(&addr);
            self.kinds.remove(&addr);
            // Nunca duas vezes o mesmo endereço na lista: seriam dois objetos vivos nele.
            if !self.livres.contains(&addr) {
                self.livres.push(addr);
            }
        }
        remaining
    }

    pub fn live_count(&self) -> usize {
        self.kinds.len()
    }

    /// Quantos objetos vivos de cada interface, do mais numeroso para o menos.
    ///
    /// Serve ao relatório. Um jogo que chega ao teto da região está vazando referência, e sem
    /// esta lista isso aparece como "uma classe qualquer parou de ser criada" — que foi
    /// exatamente como o vazamento da Z-Wheel se manifestou.
    pub fn live_by_kind(&self) -> Vec<(Interface, usize)> {
        let mut contagem: HashMap<Interface, usize> = HashMap::new();
        for kind in self.kinds.values() {
            *contagem.entry(*kind).or_default() += 1;
        }
        let mut saida: Vec<(Interface, usize)> = contagem.into_iter().collect();
        saida.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.name().cmp(b.0.name())));
        saida
    }
}


impl crate::save_state::Guardavel for ObjectStore {
    /// Grava o livro inteiro: o fim da região, o primeiro endereço nunca usado, os endereços
    /// soltos e, por ponteiro, a interface e a contagem de referências de cada objeto vivo.
    ///
    /// **Sem `kinds` o save state carrega objetos que não sabem o que são**: o ponteiro volta
    /// apontando para a vtable certa na memória, e o despacho do emulador não sabe qual interface
    /// atender. A codificação da interface é o discriminante da enumeração, e há teste que cobra a
    /// volta das 62.
    fn grava(&self, destino: &mut crate::save_state::Secoes) {
        destino.poe_u32("objects.end", self.end);
        destino.poe_u32("objects.next", self.next);
        destino.poe_u32s("objects.livres", self.livres.iter().copied());
        destino.poe_mapa(
            "objects.kinds",
            self.kinds.iter().map(|(a, i)| (*a, crate::brew::aee::codigo(*i))),
        );
        destino.poe_mapa("objects.refs", self.refs.iter().map(|(a, r)| (*a, *r)));
    }

    /// Restaura, e recusa antes de aplicar: o fim da região tem de ser o desta máquina, todo
    /// endereço vivo tem de saber a interface, e um endereço não pode estar livre e vivo.
    fn restaura(
        &mut self,
        origem: &crate::save_state::Leitor<'_>,
    ) -> Result<(), crate::save_state::Erro> {
        use crate::save_state::Erro;
        let fim = origem.u32("objects.end")?;
        if fim != self.end {
            return Err(Erro::Secao {
                nome: "objects".to_string(),
                motivo: format!(
                    "o estado é de uma região {fim:#010x}.. e esta máquina tem {:#010x}..",
                    self.end
                ),
            });
        }
        let next = origem.u32("objects.next")?;
        let livres = origem.u32s("objects.livres")?;
        let kinds_crus = origem.pares("objects.kinds")?;
        let refs = origem.pares("objects.refs")?;

        let mut kinds = std::collections::HashMap::new();
        for (endereco, codigo) in kinds_crus {
            let iface = crate::brew::aee::de_codigo(codigo).ok_or_else(|| Erro::Secao {
                nome: "objects.kinds".to_string(),
                motivo: format!("o objeto {endereco:#010x} diz ser a interface {codigo}, que este motor não conhece"),
            })?;
            kinds.insert(endereco, iface);
        }
        if let Some((endereco, _)) = kinds
            .keys()
            .find(|a| livres.contains(a))
            .map(|a| (*a, 0))
        {
            return Err(Erro::Secao {
                nome: "objects".to_string(),
                motivo: format!("o objeto {endereco:#010x} aparece solto e vivo"),
            });
        }

        self.next = next;
        self.livres = livres;
        self.kinds = kinds;
        self.refs = refs.into_iter().collect();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **O livro dos objetos volta inteiro**: o próximo endereço, os soltos, a interface de cada
    /// objeto vivo e a contagem de referências.
    ///
    /// O que este teste guarda é a diferença entre "o ponteiro voltou" e "o ponteiro voltou
    /// **sabendo o que é**". Sem a interface, o despacho do emulador não sabe qual vtable atender,
    /// e o jogo quebra no primeiro método que chamar.
    #[test]
    fn o_livro_dos_objetos_vai_e_volta() {
        use crate::save_state::{Guardavel, Leitor, Secoes};

        let mut antes = ObjectStore::new(0x4000_0000, 4096);
        let a = antes.create(Interface::Shell).expect("primeiro objeto");
        let b = antes.create(Interface::Display).expect("segundo objeto");
        // Um terceiro que **morre**: assim o estado tem uma entrada em `livres` para conferir,
        // e dois objetos vivos de interfaces diferentes.
        let morto = antes.create(Interface::Bitmap).expect("terceiro objeto");
        let _ = antes.add_ref(a);
        antes.release(morto);

        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");

        let mut depois = ObjectStore::new(0x4000_0000, 4096);
        depois.restaura(&leitor).expect("restaurou");

        assert_eq!(
            depois.kind_of(a),
            Some(Interface::Shell),
            "a interface de {a:#x} não voltou"
        );
        assert_eq!(depois.kind_of(b), Some(Interface::Display));
        assert_eq!(depois.kind_of(morto), None, "o objeto liberado não devia voltar vivo");
        assert_eq!(depois.contagem(a), antes.contagem(a), "a conta de {a:#x}");
        assert_eq!(depois.contagem(b), antes.contagem(b), "a conta de {b:#x}");
        // E o próximo objeto sai onde sairia antes: é o que faz o jogo continuar de onde parou.
        assert_eq!(depois.create(Interface::Bitmap), antes.create(Interface::Bitmap));
    }

    #[test]
    fn um_livro_de_outra_maquina_e_recusado() {
        use crate::save_state::{Guardavel, Leitor, Secoes};

        let antes = ObjectStore::new(0x4000_0000, 4096);
        let mut secoes = Secoes::nova();
        antes.grava(&mut secoes);
        let arquivo = secoes.fecha();
        let leitor = Leitor::abre(&arquivo).expect("abriu");

        let mut outra = ObjectStore::new(0x4000_0000, 8192);
        assert!(matches!(
            outra.restaura(&leitor),
            Err(crate::save_state::Erro::Secao { .. })
        ));
    }


    #[test]
    fn addref_num_objeto_solto_nao_duplica_o_endereco() {
        let mut store = ObjectStore::new(0x3000_0000, 0x1000);
        let a = store.create(Interface::Bitmap).unwrap();
        assert_eq!(store.release(a), 0);
        // Alguém ainda tinha o ponteiro e pede uma referência nele: não ressuscita.
        assert_eq!(store.add_ref(a), 0);
        assert_eq!(store.release(a), 0);
        // O endereço volta **uma** vez só: dois objetos novos não podem nascer no mesmo lugar.
        let b = store.create(Interface::Graphics).unwrap();
        let c = store.create(Interface::Bitmap).unwrap();
        assert_ne!(b, c);
    }

    #[test]
    fn objetos_ganham_enderecos_distintos() {
        let mut store = ObjectStore::new(0x3000_0000, 0x1000);
        let a = store.create(Interface::Display).unwrap();
        let b = store.create(Interface::FileMgr).unwrap();
        assert_ne!(a, b);
        assert_eq!(store.kind_of(a), Some(Interface::Display));
        assert_eq!(store.kind_of(b), Some(Interface::FileMgr));
    }

    #[test]
    fn contagem_de_referencias_sobe_e_desce() {
        let mut store = ObjectStore::new(0x3000_0000, 0x1000);
        let obj = store.create(Interface::Display).unwrap();
        assert_eq!(store.add_ref(obj), 2);
        assert_eq!(store.release(obj), 1);
        assert_eq!(store.release(obj), 0);
        assert_eq!(
            store.kind_of(obj),
            None,
            "objeto some quando a contagem zera"
        );
    }

    #[test]
    /// Soltar devolve o endereço ao alocador. Sem isso a região de objetos é um teto de mil e
    /// vinte e quatro criações para a execução inteira, e um jogo que cria e solta em laço para
    /// de conseguir criar qualquer coisa.
    fn o_endereco_solto_volta_a_ser_usado() {
        let mut loja = ObjectStore::new(0x3000_0000, 0x10000);
        let primeiro = loja.create(Interface::Widget).expect("cabe");
        let segundo = loja.create(Interface::Widget).expect("cabe");
        assert_ne!(primeiro, segundo);
        assert_eq!(loja.release(primeiro), 0);
        assert_eq!(loja.create(Interface::Widget), Some(primeiro));
    }

    #[test]
    /// O teto existe, mas é o de objetos **vivos**, não o de criações. Criar e soltar num laço
    /// tem de poder seguir para sempre.
    fn criar_e_soltar_em_laco_nao_esgota() {
        let mut loja = ObjectStore::new(0x3000_0000, 0x400);
        for _ in 0..10_000 {
            let objeto = loja
                .create(Interface::Widget)
                .expect("sempre cabe um de cada vez");
            loja.release(objeto);
        }
        assert_eq!(loja.live_count(), 0);
    }

    #[test]
    fn release_de_objeto_desconhecido_nao_estoura() {
        let mut store = ObjectStore::new(0x3000_0000, 0x1000);
        assert_eq!(store.release(0xdead), 0);
    }

    #[test]
    fn recusa_quando_a_regiao_enche() {
        let mut store = ObjectStore::new(0x3000_0000, OBJECT_STRIDE as usize);
        assert!(store.create(Interface::Display).is_some());
        assert!(store.create(Interface::Display).is_none());
    }
}
