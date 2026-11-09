//! ARM backend on top of Dynarmic.
//!
//! Dynarmic recompiles A32 blocks into the host's native code. This file keeps the same
//! [`CpuBackend`] boundary: the unmapped addresses of the BREW vtables are still the stop that
//! hands control back to the Rust dispatcher.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::mem::size_of;
use std::rc::Rc;

use dynarmic::a32::{ArchVersion, Callbacks, Dynarmic as Jit, VAddr};
use dynarmic::{CallbackImpl, GuestInt, HaltReason};

use super::mem::GuestMemory;
use super::{API_BASE, API_SIZE, RETURN_MAGIC};
use super::{CpuBackend, CpuError, Reg, StopReason};

/// ARM user-mode `CPSR`. The Superscape extension checks this field before touching the MMU
/// tables; the BREW applet is never privileged code.
const USER_MODE: u32 = 0x10;

/// The `T` bit of the `CPSR`: when set, the core fetches Thumb instructions.
const CPSR_THUMB: u32 = 1 << 5;

/// Defensive limit on the string received by `SYS_WRITE0`: a string with no terminator must not
/// trap the host in an endless read.
const MAX_SEMIHOSTING_STRING: u32 = 4096;

/// Granularity Dynarmic uses to index recompiled code.
const PAGE: u32 = 4096;

/// How many 4 KB pages fit in the guest's 32 bits: the size of the page table.
const PAGES: usize = 1 << 20;

/// Bytes reserved past the end of each writable region. A four-byte read in the last bytes of a
/// page that ends the region crosses the edge in host memory; with the slack, it reads allocated
/// memory instead of running off the vector.
const REGION_SLACK: usize = 16;

/// What a callback asked `run` to return. Dynarmic receives the memory accesses inside the
/// recompiled block; keeping the reason here preserves the distinction between API, return and
/// fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Nothing,
    Fetch(u32),
    Memory(u32),
    Exception(u32),
}

/// The guest pages that have already been fetched as code, one bit per page.
///
/// Every guest write asks here, and Kingdom Hearts's ARM renderer writes millions of pixels per
/// frame. With a `BTreeSet` behind a `RefCell`, each pixel paid for a borrow and a tree lookup —
/// and that is what made the 3D, already sped up by the JIT, start dropping frames again. One bit
/// per page, in a `Cell`, answers with a shift and a mask: 2^20 pages of 4 KB, 16,384 words,
/// 131 KB.
struct ExecutedPages {
    bits: Box<[Cell<u64>]>,
}

impl ExecutedPages {
    fn new() -> Self {
        Self {
            bits: (0..(1usize << 20).div_ceil(64)).map(|_| Cell::new(0)).collect(),
        }
    }

    fn mark(&self, page: u32) {
        let (word, bit) = ((page / 64) as usize, page % 64);
        if let Some(cell) = self.bits.get(word) {
            cell.set(cell.get() | (1 << bit));
        }
    }

    fn contains(&self, page: u32) -> bool {
        let (word, bit) = ((page / 64) as usize, page % 64);
        self.bits
            .get(word)
            .is_some_and(|cell| cell.get() & (1 << bit) != 0)
    }
}

/// State shared between the C++ callbacks and the Rust wrapper.
///
/// The JIT is kept in a `Box` in the backend, so its address does not change when the
/// `DynarmicCpu` moves. That lets the callback stop execution the instant the PC enters a BREW
/// vtable, instead of executing a bogus instruction at that address.
struct State {
    memory: Rc<RefCell<GuestMemory>>,
    semihosting: Rc<RefCell<String>>,
    /// Only a write to memory that has already been fetched as code can invalidate a JIT block.
    /// This excludes the millions of writes to the RGB565 buffers.
    executed_pages: ExecutedPages,
    /// Invalidations requested by the ARM during the running block; they are applied after `run`.
    dirty_code: RefCell<BTreeSet<u32>>,
    /// The ranges of [`CpuBackend::watch_dirty`]: `(id, start, end, dirty)`.
    watches: RefCell<Vec<(u32, u32, u32, bool)>>,
    /// The smallest interval that contains every watch. Almost every guest write falls outside
    /// it, and then it costs two comparisons instead of a walk through the list.
    bounds: Cell<(u32, u32)>,
    /// A range that is already dirty. A write inside it needs nothing more.
    ///
    /// That is almost always the case: an ARM renderer writes millions of consecutive pixels to
    /// the same buffer, and after the first one they all fall in a range that is already marked.
    /// Without the shortcut, each of them borrowed the list and walked it to mark what was
    /// already marked. Reset whenever some range may have gone back to clean.
    shortcut: Cell<(u32, u32)>,
    instructions: Cell<u64>,
    limit: Cell<u64>,
    stop: Cell<Stop>,
    /// How many times the host entered the JIT, and how long it stayed inside.
    ///
    /// **It is the number that separates the guest from dispatch.** Each API call is an exit and
    /// a re-entry, so what is left of the clock after subtracting the time spent inside `jit.run`
    /// is, almost entirely, trampoline plus method body. Without this count the cost per API call
    /// is unknown — and treating a number from the Unicorn era as current is how the external
    /// review got it wrong. See [`CpuBackend::jit_report`].
    ///
    /// They live here, and not in `DynarmicCpu`, because the `Jit` borrow is alive throughout
    /// `run`: a `Cell` in the state gets through the immutable borrow without fighting it.
    ///
    /// **Counting is free; timing is not.** Measured: a pair of `Instant::now()` per entry cost
    /// **40% of the clock** on this machine (`Instant::now` here is a system call, not the vDSO
    /// fast path), and there are 1.3 million entries in fifteen seconds of Quake. That is why
    /// counting is always on — an addition in a `Cell` — and the clock is **sampled**, and only
    /// when someone asks for the cost profile. See [`JIT_SAMPLE`].
    jit_entries: Cell<u64>,
    jit_nanos: Cell<u64>,
    jit_samples: Cell<u64>,
    jit: Cell<*mut Jit<State>>,
    /// Dynarmic's page table: `PAGES` pointers, the start of each page on the host, or null for a
    /// page that has to go through the callbacks. See [`DynarmicCpu::table`].
    table: *mut *mut u8,
}

