//! O desenho do controle que acende quando um botão é apertado.
//!
//! São **duas** peças: a arte, um PNG que só precisa ser bonito, e o mapa, um SVG invisível do
//! mesmo tamanho em que cada forma tem por `id` o nome de um botão do Zeebo (`up`, `b1`,
//! `start`...). Separar as duas é o que permite redesenhar o controle sem tocar em código, e
//! poupa a arte de ter que ser feita com um `id` em cada traço.
//!
//! O mapa é rasterizado **uma vez**, cada forma virando uma silhueta recortada. É dela que
//! saem as duas coisas que interessam — acender o botão certo e saber em qual deles o cursor
//! está. Testar o clique pela silhueta, e não por uma caixa retangular, é o que faz um
//! direcional em cruz e um botão redondo responderem só onde existe botão.
//!
//! Botão que a arte não desenha simplesmente não tem forma no mapa: ele não acende, e continua
//! configurável pela lista.

use std::fmt;

use resvg::{tiny_skia, usvg};

/// A arte do controle, embutida no binário para a tela funcionar sem arquivo nenhum.
pub const DEFAULT_ART: &[u8] = include_bytes!("../../assets/controller.png");
/// O mapa das regiões da arte embutida.
pub const DEFAULT_MAP: &str = include_str!("../../assets/controller-map.svg");

/// A partir de quanta opacidade a silhueta conta como "aqui tem botão".
const HIT_ALPHA: u8 = 96;

/// Até que claridade um pixel de fundo conta como fundo, ao recortar a moldura da arte.
const BACKDROP_LUMA: u8 = 40;

#[derive(Debug)]
pub enum ArtError {
    Png(png::DecodingError),
    Svg(usvg::Error),
    /// A arte e o mapa não têm o mesmo tamanho, ou algum deles não tem tamanho nenhum.
    Size,
}

impl fmt::Display for ArtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Png(err) => write!(f, "arte do controle ilegível: {err}"),
            Self::Svg(err) => write!(f, "mapa do controle ilegível: {err}"),
            Self::Size => write!(f, "a arte e o mapa do controle não batem de tamanho"),
        }
    }
}

impl std::error::Error for ArtError {}

/// A silhueta de um botão, recortada no tamanho dela.
///
/// O recorte não é economia de disco: cada silhueta vira uma textura na placa de vídeo, e
/// guardá-las do tamanho do controle inteiro custaria dezenas de megabytes para desenhar
/// botões de poucos pixels.
pub struct Part {
    /// O nome do botão do Zeebo, o mesmo do `id` no desenho.
    pub button: String,
    /// Onde o recorte fica no desenho, em pixels.
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    /// Só a opacidade, um byte por pixel do recorte. A cor não importa: a silhueta é pintada
    /// com a cor que a tela escolher na hora, o que a deixa legível em qualquer desenho.
    pub alpha: Vec<u8>,
}

impl Part {
    /// O lugar da peça no desenho, de 0 a 1, para a tela posicioná-la em qualquer tamanho.
    pub fn bounds(&self, art: &PadArt) -> [f32; 4] {
        [
            self.x as f32 / art.width as f32,
            self.y as f32 / art.height as f32,
            (self.x + self.width) as f32 / art.width as f32,
            (self.y + self.height) as f32 / art.height as f32,
        ]
    }

    /// A opacidade num ponto do desenho, ou zero fora do recorte.
    fn alpha_at(&self, x: usize, y: usize) -> u8 {
        if x < self.x || y < self.y || x >= self.x + self.width || y >= self.y + self.height {
            return 0;
        }
        self.alpha[(y - self.y) * self.width + (x - self.x)]
    }
}

pub struct PadArt {
    pub width: usize,
    pub height: usize,
    /// A arte, RGBA não-premultiplicado — o que a interface espera receber.
    pub base: Vec<u8>,
    parts: Vec<Part>,
}

impl PadArt {
    /// Decodifica a arte e rasteriza o mapa em cima dela, no mesmo tamanho.
    pub fn parse(art: &[u8], map: &str) -> Result<Self, ArtError> {
        let (mut base, width, height) = decode_png(art)?;
        cut_backdrop(&mut base, width as usize, height as usize);

        let tree = usvg::Tree::from_str(map, &usvg::Options::default()).map_err(ArtError::Svg)?;
        let size = tree.size();
        if size.width() <= 0.0 || size.height() <= 0.0 {
            return Err(ArtError::Size);
        }
        // O mapa é esticado até a arte em vez de exigir os mesmos números: o que precisa bater
        // é a proporção, e um mapa desenhado em metade da escala continua servindo.
        let transform = tiny_skia::Transform::from_scale(
            width as f32 / size.width(),
            height as f32 / size.height(),
        );

        let mut parts = Vec::new();
        for button in crate::input::bindings::CONFIGURABLE {
            let Some(node) = tree.node_by_id(button) else {
                continue;
            };
            // `render_node` desloca o nó para a origem do destino; devolver o deslocamento aqui
            // o mantém no lugar em que foi desenhado, que é o que alinha a silhueta ao fundo.
            let Some(bbox) = node.abs_layer_bounding_box() else {
                continue;
            };
            let mut mask = tiny_skia::Pixmap::new(width, height).ok_or(ArtError::Size)?;
            let placed = transform.pre_translate(bbox.x(), bbox.y());
            if resvg::render_node(node, placed, &mut mask.as_mut()).is_none() {
                continue;
            }
            let full: Vec<u8> = mask.pixels().iter().map(|pixel| pixel.alpha()).collect();
            let Some(part) = crop(button, &full, width as usize, height as usize) else {
                continue;
            };
            parts.push(part);
        }

        Ok(Self {
            width: width as usize,
            height: height as usize,
            base,
            parts,
        })
    }

