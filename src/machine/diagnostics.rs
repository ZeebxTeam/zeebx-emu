//! Introspection: the trace, the fault dump and the counters the interface shows.

use super::*;

/// How many odd pointers or requests from the game the diagnostics keep before stopping.
///
/// See [`Machine::note_bad_pointer`]: the text comes from the game, so the size of the list
/// must not depend on what it asks for. 512 lines are more than any report has ever needed, and
/// the excess is dropped with a notice.
const BAD_POINTERS_CAP: usize = 512;

/// How many distinct lines of the game's log are kept. See [`Machine::record_debug`].
const MAX_GAME_LINES: usize = 2000;

impl<C: CpuBackend> Machine<C> {
    /// Turns on logging of every call, in order.
    pub fn set_tracing(&mut self, on: bool) {
        self.tracing = on;
    }

    /// Restricts tracing to the calls whose name contains `part`.
    ///
    /// Without this, a game that does thousands of `strlen` per frame pushes exactly the calls
    /// one wants to see out of the window.
    pub fn set_trace_filter(&mut self, part: Option<String>) {
        self.trace_filter = part;
    }

    /// The recorded calls, in order.
    pub fn trace(&self) -> &[String] {
        &self.trace
    }

    /// Copies a chunk of guest memory, for external inspection.
    ///
    /// It exists for debugging: when the game breaks on a null pointer, what answers "why" is the
    /// struct that led it there, and that struct only exists in guest memory.
    pub fn dump(&self, addr: u32, len: usize) -> Result<Vec<u8>, CpuError> {
        let mut bytes = vec![0u8; len];
        self.cpu.read_mem(addr, &mut bytes)?;
        Ok(bytes)
    }

    /// `r0..r11` at the time of the last memory fault.
    pub fn fault_regs(&self) -> [u32; 12] {
        self.fault_regs
    }

    /// Return addresses found on the stack of the last fault.
    pub fn fault_stack(&self) -> &[u32] {
        &self.fault_stack
    }

    /// Scans the stack looking for return addresses.
    ///
    /// It is not an exact backtrace — without an unwind table, what can be done is to read the
    /// stack words and keep those that point just after a call instruction. False positives
    /// show up, but the real chain shows up with them, and it is what answers "how did I get
    /// here".
    pub(super) fn scan_stack(&self) -> Vec<u32> {
        let sp = self.cpu.read_reg(Reg::Sp);
        let top = loader::STACK_BASE + loader::STACK_SIZE as u32;
        let module = &self.module.mem.regions()[0];
        let (low, high) = (module.base, module.base + module.bytes.len() as u32);

        let mut found = Vec::new();
        let mut address = sp;
        while address < top && found.len() < STACK_DEPTH {
            let Ok(value) = self.cpu.read_u32(address) else {
                break;
            };
            address += 4;
            if value < low || value >= high || value % 4 != 0 {
                continue;
            }
            // A return address has a `bl`/`blx` right before it.
            let Ok(previous) = self.cpu.read_u32(value - 4) else {
                continue;
            };
            let is_call = (previous >> 24) & 0x0f == 0x0b || (previous >> 20) & 0xff == 0x12;
            if is_call {
                found.push(value);
            }
        }
        found
    }

    /// Calls that received an invalid pointer from the guest.
    pub fn bad_pointers(&self) -> Vec<String> {
        let lines: Vec<String> = self.bad_pointers.iter().cloned().collect();
        // The report says the list is full, instead of looking complete when it is not.
        if self.bad_pointers.len() >= BAD_POINTERS_CAP {
            let mut with_notice = lines;
            with_notice.push(format!(
                "(list at its cap of {BAD_POINTERS_CAP}; the rest was not kept)"
            ));
            return with_notice;
        }
        lines
    }

    /// Records an odd pointer or request from the game, **with a cap**.
    ///
    /// **The text is chosen by the game**: the requested file name and the address go into the
    /// line. Without a cap, this list grows with whatever the game wants to ask for — and it exists
    /// for the report, not to become memory. At the cap, the excess is dropped, and the report says
    /// so.
    pub(super) fn note_bad_pointer(&mut self, text: String) {
        if self.bad_pointers.len() < BAD_POINTERS_CAP {
            self.bad_pointers.insert(text);
        }
    }

