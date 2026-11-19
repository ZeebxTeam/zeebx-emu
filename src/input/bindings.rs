//! Where each button of the Zeebo controller comes from.
//!
//! The mapping is stored by **name** — the key's name, the host controller button's, the Zeebo
//! button's — and not by index. Indices change when the table changes; names survive, and that
//! is what lets a settings file written today keep working later.
//!
//! This module knows neither the keyboard nor the controller: it only says *what* triggers
//! *what*. The interface is what knows whether the `Z` key is pressed, and it is the one that
//! translates.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::input::{self, Pad};
use crate::speed::turbo::TurboMode;

/// An input source on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "tipo", rename_all = "snake_case")]
pub enum Source {
    /// A key, by the name the interface uses (`Z`, `Space`, `ArrowUp`).
    Key { name: String },
    /// A button of a real controller (`South`, `Start`, `LeftTrigger`).
    Button { name: String },
    /// An axis of a controller, with the direction that counts. That is how an analog stick
    /// triggers a digital D-pad.
    Axis { name: String, positive: bool },
}

/// Where an analog axis of the console comes from.
///
/// It is different from a [`Source::Axis`]: that one asks "did the player push this way?" and
/// serves to trigger a button; this one carries the axis's whole value, which is what an analog
/// stick is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AxisSource {
    /// The host controller's axis, by name (`LeftStickX`).
    pub name: String,
    /// Whether the direction runs opposite to what the console expects.
    #[serde(default)]
    pub invert: bool,
}

impl AxisSource {
    fn new(name: &str, invert: bool) -> Self {
        Self {
            name: name.to_string(),
            invert,
        }
    }
}

/// Below how much travel the analog stick counts as at rest.
///
/// Without this a stick that does not return exactly to the center would leave the axis jittering
/// near zero, and the game would see the controller oscillating on its own.
/// The stick's dead zone, as a fraction of its travel.
///
/// Public because the Libretro core needs the **same** rule in the loop that reads the RetroPad:
/// there the resting stick arrives as zero, and writing that zero on top would erase what the
/// D-pad mirror has just put there — the `zeebx_dpad_to_analog_pN` option would have no effect,
/// and only in the core.
pub const DEADZONE: f32 = 0.12;

impl Source {
    pub fn key(name: &str) -> Self {
        Self::Key {
            name: name.to_string(),
        }
    }

    pub fn button(name: &str) -> Self {
        Self::Button {
            name: name.to_string(),
        }
    }

    /// How the source appears on screen.
    pub fn label(&self) -> String {
        match self {
            Self::Key { name } => name.clone(),
            Self::Button { name } => name.clone(),
            Self::Axis { name, positive } => {
                let sign = match positive {
                    true => '+',
                    false => '-',
                };
                format!("{name}{sign}")
            }
        }
    }

    /// Whether the source is from the keyboard — which decides where the interface will read it
    /// from.
    pub fn is_key(&self) -> bool {
        matches!(self, Self::Key { .. })
    }
}

/// What the console sees connected to a port.
///
/// The Zeebo has two USB ports and accepts both things: the Z-Wheel's
/// `IHID::GetConnectedDevices` is called twice, once asking for `0x0106c3fd` — the joystick — and
/// once asking for `0x0106c3fc`. When the second comes back empty, it prints
/// `No keyboard reported`. In other words, **a keyboard is a real device** to the console, not a
/// figure of speech.
///
/// This is separate from which host controller feeds the port: the computer's keyboard driving a
/// console joystick is the combination every game understands, and it remains the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Device {
    /// The Dragon, the controller that announces `1EAA:0135` ("New Zeebo Game Controller" in
    /// `hid_devices.cfg`). It is what the USB descriptor captured from a console shows, and what
    /// the emulator has always presented; that is why the serialized name is still `Controle`.
    #[serde(rename = "Controle")]
    Controller,
    /// The Z-Pad, which announces `1A5C:3033` ("Zeebo Game Controller"). The buttons and the axes
    /// are the same as the Dragon's — what changes is the grip —, so for the game the only
    /// difference is the VID/PID pair, which the Boomerang Sports games use to choose how to
    /// handle it.
    ZPad,
    /// A USB keyboard. The console enumerates it; a game that uses one, we have not seen yet.
    #[serde(rename = "Teclado")]
    Keyboard,
    /// The Boomerang, the motion controller: D-pad, buttons 1 and 2, HOME and accelerometer. The
    /// Boomerang Sports games recognize it and switch to motion play; the others see it as a
    /// controller with few buttons. See [`crate::machine`], `boomerang_packet`.
    Boomerang,
}

impl Default for Device {
    fn default() -> Self {
        Self::Controller
    }
}

