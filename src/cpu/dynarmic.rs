//! Backend ARM sobre Dynarmic.
//!
//! O Unicorn usa o TCG do QEMU; ele é robusto, mas o renderer ARM que o Kingdom Hearts traz
//! em `swv21brew.mod` executa centenas de milhões de instruções por poucos segundos de jogo.
//! Dynarmic recompila os blocos A32 para o código nativo do host. Este arquivo mantém a mesma
//! fronteira do [`CpuBackend`]: os endereços não mapeados das vtables BREW continuam sendo a
//! parada que devolve o controle ao despachante Rust.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::mem::size_of;
use std::rc::Rc;

use dynarmic::a32::{ArchVersion, Callbacks, Dynarmic as Jit, VAddr};
use dynarmic::{CallbackImpl, GuestInt, HaltReason};

use super::mem::GuestMemory;
use super::unicorn::{API_BASE, API_SIZE, RETURN_MAGIC};
use super::{CpuBackend, CpuError, Reg, StopReason};

/// `CPSR` de modo usuário do ARM. A extensão da Superscape confere este campo antes de tocar
/// nas tabelas da MMU; o applet BREW nunca é código privilegiado.
const MODO_USUARIO: u32 = 0x10;

/// Limite defensivo da string recebida por `SYS_WRITE0`. É o mesmo contrato do backend
/// Unicorn: uma string sem terminador não pode prender o host em uma leitura sem fim.
const MAX_SEMIHOSTING_STRING: u32 = 4096;

/// Granularidade que o Dynarmic usa para indexar código recompilado.
const PAGE: u32 = 4096;

/// O que uma callback pediu que `run` devolva. O Dynarmic recebe os acessos de memória dentro
/// do bloco recompilado; guardar o motivo aqui preserva a distinção entre API, retorno e falha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parada {
    Nenhuma,
    Fetch(u32),
    Memoria(u32),
    Excecao(u32),
}

/// Estado compartilhado entre as callbacks C++ e o invólucro Rust.
///
/// O JIT é guardado em `Box` no backend, então seu endereço não muda quando o `DynarmicCpu`
/// se move. Isso permite à callback interromper a execução no instante em que o PC entra numa
/// vtable BREW, em vez de executar uma instrução falsa naquele endereço.
struct Estado {
    memoria: Rc<RefCell<GuestMemory>>,
    semihosting: Rc<RefCell<String>>,
    /// Só uma escrita em memória que já foi buscada como código pode invalidar um bloco JIT.
    /// Isto exclui os milhões de escritas nos buffers RGB565.
    paginas_executadas: RefCell<BTreeSet<u32>>,
    /// Invalidações pedidas pelo ARM durante o bloco em execução; são aplicadas após `run`.
    codigo_sujo: RefCell<BTreeSet<u32>>,
    instrucoes: Cell<u64>,
    limite: Cell<u64>,
    parada: Cell<Parada>,
    jit: Cell<*mut Jit<Estado>>,
}

impl Estado {
    fn para(cb: &CallbackImpl<Self>, parada: Parada) {
        cb.parada.set(parada);
        let jit = cb.jit.get();
        if !jit.is_null() {
            // `HaltExecution` é a via documentada pelo Dynarmic para uma callback devolver o
            // controle ao host. A razão concreta é mantida em `parada`, pois o enum público só
            // oferece os bits genéricos de usuário.
            unsafe { (*jit).halt(HaltReason::UserDefined1) };
        }
    }

    fn le(&self, addr: u32, out: &mut [u8]) -> bool {
        self.memoria
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

    fn escreve(&self, addr: u32, bytes: &[u8]) -> bool {
        if self.memoria.borrow_mut().write(addr, bytes).is_err() {
            return false;
        }
        self.marca_codigo_sujo(addr, bytes.len() as u32);
        true
    }

    fn marca_codigo_sujo(&self, addr: u32, len: u32) {
        let fim = addr.saturating_add(len.saturating_sub(1));
        let executadas = self.paginas_executadas.borrow();
        let mut sujas = self.codigo_sujo.borrow_mut();
        for pagina in (addr / PAGE)..=(fim / PAGE) {
            if executadas.contains(&pagina) {
                sujas.insert(pagina);
            }
        }
    }
}

impl Callbacks for Estado {
    fn memory_read_code(cb: &CallbackImpl<Self>, addr: VAddr) -> Option<u32> {
        let mut bytes = [0; 4];
        if cb.le(addr, &mut bytes) {
            cb.paginas_executadas.borrow_mut().insert(addr / PAGE);
            Some(u32::from_le_bytes(bytes))
        } else {
            Self::para(cb, Parada::Fetch(addr));
            None
        }
    }