    /// Turns on time measurement per API method. See [`Machine::api_profile`].
    ///
    /// The measurement is **sampled** — one call in every 64 — and out of necessity: this machine's
    /// clock is a system call, and reading the pair on every call made the instrument charge more
    /// than the method being measured. The call count stays exact in [`Machine::call_log`].
    pub fn enable_api_profile(&mut self) {
        self.profiling_api = true;
        // **The price of the instrument itself, measured on the machine it runs on.** Reading the
        // clock costs tens of nanoseconds where the `vDSO` answers and more than a microsecond
        // where it does not; without discounting that, a method that does nothing shows up costing
        // 1.4 µs — and it was that reading that put `GetClipRect` at the top of a report.
        self.clock_ns = Self::measure_clock();
    }

    /// How much real time each API method cost, **estimated**, from the most expensive to the
    /// cheapest.
    ///
    /// The estimate uses the mean of that method's samples, multiplied by its number of calls:
    /// no call is special, so the mean holds for all of them. A method with few samples comes out
    /// with a wide margin — and that is why the sample count goes along in the report, instead of
    /// the number appearing on its own.
    pub fn api_profile(&self) -> Vec<(String, u64)> {
        // The pair of reads the profile does per sampled call. Discounted from the mean before
        // estimating, or else the published number is the price of the instrument.
        let overhead = self.clock_ns.saturating_mul(2);
        let mut lines: Vec<_> = self
            .api_time
            .iter()
            .map(|(&(iface, slot), &(ns, samples))| {
                let name = aee::Interface::from_index_public(iface)
                    .and_then(|i| i.method(slot).map(|m| format!("{}::{m}", i.name())))
                    .unwrap_or_else(|| format!("interface {iface} slot {slot}"));
                let calls = self
                    .call_log()
                    .iter()
                    .find(|(other, _)| other == &name)
                    .map(|(_, times)| u64::from(*times))
                    .unwrap_or(samples);
                let estimated = match samples {
                    0 => 0,
                    // `saturating_sub`: a method cheaper than the instrument stays at zero, and
                    // zero is the right answer. Negative does not exist, and "almost zero" cannot
                    // be told apart from the noise of this measurement.
                    n => (ns / n).saturating_sub(overhead) * calls.max(n),
                };
                (name, estimated)
            })
            .collect();
        lines.sort_unstable_by_key(|line| std::cmp::Reverse(line.1));
        lines
    }

    /// Puts a network body in the serial capture, as text when possible and in hexadecimal when
    /// not — and, if it is `deflate`, the inflated contents too.
    ///
    /// Only the URL, the status and the size went into the report, and that is not enough to debug
    /// a protocol: what breaks a record is **the contents**, field by field. A Zeeboid arrived from
    /// the server with its fields shifted by one place and the password truncated to sixteen
    /// characters spliced onto the IMEI, and without the bytes there is no way to tell whether the
    /// server, the transport or the game's reading got it wrong.
    ///
    /// It goes only to the serial, which is optional: the body may carry the IMEI and password, and
    /// that does not go into a report that gets sent around without anyone meaning to.
    pub(super) fn log_body(&mut self, label: &str, bytes: &[u8]) {
        /// How much of a body fits in the log.
        const CAP: usize = 4096;

        if self.serial.is_none() {
            return;
        }
        let show = |data: &[u8]| -> String {
            let cutoff = &data[..data.len().min(CAP)];
            match cutoff
                .iter()
                .all(|&b| b == b'\n' || (0x20..0x7f).contains(&b))
            {
                true => String::from_utf8_lossy(cutoff).into_owned(),
                false => cutoff.iter().map(|b| format!("{b:02x}")).collect(),
            }
        };
        let mut line = format!("<{label} {} bytes: {}>", bytes.len(), show(bytes));
        if let Some(inflated) = inflate(bytes) {
            line.push_str(&format!("\n<{label} inflated: {}>", show(&inflated)));
        }
        self.log_serial(line);
    }

    /// Turns on serial capture: every log line goes to this file, in order and with the
    /// virtual clock's timestamp.
    ///
    /// The Zeebo has a debug UART, and what comes out of it is `DBGPRINTF` — the module does not
    /// talk to the hardware, the firmware does the routing. So the stream a serial cable would see
    /// is exactly this one.
    ///
    /// It exists apart from the report because the report **groups repetitions**, and grouping
    /// loses the two things an analysis needs: the order in which the lines came out and the
    /// interval between them. `Couldn't create z-pad instruction form (6)   (2153x)` says it
    /// happened two thousand times; it does not say it happened every 70 ms, nor what came
    /// before the first one.
    pub fn enable_serial(&mut self, serial_path: &std::path::Path) -> std::io::Result<()> {
        let file = std::fs::File::create(serial_path)?;
        self.serial = Some(std::io::BufWriter::new(file));
        Ok(())
    }

