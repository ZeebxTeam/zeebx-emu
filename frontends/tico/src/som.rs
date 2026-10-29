//! Quantos quadros estéreo entregar neste passo.
//!
//! O alto-falante do Switch consome 48 kHz no relógio de parede. Entregar só o relógio do jogo
//! deixa um buraco quando o quadro demora, e a música passa a andar no ritmo da emulação. O
//! teto de 150 ms é o do core na Vita: cabe num anel curto, e um pico maior seria descartado
//! na ponta, no hospedeiro.

/// A porta do `audout` só aceita 48 kHz. O misturador entrega nesta taxa.
pub const TAXA: u32 = 48_000;

pub fn amostras_devidas(real_ms: u64, virtual_ms: u64, taxa: u32) -> usize {
    let real = real_ms.min(150) * u64::from(taxa) / 1000;
    let virt = virtual_ms.min(1_000) * u64::from(taxa) / 1000;
    real.max(virt) as usize
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
}
