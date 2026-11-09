//! The game library, as a list model for QML, and its images.
//!
//! The list is egui's: in title order, without the Z-Wheel, with the official name it carries,
//! and filtered by the search. The images reach QML through `image://zeebx/…` — see
//! `cpp/images.h`.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++Qt" {
        include!(<QtCore/QAbstractListModel>);
        #[qobject]
        type QAbstractListModel;
    }

    unsafe extern "C++" {
        include!("cxx-qt-lib/qhash.h");
        type QHash_i32_QByteArray = cxx_qt_lib::QHash<cxx_qt_lib::QHashPair_i32_QByteArray>;
        include!("cxx-qt-lib/qvariant.h");
        type QVariant = cxx_qt_lib::QVariant;
        include!("cxx-qt-lib/qmodelindex.h");
        type QModelIndex = cxx_qt_lib::QModelIndex;
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qlist.h");
        type QList_i32 = cxx_qt_lib::QList<i32>;
        include!("cxx-qt-lib/qimage.h");
        type QImage = cxx_qt_lib::QImage;
        include!("cxx-qt-lib/qqmlengine.h");
        type QQmlEngine = cxx_qt_lib::QQmlEngine;
        include!("cxx-qt-lib/qmap.h");
        type QMap_QString_QVariant = cxx_qt_lib::QMap<cxx_qt_lib::QMapPair_QString_QVariant>;
    }

    // See `cpp/images.h`.
    #[namespace = "zeebx"]
    unsafe extern "C++" {
        include!("images.h");
        fn register_images(engine: Pin<&mut QQmlEngine>, source: fn(&str) -> QImage);
    }

    unsafe extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[base = QAbstractListModel]
        #[qproperty(QString, count)]
        #[qproperty(QString, empty)]
        type Library = super::LibraryRust;

        #[qinvokable]
        #[cxx_override]
        fn data(self: &Library, index: &QModelIndex, role: i32) -> QVariant;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "rowCount"]
        fn row_count(self: &Library, parent: &QModelIndex) -> i32;

        #[qinvokable]
        #[cxx_override]
        #[cxx_name = "roleNames"]
        fn role_names(self: &Library) -> QHash_i32_QByteArray;

        /// How many rows the list has. `rowCount` asks for the parent index, which QML does not
        /// pass.
        #[qinvokable]
        fn lines(self: &Library) -> i32;

        /// The roles of any row, as an object: the slider shows the games around the selection,
        /// wrapping around the list, and not one row per delegate.
        #[qinvokable]
        fn item(self: &Library, line: i32) -> QMap_QString_QVariant;

        /// Opens the game of the given row. Returns empty, or the reason it did not open.
        #[qinvokable]
        fn open(self: &Library, line: i32) -> QString;

        /// The key of the row's game's things, such as its settings. Empty if the row does not
        /// exist. See `zeebx::library::game_key`.
        #[qinvokable]
        #[cxx_name = "keyId"]
        fn key_id(self: &Library, line: i32) -> QString;

        /// What the details window shows about the row's game.
        #[qinvokable]
        fn details(self: &Library, line: i32) -> QMap_QString_QVariant;

        /// Opens the Z-Wheel. Returns empty, or the reason it did not open.
        #[qinvokable]
        #[cxx_name = "openZWheel"]
        fn open_z_wheel(self: &Library) -> QString;

        /// Opens the game at a path — the one that came on the command line.
        #[qinvokable]
        #[cxx_name = "openPath"]
        fn open_path(self: &Library, path: &QString) -> QString;

        /// Whether there is a Z-Wheel for the button to open.
        #[qinvokable]
        #[cxx_name = "hasZWheel"]
        fn has_z_wheel(self: &Library) -> bool;

        /// How the main window opens: 0 windowed, 1 maximized, 2 fullscreen. See
        /// `graphics.window`.
        #[qinvokable]
        #[cxx_name = "windowMode"]
        fn window_mode(self: &Library) -> i32;

        /// The width the main window had when it was closed, in points, or zero with no stored
        /// size. See `graphics.window_size`.
        #[qinvokable]
        #[cxx_name = "windowWidth"]
        fn window_width(self: &Library) -> f64;

        /// The height the main window had when it was closed, in points, or zero with no stored
        /// size. See `graphics.window_size`.
        #[qinvokable]
        #[cxx_name = "windowHeight"]
        fn window_height(self: &Library) -> f64;

        /// Stores the main window's current size. The window calls this after a while without
        /// changing size, and when closing — maximized or fullscreen, the size is the screen's,
        /// not the user's choice, and the window does not call it. See
        /// `graphics.window_size`.
        #[qinvokable]
        #[cxx_name = "saveWindowSize"]
        fn save_window_size(self: &Library, width: f64, height: f64);

        /// How the library is shown: 0 as a grid, 1 as the slider. See `library` in the
        /// settings.
        #[qinvokable]
        #[cxx_name = "libraryMode"]
        fn library_mode(self: &Library) -> i32;

        /// The search changed: the list now holds only what matches it.
        #[qinvokable]
        fn search(self: Pin<&mut Library>, text: &QString);

        /// Rebuilds the list without scanning the folder: the Z-Wheel or the language changed, and
        /// the names with them.
        #[qinvokable]
        fn rebuild(self: Pin<&mut Library>);

        /// Scans the ROMs folder again, and rereads the Z-Wheel and its collection.
        #[qinvokable]
        #[cxx_name = "searchAgain"]
        fn search_again(self: Pin<&mut Library>);

        /// What the controller asks of the library in this read: 0 up, 1 down, 2 left,
        /// 3 right, 4 open, 5 Z-Wheel. `listening` false is the library without focus.
        #[qinvokable]
        fn commands(self: &Library, listening: bool) -> QList_i32;

        /// Moves the Zeeboids sync lock back by one day. Returns what to say.
        #[qinvokable]
        #[cxx_name = "unlockSync"]
        fn unlock_sync(self: &Library) -> QString;


        #[inherit]
        fn index(self: &Library, row: i32, column: i32, parent: &QModelIndex) -> QModelIndex;

        #[inherit]
        #[cxx_name = "beginResetModel"]
        unsafe fn begin_reset_model(self: Pin<&mut Library>);

        #[inherit]
        #[cxx_name = "endResetModel"]
        unsafe fn end_reset_model(self: Pin<&mut Library>);
    }

    impl cxx_qt::Initialize for Library {}
}

