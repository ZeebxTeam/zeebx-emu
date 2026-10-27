//! O quadro de 960×544, em `A8B8G8R8`, e a lista desenhada em cima dele.
//!
//! A palavra do pixel é `0xAABBGGRR`: o firmware chama o formato de `A8B8G8R8`, e num ARM
//! little-endian o vermelho fica no byte baixo. É o layout que o `sceDisplaySetFrameBuf` varre.

use crate::fonte::FONTE;

/// Cada ponto da fonte vira um quadrado de 2×2. Em 960×544, 8×8 cru fica pequeno demais na
/// distância em que se segura o aparelho.
const ESCALA: usize = 2;
const LINHA: usize = 8 * ESCALA;

pub const PRETO: u32 = 0xFF00_0000;
pub const FUNDO: u32 = 0xFF0C_121A;
pub const TEXTO: u32 = 0xFFEE_EE_EE;
pub const DESTAQUE: u32 = 0xFF20_70E0;
pub const ERRO: u32 = 0xFF40_60FF;
pub const BARRA: u32 = 0xFF10_2840;

pub struct Quadro<'a> {
    pub pixels: &'a mut [u32],
    pub largura: usize,
    pub passo: usize,
    pub altura: usize,
}

pub fn limpa(q: &mut Quadro<'_>, cor: u32) {
    q.pixels.fill(cor);
}

pub fn barra(q: &mut Quadro<'_>, x: usize, y: usize, w: usize, h: usize, cor: u32) {
    let y1 = y.saturating_add(h).min(q.altura);
    let x1 = x.saturating_add(w).min(q.largura);
    for yy in y..y1 {
        let linha = yy * q.passo;
        for xx in x..x1 {
            q.pixels[linha + xx] = cor;
        }
    }
}

/// O acento cai fora. A fonte é ASCII, e uma lista em português sem "ã" fica legível; uma letra
/// que vira buraco, não.
pub fn sem_acento(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
        'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'A',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'É' | 'È' | 'Ê' | 'Ë' => 'E',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'Í' | 'Ì' | 'Î' | 'Ï' => 'I',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'O',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'Ú' | 'Ù' | 'Û' | 'Ü' => 'U',
        'ç' => 'c',
        'Ç' => 'C',
        'ñ' => 'n',
        'Ñ' => 'N',
        outro => outro,
    }
}

pub fn texto(q: &mut Quadro<'_>, mut x: usize, y: usize, frase: &str, cor: u32) {
    for c in frase.chars() {
        let c = sem_acento(c);
        let indice = c as usize;
        if indice >= FONTE.len() || x + 8 * ESCALA > q.largura || y + LINHA > q.altura {
            x += 8 * ESCALA;
            continue;
        }
        let glifo = &FONTE[indice];
        for linha in 0..8 {
            let bits = glifo[linha];
            for coluna in 0..8 {
                // O bit 0 é o pixel da esquerda. É a convenção do font8x8.
                if bits & (1 << coluna) == 0 {
                    continue;
                }
                let px = x + coluna * ESCALA;
                let py = y + linha * ESCALA;
                for dy in 0..ESCALA {
                    let destino = (py + dy) * q.passo + px;
                    for dx in 0..ESCALA {
                        q.pixels[destino + dx] = cor;
                    }
                }
            }
        }
        x += 8 * ESCALA;
    }
}

pub fn quebra(texto: &str, colunas: usize) -> Vec<String> {
    if colunas == 0 {
        return vec![texto.to_string()];
    }
    let mut linhas = Vec::new();
    for paragrafo in texto.split('\n') {
        let mut atual = String::new();
        for palavra in paragrafo.split_whitespace() {
            if atual.is_empty() {
                atual = palavra.to_string();
            } else if atual.chars().count() + 1 + palavra.chars().count() <= colunas {
                atual.push(' ');
                atual.push_str(palavra);
            } else {
                linhas.push(std::mem::take(&mut atual));
                atual = palavra.to_string();
            }
        }
        if !atual.is_empty() {
            linhas.push(atual);
        }
    }
    linhas
}

