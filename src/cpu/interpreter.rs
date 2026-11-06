//! ARM1176 interpreter (A32 and Thumb), the heart of the WebAssembly core.
//!
//! `dynarmic` and `unicorn` recompile the guest block into the host's native code and jump to it.
//! In a WebAssembly module that jump does not exist: the browser only executes Wasm. This file
//! reads the instruction from guest memory and executes it here, under the same [`CpuBackend`]
//! contract — stop at the vtable range, bit 0 of the address as Thumb mode, a write watch that
//! ignores what the host wrote.
//!
//! It has not been measured against the JIT. The desktop stays on `dynarmic`; this goes into
//! `wasm32`, where the emitted block does not run, and into iOS, where the system does not let the
//! process map code.

use super::mem::GuestMemory;
use super::{API_BASE, API_SIZE, RETURN_MAGIC, trim_semihosting};
use super::{CpuBackend, CpuError, Reg, StopReason};

const N: u32 = 1 << 31;
const Z: u32 = 1 << 30;
const C: u32 = 1 << 29;
const V: u32 = 1 << 28;
const Q: u32 = 1 << 27;
const T: u32 = 1 << 5;
const USER_MODE: u32 = 0x10;

impl Shortcut {
    const fn empty() -> Self {
        Self {
            base: 0,
            end: 0,
            ptr: std::ptr::null_mut(),
            writable: false,
        }
    }
}

/// Reads 1, 2 or 4 bytes at the address the shortcut already checked.
fn read_ptr(ptr: *mut u8, len: u32) -> u32 {
    unsafe {
        match len {
            1 => u32::from(ptr.read()),
            2 => u32::from(ptr.cast::<u16>().read_unaligned()),
            _ => ptr.cast::<u32>().read_unaligned(),
        }
    }
}

/// Semihosting operations the `unicorn` hook registers. The `SVC` immediate does not change the
/// course: any `SVC` looks at the operation in `r0`. The PopCap games initialize through here.
const SYS_WRITEC: u32 = 0x03;
const SYS_WRITE0: u32 = 0x04;
const MAX_SEMIHOSTING_STRING: u32 = 4096;

/// A pointer already resolved to a region. `end == 0` is empty.
///
/// Instruction fetch and data reads ask this before the region list. Within a frame the code
/// stays in the same region the whole time, and the data alternates among a handful — module,
/// stack, heap, surface. There are four data slots; the fifth reuses the oldest.
///
/// Measured on this Mac, in release, on a loop of `ADD` and `B` of two million instructions:
/// 17.0 ms looking up the region every time, 6.4 ms with the shortcut. It is not the Vita's frame.
#[derive(Clone, Copy)]
struct Shortcut {
    base: u32,
    end: u32,
    ptr: *mut u8,
    writable: bool,
}

pub struct Interpreter {
    mem: GuestMemory,
    r: [u32; 16],
    cpsr: u32,
    instructions: u64,
    watches: Vec<(u32, u32, u32, bool)>,
    semihosting: String,
    /// Address of the `LDREX` still without a `STREX`. Any write clears it: it is the exclusive
    /// monitor of a single core, which is what this process is.
    exclusive: Option<u32>,
    code: Shortcut,
    data: [Shortcut; 4],
    /// Which of the data shortcuts to replace on the next miss.
    next_data_shortcut: u8,
}

impl Interpreter {
    pub fn new() -> Result<Self, CpuError> {
        Ok(Self {
            mem: GuestMemory::new(),
            r: [0; 16],
            cpsr: USER_MODE,
            instructions: 0,
            watches: Vec::new(),
            semihosting: String::new(),
            exclusive: None,
            code: Shortcut::empty(),
            data: [Shortcut::empty(); 4],
            next_data_shortcut: 0,
        })
    }

    pub fn semihosting(&self) -> String {
        self.semihosting.clone()
    }

    fn thumb(&self) -> bool {
        self.cpsr & T != 0
    }

    fn flag_c(&self) -> bool {
        self.cpsr & C != 0
    }

    /// The value an instruction reads when it asks for the `PC`.
    ///
    /// On ARM the pipeline delivers the instruction's address plus 8; on Thumb, plus 4. `extra`
    /// covers the case of a shift with the amount in a register, where ARM delivers plus 12 — the
    /// instruction spends one more cycle and the `PC` seen moves along with it.
    fn pc_value(&self, pc: u32, extra: u32) -> u32 {
        let base = if self.thumb() { 4 } else { 8 };
        pc.wrapping_add(base + extra)
    }

    fn reg_value(&self, n: u32, pc: u32, extra: u32) -> u32 {
        if n == 15 {
            self.pc_value(pc, extra)
        } else {
            self.r[n as usize]
        }
    }

    fn set_nz(&mut self, value: u32) {
        self.cpsr = (self.cpsr & !(N | Z))
            | if value & 0x8000_0000 != 0 { N } else { 0 }
            | if value == 0 { Z } else { 0 };
    }

    fn set_c(&mut self, carry: bool) {
        if carry {
            self.cpsr |= C;
        } else {
            self.cpsr &= !C;
        }
    }

    fn set_v(&mut self, overflow: bool) {
        if overflow {
            self.cpsr |= V;
        } else {
            self.cpsr &= !V;
        }
    }

    fn set_q(&mut self) {
        self.cpsr |= Q;
    }

    /// A branch that does **not** switch instruction set. `MOV PC` and `B` stay in the mode they
    /// were in; `BX` is what switches.
    fn branch_no_exchange(&mut self, target: u32) {
        self.r[15] = if self.thumb() { target & !1 } else { target & !3 };
    }

    fn bx(&mut self, target: u32) {
        if target & 1 != 0 {
            self.cpsr |= T;
            self.r[15] = target & !1;
        } else {
            self.cpsr &= !T;
            self.r[15] = target & !3;
        }
    }

    fn search(&mut self, addr: u32, len: u32) -> Result<u32, StopReason> {
        if addr == RETURN_MAGIC {
            return Err(StopReason::Returned);
        }
        if (API_BASE..API_BASE.saturating_add(API_SIZE)).contains(&addr) {
            return Err(StopReason::ApiCall { addr });
        }
        let Some(ptr) = self.code_ptr(addr, len) else {
            return Err(StopReason::MemoryFault { addr, pc: addr });
        };
        Ok(read_ptr(ptr, len))
    }

    fn load_mem(&mut self, addr: u32, len: u32, pc: u32) -> Result<u32, StopReason> {
        let Some(ptr) = self.data_ptr(addr, len, false) else {
            return Err(StopReason::MemoryFault { addr, pc });
        };
        Ok(read_ptr(ptr, len))
    }

