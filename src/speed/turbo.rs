//! The turbo: a button that presses and releases itself, at the game's pace.
//!
//! **The pace is the virtual clock's, not the host's.** The frontend delivers the controller once
//! per window pass, and a pass can run several game frames — four to catch up with the clock, ten
//! in fast-forward at 10x. A turbo that toggled the button on every pass would pulse ten times
//! slower for the game in fast-forward, and would stand still during catch-up. That is why the
//! "pressed now" decision belongs to the session, before each pass of the event loop, by the phase
//! of the game's clock: the same game, with the same turbo, sees the same presses, however the
//! host runs. See `docs/implementation/24-speed.md`.

use serde::{Deserialize, Serialize};

use super::{Toggle, ShortcutMode};
use crate::input::Pad;

/// How a player's turbo key works. These are RetroArch's modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurboMode {
    #[default]
    #[serde(rename = "desligado")]
    Off,
    /// Holding the turbo key together with a button makes **that** button pulse.
    #[serde(rename = "classico")]
    Classic,
    /// One press of the turbo key turns the default button into turbo, and another undoes it. The
    /// button pulses while it is held — and not on its own: an autofire that fires with nobody
    /// touching it would rush through the menus, where button 1 is "confirm" in most games.
    #[serde(rename = "unico_alternar")]
    SingleToggle,
    /// While the turbo key is held, the default button pulses on its own.
    #[serde(rename = "unico_segurar")]
    SingleHold,
}

impl TurboMode {
    pub const ALL: [Self; 4] = [
        Self::Off,
        Self::Classic,
        Self::SingleToggle,
        Self::SingleHold,
    ];

    pub fn key_id(self) -> &'static str {
        match self {
            Self::Off => "common.off",
            Self::Classic => "turbo.mode.classic",
            Self::SingleToggle => "turbo.mode.toggle",
            Self::SingleHold => "turbo.mode.hold",
        }
    }
}

/// The name, in each player's mapping, of the turbo key. It is not a Zeebo button:
/// [`Pad::button_by_name`] does not know it, and that is why it presses nothing in the game.
pub const TURBO_BUTTON: &str = "turbo";

/// The factory default button of the single-button modes.
pub const DEFAULT_BUTTON: &str = "b1";

/// The buttons that can pulse, in menu order: those of the mapping
/// ([`crate::input::bindings::CONFIGURABLE`]) minus the d-pad and HOME. RetroArch excludes the
/// d-pad by default, and a pulsing HOME would open and close the console menu.
pub const PULSABLE: [&str; 8] = ["b1", "b2", "b3", "b4", "zl", "zr", "lthumb", "rthumb"];

/// Presses per second: the range and the default. Ten is the 6-frame period at 60 Hz RetroArch
/// uses out of the box.
pub const MIN_PRESSES: u8 = 5;
pub const MAX_PRESSES: u8 = 20;
pub const DEFAULT_PRESSES: u8 = 10;

/// The mask of the pulsable buttons.
fn pulsable_mask() -> u32 {
    PULSABLE
        .iter()
        .filter_map(|name| Pad::button_by_name(name))
        .fold(0, |mask, index| mask | (1 << index))
}

/// The mask of a button by name, if it can pulse.
fn button_mask(name: &str) -> u32 {
    match PULSABLE.contains(&name) {
        true => Pad::button_by_name(name).map_or(0, |index| 1 << index),
        false => 0,
    }
}

/// The turbo of one port: what needs remembering from one pass to the next.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PortTurbo {
    /// The toggle mode state.
    toggle: Toggle,
}

impl PortTurbo {
    /// The buttons that pulse on this pass, as a [`Pad`] mask.
    ///
    /// `raw` is the controller as the player holds it, and `pressed` says whether the turbo key is
    /// pressed. In hold mode, the default button pulses even if the player does not hold it: that
    /// is what the mode means.
    pub fn pulsing(&mut self, mode: TurboMode, button: &str, raw: &Pad, pressed: bool) -> u32 {
        match mode {
            TurboMode::Off => {
                self.toggle.disable();
                0
            }
            TurboMode::Classic => match pressed {
                true => raw.buttons & pulsable_mask(),
                false => 0,
            },
            TurboMode::SingleHold => match pressed {
                true => button_mask(button),
                false => 0,
            },
            TurboMode::SingleToggle => {
                match self.toggle.update(pressed, ShortcutMode::Toggle) {
                    true => raw.buttons & button_mask(button),
                    false => 0,
                }
            }
        }
    }

