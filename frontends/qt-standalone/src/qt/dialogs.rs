//! The auxiliary windows: the save manager, the startup and new-version notices, and the run's
//! log. The rules for each live in the core (`ui::saves`, `ui::running_game`); what the QML reads
//! lives here.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
        include!("cxx-qt-lib/qstringlist.h");
        type QStringList = cxx_qt_lib::QStringList;
    }

    extern "RustQt" {
        /// The saves: what each game wrote to its own folder, and what is on the device's file
        /// system, which belongs to all of them.
        #[qobject]
        #[qml_element]
        #[qproperty(i32, version)]
        type Saves = super::SavesRust;

        /// Rereads the list. Touches the disk: when the window opens and after each deletion.
        #[qinvokable]
        fn reload(self: Pin<&mut Saves>);

        #[qinvokable]
        fn count(self: &Saves) -> i32;

        #[qinvokable]
        fn title(self: &Saves, index: i32) -> QString;

        /// "3 file(s), 12 KB".
        #[qinvokable]
        fn summary(self: &Saves, index: i32) -> QString;

        /// "Delete the save for …? There is no undo."
        #[qinvokable]
        fn question(self: &Saves, index: i32) -> QString;

        /// Whether the save belongs to the device rather than to a game.
        #[qinvokable]
        #[cxx_name = "belongsToDevice"]
        fn belongs_to_device(self: &Saves, index: i32) -> bool;

        /// Deletes the save and returns what to say. There is no undo: the window confirms first.
        #[qinvokable]
        fn remove(self: Pin<&mut Saves>, index: i32) -> QString;
    }

    extern "RustQt" {
        /// The main window's notices: the startup one and the new-version one.
        #[qobject]
        #[qml_element]
        type Notices = super::NoticesRust;

        /// The startup notice has not been dismissed in this version yet.
        #[qinvokable]
        #[cxx_name = "hasStartupNotice"]
        fn has_startup_notice(self: &Notices) -> bool;

        /// The startup notice got out of the way; `for_good` is the "don't show again".
        #[qinvokable]
        #[cxx_name = "dismissStartup"]
        fn dismiss_startup(self: &Notices, for_good: bool);

        /// "Zeebx 0.4.0 is now available…", if there is a new version to announce; empty otherwise.
        #[qinvokable]
        #[cxx_name = "updateNoticeText"]
        fn update_notice_text(self: &Notices) -> QString;

        #[qinvokable]
        #[cxx_name = "updatePage"]
        fn update_page(self: &Notices) -> QString;

        #[qinvokable]
        #[cxx_name = "dismissUpdate"]
        fn dismiss_update(self: &Notices);

        /// Checks for a new version now, from the Help menu. A new one opens the usual notice;
        /// anything else comes back through `updateCheckResult`.
        #[qinvokable]
        #[cxx_name = "checkForUpdates"]
        fn check_for_updates(self: &Notices);

        /// What the check from the menu answered, when it was not a new version: up to date, or
        /// why it failed. Empty while there is no answer; once read, it goes away.
        #[qinvokable]
        #[cxx_name = "updateCheckResult"]
        fn update_check_result(self: &Notices) -> QString;

        /// This copy downloads and installs the new version by itself, instead of just opening the
        /// page.
        #[qinvokable]
        #[cxx_name = "selfUpdates"]
        fn self_updates(self: &Notices) -> bool;

        #[qinvokable]
        #[cxx_name = "installUpdate"]
        fn install_update(self: &Notices);

        /// Where the installation stands: 0 none, 1 downloading, 2 installing, 3 ready, 4 failed.
        #[qinvokable]
        #[cxx_name = "updateStep"]
        fn update_step(self: &Notices) -> i32;

        /// From 0 to 1 while downloading; -1 if the size is not known.
        #[qinvokable]
        #[cxx_name = "updateFraction"]
        fn update_fraction(self: &Notices) -> f64;

        /// The step's text: "Downloading…", "Installed, restart…", or the reason for the failure.
        #[qinvokable]
        #[cxx_name = "updateText"]
        fn update_text(self: &Notices) -> QString;

        /// Saves the settings and opens the installed version. Does not return.
        #[qinvokable]
        #[cxx_name = "restartIntoNewVersion"]
        fn restart_into_new_version(self: &Notices);
    }

    extern "RustQt" {
        /// The run's log: what the game wrote, and the emulator's complaints about it.
        #[qobject]
        #[qml_element]
        type Log = super::LogRust;

        /// The log window is requested: the option is on, and it was not closed in this run.
        #[qinvokable]
        fn active(self: &Log) -> bool;

        /// Closing the window dismisses this run's log, not the preference.
        #[qinvokable]
        fn dismiss(self: &Log);

        #[qinvokable]
        fn lines(self: &Log) -> QStringList;

        /// Where this run's report is written automatically.
        #[qinvokable]
        fn path(self: &Log) -> QString;

        /// Writes the log to a chosen file and returns what to say; empty if the user gave up.
        #[qinvokable]
        fn save(self: &Log) -> QString;
    }
}

