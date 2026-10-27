//! O som que a tela puxa e a thread do alto-falante consome.
//!
//! A `sceAudioOutOutput` bloqueia até o aparelho aceitar o bloco. Fazê-lo na thread do jogo
//! segura a emulação no relógio do alto-falante. O anel é o meio: o jogo empurra, a thread puxa
//! um grão fixo e bloqueia lá.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard};

/// Amostras por chamada da porta MAIN. Tem de ser múltiplo de 64, que é o `SCE_AUDIO_MIN_LEN`.
pub const GRAO: usize = 256;

/// Um grão estéreo alinhado a 64 bytes.
///
/// A `sceAudioOutOutput` lê este endereço por DMA. Sem o alinhamento o kernel derruba o
/// processo, e isso não passa pelo pânico do Rust: o log acaba na última linha que deu tempo
/// de gravar.
#[repr(C, align(64))]
pub struct Bloco {
    pub amostras: [i16; GRAO * 2],
}

/// A porta MAIN só aceita 48 kHz. O misturador entrega nesta taxa, e o aparelho não reamostra.
pub const TAXA: u32 = 48_000;

/// Quantos quadros estéreo entregar neste passo.
///
/// O alto-falante consome a taxa cheia no relógio de parede. Entregar só o relógio do jogo deixa
/// um buraco quando o quadro demora, e a música passa a andar no ritmo da emulação. O teto de
/// 150 ms é o do core na Vita: cabe num anel curto, e um pico maior seria descartado na ponta.
pub fn amostras_devidas(real_ms: u64, virtual_ms: u64, taxa: u32) -> usize {
    let real = real_ms.min(150) * u64::from(taxa) / 1000;
    let virt = virtual_ms.min(1_000) * u64::from(taxa) / 1000;
    real.max(virt) as usize
}

pub struct Anel {
    interno: Mutex<VecDeque<i16>>,
    /// Amostras intercaladas, não quadros. Par, sempre.
    teto: usize,
}

impl Anel {
    pub fn novo(quadros: usize) -> Self {
        let quadros = quadros.max(1);
        Self {
            interno: Mutex::new(VecDeque::with_capacity(quadros * 2)),
            teto: quadros * 2,
        }
    }

    fn trava(&self) -> MutexGuard<'_, VecDeque<i16>> {
        self.interno.lock().unwrap_or_else(|erro| erro.into_inner())
    }

    /// Empurra PCM estéreo intercalado. O que não cabe fica de fora: o anel guarda o que ainda
    /// vai tocar, e crescer sem teto aqui é a trilha enchendo a RAM quando a thread atrasou.
    pub fn empurra(&self, pcm: &[i16]) -> usize {
        let mut fila = self.trava();
        let pares = pcm.len() / 2 * 2;
        let livres = self.teto.saturating_sub(fila.len());
        let n = pares.min(livres);
        fila.extend(pcm[..n].iter().copied());
        n / 2
    }

    /// Puxa para `destino` e completa com silêncio. O retorno é em quadros estéreo.
    pub fn puxa(&self, destino: &mut [i16]) -> usize {
        let mut fila = self.trava();
        let n = destino.len().min(fila.len());
        for (i, amostra) in fila.drain(..n).enumerate() {
            destino[i] = amostra;
        }
        destino[n..].fill(0);
        n / 2
    }

    pub fn esvazia(&self) {
        self.trava().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_relogio_de_parede_manda_quando_o_jogo_atrasa() {
        assert_eq!(amostras_devidas(10, 10, TAXA), 480);
        assert_eq!(amostras_devidas(1_000, 0, TAXA), 7_200);
        assert_eq!(amostras_devidas(0, 2_000, TAXA), 48_000);
        assert_eq!(amostras_devidas(0, 0, TAXA), 0);
    }

    #[test]
    fn o_anel_guarda_pares_e_completa_com_silencio() {
        let anel = Anel::novo(2);
        assert_eq!(anel.empurra(&[1]), 0, "amostra solta não é um quadro");
        assert_eq!(anel.empurra(&[1, 2, 3, 4, 5, 6]), 2);
        let mut destino = [9i16; 6];
        assert_eq!(anel.puxa(&mut destino), 2);
        assert_eq!(&destino[..4], &[1, 2, 3, 4]);
        assert_eq!(&destino[4..], &[0, 0]);
        anel.esvazia();
        assert_eq!(anel.puxa(&mut destino), 0);
    }

    #[test]
    fn o_grao_e_multiplo_do_minimo_da_porta() {
        assert_eq!(GRAO % 64, 0);
        assert!(GRAO >= 64);
        assert!(std::mem::align_of::<Bloco>() >= 64);
        assert_eq!(std::mem::size_of::<Bloco>(), GRAO * 2 * 2);
    }
}
