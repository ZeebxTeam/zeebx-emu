//! The motion sensors of the host's controllers, read directly from Linux input devices.
//!
//! The kernel drivers separate the sensor from the rest of the controller, into a sibling device
//! marked with `INPUT_PROP_ACCELEROMETER`: "Pro Controller (IMU)" in `hid-nintendo`, "… Motion
//! Sensors" in `hid-playstation`. gilrs does not see them — they are axes of another device —, so
//! reading them is up to us, like the Wii Remote's accelerometer (which stays in
//! [`crate::input::wiimote`]).
//!
//! **The sensor's node usually has no permission.** udev only gives the session user access
//! (`uaccess`) to what it marks as a joystick, and the sensor is marked as an accelerometer. The
//! controller opens, the sensor does not; in that case the sensor shows up with
//! [`SensorState::permission_denied`], and the interface says which rule fixes it.
//!
//! Outside Linux there is no reading: the list stays empty.

use std::sync::{Arc, Mutex};

/// A motion sensor that was found, and what it measures right now.
#[derive(Debug, Clone, PartialEq)]
pub struct SensorState {
    /// The name of the sensor's device, as the kernel announces it.
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    /// The acceleration in g, on the driver's axes.
    pub acceleration: [f32; 3],
    /// Whether the sensor has sent any reading yet.
    pub has_reading: bool,
    /// Whether the node exists and could not be opened.
    pub permission_denied: bool,
}

impl SensorState {
    /// Whether this sensor belongs to the controller named `system_name` with the
    /// `vendor:product` pair.
    ///
    /// The driver gives the sensor the controller's name with a suffix: "Pro Controller" and "Pro
    /// Controller (IMU)". The pair alone is not enough — one receiver can have more than one
    /// device —, and neither is the name alone, because two manufacturers can use the same one.
    pub fn belongs_to_controller(&self, system_name: &str, vendor: u16, product: u16) -> bool {
        self.vendor == vendor
            && self.product == product
            && !system_name.is_empty()
            && self.name.starts_with(system_name)
    }
}

/// The udev rule that opens the sensors to the session user, for the interface to show.
pub const UDEV_RULE: &str =
    r#"SUBSYSTEM=="input", KERNEL=="event*", ENV{ID_INPUT_ACCELEROMETER}=="1", TAG+="uaccess""#;

/// Where the rule goes. The number is lower than that of `73-seat-late.rules`, which is what
/// applies `uaccess`: a rule after it would tag the node too late.
pub const RULE_FILE: &str = "/etc/udev/rules.d/70-zeebx-sensores.rules";

