//! O texto que os jogos mandam desenhar.
//!
//! Até aqui o `IDISPLAY_DrawText` recebia a frase certa e não tinha com o que desenhá-la: o
//! texto ia para uma lista que só aparecia no relatório. Isso é o suficiente para saber que um
//! jogo *quer* escrever, e insuficiente para qualquer tela com menu.
//!
//! A fonte do console vinha da firmware, que não temos. Mas vários módulos trazem a própria: a
//! Z-Wheel empacota uma `tectoy.ttf` de 191 KB, e é a fonte com que a loja foi desenhada. Então
//! a regra é: **usar a fonte que veio com o jogo**. Quando não há nenhuma, continua sem texto —
//! e o relatório diz isso, em vez de desenhar com uma fonte que não é a dele.

use ab_glyph::{Font as _, FontVec, PxScale, ScaleFont as _};

/// Uma fonte carregada de um `.ttf` do próprio jogo.
pub struct Font {
    face: FontVec,
    /// De onde ela veio, para o relatório.
    pub source: String,
}

/// Um glifo já rasterizado: onde pôr e com que cobertura.
pub struct Glyph {
    /// Deslocamento em relação ao ponto de desenho, já com o `bearing` aplicado.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Cobertura por pixel, de 0 a 255, na ordem de leitura.
    pub coverage: Vec<u8>,
}

impl Font {
    /// Lê um `.ttf`. `None` quando o arquivo não é uma fonte que saibamos abrir.
    pub fn load(bytes: Vec<u8>, source: String) -> Option<Self> {
        FontVec::try_from_vec(bytes)
            .ok()
            .map(|face| Self { face, source })
    }

    /// Altura da parte de cima da linha, no tamanho pedido. É o que o `GetFontMetrics` chama de
    /// `ascent`.
    pub fn ascent(&self, size: f32) -> u32 {
        self.face.as_scaled(PxScale::from(size)).ascent().ceil() as u32
    }

    /// Altura da parte de baixo, em valor positivo — o `descent` do BREW é positivo, e o da
    /// fonte é negativo.
    pub fn descent(&self, size: f32) -> u32 {
        (-self.face.as_scaled(PxScale::from(size)).descent())
            .ceil()
            .max(0.0) as u32
    }

    /// Quanto o texto ocupa na horizontal, incluindo o espaçamento entre letras.
    pub fn width(&self, text: &str, size: f32) -> u32 {
        let escala = self.face.as_scaled(PxScale::from(size));
        let mut largura = 0.0;
        let mut anterior = None;
        for c in text.chars() {
            let id = self.face.glyph_id(c);
            if let Some(antes) = anterior {
                largura += escala.kern(antes, id);
            }
            largura += escala.h_advance(id);
            anterior = Some(id);
        }
        largura.ceil().max(0.0) as u32
    }

    /// Rasteriza `text` e devolve um glifo por caractere que tenha desenho.
    ///
    /// A origem é o **canto superior esquerdo** do texto, que é como o BREW posiciona — a
    /// linha de base entra aqui, somando o `ascent`, para quem chama não precisar saber disso.
    pub fn layout(&self, text: &str, size: f32) -> Vec<Glyph> {
        let escala = self.face.as_scaled(PxScale::from(size));
        let base = escala.ascent();
        let mut saida = Vec::new();
        let mut caneta = 0.0;
        let mut anterior = None;
        for c in text.chars() {
            let id = self.face.glyph_id(c);
            if let Some(antes) = anterior {
                caneta += escala.kern(antes, id);
            }
            let glifo =
                id.with_scale_and_position(PxScale::from(size), ab_glyph::point(caneta, base));
            caneta += escala.h_advance(id);
            anterior = Some(id);
            // Espaço não tem contorno: avança a caneta e não desenha nada.
            let Some(desenho) = self.face.outline_glyph(glifo) else {
                continue;
            };
            let caixa = desenho.px_bounds();
            let (largura, altura) = (caixa.width() as u32, caixa.height() as u32);
            if largura == 0 || altura == 0 {
                continue;
            }
            let mut coverage = vec![0u8; (largura * altura) as usize];
            desenho.draw(|x, y, v| {
                if x < largura && y < altura {
                    coverage[(y * largura + x) as usize] =
                        (v * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            });
            saida.push(Glyph {
                x: caixa.min.x as i32,
                y: caixa.min.y as i32,
                width: largura,
                height: altura,
                coverage,
            });
        }
        saida
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fonte que a Z-Wheel empacota. O teste só roda quando ela está no cache — é material do
    /// usuário, não do repositório.
    fn tectoy() -> Option<Font> {
        let cache = dirs_cache()?;
        let ttf = std::fs::read(cache).ok()?;
        Font::load(ttf, "tectoy.ttf".into())
    }

    fn dirs_cache() -> Option<std::path::PathBuf> {
        let base = std::env::var_os("HOME")?;
        let raiz = std::path::Path::new(&base).join(".config/zeebx/cache");
        std::fs::read_dir(raiz)
            .ok()?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("Z-Wheel"))
            })
            .map(|p| p.join("mod/274755/tectoy.ttf"))
            .filter(|p| p.exists())
    }

    #[test]
    fn o_que_nao_e_fonte_e_recusado() {
        assert!(Font::load(b"nao sou uma fonte".to_vec(), "x".into()).is_none());
        assert!(Font::load(Vec::new(), "x".into()).is_none());
    }

    #[test]
    fn a_fonte_do_pacote_mede_e_desenha() {
        let Some(fonte) = tectoy() else {
            // Sem o jogo no cache não há o que testar: o teste existe para quem tem o material.
            return;
        };
        // Uma linha de 16 pixels tem altura de linha plausível, não zero e não absurda.
        let (ascent, descent) = (fonte.ascent(16.0), fonte.descent(16.0));
        assert!(
            (8..40).contains(&ascent),
            "ascent fora do razoável: {ascent}"
        );
        assert!(descent < 20, "descent fora do razoável: {descent}");

        // Texto mais longo ocupa mais espaço, e o espaço em branco conta.
        assert!(fonte.width("Jogar", 16.0) > fonte.width("J", 16.0));
        assert!(fonte.width("a a", 16.0) > fonte.width("aa", 16.0));

        // "Jogar" tem cinco letras e todas desenham alguma coisa.
        let glifos = fonte.layout("Jogar", 16.0);
        assert_eq!(glifos.len(), 5);
        assert!(glifos.iter().all(|g| g.coverage.iter().any(|&v| v > 0)));

        // O espaço não vira glifo, mas empurra o que vem depois.
        let com_espaco = fonte.layout("a b", 16.0);
        assert_eq!(com_espaco.len(), 2);
        assert!(com_espaco[1].x > com_espaco[0].x);
    }
}
