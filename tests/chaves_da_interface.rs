//! Toda chave de texto que a interface usa existe no catálogo.
//!
//! Uma chave que falta não quebra nada: o `Catalog` a devolve como ela mesma, e a tela mostra
//! `saves.confirm.yes` no lugar de "Excluir". O teste de `i18n.rs` confere que os idiomas têm as
//! mesmas chaves entre si; este confere o outro lado, que o código só pede chaves que existem.
//!
//! No QML as chaves chegam ao `tr()` por muitos caminhos — direto, num `rotulo:`, numa lista de
//! `model`, num ternário —, então vale todo literal com cara de chave, menos os das configurações
//! (`chave:`, `v()`, `define()`, `opcoes()`), que também têm pontos. No Rust vale o primeiro
//! argumento das chamadas ao catálogo, só onde há interface: `src/qt/`, `src/ui/` e o frontend
//! Android. Fora dali, um `.get("a.b")` é de outra coisa, e o `i18n.rs` pede uma chave que não
//! existe de propósito.
//!
//! Mora no pacote raiz, e não no do standalone, que é onde está o QML: com a interface Qt ligada,
//! o cxx-qt liga o módulo QML em todo alvo daquele pacote, e um teste de integração não tem as
//! pontes em Rust, que são do binário — não linkava.

use std::path::{Path, PathBuf};

use zeebx::ui::i18n::Catalog;

/// `settings.discord.on`: minúsculas, dígitos e `_`, com ao menos um ponto. Nome de arquivo não.
fn tem_cara_de_chave(texto: &str) -> bool {
    const ARQUIVOS: [&str; 7] = [".h", ".png", ".log", ".qml", ".json", ".rs", ".svg"];
    texto.contains('.')
        && !texto.starts_with('.')
        && !texto.ends_with('.')
        && !texto.contains("..")
        && texto.starts_with(|c: char| c.is_ascii_lowercase())
        && texto
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
        && !ARQUIVOS.iter().any(|ext| texto.ends_with(ext))
}

/// Os literais entre aspas de uma linha.
fn literais(linha: &str) -> Vec<&str> {
    linha.split('"').skip(1).step_by(2).collect()
}

fn arquivos(pasta: &Path, extensao: &str, achados: &mut Vec<PathBuf>) {
    let Ok(entradas) = std::fs::read_dir(pasta) else {
        return;
    };
    for entrada in entradas.flatten() {
        let caminho = entrada.path();
        if caminho.is_dir() {
            arquivos(&caminho, extensao, achados);
        } else if caminho.extension().is_some_and(|e| e == extensao) {
            achados.push(caminho);
        }
    }
}

/// As chaves pedidas pelo QML, com o arquivo e a linha.
fn chaves_do_qml(pasta: &Path) -> Vec<(String, String)> {
    const DAS_CONFIGURACOES: [&str; 5] = ["chave:", "v(", "define(", "opcoes(", "valor("];
    let mut qml = Vec::new();
    arquivos(pasta, "qml", &mut qml);
    let mut chaves = Vec::new();
    for arquivo in qml {
        let texto = std::fs::read_to_string(&arquivo).unwrap();
        for (n, linha) in texto.lines().enumerate() {
            if DAS_CONFIGURACOES.iter().any(|c| linha.contains(c)) {
                continue;
            }
            for literal in literais(linha).into_iter().filter(|l| tem_cara_de_chave(l)) {
                chaves.push((literal.to_string(), format!("{}:{}", arquivo.display(), n + 1)));
            }
        }
    }
    chaves
}

/// As chaves pedidas pelo Rust ao catálogo, com o arquivo e a linha.
fn chaves_do_rust(pasta: &Path) -> Vec<(String, String)> {
    const CHAMADAS: [&str; 4] = [".get(\"", ".format(\"", ".tr(\"", "texto(\""];
    let mut rust = Vec::new();
    arquivos(pasta, "rs", &mut rust);
    let mut chaves = Vec::new();
    for arquivo in rust {
        if arquivo.ends_with("i18n.rs") {
            continue;
        }
        let texto = std::fs::read_to_string(&arquivo).unwrap();
        for (n, linha) in texto.lines().enumerate() {
            for chamada in CHAMADAS {
                for (i, _) in linha.match_indices(chamada) {
                    let resto = &linha[i + chamada.len()..];
                    let Some(fim) = resto.find('"') else { continue };
                    if tem_cara_de_chave(&resto[..fim]) {
                        chaves.push((
                            resto[..fim].to_string(),
                            format!("{}:{}", arquivo.display(), n + 1),
                        ));
                    }
                }
            }
        }
    }
    chaves
}

#[test]
fn toda_chave_usada_pela_interface_existe_no_catalogo() {
    let raiz = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut chaves = chaves_do_qml(&raiz.join("frontends/classical-standalone/qml"));
    chaves.extend(chaves_do_rust(&raiz.join("frontends/classical-standalone/src")));
    chaves.extend(chaves_do_rust(&raiz.join("src/ui")));
    chaves.extend(chaves_do_rust(&raiz.join("frontends/android/src")));
    // Sem nada achado, o teste passaria por não olhar: a pasta mudou de lugar, ou a extração
    // quebrou.
    assert!(chaves.len() > 100, "só {} chaves achadas", chaves.len());

    let catalogo = Catalog::default();
    let faltando: Vec<String> = chaves
        .iter()
        .filter(|(chave, _)| catalogo.get(chave) == chave)
        .map(|(chave, onde)| format!("{onde}: {chave}"))
        .collect();
    assert!(faltando.is_empty(), "chaves fora do catálogo:\n{}", faltando.join("\n"));
}

#[test]
fn a_extracao_reconhece_chave_e_ignora_o_resto() {
    assert!(tem_cara_de_chave("saves.confirm.yes"));
    assert!(tem_cara_de_chave("settings.z_wheel_eol"));
    assert!(!tem_cara_de_chave("zeebx.log"));
    assert!(!tem_cara_de_chave("quadro.h"));
    assert!(!tem_cara_de_chave("Zeebx"));
    assert!(!tem_cara_de_chave("qrc:/zeebx/boomerang.png"));
    assert_eq!(literais(r#"tr(a ? "x.y" : "z.w")"#), ["x.y", "z.w"]);
}
