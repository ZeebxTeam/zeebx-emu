//! Jogos guardados em `.zip`.
//!
//! Um título do Zeebo é uma árvore de arquivos — o `.mod`, o `.mif` e os dados —, e é comum
//! ela circular compactada. O emulador não lê de dentro do arquivo: ele extrai para uma pasta
//! de cache e roda dali. Os jogos gravam (o Peteca tem um `.sav`), e escrever de volta num zip
//! não é coisa que se queira fazer; extrair resolve isso de graça e deixa o resto do emulador
//! sem saber que o zip existe.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::miffile::MifFile;
use crate::settings;

/// Onde as extrações ficam.
pub fn cache_dir() -> PathBuf {
    settings::config_dir().join("cache")
}

/// A raiz do sistema de arquivos do aparelho, comum a todos os jogos.
///
/// No console há **um** sistema de arquivos: o Zeeboids grava os bonecos em
/// `fs:/zeeboiddata/zeeboid.db` e o Zeebo F.C. abre esse mesmo caminho para importá-los. Uma
/// raiz por instalação faria cada jogo ver uma pasta só sua, e o F.C. concluiria — com razão,
/// do ponto de vista dele — que o Zeeboids não está instalado.
pub fn device_dir() -> PathBuf {
    settings::config_dir().join("aparelho")
}

/// O caminho interno do `.mod` dentro do zip, se houver um.
///
/// Havendo mais de um, decide nesta ordem:
///
/// 1. **Ter applet no `.mif`.** Um módulo cujo manifesto não declara applet não tem como ser
///    iniciado. O pacote do Action Hero 3D traz dois jogos, e o outro — o IMICRO3D — declara só
///    uma classe, sem applet: era ele que ganhava, e o pacote inteiro não abria.
/// 2. **A disposição do console**, `<Título>/mod/<id>/<nome>.mod`, mesmo sendo o mais fundo: um
///    pacote com o jogo e algum extra tem o módulo do jogo ali, e o extra é que costuma estar
///    solto.
/// 3. O caminho mais curto.
pub fn find_module(zip: &Path) -> Option<String> {
    let file = std::fs::File::open(zip).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut candidates: Vec<(bool, usize, String)> = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).ok()?;
        // `enclosed_name` recusa caminhos com `..` ou raiz absoluta, que é como um zip
        // malicioso escreveria fora da pasta de destino.
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        if name.extension().and_then(|e| e.to_str()) != Some("mod") {
            continue;
        }
        let name = entry.name().to_string();
        let parts: Vec<&str> = name.split('/').collect();
        let console = parts.len() >= 4 && parts[parts.len() - 3] == "mod";
        candidates.push((console, parts.len(), name));
    }
    // O manifesto só é lido quando há mais de um candidato: com um só, a resposta é ele de
    // qualquer jeito, e abrir o zip de novo seria trabalho à toa em todo jogo do acervo.
    let sozinho = candidates.len() <= 1;
    let com_applet = |name: &str| {
        sozinho
            || find_manifest(zip, name)
                .and_then(|data| MifFile::parse(&data).ok())
                .and_then(|mif| mif.main_applet())
                .is_some()
    };
    candidates
        .into_iter()
        .max_by_key(|(console, depth, name)| {
            (com_applet(name), *console, std::cmp::Reverse(*depth))
        })
        .map(|(_, _, name)| name)
}

/// O conteúdo do `.mif` que acompanha `module` dentro do zip.
///
/// O `.mif` do título fica em `<Título>/mif/<id>.mif`, irmão da pasta `mod/`. Havendo mais de
/// um, vale o que combina com o identificador do módulo; sem isso, o primeiro serve.
pub fn find_manifest(zip: &Path, module: &str) -> Option<Vec<u8>> {
    let file = std::fs::File::open(zip).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    // `<Título>/mod/<id>/x.mod` -> o identificador é a pasta que contém o módulo.
    let parts: Vec<&str> = module.split('/').collect();
    let id = parts.get(parts.len().checked_sub(2)?).copied();
    let mut best: Option<usize> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).ok()?;
        if entry.enclosed_name().is_none() {
            continue;
        }
        let name = entry.name().to_string();
        if !name.ends_with(".mif") {
            continue;
        }
        let matches = id.is_some_and(|id| name.ends_with(&format!("/{id}.mif")));
        if matches {
            best = Some(i);
            break;
        }
        best.get_or_insert(i);
    }
    let mut entry = archive.by_index(best?).ok()?;
    let mut data = Vec::new();
    entry.read_to_end(&mut data).ok()?;
    Some(data)
}

/// O título que o zip anuncia.
///
/// Um pacote feito do diretório do console tem a pasta do título na raiz, e é dela que sai o
/// nome. Sem essa pasta, o nome do próprio arquivo serve.
pub fn title_of(zip: &Path, module: &str) -> String {
    let stem = zip
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let parts: Vec<&str> = module.split('/').collect();
    // `<Título>/mod/<id>/<nome>.mod`
    match parts.len() >= 4 && parts[1] == "mod" {
        true => parts[0].to_string(),
        false => stem,
    }
}