    /// A arte embutida, com o mapa dela.
    pub fn builtin() -> Result<Self, ArtError> {
        Self::parse(DEFAULT_ART, DEFAULT_MAP)
    }

    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// O botão sob o ponto `(u, v)`, ambos de 0 a 1 sobre o desenho.
    ///
    /// Onde duas peças se sobrepõem vence a mais opaca — a de cima, na prática.
    pub fn hit(&self, u: f32, v: f32) -> Option<&str> {
        if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
            return None;
        }
        let x = (u * self.width as f32) as usize;
        let y = (v * self.height as f32) as usize;
        self.parts
            .iter()
            .filter_map(|part| {
                let alpha = part.alpha_at(x, y);
                (alpha >= HIT_ALPHA).then_some((alpha, part.button.as_str()))
            })
            .max_by_key(|(alpha, _)| *alpha)
            .map(|(_, button)| button)
    }
}

/// Decodifica a arte, devolvendo RGBA não-premultiplicado e o tamanho.
fn decode_png(data: &[u8]) -> Result<(Vec<u8>, u32, u32), ArtError> {
    let decoder = png::Decoder::new(std::io::Cursor::new(data));
    let mut reader = decoder.read_info().map_err(ArtError::Png)?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or(ArtError::Size)?];
    let info = reader.next_frame(&mut buffer).map_err(ArtError::Png)?;
    let channels = info.color_type.samples();
    if info.bit_depth != png::BitDepth::Eight || !(3..=4).contains(&channels) {
        return Err(ArtError::Size);
    }
    let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    for pixel in buffer[..info.buffer_size()].chunks_exact(channels) {
        rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2]]);
        rgba.push(match channels {
            4 => pixel[3],
            _ => 255,
        });
    }
    Ok((rgba, info.width, info.height))
}

/// Torna transparente o fundo escuro que cerca a arte.
///
/// Só o que encosta na borda some: o traço preto de dentro do desenho é tão escuro quanto o
/// fundo, e apagá-lo pela cor deixaria o controle sem contorno nenhum.
fn cut_backdrop(rgba: &mut [u8], width: usize, height: usize) {
    let dark = |index: usize| -> bool {
        let [r, g, b] = [rgba[index * 4], rgba[index * 4 + 1], rgba[index * 4 + 2]];
        (r as u32 * 30 + g as u32 * 59 + b as u32 * 11) / 100 <= BACKDROP_LUMA as u32
    };
    let mut seen = vec![false; width * height];
    let mut queue: Vec<usize> = Vec::new();
    let mut borders: Vec<usize> = (0..width)
        .flat_map(|x| [x, (height - 1) * width + x])
        .chain((0..height).flat_map(|y| [y * width, y * width + width - 1]))
        .collect();
    borders.retain(|&index| !std::mem::replace(&mut seen[index], true) && dark(index));
    queue.extend(borders);

    // A varredura só marca; apagar fica para o fim, porque o teste de escuridão lê as mesmas
    // cores que o apagamento mudaria.
    let mut background = Vec::new();
    while let Some(index) = queue.pop() {
        background.push(index);
        let (x, y) = (index % width, index / width);
        let mut visit = |next: usize, queue: &mut Vec<usize>| {
            if !seen[next] && dark(next) {
                seen[next] = true;
                queue.push(next);
            }
            seen[next] = true;
        };
        if x > 0 {
            visit(index - 1, &mut queue);
        }
        if x + 1 < width {
            visit(index + 1, &mut queue);
        }
        if y > 0 {
            visit(index - width, &mut queue);
        }
        if y + 1 < height {
            visit(index + width, &mut queue);
        }
    }
    for index in background {
        rgba[index * 4 + 3] = 0;
    }
}

