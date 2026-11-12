//! The Zeebo controller, regardless of where the input comes from.
//!
//! The identifiers come from the console's `hid_devices.original.cfg` — the entry with VID
//! `0x1EAA` and PID `0x0135`, the "New Zeebo Game Controller" — and the UID names from
//! `AEEHIDDevice_Joystick.h` in the Zeebo SDK. The game identifies each button by UID, not by
//! index, so what has to be right is the table.

pub mod bindings;
#[cfg(feature = "desktop")]
pub mod gamepads;
#[cfg(feature = "desktop")]
pub mod padview;
pub mod phone;
pub mod sensors;
pub mod touch;
pub mod wiimote;

/// How many input ports the console has.
///
/// They are the two USB ports on the front of the unit. It is not a chosen number: the Z-Wheel's
/// `GetConnectedDevices` passes **two** as the size of the output array, both times it calls it.
pub const PORTS: usize = 2;

/// The BREW key event, which is how the console delivers the keyboard to the application.
///
/// The keyboard in BREW **does not go through `IHID`**: `IHID` says one exists, and the keys
/// arrive as events at the application's handler, with the virtual code in `wParam`. The Z-Wheel
/// confirms it: the handler of the opening form, at `0x11828`, tests `r1 == 0x100` and compares
/// `r2` with `0xe030`, `0xe04a` and others — which are `AVK_`.
pub const EVT_KEY: u32 = 0x100;

/// The release of a key, in the numbering of `AEE.h`: `EVT_KEY_PRESS` is `0x101` and
/// `EVT_KEY_RELEASE` is `0x102`.
///
/// **The release went out as `0x101`, which is the press.** Quake only reads `0x101` and `0x102`
/// — it is Quake itself that confirms the numbering: the handler at `0x3a8ec` treats `0x101` as a
/// press and `0x102` as a release, and its own translation of the controller into keys sends
/// `0x101` on the press (`0x49a24`). Controller button 2 becomes `AVK_CLR`, which Quake binds to
/// `togglemenu`: its release arrived as a second press that was never released, the key stayed
/// held down forever, and every following press was discarded as a repeat — the menu no longer
/// opened during the level.
///
/// The press is still only `EVT_KEY`. With `EVT_KEY_PRESS` alongside, button 2 would open Quake's
/// menu mid-level, and on the unit it does not: in the longplay, "Sempre correr desligado"
/// ("Always run off") appears without any menu.
pub const EVT_KEY_RELEASE: u32 = 0x102;

/// The BREW virtual codes the console uses, in the numbering of `AEEVCodes.h`.
///
/// The numbering was checked against the Z-Wheel module, which is what depends on it most:
///
/// - the function at `0x44914` is the Z-Wheel's own translation of the analog stick into keys. It
///   maps the four directions, in the order up, down, left, right, to `0xe031` through `0xe034`;
/// - the handlers at `0x11888` and `0x158c0` treat `0xe030` the same as `0xe04a` (back) and
///   `0xe035` the same as `0xe064` (confirm) — `AVK_CLR` and `AVK_SELECT` next to the controller
///   buttons.
pub mod avk {
    pub const ZERO: u32 = 0xe021;
    pub const STAR: u32 = 0xe02b;
    pub const POUND: u32 = 0xe02c;
    pub const CLR: u32 = 0xe030;
    pub const UP: u32 = 0xe031;
    pub const DOWN: u32 = 0xe032;
    pub const LEFT: u32 = 0xe033;
    pub const RIGHT: u32 = 0xe034;
    pub const SELECT: u32 = 0xe035;
    /// The phone's two soft keys, under the screen. Devil May Cry's key table (`0xc78` in
    /// `dmc_mas.mod`) puts them right after `SELECT`, at `AVK_0 + 21` and `+ 22`.
    pub const SOFT1: u32 = 0xe036;
    pub const SOFT2: u32 = 0xe037;
    /// The controller's confirm button. The Z-Wheel accepts it wherever it accepts `SELECT`; the
    /// z-pad instructions screen only advances with it.
    pub const CONFIRM: u32 = 0xe064;

    /// The code of a digit, or `None` if it is not a digit.
    pub fn digit(n: u32) -> Option<u32> {
        (n <= 9).then_some(ZERO + n)
    }

    /// The code of a key by the name the configuration uses.
    pub fn by_name(name: &str) -> Option<u32> {
        Some(match name {
            "up" => UP,
            "down" => DOWN,
            "left" => LEFT,
            "right" => RIGHT,
            "select" | "ok" => SELECT,
            "confirma" => CONFIRM,
            "star" => STAR,
            "pound" => POUND,
            "clr" => CLR,
            "soft1" => SOFT1,
            "soft2" => SOFT2,
            // A raw code, in hexadecimal, for when you are finding out which key it is:
            // `0xe063` says what `right` cannot say yet.
            other if other.starts_with("0x") => {
                return u32::from_str_radix(&other[2..], 16).ok();
            }
            other => return other.parse::<u32>().ok().and_then(digit),
        })
    }
}

/// How many buttons the controller has.
pub const BUTTONS: usize = 18;

