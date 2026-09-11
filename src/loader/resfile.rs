//! Arquivos de recurso do BREW — os `.bar`.
//!
//! É o **mesmo contêiner do `.mif`**: cabeçalho `0x0011`, tabela de seções com `n + 1` offsets,
//! e o conteúdo colado no fim. A diferença é o índice, que o `.mif` não usa: uma tabela de
//! entradas de 8 bytes que diz em que seção cada recurso mora.
//!
//! ```text
//! 0x06  u16 número de entradas do índice
//! 0x08  u32 offset do índice
//! 0x0c  u32 tamanho do índice (entradas * 8)
//! 0x10  u32 offset da tabela de seções
//! 0x14  u32 número de seções
//! ```
//!
//! Cada entrada é `u16 tipo, u16 id, u16 a, u16 b`, e cobre uma **faixa** de recursos: os ids
//! `id..=id + a` moram nas seções `b..=b + a`. A regra foi confirmada nos dois `.bar` reais que
//! temos: no `pacmania.bar` a última entrada dá exatamente as seções 126 a 142, e o arquivo tem
//! 143; no `resources.bar` do Peggle o começo de cada entrada é o fim da anterior mais um, do
//! primeiro ao último recurso.
//!
//! Os tipos são os do `ResType` de `AEEShell.h`: `RESTYPE_STRING = 1`,
//! `RESTYPE_IMAGE = 6` (dados com cabeçalho `AEEResBlob`) e `RESTYPE_BINARY = 0x5000`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAGIC: u16 = 0x0011;

/// `RESTYPE_STRING`, de `AEEShell.h`.
pub const RESTYPE_STRING: u16 = 1;

/// `RESTYPE_IMAGE`, de `AEEShell.h`.
pub const RESTYPE_IMAGE: u16 = 6;

/// O conteúdo de um recurso de imagem, sem o cabeçalho `AEEResBlob` que vem na frente.
///
/// O cabeçalho é um `uint16` com o deslocamento até os dados, seguido do tipo MIME terminado em
/// zero — é o que o `RESBLOB_DATA()` do SDK pula. No `tekken2.bar` a entrada 5034 começa com
/// `0c 00` e `"image/bmp\0"`, e o `BM` vem no byte 12.
///
/// Quem chama o `LoadResData` recebe o bloco inteiro, cabeçalho incluído, porque a documentação
/// diz que é isso que sai de lá. Quem pede um **objeto** de imagem, não: aí o cabeçalho é nosso
/// para interpretar.
pub fn blob_data(raw: &[u8]) -> Option<&[u8]> {
    let offset = u16::from_le_bytes([*raw.first()?, *raw.get(1)?]) as usize;
    // O byte anterior aos dados é o zero que termina o tipo MIME. Sem essa conferência, um
    // recurso que não seja um blob viraria um recorte arbitrário do próprio conteúdo.
    match raw.get(offset.checked_sub(1)?) == Some(&0) {
        true => raw.get(offset..),
        false => None,
    }
}

/// Uma faixa de recursos do índice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Range {
    kind: u16,
    first_id: u16,
    /// Quantos recursos a faixa cobre. É `a + 1` no arquivo.
    count: u32,
    first_section: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResFile {
    data: Vec<u8>,
    /// Limites de cada seção, como `(offset, tamanho)`.
    sections: Vec<(u32, u32)>,
    ranges: Vec<Range>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResError {
    BadMagic { found: u16 },
    Truncated,
}

impl std::fmt::Display for ResError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadMagic { found } => write!(f, "não parece um .bar (magic {found:#06x})"),
            Self::Truncated => write!(f, "arquivo de recursos truncado"),
        }
    }
}

