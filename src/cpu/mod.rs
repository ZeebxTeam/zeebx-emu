//! Abstraction of the ARM core.
//!
//! The emulator never talks to a concrete core: it talks to [`CpuBackend`]. The default
//! implementation is `dynarmic`, which recompiles A32/Thumb blocks into native host code.

// Removed as soon as the core is wired into the main loop.
#![allow(dead_code)]

pub mod mem;

use crate::cpu::mem::GuestMemory;

/// Registers that API dispatch needs to read and write.
///
/// Follows the AAPCS: arguments in `r0..r3` and the rest on the stack, return value in `r0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reg {
    R0,
    R1,
    R2,
    R3,
    /// `r4..r11` are not part of the calling convention, but they are where the compiler keeps
    /// `this` and the live variables — without them the context of a fault cannot be rebuilt.
    R4,
    R5,
    R6,
    R7,
    R8,
    R9,
    R10,
    R11,
    R12,
    Sp,
    Lr,
    Pc,
}

/// Why execution stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The guest jumped into the range reserved for the BREW vtables — it is an API call.
    ApiCall { addr: u32 },
    /// The guest returned to the sentinel address placed in `lr` on entry.
    Returned,
    /// Access to unmapped memory. `addr` is the address accessed, `pc` the instruction that tried.
    MemoryFault { addr: u32, pc: u32 },
    /// Invalid instruction, SWI or another core exception.
    Exception { pc: u32 },
    /// The slice's instruction budget ran out.
    Budget,
}

pub trait CpuBackend {
    /// Prepares the core to execute with this memory map.
    fn reset(&mut self, mem: &GuestMemory) -> Result<(), CpuError>;

    fn read_reg(&self, reg: Reg) -> u32;
    fn write_reg(&mut self, reg: Reg, value: u32);

    /// Executes from `pc` until it stops, spending at most `max_instructions`.
    fn run(&mut self, pc: u32, max_instructions: u64) -> Result<StopReason, CpuError>;

    /// Reads guest memory. After `reset` the memory lives inside the core, so the API
    /// implementations must go through here instead of consulting the [`GuestMemory`].
    /// How many instructions the guest has executed so far.
    ///
    /// It is the emulator's clock: the time the game sees comes from here, not from the host, so
    /// that two identical runs give the same result.
    fn instructions(&self) -> u64;

    /// Entries into the JIT, the estimated time inside it and how many samples back the estimate.
    ///
    /// `None` when the backend has no JIT entry to separate — the interpreter has none. Whoever
    /// uses this wants the difference between the time inside the core and the time in dispatch;
    /// see [`crate::cpu::dynarmic::DynarmicCpu::run`]. **The third number is the honesty of the
    /// measurement:** zero samples means the clock was not turned on, and then only the count
    /// counts.
    fn jit_report(&self) -> Option<(u64, u64, u64)> {
        None
    }

    /// Whether the core stopped with the guest in Thumb mode.
    ///
    /// Used to resume an interrupted stretch: whoever resumes passes the address with bit 0
    /// set, which is how ARM says "continue in Thumb". Without it, a game written entirely in
    /// Thumb — Zenonia, the Extreme series — would come back decoded as ARM.
    /// The `CPSR`: the flags of the last operation and the processor mode.
    ///
    /// **Without this a save state goes wrong in a way that is hard to see.** The memory comes
    /// back, the program comes back, and the flags are those of another run: the comparison the
    /// game made before saving still holds, but the next decision may take the other path.
    /// Dynarmic exposes this through `get_cpsr`, and that is why the method has no default: a
    /// backend that cannot report the `CPSR` has to say so, not silently return zero.
    fn cpsr(&self) -> u32;

    /// Sets the `CPSR`. See [`CpuBackend::cpsr`].
    fn set_cpsr(&mut self, value: u32);

    /// Sets the instruction counter, which is the emulator's **virtual clock**.
    ///
    /// Saving the clock and not restoring it would leave the game after the save state with a
    /// different time: the `SetTimer`s, the per-frame audio and the pacing step limit depend on it.
    fn set_instructions(&mut self, value: u64);

    fn in_thumb(&self) -> bool {
        false
    }

    /// Arms a dirty flag on a range: the hook sets it when the guest writes to it.
    ///
    /// Without this, finding out whether the game touched a surface requires **reading the whole
    /// range and comparing byte by byte**. Measured on the Z-Wheel: the `sync` of the pbuffer's
    /// color buffer was called 93 thousand times in thirteen seconds, and the read plus the
    /// comparison added up to six seconds — more than a third of all API time.
    ///
    /// The `id` identifies the range, and exists because there are **several** at once: the
    /// pbuffer's color buffer and one per game surface. Arming again with the same `id` moves the
    /// range, which is what happens when a bitmap is re-exposed with another size.
    ///
    /// The default answers "always dirty", which is exactly the previous behavior: a backend that
    /// cannot arm the hook stays correct, it just does not get faster.
    fn watch_dirty(&mut self, _id: u32, _base: u32, _len: u32) -> Result<(), CpuError> {
        Ok(())
    }

