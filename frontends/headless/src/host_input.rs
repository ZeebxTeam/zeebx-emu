//! The winit keyboard and the host controllers turning into the state of the console's ports.
//!
//! The mapping itself is the core's — [`Player::pad`] — and the key names are egui's, the same
//! ones the desktop controls screen writes to `settings.json`. **This is on purpose:** a mapping
//! copied from the interface into `config.ini` has to work without translation, otherwise the
//! easiest way to find out what a button is called would stop working.
//!
//! There is no egui drawing anything here: `egui::Key` is used only as the canonical name of a
//! key, which is what it already was in the configuration file.

use std::collections::HashSet;

use winit::keyboard::{KeyCode, PhysicalKey};

use zeebx::input::bindings::{Player, Source};
use zeebx::input::{Pad, gamepads::Gamepads};

/// The host keyboard: which keys are pressed right now.
#[derive(Default)]
pub struct Keyboard {
    pressed: HashSet<egui::Key>,
}

impl Keyboard {
    /// Records what winit reported. `None` when the key is not one we know how to name — an
    /// `F13`, a media key —, and then there is nothing to record.
    pub fn event(&mut self, physical_key: PhysicalKey, pressed: bool) {
        let PhysicalKey::Code(code) = physical_key else {
            return;
        };
        let Some(key) = from_winit(code) else {
            return;
        };
        match pressed {
            true => self.pressed.insert(key),
            false => self.pressed.remove(&key),
        };
    }

    /// The window lost focus: whatever was pressed is not anymore.
    ///
    /// Without this, leaving the window with a key held leaves the game thinking it still is —
    /// the `keyup` happens in another window and never arrives here.
    pub fn release_all(&mut self) {
        self.pressed.clear();
    }

    pub fn pressed(&self) -> &HashSet<egui::Key> {
        &self.pressed
    }

    fn is_triggered(&self, source: &Source) -> bool {
        match source {
            Source::Key { name } => {
                egui::Key::from_name(name).is_some_and(|key| self.pressed.contains(&key))
            }
            _ => false,
        }
    }
}

/// The controller state of a port, from its mapping.
///
/// Keyboard and controller are queried **together**: whoever has both can use both, which is
/// what having more than one source per button means.
pub fn port_pad(player: &Player, keyboard: &Keyboard, pads: &Gamepads, port: usize) -> Pad {
    let device = player.device.as_deref();
    player.pad(
        |source| keyboard.is_triggered(source) || pads.is_active(device, port, source),
        |axis| pads.value(device, port, axis),
    )
}

