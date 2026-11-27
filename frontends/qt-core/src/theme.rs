//! The interface theme: the palette, which the C++ side applies to every window, and the two colors
//! QML used to write by hand. See `zeebx::ui::themes` and `docs/implementation/26-themes.md`.

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    // See `cpp/palette.h`.
    #[namespace = "zeebx"]
    unsafe extern "C++" {
        include!("palette.h");
        fn palette_color(disabled: bool, role: &str, rgb: u32) -> bool;
        fn palette_apply(dark: bool);
        fn system_palette();
    }

    extern "RustQt" {
        /// The colors that are not in the Qt palette: the warning text (a rejected key, a missing
        /// folder) and the error text (the game that did not open). They change with the theme.
        #[qobject]
        #[qml_element]
        #[qml_singleton]
        #[qproperty(QString, warning)]
        #[qproperty(QString, error)]
        type Theme = super::ThemeRust;

        /// The theme changed: the two colors are read again.
        #[qinvokable]
        fn reload(self: Pin<&mut Theme>);
    }

    impl cxx_qt::Initialize for Theme {}
}

use std::pin::Pin;

use cxx_qt_lib::QString;

use super::emu_core;

/// The ones from before themes, which the system palette keeps using.
const SYSTEM_WARNING: &str = "#e0a030";
const SYSTEM_ERROR: &str = "#d04040";

#[derive(Default)]
pub struct ThemeRust {
    warning: QString,
    error: QString,
}

impl cxx_qt::Initialize for qobject::Theme {
    fn initialize(self: Pin<&mut Self>) {
        self.reload();
    }
}

impl qobject::Theme {
    pub fn reload(mut self: Pin<&mut Self>) {
        let (warning, error) = emu_core::with(|emu_core| match emu_core.theme() {
            Some(theme) => (theme.warning.hex(), theme.error.hex()),
            None => (SYSTEM_WARNING.to_string(), SYSTEM_ERROR.to_string()),
        });
        self.as_mut().set_warning(QString::from(&warning));
        self.as_mut().set_error(QString::from(&error));
    }
}

/// Applies the chosen theme to every window, or the system palette with no theme.
///
/// **Outside [`emu_core::with`]**: `setPalette` notifies the windows right away, the QML bindings are
/// re-evaluated in there, and one of them that calls the core would find the `RefCell` borrowed.
/// That is why the theme is copied inside and applied out here.
pub fn apply() {
    let Some(theme) = emu_core::with(|emu_core| emu_core.theme().cloned()) else {
        qobject::system_palette();
        return;
    };
    let disabled_colors = theme.disabled.iter().map(|color| (true, color));
    for (disabled, (role, color)) in theme
        .colors
        .iter()
        .map(|color| (false, color))
        .chain(disabled_colors)
    {
        if !qobject::palette_color(disabled, role, color.0) {
            eprintln!("theme {}: Qt has no role {role}", theme.key_id);
        }
    }
    qobject::palette_apply(theme.dark);
}
