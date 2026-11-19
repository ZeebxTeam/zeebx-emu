//! The Wii Remote, read directly from Linux input devices.
//!
//! The kernel's `hid-wiimote` driver splits each controller into several devices: the buttons in
//! "Nintendo Wii Remote" and the accelerometer in "Nintendo Wii Remote Accelerometer", which only
//! starts sending data once it is opened. gilrs sees, at most, the buttons; the accelerometer is
//! an axis of another device, so reading it is up to us.
//!
//! Each device has a thread that reads the raw events (`struct input_event`) and updates a shared
//! state. A search thread looks for new controllers every second, and the reader of a controller
//! that disconnected simply ends with an error.
//!
//! Outside Linux there is no reading: [`Wiimotes::state`] returns `None`.

use std::sync::{Arc, Mutex};

use crate::input::bindings::Source;

/// What a Wii Remote is doing right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WiimoteState {
    /// One bit per button, in the order of [`BUTTONS`].
    pub buttons: u16,
    /// The acceleration in g, in the controller's frame of reference: `x` to the right, `y`
    /// forward (the infrared end) and `z` up, out of the button face. At rest with the face up,
    /// `z` is `+1`.
    pub acceleration: [f32; 3],
    /// Whether the accelerometer has sent any reading yet.
    pub with_accelerometer: bool,
}

impl Default for WiimoteState {
    fn default() -> Self {
        Self {
            buttons: 0,
            acceleration: [0.0, 0.0, 1.0],
            with_accelerometer: false,
        }
    }
}

/// The Wii Remote's buttons, with the key code `hid-wiimote` uses for each one.
pub const BUTTONS: [(&str, u16); 11] = [
    ("up", 103),     // KEY_UP
    ("down", 108),   // KEY_DOWN
    ("left", 105),   // KEY_LEFT
    ("right", 106),  // KEY_RIGHT
    ("a", 0x130),    // BTN_A
    ("b", 0x131),    // BTN_B
    ("1", 0x101),    // BTN_1
    ("2", 0x102),    // BTN_2
    ("plus", 407),   // KEY_NEXT
    ("minus", 412),  // KEY_PREVIOUS
    ("home", 0x13c), // BTN_MODE
];

/// How much `hid-wiimote` reports for 1 g. The accelerometer has 10 bits and the driver only
/// subtracts the center (`0x200`); gravity comes to about 100 units on every axis.
///
/// Only the Linux path reads directly from `hid-wiimote`; on other systems the accelerometer comes
/// from gilrs, and the constant would go unused — which already earned a dead-code warning on
/// macOS.
#[cfg(target_os = "linux")]
const UNITS_PER_G: f32 = 100.0;

/// The name of each button of [`BUTTONS`] in the mapping, in the same order.
///
/// The Wii Remote's buttons are sources just like gilrs's, and that is how it serves as a Z-Pad
/// with whatever mapping the player wants. The prefix separates them from gilrs's buttons, which
/// have names like `South` and `DPadUp`: a port with the Wii Remote selected reads these, and only
/// these.
pub const SOURCES: [&str; 11] = [
    "WiiUp", "WiiDown", "WiiLeft", "WiiRight", "WiiA", "WiiB", "Wii1", "Wii2", "WiiPlus",
    "WiiMinus", "WiiHome",
];

impl WiimoteState {
    pub fn pressed(&self, name: &str) -> bool {
        BUTTONS
            .iter()
            .position(|(n, _)| *n == name)
            .is_some_and(|i| self.buttons & (1 << i) != 0)
    }

    /// Whether the source is a button of this controller and is pressed.
    pub fn source_active(&self, source: &Source) -> bool {
        match source {
            Source::Button { name } => SOURCES
                .iter()
                .position(|n| n == name)
                .is_some_and(|i| self.buttons & (1 << i) != 0),
            _ => false,
        }
    }

    /// The first pressed button, for the settings screen to capture.
    pub fn first_source(&self) -> Option<Source> {
        (0..SOURCES.len())
            .find(|&i| self.buttons & (1 << i) != 0)
            .map(|i| Source::button(SOURCES[i]))
    }
}

/// The start of a Wii Remote's name in the controller list: "Wii Remote 1", "Wii Remote 2".
const PREFIX: &str = "Wii Remote ";

/// The controllers found, in the order they appeared.
#[derive(Clone, Default)]
pub struct Wiimotes {
    states: Arc<Mutex<Vec<(String, WiimoteState)>>>,
}

impl Wiimotes {
    /// Starts looking for controllers in the background.
    pub fn start() -> Self {
        let wiimotes = Self::default();
        #[cfg(target_os = "linux")]
        {
            let states = wiimotes.states.clone();
            let _ = std::thread::Builder::new()
                .name("wiimote-search".into())
                .spawn(move || linux::search(states));
        }
        wiimotes
    }

    /// How many controllers are connected.
    pub fn count(&self) -> usize {
        self.states.lock().map(|e| e.len()).unwrap_or(0)
    }

    /// The name the `index`-th controller appears under in the controller list.
    pub fn name(index: usize) -> String {
        format!("{PREFIX}{}", index + 1)
    }