/// UID of each button, in the order the console enumerates them.
///
/// The list has oddities — index 3 carries the UID of an axis, and `Right_Shoulder_Upper`
/// appears twice — but it is what the console's file says, and it is what the games saw when
/// they were made. Fixing it here would be inventing a controller that never existed.
pub const BUTTON_UIDS: [u32; BUTTONS] = [
    // **The four face buttons come first, in the order 1, 2, 3, 4.** The game decided this: the
    // eleven Data East arcade ports do not wait for button events, they scan `GetButtonInfo`
    // from 0 to 15 every frame and store the state **by index** (Bad Dudes' loop is at
    // `0x1f2a8` in its module). For whoever reads this way, the UID does not matter — the
    // position does. While `Button_2` opened the list and `Right_Shoulder_Upper` came right
    // after, controller button 2 reached the arcade as the right trigger and 4 as 3, while our
    // `b2` and our `b3`, at positions 16 and 17, were never even read.
    0x0106_c40b, // Z-Pad Button_1 (the console's file labels it `Button_2` — see below)
    0x0106_c40a, // Button_2
    0x0106_c40c, // Button_3
    0x0106_c40d, // Button_4
    0x0106_c408, // Right_Shoulder_Upper
    0x0106_c407, // Left_Shoulder_Lower
    0x0106_c406, // Left_Shoulder_Upper
    0x0106_c409, // Right_Shoulder_Lower
    0x0106_c405, // Right_Thumbstick
    0x0106_c403, // Back
    0x0106_c404, // Left_Thumbstick
    0x0106_c402, // Start
    // The D-pad is not in the list of the console's file, which describes it as the `X` and `Y`
    // axes. But games read it as buttons: with these four UIDs present, Quake's menu moves;
    // without them, the cursor does not budge however much the axis changes. A digital D-pad on
    // USB HID is usually reported both ways, and that is what we do.
    //
    // **The order is that of the UIDs: up, left, down, right.** Some read the button by its
    // position in the list, not by UID: the `GamepadMgr` of the SDK samples stores the state in
    // an array indexed by the `id` of `GetNextButtonEvent` (`0x16b68` in Dragon Vs Chicken). In
    // the order up, down, left, right, the demo moved down when left was pressed.
    0x0106_c3fe, // DPad_Up
    0x0106_c3ff, // DPad_Left
    0x0106_c400, // DPad_Down
    0x0106_c401, // DPad_Right
    // What is left goes **after** the range the ports scan. `lx` holds the axis UID the console's
    // file left in the middle of the button list, and `zrb` is the second appearance of
    // `Right_Shoulder_Upper` in the same file: neither of them is a controller button, and
    // leaving them in front cost two of the sixteen positions the arcade reads.
    0x0106_c4d0, // LeftThumb_X
    0x0106_c408, // Right_Shoulder_Upper, again
];

/// Indices of the four D-pad directions in [`BUTTON_UIDS`], in the order up, down, left, right.
/// In the list they are in UID order — see [`BUTTON_UIDS`].
pub const DPAD: [usize; 4] = [12, 14, 13, 15];

/// The indices of HOME, which BREW calls `Back`, and of `Start` in [`BUTTON_UIDS`]. See
/// [`Pad::with_home_on_both`].
pub const HOME: usize = 9;
pub const START: usize = 11;

/// Name of each button, for key mapping and for the command line.
/// The controller has **one** trigger on each side, ZL and ZR, and they report as the "upper"
/// one of each pair: in Zeeboids, which draws `ZL` and `ZR` on screen and turns the character
/// with them, `Left_Shoulder_Upper` and `Right_Shoulder_Upper` turn and the "lower" ones do
/// nothing. The two lower ones stay here because they are in the console's file, but they have
/// no button.
/// **`b1` sits on the UID the console's file labels `Button_2`, and this is deliberate.**
///
/// The `Button_N` labels of the controller entry in `hid_devices.original.cfg` are shifted —
/// the same file puts an axis UID in `BUTTON:3`, so its inconsistency was already known. What
/// settles the shift is the behavior of the games, measured on the unit: with `b1` on
/// `0x0106c40a`, the south button reached the games as **2** and the east one as **1**. Since
/// south is the 1 printed on the Z-Pad, button 1's UID is `0x0106c40b`.
///
/// The UIDs of `b2`, `b3` and `b4` stay where they were: there is no measurement of them yet.
/// What was measured is the **order**, and it holds for whoever scans the list by index — see
/// [`BUTTON_UIDS`].
pub const BUTTON_NAMES: [&str; BUTTONS] = [
    "b1", "b2", "b3", "b4", "zr", "l2", "zl", "r2", "rthumb", "back", "lthumb", "start", "up",
    "left", "down", "right", "lx", "zrb",
];

