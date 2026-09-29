//! O que a Z-Wheel sabe dos jogos: a caixa, o nome oficial e a descrição de cada um.
//!
//! A Z-Wheel não tira a capa do jogo — ela traz as capas no próprio pacote, em
//! `mod/<id>/assets/games/<game_id>/`, e liga cada pasta ao jogo pelo banco `tt_game_info`
//! (SQLite): a tabela `GAMEINFO` leva `game_id` ao `class_id` do applet e ao caminho da capa, e
//! `TITLETEXT` guarda o nome em cada idioma. O `class_id` é o mesmo ClassID que a biblioteca usa
//! como chave, então a ligação não depende de nome de arquivo nem de pasta.
//!
//! O logo do rolo de cima vem de outro banco, o `asset_cache`: a tabela `ASSETS` diz de que jogo
//! (`owner`, o `game_id`) é cada cena do palco (`assets/stage_slides/<dslid>/`), e a cena traz o
//! logo como textura, `slidebanner.qxt`. Só os jogos em destaque têm cena — 12 no pacote.
//!
//! O pacote pode estar solto numa pasta ou num `.zip`; os dois são lidos sem extrair nada.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::video::icon::{self, Image};

/// O banco da Z-Wheel, ao lado do `tectoy.mod`.
const BANCO: &str = "tt_game_info";
/// O banco que liga as cenas do palco aos jogos.
const CENAS: &str = "asset_cache";
/// O tipo das cenas do palco na tabela `ASSETS`. Os outros são as páginas de ajuda (6 e 7) e as
/// pastas das capas (8).
const TIPO_CENA: i64 = 5;
/// O formato de textura QX dos logos: ATITC só de cor, blocos de 8 bytes.
const QXT_ATC_RGB: u32 = 0x0c;
/// O cabeçalho de um `.qxt` antes dos texels.
const QXT_CABECALHO: usize = 40;

/// O que a Z-Wheel guarda de um jogo.
#[derive(Debug, Clone, Default)]
pub struct Ficha {
    /// Por idioma, na chave de duas letras (`pt`, `en`, `es`).
    titulos: HashMap<String, String>,
    descricoes: HashMap<String, String>,
    pub capa: Option<Image>,
    /// O logo com que o jogo aparece no rolo de cima da Z-Wheel, quando ele tem cena no palco.
    pub logo: Option<Image>,
    /// A classificação indicativa (`rating.jpg`).
    pub classificacao: Option<Image>,
}

impl Ficha {
    pub fn titulo(&self, idioma: &str) -> Option<&str> {
        escolhe(&self.titulos, idioma)
    }

    pub fn descricao(&self, idioma: &str) -> Option<&str> {
        escolhe(&self.descricoes, idioma)
    }
}

/// O idioma pedido, e na falta dele o inglês: é o que todas as fichas trazem.
fn escolhe<'a>(mapa: &'a HashMap<String, String>, idioma: &str) -> Option<&'a str> {
    let curto = idioma.get(..2).unwrap_or(idioma).to_ascii_lowercase();
    mapa.get(&curto)
        .or_else(|| mapa.get("en"))
        .or_else(|| mapa.values().next())
        .map(String::as_str)
}

/// O nome com que um jogo aparece: o oficial da Z-Wheel no idioma da interface, quando ela conhece
/// o jogo, e o da pasta ou do pacote no resto.
pub fn titulo_de(acervo: Option<&Acervo>, jogo: &crate::library::Game, idioma: &str) -> String {
    jogo.clsid
        .and_then(|classe| acervo?.ficha(classe))
        .and_then(|ficha| ficha.titulo(idioma))
        .map(str::to_string)
        .unwrap_or_else(|| jogo.title.clone())
}

/// A imagem de um jogo na biblioteca, caindo na `reserva` quando ele não tem nenhuma.
///
/// A capa deixada ao lado do jogo é escolha de quem montou a pasta, e vale mais que a da Z-Wheel;
/// a da Z-Wheel vale mais que o ícone do `.mif`. A capa ao lado já está no `jogo.art`, quando
/// existe; `capa_ao_lado` diz se ela existe, e vem de quem chama — é o [`capa_ao_lado`], que lê o
/// disco, e quem desenha a lista toda precisa guardar a resposta em vez de perguntar a cada quadro.
pub fn imagem_do_jogo<'a>(
    jogo: &'a crate::library::Game,
    ficha: Option<&'a Ficha>,
    reserva: Option<&'a Image>,
    capa_ao_lado: bool,
) -> Option<&'a Image> {
    let capa = ficha.and_then(|ficha| ficha.capa.as_ref()).filter(|_| !capa_ao_lado);
    capa.or(jogo.art.as_ref()).or(reserva)
}

