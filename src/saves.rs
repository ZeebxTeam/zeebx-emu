//! O que os jogos escreveram, e como apagar.
//!
//! Um jogo do Zeebo grava no meio dos próprios arquivos: o Z-Wheel põe o `tt_prefs.db` ao lado
//! do `tectoy.mod`, e o Zeeboids grava fora, em `fs:/zeeboiddata`, que é do **aparelho** e não
//! dele — o Zeebo F.C. lê os bonecos de lá. Por isso a lista tem dois lados, e apagar um não
//! toca no outro.
//!
//! # Como se distingue save de conteúdo do pacote
//!
//! Pela data. O pacote é extraído de uma vez, então todos os arquivos dele nascem com o mesmo
//! instante; o que o jogo escreve depois é mais novo que o `.mod`. Não é adivinhação: no cache
//! do Zeeboids o `.mod`, o `.sig` e o `resources.pakz` são todos de `09-07 16:59`, e a pasta
//! `zeeboiddata` que o jogo criou é de dois dias depois.
//!
//! O `.mod` é a referência, e não a pasta: a data da pasta muda a cada arquivo que se cria
//! dentro dela, então ela seria sempre "mais nova que ela mesma".

use std::path::{Path, PathBuf};

/// Um save: de um jogo, ou do aparelho.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    /// O que mostrar — o nome do jogo, ou o da pasta do aparelho.
    pub titulo: String,
    /// O que apagar. Mais de um quando o jogo espalhou.
    pub itens: Vec<PathBuf>,
    /// Quantos arquivos ao todo, contando o que está dentro das pastas.
    pub arquivos: usize,
    /// O tamanho somado.
    pub bytes: u64,
}

impl Save {
    fn de(titulo: String, itens: Vec<PathBuf>) -> Option<Self> {
        if itens.is_empty() {
            return None;
        }
        let (arquivos, bytes) = itens.iter().map(pesar).fold((0, 0), |(a, b), (c, d)| (a + c, b + d));
        Some(Self {
            titulo,
            itens,
            arquivos,
            bytes,
        })
    }
}

/// Quantos arquivos e quantos bytes há em `caminho`, entrando nas pastas.
fn pesar(caminho: &PathBuf) -> (usize, u64) {
    let Ok(meta) = std::fs::symlink_metadata(caminho) else {
        return (0, 0);
    };
    if !meta.is_dir() {
        return (1, meta.len());
    }
    let Ok(entradas) = std::fs::read_dir(caminho) else {
        return (0, 0);
    };
    entradas
        .flatten()
        .map(|e| pesar(&e.path()))
        .fold((0, 0), |(a, b), (c, d)| (a + c, b + d))
}

/// O que o pacote trouxe, lido do manifesto que a extração grava.
///
/// **Sem manifesto não há lista**: um cache antigo é ignorado, e não chutado. A primeira versão
/// disto comparava datas — o que fosse mais novo que o `.mod` seria save — e ela cai: os
/// arquivos de uma mesma extração diferem por milissegundos, e o `resources.pakz` de 15 MB do
/// Alice terminava de ser escrito depois do `.mod`. Numa lista com botão de excluir, errar assim
/// apaga o jogo.
///
/// Ver [`crate::archive::completar_manifesto`], que reconstrói o manifesto de um cache antigo a
/// partir do zip — que é a fonte de verdade sobre o que veio no pacote.
fn do_pacote(dir: &Path) -> Option<std::collections::HashSet<String>> {
    let texto = std::fs::read_to_string(dir.join(crate::archive::MANIFESTO)).ok()?;
    Some(texto.lines().filter(|l| !l.is_empty()).map(str::to_string).collect())
}

/// Desce pela árvore juntando o que não está no manifesto.
///
/// Uma pasta que **está** no manifesto ainda pode ter save dentro: o Zeeboids grava em
/// `mod/<id>/zeeboiddata`, e `mod` veio do pacote. Por isso a descida continua nas conhecidas e
/// para nas desconhecidas, que já entram inteiras.
fn junta(
    dir: &Path,
    pacote: &std::collections::HashSet<String>,
    prefixo: &str,
    achados: &mut Vec<PathBuf>,
) {
    /// Até onde descer. A árvore do cache é rasa; o teto é contra ligação circular.
    const FUNDO: usize = 8;

    if prefixo.matches('/').count() >= FUNDO {
        return;
    }
    let Ok(entradas) = std::fs::read_dir(dir) else {
        return;
    };
    for entrada in entradas.flatten() {
        let nome = entrada.file_name().to_string_lossy().into_owned();
        if nome == crate::archive::MANIFESTO {
            continue;
        }
        let caminho = format!("{prefixo}{nome}");
        let pasta = entrada.path().is_dir();
        // O zip lista pastas com barra no fim; arquivos, sem.
        let conhecido = pacote.contains(&caminho) || pacote.contains(&format!("{caminho}/"));
        match (conhecido, pasta) {
            (true, true) => junta(&entrada.path(), pacote, &format!("{caminho}/"), achados),
            (true, false) => {}
            (false, _) => achados.push(entrada.path()),
        }
    }
}

