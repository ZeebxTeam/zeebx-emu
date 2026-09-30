//! Procura uma versão nova nas releases do GitHub.
//!
//! A pergunta é a lista de releases, e a resposta chega numa thread, para a janela não esperar a
//! rede. A tag pode vir como `0.1.0` ou `v0.1.0`.
//!
//! **Não é `releases/latest`.** O GitHub deixa pré-lançamentos de fora do `latest`, e todas as
//! releases do Zeebx saíram assim até a 0.4.1: a resposta era 404, que virava "em dia", e ninguém
//! nunca foi avisado de versão nova. Da lista, sai a maior versão que não é rascunho — e que não
//! é pré-lançamento, se o usuário desligou a opção.
//!
//! Com a feature `atualizador`, [`instala`] baixa a versão nova e troca esta cópia por ela, nos
//! pacotes que sabem ser trocados (ver [`Instalacao`]). O que é baixado vem descrito num manifesto
//! publicado na release (`latest-qt.json`, `latest-egui.json`), com a assinatura minisign de cada
//! pacote; a chave pública está em [`CHAVE_PUBLICA`], e um pacote que não confere não é instalado.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

/// Onde as releases são publicadas.
pub const REPOSITORIO: &str = "ZeebxTeam/zeebx-emu";

/// A versão deste binário.
pub const VERSAO_ATUAL: &str = env!("CARGO_PKG_VERSION");

/// Uma release mais nova que esta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lancamento {
    /// Sem o `v`: `0.2.0`.
    pub versao: String,
    /// Como está no GitHub, com ou sem o `v`: é o que monta o endereço dos arquivos da release.
    pub tag: String,
    /// A página da release, com as notas e os instaladores.
    pub pagina: String,
}

/// O que a procura respondeu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resposta {
    Nova(Lancamento),
    EmDia,
    Falhou(String),
}

/// Começa a procura numa thread. A resposta chega pelo canal, uma vez. `pre_lancamentos` diz se
/// uma release marcada como pré-lançamento conta.
pub fn procura(pre_lancamentos: bool) -> Receiver<Resposta> {
    let (envio, recebe) = mpsc::channel();
    let _ = std::thread::Builder::new()
        .name("atualizacao".into())
        .spawn(move || {
            let resposta = match consulta(pre_lancamentos) {
                Ok(Some(lancamento)) => Resposta::Nova(lancamento),
                Ok(None) => Resposta::EmDia,
                Err(erro) => Resposta::Falhou(erro),
            };
            let _ = envio.send(resposta);
        });
    recebe
}

fn consulta(pre_lancamentos: bool) -> Result<Option<Lancamento>, String> {
    let url = format!("https://api.github.com/repos/{REPOSITORIO}/releases?per_page=20");
    let agente = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut resposta = agente
        .get(&url)
        .header("User-Agent", &format!("zeebx/{VERSAO_ATUAL}"))
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|erro| erro.to_string())?;
    // Sem release nenhuma a lista vem vazia, com 200. Um 404 aqui é repositório errado, e tem de
    // aparecer como falha, não como "em dia".
    let codigo = resposta.status().as_u16();
    if codigo != 200 {
        return Err(format!("o GitHub respondeu {codigo}"));
    }
    let texto = resposta
        .body_mut()
        .read_to_string()
        .map_err(|erro| erro.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&texto).map_err(|erro| erro.to_string())?;
    Ok(escolhe(&json, VERSAO_ATUAL, pre_lancamentos))
}

