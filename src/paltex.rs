//! Texturas paletizadas do `OES_compressed_paletted_texture`.
//!
//! São parte do núcleo do OpenGL ES 1.1 e entram pelo mesmo `glCompressedTexImage2D` das
//! texturas do Adreno. O bloco começa com a paleta — 16 cores nos formatos `PALETTE4` e 256 nos
//! `PALETTE8` — e segue com os índices, um por texel, empacotados de dois em dois byte nos
//! `PALETTE4`.
//!
//! O `level` desses formatos é **não positivo**: zero quer dizer só o nível base, e um valor
//! negativo diz quantos níveis de mipmap vêm depois dele. Como só usamos o nível base, o que
//! interessa é sempre o primeiro trecho de índices depois da paleta.

use crate::gles;

/// Como cada entrada da paleta é codificada, e quantas entradas ela tem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    /// Bits por índice: 4 ou 8.
    index_bits: u32,
    /// Bytes por entrada da paleta.
    entry_bytes: usize,
    kind: Entry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Rgb8,
    Rgba8,
    Rgb565,
    Rgba4,
    Rgb5A1,
}

impl Format {
    /// O formato de um `internalformat` do OpenGL, ou `None` se não for paletizado.
    pub fn from_gl(format: u32) -> Option<Self> {
        let (index_bits, kind) = match format {
            gles::GL_PALETTE4_RGB8_OES => (4, Entry::Rgb8),
            gles::GL_PALETTE4_RGBA8_OES => (4, Entry::Rgba8),
            gles::GL_PALETTE4_R5_G6_B5_OES => (4, Entry::Rgb565),
            gles::GL_PALETTE4_RGBA4_OES => (4, Entry::Rgba4),
            gles::GL_PALETTE4_RGB5_A1_OES => (4, Entry::Rgb5A1),
            gles::GL_PALETTE8_RGB8_OES => (8, Entry::Rgb8),
            gles::GL_PALETTE8_RGBA8_OES => (8, Entry::Rgba8),
            gles::GL_PALETTE8_R5_G6_B5_OES => (8, Entry::Rgb565),
            gles::GL_PALETTE8_RGBA4_OES => (8, Entry::Rgba4),
            gles::GL_PALETTE8_RGB5_A1_OES => (8, Entry::Rgb5A1),
            _ => return None,
        };
        let entry_bytes = match kind {
            Entry::Rgb8 => 3,
            Entry::Rgba8 => 4,
            Entry::Rgb565 | Entry::Rgba4 | Entry::Rgb5A1 => 2,
        };
        Some(Self {
            index_bits,
            entry_bytes,
            kind,
        })
    }

    fn entries(self) -> usize {
        1 << self.index_bits
    }
}

/// Expande um valor de `bits` bits para a faixa completa de um byte.
fn expand(value: u16, bits: u32) -> u8 {
    let max = (1u16 << bits) - 1;
    ((value as u32 * 255 + max as u32 / 2) / max as u32) as u8
}

fn decode_entry(kind: Entry, bytes: &[u8]) -> [u8; 4] {
    match kind {
        Entry::Rgb8 => [bytes[0], bytes[1], bytes[2], 255],
        Entry::Rgba8 => [bytes[0], bytes[1], bytes[2], bytes[3]],
        // Os formatos de 16 bits são `GLushort`, e valem na ordem de bytes do aparelho —
        // little-endian no ARM. Lidos ao contrário, o logo do Double Dragon sai com um
        // serrilhado de arco-íris no lugar do dourado; foi assim que a ordem se decidiu.
        Entry::Rgb565 => {
            let v = u16::from_le_bytes([bytes[0], bytes[1]]);
            [
                expand(v >> 11, 5),
                expand((v >> 5) & 0x3f, 6),
                expand(v & 0x1f, 5),
                255,
            ]
        }
        Entry::Rgba4 => {
            let v = u16::from_le_bytes([bytes[0], bytes[1]]);
            [
                expand((v >> 12) & 0xf, 4),
                expand((v >> 8) & 0xf, 4),
                expand((v >> 4) & 0xf, 4),
                expand(v & 0xf, 4),
            ]
        }
        Entry::Rgb5A1 => {
            let v = u16::from_le_bytes([bytes[0], bytes[1]]);
            [
                expand((v >> 11) & 0x1f, 5),
                expand((v >> 6) & 0x1f, 5),
                expand((v >> 1) & 0x1f, 5),
                match v & 1 {
                    0 => 0,
                    _ => 255,
                },
            ]
        }
    }
}