/// Recorta a silhueta no menor retângulo que a contém. `None` se ela está vazia — o que
/// acontece com um `id` colado numa peça sem traçado.
fn crop(button: &str, alpha: &[u8], width: usize, height: usize) -> Option<Part> {
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0usize, 0usize);
    for y in 0..height {
        for x in 0..width {
            if alpha[y * width + x] > 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut cropped = Vec::with_capacity(w * h);
    for y in y0..y1 {
        cropped.extend_from_slice(&alpha[y * width + x0..y * width + x1]);
    }
    Some(Part {
        button: button.to_string(),
        x: x0,
        y: y0,
        width: w,
        height: h,
        alpha: cropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uma arte mínima: 20x10 pretos com um quadrado branco no meio.
    fn art() -> Vec<u8> {
        let mut rgba = vec![0u8; 20 * 10 * 4];
        for y in 3..7 {
            for x in 6..14 {
                let i = (y * 20 + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(std::io::Cursor::new(&mut out), 20, 10);
        encoder.set_color(png::ColorType::Rgba);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&rgba)
            .unwrap();
        out
    }

    #[test]
    fn a_arte_embutida_tem_o_mapa_dos_botoes_que_ela_desenha() {
        let art = PadArt::builtin().expect("a arte embutida precisa abrir");
        // O direcional, as ações, os analógicos e o HOME — o botão do meio — estão
        // desenhados; os gatilhos de baixo não aparecem nesta arte, e por isso não têm forma.
        for button in [
            "up", "down", "left", "right", "b1", "b2", "b3", "b4", "lthumb", "rthumb", "back",
            "zl", "zr",
        ] {
            assert!(
                art.parts().iter().any(|part| part.button == button),
                "o mapa não tem a peça {button}"
            );
        }
        assert_eq!(art.width, 1616);
        assert_eq!(art.height, 973);
    }

    #[test]
    fn o_clique_cai_no_botao_desenhado() {
        let art = PadArt::builtin().unwrap();
        let at = |x: f32, y: f32| art.hit(x / 1616.0, y / 973.0);
        // O centro de cada braço do direcional, e depois os quatro botões de ação.
        assert_eq!(at(373.0, 370.0), Some("up"));
        assert_eq!(at(373.0, 620.0), Some("down"));
        assert_eq!(at(230.0, 498.0), Some("left"));
        assert_eq!(at(520.0, 498.0), Some("right"));
        assert_eq!(at(1246.0, 367.0), Some("b4"));
        assert_eq!(at(1373.0, 465.0), Some("b2"));
        assert_eq!(at(1246.0, 565.0), Some("b1"));
        assert_eq!(at(1119.0, 465.0), Some("b3"));
        assert_eq!(at(619.0, 715.0), Some("lthumb"));
        assert_eq!(at(995.0, 715.0), Some("rthumb"));
        // O botão do meio é o HOME; o console não tem Start.
        assert_eq!(at(807.0, 427.0), Some("back"));
    }

    #[test]
    fn fora_das_pecas_nao_ha_botao() {
        let art = PadArt::builtin().unwrap();
        // O miolo do direcional não é direção nenhuma, e o corpo liso não é botão.
        assert_eq!(art.hit(373.0 / 1616.0, 498.0 / 973.0), None);
        assert_eq!(art.hit(0.5, 0.95), None);
        // Fora do desenho também não.
        assert_eq!(art.hit(-0.1, 0.5), None);
        assert_eq!(art.hit(0.5, 1.5), None);
    }

    #[test]
    fn forma_com_id_desconhecido_e_ignorada() {
        let map = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 10">
            <rect id="nao_e_botao" x="0" y="0" width="20" height="10" fill="red"/>
            <rect id="back" x="8" y="4" width="4" height="2" fill="blue"/>
        </svg>"#;
        let art = PadArt::parse(&art(), map).unwrap();
        assert_eq!(art.parts().len(), 1);
        assert_eq!(art.parts()[0].button, "back");
        assert_eq!(art.hit(0.05, 0.05), None);
        assert_eq!(art.hit(0.5, 0.5), Some("back"));
    }

    #[test]
    fn a_moldura_escura_da_arte_fica_transparente() {
        // O fundo que cerca o desenho some para o controle não virar um retângulo preto na
        // tela — mas só o que encosta na borda, senão o traço preto de dentro sumiria junto.
        let map = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 10"/>"#;
        let art = PadArt::parse(&art(), map).unwrap();
        let alpha = |x: usize, y: usize| art.base[(y * 20 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0);
        assert_eq!(alpha(19, 9), 0);
        assert_eq!(alpha(8, 4), 255);
    }

    #[test]
    fn a_peca_e_recortada_no_tamanho_dela() {
        // O recorte precisa cair sobre a forma do mapa, e não sobre a folha inteira: é dele
        // que sai o lugar em que a silhueta é pintada.
        let art = PadArt::builtin().unwrap();
        let up = art.parts().iter().find(|part| part.button == "up").unwrap();
        let [u0, v0, u1, v1] = up.bounds(&art);
        // O `up` é o retângulo 299..447 x 322..432 do mapa.
        let close = |got: f32, want: f32| (got - want).abs() < 0.01;
        assert!(close(u0, 299.0 / 1616.0), "u0 = {u0}");
        assert!(close(v0, 322.0 / 973.0), "v0 = {v0}");
        assert!(close(u1, 447.0 / 1616.0), "u1 = {u1}");
        assert!(close(v1, 432.0 / 973.0), "v1 = {v1}");
        assert_eq!(up.alpha.len(), up.width * up.height);
    }

    #[test]
    fn arte_ou_mapa_invalido_nao_derruba_nada() {
        assert!(PadArt::parse(b"isto nao e um png", DEFAULT_MAP).is_err());
        assert!(PadArt::parse(&art(), "isto não é um svg").is_err());
    }
}
