//! Probing: finding out what an unknown class answers.

use super::*;

impl<C: CpuBackend> Machine<C> {
    /// Serves a call on a probe object: logs it and answers `SUCCESS`.
    ///
    /// Answering success to everything is deliberate. The probe does not try to get the behavior
    /// right — it tries to **keep the game moving** to see what it asks for next. An honest
    /// `EFAILED` would stop the investigation at the first call.
    pub(super) fn probe_call(&mut self, slot: u32) -> Result<u32, CpuError> {
        let this = self.cpu.read_reg(Reg::R0);
        let clsid = self.probe_objects.get(&this).copied().unwrap_or(0);
        // Logs **every** slot, `AddRef` and `Release` included: knowing that the game only
        // created and released the object is as useful an answer as knowing it called slot 7.
        let args = self.args();
        // The count is what separates "the app called this" from "the app is stuck on this": a
        // method with millions of calls is a loop against one of our answers, not normal use.
        if let Some(entry) = self
            .probe_log
            .iter_mut()
            .find(|(c, o, s, _, _, _)| *c == clsid && *o == this && *s == slot)
        {
            entry.5 += 1;
        } else {
            // The argument that points to readable text is almost always the interesting one —
            // the database name, the SQL statement. Reading it here saves having to find out
            // where the module was mapped in order to fetch it from the file.
            let texts = args.map(|arg| self.probe_text(arg));
            self.probe_log.push((clsid, this, slot, args, texts, 1));
        }
        match slot {
            // `AddRef` and `Release` are the first two in every BREW interface, and the count
            // has to hold: without it the object dies or leaks in the middle of the observation.
            0 => Ok(self.objects.add_ref(this)),
            1 => {
                let remaining = self.objects.release(this);
                if remaining == 0 {
                    self.probe_objects.remove(&this);
                }
                Ok(remaining)
            }
            _ => {
                // A method that returns an object writes the pointer into an output argument,
                // and returning `SUCCESS` without writing anything makes the game carry on with
                // garbage and die on first use — that is what the Z-Wheel did. So the probe
                // **hands out another probe** in whatever looks like an output pointer: that way
                // the game continues and the observation reaches the whole family of objects,
                // not just the first.
                for arg in args {
                    if self.looks_like_out_pointer(arg) {
                        let child = self.new_object(Interface::Probe)?;
                        if child != 0 {
                            self.probe_objects.insert(child, clsid);
                            self.cpu.write_u32(arg, child)?;
                        }
                        break;
                    }
                }
                // The preset answer replaces only the **return value**; handing the object out
                // through the output pointer still applies. Merging the two has already cost me
                // an investigation: I preset "answer 1" and the game got 1 with an empty output
                // pointer, which is a state that exists nowhere.
                Ok(self
                    .probe_answers
                    .get(&(clsid, slot))
                    .copied()
                    .unwrap_or(SUCCESS))
            }
        }
    }

    /// The text at `addr`, when what is there really is text.
    ///
    /// Requires starting with a printable character and having at least two of them before the
    /// zero: with fewer than that, any small integer would become a "string" and the log would
    /// be nothing but noise.
    pub(super) fn probe_text(&self, addr: u32) -> Option<String> {
        if addr == 0 {
            return None;
        }
        let text = self.cpu.read_cstring(addr, 120);
        let readable = text.len() >= 2
            && text
                .chars()
                .all(|c| c == '\n' || c == '\t' || (' '..='~').contains(&c));
        readable.then_some(text)
    }

    /// Whether `addr` looks like an output pointer: aligned, in the game's memory and holding
    /// zero.
    ///
    /// The zero requirement is what makes this safe to use: an argument that already points to
    /// something is not an output destination, and writing to it would ruin game data.
    pub(super) fn looks_like_out_pointer(&self, addr: u32) -> bool {
        let in_memory = (loader::HEAP_BASE..loader::HEAP_BASE + loader::HEAP_SIZE as u32)
            .contains(&addr)
            || (loader::STACK_BASE..loader::STACK_BASE + loader::STACK_SIZE as u32).contains(&addr);
        in_memory && addr.is_multiple_of(4) && self.cpu.read_u32(addr).unwrap_or(1) == 0
    }

    /// Has these classes served with a probe object instead of refusing them.
    pub fn probe_classes(&mut self, classes: &[u32]) {
        self.probe_classes.extend(classes);
    }

    /// Presets the answer of a probe slot, to explore the other side of a branch.
    pub fn probe_answer(&mut self, clsid: u32, slot: u32, value: u32) {
        self.probe_classes.insert(clsid);
        self.probe_answers.insert((clsid, slot), value);
    }

    /// What the games called on the probes, in the order it appeared.
    pub fn probe_log(&self) -> &[ProbeCall] {
        &self.probe_log
    }
}
