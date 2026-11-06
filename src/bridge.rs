//! Per-module bridges: the little game-specific knowledge the emulator needs, isolated here and
//! nowhere else.
//!
//! **This is not emulation.** Everything else in Zeebx implements BREW API or reads a console
//! vtable — things that hold for any title. What is in this file holds for **one** game, and
//! that is why it lives apart, is optional, and is named for what it is.
//!
//! # Why it exists
//!
//! To study a network protocol the game has to **consume** the server's response, not just
//! receive it. In Zeeboids the response becomes a vector of strings inside one of the game's own
//! objects, and those strings have to come out of its allocator: handing it foreign memory makes
//! the manager complain when it goes to free them (`TTDMemoryManager.cpp:916`). On the console
//! the one filling that vector was the firmware, which naturally used the right allocator.
//!
//! # The rule the bridge must not break
//!
//! It **does not touch what goes over the wire**. It doesn't change the request, doesn't change
//! the response, doesn't exist on the server side. A real Zeebo talking to the same server sees
//! exactly the same HTTP — that is why the bridge only changes how the response is deposited in
//! the game's memory, and not its contents.
//!
//! The corollary matters as much as the rule: **the bridge never justifies changing the
//! server.** If we tuned the response until our way of delivering it accepted it, we would get it
//! right here and wrong on the console. The format is decided by what the game does, and the
//! final proof is the real device.
//!
//! # Status: it works, and still ships disabled
//!
//! It used to crash the game — an invalid access at `0x654ac`, inside the memory manager. Two
//! hypotheses fell along the way, and both are worth recording so nobody redoes them:
//!
//! - **It wasn't the element format.** I came to think the vector held `ttdString` objects —
//!   `{ length, pointer }`, as the constructor at `0xa85e0` builds them. It doesn't: the handler
//!   calls `atoi` directly on the element, so those really are `char *`.
//! - **It wasn't the call.** The allocator receives pool index 0, which it requires to be below
//!   32; the fifth argument where it reads it, at `[sp+0x30]`; and the trace file pointer.
//!
//! It was **the timing**, and that is solved: the response waits in a queue and is deposited at
//! the call boundary, the same one signals use, when the guest is not inside anything. Calling
//! the allocator from inside the dispatch re-entered a manager that was in the middle of an
//! operation.
//!
//! With that, delivery works: the fields are written, the guards pass and the game doesn't
//! break. What decides the rest is the **content** of the response, which is what the server
//! variants exist to measure.

/// What is known about one specific module.
#[derive(Debug, Clone, Copy)]
pub struct Bridge {
    /// The `malloc` of the game's own memory manager.
    ///
    /// Observed signature: `(size, 0, line, file, 1)` — the last two are origin tracking, which
    /// it keeps for its own reports.
    pub allocator: u32,
    /// The game's response parser: `(object, chunk)`.
    ///
    /// It appends the chunk to the text the object keeps at `+0x20` and splits on `;`, filling
    /// the field vector with its own memory and setting the flags the consumer expects. It is
    /// what the console's dispatcher called, and using it beats reproducing it: there is no
    /// format to get right and no flag to guess.
    pub parser: u32,
    /// The `free` of the same manager, to give back what we replace.
    ///
    /// It walks the pool table asking each one whether the pointer is theirs, and rejects what
    /// came from none — it is the function that complained when we handed it foreign memory.
    pub free_fn: u32,
    /// A pointer to a file name, inside the module itself, for the tracking field.
    pub origin: u32,
}

/// Zeeboids (`ClassID` from the `.mif`).
const ZEEBOIDS: u32 = 0x0108_ff1a;

/// The module's bridge, if there is one.
///
/// The addresses came from disassembling `zeeboids.mod` version 1.1.1402: the string
/// constructor at `0xa85e0` calls the allocator at `0x6d440` passing `'source\ttdString.cpp'`,
/// which is at `0xa8648`. Another version of the game will have other addresses, and that is
/// why the bridge is per module and not a loose constant.
pub fn to(clsid: u32) -> Option<Bridge> {
    match clsid {
        ZEEBOIDS => Some(Bridge {
            allocator: 0x0006_d440,
            parser: 0x0008_5d18,
            free_fn: 0x0006_d694,
            origin: 0x000a_8648,
        }),
        _ => None,
    }
}

/// Moves Zeeboids' last-sync date back by one day.
///
/// Zeeboids limits syncing and exporting to **once per calendar day** — and the lock is the
/// game's, not the server's: it keeps `last_sync` and `last_export` in its own database and, at
/// `0x66774`, converts both with `GETJULIANDATE` and compares year, month and day. The server is
/// never consulted.
///
/// That makes any network test slow: one attempt per day. Moving the date back one day gives the
/// button back, and it is the minimum needed — it touches nothing beyond the two date fields, and
/// invents no avatar, score or number.
///
/// The database lives in the device file system, which is shared by every game.
pub fn unlock_sync(device: &std::path::Path) -> Result<String, String> {
    const ONE_DAY: i64 = 86_400;
    let db_path = device.join("zeeboiddata").join("zeeboid.db");
    if !db_path.exists() {
        return Err(format!("could not find {}",db_path.display()));
    }
    let soundfont = rusqlite::Connection::open(&db_path).map_err(|e| e.to_string())?;
    let changed_rows = soundfont
        .execute(
            "UPDATE zeeboid_app_info SET last_sync = last_sync - ?1, last_export = last_export - ?1",
            [ONE_DAY],
        )
        .map_err(|e| e.to_string())?;
    match changed_rows {
        0 => Err("the database does not have the control row yet".to_string()),
        _ => Ok(db_path.display().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_declared_module_has_a_bridge() {
        assert!(to(ZEEBOIDS).is_some());
        // Any other game gets no special treatment at all: that is the point of the file.
        assert!(to(0x0107_0798).is_none());
        assert!(to(0).is_none());
    }
}
