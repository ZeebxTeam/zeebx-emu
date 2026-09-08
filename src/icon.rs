//! As imagens que vêm dentro dos jogos.
//!
//! Um `.mif` guarda o ícone do título já pronto para exibição, e cada console gravou o dele no
//! formato que quis: os jogos mais novos usam PNG, os da primeira leva usam BMP paletado, e o
//! Z-Wheel usa JPEG. Como os três aparecem nas ROMs reais, os três são lidos aqui.
//!
//! O BMP é decodificado à mão porque o que os `.mif` trazem é o subconjunto mais simples do
//! formato — sem compressão, paleta de 16 ou 256 cores — e uma dependência inteira para isso
//! custaria mais que as poucas dezenas de linhas abaixo.

use std::fmt;

/// Teto de área para uma imagem vinda de um jogo. Os ícones reais não passam de 65×42; o
/// limite existe para um arquivo corrompido não pedir gigabytes ao anunciar um tamanho absurdo.
const MAX_PIXELS: usize = 4096 * 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// RGBA não-premultiplicado, uma linha por vez, de cima para baixo.
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn pixels(&self) -> usize {
        self.width * self.height
    }

    /// Reduz a imagem até nenhum lado passar de `max_side`, por média de blocos.
    ///
    /// A média é feita com a cor **premultiplicada** pelo alfa e desfeita no fim. Sem isso, um
    /// pixel transparente entra na conta com a cor que ele guarda — que costuma ser preta — e a
    /// borda de um logo com fundo transparente fica com um halo escuro.
    ///
    /// O fator é inteiro: reduzir por 5 é somar blocos de 5×5, sem interpolação nem reamostragem
    /// fracionária. Para um ícone é o suficiente, e evita um redimensionador inteiro.
    pub fn downscaled(&self, max_side: usize) -> Image {
        let longest = self.width.max(self.height);
        if longest <= max_side || max_side == 0 {
            return self.clone();
        }
        let factor = longest.div_ceil(max_side);
        let (width, height) = (self.width.div_ceil(factor), self.height.div_ceil(factor));
        let mut rgba = Vec::with_capacity(width * height * 4);
        for block_y in 0..height {
            for block_x in 0..width {
                let (mut sum, mut alpha, mut count) = ([0u32; 3], 0u32, 0u32);
                for y in block_y * factor..((block_y + 1) * factor).min(self.height) {
                    for x in block_x * factor..((block_x + 1) * factor).min(self.width) {
                        let at = (y * self.width + x) * 4;
                        let a = self.rgba[at + 3] as u32;
                        for (channel, total) in sum.iter_mut().enumerate() {
                            *total += self.rgba[at + channel] as u32 * a;
                        }
                        alpha += a;
                        count += 1;
                    }
                }
                let count = count.max(1);
                for channel in sum {
                    rgba.push(match alpha {
                        0 => 0,
                        _ => (channel / alpha) as u8,
                    });
                }
                rgba.push((alpha / count) as u8);
            }
        }
        Image {
            width,
            height,
            rgba,
        }
    }
}

#[derive(Debug)]
pub enum ImageError {
    /// O formato não é nenhum dos que sabemos ler.
    Unknown,
    /// O arquivo diz uma coisa e entrega outra.
    Malformed(&'static str),
    Png(png::DecodingError),
    Jpeg(String),
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => write!(f, "formato de imagem desconhecido"),
            Self::Malformed(what) => write!(f, "imagem inconsistente: {what}"),
            Self::Png(err) => write!(f, "png ilegível: {err}"),
            Self::Jpeg(err) => write!(f, "jpeg ilegível: {err}"),
        }
    }
}

impl std::error::Error for ImageError {}

/// Decodifica pelo que o arquivo é, não pelo que ele diz ser.
///
/// A assinatura é mais confiável que o tipo anunciado no `.mif`: um deles chama o JPEG de
/// `image/jpg`, e seguir o rótulo à risca deixaria o ícone de fora por causa de uma letra.
pub fn decode(data: &[u8]) -> Result<Image, ImageError> {
    match data {
        [0x89, b'P', b'N', b'G', ..] => decode_png(data),
        [b'B', b'M', ..] => decode_bmp(data),
        [0xff, 0xd8, 0xff, ..] => decode_jpeg(data),
        _ => Err(ImageError::Unknown),
    }
}

pub fn decode_png(data: &[u8]) -> Result<Image, ImageError> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    // Os ícones dos jogos são quase todos paletados; sem esta expansão viria o índice da cor,
    // e não a cor.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(ImageError::Png)?;
    let size = reader
        .output_buffer_size()
        .ok_or(ImageError::Malformed("tamanho impossível"))?;
    let mut buffer = vec![0; size];
    let info = reader.next_frame(&mut buffer).map_err(ImageError::Png)?;
    let channels = info.color_type.samples();
    let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    for pixel in buffer[..info.buffer_size()].chunks_exact(channels) {
        let (rgb, alpha) = match channels {
            1 => ([pixel[0]; 3], 255),
            2 => ([pixel[0]; 3], pixel[1]),
            3 => ([pixel[0], pixel[1], pixel[2]], 255),
            _ => ([pixel[0], pixel[1], pixel[2]], pixel[3]),
        };
        rgba.extend_from_slice(&rgb);
        rgba.push(alpha);
    }
    Ok(Image {
        width: info.width as usize,
        height: info.height as usize,
        rgba,
    })
}

