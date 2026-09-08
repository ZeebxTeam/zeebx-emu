//! A lista de jogos que o emulador encontra numa pasta.
//!
//! Duas disposições convivem. A do console instala cada título como
//! `<Título>/mod/<id>/<nome>.mod`, com o `.mif` num `mif/` irmão; os exemplos do SDK deixam o
//! `.mod` e o `.mif` lado a lado. Como as duas aparecem na prática, a varredura procura
//! `.mod` em qualquer profundidade razoável e deduz o título de onde o arquivo está.

use std::path::{Path, PathBuf};

use crate::archive;
use crate::icon::{self, Image};
use crate::miffile::MifFile;

/// Extensões aceitas para uma capa deixada ao lado do jogo.
const COVER_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "bmp"];

/// Até onde descer a partir da pasta escolhida. Três níveis cobrem `<Título>/mod/<id>/` com
/// folga; mais que isso só serviria para varrer o disco inteiro por engano.
const MAX_DEPTH: usize = 4;

/// Teto de jogos por varredura, para uma pasta escolhida sem querer não travar a interface.
const MAX_GAMES: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    /// Como o jogo aparece na lista.
    pub title: String,
    /// O `.mod` a carregar, ou o `.zip` que o contém.
    pub path: PathBuf,
    /// O ClassID do applet, quando há um `.mif` para dizê-lo. Um jogo em `.zip` só revela o
    /// dele depois de extraído, então aqui ele vem vazio.
    pub clsid: Option<u32>,
    /// Se o caminho é um pacote que precisa ser extraído antes de rodar.
    pub packed: bool,
    /// A imagem que representa o jogo na biblioteca. Vem de uma capa deixada ao lado do
    /// arquivo, ou, na falta dela, do maior ícone que o `.mif` guarda.
    pub art: Option<Image>,
}

/// Procura jogos em `root`, em ordem de título.
pub fn scan(root: &Path) -> Vec<Game> {
    let mut found = Vec::new();
    collect(root, 0, &mut found);
    let mut games: Vec<Game> = found.into_iter().filter_map(describe).collect();
    games.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    games
}

/// Transforma um arquivo encontrado em jogo. `None` para o que não é jogo — um `.zip` sem
/// módulo dentro é só um zip.
fn describe(path: PathBuf) -> Option<Game> {
    let packed = path.extension().and_then(|e| e.to_str()) == Some("zip");
    if !packed {
        return Some(Game {
            title: title_for(&path),
            clsid: applet_clsid(&path),
            art: cover(&path).or_else(|| manifest_art(&manifest(&path)?)),
            path,
            packed,
        });
    }
    let module = archive::find_module(&path)?;
    Some(Game {
        title: archive::title_of(&path, &module),
        clsid: None,
        art: cover(&path).or_else(|| manifest_art(&archive::find_manifest(&path, &module)?)),
        path,
        packed,
    })
}

/// Uma imagem deixada ao lado do jogo, com o mesmo nome dele.
///
/// É a saída para quem quer uma capa de verdade: os `.mif` só guardam ícones de menu, que não
/// passam de 65×42, e nenhuma ROM traz arte maior que isso.
fn cover(path: &Path) -> Option<Image> {
    COVER_EXTENSIONS
        .iter()
        .map(|extension| path.with_extension(extension))
        .filter(|candidate| candidate != path)
        .filter_map(|candidate| std::fs::read(candidate).ok())
        .find_map(|data| icon::decode(&data).ok())
}

/// O maior ícone do `.mif` — o que rende a melhor imagem na tela.
fn manifest_art(data: &[u8]) -> Option<Image> {
    let mif = MifFile::parse(data).ok()?;
    mif.images(data)
        .into_iter()
        .filter_map(|image| icon::decode(image).ok())
        .max_by_key(Image::pixels)
}

/// O conteúdo do `.mif` que acompanha um `.mod` solto no disco.
fn manifest(mod_path: &Path) -> Option<Vec<u8>> {
    manifest_paths(mod_path)
        .into_iter()
        .find_map(|path| std::fs::read(path).ok())
}

