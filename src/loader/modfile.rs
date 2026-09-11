//! Parser do formato `.mod` — o módulo executável do BREW.
//!
//! Um `.mod` é gerado pelo `elf2mod` da Qualcomm a partir de um ELF ARM linkado com
//! `elf2mod.x`, que fixa `ro-base = 0x0` e força `.text.AEEMod_Load` no início da `.text`.
//! Como a imagem é linkada na base zero, carregá-la no endereço 0 do guest dispensa
//! qualquer relocação — é o que fazemos hoje.
//!
//! Duas variantes aparecem nos arquivos reais:
//!
//! - `Raw`: o arquivo começa direto no código ARM (`AEEMod_Load`). É o que os exemplos do
//!   BREW SDK 4.0.2 usam.
//! - `BrewHeader`: os títulos do Zeebo trazem dois branches ARM, a assinatura `BREW` e um
//!   cabeçalho de 0x40 bytes antes do código.
//!
//! O significado da maioria dos campos do cabeçalho `BREW` ainda não foi confirmado — eles
//! são expostos em [`BrewHeader::unknown`] em vez de receberem nomes inventados.

use std::fmt;

/// Assinatura que identifica a variante com cabeçalho.
const MAGIC: &[u8; 4] = b"BREW";
/// Deslocamento da assinatura dentro do arquivo.
const MAGIC_OFFSET: usize = 0x08;
/// Tamanho do cabeçalho `BREW`, em bytes. O código começa logo depois.
const HEADER_LEN: usize = 0x40;

/// Codificação de uma instrução `B <label>` do ARM (condição `AL`, sem link).
const ARM_BRANCH_MASK: u32 = 0xff00_0000;
const ARM_BRANCH_AL: u32 = 0xea00_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// Arquivo começa direto no código ARM.
    Raw,
    /// Arquivo traz o cabeçalho `BREW` de 0x40 bytes.
    BrewHeader,
}

/// Cabeçalho da variante `BREW`, lido em bruto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrewHeader {
    /// Campo em 0x0C. Vale 1 em todos os módulos observados.
    pub version: u32,
    /// Campo em 0x10. Vale 0x40 em todos os módulos observados — o início do código.
    pub code_offset: u32,
    /// Campos de 0x14 a 0x3C, ainda não decifrados.
    ///
    /// Valores observados (bjt.mod / tectoy.mod):
    /// `0x101 0x200 {0x7a120,0x8fdf0} {0x70,0x20} {0x234,0xa0} {0x690,0x40} 0x90 0x0
    ///  {0x7a120,0x8fdf0} {0x8000,0x0}`.
    pub unknown: [u32; 10],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Arquivo pequeno demais para conter sequer um cabeçalho.
    TooSmall { len: usize },
    /// O arquivo tem a assinatura `BREW` mas não cabe o cabeçalho inteiro.
    TruncatedHeader { len: usize },
    /// A primeira instrução não é um branch ARM nem um prólogo plausível.
    NotArmCode { first_word: u32 },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall { len } => {
                write!(f, "arquivo pequeno demais para um .mod ({len} bytes)")
            }
            Self::TruncatedHeader { len } => {
                write!(f, "cabeçalho BREW truncado (arquivo tem {len} bytes)")
            }
            Self::NotArmCode { first_word } => {
                write!(
                    f,
                    "primeira palavra {first_word:#010x} não parece código ARM"
                )
            }
        }
    }
}

impl std::error::Error for ParseError {}

/// Um `.mod` carregado na memória do host, pronto para ser mapeado no guest.
#[derive(Debug, Clone)]
pub struct ModImage {
    data: Vec<u8>,
    variant: Variant,
    header: Option<BrewHeader>,
    entry: u32,
}

impl ModImage {
    /// Interpreta os bytes de um arquivo `.mod`.
    pub fn parse(data: Vec<u8>) -> Result<Self, ParseError> {
        if data.len() < 8 {
            return Err(ParseError::TooSmall { len: data.len() });
        }

        let has_magic = data.len() >= MAGIC_OFFSET + 4
            && &data[MAGIC_OFFSET..MAGIC_OFFSET + 4] == MAGIC.as_slice();

        if has_magic {
            if data.len() < HEADER_LEN {
                return Err(ParseError::TruncatedHeader { len: data.len() });
            }
            let version = read_u32(&data, 0x0c);
            let code_offset = read_u32(&data, 0x10);
            let mut unknown = [0u32; 10];
            for (i, slot) in unknown.iter_mut().enumerate() {
                *slot = read_u32(&data, 0x14 + i * 4);
            }
            // O branch em 0x00 aponta para o ponto de entrada real; `code_offset` é a
            // segunda opinião sobre o mesmo endereço.
            let entry = branch_target(read_u32(&data, 0), 0).unwrap_or(code_offset);
            return Ok(Self {
                data,
                variant: Variant::BrewHeader,
                header: Some(BrewHeader {
                    version,
                    code_offset,
                    unknown,
                }),
                entry,
            });
        }

        // Sem assinatura: a imagem começa direto em AEEMod_Load. Exigimos que a primeira
        // palavra ao menos tenha condição válida (nibble alto != 0xF) para não aceitar
        // qualquer arquivo como módulo.
        let first_word = read_u32(&data, 0);
        if first_word >> 28 == 0xf {
            return Err(ParseError::NotArmCode { first_word });
        }
        Ok(Self {
            data,
            variant: Variant::Raw,
            header: None,
            entry: 0,
        })
    }

