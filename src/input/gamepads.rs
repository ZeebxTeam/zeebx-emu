//! The real controllers connected to the computer.
//!
//! This module is the bridge between the names [`crate::input::bindings`] stores and what the
//! controller library understands. The names are stored as text precisely so the settings file
//! does not depend on any library's internal numbering.

use gilrs::{Axis, Button, Gilrs};

use crate::input::bindings::Source;

/// From how far an analog axis counts as triggered. Half travel is what separates "the player
/// pushed it" from "the stick did not return exactly to the center".
const AXIS_THRESHOLD: f32 = 0.5;

/// The buttons we know how to name, with the name that goes into the settings file.
const BUTTONS: [(&str, Button); 19] = [
    ("South", Button::South),
    ("East", Button::East),
    ("North", Button::North),
    ("West", Button::West),
    ("C", Button::C),
    ("Z", Button::Z),
    ("LeftTrigger", Button::LeftTrigger),
    ("LeftTrigger2", Button::LeftTrigger2),
    ("RightTrigger", Button::RightTrigger),
    ("RightTrigger2", Button::RightTrigger2),
    ("Select", Button::Select),
    ("Start", Button::Start),
    ("Mode", Button::Mode),
    ("LeftThumb", Button::LeftThumb),
    ("RightThumb", Button::RightThumb),
    ("DPadUp", Button::DPadUp),
    ("DPadDown", Button::DPadDown),
    ("DPadLeft", Button::DPadLeft),
    ("DPadRight", Button::DPadRight),
];

const AXES: [(&str, Axis); 4] = [
    ("LeftStickX", Axis::LeftStickX),
    ("LeftStickY", Axis::LeftStickY),
    ("RightStickX", Axis::RightStickX),
    ("RightStickY", Axis::RightStickY),
];

/// The axis names the settings screen offers.
pub fn axis_names() -> impl Iterator<Item = &'static str> {
    AXES.iter().map(|(name, _)| *name)
}

pub fn button_by_name(name: &str) -> Option<Button> {
    BUTTONS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, button)| *button)
}

pub fn axis_by_name(name: &str) -> Option<Axis> {
    AXES.iter()
        .find(|(known, _)| *known == name)
        .map(|(_, axis)| *axis)
}

/// How often, on Linux, we check whether the input devices changed. See
/// [`Gamepads::check_devices`].
#[cfg(target_os = "linux")]
const DEVICE_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// The connected controllers, and their state.
///
/// A computer without any controller — or without permission to read them — cannot keep the
/// emulator from opening, so the failure becomes "no controllers" and the keyboard keeps working.
pub struct Gamepads {
    gilrs: Option<Gilrs>,
    /// The controllers whose first event already went to the log. See [`Gamepads::note`].
    seen: std::collections::HashSet<gilrs::GamepadId>,
    /// When the last event from any controller arrived: the controls tab uses it to say that
    /// nothing is arriving.
    last_event: Option<std::time::Instant>,
    /// When `gilrs` last said that a controller connected.
    last_connection: Option<std::time::Instant>,
    /// What the last check of `/dev/input` saw. See [`Gamepads::check_devices`].
    #[cfg(target_os = "linux")]
    devices: Option<DeviceCheck>,
}

/// A check of the Linux input nodes.
#[cfg(target_os = "linux")]
struct DeviceCheck {
    when: std::time::Instant,
    /// The previous check: the `fresh` ones became openable between it and this one, and a
    /// `gilrs` connection since then is theirs.
    since: std::time::Instant,
    /// The nodes that could be opened.
    openable: std::collections::BTreeSet<InputNode>,
    /// The ones that became openable in this check: if `gilrs` connects no controller by the
    /// next one, it did not see them.
    fresh: std::collections::BTreeSet<InputNode>,
}

/// An `event*` node of `/dev/input`: the name and the inode, so a node recreated with the same
/// name counts as a different one.
#[cfg(target_os = "linux")]
type InputNode = (String, u64);

impl Default for Gamepads {
    fn default() -> Self {
        Self::new()
    }
}