/// Se há uma capa deixada ao lado do jogo. Lê o disco.
pub fn capa_ao_lado(jogo: &crate::library::Game) -> bool {
    crate::library::cover(&jogo.path).is_some()
}

/// Se a imagem é ampliada sem interpolar num quadro deste tamanho.
///
/// Um ícone de 26 pixels aparece ampliado quatro vezes: interpolar viraria um borrão, e o bloco
/// quadrado é o que o console mostrava. Uma imagem grande já entra reduzida, e aí a interpolação é
/// que evita o serrilhado.
pub fn amplia_sem_interpolar(imagem: &Image, quadro: [f32; 2]) -> bool {
    (imagem.width as f32) < quadro[0] && (imagem.height as f32) < quadro[1]
}

/// As fichas de todos os jogos que a Z-Wheel conhece, pelo ClassID.
#[derive(Debug, Default)]
pub struct Acervo {
    fichas: HashMap<u32, Ficha>,
}

impl Acervo {
    /// Lê o acervo do pacote da Z-Wheel em `caminho` — o `.zip`, a pasta ou o `.mod`.
    pub fn carrega(caminho: &Path) -> Option<Self> {
        let mut fonte = Fonte::abre(caminho)?;
        let banco = fonte.le(BANCO)?;
        let linhas = le_banco(&banco)
            .inspect_err(|err| eprintln!("acervo da Z-Wheel: {err}"))
            .ok()?;
        // Sem o banco das cenas o acervo continua servindo: só não há logos.
        let cenas = fonte
            .le(CENAS)
            .and_then(|dados| {
                le_cenas(&dados)
                    .inspect_err(|err| eprintln!("cenas da Z-Wheel: {err}"))
                    .ok()
            })
            .unwrap_or_default();
        let mut fichas = HashMap::new();
        for (game_id, class_id, pasta, titulos) in linhas {
            let pasta = pasta.trim_start_matches("./").trim_end_matches('/');
            let mut descricoes = HashMap::new();
            for idioma in ["pt", "en", "es"] {
                if let Some(texto) = fonte.le(&format!("{pasta}/description_{idioma}.txt")) {
                    descricoes.insert(idioma.to_string(), limpa_descricao(&texto));
                }
            }
            // A grande primeiro: a pequena é a mesma arte em 160×227 com a borda do console.
            let capa = ["boxartlg.jpg", "boxart.bmp"]
                .iter()
                .filter_map(|nome| fonte.le(&format!("{pasta}/{nome}")))
                .find_map(|dados| icon::decode(&dados).ok());
            let classificacao = fonte
                .le(&format!("{pasta}/rating.jpg"))
                .and_then(|dados| icon::decode(&dados).ok());
            let logo = cenas.get(&game_id).and_then(|cena| {
                let cena = cena.trim_start_matches("./").trim_end_matches('/');
                textura_qx(&fonte.le(&format!("{cena}/slidebanner.qxt"))?)
            });
            fichas.insert(
                class_id,
                Ficha {
                    titulos,
                    descricoes,
                    capa,
                    logo,
                    classificacao,
                },
            );
        }
        Some(Self { fichas })
    }

    pub fn ficha(&self, class_id: u32) -> Option<&Ficha> {
        self.fichas.get(&class_id)
    }

    pub fn len(&self) -> usize {
        self.fichas.len()
    }
}

/// Abre uma cópia do banco `nome` com o conteúdo `dados`.
///
/// O SQLite só abre arquivo. Copiar para o cache serve às duas origens igual, e a Z-Wheel que
/// estiver rodando nunca tem o banco dela aberto por nós.
fn abre_copia(nome: &str, dados: &[u8]) -> rusqlite::Result<rusqlite::Connection> {
    let copia = crate::loader::archive::cache_dir().join(format!("z-wheel-{nome}.db"));
    if let Some(pasta) = copia.parent() {
        let _ = std::fs::create_dir_all(pasta);
    }
    std::fs::write(&copia, dados)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(err.into()))?;
    rusqlite::Connection::open_with_flags(&copia, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
}