/// UID of each axis: `X`, `Y`, `Z` and `RZ`.
///
/// Three of them are **literally** those of the console's `hid_devices.original.cfg`, in the
/// Zeebo controller entry (`VID:0x1EAA:PID:0x0135`):
///
/// ```text
/// AXIS:X:0x0106C40C      <- not this one
/// AXIS:Y:0x0106C4D1
/// AXIS:Z:0x0106C4CE
/// AXIS:RZ:0x0106C4CF
/// ```
///
/// The file has a swap, and it is visible from both sides: `AXIS:X` got `0x0106C40C`, which is a
/// button UID (`Button_3`), and `BUTTON:3` of the same entry got `0x0106C4D0`, which is an axis
/// UID — the `LeftThumb_X` of the file's other entries.
///
/// **Here we undo the swap, and what decided it was measurement, not deduction.** With
/// `0x0106C40C` on `X`, the stick does not move left and right in any game: the game scans the
/// `GetAxesInfo` table looking for an axis UID, finds none for `X` and never stores its field —
/// `Y`, `Z` and `RZ` move, and only the horizontal stays dead. With `0x0106C4D0` the whole stick
/// moves.
///
/// This does **not** contradict the lesson that a primary source beats inference: the question
/// here is not "what does the file say", it is "what does the game look for", and the one who
/// answers that is the game. The file is still the source for all the rest of the table.
pub const AXIS_UIDS: [u32; 4] = [0x0106_c4d0, 0x0106_c4d1, 0x0106_c4ce, 0x0106_c4cf];

/// Name of each axis, in the order of [`Pad::axes`], for the mapping and the settings screen.
pub const AXIS_NAMES: [&str; 4] = ["x", "y", "z", "rz"];

/// Which word of `AEEHIDPositionInfo` each axis falls into.
///
/// The struct starts with `boolean bRelativeAxes` and continues with `nX`, `nY`, `nZ`, `nRx`,
/// `nRy`, `nRz` and eighteen more fields (`inc/AEEIHIDDevice.h` in the Zeebo SDK). The console
/// uses `X`, `Y`, `Z` and `RZ`, which are words 1, 2, 3 and 6.
pub const AXIS_SLOTS: [usize; 4] = [1, 2, 3, 6];

/// How many words `AEEHIDPositionInfo` has: `bRelativeAxes` and twenty-four axes.
pub const POSITION_INFO_WORDS: usize = 25;

/// Range of an axis **as the console reports it**: an unsigned byte, with rest in the middle.
///
/// This is not our choice, it is measured. Zeebo F.C. Super League converts each axis like this
/// (`0xfd4c0` in its module, and the same stretch is in Tênis, in Zeeboids and in every game that
/// uses this SDK layer):
///
/// ```text
/// mvn   r0, #0x7f        ; r0 = -128
/// sxtah r4, r0, r4       ; value = (int16)axis - 128
/// ```
///
/// The game subtracts **128** from the raw value to find the center, so the unit's center is 128
/// and the range is `0..=255` — which is also what the USB sticks listed in
/// `hid_devices.original.cfg` report (Logitech Dual Action, RumblePad2). While we sent zero,
/// every game on this layer read the extreme and walked on its own into a corner: that was the
/// "character walking up" with the stick at rest.
///
/// Zeeboids escaped by accident: it treats "all four axes exactly at the minimum" as "there is
/// no stick here" and zeroes everything — a defense against exactly what we were doing.
pub const AXIS_MIN: i32 = 0;
pub const AXIS_MAX: i32 = 255;

/// The rest position, in the console's range.
pub const AXIS_CENTER: i32 = 128;

/// How far [`Pad::axes`] travels to each side.
///
/// We store the axis **centered on zero**, which is the form the game itself works with after
/// the subtraction above; the translation to the unit's range is done by `IHIDDevice`, in the
/// only place where the console is the one reading.
pub const AXIS_RANGE: i32 = 128;

/// The state of the controller at one instant.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Pad {
    /// One bit per button, at the index the console enumerates it.
    pub buttons: u32,
    /// The `X`, `Y`, `Z` and `RZ` axes, in that order.
    pub axes: [i32; 4],
}

impl Pad {
    pub fn is_down(&self, index: usize) -> bool {
        self.buttons & (1 << index) != 0
    }

    /// Presses or releases a button. **The D-pad does not touch the axes**, and the reason is the
    /// hardware.
    ///
    /// `hid_devices.original.cfg` lists **four** axes — `X`/`Y` and `Z`/`RZ` —, which are two
    /// sticks. The Z-Pad has an analog stick and a digital D-pad: the D-pad is a button, and what
    /// feeds `X`/`Y` is the left stick. The Z-Wheel registers `RegisterForPositionChange`
    /// because it is navigated **with the stick**.
    ///
    /// Putting a digital D-pad on the axes breaks every game that reads changes instead of state:
    /// releasing the direction sends the axis back to center, and that return is a second change,
    /// of opposite sign. In Zeeboids the menu goes down one option and comes right back. This has
    /// been tried three times — mirrored on both channels, only on the axes, and again here — and
    /// it failed all three.
    pub fn press(&mut self, index: usize, down: bool) {
        let bit = 1 << index;
        if down {
            self.buttons |= bit;
        } else {
            self.buttons &= !bit;
        }
    }

    /// Sets an axis to the given value, clamped to the stick's travel. Zero is rest.
    pub fn set_axis(&mut self, index: usize, value: i32) {
        if let Some(axis) = self.axes.get_mut(index) {
            *axis = value.clamp(-AXIS_RANGE, AXIS_RANGE);
        }
    }

    /// The axis in the console's range: rest becomes 128, and the travel, `0..=255`.
    pub fn console_axis(&self, index: usize) -> i32 {
        let value = self.axes.get(index).copied().unwrap_or(0);
        (value + AXIS_CENTER).clamp(AXIS_MIN, AXIS_MAX)
    }

