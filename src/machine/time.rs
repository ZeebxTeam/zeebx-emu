//! The clock: BREW time, vsync and the idle skip.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Recognizes the game's busy wait and makes time pass through it.
    ///
    /// Zeebo Sports Peteca does not arm a timer for the next frame: it sits in a loop that reads
    /// the clock and yields until the deadline arrives — 69 thousand reads per frame, 14 million
    /// instructions spent just waiting. On the console this costs nothing, because time passes
    /// on its own while the ARM spins; here every iteration is really emulated, and the game ran
    /// six times slower than the device.
    ///
    /// When the game only reads the clock and yields, it is not making progress — it is waiting,
    /// and advancing the clock is exactly what the console does with the passage of real time.
    /// Any other call is a sign of work and resets the count, so a frame that reads the clock in
    /// the middle of what it does still costs what it costs.
    pub(super) fn note_spin(&mut self, iface: Interface, slot: u32) {
        let name = iface.method(slot).unwrap_or("");
        let reads_clock = iface == Interface::Helpers
            && matches!(name, "aee_GetTimeMS" | "aee_GetUpTimeMS" | "aee_GetSeconds");
        // Yielding is neither work nor waiting: it is how the loop goes around. It does not count
        // toward the threshold, but it does not undo the count either.
        let yields = matches!(
            (iface, name),
            (Interface::Thread, "Suspend" | "GetResumeCBK") | (Interface::Shell, "Resume")
        );
        // **An iteration of a wait loop is not just the clock read.** Zeebo Extreme Rolima spins
        // around five calls per iteration — clock, `memset`, controller position and two
        // controller events —, 1.35 million of each in six seconds of play: 2.45 **billion**
        // emulated instructions to advance nothing, and the game at 79% of the console's speed.
        // While any of them reset the count, the shortcut never kicked in.
        //
        // They are neutral and do not reset the count. **They do not count as waiting on their
        // own**: the only thing that increments is still the clock read, and that is what keeps
        // the shortcut tied to a loop that asks for the time. A game that only clears memory never
        // gets here.
        let neutral = matches!(
            (iface, name),
            (Interface::Helpers, "memset")
                | (
                    Interface::HidDevice,
                    "GetPositionState" | "GetNextButtonEvent"
                )
                | (Interface::Hid, "GetNextConnectEvent")
        );
        if !reads_clock {
            if !yields && !neutral {
                self.spin_polls = 0;
            }
            return;
        }
        self.spin_polls += 1;
        if self.spin_polls > SPIN_THRESHOLD {
            self.clock_us += SPIN_STEP_US;
        }
    }

    /// Advances the clock to the next timer, when there is nothing else to do right now.
    ///
    /// It is what the console does: with no pending work it sleeps, and wakes up at the deadline.
    /// Skipping that stretch is what keeps virtual time honest — the game sees exactly the
    /// interval it asked for between two frames, not what the emulator took to get there.
    pub(super) fn skip_idle_time(&mut self) {
        let busy = !self.pending_calls.is_empty()
            || !self.pending_threads.is_empty()
            || !self.pending_probes.is_empty()
            || !self.pending_blits.is_empty()
            || !self.pending_surface_blits.is_empty()
            || !self.image_notify_queue.is_empty()
            || !self.pending_signals.is_empty();
        if busy {
            return;
        }
        let now = self.now_ms();
        let Some(next) = self.timers.iter().map(|timer| timer.deadline_ms).min() else {
            return;
        };
        // The skip is **one frame**, not the whole gap. Jumping to the nearest timer is right
        // when the game is waiting for the next frame, and that is what this shortcut was born
        // for; but when the only armed timer is long, the gap is not idleness — it is the game
        // waiting for someone to press a button. The Z-Wheel arms a 120 000 ms inactivity timer on
        // the welcome screen: skipping the whole gap sent the clock two minutes ahead at once and
        // fired the idle timeout before any key had a chance to arrive.
        let vao = next.saturating_sub(now) as u64 * 1000;
        self.clock_us += vao.min(VSYNC_PERIOD_US);
    }

    /// The guest's virtual clock, in milliseconds.
    pub fn clock_ms(&self) -> u32 {
        self.now_ms()
    }

    /// The same clock in microseconds. The achievements count 60 Hz frames by it.
    pub fn clock_us(&self) -> u64 {
        self.now_us()
    }

    /// Advances the clock without executing anything, to test whoever counts time.
    #[cfg(all(test, feature = "achievements"))]
    pub(crate) fn advance_clock_us(&mut self, us: u64) {
        self.clock_us += us;
    }

    /// The clock the guest sees: the time advanced by the frame loop plus the time the guest
    /// itself spent executing instructions.
    ///
    /// The two terms measure different things and so they add up. The first is the idle time
    /// the emulator skips between frames — on the console it would really pass. The second makes
    /// the clock move *during* an execution slice, and without it a game's wait loop ("stay here
    /// until 1.5 s have passed") would never end: it reads the clock, advances nothing and reads
    /// it again.
    ///
    /// Execution time comes from the instruction count, not from the host clock: that way two
    /// runs of the same ROM give exactly the same result, which is what makes it possible to
    /// compare two frames and know that the difference was the change we made.
    pub(super) fn now_ms(&self) -> u32 {
        (self.now_us() / 1000) as u32
    }

    /// The same clock, in microseconds — the resolution at which it is kept.
    ///
    /// Milliseconds are not enough: a frame at 60 Hz lasts 16.67 ms, and rounding that every
    /// frame would accumulate an error of several percent.
    pub(super) fn now_us(&self) -> u64 {
        self.clock_us + self.cpu.instructions() / INSTRUCTIONS_PER_US
    }

    /// Waits for the vertical retrace, as the console's `eglSwapBuffers` does.
    ///
    /// If the frame took longer than one period, the next retrace is the first one that has not
    /// passed yet — missing a retrace costs the whole frame, and that is how it is on the device
    /// too.
    pub(super) fn wait_for_vsync(&mut self) {
        let now = self.now_us();
        if self.next_vsync_us <= now {
            let missed = (now - self.next_vsync_us) / VSYNC_PERIOD_US + 1;
            self.next_vsync_us += missed * VSYNC_PERIOD_US;
        }
        self.clock_us += self.next_vsync_us - now;
        self.next_vsync_us += VSYNC_PERIOD_US;
    }

    /// Whether there is nothing left for the guest to do: no armed timer, no queued callback, no
    /// thread waiting for its turn and no registered interest in input.
    ///
    /// The frame loop stops here. Looking only at the timers was not enough: when the game moves
    /// to living inside a cooperative thread — as Quake does after loading the map —, it cancels
    /// the timer and the loop ended with the game halfway through.
    pub fn is_idle(&self) -> bool {
        self.timers.is_empty()
            && self.pending_calls.is_empty()
            && self.pending_threads.is_empty()
            && self.pending_probes.is_empty()
            && self.pending_blits.is_empty()
            && self.pending_surface_blits.is_empty()
            && self.image_notify_queue.is_empty()
            && self.pending_signals.is_empty()
            // **A game that registered interest in input is waiting for the player.** Calling that
            // idle ended the session in the middle of the title screen: Crash Nitro Kart stopped
            // at 19.6 s, exactly when the intro ends and the game starts waiting for "press start"
            // — with no armed timer and the input callback registered, it was "idle" as far as
            // this function was concerned. On the console the game stays on the screen waiting;
            // here too.
            && self.input_signals.is_empty()
    }

    /// How many timers are armed.
    pub fn armed_timers(&self) -> usize {
        self.timers.len()
    }

    /// The guest's calendar, in seconds since January 6, 1980 GMT.
    ///
    /// The date comes from the host clock, captured **once** when the machine is created, and
    /// from then on it moves with the virtual clock. It is the middle ground between the two
    /// things the project wants: a game that asks for the date gets one that exists, and the
    /// time it measures is still the virtual one, independent of how long the emulator took.
    pub(super) fn brew_seconds(&self) -> u32 {
        self.epoch_seconds + self.elapsed_ms() / 1000
    }

    /// Milliseconds since the start of execution.
    pub(super) fn elapsed_ms(&self) -> u32 {
        self.now_ms()
    }
}
