//! What the Z-Wheel knows about the games: the box, the official name and the description of each.
//!
//! The Z-Wheel does not take the cover from the game — it carries the covers in its own package,
//! in `mod/<id>/assets/games/<game_id>/`, and links each folder to the game through the
//! `tt_game_info` database (SQLite): the `GAMEINFO` table maps `game_id` to the applet's
//! `class_id` and to the cover path, and `TITLETEXT` stores the name in each language. The
//! `class_id` is the same ClassID the library uses as a key, so the link does not depend on a file
//! or folder name.
//!
//! The logo on the top reel comes from another database, `asset_cache`: the `ASSETS` table says
//! which game (`owner`, the `game_id`) each stage scene belongs to
//! (`assets/stage_slides/<dslid>/`), and the scene carries the logo as a texture,
//! `slidebanner.qxt`. Only the featured games have a scene — 12 in the package.
//!
//! The package may be loose in a folder or in a `.zip`; both are read without extracting anything.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::video::icon::{self, Image};

/// The Z-Wheel database, next to `tectoy.mod`.
const DATABASE: &str = "tt_game_info";
/// The database that links the stage scenes to the games.
const SCENES: &str = "asset_cache";
/// The type of the stage scenes in the `ASSETS` table. The others are the help pages (6 and 7) and
/// the cover folders (8).
const SCENE_KIND: i64 = 5;
/// The QX texture format of the logos: color-only ATITC, 8-byte blocks.
const QXT_ATC_RGB: u32 = 0x0c;
/// The header of a `.qxt` before the texels.
const QXT_HEADER: usize = 40;

/// What the Z-Wheel keeps about a game.
#[derive(Debug, Clone, Default)]
pub struct GameEntry {
    /// Per language, keyed by the two-letter code (`pt`, `en`, `es`).
    titles: HashMap<String, String>,
    descriptions: HashMap<String, String>,
    pub cover: Option<Image>,
    /// The logo the game shows on the Z-Wheel's top reel, when it has a scene on the stage.
    pub logo: Option<Image>,
    /// The age rating (`rating.jpg`).
    pub rating: Option<Image>,
}

impl GameEntry {
    pub fn title(&self, language: &str) -> Option<&str> {
        pick(&self.titles, language)
    }

    pub fn description(&self, language: &str) -> Option<&str> {
        pick(&self.descriptions, language)
    }
}

/// The requested language, and failing that English: it is what every entry carries.
fn pick<'a>(map: &'a HashMap<String, String>, language: &str) -> Option<&'a str> {
    let short = language.get(..2).unwrap_or(language).to_ascii_lowercase();
    map.get(&short)
        .or_else(|| map.get("en"))
        .or_else(|| map.values().next())
        .map(String::as_str)
}

/// The name a game shows up with: the Z-Wheel's official one in the interface language, when it
/// knows the game, and the folder's or package's otherwise.
pub fn title_of(zwheel_metadata: Option<&ZWheelMetadata>, game: &crate::library::Game, language: &str) -> String {
    game.clsid
        .and_then(|class_id| zwheel_metadata?.info(class_id))
        .and_then(|info| info.title(language))
        .map(str::to_string)
        .unwrap_or_else(|| game.title.clone())
}

/// A game's image in the library, falling back to `fallback` when it has none.
///
/// The cover left next to the game is the choice of whoever assembled the folder, and outranks the
/// Z-Wheel's; the Z-Wheel's outranks the `.mif` icon. The cover next to the game is already in
/// `game.art`, when it exists; `has_sidecar_cover` says whether it exists, and comes from the
/// caller — it is [`has_sidecar_cover`], which reads the disk, and whoever draws the whole list
/// needs to keep the answer instead of asking every frame.
pub fn game_image<'a>(
    game: &'a crate::library::Game,
    info: Option<&'a GameEntry>,
    fallback: Option<&'a Image>,
    has_sidecar_cover: bool,
) -> Option<&'a Image> {
    let cover = info.and_then(|info| info.cover.as_ref()).filter(|_| !has_sidecar_cover);
    cover.or(game.art.as_ref()).or(fallback)
}

/// Whether there is a cover left next to the game. Reads the disk.
pub fn has_sidecar_cover(game: &crate::library::Game) -> bool {
    crate::library::cover(&game.path).is_some()
}

/// Whether the image is scaled up without interpolating in a frame of this size.
///
/// A 26-pixel icon shows up scaled four times: interpolating would turn it into a blur, and the
/// square block is what the console showed. A large image already comes in scaled down, and then
/// interpolation is what avoids the jaggies.
pub fn upscale_nearest(image: &Image, frame: [f32; 2]) -> bool {
    (image.width as f32) < frame[0] && (image.height as f32) < frame[1]
}

/// The entries of every game the Z-Wheel knows, by ClassID.
#[derive(Debug, Default)]
pub struct ZWheelMetadata {
    records: HashMap<u32, GameEntry>,
}