    /// Mirrors the D-pad onto the `X` and `Y` axes, for games that only read the stick.
    ///
    /// **It is not the default, and the reason is measured.** On the console the D-pad is a
    /// button: whoever polls state sees it in `GetButtonInfo`, and whoever only listens to the
    /// axis sees nothing. Putting the D-pad on the axes is what `e705840` did, and `4418fe9`
    /// undid — Zeeboids polls **both channels** every loop (613 `GetNextButtonEvent` and 612
    /// `GetPositionState` in twenty seconds) and started moving two squares per tap. Leaving it
    /// **only** on the axes was worse: releasing the direction sends the axis back to center, and
    /// whoever reads changes reads that return as a step in the opposite direction.
    ///
    /// It stays here, behind an option that is off by default, because the defect is **per
    /// game**: whoever only reads the stick starts seeing the D-pad, and whoever reads both
    /// channels stays as it was.
    ///
    /// The buttons **stay pressed**: the option adds the axis, it does not switch the channel.
    /// Whoever reads both is what moves two squares, and that is the case the option exists to
    /// leave off.
    ///
    /// Up is a **negative** value on the internal `Y`: on the console the axis grows downward, and
    /// the low value is what the game reads as up — see
    /// `up_on_the_analog_is_the_low_value_on_the_console`.
    pub fn mirror_dpad_on_axes(&mut self) {
        // The four directions, in the order of [`DPAD`]: up, down, left, right.
        //
        // **Releasing returns the axis to center**, rather than leaving it where it was: without
        // that the stick would stay pushed forever after the first tap. It is the same return to
        // center that [`Pad::press`] notes as the risk of the idea — whoever reads changes reads
        // that return as a step in the opposite direction.
        //
        // The two opposite directions pressed together give center, not the last one to arrive.
        let vertical = match (self.is_down(DPAD[0]), self.is_down(DPAD[1])) {
            (true, false) => -AXIS_RANGE,
            (false, true) => AXIS_RANGE,
            _ => 0,
        };
        let horizontal = match (self.is_down(DPAD[2]), self.is_down(DPAD[3])) {
            (true, false) => -AXIS_RANGE,
            (false, true) => AXIS_RANGE,
            _ => 0,
        };
        self.set_axis(1, vertical);
        self.set_axis(0, horizontal);
    }

    /// The controller as the console delivers it: HOME presses `Back` **and** `Start`.
    ///
    /// The Zeebo controller has a single button in the middle, and the games disagree on its UID.
    /// Double Dragon asks "APERTE O BOTÃO HOME" ("PRESS THE HOME BUTTON") and only recognizes
    /// `Back`: on the title screen, `back` opens the menu and `start` does nothing. Quake asks
    /// "Aperte o botão Home para chamar o menu" ("Press the Home button to bring up the menu")
    /// and only recognizes `Start`: its UID table (`0x7a4` in the module) takes `Start` to index
    /// 9, which becomes `AVK_CLR` and from there the key bound to `togglemenu`, and sends `Back`
    /// to "unknown button". Both run on the same unit, so HOME arrives with both.
    ///
    /// **It was not measured on the console.** The survey of the games' UID tables — the ones
    /// starting at `0x0106c3fe` — found none that gives the two different actions: FIFA 09, Need
    /// for Speed, Quake II and Action Hero 3D only recognize `Back`; Alien Breaker, Tork and Kral
    /// and Heavy Weapon store both in separate bits, and I found no read of the `Start` bit in any
    /// of the three.
    ///
    /// Only in this direction: `start` pressed alone, by someone who mapped it by hand, is still
    /// only `Start`.
    pub fn with_home_on_both(mut self) -> Self {
        if self.is_down(HOME) {
            self.press(START, true);
        }
        self
    }

    /// The buttons that changed between `self` and `next`, with the new state of each.
    pub fn changes(&self, next: &Pad) -> Vec<(usize, bool)> {
        (0..BUTTONS)
            .filter(|&i| self.is_down(i) != next.is_down(i))
            .map(|i| (i, next.is_down(i)))
            .collect()
    }

    /// The index of the button named `name`.
    ///
    /// Accepts `home` as another name for `back`. The Zeebo controller has a **HOME** button
    /// printed on the shell, and it is the one the console's `hid_devices.cfg` maps to
    /// `AEEUID_HIDJoystick_Back` — there is no "Home" UID in `AEEHIDDevice_Joystick.h`. Double
    /// Dragon asks "APERTE O BOTÃO HOME" ("PRESS THE HOME BUTTON") on the title screen, and
    /// whoever reads that has no way to guess the button is called `back` in here.
    pub fn button_by_name(name: &str) -> Option<usize> {
        let name = match name {
            "home" => "back",
            other => other,
        };
        BUTTON_NAMES.iter().position(|&n| n == name)
    }
}

/// Converts a frontend key to the corresponding BREW virtual code.
///
/// It lives in the engine because desktop, headless and Android need the same convention.
///
/// `Esc` and `P` are left out on purpose: in the desktop window they are quit and pause.
pub fn avk_for(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    Some(match key {
        ArrowUp => avk::UP,
        ArrowDown => avk::DOWN,
        ArrowLeft => avk::LEFT,
        ArrowRight => avk::RIGHT,
        Enter | Space => avk::CONFIRM,
        Backspace | Delete => avk::CLR,
        Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9 =>
            avk::ZERO + (key as u32 - Num0 as u32),
        _ => return None,
    })
}