/// One player's mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Player {
    /// Whether the port has anything connected. A disabled port is not enumerated, and that is how
    /// you test a game that behaves differently with two controllers.
    #[serde(rename = "ligada")]
    pub enabled: bool,
    /// Which device the console sees on this port.
    #[serde(rename = "aparelho")]
    pub device_kind: Device,
    /// Which host controller feeds this player, by the name the system gives it. `None` leaves
    /// the player on the keyboard only.
    pub device: Option<String>,
    /// The sources of each Zeebo button, by the button's name. A button can have more than one —
    /// that is what lets `Space` and `X` do the same thing.
    pub buttons: BTreeMap<String, Vec<Source>>,
    /// Where each analog axis of the console comes from, by the axis's name (`x`, `y`, `z`,
    /// `rz`).
    ///
    /// Empty leaves the axes **at rest**: the digital D-pad does not feed them on its own. For it
    /// to feed them there is [`Player::dpad_to_axes`].
    #[serde(default)]
    pub axes: BTreeMap<String, AxisSource>,
    /// Mirrors the D-pad onto the `X`/`Y` axes, for games that only read the stick.
    ///
    /// **Off by default, and it is the same setting in both frontends:** here and in the
    /// Libretro core's `zeebx_dpad_to_analog`. The reason it is not the default is in
    /// [`Pad::mirror_dpad_on_axes`], with the two measured attempts that undid it.
    #[serde(default)]
    #[serde(rename = "direcional_nos_eixos")]
    pub dpad_to_axes: bool,
    /// The calibration of the motion sensor that feeds this port, for the Boomerang.
    #[serde(default)]
    #[serde(rename = "calibracao_movimento")]
    pub motion_calibration: MotionCalibration,
    /// How this player's turbo key works. The key lives in `buttons`, under the name
    /// [`crate::speed::turbo::TURBO_BUTTON`]. See [`crate::speed::turbo`].
    #[serde(default)]
    pub turbo: TurboMode,
    /// The button that pulses in the single-button modes.
    #[serde(default = "default_turbo_button")]
    #[serde(rename = "botao_do_turbo")]
    pub turbo_button: String,
}

fn default_turbo_button() -> String {
    crate::speed::turbo::DEFAULT_BUTTON.to_string()
}

/// The correction of a host accelerometer: what it measures at rest, face up, becomes exactly
/// `[0, 0, 1]` g. In thousandths, so the mapping stays comparable by equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MotionCalibration {
    /// Each axis's offset, in mg.
    pub zero_mg: [i32; 3],
    /// How much the sensor measures for 1 g, in mg.
    #[serde(rename = "escala_mg")]
    pub scale_mg: i32,
}

impl Default for MotionCalibration {
    fn default() -> Self {
        Self {
            zero_mg: [0; 3],
            scale_mg: 1000,
        }
    }
}

impl MotionCalibration {
    /// The largest per-axis offset a calibration can correct, in mg. A controller accelerometer
    /// is off by a few tens of mg; an offset of 1 g is the controller calibrated on its side.
    const MAX_DEVIATION_MG: i32 = 250;

    /// The calibration that takes the average `stopped` — read with the controller motionless
    /// and face up — to `[0, 0, 1]`. `None` when the reading is not that of a face-up controller:
    /// calibrating like that saves a 1 g offset, and every game starts seeing doubled gravity.
    pub fn from_rest(stopped: [f32; 3]) -> Option<Self> {
        let scale = stopped.iter().map(|v| v * v).sum::<f32>().sqrt().max(0.1);
        let zero = [stopped[0], stopped[1], stopped[2] - scale];
        let calibration = Self {
            zero_mg: zero.map(|v| (v * 1000.0).round() as i32),
            scale_mg: (scale * 1000.0).round() as i32,
        };
        (stopped[2] > 0.0 && calibration.is_plausible()).then_some(calibration)
    }

    /// Whether the calibration only corrects what a sensor gets wrong. One saved on its side,
    /// from before this check existed, does not count.
    pub fn is_plausible(&self) -> bool {
        self.zero_mg.iter().all(|d| d.abs() <= Self::MAX_DEVIATION_MG)
            && (500..=2000).contains(&self.scale_mg)
    }

    pub fn apply(&self, raw: [f32; 3]) -> [f32; 3] {
        if !self.is_plausible() {
            return raw;
        }
        let scale = self.scale_mg.max(100) as f32 / 1000.0;
        std::array::from_fn(|i| (raw[i] - self.zero_mg[i] as f32 / 1000.0) / scale)
    }
}

impl Default for Player {
    /// The default is any emulator's: the left hand on the arrows and the right on the action
    /// keys. The `H` on HOME is there because the button is called `Back` internally, and nobody
    /// would guess that reading "APERTE O BOTÃO HOME" ("PRESS THE HOME BUTTON") on Double
    /// Dragon's screen.
    fn default() -> Self {
        let keyboard: [(&str, &[&str]); 12] = [
            ("up", &["ArrowUp"]),
            ("down", &["ArrowDown"]),
            ("left", &["ArrowLeft"]),
            ("right", &["ArrowRight"]),
            ("b1", &["Z"]),
            ("b2", &["Space", "X"]),
            ("b3", &["C"]),
            ("b4", &["V"]),
            ("zl", &["Q"]),
            ("zr", &["W"]),
            ("lthumb", &["F"]),
            ("rthumb", &["G"]),
        ];
        let mut buttons: BTreeMap<String, Vec<Source>> = keyboard
            .iter()
            .map(|(button, keys)| {
                (
                    button.to_string(),
                    keys.iter().map(|key| Source::key(key)).collect(),
                )
            })
            .collect();
        // The console's HOME is BREW's `Back`, and answers to both names. The Zeebo controller
        // has no Start: HOME plays that role, so Enter lands on it.
        buttons.insert(
            "back".to_string(),
            vec![
                Source::key("H"),
                Source::key("Backspace"),
                Source::key("Enter"),
            ],
        );
        Self {
            // Port one starts enabled; the others do not. See [`Controls::default`].
            enabled: true,
            device_kind: Device::Controller,
            device: None,
            buttons,
            // The keyboard has no analog stick: the axes stay at rest, and what feeds them is a
            // controller's stick or the D-pad with `dpad_to_axes` on.
            axes: BTreeMap::new(),
            dpad_to_axes: false,
            motion_calibration: MotionCalibration::default(),
            turbo: TurboMode::Off,
            turbo_button: default_turbo_button(),
        }
    }
}