/// The winit key by the name egui — and therefore `config.ini` — gives it.
///
/// It goes by **physical** code, not by character: the `Z` in the configuration is the key where
/// `Z` sits on a US keyboard, which is what every emulator does. That way an ABNT2 or AZERTY
/// keyboard plays with the same configuration, with the hand in the same place.
fn from_winit(code: KeyCode) -> Option<egui::Key> {
    use egui::Key as K;
    Some(match code {
        KeyCode::ArrowUp => K::ArrowUp,
        KeyCode::ArrowDown => K::ArrowDown,
        KeyCode::ArrowLeft => K::ArrowLeft,
        KeyCode::ArrowRight => K::ArrowRight,

        KeyCode::Escape => K::Escape,
        KeyCode::Tab => K::Tab,
        KeyCode::Backspace => K::Backspace,
        KeyCode::Enter | KeyCode::NumpadEnter => K::Enter,
        KeyCode::Space => K::Space,
        KeyCode::Insert => K::Insert,
        KeyCode::Delete => K::Delete,
        KeyCode::Home => K::Home,
        KeyCode::End => K::End,
        KeyCode::PageUp => K::PageUp,
        KeyCode::PageDown => K::PageDown,

        KeyCode::Minus | KeyCode::NumpadSubtract => K::Minus,
        KeyCode::Equal => K::Equals,
        KeyCode::NumpadAdd => K::Plus,
        KeyCode::Comma | KeyCode::NumpadComma => K::Comma,
        KeyCode::Period | KeyCode::NumpadDecimal => K::Period,
        KeyCode::Semicolon => K::Semicolon,
        KeyCode::Quote => K::Quote,
        KeyCode::Backquote => K::Backtick,
        KeyCode::Backslash => K::Backslash,
        KeyCode::Slash | KeyCode::NumpadDivide => K::Slash,
        KeyCode::BracketLeft => K::OpenBracket,
        KeyCode::BracketRight => K::CloseBracket,

        KeyCode::Digit0 | KeyCode::Numpad0 => K::Num0,
        KeyCode::Digit1 | KeyCode::Numpad1 => K::Num1,
        KeyCode::Digit2 | KeyCode::Numpad2 => K::Num2,
        KeyCode::Digit3 | KeyCode::Numpad3 => K::Num3,
        KeyCode::Digit4 | KeyCode::Numpad4 => K::Num4,
        KeyCode::Digit5 | KeyCode::Numpad5 => K::Num5,
        KeyCode::Digit6 | KeyCode::Numpad6 => K::Num6,
        KeyCode::Digit7 | KeyCode::Numpad7 => K::Num7,
        KeyCode::Digit8 | KeyCode::Numpad8 => K::Num8,
        KeyCode::Digit9 | KeyCode::Numpad9 => K::Num9,

        KeyCode::KeyA => K::A,
        KeyCode::KeyB => K::B,
        KeyCode::KeyC => K::C,
        KeyCode::KeyD => K::D,
        KeyCode::KeyE => K::E,
        KeyCode::KeyF => K::F,
        KeyCode::KeyG => K::G,
        KeyCode::KeyH => K::H,
        KeyCode::KeyI => K::I,
        KeyCode::KeyJ => K::J,
        KeyCode::KeyK => K::K,
        KeyCode::KeyL => K::L,
        KeyCode::KeyM => K::M,
        KeyCode::KeyN => K::N,
        KeyCode::KeyO => K::O,
        KeyCode::KeyP => K::P,
        KeyCode::KeyQ => K::Q,
        KeyCode::KeyR => K::R,
        KeyCode::KeyS => K::S,
        KeyCode::KeyT => K::T,
        KeyCode::KeyU => K::U,
        KeyCode::KeyV => K::V,
        KeyCode::KeyW => K::W,
        KeyCode::KeyX => K::X,
        KeyCode::KeyY => K::Y,
        KeyCode::KeyZ => K::Z,

        KeyCode::F1 => K::F1,
        KeyCode::F2 => K::F2,
        KeyCode::F3 => K::F3,
        KeyCode::F4 => K::F4,
        KeyCode::F5 => K::F5,
        KeyCode::F6 => K::F6,
        KeyCode::F7 => K::F7,
        KeyCode::F8 => K::F8,
        KeyCode::F9 => K::F9,
        KeyCode::F10 => K::F10,
        KeyCode::F11 => K::F11,
        KeyCode::F12 => K::F12,

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name `config.ini` uses has to find the key winit delivers. If this table and egui's
    /// disagree, `key:Z` becomes a line that does nothing.
    #[test]
    fn the_file_names_match_the_winit_keys() {
        let mut keyboard = Keyboard::default();
        for (code, name) in [
            (KeyCode::KeyZ, "Z"),
            (KeyCode::Space, "Space"),
            (KeyCode::ArrowUp, "ArrowUp"),
            (KeyCode::Enter, "Enter"),
            (KeyCode::Digit1, "1"),
            (KeyCode::Backspace, "Backspace"),
        ] {
            keyboard.event(PhysicalKey::Code(code), true);
            assert!(
                keyboard.is_triggered(&Source::key(name)),
                "`key:{name}` does not see {code:?}"
            );
            keyboard.event(PhysicalKey::Code(code), false);
            assert!(!keyboard.is_triggered(&Source::key(name)), "{name} got stuck");

        }
    }

    #[test]
    fn losing_focus_releases_what_was_pressed() {
        let mut keyboard = Keyboard::default();
        keyboard.event(PhysicalKey::Code(KeyCode::KeyZ), true);
        keyboard.release_all();
        assert!(!keyboard.is_triggered(&Source::key("Z")));
    }
}
