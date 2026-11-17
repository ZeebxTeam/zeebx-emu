//! IDatabase and IQuery: the console's AEECLSID_SQLMGR, served by real SQLite.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `ISQLMgr` and `ISQLDatabase` — the console's SQLite databases. See [`crate::brew::sql`].
    ///
    /// The slot order did not come from a header: it came from observation with `--probe`. The
    /// Z-Wheel creates the manager, calls slot 3 with `"tt_prefs.db"` and an output pointer, and
    /// on the database it gets back calls slot 3 again, now with `"PRAGMA integrity_check"`. That
    /// is why the two names in [`crate::brew::aee_slots::SQL_MGR`] are the only named ones.
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
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.databases.remove(&this);
                }
                remaining
            }
            // int OpenDatabase(ISQLMgr *, const char *pszName, ISQLDatabase **ppDB)
            (Interface::SqlMgr, "OpenDatabase") => {
                let db_name = self.cpu.read_cstring(a1, MAX_STRING);
                // The database lives next to the module, like any game file, and goes through
                // the VFS for the same reason as the others: nothing writes outside its
                // directory. A database that came in the package is copied to the writable layer
                // before opening, because SQLite writes to it.
                let target = self
                    .vfs
                    .open_target(&db_name, crate::brew::vfs::OpenIntent::ReadWrite);
                let Some(target) = target else {
                    if self.serial.is_some() {
                        self.log_serial(format!("<database {db_name} -> path does not resolve>"));
                    }
                    self.missing_files.insert(db_name);
                    return Ok(Some(EFAILED));
                };
                // The official catalog comes in the package, but the user's library must be
                // writable and survive a new extraction of the ZIP. For the Z-Wheel we open a
                // profile copy already synced with the ROMs the interface found.
                let path = if db_name == "tt_game_info" {
                    let profile = crate::loader::archive::device_dir().join("z-wheel/tt_game_info");
                    let catalog = crate::library::CatalogIndex::load_from(
                        &crate::library::catalog_path(),
                    );
                    let official = target.copy_from.unwrap_or(target.path);
                    let roms = catalog
                        .titles
                        .iter()
                        .filter(|entry| entry.source == crate::library::CatalogSource::Rom)
                        .map(|entry| (entry.applet_class, entry.title.as_str()));
                    // What the user owns: the ROMs found, and what the catalog says came from the
                    // console itself. The Z-Wheel is in the ROM folder too and is not a game.
                    let owned: Vec<u32> = catalog
                        .titles
                        .iter()
                        .map(|entry| entry.applet_class)
                        .filter(|class| *class != crate::session::Z_WHEEL)
                        .collect();
                    let synced = crate::brew::sql::sync_z_wheel_library(&official, &profile, roms)
                        .and_then(|path| {
                            keep_only_owned(&official, &path, &owned)
                                .map(|()| path)
                                .map_err(|error| error.to_string())
                        });
                    match synced {
                        Ok(path) => path,
                        Err(error) => {
                            self.bad_pointers
                                .insert(format!("SQL: could not prepare tt_game_info: {error}"));
                            return Ok(Some(EFAILED));
                        }
                    }
                } else {
                    if let Some(parent) = target.path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Some(origin) = &target.copy_from
                        && std::fs::copy(origin, &target.path).is_err()
                    {
                        self.missing_files.insert(db_name);
                        return Ok(Some(EFAILED));
                    }
                    target.path
                };
                let opened = crate::brew::sql::Database::open(&path);
                if self.serial.is_some() {
                    let outcome = match &opened {
                        Ok(_) => "opened".to_string(),
                        Err(error) => format!("failed: {error}"),
                    };
                    self.log_serial(format!("<database {db_name} -> {outcome}>"));
                }
                match opened {
                    Ok(db) => {
                        self.choose_language(&db);
                        if db_name == "tt_dlqueue.db" {
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
                    Err(error) => {
                        self.bad_pointers.insert(format!("SQL: {db_name}: {error}"));
                        SUCCESS
                    }
                }
            }
            // int Exec(ISQLDatabase *, const char *pszSQL, callback, void *pContext)
            (Interface::SqlDatabase, "Exec") => {
                let sql = self.cpu.read_cstring(a1, MAX_SQL);
                let Some(db) = self.databases.get(&this) else {
                    return Ok(Some(EBADPARM));
                };
                let exec_result = db.exec(&sql);
                if self.serial.is_some() {
                    let count = match &exec_result {
                        Ok(lines) => format!("{} row(s)", lines.len()),
                        Err(error) => format!("error: {error}"),
                    };
                    self.log_serial(format!("<sql {sql} -> {count}>"));
                }
                match exec_result {
                    Ok(lines) => {
                        // `Exec(this, sql, callback, context)`: the probe showed the function
                        // pointer in `r2` — inside the module's code range — and the context in
                        // `r3`, on the heap.
                        let (callback, context) = (a2, self.cpu.read_reg(Reg::R3));
                        self.sql_deliver(&lines, callback, context)?;
                        SUCCESS
                    }
                    Err(error) => {
                        self.bad_pointers.insert(format!("SQL rejected: {error}"));
                        SUCCESS
                    }
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Delivers the rows to the game's callback, one call per row.
    ///
    /// It is the shape of `sqlite3_exec`: `callback(context, nColumns, azValues, azNames)`, with
    /// the two `char *` arrays in guest memory. Without it the query "works" and the game
    /// receives nothing — that is what made the Z-Wheel pass `PRAGMA integrity_check` and still
    /// say "Invalid database version".
    ///
    /// Calling the guest from here is reentrancy, so it takes the same care as `qsort`: it saves
    /// the registers, respects the nesting ceiling and puts everything back in place.
    pub(super) fn sql_deliver(
        &mut self,
        lines: &[crate::brew::sql::Row],
        callback: u32,
        context: u32,
    ) -> Result<(), CpuError> {
        if callback == 0 || lines.is_empty() {
            return Ok(());
        }
        if self.nesting >= MAX_NESTING {
            self.assumptions
                .insert("an SQL query was delivered without a callback due to deep nesting");
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        let mut result = Ok(());
        for line in lines {
            match self.sql_deliver_row(line, callback, context) {
                Ok(true) => {}
                // A callback that returns nonzero means stop, as in SQLite.
                Ok(false) => break,
                Err(err) => {
                    result = Err(err);
                    break;
                }
            }
        }
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        result
    }

    /// Builds the two `char *` arrays for a row and calls the callback. `false` asks to stop.
    pub(super) fn sql_deliver_row(
        &mut self,
        line: &crate::brew::sql::Row,
        callback: u32,
        context: u32,
    ) -> Result<bool, CpuError> {
        let columns = line.names.len() as u32;
        let values = self.sql_write_strings(line.values.iter().map(|v| v.as_deref()))?;
        let names = self.sql_write_strings(line.names.iter().map(|n| Some(n.as_str())))?;
        let output = match (values, names) {
            (Some(values), Some(names)) => {
                let outcome = self.call_guest(
                    callback,
                    [context, columns, values, names],
                    SQL_CALLBACK_BUDGET,
                )?;
                // Only a normal return counts; a callback that gets lost does not stop the rest.
                let proceed = !matches!(outcome, Outcome::Returned { code } if code != 0);
                self.free(values);
                self.free(names);
                proceed
            }
            _ => {
                self.assumptions
                    .insert("an SQL query row did not fit in the game's memory");
                false
            }
        };
        Ok(output)
    }

    /// Writes the strings to the guest heap and returns the array of pointers to them.
    ///
    /// The array and the text come out of the same block: a single `free` returns everything,
    /// and the `sqlite3_exec` callback does not keep the pointers after it returns.
    pub(super) fn sql_write_strings<'a>(
        &mut self,
        texts: impl Iterator<Item = Option<&'a str>> + Clone,
    ) -> Result<Option<u32>, CpuError> {
        let count = texts.clone().count() as u32;
        let bytes: usize = texts.clone().map(|t| t.map_or(0, |t| t.len() + 1)).sum();
        let block = self.alloc_internal(count * 4 + bytes as u32)?;
        if block == 0 {
            return Ok(None);
        }
        let mut text_addr = block + count * 4;
        for (i, text) in texts.enumerate() {
            let pointer = match text {
                // A null column is a null pointer, as SQLite delivers it.
                None => 0,
                Some(text) => {
                    self.cpu.write_mem(text_addr, text.as_bytes())?;
                    self.cpu.write_mem(text_addr + text.len() as u32, &[0])?;
                    let location = text_addr;
                    text_addr += text.len() as u32 + 1;
                    location
                }
            };
            self.cpu.write_u32(block + i as u32 * 4, pointer)?;
        }
        Ok(Some(block))
    }
}

/// Leaves in the Z-Wheel's profile catalog only the games the user has: one row per game in
/// `owned`, and nothing else.
///
/// The package's `tt_game_info` lists the 59 titles the store sold, and the profile copy kept all
/// of them: the grid showed the same games in the same order whatever was in the ROM folder, an
/// empty folder included, and choosing one that was not there opened nothing. On top of them
/// [`crate::brew::sql::sync_z_wheel_library`] adds a bare row per ROM — no box art, the file's
/// name for a title —, so a game that was in both showed up twice in the database.
///
/// After this:
///
/// - a game in `owned` that the package knows keeps **the package's row**: its box art, its size
///   and the title in each language. The bare row is dropped;
/// - a game in `owned` the package does not know (a homebrew) keeps the bare row;
/// - a package row for a game that is not in `owned` leaves the profile.
///
/// The package's rows are read from `official` on every sync, so a game that comes back to the
/// folder comes back to the grid. A row that stayed keeps what the Z-Wheel wrote on it — play
/// count, last played —, which is why the profile is edited and not copied again.
fn keep_only_owned(
    official: &std::path::Path,
    profile: &std::path::Path,
    owned: &[u32],
) -> rusqlite::Result<()> {
    let mut connection = rusqlite::Connection::open(profile)?;
    connection.execute(
        "ATTACH DATABASE ?1 AS package",
        [official.to_string_lossy().as_ref()],
    )?;
    let tx = connection.transaction()?;
    tx.execute_batch(
        "CREATE TEMP TABLE zeebx_owned(class_id INTEGER PRIMARY KEY);
         CREATE TEMP TABLE zeebx_known(class_id INTEGER PRIMARY KEY);
         INSERT OR IGNORE INTO zeebx_known SELECT class_id FROM package.GAMEINFO;",
    )?;
    for class in owned {
        tx.execute("INSERT OR IGNORE INTO zeebx_owned VALUES (?1)", [class])?;
    }
    tx.execute_batch(
        // The package's rows of what is owned, back in if a previous sync took them out.
        "INSERT OR IGNORE INTO GAMEINFO
             SELECT * FROM package.GAMEINFO WHERE class_id IN (SELECT class_id FROM zeebx_owned);
         INSERT OR IGNORE INTO TITLETEXT
             SELECT * FROM package.TITLETEXT WHERE game_id IN
                 (SELECT game_id FROM package.GAMEINFO
                  WHERE class_id IN (SELECT class_id FROM zeebx_owned));
         -- The package's rows of what is not owned, out.
         DELETE FROM TITLETEXT WHERE game_id IN
             (SELECT game_id FROM package.GAMEINFO
              WHERE class_id NOT IN (SELECT class_id FROM zeebx_owned));
         DELETE FROM GAMEINFO WHERE game_id IN
             (SELECT game_id FROM package.GAMEINFO
              WHERE class_id NOT IN (SELECT class_id FROM zeebx_owned));
         -- The bare rows of what the package describes better, or of what is not a game.
         DELETE FROM TITLETEXT WHERE game_id IN
             (SELECT game_id FROM ZEEBX_LIBRARY
              WHERE class_id IN (SELECT class_id FROM zeebx_known)
                 OR class_id NOT IN (SELECT class_id FROM zeebx_owned));
         DELETE FROM GAMEINFO WHERE game_id IN
             (SELECT game_id FROM ZEEBX_LIBRARY
              WHERE class_id IN (SELECT class_id FROM zeebx_known)
                 OR class_id NOT IN (SELECT class_id FROM zeebx_owned));
         DELETE FROM ZEEBX_LIBRARY
              WHERE class_id IN (SELECT class_id FROM zeebx_known)
                 OR class_id NOT IN (SELECT class_id FROM zeebx_owned);
         DROP TABLE zeebx_owned;
         DROP TABLE zeebx_known;",
    )?;
    tx.commit()?;
    connection.execute("DETACH DATABASE package", [])?;
    Ok(())
}

#[cfg(test)]
mod catalog_tests {
    use super::keep_only_owned;

    fn rows(path: &std::path::Path) -> Vec<(u32, u32, String)> {
        let connection = rusqlite::Connection::open(path).unwrap();
        let mut query = connection
            .prepare(
                "SELECT g.game_id, g.class_id, t.titletext FROM GAMEINFO g
                 JOIN TITLETEXT t ON t.game_id = g.game_id ORDER BY g.game_id",
            )
            .unwrap();
        query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    #[test]
    fn the_grid_follows_the_rom_folder() {
        let dir = std::env::temp_dir().join(format!("zeebx-catalog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (official, profile) = (dir.join("package"), dir.join("profile"));
        {
            let package = rusqlite::Connection::open(&official).unwrap();
            package
                .execute_batch(
                    "CREATE TABLE GAMEINFO(game_id INTEGER PRIMARY KEY, class_id INTEGER,
                         playcount INTEGER, dt_download INTEGER, dt_lastplayed INTEGER,
                         boxart_path TEXT, flags INTEGER, size INTEGER, unique(game_id, class_id));
                     CREATE TABLE TITLETEXT(game_id INTEGER, lang_id INTEGER, titletext TEXT,
                         unique(game_id, lang_id));
                     INSERT INTO GAMEINFO VALUES (100, 1, 0, 0, 0, './a/', 0, 10);
                     INSERT INTO GAMEINFO VALUES (200, 2, 0, 0, 0, './b/', 0, 20);
                     INSERT INTO TITLETEXT VALUES (100, 7, 'Alice');
                     INSERT INTO TITLETEXT VALUES (200, 7, 'Baja');",
                )
                .unwrap();
        }
        // Class 1 is in the package and in the folder, class 3 only in the folder.
        let sync = |owned: &[(u32, &str)]| {
            crate::brew::sql::sync_z_wheel_library(&official, &profile, owned.iter().copied())
                .unwrap();
            let classes: Vec<u32> = owned.iter().map(|(class, _)| *class).collect();
            keep_only_owned(&official, &profile, &classes).unwrap();
            let mut seen = rows(&profile);
            seen.dedup_by_key(|row| row.0);
            seen
        };
        assert_eq!(
            sync(&[(1, "Alice no Pais"), (3, "Homebrew")]),
            [(3, 3, "Homebrew".to_string()), (100, 1, "Alice".to_string())]
        );
        // The play count the Z-Wheel wrote survives the next sync.
        rusqlite::Connection::open(&profile)
            .unwrap()
            .execute("UPDATE GAMEINFO SET playcount = 5 WHERE game_id = 100", [])
            .unwrap();
        // The folder changes: Alice stays, the homebrew goes, Baja arrives.
        assert_eq!(
            sync(&[(1, "Alice no Pais"), (2, "Baja 1000")]),
            [(100, 1, "Alice".to_string()), (200, 2, "Baja".to_string())]
        );
        let plays: u32 = rusqlite::Connection::open(&profile)
            .unwrap()
            .query_row("SELECT playcount FROM GAMEINFO WHERE game_id = 100", [], |r| r.get(0))
            .unwrap();
        assert_eq!(plays, 5);
        // An empty folder is an empty grid.
        assert!(sync(&[]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
