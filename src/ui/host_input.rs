//! Desktop input: the host's controllers, the Wii Remotes and the motion sensors, read through
//! each port's mapping.
//!
//! It came out of the egui `App` so the Qt window builds the controller and the Boomerang motion
//! the same way — see `docs/implementation/21-qt-migration.md`, phase 1. The keyboard is the
//! only part each window reads on its own: it hands over the set of pressed keys, as `egui::Key`,
//! and the mapping is checked against it.

use std::collections::HashSet;

use crate::input::bindings::{Device, Controls, Source};
use crate::input::gamepads::Gamepads;
use crate::input::sensors::{SensorState, Sensors};
use crate::input::wiimote::{WiimoteState, Wiimotes};
use crate::input::{PORTS, Pad};
use crate::ui::settings::Shortcuts;

/// Nintendo's VID, whose controllers have their length along Y. See
/// [`DesktopInput::port_motion`].
const VENDOR_NINTENDO: u16 = 0x057e;

/// The motion of a port without a sensor: still, face up.
pub const AT_REST: [f32; 3] = [0.0, 0.0, 1.0];

/// Where a port's Boomerang motion comes from. See [`DesktopInput::port_sensor`].
pub enum PortSensor {
    Wiimote(WiimoteState),
    Controller(SensorState),
    /// A chosen controller that has no motion sensor, by its name in the list.
    NoSensor(String),
    Off,
}

impl PortSensor {
    /// What to say about the sensor, on the controls screen and in the calibration notice.
    pub fn describe(&self, catalog: &crate::ui::i18n::Catalog) -> String {
        let (key_id, name) = match self {
            Self::Wiimote(w) if w.with_accelerometer => ("controls.boomerang.sensor", "Wii Remote"),
            Self::Wiimote(_) => ("controls.boomerang.sensor_waiting", "Wii Remote"),
            Self::Controller(s) if s.permission_denied => ("controls.boomerang.sensor_denied", s.name.as_str()),
            Self::Controller(s) if s.has_reading => ("controls.boomerang.sensor", s.name.as_str()),
            Self::Controller(s) => ("controls.boomerang.sensor_waiting", s.name.as_str()),
            Self::NoSensor(name) => ("controls.boomerang.no_sensor", name.as_str()),
            Self::Off => ("controls.boomerang.no_device", ""),
        };
        catalog.get(key_id).replace("{nome}", name)
    }

    /// The measured acceleration, once the sensor has sent any.
    pub fn acceleration(&self) -> Option<[f32; 3]> {
        match self {
            Self::Wiimote(wiimote) if wiimote.with_accelerometer => Some(wiimote.acceleration),
            Self::Controller(sensor) if sensor.has_reading => Some(sensor.acceleration),
            _ => None,
        }
    }
}

/// Whether a keyboard source of the mapping is pressed.
///
/// **The comparison is by key, never by text.** The same button shows up with two spellings in
/// `settings.json`: the factory mapping writes `ArrowUp` (`input/bindings.rs`), and a key captured
/// on the controls screen writes `egui::Key::name()`, which is `Up`. `from_name` accepts both. The
/// first version of the Qt window compared the text, and the keyboard dpad did not reach the
/// games — only the Z-Wheel, which navigates by BREW keys and not by the controller.
pub fn key_pressed(source: &Source, key_events: &HashSet<egui::Key>) -> bool {
    match source {
        Source::Key { name } => egui::Key::from_name(name).is_some_and(|key| key_events.contains(&key)),
        _ => false,
    }
}

pub struct DesktopInput {
    /// The real controllers connected to the computer.
    pub gamepads: Gamepads,
    /// The Wii Remotes: controllers mappable like the others, with their accelerometer.
    pub wiimotes: Wiimotes,
    /// The motion sensors of the other controllers, which feed the port's Boomerang.
    pub sensors: Sensors,
}

impl DesktopInput {
    pub fn start() -> Self {
        Self {
            gamepads: Gamepads::default(),
            wiimotes: Wiimotes::start(),
            sensors: Sensors::start(),
        }
    }