    fn store_mem(&mut self, addr: u32, data: &[u8], pc: u32) -> Result<(), StopReason> {
        let len = data.len() as u32;
        if let Some(ptr) = self.data_ptr(addr, len, true) {
            // SAFETY: `data_ptr` only returns a pointer to a writable region that covers the
            // request.
            unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len()) };
        } else {
            self.mem
                .write(addr, data)
                .map_err(|_| StopReason::MemoryFault { addr, pc })?;
        }
        self.exclusive = None;
        self.mark_watches(addr, data.len() as u32);
        Ok(())
    }

    /// Pointer to the code region that covers the request. The region has to allow execution.
    fn code_ptr(&mut self, addr: u32, len: u32) -> Option<*mut u8> {
        let end = addr.checked_add(len)?;
        if self.code.end != 0 && addr >= self.code.base && end <= self.code.end {
            let off = (addr - self.code.base) as usize;
            return Some(unsafe { self.code.ptr.add(off) });
        }
        let window = self.mem.window_mut(addr)?;
        if !window.executable {
            return None;
        }
        let window_end = window.base.checked_add(window.len)?;
        if end > window_end {
            return None;
        }
        self.code = Shortcut {
            base: window.base,
            end: window_end,
            ptr: window.ptr,
            writable: window.writable,
        };
        let off = (addr - window.base) as usize;
        Some(unsafe { self.code.ptr.add(off) })
    }

    /// Pointer to a data region. `for_writing` refuses a read-only one, so the slow path returns
    /// the same error as before.
    fn data_ptr(&mut self, addr: u32, len: u32, for_writing: bool) -> Option<*mut u8> {
        let end = addr.checked_add(len)?;
        for slot in &self.data {
            if slot.end != 0
                && addr >= slot.base
                && end <= slot.end
                && (!for_writing || slot.writable)
            {
                let off = (addr - slot.base) as usize;
                return Some(unsafe { slot.ptr.add(off) });
            }
        }
        let window = self.mem.window_mut(addr)?;
        let window_end = window.base.checked_add(window.len)?;
        if end > window_end || (for_writing && !window.writable) {
            return None;
        }
        let which = (self.next_data_shortcut as usize) & 3;
        self.next_data_shortcut = self.next_data_shortcut.wrapping_add(1);
        self.data[which] = Shortcut {
            base: window.base,
            end: window_end,
            ptr: window.ptr,
            writable: window.writable,
        };
        let off = (addr - window.base) as usize;
        Some(unsafe { self.data[which].ptr.add(off) })
    }

    fn mark_watches(&mut self, addr: u32, len: u32) {
        let end = addr.saturating_add(len);
        for watch in &mut self.watches {
            if addr < watch.2 && end > watch.1 {
                watch.3 = true;
            }
        }
    }

    /// `true` when the instruction has already written the `PC`.
    fn step(&mut self) -> Result<bool, StopReason> {
        let pc = self.r[15];
        if self.thumb() {
            if pc & 1 != 0 {
                return Err(StopReason::Exception { pc });
            }
            let op = self.search(pc, 2)?;
            self.thumb_exec(op as u16, pc)
        } else {
            if pc & 3 != 0 {
                return Err(StopReason::Exception { pc });
            }
            let op = self.search(pc, 4)?;
            self.arm_exec(op, pc)
        }
    }

    fn arm_exec(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let cond = instr >> 28;
        if cond == 0xF {
            return self.arm_unconditional(instr, pc);
        }
        if !condition_passed(self.cpsr, cond) {
            return Ok(false);
        }
        match (instr >> 25) & 7 {
            0b000 => self.arm_000(instr, pc),
            0b001 => self.arm_data(instr, pc, true),
            0b010 | 0b011 => self.arm_ls(instr, pc),
            0b100 => self.arm_block(instr, pc),
            0b101 => self.arm_b(instr, pc),
            0b111 if instr & (1 << 24) != 0 => self.svc(pc),
            _ => Err(StopReason::Exception { pc }),
        }
    }

    /// Immediate `BLX` and the hints (`PLD`, `SETEND`). The rest of this space is not an ARM1176
    /// user instruction that we execute.
    fn arm_unconditional(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        if (instr >> 25) & 7 == 0b101 {
            let h = (instr >> 24) & 1;
            let imm = sext(instr & 0x00FF_FFFF, 24) << 2;
            let target = pc.wrapping_add(8).wrapping_add(imm).wrapping_add(h * 2);
            self.r[14] = pc.wrapping_add(4);
            self.cpsr |= T;
            self.r[15] = target & !1;
            return Ok(true);
        }
        // PLD and the other preloads are hints. SETEND to big-endian is not: memory here is
        // little-endian, and executing the instruction "correctly" without swapping the order
        // would be a lie.
        if instr & 0x0FF0_0FF0 == 0x0100_0100 && instr & (1 << 9) != 0 {
            return Err(StopReason::Exception { pc });
        }
        if (instr >> 25) & 7 == 0b010
            || (instr >> 25) & 7 == 0b011
            || instr & 0x0FFF_0FFF == 0x0100_0000
        {
            return Ok(false);
        }
        Err(StopReason::Exception { pc })
    }

    fn arm_000(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let bit4 = instr & (1 << 4) != 0;
        let bit7 = instr & (1 << 7) != 0;
        if bit4 && bit7 && (instr >> 5) & 3 != 0 {
            return self.arm_extra(instr, pc);
        }
        if bit4 && bit7 {
            return self.arm_mul(instr, pc);
        }
        // TST/TEQ/CMP/CMN with S=0 are not ALU: this is the space of BX, CLZ, MSR, QADD.
        if (instr >> 23) & 0x1F == 0b00010 && instr & (1 << 20) == 0 {
            return self.arm_misc(instr, pc);
        }
        self.arm_data(instr, pc, false)
    }

    fn arm_b(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let imm = sext(instr & 0x00FF_FFFF, 24) << 2;
        let target = pc.wrapping_add(8).wrapping_add(imm);
        if instr & (1 << 24) != 0 {
            self.r[14] = pc.wrapping_add(4);
        }
        self.branch_no_exchange(target);
        Ok(true)
    }

    fn svc(&mut self, pc: u32) -> Result<bool, StopReason> {
        let op = self.r[0];
        let arg = self.r[1];
        match op {
            SYS_WRITEC => {
                if let Ok(b) = self.load_mem(arg, 1, pc) {
                    self.semihosting.push(b as u8 as char);
                }
            }
            SYS_WRITE0 => {
                for offset in 0..MAX_SEMIHOSTING_STRING {
                    let Ok(b) = self.load_mem(arg.wrapping_add(offset), 1, pc) else {
                        break;
                    };
                    if b == 0 {
                        break;
                    }
                    self.semihosting.push(b as u8 as char);
                }
            }
            _ => {}
        }
        trim_semihosting(&mut self.semihosting);
        self.r[0] = 0;
        Ok(false)
    }

    fn arm_misc(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        if instr & 0x0FFF_FFF0 == 0x012F_FF10 {
            self.bx(self.reg_value(instr & 0xF, pc, 0));
            return Ok(true);
        }
        if instr & 0x0FFF_FFF0 == 0x012F_FF30 {
            self.r[14] = pc.wrapping_add(4);
            self.bx(self.reg_value(instr & 0xF, pc, 0));
            return Ok(true);
        }
        if instr & 0x0FFF_0FF0 == 0x016F_0F10 {
            let rm = self.reg_value(instr & 0xF, pc, 0);
            let rd = (instr >> 12) & 0xF;
            return self.set_rd(rd, rm.leading_zeros(), pc);
        }
        if instr & 0x0FFF_0FFF == 0x010F_0000 {
            let rd = (instr >> 12) & 0xF;
            return self.set_rd(rd, self.cpsr, pc);
        }
        if instr & 0x0FB0_FFF0 == 0x0120_F000 {
            let value = self.reg_value(instr & 0xF, pc, 0);
            self.msr((instr >> 16) & 0xF, value);
            return Ok(false);
        }
        if instr & 0x0FF0_00F0 == 0x0120_0070 {
            return Err(StopReason::Exception { pc });
        }
        // QADD / QSUB / QDADD / QDSUB: bits 7–4 = 0101.
        if instr & 0x0F90_00F0 == 0x0100_0050 {
            return self.arm_qadd(instr, pc);
        }
        // SMULxy / SMLAxy / SMULWy / SMLAWy: bit 7 ligado e bit 4 desligado.
        if instr & 0x0F00_0090 == 0x0100_0080 {
            return self.arm_dsp_mul(instr, pc);
        }
        Err(StopReason::Exception { pc })
    }

    fn arm_qadd(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let rn = self.reg_value((instr >> 16) & 0xF, pc, 0) as i32 as i64;
        let rm = self.reg_value(instr & 0xF, pc, 0) as i32 as i64;
        let op = (instr >> 21) & 3;
        let (doubled, sat1) = if op & 2 != 0 {
            sat_i32(rm.saturating_mul(2))
        } else {
            (rm, false)
        };
        // The saturated doubling already sets Q, even if the following sum fits.
        if sat1 {
            self.set_q();
        }
        let sum = match op {
            0 | 2 => rn + doubled,
            _ => rn - doubled,
        };
        let (value, sat2) = sat_i32(sum);
        if sat2 {
            self.set_q();
        }
        self.set_rd((instr >> 12) & 0xF, value as u32, pc)
    }

    fn arm_dsp_mul(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let rd = (instr >> 12) & 0xF;
        let rn = self.reg_value((instr >> 16) & 0xF, pc, 0);
        let rs = self.reg_value((instr >> 8) & 0xF, pc, 0);
        let rm = self.reg_value(instr & 0xF, pc, 0);
        let x = (instr >> 5) & 1 != 0;
        let y = (instr >> 6) & 1 != 0;
        // Bits 22–21 separate the forms; bit 5 (x) separates SMULW from SMLAW within the same one.
        let product: i64 = match (instr >> 21) & 3 {
            // SMLAxy: accumulates the product of the halves into Rn.
            0 => {
                let (sum, sat) =
                    sat_i32(halfword(rm, x) as i64 * halfword(rs, y) as i64 + rn as i32 as i64);
                if sat {
                    self.set_q();
                }
                sum
            }
            // SMLAWy (x = 0) accumulates; SMULWy (x = 1) does not. SMULW's Rn field is zero in
            // the encoding, but that names r0 — it must not enter the sum.
            1 => {
                let p = ((rm as i32 as i64) * halfword(rs, y) as i64) >> 16;
                if x {
                    p
                } else {
                    let (sum, sat) = sat_i32(p + rn as i32 as i64);
                    if sat {
                        self.set_q();
                    }
                    sum
                }
            }
            // SMULxy: only the product, no accumulator and no flag.
            3 => halfword(rm, x) as i64 * halfword(rs, y) as i64,
            _ => return Err(StopReason::Exception { pc }),
        };
        self.set_rd(rd, product as u32, pc)
    }

    fn msr(&mut self, field: u32, value: u32) {
        // In user mode only the `f` field (flags) can be written. The other `CPSR` bits requested
        // here belong to privileged mode; ignoring them is what the core does with the game.
        if field & 0x8 != 0 {
            let flags = N | Z | C | V | Q;
            self.cpsr = (self.cpsr & !flags) | (value & flags);
        }
    }

    fn arm_data(&mut self, instr: u32, pc: u32, immediate: bool) -> Result<bool, StopReason> {
        let opcode = (instr >> 21) & 0xF;
        let s = instr & (1 << 20) != 0;
        let rn_i = (instr >> 16) & 0xF;
        let rd = (instr >> 12) & 0xF;
        if (8..12).contains(&opcode) && !s {
            if immediate && opcode == 0b1001 && rd == 0xF {
                // Immediate MSR: the encoding shares the immediate with the data-processing
                // shifter.
                let imm = instr & 0xFF;
                let rot = ((instr >> 8) & 0xF) * 2;
                self.msr((instr >> 16) & 0xF, imm.rotate_right(rot));
                return Ok(false);
            }
            return Err(StopReason::Exception { pc });
        }
        let extra = u32::from(!immediate && instr & (1 << 4) != 0) * 4;
        let (operand, carry_sh) = if immediate {
            let imm = instr & 0xFF;
            let rot = ((instr >> 8) & 0xF) * 2;
            let value = imm.rotate_right(rot);
            let carry = if rot == 0 {
                self.flag_c()
            } else {
                value & 0x8000_0000 != 0
            };
            (value, carry)
        } else {
            self.reg_operand(instr, pc)?
        };
        let a = self.reg_value(rn_i, pc, extra);
        let c_in = self.flag_c();
        let (value, write, logical, c, v) = alu(opcode, a, operand, c_in);
        if s && rd == 15 {
            return Err(StopReason::Exception { pc });
        }
        if s {
            self.set_nz(value);
            if logical {
                self.set_c(carry_sh);
            } else {
                self.set_c(c);
                self.set_v(v);
            }
        }
        if write {
            self.set_rd(rd, value, pc)
        } else {
            Ok(false)
        }
    }

    fn reg_operand(&mut self, instr: u32, pc: u32) -> Result<(u32, bool), StopReason> {
        let rm = self.reg_value(instr & 0xF, pc, u32::from(instr & (1 << 4) != 0) * 4);
        let kind = (instr >> 5) & 3;
        let (amount, immediate) = if instr & (1 << 4) != 0 {
            let rs = self.reg_value((instr >> 8) & 0xF, pc, 4);
            (rs & 0xFF, false)
        } else {
            ((instr >> 7) & 0x1F, true)
        };
        Ok(shift(rm, kind, amount, self.flag_c(), immediate))
    }

    fn set_rd(&mut self, rd: u32, value: u32, _pc: u32) -> Result<bool, StopReason> {
        if rd == 15 {
            self.branch_no_exchange(value);
            Ok(true)
        } else {
            self.r[rd as usize] = value;
            Ok(false)
        }
    }

    fn set_loaded_reg(&mut self, rd: u32, value: u32) -> bool {
        if rd == 15 {
            // LDR/LDM to the PC switch mode by bit 0. Data processing does not switch.
            self.bx(value);
            true
        } else {
            self.r[rd as usize] = value;
            false
        }
    }

    fn arm_ls(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let reg_form = (instr >> 25) & 1 == 1;
        if reg_form && instr & (1 << 4) != 0 {
            return self.arm_media(instr, pc);
        }
        let p = instr & (1 << 24) != 0;
        let u = instr & (1 << 23) != 0;
        let b = instr & (1 << 22) != 0;
        let w = instr & (1 << 21) != 0;
        let l = instr & (1 << 20) != 0;
        let rn = (instr >> 16) & 0xF;
        let rd = (instr >> 12) & 0xF;
        let offset = if reg_form {
            let (value, _) = self.reg_operand(instr, pc)?;
            value
        } else {
            instr & 0xFFF
        };
        let base = self.reg_value(rn, pc, 0);
        let modified = if u {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let address = if p { modified } else { base };
        let branched = if l {
            let value = if b {
                self.load_mem(address, 1, pc)?
            } else {
                self.load_mem(address, 4, pc)?
            };
            self.set_loaded_reg(rd, value)
        } else {
            let value = self.reg_value(rd, pc, 0);
            if b {
                self.store_mem(address, &[value as u8], pc)?;
            } else {
                self.store_mem(address, &value.to_le_bytes(), pc)?;
            }
            false
        };
        if (w || !p) && rn != 15 {
            self.r[rn as usize] = modified;
        }
        Ok(branched)
    }

    fn arm_extra(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let p = instr & (1 << 24) != 0;
        let u = instr & (1 << 23) != 0;
        let i = instr & (1 << 22) != 0;
        let w = instr & (1 << 21) != 0;
        let l = instr & (1 << 20) != 0;
        let rn = (instr >> 16) & 0xF;
        let rd = (instr >> 12) & 0xF;
        let sh = (instr >> 5) & 3;
        let offset = if i {
            ((instr >> 4) & 0xF0) | (instr & 0xF)
        } else {
            self.reg_value(instr & 0xF, pc, 0)
        };
        let base = self.reg_value(rn, pc, 0);
        let modified = if u {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let address = if p { modified } else { base };
        // sh: 1 unsigned halfword, 2 signed byte (or doubleword, if it is a store), 3 signed
        // halfword.
        let branched = match (l, sh) {
            (false, 1) => {
                let value = self.reg_value(rd, pc, 0);
                self.store_mem(address, &(value as u16).to_le_bytes(), pc)?;
                false
            }
            (true, 1) => {
                let value = self.load_mem(address, 2, pc)?;
                self.set_loaded_reg(rd, value)
            }
            (true, 2) => {
                let value = self.load_mem(address, 1, pc)? as i8 as i32 as u32;
                self.set_loaded_reg(rd, value)
            }
            (true, 3) => {
                let value = self.load_mem(address, 2, pc)? as i16 as i32 as u32;
                self.set_loaded_reg(rd, value)
            }
            (false, 2) => {
                // Is LDRD encoded with L=0 and S=1 H=0 STRD? No: L=0 S=1 H=0 is LDRD.
                // The table: L=0 S=1 H=0 loads the pair. What stores the pair is L=0 S=1 H=1.
                let lo = self.load_mem(address, 4, pc)?;
                let hi = self.load_mem(address.wrapping_add(4), 4, pc)?;
                let s1 = self.set_loaded_reg(rd, lo);
                let s2 = self.set_loaded_reg(rd.wrapping_add(1) & 0xF, hi);
                s1 || s2
            }
            (false, 3) => {
                let lo = self.reg_value(rd, pc, 0);
                let hi = self.reg_value(rd.wrapping_add(1) & 0xF, pc, 0);
                self.store_mem(address, &lo.to_le_bytes(), pc)?;
                self.store_mem(address.wrapping_add(4), &hi.to_le_bytes(), pc)?;
                false
            }
            _ => return Err(StopReason::Exception { pc }),
        };
        if (w || !p) && rn != 15 {
            self.r[rn as usize] = modified;
        }
        Ok(branched)
    }

    fn arm_block(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let p = instr & (1 << 24) != 0;
        let u = instr & (1 << 23) != 0;
        let s = instr & (1 << 22) != 0;
        let w = instr & (1 << 21) != 0;
        let l = instr & (1 << 20) != 0;
        let rn = (instr >> 16) & 0xF;
        let list = instr & 0xFFFF;
        let n = list.count_ones();
        if n == 0 {
            return Err(StopReason::Exception { pc });
        }
        if s && l && list & (1 << 15) != 0 {
            return Err(StopReason::Exception { pc });
        }
        let bytes = n * 4;
        let base = self.reg_value(rn, pc, 0);
        let (mut address, create) = match (u, p) {
            (true, false) => (base, base.wrapping_add(bytes)),
            (true, true) => (base.wrapping_add(4), base.wrapping_add(bytes)),
            (false, false) => (
                base.wrapping_sub(bytes).wrapping_add(4),
                base.wrapping_sub(bytes),
            ),
            (false, true) => (base.wrapping_sub(bytes), base.wrapping_sub(bytes)),
        };
        let mut branched = false;
        if l {
            let mut values = Vec::with_capacity(n as usize);
            let mut cursor = address;
            for _ in 0..n {
                values.push(self.load_mem(cursor, 4, pc)?);
                cursor = cursor.wrapping_add(4);
            }
            let mut i = 0;
            for reg in 0..16 {
                if list & (1 << reg) != 0 {
                    if self.set_loaded_reg(reg, values[i]) {
                        branched = true;
                    }
                    i += 1;
                }
            }
        } else {
            for reg in 0..16 {
                if list & (1 << reg) != 0 {
                    let value = self.reg_value(reg, pc, 0);
                    self.store_mem(address, &value.to_le_bytes(), pc)?;
                    address = address.wrapping_add(4);
                }
            }
        }
        if w && rn != 15 && (l && list & (1 << rn) == 0 || !l) {
            self.r[rn as usize] = create;
        }
        Ok(branched)
    }

    fn arm_mul(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        // SWP and LDREX/STREX share bit 23 with the multiply family, but not the rest of the
        // mask: handling the two before `op` avoids reading a SWP as UMULL.
        if instr & 0x0FB0_0FF0 == 0x0100_0090 {
            return self.arm_swp(instr, pc);
        }
        if instr & 0x0FF0_0FFF == 0x0190_0F9F || instr & 0x0FF0_0FF0 == 0x0180_0F90 {
            return self.arm_exclusive(instr, pc);
        }
        let s = instr & (1 << 20) != 0;
        let rd = (instr >> 16) & 0xF;
        let rn = (instr >> 12) & 0xF;
        let rs = self.reg_value((instr >> 8) & 0xF, pc, 0);
        let rm = self.reg_value(instr & 0xF, pc, 0);
        let op = (instr >> 21) & 0xF;
        let branched = match op {
            0b0000 => {
                // MUL Rd, Rm, Rs. The Rn field has to be zero; if it is not, it is still MUL on v4.
                let value = rm.wrapping_mul(rs);
                if s {
                    self.set_nz(value);
                }
                self.set_rd(rd, value, pc)?
            }
            0b0001 => {
                let acc = self.reg_value(rn, pc, 0);
                let value = rm.wrapping_mul(rs).wrapping_add(acc);
                if s {
                    self.set_nz(value);
                }
                self.set_rd(rd, value, pc)?
            }
            0b0100 | 0b0101 | 0b0110 | 0b0111 => {
                let signed = op & 0b0010 != 0;
                let accumulate = op & 1 != 0;
                let product = if signed {
                    (rm as i32 as i64).wrapping_mul(rs as i32 as i64) as u64
                } else {
                    u64::from(rm).wrapping_mul(u64::from(rs))
                };
                let product = if accumulate {
                    let lo = self.reg_value(rn, pc, 0);
                    let hi = self.reg_value(rd, pc, 0);
                    product.wrapping_add(u64::from(lo) | (u64::from(hi) << 32))
                } else {
                    product
                };
                let lo = product as u32;
                let hi = (product >> 32) as u32;
                if s {
                    let flags = if hi & 0x8000_0000 != 0 { N } else { 0 }
                        | if product == 0 { Z } else { 0 };
                    self.cpsr = (self.cpsr & !(N | Z)) | flags;
                }
                let s1 = self.set_rd(rn, lo, pc)?;
                let s2 = self.set_rd(rd, hi, pc)?;
                s1 || s2
            }
            _ => return Err(StopReason::Exception { pc }),
        };
        Ok(branched)
    }

    fn arm_swp(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        if instr & 0x0FB0_0FF0 != 0x0100_0090 {
            return Err(StopReason::Exception { pc });
        }
        let rn = (instr >> 16) & 0xF;
        let rd = (instr >> 12) & 0xF;
        let rm = self.reg_value(instr & 0xF, pc, 0);
        let addr = self.reg_value(rn, pc, 0);
        let byte = instr & (1 << 22) != 0;
        if byte {
            let old = self.load_mem(addr, 1, pc)?;
            self.store_mem(addr, &[rm as u8], pc)?;
            self.set_rd(rd, old, pc)
        } else {
            let old = self.load_mem(addr, 4, pc)?;
            self.store_mem(addr, &rm.to_le_bytes(), pc)?;
            self.set_rd(rd, old, pc)
        }
    }

    fn arm_exclusive(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let rn = (instr >> 16) & 0xF;
        let rd = (instr >> 12) & 0xF;
        let addr = self.reg_value(rn, pc, 0);
        // LDREX: bits 27–20 = 0001 1001. STREX: 0001 1000.
        if instr & 0x0FF0_0FFF == 0x0190_0F9F {
            let value = self.load_mem(addr, 4, pc)?;
            self.exclusive = Some(addr);
            return self.set_rd(rd, value, pc);
        }
        if instr & 0x0FF0_0FF0 == 0x0180_0F90 {
            let rm = self.reg_value(instr & 0xF, pc, 0);
            let ok = self.exclusive == Some(addr);
            self.exclusive = None;
            if ok {
                self.store_mem(addr, &rm.to_le_bytes(), pc)?;
            }
            return self.set_rd(rd, u32::from(!ok), pc);
        }
        Err(StopReason::Exception { pc })
    }

    fn arm_media(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let rd = (instr >> 12) & 0xF;
        let rn = (instr >> 16) & 0xF;
        let rm = self.reg_value(instr & 0xF, pc, 0);
        let head = (instr >> 20) & 0xFF;
        // REV / REV16 / REVSH live in the middle of the extension space, with Rn = 15 and an
        // opcode of their own in bits 11–4.
        if instr & 0x0FFF_0FF0 == 0x06BF_0F30 {
            return self.set_rd(rd, rm.swap_bytes(), pc);
        }
        if instr & 0x0FFF_0FF0 == 0x06BF_0FB0 {
            let value = ((rm & 0x00FF_00FF) << 8) | ((rm & 0xFF00_FF00) >> 8);
            return self.set_rd(rd, value, pc);
        }
        if instr & 0x0FFF_0FF0 == 0x06FF_0FB0 {
            let value = (rm as u16).swap_bytes() as i16 as i32 as u32;
            return self.set_rd(rd, value, pc);
        }
        if matches!(head, 0x6A | 0x6B | 0x6E | 0x6F) && instr & 0x70 == 0x70 {
            let rot = ((instr >> 10) & 3) * 8;
            let rotated = rm.rotate_right(rot);
            let halfword = head & 1 == 1;
            let sign = head & 4 == 0;
            let extended = if halfword {
                if sign {
                    rotated as u16 as i16 as i32 as u32
                } else {
                    u32::from(rotated as u16)
                }
            } else if sign {
                rotated as u8 as i8 as i32 as u32
            } else {
                u32::from(rotated as u8)
            };
            let value = if rn == 15 {
                extended
            } else {
                self.reg_value(rn, pc, 0).wrapping_add(extended)
            };
            return self.set_rd(rd, value, pc);
        }
        // SSAT is 0110101x, USAT is 0110111x. The leftover bit is the saturation immediate, and
        // bits 5–4 = 01 separate this from the extension, which already returned above.
        if matches!(head & 0xFE, 0x6A | 0x6E) && instr & 0x30 == 0x10 {
            return self.arm_sat(instr, pc, head & 0xFE == 0x6A);
        }
        Err(StopReason::Exception { pc })
    }

    fn arm_sat(&mut self, instr: u32, pc: u32, signed: bool) -> Result<bool, StopReason> {
        let rd = (instr >> 12) & 0xF;
        let rn = self.reg_value(instr & 0xF, pc, 0);
        let sat_imm = (instr >> 16) & 0x1F;
        let sh = (instr >> 6) & 1;
        let imm5 = (instr >> 7) & 0x1F;
        let mut value = rn as i32 as i64;
        if sh == 0 {
            value <<= imm5;
        } else {
            let q = if imm5 == 0 { 32 } else { imm5 };
            value >>= q;
        }
        let (output, sat) = if signed {
            let bits = sat_imm + 1;
            let max = (1i64 << (bits - 1)) - 1;
            let min = -(1i64 << (bits - 1));
            if value > max {
                (max as u32, true)
            } else if value < min {
                (min as u32, true)
            } else {
                (value as u32, false)
            }
        } else {
            let max = if sat_imm == 31 {
                u32::MAX as i64
            } else {
                (1i64 << sat_imm) - 1
            };
            if value < 0 {
                (0, true)
            } else if value > max {
                (max as u32, true)
            } else {
                (value as u32, false)
            }
        };
        if sat {
            self.set_q();
        }
        self.set_rd(rd, output, pc)
    }

    fn thumb_exec(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let op = instr >> 11;
        match op {
            0b00000 | 0b00001 | 0b00010 => self.thumb_shift_imm(instr, pc),
            0b00011 => self.thumb_add_sub(instr, pc),
            0b00100 | 0b00101 | 0b00110 | 0b00111 => self.thumb_imm(instr, pc),
            0b01000 => {
                if instr & (1 << 10) == 0 {
                    self.thumb_alu(instr, pc)
                } else {
                    self.thumb_high(instr, pc)
                }
            }
            0b01001 => self.thumb_ldr_pc(instr, pc),
            0b01010 | 0b01011 => self.thumb_ls_reg(instr, pc),
            0b01100 | 0b01101 | 0b01110 | 0b01111 => self.thumb_ls_imm(instr, pc, false),
            0b10000 | 0b10001 => self.thumb_ls_imm(instr, pc, true),
            0b10010 | 0b10011 => self.thumb_ls_sp(instr, pc),
            0b10100 | 0b10101 => self.thumb_adr(instr, pc),
            0b10110 | 0b10111 => self.thumb_misc(instr, pc),
            0b11000 | 0b11001 => self.thumb_block(instr, pc),
            0b11010 | 0b11011 => self.thumb_bcond(instr, pc),
            0b11100 => {
                let imm = sext(u32::from(instr & 0x7FF), 11) << 1;
                self.branch_no_exchange(pc.wrapping_add(4).wrapping_add(imm));
                Ok(true)
            }
            0b11101 | 0b11110 | 0b11111 => self.thumb_bl(instr, pc),
            _ => Err(StopReason::Exception { pc }),
        }
    }

    fn thumb_shift_imm(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let kind = u32::from((instr >> 11) & 3);
        let amount = u32::from((instr >> 6) & 0x1F);
        let rm = self.reg_value(u32::from((instr >> 3) & 7), pc, 0);
        let rd = u32::from(instr & 7);
        let (value, carry) = shift(rm, kind, amount, self.flag_c(), true);
        self.set_nz(value);
        self.set_c(carry);
        self.r[rd as usize] = value;
        Ok(false)
    }

    fn thumb_add_sub(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rd = u32::from(instr & 7);
        let rn = self.reg_value(u32::from((instr >> 3) & 7), pc, 0);
        let immediate = instr & (1 << 10) != 0;
        let sub = instr & (1 << 9) != 0;
        let op = if immediate {
            u32::from((instr >> 6) & 7)
        } else {
            self.reg_value(u32::from((instr >> 6) & 7), pc, 0)
        };
        let (value, c, v) = if sub {
            sub_with_carry(rn, op, 0)
        } else {
            add_with_carry(rn, op, 0)
        };
        self.set_nz(value);
        self.set_c(c);
        self.set_v(v);
        self.r[rd as usize] = value;
        Ok(false)
    }

    fn thumb_imm(&mut self, instr: u16, _pc: u32) -> Result<bool, StopReason> {
        let kind = (instr >> 11) & 3;
        let rd = u32::from((instr >> 8) & 7);
        let imm = u32::from(instr & 0xFF);
        let current = self.r[rd as usize];
        match kind {
            0 => {
                self.r[rd as usize] = imm;
                self.set_nz(imm);
            }
            1 => {
                let (value, c, v) = sub_with_carry(current, imm, 0);
                self.set_nz(value);
                self.set_c(c);
                self.set_v(v);
            }
            2 => {
                let (value, c, v) = add_with_carry(current, imm, 0);
                self.r[rd as usize] = value;
                self.set_nz(value);
                self.set_c(c);
                self.set_v(v);
            }
            _ => {
                let (value, c, v) = sub_with_carry(current, imm, 0);
                self.r[rd as usize] = value;
                self.set_nz(value);
                self.set_c(c);
                self.set_v(v);
            }
        }
        Ok(false)
    }

    fn thumb_alu(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let op = (instr >> 6) & 0xF;
        let rm = self.reg_value(u32::from((instr >> 3) & 7), pc, 0);
        let rd_i = u32::from(instr & 7);
        let rd = self.r[rd_i as usize];
        let c_in = self.flag_c();
        let (value, logical, c, v, write) = match op {
            0x0 => (rd & rm, true, c_in, false, true),
            0x1 => (rd ^ rm, true, c_in, false, true),
            0x2 => {
                let (v, c) = shift(rd, 0, rm & 0xFF, c_in, false);
                (v, true, c, false, true)
            }
            0x3 => {
                let (v, c) = shift(rd, 1, rm & 0xFF, c_in, false);
                (v, true, c, false, true)
            }
            0x4 => {
                let (v, c) = shift(rd, 2, rm & 0xFF, c_in, false);
                (v, true, c, false, true)
            }
            0x5 => {
                let (v, c, ov) = add_with_carry(rd, rm, u32::from(c_in));
                (v, false, c, ov, true)
            }
            0x6 => {
                let (v, c, ov) = sub_with_carry(rd, rm, u32::from(!c_in));
                (v, false, c, ov, true)
            }
            0x7 => {
                let (v, c) = shift(rd, 3, rm & 0xFF, c_in, false);
                (v, true, c, false, true)
            }
            0x8 => (rd & rm, true, c_in, false, false),
            0x9 => {
                let (v, c, ov) = sub_with_carry(0, rm, 0);
                (v, false, c, ov, true)
            }
            0xA => {
                let (v, c, ov) = sub_with_carry(rd, rm, 0);
                (v, false, c, ov, false)
            }
            0xB => {
                let (v, c, ov) = add_with_carry(rd, rm, 0);
                (v, false, c, ov, false)
            }
            0xC => (rd | rm, true, c_in, false, true),
            0xD => (rd.wrapping_mul(rm), true, c_in, false, true),
            0xE => (rd & !rm, true, c_in, false, true),
            _ => (!rm, true, c_in, false, true),
        };
        self.set_nz(value);
        if logical {
            // Thumb's MUL on ARMv6 does not touch C. The shifts do.
            if matches!(op, 0x2 | 0x3 | 0x4 | 0x7) {
                self.set_c(c);
            }
        } else {
            self.set_c(c);
            self.set_v(v);
        }
        if write {
            self.r[rd_i as usize] = value;
        }
        Ok(false)
    }

    fn thumb_high(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let op = (instr >> 8) & 3;
        let h1 = u32::from((instr >> 7) & 1);
        let h2 = u32::from((instr >> 6) & 1);
        let rm = (u32::from((instr >> 3) & 7)) | (h2 << 3);
        let rd = (u32::from(instr & 7)) | (h1 << 3);
        let a = self.reg_value(rd, pc, 0);
        let b = self.reg_value(rm, pc, 0);
        match op {
            0 => {
                let value = a.wrapping_add(b);
                if rd == 15 {
                    self.branch_no_exchange(value);
                    Ok(true)
                } else {
                    self.r[rd as usize] = value;
                    Ok(false)
                }
            }
            1 => {
                let (value, c, v) = sub_with_carry(a, b, 0);
                self.set_nz(value);
                self.set_c(c);
                self.set_v(v);
                Ok(false)
            }
            2 => {
                if rd == 15 {
                    self.branch_no_exchange(b);
                    Ok(true)
                } else {
                    self.r[rd as usize] = b;
                    Ok(false)
                }
            }
            _ => {
                if h1 != 0 {
                    self.r[14] = pc.wrapping_add(2) | 1;
                }
                self.bx(b);
                Ok(true)
            }
        }
    }

    fn thumb_ldr_pc(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rd = u32::from((instr >> 8) & 7);
        let imm = u32::from(instr & 0xFF) << 2;
        // Bit 1 of the PC is forced to zero: the word is aligned, even with the instruction at
        // an odd halfword address.
        let addr = (pc.wrapping_add(4) & !3).wrapping_add(imm);
        let value = self.load_mem(addr, 4, pc)?;
        self.r[rd as usize] = value;
        Ok(false)
    }

    fn thumb_ls_reg(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rd = u32::from(instr & 7);
        let rn = self.reg_value(u32::from((instr >> 3) & 7), pc, 0);
        let rm = self.reg_value(u32::from((instr >> 6) & 7), pc, 0);
        let addr = rn.wrapping_add(rm);
        let op = (instr >> 9) & 7;
        match op {
            0 => self
                .store_mem(addr, &self.r[rd as usize].to_le_bytes(), pc)
                .map(|()| false),
            1 => self
                .store_mem(addr, &(self.r[rd as usize] as u16).to_le_bytes(), pc)
                .map(|()| false),
            2 => self
                .store_mem(addr, &[self.r[rd as usize] as u8], pc)
                .map(|()| false),
            3 => {
                let v = self.load_mem(addr, 1, pc)? as i8 as i32 as u32;
                self.r[rd as usize] = v;
                Ok(false)
            }
            4 => {
                let v = self.load_mem(addr, 4, pc)?;
                self.r[rd as usize] = v;
                Ok(false)
            }
            5 => {
                let v = self.load_mem(addr, 2, pc)?;
                self.r[rd as usize] = v;
                Ok(false)
            }
            6 => {
                let v = self.load_mem(addr, 1, pc)?;
                self.r[rd as usize] = v;
                Ok(false)
            }
            _ => {
                let v = self.load_mem(addr, 2, pc)? as i16 as i32 as u32;
                self.r[rd as usize] = v;
                Ok(false)
            }
        }
    }

    fn thumb_ls_imm(&mut self, instr: u16, pc: u32, halfword: bool) -> Result<bool, StopReason> {
        let rd = u32::from(instr & 7);
        let rn = self.reg_value(u32::from((instr >> 3) & 7), pc, 0);
        let imm5 = u32::from((instr >> 6) & 0x1F);
        let l = instr & (1 << 11) != 0;
        let (addr, len, byte) = if halfword {
            (rn.wrapping_add(imm5 << 1), 2, false)
        } else if (instr >> 12) & 1 == 0 {
            (rn.wrapping_add(imm5 << 2), 4, false)
        } else {
            (rn.wrapping_add(imm5), 1, true)
        };
        if l {
            let value = self.load_mem(addr, len, pc)?;
            self.r[rd as usize] = value;
            Ok(false)
        } else {
            let value = self.r[rd as usize];
            if byte {
                self.store_mem(addr, &[value as u8], pc)?;
            } else if len == 2 {
                self.store_mem(addr, &(value as u16).to_le_bytes(), pc)?;
            } else {
                self.store_mem(addr, &value.to_le_bytes(), pc)?;
            }
            Ok(false)
        }
    }

    fn thumb_ls_sp(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rd = u32::from((instr >> 8) & 7);
        let imm = u32::from(instr & 0xFF) << 2;
        let addr = self.r[13].wrapping_add(imm);
        if instr & (1 << 11) != 0 {
            self.r[rd as usize] = self.load_mem(addr, 4, pc)?;
        } else {
            self.store_mem(addr, &self.r[rd as usize].to_le_bytes(), pc)?;
        }
        Ok(false)
    }

    fn thumb_adr(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rd = u32::from((instr >> 8) & 7);
        let imm = u32::from(instr & 0xFF) << 2;
        let base = if instr & (1 << 11) == 0 {
            pc.wrapping_add(4) & !3
        } else {
            self.r[13]
        };
        self.r[rd as usize] = base.wrapping_add(imm);
        Ok(false)
    }

    fn thumb_misc(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let group = (instr >> 8) & 0xF;
        match group {
            0b0000 => {
                let imm = u32::from(instr & 0x7F) << 2;
                if instr & (1 << 7) != 0 {
                    self.r[13] = self.r[13].wrapping_sub(imm);
                } else {
                    self.r[13] = self.r[13].wrapping_add(imm);
                }
                Ok(false)
            }
            0b0010 => self.thumb_extend(instr, pc),
            0b0100 | 0b0101 => self.thumb_push(instr, pc, false),
            0b1010 => self.thumb_rev(instr, pc),
            0b1100 | 0b1101 => self.thumb_push(instr, pc, true),
            0b1110 => Err(StopReason::Exception { pc }),
            _ => Err(StopReason::Exception { pc }),
        }
    }

    fn thumb_extend(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rm = self.reg_value(u32::from((instr >> 3) & 7), pc, 0);
        let rd = u32::from(instr & 7);
        let value = match (instr >> 6) & 3 {
            0 => rm as u16 as i16 as i32 as u32,
            1 => rm as u8 as i8 as i32 as u32,
            2 => u32::from(rm as u16),
            _ => u32::from(rm as u8),
        };
        self.r[rd as usize] = value;
        Ok(false)
    }

    fn thumb_rev(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rm = self.reg_value(u32::from((instr >> 3) & 7), pc, 0);
        let rd = u32::from(instr & 7);
        let value = match (instr >> 6) & 3 {
            0 => rm.swap_bytes(),
            1 => ((rm & 0x00FF_00FF) << 8) | ((rm & 0xFF00_FF00) >> 8),
            3 => (rm as u16).swap_bytes() as i16 as i32 as u32,
            _ => return Err(StopReason::Exception { pc }),
        };
        self.r[rd as usize] = value;
        Ok(false)
    }

    fn thumb_push(&mut self, instr: u16, pc: u32, pop: bool) -> Result<bool, StopReason> {
        let mut list = u32::from(instr & 0xFF);
        let extra = instr & (1 << 8) != 0;
        if extra {
            list |= if pop { 1 << 15 } else { 1 << 14 };
        }
        let n = list.count_ones();
        if n == 0 {
            return Err(StopReason::Exception { pc });
        }
        if pop {
            let mut addr = self.r[13];
            let mut branched = false;
            for reg in 0..16 {
                if list & (1 << reg) != 0 {
                    let value = self.load_mem(addr, 4, pc)?;
                    if reg == 15 {
                        self.bx(value);
                        branched = true;
                    } else {
                        self.r[reg] = value;
                    }
                    addr = addr.wrapping_add(4);
                }
            }
            self.r[13] = self.r[13].wrapping_add(n * 4);
            Ok(branched)
        } else {
            let mut addr = self.r[13].wrapping_sub(n * 4);
            self.r[13] = addr;
            for reg in 0..16 {
                if list & (1 << reg) != 0 {
                    let value = self.reg_value(reg, pc, 0);
                    self.store_mem(addr, &value.to_le_bytes(), pc)?;
                    addr = addr.wrapping_add(4);
                }
            }
            Ok(false)
        }
    }

    fn thumb_block(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rn = u32::from((instr >> 8) & 7);
        let list = u32::from(instr & 0xFF);
        let l = instr & (1 << 11) != 0;
        let n = list.count_ones();
        if n == 0 {
            return Err(StopReason::Exception { pc });
        }
        let mut addr = self.r[rn as usize];
        if l {
            for reg in 0..8 {
                if list & (1 << reg) != 0 {
                    self.r[reg] = self.load_mem(addr, 4, pc)?;
                    addr = addr.wrapping_add(4);
                }
            }
        } else {
            for reg in 0..8 {
                if list & (1 << reg) != 0 {
                    self.store_mem(addr, &self.r[reg].to_le_bytes(), pc)?;
                    addr = addr.wrapping_add(4);
                }
            }
        }
        if !l || list & (1 << rn) == 0 {
            self.r[rn as usize] = self.r[rn as usize].wrapping_add(n * 4);
        }
        Ok(false)
    }

    fn thumb_bcond(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let cond = u32::from((instr >> 8) & 0xF);
        if cond == 0xF {
            return self.svc(pc);
        }
        if cond == 0xE {
            return Err(StopReason::Exception { pc });
        }
        if condition_passed(self.cpsr, cond) {
            let imm = sext(u32::from(instr & 0xFF), 8) << 1;
            self.branch_no_exchange(pc.wrapping_add(4).wrapping_add(imm));
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn thumb_bl(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let prefix = (instr >> 11) & 0x1F;
        let imm = u32::from(instr & 0x7FF);
        match prefix {
            0b11110 => {
                // Bit 1 goes away: the pair takes two halfwords and the offset is in words.
                let base = pc.wrapping_add(4) & !2;
                self.r[14] = base.wrapping_add(sext(imm, 11) << 12);
                Ok(false)
            }
            0b11111 => {
                let next = pc.wrapping_add(2);
                self.r[15] = self.r[14].wrapping_add(imm << 1);
                self.r[14] = next | 1;
                self.cpsr |= T;
                Ok(true)
            }
            0b11101 => {
                let next = pc.wrapping_add(2);
                let target = self.r[14].wrapping_add(imm << 1) & !3;
                self.r[14] = next | 1;
                self.cpsr &= !T;
                self.r[15] = target;
                Ok(true)
            }
            _ => Err(StopReason::Exception { pc }),
        }
    }
}

fn halfword(value: u32, high: bool) -> i32 {
    let h = if high { value >> 16 } else { value & 0xFFFF };
    h as u16 as i16 as i32
}

fn sat_i32(v: i64) -> (i64, bool) {
    if v > i64::from(i32::MAX) {
        (i64::from(i32::MAX), true)
    } else if v < i64::from(i32::MIN) {
        (i64::from(i32::MIN), true)
    } else {
        (v, false)
    }
}

fn sext(value: u32, bits: u32) -> u32 {
    let shift = 32 - bits;
    (((value << shift) as i32) >> shift) as u32
}

fn condition_passed(cpsr: u32, cond: u32) -> bool {
    let n = cpsr & N != 0;
    let z = cpsr & Z != 0;
    let c = cpsr & C != 0;
    let v = cpsr & V != 0;
    match cond {
        0x0 => z,
        0x1 => !z,
        0x2 => c,
        0x3 => !c,
        0x4 => n,
        0x5 => !n,
        0x6 => v,
        0x7 => !v,
        0x8 => c && !z,
        0x9 => !c || z,
        0xA => n == v,
        0xB => n != v,
        0xC => !z && n == v,
        0xD => z || n != v,
        0xE => true,
        _ => false,
    }
}

fn add_with_carry(a: u32, b: u32, carry: u32) -> (u32, bool, bool) {
    let (s1, c1) = a.overflowing_add(b);
    let (s2, c2) = s1.overflowing_add(carry);
    let c = c1 || c2;
    let v = (a ^ s2) & (b ^ s2) & 0x8000_0000 != 0;
    (s2, c, v)
}

/// Carry in the ARM sense: `true` means there was **no** borrow.
fn sub_with_carry(a: u32, b: u32, borrow: u32) -> (u32, bool, bool) {
    let (s1, b1) = a.overflowing_sub(b);
    let (s2, b2) = s1.overflowing_sub(borrow);
    let c = !(b1 || b2);
    let v = (a ^ b) & (a ^ s2) & 0x8000_0000 != 0;
    (s2, c, v)
}

/// `(result, carry out)`.
///
/// An immediate amount of zero is not "no shift" for LSR, ASR and ROR: the encoding stores 32 in
/// that zero, and ROR by zero is RRX. An amount coming from a register has no such shortcut — a
/// real zero does not shift.
fn shift(value: u32, kind: u32, amount: u32, carry: bool, immediate: bool) -> (u32, bool) {
    if !immediate && amount == 0 {
        return (value, carry);
    }
    match kind {
        0 => {
            if amount == 0 {
                (value, carry)
            } else if amount < 32 {
                (value << amount, value & (1 << (32 - amount)) != 0)
            } else if amount == 32 {
                (0, value & 1 != 0)
            } else {
                (0, false)
            }
        }
        1 => {
            let q = if immediate && amount == 0 { 32 } else { amount };
            if q < 32 {
                (value >> q, value & (1 << (q - 1)) != 0)
            } else if q == 32 {
                (0, value & 0x8000_0000 != 0)
            } else {
                (0, false)
            }
        }
        2 => {
            let q = if immediate && amount == 0 { 32 } else { amount };
            if q < 32 {
                let output = ((value as i32) >> q) as u32;
                (output, value & (1 << (q - 1)) != 0)
            } else {
                let sign = value & 0x8000_0000 != 0;
                (if sign { 0xFFFF_FFFF } else { 0 }, sign)
            }
        }
        _ => {
            if immediate && amount == 0 {
                let bit = u32::from(carry);
                ((value >> 1) | (bit << 31), value & 1 != 0)
            } else if amount == 0 {
                (value, carry)
            } else {
                let q = amount & 31;
                if q == 0 {
                    (value, value & 0x8000_0000 != 0)
                } else {
                    let output = value.rotate_right(q);
                    (output, output & 0x8000_0000 != 0)
                }
            }
        }
    }
}

/// `(value, writes Rd, C comes from the shifter, arithmetic carry, overflow)`.
fn alu(opcode: u32, a: u32, b: u32, carry: bool) -> (u32, bool, bool, bool, bool) {
    let cin = u32::from(carry);
    match opcode {
        0x0 => (a & b, true, true, false, false),
        0x1 => (a ^ b, true, true, false, false),
        0x2 => {
            let (v, c, ov) = sub_with_carry(a, b, 0);
            (v, true, false, c, ov)
        }
        0x3 => {
            let (v, c, ov) = sub_with_carry(b, a, 0);
            (v, true, false, c, ov)
        }
        0x4 => {
            let (v, c, ov) = add_with_carry(a, b, 0);
            (v, true, false, c, ov)
        }
        0x5 => {
            let (v, c, ov) = add_with_carry(a, b, cin);
            (v, true, false, c, ov)
        }
        0x6 => {
            let (v, c, ov) = sub_with_carry(a, b, u32::from(!carry));
            (v, true, false, c, ov)
        }
        0x7 => {
            let (v, c, ov) = sub_with_carry(b, a, u32::from(!carry));
            (v, true, false, c, ov)
        }
        0x8 => (a & b, false, true, false, false),
        0x9 => (a ^ b, false, true, false, false),
        0xA => {
            let (v, c, ov) = sub_with_carry(a, b, 0);
            (v, false, false, c, ov)
        }
        0xB => {
            let (v, c, ov) = add_with_carry(a, b, 0);
            (v, false, false, c, ov)
        }
        0xC => (a | b, true, true, false, false),
        0xD => (b, true, true, false, false),
        0xE => (a & !b, true, true, false, false),
        _ => (!b, true, true, false, false),
    }
}

impl CpuBackend for Interpreter {
    fn reset(&mut self, mem: &GuestMemory) -> Result<(), CpuError> {
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
        self.mem = copy;
        // The pointers pointed at the memory that was just swapped out.
        self.code = Shortcut::empty();
        self.data = [Shortcut::empty(); 4];
        self.r = [0; 16];
        self.cpsr = USER_MODE;
        self.instructions = 0;
        self.watches.clear();
        self.semihosting.clear();
        self.exclusive = None;
        Ok(())
    }

    fn read_reg(&self, reg: Reg) -> u32 {
        self.r[index(reg)]
    }

    fn write_reg(&mut self, reg: Reg, value: u32) {
        self.r[index(reg)] = value;
    }

    fn run(&mut self, pc: u32, max_instructions: u64) -> Result<StopReason, CpuError> {
        if pc & 1 == 1 {
            self.cpsr |= T;
        } else {
            self.cpsr &= !T;
        }
        self.r[15] = pc & !1;
        let limit = self.instructions.saturating_add(max_instructions);
        loop {
            if self.instructions >= limit {
                return Ok(StopReason::Budget);
            }
            match self.step() {
                Ok(branched) => {
                    if !branched {
                        let step = if self.thumb() { 2 } else { 4 };
                        self.r[15] = self.r[15].wrapping_add(step);
                    }
                    self.instructions += 1;
                }
                Err(reason) => return Ok(reason),
            }
        }
    }

    fn instructions(&self) -> u64 {
        self.instructions
    }

    fn set_instructions(&mut self, value: u64) {
        self.instructions = value;
    }

    fn cpsr(&self) -> u32 {
        self.cpsr
    }

    fn set_cpsr(&mut self, value: u32) {
        self.cpsr = value;
    }

    fn in_thumb(&self) -> bool {
        self.thumb()
    }

    fn watch_dirty(&mut self, id: u32, base: u32, len: u32) -> Result<(), CpuError> {
        self.unwatch_dirty(id);
        self.watches.push((id, base, base.saturating_add(len), true));
        Ok(())
    }

    fn unwatch_dirty(&mut self, id: u32) {
        self.watches.retain(|watch| watch.0 != id);
    }

    fn take_dirty(&mut self, id: u32) -> bool {
        match self.watches.iter_mut().find(|watch| watch.0 == id) {
            Some(watch) => std::mem::replace(&mut watch.3, false),
            None => true,
        }
    }

    fn mark_dirty(&mut self, addr: u32, len: u32) {
        self.mark_watches(addr, len);
    }

    fn read_mem(&self, addr: u32, buf: &mut [u8]) -> Result<(), CpuError> {
        self.mem
            .read(addr, buf.len() as u32)
            .map(|bytes| buf.copy_from_slice(bytes))
            .map_err(|e| CpuError(e.to_string()))
    }

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> Result<(), CpuError> {
        self.mem
            .write(addr, data)
            .map_err(|e| CpuError(e.to_string()))
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu_with(code: &[u8]) -> Interpreter {
        let mut mem = GuestMemory::new();
        let mut bytes = code.to_vec();
        bytes.resize(0x1000, 0);
        mem.map("code", 0, bytes, true).unwrap();
        mem.map_zeroed("data", 0x1000, 0x1000).unwrap();
        mem.map_zeroed("stack", 0x2000_0000, 0x1000).unwrap();
        let mut cpu = Interpreter::new().unwrap();
        cpu.reset(&mem).unwrap();
        cpu.write_reg(Reg::Sp, 0x2000_0f00);
        cpu
    }

    fn arm(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|p| p.to_le_bytes()).collect()
    }

    #[test]
    fn executes_an_instruction_and_reads_a_register() {
        let mut cpu = cpu_with(&arm(&[0xe3a0_0037, 0xeaff_fffe]));
        assert_eq!(cpu.run(0, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    #[test]
    fn a_jump_into_the_api_range_becomes_a_call() {
        let mut cpu = cpu_with(&arm(&[0xe3a0_020f, 0xe12f_ff10]));
        assert_eq!(
            cpu.run(0, 10).unwrap(),
            StopReason::ApiCall { addr: API_BASE }
        );
    }

    #[test]
    fn an_address_with_bit_zero_enters_thumb() {
        let mut code = vec![0u8; 0x100];
        code.extend_from_slice(&[0x37, 0x20, 0xfe, 0xe7]);
        let mut cpu = cpu_with(&code);
        assert_eq!(cpu.run(0x101, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    #[test]
    fn an_even_address_returns_to_arm_after_thumb() {
        let mut code = arm(&[0xe3a0_0037, 0xeaff_fffe]);
        code.resize(0x100, 0);
        code.extend_from_slice(&[0x00, 0x20, 0xfe, 0xe7]);
        let mut cpu = cpu_with(&code);
        cpu.run(0x101, 1).unwrap();
        assert_eq!(cpu.run(0, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    #[test]
    fn the_watch_trips_on_guest_writes_not_host_writes() {
        let mut cpu = cpu_with(&arm(&[0xe3a0_1a01, 0xe581_0000, 0xeaff_fffe]));
        cpu.watch_dirty(7, 0x1000, 4).unwrap();
        assert!(cpu.take_dirty(7));
        assert!(!cpu.take_dirty(7));
        cpu.write_mem(0x1000, &[1, 2, 3, 4]).unwrap();
        assert!(!cpu.take_dirty(7));
        cpu.run(0, 2).unwrap();
        assert!(cpu.take_dirty(7));
        cpu.unwatch_dirty(7);
        assert!(cpu.take_dirty(7));
    }

    #[test]
    fn a_return_to_the_sentinel_is_recognized() {
        let mut cpu = cpu_with(&arm(&[0xe12f_ff1e]));
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
    }

    #[test]
    fn semihosting_writes_without_interrupting_the_guest() {
        let mut cpu = cpu_with(&arm(&[0xe3a0_0003, 0xe3a0_1a01, 0xef00_00ab, 0xe12f_ff1e]));
        cpu.write_mem(0x1000, b"Z").unwrap();
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.semihosting(), "Z");
        assert_eq!(cpu.read_reg(Reg::R0), 0);
    }

    #[test]
    fn code_changed_by_the_host_is_executed() {
        let mut cpu = cpu_with(&arm(&[0xe3a0_0001, 0xe12f_ff1e]));
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 1);
        cpu.write_mem(0, &0xe3a0_0002u32.to_le_bytes()).unwrap();
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 2);
    }

    #[test]
    fn adds_updates_flags_and_a_shift_by_32_sets_carry() {
        // adds r0, r1, #1  with r1 = 0xFFFFFFFF → 0, C and Z
        let mut cpu = cpu_with(&arm(&[0xe291_0001, 0xe1b0_1020, 0xe12f_ff1e]));
        cpu.write_reg(Reg::R1, 0xFFFF_FFFF);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0);
        assert_ne!(cpu.cpsr() & Z, 0);
        assert_ne!(cpu.cpsr() & C, 0);
        // movs r1, r0, lsr #32 — r0 is still 0 here; re-run from the second instruction with
        // r0 = 0x80000000.
        cpu.write_reg(Reg::R0, 0x8000_0000);
        cpu.run(4, 1).unwrap();
        assert_eq!(cpu.read_reg(Reg::R1), 0);
        assert_ne!(cpu.cpsr() & C, 0);
        assert_ne!(cpu.cpsr() & Z, 0);
    }

    #[test]
    fn ldr_str_and_ldm_round_trip_the_bytes() {
        // mov r0, #0x11; mov r1, #0x1000; str r0, [r1]; ldr r2, [r1]; bx lr
        let mut cpu = cpu_with(&arm(&[
            0xe3a0_0011,
            0xe3a0_1a01,
            0xe581_0000,
            0xe591_2000,
            0xe12f_ff1e,
        ]));
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R2), 0x11);
    }

    #[test]
    fn clz_rev_and_sxtb() {
        let mut cpu = cpu_with(&arm(&[0xe16f_0f11, 0xe6bf_0f32, 0xe6af_1071, 0xe12f_ff1e]));
        cpu.write_reg(Reg::R1, 0x0001_0000);
        cpu.write_reg(Reg::R2, 0x1234_5678);
        cpu.write_reg(Reg::R1, 0x0001_0000);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        // The third instruction reads r1 again, so SXTB uses the original value only if we have
        // not overwritten r1. CLZ writes r0. REV writes r0 from r2. SXTB reads r1.
        cpu.write_reg(Reg::R1, 0x80);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 0x7856_3412);
        assert_eq!(cpu.read_reg(Reg::R1), 0xFFFF_FF80);
    }

    #[test]
    fn clz_of_zero_is_thirty_two() {
        let mut cpu = cpu_with(&arm(&[0xe16f_0f11, 0xe12f_ff1e]));
        cpu.write_reg(Reg::R1, 0);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 32);
    }

    #[test]
    fn thumb_bl_saves_the_return_with_the_thumb_bit() {
        // BL to 8, then not the caller's `movs r0, #1` and `bx lr`: the target does `movs r1, #7`.
        // at 0: f000 f801  (BL, imm hi 0, imm lo 1 → PC = 4 + 2 = 6? )
        // Recomputed in the test itself: the target is 8.
        // first: LR = (0+4) + 0 = 4
        // second at 2: PC = 4 + (imm<<1). To land on 8, imm = 2. 11111 00000000010 = 0xF802
        let mut code = vec![0x00, 0xF0, 0x02, 0xF8];
        code.resize(8, 0);
        code.extend_from_slice(&[0x07, 0x21]); // movs r1, #7
        code.extend_from_slice(&[0x70, 0x47]); // bx lr — back to the address right after the pair
        let mut cpu = cpu_with(&code);
        // Four instructions: the two halves of the BL, the movs and the bx. The next would be the
        // zero word left at address 4, and the budget runs out before it.
        assert_eq!(cpu.run(1, 4).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R1), 7);
        assert_eq!(cpu.read_reg(Reg::Lr) & 1, 1);
        assert_eq!(cpu.read_reg(Reg::Pc), 4);
    }

    #[test]
    fn ssat_clamps_and_sets_q() {
        // `SSAT r0, #8, r1`. Bit 4 has to be set: without it the word falls into the load/store
        // space and the test ends up measuring another instruction.
        let mut cpu = cpu_with(&arm(&[0xe6a7_0011, 0xe12f_ff1e]));
        cpu.write_reg(Reg::R1, 200);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 127);
        assert_ne!(cpu.cpsr() & Q, 0);
    }

    #[test]
    fn a_failed_conditional_does_not_write() {
        // cmp r0, #1; moveq r1, #2; bx lr
        let mut cpu = cpu_with(&arm(&[0xe350_0001, 0x03a0_1002, 0xe12f_ff1e]));
        cpu.write_reg(Reg::R0, 0);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R1), 0);
    }

    #[test]
    fn arm_loop_adds_ten_and_the_untaken_branch_falls_into_the_return() {
        // mov r0, #0; add r0, #1; cmp r0, #10; bne add; bx lr
        let mut cpu = cpu_with(&arm(&[
            0xe3a0_0000,
            0xe280_0001,
            0xe350_000a,
            0x1aff_fffc,
            0xe12f_ff1e,
        ]));
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 40).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 10);
    }

    #[test]
    fn arm_loop_increments_the_word_in_memory() {
        // r1 = 0x1000; repete r0 vezes: ldr r2, [r1]; add r2, #1; str r2, [r1]; subs r0, #1; bne
        let mut cpu = cpu_with(&arm(&[
            0xe3a0_1a01,
            0xe591_2000,
            0xe282_2001,
            0xe581_2000,
            0xe250_0001,
            0x1aff_fffa,
            0xe12f_ff1e,
        ]));
        cpu.write_reg(Reg::R0, 4);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        cpu.write_mem(0x1000, &0u32.to_le_bytes()).unwrap();
        assert_eq!(cpu.run(0, 40).unwrap(), StopReason::Returned);
        let mut buf = [0u8; 4];
        cpu.read_mem(0x1000, &mut buf).unwrap();
        assert_eq!(u32::from_le_bytes(buf), 4);
        assert_eq!(cpu.read_reg(Reg::R0), 0);
    }

    #[test]
    fn thumb_loop_adds_ten() {
        // movs r0, #0; adds r0, #1; cmp r0, #10; bne adds; bx lr
        let code = [
            0x00, 0x20, 0x01, 0x30, 0x0a, 0x28, 0xfc, 0xd1, 0x70, 0x47,
        ];
        let mut cpu = cpu_with(&code);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC | 1);
        assert_eq!(cpu.run(1, 40).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 10);
    }
}
