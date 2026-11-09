//! The game list: what a press does, and where the cursor ends up.
//!
//! Kept apart from reading the controller so it can be tested without the device. The d-pad
//! repeat exists because a folder with dozens of games is not browsed one tap at a time.

use std::time::Duration;

use crate::host_input;

pub enum Press {
    Nothing,
    Confirm,
    Quit,
    Refresh,
}

/// How long to hold the d-pad before it starts repeating, and the repeat interval.
pub const REPEAT_DELAY: Duration = Duration::from_millis(280);
pub const REPEAT_INTERVAL: Duration = Duration::from_millis(90);

pub fn touch(now: u32, before: u32) -> Press {
    let went_up = |bit: u32| now & bit != 0 && before & bit == 0;
    if went_up(host_input::CROSS) {
        Press::Confirm
    } else if went_up(host_input::CIRCLE) {
        Press::Quit
    } else if went_up(host_input::TRIANGLE) {
        Press::Refresh
    } else {
        Press::Nothing
    }
}

/// `-1` is up, `1` is down. Both together are not a direction: it is the stick pushed into the
/// corner, or both keys, and moving both ways in the same frame has no destination.
pub fn direction(buttons: u32) -> i32 {
    match (buttons & host_input::UP != 0, buttons & host_input::DOWN != 0) {
        (true, false) => -1,
        (false, true) => 1,
        _ => 0,
    }
}

/// The edge takes effect immediately. Holding only repeats after the delay, and from then on at
/// the interval.
pub fn list_step(border: i32, holding: i32, held_for: Duration, since_last: Duration) -> i32 {
    if border != 0 {
        return border;
    }
    if holding != 0 && held_for >= REPEAT_DELAY && since_last >= REPEAT_INTERVAL {
        return holding;
    }
    0
}

pub fn move_cursor(cursor: usize, total: usize, delta: i32) -> usize {
    if total == 0 {
        return 0;
    }
    let current = cursor.min(total - 1) as i32;
    (current + delta).clamp(0, total as i32 - 1) as usize
}

/// The first visible row, with the cursor always inside the window and stuck to the last row
/// when the list is longer than the screen.
pub fn window(cursor: usize, total: usize, visible_items: usize) -> usize {
    if visible_items == 0 || total <= visible_items {
        return 0;
    }
    let cursor = cursor.min(total - 1);
    cursor.saturating_sub(visible_items - 1).min(total - visible_items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_counts_once() {
        assert!(matches!(touch(host_input::CROSS, 0), Press::Confirm));
        assert!(matches!(touch(host_input::CROSS, host_input::CROSS), Press::Nothing));
        assert!(matches!(touch(host_input::CIRCLE, 0), Press::Quit));
        assert!(matches!(touch(host_input::TRIANGLE, 0), Press::Refresh));
    }

    #[test]
    fn the_dpad_repeats_only_after_the_delay() {
        assert_eq!(direction(host_input::UP), -1);
        assert_eq!(direction(host_input::UP | host_input::DOWN), 0);
        assert_eq!(list_step(1, 1, Duration::ZERO, Duration::ZERO), 1);
        assert_eq!(
            list_step(0, 1, Duration::from_millis(100), Duration::from_millis(100)),
            0
        );
        assert_eq!(
            list_step(0, 1, REPEAT_DELAY, REPEAT_INTERVAL),
            1
        );
        assert_eq!(
            list_step(0, 0, Duration::from_secs(2), Duration::from_secs(2)),
            0
        );
    }

    #[test]
    fn the_cursor_stays_in_the_list_and_the_window_follows() {
        assert_eq!(move_cursor(0, 0, 1), 0);
        assert_eq!(move_cursor(0, 3, -1), 0);
        assert_eq!(move_cursor(0, 3, 1), 1);
        assert_eq!(move_cursor(2, 3, 1), 2);
        assert_eq!(window(0, 20, 10), 0);
        assert_eq!(window(10, 20, 10), 1);
        assert_eq!(window(19, 20, 10), 10);
        assert_eq!(window(3, 4, 10), 0);
    }
}
