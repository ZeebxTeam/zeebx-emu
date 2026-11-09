//! Host window, to follow the running game.
//!
//! It is the only part of the emulator that talks to the window system, and that is why the
//! dependency stays confined here: the rest of the code only hands over a [`Framebuffer`] and asks
//! whether the window is still open.

use minifb::{Key, KeyRepeat, Scale, WindowOptions};

use crate::input::{self, Pad};
use crate::video::display::Framebuffer;

pub struct Window {
    inner: minifb::Window,
    width: usize,
    height: usize,
}

impl Window {
    /// Opens a window the size of the console screen.
    pub fn open(title: &str, width: u32, height: u32) -> Result<Self, String> {
        let (width, height) = (width as usize, height as usize);
        let mut inner = minifb::Window::new(
            title,
            width,
            height,
            WindowOptions {
                scale: Scale::X1,
                resize: true,
                ..WindowOptions::default()
            },
        )
        .map_err(|err| err.to_string())?;
        // No limit: the emulator sets the pace, and holding back here would only make it slower.
        inner.set_target_fps(0);
        Ok(Self {
            inner,
            width,
            height,
        })
    }

    /// Whether the window is still open. `Esc` closes it, as in any emulator.
    pub fn is_open(&self) -> bool {
        self.inner.is_open() && !self.inner.is_key_down(Key::Escape)
    }

    /// The controller state, read from the keyboard.
    ///
    /// The arrows move the `X` and `Y` axes, which is where the Zeebo controller reports the
    /// d-pad; the rest are buttons. The button names are the controller's, not the keys' — the
    /// table of who is who is in [`crate::input`].
    pub fn pad(&self) -> Pad {
        let mut pad = Pad::default();
        // A key pressed and released between two frames would not show up in `is_key_down`, and
        // the emulator only looks at the keyboard once per frame: without the edge events, a
        // quick tap got lost.
        let tapped = self.inner.get_keys_pressed(KeyRepeat::No);
        let held = |key| self.inner.is_key_down(key) || tapped.contains(&key);

        let arrows = [
            (Key::Up, input::DPAD[0]),
            (Key::Down, input::DPAD[1]),
            (Key::Left, input::DPAD[2]),
            (Key::Right, input::DPAD[3]),
        ];
        for (key, index) in arrows {
            pad.press(index, held(key));
        }
        for (key, button) in KEYMAP {
            if held(key) {
                if let Some(index) = Pad::button_by_name(button) {
                    pad.press(index, true);
                }
            }
        }
        pad
    }

    /// Shows the frame.
    pub fn show(&mut self, frame: &Framebuffer) -> Result<(), String> {
        let pixels = frame.to_argb();
        // The guest screen may change size between one frame and the next; the window does not.
        if pixels.len() != self.width * self.height {
            return Ok(());
        }
        self.inner
            .update_with_buffer(&pixels, self.width, self.height)
            .map_err(|err| err.to_string())
    }
}

/// Which key each controller button comes from.
///
/// The choice is that of any emulator: the left hand on the arrows and the right on the action
/// keys, with `Enter` and `Backspace` on `Start` and `Back`.
const KEYMAP: [(Key, &str); 12] = [
    (Key::Z, "b1"),
    (Key::Space, "b2"),
    (Key::X, "b2"),
    (Key::C, "b3"),
    (Key::V, "b4"),
    (Key::Q, "zl"),
    (Key::W, "zr"),
    (Key::F, "lthumb"),
    (Key::G, "rthumb"),
    // The Zeebo controller has no Start: HOME plays that role.
    (Key::Enter, "back"),
    (Key::Backspace, "back"),
    (Key::H, "home"),
];