    extern "C" fn memory_read<T: GuestInt>(cb: &CallbackImpl<Self>, addr: VAddr) -> T {
        let mut bytes = [0u8; 8];
        let len = size_of::<T>();
        if !cb.le(addr, &mut bytes[..len]) {
            Self::para(cb, Parada::Memoria(addr));
        }
        // Os hosts aceitos pelo Dynarmic são x86-64 e AArch64, ambos little-endian. `T` só é
        // instanciado pelo JIT para u8/u16/u32/u64.
        unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast::<T>()) }
    }

    extern "C" fn memory_write<T: GuestInt>(cb: &mut CallbackImpl<Self>, addr: VAddr, value: T) {
        let len = size_of::<T>();
        let bytes = unsafe { std::slice::from_raw_parts((&value as *const T).cast::<u8>(), len) };
        if !cb.escreve(addr, bytes) {
            Self::para(cb, Parada::Memoria(addr));
        }
    }

    extern "C" fn call_svc(cb: &mut CallbackImpl<Self>, swi: u32) {
        // `SVC #0xAB` é o semihosting ARM que Peggle e Zuma usam para log. Não é uma
        // interrupção BREW: depois de atendê-la a execução continua na instrução seguinte.
        // Reconhecemos as duas operações de saída que o Unicorn já suporta e devolvemos zero
        // nas outras, como o monitor ARM faz para a maioria das consultas inofensivas.
        if swi == 0xab {
            let jit = unsafe { &mut *cb.jit.get() };
            let op = jit.get_reg(0);
            let arg = jit.get_reg(1);
            let mut saida = cb.semihosting.borrow_mut();
            match op {
                0x03 => {
                    let mut byte = [0u8; 1];
                    if cb.le(arg, &mut byte) {
                        saida.push(byte[0] as char);
                    }
                }
                0x04 => {
                    for offset in 0..MAX_SEMIHOSTING_STRING {
                        let mut byte = [0u8; 1];
                        if !cb.le(arg.saturating_add(offset), &mut byte) || byte[0] == 0 {
                            break;
                        }
                        saida.push(byte[0] as char);
                    }
                }
                _ => {}
            }
            jit.set_reg(0, 0);
        } else if cb.parada.get() == Parada::Nenhuma {
            let pc = unsafe { (*cb.jit.get()).get_pc() };
            Self::para(cb, Parada::Excecao(pc));
        }
    }

    extern "C" fn add_ticks(cb: &mut CallbackImpl<Self>, ticks: u64) {
        cb.instrucoes.set(cb.instrucoes.get().saturating_add(ticks));
    }

    extern "C" fn get_ticks_remaining(cb: &CallbackImpl<Self>) -> u64 {
        cb.limite.get().saturating_sub(cb.instrucoes.get())
    }

    extern "C" fn raised_exception(
        cb: &mut CallbackImpl<Self>,
        pc: VAddr,
        _exception: ::dynarmic::a32::Exception,
    ) {
        // `MemoryReadCode(None)` termina como `NoExecuteFault`, que chega aqui depois de a
        // callback já ter registrado o fetch. Não sobrescrevê-lo é o que mantém a fronteira
        // das vtables BREW distinguível de uma instrução realmente inválida.
        if cb.parada.get() == Parada::Nenhuma {
            // A binding atual transforma `optional<u32>::none()` do fetch em
            // `NoExecuteFault` antes de devolver o controle. A faixa das APIs e o sentinela
            // nunca contêm código por definição, então ainda dá para classificá-los aqui sem
            // ambiguidade.
            if pc == RETURN_MAGIC || (API_BASE..API_BASE.saturating_add(API_SIZE)).contains(&pc) {
                Self::para(cb, Parada::Fetch(pc));
            } else {
                Self::para(cb, Parada::Excecao(pc));
            }
        }
    }
}