/// Decodifica o nível base de uma textura paletizada para RGBA de 8 bits.
///
/// Devolve `None` quando os dados não chegam nem para a paleta. Índices que passem do fim do
/// bloco viram transparente, porque uma textura truncada tem que sair incompleta, não derrubar
/// o emulador.
pub fn decode(data: &[u8], width: usize, height: usize, format: Format) -> Option<Vec<[u8; 4]>> {
    let palette_bytes = format.entries() * format.entry_bytes;
    if data.len() < palette_bytes {
        return None;
    }
    let palette: Vec<[u8; 4]> = data[..palette_bytes]
        .chunks_exact(format.entry_bytes)
        .map(|entry| decode_entry(format.kind, entry))
        .collect();

    let indices = &data[palette_bytes..];
    let mut out = vec![[0u8, 0, 0, 0]; width * height];
    for (texel, slot) in out.iter_mut().enumerate() {
        let index = match format.index_bits {
            4 => {
                // Dois texels por byte, o de índice par no nibble alto.
                let Some(&byte) = indices.get(texel / 2) else {
                    continue;
                };
                match texel % 2 {
                    0 => byte >> 4,
                    _ => byte & 0xf,
                }
            }
            _ => match indices.get(texel) {
                Some(&byte) => byte,
                None => continue,
            },
        };
        if let Some(&color) = palette.get(index as usize) {
            *slot = color;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_palette8_rgb5_a1_le_a_paleta_e_os_indices() {
        let format = Format::from_gl(gles::GL_PALETTE8_RGB5_A1_OES).unwrap();
        // Paleta de 256 entradas de dois bytes: a 0 é preta e opaca, a 1 é vermelha e opaca.
        let mut data = vec![0u8; 256 * 2];
        data[0..2].copy_from_slice(&0x0001u16.to_le_bytes());
        data[2..4].copy_from_slice(&0xf801u16.to_le_bytes());
        // Quatro texels: 0, 1, 1, 0.
        data.extend_from_slice(&[0, 1, 1, 0]);

        let pixels = decode(&data, 2, 2, format).unwrap();
        assert_eq!(pixels[0], [0, 0, 0, 255]);
        assert_eq!(pixels[1], [255, 0, 0, 255]);
        assert_eq!(pixels[3], [0, 0, 0, 255]);
    }

    #[test]
    fn o_palette4_empacota_dois_texels_por_byte() {
        let format = Format::from_gl(gles::GL_PALETTE4_RGB5_A1_OES).unwrap();
        let mut data = vec![0u8; 16 * 2];
        data[0..2].copy_from_slice(&0x0001u16.to_le_bytes()); // preto
        data[2..4].copy_from_slice(&0xf801u16.to_le_bytes()); // vermelho
        // Um byte, dois texels: o alto é 0, o baixo é 1.
        data.push(0x01);

        let pixels = decode(&data, 2, 1, format).unwrap();
        assert_eq!(pixels[0], [0, 0, 0, 255]);
        assert_eq!(pixels[1], [255, 0, 0, 255]);
    }

    #[test]
    fn o_bit_de_alfa_do_rgb5_a1_e_tudo_ou_nada() {
        let format = Format::from_gl(gles::GL_PALETTE8_RGB5_A1_OES).unwrap();
        let mut data = vec![0u8; 256 * 2];
        // Mesma cor, uma com o bit de alfa ligado e outra sem.
        data[0..2].copy_from_slice(&0xf800u16.to_le_bytes());
        data[2..4].copy_from_slice(&0xf801u16.to_le_bytes());
        data.extend_from_slice(&[0, 1]);
        let pixels = decode(&data, 2, 1, format).unwrap();
        assert_eq!(pixels[0][3], 0);
        assert_eq!(pixels[1][3], 255);
    }

    #[test]
    fn dados_que_nao_cobrem_a_paleta_sao_recusados() {
        let format = Format::from_gl(gles::GL_PALETTE8_RGB8_OES).unwrap();
        assert_eq!(decode(&[0u8; 10], 4, 4, format), None);
    }

    #[test]
    fn indices_que_faltam_saem_transparentes() {
        // A paleta está inteira, mas os índices acabam antes: o resto fica transparente em vez
        // de derrubar o emulador.
        let format = Format::from_gl(gles::GL_PALETTE8_RGB8_OES).unwrap();
        let mut data = vec![0u8; 256 * 3];
        data[0..3].copy_from_slice(&[10, 20, 30]);
        data.push(0);
        let pixels = decode(&data, 2, 1, format).unwrap();
        assert_eq!(pixels[0], [10, 20, 30, 255]);
        assert_eq!(pixels[1], [0, 0, 0, 0]);
    }

    #[test]
    fn formato_que_nao_e_paletizado_nao_e_reconhecido() {
        assert_eq!(Format::from_gl(gles::GL_ATC_RGB_AMD), None);
    }
}
