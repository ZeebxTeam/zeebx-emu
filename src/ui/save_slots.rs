//! Each game's save state slots, written to disk.
//!
//! The machine state does not live here: the ZBXS format is defined and validated by the core
//! ([`crate::save_state`]). This module only names the slots, writes the bytes atomically and
//! keeps an optional thumbnail. Keeping the two apart matters: losing or corrupting the thumbnail
//! must never make a valid state impossible to load.
//!
//! It was born in the Android frontend, in 0.4.0, and came here when the Qt interface got save
//! states: both write the same way, in the same layout, and a state copied from the phone to the
//! computer shows up in the same slot, because the folder is the content's identity
//! ([`crate::storage::ContentId`]), the hash of the same file on both.
//!
//! ```text
//! <root>/states/<ContentId>/slot-N.zbxstate   the state
//! <root>/states/<ContentId>/slot-N.rgb565     the thumbnail, optional
//! ```

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::session::Session;
use crate::video::display::Framebuffer;

/// How many slots each game has. There were five on Android; it went up to ten, as in PCSX2, when
/// the Qt interface got the shortcuts to switch slots. The old `slot-1` to `slot-5` stay where
/// they were.
pub const SLOTS: usize = 10;
/// The cap on the file read. A real state is tens of MB; the cap exists so that a broken or odd
/// file does not ask for an absurd allocation.
const MAX_STATE: u64 = 128 * 1024 * 1024;
const THUMB_WIDTH: usize = 160;
const THUMB_HEIGHT: usize = 120;
const THUMB_MAGIC: &[u8; 4] = b"ZBXT";

#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub width: usize,
    pub height: usize,
    pub rgb565: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Slot {
    pub bytes: u64,
    /// When the state was written, by the file's date. `None` where the system does not give it.
    pub recorded: Option<SystemTime>,
    pub thumbnail: Option<Thumbnail>,
}

/// What is in each slot of game `id`. An empty, unreadable or oversized slot is `None`.
pub fn list(root: &Path, id: &str) -> [Option<Slot>; SLOTS] {
    std::array::from_fn(|slot| {
        let path = state_path(root, id, slot);
        recover_backup(&path);
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_STATE {
            return None;
        }
        Some(Slot {
            bytes: meta.len(),
            recorded: meta.modified().ok(),
            thumbnail: read_thumbnail(&thumbnail_path(root, id, slot)).ok(),
        })
    })
}