    /// Whether toggle mode is on: the window says "Turbo" on screen, because a turbo forgotten on
    /// looks like a game bug.
    pub fn enabled(&self) -> bool {
        self.toggle.enabled()
    }

    pub fn disable(&mut self) {
        self.toggle.disable();
    }
}

/// Whether the pulsing buttons are pressed at virtual instant `clock_ms`.
///
/// Half of the period pressed, half released: the duty cycle is fixed at 50%.
pub fn pressed_phase(clock_ms: u32, taps_per_second: u8) -> bool {
    let presses = u32::from(taps_per_second.clamp(MIN_PRESSES, MAX_PRESSES));
    let half_period = (500 / presses).max(1);
    (clock_ms / half_period).is_multiple_of(2)
}

/// The controller the game sees: the raw one, with the pulsing buttons in the current phase.
pub fn modulate(raw: Pad, pulsing: u32, pressed: bool) -> Pad {
    let mut effective = raw;
    effective.buttons &= !pulsing;
    if pressed {
        effective.buttons |= pulsing;
    }
    effective
}

#[cfg(test)]
mod tests {
    use super::*;

    fn com(buttons: &[&str]) -> Pad {
        let mut pad = Pad::default();
        for name in buttons {
            pad.press(Pad::button_by_name(name).unwrap(), true);
        }
        pad
    }

    fn bit(name: &str) -> u32 {
        1 << Pad::button_by_name(name).unwrap()
    }

    /// Ten presses per second are 50 ms pressed and 50 ms released, by the game's clock.
    #[test]
    fn the_phase_follows_the_virtual_clock() {
        let phases: Vec<bool> = [0, 49, 50, 99, 100].iter().map(|&t| pressed_phase(t, 10)).collect();
        assert_eq!(phases, [true, true, false, false, true]);
        // Twenty per second: 25 ms each half.
        assert!(!pressed_phase(25, 20));
        // Out of range, the limit applies.
        assert_eq!(pressed_phase(110, 200), pressed_phase(110, MAX_PRESSES));
    }

    /// Classic pulses whatever is held together with the turbo — except the d-pad and HOME.
    #[test]
    fn classic_pulses_what_is_held_except_dpad_and_home() {
        let mut turbo = PortTurbo::default();
        let raw = com(&["b1", "b3", "up", "back"]);
        let pulsing = turbo.pulsing(TurboMode::Classic, DEFAULT_BUTTON, &raw, true);
        assert_eq!(pulsing, bit("b1") | bit("b3"));
        assert_eq!(turbo.pulsing(TurboMode::Classic, DEFAULT_BUTTON, &raw, false), 0);
    }

    /// Hold pulses the default button without it being held; toggle only while it is.
    #[test]
    fn the_single_button_modes() {
        let mut turbo = PortTurbo::default();
        let nothing = Pad::default();
        assert_eq!(turbo.pulsing(TurboMode::SingleHold, "b2", &nothing, true), bit("b2"));
        assert_eq!(turbo.pulsing(TurboMode::SingleHold, "b2", &nothing, false), 0);

        let mut turbo = PortTurbo::default();
        let b1 = com(&["b1"]);
        // One press turns it on; the button pulses only when held.
        assert_eq!(turbo.pulsing(TurboMode::SingleToggle, "b1", &nothing, true), 0);
        assert!(turbo.enabled());
        assert_eq!(turbo.pulsing(TurboMode::SingleToggle, "b1", &b1, false), bit("b1"));
        // Another press turns it off.
        turbo.pulsing(TurboMode::SingleToggle, "b1", &b1, true);
        assert!(!turbo.enabled());
        assert_eq!(turbo.pulsing(TurboMode::SingleToggle, "b1", &b1, false), 0);
    }

    /// A default button that cannot pulse does not pulse, and turning the mode off resets the
    /// toggle.
    #[test]
    fn a_button_that_does_not_pulse_means_off() {
        let mut turbo = PortTurbo::default();
        assert_eq!(turbo.pulsing(TurboMode::SingleHold, "back", &Pad::default(), true), 0);
        turbo.pulsing(TurboMode::SingleToggle, "b1", &Pad::default(), true);
        assert!(turbo.enabled());
        turbo.pulsing(TurboMode::Off, "b1", &Pad::default(), false);
        assert!(!turbo.enabled());
    }

    #[test]
    fn modulate_only_touches_what_pulses() {
        let raw = com(&["b1", "up"]);
        let pulsing = bit("b1") | bit("b2");
        assert_eq!(modulate(raw, pulsing, true).buttons, raw.buttons | bit("b2"));
        assert_eq!(modulate(raw, pulsing, false).buttons, bit("up"));
    }
}
