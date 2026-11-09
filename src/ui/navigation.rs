//! Walking the library with the controller and the keyboard, and not only with the mouse.
//!
//! The dpad, the left stick and the arrow keys choose; button 1, Start, Enter and space open; HOME
//! opens the Z-Wheel. Holding a direction repeats. It came out of egui so that the Qt library
//! walks the same way: reading the keyboard and drawing is each window's job, and what becomes a
//! command lives here.

use std::time::{Duration, Instant};

use crate::input::Pad;

/// Holding a direction repeats the choice after this wait, and at this pace.
const REPEAT_AFTER: Duration = Duration::from_millis(380);
const REPEAT_EVERY: Duration = Duration::from_millis(110);
/// How far the stick has to leave the center to count as a direction, on the ±128 travel.
const STICK_THRESHOLD: i32 = 72;

/// What the input asks of the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Up,
    Down,
    Left,
    Right,
    Open,
    ZWheel,
    /// The settings of the chosen game: the controller's X.
    Configure,
    /// The details of the chosen game: the controller's Y.
    Inspect,
}

const COMMANDS: [Command; 8] = [
    Command::Up,
    Command::Down,
    Command::Left,
    Command::Right,
    Command::Open,
    Command::ZWheel,
    Command::Configure,
    Command::Inspect,
];

/// What is pressed now, in the order of the [`Command`]s: `[up, down, left, right, open, home,
/// configure, inspect]`.
///
/// Configure and inspect are the X and Y of a common controller: `b2` and `b3`, which on the Z-Pad
/// sit to the left of and above the diamond, where X and Y are.
///
/// `key_events` is the window's keyboard — the four arrows and open (Enter or space) —, and `pads`
/// the controllers of the connected ports. **Both become a single state before the press is
/// detected:** an arrow also mapped on the controller is still one step, not two.
pub fn pressed(key_events: [bool; 5], pads: &[Pad]) -> [bool; 8] {
    let button = |name: &str| {
        Pad::button_by_name(name).is_some_and(|i| pads.iter().any(|pad| pad.is_down(i)))
    };
    let axis = |axis: usize, sign: i32| pads.iter().any(|pad| pad.axes[axis] * sign > STICK_THRESHOLD);
    [
        key_events[0] || button("up") || axis(1, -1),
        key_events[1] || button("down") || axis(1, 1),
        key_events[2] || button("left") || axis(0, -1),
        key_events[3] || button("right") || axis(0, 1),
        key_events[4] || button("b1") || button("start"),
        button("back"),
        button("b2"),
        button("b3"),
    ]
}

/// Turns what is pressed into commands: only the press counts, and a held direction repeats.
#[derive(Default)]
pub struct Navigation {
    /// What was pressed on the previous read.
    before: [bool; 8],
    /// Since when the held direction has been pressed, and when it last repeated.
    holding: Option<(usize, Instant, Instant)>,
}

impl Navigation {
    /// The library is not listening — a game open, the settings on top. Everything counts as
    /// already pressed, so the button that closes it does not open a game on the way back.
    pub fn mute(&mut self) {
        self.before = [true; 8];
        self.holding = None;
    }

    /// The commands of this read.
    pub fn commands(&mut self, now: [bool; 8], instant: Instant) -> Vec<Command> {
        let mut commands = Vec::new();
        for (i, command) in COMMANDS.iter().enumerate() {
            if now[i] && !self.before[i] {
                commands.push(*command);
                if i < 4 {
                    self.holding = Some((i, instant, instant));
                }
            }
        }
        match self.holding {
            Some((i, _, _)) if !now[i] => self.holding = None,
            Some((i, since, last))
                if instant - since >= REPEAT_AFTER && instant - last >= REPEAT_EVERY =>
            {
                commands.push(COMMANDS[i]);
                self.holding = Some((i, since, instant));
            }
            _ => {}
        }
        self.before = now;
        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIGHT: [bool; 8] = [false, false, false, true, false, false, false, false];
    const NOTHING: [bool; 8] = [false; 8];

    /// Only the press becomes a command, and the held direction repeats after the wait, at the
    /// pace.
    #[test]
    fn holding_a_direction_repeats_at_the_pace() {
        let start = Instant::now();
        let mut navigation = Navigation::default();
        assert_eq!(navigation.commands(RIGHT, start), vec![Command::Right]);
        let early = start + Duration::from_millis(200);
        assert!(navigation.commands(RIGHT, early).is_empty(), "before the wait");
        let after = start + REPEAT_AFTER;
        assert_eq!(navigation.commands(RIGHT, after), vec![Command::Right]);
        let logo = after + Duration::from_millis(50);
        assert!(navigation.commands(RIGHT, logo).is_empty(), "before the pace");
        assert!(navigation.commands(NOTHING, logo).is_empty());
    }

    /// Muted, the library does not take what was already pressed as a press.
    #[test]
    fn silencing_keeps_the_close_button_from_opening_a_game() {
        let mut navigation = Navigation::default();
        navigation.mute();
        let open = [false, false, false, false, true, false, false, false];
        assert!(navigation.commands(open, Instant::now()).is_empty());
    }

    /// The keyboard arrow and the controller dpad are a single press.
    #[test]
    fn keyboard_and_controller_together_are_one_step() {
        let mut pad = Pad::default();
        pad.press(Pad::button_by_name("right").unwrap(), true);
        let now = pressed([false, false, false, true, false], &[pad]);
        assert_eq!(now, RIGHT);
        let mut stick = Pad::default();
        stick.axes[0] = 100;
        assert_eq!(pressed([false; 5], &[stick]), RIGHT, "the stick moves it too");
    }
}