    /// Bytes da imagem, na ordem em que devem ser mapeados no guest.
    // Passa a ser usado quando o loader mapear o módulo na memória do guest.
    #[allow(dead_code)]
    pub fn image(&self) -> &[u8] {
        &self.data
    }

    pub fn variant(&self) -> Variant {
        self.variant
    }

    pub fn header(&self) -> Option<&BrewHeader> {
        self.header.as_ref()
    }

    /// Deslocamento de `AEEMod_Load` dentro da imagem.
    ///
    /// Como a imagem é linkada em ro-base 0, esse valor também é o endereço absoluto do
    /// ponto de entrada quando o módulo é carregado no endereço 0 do guest.
    pub fn entry(&self) -> u32 {
        self.entry
    }
}

/// Lê um `u32` little-endian. O chamador garante que `offset + 4 <= data.len()`.
fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

/// Resolve o destino de um `B <label>` ARM situado em `pc`.
///
/// O offset é de 24 bits com sinal, em palavras, relativo a `pc + 8` por causa do pipeline.
fn branch_target(word: u32, pc: u32) -> Option<u32> {
    if word & ARM_BRANCH_MASK != ARM_BRANCH_AL {
        return None;
    }
    let imm24 = word & 0x00ff_ffff;
    // Estende o sinal de 24 para 32 bits e converte de palavras para bytes.
    let offset = ((imm24 << 8) as i32 >> 8) * 4;
    Some(pc.wrapping_add(8).wrapping_add(offset as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta um `.mod` sintético com cabeçalho BREW e código a partir de 0x40.
    fn brew_mod() -> Vec<u8> {
        let mut data = vec![0u8; 0x50];
        // b 0x40  ->  offset em palavras = (0x40 - 0 - 8) / 4 = 0x0e
        data[0x00..0x04].copy_from_slice(&0xea00_000eu32.to_le_bytes());
        // b 0x48
        data[0x04..0x08].copy_from_slice(&0xea00_000fu32.to_le_bytes());
        data[0x08..0x0c].copy_from_slice(MAGIC);
        data[0x0c..0x10].copy_from_slice(&1u32.to_le_bytes());
        data[0x10..0x14].copy_from_slice(&0x40u32.to_le_bytes());
        data[0x14..0x18].copy_from_slice(&0x101u32.to_le_bytes());
        data
    }

    #[test]
    fn le_cabecalho_brew_e_resolve_o_entry_pelo_branch() {
        let image = ModImage::parse(brew_mod()).unwrap();
        assert_eq!(image.variant(), Variant::BrewHeader);
        assert_eq!(image.entry(), 0x40);
        let header = image.header().unwrap();
        assert_eq!(header.version, 1);
        assert_eq!(header.code_offset, 0x40);
        assert_eq!(header.unknown[0], 0x101);
    }

    #[test]
    fn modulo_sem_assinatura_entra_em_zero() {
        // push {lr} — primeira instrução dos exemplos do BREW SDK.
        let data = 0xe52d_e004u32.to_le_bytes().repeat(4);
        let image = ModImage::parse(data).unwrap();
        assert_eq!(image.variant(), Variant::Raw);
        assert_eq!(image.entry(), 0);
    }

    #[test]
    fn recusa_arquivo_que_nao_comeca_com_instrucao_valida() {
        let data = 0xf000_0000u32.to_le_bytes().repeat(4);
        assert!(matches!(
            ModImage::parse(data),
            Err(ParseError::NotArmCode { .. })
        ));
    }

    #[test]
    fn recusa_arquivo_curto_demais() {
        assert!(matches!(
            ModImage::parse(vec![0, 1, 2]),
            Err(ParseError::TooSmall { len: 3 })
        ));
    }

    #[test]
    fn branch_negativo_volta_para_tras() {
        // b -8 : offset em palavras = -4  ->  0xea_fffffc, a partir de pc = 0x20
        assert_eq!(branch_target(0xeaff_fffc, 0x20), Some(0x18));
    }
}