impl State {
    /// Removes the page from the table: from then on reads and writes to it go through the
    /// callbacks.
    fn clear_page(&self, page: u32) {
        if (page as usize) < PAGES {
            unsafe { *self.table.add(page as usize) = std::ptr::null_mut() };
        }
    }

    fn halt(cb: &CallbackImpl<Self>, stop: Stop) {
        cb.stop.set(stop);
        let jit = cb.jit.get();
        if !jit.is_null() {
            // `HaltExecution` is the path Dynarmic documents for a callback to hand control back
            // to the host. The concrete reason is kept in `stop`, since the public enum only
            // offers the generic user bits.
            unsafe { (*jit).halt(HaltReason::UserDefined1) };
        }
    }

    fn read(&self, addr: u32, out: &mut [u8]) -> bool {
        self.memory
            .borrow()
            .read(addr, out.len() as u32)
            .map_or_else(
                |_| false,
                |bytes| {
                    out.copy_from_slice(bytes);
                    true
                },
            )
    }

    fn write(&self, addr: u32, bytes: &[u8]) -> bool {
        if self.memory.borrow_mut().write(addr, bytes).is_err() {
            return false;
        }
        self.mark_code_dirty(addr, bytes.len() as u32);
        self.mark_watches(addr, addr.saturating_add(bytes.len() as u32));
        true
    }

    fn mark_watches(&self, start: u32, end: u32) {
        let (smallest, largest) = self.bounds.get();
        if end <= smallest || start >= largest {
            return;
        }
        let (a, b) = self.shortcut.get();
        if start >= a && end <= b {
            return;
        }
        let mut watches = self.watches.borrow_mut();
        let mut touched = 0;
        let mut strip = (0, 0);
        for watch in watches.iter_mut() {
            if start < watch.2 && end > watch.1 {
                watch.3 = true;
                touched += 1;
                strip = (watch.1, watch.2);
            }
        }
        // The shortcut only applies to a range that contains the whole write and is the only one
        // touched: with two overlapping, skipping the second would leave it unmarked.
        if touched == 1 && start >= strip.0 && end <= strip.1 {
            self.shortcut.set(strip);
        }
    }

    fn recompute_bounds(&self) {
        self.shortcut.set((0, 0));
        let watches = self.watches.borrow();
        let smallest = watches.iter().map(|v| v.1).min().unwrap_or(0);
        let largest = watches.iter().map(|v| v.2).max().unwrap_or(0);
        self.bounds.set((smallest, largest));
    }

    fn mark_code_dirty(&self, addr: u32, len: u32) {
        let end = addr.saturating_add(len.saturating_sub(1));
        for page in (addr / PAGE)..=(end / PAGE) {
            // The one-bit test comes before any borrow: almost every write lands on a data page,
            // and only one that lands on code pays for the dirty list.
            if self.executed_pages.contains(page) {
                self.dirty_code.borrow_mut().insert(page);
            }
        }
    }
}

impl Callbacks for State {
    fn memory_read_code(cb: &CallbackImpl<Self>, addr: VAddr) -> Option<u32> {
        let mut bytes = [0; 4];
        if cb.memory.borrow().executable(addr) && cb.read(addr, &mut bytes) {
            // A page that becomes code leaves the table, so every write to it reaches the
            // callback that invalidates the recompiled block.
            let page = addr / PAGE;
            if !cb.executed_pages.contains(page) {
                cb.executed_pages.mark(page);
                cb.clear_page(page);
            }
            Some(u32::from_le_bytes(bytes))
        } else {
            Self::halt(cb, Stop::Fetch(addr));
            None
        }
    }