/// O que um jogo escreveu na pasta dele: o que não veio no pacote.
fn do_jogo(raiz: &Path, titulo: &str) -> Option<Save> {
    let pacote = do_pacote(raiz)?;
    let mut itens: Vec<PathBuf> = Vec::new();
    junta(raiz, &pacote, "", &mut itens);
    itens.sort();
    Save::de(titulo.to_string(), itens)
}

/// Os saves de todos os jogos do cache.
///
/// O nome que aparece é o da pasta do cache — `Zeeboids-6518125-1788761080` —, com a numeração
/// tirada: é o título que o jogador reconhece.
pub fn dos_jogos(cache: &Path) -> Vec<Save> {
    let Ok(entradas) = std::fs::read_dir(cache) else {
        return Vec::new();
    };
    let mut achados: Vec<Save> = entradas
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let nome = e.file_name().to_string_lossy().into_owned();
            do_jogo(&e.path(), &titulo_de(&nome))
        })
        .collect();
    achados.sort_by(|a, b| a.titulo.cmp(&b.titulo));
    achados
}

/// O título sem a numeração que o cache acrescenta.
///
/// `Zeeboids-6518125-1788761080` vira `Zeeboids`. Os dois números do fim são o identificador do
/// pacote e o carimbo de tempo; nenhum dos dois diz nada a quem está olhando a lista.
fn titulo_de(pasta: &str) -> String {
    let mut partes: Vec<&str> = pasta.split('-').collect();
    while partes.len() > 1 && partes.last().is_some_and(|p| p.chars().all(|c| c.is_ascii_digit())) {
        partes.pop();
    }
    partes.join("-").replace('-', " ")
}

/// O que está guardado no sistema de arquivos do aparelho, uma linha por pasta.
///
/// Aqui **não** se compara data: nada disso veio de pacote nenhum. O aparelho só tem o que os
/// jogos escreveram — o `zeeboiddata` do Zeeboids, o `shared` do sistema.
pub fn do_aparelho(device: &Path) -> Vec<Save> {
    let Ok(entradas) = std::fs::read_dir(device) else {
        return Vec::new();
    };
    let mut achados: Vec<Save> = entradas
        .flatten()
        .filter_map(|e| {
            let nome = e.file_name().to_string_lossy().into_owned();
            Save::de(nome, vec![e.path()])
        })
        .collect();
    achados.sort_by(|a, b| a.titulo.cmp(&b.titulo));
    achados
}

/// Apaga um save. Devolve o primeiro erro, se houver.
pub fn apagar(save: &Save) -> std::io::Result<()> {
    for item in &save.itens {
        let resultado = match item.is_dir() {
            true => std::fs::remove_dir_all(item),
            false => std::fs::remove_file(item),
        };
        // Já não existir não é erro: o que se queria é que não exista.
        if let Err(erro) = resultado {
            if erro.kind() != std::io::ErrorKind::NotFound {
                return Err(erro);
            }
        }
    }
    Ok(())
}