use std::path::{Path, PathBuf};
use std::pin::Pin;

use cxx_qt_lib::{
    QByteArray, QHash, QHashPair_i32_QByteArray, QImage, QImageFormat, QList, QMap,
    QMapPair_QString_QVariant, QModelIndex, QString, QVariant,
};

use zeebx::loader::archive;
use zeebx::bridge;
use zeebx::ui::zwheel_metadata;
use zeebx::ui::navigation::Command;
use zeebx::ui::settings::LibraryMode;

use super::emu_core;

/// The first role free for the model: below it are Qt's own, such as `DisplayRole`.
const USER_ROLE: i32 = 0x0100;
const TITLE: i32 = USER_ROLE;
const PATH: i32 = USER_ROLE + 1;
const DESCRIPTION: i32 = USER_ROLE + 2;
const CLASS_ID: i32 = USER_ROLE + 3;
const COVER: i32 = USER_ROLE + 4;
const SHARP_COVER: i32 = USER_ROLE + 5;
const LOGO: i32 = USER_ROLE + 6;
const RATING: i32 = USER_ROLE + 7;

/// The image frame in a grid card, portrait, in the proportion of the boxes the Z-Wheel carries
/// (170×220). `LibraryGrid.qml` draws the card with these measurements.
const COVER_FRAME: [f32; 2] = [112.0, 145.0];

#[derive(Default)]
pub struct LibraryRust {
    count: QString,
    empty: QString,
}