    extern "C" fn memory_read<T: GuestInt>(cb: &CallbackImpl<Self>, addr: VAddr) -> T {
        let mut bytes = [0u8; 8];
        let len = size_of::<T>();
        if !cb.read(addr, &mut bytes[..len]) {
            Self::halt(cb, Stop::Memory(addr));
        }
        // The hosts Dynarmic supports are x86-64 and AArch64, both little-endian. `T` is only
        // instantiated by the JIT for u8/u16/u32/u64.
        unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<T>()) }
    }

    extern "C" fn memory_write<T: GuestInt>(cb: &mut CallbackImpl<Self>, addr: VAddr, value: T) {
        let len = size_of::<T>();
        let bytes = unsafe { std::slice::from_raw_parts((&value as *const T).cast::<u8>(), len) };
        if !cb.write(addr, bytes) {
            Self::halt(cb, Stop::Memory(addr));
        }
    }

    extern "C" fn call_svc(cb: &mut CallbackImpl<Self>, swi: u32) {
        // `SVC #0xAB` is the ARM semihosting Peggle and Zuma use for logging. It is not a BREW
        // interrupt: after serving it, execution continues at the next instruction. We recognize
        // the two output operations the observed games use and return zero for the others, as
        // the ARM monitor does for most harmless queries.
        //
        // **The number changes with the core's state**: `0xAB` is Thumb's, and `0x123456` ARM's.
        // Ultimate Chess 3D writes its log in ARM, and with only `0xAB` recognized the first
        // `SYS_WRITEC` — when capturing an opponent's piece — stopped the game with an "ARM core
        // exception". The interpreter already served both: it only looks at `r0`.
        if SEMIHOSTING.contains(&swi) {
            let jit = unsafe { &mut *cb.jit.get() };
            let op = jit.get_reg(0);
            let arg = jit.get_reg(1);
            let mut output = cb.semihosting.borrow_mut();
            match op {
                0x03 => {
                    let mut byte = [0u8; 1];
                    if cb.read(arg, &mut byte) {
                        output.push(byte[0] as char);
                    }
                }
                0x04 => {
                    for offset in 0..MAX_SEMIHOSTING_STRING {
                        let mut byte = [0u8; 1];
                        if !cb.read(arg.saturating_add(offset), &mut byte) || byte[0] == 0 {
                            break;
                        }
                        output.push(byte[0] as char);
                    }
                }
                _ => {}
            }
            super::trim_semihosting(&mut output);
            jit.set_reg(0, 0);
        } else if cb.stop.get() == Stop::Nothing {
            let pc = unsafe { (*cb.jit.get()).get_pc() };
            Self::halt(cb, Stop::Exception(pc));
        }
    }

    extern "C" fn add_ticks(cb: &mut CallbackImpl<Self>, ticks: u64) {
        cb.instructions.set(cb.instructions.get().saturating_add(ticks));
    }

    extern "C" fn get_ticks_remaining(cb: &CallbackImpl<Self>) -> u64 {
        cb.limit.get().saturating_sub(cb.instructions.get())
    }

    extern "C" fn raised_exception(
        cb: &mut CallbackImpl<Self>,
        pc: VAddr,
        _exception: ::dynarmic::a32::Exception,
    ) {
        // `MemoryReadCode(None)` ends up as `NoExecuteFault`, which arrives here after the
        // callback has already recorded the fetch. Not overwriting it is what keeps the BREW
        // vtable boundary distinguishable from a truly invalid instruction.
        if cb.stop.get() == Stop::Nothing {
            // The current binding turns the fetch's `optional<u32>::none()` into
            // `NoExecuteFault` before handing control back. The API range and the sentinel never
            // contain code by definition, so they can still be classified here without
            // ambiguity.
            if pc == RETURN_MAGIC || (API_BASE..API_BASE.saturating_add(API_SIZE)).contains(&pc) {
                Self::halt(cb, Stop::Fetch(pc));
            } else {
                Self::halt(cb, Stop::Exception(pc));
            }
        }
    }
}

/// The `SVC` numbers of ARM semihosting: Thumb's and ARM's.
const SEMIHOSTING: [u32; 2] = [0xab, 0x12_3456];

/// Every how many JIT entries the clock is read, when measurement is on.
const JIT_SAMPLE: u64 = 64;