    /// Writes **only** to the serial capture, without going through the report.
    ///
    /// Instrumentation — classes created, databases opened, SQL — is tooling, not something the
    /// game said. Sending it through `record_debug` put it in the report's "game log", where it
    /// mixes with what the game actually printed and gets in the way of exactly whoever is reading
    /// to understand the game.
    pub(super) fn log_serial(&mut self, message: String) {
        let now = self.now_ms();
        if let Some(serial) = self.serial.as_mut() {
            use std::io::Write;
            let _ = writeln!(serial, "[{now:>9} ms] {message}");
            let _ = serial.flush();
        }
    }

    /// Keeps a log line, grouping repetitions instead of filling the report.
    ///
    /// With serial on, the line also goes raw to the file. See [`Machine::enable_serial`].
    pub(super) fn record_debug(&mut self, message: String) {
        self.find_calibration(&message);
        // The same clock the rest of the emulator reports: `clock_us` alone ignores the time
        // the instructions took, and the serial would lag behind the trace.
        let now = self.now_ms();
        if let Some(serial) = self.serial.as_mut() {
            use std::io::Write;
            // A write failure must not bring down the game: the serial is an instrument, not
            // emulation.
            let _ = writeln!(serial, "[{:>9} ms] {message}", now);
            // **Flushes on every line.** Without this the file stays empty while the session runs
            // — `BufWriter` only writes when it fills up or when it is dropped — and a capture that
            // only shows up after the game is closed is no use for following what is happening.
            // That is the tool's whole purpose.
            let _ = serial.flush();
        }
        if let Some(&index) = self.debug_index.get(&message) {
            self.debug_output[index].1 += 1;
            return;
        }
        // **With a cap.** A line carrying a changing number — frame, time, fps — is a new line
        // on every call, and the list grew for the whole session. Past the cap, the oldest half
        // goes: what matters in a log is what happened last.
        if self.debug_output.len() >= MAX_GAME_LINES {
            self.debug_output.drain(..MAX_GAME_LINES / 2);
            self.debug_index = self
                .debug_output
                .iter()
                .enumerate()
                .map(|(index, (text, _))| (text.clone(), index))
                .collect();
        }
        self.debug_index
            .insert(message.clone(), self.debug_output.len());
        self.debug_output.push((message, 1));
    }

    /// How many times each method was called, in order — the API backlog, measured.
    /// How many position reads found some axis off center. See the field.
    pub fn off_center_axis_reads(&self) -> u64 {
        self.off_center_axes
    }

