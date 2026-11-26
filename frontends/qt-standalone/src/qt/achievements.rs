//! RetroAchievements achievements in the interface: the account and the notices over the game. The
//! object exists with or without the feature, so the QML is a single one; without it, `available`
//! hides the tab.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        type Achievements = super::AchievementsRust;

        #[qinvokable]
        fn available(self: &Achievements) -> bool;

        #[qinvokable]
        #[cxx_name = "loggingIn"]
        fn logging_in(self: &Achievements) -> bool;

        /// "Logged in as …"; empty without an account.
        #[qinvokable]
        fn account(self: &Achievements) -> QString;

        /// The error of the last login; empty if it worked.
        #[qinvokable]
        fn error(self: &Achievements) -> QString;

        /// The password goes to the server and is forgotten: what stays in the settings is the
        /// token.
        #[qinvokable]
        fn login(self: &Achievements, username: &QString, password: &QString);

        /// Logs out of the account and forgets the token.
        #[qinvokable]
        fn logout(self: &Achievements);
    }
}

use cxx_qt_lib::QString;

use super::emu_core;

#[derive(Default)]
pub struct AchievementsRust;

impl qobject::Achievements {
    pub fn available(&self) -> bool {
        cfg!(feature = "achievements")
    }

    pub fn logging_in(&self) -> bool {
        emu_core::with(|c| c.achievements.logging_in())
    }

    pub fn account(&self) -> QString {
        QString::from(&emu_core::with(|c| c.achievements.account(&c.catalog)))
    }

    pub fn error(&self) -> QString {
        QString::from(&emu_core::with(|c| c.achievements.error()))
    }

    pub fn login(&self, username: &QString, password: &QString) {
        let (username, password) = (String::from(username), String::from(password));
        emu_core::with(|c| c.achievements.login(username.trim(), &password));
    }

    pub fn logout(&self) {
        emu_core::with(|c| {
            c.achievements.logout();
            c.settings.achievements = Default::default();
            c.save_settings();
        });
    }
}

/// A notice over the game, already translated. `icon` is a URL, or empty.
pub struct AchievementNotice {
    pub title: String,
    pub text: String,
    pub icon: String,
}

#[cfg(feature = "achievements")]
pub use with_feature::InCore;
#[cfg(not(feature = "achievements"))]
pub use without_feature::InCore;

#[cfg(not(feature = "achievements"))]
mod without_feature {
    use zeebx::session::Session;
    use zeebx::ui::i18n::Catalog;
    use zeebx::ui::settings::Settings;

    use super::AchievementNotice;

    #[derive(Default)]
    pub struct InCore;

    impl InCore {
        pub fn logging_in(&self) -> bool {
            false
        }
        pub fn account(&self, _: &Catalog) -> String {
            String::new()
        }
        pub fn error(&self) -> String {
            String::new()
        }
        pub fn login(&mut self, _: &str, _: &str) {}
        pub fn logout(&mut self) {}
        pub fn each_frame(&mut self, _: &mut Settings) -> bool {
            false
        }
        pub fn enable(&mut self, _: &mut Session) {}
        pub fn disable(&mut self, _: &mut Session) {}
        pub fn after_step(
            &mut self,
            _: &mut Session,
            _: bool,
            _: &mut Settings,
            _: &Catalog,
        ) -> (Vec<AchievementNotice>, bool) {
            (Vec::new(), false)
        }
    }
}

#[cfg(feature = "achievements")]
mod with_feature {
    use std::cell::RefCell;
    use std::rc::Rc;

    use zeebx::achievements::http::HttpServer;
    use zeebx::achievements::{Client, Event, Shared};
    use zeebx::session::Session;
    use zeebx::ui::i18n::Catalog;
    use zeebx::ui::settings::Settings;

    use super::AchievementNotice;

    #[derive(Default)]
    pub struct InCore {
        /// Created on the first login, and not at startup: without an account, nothing talks to the
        /// server.
        client: Option<Shared>,
        /// The login on its way, and whether it is with the stored token.
        logging_in: Option<bool>,
        error: String,
        tried_token: bool,
        /// There is an open game, and whether its session already has the client.
        game: Option<bool>,
    }

    impl InCore {
        fn client(&mut self) -> Shared {
            let client = self
                .client
                .get_or_insert_with(|| Rc::new(RefCell::new(Client::new(Box::new(HttpServer::default())))));
            Rc::clone(client)
        }

        fn logged_in(&self) -> bool {
            self.client
                .as_ref()
                .is_some_and(|c| c.borrow().user().is_some())
        }

        pub fn logging_in(&self) -> bool {
            self.logging_in.is_some()
        }

