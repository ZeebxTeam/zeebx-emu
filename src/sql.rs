//! O `AEECLSID_SQLMGR` do console, sobre SQLite de verdade.
//!
//! Não é escolha de conveniência: o console usava SQLite mesmo. O pacote da Z-Wheel traz um
//! `tt_prefs.db` de 4 KB cujos primeiros bytes são literalmente `SQLite format 3`, e as
//! instruções que o módulo carrega em texto — `INSERT OR REPLACE`, `COLLATE NOCASE`, JOIN entre
//! `GAMEINFO` e `TITLETEXT`, `PRAGMA integrity_check` — são o dialeto. Reimplementar isso seria
//! reinventar mal justamente o que já existe pronto e em domínio público.
//!
//! O que este módulo faz é só a ponte: abrir o arquivo que o jogo nomeia, executar a instrução
//! que ele manda e entregar as linhas de volta na forma que o `sqlite3_exec` usa — uma chamada
//! ao callback do jogo por linha, com os valores já em texto.

use std::path::Path;

/// Um banco aberto.
pub struct Database {
    conn: rusqlite::Connection,
}

/// Uma linha de resultado, com os valores e os nomes das colunas já em texto.
///
/// Texto porque é assim que o `sqlite3_exec` entrega: o callback recebe `char **`. Quem quer
/// número converte, e é o que o app faz.
#[derive(Debug)]
pub struct Row {
    pub values: Vec<Option<String>>,
    pub names: Vec<String>,
}

impl Database {
    /// Abre (ou cria) o banco no caminho do host já resolvido pelo VFS.
    pub fn open(path: &Path) -> Result<Self, String> {
        rusqlite::Connection::open(path)
            .map(|conn| Self { conn })
            .map_err(|err| err.to_string())
    }

    /// Executa a instrução e devolve as linhas que ela produziu.
    ///
    /// Instrução sem resultado — um `CREATE`, um `INSERT` — devolve lista vazia, que é o mesmo
    /// que o `sqlite3_exec` faz quando nunca chama o callback.
    pub fn exec(&self, sql: &str) -> Result<Vec<Row>, String> {
        let mut stmt = self.conn.prepare(sql).map_err(|err| err.to_string())?;
        let names: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
        // Sem colunas não há o que percorrer: é comando, não consulta.
        if names.is_empty() {
            stmt.raw_execute().map_err(|err| err.to_string())?;
            return Ok(Vec::new());
        }
        let mut linhas = Vec::new();
        let mut rows = stmt.raw_query();
        while let Some(row) = rows.next().map_err(|err| err.to_string())? {
            let values = (0..names.len())
                .map(|i| {
                    row.get_ref(i).ok().and_then(|valor| match valor {
                        rusqlite::types::ValueRef::Null => None,
                        rusqlite::types::ValueRef::Integer(n) => Some(n.to_string()),
                        rusqlite::types::ValueRef::Real(n) => Some(n.to_string()),
                        rusqlite::types::ValueRef::Text(t) => {
                            Some(String::from_utf8_lossy(t).into_owned())
                        }
                        // Um blob não tem forma de texto; o `sqlite3_exec` entrega os bytes
                        // como estão, e é o que o app receberia no console.
                        rusqlite::types::ValueRef::Blob(b) => {
                            Some(String::from_utf8_lossy(b).into_owned())
                        }
                    })
                })
                .collect();
            linhas.push(Row {
                values,
                names: names.clone(),
            });
        }
        Ok(linhas)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O banco de preferências que a Z-Wheel traz no pacote, montado do zero com o mesmo
    /// esquema que o módulo dela carrega em texto.
    fn prefs(dir: &Path) -> Database {
        let db = Database::open(&dir.join("tt_prefs.db")).unwrap();
        db.exec("CREATE TABLE DBINFO(version INTEGER, subversion INTEGER)")
            .unwrap();
        db.exec("INSERT OR REPLACE INTO DBINFO values (1, 0)")
            .unwrap();
        db.exec(
            "CREATE TABLE PREFSINFO(name TEXT PRIMARY KEY, strValue TEXT, dwValue INTEGER, flags INTEGER)",
        )
        .unwrap();
        db
    }

    #[test]
    fn a_consulta_de_versao_do_zwheel_responde() {
        let dir = std::env::temp_dir().join("zeebx-sql-versao");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = prefs(&dir);

        // É a primeira coisa que a Z-Wheel pergunta depois de abrir o banco.
        let linhas = db.exec("SELECT version, subversion FROM DBINFO").unwrap();
        assert_eq!(linhas.len(), 1);
        assert_eq!(linhas[0].names, ["version", "subversion"]);
        assert_eq!(
            linhas[0].values,
            [Some("1".to_string()), Some("0".to_string())]
        );
    }

    #[test]
    fn o_integrity_check_responde_ok() {
        // A instrução que aparece na sonda antes de qualquer outra. Um `PRAGMA` devolve linha
        // como uma consulta qualquer, e é isso que o app confere.
        let dir = std::env::temp_dir().join("zeebx-sql-integridade");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = prefs(&dir);
        let linhas = db.exec("PRAGMA integrity_check").unwrap();
        assert_eq!(linhas.len(), 1);
        assert_eq!(linhas[0].values[0].as_deref(), Some("ok"));
    }

    #[test]
    fn instrucao_sem_resultado_nao_devolve_linha_e_grava() {
        let dir = std::env::temp_dir().join("zeebx-sql-gravacao");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = prefs(&dir);
        let gravou = db
            .exec("INSERT OR REPLACE INTO PREFSINFO values ('Initialized', '', 1, 2)")
            .unwrap();
        assert!(gravou.is_empty());

        // O valor precisa sobreviver à consulta seguinte: é assim que a Z-Wheel evita a tela
        // de configuração inicial a cada abertura.
        let linhas = db
            .exec("SELECT * FROM PREFSINFO WHERE PREFSINFO.name = 'Initialized'")
            .unwrap();
        assert_eq!(linhas.len(), 1);
        assert_eq!(linhas[0].values[2].as_deref(), Some("1"));
    }

    #[test]
    fn instrucao_invalida_vira_erro_com_motivo() {
        let dir = std::env::temp_dir().join("zeebx-sql-erro");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = prefs(&dir);
        let erro = db.exec("SELECT * FROM NAO_EXISTE").unwrap_err();
        assert!(
            erro.contains("NAO_EXISTE"),
            "o motivo precisa dizer o quê: {erro}"
        );
    }
}
