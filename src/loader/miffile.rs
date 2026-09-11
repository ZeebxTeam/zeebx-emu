//! Parser do `.mif` — o Module Information File, que descreve o que um módulo BREW oferece.
//!
//! O formato não é público. O que está aqui foi levantado comparando 24 arquivos reais: os
//! `.mif` do BREW SDK 4.0.2 (cujos ClassIDs conhecemos pelos `.bid`), os módulos de sistema do
//! simulador, os homebrew do OpenZeebo e os títulos do console.
//!
//! Estrutura confirmada:
//!
//! ```text
//! 0x00  u16 magic = 0x0011
//! 0x02  u16 versão = 1
//! 0x04  u16 (sempre 1)
//! 0x06  u16 número de entradas do índice
//! 0x08  u32 offset do índice          (0x20 em todos os arquivos vistos)
//! 0x0c  u32 tamanho do índice         (entradas * 8)
//! 0x10  u32 offset da tabela de seções
//! 0x14  u32 número de seções
//! 0x18  u32 offset da primeira seção
//! 0x1c  u32 tamanho total das seções
//! ```
//!
//! A tabela de seções tem `n + 1` offsets: cada seção vai de `offs[i]` a `offs[i+1]`, e o
//! último valor é o tamanho do arquivo.
//!
//! O registro de applet tem **20 bytes** e começa com o `AEECLSID`. Em todos os `.mif` de
//! applet examinados existe exatamente uma seção desse tamanho. A regra foi validada contra
//! valores conhecidos por outra fonte: `mediaplayer.mif` devolve `0x01010EF6`, o mesmo ClassID
//! que a engenharia reversa da firmware do Zeebo registrou para o Media Player, e
//! `274755.mif` (o Z-Wheel) devolve `0x01070798`, o App ID que a firmware chama de "TECTOY".

use std::fmt;

const MAGIC: u16 = 0x0011;
/// Tamanho do registro que descreve um applet.
const APPLET_RECORD_LEN: u32 = 20;
/// Palavras do registro de applet que são sempre zero.
///
/// O registro é `ClassID`, um zero, um número, outro zero e um campo que varia. São esses dois
/// zeros que separam um registro de applet de outra seção que por acaso também tenha 20 bytes.
const APPLET_ZEROS: [usize; 2] = [4, 12];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MifFile {
    /// ClassIDs dos applets que o módulo expõe.
    pub applets: Vec<u32>,
    /// Limites de cada seção, como `(offset, tamanho)`.
    pub sections: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MifError {
    BadMagic { found: u16 },
    Truncated { needed: usize, len: usize },
}

impl fmt::Display for MifError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic { found } => write!(f, "não parece um .mif (magic {found:#06x})"),
            Self::Truncated { needed, len } => {
                write!(f, ".mif truncado: precisa de {needed} bytes, tem {len}")
            }
        }
    }
}

impl std::error::Error for MifError {}

impl MifFile {
    pub fn parse(data: &[u8]) -> Result<Self, MifError> {
        let magic = read_u16(data, 0)?;
        if magic != MAGIC {
            return Err(MifError::BadMagic { found: magic });
        }

        let table_offset = read_u32(data, 0x10)? as usize;
        let section_count = read_u32(data, 0x14)? as usize;

        // A tabela guarda n+1 offsets: o extra fecha a última seção.
        let mut bounds = Vec::with_capacity(section_count + 1);
        for i in 0..=section_count {
            bounds.push(read_u32(data, table_offset + i * 4)?);
        }

        // Em todos os arquivos reais examinados o último limite é exatamente o tamanho do
        // arquivo. Exigir isso evita que um `.mif` truncado passe adiante e simplesmente não
        // reporte applet nenhum.
        if let Some(&last) = bounds.last() {
            if last as usize > data.len() {
                return Err(MifError::Truncated {
                    needed: last as usize,
                    len: data.len(),
                });
            }
        }

        let sections: Vec<(u32, u32)> = bounds
            .windows(2)
            .map(|w| (w[0], w[1].saturating_sub(w[0])))
            .collect();

        let applets = sections
            .iter()
            .filter(|&&(_, len)| len == APPLET_RECORD_LEN)
            .filter(|&&(offset, _)| is_applet_record(data, offset as usize))
            .filter_map(|&(offset, _)| read_u32(data, offset as usize).ok())
            .filter(|&id| id != 0)
            .collect();

        Ok(Self { applets, sections })
    }

    /// As imagens do módulo — os ícones do título —, na ordem em que aparecem.
    ///
    /// Uma seção de imagem começa com `u16` de comprimento **do próprio cabeçalho**, seguido do
    /// tipo MIME terminado em zero; o arquivo vem logo depois. Os `.mif` reais gravam sempre 12
    /// e `image/png`, `image/bmp` ou `image/jpg`.
    pub fn images<'d>(&self, data: &'d [u8]) -> Vec<&'d [u8]> {
        self.sections
            .iter()
            .filter_map(|&(offset, len)| {
                let section = data.get(offset as usize..(offset + len) as usize)?;
                let header = u16::from_le_bytes([*section.first()?, *section.get(1)?]) as usize;
                let mime = section.get(2..header)?.split(|&b| b == 0).next()?;
                mime.starts_with(b"image/")
                    .then(|| section.get(header..))
                    .flatten()
            })
            .collect()
    }

    /// ClassID do applet principal — o que o emulador instancia.
    pub fn main_applet(&self) -> Option<u32> {
        self.applets.first().copied()
    }
}