        pub fn account(&self, catalog: &Catalog) -> String {
            let Some(user) = self.client.as_ref().and_then(|c| c.borrow().user()) else {
                return String::new();
            };
            let points = user.points.to_string();
            let text = [("user", user.name.as_str()), ("points", &points)];
            catalog.format("achievements.connected", &text)
        }

        pub fn error(&self) -> String {
            self.error.clone()
        }

        pub fn login(&mut self, username: &str, password: &str) {
            if username.is_empty() || password.is_empty() || self.logging_in.is_some() {
                return;
            }
            self.error.clear();
            self.logging_in = Some(false);
            self.client().borrow_mut().login_with_password(username, password);
        }

        pub fn logout(&mut self) {
            if let Some(client) = &self.client {
                client.borrow_mut().logout();
            }
            self.error.clear();
        }

        /// Logs in with the stored token the first time, and moves the requests along when no
        /// session has the client. Returns whether the settings changed.
        pub fn each_frame(&mut self, settings: &mut Settings) -> bool {
            if !std::mem::replace(&mut self.tried_token, true)
                && let (Some(username), Some(token)) =
                    (&settings.achievements.username, &settings.achievements.token)
            {
                self.logging_in = Some(true);
                let (username, token) = (username.clone(), token.clone());
                self.client().borrow_mut().login_with_token(&username, &token);
            }
            let Some(client) = self.client.clone() else {
                return false;
            };
            if self.game == Some(true) {
                return false;
            }
            client.borrow_mut().idle(None);
            let events = client.borrow_mut().events();
            events
                .iter()
                .fold(false, |changed, event| self.on_login(event, settings) | changed)
        }

        /// Keeps the token of a login, or the error. Returns whether the settings changed.
        fn on_login(&mut self, event: &Event, settings: &mut Settings) -> bool {
            let Event::LoggedIn(result) = event else {
                return false;
            };
            let with_token = self.logging_in.take().unwrap_or(false);
            match result {
                Ok(()) => {
                    let Some(user) = self.client.as_ref().and_then(|c| c.borrow().user()) else {
                        return false;
                    };
                    self.error.clear();
                    settings.achievements.username = Some(user.name);
                    settings.achievements.token = Some(user.token);
                    true
                }
                Err(reason) => {
                    self.error = reason.clone();
                    // A rejected token has expired: kept, it would fail on every startup.
                    with_token && settings.achievements.token.take().is_some()
                }
            }
        }

        /// The session gets the client if there is an account or a login on its way, which
        /// rcheevos waits for.
        pub fn enable(&mut self, session: &mut Session) {
            let enable = self.logged_in() || self.logging_in.is_some();
            if let (true, Some(client)) = (enable, &self.client) {
                session.enable_achievements(Rc::clone(client));
            }
            self.game = Some(enable && self.client.is_some());
        }

        pub fn disable(&mut self, session: &mut Session) {
            session.disable_achievements();
            self.game = None;
        }

        /// The requests move along with the game paused, and the events become notices. Also
        /// returns whether the settings changed.
        pub fn after_step(
            &mut self,
            session: &mut Session,
            paused: bool,
            settings: &mut Settings,
            catalog: &Catalog,
        ) -> (Vec<AchievementNotice>, bool) {
            // The login finished with the game already open: the session gets the client now.
            if self.game == Some(false) && self.logged_in() {
                self.enable(session);
            }
            let (Some(true), Some(client)) = (self.game, self.client.clone()) else {
                return (Vec::new(), false);
            };
            if paused {
                session.idle_achievements();
            }
            let events = client.borrow_mut().events();
            let mut notices = Vec::new();
            let mut changed = false;
            for event in events {
                let (title, text, icon) = match event {
                    Event::LoggedIn(_) => {
                        changed |= self.on_login(&event, settings);
                        continue;
                    }
                    Event::GameLoaded(Ok(Some(g))) => {
                        let (u, t) = (g.unlocked.to_string(), g.total.to_string());
                        let text = [("unlocked", u.as_str()), ("total", &t)];
                        (g.title, catalog.format("achievements.game", &text), g.icon)
                    }
                    Event::GameLoaded(Ok(None)) => (
                        catalog.get("achievements.game.none").into(),
                        String::new(),
                        None,
                    ),
                    Event::GameLoaded(Err(m)) => {
                        let title = catalog.format("achievements.game.error", &[("reason", &m)]);
                        (title, String::new(), None)
                    }
                    Event::Unlocked(a) => {
                        let points = a.points.to_string();
                        let text = catalog.format("achievements.unlocked", &[("points", &points)]);
                        (a.title, text, a.icon)
                    }
                };
                let icon = icon.unwrap_or_default();
                notices.push(AchievementNotice { title, text, icon });
            }
            (notices, changed)
        }
    }
}
