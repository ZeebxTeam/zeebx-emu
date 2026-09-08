//! Decodificação das texturas comprimidas ATITC, do `GL_AMD_compressed_ATC_texture`.
//!
//! É o formato que o Adreno 130 do Zeebo aceita — o núcleo gráfico veio do ATI Imageon, e a
//! compressão veio junto. O Boomerang Sports Dodgeball carrega **todas** as texturas dele
//! assim, sem uma única chamada a `glTexImage2D`.
//!
//! Como o DXT1, os blocos são de 4×4 texels: duas cores e dois bits de índice por texel. A
//! diferença está no bit mais alto da primeira cor, que escolhe entre interpolar as quatro
//! cores da paleta ou reservar a primeira para o preto.

/// Lado do bloco, em texels.
const BLOCK: usize = 4;

/// Expande um valor de `bits` bits para a faixa completa de um byte.
fn expand(value: u16, bits: u32) -> u8 {
    let max = (1u16 << bits) - 1;
    ((value as u32 * 255 + max as u32 / 2) / max as u32) as u8
}

fn rgb555(value: u16) -> [u8; 3] {
    [
        expand((value >> 10) & 0x1f, 5),
        expand((value >> 5) & 0x1f, 5),
        expand(value & 0x1f, 5),
    ]
}

fn rgb565(value: u16) -> [u8; 3] {
    [
        expand(value >> 11, 5),
        expand((value >> 5) & 0x3f, 6),
        expand(value & 0x1f, 5),
    ]
}

/// A paleta de quatro cores de um bloco ATC.
fn palette(c0: u16, c1: u16) -> [[u8; 3]; 4] {
    let (a, b) = (rgb555(c0), rgb565(c1));
    // O bit 15 da primeira cor escolhe o modo. Com ele ligado a entrada 0 é preto, e é assim
    // que o formato representa recortes duros sem gastar um canal de alfa.
    if c0 & 0x8000 == 0 {
        let mix = |x: u8, y: u8, wx: u32, wy: u32| ((x as u32 * wx + y as u32 * wy) / 8) as u8;
        [
            a,
            std::array::from_fn(|i| mix(a[i], b[i], 5, 3)),
            std::array::from_fn(|i| mix(a[i], b[i], 3, 5)),
            b,
        ]
    } else {
        [
            [0, 0, 0],
            std::array::from_fn(|i| a[i].saturating_sub(b[i] / 4)),
            a,
            b,
        ]
    }
}

/// Lê o bloco de cor de 8 bytes na posição `at` e escreve os 16 texels em `out`, que é a
/// imagem inteira em RGBA de 8 bits.
fn decode_color_block(
    data: &[u8],
    at: usize,
    out: &mut [[u8; 4]],
    width: usize,
    height: usize,
    bx: usize,
    by: usize,
) {
    let c0 = u16::from_le_bytes([data[at], data[at + 1]]);
    let c1 = u16::from_le_bytes([data[at + 2], data[at + 3]]);
    let indices = u32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]);
    let colors = palette(c0, c1);
    for row in 0..BLOCK {
        for column in 0..BLOCK {
            let (x, y) = (bx * BLOCK + column, by * BLOCK + row);
            // A textura pode não ser múltipla de quatro; o bloco continua completo no arquivo,
            // e o que passa da borda é descartado.
            if x >= width || y >= height {
                continue;
            }
            let index = (indices >> ((row * BLOCK + column) * 2)) & 0b11;
            let rgb = colors[index as usize];
            out[y * width + x] = [rgb[0], rgb[1], rgb[2], 255];
        }
    }
}