/// Writes the session's state to `slot`, over whatever is in it.
///
/// **With the current game's GL context**, when it draws on the GPU: the thumbnail and the frame
/// that goes into the state are read from the texture, and the texture can only be read there.
pub fn save(root: &Path, id: &str, slot: usize, session: &mut Session) -> Result<Slot, String> {
    validate_slot(slot)?;
    session.can_save()?;
    // Whoever shares the context with the engine may have painted on it since the last step.
    session.resume_context();
    // On the GPU path, `screen()` is the device's framebuffer, and may not be the 3D texture the
    // player is seeing. The large readback exists precisely to capture the final frame.
    let thumbnail = match session.large_frame() {
        Some(frame) => make_thumbnail(&frame),
        None => make_thumbnail(session.screen()),
    };
    // **The state carries the image of the moment**, like the rewind point: on the GPU, the last
    // frame stays in the texture, and the texture does not go into the state. Without bringing it
    // to the screen, loading came back with the screen of an old frame, or black in a game that
    // only draws through GL. See `docs/implementation/24-speed.md`.
    session.materialize_gl_frame();
    let state = session.save_state();
    if state.is_empty() {
        return Err("the core returned an empty save state".to_string());
    }
    if state.len() as u64 > MAX_STATE {
        return Err(format!(
            "the save state went over the {} MB limit",
            MAX_STATE / (1024 * 1024)
        ));
    }

    let dir = game_dir(root, id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = state_path(root, id, slot);
    recover_backup(&path);
    write_atomic(&path, &state)?;

    // The state is already safe on disk. A thumbnail failure must not turn success into an error:
    // the next opening just shows the slot without an image.
    if let Err(error) = write_thumbnail(&thumbnail_path(root, id, slot), &thumbnail) {
        crate::logging!(
            crate::logging::Level::Warning,
            "estados",
            "did not write the thumbnail of save state {}: {error}",
            slot + 1
        );
    }

    Ok(Slot {
        bytes: state.len() as u64,
        recorded: Some(SystemTime::now()),
        thumbnail: Some(thumbnail),
    })
}

/// Puts the state of `slot` into the session. If it is rejected, the session stays as it was: the
/// engine reads the whole file before writing anything.
///
/// With the current game's GL context, like [`save`]: the restored state rebuilds the textures on
/// the GPU.
pub fn load(root: &Path, id: &str, slot: usize, session: &mut Session) -> Result<Slot, String> {
    validate_slot(slot)?;
    let path = state_path(root, id, slot);
    recover_backup(&path);
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if meta.len() == 0 {
        return Err("the save state is empty".to_string());
    }
    if meta.len() > MAX_STATE {
        return Err(format!(
            "the save state went over the {} MB limit",
            MAX_STATE / (1024 * 1024)
        ));
    }
    let state = std::fs::read(&path).map_err(|e| e.to_string())?;
    session.resume_context();
    session.restore_state(&state).map_err(|e| e.to_string())?;

    Ok(Slot {
        bytes: meta.len(),
        recorded: meta.modified().ok(),
        thumbnail: read_thumbnail(&thumbnail_path(root, id, slot)).ok(),
    })
}

fn validate_slot(slot: usize) -> Result<(), String> {
    (slot < SLOTS)
        .then_some(())
        .ok_or_else(|| "invalid save state slot".to_string())
}

fn game_dir(root: &Path, id: &str) -> PathBuf {
    root.join("states").join(id)
}

fn state_path(root: &Path, id: &str, slot: usize) -> PathBuf {
    game_dir(root, id).join(format!("slot-{}.zbxstate", slot + 1))
}

fn recover_backup(path: &Path) {
    let bak = path.with_extension("zbxstate.bak");
    // A crash after moving the old state, but before publishing the new one, leaves only the
    // backup. It is still the last confirmed state and must reappear automatically.
    if !path.exists() && bak.is_file() {
        let _ = std::fs::rename(bak, path);
    }
}

fn thumbnail_path(root: &Path, id: &str, slot: usize) -> PathBuf {
    game_dir(root, id).join(format!("slot-{}.rgb565", slot + 1))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("zbxstate.tmp");
    let bak = path.with_extension("zbxstate.bak");
    let _ = std::fs::remove_file(&tmp);

    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }

    let _ = std::fs::remove_file(&bak);
    if path.exists() {
        std::fs::rename(path, &bak).map_err(|e| e.to_string())?;
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        if bak.exists() {
            let _ = std::fs::rename(&bak, path);
        }
        return Err(error.to_string());
    }
    let _ = std::fs::remove_file(bak);
    Ok(())
}

fn make_thumbnail(frame: &Framebuffer) -> Thumbnail {
    let src_w = frame.width().max(1) as usize;
    let src_h = frame.height().max(1) as usize;
    let scale = (THUMB_WIDTH as f32 / src_w as f32)
        .min(THUMB_HEIGHT as f32 / src_h as f32)
        .min(1.0);
    let width = ((src_w as f32 * scale).round() as usize).max(1);
    let height = ((src_h as f32 * scale).round() as usize).max(1);
    let mut rgb565 = Vec::with_capacity(width * height * 2);

    for y in 0..height {
        let src_y = y * src_h / height;
        for x in 0..width {
            let src_x = x * src_w / width;
            rgb565.extend_from_slice(
                &frame
                    .get_pixel(src_x as i32, src_y as i32)
                    .to_le_bytes(),
            );
        }
    }

    Thumbnail {
        width,
        height,
        rgb565,
    }
}