fn decode_jpeg(data: &[u8]) -> Result<Image, ImageError> {
    let mut decoder = zune_jpeg::JpegDecoder::new(std::io::Cursor::new(data));
    decoder
        .decode_headers()
        .map_err(|err| ImageError::Jpeg(err.to_string()))?;
    let info = decoder
        .info()
        .ok_or_else(|| ImageError::Jpeg("sem cabeçalho".into()))?;
    let pixels = decoder
        .decode()
        .map_err(|err| ImageError::Jpeg(err.to_string()))?;
    let (width, height) = (info.width as usize, info.height as usize);
    let channels = match width * height {
        0 => return Err(ImageError::Malformed("jpeg vazio")),
        area => pixels.len() / area,
    };
    let mut rgba = Vec::with_capacity(width * height * 4);
    for pixel in pixels.chunks_exact(channels.max(1)) {
        match channels {
            1 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
            _ => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
        }
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

/// Decodifica o BMP sem compressão que os `.mif` guardam.
///
/// Cobre 1, 4, 8, 24 e 32 bits por pixel. Formatos comprimidos ficam de fora de propósito: as
/// ROMs não os usam, e adivinhar o que não se pode testar não ajuda ninguém.
fn decode_bmp(data: &[u8]) -> Result<Image, ImageError> {
    let u16_at = |at: usize| -> u16 { u16::from_le_bytes([data[at], data[at + 1]]) };
    let u32_at = |at: usize| -> u32 {
        u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
    };
    if data.len() < 54 {
        return Err(ImageError::Malformed("cabeçalho truncado"));
    }
    let data_offset = u32_at(10) as usize;
    let header_size = u32_at(14) as usize;
    let width = u32_at(18) as i32 as isize;
    let raw_height = u32_at(22) as i32 as isize;
    let depth = u16_at(28) as usize;
    let compression = u32_at(30);
    // Altura negativa significa que as linhas vêm de cima para baixo, ao contrário do usual.
    let (height, top_down) = (raw_height.unsigned_abs(), raw_height < 0);
    let width = match width > 0 {
        true => width as usize,
        false => return Err(ImageError::Malformed("largura inválida")),
    };
    if compression != 0 {
        return Err(ImageError::Malformed("bmp comprimido"));
    }
    if width * height > MAX_PIXELS {
        return Err(ImageError::Malformed("imagem grande demais"));
    }

    // A paleta fica entre o cabeçalho e os pixels. Quando a contagem de cores vem zerada, ela
    // ocupa tudo o que couber ali — que é como os `.mif` a gravam.
    let palette_start = 14 + header_size;
    let mut palette = Vec::new();
    if depth <= 8 {
        let declared = u32_at(46) as usize;
        let available = data_offset.saturating_sub(palette_start) / 4;
        let count = match declared {
            0 => available,
            n => n.min(available),
        };
        for i in 0..count {
            let at = palette_start + i * 4;
            if at + 3 >= data.len() {
                break;
            }
            // O BMP guarda azul, verde e vermelho nesta ordem.
            palette.push([data[at + 2], data[at + 1], data[at]]);
        }
    }

    // Cada linha é preenchida até um múltiplo de quatro bytes.
    let stride = (width * depth).div_ceil(8).div_ceil(4) * 4;
    if data_offset + stride * height > data.len() {
        return Err(ImageError::Malformed("pixels truncados"));
    }
    let color = |index: usize| -> [u8; 3] {
        palette.get(index).copied().unwrap_or([255, 0, 255]) // Índice fora da paleta salta à vista em vez de sumir.
    };

    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        let row = match top_down {
            true => y,
            false => height - 1 - y,
        };
        let line = &data[data_offset + row * stride..][..stride];
        for x in 0..width {
            let (rgb, alpha) = match depth {
                1 => (color((line[x / 8] >> (7 - x % 8)) as usize & 1), 255),
                4 => {
                    let byte = line[x / 2];
                    let index = match x % 2 {
                        0 => byte >> 4,
                        _ => byte & 0x0f,
                    };
                    (color(index as usize), 255)
                }
                8 => (color(line[x] as usize), 255),
                24 => ([line[x * 3 + 2], line[x * 3 + 1], line[x * 3]], 255),
                32 => (
                    [line[x * 4 + 2], line[x * 4 + 1], line[x * 4]],
                    // Um BMP de 32 bits com o canal sempre zerado é opaco, não invisível.
                    line[x * 4 + 3],
                ),
                _ => return Err(ImageError::Malformed("profundidade não suportada")),
            };
            rgba.extend_from_slice(&rgb);
            rgba.push(alpha);
        }
    }
    // A regra do canal zerado só pode ser aplicada depois de ver a imagem inteira.
    if depth == 32 && rgba.iter().skip(3).step_by(4).all(|&a| a == 0) {
        rgba.iter_mut().skip(3).step_by(4).for_each(|a| *a = 255);
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta um BMP paletado de 4 bits, 2x2, com o cabeçalho que os `.mif` usam.
    fn bmp4() -> Vec<u8> {
        let palette: [[u8; 4]; 2] = [[0, 0, 255, 0], [0, 255, 0, 0]]; // BGRA: vermelho, verde.
        let data_offset = 54 + palette.len() * 4;
        let mut bmp = vec![0u8; data_offset];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[10..14].copy_from_slice(&(data_offset as u32).to_le_bytes());
        bmp[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp[18..22].copy_from_slice(&2u32.to_le_bytes());
        bmp[22..26].copy_from_slice(&2u32.to_le_bytes());
        bmp[26..28].copy_from_slice(&1u16.to_le_bytes());
        bmp[28..30].copy_from_slice(&4u16.to_le_bytes());
        bmp[46..50].copy_from_slice(&(palette.len() as u32).to_le_bytes());
        for (i, entry) in palette.iter().enumerate() {
            bmp[54 + i * 4..54 + i * 4 + 4].copy_from_slice(entry);
        }
        // De baixo para cima: primeiro a linha de baixo (verde, vermelho).
        bmp.extend_from_slice(&[0x10, 0, 0, 0]);
        bmp.extend_from_slice(&[0x01, 0, 0, 0]);
        bmp
    }

    #[test]
    fn o_bmp_paletado_vira_cores_na_ordem_certa() {
        // O BMP guarda as linhas de baixo para cima e as cores em BGR: errar qualquer um dos
        // dois dá uma imagem plausível e errada, que ninguém percebe olhando.
        let image = decode(&bmp4()).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(&image.rgba[0..4], &[255, 0, 0, 255], "topo à esquerda");
        assert_eq!(&image.rgba[4..8], &[0, 255, 0, 255], "topo à direita");
        assert_eq!(&image.rgba[8..12], &[0, 255, 0, 255], "base à esquerda");
        assert_eq!(&image.rgba[12..16], &[255, 0, 0, 255], "base à direita");
    }

    #[test]
    fn a_reducao_respeita_o_lado_maior_e_nao_mexe_no_que_ja_cabe() {
        let grande = Image {
            width: 100,
            height: 50,
            rgba: vec![255; 100 * 50 * 4],
        };
        // Fator 10: 100 -> 10, e a altura acompanha.
        let pequena = grande.downscaled(10);
        assert_eq!((pequena.width, pequena.height), (10, 5));
        // O que já cabe volta igual.
        assert_eq!(pequena.downscaled(64), pequena);
    }

    #[test]
    fn o_transparente_nao_escurece_a_borda_ao_reduzir() {
        // Um bloco 2x2 com um pixel branco opaco e três transparentes pretos. Media ingênua
        // daria cinza escuro; com a cor premultiplicada, a cor que sobra é a do único pixel que
        // existe de verdade — e só o alfa cai.
        let image = Image {
            width: 2,
            height: 2,
            rgba: vec![
                255, 255, 255, 255, // branco opaco
                0, 0, 0, 0, //
                0, 0, 0, 0, //
                0, 0, 0, 0,
            ],
        };
        let one = image.downscaled(1);
        assert_eq!((one.width, one.height), (1, 1));
        assert_eq!(
            &one.rgba[0..3],
            &[255, 255, 255],
            "a cor é a do pixel visível"
        );
        assert_eq!(one.rgba[3], 63, "e o alfa é a média dos quatro");
    }

    #[test]
    fn o_formato_sai_da_assinatura_e_nao_do_rotulo() {
        assert!(matches!(decode(b"nada disso"), Err(ImageError::Unknown)));
        assert!(matches!(decode(&[]), Err(ImageError::Unknown)));
    }

    #[test]
    fn bmp_comprimido_e_recusado_por_nome() {
        // Recusar dizendo o motivo evita a caçada a um ícone que some sem explicação.
        let mut bmp = bmp4();
        bmp[30..34].copy_from_slice(&1u32.to_le_bytes());
        let err = decode(&bmp).unwrap_err();
        assert_eq!(err.to_string(), "imagem inconsistente: bmp comprimido");
    }

    #[test]
    fn bmp_truncado_nao_derruba_nada() {
        let bmp = bmp4();
        assert!(decode(&bmp[..bmp.len() - 4]).is_err());
        assert!(decode(&bmp[..20]).is_err());
    }

    #[test]
    fn o_png_paletado_e_expandido_para_cores() {
        // O `png` entrega o índice da paleta se não pedirmos a expansão, e os ícones dos jogos
        // são quase todos paletados.
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut out), 2, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_palette(vec![255, 0, 0, 0, 0, 255]);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[0, 1])
                .unwrap();
        }
        let image = decode(&out).unwrap();
        assert_eq!(&image.rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&image.rgba[4..8], &[0, 0, 255, 255]);
    }
}
