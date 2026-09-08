//! Ponto flutuante da stdlib do BREW.
//!
//! Um módulo BREW é compilado sem unidade de ponto flutuante disponível para a biblioteca: em
//! vez de chamar `sin`, o código chama `GET_HELPER()->f_calc(x, FCALC_SIN)`. Toda a família
//! `f_*` funciona assim — um único ponto de entrada com um código de operação —, e os códigos
//! saem de `sdk/inc/AEEStdLib.h`, onde `FO_*`, `FCALC_*` e `FGET_*` dividem a mesma numeração.
//!
//! O `double` viaja na AAPCS em par de registradores (`r0:r1` e `r2:r3`), com a palavra baixa
//! primeiro; quem faz essa tradução é o chamador destas funções.

/// Operações de `f_op` e `f_cmp`, e códigos de `f_calc` e `f_get`.
pub const FO_ADD: u32 = 0;
pub const FO_SUB: u32 = 1;
pub const FO_MUL: u32 = 2;
pub const FO_DIV: u32 = 3;
pub const FO_CMP_L: u32 = 4;
pub const FO_CMP_LE: u32 = 5;
pub const FO_CMP_E: u32 = 6;
pub const FO_CMP_G: u32 = 7;
pub const FO_CMP_GE: u32 = 8;
pub const FO_POW: u32 = 9;
pub const FCALC_FLOOR: u32 = 10;
pub const FCALC_CEIL: u32 = 11;
pub const FCALC_SQRT: u32 = 12;
pub const FGET_HUGEVAL: u32 = 13;
pub const FGET_FLTMAX: u32 = 14;
pub const FGET_FLTMIN: u32 = 15;
pub const FCALC_SIN: u32 = 16;
pub const FCALC_COS: u32 = 17;
pub const FCALC_ABS: u32 = 18;
pub const FCALC_TAN: u32 = 19;

/// `double f_op(double v1, double v2, int nType)`.
pub fn op(v1: f64, v2: f64, kind: u32) -> Option<f64> {
    Some(match kind {
        FO_ADD => v1 + v2,
        FO_SUB => v1 - v2,
        FO_MUL => v1 * v2,
        FO_DIV => v1 / v2,
        FO_POW => v1.powf(v2),
        _ => return None,
    })
}

/// `boolean f_cmp(double v1, double v2, int nType)`.
pub fn cmp(v1: f64, v2: f64, kind: u32) -> Option<bool> {
    Some(match kind {
        FO_CMP_L => v1 < v2,
        FO_CMP_LE => v1 <= v2,
        FO_CMP_E => v1 == v2,
        FO_CMP_G => v1 > v2,
        FO_CMP_GE => v1 >= v2,
        _ => return None,
    })
}

/// `double f_calc(double x, int calcType)`.
pub fn calc(x: f64, kind: u32) -> Option<f64> {
    Some(match kind {
        FCALC_FLOOR => x.floor(),
        FCALC_CEIL => x.ceil(),
        FCALC_SQRT => x.sqrt(),
        FCALC_SIN => x.sin(),
        FCALC_COS => x.cos(),
        FCALC_ABS => x.abs(),
        FCALC_TAN => x.tan(),
        _ => return None,
    })
}

/// `double f_get(uint32 fgetType)` — as constantes de limite do `float.h`.
///
/// São os limites do `float` de 32 bits, não os do `double`: os nomes no header são
/// `FGETFLT_MAX`/`FGETFLT_MIN`, e é com eles que o jogo compara valores que vai guardar como
/// `float`.
pub fn get(kind: u32) -> Option<f64> {
    Some(match kind {
        FGET_HUGEVAL => f64::INFINITY,
        FGET_FLTMAX => f32::MAX as f64,
        FGET_FLTMIN => f32::MIN_POSITIVE as f64,
        _ => return None,
    })
}

/// Monta um `double` a partir do par de palavras da AAPCS (baixa primeiro).
pub fn from_words(low: u32, high: u32) -> f64 {
    f64::from_bits(((high as u64) << 32) | low as u64)
}

/// Desmonta um `double` no par de palavras da AAPCS: `(baixa, alta)`.
pub fn to_words(value: f64) -> (u32, u32) {
    let bits = value.to_bits();
    (bits as u32, (bits >> 32) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_pair_roundtrip() {
        // 180.0 é uma das constantes que o Quake carrega com `ldrd` para converter graus em
        // radianos — serve de âncora para a ordem das palavras.
        assert_eq!(to_words(180.0), (0x0000_0000, 0x4066_8000));
        assert_eq!(from_words(0x0000_0000, 0x4066_8000), 180.0);
        assert_eq!(from_words(0x5444_2d18, 0x4009_21fb), std::f64::consts::PI);
    }

    #[test]
    fn calc_covers_the_codes_the_games_use() {
        assert_eq!(calc(0.0, FCALC_SIN), Some(0.0));
        assert_eq!(calc(-2.5, FCALC_ABS), Some(2.5));
        assert_eq!(calc(-2.5, FCALC_FLOOR), Some(-3.0));
        assert_eq!(calc(-2.5, FCALC_CEIL), Some(-2.0));
        assert_eq!(calc(9.0, FCALC_SQRT), Some(3.0));
        assert!(calc(0.0, 99).is_none());
    }

    #[test]
    fn op_and_cmp_follow_the_shared_numbering() {
        assert_eq!(op(2.0, 3.0, FO_POW), Some(8.0));
        assert_eq!(op(1.0, 4.0, FO_DIV), Some(0.25));
        assert!(op(1.0, 1.0, FO_CMP_L).is_none());
        assert_eq!(cmp(1.0, 2.0, FO_CMP_L), Some(true));
        assert_eq!(cmp(2.0, 2.0, FO_CMP_GE), Some(true));
        assert!(cmp(1.0, 1.0, FO_ADD).is_none());
    }

    #[test]
    fn limits_are_the_float_ones() {
        assert_eq!(get(FGET_HUGEVAL), Some(f64::INFINITY));
        assert_eq!(get(FGET_FLTMAX), Some(f32::MAX as f64));
        assert!(get(FGET_FLTMIN).is_some_and(|v| v > 0.0));
    }
}