impl Player {
    /// The typical mapping of the host controller `device` — or the plain keyboard, without a
    /// controller. It is what the controls screen's "restore" puts on the port.
    pub fn default_for_controller(device: Option<String>) -> Self {
        match device {
            // The Wii Remote does not go through gilrs, but it is a controller like the others:
            // its typical mapping comes along, and is changed on screen like any other.
            Some(name) if crate::input::wiimote::Wiimotes::index_of_name(&name).is_some() => {
                Self::with_wiimote(name)
            }
            Some(name) => Self::with_gamepad(name),
            None => Self::default(),
        }
    }

    /// Switches the host controller that feeds this port.
    ///
    /// Choosing a controller brings its typical mapping along; going without a controller returns
    /// to the plain keyboard. In both cases whatever was configured by hand is lost, and that is
    /// why the switch is a deliberate click in a list. Switching the controller switches **the
    /// mapping**, not the port: whether it is enabled and what the console sees on it were decided
    /// before, and losing that here would be the configuration undoing itself when a device is
    /// picked in the list.
    pub fn switch_controller(&mut self, device: Option<String>) {
        let (enabled, device_kind) = (self.enabled, self.device_kind);
        *self = Self::default_for_controller(device);
        self.enabled = enabled;
        // **A host controller on a keyboard port becomes a controller for the console.**
        // `device_kind` is what the console enumerates, and a port marked as keyboard does not
        // enter the list of joysticks the games ask for: whoever picked the second controller for
        // port two still went unseen as the second player. The other choices (Z-Pad, Boomerang)
        // are already controllers and stay where they are.
        self.device_kind = match (device_kind, &self.device) {
            (Device::Keyboard, Some(_)) => Device::Controller,
            (other, _) => other,
        };
    }

    /// The typical mapping of a modern controller, for whoever connects one and wants to play.
    pub fn with_gamepad(device: String) -> Self {
        // **The hand's position, not the button's label.** On the device 1 is at the bottom, 2
        // on the left, 3 at the top and 4 on the right (official images of the controller); on a
        // modern controller, `South` is the bottom one, `West` the left, `North` the top and
        // `East` the right. Each Zeebo button lands on the host button **in the same place** —
        // that is what issue #41 asked for, and what keeps the hand from relearning anything when
        // switching controllers.
        //
        // The four come from [`Self::action_buttons_by_position`], which is **the same table as
        // the migration's**: two parallel lists is exactly what diverged in issue #41.
        let pad: [(&str, &str); 5] = [
            ("zl", "LeftTrigger"),
            ("zr", "RightTrigger"),
            // The Zeebo controller has no Start; HOME takes its place.
            ("back", "Start"),
            ("lthumb", "LeftThumb"),
            ("rthumb", "RightThumb"),
        ];
        let dpad: [(&str, &str); 4] = [
            ("up", "DPadUp"),
            ("down", "DPadDown"),
            ("left", "DPadLeft"),
            ("right", "DPadRight"),
        ];
        let mut player = Self::default();
        for (button, source) in Self::action_buttons_by_position() {
            player
                .buttons
                .entry(button.to_string())
                .or_default()
                .push(source);
        }
        for (button, source) in pad.iter().chain(dpad.iter()) {
            player
                .buttons
                .entry(button.to_string())
                .or_default()
                .push(Source::button(source));
        }
        // The stick does **not** press the D-pad. It is an axis, and it comes in as an axis right
        // below; also binding it to the buttons would make the two act together and neither on
        // its own, which is exactly what you want to avoid on a controller with a cross D-pad.
        player.buttons.entry("back".to_string()).or_default().push(
            // The host controller's HOME lands on the Zeebo's HOME, which is `Back`.
            Source::button("Select"),
        );
        player.axes = Self::default_axes();
        player.device = Some(device);
        player
    }

    /// The Wii Remote's typical mapping, added to the keyboard.
    ///
    /// It is what it did before it was mappable — D-pad on the D-pad, 1 and A on button 1, 2 and
    /// B on button 2, HOME on HOME —, with minus and plus on buttons 3 and 4 for whoever uses it as
    /// a Z-Pad. The rest is changed on the controls screen, as with any controller.
    pub fn with_wiimote(device: String) -> Self {
        let mut player = Self::default();
        player.add_wiimote_buttons();
        player.device = Some(device);
        player
    }

    fn add_wiimote_buttons(&mut self) {
        let wiimote: [(&str, &str); 11] = [
            ("up", "WiiUp"),
            ("down", "WiiDown"),
            ("left", "WiiLeft"),
            ("right", "WiiRight"),
            ("b1", "Wii1"),
            ("b1", "WiiA"),
            ("b2", "Wii2"),
            ("b2", "WiiB"),
            ("b3", "WiiMinus"),
            ("b4", "WiiPlus"),
            ("back", "WiiHome"),
        ];
        for (button, source) in wiimote {
            self.bind(button, Source::button(source));
        }
    }

    /// A modern controller's axes on the console's axes.
    ///
    /// The Zeebo's D-pad is reported as `X` and `Y`, so the left stick lands on those and the
    /// right one on the ones left over, `Z` and `RZ`.
    ///
    /// `Y` is inverted because on the console up is the negative value, and in the controller
    /// library up is positive. The right pair follows the same convention for lack of any other
    /// reference: the console's file names `Z` and `RZ` without giving the direction of either.
    pub fn default_axes() -> BTreeMap<String, AxisSource> {
        [
            ("x", "LeftStickX", false),
            ("y", "LeftStickY", true),
            ("z", "RightStickX", false),
            ("rz", "RightStickY", true),
        ]
        .into_iter()
        .map(|(axis, source, invert)| (axis.to_string(), AxisSource::new(source, invert)))
        .collect()
    }

