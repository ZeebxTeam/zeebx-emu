//! Jogos guardados em `.zip`.
//!
//! Um título do Zeebo é uma árvore de arquivos — o `.mod`, o `.mif` e os dados —, e é comum
//! ela circular compactada. O emulador não lê de dentro do arquivo: ele extrai para uma pasta
//! de cache e roda dali. Os jogos gravam (o Peteca tem um `.sav`), e escrever de volta num zip
//! não é coisa que se queira fazer; extrair resolve isso de graça e deixa o resto do emulador
//! sem saber que o zip existe.

use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config;
use crate::loader::miffile::MifFile;
use crate::loader::sete_z;
use crate::storage::ContentId;

/// Limites antes de descompactar conteúdo não confiável. São generosos para não rejeitar jogos
/// legítimos, mas impedem que um ZIP com metadados maliciosos consuma espaço/memória sem teto.
pub(crate) const MAX_ENTRIES: usize = 20_000;
pub(crate) const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Um `.mif` é metadado pequeno; teto próprio evita alocar centenas de MiB só para escolher módulo.
pub(crate) const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct ArchiveLimits {
    pub(crate) entries: usize,
    pub(crate) file_bytes: u64,
    pub(crate) total_bytes: u64,
}

impl ArchiveLimits {
    /// Os limites de sempre, para quem precisa extrair fora do caminho de `extract_in`.
    #[cfg(test)]
    pub(crate) fn padrao() -> Self {
        ARCHIVE_LIMITS
    }
}

const ARCHIVE_LIMITS: ArchiveLimits = ArchiveLimits {
    entries: MAX_ENTRIES,
    file_bytes: MAX_FILE_BYTES,
    total_bytes: MAX_TOTAL_BYTES,
};

/// Impede que duas aberturas no mesmo processo apaguem o diretório parcial uma da outra.
static NEXT_PARTIAL: AtomicU64 = AtomicU64::new(0);

fn partial_dir(target: &Path) -> PathBuf {
    let serial = NEXT_PARTIAL.fetch_add(1, Ordering::Relaxed);
    target.with_extension(format!("partial-{}-{serial}", std::process::id()))
}

/// Confere metadados antes de ler qualquer payload. `enclosed_name` é conferido outra vez no
/// ponto de extração, porque é ele que decide o caminho de saída.
fn validate_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
    limits: ArchiveLimits,
) -> std::io::Result<()> {
    if archive.len() > limits.entries {
        return Err(std::io::Error::other("o zip tem entradas demais"));
    }
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(std::io::Error::other)?;
        if entry.enclosed_name().is_none() {
            return Err(std::io::Error::other("o zip contém caminho inseguro"));
        }
        if entry.is_symlink() {
            return Err(std::io::Error::other("o zip contém link simbólico"));
        }
        if entry.is_dir() {
            continue;
        }
        if entry.size() > limits.file_bytes {
            return Err(std::io::Error::other("o zip contém arquivo grande demais"));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| std::io::Error::other("o zip tem tamanho inválido"))?;
        if total > limits.total_bytes {
            return Err(std::io::Error::other("o zip descompacta bytes demais"));
        }
    }
    Ok(())
}
/// Onde as extrações ficam.
pub fn cache_dir() -> PathBuf {
    config::config_dir().join("cache")
}

/// A raiz do sistema de arquivos do aparelho, comum a todos os jogos.
///
/// No console há **um** sistema de arquivos: o Zeeboids grava os bonecos em
/// `fs:/zeeboiddata/zeeboid.db` e o Zeebo F.C. abre esse mesmo caminho para importá-los. Uma
/// raiz por instalação faria cada jogo ver uma pasta só sua, e o F.C. concluiria — com razão,
/// do ponto de vista dele — que o Zeeboids não está instalado.
pub fn device_dir() -> PathBuf {
    config::config_dir().join("aparelho")
}

