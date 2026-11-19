//! IThread, IQueue and the BREW heap: the game's cooperation and memory.

use super::*;

/// What goes in the bytes between the end of what the game asked `MALLOC` for and the next
/// block.
///
/// Every block gets at least one of these bytes. Um Jogo de Ovos loads each scene with an exact
/// `MALLOC(size)` and its parser reads the byte **after** the end: if it is zero, it rejects
/// every token, prints "Error loading cutscene" once per character and the boss stage does not
/// start. On the device that byte is not zero — the game shipped like that —, and in our heap,
/// with the blocks packed together and the padding zeroed, it was.
///
/// **Why a newline, and not just any pattern.** Alpine Racer also reads past the end of a text
/// buffer, but its read stops at the first separator: with `0xaa`, it appended two `ª` to a
/// field before stopping. With `\n` (and with a space), its call sequence in the first seven
/// seconds is the same as when there was a zero there. Between the two, the newline also stops
/// whoever reads to the end of the line. The device's heap layout has not been checked.
pub(super) const BLOCK_PADDING: u8 = b'\n';

impl<C: CpuBackend> Machine<C> {
    /// `IThread` (`AEECLSID_THREAD` = `0x01001017`), from `sdk/inc/AEEThread.h`.
    ///
    /// The BREW thread is cooperative and relies on callbacks: it runs until it calls `Suspend`,
    /// and comes back when the `AEECallback` from `GetResumeCBK` fires — typically from the game
    /// itself, via `ISHELL_Resume`. Here that becomes saving and restoring registers, with one
    /// stack per thread allocated on the guest heap.
    pub(super) fn thread_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Thread.method(slot) else {
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
                    if let Some(state) = self.threads.remove(&this) {
                        self.resume_callbacks.remove(&state.resume_cb);
                        self.free(state.resume_cb);
                        self.free(state.stack);
                    }
                }
                remaining
            }
            "QueryInterface" => {
                if a2 != 0 {
                    self.cpu.write_u32(a2, this)?;
                }
                SUCCESS
            }
            "Malloc" => self.malloc(a1)?,
            "Free" => {
                self.free(a1);
                SUCCESS
            }
            // The resource pool exists to release everything together at the end of the thread;
            // our heap already outlives the thread, so holding and releasing changes nothing.
            "HoldRsc" => SUCCESS,
            "ReleaseRsc" => SUCCESS,
            // int Start(IThread *, int nStackSz, PFNTHREAD pfStart, void *pvStart)
            "Start" => {
                let state = self.threads.entry(this).or_default();
                if state.started {
                    EALREADY
                } else {
                    let stack = self.alloc_internal(a1.max(THREAD_MIN_STACK))?;
                    if stack == 0 {
                        ENOMEMORY
                    } else {
                        // The ARM stack grows downward, so the top of the block is the initial
                        // `sp`; the AAPCS asks for 8-byte alignment.
                        let top = (stack + a1.max(THREAD_MIN_STACK)) & !7;
                        let state = self.threads.entry(this).or_default();
                        state.started = true;
                        state.stack = stack;
                        state.resume_pc = a2;
                        state.context = [0; 14];
                        state.context[0] = this;
                        state.context[1] = a3;
                        state.context[13] = top;
                        self.pending_threads.push(this);
                        SUCCESS
                    }
                }
            }
            // int Exit(IThread *, int nRv) — the thread is done; control returns to whoever
            // resumed it.
            "Exit" => {
                self.finish_thread(this, a1)?;
                self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
                SUCCESS
            }
            // void Join(IThread *, AEECallback *pcb, int *pnRv)
            "Join" => {
                let call = self.resolve_notify(Callback {
                    function: a1,
                    context: a1,
                })?;
                let state = self.threads.entry(this).or_default();
                if state.finished {
                    let (code, joiner) = (state.exit_code, call);
                    if a2 != 0 {
                        self.cpu.write_u32(a2, code)?;
                    }
                    self.queue_call(joiner);
                } else {
                    state.joiners.push((call, a2));
                }
                SUCCESS
            }
            // void Suspend(IThread *) — the only point where the thread gives control back.
            //
            // We save the registers as they are and swap the return address for the sentinel:
            // the execution loop will stop there, and whoever resumes continues at the `lr` we
            // stored.
            "Suspend" => {
                let resume_pc = self.cpu.read_reg(Reg::Lr);
                let context = THREAD_REGS.map(|reg| self.cpu.read_reg(reg));
                let state = self.threads.entry(this).or_default();
                state.resume_pc = resume_pc;
                state.context = context;
                state.suspended = true;
                self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
                SUCCESS
            }
            // AEECallback *GetResumeCBK(IThread *) — the same callback on every call, because it
            // is how we recognize an `ISHELL_Resume` aimed at the thread.
            "GetResumeCBK" => {
                let existing = self.threads.entry(this).or_default().resume_cb;
                if existing != 0 {
                    existing
                } else {
                    let cb = self.alloc_internal(CALLBACK_SIZE)?;
                    self.threads.entry(this).or_default().resume_cb = cb;
                    if cb != 0 {
                        self.resume_callbacks.insert(cb, this);
                    }
                    cb
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// Ends a thread: frees the stack and releases whoever was waiting for it.
    pub(super) fn finish_thread(&mut self, thread: u32, code: u32) -> Result<(), CpuError> {
        let Some(state) = self.threads.get_mut(&thread) else {
            return Ok(());
        };
        state.finished = true;
        state.exit_code = code;
        let stack = std::mem::take(&mut state.stack);
        let joiners = std::mem::take(&mut state.joiners);
        self.free(stack);
        self.pending_threads.retain(|&t| t != thread);
        for (call, out) in joiners {
            if out != 0 {
                self.cpu.write_u32(out, code)?;
            }
            self.queue_call(call);
        }
        Ok(())
    }

    /// Gives a turn to each thread that asked to come back.
    ///
    /// One turn per frame, and only from here. A cooperative thread yields control expecting to
    /// be resumed on the next pass of the event loop, and this emulator's event loop is the frame
    /// loop — also resuming it at every boundary between API calls would make the game run dozens
    /// of internal frames for each of ours.
    pub(super) fn run_pending_threads(&mut self, budget: u64) -> Result<(), CpuError> {
        if self.pending_threads.is_empty() || self.current_thread.is_some() {
            return Ok(());
        }
        let saved = SAVED_REGS.map(|reg| self.cpu.read_reg(reg));
        for thread in std::mem::take(&mut self.pending_threads) {
            self.resume_thread(thread, budget)?;
        }
        for (reg, value) in SAVED_REGS.iter().zip(saved) {
            self.cpu.write_reg(*reg, value);
        }
        Ok(())
    }

    /// Resumes a thread from the point where it stopped.
    pub(super) fn resume_thread(&mut self, thread: u32, budget: u64) -> Result<(), CpuError> {
        let Some(state) = self.threads.get(&thread) else {
            return Ok(());
        };
        if state.finished {
            return Ok(());
        }
        let (context, pc) = (state.context, state.resume_pc);
        for (reg, value) in THREAD_REGS.iter().zip(context) {
            self.cpu.write_reg(*reg, value);
        }
        self.cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        self.threads.entry(thread).or_default().suspended = false;
        self.current_thread = Some(thread);
        let outcome = self.execute(pc, budget);
        self.current_thread = None;
        // Coming back without having gone through `Suspend` means the entry function returned:
        // the thread is done, even without `Exit`.
        match outcome? {
            Outcome::Returned { code } => {
                let ceded = self
                    .threads
                    .get(&thread)
                    .is_some_and(|t| t.suspended || t.finished);
                if !ceded {
                    self.finish_thread(thread, code)?;
                }
            }
            // An outcome that is not a return — missing API, memory fault — must reach the
            // report: the thread is where the game spends most of its time.
            other => self.stalled = Some(other),
        }
        Ok(())
    }

    /// Queues a guest callback for the next boundary between calls.
    pub(super) fn queue_call(&mut self, call: Callback) {
        if call.function != 0 {
            self.pending_calls.push(GuestCall {
                function: call.function,
                args: [call.context, 0, 0, 0],
            });
        }
    }

    /// `IHeap` (`AEECLSID_HEAP` = `0x01001002`), from `sdk/inc/AEEHeap.h`.
    ///
    /// Allocation is the same as the helpers' `MALLOC` — it is the same guest heap, just reached
    /// through another door. What was really missing was `CheckAvail`: Double Dragon asks whether
    /// what it wants fits before starting and, with no one to answer, drew "Memory is
    /// insufficient" instead of the game.
    pub(super) fn heap_call(&mut self, slot: u32) -> Result<Option<u32>, CpuError> {
        let Some(name) = Interface::Heap.method(slot) else {
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
            "Release" => self.objects.release(this),
            "Malloc" => self.malloc(a1 & !ALLOC_NO_ZMEM)?,
            "Realloc" => self.realloc(a1, a2 & !ALLOC_NO_ZMEM)?,
            "Free" => {
                self.free(a1);
                SUCCESS
            }
            "StrDup" => {
                // `AECHAR` is UTF-16: the copy carries the terminator along.
                let units = self.read_aechar_units(a1)?;
                let bytes: Vec<u8> = units
                    .iter()
                    .chain(std::iter::once(&0))
                    .flat_map(|u| u.to_le_bytes())
                    .collect();
                let ptr = self.malloc(bytes.len() as u32)?;
                if ptr != 0 {
                    self.cpu.write_mem(ptr, &bytes)?;
                }
                ptr
            }
            // `boolean`: it fits if the guest heap still has that much free.
            "CheckAvail" => {
                let fits = self.heap_available() >= a1 as u64;
                if !fits {
                    // A "does not fit" is the trigger for the "Memory is insufficient" screen in
                    // more than one game, and without recording the request there is no way to
                    // know how much it wanted.
                    self.refused_checks
                        .insert((a1, self.cpu.read_reg(Reg::Lr)));
                }
                u32::from(fits)
            }
            // The documentation is explicit: what comes out of here is the total **in use**, and
            // the device's total comes from `ISHELL_GetDeviceInfo`.
            "GetMemStats" => self.heap.used(),
            "GetModuleMemStats" => {
                // The peak is not tracked; returning the current usage in both is the closest
                // honest answer, and it is what the game compares against what it needs.
                let used = self.heap.used();
                self.write_at(a2, used)?;
                self.write_at(a3, used)?;
                SUCCESS
            }
            _ => return Ok(None),
        };
        Ok(Some(result))
    }

    /// How many bytes the guest heap can still hand out.
    pub(super) fn heap_available(&self) -> u64 {
        // **What a single allocation can obtain**, not the sum of what is left: see
        // [`Heap::largest_block`]. The difference rejected the Z-Wheel's request, which asks for
        // the free amount and requests exactly that in a single allocation.
        u64::from(self.heap.largest_block())
    }

    /// `realloc`: allocates the new size and copies the old contents.
    pub(super) fn realloc(&mut self, ptr: u32, size: u32) -> Result<u32, CpuError> {
        if ptr == 0 {
            return self.malloc(size);
        }
        // Size zero is `FREE`: the block goes away and the pointer becomes null. Handing out a
        // new eight-byte block left `ERR_REALLOC(0, &p)` with a non-null `p`, and a game that
        // relies on the null never gave it back.
        if size == 0 {
            self.free(ptr);
            return Ok(0);
        }
        let old_size = self.block_size(ptr).unwrap_or(0);
        let new_ptr = self.malloc(size)?;
        // Without space, the old block stays as it was: that is what `realloc` promises, and the
        // game that checks for null still has its data.
        if new_ptr == 0 {
            return Ok(0);
        }
        if old_size > 0 {
            self.copy_guest(new_ptr, ptr, old_size.min(size))?;
        }
        self.free(ptr);
        Ok(new_ptr)
    }

    /// BREW's `MALLOC`: returns memory, or 0 if there is no space.
    ///
    /// The size may come with `ALLOC_NO_ZMEM` (`0x80000000`) in the high bit, asking for memory
    /// **without** zeroing — that is what the SDK libc's `malloc` does
    /// (`MALLOC(size|ALLOC_NO_ZMEM)`). Ignoring that flag makes the emulator try to allocate two
    /// gigabytes and return null, and the game concludes, correctly, that it ran out of memory.
    pub(super) fn malloc(&mut self, size: u32) -> Result<u32, CpuError> {
        let zero = size & ALLOC_NO_ZMEM == 0;
        let size = size & !ALLOC_NO_ZMEM;
        // One extra byte, which becomes the slack after the block: see `BLOCK_PADDING`.
        let Some(addr) = self.heap.alloc(size.saturating_add(1)) else {
            // Recorded: a silent refusal turns into "the game does not work" with no clue at all
            // in the report. `lr` says who asked.
            self.refused_allocs
                .insert((size, self.cpu.read_reg(Reg::Lr)));
            return Ok(0);
        };
        // Reused blocks carry garbage from the previous owner; we zero them when asked.
        if zero && size > 0 {
            self.cpu.write_mem(addr, &vec![0u8; size as usize])?;
        }
        let block = self.heap.size_of(addr).unwrap_or(size);
        self.cpu
            .write_mem(addr + size, &vec![BLOCK_PADDING; (block - size) as usize])?;
        Ok(addr)
    }

    /// Memory for the emulator itself, in the internal region and zeroed: see
    /// [`loader::INTERNAL_BASE`]. Returns 0 if the region is full, like `malloc`.
    pub(super) fn alloc_internal(&mut self, size: u32) -> Result<u32, CpuError> {
        let Some(addr) = self.internal_heap.alloc(size) else {
            self.assumptions
                .insert("the emulator's internal region filled up");
            return Ok(0);
        };
        if size > 0 {
            self.cpu.write_mem(addr, &vec![0u8; size as usize])?;
        }
        Ok(addr)
    }

    /// Returns a block to the allocator it came from, by address.
    ///
    /// Every `free` goes through here, including the game's: one of our blocks can reach it (and
    /// a save state from before the internal region carries ours inside the heap).
    pub(super) fn free(&mut self, ptr: u32) -> Option<u32> {
        if loader::is_internal(ptr) {
            self.internal_heap.free(ptr)
        } else {
            self.heap.free(ptr)
        }
    }

    /// Size of a live block, in either of the two allocators.
    pub(super) fn block_size(&self, ptr: u32) -> Option<u32> {
        if loader::is_internal(ptr) {
            self.internal_heap.size_of(ptr)
        } else {
            self.heap.size_of(ptr)
        }
    }

    /// The allocations the heap refused, with the size and who asked.
    pub fn refused_allocations(&self) -> Vec<(u32, u32)> {
        self.refused_allocs.iter().copied().collect()
    }

    /// The `CheckAvail` queries answered with "does not fit".
    pub fn refused_availability_checks(&self) -> Vec<(u32, u32)> {
        self.refused_checks.iter().copied().collect()
    }

    pub fn heap_used(&self) -> u32 {
        self.heap.used()
    }

    /// The snapshot of the game's heap — holes, largest block, free and used.
    pub fn heap_snapshot(&self) -> crate::brew::heap::Snapshot {
        self.heap.snapshot()
    }
}