impl ZWheelMetadata {
    /// Reads the metadata from the Z-Wheel package at `path` — the `.zip`, the folder or the
    /// `.mod`.
    pub fn load(path: &Path) -> Option<Self> {
        let mut source = Source::open(path)?;
        let soundfont = source.read(DATABASE)?;
        let lines = read_database(&soundfont)
            .inspect_err(|err| eprintln!("Z-Wheel metadata: {err}"))
            .ok()?;
        // Without the scenes database the metadata still works: there are just no logos.
        let scenes = source
            .read(SCENES)
            .and_then(|data| {
                read_scenes(&data)
                    .inspect_err(|err| eprintln!("Z-Wheel scenes: {err}"))
                    .ok()
            })
            .unwrap_or_default();
        let mut records = HashMap::new();
        for (game_id, class_id, dir, titles) in lines {
            let dir = dir.trim_start_matches("./").trim_end_matches('/');
            let mut descriptions = HashMap::new();
            for language in ["pt", "en", "es"] {
                if let Some(text) = source.read(&format!("{dir}/description_{language}.txt")) {
                    descriptions.insert(language.to_string(), clean_description(&text));
                }
            }
            // The large one first: the small one is the same art at 160×227 with the console's
            // border.
            let cover = ["boxartlg.jpg", "boxart.bmp"]
                .iter()
                .filter_map(|name| source.read(&format!("{dir}/{name}")))
                .find_map(|data| icon::decode(&data).ok());
            let rating = source
                .read(&format!("{dir}/rating.jpg"))
                .and_then(|data| icon::decode(&data).ok());
            let logo = scenes.get(&game_id).and_then(|scene| {
                let scene = scene.trim_start_matches("./").trim_end_matches('/');
                qx_texture(&source.read(&format!("{scene}/slidebanner.qxt"))?)
            });
            records.insert(
                class_id,
                GameEntry {
                    titles,
                    descriptions,
                    cover,
                    logo,
                    rating,
                },
            );
        }
        Some(Self { records })
    }

    pub fn info(&self, class_id: u32) -> Option<&GameEntry> {
        self.records.get(&class_id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }
}

/// Opens a copy of the `name` database with the contents `data`.
///
/// SQLite only opens files. Copying to the cache serves both sources the same way, and a running
/// Z-Wheel never has its database opened by us.
fn open_copy(name: &str, data: &[u8]) -> rusqlite::Result<rusqlite::Connection> {
    let copy = crate::loader::archive::cache_dir().join(format!("z-wheel-{name}.db"));
    if let Some(dir) = copy.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&copy, data)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(err.into()))?;
    rusqlite::Connection::open_with_flags(&copy, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
}

/// The folder of each game's first scene, by `game_id`. Crash and Rally Master Pro have two, with
/// the same logo.
fn read_scenes(data: &[u8]) -> rusqlite::Result<HashMap<i64, String>> {
    let connection = open_copy(SCENES, data)?;
    let mut query = connection
        .prepare("SELECT owner, path FROM ASSETS WHERE type = ?1 AND owner <> 0 ORDER BY dslid")?;
    let lines = query.query_map([SCENE_KIND], |line| {
        Ok((line.get::<_, i64>(0)?, line.get::<_, String>(1)?))
    })?;
    let mut scenes = HashMap::new();
    for line in lines {
        let (game, dir) = line?;
        scenes.entry(game).or_insert(dir);
    }
    Ok(scenes)
}

/// A QXEngine texture, in the only format the logos use.
///
/// The header has the signature `QX\0\0QXT\0`, the width at `+12`, the height at `+16` and the
/// format at `+20`; the texels start at `+40`. The model textures use other formats, which are
/// left for when the models get read.
fn qx_texture(data: &[u8]) -> Option<Image> {
    if data.get(..8)? != b"QX\0\0QXT\0" {
        return None;
    }
    let field = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
    let (width, height, format) = (field(12)? as usize, field(16)? as usize, field(20)?);
    let blocks = width.div_ceil(4) * height.div_ceil(4) * 8;
    if format != QXT_ATC_RGB || width == 0 || height == 0 || width * height > 1024 * 1024 {
        return None;
    }
    let texels = data.get(QXT_HEADER..QXT_HEADER + blocks)?;
    let rgba = crate::video::atc::decode(texels, width, height, false)
        .into_iter()
        .flatten()
        .collect();
    Some(Image {
        width,
        height,
        rgba,
    })
}