/// A32 recompiler. It stays separate from the default backend until equivalence is established
/// game by game; creating the CPU does not allocate the JIT, because the guest map only exists in
/// `reset`.
pub struct DynarmicCpu {
    memory: Rc<RefCell<GuestMemory>>,
    semihosting: Rc<RefCell<String>>,
    jit: Option<Box<Jit<State>>>,
    /// **The page table: memory access without a callback.** Without it, every read and write
    /// of the recompiled code left the JIT for a Rust callback, took the map through `RefCell` and
    /// looked up the region. In Need for Speed, during a race, that left the emulator at 176
    /// million instructions per second — 69% of the console's speed.
    ///
    /// Each entry points to the start of the page in the memory of the writable regions, which
    /// does not move after `reset`. These stay null, and keep going through the callbacks: the
    /// read-only regions, the pages that have already been executed (a write to them invalidates
    /// code), the pages with a write watch (surfaces and buffers the emulator needs to see dirty)
    /// and the last page of a region that does not fill it.
    table: Box<[*mut u8]>,
}

impl DynarmicCpu {
    pub fn new() -> Result<Self, CpuError> {
        Ok(Self {
            memory: Default::default(),
            semihosting: Default::default(),
            jit: None,
            table: vec![std::ptr::null_mut(); PAGES].into_boxed_slice(),
        })
    }

    /// Puts back into the table the pages of `[start, end)` that nothing needs to intercept any
    /// more.
    fn restore_pages(&mut self, start: u32, end: u32) {
        let Some(jit) = self.jit.as_deref() else {
            return;
        };
        let watches = jit.watches.borrow();
        let memory = self.memory.borrow();
        for page in (start / PAGE)..=(end.saturating_sub(1) / PAGE) {
            let (page_start, page_end) = (page * PAGE, (page * PAGE).saturating_add(PAGE));
            let watched = watches.iter().any(|v| page_start < v.2 && page_end > v.1);
            if watched || jit.executed_pages.contains(page) {
                continue;
            }
            self.table[page as usize] = page_pointer(&memory, page);
        }
    }

    fn jit(&self) -> Result<&Jit<State>, CpuError> {
        self.jit
            .as_deref()
            .ok_or_else(|| CpuError("Dynarmic received no memory map".into()))
    }

    fn jit_mut(&mut self) -> Result<&mut Jit<State>, CpuError> {
        self.jit
            .as_deref_mut()
            .ok_or_else(|| CpuError("Dynarmic received no memory map".into()))
    }

    fn index(reg: Reg) -> usize {
        match reg {
            Reg::R0 => 0,
            Reg::R1 => 1,
            Reg::R2 => 2,
            Reg::R3 => 3,
            Reg::R4 => 4,
            Reg::R5 => 5,
            Reg::R6 => 6,
            Reg::R7 => 7,
            Reg::R8 => 8,
            Reg::R9 => 9,
            Reg::R10 => 10,
            Reg::R11 => 11,
            Reg::R12 => 12,
            Reg::Sp => 13,
            Reg::Lr => 14,
            Reg::Pc => 15,
        }
    }

    /// The diagnostics interface calls this method on both cores.
    pub fn semihosting(&self) -> String {
        self.semihosting.borrow().clone()
    }

    /// The host API also writes guest memory, always between two JIT entries. If it changes a
    /// page that has already been executed, the next block must be recompiled.
    fn invalidate_written_code(&mut self, addr: u32, len: u32) {
        let Ok(jit) = self.jit_mut() else {
            return;
        };
        jit.mark_code_dirty(addr, len);
        let pages = std::mem::take(&mut *jit.dirty_code.borrow_mut());
        // **How many code pages fell because of a host write.** It is the number that tells
        // whether a read/write to an already executed page is a rare event (nothing to do) or a
        // hot path (a candidate for direct reads with a write trap). Without it, the cost of SMC
        // is invisible in the profile: it shows up diluted in dispatch, as "some API call".
        if !pages.is_empty() {
            crate::logging!(
                crate::logging::Level::Debug,
                "cpu",
                "write to {addr:#010x}+{len} invalidated {} code page(s)",
                pages.len()
            );
        }
        for page in pages {
            jit.invalidate_cache_range(page * PAGE, PAGE as usize);
        }
    }
}

