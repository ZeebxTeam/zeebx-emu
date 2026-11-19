//! The physical controller: from what `GCController` reports to the Zeebo's buttons.
//!
//! Swift reads the controller and hands over one mask per turn, in the order of [`NAMES`]. What
//! each bit presses is decided here, and not in Swift, for two reasons: the rule can be tested on
//! the host, and it has to read the same `Settings` the shortcuts and the remapping are stored in.
//!
//! **The names are desktop's on purpose** — the `gilrs` ones, see `zeebx::input::gamepads`. A
//! shortcut saved as `Source::button("LeftThumb")` works on the three frontends, and the same
//! `settings.json` can move from the phone to the computer. What the player reads on screen is
//! [`label`], which uses the words `GameController` itself uses.

use zeebx::input::bindings::{Player, Source};
use zeebx::input::{AXIS_RANGE, Pad};

/// The physical buttons, in the order of the mask's bits. `include/zeebx_ios.h` repeats this
/// order, and the test below is what keeps the two from drifting apart on this side.
pub const NAMES: [&str; 17] = [
    "South",
    "East",
    "West",
    "North",
    "LeftTrigger",
    "RightTrigger",
    "LeftTrigger2",
    "RightTrigger2",
    "LeftThumb",
    "RightThumb",
    "Start",
    "Select",
    "Mode",
    "DPadUp",
    "DPadDown",
    "DPadLeft",
    "DPadRight",
];

/// The Zeebo button each physical one presses when the player has remapped nothing.
///
/// It is the map this frontend has always had, and the same as Android's: south on `b1`, east on
/// `b2`, west on `b3`, north on `b4`, the upper triggers on `zl`/`zr`. The console has no Start:
/// Menu lands on HOME, which BREW calls `back`.
const FACTORY: [&str; 17] = [
    "b1", "b2", "b3", "b4", "zl", "zr", "l2", "r2", "lthumb", "rthumb", "back", "start", "start",
    "up", "down", "left", "right",
];

/// How many of [`NAMES`] can become a session shortcut: all but the D-pad, as on Android. A
/// direction that rewinds the game is a mistake waiting for a thumb.
pub const SHORTCUT_CAPABLE: usize = 13;

/// Below this the stick is at rest. It is the value this frontend already used; it was not
/// measured against the 0.12 of the other two.
const DEAD_ZONE: f32 = 0.15;

/// What the player reads for a physical button: the words `GameController` uses.
pub fn label(name: &str) -> &str {
    match name {
        "South" => "A",
        "East" => "B",
        "West" => "X",
        "North" => "Y",
        "LeftTrigger" => "L1",
        "RightTrigger" => "R1",
        "LeftTrigger2" => "L2",
        "RightTrigger2" => "R2",
        "LeftThumb" => "L3",
        "RightThumb" => "R3",
        "Start" => "Menu",
        "Select" => "Options",
        "Mode" => "Home",
        "DPadUp" => "D-pad ↑",
        "DPadDown" => "D-pad ↓",
        "DPadLeft" => "D-pad ←",
        "DPadRight" => "D-pad →",
        other => other,
    }
}

/// The physical buttons that press `zeebo` out of the box.
pub fn factory_for(zeebo: &str) -> Vec<&'static str> {
    NAMES
        .iter()
        .zip(FACTORY)
        .filter(|(_, target)| *target == zeebo)
        .map(|(name, _)| *name)
        .collect()
}