    /// Disarms the range of `id`. Without this, the surface of an already freed bitmap would keep
    /// costing a hook on every guest write to that address.
    fn unwatch_dirty(&mut self, _id: u32) {}

    /// Reads **and clears** the flag of `id`. `true` when the guest may have written since the
    /// last time, and also when no range is armed with that `id` — when in doubt, dirty.
    fn take_dirty(&mut self, _id: u32) -> bool {
        true
    }

    /// Sets the flag of the watched ranges that overlap `addr..addr+len`.
    ///
    /// The watch only sees writes **by the guest**, and that is what we want for the emulator's
    /// own writes. But a `MEMMOVE` the game asks of the helper is a game write done by our
    /// hands: the Z-Wheel composes the 3D stage this way, copying the pbuffer into the pixels of
    /// the destination bitmap, and without this notice the surface never imported the copy — the
    /// stage stayed gray.
    fn mark_dirty(&mut self, _addr: u32, _len: u32) {}

    fn read_mem(&self, addr: u32, buf: &mut [u8]) -> Result<(), CpuError>;

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> Result<(), CpuError>;

    /// Fills `len` bytes with `value`, without allocating.
    ///
    /// It exists because of a measurement: the guest's `memset` is called in a wait loop, and the
    /// implementation through `write_mem` **allocated a `Vec` of the requested size on every
    /// call**. In Zeebo Extreme Rolima that was 731 ms of the 1,149 ms spent in API calls — 63% —,
    /// for 12 thousand clearing calls. Here the clearing goes in blocks from a stack buffer.
    fn fill_mem(&mut self, addr: u32, value: u8, len: u32) -> Result<(), CpuError> {
        const BLOCK: usize = 4096;
        let stack = [value; BLOCK];
        let mut remaining = len as usize;
        let mut location = addr;
        while remaining > 0 {
            let step = remaining.min(BLOCK);
            self.write_mem(location, &stack[..step])?;
            location = location.wrapping_add(step as u32);
            remaining -= step;
        }
        Ok(())
    }

    /// Zeroes `len` bytes of the guest stack **without** invalidating JIT code.
    ///
    /// Only for stack dirtying (see `Machine::spend_stack`): the range is confined to the game's
    /// stack, where there is never compiled code, and dirtying on every API call with the
    /// invalidation along cost 30% of the speed in Crash Nitro Kart mid-race (from 385% to 296%
    /// in `bench`, measured) — almost all of it in `jit_mut` and in the borrows of the marking,
    /// which here always comes out empty. Outside the stack, use [`Self::fill_mem`], which
    /// invalidates.
    fn mark_stack_dirty(&mut self, addr: u32, len: u32) -> Result<(), CpuError> {
        self.fill_mem(addr, 0, len)
    }

