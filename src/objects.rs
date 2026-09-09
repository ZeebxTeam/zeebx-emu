//! Objetos que o emulador expõe ao guest.
//!
//! Um objeto BREW é um ponteiro cuja primeira palavra aponta para a vtable. Como as nossas
//! vtables são fixas por interface e vivem em memória só de leitura, cada objeto precisa de
//! apenas uma palavra na memória do guest — o resto do estado fica aqui, no host.

use std::collections::HashMap;

use crate::aee::Interface;

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

    /// Incrementa e devolve a nova contagem.
    pub fn add_ref(&mut self, addr: u32) -> u32 {
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
            self.livres.push(addr);
        }
        remaining
    }

    pub fn live_count(&self) -> usize {
        self.kinds.len()
    }

    /// Quantos objetos cabem ao todo, e quantos já foram usados alguma vez.
    ///
    /// Serve ao relatório: um jogo que chegue perto do teto está vazando referência, e sem este
    /// número isso aparece como "uma classe qualquer parou de ser criada".
    pub fn capacity(&self) -> (usize, usize) {
        let stride = OBJECT_STRIDE as usize;
        let total = (self.end as usize).saturating_sub(self.next as usize) / stride
            + self.kinds.len()
            + self.livres.len();
        (self.kinds.len(), total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            let objeto = loja.create(Interface::Widget).expect("sempre cabe um de cada vez");
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