/// Extrai o zip para o cache e devolve o caminho do `.mod`.
///
/// A pasta de destino é nomeada pelo arquivo e pelo que ele tem de tamanho e data: trocar o zip
/// por outro gera outra pasta, e reabrir o mesmo jogo reaproveita a extração anterior.
pub fn extract(zip: &Path) -> std::io::Result<PathBuf> {
    let module = find_module(zip)
        .ok_or_else(|| std::io::Error::other("o zip não contém nenhum arquivo .mod"))?;
    let target = cache_dir().join(fingerprint(zip)?);
    let extracted = target.join(&module);
    // Já extraído antes: nada a fazer.
    if extracted.is_file() {
        return Ok(extracted);
    }

    let file = std::fs::File::open(zip)?;
    let mut archive = zip::ZipArchive::new(file).map_err(std::io::Error::other)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(std::io::Error::other)?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let out = target.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut bytes)?;
        std::fs::write(&out, bytes)?;
    }
    escrever_manifesto(zip, &target)?;
    match extracted.is_file() {
        true => Ok(extracted),
        false => Err(std::io::Error::other(
            "o .mod não apareceu depois de extrair o zip",
        )),
    }
}

/// O nome do arquivo que lista o que veio do pacote.
pub const MANIFESTO: &str = ".zeebx-pacote";

/// Grava, dentro do cache, a lista do que o zip trouxe.
///
/// Existe para o gerenciador de saves poder dizer, **sem adivinhar**, o que é save e o que é
/// conteúdo do jogo. A primeira tentativa comparava datas — o que é mais novo que o `.mod` seria
/// save — e ela cai: os arquivos de uma mesma extração diferem por milissegundos, e o
/// `resources.pakz` de 15 MB do Alice terminava de ser escrito depois do `.mod`. Numa lista com
/// botão de excluir, errar assim apaga o jogo.
///
/// A lista vem do zip, que é a fonte: o que está nele é do pacote, o resto o jogo escreveu.
pub fn escrever_manifesto(zip: &Path, destino: &Path) -> std::io::Result<()> {
    let file = std::fs::File::open(zip)?;
    let mut archive = zip::ZipArchive::new(file).map_err(std::io::Error::other)?;
    let mut nomes = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(std::io::Error::other)?;
        if let Some(nome) = entry.enclosed_name() {
            nomes.push(nome.to_string_lossy().replace('\\', "/"));
        }
    }
    nomes.sort();
    nomes.dedup();
    std::fs::write(destino.join(MANIFESTO), nomes.join("\n"))
}

/// Onde o zip foi extraído, exista ou não.
pub fn cache_de(zip: &Path) -> std::io::Result<PathBuf> {
    Ok(cache_dir().join(fingerprint(zip)?))
}

/// Escreve o manifesto de um pacote já extraído que ainda não tem um.
///
/// Os caches feitos antes de o manifesto existir não têm como saber o que era do pacote. Em vez
/// de deixá-los de fora do gerenciador de saves — ou pior, de adivinhar por data —, o manifesto é
/// reconstruído do zip, que continua ali.
pub fn completar_manifesto(zip: &Path) -> std::io::Result<()> {
    let destino = cache_de(zip)?;
    if !destino.is_dir() || destino.join(MANIFESTO).is_file() {
        return Ok(());
    }
    escrever_manifesto(zip, &destino)
}

/// Um nome de pasta que muda quando o arquivo muda.
fn fingerprint(zip: &Path) -> std::io::Result<String> {
    let meta = std::fs::metadata(zip)?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stem: String = zip
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("jogo")
        .chars()
        .map(|c| match c.is_alphanumeric() {
            true => c,
            false => '-',
        })
        .collect();
    Ok(format!("{stem}-{}-{stamp}", meta.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Escreve um zip com os caminhos dados, todos com conteúdo de brincadeira.
    fn make_zip(name: &str, entries: &[&str]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("zeebx-teste-{name}.zip"));
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for entry in entries {
            writer.start_file(*entry, options).unwrap();
            writer.write_all(entry.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    #[test]
    fn acha_o_modulo_mais_perto_da_raiz() {
        let zip = make_zip(
            "achar",
            &[
                "Peteca/mif/279159.mif",
                "Peteca/mod/279159/zeebopeteca.mod",
                "extras/demo/outro.mod",
            ],
        );
        assert_eq!(
            find_module(&zip).as_deref(),
            Some("Peteca/mod/279159/zeebopeteca.mod")
        );
        let _ = std::fs::remove_file(&zip);
    }

    #[test]
    fn um_zip_sem_modulo_nao_e_um_jogo() {
        let zip = make_zip("vazio", &["leiame.txt", "capa.png"]);
        assert_eq!(find_module(&zip), None);
        let _ = std::fs::remove_file(&zip);
    }

    #[test]
    fn o_titulo_vem_da_pasta_de_dentro_quando_ela_existe() {
        assert_eq!(
            title_of(
                Path::new("/baixados/279159.zip"),
                "Zeebo Sports Peteca/mod/279159/zeebopeteca.mod"
            ),
            "Zeebo Sports Peteca"
        );
        // Sem a pasta do título, o nome do arquivo é o que há.
        assert_eq!(
            title_of(Path::new("/baixados/peteca.zip"), "zeebopeteca.mod"),
            "peteca"
        );
    }

    #[test]
    fn extrair_devolve_o_modulo_e_reaproveita_a_segunda_vez() {
        let zip = make_zip(
            "extrair",
            &["Jogo/mif/1.mif", "Jogo/mod/1/jogo.mod", "Jogo/dados.pak"],
        );
        let module = extract(&zip).unwrap();
        assert!(module.is_file());
        assert!(module.ends_with("Jogo/mod/1/jogo.mod"));
        // Os arquivos vizinhos vêm junto: são eles que o jogo abre em tempo de execução.
        assert!(module.parent().unwrap().join("../../dados.pak").exists());

        // A segunda extração aponta para o mesmo lugar, sem refazer o trabalho.
        assert_eq!(extract(&zip).unwrap(), module);

        let _ = std::fs::remove_dir_all(cache_dir().join(fingerprint(&zip).unwrap()));
        let _ = std::fs::remove_file(&zip);
    }
}
