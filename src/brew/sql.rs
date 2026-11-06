//! The console's `AEECLSID_SQLMGR`, on top of real SQLite.
//!
//! This is not a convenience choice: the console really did use SQLite. The Z-Wheel package
//! ships a 4 KB `tt_prefs.db` whose first bytes are literally `SQLite format 3`, and the
//! statements the module carries as text — `INSERT OR REPLACE`, `COLLATE NOCASE`, a JOIN between
//! `GAMEINFO` and `TITLETEXT`, `PRAGMA integrity_check` — are that dialect. Reimplementing it
//! would be a poor reinvention of exactly what already exists, ready and in the public domain.
//!
//! All this module does is the bridge: open the file the game names, run the statement it sends
//! and hand the rows back in the shape `sqlite3_exec` uses — one call to the game's callback per
//! row, with the values already as text.

use std::path::{Path, PathBuf};

use rusqlite::params;

use crate::library::CatalogIndex;

/// Prepares the writable copy of the Z-Wheel library and syncs the local ROMs.
///
/// The database shipped in the package is the source of the official catalog; we never modify
/// it. The profile copy holds a tracking table of ours, so that a new scan removes only records
/// Zeebx created, without deleting titles that came from the NAND or the package.
pub fn sync_z_wheel_library(
    packaged: &Path,
    profile: &Path,
    catalog: &CatalogIndex,
) -> Result<PathBuf, String> {
    if !profile.exists() {
        let parent = profile.parent().ok_or("profile has no directory")?;
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        std::fs::copy(packaged, profile).map_err(|err| err.to_string())?;
    }
    let mut conn = rusqlite::Connection::open(profile).map_err(|err| err.to_string())?;
    let tx = conn.transaction().map_err(|err| err.to_string())?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS ZEEBX_LIBRARY(class_id INTEGER PRIMARY KEY, game_id INTEGER NOT NULL)",
    )
    .map_err(|err| err.to_string())?;
    let old: Vec<(u32, u32)> = {
        let mut stmt = tx
            .prepare("SELECT class_id, game_id FROM ZEEBX_LIBRARY")
            .map_err(|err| err.to_string())?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|err| err.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|err| err.to_string())?
    };
    for (class_id, game_id) in old {
        tx.execute("DELETE FROM TITLETEXT WHERE game_id = ?1", params![game_id])
            .map_err(|err| err.to_string())?;
        tx.execute(
            "DELETE FROM GAMEINFO WHERE game_id = ?1 AND class_id = ?2",
            params![game_id, class_id],
        )
        .map_err(|err| err.to_string())?;
    }
    tx.execute("DELETE FROM ZEEBX_LIBRARY", [])
        .map_err(|err| err.to_string())?;
    for entry in &catalog.titles {
        if entry.source != crate::library::CatalogSource::Rom {
            continue;
        }
        // ClassID is stable and fits in SQLite's INTEGER; using it as game_id too avoids a local
        // counter that would change whenever the folder is rescanned.
        let id = entry.applet_class;
        tx.execute(
            "INSERT OR REPLACE INTO GAMEINFO(game_id,class_id,playcount,dt_download,dt_lastplayed,boxart_path,flags,size) VALUES (?1,?2,0,0,0,'',2,0)",
            params![id, id],
        ).map_err(|err| err.to_string())?;
        for lang in [0x2020_6e65u32, 0x2020_7365, 0x2020_7470] {
            tx.execute(
                "INSERT OR REPLACE INTO TITLETEXT(game_id,lang_id,titletext) VALUES (?1,?2,?3)",
                params![id, lang, entry.title],
            ).map_err(|err| err.to_string())?;
        }
        tx.execute(
            "INSERT INTO ZEEBX_LIBRARY(class_id,game_id) VALUES (?1,?2)",
            params![id, id],
        ).map_err(|err| err.to_string())?;
    }
    tx.commit().map_err(|err| err.to_string())?;
    Ok(profile.to_path_buf())
}

/// An open database.
pub struct Database {
    conn: rusqlite::Connection,
    /// Where it lives on the host.
    ///
    /// Kept because the **database contents are not in the engine**: they are in the file. It is
    /// the same case as open files — a save state records the path, and loading reopens it.
    /// Without the path loading would be impossible, and the alternative would be writing the
    /// whole database into the save state, duplicating what is already on disk.
    path: std::path::PathBuf,
}

/// One result row, with the values and the column names already as text.
///
/// Text because that is how `sqlite3_exec` delivers them: the callback receives `char **`.
/// Whoever wants a number converts it, and that is what the app does.
#[derive(Debug)]
pub struct Row {
    pub values: Vec<Option<String>>,
    pub names: Vec<String>,
}

impl Database {
    /// Opens (or creates) the database at the host path already resolved by the VFS.
    pub fn open(path: &Path) -> Result<Self, String> {
        rusqlite::Connection::open(path)
            .map(|conn| Self {
                conn,
                path: path.to_path_buf(),
            })
            .map_err(|err| err.to_string())
    }