    /// Gives axes to a mapping saved before they existed.
    ///
    /// Without this, whoever already had a controller configured was left with mute sticks until
    /// redoing the mapping by hand — and would have no way to guess that it was needed.
    fn adopt_axes(&mut self) {
        if self.device.is_none() || !self.axes.is_empty() {
            return;
        }
        self.axes = Self::default_axes();
        // The old mapping bound the stick to the D-pad buttons, which was the way for it to be
        // of any use when there were no axes. Now there are, and keeping both would make the
        // stick press the D-pad on top of moving the axis.
        for button in ["up", "down", "left", "right"] {
            if let Some(sources) = self.buttons.get_mut(button) {
                sources.retain(|source| !matches!(source, Source::Axis { .. }));
            }
        }
    }

    /// Gives the vertical inversion back to whoever lost it.
    ///
    /// There was a version that saved all four axes with no inversion at all, and whoever ran it
    /// was left with the file that way — the default changing back does not fix a map already
    /// saved. Since that version's map is recognizable (it is today's default with `y` and `rz`
    /// upright), it can be undone without touching anyone who changed the mapping by hand.
    /// The host controller's four action buttons, as they were **before** the remapping by
    /// position (issue #41): east on `b2`, west on `b3` and north on `b4`.
    fn legacy_action_buttons() -> [(&'static str, Source); 4] {
        [
            ("b1", Source::button("South")),
            ("b2", Source::button("East")),
            ("b3", Source::button("West")),
            ("b4", Source::button("North")),
        ]
    }

    /// The same four, matched by **position**: bottom, left, top and right.
    fn action_buttons_by_position() -> [(&'static str, Source); 4] {
        [
            ("b1", Source::button("South")),
            ("b2", Source::button("West")),
            ("b3", Source::button("North")),
            ("b4", Source::button("East")),
        ]
    }

    /// The **button** sources of a name — without the keyboard ones and without the axis ones.
    ///
    /// It is what the action-button migration needs to look at: whoever has a controller also
    /// carries the keyboard keys, because [`Self::with_gamepad`] **adds** to [`Self::default`].
    fn button_sources(&self, name: &str) -> Vec<Source> {
        self.sources(name)
            .iter()
            .filter(|origin| matches!(origin, Source::Button { .. }))
            .cloned()
            .collect()
    }

    /// **The remapping by position reaches whoever already had the mapping saved.**
    ///
    /// Without this, only whoever deleted `settings.json` would see the fix: what is saved
    /// outranks the new default, and the old mapping would keep delivering east on `b2` — which is
    /// exactly the defect of issue #41.
    ///
    /// **The comparison is only between button sources, not of the whole list.** What a player
    /// with a controller has saved is `b2 = [Space, X, East]`: the keyboard keys plus the
    /// controller button. Comparing the whole list would never match what is saved, and the
    /// migration would never happen — which was the defect of the first version of this function.
    ///
    /// The switch only happens when all four are still **exactly** the old ones: whoever changed
    /// any of them keeps what they wrote. It is the same rule as
    /// [`Self::migrate_axis_convention`], and the keyboard keys stay where they are.
    fn migrate_action_buttons(&mut self) {
        let legacy = Self::legacy_action_buttons();
        let untouched = legacy
            .iter()
            .all(|(name, source)| self.button_sources(name) == [source.clone()]);
        if !untouched {
            return;
        }
        for (name, source) in Self::action_buttons_by_position() {
            let origins = self.buttons.entry(name.to_string()).or_default();
            origins.retain(|origin| !matches!(origin, Source::Button { .. }));
            origins.push(source);
        }
    }

    fn migrate_axis_convention(&mut self) {
        let mut upright = Self::default_axes();
        upright.get_mut("y").unwrap().invert = false;
        upright.get_mut("rz").unwrap().invert = false;
        if self.axes == upright {
            self.axes = Self::default_axes();
        }
    }

    /// A button's sources, or nothing if it has none.
    pub fn sources(&self, button: &str) -> &[Source] {
        self.buttons.get(button).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Binds `source` to `button`, without repeating what is already there.
    pub fn bind(&mut self, button: &str, source: Source) {
        let sources = self.buttons.entry(button.to_string()).or_default();
        if !sources.contains(&source) {
            sources.push(source);
        }
    }

    /// Unbinds everything from `button`.
    pub fn clear(&mut self, button: &str) {
        self.buttons.remove(button);
    }

    /// Builds the controller state: `active` says whether a source is triggered, and `analog`
    /// gives the travel of a host controller axis, from -1 to 1.
    pub fn pad(
        &self,
        active: impl Fn(&Source) -> bool,
        analog: impl Fn(&str) -> Option<f32>,
    ) -> Pad {
        let mut pad = Pad::default();
        for (button, sources) in &self.buttons {
            if !sources.iter().any(&active) {
                continue;
            }
            if let Some(index) = Pad::button_by_name(button) {
                pad.press(index, true);
            }
        }
        // The D-pad mirror, when on, runs **before** the analog loop. It writes zero at rest —
        // that is how the stick returns to center when the direction is released —, and the one
        // with the last word has to be the real stick, when it is outside the dead zone.
        if self.dpad_to_axes {
            pad.mirror_dpad_on_axes();
        }
        // The D-pad does **not** write to the axes on its own — see the note on [`Pad::press`].
        // This loop belongs to the analog stick, and it is what feeds `X`/`Y` and `Z`/`RZ`, as on
        // the device.
        for (index, name) in input::AXIS_NAMES.iter().enumerate() {
            let Some(source) = self.axes.get(*name) else {
                continue;
            };
            let Some(value) = analog(&source.name) else {
                continue;
            };
            let value = match source.invert {
                true => -value,
                false => value,
            };
            if value.abs() < DEADZONE {
                continue;
            }
            pad.set_axis(index, (value * input::AXIS_RANGE as f32) as i32);
        }
        pad
    }
}

/// The mappings of all players.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Controls {
    pub players: Vec<Player>,
}

impl Default for Controls {
    /// One port enabled and the others free.
    ///
    /// Enabling both by default would be worse than it seems: whoever plays with a single
    /// controller would see the keyboard and the controller fighting over the **same** port, or a
    /// two-player game seeing an idle second player.
    fn default() -> Self {
        let mut players = vec![Player::default()];
        while players.len() < crate::input::PORTS {
            players.push(Player {
                enabled: false,
                ..Player::default()
            });
        }
        Self { players }
    }
}

impl Controls {
    /// The mapping of player `index`, creating any that are missing.
    pub fn player_mut(&mut self, index: usize) -> &mut Player {
        while self.players.len() <= index {
            self.players.push(Player::default());
        }
        &mut self.players[index]
    }

    pub fn player(&self, index: usize) -> Option<&Player> {
        self.players.get(index)
    }

    /// The enabled ports, in order, as `(index, player)`.
    pub fn enabled_players(&self) -> impl Iterator<Item = (usize, &Player)> {
        self.players
            .iter()
            .enumerate()
            .take(crate::input::PORTS)
            .filter(|(_, player)| player.enabled)
    }

    /// Adjusts a mapping coming from an earlier version of the emulator.
    ///
    /// A file saved before ports existed has **one** player and no `ligada` field. What takes care
    /// of that is the struct's `serde(default)`, which fills a missing field from
    /// `Player::default()` — and there `enabled` is true. That way the controller of whoever
    /// already used the emulator stays enabled, with no patch needed here.
    ///
    /// All that is left to do is fill in the ports the old file did not have, and they come in
    /// **disabled**: nobody asked for a second player.
    pub fn adopt(&mut self) {
        for player in &mut self.players {
            player.adopt_axes();
            player.migrate_axis_convention();
            player.migrate_action_buttons();
            player.migrate_controller_device();
            player.migrate_wiimote_buttons();
        }
        while self.players.len() < crate::input::PORTS {
            self.players.push(Player {
                enabled: false,
                ..Player::default()
            });
        }
    }
}

impl Player {
    /// A port with a host controller selected is not a keyboard port.
    ///
    /// Choosing the controller in the list switched the mapping and left `device_kind` as it
    /// was, and `device_kind` is what the console **enumerates**: a port marked as keyboard does
    /// not enter the list of joysticks the games ask for. Whoever put the second controller on
    /// port two to play with two still had a single joystick as far as the game counted, and the
    /// two-player option stayed greyed out.
    ///
    /// It also applies to the Wii Remote, since it became a mappable controller like the others.
    fn migrate_controller_device(&mut self) {
        if self.device_kind == Device::Keyboard && self.device.is_some() {
            self.device_kind = Device::Controller;
        }
    }

    /// Whoever chose a Wii Remote before it was mappable keeps its buttons.
    ///
    /// The Wii Remote's buttons were added on their own to the port's keyboard ones, and the saved
    /// mapping had none. Without this, the update would leave the controller mute.
    fn migrate_wiimote_buttons(&mut self) {
        let wiimote = self
            .device
            .as_deref()
            .is_some_and(|device_name| crate::input::wiimote::Wiimotes::index_of_name(device_name).is_some());
        let has_wiimote_button = self.buttons.values().flatten().any(|source| {
            matches!(source, Source::Button { name } if name.starts_with("Wii"))
        });
        if wiimote && !has_wiimote_button {
            self.add_wiimote_buttons();
        }
    }
}

/// The Zeebo's buttons in the order that makes sense on a settings screen — the D-pad first,
/// then the actions, then the shoulders and the rest.
///
/// There is no Start nor a pair of shoulders: the console's controller has four action buttons,
/// one trigger on each side (ZL and ZR) and HOME, which plays the role of Start and shows up here
/// as `back` because `AEEUID_HIDJoystick_Back` is where the console's `hid_devices.cfg` maps it.
///
/// The leftover UIDs — Start and the two "lower" shoulders — remain in the table of
/// [`crate::input::BUTTON_UIDS`], which describes the device's HID descriptor and not its shell,
/// but there is no way to bind them to any key.
pub const CONFIGURABLE: [&str; 13] = [
    "up", "down", "left", "right", "b1", "b2", "b3", "b4", "zl", "zr", "lthumb", "rthumb", "back",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Switching the controller switches the mapping, not the port: it stays enabled, and a
    /// keyboard port that gets a controller becomes a controller to the console — otherwise the
    /// games do not enumerate it as the second player. A Boomerang stays a Boomerang.
    #[test]
    fn switching_the_controller_keeps_the_port() {
        let mut keyboard = Player {
            enabled: true,
            device_kind: Device::Keyboard,
            ..Player::default()
        };
        keyboard.switch_controller(Some("Xbox Controller".into()));
        assert!(keyboard.enabled);
        assert_eq!(keyboard.device_kind, Device::Controller);
        assert_eq!(keyboard.device.as_deref(), Some("Xbox Controller"));

        let mut boomerang = Player {
            enabled: true,
            device_kind: Device::Boomerang,
            ..Player::default()
        };
        boomerang.switch_controller(Some("Pro Controller".into()));
        assert_eq!(boomerang.device_kind, Device::Boomerang);

        boomerang.switch_controller(None);
        assert_eq!(boomerang.device, None, "without a controller, back to the plain keyboard");
        assert!(boomerang.enabled);
    }

    #[test]
    fn calibration_maps_rest_to_one_g_up() {
        let calibration = MotionCalibration::from_rest([0.02, -0.05, 1.14]).unwrap();
        let [x, y, z] = calibration.apply([0.02, -0.05, 1.14]);
        assert!(x.abs() < 0.01 && y.abs() < 0.01 && (z - 1.0).abs() < 0.01, "{x} {y} {z}");
    }

    /// Calibrated on its side, the offset would be 1 g: refused, and one saved that way is
    /// ignored.
    #[test]
    fn a_sideways_calibration_is_not_valid() {
        assert!(MotionCalibration::from_rest([-1.0, 0.0, 0.1]).is_none());
        let saved = MotionCalibration {
            zero_mg: [-1002, 5, -906],
            scale_mg: 1007,
        };
        assert_eq!(saved.apply([0.0, 0.0, 1.13]), [0.0, 0.0, 1.13]);
    }
    use crate::input::BUTTON_NAMES;

    /// The index of `up` on the console's controller, so the tests do not repeat the lookup.
    const DPAD_UP: usize = input::DPAD[0];

    #[test]
    fn the_configurable_buttons_exist_on_the_controller() {
        // The screen's list and the console's table have to agree: a name that does not exist
        // becomes a row that does nothing, and nobody finds out until they try to use it.
        for name in CONFIGURABLE {
            assert!(BUTTON_NAMES.contains(&name), "button {name} does not exist");
        }
    }

    #[test]
    fn the_default_covers_every_configurable_button() {
        let player = Player::default();
        for button in CONFIGURABLE {
            assert!(
                !player.sources(button).is_empty(),
                "button {button} was left without a key"
            );
        }
    }

    #[test]
    fn an_active_source_presses_the_button() {
        let player = Player::default();
        let pad = player.pad(|source| *source == Source::key("Z"), |_| None);
        assert!(pad.is_down(Pad::button_by_name("b1").unwrap()));
        assert!(!pad.is_down(Pad::button_by_name("b2").unwrap()));
    }

    #[test]
    fn two_keys_on_the_same_button_both_count() {
        // `Space` and `X` do the same thing, and that is what having more than one source means.
        let player = Player::default();
        let b2 = Pad::button_by_name("b2").unwrap();
        assert!(
            player
                .pad(|s| *s == Source::key("Space"), |_| None)
                .is_down(b2)
        );
        assert!(player.pad(|s| *s == Source::key("X"), |_| None).is_down(b2));
    }

    #[test]
    fn the_dpad_does_not_come_out_through_the_axes() {
        // It is a button, and only that: on the axes, a digital D-pad undoes its own step when
        // released. See the note on `Pad::press`. What feeds `X`/`Y` on the Z-Pad is the stick.
        let player = Player::default();
        let right = Pad::button_by_name("right").unwrap();
        let pad = player.pad(|s| *s == Source::key("ArrowRight"), |_| None);
        assert!(pad.is_down(right));
        assert_eq!(pad.axes, [0; 4]);
        let pad = player.pad(|s| *s == Source::key("ArrowUp"), |_| None);
        assert_eq!(pad.axes, [0; 4]);
        assert_eq!(player.pad(|_| false, |_| None).axes, [0; 4]);
    }

    #[test]
    fn binding_and_unbinding_a_source() {
        let mut player = Player::default();
        player.bind("b1", Source::button("South"));
        assert_eq!(player.sources("b1").len(), 2);
        // Binding again does not duplicate.
        player.bind("b1", Source::button("South"));
        assert_eq!(player.sources("b1").len(), 2);
        player.clear("b1");
        assert!(player.sources("b1").is_empty());
    }

    #[test]
    fn a_controller_mapping_keeps_the_keyboard() {
        // Connecting a controller must not take the keyboard away from whoever was using both.
        let player = Player::with_gamepad("Meu Controle".into());
        let b1 = Pad::button_by_name("b1").unwrap();
        assert!(player.pad(|s| *s == Source::key("Z"), |_| None).is_down(b1));
        assert!(
            player
                .pad(|s| *s == Source::button("South"), |_| None)
                .is_down(b1)
        );
        assert_eq!(player.device.as_deref(), Some("Meu Controle"));
    }

    #[test]
    fn the_analog_writes_the_axis_with_full_travel() {
        // The stick is not a D-pad under another name: the game needs to see half travel as half
        // travel, and not as "pushed all the way".
        let player = Player::with_gamepad("Meu Controle".into());
        let pad = player.pad(
            |_| false,
            |axis| match axis {
                "LeftStickX" => Some(0.5),
                _ => None,
            },
        );
        assert_eq!(pad.axes[0], input::AXIS_RANGE / 2);
        assert_eq!(pad.axes[1], 0);
    }

    /// **The saved mapping of whoever already played follows the fix for #41** — and only that
    /// one: a mapping changed by hand stays as it is.
    #[test]
    fn the_saved_action_button_mapping_is_remapped_by_position() {
        // **The real saved state**, not a made-up one: whoever has a controller carries the
        // keyboard keys along (`with_gamepad` adds to `default`), so the file says
        // `b2 = [Space, X, East]`. The first version of this migration compared the whole list
        // and so would never match — it would never migrate anyone.
        let mut old = Player::with_gamepad("Controle".into());
        for (name, source) in Player::legacy_action_buttons() {
            let origins = old.buttons.entry(name.to_string()).or_default();
            origins.retain(|origin| !matches!(origin, Source::Button { .. }));
            origins.push(source);
        }
        assert_eq!(old.button_sources("b2"), [Source::button("East")], "the starting point");

        old.migrate_action_buttons();
        assert_eq!(old.button_sources("b2"), [Source::button("West")], "east leaves b2");
        assert_eq!(old.button_sources("b3"), [Source::button("North")]);
        assert_eq!(old.button_sources("b4"), [Source::button("East")]);
        assert_eq!(old.button_sources("b1"), [Source::button("South")]);
        assert!(
            old.sources("b2").contains(&Source::key("Space")),
            "the keyboard keys must not vanish in the migration"
        );

        // Whoever changed one of them keeps what they wrote: nothing is swapped underneath.
        let mut tweaked = Player::with_gamepad("Controle".into());
        tweaked.bind("b2", Source::button("North"));
        let before = tweaked.buttons.clone();
        tweaked.migrate_action_buttons();
        assert_eq!(
            tweaked.buttons, before,
            "a mapping changed by hand must not be swapped underneath"
        );

        // And a mapping that starts out right is not changed twice. The keyboard stays alongside
        // the controller's sources, so what is checked is the button source, not the whole list.
        let mut create = Player::with_gamepad("Controle".into());
        create.migrate_action_buttons();
        assert!(create.sources("b2").contains(&Source::button("West")));
        assert!(!create.sources("b2").contains(&Source::button("East")));
        assert!(create.sources("b4").contains(&Source::button("East")));
    }


    /// The request of issue #39, on the standalone side: it is the setting the user turns on in
    /// the screen.
    #[test]
    fn dpad_on_axes_is_opt_in_and_does_not_disable_the_stick() {
        let up = |source: &Source| *source == Source::key("ArrowUp");
        let mut player = Player::default();

        // Without the option, what was already true still holds: the D-pad only presses buttons.
        let pad = player.pad(up, |_| None);
        assert_eq!(pad.axes[1], 0, "without the option, the axis does not move");
        assert!(pad.is_down(input::DPAD[0]), "and the button stays pressed");

        player.dpad_to_axes = true;
        let pad = player.pad(up, |_| None);
        assert_eq!(pad.axes[1], -input::AXIS_RANGE);
        assert!(
            pad.console_axis(1) < input::AXIS_CENTER,
            "up is the low value"
        );
        assert!(pad.is_down(input::DPAD[0]));

        // Releasing the direction returns the axis to center.
        let pad = player.pad(|_| false, |_| None);
        assert_eq!([pad.axes[0], pad.axes[1]], [0, 0]);

        // **The stick has the last word:** with the D-pad pressed and the analog stick outside the
        // dead zone, the analog stick is what controls the axis. That is why the mirror runs
        // before the loop — and this only shows with a controller, because the keyboard has no
        // axis source at all.
        let mut with_stick = Player::with_gamepad("Controle de teste".into());
        with_stick.dpad_to_axes = true;
        let pad = with_stick.pad(
            |source| *source == Source::button("DPadUp"),
            |axis| match axis {
                "LeftStickX" => Some(0.5),
                _ => None,
            },
        );
        assert_eq!(pad.axes[0], input::AXIS_RANGE / 2, "the stick won");
        assert_eq!(
            pad.axes[1],
            -input::AXIS_RANGE,
            "and the D-pad stayed on the other axis"
        );
    }

    #[test]
    fn up_on_the_analog_is_the_low_value_on_the_console() {
        // The controller library says up is positive; HID, that the `Y` axis grows downward.
        // Getting this sign wrong inverts the vertical axis of every game that reads it.
        let player = Player::with_gamepad("Meu Controle".into());
        let pad = player.pad(
            |_| false,
            |axis| match axis {
                "LeftStickY" => Some(1.0),
                _ => None,
            },
        );
        assert!(pad.axes[1] < 0, "vertical axis gave {}", pad.axes[1]);
        // And it is the low value that reaches the console, which is what the game reads as "up".
        assert!(pad.console_axis(1) < input::AXIS_CENTER);
    }

    #[test]
    fn the_right_stick_feeds_the_axes_the_dpad_does_not_use() {
        // `Z` and `RZ` exist on the console's controller and had nothing feeding them: the right
        // stick had no function at all.
        let player = Player::with_gamepad("Meu Controle".into());
        let pad = player.pad(
            |_| false,
            |axis| match axis {
                "RightStickX" => Some(-1.0),
                _ => None,
            },
        );
        assert_eq!(pad.axes[2], -input::AXIS_RANGE);
        assert_eq!(pad.axes[3], 0);
    }

    #[test]
    fn an_analog_at_rest_leaves_the_axis_centered() {
        // The dead zone exists so that a stick that does not return exactly to the center does
        // not leave the axis jittering, and the game does not see the controller moving on its own.
        let player = Player::with_gamepad("Meu Controle".into());
        let pad = player.pad(|_| false, |_| Some(0.05));
        assert_eq!(pad.axes[0], 0);
    }

    #[test]
    fn without_a_controller_the_axes_stay_centered() {
        // The keyboard has no analog stick, and the keyboard mapping maps no axis at all: not even
        // the value returned by the analog stick reaches the axes, because no source is bound to
        // them.
        let player = Player::default();
        assert!(player.axes.is_empty());
        let pad = player.pad(|s| *s == Source::key("Enter"), |_| Some(1.0));
        assert_eq!(pad.axes, [0; 4]);
    }

    #[test]
    fn the_left_stick_does_not_press_the_dpad() {
        // The stick and the cross D-pad are different controls: also binding the stick to the
        // buttons would make the two act together and neither on its own.
        let player = Player::with_gamepad("Meu Controle".into());
        for button in ["up", "down", "left", "right"] {
            for source in player.sources(button) {
                assert!(
                    !matches!(source, Source::Axis { .. }),
                    "{button} still comes from an axis"
                );
            }
        }
        let stick = Source::Axis {
            name: "LeftStickY".into(),
            positive: true,
        };
        assert!(!player.pad(|s| *s == stick, |_| None).is_down(DPAD_UP));
    }

    #[test]
    fn adopting_the_axes_takes_the_stick_off_the_dpad() {
        // The old mapping bound the stick to the D-pad buttons for lack of axes. Keeping both
        // would make the stick press the D-pad on top of moving the axis.
        let mut player = Player {
            device: Some("Meu Controle".into()),
            ..Player::default()
        };
        player.bind(
            "up",
            Source::Axis {
                name: "LeftStickY".into(),
                positive: true,
            },
        );
        let mut controls = Controls {
            players: vec![player],
        };
        controls.adopt();
        let up = controls.players[0].sources("up");
        assert!(up.contains(&Source::key("ArrowUp")), "the key stays");
        assert!(!up.iter().any(|s| matches!(s, Source::Axis { .. })));
    }

    #[test]
    fn a_mapping_without_axes_gets_the_controllers() {
        // A file written before the axes existed would leave the sticks mute, and nobody would
        // guess the mapping had to be redone by hand.
        let mut controls = Controls {
            players: vec![Player {
                device: Some("Meu Controle".into()),
                ..Player::default()
            }],
        };
        controls.adopt();
        assert_eq!(controls.players[0].axes, Player::default_axes());

        // Whoever is on the keyboard only still has no axes at all: there is no stick to map.
        let mut keyboard = Controls::default();
        keyboard.adopt();
        assert!(keyboard.players[0].axes.is_empty());
    }

    #[test]
    fn a_missing_player_is_created_with_the_default() {
        let mut controls = Controls::default();
        assert_eq!(controls.players.len(), crate::input::PORTS);
        controls.player_mut(3);
        assert_eq!(controls.players.len(), 4);
        assert_eq!(controls.player(3), Some(&Player::default()));
    }

    #[test]
    fn a_port_with_a_chosen_controller_stops_being_keyboard() {
        // `device_kind` is what the console enumerates: with a keyboard there, the port does not
        // count as a joystick and the two-player game does not see the second one.
        let mut controls = Controls::default();
        controls.players[1].enabled = true;
        controls.players[1].device_kind = Device::Keyboard;
        controls.players[1].device = Some("Meu Controle #2".into());
        controls.adopt();
        assert_eq!(controls.players[1].device_kind, Device::Controller);
        // With no controller selected, a keyboard stays a keyboard.
        let mut keyboard_only = Controls::default();
        keyboard_only.players[0].device_kind = Device::Keyboard;
        keyboard_only.players[0].device = None;
        keyboard_only.adopt();
        assert_eq!(keyboard_only.players[0].device_kind, Device::Keyboard);
    }

    /// Both ports start with the same mapping, and only the first one enabled. Enabling both by
    /// default would make a two-player game see a second player nobody asked for.
    #[test]
    fn only_the_first_port_starts_enabled() {
        let controls = Controls::default();
        let enabled_players: Vec<usize> = controls.enabled_players().map(|(i, _)| i).collect();
        assert_eq!(enabled_players, vec![0]);
        assert_eq!(controls.players[1].device_kind, Device::Controller);
    }

    /// A file saved before ports existed has no `ligada` field, and its player **must not** show
    /// up disabled: whoever already used the emulator would be left with no controller at all.
    #[test]
    fn a_legacy_mapping_stays_bound() {
        let old = r#"{"players":[{"device":null}]}"#;
        let mut controls: Controls = serde_json::from_str(old).expect("should read");
        controls.adopt();
        assert!(controls.players[0].enabled);
        assert_eq!(controls.players.len(), crate::input::PORTS);
        assert!(!controls.players[1].enabled);
    }

    #[test]
    fn the_wii_remote_is_a_mappable_controller() {
        // As a Z-Pad it has to respond through its own buttons, in the port's mapping, and the
        // port stops being a keyboard to the console.
        let mut controls = Controls {
            players: vec![Player {
                device_kind: Device::Keyboard,
                ..Player::with_wiimote("Wii Remote 1".into())
            }],
        };
        controls.adopt();
        let player = &controls.players[0];
        assert_eq!(player.device_kind, Device::Controller);
        assert!(player.buttons["b1"].contains(&Source::button("Wii1")));
        assert!(player.buttons["back"].contains(&Source::button("WiiHome")));
        // The keyboard still works alongside it.
        assert!(player.buttons["b1"].contains(&Source::key("Z")));
    }

    #[test]
    fn whoever_already_had_a_wii_remote_keeps_its_buttons() {
        // Before, the buttons were added on their own and the saved mapping only had the keyboard.
        let mut controls = Controls {
            players: vec![Player {
                device: Some("Wii Remote 1".into()),
                ..Player::default()
            }],
        };
        controls.adopt();
        assert!(controls.players[0].buttons["up"].contains(&Source::button("WiiUp")));
        // And whoever already mapped by hand gets nothing extra.
        let mut mapped = Player {
            device: Some("Wii Remote 1".into()),
            ..Player::default()
        };
        mapped.bind("b1", Source::button("WiiB"));
        let mut controls = Controls { players: vec![mapped] };
        controls.adopt();
        assert!(!controls.players[0].buttons["up"].contains(&Source::button("WiiUp")));
    }
}