impl CpuBackend for DynarmicCpu {
    fn reset(&mut self, mem: &GuestMemory) -> Result<(), CpuError> {
        // `GuestMemory` is deliberately built once before execution. Copying the map into the
        // memory the callbacks own keeps the backend's contract: the Rust API and the ARM see the
        // same bytes from here on.
        let mut copy = GuestMemory::new();
        for region in mem.regions() {
            copy
                .map_executable(
                    region.name,
                    region.base,
                    region.bytes.clone(),
                    region.writable,
                    region.executable,
                )
                .map_err(|e| CpuError(e.to_string()))?;
        }
        for region in copy.regions_mut() {
            region.bytes.reserve_exact(REGION_SLACK);
        }
        self.table.fill(std::ptr::null_mut());
        for page in 0..PAGES as u32 {
            self.table[page as usize] = page_pointer(&copy, page);
        }
        // How many pages of the guest's 32 bits have direct access and how many stayed on the
        // callback. It is the first number to look at when discussing the JIT's memory cost: the
        // difference between the two columns is what goes through Rust on every read and write.
        let direct = self
            .table
            .iter()
            .filter(|pointer| !pointer.is_null())
            .count();
        crate::logging!(
            crate::logging::Level::Debug,
            "cpu",
            "page table: {direct} of {} with direct access ({} region(s))",
            PAGES,
            copy.regions().len()
        );
        self.memory = Rc::new(RefCell::new(copy));
        self.semihosting.borrow_mut().clear();
        let state = State {
            memory: self.memory.clone(),
            semihosting: self.semihosting.clone(),
            executed_pages: ExecutedPages::new(),
            dirty_code: Default::default(),
            watches: Default::default(),
            bounds: Cell::new((0, 0)),
            shortcut: Cell::new((0, 0)),
            instructions: Cell::new(0),
            limit: Cell::new(0),
            stop: Cell::new(Stop::Nothing),
            jit_entries: Cell::new(0),
            jit_nanos: Cell::new(0),
            jit_samples: Cell::new(0),
            jit: Cell::new(std::ptr::null_mut()),
            table: self.table.as_mut_ptr(),
        };
        let mut config = Jit::<State>::new_config();
        config.arch_ver(ArchVersion::V6K);
        // 64 MiB fits on the desktop. On the Switch this block competes for the heap with the
        // game and with Mesa; 32 MiB still covers Double Dragon's translated code when the title
        // is opened holding R.
        config.code_cache_size(if cfg!(zeebx_switch) {
            32 * 1024 * 1024
        } else {
            64 * 1024 * 1024
        });
        // Entry = start of the page on the host, with no absolute offset and no attribute bits.
        config.page_table_mask(0);
        unsafe { config.page_table(self.table.as_mut_ptr().cast()) };
        let mut jit = Box::new(config.init(state));
        let ptr = &mut *jit as *mut Jit<State>;
        jit.jit.set(ptr);
        jit.set_cpsr(USER_MODE);
        self.jit = Some(jit);
        Ok(())
    }

    fn read_reg(&self, reg: Reg) -> u32 {
        self.jit().map_or(0, |jit| jit.get_reg(Self::index(reg)))
    }

    fn write_reg(&mut self, reg: Reg, value: u32) {
        if let Ok(jit) = self.jit_mut() {
            jit.set_reg(Self::index(reg), value);
        }
    }

    fn instructions(&self) -> u64 {
        self.jit().map_or(0, |jit| jit.instructions.get())
    }

    fn jit_report(&self) -> Option<(u64, u64, u64)> {
        self.jit().ok().map(|jit| {
            let entries = jit.jit_entries.get();
            let samples = jit.jit_samples.get();
            // The sampled average applies to every entry: none of them is special.
            let nanos = match samples {
                0 => 0,
                n => jit.jit_nanos.get() / n * entries,
            };
            (entries, nanos, samples)
        })
    }

    fn set_instructions(&mut self, value: u64) {
        if let Ok(jit) = self.jit_mut() {
            jit.instructions.set(value);
        }
    }

    fn cpsr(&self) -> u32 {
        self.jit().map_or(0, |jit| jit.get_cpsr())
    }

    fn set_cpsr(&mut self, value: u32) {
        if let Ok(jit) = self.jit_mut() {
            jit.set_cpsr(value);
        }
    }

    fn in_thumb(&self) -> bool {
        self.jit().is_ok_and(|jit| jit.get_cpsr() & CPSR_THUMB != 0)
    }

    /// The write watch: only a write **by the guest** sets the flag.
    ///
    /// Without it the default contract answers "always dirty", and every drawing call imported
    /// all the surfaces in full. In Pac-Mania, 100 thousand `IIMAGE_Draw` added up to 22 seconds
    /// just reading buffers the game had not touched.
    fn watch_dirty(&mut self, id: u32, base: u32, len: u32) -> Result<(), CpuError> {
        self.unwatch_dirty(id);
        let jit = self.jit_mut()?;
        // Starts dirty: we have not seen anything of this range yet.
        jit.watches
            .borrow_mut()
            .push((id, base, base.saturating_add(len), true));
        jit.recompute_bounds();
        // A write to a watched range has to go through the callback that marks it dirty.
        let end = base.saturating_add(len);
        for page in (base / PAGE)..=(end.saturating_sub(1) / PAGE) {
            jit.clear_page(page);
        }
        Ok(())
    }