/// The game file of a row of the list.
fn row_path(emu_core: &emu_core::Core, line: i32) -> Option<PathBuf> {
    let line = usize::try_from(line).ok()?;
    let &(_, index) = emu_core.list.get(line)?;
    Some(emu_core.games[index].path.clone())
}

/// A file size for people to read: "512 KB", "34,2 MB" — or "34.2 MB" in English.
fn readable_size(bytes: u64, decimal_comma: bool) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let text = match bytes {
        0..1_048_576 => format!("{} KB", bytes.div_ceil(1024)),
        _ => format!("{:.1} MB", bytes as f64 / MB),
    };
    match decimal_comma {
        true => text.replace('.', ","),
        false => text,
    }
}

/// The result of an opening, as QML reads it: empty is success.
fn response(opened: Result<(), String>) -> QString {
    match opened {
        Ok(()) => QString::default(),
        Err(error) => QString::from(&error),
    }
}

/// The image QML asked for, by its address without the scheme. It is the function the provider
/// calls.
fn image(address: &str) -> QImage {
    emu_core::with(|emu_core| {
        // The controller art is made on the spot, in the requested color; the rest is already in
        // memory.
        let tinted;
        let image = match address.starts_with("controle/") {
            true => {
                tinted = emu_core.controller_image(address);
                tinted.as_ref()
            }
            false => emu_core.image(address),
        };
        let Some(image) = image else {
            return QImage::default();
        };
        // SAFETY: `rgba` has `width × height × 4` bytes, which is `Format_RGBA8888`, and a row of
        // four-byte pixels always has the alignment `QImage` requires.
        unsafe {
            QImage::from_raw_bytes(
                image.rgba.clone(),
                image.width as i32,
                image.height as i32,
                QImageFormat::Format_RGBA8888,
            )
        }
    })
}

/// Registers the image provider in the engine: `image://zeebx/…`.
pub fn register_images(engine: Pin<&mut qobject::QQmlEngine>) {
    qobject::register_images(engine, image);
}

impl cxx_qt::Initialize for qobject::Library {
    fn initialize(self: Pin<&mut Self>) {
        self.refresh_texts();
    }
}

impl qobject::Library {
    pub fn data(&self, index: &QModelIndex, role: i32) -> QVariant {
        let Ok(line) = usize::try_from(index.row()) else {
            return QVariant::default();
        };
        let text = |text: &str| QVariant::from(&QString::from(text));
        emu_core::with(|emu_core| {
            let Some((title, game_index)) = emu_core.list.get(line).cloned() else {
                return QVariant::default();
            };
            let generation = emu_core.generation;
            let class_id = emu_core.games[game_index].clsid;
            let info = emu_core.info(line);
            match role {
                TITLE => text(&title),
                PATH => text(&emu_core.games[game_index].path.display().to_string()),
                DESCRIPTION => {
                    let language = emu_core.catalog.current();
                    text(info.and_then(|info| info.description(language)).unwrap_or_default())
                }
                CLASS_ID => text(&class_id.map(|c| format!("{c:#010x}")).unwrap_or_default()),
                COVER => text(&format!("image://zeebx/cover/{generation}/{game_index}")),
                SHARP_COVER => {
                    let sharp = emu_core.cover(game_index).is_some_and(|image| {
                        zwheel_metadata::upscale_nearest(image, COVER_FRAME)
                    });
                    QVariant::from(&sharp)
                }
                LOGO => match info.is_some_and(|info| info.logo.is_some()) {
                    true => text(&format!("image://zeebx/logo/{generation}/{}", class_id.unwrap_or(0))),
                    false => text(""),
                },
                RATING => match info.is_some_and(|info| info.rating.is_some()) {
                    true => text(&format!(
                        "image://zeebx/rating/{generation}/{}",
                        class_id.unwrap_or(0)
                    )),
                    false => text(""),
                },
                _ => QVariant::default(),
            }
        })
    }

    pub fn row_count(&self, _parent: &QModelIndex) -> i32 {
        emu_core::with(|emu_core| emu_core.list.len() as i32)
    }