/// A fonte do sistema, no lugar em que o console a guarda: `fs:/shared/fonts/tectoy.ttf`.
///
/// O caminho está no firmware. É a fonte com que o `IDisplay` escreve quando o jogo pede uma
/// fonte do aparelho (`AEE_FONT_NORMAL` e as irmãs) — e a maioria dos jogos não traz fonte
/// própria, porque no console não precisava.
///
/// O arquivo **não** vem com o emulador: é da TecToy. Mas ele vem no pacote da Z-Wheel, que é o
/// sistema do console, então quando o aparelho ainda não o tem e a Z-Wheel já foi aberta alguma
/// vez, ele é instalado a partir dela — o mesmo que a Z-Wheel fazia no console.
pub fn fonte_do_sistema() -> Option<PathBuf> {
    fonte_do_sistema_em(&cache_dir(), &device_dir())
}

/// Como [`fonte_do_sistema`], mas com o cache e a raiz do aparelho que o frontend forneceu.
///
/// A versão sem argumentos é o caminho do desktop, que lê a configuração do usuário. Um frontend
/// como o Libretro extrai o pacote no **cache dele** e escreve o `fs:/` na raiz dele: procurar no
/// lugar do desktop devolvia `None` mesmo com o `tectoy.ttf` dentro do pacote da Z-Wheel — e sem
/// fonte o jogo não desenha texto nenhum, que foi o caso do Double Dragon.
pub fn fonte_do_sistema_em(cache: &Path, device: &Path) -> Option<PathBuf> {
    let destino = device.join("shared").join("fonts").join("tectoy.ttf");
    if destino.is_file() {
        return Some(destino);
    }
    let origem = std::fs::read_dir(cache)
        .ok()?
        .filter_map(Result::ok)
        .flat_map(|pacote| {
            std::fs::read_dir(pacote.path().join("mod"))
                .into_iter()
                .flatten()
                .flatten()
        })
        .map(|modulo| modulo.path().join("tectoy.ttf"))
        .find(|ttf| ttf.is_file())?;
    std::fs::create_dir_all(destino.parent()?).ok()?;
    std::fs::copy(origem, &destino).ok()?;
    Some(destino)
}

/// Instala o `tectoy.ttf` que vem **dentro do pacote da Z-Wheel** na raiz do aparelho.
///
/// É o caminho para quem abre um jogo sem nunca ter aberto a Z-Wheel: a fonte do sistema mora no
/// pacote dela, e sem fonte o jogo não desenha texto do sistema — o relatório do Double Dragon
/// listava "texto na tela (ainda sem fonte para desenhar)".
///
/// **Não é a causa da tela branca dele.** Medido: com a fonte instalada, o quadro do Double Dragon
/// no core continua branco e uniforme. O que falta ali é outra coisa, no caminho Dynarmic/`Session`,
/// porque o mesmo jogo desenha pelo caminho Unicorn da linha de comando.
///
/// Extrai **só** o arquivo da fonte: não vale materializar o pacote inteiro por causa de 190 KB.
pub fn instala_fonte_do_pacote(pacote: &Path, device: &Path) -> Option<PathBuf> {
    let destino = device.join("shared").join("fonts").join("tectoy.ttf");
    if destino.is_file() {
        return Some(destino);
    }
    // Duas formas de Z-Wheel aparecem no mesmo acervo: o `.zip` do pacote e uma cópia já extraída,
    // em que a fonte fica **ao lado** do módulo (`mod/274755/tectoy.ttf`). Tratar só o zip deixava
    // o acervo extraído sem fonte.
    let bytes = if sete_z::eh_sete_z(pacote) {
        // No `.7z` a fonte é procurada pelo nome, e o formato é decidido pela assinatura: o
        // pacote do acervo pode ter sido renomeado.
        let nome = sete_z::listar(pacote)?
            .into_iter()
            .map(|(nome, _)| nome)
            .find(|nome| nome.to_ascii_lowercase().ends_with(".ttf"))?;
        sete_z::ler(pacote, &nome, MAX_MANIFEST_BYTES)?
    } else {
        match pacote.extension().and_then(|e| e.to_str()) {
        Some("zip") => {
            let file = std::fs::File::open(pacote).ok()?;
            let mut archive = zip::ZipArchive::new(file).ok()?;
            let indice = (0..archive.len()).find(|&i| {
                archive.by_index(i).is_ok_and(|entrada| {
                    entrada.name().replace('\\', "/").ends_with("/tectoy.ttf")
                })
            })?;
            let mut entrada = archive.by_index(indice).ok()?;
            let mut bytes = Vec::new();
            entrada.read_to_end(&mut bytes).ok()?;
            bytes
        }
        _ => std::fs::read(pacote.parent()?.join("tectoy.ttf")).ok()?,
        }
    };
    std::fs::create_dir_all(destino.parent()?).ok()?;
    std::fs::write(&destino, bytes).ok()?;
    Some(destino)
}