/// A pasta da primeira cena de cada jogo, pelo `game_id`. O Crash e o Rally Master Pro têm duas,
/// com o mesmo logo.
fn le_cenas(dados: &[u8]) -> rusqlite::Result<HashMap<i64, String>> {
    let conexao = abre_copia(CENAS, dados)?;
    let mut consulta = conexao
        .prepare("SELECT owner, path FROM ASSETS WHERE type = ?1 AND owner <> 0 ORDER BY dslid")?;
    let linhas = consulta.query_map([TIPO_CENA], |linha| {
        Ok((linha.get::<_, i64>(0)?, linha.get::<_, String>(1)?))
    })?;
    let mut cenas = HashMap::new();
    for linha in linhas {
        let (jogo, pasta) = linha?;
        cenas.entry(jogo).or_insert(pasta);
    }
    Ok(cenas)
}

/// Uma textura do QXEngine, no único formato que os logos usam.
///
/// O cabeçalho tem a assinatura `QX\0\0QXT\0`, a largura em `+12`, a altura em `+16` e o formato
/// em `+20`; os texels começam em `+40`. As texturas dos modelos usam outros formatos, que ficam
/// para quando os modelos forem lidos.
fn textura_qx(dados: &[u8]) -> Option<Image> {
    if dados.get(..8)? != b"QX\0\0QXT\0" {
        return None;
    }
    let campo = |em: usize| Some(u32::from_le_bytes(dados.get(em..em + 4)?.try_into().ok()?));
    let (largura, altura, formato) = (campo(12)? as usize, campo(16)? as usize, campo(20)?);
    let blocos = largura.div_ceil(4) * altura.div_ceil(4) * 8;
    if formato != QXT_ATC_RGB || largura == 0 || altura == 0 || largura * altura > 1024 * 1024 {
        return None;
    }
    let texels = dados.get(QXT_CABECALHO..QXT_CABECALHO + blocos)?;
    let rgba = crate::video::atc::decode(texels, largura, altura, false)
        .into_iter()
        .flatten()
        .collect();
    Some(Image {
        width: largura,
        height: altura,
        rgba,
    })
}