use std::pin::Pin;

use cxx_qt::CxxQtType;
use cxx_qt_lib::{QList, QString, QStringList};

use zeebx::ui::updater;
use zeebx::ui::saves::{self, Save};

use super::emu_core;

#[derive(Default)]
pub struct SavesRust {
    version: i32,
    /// The listed saves, `true` on the device's ones.
    list: Vec<(bool, Save)>,
}

impl qobject::Saves {
    fn save(&self, index: i32) -> Option<&(bool, Save)> {
        usize::try_from(index).ok().and_then(|i| self.rust().list.get(i))
    }

    pub fn reload(mut self: Pin<&mut Self>) {
        let roms = emu_core::with(|emu_core| emu_core.settings.roms_dir.clone());
        self.as_mut().rust_mut().list = saves::all(roms.as_deref());
        let version = self.version().wrapping_add(1);
        self.as_mut().set_version(version);
    }

    pub fn count(&self) -> i32 {
        self.rust().list.len() as i32
    }

    pub fn title(&self, index: i32) -> QString {
        QString::from(self.save(index).map(|(_, save)| save.title.as_str()).unwrap_or_default())
    }

    pub fn summary(&self, index: i32) -> QString {
        let Some((_, save)) = self.save(index) else {
            return QString::default();
        };
        emu_core::with(|emu_core| {
            QString::from(&emu_core.catalog.format(
                "saves.files",
                &[
                    ("count", &save.files.to_string()),
                    ("size", &saves::size(save.bytes)),
                ],
            ))
        })
    }

    pub fn question(&self, index: i32) -> QString {
        let Some((_, save)) = self.save(index) else {
            return QString::default();
        };
        emu_core::with(|emu_core| {
            QString::from(&emu_core.catalog.format("saves.confirm", &[("name", &save.title)]))
        })
    }

    pub fn belongs_to_device(&self, index: i32) -> bool {
        self.save(index).is_some_and(|(device, _)| *device)
    }

    pub fn remove(mut self: Pin<&mut Self>, index: i32) -> QString {
        let Some((_, save)) = self.save(index) else {
            return QString::default();
        };
        let title = save.title.clone();
        let result = saves::remove(save);
        let message = emu_core::with(|emu_core| match result {
            Ok(()) => emu_core.catalog.format("saves.deleted", &[("name", &title)]),
            Err(error) => emu_core
                .catalog
                .format("saves.failed", &[("name", &title), ("reason", &error.to_string())]),
        });
        self.as_mut().reload();
        QString::from(&message)
    }
}

#[derive(Default)]
pub struct NoticesRust;

impl qobject::Notices {
    pub fn has_startup_notice(&self) -> bool {
        emu_core::with(|emu_core| emu_core.startup_notice_pending())
    }

    pub fn dismiss_startup(&self, for_good: bool) {
        emu_core::with(|emu_core| emu_core.dismiss_startup_notice(for_good));
    }

    pub fn update_notice_text(&self) -> QString {
        emu_core::with(|emu_core| {
            let Some(release) = emu_core.update_notice() else {
                return QString::default();
            };
            QString::from(&emu_core.catalog.format(
                "update.available",
                &[("new", &release.version), ("current", updater::CURRENT_VERSION)],
            ))
        })
    }