/// Writes the [`UDEV_RULE`] and applies it to the nodes that already exist, asking for the
/// password through `pkexec`.
///
/// It is a single command, so the system's password window appears only once: it writes the file,
/// reloads the rules and runs the input devices through them again, which gives the ACL to the
/// sensors already connected without having to reconnect the controller. Reading the ones that had
/// no permission is retried every second, and comes back on its own.
///
/// Blocks until the person answers the window: the caller runs this on a thread.
pub fn unlock_sensors() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        let script = format!(
            "cat > {RULE_FILE} && udevadm control --reload-rules && \
             udevadm trigger --subsystem-match=input --action=change"
        );
        let mut child = std::process::Command::new("pkexec")
            .args(["sh", "-c", &script])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| format!("pkexec: {error}"))?;
        if let Some(mut entry) = child.stdin.take() {
            entry
                .write_all(format!("{UDEV_RULE}\n").as_bytes())
                .map_err(|error| error.to_string())?;
        }
        let output = child.wait_with_output().map_err(|error| error.to_string())?;
        match output.status.code() {
            Some(0) => Ok(()),
            // 126 is the window closed or the password refused; 127, polkit with no agent to ask.
            Some(126) => Err("authorization denied".into()),
            Some(127) => Err("there is no way to ask for the password in this session".into()),
            _ => Err(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    Err("only exists on Linux".into())
}

/// The sensors found, in the order of their nodes in `/dev/input`.
#[derive(Clone, Default)]
pub struct Sensors {
    states: Arc<Mutex<Vec<(std::path::PathBuf, SensorState)>>>,
}

impl Sensors {
    /// Starts looking for sensors in the background.
    pub fn start() -> Self {
        let sensors = Self::default();
        #[cfg(target_os = "linux")]
        {
            let states = sensors.states.clone();
            let _ = std::thread::Builder::new()
                .name("sensors-search".into())
                .spawn(move || linux::search(states));
        }
        sensors
    }

    /// The `order`-th sensor of the controller named `system_name` with the `vendor:product`
    /// pair.
    ///
    /// The order is that in which the nodes arrived, the same in which the system lists the
    /// controllers: with two Pro Controllers connected, the second controller gets the second
    /// sensor.
    pub fn for_controller(
        &self,
        system_name: &str,
        vendor: u16,
        product: u16,
        order: usize,
    ) -> Option<SensorState> {
        let states = self.states.lock().ok()?;
        states
            .iter()
            .map(|(_, state)| state)
            .filter(|state| state.belongs_to_controller(system_name, vendor, product))
            .nth(order)
            .cloned()
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::collections::HashSet;
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::path::{Path, PathBuf};

    const EV_ABS: u16 = 3;
    /// The bit of `INPUT_PROP_ACCELEROMETER` in the `properties` file.
    const PROP_ACCELEROMETER: u64 = 1 << 6;
    /// `struct input_event` on 64 bits: `timeval` (16), `type` (2), `code` (2), `value` (4).
    const EVENT_SIZE: usize = 24;
    /// Units per g when the driver declares no resolution.
    const UNITS_PER_G_WITHOUT_RESOLUTION: f32 = 100.0;

    /// Looks for sensors forever, opening a reader for each new node.
    pub(super) fn search(states: Arc<Mutex<Vec<(PathBuf, SensorState)>>>) {
        let opened: Arc<Mutex<HashSet<PathBuf>>> = Default::default();
        loop {
            for (node, mut state, axes) in devices() {
                let create = opened.lock().is_ok_and(|mut a| a.insert(node.clone()));
                if !create {
                    continue;
                }
                if let Ok(mut list) = states.lock() {
                    // A sensor retried after having no permission is already in the list, and
                    // stays marked until the new attempt decides: without this the preview would
                    // flicker between "no permission" and "waiting" every second.
                    if let Some((_, old)) = list.iter().find(|(path, _)| *path == node) {
                        state.permission_denied = old.permission_denied;
                    }
                    list.retain(|(path, _)| *path != node);
                    list.push((node.clone(), state));
                    list.sort_by(|a, b| node_number(&a.0).cmp(&node_number(&b.0)));
                }
                let (states, opened) = (states.clone(), opened.clone());
                let _ = std::thread::Builder::new()
                    .name("sensor".into())
                    .spawn(move || {
                        let permission_denied = read(&node, axes, &states);
                        // Without permission the sensor stays in the list, marked, for the
                        // interface to explain, and leaves `opened` to be retried on the next
                        // round: that is how it comes back on its own once the rule is applied.
                        // The others leave the list when the controller disconnects.
                        if let Ok(mut a) = opened.lock() {
                            a.remove(&node);
                        }
                        if !permission_denied && let Ok(mut list) = states.lock() {
                            list.retain(|(path, _)| *path != node);
                        }
                    });
            }
            // A sensor without permission that vanished (the controller disconnected) leaves the
            // list.
            if let Ok(mut list) = states.lock() {
                list.retain(|(node, state)| !state.permission_denied || node.exists());
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    fn node_number(node: &Path) -> u32 {
        node.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("event"))
            .and_then(|n| n.parse().ok())
            .unwrap_or(u32::MAX)
    }

    /// `(node in /dev/input, initial state, codes of the X, Y and Z axes)` of each sensor.
    ///
    /// The Wii Remote's accelerometer is left out: it is read together with the buttons, in
    /// [`crate::input::wiimote`].
    fn devices() -> Vec<(PathBuf, SensorState, [u16; 3])> {
        let Ok(entries) = std::fs::read_dir("/sys/class/input") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                if !name.starts_with("event") {
                    return None;
                }
                let base = entry.path().join("device");
                let read = |file: &str| std::fs::read_to_string(base.join(file)).ok();
                let properties = u64::from_str_radix(read("properties")?.trim(), 16).ok()?;
                if properties & PROP_ACCELEROMETER == 0 {
                    return None;
                }
                let label = read("name")?.trim().to_string();
                if label.starts_with("Nintendo Wii Remote") {
                    return None;
                }
                let hex = |file: &str| u16::from_str_radix(read(file)?.trim(), 16).ok();
                let (vendor, product) = (hex("id/vendor")?, hex("id/product")?);
                // Controllers with a gyroscope put the acceleration on X, Y and Z and the
                // rotation on RX, RY and RZ; those with only an accelerometer may use the R ones.
                let abs_axes = read("capabilities/abs")?;
                let last = abs_axes.split_whitespace().last()?;
                let bits = u64::from_str_radix(last, 16).ok()?;
                let axes = match bits & 0b111 == 0b111 {
                    true => [0, 1, 2],
                    false => [3, 4, 5],
                };
                let state = SensorState {
                    name: label,
                    vendor,
                    product,
                    acceleration: [0.0, 0.0, 1.0],
                    has_reading: false,
                    permission_denied: false,
                };
                Some((Path::new("/dev/input").join(name), state, axes))
            })
            .collect()
    }

    /// The resolution of an axis, in units per g. It is the `resolution` of
    /// `struct input_absinfo`, which `hid-nintendo` and `hid-playstation` fill in with the
    /// sensor's scale.
    fn units_per_g(file: &std::fs::File, axis: u16) -> f32 {
        // `EVIOCGABS(axis)`: `_IOR('E', 0x40 + axis, struct input_absinfo)`, 24 bytes.
        let request = 0x8000_0000u64 | (24 << 16) | ((b'E' as u64) << 8) | (0x40 + axis as u64);
        let mut info = [0i32; 6];
        // SAFETY: the `ioctl` writes exactly the 24 bytes of `input_absinfo` into `info`.
        let result =
            unsafe { libc::ioctl(file.as_raw_fd(), request as _, info.as_mut_ptr()) };
        match (result, info[5]) {
            (0, resolution) if resolution > 0 => resolution as f32,
            _ => UNITS_PER_G_WITHOUT_RESOLUTION,
        }
    }

    /// Reads the sensor until it vanishes. Returns whether the node could not be opened for lack
    /// of permission.
    fn read(node: &Path, axes: [u16; 3], states: &Mutex<Vec<(PathBuf, SensorState)>>) -> bool {
        let mut file = match std::fs::File::open(node) {
            Ok(file) => file,
            Err(error) => {
                let permission_denied = error.kind() == std::io::ErrorKind::PermissionDenied;
                if permission_denied
                    && let Ok(mut list) = states.lock()
                    && let Some((_, state)) = list.iter_mut().find(|(n, _)| n == node)
                {
                    state.permission_denied = true;
                }
                return permission_denied;
            }
        };
        if let Ok(mut list) = states.lock()
            && let Some((_, state)) = list.iter_mut().find(|(n, _)| n == node)
        {
            state.permission_denied = false;
        }
        let scale = axes.map(|axis| units_per_g(&file, axis));
        let mut raw = [0u8; EVENT_SIZE];
        let mut acceleration = [0f32; 3];
        while file.read_exact(&mut raw).is_ok() {
            let kind = u16::from_ne_bytes([raw[16], raw[17]]);
            let code = u16::from_ne_bytes([raw[18], raw[19]]);
            let value = i32::from_ne_bytes([raw[20], raw[21], raw[22], raw[23]]);
            if kind != EV_ABS {
                continue;
            }
            let Some(i) = axes.iter().position(|&e| e == code) else {
                continue;
            };
            acceleration[i] = value as f32 / scale[i];
            let Ok(mut list) = states.lock() else {
                return false;
            };
            if let Some((_, state)) = list.iter_mut().find(|(n, _)| n == node) {
                state.acceleration = acceleration;
                state.has_reading = true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor(name: &str, vendor: u16, product: u16) -> SensorState {
        SensorState {
            name: name.into(),
            vendor,
            product,
            acceleration: [0.0, 0.0, 1.0],
            has_reading: false,
            permission_denied: false,
        }
    }

    #[test]
    fn the_sensor_binds_to_the_controller_by_name_and_id_pair() {
        let imu = sensor("Pro Controller (IMU)", 0x057e, 0x2009);
        assert!(imu.belongs_to_controller("Pro Controller", 0x057e, 0x2009));
        // Another device with the same pair, or another controller with a similar name, is not
        // it.
        assert!(!imu.belongs_to_controller("Joy-Con (L)", 0x057e, 0x2009));
        assert!(!imu.belongs_to_controller("Pro Controller", 0x057e, 0x2006));
        assert!(!imu.belongs_to_controller("", 0x057e, 0x2009));
    }

    #[test]
    fn the_second_identical_controller_gets_the_second_sensor() {
        let sensors = Sensors::default();
        {
            let mut list = sensors.states.lock().unwrap();
            let mut first = sensor("Pro Controller (IMU)", 0x057e, 0x2009);
            first.acceleration = [1.0, 0.0, 0.0];
            let mut second = sensor("Pro Controller (IMU)", 0x057e, 0x2009);
            second.acceleration = [0.0, 1.0, 0.0];
            list.push(("/dev/input/event22".into(), first));
            list.push(("/dev/input/event30".into(), second));
        }
        let second = sensors.for_controller("Pro Controller", 0x057e, 0x2009, 1).unwrap();
        assert_eq!(second.acceleration, [0.0, 1.0, 0.0]);
        assert!(sensors.for_controller("Pro Controller", 0x057e, 0x2009, 2).is_none());
    }
}