    pub fn role_names(&self) -> QHash<QHashPair_i32_QByteArray> {
        let mut roles = QHash::<QHashPair_i32_QByteArray>::default();
        for (role, name) in [
            (TITLE, "title"),
            (PATH, "path"),
            (DESCRIPTION, "description"),
            (CLASS_ID, "classId"),
            (COVER, "cover"),
            (SHARP_COVER, "sharpCover"),
            (LOGO, "logo"),
            (RATING, "rating"),
        ] {
            roles.insert(role, QByteArray::from(name));
        }
        roles
    }

    pub fn lines(&self) -> i32 {
        emu_core::with(|emu_core| emu_core.list.len() as i32)
    }

    pub fn item(&self, line: i32) -> QMap<QMapPair_QString_QVariant> {
        let mut item = QMap::<QMapPair_QString_QVariant>::default();
        let index = self.index(line, 0, &QModelIndex::default());
        for (role, name) in [
            (TITLE, "title"),
            (DESCRIPTION, "description"),
            (COVER, "cover"),
            (SHARP_COVER, "sharpCover"),
            (LOGO, "logo"),
            (RATING, "rating"),
        ] {
            item.insert(QString::from(name), self.data(&index, role));
        }
        item
    }

    pub fn open(&self, line: i32) -> QString {
        response(emu_core::with(|emu_core| {
            let path = usize::try_from(line)
                .ok()
                .and_then(|line| emu_core.list.get(line))
                .map(|&(_, index)| emu_core.games[index].path.clone())
                .ok_or_else(|| "that game is not in the list".to_string())?;
            emu_core.open_from_library(&path)
        }))
    }

    pub fn key_id(&self, line: i32) -> QString {
        let path = emu_core::with(|emu_core| row_path(emu_core, line));
        QString::from(path.and_then(|c| zeebx::library::game_key(&c)).unwrap_or_default())
    }

    pub fn details(&self, line: i32) -> QMap<QMapPair_QString_QVariant> {
        let mut details = self.item(line);
        let Some(path) = emu_core::with(|emu_core| row_path(emu_core, line)) else {
            return details;
        };
        let key_id = zeebx::library::game_key(&path).unwrap_or_default();
        let index = self.index(line, 0, &QModelIndex::default());
        let text = |text: &str| QVariant::from(&QString::from(text));
        details.insert(QString::from("classId"), self.data(&index, CLASS_ID));
        details.insert(QString::from("path"), text(&path.display().to_string()));
        details.insert(
            QString::from("module"),
            text(&zeebx::library::module_id(&path).unwrap_or_default()),
        );
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let (custom, decimal_comma) = emu_core::with(|emu_core| {
            (emu_core.custom(&key_id), !emu_core.catalog.current().starts_with("en"))
        });
        details.insert(QString::from("size"), text(&readable_size(bytes, decimal_comma)));
        details.insert(QString::from("custom"), QVariant::from(&custom));
        details.insert(QString::from("keyId"), text(&key_id));
        details
    }

    pub fn open_z_wheel(&self) -> QString {
        response(emu_core::with(|emu_core| {
            let path = emu_core
                .z_wheel
                .clone()
                .ok_or_else(|| emu_core.catalog.get("nav.z_wheel.missing").to_string())?;
            emu_core.open_from_library(&path)
        }))
    }

    pub fn open_path(&self, path: &QString) -> QString {
        let path = PathBuf::from(String::from(path));
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        response(emu_core::with(|emu_core| emu_core.open_from_library(Path::new(&path))))
    }

    pub fn has_z_wheel(&self) -> bool {
        emu_core::with(|emu_core| emu_core.z_wheel.is_some())
    }

    pub fn window_mode(&self) -> i32 {
        emu_core::with(|emu_core| super::bridge::mode(emu_core.settings.graphics.window))
    }

    pub fn window_width(&self) -> f64 {
        emu_core::with(|emu_core| {
            emu_core.settings.graphics.window_size.map_or(0.0, |[width, _]| width as f64)
        })
    }