/// Recompilador A32. Fica separado do backend padrão até a equivalência ser estabelecida jogo
/// a jogo; criar a CPU não aloca o JIT, porque o mapa do guest só existe em `reset`.
pub struct DynarmicCpu {
    memoria: Rc<RefCell<GuestMemory>>,
    semihosting: Rc<RefCell<String>>,
    jit: Option<Box<Jit<Estado>>>,
}

impl DynarmicCpu {
    pub fn new() -> Result<Self, CpuError> {
        Ok(Self {
            memoria: Default::default(),
            semihosting: Default::default(),
            jit: None,
        })
    }

    fn jit(&self) -> Result<&Jit<Estado>, CpuError> {
        self.jit
            .as_deref()
            .ok_or_else(|| CpuError("Dynarmic não recebeu mapa de memória".into()))
    }

    fn jit_mut(&mut self) -> Result<&mut Jit<Estado>, CpuError> {
        self.jit
            .as_deref_mut()
            .ok_or_else(|| CpuError("Dynarmic não recebeu mapa de memória".into()))
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

    /// A interface de diagnóstico chama este método nos dois núcleos.
    pub fn semihosting(&self) -> String {
        self.semihosting.borrow().clone()
    }

    /// A API host também escreve memória do guest, sempre entre duas entradas no JIT. Caso ela
    /// altere uma página que já foi executada, o próximo bloco deve ser recompilado.
    fn invalida_codigo_escrito(&mut self, addr: u32, len: u32) {
        let Ok(jit) = self.jit_mut() else {
            return;
        };
        jit.marca_codigo_sujo(addr, len);
        let paginas = std::mem::take(&mut *jit.codigo_sujo.borrow_mut());
        for pagina in paginas {
            jit.invalidate_cache_range(pagina * PAGE, PAGE as usize);
        }
    }
}

impl CpuBackend for DynarmicCpu {
    fn reset(&mut self, mem: &GuestMemory) -> Result<(), CpuError> {
        // `GuestMemory` é deliberadamente construído uma vez antes da execução. Copiar o mapa
        // para a memória que as callbacks possuem mantém a mesma semântica do Unicorn: a API
        // Rust e o ARM enxergam os mesmos bytes a partir daqui.
        let mut copia = GuestMemory::new();
        for regiao in mem.regions() {
            copia
                .map(
                    regiao.name,
                    regiao.base,
                    regiao.bytes.clone(),
                    regiao.writable,
                )
                .map_err(|e| CpuError(e.to_string()))?;
        }
        self.memoria = Rc::new(RefCell::new(copia));
        self.semihosting.borrow_mut().clear();
        let estado = Estado {
            memoria: self.memoria.clone(),
            semihosting: self.semihosting.clone(),
            paginas_executadas: Default::default(),
            codigo_sujo: Default::default(),
            instrucoes: Cell::new(0),
            limite: Cell::new(0),
            parada: Cell::new(Parada::Nenhuma),
            jit: Cell::new(std::ptr::null_mut()),
        };
        let mut config = Jit::<Estado>::new_config();
        config.arch_ver(ArchVersion::V6K);
        config.code_cache_size(64 * 1024 * 1024);
        let mut jit = Box::new(config.init(estado));
        let ptr = &mut *jit as *mut Jit<Estado>;
        jit.jit.set(ptr);
        jit.set_cpsr(MODO_USUARIO);
        self.jit = Some(jit);
        Ok(())
    }

    fn read_reg(&self, reg: Reg) -> u32 {
        self.jit().map_or(0, |jit| jit.get_reg(Self::indice(reg)))
    }

    fn write_reg(&mut self, reg: Reg, value: u32) {
        if let Ok(jit) = self.jit_mut() {
            jit.set_reg(Self::indice(reg), value);
        }
    }

    fn instructions(&self) -> u64 {
        self.jit().map_or(0, |jit| jit.instrucoes.get())
    }

    fn read_mem(&self, addr: u32, buf: &mut [u8]) -> Result<(), CpuError> {
        self.memoria
            .borrow()
            .read(addr, buf.len() as u32)
            .map(|bytes| buf.copy_from_slice(bytes))
            .map_err(|e| CpuError(e.to_string()))
    }

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> Result<(), CpuError> {
        self.memoria
            .borrow_mut()
            .write(addr, data)
            .map_err(|e| CpuError(e.to_string()))?;
        self.invalida_codigo_escrito(addr, data.len() as u32);
        Ok(())
    }