    fn unwatch_dirty(&mut self, id: u32) {
        let Ok(jit) = self.jit_mut() else {
            return;
        };
        let strip = jit
            .watches
            .borrow()
            .iter()
            .find(|watch| watch.0 == id)
            .map(|watch| (watch.1, watch.2));
        jit.watches.borrow_mut().retain(|watch| watch.0 != id);
        jit.recompute_bounds();
        if let Some((start, end)) = strip {
            self.restore_pages(start, end);
        }
    }

    fn take_dirty(&mut self, id: u32) -> bool {
        let Ok(jit) = self.jit_mut() else {
            return true;
        };
        // A range may go back to clean here, so the shortcut no longer applies.
        jit.shortcut.set((0, 0));
        let mut watches = jit.watches.borrow_mut();
        match watches.iter_mut().find(|watch| watch.0 == id) {
            Some(watch) => std::mem::replace(&mut watch.3, false),
            None => true,
        }
    }

    fn mark_dirty(&mut self, addr: u32, len: u32) {
        let Ok(jit) = self.jit_mut() else {
            return;
        };
        let end = addr.saturating_add(len);
        for watch in jit.watches.borrow_mut().iter_mut() {
            if addr < watch.2 && end > watch.1 {
                watch.3 = true;
            }
        }
    }

    fn read_mem(&self, addr: u32, buf: &mut [u8]) -> Result<(), CpuError> {
        self.memory
            .borrow()
            .read(addr, buf.len() as u32)
            .map(|bytes| buf.copy_from_slice(bytes))
            .map_err(|e| CpuError(e.to_string()))
    }

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> Result<(), CpuError> {
        self.memory
            .borrow_mut()
            .write(addr, data)
            .map_err(|e| CpuError(e.to_string()))?;
        self.invalidate_written_code(addr, data.len() as u32);
        Ok(())
    }

    fn fill_mem(&mut self, addr: u32, value: u8, len: u32) -> Result<(), CpuError> {
        self.memory
            .borrow_mut()
            .fill(addr, value, len)
            .map_err(|e| CpuError(e.to_string()))?;
        self.invalidate_written_code(addr, len);
        Ok(())
    }

    fn mark_stack_dirty(&mut self, addr: u32, len: u32) -> Result<(), CpuError> {
        // No invalidation, on purpose: the stack never became code (see the trace). The marking
        // pays for `jit_mut` and two borrows per API call only to always come out empty, and that
        // is what took the 30% — measured in Crash Nitro Kart's `bench`, which goes back from
        // 296% to 368% just by dropping it.
        debug_assert!(
            self.jit_mut().is_ok_and(|jit| {
                let end = addr.saturating_add(len.saturating_sub(1));
                ((addr / PAGE)..=(end / PAGE)).all(|p| !jit.executed_pages.contains(p))
            }),
            "stack dirtying at {addr:#010x}+{len} landed on an executed page"
        );
        self.memory
            .borrow_mut()
            .fill(addr, 0, len)
            .map_err(|e| CpuError(e.to_string()))
    }

    fn run(&mut self, pc: u32, max_instructions: u64) -> Result<StopReason, CpuError> {
        let jit = self.jit_mut()?;
        // **Bit 0 of the address is the mode, not part of the address.** The dispatcher resumes
        // at `lr` the way it came, and the `lr` of a call made from Thumb code has bit 0 set —
        // that is ARM's interworking convention. Here it has to be explicit: writing the raw
        // address, Zenonia, which is all Thumb, came back from every API one byte ahead and the
        // core stopped at an "instruction" made of halves of two.
        let cpsr = jit.get_cpsr();
        match pc & 1 {
            1 => jit.set_cpsr(cpsr | CPSR_THUMB),
            _ => jit.set_cpsr(cpsr & !CPSR_THUMB),
        }
        jit.set_pc(pc & !1);
        // `HaltExecution` leaves the reason armed for the run that just exited. The next stretch
        // starts after the BREW dispatcher has written r0/pc, so it has to clear the generic bit
        // before entering the JIT again.
        jit.clear_halt(HaltReason::UserDefined1);
        jit.stop.set(Stop::Nothing);
        jit.limit
            .set(jit.instructions.get().saturating_add(max_instructions));
        // The clock around `jit.run`, and only it: everything that happens in here is guest
        // execution (plus the memory callbacks, which the JIT itself calls). What is left out is
        // dispatch — and that is where the trampoline lives.
        //
        // **Sampled on purpose, and always on.** `Instant::now` on this machine is a system call,
        // and a pair per entry cost 40% of the clock. One entry in every `JIT_SAMPLE` measures the
        // same for 1/64 of the price — about 0.6% of the clock, measured —, and that is why the
        // split comes out in every report instead of depending on someone remembering to turn it
        // on. The sampled average is multiplied by the counter afterwards.
        let entry = jit.jit_entries.get();
        let timed = entry % JIT_SAMPLE == 0;
        let begin = timed.then(std::time::Instant::now);
        let _ = unsafe { jit.run() };
        if let Some(begin) = begin {
            jit.jit_nanos.set(
                jit.jit_nanos
                    .get()
                    .saturating_add(begin.elapsed().as_nanos() as u64),
            );
            jit.jit_samples.set(jit.jit_samples.get().saturating_add(1));
        }
        jit.jit_entries.set(entry.saturating_add(1));
        // There is no invalidation for data pages: only previously executed code gets here.
        // It is safe to touch the cache after the JIT hands control back, never from the callback.
        let pages = std::mem::take(&mut *jit.dirty_code.borrow_mut());
        for page in pages {
            jit.invalidate_cache_range(page * PAGE, PAGE as usize);
        }
        match jit.stop.get() {
            Stop::Nothing => Ok(StopReason::Budget),
            Stop::Fetch(addr) if addr == RETURN_MAGIC => Ok(StopReason::Returned),
            Stop::Fetch(addr)
                if (API_BASE..API_BASE.saturating_add(API_SIZE)).contains(&addr) =>
            {
                Ok(StopReason::ApiCall { addr })
            }
            Stop::Fetch(addr) => Ok(StopReason::MemoryFault { addr, pc: addr }),
            Stop::Memory(addr) => Ok(StopReason::MemoryFault {
                addr,
                pc: jit.get_pc(),
            }),
            Stop::Exception(pc) => Ok(StopReason::Exception { pc }),
        }
    }
}