    pub fn window_height(&self) -> f64 {
        emu_core::with(|emu_core| {
            emu_core.settings.graphics.window_size.map_or(0.0, |[_, height]| height as f64)
        })
    }

    pub fn save_window_size(&self, width: f64, height: f64) {
        emu_core::with(|emu_core| {
            emu_core.settings.graphics.window_size =
                (width > 0.0 && height > 0.0).then_some([width as f32, height as f32]);
            if let Err(error) = emu_core.settings.save() {
                eprintln!("could not save the settings: {error}");
            }
        })
    }

    pub fn library_mode(&self) -> i32 {
        emu_core::with(|emu_core| match emu_core.settings.library {
            LibraryMode::Grid => 0,
            LibraryMode::Slider => 1,
        })
    }

    pub fn search(mut self: Pin<&mut Self>, text: &QString) {
        let text = String::from(text);
        // The reset stays outside `emu_core::with`: it notifies QML, which asks for the rows back.
        unsafe { self.as_mut().begin_reset_model() };
        emu_core::with(|emu_core| emu_core.set_search(&text));
        unsafe { self.as_mut().end_reset_model() };
        self.refresh_texts();
    }

    pub fn rebuild(mut self: Pin<&mut Self>) {
        unsafe { self.as_mut().begin_reset_model() };
        emu_core::with(|emu_core| emu_core.rebuild_list());
        unsafe { self.as_mut().end_reset_model() };
        self.refresh_texts();
    }

    pub fn search_again(mut self: Pin<&mut Self>) {
        unsafe { self.as_mut().begin_reset_model() };
        emu_core::with(|emu_core| emu_core.search_again());
        unsafe { self.as_mut().end_reset_model() };
        self.refresh_texts();
    }

    pub fn commands(&self, listening: bool) -> QList<i32> {
        let commands = emu_core::with(|emu_core| {
            // The same timer reads the controller and keeps the rest up to date: the Discord
            // presence and the answer of the new-version check.
            emu_core.each_frame();
            emu_core.commands(listening)
        });
        let codes: Vec<i32> = commands
            .into_iter()
            .map(|command| match command {
                Command::Up => 0,
                Command::Down => 1,
                Command::Left => 2,
                Command::Right => 3,
                Command::Open => 4,
                Command::ZWheel => 5,
                Command::Configure => 6,
                Command::Inspect => 7,
            })
            .collect();
        QList::from(codes)
    }

    pub fn unlock_sync(&self) -> QString {
        let message = match bridge::unlock_sync(&archive::device_dir()) {
            Ok(_) => emu_core::with(|emu_core| emu_core.catalog.get("nav.unlock_sync.done").to_string()),
            Err(error) => error,
        };
        QString::from(&message)
    }

    /// The count and the empty-list message, which change with the search and with the scan.
    fn refresh_texts(mut self: Pin<&mut Self>) {
        let (count, empty) = emu_core::with(|emu_core| {
            let catalog = &emu_core.catalog;
            let shown = emu_core.list.len().to_string();
            let count = match emu_core.searching() {
                true => catalog.format(
                    "library.count_filtered",
                    &[("shown", &shown), ("count", &emu_core.total().to_string())],
                ),
                false => catalog.format("library.count", &[("count", &shown)]),
            };
            let empty = match (&emu_core.settings.roms_dir, emu_core.list.is_empty()) {
                (_, false) => String::new(),
                (None, true) => catalog.get("library.no_folder").to_string(),
                (Some(_), true) if emu_core.searching() => catalog.format(
                    "library.no_match",
                    &[("query", emu_core.current_search().trim())],
                ),
                (Some(dir), true) => catalog.format(
                    "library.empty",
                    &[("folder", &dir.display().to_string())],
                ),
            };
            (count, empty)
        });
        self.as_mut().set_count(QString::from(&count));
        self.as_mut().set_empty(QString::from(&empty));
    }
}
