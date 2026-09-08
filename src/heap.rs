//! Alocador do heap do guest.
//!
//! Serve o `MALLOC`/`FREE` do BREW. A contabilidade fica toda no host — nenhum cabeçalho é
//! escrito na memória do guest — porque o jogo nunca inspeciona a estrutura interna do heap.

use std::collections::HashMap;

/// Alinhamento dos blocos. O ARM exige 4; usamos 8 para não penalizar acessos de 64 bits.
const ALIGN: u32 = 8;

#[derive(Debug)]
pub struct Heap {
    base: u32,
    end: u32,
    /// Primeiro endereço nunca usado.
    next: u32,
    /// Blocos devolvidos por [`Heap::free`], como `(endereço, tamanho)`.
    free_list: Vec<(u32, u32)>,
    /// Blocos em uso, para que `free` saiba o tamanho sem o chamador informar.
    live: HashMap<u32, u32>,
}

impl Heap {
    pub fn new(base: u32, size: usize) -> Self {
        Self {
            base,
            end: base + size as u32,
            next: base,
            free_list: Vec::new(),
            live: HashMap::new(),
        }
    }

    /// Reserva `size` bytes e devolve o endereço, ou `None` se o heap encheu.
    ///
    /// Um `size` zero ainda recebe endereço próprio: o BREW devolve ponteiro válido nesse caso,
    /// e devolver o mesmo endereço duas vezes confundiria o `free`.
    pub fn alloc(&mut self, size: u32) -> Option<u32> {
        let total = size.max(1).div_ceil(ALIGN) * ALIGN;

        // Primeiro ajuste entre os blocos livres.
        if let Some(index) = self.free_list.iter().position(|&(_, len)| len >= total) {
            let (addr, len) = self.free_list.swap_remove(index);
            // O resto do bloco volta para a lista em vez de virar desperdício.
            if len > total {
                self.free_list.push((addr + total, len - total));
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
    pub fn free(&mut self, ptr: u32) -> Option<u32> {
        let size = self.live.remove(&ptr)?;
        self.free_list.push((ptr, size));
        Some(size)
    }

    /// Quanto do heap já foi entregue, em bytes.
    pub fn used(&self) -> u32 {
        self.next - self.base
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn recusa_quando_o_heap_enche() {
        let mut heap = Heap::new(0x1000, 16);
        assert!(heap.alloc(8).is_some());
        assert!(heap.alloc(8).is_some());
        assert_eq!(heap.alloc(8), None);
    }
}