/// A maior release da lista, se ela for mais nova que `atual`. Rascunho nunca conta — a API só os
/// mostra a quem tem acesso de escrita, mas o filtro não custa nada.
fn escolhe(json: &serde_json::Value, atual: &str, pre_lancamentos: bool) -> Option<Lancamento> {
    let (tag, release) = json
        .as_array()?
        .iter()
        .filter(|release| release["draft"].as_bool() != Some(true))
        .filter(|release| pre_lancamentos || release["prerelease"].as_bool() != Some(true))
        .filter_map(|release| Some((release["tag_name"].as_str()?.trim(), release)))
        .max_by_key(|(tag, _)| numeros(tag))?;
    let versao = sem_v(tag);
    if !mais_nova(versao, atual) {
        return None;
    }
    let pagina = release["html_url"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://github.com/{REPOSITORIO}/releases/tag/{tag}"));
    Some(Lancamento { versao: versao.to_string(), tag: tag.to_string(), pagina })
}

fn sem_v(tag: &str) -> &str {
    let tag = tag.trim();
    tag.strip_prefix(['v', 'V']).unwrap_or(tag)
}

/// Os números de uma versão, `1.2.3` → `[1, 2, 3]`. O que vem depois de `-` ou `+` não conta, e
/// uma parte que não é número vale zero.
fn numeros(versao: &str) -> [u64; 3] {
    let nucleo = sem_v(versao).split(['-', '+']).next().unwrap_or("");
    let mut partes = nucleo.split('.').map(|p| p.trim().parse::<u64>().unwrap_or(0));
    [
        partes.next().unwrap_or(0),
        partes.next().unwrap_or(0),
        partes.next().unwrap_or(0),
    ]
}

/// Se `candidata` é mais nova que `atual`.
pub fn mais_nova(candidata: &str, atual: &str) -> bool {
    numeros(candidata) > numeros(atual)
}

/// A chave pública minisign que assina os pacotes da release, no formato do `cargo packager`
/// (o `.key.pub` inteiro, em base64). A privada fica nos segredos do GitHub
/// (`ZEEBX_ATUALIZADOR_CHAVE`); trocar uma sem a outra faz toda atualização ser recusada.
pub const CHAVE_PUBLICA: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDAxMkMyQzgwMjQ3QUMyNTAKUldSUXdub2tnQ3dzQVFQNnpTNEt1N0txbFIvTXJYUG5IOEYxMS9WdGIzUXh0cWpFM29HK2R5L1gK";

/// Como esta cópia foi instalada, que decide se ela pode trocar a si mesma.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instalacao {
    /// Um AppImage: o arquivo é trocado no lugar.
    AppImage,
    /// O instalador NSIS do Windows: o novo roda em modo passivo e reabre o emulador.
    InstaladorWindows,
    /// Um `Zeebx.app` no macOS: o bundle é trocado inteiro.
    PacoteMac,
    /// Um gerenciador de pacotes cuida dela (Flatpak). O emulador não mexe, nem avisa: a versão
    /// nova chega pelo gerenciador, e um aviso só confundiria.
    Gerenciada,
    /// `.deb`, compilado à mão, ou qualquer outra coisa: os arquivos não são nossos para trocar
    /// (no `.deb` são do root e do dpkg). Fica só o aviso, com a página da release.
    Manual,
}

impl Instalacao {
    /// A desta cópia.
    pub fn desta() -> Self {
        let exe = std::env::current_exe().ok();
        detecta(
            cfg!(target_os = "linux"),
            cfg!(windows),
            cfg!(target_os = "macos"),
            std::env::var_os("FLATPAK_ID").is_some(),
            std::env::var_os("APPIMAGE").is_some(),
            exe.as_deref(),
        )
    }

    /// Se o botão de atualizar pode baixar e instalar, em vez de só abrir a página.
    pub fn troca_sozinha(self) -> bool {
        matches!(self, Self::AppImage | Self::InstaladorWindows | Self::PacoteMac) && cfg!(feature = "atualizador")
    }

    /// Se a procura por versão nova faz sentido aqui.
    pub fn avisa(self) -> bool {
        self != Self::Gerenciada
    }
}

fn detecta(
    linux: bool,
    windows: bool,
    mac: bool,
    flatpak: bool,
    appimage: bool,
    exe: Option<&std::path::Path>,
) -> Instalacao {
    if linux && flatpak {
        Instalacao::Gerenciada
    } else if linux && appimage {
        Instalacao::AppImage
    } else if windows && exe.and_then(|e| e.parent()).is_some_and(|d| d.join("uninstall.exe").is_file()) {
        // O `uninstall.exe` é o que o NSIS do `cargo packager` deixa ao lado do executável. Um
        // `.exe` solto, compilado à mão, não tem, e trocar o instalador por cima dele instalaria
        // uma segunda cópia em outro lugar.
        Instalacao::InstaladorWindows
    } else if mac && exe.and_then(pacote_mac).is_some() {
        Instalacao::PacoteMac
    } else {
        Instalacao::Manual
    }
}

/// O `Zeebx.app` em que o executável está, se está em um: `.../Zeebx.app/Contents/MacOS/zeebx`.
fn pacote_mac(exe: &std::path::Path) -> Option<&std::path::Path> {
    let pacote = exe.parent()?.parent()?.parent()?;
    (pacote.extension()? == "app").then_some(pacote)
}

/// Onde uma instalação em andamento está. Os frontends leem a cada quadro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Andamento {
    Baixando { baixado: u64, total: Option<u64> },
    Instalando,
    /// Instalada. Falta reabrir: ver [`reinicia`]. No Windows não se chega aqui — o instalador
    /// fecha o emulador e reabre o novo sozinho.
    Pronta,
    Falhou(String),
}

/// Uma instalação começada por [`instala`].
#[derive(Clone)]
pub struct Atualizacao(std::sync::Arc<std::sync::Mutex<Andamento>>);

