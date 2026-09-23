//! Diretório temporário que se apaga sozinho, para os testes.
//!
//! Existia um vazamento pequeno e constante na suíte: os testes criavam `/tmp/zeebx-sql-*` e
//! `/tmp/zeebx-saves-*` e limpavam **no começo** — para o caso de a rodada anterior ter falhado —
//! e nunca no fim. Uma sessão com várias rodadas deixa dezenas de diretórios para trás. Com este
//! guarda, o diretório sai quando o teste termina, passe ele ou não.

use std::path::{Path, PathBuf};

/// Uma pasta em `temp_dir()` que se remove ao sair de escopo.
pub struct TempDir(PathBuf);

impl TempDir {
    /// Cria a pasta, apagando o que houver com o mesmo nome.
    pub fn new(nome: &str) -> Self {
        let dir = std::env::temp_dir().join(nome);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("criar diretório temporário");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Um caminho dentro da pasta.
    pub fn join(&self, resto: &str) -> PathBuf {
        self.0.join(resto)
    }

    /// O caminho como `PathBuf`, para quem precisa de dono.
    pub fn to_path_buf(&self) -> PathBuf {
        self.0.clone()
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

/// Confere que ainda cabe escrever no volume de `dir`.
///
/// **Por que tentar em vez de perguntar.** Não existe pergunta portátil de "quanto espaço há" sem
/// puxar dependência, e o que a varredura precisa saber não é o número: é se ainda cabe. A prova
/// escreve e apaga um arquivo, e devolve o erro do sistema quando ele diz que não.
///
/// Isto existe porque a falta de espaço **mente**: rodando a varredura das 62 ROMs com o disco
/// cheio, dois jogos apareceram como "não carrega" com `No space left on device`, e o relatório
/// acusou defeito onde havia ambiente. A extração de cada ROM para o cache é o que consome, e a
/// varredura inteira gasta perto de 1 GB.
pub fn cabe_escrever(dir: &Path, bytes: usize) -> Result<(), String> {
    const PASSO: usize = 1024 * 1024;
    let prova = dir.join("zeebx-prova-de-espaco");
    let escrita = std::fs::create_dir_all(dir).and_then(|()| {
        let mut arquivo = std::fs::File::create(&prova)?;
        let bloco = vec![0u8; PASSO];
        let mut restante = bytes;
        while restante > 0 {
            let passo = restante.min(PASSO);
            std::io::Write::write_all(&mut arquivo, &bloco[..passo])?;
            restante -= passo;
        }
        std::io::Write::flush(&mut arquivo)
    });
    let _ = std::fs::remove_file(&prova);
    escrita.map_err(|erro| {
        format!(
            "{erro} (prova de {} MB em {})",
            bytes / PASSO,
            dir.display()
        )
    })
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