/// O retângulo inteiro do jogo dentro da tela, sem cortar. Inteiro, para o vizinho de pixel não
/// depender de arredondamento.
pub fn encaixa(
    destino_w: u32,
    destino_h: u32,
    origem_w: u32,
    origem_h: u32,
) -> (u32, u32, u32, u32) {
    if destino_w == 0 || destino_h == 0 || origem_w == 0 || origem_h == 0 {
        return (0, 0, 0, 0);
    }
    let dw = u64::from(destino_w);
    let dh = u64::from(destino_h);
    let sw = u64::from(origem_w);
    let sh = u64::from(origem_h);
    let (w, h) = if dw * sh <= dh * sw {
        (destino_w, (sh * dw / sw) as u32)
    } else {
        ((sw * dh / sh) as u32, destino_h)
    };
    ((destino_w - w) / 2, (destino_h - h) / 2, w, h)
}

pub fn rgb565_para_tela(pixel: u16) -> u32 {
    let r5 = (pixel >> 11) & 0x1f;
    let g6 = (pixel >> 5) & 0x3f;
    let b5 = pixel & 0x1f;
    // Repetir os bits altos nos baixos: o branco do console é 31, e sem isto vira 248.
    // `u32`: o deslocamento de 16 não cabe num `u16`, e o tipo do framebuffer é a palavra.
    let r = u32::from((r5 << 3) | (r5 >> 2));
    let g = u32::from((g6 << 2) | (g6 >> 4));
    let b = u32::from((b5 << 3) | (b5 >> 2));
    0xFF00_0000 | (b << 16) | (g << 8) | r
}

pub fn apresenta_rgb565(q: &mut Quadro<'_>, src: &[u16], origem_w: u32, origem_h: u32) {
    let (x0, y0, w, h) = encaixa(q.largura as u32, q.altura as u32, origem_w, origem_h);
    if w == 0 || h == 0 {
        return;
    }
    let sw = origem_w as usize;
    let sh = origem_h as usize;
    if src.len() < sw * sh {
        return;
    }
    let w = w as usize;
    let h = h as usize;
    for y in 0..h {
        let sy = y * sh / h;
        let linha = (y0 as usize + y) * q.passo + x0 as usize;
        for x in 0..w {
            let sx = x * sw / w;
            q.pixels[linha + x] = rgb565_para_tela(src[sy * sw + sx]);
        }
    }
}

/// Copia o quadro do console pixel a pixel, centrado. A faixa em volta fica como está.
///
/// A Vita tem 960×544 e o Zeebo 640×480. O único inteiro que cabe é 1. Esticar até 725×544
/// com o vizinho mais próximo repete uma coluna e não a seguinte: a imagem treme e parece
/// borrada mesmo quando o 3D está na resolução nativa. Se a origem não couber, cai no
/// encaixe — o quadro do console cabe.
pub fn copia_rgb565(q: &mut Quadro<'_>, src: &[u16], origem_w: u32, origem_h: u32) {
    if origem_w == 0 || origem_h == 0 {
        return;
    }
    let sw = origem_w as usize;
    let sh = origem_h as usize;
    if src.len() < sw * sh || sw > q.largura || sh > q.altura {
        apresenta_rgb565(q, src, origem_w, origem_h);
        return;
    }
    let x0 = (q.largura - sw) / 2;
    let y0 = (q.altura - sh) / 2;
    for y in 0..sh {
        let destino = (y0 + y) * q.passo + x0;
        let origem = y * sw;
        for x in 0..sw {
            q.pixels[destino + x] = rgb565_para_tela(src[origem + x]);
        }
    }
}

fn colunas(q: &Quadro<'_>) -> usize {
    q.largura.saturating_sub(32) / (8 * ESCALA)
}

