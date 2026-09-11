//! IDatabase e IQuery: o AEECLSID_SQLMGR do console, atendido pelo SQLite de verdade.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `ISQLMgr` e `ISQLDatabase` — os bancos SQLite do console. Ver [`crate::brew::sql`].
    ///
    /// A ordem dos slots não veio de header: veio da observação com o `--sonda`. O Z-Wheel cria
    /// o gerenciador, chama o slot 3 com `"tt_prefs.db"` e um ponteiro de saída, e no banco que
    /// recebe chama o slot 3 de novo, agora com `"PRAGMA integrity_check"`. Por isso os dois
    /// nomes que estão em [`crate::brew::aee_slots::SQL_MGR`] são os únicos com nome.
    pub(super) fn sql_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2) = (self.cpu.read_reg(Reg::R1), self.cpu.read_reg(Reg::R2));
        let result = match (iface, name) {
            (_, "AddRef") => self.objects.add_ref(this),
            (_, "Release") => {
                let restantes = self.objects.release(this);
                if restantes == 0 {
                    self.databases.remove(&this);
                }
                restantes
            }
            // int OpenDatabase(ISQLMgr *, const char *pszName, ISQLDatabase **ppDB)
            (Interface::SqlMgr, "OpenDatabase") => {
                let nome = self.cpu.read_cstring(a1, MAX_STRING);
                // O banco fica ao lado do módulo, como qualquer arquivo do jogo, e passa pelo
                // VFS pelo mesmo motivo dos outros: nada escreve fora do diretório dele.
                let Some(caminho) = self.vfs.resolve_new(&nome) else {
                    if self.serial.is_some() {
                        self.registra_serial(format!("<banco {nome} -> caminho não resolve>"));
                    }
                    self.missing_files.insert(nome);
                    return Ok(Some(EFAILED));
                };
                // O catálogo oficial vem no pacote, mas a biblioteca do usuário precisa ser
                // gravável e sobreviver a uma nova extração do ZIP. Para a Z-Wheel abrimos uma
                // cópia de perfil já sincronizada com as ROMs que a interface encontrou.
                let caminho = if nome == "tt_game_info" {
                    let perfil = crate::loader::archive::device_dir().join("z-wheel/tt_game_info");
                    let catalogo =
                        crate::library::CatalogIndex::load_from(&crate::library::catalog_path());
                    match crate::brew::sql::sync_z_wheel_library(&caminho, &perfil, &catalogo) {
                        Ok(caminho) => caminho,
                        Err(erro) => {
                            self.bad_pointers
                                .insert(format!("SQL: não deu para preparar tt_game_info: {erro}"));
                            return Ok(Some(EFAILED));
                        }
                    }
                } else {
                    caminho
                };
                let aberto = crate::brew::sql::Database::open(&caminho);
                if self.serial.is_some() {
                    let como = match &aberto {
                        Ok(_) => "abriu".to_string(),
                        Err(erro) => format!("falhou: {erro}"),
                    };
                    self.registra_serial(format!("<banco {nome} -> {como}>"));
                }
                match aberto {
                    Ok(db) => {
                        self.escolhe_idioma(&db);
                        if nome == "tt_dlqueue.db" {
                            let _ = db.exec("CREATE TABLE IF NOT EXISTS DBINFO(version INTEGER, subversion INTEGER)");
                            let _ = db.exec("INSERT OR IGNORE INTO DBINFO values (1, 0)");
                            let _ = db.exec("CREATE TABLE IF NOT EXISTS DLITEMINFO(item_id INTEGER PRIMARY KEY, price INTEGER, size INTEGER, titletext TEXT, boxart_path TEXT, flags INTEGER, upgrade_id INTEGER)");
                        }
                        let object = self.new_object(Interface::SqlDatabase)?;
                        if object == 0 {
                            return Ok(Some(ENOMEMORY));
                        }
                        self.databases.insert(object, db);
                        if a2 != 0 {
                            self.cpu.write_u32(a2, object)?;
                        }
                        SUCCESS
                    }
                    Err(erro) => {
                        self.bad_pointers.insert(format!("SQL: {nome}: {erro}"));
                        SUCCESS
                    }
                }
            }
            // int Exec(ISQLDatabase *, const char *pszSQL, callback, void *pContexto)
            (Interface::SqlDatabase, "Exec") => {
                let sql = self.cpu.read_cstring(a1, MAX_SQL);
                let Some(db) = self.databases.get(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let resultado = db.exec(&sql);
                if self.serial.is_some() {
                    let quantas = match &resultado {
                        Ok(linhas) => format!("{} linha(s)", linhas.len()),
                        Err(erro) => format!("erro: {erro}"),
                    };
                    self.registra_serial(format!("<sql {sql} -> {quantas}>"));
                }
                match resultado {
                    Ok(linhas) => {
                        // `Exec(this, sql, callback, contexto)`: a sonda mostrou o ponteiro de
                        // função em `r2` — dentro da faixa de código do módulo — e o contexto
                        // em `r3`, no heap.
                        let (callback, contexto) = (a2, self.cpu.read_reg(Reg::R3));
                        self.sql_deliver(&linhas, callback, contexto)?;
                        SUCCESS
                    }
                    Err(erro) => {
                        self.bad_pointers.insert(format!("SQL recusado: {erro}"));
                        SUCCESS
                    }
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Entrega as linhas ao callback do jogo, uma chamada por linha.
    ///
    /// É a forma do `sqlite3_exec`: `callback(contexto, nColunas, azValores, azNomes)`, com os
    /// dois vetores de `char *` na memória do guest. Sem isso a consulta "funciona" e o jogo
    /// não recebe nada — foi o que fez o Z-Wheel passar no `PRAGMA integrity_check` e ainda
    /// assim dizer "Invalid database version".
    ///
    /// Chamar o guest daqui é reentrância, e por isso segue o mesmo cuidado do `qsort`: salva
    /// os registradores, respeita o teto de aninhamento e devolve tudo no lugar.
    pub(super) fn sql_deliver(
        &mut self,
        linhas: &[crate::brew::sql::Row],
        callback: u32,
        contexto: u32,
    ) -> Result<(), CpuError> {
        if callback == 0 || linhas.is_empty() {
            return Ok(());
        }
        if self.nesting >= MAX_NESTING {
            self.assumptions
                .insert("uma consulta SQL foi entregue sem callback por aninhamento profundo");
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        let mut resultado = Ok(());
        for linha in linhas {
            match self.sql_deliver_row(linha, callback, contexto) {
                Ok(true) => {}
                // Callback que devolve diferente de zero manda parar, como no SQLite.
                Ok(false) => break,
                Err(err) => {
                    resultado = Err(err);
                    break;
                }
            }
        }
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        resultado
    }

    /// Monta os dois vetores de `char *` de uma linha e chama o callback. `false` pede parada.
    pub(super) fn sql_deliver_row(
        &mut self,
        linha: &crate::brew::sql::Row,
        callback: u32,
        contexto: u32,
    ) -> Result<bool, CpuError> {
        let colunas = linha.names.len() as u32;
        let valores = self.sql_write_strings(linha.values.iter().map(|v| v.as_deref()))?;
        let nomes = self.sql_write_strings(linha.names.iter().map(|n| Some(n.as_str())))?;
        let saida = match (valores, nomes) {
            (Some(valores), Some(nomes)) => {
                let outcome = self.call_guest(
                    callback,
                    [contexto, colunas, valores, nomes],
                    SQL_CALLBACK_BUDGET,
                )?;
                // Só o retorno normal conta; um callback que se perde não interrompe o resto.
                let seguir = !matches!(outcome, Outcome::Returned { code } if code != 0);
                self.heap.free(valores);
                self.heap.free(nomes);
                seguir
            }
            _ => {
                self.assumptions
                    .insert("uma linha de consulta SQL não coube na memória do jogo");
                false
            }
        };
        Ok(saida)
    }

    /// Grava as strings no heap do guest e devolve o vetor de ponteiros para elas.
    ///
    /// O vetor e o texto saem do mesmo bloco: um `free` só devolve tudo, e o callback do
    /// `sqlite3_exec` não guarda os ponteiros depois de retornar.
    pub(super) fn sql_write_strings<'a>(
        &mut self,
        textos: impl Iterator<Item = Option<&'a str>> + Clone,
    ) -> Result<Option<u32>, CpuError> {
        let contagem = textos.clone().count() as u32;
        let bytes: usize = textos.clone().map(|t| t.map_or(0, |t| t.len() + 1)).sum();
        let bloco = self.malloc(contagem * 4 + bytes as u32)?;
        if bloco == 0 {
            return Ok(None);
        }
        let mut texto_em = bloco + contagem * 4;
        for (i, texto) in textos.enumerate() {
            let ponteiro = match texto {
                // Coluna nula é ponteiro nulo, como o SQLite entrega.
                None => 0,
                Some(texto) => {
                    self.cpu.write_mem(texto_em, texto.as_bytes())?;
                    self.cpu.write_mem(texto_em + texto.len() as u32, &[0])?;
                    let onde = texto_em;
                    texto_em += texto.len() as u32 + 1;
                    onde
                }
            };
            self.cpu.write_u32(bloco + i as u32 * 4, ponteiro)?;
        }
        Ok(Some(bloco))
    }
}