    pub fn update_page(&self) -> QString {
        emu_core::with(|emu_core| {
            QString::from(emu_core.update_notice().map(|l| l.page.as_str()).unwrap_or_default())
        })
    }

    pub fn dismiss_update(&self) {
        emu_core::with(|emu_core| emu_core.dismiss_update_notice());
    }

    pub fn check_for_updates(&self) {
        emu_core::with(|emu_core| emu_core.check_for_updates_from_menu());
    }

    pub fn update_check_result(&self) -> QString {
        QString::from(&emu_core::with(|emu_core| emu_core.update_check_result()).unwrap_or_default())
    }

    pub fn self_updates(&self) -> bool {
        updater::Installation::current().can_self_replace()
    }

    pub fn install_update(&self) {
        emu_core::with(|emu_core| emu_core.install_update());
    }

    pub fn update_step(&self) -> i32 {
        emu_core::with(|emu_core| match emu_core.update_installation.as_ref().map(|a| a.progress()) {
            None => 0,
            Some(updater::Progress::Downloading { .. }) => 1,
            Some(updater::Progress::Installing) => 2,
            Some(updater::Progress::Ready) => 3,
            Some(updater::Progress::Failed(_)) => 4,
        })
    }

    pub fn update_fraction(&self) -> f64 {
        emu_core::with(|emu_core| {
            let fraction = emu_core.update_installation.as_ref().and_then(|a| a.fraction());
            fraction.map_or(-1.0, f64::from)
        })
    }

    pub fn update_text(&self) -> QString {
        emu_core::with(|emu_core| {
            let Some(installation) = &emu_core.update_installation else {
                return QString::default();
            };
            let version = match &emu_core.updater {
                Some(updater::Response::NewVersion(release)) => release.version.as_str(),
                _ => "",
            };
            let catalog = &emu_core.catalog;
            let text = match installation.progress() {
                updater::Progress::Downloading { .. } => catalog.get("update.downloading").to_string(),
                updater::Progress::Installing => catalog.get("update.installing").to_string(),
                updater::Progress::Ready => catalog.format(
                    "update.ready",
                    &[("new", version), ("current", updater::CURRENT_VERSION)],
                ),
                updater::Progress::Failed(reason) => {
                    catalog.format("update.failed", &[("reason", &reason)])
                }
            };
            QString::from(&text)
        })
    }

    pub fn restart_into_new_version(&self) {
        emu_core::with(|emu_core| emu_core.restart_into_new_version())
    }
}

#[derive(Default)]
pub struct LogRust;

impl qobject::Log {
    pub fn active(&self) -> bool {
        emu_core::with(|emu_core| emu_core.settings.debug.log && !emu_core.log_dismissed)
    }

    pub fn dismiss(&self) {
        emu_core::with(|emu_core| emu_core.log_dismissed = true);
    }

    pub fn lines(&self) -> QStringList {
        let mut list = QList::<QString>::default();
        for line in emu_core::with(|emu_core| emu_core.log()) {
            list.append(QString::from(&line));
        }
        QStringList::from(&list)
    }

    pub fn path(&self) -> QString {
        let path = emu_core::with(|emu_core| emu_core.report_path());
        QString::from(&path.map(|c| c.display().to_string()).unwrap_or_default())
    }

    pub fn save(&self) -> QString {
        let (lines, suggestion) = emu_core::with(|emu_core| {
            let suggestion = emu_core
                .report_path()
                .and_then(|c| c.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "zeebx.log".to_string());
            (emu_core.log(), suggestion)
        });
        let Some(dest) = rfd::FileDialog::new()
            .set_file_name(&suggestion)
            .add_filter("log", &["log"])
            .save_file()
        else {
            return QString::default();
        };
        let result = std::fs::write(&dest, lines.join("\n") + "\n");
        emu_core::with(|emu_core| {
            QString::from(&match result {
                Ok(()) => emu_core
                    .catalog
                    .format("debug.log.saved", &[("path", &dest.display().to_string())]),
                Err(error) => emu_core
                    .catalog
                    .format("debug.log.failed", &[("reason", &error.to_string())]),
            })
        })
    }
}
