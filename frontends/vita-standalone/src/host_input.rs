//! The Vita's controls turned into the Zeebo controller.
//!
//! The numbers are vitasdk's `SceCtrlButtons`, on firmware 3.60. The layout is by position on the
//! device, the same table as the RetroPad in the core: the bottom button is 1, the left one is 2,
//! the top one is 3 and the right one is 4.

use zeebx::input::Pad;

pub const SELECT: u32 = 0x0000_0001;
pub const LEFT_STICK_CLICK: u32 = 0x0000_0002;
pub const RIGHT_STICK_CLICK: u32 = 0x0000_0004;
pub const START: u32 = 0x0000_0008;
pub const UP: u32 = 0x0000_0010;
pub const RIGHT: u32 = 0x0000_0020;
pub const DOWN: u32 = 0x0000_0040;
pub const LEFT: u32 = 0x0000_0080;
pub const LEFT_TRIGGER: u32 = 0x0000_0100;
pub const RIGHT_TRIGGER: u32 = 0x0000_0200;
pub const TRIANGLE: u32 = 0x0000_1000;
pub const CIRCLE: u32 = 0x0000_2000;
pub const CROSS: u32 = 0x0000_4000;
pub const SQUARE: u32 = 0x0000_8000;

/// How much the stick can jitter around the center without the game moving.
///
/// The Vita analog stick rests at 128, and when idle it wobbles a few points. Without this cutoff
/// the character walks on its own, which is the defect the console's center at 128 exists to
/// prevent when the value arrives wrong.
pub const DEAD_ZONE: i32 = 16;

/// Select and Start held together return to the list. Half a second: tapping both in the middle
/// of a game must not exit.
pub const EXIT_HOLD: std::time::Duration = std::time::Duration::from_millis(500);

const MAP: &[(u32, &str)] = &[
    (UP, "up"),
    (DOWN, "down"),
    (LEFT, "left"),
    (RIGHT, "right"),
    (CROSS, "b1"),
    (SQUARE, "b2"),
    (TRIANGLE, "b3"),
    (CIRCLE, "b4"),
    (LEFT_TRIGGER, "zl"),
    (RIGHT_TRIGGER, "zr"),
    (START, "start"),
    (SELECT, "back"),
    (LEFT_STICK_CLICK, "lthumb"),
    (RIGHT_STICK_CLICK, "rthumb"),
];

/// A `sceCtrl` reading, already decoupled from the firmware struct's layout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reading {
    pub buttons: u32,
    pub lx: u8,
    pub ly: u8,
    pub rx: u8,
    pub ry: u8,
}

/// The Vita stick grows to the right and downwards, centered at 128. The console's internal axis
/// grows the same way and stores the value already centered on zero: up is negative. That is why
/// the raw `ly` goes straight in, without inverting.
fn axis(raw: u8) -> i32 {
    let value = i32::from(raw) - 128;
    if value.abs() <= DEAD_ZONE { 0 } else { value }
}

pub fn to_pad(reading: &Reading) -> Pad {
    let mut pad = Pad::default();
    for (bit, name) in MAP {
        if reading.buttons & bit != 0
            && let Some(index) = Pad::button_by_name(name)
        {
            pad.press(index, true);
        }
    }
    pad.set_axis(0, axis(reading.lx));
    pad.set_axis(1, axis(reading.ly));
    pad.set_axis(2, axis(reading.rx));
    pad.set_axis(3, axis(reading.ry));
    pad
}

/// Select and Start at the same time are the request to exit, and do not reach the game. Releasing
/// both in the guest while the chord is held avoids opening the game's menu in the middle of the
/// exit gesture.
pub fn filter_exit(buttons: u32) -> u32 {
    if is_exit_combo(buttons) {
        buttons & !(SELECT | START)
    } else {
        buttons
    }
}

pub fn is_exit_combo(buttons: u32) -> bool {
    buttons & SELECT != 0 && buttons & START != 0
}

pub fn exit_hold_elapsed(elapsed: std::time::Duration) -> bool {
    elapsed >= EXIT_HOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_is_the_bottom_button_and_the_dpad_is_buttons() {
        let reading = Reading {
            buttons: CROSS | UP,
            lx: 0,
            ly: 128,
            rx: 255,
            ry: 128,
        };
        let pad = to_pad(&reading);
        assert!(pad.is_down(Pad::button_by_name("b1").unwrap()));
        assert!(pad.is_down(Pad::button_by_name("up").unwrap()));
        assert!(!pad.is_down(Pad::button_by_name("b4").unwrap()));
        assert_eq!(pad.axes[0], -128, "stick all the way left");
        assert_eq!(pad.axes[1], 0, "the center does not move");
        assert_eq!(pad.axes[2], 127, "right stick all the way right");
    }

    #[test]
    fn the_dead_zone_holds_rest_and_lets_the_rest_through() {
        let stopped = Reading {
            lx: 140,
            ly: 128,
            rx: 128,
            ry: 128,
            ..Reading::default()
        };
        assert_eq!(to_pad(&stopped).axes[0], 0);
        let moving = Reading { lx: 145, ..stopped };
        assert_eq!(to_pad(&moving).axes[0], 17);
    }

    #[test]
    fn select_and_start_together_exit_and_are_hidden_from_the_game() {
        assert!(!is_exit_combo(SELECT | CROSS));
        assert!(is_exit_combo(SELECT | START));
        assert_eq!(filter_exit(SELECT | START | CROSS), CROSS);
        assert_eq!(filter_exit(SELECT | CROSS), SELECT | CROSS);
        assert!(!exit_hold_elapsed(std::time::Duration::from_millis(499)));
        assert!(exit_hold_elapsed(EXIT_HOLD));
    }
}
