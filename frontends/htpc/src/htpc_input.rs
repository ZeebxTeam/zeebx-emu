//! Gamepad-first HTPC navigation and Quick Menu combo detection.
//!
//! Unlike the desktop library navigation in [`zeebx::ui::navigation`], the HTPC interface reads
//! standard controller buttons directly across all connected gamepads so any controller (Xbox,
//! DualSense, Switch Pro, Steam Input) navigates every screen out of the box without manual port
//! setup, while also honoring the user's Quick Menu combo without colliding with the Zeebo `HOME`
//! button.

use std::time::{Duration, Instant};

use zeebx::input::bindings::Source;
use zeebx::input::gamepads::Gamepads;
use zeebx::ui::settings::QuickMenuCombo;

/// Initial delay before a held direction starts repeating.
const REPEAT_AFTER: Duration = Duration::from_millis(360);
/// Interval between repeated steps while a direction stays held.
const REPEAT_EVERY: Duration = Duration::from_millis(105);

/// Commands emitted to the HTPC QML shell and active theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum HtpcCommand {
    Up = 0,
    Down = 1,
    Left = 2,
    Right = 3,
    Confirm = 4,
    Back = 5,
    ActionX = 6,
    ActionY = 7,
    TabPrev = 8,
    TabNext = 9,
    Start = 10,
    Select = 11,
    QuickMenu = 12,
}

const ALL: [HtpcCommand; 13] = [
    HtpcCommand::Up,
    HtpcCommand::Down,
    HtpcCommand::Left,
    HtpcCommand::Right,
    HtpcCommand::Confirm,
    HtpcCommand::Back,
    HtpcCommand::ActionX,
    HtpcCommand::ActionY,
    HtpcCommand::TabPrev,
    HtpcCommand::TabNext,
    HtpcCommand::Start,
    HtpcCommand::Select,
    HtpcCommand::QuickMenu,
];

/// Reads the current pressed state of all 13 HTPC commands from all connected controllers.
pub fn read_gamepads(gamepads: &Gamepads, combo: QuickMenuCombo) -> [bool; 13] {
    let btn = |name: &str| gamepads.any_active(&Source::Button { name: name.to_string() });
    let axis = |name: &str, positive: bool| {
        gamepads.any_active(&Source::Axis {
            name: name.to_string(),
            positive,
        })
    };

    let quick_menu = match combo {
        QuickMenuCombo::L3R3 => btn("LeftThumb") && btn("RightThumb"),
        QuickMenuCombo::StartSelect => btn("Start") && btn("Select"),
        QuickMenuCombo::Guide => btn("Mode"),
    };

    [
        btn("DPadUp") || axis("LeftStickY", true),
        btn("DPadDown") || axis("LeftStickY", false),
        btn("DPadLeft") || axis("LeftStickX", false),
        btn("DPadRight") || axis("LeftStickX", true),
        btn("South"),
        btn("East"),
        btn("West"),
        btn("North"),
        btn("LeftTrigger"),
        btn("RightTrigger"),
        // Suppress standalone Start/Select when the Start+Select Quick Menu combo is held.
        btn("Start") && !quick_menu,
        btn("Select") && !quick_menu,
        quick_menu,
    ]
}

/// Edge detector with directional repeat and safe screen-transition muting.
#[derive(Default)]
pub struct HtpcNavigation {
    before: [bool; 13],
    holding: Option<(usize, Instant, Instant)>,
}

impl HtpcNavigation {
    /// Marks every button as already held so closing a screen or opening the Quick Menu never
    /// immediately triggers an action in the newly focused screen.
    pub fn mute(&mut self) {
        self.before = [true; 13];
        self.holding = None;
    }

    /// Processes the current button state.
    ///
    /// When `quick_menu_only` is true (a game is actively running with the Quick Menu closed),
    /// only [`HtpcCommand::QuickMenu`] is emitted and all other inputs remain muted until the
    /// overlay opens.
    pub fn poll(
        &mut self,
        now: [bool; 13],
        quick_menu_only: bool,
        instant: Instant,
    ) -> Vec<HtpcCommand> {
        if quick_menu_only {
            let q = HtpcCommand::QuickMenu as usize;
            let triggered = now[q] && !self.before[q];
            self.before = [true; 13];
            self.before[q] = now[q];
            self.holding = None;
            return if triggered {
                vec![HtpcCommand::QuickMenu]
            } else {
                Vec::new()
            };
        }

        let mut out = Vec::new();
        for (i, &cmd) in ALL.iter().enumerate() {
            if now[i] && !self.before[i] {
                out.push(cmd);
                if i < 4 {
                    self.holding = Some((i, instant, instant));
                }
            }
        }
        match self.holding {
            Some((i, _, _)) if !now[i] => self.holding = None,
            Some((i, since, last))
                if instant.saturating_duration_since(since) >= REPEAT_AFTER
                    && instant.saturating_duration_since(last) >= REPEAT_EVERY =>
            {
                out.push(ALL[i]);
                self.holding = Some((i, since, instant));
            }
            _ => {}
        }
        self.before = now;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_menu_only_blocks_other_buttons_and_detects_edge() {
        let mut nav = HtpcNavigation::default();
        let t0 = Instant::now();
        let mut state = [false; 13];
        state[HtpcCommand::Confirm as usize] = true;
        assert!(nav.poll(state, true, t0).is_empty());

        state[HtpcCommand::QuickMenu as usize] = true;
        assert_eq!(nav.poll(state, true, t0), vec![HtpcCommand::QuickMenu]);
        // Holding the combo does not re-trigger on the next frame.
        assert!(nav.poll(state, true, t0 + Duration::from_millis(16)).is_empty());
    }

    #[test]
    fn directional_hold_repeats_at_pace() {
        let mut nav = HtpcNavigation::default();
        let t0 = Instant::now();
        let mut right = [false; 13];
        right[HtpcCommand::Right as usize] = true;

        assert_eq!(nav.poll(right, false, t0), vec![HtpcCommand::Right]);
        assert!(nav.poll(right, false, t0 + Duration::from_millis(200)).is_empty());
        assert_eq!(
            nav.poll(right, false, t0 + REPEAT_AFTER),
            vec![HtpcCommand::Right]
        );
        assert_eq!(
            nav.poll(right, false, t0 + REPEAT_AFTER + REPEAT_EVERY),
            vec![HtpcCommand::Right]
        );
    }
}