/// The physical buttons the player bound to `zeebo`, in the order they were bound.
pub fn bound_to<'a>(player: &'a Player, zeebo: &str) -> Vec<&'a str> {
    player
        .sources(zeebo)
        .iter()
        .filter_map(|source| match source {
            Source::Button { name } => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// The controller as of the last reading.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Controller {
    /// One bit per entry of [`NAMES`].
    held: u32,
    /// `X`, `Y`, `Z` and `RZ`, already in the console's direction and travel.
    axes: [i32; 4],
}

impl Controller {
    /// Takes a reading and says which buttons went down in it.
    ///
    /// `axes` is the left stick and the right stick as `GameController` gives them: from -1 to 1,
    /// with `y` growing **upward**. The console's grows downward, and the flip is here so Swift
    /// does not have to know it.
    pub fn update(&mut self, held: u32, axes: [f32; 4]) -> u32 {
        let held = held & ((1 << NAMES.len()) - 1);
        let pressed = held & !self.held;
        self.held = held;
        let [x, y, z, rz] = axes;
        for (slot, raw) in self.axes.iter_mut().zip([x, -y, z, -rz]) {
            let value = match raw.abs() < DEAD_ZONE {
                true => 0.0,
                false => raw,
            };
            *slot = (value * AXIS_RANGE as f32).round() as i32;
        }
        pressed
    }

    /// Releases everything: the controller went away, or the app left the screen with a button
    /// still held and will never see it come up.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_held(&self, name: &str) -> bool {
        NAMES
            .iter()
            .position(|known| *known == name)
            .is_some_and(|bit| self.held & (1 << bit) != 0)
    }

    /// Whether anything is pressed or pushed. It is what hides the on-screen controls in
    /// automatic mode: a resting stick does not count, or a controller lying on the table would
    /// take the pieces away from whoever plays on the glass.
    pub fn active(&self) -> bool {
        self.held != 0 || self.axes.iter().any(|axis| *axis != 0)
    }

    /// The Zeebo controller this reading amounts to.
    ///
    /// A physical button the player bound presses what it was bound to, and nothing else. One the
    /// player left alone keeps its factory role — unless that Zeebo button was given to other
    /// physical buttons, in which case it has moved and the old one goes quiet. `reserved` names
    /// the buttons that belong to a session shortcut: those never reach the game.
    pub fn pad(&self, player: Option<&Player>, reserved: impl Fn(&str) -> bool) -> Pad {
        let mut pad = Pad::default();
        for (bit, physical) in NAMES.iter().enumerate() {
            if self.held & (1 << bit) == 0 || reserved(physical) {
                continue;
            }
            let mut bound = false;
            for (zeebo, sources) in player.iter().flat_map(|player| &player.buttons) {
                // The turbo key lives in the same map and is not a Zeebo button: it has no index,
                // and so it presses nothing here.
                let Some(index) = Pad::button_by_name(zeebo) else {
                    continue;
                };
                let here = sources
                    .iter()
                    .any(|source| matches!(source, Source::Button { name } if name == physical));
                if here {
                    pad.press(index, true);
                    bound = true;
                }
            }
            if bound {
                continue;
            }
            let zeebo = FACTORY[bit];
            let moved = player.is_some_and(|player| !bound_to(player, zeebo).is_empty());
            if let (false, Some(index)) = (moved, Pad::button_by_name(zeebo)) {
                pad.press(index, true);
            }
        }
        pad.axes = self.axes;
        pad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bit(name: &str) -> u32 {
        1 << NAMES.iter().position(|known| *known == name).unwrap()
    }

    fn index(name: &str) -> usize {
        Pad::button_by_name(name).unwrap()
    }

    #[test]
    fn every_factory_target_is_a_zeebo_button() {
        for target in FACTORY {
            assert!(Pad::button_by_name(target).is_some(), "{target}");
        }
    }

    #[test]
    fn the_factory_map_is_the_one_ios_always_had() {
        let mut controller = Controller::default();
        controller.update(bit("South") | bit("RightTrigger") | bit("Start"), [0.0; 4]);
        let pad = controller.pad(None, |_| false);
        assert!(pad.is_down(index("b1")));
        assert!(pad.is_down(index("zr")));
        assert!(pad.is_down(index("back")));
        assert!(!pad.is_down(index("b2")));
    }

    #[test]
    fn the_stick_points_down_when_the_console_expects_down() {
        let mut controller = Controller::default();
        controller.update(0, [1.0, 1.0, 0.1, -1.0]);
        let pad = controller.pad(None, |_| false);
        assert_eq!(pad.axes, [128, -128, 0, 128]);
    }

    #[test]
    fn a_bound_button_presses_only_what_it_was_bound_to() {
        let mut player = Player::default();
        player.bind("b1", Source::button("East"));
        let mut controller = Controller::default();

        controller.update(bit("East"), [0.0; 4]);
        let pad = controller.pad(Some(&player), |_| false);
        assert!(pad.is_down(index("b1")));
        assert!(!pad.is_down(index("b2")), "East left its factory role");

        controller.update(bit("South"), [0.0; 4]);
        let pad = controller.pad(Some(&player), |_| false);
        assert_eq!(pad.buttons, 0, "b1 moved to East, and South presses nothing");
    }

    #[test]
    fn the_keyboard_map_of_a_fresh_file_changes_nothing() {
        // `Player::default()` binds keys, not buttons: it must read as "nothing remapped".
        let player = Player::default();
        let mut controller = Controller::default();
        controller.update(bit("North") | bit("DPadLeft"), [0.0; 4]);
        assert_eq!(
            controller.pad(Some(&player), |_| false),
            controller.pad(None, |_| false)
        );
    }

    #[test]
    fn a_reserved_button_does_not_reach_the_game() {
        let mut controller = Controller::default();
        controller.update(bit("LeftThumb") | bit("South"), [0.0; 4]);
        let pad = controller.pad(None, |name| name == "LeftThumb");
        assert!(!pad.is_down(index("lthumb")));
        assert!(pad.is_down(index("b1")));
    }

    #[test]
    fn update_reports_only_what_went_down_now() {
        let mut controller = Controller::default();
        assert_eq!(controller.update(bit("South"), [0.0; 4]), bit("South"));
        assert_eq!(controller.update(bit("South") | bit("East"), [0.0; 4]), bit("East"));
        assert_eq!(controller.update(0, [0.0; 4]), 0);
        assert!(!controller.active());
    }
}