/// O arquivo é um pacote: `.zip` ou `.7z`.
///
/// **A extensão não decide o formato** — quem decide é a assinatura, no momento de abrir —, mas
/// ela decide se vale tratar o arquivo como pacote, e é essa a pergunta que o resto do emulador
/// faz. Um título circula das duas formas, e recusar o `.7z` aqui dava o pior sintoma possível:
/// o jogo simplesmente "não carrega", sem dizer por quê.
pub fn embalado(caminho: &Path) -> bool {
    matches!(
        caminho
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("zip" | "7z")
    )
}

/// O caminho interno do `.mod` dentro do pacote, se houver um.
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
pub fn find_module(pacote: &Path) -> Option<String> {
    let nomes = nomes_do_pacote(pacote)?;
    // O manifesto só é lido quando há mais de um candidato: com um só, a resposta é ele de
    // qualquer jeito, e abrir o pacote de novo seria trabalho à toa em todo jogo do acervo.
    escolhe_modulo(nomes, |nome| {
        find_manifest(pacote, nome)
            .and_then(|dados| MifFile::parse(&dados).ok())
            .and_then(|mif| mif.main_applet())
            .is_some()
    })
}

/// Os nomes de dentro do pacote, seja ele `.zip` ou `.7z`.
///
/// É a única pergunta que o resto do arquivo faz sobre o formato: daqui para baixo, zip e 7z são
/// o mesmo pacote. O `.7z` é reconhecido pela assinatura, e não pela extensão — um `.7z`
/// renomeado para `.zip` continua sendo um `.7z`.
fn nomes_do_pacote(pacote: &Path) -> Option<Vec<String>> {
    if sete_z::eh_sete_z(pacote) {
        return sete_z::listar(pacote).map(|lista| lista.into_iter().map(|(nome, _)| nome).collect());
    }
    let arquivo = std::fs::File::open(pacote).ok()?;
    let mut zip = zip::ZipArchive::new(arquivo).ok()?;
    validate_archive(&mut zip, ARCHIVE_LIMITS).ok()?;
    let mut nomes = Vec::with_capacity(zip.len());
    for indice in 0..zip.len() {
        let entrada = zip.by_index(indice).ok()?;
        // `enclosed_name` recusa caminhos com `..` ou raiz absoluta, que é como um zip
        // malicioso escreveria fora da pasta de destino.
        entrada.enclosed_name()?;
        nomes.push(entrada.name().to_string());
    }
    Some(nomes)
}

/// Escolhe o `.mod`, na ordem documentada em [`find_module`].
///
/// Está separado porque a escolha é a mesma para zip e para 7z — o que muda é quem lista os
/// nomes. Duas cópias desta regra divergiriam no primeiro ajuste.
fn escolhe_modulo(
    nomes: Vec<String>,
    com_applet: impl Fn(&str) -> bool,
) -> Option<String> {
    let mut candidatos: Vec<(bool, usize, String)> = Vec::new();
    for nome in nomes {
        if Path::new(&nome).extension().and_then(|e| e.to_str()) != Some("mod") {
            continue;
        }
        let partes: Vec<&str> = nome.split(['/', '\\']).collect();
        let console = partes.len() >= 4 && partes[partes.len() - 3] == "mod";
        candidatos.push((console, partes.len(), nome));
    }
    let sozinho = candidatos.len() <= 1;
    candidatos
        .into_iter()
        .max_by_key(|(console, profundidade, nome)| {
            (sozinho || com_applet(nome), *console, std::cmp::Reverse(*profundidade))
        })
        .map(|(_, _, nome)| nome)
}