    /// The index of a name given by [`Wiimotes::name`].
    pub fn index_of_name(name: &str) -> Option<usize> {
        name.strip_prefix(PREFIX)?.parse::<usize>().ok()?.checked_sub(1)
    }

    /// The state of the `index`-th connected controller.
    pub fn state(&self, index: usize) -> Option<WiimoteState> {
        self.states.lock().ok()?.get(index).map(|(_, state)| *state)
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::collections::HashSet;
    use std::io::Read;
    use std::path::{Path, PathBuf};

    const EV_KEY: u16 = 1;
    const EV_ABS: u16 = 3;
    const ABS_RX: u16 = 3;
    const ABS_RY: u16 = 4;
    const ABS_RZ: u16 = 5;
    /// `struct input_event` on 64 bits: `timeval` (16), `type` (2), `code` (2), `value` (4).
    const EVENT_SIZE: usize = 24;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Part {
        Buttons,
        Accelerometer,
    }

    /// Looks for controllers forever, opening a reader for each new device.
    pub(super) fn search(states: Arc<Mutex<Vec<(String, WiimoteState)>>>) {
        let opened: Arc<Mutex<HashSet<PathBuf>>> = Default::default();
        loop {
            for (event, controller, part) in devices() {
                let create = opened.lock().is_ok_and(|mut a| a.insert(event.clone()));
                if !create {
                    continue;
                }
                let (states, opened) = (states.clone(), opened.clone());
                let _ = std::thread::Builder::new()
                    .name("wiimote".into())
                    .spawn(move || {
                        read(&event, &controller, part, &states);
                        if let Ok(mut a) = opened.lock() {
                            a.remove(&event);
                        }
                        // The controller leaves the list when the buttons stop: that is the
                        // device that exists while it is paired.
                        if part == Part::Buttons
                            && let Ok(mut e) = states.lock()
                        {
                            e.retain(|(name, _)| *name != controller);
                        }
                    });
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    /// `(node in /dev/input, controller identifier, part)` of each Wii Remote device.
    ///
    /// The identifier is the parent HID device: the buttons and the accelerometer of the same
    /// controller share it, and that is how the two are joined.
    fn devices() -> Vec<(PathBuf, String, Part)> {
        let Ok(entries) = std::fs::read_dir("/sys/class/input") else {
            return Vec::new();
        };
        let mut found: Vec<_> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                if !name.starts_with("event") {
                    return None;
                }
                let base = entry.path().join("device");
                let label = std::fs::read_to_string(base.join("name")).ok()?;
                let part = match label.trim() {
                    "Nintendo Wii Remote" => Part::Buttons,
                    "Nintendo Wii Remote Accelerometer" => Part::Accelerometer,
                    _ => return None,
                };
                let controller = controller_of(&base)?;
                Some((Path::new("/dev/input").join(name), controller, part))
            })
            .collect();
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found
    }

    fn controller_of(device: &Path) -> Option<String> {
        let real = std::fs::canonicalize(device).ok()?;
        // `.../uhid/0005:057E:0306.0018/input/input46` -> `0005:057E:0306.0018`
        Some(real.parent()?.parent()?.file_name()?.to_str()?.to_string())
    }

    fn read(
        event: &Path,
        controller: &str,
        part: Part,
        states: &Mutex<Vec<(String, WiimoteState)>>,
    ) {
        let Ok(mut file) = std::fs::File::open(event) else {
            return;
        };
        // The controller joins the list when the buttons are opened, not on the first press: at
        // rest it sends no events at all, and the accelerometer would have nowhere to write.
        if part == Part::Buttons
            && let Ok(mut list) = states.lock()
            && !list.iter().any(|(name, _)| name == controller)
        {
            list.push((controller.to_string(), WiimoteState::default()));
        }
        let mut raw = [0u8; EVENT_SIZE];
        let mut acceleration = [0f32; 3];
        while file.read_exact(&mut raw).is_ok() {
            let kind = u16::from_ne_bytes([raw[16], raw[17]]);
            let code = u16::from_ne_bytes([raw[18], raw[19]]);
            let value = i32::from_ne_bytes([raw[20], raw[21], raw[22], raw[23]]);
            let Ok(mut list) = states.lock() else {
                return;
            };
            // An accelerometer without the buttons device is what remains of a controller that is
            // leaving.
            let Some(position) = list.iter().position(|(name, _)| name == controller) else {
                continue;
            };
            let state = &mut list[position].1;
            match (kind, part) {
                (EV_KEY, Part::Buttons) => {
                    if let Some(i) = BUTTONS.iter().position(|(_, c)| *c == code) {
                        match value {
                            0 => state.buttons &= !(1 << i),
                            _ => state.buttons |= 1 << i,
                        }
                    }
                }
                (EV_ABS, Part::Accelerometer) => {
                    let axis = match code {
                        ABS_RX => 0,
                        ABS_RY => 1,
                        ABS_RZ => 2,
                        _ => continue,
                    };
                    acceleration[axis] = value as f32 / UNITS_PER_G;
                    state.acceleration = acceleration;
                    state.with_accelerometer = true;
                }
                _ => {}
            }
        }
    }
}