/// O tamanho em algo que se lê.
pub fn tamanho(bytes: u64) -> String {
    const UNIDADES: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut valor = bytes as f64;
    let mut unidade = 0;
    while valor >= 1024.0 && unidade + 1 < UNIDADES.len() {
        valor /= 1024.0;
        unidade += 1;
    }
    match unidade {
        0 => format!("{bytes} {}", UNIDADES[0]),
        _ => format!("{valor:.1} {}", UNIDADES[unidade]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Monta um cache com um jogo: o manifesto lista o pacote, e o save fica de fora dele.
    fn cache_com_jogo(raiz: &Path) -> PathBuf {
        let jogo = raiz.join("Zeeboids-6518125-1788761080");
        let dir = jogo.join("mod/274");
        std::fs::create_dir_all(&dir).unwrap();
        let pacote = ["zeeboids.mod", "zeeboids.sig", "resources.pakz"];
        for nome in pacote {
            std::fs::File::create(dir.join(nome))
                .unwrap()
                .write_all(b"pacote")
                .unwrap();
        }
        let mut linhas = vec!["mod/".to_string(), "mod/274/".to_string()];
        linhas.extend(pacote.iter().map(|n| format!("mod/274/{n}")));
        std::fs::write(jogo.join(crate::archive::MANIFESTO), linhas.join("\n")).unwrap();
        // O save não está no manifesto — é o que o jogo escreveu depois.
        std::fs::create_dir_all(dir.join("zeeboiddata")).unwrap();
        std::fs::File::create(dir.join("zeeboiddata/zeeboid.db"))
            .unwrap()
            .write_all(b"save")
            .unwrap();
        raiz.to_path_buf()
    }

    /// Um cache sem manifesto não entra na lista: melhor ficar de fora do que ser chutado.
    #[test]
    fn cache_sem_manifesto_e_ignorado() {
        let raiz = temporario("sem-manifesto");
        cache_com_jogo(&raiz);
        std::fs::remove_file(
            raiz.join("Zeeboids-6518125-1788761080").join(crate::archive::MANIFESTO),
        )
        .unwrap();
        assert!(dos_jogos(&raiz).is_empty());
    }

    fn temporario(nome: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zeebx-saves-{nome}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn o_que_o_jogo_escreveu_depois_e_save_e_o_pacote_nao() {
        let raiz = temporario("jogo");
        cache_com_jogo(&raiz);
        let saves = dos_jogos(&raiz);
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].titulo, "Zeeboids");
        // Só o `zeeboiddata`. O `.mod`, o `.sig` e o `.pakz` são do pacote.
        assert_eq!(saves[0].itens.len(), 1);
        assert!(saves[0].itens[0].ends_with("zeeboiddata"));
        assert_eq!(saves[0].arquivos, 1);
    }

    #[test]
    fn apagar_tira_o_save_e_deixa_o_pacote() {
        let raiz = temporario("apagar");
        cache_com_jogo(&raiz);
        let saves = dos_jogos(&raiz);
        apagar(&saves[0]).unwrap();
        assert!(dos_jogos(&raiz).is_empty());
        let dir = raiz.join("Zeeboids-6518125-1788761080/mod/274");
        assert!(dir.join("zeeboids.mod").exists());
        assert!(dir.join("resources.pakz").exists());
    }

    #[test]
    fn apagar_o_que_ja_sumiu_nao_e_erro() {
        let raiz = temporario("sumiu");
        cache_com_jogo(&raiz);
        let saves = dos_jogos(&raiz);
        apagar(&saves[0]).unwrap();
        apagar(&saves[0]).unwrap();
    }

    #[test]
    fn o_aparelho_lista_cada_pasta() {
        let raiz = temporario("aparelho");
        std::fs::create_dir_all(raiz.join("zeeboiddata")).unwrap();
        std::fs::create_dir_all(raiz.join("shared")).unwrap();
        std::fs::File::create(raiz.join("zeeboiddata/zeeboid.db"))
            .unwrap()
            .write_all(b"12345")
            .unwrap();
        let saves = do_aparelho(&raiz);
        assert_eq!(saves.len(), 2);
        assert_eq!(saves[0].titulo, "shared");
        assert_eq!(saves[1].titulo, "zeeboiddata");
        assert_eq!(saves[1].bytes, 5);
    }

    #[test]
    fn o_titulo_perde_a_numeracao_do_cache() {
        assert_eq!(titulo_de("Zeeboids-6518125-1788761080"), "Zeeboids");
        assert_eq!(titulo_de("Zeebo-F-C--Foot-Camp-17313250-1788761080"), "Zeebo F C  Foot Camp");
        // Sem numeração, fica como está.
        assert_eq!(titulo_de("Quake"), "Quake");
    }

    /// Só para olhar: lista o cache de verdade desta máquina.
    #[test]
    #[ignore]
    fn olhar_o_cache_de_verdade() {
        let base = dirs_config();
        for s in dos_jogos(&base.join("cache")) {
            println!("jogo     {:<28} {} arq  {}", s.titulo, s.arquivos, tamanho(s.bytes));
        }
        for s in do_aparelho(&base.join("aparelho")) {
            println!("aparelho {:<28} {} arq  {}", s.titulo, s.arquivos, tamanho(s.bytes));
        }
    }

    fn dirs_config() -> PathBuf {
        PathBuf::from(std::env::var("HOME").unwrap()).join(".config/zeebx")
    }

    #[test]
    fn o_tamanho_sai_legivel() {
        assert_eq!(tamanho(0), "0 B");
        assert_eq!(tamanho(999), "999 B");
        assert_eq!(tamanho(1536), "1.5 KB");
        assert_eq!(tamanho(5 * 1024 * 1024), "5.0 MB");
    }
}
