//! Games made for BREW phones, not for the Zeebo.
//!
//! The Zeebo runs BREW, so a phone game's binary loads and talks to the same API. What differs is
//! what the firmware around it does, and the first difference that stops a game is the keys.

use crate::input::{avk, BUTTON_NAMES, EVT_KEY, EVT_KEY_RELEASE};

/// `EVT_KEY_PRESS`, from `AEE.h`. See [`crate::input::EVT_KEY_RELEASE`] for the numbering.
pub const EVT_KEY_PRESS: u32 = 0x101;

/// A phone game: its applet class, its screen, and the phone key each controller button presses.
struct PhoneGame {
    class: u32,
    /// The phone's screen, width by height. The game is told this size instead of the console's,
    /// and the frontends show only that area — see `Machine::visible_area`.
    screen: (u16, u16),
    /// Button names from [`BUTTON_NAMES`], and the key each one becomes.
    buttons: &'static [(&'static str, u32)],
}

/// The phone games, the ones that get the phone's key sequence and the phone's keys.
///
/// A list and not a guess: nothing in the package says "phone". Devil May Cry answers "handled"
/// to `EVT_KEY` exactly as a Zeebo game would, and the sequence Zeebo games get is not negotiable
/// either — see [`key_events`].
const PHONE_GAMES: &[PhoneGame] = &[
    // Devil May Cry: Dante x Virgil (Capcom, 2008). Its handler (`0xae8` in `dmc_mas.mod`)
    // answers `EVT_KEY` with a bare `return TRUE` and keeps its key bits only on `EVT_KEY_PRESS`
    // and `EVT_KEY_RELEASE` — without the press, the first menu never moved.
    //
    // The buttons follow the game's own defaults, from HELP > KEY OPTION: gun `9`, sword `8`,
    // jump `#`, style `0`, evade `OK`. In the menus `OK` confirms and the left soft key goes back;
    // in a mission the soft keys are OPTION and INFO. Whoever wants another layout remaps it in
    // the game, which accepts 0-9, `#`, `*` and `SELECT` for every action.
    //
    // Its art is `wqvga*.ggz`, made for a 240×400 phone held upright. Told 640×480 it still
    // runs — it centres a 240-pixel column and paints the rest almost white — but told 240×400 it
    // fills exactly that area (measured on the title, the menus and a mission). 240×320 also
    // works, more cramped; 240×400 is what its art was drawn for.
    PhoneGame {
        class: 0x0107_0e36,
        screen: (240, 400),
        buttons: &[
            ("up", avk::UP),
            ("down", avk::DOWN),
            ("left", avk::LEFT),
            ("right", avk::RIGHT),
            ("b1", avk::SELECT),
            ("b2", avk::ZERO + 8),
            ("b3", avk::ZERO + 9),
            ("b4", avk::POUND),
            ("zr", avk::ZERO),
            // HOME arrives as `start` too (`Pad::with_home_on_both`): `back` stays unmapped, or
            // one press would be two.
            ("start", avk::SOFT1),
            ("zl", avk::SOFT2),
        ],
    },
];

fn phone_game(class: u32) -> Option<&'static PhoneGame> {
    PHONE_GAMES.iter().find(|game| game.class == class)
}

/// Whether the applet of this class is a phone game.
pub fn is_phone_applet(class: u32) -> bool {
    phone_game(class).is_some()
}

/// The phone's screen, width by height, when the applet of this class is a phone game.
pub fn phone_screen(class: u32) -> Option<(u16, u16)> {
    phone_game(class).map(|game| game.screen)
}

/// The phone key a controller button presses in the game of this class, if it is a phone game
/// and the button has one.
pub fn button_key(class: u32, button: usize) -> Option<u32> {
    let name = BUTTON_NAMES.get(button)?;
    phone_game(class)?
        .buttons
        .iter()
        .find(|(known, _)| known == name)
        .map(|&(_, key)| key)
}

