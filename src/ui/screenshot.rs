//! Where and how a screenshot is written. See `docs/implementation/22-screenshots.md`.
//!
//! Which frame comes out is up to [`crate::session::Session::capture`]; the rest lives here,
//! without a window: the name of each game's folder, the file name and the `.png`. The shortcut
//! and the notice belong to each frontend.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

/// The folder for whoever did not choose another.
pub fn default_dir() -> PathBuf {
    crate::config::config_dir().join("screenshots")
}

/// The actual folder: the chosen one, or the default.
pub fn dir(chosen: Option<&Path>) -> PathBuf {
    chosen.map_or_else(default_dir, Path::to_path_buf)
}

/// The title as a folder and file name on all three systems.
///
/// What Windows rejects goes, and so do trailing dots and spaces, which it silently strips: a
/// folder created on Linux as `Game.` would turn into another one when copied. A title with
/// nothing left becomes `Zeebx`.
pub fn safe_name(title: &str) -> String {
    let clean: String = title
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean = clean.trim_end_matches(['.', ' ']);
    match clean.is_empty() {
        true => "Zeebx".to_string(),
        false => clean.to_string(),
    }
}

/// Creates the screenshot file, with a name nobody has: `<title> - <timestamp>.png`, and
/// ` (2)`, ` (3)`… when two land in the same second.
///
/// **The reservation is `create_new`**, not a check beforehand. Two screenshots in a row write on
/// different threads; "is the name free?" followed by "create" would leave both with the same.
fn create_unused(dir: &Path, base: &str) -> io::Result<(File, PathBuf)> {
    for n in 1u32.. {
        let name = match n {
            1 => format!("{base}.png"),
            n => format!("{base} ({n}).png"),
        };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!("ran out of numbers before names")
}

/// Writes the frame to `<root>/<title>/<title> - <timestamp>.png` and returns the path.
///
/// `rgb` is three bytes per pixel, rows top to bottom. `timestamp` is the local time already as
/// text: the core does not know time zones, and the caller has one.
pub fn save(
    root: &Path,
    title: &str,
    timestamp: &str,
    width: u32,
    height: u32,
    rgb: &[u8],
) -> io::Result<PathBuf> {
    let title = safe_name(title);
    let dir = root.join(&title);
    std::fs::create_dir_all(&dir)?;
    let (file, path) = create_unused(&dir, &format!("{title} - {}", safe_name(timestamp)))?;
    let written = write_png(file, width, height, rgb);
    // A half-written file is not a screenshot: if the write failed, the reserved name goes too.
    if written.is_err() {
        let _ = std::fs::remove_file(&path);
    }
    written.map(|()| path)
}

fn write_png(file: File, width: u32, height: u32, rgb: &[u8]) -> io::Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    writer.write_image_data(rgb).map_err(io::Error::other)?;
    writer.finish().map_err(io::Error::other)
}

/// The `file://` address of a folder, for the system to open in the file manager.
///
/// Escaped by hand: a `#` or a `?` in a folder name would be read as the start of another part of
/// the address, and the file manager would open somewhere else.
pub fn url_of(dir: &Path) -> String {
    let text = dir.display().to_string().replace('\\', "/");
    let mut address = String::from("file://");
    if !text.starts_with('/') {
        // `C:/...` on Windows: the address is `file:///C:/...`.
        address.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                address.push(byte as char)
            }
            other => address.push_str(&format!("%{other:02X}")),
        }
    }
    address
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_drops_what_windows_rejects() {
        assert_eq!(safe_name("Crash: Nitro Kart"), "Crash Nitro Kart");
        assert_eq!(safe_name("A/B\\C?*"), "A B C");
        assert_eq!(safe_name("Jogo..."), "Jogo");
        assert_eq!(safe_name("Zeebo Extreme Bóia Cross"), "Zeebo Extreme Bóia Cross");
        assert_eq!(safe_name(" :?. "), "Zeebx");
        assert_eq!(safe_name("2026-09-25 14-03-07"), "2026-09-25 14-03-07");
    }

    #[test]
    fn two_screenshots_in_the_same_second_do_not_overwrite_each_other() {
        let root = std::env::temp_dir().join(format!("zeebx-testes-screenshot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let black = vec![0u8; 4 * 3 * 3];
        let first = save(&root, "Quake", "2026-09-25 14-03-07", 4, 3, &black).unwrap();
        let second = save(&root, "Quake", "2026-09-25 14-03-07", 4, 3, &black).unwrap();
        assert_eq!(first, root.join("Quake").join("Quake - 2026-09-25 14-03-07.png"));
        assert_eq!(second, root.join("Quake").join("Quake - 2026-09-25 14-03-07 (2).png"));

        // What was written is a 4×3 PNG, RGB, without alpha.
        let reader = png::Decoder::new(io::BufReader::new(File::open(&first).unwrap()));
        let reader = reader.read_info().unwrap();
        let info = reader.info();
        assert_eq!((info.width, info.height), (4, 3));
        assert_eq!(info.color_type, png::ColorType::Rgb);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_short_frame_does_not_leave_a_half_written_file() {
        let root = std::env::temp_dir().join(format!("zeebx-testes-screenshot-curto-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        assert!(save(&root, "Quake", "agora", 4, 3, &[0u8; 5]).is_err());
        assert_eq!(std::fs::read_dir(root.join("Quake")).unwrap().count(), 0);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_dir_url_escapes_what_would_confuse_it() {
        assert_eq!(
            url_of(Path::new("/home/eu/Meus prints#1")),
            "file:///home/eu/Meus%20prints%231"
        );
        assert_eq!(url_of(Path::new("/a/Bóia")), "file:///a/B%C3%B3ia");
    }
}