/// Decodifica uma imagem ATC para RGBA de 8 bits.
///
/// `explicit_alpha` distingue os dois formatos que o console usa: sem ele o bloco tem 8 bytes e
/// é só cor; com ele o bloco tem 16, e os oito primeiros trazem um alfa de quatro bits por
/// texel, sem compressão nenhuma.
pub fn decode(data: &[u8], width: usize, height: usize, explicit_alpha: bool) -> Vec<[u8; 4]> {
    let mut out = vec![[0u8, 0, 0, 255]; width * height];
    let stride = if explicit_alpha { 16 } else { 8 };
    let (columns, rows) = (width.div_ceil(BLOCK), height.div_ceil(BLOCK));
    for by in 0..rows {
        for bx in 0..columns {
            let at = (by * columns + bx) * stride;
            if at + stride > data.len() {
                return out;
            }
            let color_at = if explicit_alpha { at + 8 } else { at };
            decode_color_block(data, color_at, &mut out, width, height, bx, by);
            if !explicit_alpha {
                continue;
            }
            for row in 0..BLOCK {
                let line = u16::from_le_bytes([data[at + row * 2], data[at + row * 2 + 1]]);
                for column in 0..BLOCK {
                    let (x, y) = (bx * BLOCK + column, by * BLOCK + row);
                    if x >= width || y >= height {
                        continue;
                    }
                    out[y * width + x][3] = expand((line >> (column * 4)) & 0xf, 4);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta um bloco ATC de cor com as duas cores e os índices dados.
    fn block(c0: u16, c1: u16, indices: u32) -> Vec<u8> {
        let mut bytes = c0.to_le_bytes().to_vec();
        bytes.extend_from_slice(&c1.to_le_bytes());
        bytes.extend_from_slice(&indices.to_le_bytes());
        bytes
    }

    #[test]
    fn o_modo_interpolado_usa_as_quatro_cores_da_paleta() {
        // Branco e preto nas pontas, com um texel de cada índice na primeira linha.
        let data = block(0x7fff, 0x0000, 0b11_10_01_00);
        let pixels = decode(&data, 4, 4, false);
        assert_eq!(pixels[0], [255, 255, 255, 255]);
        assert_eq!(pixels[3], [0, 0, 0, 255]);
        // As do meio ficam entre as duas, e a de índice 1 é a mais clara das duas.
        assert!(pixels[1][0] > pixels[2][0]);
        assert!(pixels[2][0] > pixels[3][0]);
    }

    #[test]
    fn o_bit_de_modo_reserva_o_indice_zero_para_o_preto() {
        // Mesmo par de cores, agora com o bit 15 ligado: o índice 0 vira preto e o 2 passa a
        // ser a cor cheia. Confundir os dois modos deixa a textura escura ou lavada.
        let data = block(0xffff, 0x0000, 0b00_00_10_00);
        let pixels = decode(&data, 4, 4, false);
        assert_eq!(pixels[0], [0, 0, 0, 255]);
        assert_eq!(pixels[1], [255, 255, 255, 255]);
    }

    #[test]
    fn o_alfa_explicito_vem_em_quatro_bits_por_texel() {
        let mut data: Vec<u8> = Vec::new();
        // Primeira linha: alfa 0, 5, 10, 15.
        data.extend_from_slice(&0xfa50u16.to_le_bytes());
        data.extend_from_slice(&[0u8; 6]);
        data.extend_from_slice(&block(0x7fff, 0x7fff, 0));
        let pixels = decode(&data, 4, 4, true);
        assert_eq!(pixels[0][3], 0);
        assert_eq!(pixels[3][3], 255);
        assert!(pixels[1][3] < pixels[2][3]);
    }

    #[test]
    fn texturas_fora_do_multiplo_de_quatro_nao_estouram() {
        // 6×3 ocupa dois blocos de largura e um de altura; o que passa da borda é descartado.
        let mut data = block(0x7fff, 0x7fff, 0);
        data.extend_from_slice(&block(0x7fff, 0x7fff, 0));
        let pixels = decode(&data, 6, 3, false);
        assert_eq!(pixels.len(), 18);
        assert!(pixels.iter().all(|p| *p == [255, 255, 255, 255]));
    }

    #[test]
    fn dados_truncados_devolvem_o_que_deu_para_ler() {
        let pixels = decode(&[0u8; 4], 4, 4, false);
        assert_eq!(pixels.len(), 16);
    }
}
