//! ISignal and the callbacks: what the game asked to be notified of.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Runs the pending callbacks at the boundary between two API calls.
    ///
    /// BREW delivers notifications through its event loop, once the applet has handed back
    /// control. Our closest equivalent is this point: the API call has finished, the result is
    /// already in `r0` and the guest has not resumed yet. All the context is saved and restored,
    /// so that the caller does not notice the detour.
    pub(super) fn run_pending_callbacks(&mut self, budget: u64) -> Result<(), CpuError> {
        self.poll_media()?;
        let idle = self.pending_calls.is_empty()
            && self.pending_probes.is_empty()
            && self.pending_blits.is_empty()
            && self.pending_surface_blits.is_empty();
        if idle || self.nesting >= MAX_NESTING {
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        for _ in 0..CALLBACK_ROUNDS {
            for target in std::mem::take(&mut self.pending_probes) {
                self.probe_foreign_surface(target, budget)?;
            }
            for blit in std::mem::take(&mut self.pending_blits) {
                self.blit_into_foreign(blit, budget)?;
            }
            for blit in std::mem::take(&mut self.pending_surface_blits) {
                self.blit_surface_into_foreign(blit, budget)?;
            }
            let pending = std::mem::take(&mut self.pending_calls);
            if pending.is_empty() {
                break;
            }
            for call in pending {
                if call.function != 0 {
                    self.call_guest(call.function, call.args, budget)?;
                }
            }
        }
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        Ok(())
    }

    /// Finds the function to call behind a `(pfn, pUser)` pair.
    ///
    /// The SDK's `ISHELL_SetTimerEx` is a macro over `SetTimer`, and it passes **the same
    /// pointer** in both arguments:
    ///
    /// ```c
    /// #define ISHELL_SetTimerEx(p,s,pcb) \
    ///     GET_PVTBL(p,IShell)->SetTimer(p, s, (PFNNOTIFY)(void *)pcb, (void *)pcb)
    /// ```
    ///
    /// When the two are equal, what arrived is an `AEECallback *`, and what should be called is
    /// inside it: `pfnNotify` at offset 16 and `pNotifyData` at 20 (`inc/AEECallback.h`).
    /// A real `pfn` never coincides with its `pUser` — one is code, the other is data.
    pub(super) fn resolve_notify(&mut self, callback: Callback) -> Result<Callback, CpuError> {
        if callback.function == 0 || callback.function != callback.context {
            return Ok(callback);
        }
        let base = callback.function;
        Ok(Callback {
            function: self.cpu.read_u32(base + 16).unwrap_or(0),
            context: self.cpu.read_u32(base + 20).unwrap_or(0),
        })
    }

    /// Delivers the fired signals, calling the guest's callbacks.
    ///
    /// Must run outside the dispatch of a call, when the guest is not in the middle of
    /// another — hence the queue.
    pub fn deliver_signals(&mut self, budget: u64) -> Result<Vec<Outcome>, CpuError> {
        // The network response shares this boundary for the same reason as the signals:
        // delivering it requires calling the game's allocator, and that is only safe outside
        // dispatch.
        self.flush_response()?;
        // **The UI is redrawn per frame, not per loop turn.** See [`Machine::last_draw_us`]: the
        // drawing of an `OwnerDrawWidget` is game code, and calling it more often than the
        // device would is work nobody asked for.
        let now = self.now_us();
        if now.saturating_sub(self.last_draw_us) >= VSYNC_PERIOD_US {
            self.last_draw_us = now;
            self.paint_widgets()?;
            self.draw_widgets()?;
        }
        self.deliver_activation()?;
        self.start_animation()?;
        self.skip_wheel_instructions()?;
        self.flush_keys()?;
        self.boomerang_report();
        // The sound buffers delivered on this turn are read now, after the game's handler has
        // finished writing them.
        self.resolve_pending_media()?;
        // This turn's `IMedia` notifications: see [`Machine::notify_media`].
        self.poll_media()?;
        self.deliver_media_notifies(budget)?;
        self.pump_pcm_streams(budget)?;
        self.pump_live_buffers()?;
        let mut outcomes = Vec::new();
        // See [`Machine::notify_image`]. A notification born inside another handler waits for
        // the next turn, as on the device.
        for image in std::mem::take(&mut self.image_notify_queue) {
            outcomes.extend(self.deliver_image_notify(image, budget)?);
        }
        let pending = std::mem::take(&mut self.pending_signals);
        for callback in pending {
            if callback.function == 0 {
                continue;
            }
            outcomes.push(self.call_guest(
                callback.function,
                [callback.context, 0, 0, 0],
                budget,
            )?);
        }
        Ok(outcomes)
    }

    /// Delivers the queued callbacks (sound, image, whatever comes).
    ///
    /// Same reason as the signal queue: they run in the guest, and the guest cannot be
    /// reentered in the middle of dispatching one of its calls.
    pub fn deliver_callbacks(&mut self, budget: u64) -> Result<Vec<Outcome>, CpuError> {
        let mut outcomes = Vec::new();
        // A callback can queue another, so drain until empty — with a cap, so that a cycle
        // between callbacks does not lock up the emulator.
        for _ in 0..CALLBACK_ROUNDS {
            let pending = std::mem::take(&mut self.pending_calls);
            if pending.is_empty() {
                break;
            }
            for call in pending {
                if call.function == 0 {
                    continue;
                }
                outcomes.push(self.call_guest(call.function, call.args, budget)?);
            }
        }
        Ok(outcomes)
    }

    /// `ISignal`, `ISignalCtl` and `ISignalCBFactory`.
    ///
    /// A signal is the "something happened" notice. The app creates one through the factory,
    /// passing a callback function and a context, and hands the `ISignal` to whoever will
    /// notify it — the gamepad, for example, via `RegisterForButtonEvent`. When the system calls
    /// `ISIGNAL_Set`, the app's callback runs.
    ///
    /// Here firing is deferred instead of immediate: calling the guest from inside the dispatch
    /// of a guest call would be reentrancy, and BREW does not fire right away either — it
    /// schedules it for the app's event loop. Pending signals stay recorded until we have that
    /// loop.
    pub(super) fn signal_call(
        &mut self,
        iface: Interface,
        slot: u32,
    ) -> Result<Option<u32>, CpuError> {
        let Some(name) = iface.method(slot) else {
            return Ok(None);
        };
        let this = self.cpu.read_reg(Reg::R0);
        let (a1, a2, a3) = (
            self.cpu.read_reg(Reg::R1),
            self.cpu.read_reg(Reg::R2),
            self.cpu.read_reg(Reg::R3),
        );
        let result = match name {
            "AddRef" => self.objects.add_ref(this),
            "Release" => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.signals.remove(&this);
                    // A released signal cannot stay registered for input: the address comes
                    // back as another signal, and a press would fire that one's callback.
                    self.input_signals.retain(|_, sign| *sign != this);
                }
                remaining
            }
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                SUCCESS
            }
            // int CreateSignal(ISignalCBFactory *, void (*pfn)(void *pCx), void *pCx,
            //                  ISignal **ppiSig, ISignalCtl **ppiSigCtl)
            "CreateSignal" => {
                let signal = self.new_object(Interface::Signal)?;
                let control = self.new_object(Interface::SignalCtl)?;
                if signal == 0 || control == 0 {
                    return Ok(Some(ENOMEMORY));
                }
                // Both objects refer to the same signal, so they share the callback.
                let callback = Callback {
                    function: a1,
                    context: a2,
                };
                self.signals.insert(signal, callback);
                self.signals.insert(control, callback);
                if a3 != 0 {
                    self.cpu.write_u32(a3, signal)?;
                }
                let out_control = self.stack_arg(0)?;
                if out_control != 0 {
                    self.cpu.write_u32(out_control, control)?;
                }
                SUCCESS
            }
            "Set" => {
                if let Some(&callback) = self.signals.get(&this) {
                    self.pending_signals.push(callback);
                }
                SUCCESS
            }
            // `Enable` rearms the signal and `Detach` disconnects it from the source. With no
            // event loop yet, both are just state bookkeeping.
            "Enable" => SUCCESS,
            "Detach" => {
                self.signals.remove(&this);
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Delivers an event to the running applet, and returns what it answered.
    ///
    /// There is only one applet here, so a `cls` other than its own is an event for someone who
    /// does not exist — and the right answer in that case is "nobody handled it", which is what
    /// BREW answers.
    ///
    /// Calling the guest from here is reentrancy, with the same care as `qsort` and the delivery
    /// of SQL rows: save the registers, respect the nesting cap, restore everything.
    pub(super) fn send_applet_event(
        &mut self,
        cls: u32,
        evt: u32,
        w: u16,
        dw: u32,
    ) -> Result<u32, CpuError> {
        // `current_applet` is only filled in when `EVT_APP_START` is dispatched, and there are
        // events before that: the Z-Wheel builds its preferences database **during the
        // construction** of the applet, and to do so it sends an event to its own class. At
        // that moment the object already exists — `AEEApplet_New` wrote the output pointer
        // before the game's code ran —, so reading it from there is what the console does: for
        // the shell, the applet exists once it is registered, not once it is started.
        //
        // Without this the event came back "nobody handled it", and the Z-Wheel printed
        // `SendEvent to get PrefsDB failed` eleven times in a row before giving up on the whole
        // configuration.
        let applet = match self.current_applet {
            0 => self.cpu.read_u32(self.module.out_module + 4).unwrap_or(0),
            live => live,
        };
        if applet == 0 || (cls != 0 && cls != self.applet_class) {
            return Ok(FALSE);
        }
        if self.nesting >= MAX_NESTING {
            self.assumptions
                .insert("an applet event was not delivered because of deep nesting");
            return Ok(FALSE);
        }
        let vtable = self.cpu.read_u32(applet)?;
        let handle_event = self.cpu.read_u32(vtable + 2 * 4)?;
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        self.nesting += 1;
        let outcome = self.call_guest(handle_event, [applet, evt, u32::from(w), dw], QSORT_BUDGET);
        self.nesting -= 1;
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        Ok(match outcome? {
            Outcome::Returned { code } => code,
            // A handler that goes astray does not take down whoever sent the event: for the
            // sender, nobody handled it.
            _ => FALSE,
        })
    }
}