    /// The state of each port enabled now, built from its mapping.
    ///
    /// A disabled port does not go in: what comes out of here is the `(port, controller)` pairs of
    /// the enabled ones, and that is what the session hands to the game.
    ///
    /// What belongs to a shortcut — see [`Shortcuts::reserved`] — does not go in: the fast-forward
    /// key or button does not press anything in the game.
    pub fn pads(
        &mut self,
        controls: &Controls,
        shortcuts: &Shortcuts,
        key_events: &HashSet<egui::Key>,
    ) -> Vec<(usize, Pad)> {
        self.gamepads.poll();
        controls
            .enabled_players()
            .map(|(port, _)| (port, self.pad(controls, shortcuts, port, key_events)))
            .collect()
    }

    /// Whether each port's turbo key is pressed: the keyboard key, or the button on that port's
    /// controller. See [`crate::speed::turbo::TURBO_BUTTON`].
    pub fn turbos(&self, controls: &Controls, key_events: &HashSet<egui::Key>) -> [bool; PORTS] {
        std::array::from_fn(|port| {
            let Some(player) = controls.player(port).filter(|j| j.enabled) else {
                return false;
            };
            let device = player.device.as_deref();
            let wiimote = self.port_wiimote(controls, port);
            player
                .sources(crate::speed::turbo::TURBO_BUTTON)
                .iter()
                .any(|source| {
                    key_pressed(source, key_events)
                        || self.gamepads.is_active(device, port, source)
                        || wiimote.is_some_and(|w| w.source_active(source))
                })
        })
    }

    /// Whether any of the `sources` shortcuts is pressed now: the key, or the button on any
    /// connected controller. The caller already did this round's `poll`, in [`DesktopInput::pads`].
    pub fn shortcut_pressed(&self, sources: &[Source], key_events: &HashSet<egui::Key>) -> bool {
        sources
            .iter()
            .any(|source| key_pressed(source, key_events) || self.gamepads.any_active(source))
    }

    /// The state of a port's controller, built from its mapping.
    ///
    /// The keyboard and the controller are queried together: whoever has both can use both, and
    /// that is what having more than one source per button means.
    pub fn pad(
        &self,
        controls: &Controls,
        shortcuts: &Shortcuts,
        port: usize,
        key_events: &HashSet<egui::Key>,
    ) -> Pad {
        let Some(player) = controls.player(port) else {
            return Pad::default();
        };
        let device = player.device.as_deref();
        let gamepads = &self.gamepads;
        // The Wii Remote does not go through gilrs: its buttons are sources of their own (`WiiA`,
        // `Wii1`…), read from the state its thread keeps.
        let wiimote = self.port_wiimote(controls, port);
        let mut pad = player.pad(
            |source| {
                !shortcuts.reserved().any(|reserved| reserved == source)
                    && (key_pressed(source, key_events)
                        || gamepads.is_active(device, port, source)
                        || wiimote.is_some_and(|w| w.source_active(source)))
            },
            |axis| gamepads.value(device, port, axis),
        );
        // A Boomerang with no chosen controller takes the Wii Remote whose turn it is, and then
        // the port's mapping is the keyboard one, with none of its buttons: the buttons are added
        // through this table, which is the Wii Remote's default mapping.
        if let (None, Some(wiimote)) = (device, wiimote) {
            const WIIMOTE_MAP: [(&str, &str); 9] = [
                ("up", "up"),
                ("down", "down"),
                ("left", "left"),
                ("right", "right"),
                ("1", "b1"),
                ("a", "b1"),
                ("2", "b2"),
                ("b", "b2"),
                ("home", "back"),
            ];
            for (theirs, ours) in WIIMOTE_MAP {
                if wiimote.pressed(theirs)
                    && let Some(index) = Pad::button_by_name(ours)
                {
                    pad.press(index, true);
                }
            }
        }
        pad
    }

    /// A port's Wii Remote: the one chosen in the controller list, or, on a Boomerang port with no
    /// chosen controller, the first Wii Remote for the first Boomerang and the second for the
    /// second. With another controller chosen, none.
    pub fn port_wiimote(&self, controls: &Controls, port: usize) -> Option<WiimoteState> {
        let player = controls.player(port)?;
        if let Some(chosen) = player.device.as_deref() {
            return self.wiimotes.state(Wiimotes::index_of_name(chosen)?);
        }
        if player.device_kind != Device::Boomerang {
            return None;
        }
        let boomerangs: Vec<usize> = controls
            .enabled_players()
            .filter(|(_, player)| player.device_kind == Device::Boomerang)
            .map(|(index, _)| index)
            .collect();
        let order = boomerangs.iter().position(|&p| p == port)?;
        self.wiimotes.state(order)
    }