/// `(game_id, class_id, pasta da capa, títulos por idioma)` de cada jogo do banco.
#[allow(clippy::type_complexity)]
fn le_banco(dados: &[u8]) -> rusqlite::Result<Vec<(i64, u32, String, HashMap<String, String>)>> {
    let conexao = abre_copia(BANCO, dados)?;
    let mut titulos: HashMap<i64, HashMap<String, String>> = HashMap::new();
    let mut consulta = conexao.prepare("SELECT game_id, lang_id, titletext FROM TITLETEXT")?;
    let linhas = consulta.query_map([], |linha| {
        Ok((
            linha.get::<_, i64>(0)?,
            linha.get::<_, i64>(1)?,
            linha.get::<_, String>(2)?,
        ))
    })?;
    for linha in linhas {
        let (jogo, idioma, titulo) = linha?;
        titulos
            .entry(jogo)
            .or_default()
            .insert(idioma_do_brew(idioma), titulo);
    }
    let mut consulta = conexao.prepare("SELECT game_id, class_id, boxart_path FROM GAMEINFO")?;
    let linhas = consulta.query_map([], |linha| {
        Ok((
            linha.get::<_, i64>(0)?,
            linha.get::<_, i64>(1)?,
            linha.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut jogos = Vec::new();
    for linha in linhas {
        let (jogo, class_id, pasta) = linha?;
        let Some(pasta) = pasta else { continue };
        jogos.push((
            jogo,
            class_id as u32,
            pasta,
            titulos.remove(&jogo).unwrap_or_default(),
        ));
    }
    Ok(jogos)
}

/// O idioma do BREW é um inteiro com as letras em little-endian: `"pt  "` é `0x20207470`.
fn idioma_do_brew(codigo: i64) -> String {
    let letras = (codigo as u32).to_le_bytes();
    String::from_utf8_lossy(&letras).trim().to_ascii_lowercase()
}

/// A descrição sem o aviso da loja que fechou em 2011, que abre quase todas entre `**`.
fn limpa_descricao(dados: &[u8]) -> String {
    let texto = String::from_utf8_lossy(dados);
    let texto = texto.trim_start_matches('\u{feff}').trim();
    let sem_aviso = match texto
        .strip_prefix("**")
        .and_then(|resto| resto.split_once("**"))
    {
        Some((_, depois)) => depois.trim_start(),
        None => texto,
    };
    sem_aviso.trim().to_string()
}

/// De onde vêm os arquivos do pacote: uma pasta, ou um zip com o prefixo até a pasta do módulo.
enum Fonte {
    Pasta(PathBuf),
    Zip(zip::ZipArchive<std::fs::File>, String),
}

impl Fonte {
    fn abre(caminho: &Path) -> Option<Self> {
        let e_zip = caminho
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
        if e_zip {
            let arquivo = zip::ZipArchive::new(std::fs::File::open(caminho).ok()?).ok()?;
            let prefixo = arquivo
                .file_names()
                .find_map(|nome| nome.strip_suffix(BANCO).map(str::to_string))
                .filter(|prefixo| prefixo.is_empty() || prefixo.ends_with('/'))?;
            return Some(Self::Zip(arquivo, prefixo));
        }
        let pasta = if caminho.is_dir() {
            caminho.to_path_buf()
        } else {
            caminho.parent()?.to_path_buf()
        };
        acha_banco(&pasta, 3).map(Self::Pasta)
    }

    fn le(&mut self, relativo: &str) -> Option<Vec<u8>> {
        match self {
            Self::Pasta(pasta) => std::fs::read(pasta.join(relativo)).ok(),
            Self::Zip(arquivo, prefixo) => {
                let mut entrada = arquivo.by_name(&format!("{prefixo}{relativo}")).ok()?;
                let mut dados = Vec::new();
                entrada.read_to_end(&mut dados).ok()?;
                Some(dados)
            }
        }
    }
}

/// A pasta que guarda o banco, descendo de `pasta` até `profundidade` níveis: a raiz do pacote
/// tem o banco em `mod/<id>/`.
fn acha_banco(pasta: &Path, profundidade: usize) -> Option<PathBuf> {
    if pasta.join(BANCO).is_file() {
        return Some(pasta.to_path_buf());
    }
    if profundidade == 0 {
        return None;
    }
    let mut filhas: Vec<PathBuf> = std::fs::read_dir(pasta)
        .ok()?
        .flatten()
        .map(|entrada| entrada.path())
        .filter(|caminho| caminho.is_dir())
        .collect();
    filhas.sort();
    filhas
        .into_iter()
        .find_map(|filha| acha_banco(&filha, profundidade - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_idioma_do_brew_sai_das_letras_em_little_endian() {
        assert_eq!(idioma_do_brew(0x2020_7470), "pt");
        assert_eq!(idioma_do_brew(538996325), "en");
    }

    #[test]
    fn a_descricao_perde_o_aviso_da_loja() {
        let texto = "** Título disponível até 2011. **\nEste incrível jogo de xadrez.".as_bytes();
        assert_eq!(limpa_descricao(texto), "Este incrível jogo de xadrez.");
        assert_eq!(limpa_descricao(b"Sem aviso."), "Sem aviso.");
    }

    #[test]
    fn sem_o_idioma_pedido_vale_o_ingles() {
        let mapa = HashMap::from([("en".to_string(), "Alien Breaker".to_string())]);
        assert_eq!(escolhe(&mapa, "pt-BR"), Some("Alien Breaker"));
    }
}

/// Com o pacote real: `ZEEBX_Z_WHEEL=<zip ou pasta> cargo test --release acervo -- --ignored`.
#[cfg(test)]
mod pacote_real {
    use super::*;

    #[test]
    #[ignore]
    fn o_acervo_do_pacote_real_traz_capas_e_nomes() {
        let caminho = PathBuf::from(std::env::var("ZEEBX_Z_WHEEL").expect("ZEEBX_Z_WHEEL"));
        let acervo = Acervo::carrega(&caminho).expect("acervo");
        let com_capa = acervo.fichas.values().filter(|f| f.capa.is_some()).count();
        let com_logo = acervo.fichas.values().filter(|f| f.logo.is_some()).count();
        println!("{com_logo} com logo");
        assert_eq!(com_logo, 12);
        let alien = acervo
            .fichas
            .values()
            .find(|f| f.titulo("pt") == Some("Alien Breaker"));
        println!("{} fichas, {com_capa} com capa", acervo.len());
        assert!(acervo.len() > 0 && com_capa == acervo.len() && alien.is_some());
    }
}
