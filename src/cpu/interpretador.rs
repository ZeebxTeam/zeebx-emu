//! Interpretador ARM1176 (A32 e Thumb), o núcleo do core WebAssembly.
//!
//! O `dynarmic` e o `unicorn` recompilam o bloco do guest para o código nativo do host e saltam
//! para ele. Num módulo WebAssembly esse salto não existe: o navegador só executa Wasm. Este
//! arquivo lê a instrução na memória do guest e a executa aqui, no mesmo contrato de
//! [`CpuBackend`] — parada na faixa de vtables, bit 0 do endereço como modo Thumb, vigia de
//! escrita que ignora o que o host gravou.
//!
//! Não foi medido contra o JIT. O desktop continua no `dynarmic`; isto entra no `wasm32`, onde
//! o bloco emitido não roda, e no iOS, onde o sistema não deixa o processo mapear código.

use super::mem::GuestMemory;
use super::{API_BASE, API_SIZE, RETURN_MAGIC, apara_semihosting};
use super::{CpuBackend, CpuError, Reg, StopReason};

const N: u32 = 1 << 31;
const Z: u32 = 1 << 30;
const C: u32 = 1 << 29;
const V: u32 = 1 << 28;
const Q: u32 = 1 << 27;
const T: u32 = 1 << 5;
const MODO_USUARIO: u32 = 0x10;

/// Operações de semihosting que o gancho do `unicorn` registra. O imediato do `SVC` não muda o
/// rumo: qualquer `SVC` olha a operação em `r0`. Os jogos da PopCap inicializam por aqui.
const SYS_WRITEC: u32 = 0x03;
const SYS_WRITE0: u32 = 0x04;
const MAX_SEMIHOSTING_STRING: u32 = 4096;

pub struct Interpretador {
    mem: GuestMemory,
    r: [u32; 16],
    cpsr: u32,
    instrucoes: u64,
    vigias: Vec<(u32, u32, u32, bool)>,
    semihosting: String,
    /// Endereço do `LDREX` ainda sem `STREX`. Uma escrita qualquer o derruba: é o monitor
    /// exclusivo de um núcleo só, que é o que este processo é.
    exclusivo: Option<u32>,
}