impl Atualizacao {
    pub fn andamento(&self) -> Andamento {
        self.0.lock().map(|a| a.clone()).unwrap_or(Andamento::Falhou("trava envenenada".into()))
    }

    /// De 0 a 1, enquanto baixa com tamanho conhecido.
    pub fn fracao(&self) -> Option<f32> {
        match self.andamento() {
            Andamento::Baixando { baixado, total: Some(total) } if total > 0 => {
                Some((baixado as f32 / total as f32).min(1.0))
            }
            _ => None,
        }
    }

    pub fn terminou(&self) -> bool {
        matches!(self.andamento(), Andamento::Pronta | Andamento::Falhou(_))
    }
}

/// Baixa e instala `lancamento` numa thread. `manifesto` é o arquivo da release que descreve os
/// pacotes deste frontend: `latest-qt.json` ou `latest-egui.json`. Sem a feature `atualizador`
/// (o headless, que compila a mesma janela) a resposta é uma falha, e o botão nem aparece: ver
/// [`Instalacao::troca_sozinha`].
#[cfg(not(feature = "atualizador"))]
pub fn instala(_: &Lancamento, _: &str) -> Atualizacao {
    let falhou = Andamento::Falhou("compilado sem a feature `atualizador`".into());
    Atualizacao(std::sync::Arc::new(std::sync::Mutex::new(falhou)))
}

/// Baixa e instala `lancamento` numa thread. `manifesto` é o arquivo da release que descreve os
/// pacotes deste frontend: `latest-qt.json` ou `latest-egui.json`.
#[cfg(feature = "atualizador")]
pub fn instala(lancamento: &Lancamento, manifesto: &str) -> Atualizacao {
    use std::sync::{Arc, Mutex};
    let estado = Arc::new(Mutex::new(Andamento::Baixando { baixado: 0, total: None }));
    let atualizacao = Atualizacao(estado.clone());
    let url = format!(
        "https://github.com/{REPOSITORIO}/releases/download/{}/{manifesto}",
        lancamento.tag
    );
    let _ = std::thread::Builder::new()
        .name("instala-atualizacao".into())
        .spawn(move || {
            let poe = |andamento| {
                if let Ok(mut atual) = estado.lock() {
                    *atual = andamento;
                }
            };
            match baixa_e_instala(&url, &estado) {
                Ok(()) => poe(Andamento::Pronta),
                Err(erro) => poe(Andamento::Falhou(erro)),
            }
        });
    atualizacao
}

#[cfg(feature = "atualizador")]
fn baixa_e_instala(
    url: &str,
    estado: &std::sync::Arc<std::sync::Mutex<Andamento>>,
) -> Result<(), String> {
    use cargo_packager_updater::{Config, UpdaterBuilder, WindowsConfig, WindowsUpdateInstallMode};
    let config = Config {
        endpoints: vec![url.parse().map_err(|e| format!("{e}"))?],
        pubkey: CHAVE_PUBLICA.into(),
        windows: Some(WindowsConfig {
            installer_args: None,
            install_mode: Some(WindowsUpdateInstallMode::Passive),
        }),
    };
    let atual = VERSAO_ATUAL.parse().map_err(|e| format!("{e}"))?;
    // Sem `timeout`: no `reqwest` ele vale para a resposta inteira, e um AppImage de 100 MB numa
    // conexão lenta passaria de qualquer número razoável.
    let atualizador = UpdaterBuilder::new(atual, config).build().map_err(|e| e.to_string())?;
    let Some(pacote) = atualizador.check().map_err(|e| e.to_string())? else {
        return Err("o manifesto da release não traz versão mais nova".into());
    };
    let baixado = std::cell::Cell::new(0u64);
    let bytes = pacote
        .download_extended(
            |pedaco, total| {
                baixado.set(baixado.get() + pedaco as u64);
                if let Ok(mut atual) = estado.lock() {
                    *atual = Andamento::Baixando { baixado: baixado.get(), total };
                }
            },
            || {
                if let Ok(mut atual) = estado.lock() {
                    *atual = Andamento::Instalando;
                }
            },
        )
        .map_err(|e| e.to_string())?;
    // No Windows esta chamada não volta: ela abre o instalador e encerra o processo.
    pacote.install(bytes).map_err(|e| e.to_string())
}