    /// The database's host path, for the save state to reopen.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Runs the statement and returns the rows it produced.
    ///
    /// A statement without results — a `CREATE`, an `INSERT` — returns an empty list, which is
    /// the same as what `sqlite3_exec` does when it never calls the callback.
    pub fn exec(&self, sql: &str) -> Result<Vec<Row>, String> {
        let mut stmt = self.conn.prepare(sql).map_err(|err| err.to_string())?;
        let names: Vec<String> = stmt.column_names().into_iter().map(String::from).collect();
        // With no columns there is nothing to iterate: it is a command, not a query.
        if names.is_empty() {
            stmt.raw_execute().map_err(|err| err.to_string())?;
            return Ok(Vec::new());
        }
        let mut lines = Vec::new();
        let mut rows = stmt.raw_query();
        while let Some(row) = rows.next().map_err(|err| err.to_string())? {
            let values = (0..names.len())
                .map(|i| {
                    row.get_ref(i).ok().and_then(|value| match value {
                        rusqlite::types::ValueRef::Null => None,
                        rusqlite::types::ValueRef::Integer(n) => Some(n.to_string()),
                        rusqlite::types::ValueRef::Real(n) => Some(n.to_string()),
                        rusqlite::types::ValueRef::Text(t) => {
                            Some(String::from_utf8_lossy(t).into_owned())
                        }
                        // A blob has no text form; `sqlite3_exec` hands over the bytes as they
                        // are, and that is what the app would receive on the console.
                        rusqlite::types::ValueRef::Blob(b) => {
                            Some(String::from_utf8_lossy(b).into_owned())
                        }
                    })
                })
                .collect();
            lines.push(Row {
                values,
                names: names.clone(),
            });
        }
        Ok(lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{CatalogEntry, CatalogSource};

    /// The preferences database the Z-Wheel ships in its package, built from scratch with the
    /// same schema its module carries as text.
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
    fn the_zwheel_version_query_answers() {
        let dir = crate::scratch::TempDir::new("zeebx-sql-versao");
        let db = prefs(dir.path());

        // It is the first thing the Z-Wheel asks after opening the database.
        let lines = db.exec("SELECT version, subversion FROM DBINFO").unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].names, ["version", "subversion"]);
        assert_eq!(
            lines[0].values,
            [Some("1".to_string()), Some("0".to_string())]
        );
    }

    #[test]
    fn the_integrity_check_answers_ok() {
        // The statement that shows up in the probe before any other. A `PRAGMA` returns a row
        // like any query, and that is what the app checks.
        let dir = crate::scratch::TempDir::new("zeebx-sql-integridade");
        let db = prefs(dir.path());
        let lines = db.exec("PRAGMA integrity_check").unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].values[0].as_deref(), Some("ok"));
    }

    #[test]
    fn a_statement_without_results_returns_no_rows_and_writes() {
        let dir = crate::scratch::TempDir::new("zeebx-sql-gravacao");
        let db = prefs(dir.path());
        let saved = db
            .exec("INSERT OR REPLACE INTO PREFSINFO values ('Initialized', '', 1, 2)")
            .unwrap();
        assert!(saved.is_empty());

        // The value has to survive the next query: that is how the Z-Wheel avoids the initial
        // setup screen on every launch.
        let lines = db
            .exec("SELECT * FROM PREFSINFO WHERE PREFSINFO.name = 'Initialized'")
            .unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].values[2].as_deref(), Some("1"));
    }

    #[test]
    fn an_invalid_statement_becomes_an_error_with_a_reason() {
        let dir = crate::scratch::TempDir::new("zeebx-sql-erro");
        let db = prefs(dir.path());
        let error = db.exec("SELECT * FROM NAO_EXISTE").unwrap_err();
        assert!(
            error.contains("NAO_EXISTE"),
            "the reason has to say what: {error}"
        );
    }

    #[test]
    fn the_profile_library_keeps_the_official_entries_and_updates_roms() {
        let dir = crate::scratch::TempDir::new("zeebx-sql-biblioteca");
        let package = dir.join("tt_game_info");
        let db = Database::open(&package).unwrap();
        db.exec("CREATE TABLE GAMEINFO(game_id INTEGER PRIMARY KEY, class_id INTEGER, playcount INTEGER, dt_download INTEGER, dt_lastplayed INTEGER, boxart_path TEXT, flags INTEGER, size INTEGER)").unwrap();
        db.exec("CREATE TABLE TITLETEXT(game_id INTEGER, lang_id INTEGER, titletext TEXT)").unwrap();
        db.exec("INSERT INTO GAMEINFO values (1, 2, 0, 0, 0, '', 2, 0)").unwrap();
        let catalog = CatalogIndex {
            titles: vec![CatalogEntry {
                applet_class: 0x0100_9999,
                title: "Homebrew".into(),
                rom_path: dir.join("homebrew.mod"),
                source: CatalogSource::Rom,
            }],
        };
        let profile = dir.join("perfil/tt_game_info");
        sync_z_wheel_library(&package, &profile, &catalog).unwrap();
        let profile_db = Database::open(&profile).unwrap();
        assert_eq!(profile_db.exec("SELECT * FROM GAMEINFO").unwrap().len(), 2);
        assert_eq!(
            profile_db
                .exec("SELECT * FROM TITLETEXT WHERE titletext='Homebrew'")
                .unwrap()
                .len(),
            3
        );
    }
}