/// O conteúdo do `.mif` que acompanha `module` dentro do zip.
///
/// O `.mif` do título fica em `<Título>/mif/<id>.mif`, irmão da pasta `mod/`. Havendo mais de
/// um, vale o que combina com o identificador do módulo; sem isso, o primeiro serve.
pub fn find_manifest(pacote: &Path, module: &str) -> Option<Vec<u8>> {
    // `<Título>/mod/<id>/x.mod` -> o identificador é a pasta que contém o módulo.
    let partes: Vec<&str> = module.split('/').collect();
    let id = partes.get(partes.len().checked_sub(2)?).copied();

    if sete_z::eh_sete_z(pacote) {
        let lista = sete_z::listar(pacote)?;
        let nome = escolhe_mif(lista.into_iter().filter(|(_, pasta)| !pasta).map(|(nome, _)| nome), id)?;
        return sete_z::ler(pacote, &nome, MAX_MANIFEST_BYTES);
    }

    let arquivo = std::fs::File::open(pacote).ok()?;
    let mut zip = zip::ZipArchive::new(arquivo).ok()?;
    validate_archive(&mut zip, ARCHIVE_LIMITS).ok()?;
    let mut nomes = Vec::with_capacity(zip.len());
    for indice in 0..zip.len() {
        let entrada = zip.by_index(indice).ok()?;
        entrada.enclosed_name()?;
        nomes.push(entrada.name().to_string());
    }
    let nome = escolhe_mif(nomes.into_iter(), id)?;
    let mut entrada = zip.by_name(&nome).ok()?;
    if entrada.size() > MAX_MANIFEST_BYTES {
        return None;
    }
    let mut dados = Vec::with_capacity(entrada.size() as usize);
    entrada
        .by_ref()
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut dados)
        .ok()?;
    (dados.len() as u64 <= MAX_MANIFEST_BYTES).then_some(dados)
}