/// The BREW keys that are active now: the keyboard ones, already as AVK, plus the ones the
/// controllers press.
///
/// A physical arrow can also be mapped on the controller, and it is still a single press: that is
/// why the result is a set, and only the difference between two of them becomes an event
/// ([`transitions`]).
pub fn active_avks(
    keyboard: impl IntoIterator<Item = u32>,
    pads: &[Pad],
) -> std::collections::HashSet<u32> {
    let mut active: std::collections::HashSet<u32> = keyboard.into_iter().collect();
    for pad in pads {
        active.extend(
            controller_key_events(&Pad::default(), pad)
                .into_iter()
                .filter_map(|(key, down)| down.then_some(key)),
        );
    }
    active
}

/// The key events that lead from `prior` to `current`, releases before presses, and `prior`
/// becomes `current`.
///
/// Storing the delivered set, and not each source, is what makes releasing `4` while the arrow
/// is still held not release the AVK that both represent.
pub fn transitions(
    prior: &mut std::collections::HashSet<u32>,
    current: std::collections::HashSet<u32>,
) -> Vec<(u32, bool)> {
    let mut events: Vec<_> = prior
        .difference(&current)
        .map(|&key| (key, false))
        .collect();
    events.extend(current.difference(prior).map(|&key| (key, true)));
    events.sort_unstable();
    *prior = current;
    events
}

/// The keys the controller sends, compared with the previous frame.
///
/// On the console the D-pad reaches applications as the four BREW arrows, and those are what the
/// Z-Wheel navigates with: left and right turn the wheel and switch the list tab, up and down
/// flip the pages. The analog stick is not handled here: the Z-Wheel reads the position and
/// translates it by itself (`0x44914` in the module).
///
/// It lives outside the UI because **every frontend** needs this translation: the desktop window
/// and the Libretro core deliver the same pair of frames and expect the same keys.
pub fn controller_key_events(before: &Pad, now: &Pad) -> Vec<(u32, bool)> {
    // The two face buttons follow the Z-Wheel's own help (`assets/zeebo/pt/controls.html`):
    // "Sim (Botão 1)" ("Yes (Button 1)") chooses and "Voltar (Botão 2)" ("Back (Button 2)")
    // cancels. Back is `AVK_CLR`, measured: on the help screen it goes back to the menu, and
    // `0xe065` does nothing.
    const BUTTON_KEYS: [(&str, u32); 6] = [
        ("up", avk::UP),
        ("down", avk::DOWN),
        ("left", avk::LEFT),
        ("right", avk::RIGHT),
        ("b1", avk::CONFIRM),
        ("b2", avk::CLR),
    ];
    let mut key_events = Vec::new();
    for (name, code) in BUTTON_KEYS {
        let Some(index) = Pad::button_by_name(name) else {
            continue;
        };
        if now.is_down(index) != before.is_down(index) {
            key_events.push((code, now.is_down(index)));
        }
    }
    key_events
}

/// A key script, to exercise input without a window.
///
/// It exists so things can be tested: without it, the only way to know whether input works is to
/// press the key and look, and that does not fit in a test or an automated run.
#[derive(Debug, Default, Clone)]
pub struct Script {
    /// Each entry is `(start, end, button index)`, in milliseconds of virtual time.
    steps: Vec<(u32, u32, usize)>,
    /// The same for the axes: `(start, end, axis index, value)`.
    ///
    /// It exists because not every console input is a button. The Z-Wheel registers
    /// `RegisterForPositionChange` and **only** that: the game wheel moves by the axis, and a
    /// script that only knows how to press buttons has no way to measure it.
    axes: Vec<(u32, u32, usize, i32)>,
}

impl Script {
    /// How long each key stays pressed, when the script does not say. A human tap.
    pub const HOLD_MS: u32 = 120;