    fn read_u32(&self, addr: u32) -> Result<u32, CpuError> {
        let mut buf = [0u8; 4];
        self.read_mem(addr, &mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn write_u32(&mut self, addr: u32, value: u32) -> Result<(), CpuError> {
        self.write_mem(addr, &value.to_le_bytes())
    }

    /// Reads a zero-terminated string. Stops at the first access fault and at the given limit,
    /// so that a corrupted pointer does not turn into an endless read.
    /// The bytes of a guest string, up to the terminator — without interpreting the encoding.
    ///
    /// This is the version the C string functions need. Converting to `String` replaces each
    /// byte that is invalid in UTF-8 with `U+FFFD`, which takes **three** bytes:
    /// `strlen("\x89PNG")` would return 6 instead of 4, and that exact error is what made
    /// Bejeweled Twist reject its own PNGs.
    fn read_cbytes(&self, addr: u32, max_len: usize) -> Vec<u8> {
        /// How much to read at once. Each read is a call into the core, and byte by byte that
        /// was expensive where it hurts most: `strtoul` asks for up to `MAX_STRING` bytes and
        /// Turma da Mônica calls it a hundred thousand times, which came to hundreds of millions
        /// of calls just to convert numbers. Sixty-four bytes cover the typical short string in
        /// a single read.
        const BLOCK: usize = 64;

        let mut bytes = Vec::new();
        while bytes.len() < max_len {
            let wanted = BLOCK.min(max_len - bytes.len());
            let base = addr + bytes.len() as u32;
            let mut buf = vec![0u8; wanted];
            if self.read_mem(base, &mut buf).is_err() {
                // The block may cross the end of the mapped region, and then the whole read
                // fails even though there are valid bytes before it. Byte by byte only in this
                // case.
                for offset in 0..wanted {
                    let mut byte = [0u8; 1];
                    if self.read_mem(base + offset as u32, &mut byte).is_err() || byte[0] == 0 {
                        return bytes;
                    }
                    bytes.push(byte[0]);
                }
                continue;
            }
            match buf.iter().position(|&b| b == 0) {
                Some(end) => {
                    bytes.extend_from_slice(&buf[..end]);
                    return bytes;
                }
                None => bytes.extend_from_slice(&buf),
            }
        }
        bytes
    }

    /// The same string, interpreted as text.
    fn read_cstring(&self, addr: u32, max_len: usize) -> String {
        latin1_decode(&self.read_cbytes(addr, max_len))
    }
}

/// Interprets the bytes of a BREW `char` string as text.
///
/// BREW's `char` is ISO-8859-1: one byte, one character. Reading it as UTF-8 destroyed anything
/// with an accent — `0xE7` is not valid UTF-8, it became `U+FFFD` and went back into the game's
/// memory as three bytes of garbage. That is why accented text came out broken on the screen of
/// Resident Evil 4 and Double Dragon, which pass the text through `strtowstr`.
///
/// The mapping is direct because the first 256 Unicode code points *are* ISO-8859-1, and so the
/// round trip never loses a single byte.
pub fn latin1_decode(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// The way back: each character becomes one byte.
///
/// What does not fit in a byte did not come from guest memory and has no representation there;
/// it becomes `?`, which is what libc does with a character outside the code page.
pub fn latin1_encode(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| u8::try_from(c as u32).unwrap_or(b'?'))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuError(pub String);

impl std::fmt::Display for CpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ARM core failure: {}", self.0)
    }
}

impl std::error::Error for CpuError {}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "vita")))]
pub mod dynarmic;

/// A32/T32 interpreter. It steps in where the JIT cannot emit code the process can execute: on
/// `wasm32` the browser does not jump into the block, on iOS the kernel refuses the executable
/// page, and on the Vita the host is a Cortex-A9 — dynarmic has no ARMv7 backend. It also steps
/// in for the file's own tests. The desktop stays on the JIT.
#[cfg(any(test, target_arch = "wasm32", target_os = "ios", target_os = "vita"))]
pub mod interpreter;

/// The alias the rest of the code uses to ask for "the default backend".
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "vita")))]
pub type DefaultBackend = dynarmic::DynarmicCpu;
#[cfg(any(target_arch = "wasm32", target_os = "ios", target_os = "vita"))]
pub type DefaultBackend = interpreter::Interpreter;

// The constants of the BREW vtable range are part of the contract between the backend and the
// dispatcher.
pub use brew_ranges::{API_BASE, API_SIZE, RETURN_MAGIC};


/// The three constants of the range reserved for the BREW vtables.
///
/// They live in a module of their own because they belong **to the contract**, not to a backend:
/// this is how the Rust dispatcher finds out which API the game called.
mod brew_ranges {
    /// Base of the range reserved for the BREW vtables.
    pub const API_BASE: u32 = 0xf000_0000;
    /// Size of the range.
    pub const API_SIZE: u32 = 0x0100_0000;
    /// Sentinel address `lr` receives to mark "returned from the API".
    pub const RETURN_MAGIC: u32 = 0xfff0_0000;
}

/// How much of the semihosting log is kept, in bytes.
const MAX_SEMIHOSTING: usize = 256 * 1024;

/// Keeps the semihosting log within [`MAX_SEMIHOSTING`], throwing away the oldest half.
///
/// It was a `String` that only grew: Peggle and Zuma write through it for the whole session, and
/// the report clones all the text on every frame in which the log window is open.
pub(crate) fn trim_semihosting(text: &mut String) {
    if text.len() <= MAX_SEMIHOSTING {
        return;
    }
    let mut cutoff = text.len() - MAX_SEMIHOSTING / 2;
    while !text.is_char_boundary(cutoff) {
        cutoff += 1;
    }
    text.drain(..cutoff);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Accented text has to survive the round trip byte by byte: that is what the game keeps in
    /// its memory.
    #[test]
    fn latin1_round_trip_keeps_accents() {
        let bytes = b"Configura\xe7\xf5es";
        let text = latin1_decode(bytes);
        assert_eq!(text, "Configurações");
        assert_eq!(latin1_encode(&text), bytes);
    }

    #[test]
    fn latin1_covers_every_byte() {
        let bytes: Vec<u8> = (1..=255).collect();
        assert_eq!(latin1_encode(&latin1_decode(&bytes)), bytes);
    }
}