/// Escolhe o `.mif` do título entre os nomes do pacote.
///
/// Vale o que combina com o identificador do módulo; sem combinação, o primeiro serve — o mesmo
/// critério para zip e para 7z.
fn escolhe_mif(nomes: impl Iterator<Item = String>, id: Option<&str>) -> Option<String> {
    let mut primeiro: Option<String> = None;
    for nome in nomes {
        if !nome.ends_with(".mif") {
            continue;
        }
        if id.is_some_and(|id| nome.ends_with(&format!("/{id}.mif"))) {
            return Some(nome);
        }
        primeiro.get_or_insert(nome);
    }
    primeiro
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
/// A pasta de destino é nomeada pelo hash BLAKE3 dos bytes do arquivo: trocar o conteúdo gera
/// outra pasta, e cópia idêntica reaproveita a extração anterior sem confiar em tamanho/data.
pub fn extract(zip: &Path) -> std::io::Result<PathBuf> {
    extract_in(zip, &cache_dir())
}

/// Como [`extract`], mas com cache explícito para o frontend que controla sua raiz persistente.
pub fn extract_in(zip: &Path, cache: &Path) -> std::io::Result<PathBuf> {
    // A seleção e a extração ainda precisam abrir o arquivo mais de uma vez. Conferir o digest
    // entre essas fases recusa a troca da ROM no meio da operação, em vez de pôr bytes diferentes
    // sob uma chave de conteúdo errada.
    let source_fingerprint = fingerprint(zip)?;
    let module = find_module(zip)
        .ok_or_else(|| std::io::Error::other("o pacote não contém nenhum arquivo .mod"))?;
    if fingerprint(zip)? != source_fingerprint {
        return Err(std::io::Error::other("o zip mudou enquanto era analisado"));
    }
    let target = cache.join(&source_fingerprint);
    let extracted = target.join(&module);
    // Já extraído com identidade forte: nada a fazer.
    if extracted.is_file() {
        return Ok(extracted);
    }
    // A versão anterior usava nome+tamanho+mtime. Mantemos o cache antigo como leitura
    // transitória para não esconder saves que ainda vivem dentro dele; nunca escrevemos conteúdo
    // novo ali, pois aquela chave podia colidir.
    let legacy = cache.join(legacy_fingerprint(zip)?).join(&module);
    if legacy.is_file() {
        return Ok(legacy);
    }

    // Nunca deixamos um cache parcialmente extraído parecer utilizável. Uma queda no meio da
    // cópia fica em `.partial`, que a próxima abertura remove antes de tentar de novo.
    let partial = partial_dir(&target);
    let result = (|| -> std::io::Result<()> {
        // O `.7z` descompacta por caminho próprio, e **daqui para baixo o caminho é o mesmo**:
        // conferir o digest, escrever o manifesto, exigir a presença do `.mod` e publicar a
        // extração com um renomeio. Sair cedo daqui deixava o conteúdo numa pasta `.partial` e
        // devolvia um caminho que não existia — foi o que aconteceu na primeira versão.
        if sete_z::eh_sete_z(zip) {
            std::fs::create_dir_all(&partial)?;
            sete_z::extrair(zip, &partial, ARCHIVE_LIMITS)?;
        } else {
        let file = std::fs::File::open(zip)?;
        let mut archive = zip::ZipArchive::new(file).map_err(std::io::Error::other)?;
        validate_archive(&mut archive, ARCHIVE_LIMITS)?;
        let mut extracted_bytes = 0u64;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(std::io::Error::other)?;
            let Some(name) = entry.enclosed_name() else {
                continue;
            };
            let out = partial.join(name);
            if entry.is_dir() {
                std::fs::create_dir_all(&out)?;
                continue;
            }
            if let Some(dir) = out.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Não aloca um Vec do tamanho declarado pelo ZIP. Além da pré-checagem de
            // metadados, limita o fluxo real: um cabeçalho mentiroso não pode escrever além do
            // teto durante a descompressão.
            let remaining = MAX_TOTAL_BYTES
                .checked_sub(extracted_bytes)
                .ok_or_else(|| std::io::Error::other("o zip descompacta bytes demais"))?;
            let allowed = MAX_FILE_BYTES.min(remaining);
            let mut output = std::fs::File::create(&out)?;
            let written = std::io::copy(&mut entry.by_ref().take(allowed + 1), &mut output)?;
            if written > allowed {
                return Err(std::io::Error::other(
                    "o zip ultrapassou o limite ao descompactar",
                ));
            }
            extracted_bytes = extracted_bytes
                .checked_add(written)
                .ok_or_else(|| std::io::Error::other("o zip tem tamanho inválido"))?;
        }
        }
        if fingerprint(zip)? != source_fingerprint {
            return Err(std::io::Error::other("o pacote mudou durante a extração"));
        }
        escrever_manifesto(zip, &partial)?;
        if fingerprint(zip)? != source_fingerprint {
            return Err(std::io::Error::other("o pacote mudou durante a extração"));
        }
        if !partial.join(&module).is_file() {
            return Err(std::io::Error::other(
                "o .mod não apareceu depois de extrair o pacote",
            ));
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Err(error) = std::fs::rename(&partial, &target) {
            // Outro processo pode ter publicado o mesmo hash primeiro. Se a publicação dele é
            // completa, ela é equivalente à nossa; caso contrário, não escondemos o erro.
            if target.join(&module).is_file() {
                let _ = std::fs::remove_dir_all(&partial);
            } else {
                return Err(error);
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&partial);
    }
    result?;
    Ok(extracted)
}

/// Teto do cache de conteúdo extraído, em bytes.
///
/// Sem ele, cada jogo aberto deixa a própria extração para sempre: um acervo de teste de 62
/// títulos passa de 1,5 GB e enche o disco de quem só queria jogar. O valor é generoso para o
/// jogo em uso e apertado para o acervo inteiro.
pub const CACHE_LIMIT_BYTES: u64 = 512 * 1024 * 1024;

/// O nome do arquivo que lista o que veio do pacote.
pub const MANIFESTO: &str = ".zeebx-pacote";

/// Apaga extrações antigas até o cache caber em `limite`, preservando a que contém `manter`.
///
/// `manter` é um caminho **dentro** da extração em uso — o `.mod`, por exemplo. A entrada que o
/// contém nunca é removida, nem que sozinha estoure o teto: apagar o jogo em execução seria pior
/// que o disco cheio. As outras saem da mais antiga para a mais nova, pela data de modificação.
///
/// Devolve quantos bytes foram liberados.
pub fn prune_cache(cache: &Path, manter: Option<&Path>, limite: u64) -> std::io::Result<u64> {
    let Ok(entradas) = std::fs::read_dir(cache) else {
        // Sem cache não há o que podar.
        return Ok(0);
    };
    let mut itens: Vec<(std::time::SystemTime, PathBuf, u64)> = Vec::new();
    for entrada in entradas.flatten() {
        let caminho = entrada.path();
        if !caminho.is_dir() {
            continue;
        }
        if manter.is_some_and(|manter| manter.starts_with(&caminho)) {
            continue;
        }
        let idade = entrada
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        itens.push((idade, caminho, tamanho_em_disco(&entrada.path())));
    }
    let mut total: u64 = itens.iter().map(|(_, _, bytes)| bytes).sum();
    // A entrada em uso já está fora da soma; o teto vale para o que pode sair.
    itens.sort_by_key(|(idade, _, _)| *idade);
    let mut liberado = 0u64;
    for (_, caminho, bytes) in itens {
        if total <= limite {
            break;
        }
        if std::fs::remove_dir_all(&caminho).is_ok() {
            total = total.saturating_sub(bytes);
            liberado += bytes;
        }
    }
    Ok(liberado)
}

/// Tamanho de uma árvore de diretórios, em bytes de arquivo.
fn tamanho_em_disco(caminho: &Path) -> u64 {
    let Ok(entradas) = std::fs::read_dir(caminho) else {
        return 0;
    };
    entradas
        .flatten()
        .map(|entrada| match entrada.file_type() {
            Ok(tipo) if tipo.is_dir() => tamanho_em_disco(&entrada.path()),
            Ok(_) => entrada.metadata().map(|meta| meta.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// Grava, dentro do cache, a lista do que o zip trouxe.
///
/// Existe para o gerenciador de saves poder dizer, **sem adivinhar**, o que é save e o que é
/// conteúdo do jogo. A primeira tentativa comparava datas — o que é mais novo que o `.mod` seria
/// save — e ela cai: os arquivos de uma mesma extração diferem por milissegundos, e o
/// `resources.pakz` de 15 MB do Alice terminava de ser escrito depois do `.mod`. Numa lista com
/// botão de excluir, errar assim apaga o jogo.
///
/// A lista vem do zip, que é a fonte: o que está nele é do pacote, o resto o jogo escreveu.
pub fn escrever_manifesto(pacote: &Path, destino: &Path) -> std::io::Result<()> {
    // Os nomes saem do mesmo lugar que a escolha do módulo, então o manifesto cobre zip e 7z sem
    // caminho separado. Ele é o que diz ao motor quais arquivos vieram do pacote — sem ele, um
    // `.7z` extraído trataria o conteúdo como se fosse do aparelho.
    let mut nomes: Vec<String> = nomes_do_pacote(pacote).unwrap_or_default();
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
    if destino.is_dir() {
        if !destino.join(MANIFESTO).is_file() {
            escrever_manifesto(zip, &destino)?;
        }
        return Ok(());
    }
    // Ver `extract`: caches antigos podiam conter saves relativos. Ainda que a chave velha não
    // seja usada para conteúdo novo, reconstituir seu manifesto mantém esses saves visíveis na UI.
    let legado = cache_dir().join(legacy_fingerprint(zip)?);
    if legado.is_dir() && !legado.join(MANIFESTO).is_file() {
        escrever_manifesto(zip, &legado)?;
    }
    Ok(())
}

/// Um nome de pasta que identifica o conteúdo, não onde/quando ele foi copiado.
///
/// O título é só rótulo humano para o gerenciador de saves; a chave que impede colisão é o digest
/// BLAKE3 completo no fim. Duas cópias com o mesmo nome e bytes vão para o mesmo cache.
fn fingerprint(zip: &Path) -> std::io::Result<String> {
    let content = ContentId::from_reader(std::fs::File::open(zip)?)?;
    Ok(format!("{}-{}", cache_label(zip), content.as_str()))
}

fn cache_label(zip: &Path) -> String {
    zip.file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("jogo")
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// A chave de cache anterior, só para manter leitura de saves/cache já criados pela versão velha.
fn legacy_fingerprint(zip: &Path) -> std::io::Result<String> {
    let meta = std::fs::metadata(zip)?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    Ok(format!("{}-{}-{stamp}", cache_label(zip), meta.len()))
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
    fn fingerprint_e_dos_bytes_nao_de_data() {
        let first = make_zip("hash-origem", &["mod/1/jogo.mod"]);
        let dir = std::env::temp_dir().join("zeebx-teste-hash-copia");
        std::fs::create_dir_all(&dir).unwrap();
        let second = dir.join("zeebx-teste-hash-origem.zip");
        std::fs::copy(&first, &second).unwrap();
        assert_eq!(fingerprint(&first).unwrap(), fingerprint(&second).unwrap());
        let _ = std::fs::remove_file(first);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn limite_de_entradas_e_recusado_antes_da_extracao() {
        let zip = make_zip("limite", &["a", "b", "c"]);
        let file = std::fs::File::open(&zip).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let err = validate_archive(
            &mut archive,
            ArchiveLimits {
                entries: 2,
                file_bytes: MAX_FILE_BYTES,
                total_bytes: MAX_TOTAL_BYTES,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("entradas"));
        let _ = std::fs::remove_file(zip);
    }

    #[test]
    fn a_poda_larga_o_cache_antigo_e_preserva_o_jogo_em_uso() {
        let raiz = std::env::temp_dir().join("zeebx-teste-poda");
        let _ = std::fs::remove_dir_all(&raiz);
        let cache = raiz.join("cache");
        let antigo = cache.join("jogo-antigo");
        let em_uso = cache.join("jogo-em-uso");
        for (pasta, tamanho) in [(&antigo, 4096usize), (&em_uso, 4096usize)] {
            std::fs::create_dir_all(pasta).unwrap();
            std::fs::write(pasta.join("dados.bin"), vec![0u8; tamanho]).unwrap();
        }
        let modulo = em_uso.join("mod/1/jogo.mod");
        std::fs::create_dir_all(modulo.parent().unwrap()).unwrap();
        std::fs::write(&modulo, b"mod").unwrap();

        // Teto zero: só o jogo em uso sobrevive.
        let liberado = prune_cache(&cache, Some(&modulo), 0).unwrap();
        assert!(liberado > 0, "deveria ter liberado espaço");
        assert!(!antigo.exists(), "a extração antiga sai");
        assert!(modulo.is_file(), "a extração em uso fica");

        // Sem nada acima do teto, ninguém é tocado.
        assert_eq!(prune_cache(&cache, Some(&modulo), u64::MAX).unwrap(), 0);

        let _ = std::fs::remove_dir_all(&raiz);
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