/// `(game_id, class_id, cover dir, titles by language)` for each game in the database.
#[allow(clippy::type_complexity)]
fn read_database(data: &[u8]) -> rusqlite::Result<Vec<(i64, u32, String, HashMap<String, String>)>> {
    let connection = open_copy(DATABASE, data)?;
    let mut titles: HashMap<i64, HashMap<String, String>> = HashMap::new();
    let mut query = connection.prepare("SELECT game_id, lang_id, titletext FROM TITLETEXT")?;
    let lines = query.query_map([], |line| {
        Ok((
            line.get::<_, i64>(0)?,
            line.get::<_, i64>(1)?,
            line.get::<_, String>(2)?,
        ))
    })?;
    for line in lines {
        let (game, language, title) = line?;
        titles
            .entry(game)
            .or_default()
            .insert(brew_language(language), title);
    }
    let mut query = connection.prepare("SELECT game_id, class_id, boxart_path FROM GAMEINFO")?;
    let lines = query.query_map([], |line| {
        Ok((
            line.get::<_, i64>(0)?,
            line.get::<_, i64>(1)?,
            line.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut games = Vec::new();
    for line in lines {
        let (game, class_id, dir) = line?;
        let Some(dir) = dir else { continue };
        games.push((
            game,
            class_id as u32,
            dir,
            titles.remove(&game).unwrap_or_default(),
        ));
    }
    Ok(games)
}

/// The BREW language is an integer with the letters in little-endian: `"pt  "` is `0x20207470`.
fn brew_language(code: i64) -> String {
    let letters = (code as u32).to_le_bytes();
    String::from_utf8_lossy(&letters).trim().to_ascii_lowercase()
}

/// The description without the notice about the store that closed in 2011, which opens almost all
/// of them between `**`.
fn clean_description(data: &[u8]) -> String {
    let text = String::from_utf8_lossy(data);
    let text = text.trim_start_matches('\u{feff}').trim();
    let without_notice = match text
        .strip_prefix("**")
        .and_then(|rest| rest.split_once("**"))
    {
        Some((_, after)) => after.trim_start(),
        None => text,
    };
    without_notice.trim().to_string()
}

/// Where the package files come from: a folder, or a zip with the prefix up to the module folder.
enum Source {
    Dir(PathBuf),
    Zip(zip::ZipArchive<std::fs::File>, String),
}

impl Source {
    fn open(path: &Path) -> Option<Self> {
        let is_zip = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
        if is_zip {
            let file = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
            let prefix = file
                .file_names()
                .find_map(|name| name.ok()?.strip_suffix(DATABASE).map(str::to_string))
                .filter(|prefix| prefix.is_empty() || prefix.ends_with('/'))?;
            return Some(Self::Zip(file, prefix));
        }
        let dir = if path.is_dir() {
            path.to_path_buf()
        } else {
            path.parent()?.to_path_buf()
        };
        find_database(&dir, 3).map(Self::Dir)
    }

    fn read(&mut self, relative: &str) -> Option<Vec<u8>> {
        match self {
            Self::Dir(dir) => std::fs::read(dir.join(relative)).ok(),
            Self::Zip(file, prefix) => {
                let mut entry = file.by_name(&format!("{prefix}{relative}")).ok()?;
                let mut data = Vec::new();
                entry.read_to_end(&mut data).ok()?;
                Some(data)
            }
        }
    }
}

/// The folder that holds the database, descending from `dir` up to `depth` levels: the package
/// root has the database in `mod/<id>/`.
fn find_database(dir: &Path, depth: usize) -> Option<PathBuf> {
    if dir.join(DATABASE).is_file() {
        return Some(dir.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    children.sort();
    children
        .into_iter()
        .find_map(|child| find_database(&child, depth - 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_brew_language_comes_from_little_endian_letters() {
        assert_eq!(brew_language(0x2020_7470), "pt");
        assert_eq!(brew_language(538996325), "en");
    }

    #[test]
    fn the_description_loses_the_store_notice() {
        let text = "** Título disponível até 2011. **\nEste incrível jogo de xadrez.".as_bytes();
        assert_eq!(clean_description(text), "Este incrível jogo de xadrez.");
        assert_eq!(clean_description(b"Sem aviso."), "Sem aviso.");
    }

    #[test]
    fn without_the_requested_language_english_applies() {
        let map = HashMap::from([("en".to_string(), "Alien Breaker".to_string())]);
        assert_eq!(pick(&map, "pt-BR"), Some("Alien Breaker"));
    }
}

/// With the real package: `ZEEBX_Z_WHEEL=<zip or dir> cargo test --release zwheel_metadata --
/// --ignored`.
#[cfg(test)]
mod real_package {
    use super::*;

    #[test]
    #[ignore]
    fn the_real_packages_metadata_has_covers_and_names() {
        let path = PathBuf::from(std::env::var("ZEEBX_Z_WHEEL").expect("ZEEBX_Z_WHEEL"));
        let zwheel_metadata = ZWheelMetadata::load(&path).expect("metadata");
        let with_cover = zwheel_metadata.records.values().filter(|f| f.cover.is_some()).count();
        let with_logo = zwheel_metadata.records.values().filter(|f| f.logo.is_some()).count();
        println!("{with_logo} with a logo");
        assert_eq!(with_logo, 12);
        let alien = zwheel_metadata
            .records
            .values()
            .find(|f| f.title("pt") == Some("Alien Breaker"));
        println!("{} entries, {with_cover} with a cover", zwheel_metadata.len());
        assert!(zwheel_metadata.len() > 0 && with_cover == zwheel_metadata.len() && alien.is_some());
    }
}
