//! IHID and the keyboard: the ports, the axes and key delivery.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// `IHID` and `IHIDDevice` — the Zeebo gamepad.
    ///
    /// We present an always-connected controller, with the twelve buttons and the four axes that
    /// the console's `hid_devices.cfg` describes. What the player presses arrives through
    /// [`Machine::set_pad`].
    pub(super) fn hid_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let this = self.cpu.read_reg(Reg::R0);
        let result = match name {
            // `IHID` inherits from `IQI`, so AddRef and Release sit in the same slots 0 and 1.
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.device_ports.remove(&this);
                }
                remaining
            }
            // QueryInterface: we return the object itself, since each of our objects has a
            // single interface.
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                SUCCESS
            }
            // GetConnectedDevices(int nDeviceType, int *pnHandles, int nLen, int *pnLenReq).
            // The fourth argument does not fit in the registers and comes from the stack.
            "GetConnectedDevices" => {
                let wanted = a1;
                let handles = a2;
                let capacity = a3;
                let out_needed = self.stack_arg(0)?;
                // `nLenReq` is what the device **has**, and the array gets whatever fits. The
                // Z-Wheel passes a capacity of two in both calls, which is the console's number
                // of USB ports.
                let which = match wanted {
                    // One receiver serves both Boomerangs, so it goes in only once, at the
                    // first port that has one.
                    UID_JOYSTICK_DEVICE => {
                        let mut ports =
                            self.ports_with(crate::input::bindings::Device::Controller);
                        ports.extend(self.ports_with(crate::input::bindings::Device::ZPad));
                        ports.extend(
                            self.ports_with(crate::input::bindings::Device::Boomerang)
                                .first(),
                        );
                        ports.sort_unstable();
                        ports
                    }
                    UID_KEYBOARD_DEVICE => {
                        self.ports_with(crate::input::bindings::Device::Keyboard)
                    }
                    _ => Vec::new(),
                };
                if out_needed != 0 {
                    self.cpu.write_u32(out_needed, which.len() as u32)?;
                }
                if handles != 0 {
                    for (i, &port) in which.iter().take(capacity as usize).enumerate() {
                        self.cpu
                            .write_u32(handles + i as u32 * 4, port_handle(port))?;
                    }
                }
                SUCCESS
            }
            // CreateDevice(int nDevHandle, IHIDDevice **ppDevice)
            //
            // The handle is the one that came out of `GetConnectedDevices`, and this is where
            // it becomes a port: without keeping that link, both devices would read the same
            // controller.
            "CreateDevice" => {
                let device = self.new_object(Interface::HidDevice)?;
                if device == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                let port = (a1.saturating_sub(1) as usize).min(input::PORTS - 1);
                self.device_ports.insert(device, port);
                if a2 != 0 {
                    self.cpu.write_u32(a2, device)?;
                }
                SUCCESS
            }
            // GetDeviceInfo(AEEHIDDeviceInfo *pInfo): { AEEUID type; uint16 pid; uint16 vid;
            // boolean bluetooth }. In IHID the struct comes in the second argument.
            //
            // `type` is the **UID of the device type**, the same one the game passes to
            // `GetConnectedDevices`, not a small number. Bad Dudes vs. DragonNinja compares the
            // field against `0x0106c3fd` before creating the device: with the `1` we used to
            // answer, it never called `CreateDevice`, and the menu saw no buttons at all.
            "GetDeviceInfo" => {
                let out = if iface == Interface::Hid { a2 } else { a1 };
                // In `IHID` the port handle comes in `r1`; in `IHIDDevice` it is the object
                // itself that says which port it belongs to.
                let port = match iface == Interface::Hid {
                    true => (a1.saturating_sub(1) as usize).min(input::PORTS - 1),
                    false => self.port_of(this),
                };
                let kind = match self.ports[port] {
                    Some(crate::input::bindings::Device::Keyboard) => UID_KEYBOARD_DEVICE,
                    _ => UID_JOYSTICK_DEVICE,
                };
                let (vendor, product) = match self.ports[port] {
                    Some(crate::input::bindings::Device::Boomerang) => {
                        (BOOMERANG_VENDOR_ID, BOOMERANG_PRODUCT_ID)
                    }
                    Some(crate::input::bindings::Device::ZPad) => (ZPAD_VENDOR_ID, ZPAD_PRODUCT_ID),
                    _ => (GAMEPAD_VENDOR_ID, GAMEPAD_PRODUCT_ID),
                };
                if out != 0 {
                    self.cpu.write_u32(out, kind)?;
                    self.cpu.write_mem(out + 4, &product.to_le_bytes())?;
                    self.cpu.write_mem(out + 6, &vendor.to_le_bytes())?;
                    self.cpu.write_u32(out + 8, 0)?;
                }
                SUCCESS
            }
            "GetNumberOfButtons" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, input::BUTTON_UIDS.len() as u32)?;
                }
                SUCCESS
            }
            // GetButtonInfo(int nButtonID, AEEHIDButtonInfo *pInfo):
            // { id, state, UID, minimum, maximum }
            "GetButtonInfo" => {
                // The game passes a UID, not an index — we accept both forms.
                let (index, uid) = match input::BUTTON_UIDS.iter().position(|&u| u == a1) {
                    Some(i) => (i as u32, a1),
                    None => match input::BUTTON_UIDS.get(a1 as usize) {
                        Some(&uid) => (a1, uid),
                        None => return Ok(Some(EBADPARM)),
                    },
                };
                // The state has to be the current one: a game that polls instead of waiting for
                // the event only sees the key through here.
                let state = u32::from(self.pads[self.port_of(this)].is_down(index as usize));
                if a2 != 0 {
                    for (i, value) in [index, state, uid, 0, 1].iter().enumerate() {
                        self.cpu.write_u32(a2 + i as u32 * 4, *value)?;
                    }
                }
                SUCCESS
            }
            "GetDeviceStatus" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, HID_STATUS_CONNECTED)?;
                }
                SUCCESS
            }
            // The current position of each axis.
            "GetPositionState" if self.is_boomerang(self.port_of(this)) => {
                self.boomerang_packet(a1)?;
                SUCCESS
            }
            "GetPositionState" => {
                let pad = self.pads[self.port_of(this)];
                let axes = [0, 1, 2, 3].map(|i| pad.console_axis(i));
                // A game that polls the axis every frame reads the center almost always. The
                // counter separates "reads the axis" from "the axis arrived off center", which is
                // what the d-pad mirror changes — and what can be measured without looking at the
                // screen.
                let mut outside = 0u32;
                for (axis_index, &axis_value) in axes.iter().enumerate() {
                    if axis_value != input::AXIS_CENTER {
                        outside |= 1 << axis_index;
                    }
                }
                if outside != 0 {
                    self.off_center_axes += 1;
                    self.off_center_axes_mask |= outside;
                }
                self.write_position_info(a1, &axes)?;
                SUCCESS
            }
            // `GetAxesInfo` does not return values: it returns, in each field, the UID of the
            // axis that occupies that field. That is how the SDK's `AEEHIDThumbsticks.c` finds
            // out where each direction is — and while we answered zeros, it found none.
            "GetAxesInfo" => {
                // The Boomerang receiver swaps Z and RZ relative to the controller, as its entry
                // in `hid_devices.cfg` says. Crash Nitro Kart reads the accelerometer through the
                // analog sticks and finds gravity by UID: with the controller's table, gravity
                // landed in the wrong field and the calibration never accepted the reading.
                let uids = match self.is_boomerang(self.port_of(this)) {
                    true => [0x0106_c4d0, 0x0106_c4d1, 0x0106_c4cf, 0x0106_c4ce],
                    false => input::AXIS_UIDS,
                };
                self.write_axes_info(a1, &uids.map(|uid| uid as i32))?;
                SUCCESS
            }
            // The limits apply to **all** the axes in the struct, not just the four the console
            // uses. Filling in only four, the other twenty were left with minimum and maximum
            // both zero, and a game that normalizes one of those axes — `(value - min) /
            // (max - min)` — divides by zero and locks the direction at one end. That was what
            // kept Crash's car turning left.
            "GetMinPositionInfo" => {
                self.write_axis_range(a1, input::AXIS_MIN)?;
                SUCCESS
            }
            "GetMaxPositionInfo" => {
                self.write_axis_range(a1, input::AXIS_MAX)?;
                SUCCESS
            }
            // GetNextButtonEvent(AEEHIDButtonInfo *, uint32 *pdwTimestamp, boolean *pbDropped).
            //
            // The queue holds events, not state: each press and each release becomes an entry,
            // and the game reads until the queue is empty. `EFAILED` is the "nothing more".
            "GetNextButtonEvent" => {
                let Some((index, down)) = self.pad_events[self.port_of(this)].pop_front() else {
                    if a1 != 0 {
                        self.cpu.write_mem(a1, &[0u8; 20])?;
                    }
                    if a2 != 0 {
                        self.cpu.write_u32(a2, self.elapsed_ms())?;
                    }
                    if a3 != 0 {
                        self.cpu.write_u32(a3, 0)?;
                    }
                    return Ok(Some(EFAILED));
                };
                let uid = input::BUTTON_UIDS.get(index).copied().unwrap_or(0);
                if a1 != 0 {
                    // `AEEHIDButtonInfo`: id, state, UID, minimum, maximum.
                    let info = [index as u32, u32::from(down), uid, 0, 1];
                    for (i, value) in info.iter().enumerate() {
                        self.cpu.write_u32(a1 + i as u32 * 4, *value)?;
                    }
                }
                if a2 != 0 {
                    self.cpu.write_u32(a2, self.elapsed_ms())?;
                }
                if a3 != 0 {
                    self.cpu.write_u32(a3, 0)?;
                }
                SUCCESS
            }
            // GetNextConnectEvent(int *pnDevHandle, int *pnStatus, boolean *pbDropped).
            // GetNextConnectEvent(uint32 *pdwHandle, boolean *pbConnected, uint32 *pdwTimestamp)
            //
            // **The queue is always empty, and empty answers `EFAILED`** — it is BREW's "no more
            // events", the same as in `GetNextButtonEvent`. Answering success with zeroed fields,
            // the game took it that there was a connection event on every query: the Zeebo
            // Extreme games stayed in `GetNextConnectEvent` and `GetDeviceInfo` forever, without
            // arming a timer or drawing, and the session ended for lack of anything to do.
            "GetNextConnectEvent" => {
                for out in [a1, a2, a3] {
                    if out != 0 {
                        self.cpu.write_u32(out, 0)?;
                    }
                }
                EFAILED
            }
            // The `RegisterFor*` calls receive an `ISignal` that we must raise when there is an
            // event. We store which one it is; actually raising it depends on wiring up the
            // host's input.
            "RegisterForConnectEvents"
            | "RegisterForStatusChange"
            | "RegisterForButtonEvent"
            | "RegisterForPositionChange" => {
                if a1 != 0 {
                    self.input_signals.insert((name, self.port_of(this)), a1);
                }
                // **Registering for position already earns a notification.** On the console the
                // controller is connected and at rest when the game registers, and the driver
                // delivers the first position right away; the game uses that notification to
                // ask for the axis range (`GetMinPositionInfo`, `GetMaxPositionInfo`) and where
                // the axes are. While the notification only went out on the first movement,
                // anyone who never touched the analog stick played with uncalibrated axes —
                // Ridge Racer reaches the title screen without asking anything before that.
                if name == "RegisterForPositionChange" {
                    self.raise_input_signal(name, self.port_of(this));
                }
                SUCCESS
            }
            "SetExclusiveLevel" | "Rumble" => SUCCESS,
            "GetExclusiveLevel" | "GetRumbleStatus" => {
                if a1 != 0 {
                    self.cpu.write_u32(a1, 0)?;
                }
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Fills an `AEEHIDPositionInfo` with the position of each axis.
    ///
    /// The struct is `boolean bRelativeAxes` followed by twenty-four integers, one per possible
    /// axis. The Zeebo controller uses four of them — `X`, `Y`, `Z` and `RZ`.
    ///
    /// **The other twenty stay at the center, not at zero.** Zero is the minimum of the device's
    /// range, and a game that reads an axis we don't use would find it pushed against the stop
    /// instead of at rest. The axes are absolute, so `bRelativeAxes` stays zero.
    pub(super) fn write_position_info(
        &mut self,
        addr: u32,
        values: &[i32; 4],
    ) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let mut words = [input::AXIS_CENTER as u32; input::POSITION_INFO_WORDS];
        words[0] = 0;
        for (slot, value) in input::AXIS_SLOTS.iter().zip(values) {
            words[*slot] = *value as u32;
        }
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes).map_err(|e| {
            CpuError(format!(
                "{e} while writing {} bytes at {addr:#010x}",
                bytes.len()
            ))
        })
    }

    /// Writes the `GetAxesInfo` table: in each field, the UID of the axis that occupies it.
    ///
    /// Here the leftover fields are **zeroed**, the opposite of what the position does: zero is
    /// not an axis value, it is "there is no axis in this field", and that is how the game stops
    /// searching.
    pub(super) fn write_axes_info(
        &mut self,
        addr: u32,
        uids: &[i32; 4],
    ) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let mut words = [0u32; input::POSITION_INFO_WORDS];
        for (slot, uid) in input::AXIS_SLOTS.iter().zip(uids) {
            words[*slot] = *uid as u32;
        }
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes)
    }

    /// Writes `value` into every axis field of the `AEEHIDPositionInfo`.
    ///
    /// The first field of the struct is not an axis: it is `boolean bRelativeAxes`, and it stays
    /// zero — the controller's axes are absolute, and saying otherwise would make the game treat
    /// each reading as a displacement and accumulate it.
    pub(super) fn write_axis_range(&mut self, addr: u32, value: i32) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        let mut words = [value as u32; input::POSITION_INFO_WORDS];
        words[0] = 0;
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes)
    }

    /// Delivers a new controller state.
    ///
    /// Each button that changed becomes an event in the queue, and the change wakes the game up
    /// through the `ISignal`s it registered — without that the game would only see the key the
    /// next time it decided to ask, and some never ask.
    pub fn set_pad(&mut self, pad: Pad) {
        self.set_port_pad(0, pad);
    }

    /// The controller the game currently sees on a port. Only for the session tests, which need to
    /// see the turbo on top of the raw controller.
    #[cfg(test)]
    pub(crate) fn port_pad(&self, port: usize) -> Pad {
        self.pads[port]
    }

    /// The same, for a chosen port.
    pub fn set_port_pad(&mut self, port: usize, pad: Pad) {
        let pad = pad.with_home_on_both();
        if port >= input::PORTS || pad == self.pads[port] {
            return;
        }
        let changes = self.pads[port].changes(&pad);
        let moved = pad.axes != self.pads[port].axes;
        self.pads[port] = pad;
        self.pad_events[port].extend(changes.iter().copied());
        // Only `GetNextButtonEvent` drains this queue, and a game that reads the controller via
        // `GetState` or `EVT_KEY` never calls it: the queue grew with every press for the whole
        // session. The oldest go first, which is what a hardware queue does when it
        // overflows.
        let excess = self.pad_events[port].len().saturating_sub(PAD_EVENTS_MAX);
        self.pad_events[port].drain(..excess);
        let now = self.elapsed_ms();
        for &(index, down) in &changes {
            if self.pad_log.len() == PAD_LOG_MAX {
                self.pad_log.pop_front();
            }
            self.pad_log.push_back((now, port, index, down));
        }

        if !changes.is_empty() {
            self.raise_input_signal("RegisterForButtonEvent", port);
        }
        // A phone has one keypad: the first controller is it. See [`input::phone::button_key`].
        if port == 0 {
            for &(index, down) in &changes {
                if let Some(key) = input::phone::button_key(self.applet_class, index) {
                    self.key_events.push_back((key, down));
                }
            }
        }
        // **Returning to center is also a change of position.** Releasing the stick has to wake
        // the callback just like pushing it: whoever reads the axis only from inside the
        // callback — and that is how an event-driven menu is written — keeps the last direction
        // and never finds out the player let go. The symptom is the stick "stuck" in the last
        // direction forever. The doubled navigation this seemed to cause was something else, and
        // is solved in [`Machine::raise_input_signal`].
        if moved {
            self.raise_input_signal("RegisterForPositionChange", port);
        }
    }

    /// The acceleration of a port with a Boomerang, in g in its own frame: `x` along the
    /// controller, `y` forward and `z` out of the button face. At rest face up,
    /// `[0, 0, 1]`.
    pub fn set_port_motion(&mut self, port: usize, acceleration: [f32; 3]) {
        if let Some(motion) = self.motion.get_mut(port) {
            *motion = acceleration;
        }
    }

    /// The Boomerang receiver sends reports nonstop, and each one wakes whoever registered
    /// `RegisterForPositionChange`, even with the same acceleration. Crash Nitro Kart only reads
    /// the position inside that notification: without it, the calibration got one reading and
    /// waited for the others forever. The pace is that of virtual time, not that of whoever
    /// delivers the motion.
    pub(super) fn boomerang_report(&mut self) {
        let now = self.now_us();
        if now.saturating_sub(self.last_boomerang_report_us) < BOOMERANG_PERIOD_US {
            return;
        }
        self.last_boomerang_report_us = now;
        for port in 0..input::PORTS {
            if self.is_boomerang(port) {
                self.raise_input_signal("RegisterForPositionChange", port);
            }
        }
    }

    /// `(started, finished)`: the motion calibrations the game has done so far.
    ///
    /// No game tells the system, so this is what can be seen from outside: the game's debug
    /// messages ("calib" or Crash Nitro Kart's `ACCEL 1` start one; its `ACCEL…CENTER` ends it).
    /// The
    /// interface fills in the rest by checking whether the controller is at rest.
    ///
    /// **The Boomerang's first reading is no signal.** Every game reads the controller at
    /// startup, and the Boomerang Sports games only calibrate after character selection: the
    /// notice opened in the wrong place. They also leave no text trail at that moment — the
    /// names of the calibration screens only go through the BREW functions during preload —, and
    /// the calibration state lives in an object whose layout changes from game to game.
    pub fn calibration(&self) -> (u32, u32) {
        self.calibrations
    }

    /// Reads a debug message from the game looking for calibration.
    pub(super) fn find_calibration(&mut self, message: &str) {
        let lowercase = message.to_ascii_lowercase();
        // Crash Nitro Kart writes "ACCEL 1" when it recognizes the Boomerang, and opens the
        // calibration right after.
        if lowercase.contains("calib") || matches!(message.trim(), "ACCEL 1" | "ACCEL 2") {
            self.calibrations.0 += 1;
        }
        if lowercase.contains("center =") {
            self.calibrations.1 += 1;
        }
    }

    pub(super) fn is_boomerang(&self, port: usize) -> bool {
        self.ports.get(port).copied().flatten() == Some(crate::input::bindings::Device::Boomerang)
    }

    /// Writes a Boomerang receiver packet into the `AEEHIDPositionInfo`.
    ///
    /// The Boomerang does not use the structure as axes: it is the receiver's raw report, and the
    /// game takes it apart. Read in Zeebo Sports Queimada (`0x4231c`, `0x42458`, `0x40b0c`,
    /// `0x413d0`, `0x412f4`), with the fields counted from the first integer after
    /// `bRelativeAxes`:
    ///
    /// - **Fields 1, 2 and 3: the accelerometer**, one byte per axis centered at `0x80`. The game
    ///   rotates X and Y by 20° (`0.94` and `0.342`) before using them — the sensor sits crooked
    ///   inside the arm — and calibrates zero and scale with the controller at rest face up and
    ///   then face down.
    /// - **Field 4: the buttons**, in inverted logic (a zero bit is pressed): bit 0 button 1,
    ///   1 button 2, 2 HOME, 3 left, 4 down, 5 right and 6 up.
    /// - **Field 5: the player and the type.** Bit 0 says which of the two Boomerangs the packet
    ///   belongs to; the rest is the type. Type 0 is "disconnected"; 1 to 4 connects, and field 6
    ///   carries a device value that changes with the type.
    /// - **Field 8: the counter**, 8 bits wide. The game expects it to step by one; a jump marks a
    ///   lost packet, and at startup it waits for the counter to change before considering the
    ///   receiver alive. If it stands still, the game gets stuck in that loop.
    ///
    /// A new packet every [`BOOMERANG_PERIOD_US`], alternating the two players.
    pub(super) fn boomerang_packet(&mut self, addr: u32) -> Result<(), CpuError> {
        if addr == 0 {
            return Ok(());
        }
        // 1 g in sensor units. Crash Nitro Kart's calibration only accepts gravity between 162 and
        // 220, which centers 1 g near 63; Queimada normalizes by the at-rest reading.
        const SCALE: f32 = 60.0;
        const COS: f32 = 0.94;
        const SIN: f32 = 0.342;
        // **One new packet per receiver period, not per read.** Queimada reads three times per
        // frame and only processes a player's buttons on a read with no new packet from them: with
        // a packet on every read, no press ever got through. Within the period, a read repeats the
        // last packet.
        let now = self.now_us();
        if now.saturating_sub(self.last_boomerang_packet_us) >= BOOMERANG_PERIOD_US {
            self.last_boomerang_packet_us = now;
            self.boomerang_sequence = self.boomerang_sequence.wrapping_add(1);
        }
        let sequence = self.boomerang_sequence;
        // The two players always alternate; the one without a controller goes as "disconnected".
        // Queimada needs this alternation to process the buttons.
        let boomerangs = self.ports_with(crate::input::bindings::Device::Boomerang);
        let player = usize::from(sequence & 1);
        let port = boomerangs.get(player).copied();
        let mut fields = [0u32; input::POSITION_INFO_WORDS];
        // **The disconnected player carries the first one's acceleration.** Crash Nitro Kart reads
        // the accelerometer without checking whose packet it is: with the second player coming in
        // at rest on every report, the steering jumped between motion and rest, the kart turned at
        // random and the calibration went through without waiting. Queimada ignores the
        // acceleration of a disconnected player.
        let [x, y, z] = port
            .or(boomerangs.first().copied())
            .map_or([0.0, 0.0, 1.0], |p| self.motion[p]);
        // The inverse of the rotation the game applies: that way it arrives at the acceleration
        // we deliver.
        let raw = [COS * x - SIN * y, SIN * x + COS * y, z];
        for (field, value) in fields[1..4].iter_mut().zip(raw) {
            *field = (0x80 as f32 + value * SCALE).round().clamp(0.0, 255.0) as u32;
        }
        let mut loose = 0x7fu32;
        if let Some(port) = port {
            let pad = self.pads[port];
            const BITS: [(&str, u32); 7] = [
                ("b1", 0),
                ("b2", 1),
                ("back", 2),
                ("left", 3),
                ("down", 4),
                ("right", 5),
                ("up", 6),
            ];
            for (name, bit) in BITS {
                if Pad::button_by_name(name).is_some_and(|i| pad.is_down(i)) {
                    loose &= !(1 << bit);
                }
            }
        }
        fields[4] = loose;
        let kind = match port {
            Some(_) => 1,
            None => 0,
        };
        fields[5] = (kind << 1) | player as u32;
        fields[8] = u32::from(sequence);
        let bytes: Vec<u8> = fields.iter().flat_map(|w| w.to_le_bytes()).collect();
        self.cpu.write_mem(addr, &bytes)
    }

    /// The port of an `IHIDDevice`, or the first one when the object was not registered.
    ///
    /// The windowless path and the tests create devices without going through `CreateDevice` with
    /// a handle; for them port one is the right answer, because it is the only one that exists.
    pub(super) fn port_of(&self, device: u32) -> usize {
        self.device_ports
            .get(&device)
            .copied()
            .unwrap_or(0)
            .min(input::PORTS - 1)
    }

    /// Which ports are connected to the requested device, in order.
    pub(super) fn ports_with(&self, device: crate::input::bindings::Device) -> Vec<usize> {
        (0..input::PORTS)
            .filter(|&n| self.ports[n] == Some(device))
            .collect()
    }

    pub fn set_key(&mut self, avk: u32, down: bool) {
        // In a phone game the controller's own keys come from the phone mapping, in
        // [`Machine::set_port_pad`] — see [`input::phone::replaces_console_key`].
        if input::phone::replaces_console_key(self.applet_class, avk) {
            return;
        }
        self.key_events.push_back((avk, down));
    }

    /// Delivers the queued keys.
    ///
    /// The event is BREW's `EVT_KEY`, with the virtual code in `wParam`. The release goes as
    /// [`input::EVT_KEY_RELEASE`]: a game that only looks at the press ignores the second one at no
    /// cost, and one that counts both needs both.
    ///
    /// **The widget gets the key first, not the application.** In BREW it is the UI extension
    /// that routes input to whoever has focus, and the handler of the Z-Wheel's opening form
    /// proves it: it tests `evt == 0x100` and compares `wParam` against `AVK_` codes. Sending it
    /// straight to the application returns zero — measured.
    ///
    /// The order is BREW's: the widget gets the first chance and, if nobody handled it, the event
    /// goes up to the application. An event nobody handles does nothing, and that is why
    /// delivering keys is safe in a way that inventing property events was not.
    pub(super) fn flush_keys(&mut self) -> Result<(), CpuError> {
        // The whole tree goes to the serial on the first key: it is the snapshot of the model at
        // the moment the question "who does this go to?" comes up.
        if !self.key_events.is_empty() && !self.widgets_dumped {
            self.widgets_dumped = true;
            self.dump_widgets();
        }
        while let Some((avk, down)) = self.key_events.pop_front() {
            // **The exit from the queue, before any shortcut.** The delivery line, further down,
            // only shows up for keys that reach the handlers: an arrow consumed by HTML scrolling
            // leaves the loop without a trace, and "did not arrive" becomes indistinguishable from
            // "was consumed".
            if self.serial.is_some() {
                self.log_serial(format!(
                    "<queue {avk:#x} {} leaves dispatch, {} in queue>",
                    match down {
                        true => "press",
                        false => "release",
                    },
                    self.key_events.len()
                ));
            }
            let event = match down {
                true => input::EVT_KEY,
                false => input::EVT_KEY_RELEASE,
            };
            let mut handled = false;
            // With the HTML panel in focus, the vertical arrows belong to it while there is text
            // left to scroll. See [`Machine::scroll_focused_html`].
            if matches!(avk, input::avk::UP | input::avk::DOWN) {
                let downward = avk == input::avk::DOWN;
                if down && self.scroll_focused_html(downward) {
                    self.scroll_keys.insert(avk);
                    continue;
                }
                if !down && self.scroll_keys.remove(&avk) {
                    continue;
                }
            }
            // **Newest first.** Forms stack up and none is destroyed here, so the list has the
            // opening form's handler next to the current screen's. The opening one returns 1
            // for any press — it handles key 0 and CLR —, and coming first it decided
            // everything: the z-pad form registered its own and never saw a key.
            //
            // Sorting by creation order is what a form stack does: whoever is on top gets the
            // first chance, and whatever it doesn't consume goes down.
            // The order here still has no model, and three heuristics have already failed: all
            // widgets, only the current form's tree, newest to oldest and the reverse. In all
            // of them the one that answers is a handler that is literally
            // `mov r0,#1; bx lr` — the game's `0x77300`, which returns "handled" for any
            // key. On the console the one that receives is the widget **with focus**, and focus
            // is something we still can't read. Until we can, the map order stays, which is the
            // one that was here before I started touching it.
            // **Inside the current form, every handler sees the key.** Stopping at the first one
            // that returns non-zero seemed right — it is what a BREW container does —, but
            // `0x77300` is `mov r0,#1; bx lr`: it says "handled" to anything that reaches it,
            // and it is registered on the **status bar**, a 640×50 container that on the console
            // receives no keys at all. With it in front, the stage never saw a press and the
            // wheel didn't turn.
            //
            // On the console the one that receives is the widget **with focus**, and focus is
            // still something we can't read. Delivering to all of the current screen's handlers
            // is the approximation that doesn't depend on knowing: a handler that doesn't
            // recognize the event does nothing, and one that does acts. The off-screen ones still
            // count only when nobody inside answered, which is what keeps the opening form from
            // answering on top of the menu.
            let inside = self.form_tree();
            let (on_screen, outside): (Vec<_>, Vec<_>) = self
                .handlers_in_order(&inside)
                .into_iter()
                .partition(|(is_inside, _)| *is_inside);
            // The counts and the addresses are taken before the loops: after them the lists have
            // been consumed. **The addresses are what answers "did the key reach the list's
            // handler?"** — the Z-Wheel's game list is the roller `0x30000ad0`, handler `0x75724`.
            let (onscreen_count, offscreen_count) = (on_screen.len(), outside.len());
            let who: Vec<String> = on_screen
                .iter()
                .chain(outside.iter())
                .take(4)
                .map(|(_, (callback, _))| format!("{callback:#x}"))
                .collect();
            for (_, (callback, context)) in on_screen {
                let output = self.call_guest(callback, [context, event, avk, 0], QSORT_BUDGET)?;
                if matches!(output, Outcome::Returned { code } if code != 0) {
                    handled = true;
                }
            }
            if !handled {
                for (_, (callback, context)) in outside {
                    let output =
                        self.call_guest(callback, [context, event, avk, 0], QSORT_BUDGET)?;
                    if matches!(output, Outcome::Returned { code } if code != 0) {
                        handled = true;
                        break;
                    }
                }
            }
            // **Key delivery goes to the serial capture.** It is the instrument that answers the
            // two questions left when an applet reacts to the first key and ignores the following
            // ones: **to whom** it was delivered, and whether anyone handled it. Measured: on the
            // core path the second key has no effect at all, with the widget tree identical to the
            // scan's — without this line, "the key did not arrive" and "it arrived and nobody
            // handled it" are indistinguishable.
            if self.serial.is_some() {
                self.log_serial(format!(
                    "<key {avk:#x} {} to {} handler(s) on screen and {} off ({}){}>",
                    match down {
                        true => "press",
                        false => "release",
                    },
                    onscreen_count,
                    offscreen_count,
                    who.join(" "),
                    match handled {
                        true => ", handled",
                        false => ", nobody handled",
                    }
                ));
            }
            if !handled {
                // A phone game also gets `EVT_KEY_PRESS` — see [`input::phone::key_events`].
                for &applet_event in input::phone::key_events(self.applet_class, down) {
                    let _ = self.send_applet_event(self.applet_class, applet_event, avk as u16, 0)?;
                }
            }
        }
        Ok(())
    }

    /// Says which device the console sees on each port. `None` disconnects the port.
    pub fn set_ports(
        &mut self,
        ports: [Option<crate::input::bindings::Device>; input::PORTS],
    ) {
        self.ports = ports;
    }

    /// Raises the signal registered in one of the `IHIDDevice` `RegisterFor*` calls.
    /// `port` is part of the key because registration is done **on the device object**, one per
    /// controller: with two connected, keying only by the registration name made the second
    /// overwrite the first and every event wake controller two's callback.
    pub(super) fn raise_input_signal(&mut self, register: &'static str, port: usize) {
        let Some(&signal) = self.input_signals.get(&(register, port)) else {
            return;
        };
        if let Some(&callback) = self.signals.get(&signal) {
            // A BREW signal is a "there is work" notice, not a counted event. The host may update
            // more than one port (or move the axis and return to center right away) before the
            // scheduler delivers the callback. Queuing the same function twice makes many menus
            // process the same navigation in opposite directions. Coalescing here preserves every
            // change in the state the game can query and removes the spurious reentry.
            if !self.pending_signals.contains(&callback) {
                self.pending_signals.push(callback);
            }
        }
    }

    /// The URLs the game tried to fetch through `IWeb`.
    /// The last presses delivered to the game, as `(instant, button name, pressed)`.
    pub fn pad_log(&self) -> Vec<(u32, usize, &'static str, bool)> {
        self.pad_log
            .iter()
            .map(|&(ms, port, index, down)| {
                (
                    ms,
                    port,
                    input::BUTTON_NAMES.get(index).copied().unwrap_or("?"),
                    down,
                )
            })
            .collect()
    }
}