/// Abre a versão recém-instalada e encerra esta. Só depois de [`Andamento::Pronta`]; quem chama
/// guarda o que tiver de guardar antes, porque daqui não se volta.
pub fn reinicia() -> ! {
    let exe = std::env::current_exe().ok();
    let aberta = if let Some(appimage) = std::env::var_os("APPIMAGE") {
        std::process::Command::new(appimage).spawn().is_ok()
    } else if let Some(pacote) = exe.as_deref().and_then(pacote_mac) {
        // `open -n` abre uma instância nova mesmo com esta ainda de pé.
        std::process::Command::new("open").arg("-n").arg(pacote).spawn().is_ok()
    } else {
        exe.is_some_and(|exe| std::process::Command::new(exe).spawn().is_ok())
    };
    if !aberta {
        eprintln!("não deu para abrir a versão nova; ela abre na próxima vez");
    }
    std::process::exit(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pergunta ao GitHub de verdade: `cargo test --release consulta_de_verdade -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn consulta_de_verdade() {
        println!("{:?}", consulta(true));
    }

    #[test]
    fn compara_as_versoes_com_e_sem_v() {
        assert!(mais_nova("v0.2.0", "0.1.0"));
        assert!(mais_nova("0.1.1", "0.1.0"));
        assert!(mais_nova("1.0.0", "0.9.9"));
        assert!(mais_nova("0.10.0", "0.9.0"));
        assert!(!mais_nova("v0.1.0", "0.1.0"));
        assert!(!mais_nova("0.0.9", "0.1.0"));
    }

    fn release(tag: &str, rascunho: bool, pre: bool) -> serde_json::Value {
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/{REPOSITORIO}/releases/tag/{tag}"),
            "draft": rascunho,
            "prerelease": pre,
        })
    }

    /// A instalação de ponta a ponta, contra um servidor local: `ZEEBX_TESTE_MANIFESTO` é o
    /// endereço de um manifesto com versão maior que esta, e `APPIMAGE` o arquivo a ser trocado.
    /// A assinatura tem de ter sido feita com a chave de [`CHAVE_PUBLICA`].
    #[cfg(all(feature = "atualizador", target_os = "linux"))]
    #[test]
    #[ignore]
    fn instala_de_um_servidor_local() {
        let url = std::env::var("ZEEBX_TESTE_MANIFESTO").expect("ZEEBX_TESTE_MANIFESTO");
        let estado = std::sync::Arc::new(std::sync::Mutex::new(Andamento::Instalando));
        println!("{:?}", baixa_e_instala(&url, &estado));
        println!("{:?}", estado.lock().unwrap());
    }

    #[test]
    fn detecta_a_instalacao() {
        use std::path::Path;
        let mac = Path::new("/Applications/Zeebx.app/Contents/MacOS/zeebx");
        let solto = Path::new("/usr/bin/zeebx");
        assert_eq!(detecta(true, false, false, true, true, Some(solto)), Instalacao::Gerenciada);
        assert_eq!(detecta(true, false, false, false, true, Some(solto)), Instalacao::AppImage);
        assert_eq!(detecta(true, false, false, false, false, Some(solto)), Instalacao::Manual);
        assert_eq!(detecta(false, false, true, false, false, Some(mac)), Instalacao::PacoteMac);
        assert_eq!(detecta(false, false, true, false, false, Some(solto)), Instalacao::Manual);
        // Sem `uninstall.exe` ao lado: um `.exe` solto.
        assert_eq!(
            detecta(false, true, false, false, false, Some(Path::new("C:/x/zeebx.exe"))),
            Instalacao::Manual
        );
    }

    #[test]
    fn escolhe_a_maior_da_lista_mesmo_em_pre_lancamento() {
        // A ordem da API é por data, não por versão: a maior pode não vir primeiro.
        let lista = serde_json::json!([
            release("v0.4.0", false, true),
            release("v0.4.1", false, true),
            release("v0.3.0", false, true),
        ]);
        assert_eq!(
            escolhe(&lista, "0.4.0", true),
            Some(Lancamento {
                versao: "0.4.1".into(),
                tag: "v0.4.1".into(),
                pagina: format!("https://github.com/{REPOSITORIO}/releases/tag/v0.4.1"),
            })
        );
        assert_eq!(escolhe(&lista, "0.4.1", true), None);
    }

    #[test]
    fn sem_pre_lancamento_fica_a_ultima_estavel() {
        let lista = serde_json::json!([
            release("v0.5.0", false, true),
            release("v0.4.2", false, false),
        ]);
        assert_eq!(escolhe(&lista, "0.4.1", false).map(|l| l.versao), Some("0.4.2".into()));
        assert_eq!(escolhe(&lista, "0.4.1", true).map(|l| l.versao), Some("0.5.0".into()));
        assert_eq!(escolhe(&lista, "0.4.2", false), None);
    }

    #[test]
    fn ignora_rascunho_e_lista_vazia() {
        let lista = serde_json::json!([
            release("v0.5.0", true, false),
            release("v0.4.1", false, true),
        ]);
        assert_eq!(escolhe(&lista, "0.4.1", true), None);
        assert_eq!(escolhe(&serde_json::json!([]), "0.1.0", true), None);
    }
}