impl Gamepads {
    pub fn new() -> Self {
        Self {
            gilrs: open_gilrs(),
            seen: std::collections::HashSet::new(),
            last_event: None,
            last_connection: None,
            #[cfg(target_os = "linux")]
            devices: None,
        }
    }

    /// Consumes the pending events. It must be called every frame: that is what keeps the button
    /// state up to date and detects a controller connected midway.
    pub fn poll(&mut self) {
        if let Some(gilrs) = &mut self.gilrs {
            let mut events = Vec::new();
            while let Some(event) = gilrs.next_event() {
                events.push(event);
            }
            for event in events {
                self.note(&event);
            }
        }
        // After the events: a controller `gilrs` has just connected is already noted.
        #[cfg(target_os = "linux")]
        self.check_devices();
    }

    /// When the last event from some controller arrived — a button, an axis, a connection.
    pub fn last_event(&self) -> Option<std::time::Instant> {
        self.last_event
    }

    /// Writes to the log what is needed to understand a controller that does not respond.
    ///
    /// **It is the diagnostic for the PS4 controller on the Mac (issue #59)**: the capture waited
    /// forever and there was no way to know whether the controller did not even reach the
    /// emulator or reached it without names. Each connection gives the system name, the VID/PID
    /// and whether there is a mapping; the first event of each controller gives the raw code —
    /// including for a button the mapping does not name.
    fn note(&mut self, event: &gilrs::Event) {
        use gilrs::EventType;
        self.last_event = Some(std::time::Instant::now());
        let Some(gilrs) = &self.gilrs else {
            return;
        };
        let pad = gilrs.gamepad(event.id);
        match event.event {
            EventType::Connected => {
                self.seen.remove(&event.id);
                self.last_connection = Some(std::time::Instant::now());
                crate::logging!(
                    crate::logging::Level::Info,
                    "controles",
                    "connected: {}",
                    describe(&pad)
                );
            }
            EventType::Disconnected => {
                crate::logging!(
                    crate::logging::Level::Info,
                    "controles",
                    "disconnected: {}",
                    pad.name()
                );
            }
            EventType::ButtonPressed(button, code) | EventType::ButtonChanged(button, _, code)
                if self.seen.insert(event.id) =>
            {
                crate::logging!(
                    crate::logging::Level::Info,
                    "controles",
                    "first button of {}: {button:?} (code {code})",
                    pad.name()
                );
            }
            EventType::AxisChanged(axis, value, code)
                if value.abs() >= AXIS_THRESHOLD && self.seen.insert(event.id) =>
            {
                crate::logging!(
                    crate::logging::Level::Info,
                    "controles",
                    "first axis of {}: {axis:?} at {value:.2} (code {code})",
                    pad.name()
                );
            }
            _ => {}
        }
    }