fn write_thumbnail(path: &Path, thumbnail: &Thumbnail) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(8 + thumbnail.rgb565.len());
    bytes.extend_from_slice(THUMB_MAGIC);
    bytes.extend_from_slice(&(thumbnail.width as u16).to_le_bytes());
    bytes.extend_from_slice(&(thumbnail.height as u16).to_le_bytes());
    bytes.extend_from_slice(&thumbnail.rgb565);
    let tmp = path.with_extension("rgb565.tmp");
    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn read_thumbnail(path: &Path) -> Result<Thumbnail, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 8 || &bytes[..4] != THUMB_MAGIC {
        return Err("invalid thumbnail".to_string());
    }
    let width = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
    let height = usize::from(u16::from_le_bytes([bytes[6], bytes[7]]));
    if width == 0 || height == 0 || width > THUMB_WIDTH || height > THUMB_HEIGHT {
        return Err("invalid thumbnail dimensions".to_string());
    }
    let expected = width * height * 2;
    if bytes.len() != 8 + expected {
        return Err("truncated thumbnail".to_string());
    }
    Ok(Thumbnail {
        width,
        height,
        rgb565: bytes[8..].to_vec(),
    })
}

pub fn size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::tests::minimal_session_for_save_state;

    const ID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "zeebx-testes-estados-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn the_state_goes_to_the_slot_and_back() {
        let root = root("ida-e-volta");
        let mut session = minimal_session_for_save_state();
        assert!(list(&root, ID).iter().all(Option::is_none));

        let saved = save(&root, ID, 9, &mut session).unwrap();
        assert!(saved.bytes > 0);
        assert!(
            root.join("states")
                .join(ID)
                .join("slot-10.zbxstate")
                .is_file()
        );

        let slots = list(&root, ID);
        assert!(slots[..9].iter().all(Option::is_none));
        let tenth = slots[9]
            .as_ref()
            .expect("slot 10 should appear in the list");
        assert_eq!(tenth.bytes, saved.bytes);
        assert!(tenth.thumbnail.is_some());

        load(&root, ID, 9, &mut session).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_out_of_range_slot_is_rejected() {
        let root = root("fora");
        let mut session = minimal_session_for_save_state();
        assert!(save(&root, ID, SLOTS, &mut session).is_err());
        assert!(load(&root, ID, SLOTS, &mut session).is_err());
        assert!(!root.exists(), "the rejection must not create the folder");
    }

    /// A corrupt state is rejected by the engine, and the session carries on: that is what lets
    /// loading a truncated file not lose the game in progress.
    #[test]
    fn a_corrupt_state_is_rejected() {
        let root = root("estragado");
        let mut session = minimal_session_for_save_state();
        save(&root, ID, 0, &mut session).unwrap();
        let path = state_path(&root, ID, 0);
        let mut bytes = std::fs::read(&path).unwrap();
        let middle = bytes.len() / 2;
        bytes[middle] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();

        assert!(load(&root, ID, 0, &mut session).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The thumbnail is an ornament: if it is corrupt, the slot shows up without an image and
    /// loads the same.
    #[test]
    fn a_corrupt_thumbnail_does_not_block_the_state() {
        let root = root("miniatura");
        let mut session = minimal_session_for_save_state();
        save(&root, ID, 1, &mut session).unwrap();
        std::fs::write(thumbnail_path(&root, ID, 1), b"lixo").unwrap();

        let slot = list(&root, ID)[1]
            .clone()
            .expect("the state stays in the slot");
        assert!(slot.thumbnail.is_none());
        let loaded = load(&root, ID, 1, &mut session).unwrap();
        assert!(loaded.thumbnail.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A crash between moving the old state and publishing the new one leaves only the `.bak`,
    /// and it becomes the slot's state again.
    #[test]
    fn the_backup_of_an_interrupted_write_comes_back() {
        let root = root("backup");
        let mut session = minimal_session_for_save_state();
        save(&root, ID, 2, &mut session).unwrap();
        let path = state_path(&root, ID, 2);
        std::fs::rename(&path, path.with_extension("zbxstate.bak")).unwrap();

        assert!(list(&root, ID)[2].is_some());
        assert!(path.is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn size_picks_the_unit() {
        assert_eq!(size(512), "512 B");
        assert_eq!(size(3 * 1024), "3 KB");
        assert_eq!(size(5 * 1024 * 1024 + 512 * 1024), "5.5 MB");
    }
}