impl ResFile {
    pub fn parse(data: Vec<u8>) -> Result<Self, ResError> {
        let magic = u16(&data, 0)?;
        if magic != MAGIC {
            return Err(ResError::BadMagic { found: magic });
        }
        let index_count = u16(&data, 0x06)? as usize;
        let index_offset = u32(&data, 0x08)? as usize;
        let table_offset = u32(&data, 0x10)? as usize;
        let section_count = u32(&data, 0x14)? as usize;

        // A tabela guarda n+1 offsets: o extra fecha a última seção.
        let mut bounds = Vec::with_capacity(section_count + 1);
        for i in 0..=section_count {
            bounds.push(u32(&data, table_offset + i * 4)?);
        }
        if bounds
            .last()
            .is_some_and(|&last| last as usize > data.len())
        {
            return Err(ResError::Truncated);
        }
        let sections: Vec<(u32, u32)> = bounds
            .windows(2)
            .map(|w| (w[0], w[1].saturating_sub(w[0])))
            .collect();

        let mut ranges = Vec::with_capacity(index_count);
        for i in 0..index_count {
            let at = index_offset + i * 8;
            ranges.push(Range {
                kind: u16(&data, at)?,
                first_id: u16(&data, at + 2)?,
                count: u32::from(u16(&data, at + 4)?) + 1,
                first_section: u16(&data, at + 6)?,
            });
        }
        Ok(Self {
            data,
            sections,
            ranges,
        })
    }

    /// O conteúdo bruto do recurso `(kind, id)`.
    pub fn get(&self, kind: u16, id: u16) -> Option<&[u8]> {
        let section = self.section_of(kind, id)?;
        let &(offset, len) = self.sections.get(section)?;
        self.data
            .get(offset as usize..(offset as usize).saturating_add(len as usize))
    }

    fn section_of(&self, kind: u16, id: u16) -> Option<usize> {
        self.ranges.iter().find_map(|range| {
            if range.kind != kind {
                return None;
            }
            let offset = u32::from(id).checked_sub(u32::from(range.first_id))?;
            match offset < range.count {
                true => Some(range.first_section as usize + offset as usize),
                false => None,
            }
        })
    }

    /// O texto de um recurso de string, em UTF-16 como o BREW entrega.
    ///
    /// A seção diz a própria codificação no começo. Duas aparecem nos arquivos que temos:
    ///
    /// - **UTF-16**, marcada por um BOM — `FF FE` para menor primeiro, `FE FF` para maior. É a
    ///   do `tectoy_pt.brf` da Z-Wheel, e o texto vem em unidades de dois bytes.
    /// - **ISO-Latin-1**, com um byte de codificação na frente e um ponto de código por byte.
    ///   É a dos `.bar` dos jogos.
    ///
    /// Ler tudo como Latin-1 fazia o BOM virar texto e o primeiro byte alto do primeiro
    /// caractere terminar a cadeia no zero seguinte: `"Meus Z-Credits"` saía como `"þM"`, e era
    /// isso que a Z-Wheel punha nos widgets do formulário do z-pad.
    ///
    /// O texto pode trazer vários itens separados por `^`, mas isso é empacotamento do próprio
    /// jogo: o que o BREW entrega é a seção inteira.
    pub fn string(&self, id: u16) -> Option<Vec<u16>> {
        let raw = self.get(RESTYPE_STRING, id)?;
        let largo = |pares: &[u8], grande: bool| {
            pares
                .chunks_exact(2)
                .map(|p| match grande {
                    true => u16::from_be_bytes([p[0], p[1]]),
                    false => u16::from_le_bytes([p[0], p[1]]),
                })
                .take_while(|&u| u != 0)
                .collect()
        };
        match raw {
            [0xff, 0xfe, resto @ ..] => Some(largo(resto, false)),
            [0xfe, 0xff, resto @ ..] => Some(largo(resto, true)),
            _ => {
                let (_encoding, text) = raw.split_first()?;
                Some(
                    text.iter()
                        .take_while(|&&b| b != 0)
                        .map(|&b| u16::from(b))
                        .collect(),
                )
            }
        }
    }
}

/// Guarda os `.bar` já abertos, para não reler um arquivo de treze megabytes a cada recurso.
#[derive(Debug, Default)]
pub struct ResCache {
    files: HashMap<PathBuf, Option<ResFile>>,
}