    /// Recreates `gilrs` when a controller was granted access after connecting, and it did not
    /// see it.
    ///
    /// **A controller reconnected with the emulator open did not come back.** `gilrs` only tries
    /// to open a device on udev's `add`. With `joycond` — common on Arch and derivatives —, the
    /// node of a Nintendo controller is born `crw-------`, and only a little over a second later,
    /// in a `change`, does it get read permission. `gilrs` gets `EACCES` on the `add`, ignores
    /// the `change`, and the controller only came back by restarting the emulator. Measured with
    /// a Pro Controller on 2026-10-11 — see `docs/implementation/09-input.md`.
    ///
    /// Once per second, the check sees which `event*` nodes the process can open — through
    /// `access`, which respects the ACLs udev gives to whoever is sitting at the machine. A node
    /// that becomes openable is noted; if by the next check `gilrs` has connected no controller at
    /// all, it did not see that node, and it is created again: its initial scan, the one that
    /// always worked, finds the controller that was granted access.
    ///
    /// **Only in that case, and not on every change.** Recreating reorders the list: a port with
    /// no controller selected takes the one at its position, and two players would swap
    /// controllers. An ordinary controller `gilrs` connects on its own on the `add`, and then the
    /// list stays as it is.
    #[cfg(target_os = "linux")]
    fn check_devices(&mut self) {
        let check_time = std::time::Instant::now();
        if self
            .devices
            .as_ref()
            .is_some_and(|before| check_time - before.when < DEVICE_CHECK_INTERVAL)
        {
            return;
        }
        let openable = openable_nodes(std::path::Path::new("/dev/input"));
        let Some(before) = self.devices.take() else {
            self.devices = Some(DeviceCheck {
                when: check_time,
                since: check_time,
                openable,
                fresh: Default::default(),
            });
            return;
        };
        let connected_since = self.last_connection.is_some_and(|when| when >= before.since);
        let lost: Vec<&InputNode> =
            before.fresh.iter().filter(|node| openable.contains(*node)).collect();
        if !lost.is_empty() && !connected_since {
            crate::logging!(
                crate::logging::Level::Info,
                "controles",
                "{} became openable after connecting, and the controller reader did not see \
                 it: it is recreated",
                lost.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", ")
            );
            self.gilrs = open_gilrs();
            self.seen.clear();
        }
        let fresh = openable.difference(&before.openable).cloned().collect();
        self.devices = Some(DeviceCheck {
            when: check_time,
            since: before.when,
            openable,
            fresh,
        });
    }

    /// The name of each connected controller, in the order the system lists them.
    ///
    /// **Two controllers of the same model have the same name**, and whoever picks "the second
    /// one" in the list needs something to tell it from the first. From the second appearance on,
    /// the name gets a ` #2`, ` #3` and so on. While the name was only the model's, selecting the
    /// second controller on port 2 stored the same string as port 1 — and the preview, which looks
    /// up the controller by name, always found the first one: port 2 responded to controller 1.
    ///
    /// It is still a name, not an index: the system's index changes when someone disconnects a
    /// controller, and the settings saved yesterday have to hold today.
    pub fn names(&self) -> Vec<String> {
        let Some(gilrs) = &self.gilrs else {
            return Vec::new();
        };
        number_duplicates(gilrs.gamepads().map(|(_, pad)| pad.name().to_string()))
    }

    /// The controller named `device`; with no name, the one whose turn it is for the port.
    ///
    /// The name looked up is the one in the [`Self::names`] list, with the numbering of repeats.
    ///
    /// **With no name selected, the port takes the controller at its position**: the first for
    /// port one, the second for port two. While every port with no selection took the first
    /// controller, enabling port two just to play with two duplicated controller one on both — and
    /// the game that asks "player 2, press a button" never saw a real second player.
    fn find(&self, device: Option<&str>, port: usize) -> Option<gilrs::Gamepad<'_>> {
        let gilrs = self.gilrs.as_ref()?;
        match device {
            Some(name) => {
                let which = self.names().iter().position(|n| n == name)?;
                gilrs.gamepads().nth(which)
            }
            None => gilrs.gamepads().nth(port),
        }
        .map(|(_, pad)| pad)
    }

    /// Who the port's controller is, to find its motion sensor: the name the system gives it,
    /// the VID/PID pair and its position among the identical controllers connected before it.
    ///
    /// It is the **system's** name, not the one from gilrs's mapping: the sensor gets the
    /// device's name with a suffix ("Pro Controller" and "Pro Controller (IMU)"), and gilrs calls
    /// the same controller "Nintendo Switch Pro Controller".
    pub fn identity(&self, device: Option<&str>, port: usize) -> Option<(String, u16, u16, usize)> {
        let gilrs = self.gilrs.as_ref()?;
        let pad = self.find(device, port)?;
        let key_id = |p: &gilrs::Gamepad<'_>| (p.os_name().to_string(), p.vendor_id(), p.product_id());
        let (name, vendor, product) = key_id(&pad);
        let order = gilrs
            .gamepads()
            .take_while(|(id, _)| *id != pad.id())
            .filter(|(_, other)| key_id(other) == (name.clone(), vendor, product))
            .count();
        Some((name, vendor?, product?, order))
    }

    /// Whether the source is triggered on the player's controller.
    ///
    /// Keyboard sources do not belong here: the window is what knows about the keyboard.
    pub fn is_active(&self, device: Option<&str>, port: usize, source: &Source) -> bool {
        let Some(pad) = self.find(device, port) else {
            return false;
        };
        is_triggered(&pad, source)
    }

    /// Whether `source` is triggered on **any** connected controller.
    ///
    /// It is the question the window's shortcuts ask, like fast-forward: they belong to the
    /// session, not to a port, and whoever plays on controller two fast-forwards with controller
    /// two's button.
    pub fn any_active(&self, source: &Source) -> bool {
        let Some(gilrs) = &self.gilrs else {
            return false;
        };
        gilrs.gamepads().any(|(_, pad)| is_triggered(&pad, source))
    }

    /// The first button pressed on any connected controller, to capture a shortcut. Only
    /// buttons: a shortcut on an axis would fire with the stick off center.
    pub fn first_button_from_any(&self) -> Option<Source> {
        let gilrs = self.gilrs.as_ref()?;
        gilrs.gamepads().find_map(|(_, pad)| {
            BUTTONS
                .iter()
                .find(|(_, button)| pad.is_pressed(*button))
                .map(|(name, _)| Source::button(name))
        })
    }

    /// The travel of a controller axis, from -1 to 1. `None` if there is no controller or the
    /// axis is unknown — and then the mapping of that axis simply does not apply.
    pub fn value(&self, device: Option<&str>, port: usize, axis: &str) -> Option<f32> {
        let pad = self.find(device, port)?;
        Some(pad.value(axis_by_name(axis)?))
    }

    /// The first source triggered right now, for the settings screen to capture.
    ///
    /// Buttons come before axes: whoever presses the D-pad of a controller that also reports it
    /// as an axis wants the button, which is the more specific one.
    pub fn first_active(&self, device: Option<&str>, port: usize) -> Option<Source> {
        let pad = self.find(device, port)?;
        for (name, button) in BUTTONS {
            if pad.is_pressed(button) {
                return Some(Source::button(name));
            }
        }
        for (name, axis) in AXES {
            let value = pad.value(axis);
            if value.abs() >= AXIS_THRESHOLD {
                return Some(Source::Axis {
                    name: name.to_string(),
                    positive: value > 0.0,
                });
            }
        }
        None
    }
}