impl Interpretador {
    pub fn new() -> Result<Self, CpuError> {
        Ok(Self {
            mem: GuestMemory::new(),
            r: [0; 16],
            cpsr: MODO_USUARIO,
            instrucoes: 0,
            vigias: Vec::new(),
            semihosting: String::new(),
            exclusivo: None,
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

    /// O valor que uma instrução lê quando pede o `PC`.
    ///
    /// No ARM o pipeline entrega o endereço da instrução mais 8; no Thumb, mais 4. `extra` cobre
    /// o caso do deslocamento com quantidade num registrador, em que o ARM entrega mais 12 — a
    /// instrução gasta um ciclo a mais e o `PC` visto anda junto.
    fn le_pc(&self, pc: u32, extra: u32) -> u32 {
        let base = if self.thumb() { 4 } else { 8 };
        pc.wrapping_add(base + extra)
    }

    fn le_reg(&self, n: u32, pc: u32, extra: u32) -> u32 {
        if n == 15 {
            self.le_pc(pc, extra)
        } else {
            self.r[n as usize]
        }
    }

    fn poe_nz(&mut self, valor: u32) {
        self.cpsr = (self.cpsr & !(N | Z))
            | if valor & 0x8000_0000 != 0 { N } else { 0 }
            | if valor == 0 { Z } else { 0 };
    }

    fn poe_c(&mut self, carry: bool) {
        if carry {
            self.cpsr |= C;
        } else {
            self.cpsr &= !C;
        }
    }

    fn poe_v(&mut self, overflow: bool) {
        if overflow {
            self.cpsr |= V;
        } else {
            self.cpsr &= !V;
        }
    }

    fn poe_q(&mut self) {
        self.cpsr |= Q;
    }

    /// Desvio que **não** troca de conjunto de instruções. `MOV PC` e o `B` ficam no modo em
    /// que estavam; quem troca é o `BX`.
    fn desvia_sem_troca(&mut self, alvo: u32) {
        self.r[15] = if self.thumb() { alvo & !1 } else { alvo & !3 };
    }

    fn bx(&mut self, alvo: u32) {
        if alvo & 1 != 0 {
            self.cpsr |= T;
            self.r[15] = alvo & !1;
        } else {
            self.cpsr &= !T;
            self.r[15] = alvo & !3;
        }
    }

    fn busca(&self, addr: u32, len: u32) -> Result<u32, StopReason> {
        if addr == RETURN_MAGIC {
            return Err(StopReason::Returned);
        }
        if (API_BASE..API_BASE.saturating_add(API_SIZE)).contains(&addr) {
            return Err(StopReason::ApiCall { addr });
        }
        let bytes = self
            .mem
            .read(addr, len)
            .map_err(|_| StopReason::MemoryFault { addr, pc: addr })?;
        // `GuestMemory::executavel` exige quatro bytes. Uma Thumb no fim da região cabe em dois
        // e seria recusada; a pergunta certa é se a região que a leitura achou deixa executar.
        let executavel = self.mem.regions().iter().any(|regiao| {
            regiao.executavel
                && addr >= regiao.base
                && u64::from(addr) + u64::from(len)
                    <= u64::from(regiao.base) + regiao.bytes.len() as u64
        });
        if !executavel {
            return Err(StopReason::MemoryFault { addr, pc: addr });
        }
        Ok(match len {
            2 => u32::from(u16::from_le_bytes([bytes[0], bytes[1]])),
            _ => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        })
    }

    fn le_mem(&self, addr: u32, len: u32, pc: u32) -> Result<u32, StopReason> {
        let bytes = self
            .mem
            .read(addr, len)
            .map_err(|_| StopReason::MemoryFault { addr, pc })?;
        Ok(match len {
            1 => u32::from(bytes[0]),
            2 => u32::from(u16::from_le_bytes([bytes[0], bytes[1]])),
            _ => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        })
    }

    fn es_mem(&mut self, addr: u32, dados: &[u8], pc: u32) -> Result<(), StopReason> {
        self.mem
            .write(addr, dados)
            .map_err(|_| StopReason::MemoryFault { addr, pc })?;
        self.exclusivo = None;
        self.suja(addr, dados.len() as u32);
        Ok(())
    }

    fn suja(&mut self, addr: u32, len: u32) {
        let fim = addr.saturating_add(len);
        for vigia in &mut self.vigias {
            if addr < vigia.2 && fim > vigia.1 {
                vigia.3 = true;
            }
        }
    }

    /// `true` quando a instrução já escreveu o `PC`.
    fn passo(&mut self) -> Result<bool, StopReason> {
        let pc = self.r[15];
        if self.thumb() {
            if pc & 1 != 0 {
                return Err(StopReason::Exception { pc });
            }
            let op = self.busca(pc, 2)?;
            self.thumb_exec(op as u16, pc)
        } else {
            if pc & 3 != 0 {
                return Err(StopReason::Exception { pc });
            }
            let op = self.busca(pc, 4)?;
            self.arm_exec(op, pc)
        }
    }

    fn arm_exec(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let cond = instr >> 28;
        if cond == 0xF {
            return self.arm_incondicional(instr, pc);
        }
        if !condicao(self.cpsr, cond) {
            return Ok(false);
        }
        match (instr >> 25) & 7 {
            0b000 => self.arm_000(instr, pc),
            0b001 => self.arm_dados(instr, pc, true),
            0b010 | 0b011 => self.arm_ls(instr, pc),
            0b100 => self.arm_bloco(instr, pc),
            0b101 => self.arm_b(instr, pc),
            0b111 if instr & (1 << 24) != 0 => self.svc(pc),
            _ => Err(StopReason::Exception { pc }),
        }
    }

    /// `BLX` imediato e as dicas (`PLD`, `SETEND`). O resto deste espaço não é instrução de
    /// usuário do ARM1176 que a gente execute.
    fn arm_incondicional(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        if (instr >> 25) & 7 == 0b101 {
            let h = (instr >> 24) & 1;
            let imm = sext(instr & 0x00FF_FFFF, 24) << 2;
            let alvo = pc.wrapping_add(8).wrapping_add(imm).wrapping_add(h * 2);
            self.r[14] = pc.wrapping_add(4);
            self.cpsr |= T;
            self.r[15] = alvo & !1;
            return Ok(true);
        }
        // PLD e as outras pré-cargas são dicas. SETEND para big-endian não: a memória daqui é
        // little-endian, e executar a instrução "certo" sem trocar a ordem seria mentira.
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
        // TST/TEQ/CMP/CMN com S=0 não são ALU: este é o espaço de BX, CLZ, MSR, QADD.
        if (instr >> 23) & 0x1F == 0b00010 && instr & (1 << 20) == 0 {
            return self.arm_misc(instr, pc);
        }
        self.arm_dados(instr, pc, false)
    }

    fn arm_b(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let imm = sext(instr & 0x00FF_FFFF, 24) << 2;
        let alvo = pc.wrapping_add(8).wrapping_add(imm);
        if instr & (1 << 24) != 0 {
            self.r[14] = pc.wrapping_add(4);
        }
        self.desvia_sem_troca(alvo);
        Ok(true)
    }

    fn svc(&mut self, pc: u32) -> Result<bool, StopReason> {
        let op = self.r[0];
        let arg = self.r[1];
        match op {
            SYS_WRITEC => {
                if let Ok(b) = self.le_mem(arg, 1, pc) {
                    self.semihosting.push(b as u8 as char);
                }
            }
            SYS_WRITE0 => {
                for offset in 0..MAX_SEMIHOSTING_STRING {
                    let Ok(b) = self.le_mem(arg.wrapping_add(offset), 1, pc) else {
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
        apara_semihosting(&mut self.semihosting);
        self.r[0] = 0;
        Ok(false)
    }

    fn arm_misc(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        if instr & 0x0FFF_FFF0 == 0x012F_FF10 {
            self.bx(self.le_reg(instr & 0xF, pc, 0));
            return Ok(true);
        }
        if instr & 0x0FFF_FFF0 == 0x012F_FF30 {
            self.r[14] = pc.wrapping_add(4);
            self.bx(self.le_reg(instr & 0xF, pc, 0));
            return Ok(true);
        }
        if instr & 0x0FFF_0FF0 == 0x016F_0F10 {
            let rm = self.le_reg(instr & 0xF, pc, 0);
            let rd = (instr >> 12) & 0xF;
            return self.poe_rd(rd, rm.leading_zeros(), pc);
        }
        if instr & 0x0FFF_0FFF == 0x010F_0000 {
            let rd = (instr >> 12) & 0xF;
            return self.poe_rd(rd, self.cpsr, pc);
        }
        if instr & 0x0FB0_FFF0 == 0x0120_F000 {
            let valor = self.le_reg(instr & 0xF, pc, 0);
            self.msr((instr >> 16) & 0xF, valor);
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
        let rn = self.le_reg((instr >> 16) & 0xF, pc, 0) as i32 as i64;
        let rm = self.le_reg(instr & 0xF, pc, 0) as i32 as i64;
        let op = (instr >> 21) & 3;
        let (dobro, sat1) = if op & 2 != 0 {
            sat_i32(rm.saturating_mul(2))
        } else {
            (rm, false)
        };
        // O dobro saturado já liga Q, mesmo que a soma seguinte caiba.
        if sat1 {
            self.poe_q();
        }
        let soma = match op {
            0 | 2 => rn + dobro,
            _ => rn - dobro,
        };
        let (valor, sat2) = sat_i32(soma);
        if sat2 {
            self.poe_q();
        }
        self.poe_rd((instr >> 12) & 0xF, valor as u32, pc)
    }

    fn arm_dsp_mul(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let rd = (instr >> 12) & 0xF;
        let rn = self.le_reg((instr >> 16) & 0xF, pc, 0);
        let rs = self.le_reg((instr >> 8) & 0xF, pc, 0);
        let rm = self.le_reg(instr & 0xF, pc, 0);
        let x = (instr >> 5) & 1 != 0;
        let y = (instr >> 6) & 1 != 0;
        // Bits 22–21 separam as formas; o bit 5 (x) separa SMULW de SMLAW dentro da mesma.
        let produto: i64 = match (instr >> 21) & 3 {
            // SMLAxy: acumula o produto das meias em Rn.
            0 => {
                let (soma, sat) =
                    sat_i32(meia(rm, x) as i64 * meia(rs, y) as i64 + rn as i32 as i64);
                if sat {
                    self.poe_q();
                }
                soma
            }
            // SMLAWy (x = 0) acumula; SMULWy (x = 1) não. O campo Rn do SMULW é zero na
            // codificação, mas isso nomeia o r0 — não pode entrar na soma.
            1 => {
                let p = ((rm as i32 as i64) * meia(rs, y) as i64) >> 16;
                if x {
                    p
                } else {
                    let (soma, sat) = sat_i32(p + rn as i32 as i64);
                    if sat {
                        self.poe_q();
                    }
                    soma
                }
            }
            // SMULxy: só o produto, sem acumulador e sem flag.
            3 => meia(rm, x) as i64 * meia(rs, y) as i64,
            _ => return Err(StopReason::Exception { pc }),
        };
        self.poe_rd(rd, produto as u32, pc)
    }

    fn msr(&mut self, campo: u32, valor: u32) {
        // Em modo usuário só o campo `f` (flags) pode ser escrito. Os outros bits do `CPSR`
        // pedidos aqui são do modo privilegiado; ignorá-los é o que o núcleo faz com o jogo.
        if campo & 0x8 != 0 {
            let flags = N | Z | C | V | Q;
            self.cpsr = (self.cpsr & !flags) | (valor & flags);
        }
    }

    fn arm_dados(&mut self, instr: u32, pc: u32, imediato: bool) -> Result<bool, StopReason> {
        let opcode = (instr >> 21) & 0xF;
        let s = instr & (1 << 20) != 0;
        let rn_i = (instr >> 16) & 0xF;
        let rd = (instr >> 12) & 0xF;
        if (8..12).contains(&opcode) && !s {
            if imediato && opcode == 0b1001 && rd == 0xF {
                // MSR imediato: a codificação divide o imediato com o shifter do processamento.
                let imm = instr & 0xFF;
                let rot = ((instr >> 8) & 0xF) * 2;
                self.msr((instr >> 16) & 0xF, imm.rotate_right(rot));
                return Ok(false);
            }
            return Err(StopReason::Exception { pc });
        }
        let extra = u32::from(!imediato && instr & (1 << 4) != 0) * 4;
        let (operando, carry_sh) = if imediato {
            let imm = instr & 0xFF;
            let rot = ((instr >> 8) & 0xF) * 2;
            let valor = imm.rotate_right(rot);
            let carry = if rot == 0 {
                self.flag_c()
            } else {
                valor & 0x8000_0000 != 0
            };
            (valor, carry)
        } else {
            self.operando_reg(instr, pc)?
        };
        let a = self.le_reg(rn_i, pc, extra);
        let c_in = self.flag_c();
        let (valor, escreve, logico, c, v) = alu(opcode, a, operando, c_in);
        if s && rd == 15 {
            return Err(StopReason::Exception { pc });
        }
        if s {
            self.poe_nz(valor);
            if logico {
                self.poe_c(carry_sh);
            } else {
                self.poe_c(c);
                self.poe_v(v);
            }
        }
        if escreve {
            self.poe_rd(rd, valor, pc)
        } else {
            Ok(false)
        }
    }

    fn operando_reg(&mut self, instr: u32, pc: u32) -> Result<(u32, bool), StopReason> {
        let rm = self.le_reg(instr & 0xF, pc, u32::from(instr & (1 << 4) != 0) * 4);
        let tipo = (instr >> 5) & 3;
        let (quant, imediato) = if instr & (1 << 4) != 0 {
            let rs = self.le_reg((instr >> 8) & 0xF, pc, 4);
            (rs & 0xFF, false)
        } else {
            ((instr >> 7) & 0x1F, true)
        };
        Ok(desloca(rm, tipo, quant, self.flag_c(), imediato))
    }

    fn poe_rd(&mut self, rd: u32, valor: u32, _pc: u32) -> Result<bool, StopReason> {
        if rd == 15 {
            self.desvia_sem_troca(valor);
            Ok(true)
        } else {
            self.r[rd as usize] = valor;
            Ok(false)
        }
    }

    fn poe_carga_pc(&mut self, rd: u32, valor: u32) -> bool {
        if rd == 15 {
            // LDR/LDM para o PC trocam de modo pelo bit 0. O processamento de dados não troca.
            self.bx(valor);
            true
        } else {
            self.r[rd as usize] = valor;
            false
        }
    }

    fn arm_ls(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let forma_reg = (instr >> 25) & 1 == 1;
        if forma_reg && instr & (1 << 4) != 0 {
            return self.arm_media(instr, pc);
        }
        let p = instr & (1 << 24) != 0;
        let u = instr & (1 << 23) != 0;
        let b = instr & (1 << 22) != 0;
        let w = instr & (1 << 21) != 0;
        let l = instr & (1 << 20) != 0;
        let rn = (instr >> 16) & 0xF;
        let rd = (instr >> 12) & 0xF;
        let offset = if forma_reg {
            let (valor, _) = self.operando_reg(instr, pc)?;
            valor
        } else {
            instr & 0xFFF
        };
        let base = self.le_reg(rn, pc, 0);
        let modificado = if u {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let endereco = if p { modificado } else { base };
        let saltou = if l {
            let valor = if b {
                self.le_mem(endereco, 1, pc)?
            } else {
                self.le_mem(endereco, 4, pc)?
            };
            self.poe_carga_pc(rd, valor)
        } else {
            let valor = self.le_reg(rd, pc, 0);
            if b {
                self.es_mem(endereco, &[valor as u8], pc)?;
            } else {
                self.es_mem(endereco, &valor.to_le_bytes(), pc)?;
            }
            false
        };
        if (w || !p) && rn != 15 {
            self.r[rn as usize] = modificado;
        }
        Ok(saltou)
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
            self.le_reg(instr & 0xF, pc, 0)
        };
        let base = self.le_reg(rn, pc, 0);
        let modificado = if u {
            base.wrapping_add(offset)
        } else {
            base.wrapping_sub(offset)
        };
        let endereco = if p { modificado } else { base };
        // sh: 1 meia sem sinal, 2 byte com sinal (ou dupla, se for store), 3 meia com sinal.
        let saltou = match (l, sh) {
            (false, 1) => {
                let valor = self.le_reg(rd, pc, 0);
                self.es_mem(endereco, &(valor as u16).to_le_bytes(), pc)?;
                false
            }
            (true, 1) => {
                let valor = self.le_mem(endereco, 2, pc)?;
                self.poe_carga_pc(rd, valor)
            }
            (true, 2) => {
                let valor = self.le_mem(endereco, 1, pc)? as i8 as i32 as u32;
                self.poe_carga_pc(rd, valor)
            }
            (true, 3) => {
                let valor = self.le_mem(endereco, 2, pc)? as i16 as i32 as u32;
                self.poe_carga_pc(rd, valor)
            }
            (false, 2) => {
                // LDRD codificado com L=0 e S=1 H=0 é STRD? Não: L=0 S=1 H=0 é LDRD.
                // A tabela: L=0 S=1 H=0 carrega o par. Quem grava o par é L=0 S=1 H=1.
                let lo = self.le_mem(endereco, 4, pc)?;
                let hi = self.le_mem(endereco.wrapping_add(4), 4, pc)?;
                let s1 = self.poe_carga_pc(rd, lo);
                let s2 = self.poe_carga_pc(rd.wrapping_add(1) & 0xF, hi);
                s1 || s2
            }
            (false, 3) => {
                let lo = self.le_reg(rd, pc, 0);
                let hi = self.le_reg(rd.wrapping_add(1) & 0xF, pc, 0);
                self.es_mem(endereco, &lo.to_le_bytes(), pc)?;
                self.es_mem(endereco.wrapping_add(4), &hi.to_le_bytes(), pc)?;
                false
            }
            _ => return Err(StopReason::Exception { pc }),
        };
        if (w || !p) && rn != 15 {
            self.r[rn as usize] = modificado;
        }
        Ok(saltou)
    }

    fn arm_bloco(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let p = instr & (1 << 24) != 0;
        let u = instr & (1 << 23) != 0;
        let s = instr & (1 << 22) != 0;
        let w = instr & (1 << 21) != 0;
        let l = instr & (1 << 20) != 0;
        let rn = (instr >> 16) & 0xF;
        let lista = instr & 0xFFFF;
        let n = lista.count_ones();
        if n == 0 {
            return Err(StopReason::Exception { pc });
        }
        if s && l && lista & (1 << 15) != 0 {
            return Err(StopReason::Exception { pc });
        }
        let bytes = n * 4;
        let base = self.le_reg(rn, pc, 0);
        let (mut endereco, novo) = match (u, p) {
            (true, false) => (base, base.wrapping_add(bytes)),
            (true, true) => (base.wrapping_add(4), base.wrapping_add(bytes)),
            (false, false) => (
                base.wrapping_sub(bytes).wrapping_add(4),
                base.wrapping_sub(bytes),
            ),
            (false, true) => (base.wrapping_sub(bytes), base.wrapping_sub(bytes)),
        };
        let mut saltou = false;
        if l {
            let mut valores = Vec::with_capacity(n as usize);
            let mut cursor = endereco;
            for _ in 0..n {
                valores.push(self.le_mem(cursor, 4, pc)?);
                cursor = cursor.wrapping_add(4);
            }
            let mut i = 0;
            for reg in 0..16 {
                if lista & (1 << reg) != 0 {
                    if self.poe_carga_pc(reg, valores[i]) {
                        saltou = true;
                    }
                    i += 1;
                }
            }
        } else {
            for reg in 0..16 {
                if lista & (1 << reg) != 0 {
                    let valor = self.le_reg(reg, pc, 0);
                    self.es_mem(endereco, &valor.to_le_bytes(), pc)?;
                    endereco = endereco.wrapping_add(4);
                }
            }
        }
        if w && rn != 15 && (l && lista & (1 << rn) == 0 || !l) {
            self.r[rn as usize] = novo;
        }
        Ok(saltou)
    }

    fn arm_mul(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        // SWP e LDREX/STREX dividem o bit 23 com a família de multiplicação, mas o resto da
        // máscara não: tratar os dois antes do `op` evita ler um SWP como UMULL.
        if instr & 0x0FB0_0FF0 == 0x0100_0090 {
            return self.arm_swp(instr, pc);
        }
        if instr & 0x0FF0_0FFF == 0x0190_0F9F || instr & 0x0FF0_0FF0 == 0x0180_0F90 {
            return self.arm_exclusivo(instr, pc);
        }
        let s = instr & (1 << 20) != 0;
        let rd = (instr >> 16) & 0xF;
        let rn = (instr >> 12) & 0xF;
        let rs = self.le_reg((instr >> 8) & 0xF, pc, 0);
        let rm = self.le_reg(instr & 0xF, pc, 0);
        let op = (instr >> 21) & 0xF;
        let saltou = match op {
            0b0000 => {
                // MUL Rd, Rm, Rs. O campo Rn tem de ser zero; se não for, ainda é MUL no v4.
                let valor = rm.wrapping_mul(rs);
                if s {
                    self.poe_nz(valor);
                }
                self.poe_rd(rd, valor, pc)?
            }
            0b0001 => {
                let acc = self.le_reg(rn, pc, 0);
                let valor = rm.wrapping_mul(rs).wrapping_add(acc);
                if s {
                    self.poe_nz(valor);
                }
                self.poe_rd(rd, valor, pc)?
            }
            0b0100 | 0b0101 | 0b0110 | 0b0111 => {
                let com_sinal = op & 0b0010 != 0;
                let acumula = op & 1 != 0;
                let produto = if com_sinal {
                    (rm as i32 as i64).wrapping_mul(rs as i32 as i64) as u64
                } else {
                    u64::from(rm).wrapping_mul(u64::from(rs))
                };
                let produto = if acumula {
                    let lo = self.le_reg(rn, pc, 0);
                    let hi = self.le_reg(rd, pc, 0);
                    produto.wrapping_add(u64::from(lo) | (u64::from(hi) << 32))
                } else {
                    produto
                };
                let lo = produto as u32;
                let hi = (produto >> 32) as u32;
                if s {
                    let flags = if hi & 0x8000_0000 != 0 { N } else { 0 }
                        | if produto == 0 { Z } else { 0 };
                    self.cpsr = (self.cpsr & !(N | Z)) | flags;
                }
                let s1 = self.poe_rd(rn, lo, pc)?;
                let s2 = self.poe_rd(rd, hi, pc)?;
                s1 || s2
            }
            _ => return Err(StopReason::Exception { pc }),
        };
        Ok(saltou)
    }

    fn arm_swp(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        if instr & 0x0FB0_0FF0 != 0x0100_0090 {
            return Err(StopReason::Exception { pc });
        }
        let rn = (instr >> 16) & 0xF;
        let rd = (instr >> 12) & 0xF;
        let rm = self.le_reg(instr & 0xF, pc, 0);
        let addr = self.le_reg(rn, pc, 0);
        let byte = instr & (1 << 22) != 0;
        if byte {
            let velho = self.le_mem(addr, 1, pc)?;
            self.es_mem(addr, &[rm as u8], pc)?;
            self.poe_rd(rd, velho, pc)
        } else {
            let velho = self.le_mem(addr, 4, pc)?;
            self.es_mem(addr, &rm.to_le_bytes(), pc)?;
            self.poe_rd(rd, velho, pc)
        }
    }

    fn arm_exclusivo(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let rn = (instr >> 16) & 0xF;
        let rd = (instr >> 12) & 0xF;
        let addr = self.le_reg(rn, pc, 0);
        // LDREX: bits 27–20 = 0001 1001. STREX: 0001 1000.
        if instr & 0x0FF0_0FFF == 0x0190_0F9F {
            let valor = self.le_mem(addr, 4, pc)?;
            self.exclusivo = Some(addr);
            return self.poe_rd(rd, valor, pc);
        }
        if instr & 0x0FF0_0FF0 == 0x0180_0F90 {
            let rm = self.le_reg(instr & 0xF, pc, 0);
            let ok = self.exclusivo == Some(addr);
            self.exclusivo = None;
            if ok {
                self.es_mem(addr, &rm.to_le_bytes(), pc)?;
            }
            return self.poe_rd(rd, u32::from(!ok), pc);
        }
        Err(StopReason::Exception { pc })
    }

    fn arm_media(&mut self, instr: u32, pc: u32) -> Result<bool, StopReason> {
        let rd = (instr >> 12) & 0xF;
        let rn = (instr >> 16) & 0xF;
        let rm = self.le_reg(instr & 0xF, pc, 0);
        let cabeca = (instr >> 20) & 0xFF;
        // REV / REV16 / REVSH moram no meio do espaço de extensão, com Rn = 15 e um opcode
        // próprio nos bits 11–4.
        if instr & 0x0FFF_0FF0 == 0x06BF_0F30 {
            return self.poe_rd(rd, rm.swap_bytes(), pc);
        }
        if instr & 0x0FFF_0FF0 == 0x06BF_0FB0 {
            let valor = ((rm & 0x00FF_00FF) << 8) | ((rm & 0xFF00_FF00) >> 8);
            return self.poe_rd(rd, valor, pc);
        }
        if instr & 0x0FFF_0FF0 == 0x06FF_0FB0 {
            let valor = (rm as u16).swap_bytes() as i16 as i32 as u32;
            return self.poe_rd(rd, valor, pc);
        }
        if matches!(cabeca, 0x6A | 0x6B | 0x6E | 0x6F) && instr & 0x70 == 0x70 {
            let rot = ((instr >> 10) & 3) * 8;
            let girado = rm.rotate_right(rot);
            let meia = cabeca & 1 == 1;
            let sinal = cabeca & 4 == 0;
            let estendido = if meia {
                if sinal {
                    girado as u16 as i16 as i32 as u32
                } else {
                    u32::from(girado as u16)
                }
            } else if sinal {
                girado as u8 as i8 as i32 as u32
            } else {
                u32::from(girado as u8)
            };
            let valor = if rn == 15 {
                estendido
            } else {
                self.le_reg(rn, pc, 0).wrapping_add(estendido)
            };
            return self.poe_rd(rd, valor, pc);
        }
        // SSAT é 0110101x, USAT é 0110111x. O bit que sobra é o imediato de saturação, e os
        // bits 5–4 = 01 separam isto da extensão, que já voltou acima.
        if matches!(cabeca & 0xFE, 0x6A | 0x6E) && instr & 0x30 == 0x10 {
            return self.arm_sat(instr, pc, cabeca & 0xFE == 0x6A);
        }
        Err(StopReason::Exception { pc })
    }

    fn arm_sat(&mut self, instr: u32, pc: u32, com_sinal: bool) -> Result<bool, StopReason> {
        let rd = (instr >> 12) & 0xF;
        let rn = self.le_reg(instr & 0xF, pc, 0);
        let sat_imm = (instr >> 16) & 0x1F;
        let sh = (instr >> 6) & 1;
        let imm5 = (instr >> 7) & 0x1F;
        let mut valor = rn as i32 as i64;
        if sh == 0 {
            valor <<= imm5;
        } else {
            let q = if imm5 == 0 { 32 } else { imm5 };
            valor >>= q;
        }
        let (saida, sat) = if com_sinal {
            let bits = sat_imm + 1;
            let max = (1i64 << (bits - 1)) - 1;
            let min = -(1i64 << (bits - 1));
            if valor > max {
                (max as u32, true)
            } else if valor < min {
                (min as u32, true)
            } else {
                (valor as u32, false)
            }
        } else {
            let max = if sat_imm == 31 {
                u32::MAX as i64
            } else {
                (1i64 << sat_imm) - 1
            };
            if valor < 0 {
                (0, true)
            } else if valor > max {
                (max as u32, true)
            } else {
                (valor as u32, false)
            }
        };
        if sat {
            self.poe_q();
        }
        self.poe_rd(rd, saida, pc)
    }

    fn thumb_exec(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let op = instr >> 11;
        match op {
            0b00000 | 0b00001 | 0b00010 => self.thumb_desloca_imm(instr, pc),
            0b00011 => self.thumb_add_sub(instr, pc),
            0b00100 | 0b00101 | 0b00110 | 0b00111 => self.thumb_imm(instr, pc),
            0b01000 => {
                if instr & (1 << 10) == 0 {
                    self.thumb_alu(instr, pc)
                } else {
                    self.thumb_alta(instr, pc)
                }
            }
            0b01001 => self.thumb_ldr_pc(instr, pc),
            0b01010 | 0b01011 => self.thumb_ls_reg(instr, pc),
            0b01100 | 0b01101 | 0b01110 | 0b01111 => self.thumb_ls_imm(instr, pc, false),
            0b10000 | 0b10001 => self.thumb_ls_imm(instr, pc, true),
            0b10010 | 0b10011 => self.thumb_ls_sp(instr, pc),
            0b10100 | 0b10101 => self.thumb_adr(instr, pc),
            0b10110 | 0b10111 => self.thumb_misc(instr, pc),
            0b11000 | 0b11001 => self.thumb_bloco(instr, pc),
            0b11010 | 0b11011 => self.thumb_bcond(instr, pc),
            0b11100 => {
                let imm = sext(u32::from(instr & 0x7FF), 11) << 1;
                self.desvia_sem_troca(pc.wrapping_add(4).wrapping_add(imm));
                Ok(true)
            }
            0b11101 | 0b11110 | 0b11111 => self.thumb_bl(instr, pc),
            _ => Err(StopReason::Exception { pc }),
        }
    }

    fn thumb_desloca_imm(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let tipo = u32::from((instr >> 11) & 3);
        let quant = u32::from((instr >> 6) & 0x1F);
        let rm = self.le_reg(u32::from((instr >> 3) & 7), pc, 0);
        let rd = u32::from(instr & 7);
        let (valor, carry) = desloca(rm, tipo, quant, self.flag_c(), true);
        self.poe_nz(valor);
        self.poe_c(carry);
        self.r[rd as usize] = valor;
        Ok(false)
    }

    fn thumb_add_sub(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rd = u32::from(instr & 7);
        let rn = self.le_reg(u32::from((instr >> 3) & 7), pc, 0);
        let imediato = instr & (1 << 10) != 0;
        let sub = instr & (1 << 9) != 0;
        let op = if imediato {
            u32::from((instr >> 6) & 7)
        } else {
            self.le_reg(u32::from((instr >> 6) & 7), pc, 0)
        };
        let (valor, c, v) = if sub {
            subtracao(rn, op, 0)
        } else {
            adicao(rn, op, 0)
        };
        self.poe_nz(valor);
        self.poe_c(c);
        self.poe_v(v);
        self.r[rd as usize] = valor;
        Ok(false)
    }

    fn thumb_imm(&mut self, instr: u16, _pc: u32) -> Result<bool, StopReason> {
        let kind = (instr >> 11) & 3;
        let rd = u32::from((instr >> 8) & 7);
        let imm = u32::from(instr & 0xFF);
        let atual = self.r[rd as usize];
        match kind {
            0 => {
                self.r[rd as usize] = imm;
                self.poe_nz(imm);
            }
            1 => {
                let (valor, c, v) = subtracao(atual, imm, 0);
                self.poe_nz(valor);
                self.poe_c(c);
                self.poe_v(v);
            }
            2 => {
                let (valor, c, v) = adicao(atual, imm, 0);
                self.r[rd as usize] = valor;
                self.poe_nz(valor);
                self.poe_c(c);
                self.poe_v(v);
            }
            _ => {
                let (valor, c, v) = subtracao(atual, imm, 0);
                self.r[rd as usize] = valor;
                self.poe_nz(valor);
                self.poe_c(c);
                self.poe_v(v);
            }
        }
        Ok(false)
    }

    fn thumb_alu(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let op = (instr >> 6) & 0xF;
        let rm = self.le_reg(u32::from((instr >> 3) & 7), pc, 0);
        let rd_i = u32::from(instr & 7);
        let rd = self.r[rd_i as usize];
        let c_in = self.flag_c();
        let (valor, logico, c, v, escreve) = match op {
            0x0 => (rd & rm, true, c_in, false, true),
            0x1 => (rd ^ rm, true, c_in, false, true),
            0x2 => {
                let (v, c) = desloca(rd, 0, rm & 0xFF, c_in, false);
                (v, true, c, false, true)
            }
            0x3 => {
                let (v, c) = desloca(rd, 1, rm & 0xFF, c_in, false);
                (v, true, c, false, true)
            }
            0x4 => {
                let (v, c) = desloca(rd, 2, rm & 0xFF, c_in, false);
                (v, true, c, false, true)
            }
            0x5 => {
                let (v, c, ov) = adicao(rd, rm, u32::from(c_in));
                (v, false, c, ov, true)
            }
            0x6 => {
                let (v, c, ov) = subtracao(rd, rm, u32::from(!c_in));
                (v, false, c, ov, true)
            }
            0x7 => {
                let (v, c) = desloca(rd, 3, rm & 0xFF, c_in, false);
                (v, true, c, false, true)
            }
            0x8 => (rd & rm, true, c_in, false, false),
            0x9 => {
                let (v, c, ov) = subtracao(0, rm, 0);
                (v, false, c, ov, true)
            }
            0xA => {
                let (v, c, ov) = subtracao(rd, rm, 0);
                (v, false, c, ov, false)
            }
            0xB => {
                let (v, c, ov) = adicao(rd, rm, 0);
                (v, false, c, ov, false)
            }
            0xC => (rd | rm, true, c_in, false, true),
            0xD => (rd.wrapping_mul(rm), true, c_in, false, true),
            0xE => (rd & !rm, true, c_in, false, true),
            _ => (!rm, true, c_in, false, true),
        };
        self.poe_nz(valor);
        if logico {
            // O MUL do Thumb no ARMv6 não mexe em C. Os deslocamentos mexem.
            if matches!(op, 0x2 | 0x3 | 0x4 | 0x7) {
                self.poe_c(c);
            }
        } else {
            self.poe_c(c);
            self.poe_v(v);
        }
        if escreve {
            self.r[rd_i as usize] = valor;
        }
        Ok(false)
    }

    fn thumb_alta(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let op = (instr >> 8) & 3;
        let h1 = u32::from((instr >> 7) & 1);
        let h2 = u32::from((instr >> 6) & 1);
        let rm = (u32::from((instr >> 3) & 7)) | (h2 << 3);
        let rd = (u32::from(instr & 7)) | (h1 << 3);
        let a = self.le_reg(rd, pc, 0);
        let b = self.le_reg(rm, pc, 0);
        match op {
            0 => {
                let valor = a.wrapping_add(b);
                if rd == 15 {
                    self.desvia_sem_troca(valor);
                    Ok(true)
                } else {
                    self.r[rd as usize] = valor;
                    Ok(false)
                }
            }
            1 => {
                let (valor, c, v) = subtracao(a, b, 0);
                self.poe_nz(valor);
                self.poe_c(c);
                self.poe_v(v);
                Ok(false)
            }
            2 => {
                if rd == 15 {
                    self.desvia_sem_troca(b);
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
        // O bit 1 do PC é forçado a zero: a palavra está alinhada, mesmo com a instrução em
        // endereço ímpar de meia-palavra.
        let addr = (pc.wrapping_add(4) & !3).wrapping_add(imm);
        let valor = self.le_mem(addr, 4, pc)?;
        self.r[rd as usize] = valor;
        Ok(false)
    }

    fn thumb_ls_reg(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rd = u32::from(instr & 7);
        let rn = self.le_reg(u32::from((instr >> 3) & 7), pc, 0);
        let rm = self.le_reg(u32::from((instr >> 6) & 7), pc, 0);
        let addr = rn.wrapping_add(rm);
        let op = (instr >> 9) & 7;
        match op {
            0 => self
                .es_mem(addr, &self.r[rd as usize].to_le_bytes(), pc)
                .map(|()| false),
            1 => self
                .es_mem(addr, &(self.r[rd as usize] as u16).to_le_bytes(), pc)
                .map(|()| false),
            2 => self
                .es_mem(addr, &[self.r[rd as usize] as u8], pc)
                .map(|()| false),
            3 => {
                let v = self.le_mem(addr, 1, pc)? as i8 as i32 as u32;
                self.r[rd as usize] = v;
                Ok(false)
            }
            4 => {
                let v = self.le_mem(addr, 4, pc)?;
                self.r[rd as usize] = v;
                Ok(false)
            }
            5 => {
                let v = self.le_mem(addr, 2, pc)?;
                self.r[rd as usize] = v;
                Ok(false)
            }
            6 => {
                let v = self.le_mem(addr, 1, pc)?;
                self.r[rd as usize] = v;
                Ok(false)
            }
            _ => {
                let v = self.le_mem(addr, 2, pc)? as i16 as i32 as u32;
                self.r[rd as usize] = v;
                Ok(false)
            }
        }
    }

    fn thumb_ls_imm(&mut self, instr: u16, pc: u32, meia: bool) -> Result<bool, StopReason> {
        let rd = u32::from(instr & 7);
        let rn = self.le_reg(u32::from((instr >> 3) & 7), pc, 0);
        let imm5 = u32::from((instr >> 6) & 0x1F);
        let l = instr & (1 << 11) != 0;
        let (addr, len, byte) = if meia {
            (rn.wrapping_add(imm5 << 1), 2, false)
        } else if (instr >> 12) & 1 == 0 {
            (rn.wrapping_add(imm5 << 2), 4, false)
        } else {
            (rn.wrapping_add(imm5), 1, true)
        };
        if l {
            let valor = self.le_mem(addr, len, pc)?;
            self.r[rd as usize] = valor;
            Ok(false)
        } else {
            let valor = self.r[rd as usize];
            if byte {
                self.es_mem(addr, &[valor as u8], pc)?;
            } else if len == 2 {
                self.es_mem(addr, &(valor as u16).to_le_bytes(), pc)?;
            } else {
                self.es_mem(addr, &valor.to_le_bytes(), pc)?;
            }
            Ok(false)
        }
    }

    fn thumb_ls_sp(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rd = u32::from((instr >> 8) & 7);
        let imm = u32::from(instr & 0xFF) << 2;
        let addr = self.r[13].wrapping_add(imm);
        if instr & (1 << 11) != 0 {
            self.r[rd as usize] = self.le_mem(addr, 4, pc)?;
        } else {
            self.es_mem(addr, &self.r[rd as usize].to_le_bytes(), pc)?;
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
        let grupo = (instr >> 8) & 0xF;
        match grupo {
            0b0000 => {
                let imm = u32::from(instr & 0x7F) << 2;
                if instr & (1 << 7) != 0 {
                    self.r[13] = self.r[13].wrapping_sub(imm);
                } else {
                    self.r[13] = self.r[13].wrapping_add(imm);
                }
                Ok(false)
            }
            0b0010 => self.thumb_estende(instr, pc),
            0b0100 | 0b0101 => self.thumb_push(instr, pc, false),
            0b1010 => self.thumb_rev(instr, pc),
            0b1100 | 0b1101 => self.thumb_push(instr, pc, true),
            0b1110 => Err(StopReason::Exception { pc }),
            _ => Err(StopReason::Exception { pc }),
        }
    }

    fn thumb_estende(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rm = self.le_reg(u32::from((instr >> 3) & 7), pc, 0);
        let rd = u32::from(instr & 7);
        let valor = match (instr >> 6) & 3 {
            0 => rm as u16 as i16 as i32 as u32,
            1 => rm as u8 as i8 as i32 as u32,
            2 => u32::from(rm as u16),
            _ => u32::from(rm as u8),
        };
        self.r[rd as usize] = valor;
        Ok(false)
    }

    fn thumb_rev(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rm = self.le_reg(u32::from((instr >> 3) & 7), pc, 0);
        let rd = u32::from(instr & 7);
        let valor = match (instr >> 6) & 3 {
            0 => rm.swap_bytes(),
            1 => ((rm & 0x00FF_00FF) << 8) | ((rm & 0xFF00_FF00) >> 8),
            3 => (rm as u16).swap_bytes() as i16 as i32 as u32,
            _ => return Err(StopReason::Exception { pc }),
        };
        self.r[rd as usize] = valor;
        Ok(false)
    }

    fn thumb_push(&mut self, instr: u16, pc: u32, pop: bool) -> Result<bool, StopReason> {
        let mut lista = u32::from(instr & 0xFF);
        let extra = instr & (1 << 8) != 0;
        if extra {
            lista |= if pop { 1 << 15 } else { 1 << 14 };
        }
        let n = lista.count_ones();
        if n == 0 {
            return Err(StopReason::Exception { pc });
        }
        if pop {
            let mut addr = self.r[13];
            let mut saltou = false;
            for reg in 0..16 {
                if lista & (1 << reg) != 0 {
                    let valor = self.le_mem(addr, 4, pc)?;
                    if reg == 15 {
                        self.bx(valor);
                        saltou = true;
                    } else {
                        self.r[reg] = valor;
                    }
                    addr = addr.wrapping_add(4);
                }
            }
            self.r[13] = self.r[13].wrapping_add(n * 4);
            Ok(saltou)
        } else {
            let mut addr = self.r[13].wrapping_sub(n * 4);
            self.r[13] = addr;
            for reg in 0..16 {
                if lista & (1 << reg) != 0 {
                    let valor = self.le_reg(reg, pc, 0);
                    self.es_mem(addr, &valor.to_le_bytes(), pc)?;
                    addr = addr.wrapping_add(4);
                }
            }
            Ok(false)
        }
    }

    fn thumb_bloco(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let rn = u32::from((instr >> 8) & 7);
        let lista = u32::from(instr & 0xFF);
        let l = instr & (1 << 11) != 0;
        let n = lista.count_ones();
        if n == 0 {
            return Err(StopReason::Exception { pc });
        }
        let mut addr = self.r[rn as usize];
        if l {
            for reg in 0..8 {
                if lista & (1 << reg) != 0 {
                    self.r[reg] = self.le_mem(addr, 4, pc)?;
                    addr = addr.wrapping_add(4);
                }
            }
        } else {
            for reg in 0..8 {
                if lista & (1 << reg) != 0 {
                    self.es_mem(addr, &self.r[reg].to_le_bytes(), pc)?;
                    addr = addr.wrapping_add(4);
                }
            }
        }
        if !l || lista & (1 << rn) == 0 {
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
        if condicao(self.cpsr, cond) {
            let imm = sext(u32::from(instr & 0xFF), 8) << 1;
            self.desvia_sem_troca(pc.wrapping_add(4).wrapping_add(imm));
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn thumb_bl(&mut self, instr: u16, pc: u32) -> Result<bool, StopReason> {
        let prefixo = (instr >> 11) & 0x1F;
        let imm = u32::from(instr & 0x7FF);
        match prefixo {
            0b11110 => {
                // O bit 1 sai: o par ocupa duas meias-palavras e o offset é de palavra.
                let base = pc.wrapping_add(4) & !2;
                self.r[14] = base.wrapping_add(sext(imm, 11) << 12);
                Ok(false)
            }
            0b11111 => {
                let proxima = pc.wrapping_add(2);
                self.r[15] = self.r[14].wrapping_add(imm << 1);
                self.r[14] = proxima | 1;
                self.cpsr |= T;
                Ok(true)
            }
            0b11101 => {
                let proxima = pc.wrapping_add(2);
                let alvo = self.r[14].wrapping_add(imm << 1) & !3;
                self.r[14] = proxima | 1;
                self.cpsr &= !T;
                self.r[15] = alvo;
                Ok(true)
            }
            _ => Err(StopReason::Exception { pc }),
        }
    }
}

fn meia(valor: u32, alta: bool) -> i32 {
    let h = if alta { valor >> 16 } else { valor & 0xFFFF };
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

fn sext(valor: u32, bits: u32) -> u32 {
    let shift = 32 - bits;
    (((valor << shift) as i32) >> shift) as u32
}

fn condicao(cpsr: u32, cond: u32) -> bool {
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

fn adicao(a: u32, b: u32, carry: u32) -> (u32, bool, bool) {
    let (s1, c1) = a.overflowing_add(b);
    let (s2, c2) = s1.overflowing_add(carry);
    let c = c1 || c2;
    let v = (a ^ s2) & (b ^ s2) & 0x8000_0000 != 0;
    (s2, c, v)
}

/// Carry no sentido do ARM: `true` quer dizer que **não** houve borrow.
fn subtracao(a: u32, b: u32, borrow: u32) -> (u32, bool, bool) {
    let (s1, b1) = a.overflowing_sub(b);
    let (s2, b2) = s1.overflowing_sub(borrow);
    let c = !(b1 || b2);
    let v = (a ^ b) & (a ^ s2) & 0x8000_0000 != 0;
    (s2, c, v)
}

/// `(resultado, carry de saída)`.
///
/// Quantidade imediata zero não é "não desloca" para LSR, ASR e ROR: a codificação guarda o
/// 32 nesse zero, e o ROR zero é o RRX. Quantidade vinda de registrador não tem esse atalho —
/// zero de verdade não desloca.
fn desloca(valor: u32, tipo: u32, quant: u32, carry: bool, imediato: bool) -> (u32, bool) {
    if !imediato && quant == 0 {
        return (valor, carry);
    }
    match tipo {
        0 => {
            if quant == 0 {
                (valor, carry)
            } else if quant < 32 {
                (valor << quant, valor & (1 << (32 - quant)) != 0)
            } else if quant == 32 {
                (0, valor & 1 != 0)
            } else {
                (0, false)
            }
        }
        1 => {
            let q = if imediato && quant == 0 { 32 } else { quant };
            if q < 32 {
                (valor >> q, valor & (1 << (q - 1)) != 0)
            } else if q == 32 {
                (0, valor & 0x8000_0000 != 0)
            } else {
                (0, false)
            }
        }
        2 => {
            let q = if imediato && quant == 0 { 32 } else { quant };
            if q < 32 {
                let saida = ((valor as i32) >> q) as u32;
                (saida, valor & (1 << (q - 1)) != 0)
            } else {
                let sinal = valor & 0x8000_0000 != 0;
                (if sinal { 0xFFFF_FFFF } else { 0 }, sinal)
            }
        }
        _ => {
            if imediato && quant == 0 {
                let bit = u32::from(carry);
                ((valor >> 1) | (bit << 31), valor & 1 != 0)
            } else if quant == 0 {
                (valor, carry)
            } else {
                let q = quant & 31;
                if q == 0 {
                    (valor, valor & 0x8000_0000 != 0)
                } else {
                    let saida = valor.rotate_right(q);
                    (saida, saida & 0x8000_0000 != 0)
                }
            }
        }
    }
}

/// `(valor, escreve no Rd, C vem do shifter, carry aritmético, overflow)`.
fn alu(opcode: u32, a: u32, b: u32, carry: bool) -> (u32, bool, bool, bool, bool) {
    let cin = u32::from(carry);
    match opcode {
        0x0 => (a & b, true, true, false, false),
        0x1 => (a ^ b, true, true, false, false),
        0x2 => {
            let (v, c, ov) = subtracao(a, b, 0);
            (v, true, false, c, ov)
        }
        0x3 => {
            let (v, c, ov) = subtracao(b, a, 0);
            (v, true, false, c, ov)
        }
        0x4 => {
            let (v, c, ov) = adicao(a, b, 0);
            (v, true, false, c, ov)
        }
        0x5 => {
            let (v, c, ov) = adicao(a, b, cin);
            (v, true, false, c, ov)
        }
        0x6 => {
            let (v, c, ov) = subtracao(a, b, u32::from(!carry));
            (v, true, false, c, ov)
        }
        0x7 => {
            let (v, c, ov) = subtracao(b, a, u32::from(!carry));
            (v, true, false, c, ov)
        }
        0x8 => (a & b, false, true, false, false),
        0x9 => (a ^ b, false, true, false, false),
        0xA => {
            let (v, c, ov) = subtracao(a, b, 0);
            (v, false, false, c, ov)
        }
        0xB => {
            let (v, c, ov) = adicao(a, b, 0);
            (v, false, false, c, ov)
        }
        0xC => (a | b, true, true, false, false),
        0xD => (b, true, true, false, false),
        0xE => (a & !b, true, true, false, false),
        _ => (!b, true, true, false, false),
    }
}

impl CpuBackend for Interpretador {
    fn reset(&mut self, mem: &GuestMemory) -> Result<(), CpuError> {
        let mut copia = GuestMemory::new();
        for regiao in mem.regions() {
            copia
                .map_com_execucao(
                    regiao.name,
                    regiao.base,
                    regiao.bytes.clone(),
                    regiao.writable,
                    regiao.executavel,
                )
                .map_err(|e| CpuError(e.to_string()))?;
        }
        self.mem = copia;
        self.r = [0; 16];
        self.cpsr = MODO_USUARIO;
        self.instrucoes = 0;
        self.vigias.clear();
        self.semihosting.clear();
        self.exclusivo = None;
        Ok(())
    }

    fn read_reg(&self, reg: Reg) -> u32 {
        self.r[indice(reg)]
    }

    fn write_reg(&mut self, reg: Reg, value: u32) {
        self.r[indice(reg)] = value;
    }

    fn run(&mut self, pc: u32, max_instructions: u64) -> Result<StopReason, CpuError> {
        if pc & 1 == 1 {
            self.cpsr |= T;
        } else {
            self.cpsr &= !T;
        }
        self.r[15] = pc & !1;
        let limite = self.instrucoes.saturating_add(max_instructions);
        loop {
            if self.instrucoes >= limite {
                return Ok(StopReason::Budget);
            }
            match self.passo() {
                Ok(saltou) => {
                    if !saltou {
                        let passo = if self.thumb() { 2 } else { 4 };
                        self.r[15] = self.r[15].wrapping_add(passo);
                    }
                    self.instrucoes += 1;
                }
                Err(motivo) => return Ok(motivo),
            }
        }
    }

    fn instructions(&self) -> u64 {
        self.instrucoes
    }

    fn set_instructions(&mut self, valor: u64) {
        self.instrucoes = valor;
    }

    fn cpsr(&self) -> u32 {
        self.cpsr
    }

    fn set_cpsr(&mut self, valor: u32) {
        self.cpsr = valor;
    }

    fn em_thumb(&self) -> bool {
        self.thumb()
    }

    fn watch_dirty(&mut self, id: u32, base: u32, len: u32) -> Result<(), CpuError> {
        self.unwatch_dirty(id);
        self.vigias.push((id, base, base.saturating_add(len), true));
        Ok(())
    }

    fn unwatch_dirty(&mut self, id: u32) {
        self.vigias.retain(|vigia| vigia.0 != id);
    }

    fn take_dirty(&mut self, id: u32) -> bool {
        match self.vigias.iter_mut().find(|vigia| vigia.0 == id) {
            Some(vigia) => std::mem::replace(&mut vigia.3, false),
            None => true,
        }
    }

    fn marca_sujo(&mut self, addr: u32, len: u32) {
        self.suja(addr, len);
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

fn indice(reg: Reg) -> usize {
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

    fn cpu_with(code: &[u8]) -> Interpretador {
        let mut mem = GuestMemory::new();
        let mut bytes = code.to_vec();
        bytes.resize(0x1000, 0);
        mem.map("code", 0, bytes, true).unwrap();
        mem.map_zeroed("data", 0x1000, 0x1000).unwrap();
        mem.map_zeroed("stack", 0x2000_0000, 0x1000).unwrap();
        let mut cpu = Interpretador::new().unwrap();
        cpu.reset(&mem).unwrap();
        cpu.write_reg(Reg::Sp, 0x2000_0f00);
        cpu
    }

    fn arm(palavras: &[u32]) -> Vec<u8> {
        palavras.iter().flat_map(|p| p.to_le_bytes()).collect()
    }

    #[test]
    fn executa_instrucao_e_le_registrador() {
        let mut cpu = cpu_with(&arm(&[0xe3a0_0037, 0xeaff_fffe]));
        assert_eq!(cpu.run(0, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    #[test]
    fn salto_para_a_faixa_de_api_vira_chamada() {
        let mut cpu = cpu_with(&arm(&[0xe3a0_020f, 0xe12f_ff10]));
        assert_eq!(
            cpu.run(0, 10).unwrap(),
            StopReason::ApiCall { addr: API_BASE }
        );
    }

    #[test]
    fn endereco_com_bit_zero_entra_em_thumb() {
        let mut code = vec![0u8; 0x100];
        code.extend_from_slice(&[0x37, 0x20, 0xfe, 0xe7]);
        let mut cpu = cpu_with(&code);
        assert_eq!(cpu.run(0x101, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    #[test]
    fn endereco_par_volta_para_arm_depois_de_thumb() {
        let mut code = arm(&[0xe3a0_0037, 0xeaff_fffe]);
        code.resize(0x100, 0);
        code.extend_from_slice(&[0x00, 0x20, 0xfe, 0xe7]);
        let mut cpu = cpu_with(&code);
        cpu.run(0x101, 1).unwrap();
        assert_eq!(cpu.run(0, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    #[test]
    fn a_vigia_liga_com_escrita_do_guest_e_nao_com_a_do_host() {
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
    fn retorno_para_o_sentinela_e_reconhecido() {
        let mut cpu = cpu_with(&arm(&[0xe12f_ff1e]));
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
    }

    #[test]
    fn semihosting_escreve_sem_interromper_o_guest() {
        let mut cpu = cpu_with(&arm(&[0xe3a0_0003, 0xe3a0_1a01, 0xef00_00ab, 0xe12f_ff1e]));
        cpu.write_mem(0x1000, b"Z").unwrap();
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.semihosting(), "Z");
        assert_eq!(cpu.read_reg(Reg::R0), 0);
    }

    #[test]
    fn codigo_alterado_pelo_host_e_executado() {
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
    fn adds_atualiza_flags_e_o_deslocamento_de_32_poe_o_carry() {
        // adds r0, r1, #1  com r1 = 0xFFFFFFFF → 0, C e Z
        let mut cpu = cpu_with(&arm(&[0xe291_0001, 0xe1b0_1020, 0xe12f_ff1e]));
        cpu.write_reg(Reg::R1, 0xFFFF_FFFF);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0);
        assert_ne!(cpu.cpsr() & Z, 0);
        assert_ne!(cpu.cpsr() & C, 0);
        // movs r1, r0, lsr #32 — r0 ainda é 0 aqui; reexecuta a partir da segunda instrução
        // com r0 = 0x80000000.
        cpu.write_reg(Reg::R0, 0x8000_0000);
        cpu.run(4, 1).unwrap();
        assert_eq!(cpu.read_reg(Reg::R1), 0);
        assert_ne!(cpu.cpsr() & C, 0);
        assert_ne!(cpu.cpsr() & Z, 0);
    }

    #[test]
    fn ldr_str_e_ldm_voltam_os_bytes() {
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
    fn clz_rev_e_sxtb() {
        let mut cpu = cpu_with(&arm(&[0xe16f_0f11, 0xe6bf_0f32, 0xe6af_1071, 0xe12f_ff1e]));
        cpu.write_reg(Reg::R1, 0x0001_0000);
        cpu.write_reg(Reg::R2, 0x1234_5678);
        cpu.write_reg(Reg::R1, 0x0001_0000);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        // A terceira instrução lê r1 de novo, então o SXTB usa o valor original só se a gente
        // não tiver sobrescrito r1. CLZ escreve r0. REV escreve r0 a partir de r2. SXTB lê r1.
        cpu.write_reg(Reg::R1, 0x80);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 0x7856_3412);
        assert_eq!(cpu.read_reg(Reg::R1), 0xFFFF_FF80);
    }

    #[test]
    fn clz_de_zero_e_trinta_e_dois() {
        let mut cpu = cpu_with(&arm(&[0xe16f_0f11, 0xe12f_ff1e]));
        cpu.write_reg(Reg::R1, 0);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 32);
    }

    #[test]
    fn bl_thumb_grava_o_retorno_com_bit_de_thumb() {
        // BL para 8, depois `movs r0, #1` e `bx lr` de quem chamou não: o destino faz `movs r1, #7`.
        // no 0: f000 f801  (BL, imm hi 0, imm lo 1 → PC = 4 + 2 = 6? )
        // Recalculado no próprio teste: o destino é 8.
        // primeira: LR = (0+4) + 0 = 4
        // segunda em 2: PC = 4 + (imm<<1). Para cair em 8, imm = 2. 11111 00000000010 = 0xF802
        let mut code = vec![0x00, 0xF0, 0x02, 0xF8];
        code.resize(8, 0);
        code.extend_from_slice(&[0x07, 0x21]); // movs r1, #7
        code.extend_from_slice(&[0x70, 0x47]); // bx lr — volta para o endereço logo após o par
        let mut cpu = cpu_with(&code);
        // Quatro instruções: as duas metades do BL, o movs e o bx. A próxima seria a palavra
        // zero que ficou no endereço 4, e o orçamento acaba antes dela.
        assert_eq!(cpu.run(1, 4).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R1), 7);
        assert_eq!(cpu.read_reg(Reg::Lr) & 1, 1);
        assert_eq!(cpu.read_reg(Reg::Pc), 4);
    }

    #[test]
    fn ssat_corta_e_liga_q() {
        // `SSAT r0, #8, r1`. O bit 4 tem de estar ligado: sem ele a palavra cai no espaço de
        // load/store e o teste passa a medir outra instrução.
        let mut cpu = cpu_with(&arm(&[0xe6a7_0011, 0xe12f_ff1e]));
        cpu.write_reg(Reg::R1, 200);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 127);
        assert_ne!(cpu.cpsr() & Q, 0);
    }

    #[test]
    fn condicional_falha_nao_escreve() {
        // cmp r0, #1; moveq r1, #2; bx lr
        let mut cpu = cpu_with(&arm(&[0xe350_0001, 0x03a0_1002, 0xe12f_ff1e]));
        cpu.write_reg(Reg::R0, 0);
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R1), 0);
    }
}