    /// Reads a script in the format `ms:key[:duration_ms][,...]`.
    ///
    /// The time is that of the game's virtual clock, not the number of loop iterations: an
    /// iteration does not always last the same, and the same script has to hold across runs.
    ///
    /// The keys are the button names in [`BUTTON_NAMES`], including `up`, `down`, `left` and
    /// `right` for the D-pad.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut steps = Vec::new();
        let mut axes = Vec::new();
        for item in text.split(',').filter(|s| !s.is_empty()) {
            let mut parts = item.split(':');
            let at = parts.next().unwrap_or_default();
            let key = parts
                .next()
                .ok_or_else(|| format!("expected ms:key in {item:?}"))?;
            let at: u32 = at
                .parse()
                .map_err(|_| format!("invalid time in {item:?}"))?;
            let hold: u32 = match parts.next() {
                Some(text) => text
                    .parse()
                    .map_err(|_| format!("invalid duration in {item:?}"))?,
                None => Self::HOLD_MS,
            };
            let end = at.saturating_add(hold.max(1));
            if let Some((axis, value)) = axis_from_name(key) {
                axes.push((at, end, axis, value));
                continue;
            }
            let index =
                Pad::button_by_name(key).ok_or_else(|| format!("unknown key: {key:?}"))?;
            steps.push((at, end, index));
        }
        Ok(Self { steps, axes })
    }

    /// Puts on the controller what the script says at this instant.
    /// A button stays pressed if **any** step wants it pressed now.
    ///
    /// Applying step by step looks the same and is not: a script that uses the same button twice
    /// — and every script that navigates a menu does, because confirming is always the same key —
    /// would have its first press undone by the second step, which has not arrived yet and so says
    /// release. The press vanished without a trace, and the script seemed to have no effect.
    pub fn apply(&self, now_ms: u32, pad: &mut Pad) {
        let mut pressed_buttons = 0u32;
        for &(start, end, index) in &self.steps {
            if (start..end).contains(&now_ms) {
                pressed_buttons |= 1 << index;
            }
        }
        for &(_, _, index) in &self.steps {
            pad.press(index, pressed_buttons & (1 << index) != 0);
        }
        // The axis returns to center when no step wants it displaced — releasing the analog stick
        // is part of the gesture, and an axis stuck at an extreme turns the wheel forever.
        for &(_, _, axis, _) in &self.axes {
            let value = self
                .axes
                .iter()
                .find(|&&(axis_start, axis_end, which, _)| which == axis && (axis_start..axis_end).contains(&now_ms))
                .map_or(0, |&(_, _, _, value)| value);
            pad.set_axis(axis, value);
        }
    }
}