/// Opens `gilrs`, and says in the log what it found — or why it did not open.
fn open_gilrs() -> Option<Gilrs> {
    match Gilrs::new() {
        Ok(gilrs) => {
            let list: Vec<String> = gilrs.gamepads().map(|(_, pad)| describe(&pad)).collect();
            crate::logging!(
                crate::logging::Level::Info,
                "controles",
                "{} controller(s): {}",
                list.len(),
                match list.is_empty() {
                    true => "none".to_string(),
                    false => list.join("; "),
                }
            );
            Some(gilrs)
        }
        Err(error) => {
            crate::logging!(
                crate::logging::Level::Warning,
                "controles",
                "the controller reader did not open: {error}"
            );
            None
        }
    }
}

/// A controller as the log describes it: the mapping's name, the system's, the VID/PID and
/// whether there is an SDL mapping for it. Without a mapping, `gilrs` names the buttons by their
/// raw position.
fn describe(pad: &gilrs::Gamepad<'_>) -> String {
    let hex = |id: Option<u16>| id.map_or("?".to_string(), |id| format!("{id:04x}"));
    format!(
        "\"{}\" (system: \"{}\", {}:{}, mapping {:?})",
        pad.name(),
        pad.os_name(),
        hex(pad.vendor_id()),
        hex(pad.product_id()),
        pad.mapping_source()
    )
}

/// The `event*` nodes of `dir` this process can open for reading and writing, the way `gilrs`
/// opens them. See [`Gamepads::check_devices`].
#[cfg(target_os = "linux")]
fn openable_nodes(dir: &std::path::Path) -> std::collections::BTreeSet<InputNode> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("event") {
                return None;
            }
            let node_path = std::ffi::CString::new(entry.path().as_os_str().as_bytes()).ok()?;
            // SAFETY: `node_path` is a valid C string, alive during the call.
            let open = unsafe { libc::access(node_path.as_ptr(), libc::R_OK | libc::W_OK) } == 0;
            let inode = entry.metadata().ok()?.ino();
            open.then_some((name, inode))
        })
        .collect()
}