fn collect(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || found.len() >= MAX_GAMES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // Ordenar deixa a varredura repetível: `read_dir` não promete ordem nenhuma, e uma lista
    // que muda de posição entre duas aberturas é confusa para quem usa.
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(&path, depth + 1, found);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("mod" | "zip")
        ) {
            found.push(path);
        }
    }
}

/// O nome a mostrar para um `.mod`.
///
/// No layout do console o nome do arquivo é um identificador numérico sem graça, e quem tem o
/// nome do jogo é a pasta do título, dois níveis acima. Fora desse layout, o nome do arquivo é
/// o que há.
pub fn title_for(mod_path: &Path) -> String {
    let stem = mod_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let parent = mod_path.parent();
    let is_console_layout = parent
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        == Some("mod");
    if is_console_layout {
        if let Some(title) = parent
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
        {
            return title.to_string();
        }
    }
    stem.to_string()
}

/// Procura o `.mif` do título e devolve o ClassID do applet.
///
/// O layout instalado é `<titulo>/mod/<id>/<nome>.mod` com o `.mif` em `<titulo>/mif/<id>.mif`;
/// os exemplos do SDK deixam o `.mif` ao lado do `.mod`. Cobrimos os dois casos.
pub fn applet_clsid(mod_path: &Path) -> Option<u32> {
    manifest_paths(mod_path)
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .filter_map(|data| MifFile::parse(&data).ok())
        .find_map(|mif| mif.main_applet())
}

/// Onde o `.mif` de um `.mod` pode estar, na ordem em que vale procurar.
fn manifest_paths(mod_path: &Path) -> Vec<PathBuf> {
    let mut candidates = vec![mod_path.with_extension("mif")];
    if let Some(dir) = mod_path.parent() {
        if let Some(id) = dir.file_name() {
            // .../mod/<id>/x.mod  ->  .../mif/<id>.mif
            if let Some(title_dir) = dir.parent().and_then(|p| p.parent()) {
                candidates.push(title_dir.join("mif").join(id).with_extension("mif"));
            }
        }
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Monta uma árvore de arquivos vazios e devolve a raiz.
    fn tree(name: &str, files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("zeebx-testes-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, []).unwrap();
        }
        root
    }

    #[test]
    fn o_titulo_vem_da_pasta_no_layout_do_console() {
        // `<Título>/mod/<id>/<nome>.mod` — o nome do arquivo é um identificador, o título está
        // três níveis acima.
        let path = Path::new("/roms/Zeebo Sports Peteca/mod/279159/zeebopeteca.mod");
        assert_eq!(title_for(path), "Zeebo Sports Peteca");
    }

    #[test]
    fn fora_do_layout_do_console_o_titulo_e_o_nome_do_arquivo() {
        assert_eq!(title_for(Path::new("/roms/helloworld.mod")), "helloworld");
        // Uma pasta chamada `mod` no lugar errado não faz do avô um título.
        assert_eq!(title_for(Path::new("/mod/x.mod")), "x");
    }

    #[test]
    fn a_varredura_acha_as_duas_disposicoes_e_ordena_por_titulo() {
        let root = tree(
            "biblioteca",
            &[
                "Quake/mod/274802/quake.mod",
                "Quake/mif/274802.mif",
                "Crash/mod/274214/cnk2.mod",
                "avulso.mod",
                "leiame.txt",
                "Quake/mod/274802/dados.bin",
            ],
        );
        let games = scan(&root);
        let titles: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["avulso", "Crash", "Quake"]);
        // O `.mif` está vazio, então não há ClassID — mas o jogo continua na lista, porque
        // quem diz se o módulo presta é o emulador, não a varredura.
        assert!(games.iter().all(|g| g.clsid.is_none()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pasta_que_nao_existe_devolve_lista_vazia() {
        let games = scan(&std::env::temp_dir().join("zeebx-nao-existe-mesmo"));
        assert!(games.is_empty());
    }
}