pub fn desenha_menu(q: &mut Quadro<'_>, titulos: &[&str], cursor: usize, erro: &str) {
    limpa(q, FUNDO);
    texto(q, 16, 16, "Zeebx", DESTAQUE);
    texto(q, 16, 16 + LINHA, "ux0:data/zeebx/roms", TEXTO);
    let mut y = 16 + LINHA * 3;
    if !erro.is_empty() {
        for linha in quebra(erro, colunas(q)).into_iter().take(4) {
            texto(q, 16, y, &linha, ERRO);
            y += LINHA;
        }
        y += LINHA / 2;
    }
    let rodape = LINHA * 2 + 8;
    let limite = q.altura.saturating_sub(rodape);
    if titulos.is_empty() && erro.is_empty() {
        texto(q, 16, y, "Nenhum jogo nesta pasta.", TEXTO);
    } else {
        let visiveis = limite.saturating_sub(y) / LINHA;
        let inicio = janela_segura(cursor, titulos.len(), visiveis);
        for (i, titulo) in titulos
            .iter()
            .enumerate()
            .skip(inicio)
            .take(visiveis.max(1))
        {
            if y + LINHA > limite {
                break;
            }
            if i == cursor {
                barra(q, 8, y, q.largura.saturating_sub(16), LINHA, BARRA);
            }
            let marca = if i == cursor { "> " } else { "  " };
            texto(q, 16, y, &format!("{marca}{titulo}"), TEXTO);
            y += LINHA;
        }
    }
    let yrod = q.altura.saturating_sub(rodape);
    texto(
        q,
        16,
        yrod,
        "X inicia    O sai    Triângulo atualiza",
        TEXTO,
    );
    texto(q, 16, yrod + LINHA, "Select+Start sai do jogo", TEXTO);
}

fn janela_segura(cursor: usize, total: usize, visiveis: usize) -> usize {
    crate::lista::janela(cursor, total, visiveis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_branco_e_o_vermelho_do_console_enchem_o_canal() {
        assert_eq!(rgb565_para_tela(0), PRETO);
        assert_eq!(rgb565_para_tela(0xffff), 0xFFFF_FFFF);
        assert_eq!(
            rgb565_para_tela(0xf800),
            0xFF00_00FF,
            "vermelho no byte baixo"
        );
    }

    #[test]
    fn a_tela_do_zeebo_cabe_inteira_com_faixa_nas_laterais() {
        assert_eq!(encaixa(960, 544, 640, 480), (117, 0, 725, 544));
        assert_eq!(encaixa(0, 10, 10, 10), (0, 0, 0, 0));
    }

    #[test]
    fn um_pixel_vermelho_enche_o_destino() {
        let mut pixels = vec![0u32; 4];
        let mut q = Quadro {
            pixels: &mut pixels,
            largura: 2,
            passo: 2,
            altura: 2,
        };
        apresenta_rgb565(&mut q, &[0xf800], 1, 1);
        assert!(q.pixels.iter().all(|p| *p == 0xFF00_00FF));
    }

    #[test]
    fn o_pixel_do_console_cai_no_centro_sem_esticar() {
        let mut pixels = vec![FUNDO; 8 * 6];
        let mut q = Quadro {
            pixels: &mut pixels,
            largura: 8,
            passo: 8,
            altura: 6,
        };
        copia_rgb565(&mut q, &[0xf800, 0, 0, 0], 2, 2);
        assert_eq!(
            q.pixels[2 * 8 + 3],
            0xFF00_00FF,
            "canto do bloco, no centro"
        );
        assert_eq!(
            q.pixels[2 * 8 + 4],
            PRETO,
            "o pixel ao lado, ainda da origem"
        );
        assert_eq!(q.pixels[0], FUNDO, "a faixa não é esticada");
        assert_eq!(q.pixels[2 * 8 + 5], FUNDO);
    }

    #[test]
    fn a_exclamacao_acende_o_meio_e_o_acento_cai() {
        assert_eq!(sem_acento('ã'), 'a');
        assert_eq!(sem_acento('Ç'), 'C');
        let mut pixels = vec![FUNDO; 32 * 16];
        let mut q = Quadro {
            pixels: &mut pixels,
            largura: 32,
            passo: 32,
            altura: 16,
        };
        texto(&mut q, 0, 0, "!", TEXTO);
        assert_eq!(q.pixels[6], TEXTO);
        assert_eq!(q.pixels[0], FUNDO);
    }

    #[test]
    fn a_quebra_respeita_a_coluna() {
        let linhas = quebra("um dois tres", 7);
        assert_eq!(linhas, vec!["um dois".to_string(), "tres".to_string()]);
    }

    #[test]
    fn o_menu_pinta_o_titulo() {
        let mut pixels = vec![0u32; 960 * 64];
        let mut q = Quadro {
            pixels: &mut pixels,
            largura: 960,
            passo: 960,
            altura: 64,
        };
        desenha_menu(&mut q, &["Quake"], 0, "");
        assert!(q.pixels.contains(&DESTAQUE));
    }
}