/// Whether the source is triggered on a controller. The keyboard does not belong here: it
/// answers no.
fn is_triggered(pad: &gilrs::Gamepad<'_>, source: &Source) -> bool {
    match source {
        Source::Key { .. } => false,
        Source::Button { name } => {
            button_by_name(name).is_some_and(|button| pad.is_pressed(button))
        }
        Source::Axis { name, positive } => axis_by_name(name).is_some_and(|axis| {
            let value = pad.value(axis);
            match positive {
                true => value >= AXIS_THRESHOLD,
                false => value <= -AXIS_THRESHOLD,
            }
        }),
    }
}

/// Tells repeated names apart by appending ` #2`, ` #3` and so on, in order of arrival.
fn number_duplicates(names: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    names
        .map(|name| {
            let count = seen.entry(name.clone()).or_insert(0);
            *count += 1;
            match *count {
                1 => name,
                n => format!("{name} #{n}"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_controllers_of_the_same_model_get_different_names() {
        // Without this, picking "the second one" in the list stores the same string as the first
        // and port 2 ends up responding to port 1's controller.
        let names = ["Z-Pad", "Z-Pad", "Wii Remote", "Z-Pad"];
        assert_eq!(
            number_duplicates(names.iter().map(|n| n.to_string())),
            ["Z-Pad", "Z-Pad #2", "Wii Remote", "Z-Pad #3"]
        );
    }

    #[test]
    fn button_names_round_trip() {
        // The settings file stores the name, not the library's number: a version change must not
        // remap the controller of whoever already configured it.
        for (name, button) in BUTTONS {
            assert_eq!(button_by_name(name), Some(button), "button {name}");
        }
        assert_eq!(button_by_name("NaoExiste"), None);
    }

    #[test]
    fn axis_names_round_trip() {
        for (name, axis) in AXES {
            assert_eq!(axis_by_name(name), Some(axis), "axis {name}");
        }
        assert_eq!(axis_by_name("NaoExiste"), None);
    }

    #[test]
    fn the_default_controller_mapping_only_uses_known_names() {
        // A name in the default mapping that the library does not know becomes a button that
        // never works, and nobody finds out until they connect a controller.
        let player = crate::input::bindings::Player::with_gamepad("qualquer".into());
        for sources in player.buttons.values() {
            for source in sources {
                match source {
                    Source::Key { .. } => {}
                    Source::Button { name } => {
                        assert!(button_by_name(name).is_some(), "button {name}")
                    }
                    Source::Axis { name, .. } => {
                        assert!(axis_by_name(name).is_some(), "axis {name}")
                    }
                }
            }
        }
    }

    /// A node counts when it becomes openable — which `joycond` does a second after the
    /// controller connects —, and only the `event*` ones count.
    #[cfg(target_os = "linux")]
    #[test]
    fn openable_nodes_see_the_granted_permission() {
        use std::os::unix::fs::PermissionsExt;
        // As root, `access` says everything opens, and there is nothing to measure.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = std::env::temp_dir().join(format!("zeebx-aparelhos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let node = dir.join("event18");
        std::fs::write(&node, b"").unwrap();
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o000)).unwrap();
        std::fs::write(dir.join("js0"), b"").unwrap();
        assert!(openable_nodes(&dir).is_empty(), "the closed node counted, or js0 got in");

        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o666)).unwrap();
        let opened: Vec<String> = openable_nodes(&dir).into_iter().map(|(name, _)| name).collect();
        assert_eq!(opened, ["event18"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_controller_nothing_is_active() {
        // A computer without a controller cannot keep the emulator from opening.
        let pads = Gamepads {
            gilrs: None,
            seen: Default::default(),
            last_event: None,
            last_connection: None,
            #[cfg(target_os = "linux")]
            devices: None,
        };
        assert!(pads.names().is_empty());
        assert!(!pads.is_active(None, 0, &Source::button("South")));
        assert_eq!(pads.first_active(None, 0), None);
    }
}