    /// Which axes were seen off center, by the console's names: `X` and `Y` are the left
    /// stick's, `Z` and `RZ` the right one's.
    pub fn off_center_axes_seen(&self) -> Vec<&'static str> {
        crate::input::AXIS_NAMES
            .iter()
            .enumerate()
            .filter(|(index, _)| self.off_center_axes_mask & (1 << index) != 0)
            .map(|(_, name)| *name)
            .collect()
    }

    pub fn call_log(&self) -> Vec<(String, u64)> {
        self.calls
            .iter()
            .map(|(&(iface, slot), &count)| (aee::describe(aee::encode_raw(iface, slot)), count))
            .collect()
    }

    /// Invalid accesses the execution carried on past. See [`Machine::execute`].
    pub fn swallowed_faults(&self) -> Vec<String> {
        self.swallowed_failures.iter().cloned().collect()
    }

    /// The APIs that were missing during execution, including inside callbacks.
    /// What each widget class received in the accessor, by selector, in class order.
    ///
    /// It serves to answer "what does this proprietary class expect?" **from usage**: the widget
    /// family is served as a block (by numbering proximity), and the difference between its
    /// classes is not in any header — it is in what each one receives. `0x01028e19` is the case
    /// that motivated the instrument.
    pub fn enable_widget_census(&mut self) {
        self.widget_census = true;
    }

    pub fn selectors_per_class(&self) -> Vec<(u32, u32, u32)> {
        self.selectors_per_class
            .iter()
            .map(|(&(class_id, selector), &times)| (class_id, selector, times))
            .collect()
    }

    pub fn missing_apis(&self) -> Vec<String> {
        self.missing_apis.iter().cloned().collect()
    }

    /// How many live objects of each interface. See
    /// [`crate::brew::objects::ObjectStore::live_by_kind`].
    pub fn live_objects_by_kind(&self) -> Vec<(&'static str, usize)> {
        self.objects
            .live_by_kind()
            .into_iter()
            .map(|(iface, count)| (iface.name(), count))
            .collect()
    }

    /// Requested ClassIDs that we do not know how to instantiate yet.
    /// The calls to `IFileMgr`, in order, with the path and the return value.
    ///
    /// It exists because a game's error screen does not say **which** check failed: Double Dragon
    /// shows "Memory is insufficient" when any space or file query does not answer what it
    /// expects, and without this list there is no way to know what it asked.
    pub fn fs_log(&self) -> impl Iterator<Item = &String> {
        self.fs_log.iter()
    }

    /// The classes the game asked for, with how many times — known and unknown.
    ///
    /// The list of **unknown** ones is not enough to find an interface defect: Double Dragon asked
    /// for `AEECLSID_FONT_STANDARD*`, we answered "unknown class", and the symptom was an
    /// out-of-memory screen. After the fonts were served, the request left the list — and without
    /// this complete list there is no way to see that the class was served with the **wrong
    /// interface**.
    pub fn requested_classes(&self) -> Vec<(u32, u32)> {
        self.requested_classes.iter().map(|(c, n)| (*c, *n)).collect()
    }

    pub fn unknown_classes(&self) -> Vec<u32> {
        self.unknown_classes.iter().copied().collect()
    }

    /// How many instructions the guest executed.
    pub fn instructions(&self) -> u64 {
        self.cpu.instructions()
    }

    /// JIT entries and time spent inside it, when the backend measures.
    ///
    /// It serves to separate guest from dispatch: `calls` counts the APIs, and the wall-clock time
    /// that is **not** in here is trampoline plus method body. See [`Machine::api_profile`] for
    /// what happens outside, and the scan report for the ratio between the two.
    pub fn jit_report(&self) -> Option<(u64, u64, u64)> {
        self.cpu.jit_report()
    }

    /// How many frames the game presented through OpenGL.
    ///
    /// Only buffer swaps, because that is what the frame loop uses to know there is something
    /// new to show. For the window's indicator, see [`Machine::frames`].
    pub fn gl_swaps(&self) -> u32 {
        self.egl_swaps
    }

    /// How many frames the game drew, for whoever wants to show a rate.
    ///
    /// **Not every game presents by swapping buffers.** The Z-Wheel draws the stage into a pbuffer
    /// and takes the result through `eglGetColorBufferQUALCOMM` to composite it with the 2D: it
    /// never calls `eglSwapBuffers`, and while the rate counted only swaps the window's indicator
    /// read zero with the stage spinning on screen.
    ///
    /// Without a swap, what delimits a frame is the color `glClear` — the start of the next draw.
    /// Counting starts and counting ends gives the same rate, which is what the indicator shows.
    pub fn frames(&self) -> u32 {
        match self.egl_swaps {
            0 => self.gl_clears,
            swaps => swaps,
        }
    }

    /// The last frame the game presented through OpenGL, if there was one.
    ///
    /// It comes out apart from the screen because the two drawings coexist: `IDisplay` keeps
    /// painting on top between one `eglSwapBuffers` and the next, and in the end whoever wrote
    /// last is what shows. Seeing the OpenGL frame alone is what tells whether the 3D rendering is
    /// right.
    pub fn gl_frame(&mut self) -> Option<Framebuffer> {
        // Whoever asks for the OpenGL frame wants **the pixels**, so the pending readback
        // happens here: without this the diagnostics would show the previous frame.
        self.materialize_gl_frame();
        if self.gl_last_frame_words.is_empty() {
            return None;
        }
        let (width, height) = {
            let target = self.screen();
            (target.width(), target.height())
        };
        let mut surface = Framebuffer::new(width, height);
        surface.load_rgb565_words(&self.gl_last_frame_words);
        Some(surface)
    }

    /// How many drawing surfaces exist.
    pub fn bitmap_count(&self) -> usize {
        self.bitmaps.len()
    }

    /// Messages the game sent to the log, with the repetition count.
    pub fn debug_output(&self) -> &[(String, u64)] {
        &self.debug_output
    }

    /// Guesses in use in this run.
    pub fn assumptions(&self) -> Vec<&'static str> {
        self.assumptions.iter().copied().collect()
    }

    /// `this` pointers that do not match the interface called.
    pub fn suspicious_objects(&self) -> Vec<(u32, u32)> {
        self.suspicious_objects.iter().copied().collect()
    }

    pub fn live_objects(&self) -> usize {
        self.objects.live_count()
    }

    /// API calls served since the game opened.
    pub fn api_calls(&self) -> u64 {
        self.calls_total
    }
}