/// The start of `page` in host memory, when it is writable and fits entirely in one region.
fn page_pointer(memory: &GuestMemory, page: u32) -> *mut u8 {
    let start = u64::from(page) * u64::from(PAGE);
    let end = start + u64::from(PAGE);
    memory
        .regions()
        .iter()
        .find(|r| {
            r.writable
                && start >= u64::from(r.base)
                && end <= u64::from(r.base) + r.bytes.len() as u64
        })
        .map_or(std::ptr::null_mut(), |r| {
            // The region is not reallocated after `reset`: the vector is only written, it never
            // grows.
            unsafe { r.bytes.as_ptr().add((start - u64::from(r.base)) as usize) as *mut u8 }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu_with(code: &[u8]) -> DynarmicCpu {
        let mut mem = GuestMemory::new();
        let mut bytes = code.to_vec();
        bytes.resize(0x1000, 0);
        // The Zeebo's module image is writable: extensions can build small routines or tables in
        // RAM. Keeping the test code writable lets us check JIT invalidation.
        mem.map("code", 0, bytes, true).unwrap();
        mem.map_zeroed("data", 0x1000, 0x1000).unwrap();
        mem.map_zeroed("stack", 0x2000_0000, 0x1000).unwrap();
        let mut cpu = DynarmicCpu::new().unwrap();
        cpu.reset(&mem).unwrap();
        cpu.write_reg(Reg::Sp, 0x2000_0f00);
        cpu
    }

    #[test]
    fn executes_an_instruction_and_reads_a_register() {
        let code = [0xe3a0_0037u32.to_le_bytes(), 0xeaff_fffeu32.to_le_bytes()].concat();
        let mut cpu = cpu_with(&code); // mov r0, #0x37 ; b .
        assert_eq!(cpu.run(0, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    /// **An exclusive instruction must not bring down the process.**
    ///
    /// Dynarmic's x64 emitter requires a `global_monitor` **at translation time** for
    /// LDREX/STREX: `EmitExclusiveReadMemory` does `ASSERT(conf.global_monitor != nullptr)` and
    /// then dereferences it (`emit_x64_memory.cpp.inc`). The wrapper never filled in that field,
    /// and crate 0.1.3 has no setter (`a32.rs` does `unsafe { std::mem::zeroed() }` with a `todo`),
    /// that is: the field starts out null. Nothing downgrades these instructions when the monitor
    /// is missing.
    ///
    /// 40 of the 62 `.mod` files in the collection contain this pattern somewhere. The test builds
    /// `ldrex r0, [r1]` followed by `b .` and runs the block: without a monitor, translation aborts
    /// and the process dies before the assertion is read.
    ///
    /// **Measured, and that is why it stays ignored instead of green**: on 2026-09-24 this test
    /// killed the process with `assertion failed: conf.global_monitor != nullptr` and
    /// `signal: 6, SIGABRT`. The fix is not ours — it needs a patch to `dynarmic` 0.1.3 (adding
    /// the `global_monitor` field to the public `Config`), and the crate has no setter. With the
    /// patch, remove the `#[ignore]` and this test becomes the guard.
    #[ignore = "proves a known defect in the dependency; see the comment above"]
    #[test]
    fn an_exclusive_instruction_does_not_crash_the_process() {
        // `ldrex r0, [r1]` (0xE1910F9F) and `b .` (0xEAFFFFFE).
        let code = [0xe191_0f9fu32.to_le_bytes(), 0xeaff_fffeu32.to_le_bytes()].concat();
        let mut cpu = cpu_with(&code);
        // r1 points to the data memory, so the read has a mapped address.
        cpu.write_reg(Reg::R1, 0x1000);
        assert_eq!(cpu.run(0, 2).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0, "memory starts zeroed");
    }

    #[test]
    fn a_jump_into_the_api_range_becomes_a_call() {
        let code = [0xe3a0_020fu32.to_le_bytes(), 0xe12f_ff10u32.to_le_bytes()].concat();
        let mut cpu = cpu_with(&code);
        assert_eq!(
            cpu.run(0, 10).unwrap(),
            StopReason::ApiCall { addr: API_BASE }
        );
    }

    #[test]
    fn an_address_with_bit_zero_enters_thumb() {
        // In Thumb, `movs r0, #0x37` and then `b .` — aligned at 0x100. Called with 0x101, which
        // is how the dispatcher resumes a Thumb caller.
        let mut code = vec![0u8; 0x100];
        code.extend_from_slice(&[0x37, 0x20, 0xfe, 0xe7]);
        let mut cpu = cpu_with(&code);
        assert_eq!(cpu.run(0x101, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    #[test]
    fn an_even_address_returns_to_arm_after_thumb() {
        // `mov r0, #0x37` in ARM at zero; at 0x100, arbitrary Thumb code.
        let mut code = [0xe3a0_0037u32.to_le_bytes(), 0xeaff_fffeu32.to_le_bytes()].concat();
        code.resize(0x100, 0);
        code.extend_from_slice(&[0x00, 0x20, 0xfe, 0xe7]);
        let mut cpu = cpu_with(&code);
        cpu.run(0x101, 1).unwrap();
        assert_eq!(cpu.run(0, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    #[test]
    fn the_watch_trips_on_guest_writes_not_host_writes() {
        // `mov r1, #0x1000 ; str r0, [r1] ; b .`
        let code = [
            0xe3a0_1a01u32.to_le_bytes(),
            0xe581_0000u32.to_le_bytes(),
            0xeaff_fffeu32.to_le_bytes(),
        ]
        .concat();
        let mut cpu = cpu_with(&code);
        cpu.watch_dirty(7, 0x1000, 4).unwrap();
        assert!(cpu.take_dirty(7), "starts dirty");
        assert!(!cpu.take_dirty(7), "and reading clears it");
        cpu.write_mem(0x1000, &[1, 2, 3, 4]).unwrap();
        assert!(!cpu.take_dirty(7), "a host write does not count");
        cpu.run(0, 2).unwrap();
        assert!(cpu.take_dirty(7), "a guest write counts");
        // A disarmed range goes back to the default contract: when in doubt, dirty.
        cpu.unwatch_dirty(7);
        assert!(cpu.take_dirty(7));
    }

    #[test]
    fn a_return_to_the_sentinel_is_recognized() {
        let mut cpu = cpu_with(&0xe12f_ff1eu32.to_le_bytes()); // bx lr
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
    }

    #[test]
    fn semihosting_writes_without_interrupting_the_guest() {
        // mov r0,#SYS_WRITEC ; mov r1,#0x1000 ; svc #0xab ; bx lr
        let code = [0xe3a0_0003u32, 0xe3a0_1a01, 0xef00_00ab, 0xe12f_ff1e]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        let mut cpu = cpu_with(&code);
        cpu.write_mem(0x1000, b"Z").unwrap();
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.semihosting(), "Z");
        assert_eq!(cpu.read_reg(Reg::R0), 0);
    }

    /// Semihosting in ARM state uses `SVC #0x123456`, and must not stop the game.
    #[test]
    fn semihosting_from_arm_state_also_writes() {
        // mov r0,#SYS_WRITEC ; mov r1,#0x1000 ; svc #0x123456 ; bx lr
        let code = [0xe3a0_0003u32, 0xe3a0_1a01, 0xef12_3456, 0xe12f_ff1e]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        let mut cpu = cpu_with(&code);
        cpu.write_mem(0x1000, b"C").unwrap();
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.semihosting(), "C");
    }

    #[test]
    fn code_changed_by_the_host_is_recompiled() {
        let mut cpu = cpu_with(
            &[0xe3a0_0001u32.to_le_bytes(), 0xe12f_ff1eu32.to_le_bytes()].concat(),
        );
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 1);

        // It is the equivalent of a BREW API updating a table/routine of the module between two
        // calls. Without `invalidate_cache_range`, Dynarmic would reuse the `mov r0,#1`.
        cpu.write_mem(0, &0xe3a0_0002u32.to_le_bytes()).unwrap();
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 2);
    }
}