/// Se a seção de 20 bytes em `offset` tem a forma de um registro de applet.
///
/// A regra era outra: exigir que o ClassID caísse na faixa `0x01xxxxxx` da Qualcomm. Ela
/// funcionava para 62 dos 67 registros dos jogos que temos, e deixava de fora o Zenonia
/// (`0xbf2e2021`) — que não abria por isso. A forma acerta os 67: pega os 62 de antes, mais o
/// Zenonia e um homebrew de ClassID `0x12345678`, e continua recusando as três seções de 20
/// bytes que não são applet nenhum (Prey Evil, Reckless Racing e Zeebo App, todas começando em
/// `0x00xxfeff`).
fn is_applet_record(data: &[u8], offset: usize) -> bool {
    APPLET_ZEROS
        .iter()
        .all(|&campo| read_u32(data, offset + campo) == Ok(0))
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, MifError> {
    let bytes = slice(data, offset, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, MifError> {
    let b = slice(data, offset, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn slice(data: &[u8], offset: usize, len: usize) -> Result<&[u8], MifError> {
    let end = offset.checked_add(len).ok_or(MifError::Truncated {
        needed: len,
        len: data.len(),
    })?;
    data.get(offset..end).ok_or(MifError::Truncated {
        needed: end,
        len: data.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_imagens_saem_das_secoes_que_anunciam_um_tipo_mime() {
        // O cabeçalho conta o próprio tamanho: ler o comprimento como se fosse só do texto
        // deixaria dois bytes do arquivo para trás e nenhum decodificador aceitaria a imagem.
        let mut section = Vec::new();
        section.extend_from_slice(&12u16.to_le_bytes());
        section.extend_from_slice(b"image/png\0");
        section.extend_from_slice(b"CONTEUDO");

        let mut outra = Vec::new();
        outra.extend_from_slice(&12u16.to_le_bytes());
        outra.extend_from_slice(b"text/plain");
        outra.extend_from_slice(b"nao e imagem");

        let mut data = vec![0u8; 4];
        let mut sections = Vec::new();
        for part in [&section, &outra] {
            sections.push((data.len() as u32, part.len() as u32));
            data.extend_from_slice(part);
        }
        let mif = MifFile {
            applets: Vec::new(),
            sections,
        };
        assert_eq!(mif.images(&data), vec![b"CONTEUDO".as_slice()]);
    }

    #[test]
    fn secao_truncada_nao_vira_imagem() {
        let mif = MifFile {
            applets: Vec::new(),
            sections: vec![(0, 40)],
        };
        assert!(mif.images(&[0u8; 4]).is_empty());
    }

    /// Monta um `.mif` mínimo com duas seções, sendo a segunda o registro de applet.
    fn mif_with_applet(clsid: u32) -> Vec<u8> {
        let table_offset = 0x20u32;
        let sections = [0x40u32, 0x50, 0x64];
        let mut data = vec![0u8; 0x64];
        data[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        data[2..4].copy_from_slice(&1u16.to_le_bytes());
        data[4..6].copy_from_slice(&1u16.to_le_bytes());
        data[6..8].copy_from_slice(&2u16.to_le_bytes());
        data[0x10..0x14].copy_from_slice(&table_offset.to_le_bytes());
        data[0x14..0x18].copy_from_slice(&2u32.to_le_bytes());
        for (i, offset) in sections.iter().enumerate() {
            let at = table_offset as usize + i * 4;
            data[at..at + 4].copy_from_slice(&offset.to_le_bytes());
        }
        // Seção 1: 0x40..0x50 (16 bytes, não é applet). Seção 2: 0x50..0x64 (20 bytes).
        data[0x50..0x54].copy_from_slice(&clsid.to_le_bytes());
        data
    }

    #[test]
    fn extrai_o_classid_do_applet() {
        let mif = MifFile::parse(&mif_with_applet(0x0100_9ff0)).unwrap();
        assert_eq!(mif.applets, vec![0x0100_9ff0]);
        assert_eq!(mif.main_applet(), Some(0x0100_9ff0));
    }

    /// O Zenonia declara o applet dele com ClassID `0xbf2e2021`, e um homebrew do conjunto usa
    /// `0x12345678`. A regra antiga exigia a faixa `0x01xxxxxx` da Qualcomm e recusava os dois:
    /// o Zenonia não abria por causa disso.
    #[test]
    fn aceita_classid_fora_da_faixa_da_qualcomm() {
        let mif = MifFile::parse(&mif_with_applet(0xbf2e_2021)).unwrap();
        assert_eq!(mif.main_applet(), Some(0xbf2e_2021));
    }

    /// Nem toda seção de 20 bytes é applet: o Prey Evil, o Reckless Racing e o Zeebo App têm
    /// uma que começa em `0x00xxfeff` e não é. O que as separa é a forma — o registro de applet
    /// tem zeros no segundo e no quarto campo.
    #[test]
    fn ignora_secao_de_20_bytes_sem_forma_de_applet() {
        let mut data = mif_with_applet(0x0050_feff);
        data[0x54..0x58].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        let mif = MifFile::parse(&data).unwrap();
        assert!(mif.applets.is_empty());
        assert_eq!(mif.main_applet(), None);
    }

    #[test]
    fn le_os_limites_das_secoes() {
        let mif = MifFile::parse(&mif_with_applet(0x0100_9ff0)).unwrap();
        assert_eq!(mif.sections, vec![(0x40, 0x10), (0x50, 20)]);
    }

    #[test]
    fn recusa_arquivo_com_magic_errado() {
        let mut data = mif_with_applet(0x0100_9ff0);
        data[0] = 0xff;
        assert!(matches!(
            MifFile::parse(&data),
            Err(MifError::BadMagic { .. })
        ));
    }

    #[test]
    fn recusa_arquivo_truncado() {
        let data = mif_with_applet(0x0100_9ff0);
        assert!(matches!(
            MifFile::parse(&data[..0x30]),
            Err(MifError::Truncated { .. })
        ));
    }
}
