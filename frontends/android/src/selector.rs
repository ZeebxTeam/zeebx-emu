//! The folder browser.
//!
//! It is our own browser, not the system picker, for a fundamental reason:
//! `ACTION_OPEN_DOCUMENT_TREE` returns a `content://`, and the core's loader opens file paths. A
//! browser over `std::fs` returns what the loader already knows how to use — and costs not one
//! line of JNI.
//!
//! It serves two choices: the ROM folder ([`View::FolderSelector`]) and the `.sf2` bank for MIDI
//! music ([`View::SoundfontSelector`]). Both browse the folders the same way.

use std::path::{Path, PathBuf};

use crate::{Emulator, View};

impl Emulator {
    pub(crate) fn selector(&mut self, ctx: &egui::Context, current: &Path) {
        // What the folder holds: the subfolders to browse, and how many games, to decide.
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut modules = 0usize;
        let mut unreadable = None;
        match std::fs::read_dir(current) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        dirs.push(path);
                    } else if is_module(&path) {
                        modules += 1;
                    }
                }
                dirs.sort();
            }
            Err(error) => unreadable = Some(error.to_string()),
        }

        egui::TopBottomPanel::top("caminho").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.label(self.tr("settings.roms_folder"));
            ui.weak(current.display().to_string());
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("acoes").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let width = (ui.available_width() - 24.0) / 2.0;
                let cancel =
                    ui.add_sized([width, 56.0], egui::Button::new(self.tr("common.cancel")));
                if cancel.clicked() {
                    self.location = View::Settings;
                }
                // Same as in settings: egui only moves a focus that already exists, and from
                // nothing it is `Tab` that grants it. Without this first focus the D-pad does
                // not move here.
                if ctx.memory(|m| m.focused()).is_none() {
                    cancel.request_focus();
                }
                let label = match modules {
                    0 => self.tr("selector.use_folder.empty").to_string(),
                    1 => self.tr("selector.use_folder.one").to_string(),
                    n => self
                        .catalog
                        .format("selector.use_folder.many", &[("count", &n.to_string())]),
                };
                if ui
                    .add_sized([width, 56.0], egui::Button::new(label))
                    .clicked()
                {
                    self.settings.roms_dir = Some(current.to_path_buf());
                    self.save();
                    self.location = View::Library;
                    self.reload();
                }
            });
            ui.add_space(8.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(dest) = self.browse_dirs(ui, current, &dirs, unreadable.as_deref()) {
                self.location = View::FolderSelector(dest);
            }
        });
    }

    /// The `.sf2` bank browser: the same folders, and each one's banks to tap and choose.
    pub(crate) fn soundfont_selector(&mut self, ctx: &egui::Context, current: &Path) {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut soundfonts: Vec<PathBuf> = Vec::new();
        let mut unreadable = None;
        match std::fs::read_dir(current) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        dirs.push(path);
                    } else if is_soundfont(&path) {
                        soundfonts.push(path);
                    }
                }
                dirs.sort();
                soundfonts.sort();
            }
            Err(error) => unreadable = Some(error.to_string()),
        }

        egui::TopBottomPanel::top("caminho").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.label(self.tr("audio.soundfont"));
            ui.weak(current.display().to_string());
            ui.add_space(6.0);
        });

        egui::TopBottomPanel::bottom("acoes").show(ctx, |ui| {
            ui.add_space(8.0);
            let cancel = ui.add_sized(
                [ui.available_width(), 56.0],
                egui::Button::new(self.tr("common.cancel")),
            );
            if cancel.clicked() {
                self.location = View::Settings;
            }
            if ctx.memory(|m| m.focused()).is_none() {
                cancel.request_focus();
            }
            ui.add_space(8.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let mut chosen = None;
            // The banks come before the folders: in a folder that has them, they are what you
            // came for.
            for soundfont in &soundfonts {
                let name = soundfont
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if ui
                    .add_sized(
                        [ui.available_width(), 52.0],
                        egui::Button::new(format!("🎵  {name}")),
                    )
                    .clicked()
                {
                    chosen = Some(soundfont.clone());
                }
            }
            if let Some(soundfont) = chosen {
                // Takes effect from the next game on: the bank is opened when the machine is born.
                self.settings.audio.soundfont = Some(soundfont);
                self.save();
                self.location = View::Settings;
                return;
            }
            if let Some(dest) = self.browse_dirs(ui, current, &dirs, unreadable.as_deref()) {
                self.location = View::SoundfontSelector(dest);
            }
        });
    }

    /// The shortcuts, the "go up" and the subfolders of `current`. Returns the folder tapped, if
    /// any.
    fn browse_dirs(
        &self,
        ui: &mut egui::Ui,
        current: &Path,
        dirs: &[PathBuf],
        unreadable: Option<&str>,
    ) -> Option<PathBuf> {
        {
            // Going straight to the two places that always exist saves a dozen taps — and the
            // app's folder is the only one that needs no permission.
            let mut dest = None;
            ui.horizontal(|ui| {
                if ui.button(self.tr("selector.storage")).clicked() {
                    dest = Some(PathBuf::from("/sdcard"));
                }
                if ui.button(self.tr("selector.app_folder")).clicked() {
                    dest = Some(self.own_dir.clone());
                }
            });
            ui.add_space(4.0);

            if let Some(error) = unreadable {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    self.catalog.format("selector.unreadable", &[("reason", error)]),
                );
                ui.label(self.tr("selector.unreadable.hint"));
            }

            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some(above) = current.parent() {
                    if ui
                        .add_sized([ui.available_width(), 52.0], egui::Button::new("⬆  .."))
                        .clicked()
                    {
                        dest = Some(above.to_path_buf());
                    }
                }
                for dir in dirs {
                    let name = dir
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if ui
                        .add_sized(
                            [ui.available_width(), 52.0],
                            egui::Button::new(format!("📁  {name}")),
                        )
                        .clicked()
                    {
                        dest = Some(dir.clone());
                    }
                }
            });
            dest
        }
    }
}

/// A `.sf2` instrument bank.
fn is_soundfont(path: &Path) -> bool {
    path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("sf2"))
}

/// A `.mod` is the game; a `.zip` is the package the loader opens.
fn is_module(path: &Path) -> bool {
    path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mod") || e.eq_ignore_ascii_case("zip"))
}