/// Whether a key the frontend sends is one the phone mapping replaces.
///
/// Every frontend turns the D-pad into arrows and buttons 1 and 2 into `CONFIRM` and `CLR` (see
/// [`crate::input::controller_key_events`]). In a phone game the same buttons already press the
/// game's keys through [`button_key`], so these would arrive twice — or, for `CONFIRM`, as a key
/// the phone never had. Other keys, such as a digit typed on the desktop keyboard, go through.
pub fn replaces_console_key(class: u32, key: u32) -> bool {
    is_phone_applet(class)
        && matches!(
            key,
            avk::UP | avk::DOWN | avk::LEFT | avk::RIGHT | avk::CONFIRM | avk::CLR
        )
}

/// The events one key change becomes, in order, for the applet of this class.
///
/// A BREW phone sends `EVT_KEY_PRESS`, `EVT_KEY` and `EVT_KEY_RELEASE`. The Zeebo does not send
/// the press: Quake reads `0x101` as one, and with it controller button 2 would open its menu in
/// the middle of a level, which the console does not do (see [`crate::input::EVT_KEY_RELEASE`]).
/// So the press goes only to the games in [`PHONE_GAMES`].
pub fn key_events(class: u32, down: bool) -> &'static [u32] {
    match (down, is_phone_applet(class)) {
        (true, true) => &[EVT_KEY_PRESS, EVT_KEY],
        (true, false) => &[EVT_KEY],
        (false, _) => &[EVT_KEY_RELEASE],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVIL_MAY_CRY: u32 = 0x0107_0e36;

    #[test]
    fn a_phone_game_gets_the_press_before_the_key() {
        assert!(is_phone_applet(DEVIL_MAY_CRY));
        assert_eq!(key_events(DEVIL_MAY_CRY, true), [EVT_KEY_PRESS, EVT_KEY]);
        assert_eq!(key_events(DEVIL_MAY_CRY, false), [EVT_KEY_RELEASE]);
    }

    #[test]
    fn the_controller_plays_devil_may_cry_with_its_default_keys() {
        use crate::input::{avk, Pad};
        let key = |name: &str| button_key(DEVIL_MAY_CRY, Pad::button_by_name(name).unwrap());
        assert_eq!(key("up"), Some(avk::UP));
        assert_eq!(key("b1"), Some(avk::SELECT), "evade, and confirm in the menus");
        assert_eq!(key("b2"), avk::digit(8), "sword");
        assert_eq!(key("b3"), avk::digit(9), "gun");
        assert_eq!(key("b4"), Some(avk::POUND), "jump");
        assert_eq!(key("zr"), avk::digit(0), "style");
        assert_eq!(key("start"), Some(avk::SOFT1), "back / option");
        assert_eq!(key("zl"), Some(avk::SOFT2), "info / reset");
        // HOME arrives as START too (`Pad::with_home_on_both`): mapping both would press twice.
        assert_eq!(key("back"), None);
    }

    #[test]
    fn the_console_keys_are_replaced_only_for_a_phone_game() {
        use crate::input::avk;
        for code in [avk::UP, avk::DOWN, avk::LEFT, avk::RIGHT, avk::CONFIRM, avk::CLR] {
            assert!(replaces_console_key(DEVIL_MAY_CRY, code), "{code:#x}");
            assert!(!replaces_console_key(0x0101_0000, code), "{code:#x}");
        }
        // A digit typed on the desktop keyboard is still a digit.
        assert!(!replaces_console_key(DEVIL_MAY_CRY, avk::digit(5).unwrap()));
    }

    #[test]
    fn devil_may_cry_gets_the_phone_screen_it_was_drawn_for() {
        // Its art is `wqvga*.ggz`: a 240×400 phone, held upright.
        assert_eq!(phone_screen(DEVIL_MAY_CRY), Some((240, 400)));
        assert_eq!(phone_screen(0x0101_0000), None, "a Zeebo game keeps the console's screen");
    }

    #[test]
    fn a_zeebo_game_keeps_the_console_sequence() {
        let zeebo_game = 0x0101_0000;
        assert!(!is_phone_applet(zeebo_game));
        assert_eq!(key_events(zeebo_game, true), [EVT_KEY]);
        assert_eq!(key_events(zeebo_game, false), [EVT_KEY_RELEASE]);
    }
}