impl ResCache {
    /// O arquivo de recursos em `path`, lido na primeira vez que for pedido.
    ///
    /// O que não abre fica marcado como ausente, e não é tentado de novo: um jogo que pede o
    /// mesmo recurso a cada quadro não pode gerar uma leitura de disco por quadro.
    pub fn open(&mut self, path: &Path) -> Option<&ResFile> {
        self.files
            .entry(path.to_path_buf())
            .or_insert_with(|| {
                let bytes = std::fs::read(path).ok()?;
                ResFile::parse(bytes).ok()
            })
            .as_ref()
    }
}

fn u16(data: &[u8], at: usize) -> Result<u16, ResError> {
    let bytes = data.get(at..at + 2).ok_or(ResError::Truncated)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn u32(data: &[u8], at: usize) -> Result<u32, ResError> {
    let b = data.get(at..at + 4).ok_or(ResError::Truncated)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_cabecalho_do_blob_e_pulado_pelo_deslocamento() {
        // A entrada 5034 do `tekken2.bar`, do jeito que ela começa no arquivo.
        let mut raw = vec![0x0c, 0x00];
        raw.extend(b"image/bmp\0");
        raw.extend(b"BM\x38\xb4");
        assert_eq!(blob_data(&raw), Some(&b"BM\x38\xb4"[..]));
    }

    #[test]
    fn o_que_nao_e_blob_nao_vira_recorte_arbitrario() {
        // Um PNG solto: o deslocamento leria lixo, e o zero que termina o MIME não está lá.
        assert_eq!(blob_data(&[0x89, b'P', b'N', b'G', 1, 2, 3, 4]), None);
        assert_eq!(blob_data(&[]), None);
        assert_eq!(blob_data(&[0x0c, 0x00]), None);
    }

    /// Monta um `.bar` com as faixas e seções dadas.
    fn build(ranges: &[(u16, u16, u16, u16)], sections: &[&[u8]]) -> Vec<u8> {
        let index_offset = 0x20usize;
        let table_offset = index_offset + ranges.len() * 8;
        let first_section = table_offset + (sections.len() + 1) * 4;

        let mut header = vec![0u8; 0x20];
        header[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        header[2..4].copy_from_slice(&1u16.to_le_bytes());
        header[4..6].copy_from_slice(&1u16.to_le_bytes());
        header[6..8].copy_from_slice(&(ranges.len() as u16).to_le_bytes());
        header[8..12].copy_from_slice(&(index_offset as u32).to_le_bytes());
        header[12..16].copy_from_slice(&((ranges.len() * 8) as u32).to_le_bytes());
        header[16..20].copy_from_slice(&(table_offset as u32).to_le_bytes());
        header[20..24].copy_from_slice(&(sections.len() as u32).to_le_bytes());
        header[24..28].copy_from_slice(&(first_section as u32).to_le_bytes());

        let mut index = Vec::new();
        for (kind, id, a, b) in ranges {
            index.extend_from_slice(&kind.to_le_bytes());
            index.extend_from_slice(&id.to_le_bytes());
            index.extend_from_slice(&a.to_le_bytes());
            index.extend_from_slice(&b.to_le_bytes());
        }

        let mut table = Vec::new();
        let mut at = first_section as u32;
        for section in sections {
            table.extend_from_slice(&at.to_le_bytes());
            at += section.len() as u32;
        }
        table.extend_from_slice(&at.to_le_bytes());

        let mut out = header;
        out.extend_from_slice(&index);
        out.extend_from_slice(&table);
        for section in sections {
            out.extend_from_slice(section);
        }
        out
    }

    /// A seção 44 do `tectoy_pt.brf` da Z-Wheel, como ela está no arquivo: BOM e UTF-16LE.
    #[test]
    fn a_secao_com_bom_e_lida_como_utf16() {
        let mut secao = vec![0xff, 0xfe];
        for u in "Meus Z-Credits".encode_utf16() {
            secao.extend_from_slice(&u.to_le_bytes());
        }
        let bar = build(&[(RESTYPE_STRING, 1049, 0, 0)], &[&secao]);
        let res = ResFile::parse(bar).unwrap();
        let lido = String::from_utf16_lossy(&res.string(1049).unwrap());
        assert_eq!(lido, "Meus Z-Credits");
    }

    /// O outro sentido do BOM, que nenhum arquivo nosso usa e o formato permite.
    #[test]
    fn o_bom_ao_contrario_tambem_e_lido() {
        let mut secao = vec![0xfe, 0xff];
        for u in "Idioma".encode_utf16() {
            secao.extend_from_slice(&u.to_be_bytes());
        }
        let bar = build(&[(RESTYPE_STRING, 1045, 0, 0)], &[&secao]);
        let res = ResFile::parse(bar).unwrap();
        assert_eq!(
            String::from_utf16_lossy(&res.string(1045).unwrap()),
            "Idioma"
        );
    }

    /// Sem BOM continua sendo Latin-1 com um byte de codificação na frente, que é o que os
    /// `.bar` dos jogos trazem.
    #[test]
    fn sem_bom_continua_latin1() {
        let mut secao = vec![0x01];
        secao.extend_from_slice(
            "Ação\0"
                .as_bytes()
                .iter()
                .copied()
                .collect::<Vec<_>>()
                .as_slice(),
        );
        let bar = build(&[(RESTYPE_STRING, 7, 0, 0)], &[&secao]);
        let res = ResFile::parse(bar).unwrap();
        let lido = String::from_utf16_lossy(&res.string(7).unwrap());
        assert!(lido.starts_with('A'), "{lido:?}");
    }

    #[test]
    fn uma_faixa_cobre_ids_consecutivos_em_secoes_consecutivas() {
        // `a = 2` significa três recursos: os ids 100, 101 e 102 nas seções 0, 1 e 2.
        let bar = build(&[(6, 100, 2, 0)], &[b"zero", b"um", b"dois"]);
        let res = ResFile::parse(bar).unwrap();
        assert_eq!(res.get(6, 100), Some(&b"zero"[..]));
        assert_eq!(res.get(6, 101), Some(&b"um"[..]));
        assert_eq!(res.get(6, 102), Some(&b"dois"[..]));
        // Fora da faixa, nada.
        assert_eq!(res.get(6, 103), None);
        // E o tipo faz parte da chave.
        assert_eq!(res.get(1, 100), None);
    }

    #[test]
    fn faixas_de_tipos_diferentes_convivem() {
        let bar = build(
            &[(1, 4000, 0, 0), (0x5000, 3000, 1, 1)],
            &[b"\x03texto", b"bin0", b"bin1"],
        );
        let res = ResFile::parse(bar).unwrap();
        assert_eq!(res.get(0x5000, 3001), Some(&b"bin1"[..]));
        assert_eq!(res.string(4000), Some("texto".encode_utf16().collect()));
    }

    #[test]
    fn a_string_perde_o_byte_de_codificacao_e_para_no_terminador() {
        let bar = build(&[(1, 7, 0, 0)], &[b"\x03NEW GAME\0lixo depois"]);
        let res = ResFile::parse(bar).unwrap();
        assert_eq!(res.string(7), Some("NEW GAME".encode_utf16().collect()));
    }

    #[test]
    fn um_arquivo_que_nao_e_bar_e_recusado() {
        let err = ResFile::parse(b"nao sou um bar".to_vec());
        assert!(matches!(err, Err(ResError::BadMagic { .. })));
    }

    #[test]
    fn um_bar_truncado_nao_estoura() {
        let mut bar = build(&[(6, 1, 0, 0)], &[b"dados"]);
        bar.truncate(bar.len() - 3);
        // Ou recusa na abertura, ou devolve menos do que a tabela promete — o que não pode é
        // ler fora do vetor.
        if let Ok(res) = ResFile::parse(bar) {
            assert!(res.get(6, 1).is_none_or(|d| d.len() <= 5));
        }
    }
}
