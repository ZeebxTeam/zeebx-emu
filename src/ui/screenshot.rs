//! Onde e como um screenshot é gravado. Ver `docs/implementacao/22-screenshots.md`.
//!
//! Qual quadro sai é do [`crate::session::Session::captura`]; aqui fica o resto, sem janela: o
//! nome da pasta de cada jogo, o nome do arquivo e o `.png`. O atalho e o aviso são de cada
//! frontend.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

/// A pasta de quem não escolheu outra.
pub fn pasta_padrao() -> PathBuf {
    crate::config::config_dir().join("screenshots")
}

/// A pasta de fato: a escolhida, ou a padrão.
pub fn pasta(escolhida: Option<&Path>) -> PathBuf {
    escolhida.map_or_else(pasta_padrao, Path::to_path_buf)
}

/// O título como nome de pasta e de arquivo nos três sistemas.
///
/// Sai o que o Windows recusa, e os pontos e espaços do fim, que ele apaga em silêncio: uma pasta
/// criada no Linux com `Jogo.` viraria outra ao ser copiada. Um título que não sobra nada vira
/// `Zeebx`.
pub fn nome_seguro(titulo: &str) -> String {
    let limpo: String = titulo
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let limpo = limpo.split_whitespace().collect::<Vec<_>>().join(" ");
    let limpo = limpo.trim_end_matches(['.', ' ']);
    match limpo.is_empty() {
        true => "Zeebx".to_string(),
        false => limpo.to_string(),
    }
}

/// Cria o arquivo do screenshot, com um nome que ninguém tem: `<título> - <carimbo>.png`, e
/// ` (2)`, ` (3)`… quando dois caem no mesmo segundo.
///
/// **A reserva é o `create_new`**, e não uma consulta antes. Dois prints seguidos gravam em
/// threads diferentes; "o nome está livre?" seguido de "cria" deixaria as duas com o mesmo.
fn cria_livre(pasta: &Path, base: &str) -> io::Result<(File, PathBuf)> {
    for n in 1u32.. {
        let nome = match n {
            1 => format!("{base}.png"),
            n => format!("{base} ({n}).png"),
        };
        let caminho = pasta.join(nome);
        match OpenOptions::new().write(true).create_new(true).open(&caminho) {
            Ok(arquivo) => return Ok((arquivo, caminho)),
            Err(erro) if erro.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(erro) => return Err(erro),
        }
    }
    unreachable!("os números acabaram antes dos nomes")
}

/// Grava o quadro em `<raiz>/<título>/<título> - <carimbo>.png` e devolve o caminho.
///
/// `rgb` são três bytes por pixel, linhas de cima para baixo. `carimbo` é a hora local já em
/// texto: o núcleo não conhece fuso horário, e quem chama tem um.
pub fn grava(
    raiz: &Path,
    titulo: &str,
    carimbo: &str,
    largura: u32,
    altura: u32,
    rgb: &[u8],
) -> io::Result<PathBuf> {
    let titulo = nome_seguro(titulo);
    let pasta = raiz.join(&titulo);
    std::fs::create_dir_all(&pasta)?;
    let (arquivo, caminho) = cria_livre(&pasta, &format!("{titulo} - {}", nome_seguro(carimbo)))?;
    let escrito = escreve_png(arquivo, largura, altura, rgb);
    // Um arquivo pela metade não é screenshot: se a escrita falhou, o nome reservado sai junto.
    if escrito.is_err() {
        let _ = std::fs::remove_file(&caminho);
    }
    escrito.map(|()| caminho)
}

fn escreve_png(arquivo: File, largura: u32, altura: u32, rgb: &[u8]) -> io::Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(arquivo), largura, altura);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut escritor = encoder.write_header().map_err(io::Error::other)?;
    escritor.write_image_data(rgb).map_err(io::Error::other)?;
    escritor.finish().map_err(io::Error::other)
}

/// O endereço `file://` de uma pasta, para o sistema abrir no gerenciador de arquivos.
///
/// Escapado à mão: um `#` ou um `?` no nome de uma pasta seria lido como o começo de outra parte
/// do endereço, e o gerenciador abriria outro lugar.
pub fn endereco_de(pasta: &Path) -> String {
    let texto = pasta.display().to_string().replace('\\', "/");
    let mut endereco = String::from("file://");
    if !texto.starts_with('/') {
        // `C:/...` no Windows: o endereço é `file:///C:/...`.
        endereco.push('/');
    }
    for byte in texto.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                endereco.push(byte as char)
            }
            outro => endereco.push_str(&format!("%{outro:02X}")),
        }
    }
    endereco
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_titulo_perde_o_que_o_windows_recusa() {
        assert_eq!(nome_seguro("Crash: Nitro Kart"), "Crash Nitro Kart");
        assert_eq!(nome_seguro("A/B\\C?*"), "A B C");
        assert_eq!(nome_seguro("Jogo..."), "Jogo");
        assert_eq!(nome_seguro("Zeebo Extreme Bóia Cross"), "Zeebo Extreme Bóia Cross");
        assert_eq!(nome_seguro(" :?. "), "Zeebx");
        assert_eq!(nome_seguro("2026-09-25 14-03-07"), "2026-09-25 14-03-07");
    }

    #[test]
    fn dois_prints_no_mesmo_segundo_nao_se_sobrescrevem() {
        let raiz = std::env::temp_dir().join(format!("zeebx-testes-screenshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&raiz);
        let preto = vec![0u8; 4 * 3 * 3];
        let primeiro = grava(&raiz, "Quake", "2026-09-25 14-03-07", 4, 3, &preto).unwrap();
        let segundo = grava(&raiz, "Quake", "2026-09-25 14-03-07", 4, 3, &preto).unwrap();
        assert_eq!(primeiro, raiz.join("Quake").join("Quake - 2026-09-25 14-03-07.png"));
        assert_eq!(segundo, raiz.join("Quake").join("Quake - 2026-09-25 14-03-07 (2).png"));

        // O que foi gravado é um PNG de 4×3, RGB, sem alfa.
        let leitor = png::Decoder::new(io::BufReader::new(File::open(&primeiro).unwrap()));
        let leitor = leitor.read_info().unwrap();
        let info = leitor.info();
        assert_eq!((info.width, info.height), (4, 3));
        assert_eq!(info.color_type, png::ColorType::Rgb);
        std::fs::remove_dir_all(&raiz).unwrap();
    }

    #[test]
    fn um_quadro_curto_nao_deixa_arquivo_pela_metade() {
        let raiz = std::env::temp_dir().join(format!("zeebx-testes-screenshot-curto-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&raiz);
        assert!(grava(&raiz, "Quake", "agora", 4, 3, &[0u8; 5]).is_err());
        assert_eq!(std::fs::read_dir(raiz.join("Quake")).unwrap().count(), 0);
        std::fs::remove_dir_all(&raiz).unwrap();
    }

    #[test]
    fn o_endereco_da_pasta_escapa_o_que_confundiria() {
        assert_eq!(
            endereco_de(Path::new("/home/eu/Meus prints#1")),
            "file:///home/eu/Meus%20prints%231"
        );
        assert_eq!(endereco_de(Path::new("/a/Bóia")), "file:///a/B%C3%B3ia");
    }
}