/// Reads `x+`, `x-`, `y+`, `y-`, `z+`, `z-`, `rz+`, `rz-` as `(axis index, value)`.
///
/// The four axes are those of the console's `hid_devices.cfg`, in the order it enumerates them:
/// `X`, `Y`, `Z` and `RZ`. The sign says which way, and the value is the end stop — halfway is
/// no use for finding out whether an axis does anything.
fn axis_from_name(name: &str) -> Option<(usize, i32)> {
    let (axis, sign) = name.split_at(name.len().checked_sub(1)?);
    let value = match sign {
        "+" => AXIS_RANGE,
        "-" => -AXIS_RANGE,
        _ => return None,
    };
    let axis = match axis {
        "x" => 0,
        "y" => 1,
        "z" => 2,
        "rz" => 3,
        _ => return None,
    };
    Some((axis, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_and_controller_share_a_press() {
        use std::collections::HashSet;
        // The controller's `b1` and the keyboard's `Enter` send the same `CONFIRM`: it is the
        // pair that shares a command now that the D-pad has left the translation.
        let mut pad = Pad::default();
        pad.press(Pad::button_by_name("b1").unwrap(), true);
        let keyboard = [avk_for(egui::Key::Enter).unwrap()];
        let mut delivered = HashSet::new();
        let active = active_avks(keyboard, &[pad]);
        assert_eq!(transitions(&mut delivered, active.clone()), vec![(avk::CONFIRM, true)]);
        assert!(transitions(&mut delivered, active).is_empty());
        // Releasing the keyboard does not release a command still held by the controller.
        let active = active_avks([], &[pad]);
        assert!(transitions(&mut delivered, active).is_empty());
        assert_eq!(transitions(&mut delivered, HashSet::new()), vec![(avk::CONFIRM, false)]);
    }

    /// The digits come from the order of `egui::Key`, and from `AVK_0` onward. Both lists are
    /// contiguous today; if one stops being so, this is where it shows.
    #[test]
    fn digits_become_avks() {
        use egui::Key;
        assert_eq!(avk_for(Key::Num0), Some(avk::ZERO));
        assert_eq!(avk_for(Key::Num7), Some(avk::ZERO + 7));
        assert_eq!(avk_for(Key::Num9), Some(avk::ZERO + 9));
        assert_eq!(avk_for(Key::Backspace), Some(avk::CLR));
        assert_eq!(avk_for(Key::Escape), None);
        assert_eq!(avk_for(Key::P), None);
    }


    /// Button 1 is south, and its UID is the one the console's file labels `Button_2`.
    ///
    /// Measured on the unit: with `b1` on `0x0106c40a`, south reached the games as 2 and east
    /// as 1. See the note on [`BUTTON_NAMES`] — this test exists so the swap is not undone by
    /// someone who trusts the file's label.
    #[test]
    fn button_one_uses_the_uid_the_file_calls_two() {
        let one = Pad::button_by_name("b1").expect("b1 exists");
        let two = Pad::button_by_name("b2").expect("b2 exists");
        assert_eq!(BUTTON_UIDS[one], 0x0106_c40b);
        assert_eq!(BUTTON_UIDS[two], 0x0106_c40a);
    }

    /// The indices of [`DPAD`] are written by hand; this test keeps them and [`BUTTON_NAMES`] from
    /// drifting apart without anyone noticing.
    #[test]
    fn the_dpad_indices_match_the_names() {
        let [up, down, left, right] = DPAD;
        for (index, name) in [
            (up, "up"),
            (down, "down"),
            (left, "left"),
            (right, "right"),
        ] {
            assert_eq!(BUTTON_NAMES[index], name, "index {index}");
        }
    }

    /// The raw code is for finding out keys; the names still work.
    #[test]
    fn raw_code_in_hexadecimal() {
        assert_eq!(avk::by_name("0xe034"), Some(0xe034));
        assert_eq!(avk::by_name("0xE033"), Some(0xe033));
        assert_eq!(avk::by_name("3"), Some(avk::ZERO + 3));
        assert_eq!(avk::by_name("clr"), Some(avk::CLR));
        assert_eq!(avk::by_name("0xzz"), None);
    }

    /// The arrows in the numbering of `AEEVCodes.h`, which is the one the Z-Wheel uses at
    /// `0x44914`.
    #[test]
    fn the_arrows_follow_the_header() {
        assert_eq!(
            [
                avk::CLR,
                avk::UP,
                avk::DOWN,
                avk::LEFT,
                avk::RIGHT,
                avk::SELECT
            ],
            [0xe030, 0xe031, 0xe032, 0xe033, 0xe034, 0xe035]
        );
        assert_eq!(avk::by_name("confirma"), Some(0xe064));
    }

    #[test]
    fn home_presses_back_and_start() {
        assert_eq!(BUTTON_NAMES[HOME], "back");
        assert_eq!(BUTTON_NAMES[START], "start");
        let mut home = Pad::default();
        home.press(HOME, true);
        let seen = home.with_home_on_both();
        assert!(seen.is_down(HOME) && seen.is_down(START));
        assert_eq!(
            Pad::default().changes(&seen),
            vec![(HOME, true), (START, true)]
        );

        let mut start = Pad::default();
        start.press(START, true);
        assert!(
            !start.with_home_on_both().is_down(HOME),
            "start alone does not become HOME"
        );
    }

    #[test]
    fn the_home_button_is_back_by_another_name() {
        // The Zeebo controller has HOME printed on the shell, and the console's `hid_devices.cfg`
        // maps it to `Back` — there is no "Home" UID in the joystick header. Double Dragon asks
        // "APERTE O BOTÃO HOME" ("PRESS THE HOME BUTTON"), and nobody would guess it is called
        // `back` in here.
        assert_eq!(Pad::button_by_name("home"), Pad::button_by_name("back"));
        assert!(Pad::button_by_name("home").is_some());
    }

    #[test]
    fn the_dpad_moves_the_button_not_the_axis() {
        let [up, down, left, right] = DPAD;
        let mut pad = Pad::default();

        // The D-pad is a button. On the axes, a digital D-pad undoes its own step when released
        // — see the note on `press`, and the Zeeboids menu.
        pad.press(left, true);
        assert!(pad.is_down(left));
        assert_eq!(pad.axes, [0; 4]);
        pad.press(up, true);
        pad.press(down, true);
        pad.press(left, false);
        assert_eq!(pad.axes, [0; 4]);

        // What reaches the axes is the stick, without going through the buttons.
        pad.set_axis(0, AXIS_RANGE);
        assert_eq!(pad.axes[0], AXIS_RANGE);
        assert!(!pad.is_down(right));
    }

    #[test]
    fn an_idle_axis_reaches_the_console_as_its_center() {
        // The game does `value = (int16)axis - 128` (Super League's `0xfd4c0`). Sending zero is
        // sending the end stop: that was the character walking into a corner on its own with the
        // stick at rest.
        let pad = Pad::default();
        for axis in 0..AXIS_NAMES.len() {
            assert_eq!(pad.console_axis(axis), AXIS_CENTER, "axis {axis}");
            assert_eq!(
                pad.console_axis(axis) - AXIS_CENTER,
                0,
                "the game sees it at rest"
            );
        }
    }

    #[test]
    fn the_stick_travel_covers_the_console_range_without_exceeding_it() {
        let mut pad = Pad::default();
        pad.set_axis(0, AXIS_RANGE);
        pad.set_axis(1, -AXIS_RANGE);
        assert_eq!(pad.console_axis(0), AXIS_MAX);
        assert_eq!(pad.console_axis(1), AXIS_MIN);
        // And a value beyond the travel does not escape the range we told the game in `GetMax`.
        pad.set_axis(2, AXIS_RANGE * 4);
        assert_eq!(pad.console_axis(2), AXIS_MAX);
    }

    #[test]
    fn changes_come_out_in_index_order() {
        let mut before = Pad::default();
        before.press(0, true);
        before.press(11, true);

        let mut after = before;
        after.press(0, false);
        after.press(5, true);

        assert_eq!(before.changes(&after), vec![(0, false), (5, true)]);
        // Button 11 is still pressed, so it is not a change.
        assert!(after.is_down(11));
        assert_eq!(after.changes(&after), vec![]);
    }

    #[test]
    fn the_script_presses_and_releases_buttons_and_directions() {
        let start = Pad::button_by_name("start").unwrap();
        let script = Script::parse("1000:start,2000:right:50").unwrap();
        let mut pad = Pad::default();

        script.apply(1000, &mut pad);
        assert!(pad.is_down(start));
        script.apply(1000 + Script::HOLD_MS, &mut pad);
        assert!(!pad.is_down(start));

        // The D-pad is a button, and the duration given in the script holds.
        let right = Pad::button_by_name("right").unwrap();
        script.apply(2000, &mut pad);
        assert!(pad.is_down(right));
        script.apply(2050, &mut pad);
        assert!(!pad.is_down(right));

        // The same button at two moments: the first press cannot be undone by the next step,
        // which has not arrived yet.
        let script = Script::parse("1000:b2:200,3000:b2:200").unwrap();
        let b2 = Pad::button_by_name("b2").unwrap();
        let mut pad = Pad::default();
        script.apply(1100, &mut pad);
        assert!(pad.is_down(b2), "the second step undid the first press");
        script.apply(2000, &mut pad);
        assert!(!pad.is_down(b2));
        script.apply(3100, &mut pad);
        assert!(pad.is_down(b2));

        assert!(Script::parse("10:nao-existe").is_err());
        assert!(Script::parse("start").is_err());
        assert!(Script::parse("10:start:xis").is_err());

        // An empty script is valid and touches nothing.
        let before = pad;
        Script::parse("").unwrap().apply(20, &mut pad);
        assert_eq!(pad, before);
    }

    #[test]
    fn the_axes_are_the_console_files_except_for_the_x_swap() {
        // `Y`, `Z` and `RZ` are a literal transcription of the `VID:0x1EAA:PID:0x0135` entry of
        // `hid_devices.original.cfg`. `X` is `LeftThumb_X`, and not the button UID the file put
        // there: with the file's one, the stick does not move sideways in any game, because no
        // game looks for a button UID in the axis table. Measured on the unit.
        assert_eq!(
            AXIS_UIDS,
            [0x0106_c4d0, 0x0106_c4d1, 0x0106_c4ce, 0x0106_c4cf]
        );
        assert_eq!(AXIS_NAMES, ["x", "y", "z", "rz"]);
        // And the axis UID cannot also sit on a button that fires: it would be the same number
        // reaching the game as two things. The file's `lx` is a button that does not exist on
        // the unit and is never pressed — but the test records the overlap on purpose.
        let lx = BUTTON_NAMES
            .iter()
            .position(|&n| n == "lx")
            .expect("lx is in the list");
        assert_eq!(
            BUTTON_UIDS[lx], AXIS_UIDS[0],
            "the file's swap is mirrored"
        );
        // And it stays **outside** the first sixteen positions, which are the ones the arcade
        // ports scan: a button that does not exist on the unit cannot take the slot of one that
        // does.
        assert!(lx >= 16, "lx does not take a slot in the range the arcade reads");
    }

    /// The option from issue #39: the D-pad also writes to the axes, when enabled.
    #[test]
    fn the_mirrored_dpad_moves_the_axes_and_keeps_the_button_held() {
        let mut pad = Pad::default();
        assert_eq!(pad.axes, [0, 0, 0, 0], "rest is the internal zero");

        // Up: negative on `Y`, because on the console the axis grows downward.
        pad.press(DPAD[0], true);
        pad.mirror_dpad_on_axes();
        assert_eq!(pad.axes[1], -AXIS_RANGE);
        assert_eq!(pad.console_axis(1), AXIS_MIN, "up is the low value");
        assert!(pad.is_down(DPAD[0]), "the button stays pressed");

        // Down, on the same `Pad`, to prove one direction undoes the other.
        pad.press(DPAD[0], false);
        pad.press(DPAD[1], true);
        pad.mirror_dpad_on_axes();
        assert_eq!(pad.axes[1], AXIS_RANGE);
        assert_eq!(pad.console_axis(1), AXIS_MAX);

        // Right and left on `X`.
        pad.press(DPAD[1], false);
        pad.press(DPAD[3], true);
        pad.mirror_dpad_on_axes();
        assert_eq!(pad.axes[0], AXIS_RANGE);
        pad.press(DPAD[3], false);
        pad.press(DPAD[2], true);
        pad.mirror_dpad_on_axes();
        assert_eq!(pad.axes[0], -AXIS_RANGE);

        // Releasing the direction does **not** zero the axis: the next frame does, when the
        // D-pad is no longer pressed — and that return to center is what `Pad::press` notes.
        pad.press(DPAD[2], false);
        pad.mirror_dpad_on_axes();
        pad.mirror_dpad_on_axes();
        assert_eq!(pad.axes[0], 0);

        // And the axes that do not belong to the left stick are left untouched.
        assert_eq!([pad.axes[2], pad.axes[3]], [0, 0]);
    }

    /// The D-pad at rest **centers** the left stick's axes, and does not touch the right one's.
    ///
    /// The order matters for the caller: the mirror writes zero, so it has to run **before** the
    /// analog loop — otherwise the real stick, resting at center, would be wiped out without
    /// anyone having pressed anything.
    #[test]
    fn a_released_dpad_centers_the_left_stick_and_leaves_the_right_alone() {
        let mut pad = Pad::default();
        pad.set_axis(2, AXIS_RANGE);
        pad.mirror_dpad_on_axes();
        assert_eq!([pad.axes[0], pad.axes[1]], [0, 0]);
        assert_eq!(pad.axes[2], AXIS_RANGE, "`Z` does not belong to the D-pad");
    }

    #[test]
    fn every_button_has_a_name_and_names_do_not_repeat() {
        assert_eq!(BUTTON_NAMES.len(), BUTTON_UIDS.len());
        for (i, name) in BUTTON_NAMES.iter().enumerate() {
            assert_eq!(Pad::button_by_name(name), Some(i));
        }
        assert_eq!(Pad::button_by_name("nao-existe"), None);
    }
}