    /// The motion sensor that feeds a port's Boomerang.
    ///
    /// **It is the one of the controller chosen on the port**, whatever it is: a Wii Remote, a Pro
    /// Controller, a DualShock. Before, a Boomerang port always looked for a Wii Remote, and
    /// whoever chose another controller with a sensor saw the Boomerang standing still. With no
    /// chosen controller, the Wii Remote whose turn it is counts — the first for the first
    /// Boomerang.
    pub fn port_sensor(&self, controls: &Controls, port: usize) -> PortSensor {
        let Some(player) = controls.player(port) else {
            return PortSensor::Off;
        };
        let chosen = player.device.as_deref();
        if chosen.is_none_or(|name| Wiimotes::index_of_name(name).is_some()) {
            return match self.port_wiimote(controls, port) {
                Some(wiimote) => PortSensor::Wiimote(wiimote),
                None => PortSensor::Off,
            };
        }
        let Some((name, vendor, product, order)) = self.gamepads.identity(chosen, port) else {
            return PortSensor::Off;
        };
        match self.sensors.for_controller(&name, vendor, product, order) {
            Some(sensor) => PortSensor::Controller(sensor),
            None => PortSensor::NoSensor(chosen.unwrap_or(&name).to_string()),
        }
    }

    /// The acceleration a port's Boomerang feels. Without a sensor, still and face up.
    ///
    /// **The Boomerang's length is its X; the Wii Remote's is its Y.** Crash Nitro Kart tells you
    /// to hold the Boomerang lying flat, with both hands and the face toward the player, and turn
    /// it like a steering wheel: the steering is the angle of gravity between X and Y. With the
    /// axes passed straight through, a Wii Remote held the same way put gravity on the wrong axis,
    /// and the kart turned at random. The face is the same on both, so Z stays, and X and Y swap
    /// places.
    ///
    /// The sign of X was set by hand, in Crash Nitro Kart: with the Boomerang's X opposite to the
    /// Wii Remote's Y, turning the wheel to the right took the kart to the left.
    ///
    /// Nintendo controllers also swap X and Y. `hid-nintendo` reports the Pro Controller's length
    /// on Y: upright, facing the player, gravity fell on the Boomerang's X, and the preview rotated
    /// ninety degrees to the right. And the sign is the opposite of the Wii Remote's: with Y passed
    /// as it came, turning left took the kart to the right. The other controllers pass straight
    /// through until someone measures them.
    pub fn port_motion(&self, controls: &Controls, port: usize) -> [f32; 3] {
        let sensor = self.port_sensor(controls, port);
        let Some(raw) = sensor.acceleration() else {
            return AT_REST;
        };
        let [x, y, z] = controls
            .player(port)
            .map_or(raw, |player| player.motion_calibration.apply(raw));
        match sensor {
            PortSensor::Wiimote(_) => [y, x, z],
            PortSensor::Controller(s) if s.vendor == VENDOR_NINTENDO => [-y, x, z],
            _ => [x, y, z],
        }
    }

    /// The motion of all ports, in their order.
    pub fn motions(&self, controls: &Controls) -> [[f32; 3]; PORTS] {
        std::array::from_fn(|port| self.port_motion(controls, port))
    }

    /// The acceleration the port's sensor measures, without calibration.
    pub fn raw_port_motion(&self, controls: &Controls, port: usize) -> Option<[f32; 3]> {
        self.port_sensor(controls, port).acceleration()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The arrow triggers the dpad with both spellings `settings.json` can have.
    #[test]
    fn the_arrow_triggers_the_mapping_in_both_spellings() {
        let pressed = HashSet::from([egui::Key::ArrowUp]);
        assert!(key_pressed(&Source::key("ArrowUp"), &pressed), "the factory one");
        assert!(key_pressed(&Source::key("Up"), &pressed), "the captured one");
        assert!(!key_pressed(&Source::key("ArrowDown"), &pressed));
        assert!(!key_pressed(&Source::button("DPadUp"), &pressed), "a button is not a key");
    }
}