    fn run(&mut self, pc: u32, max_instructions: u64) -> Result<StopReason, CpuError> {
        let jit = self.jit_mut()?;
        jit.set_pc(pc);
        // `HaltExecution` deixa a razão armada para a volta que acabou de sair. O próximo
        // trecho começa depois de o despachante BREW ter escrito r0/pc, portanto precisa limpar
        // o bit genérico antes de entrar novamente no JIT.
        jit.clear_halt(HaltReason::UserDefined1);
        jit.parada.set(Parada::Nenhuma);
        jit.limite
            .set(jit.instrucoes.get().saturating_add(max_instructions));
        let _ = unsafe { jit.run() };
        // Não há invalidação para páginas de dados: só código previamente executado chega aqui.
        // É seguro mexer no cache depois de o JIT devolver o controle, nunca da callback.
        let paginas = std::mem::take(&mut *jit.codigo_sujo.borrow_mut());
        for pagina in paginas {
            jit.invalidate_cache_range(pagina * PAGE, PAGE as usize);
        }
        match jit.parada.get() {
            Parada::Nenhuma => Ok(StopReason::Budget),
            Parada::Fetch(addr) if addr == RETURN_MAGIC => Ok(StopReason::Returned),
            Parada::Fetch(addr)
                if (API_BASE..API_BASE.saturating_add(API_SIZE)).contains(&addr) =>
            {
                Ok(StopReason::ApiCall { addr })
            }
            Parada::Fetch(addr) => Ok(StopReason::MemoryFault { addr, pc: addr }),
            Parada::Memoria(addr) => Ok(StopReason::MemoryFault {
                addr,
                pc: jit.get_pc(),
            }),
            Parada::Excecao(pc) => Ok(StopReason::Exception { pc }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu_with(code: &[u8]) -> DynarmicCpu {
        let mut mem = GuestMemory::new();
        let mut bytes = code.to_vec();
        bytes.resize(0x1000, 0);
        // A imagem dos módulos do Zeebo é gravável: extensões podem montar pequenas rotinas ou
        // tabelas em RAM. Deixar o código do teste gravável permite conferir a invalidação JIT.
        mem.map("code", 0, bytes, true).unwrap();
        mem.map_zeroed("data", 0x1000, 0x1000).unwrap();
        mem.map_zeroed("stack", 0x2000_0000, 0x1000).unwrap();
        let mut cpu = DynarmicCpu::new().unwrap();
        cpu.reset(&mem).unwrap();
        cpu.write_reg(Reg::Sp, 0x2000_0f00);
        cpu
    }

    #[test]
    fn executa_instrucao_e_le_registrador() {
        let code = [0xe3a0_0037u32.to_le_bytes(), 0xeaff_fffeu32.to_le_bytes()].concat();
        let mut cpu = cpu_with(&code); // mov r0, #0x37 ; b .
        assert_eq!(cpu.run(0, 1).unwrap(), StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    #[test]
    fn salto_para_a_faixa_de_api_vira_chamada() {
        let code = [0xe3a0_020fu32.to_le_bytes(), 0xe12f_ff10u32.to_le_bytes()].concat();
        let mut cpu = cpu_with(&code);
        assert_eq!(
            cpu.run(0, 10).unwrap(),
            StopReason::ApiCall { addr: API_BASE }
        );
    }

    #[test]
    fn retorno_para_o_sentinela_e_reconhecido() {
        let mut cpu = cpu_with(&0xe12f_ff1eu32.to_le_bytes()); // bx lr
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
    }

    #[test]
    fn semihosting_escreve_sem_interromper_o_guest() {
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

    #[test]
    fn codigo_alterado_pelo_host_e_recompilado() {
        let mut cpu = cpu_with(
            &[0xe3a0_0001u32.to_le_bytes(), 0xe12f_ff1eu32.to_le_bytes()].concat(),
        );
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 1);

        // É o equivalente a uma API BREW atualizar uma tabela/rotina do módulo entre duas
        // chamadas. Sem `invalidate_cache_range`, o Dynarmic reutilizaria o `mov r0,#1`.
        cpu.write_mem(0, &0xe3a0_0002u32.to_le_bytes()).unwrap();
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
        assert_eq!(cpu.read_reg(Reg::R0), 2);
    }
}
