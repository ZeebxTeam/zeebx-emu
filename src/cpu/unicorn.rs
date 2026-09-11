//! Backend de CPU sobre o `unicorn-engine` (QEMU/TCG), configurado como ARM1176 — o núcleo
//! do MSM7201A.
//!
//! O truque central do emulador está aqui: as vtables do BREW ficam numa faixa de endereços
//! que **não é mapeada**. Quando o jogo chama um método de interface, o salto cai nessa faixa,
//! o unicorn aborta a execução e nós lemos o PC para descobrir qual API foi chamada. O mesmo
//! vale para o retorno: `lr` recebe um endereço-sentinela igualmente não mapeado.

use unicorn_engine::{Arch, ArmCpuModel, HookType, Mode, Prot, RegisterARM, Unicorn, uc_error};

use super::{CpuBackend, CpuError, Reg, StopReason};
use crate::cpu::mem::GuestMemory;

/// Base da faixa reservada às vtables do BREW. Nunca é mapeada.
pub const API_BASE: u32 = 0xf000_0000;
/// Tamanho da faixa reservada às vtables.
pub const API_SIZE: u32 = 0x0100_0000;
/// Endereço-sentinela colocado em `lr`: chegar aqui significa que o módulo retornou.
pub const RETURN_MAGIC: u32 = 0xfff0_0000;

/// Alinhamento exigido pelo `mem_map` do unicorn.
const PAGE: u64 = 0x1000;

/// Teto para a string de um `SYS_WRITE0`, para um ponteiro ruim não virar leitura infinita.
const MAX_SEMIHOSTING_STRING: usize = 4096;

/// Estado que os hooks precisam compartilhar com o resto do backend.
#[derive(Debug, Default, Clone, Copy)]
struct HookState {
    /// Endereço de dado que causou a última falha de acesso — diferente do PC.
    last_fault: Option<u32>,
    /// O PC no instante da falha, lido dentro do hook.
    ///
    /// Depois que o `emu_start` volta, o PC já não é confiável: ele pode ter ficado na
    /// instrução anterior ou avançado. Dentro do hook ele é a instrução que faltou — e é a
    /// diferença entre "alguma coisa era nula" e "esta instrução leu este ponteiro nulo".
    last_fault_pc: Option<u32>,
}

/// Registro de uma escrita observada por um watchpoint.
#[derive(Debug, Clone, Copy)]
pub struct Write {
    pub addr: u32,
    pub value: i64,
    /// PC de origem, ou zero quando quem escreveu foi o próprio emulador (implementação de
    /// API), que não passa pelos hooks do unicorn.
    pub pc: u32,
    /// `lr` no momento da escrita: quando o PC cai numa função utilitária compartilhada — um
    /// `operator=`, um `memcpy` —, é o `lr` que diz quem pediu.
    pub lr: u32,
}

pub struct UnicornCpu {
    uc: Unicorn<'static, HookState>,
    /// Escritas capturadas pelo watchpoint, quando há um armado.
    writes: std::rc::Rc<std::cell::RefCell<Vec<Write>>>,
    /// Faixa vigiada, para reconhecer também as escritas feitas pelo host.
    watched: Option<(u32, u32)>,
    /// Instruções executadas dentro da faixa rastreada, com o `r0` de cada uma.
    steps: std::rc::Rc<std::cell::RefCell<Vec<(u32, u32, u32)>>>,
    /// Instruções executadas desde o início. Contadas por bloco de tradução, que é ordens de
    /// grandeza mais barato que um hook por instrução e dá o mesmo número: no ARM todas as
    /// instruções têm quatro bytes.
    instructions: std::rc::Rc<std::cell::Cell<u64>>,
    /// Valor de `instructions` em que a fatia atual precisa parar. O mesmo hook que conta é
    /// quem cobra o teto — ver [`UnicornCpu::run`].
    deadline: std::rc::Rc<std::cell::Cell<u64>>,
    /// Se o perfilador está ligado. Fica separado do histograma para que o hook de bloco pague
    /// só a leitura de um `bool` quando ele está desligado, que é o caso normal.
    profiling: std::rc::Rc<std::cell::Cell<bool>>,
    /// Quantas instruções foram executadas em cada bloco de tradução, pelo endereço de entrada.
    profile: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<u32, u64>>>,
    /// O que o guest escreveu por semihosting do ARM.
    semihosting: std::rc::Rc<std::cell::RefCell<String>>,
    /// Instante de relógio real em que a execução tem de parar, venha o que vier.
    wall: std::rc::Rc<std::cell::Cell<Option<std::time::Instant>>>,
    /// Se o teto de tempo real já venceu. Quem chama precisa saber para não recomeçar.
    expired: std::rc::Rc<std::cell::Cell<bool>>,
}

impl UnicornCpu {
    pub fn new() -> Result<Self, CpuError> {
        let mut uc =
            Unicorn::new_with_data(Arch::ARM, Mode::ARM, HookState::default()).map_err(uc_err)?;
        uc.ctl_set_cpu_model(ArmCpuModel::Model_1176 as i32)
            .map_err(uc_err)?;
        // Guarda o endereço acessado quando o guest toca memória fora do mapa. Sem isso só
        // teríamos o PC, que diz onde está a instrução, não o que ela tentou acessar.
        uc.add_mem_hook(
            HookType::MEM_UNMAPPED,
            0,
            u64::MAX,
            |uc, _type, address, _size, _value| {
                let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                let estado = uc.get_data_mut();
                estado.last_fault = Some(address as u32);
                estado.last_fault_pc = Some(pc);
                // `false` mantém a falha: queremos que a execução pare.
                false
            },
        )
        .map_err(uc_err)?;
        // Semihosting do ARM: `SVC #0xAB` com a operação em `r0`. Os jogos da PopCap — o
        // Peggle e o Zuma's Revenge — emitem log assim, com a mesma sequência de instruções nos
        // dois. Sem tratar a interrupção, o unicorn levanta exceção e o emulador para antes de
        // o jogo terminar de inicializar.
        //
        // Só as operações de escrita interessam, que são as que um jogo usa: `SYS_WRITEC`
        // escreve um caractere e `SYS_WRITE0` uma string terminada em zero. O resto é
        // reconhecido e ignorado — devolver zero é "deu certo" para a maioria delas, e parar
        // seria pior do que seguir.
        let semihosting = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let saida = semihosting.clone();
        uc.add_intr_hook(move |uc, numero| {
            // No ARM do unicorn a interrupção de `SVC` é a de número 2.
            if numero != 2 {
                return;
            }
            let op = uc.reg_read(RegisterARM::R0).unwrap_or(0);
            let arg = uc.reg_read(RegisterARM::R1).unwrap_or(0) as u32;
            match op {
                // SYS_WRITEC: `r1` aponta para o caractere.
                0x03 => {
                    let mut byte = [0u8; 1];
                    if uc.mem_read(arg as u64, &mut byte).is_ok() {
                        saida.borrow_mut().push(byte[0] as char);
                    }
                }
                // SYS_WRITE0: `r1` aponta para a string.
                0x04 => {
                    for offset in 0..MAX_SEMIHOSTING_STRING {
                        let mut byte = [0u8; 1];
                        if uc.mem_read(arg as u64 + offset as u64, &mut byte).is_err()
                            || byte[0] == 0
                        {
                            break;
                        }
                        saida.borrow_mut().push(byte[0] as char);
                    }
                }
                _ => {}
            }
            let _ = uc.reg_write(RegisterARM::R0, 0);
        })
        .map_err(uc_err)?;

        let instructions = std::rc::Rc::new(std::cell::Cell::new(0u64));
        let deadline = std::rc::Rc::new(std::cell::Cell::new(u64::MAX));
        let profiling = std::rc::Rc::new(std::cell::Cell::new(false));
        let profile: std::rc::Rc<std::cell::RefCell<std::collections::HashMap<u32, u64>>> =
            Default::default();
        let counter = instructions.clone();
        let limit = deadline.clone();
        let (ligado, histograma) = (profiling.clone(), profile.clone());
        let wall = std::rc::Rc::new(std::cell::Cell::new(None));
        let expired = std::rc::Rc::new(std::cell::Cell::new(false));
        let (prazo, vencido) = (wall.clone(), expired.clone());
        uc.add_block_hook(0, u64::MAX, move |uc, address, size| {
            if ligado.get() {
                *histograma.borrow_mut().entry(address as u32).or_insert(0) +=
                    (size / 4).max(1) as u64;
            }
            // O teto é cobrado aqui, e não pelo `count` do `emu_start`: aquele instala um
            // `UC_HOOK_CODE`, um callback por instrução, e o guest roda a um terço da
            // velocidade. Este hook já existia para contar, e uma volta por bloco de tradução
            // é precisão de sobra para uma rede de segurança contra laço infinito.
            //
            // A conferência vem antes de somar porque o hook roda *antes* do bloco: contar
            // primeiro pararia sem executar nada quando o orçamento couber num bloco só.
            if counter.get() >= limit.get() {
                let _ = uc.emu_stop();
                return;
            }
            counter.set(counter.get() + (size / 4).max(1) as u64);
            // O teto de tempo real precisa valer também **dentro** de uma fatia: um jogo pode
            // passar minutos sem chamar API nenhuma, e é justamente esse que se quer perfilar.
            // Ler o relógio a cada bloco seria caro, e um bloco a mais ou a menos não muda
            // nada num teto que existe para depuração.
            if let Some(limite) = prazo.get() {
                const PERIODO: u64 = 8192;
                if counter.get() % PERIODO < (size / 4).max(1) as u64
                    && std::time::Instant::now() >= limite
                {
                    vencido.set(true);
                    let _ = uc.emu_stop();
                }
            }
        })
        .map_err(uc_err)?;
        Ok(Self {
            uc,
            writes: Default::default(),
            watched: None,
            steps: Default::default(),
            instructions,
            deadline,
            semihosting,
            profiling,
            profile,
            wall,
            expired,
        })
    }

    /// Arma um watchpoint de escrita numa faixa de endereços.
    ///
    /// É a ferramenta para responder "quem deveria ter preenchido este campo?": quando o jogo
    /// quebra num ponteiro nulo, o watchpoint diz se alguém chegou a escrever ali — e de qual
    /// instrução partiu a escrita.
    pub fn watch(&mut self, base: u32, len: u32) -> Result<(), CpuError> {
        self.watched = Some((base, base + len));
        // Leituras também: "quem escreveu aqui" e "quem leu daqui" são a mesma pergunta vista
        // de dois lados, e há casos em que só a segunda tem resposta.
        let reads = self.writes.clone();
        self.uc
            .add_mem_hook(
                HookType::MEM_READ,
                base as u64,
                (base + len) as u64,
                move |uc, _type, address, size, _value| {
                    let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                    reads.borrow_mut().push(Write {
                        addr: address as u32,
                        value: -(size as i64),
                        pc,
                        lr: uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32,
                    });
                    true
                },
            )
            .map_err(uc_err)?;
        let writes = self.writes.clone();
        self.uc
            .add_mem_hook(
                HookType::MEM_WRITE,
                base as u64,
                (base + len) as u64,
                move |uc, _type, address, _size, value| {
                    let pc = uc.reg_read(RegisterARM::PC).unwrap_or(0) as u32;
                    let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                    writes.borrow_mut().push(Write {
                        addr: address as u32,
                        value,
                        pc,
                        lr,
                    });
                    true
                },
            )
            .map_err(uc_err)?;
        Ok(())
    }

    /// Registra cada instrução executada dentro de uma faixa de endereços.
    ///
    /// A desmontagem estática mostra todos os caminhos possíveis; isto mostra o que de fato
    /// aconteceu. É a diferença entre ler o mapa e seguir a trilha.
    ///
    /// O que fica são as **últimas** `limit` instruções, porque a pergunta quase sempre é "como
    /// ele chegou aqui", e não "por onde ele começou".
    pub fn trace_code(&mut self, begin: u32, end: u32, limit: usize) -> Result<(), CpuError> {
        let steps = self.steps.clone();
        self.uc
            .add_code_hook(begin as u64, end as u64, move |uc, address, _size| {
                // `r0` e `lr` juntos: um responde "o que essa chamada devolveu", o outro
                // "quem chamou" — na entrada de uma função, `lr` é o endereço de retorno do
                // chamador.
                let r0 = uc.reg_read(RegisterARM::R0).unwrap_or(0) as u32;
                let lr = uc.reg_read(RegisterARM::LR).unwrap_or(0) as u32;
                let mut steps = steps.borrow_mut();
                // Guarda as **últimas** instruções, não as primeiras. Quem olha um rastro de
                // execução está quase sempre investigando como o jogo chegou onde parou, e o
                // começo de um laço que roda um milhão de vezes não responde isso. Guardar o
                // começo já me fez ler "última instrução executada" onde era só "última que
                // coube", e perseguir a instrução errada por três execuções.
                if steps.len() == limit {
                    steps.remove(0);
                }
                steps.push((address as u32, r0, lr));
            })
            .map_err(uc_err)?;
        Ok(())
    }

    /// Liga o perfilador: a partir daqui, cada bloco de tradução executado soma as instruções
    /// dele no endereço em que começa.
    ///
    /// A conta é por bloco, não por instrução — o mesmo motivo pelo qual o orçamento é cobrado
    /// ali: um hook por instrução faz o guest rodar a um terço da velocidade, e um perfil que
    /// muda o que está sendo medido não serve para nada. Como um bloco só termina em desvio, o
    /// endereço de entrada dele já aponta o laço.
    pub fn enable_profile(&mut self) {
        self.profiling.set(true);
    }

    /// Interrompe a execução depois de `limite` de tempo **real**, esteja o guest onde estiver.
    ///
    /// É ferramenta de depuração: existe para conseguir um perfil de um jogo que não termina.
    pub fn set_wall_limit(&mut self, limite: std::time::Duration) {
        self.wall.set(Some(std::time::Instant::now() + limite));
    }

    /// Se o teto de tempo real venceu.
    pub fn wall_expired(&self) -> bool {
        self.expired.get()
    }

    /// O que o guest escreveu por semihosting do ARM, se escreveu algo.
    pub fn semihosting(&self) -> String {
        self.semihosting.borrow().clone()
    }

    /// O perfil acumulado, do mais quente para o mais frio.
    pub fn profile(&self) -> Vec<(u32, u64)> {
        let mut linhas: Vec<(u32, u64)> = self
            .profile
            .borrow()
            .iter()
            .map(|(&addr, &n)| (addr, n))
            .collect();
        // Desempate pelo endereço: duas execuções iguais precisam imprimir a mesma lista.
        linhas.sort_by_key(|&(addr, n)| (std::cmp::Reverse(n), addr));
        linhas
    }

    /// As instruções registradas por [`UnicornCpu::trace_code`].
    pub fn steps(&self) -> Vec<(u32, u32, u32)> {
        self.steps.borrow().clone()
    }

    /// Escritas capturadas desde o início.
    pub fn writes(&self) -> Vec<Write> {
        self.writes.borrow().clone()
    }

    fn reg_id(reg: Reg) -> RegisterARM {
        match reg {
            Reg::R0 => RegisterARM::R0,
            Reg::R1 => RegisterARM::R1,
            Reg::R2 => RegisterARM::R2,
            Reg::R3 => RegisterARM::R3,
            Reg::R4 => RegisterARM::R4,
            Reg::R5 => RegisterARM::R5,
            Reg::R6 => RegisterARM::R6,
            Reg::R7 => RegisterARM::R7,
            Reg::R8 => RegisterARM::R8,
            Reg::R9 => RegisterARM::R9,
            Reg::R10 => RegisterARM::R10,
            Reg::R11 => RegisterARM::R11,
            Reg::R12 => RegisterARM::R12,
            Reg::Sp => RegisterARM::SP,
            Reg::Lr => RegisterARM::LR,
            Reg::Pc => RegisterARM::PC,
        }
    }
}

impl CpuBackend for UnicornCpu {
    fn reset(&mut self, mem: &GuestMemory) -> Result<(), CpuError> {
        for region in mem.regions() {
            let base = region.base as u64;
            let len = region.bytes.len() as u64;
            // O unicorn só mapeia em múltiplos de página; arredondamos para cima.
            let size = len.div_ceil(PAGE) * PAGE;
            let prot = if region.writable {
                Prot::ALL
            } else {
                Prot::READ | Prot::EXEC
            };
            self.uc.mem_map(base, size, prot).map_err(uc_err)?;
            if !region.bytes.is_empty() {
                self.uc.mem_write(base, &region.bytes).map_err(uc_err)?;
            }
        }
        Ok(())
    }

    fn read_reg(&self, reg: Reg) -> u32 {
        self.uc.reg_read(Self::reg_id(reg)).unwrap_or(0) as u32
    }

    fn write_reg(&mut self, reg: Reg, value: u32) {
        let _ = self.uc.reg_write(Self::reg_id(reg), value as u64);
    }

    fn instructions(&self) -> u64 {
        self.instructions.get()
    }

    fn read_mem(&self, addr: u32, buf: &mut [u8]) -> Result<(), CpuError> {
        // Como nas escritas, as leituras do host não passam pelos hooks — e um `memmove` da
        // nossa stdlib é exatamente a leitura que interessa saber se aconteceu.
        if let Some((base, end)) = self.watched {
            if addr < end && addr + buf.len() as u32 > base {
                self.writes.borrow_mut().push(Write {
                    addr,
                    value: -(buf.len() as i64),
                    pc: 0,
                    lr: 0,
                });
            }
        }
        self.uc.mem_read(addr as u64, buf).map_err(uc_err)
    }

    fn write_mem(&mut self, addr: u32, data: &[u8]) -> Result<(), CpuError> {
        // As escritas do host não passam pelos hooks do unicorn, então o watchpoint precisa
        // vê-las por aqui — senão uma API nossa pode sobrescrever memória do jogo sem deixar
        // rastro nenhum.
        if let Some((base, end)) = self.watched {
            let touched = addr < end && addr + data.len() as u32 > base;
            if touched {
                self.writes.borrow_mut().push(Write {
                    addr,
                    value: data.len() as i64,
                    pc: 0,
                    lr: 0,
                });
            }
        }
        self.uc.mem_write(addr as u64, data).map_err(uc_err)
    }

    fn run(&mut self, pc: u32, max_instructions: u64) -> Result<StopReason, CpuError> {
        let estado = self.uc.get_data_mut();
        estado.last_fault = None;
        estado.last_fault_pc = None;
        self.deadline
            .set(self.instructions.get().saturating_add(max_instructions));
        // `until` fica num endereço inalcançável de propósito: quem termina a execução é
        // sempre uma parada anômala — salto para a faixa de API, retorno para o sentinela,
        // falha de memória ou fim do orçamento de instruções. O `count` fica em zero porque
        // quem cobra o orçamento é o hook de bloco; pedir a contagem ao unicorn custaria um
        // callback por instrução.
        let result = self.uc.emu_start(pc as u64, u64::MAX, 0, 0);
        let stopped_at = self.read_reg(Reg::Pc);

        match result {
            // Sem erro significa que alguém chamou `emu_stop`, e o único que chama é o hook
            // de bloco ao ver o orçamento estourar.
            Ok(()) => Ok(StopReason::Budget),
            Err(uc_error::FETCH_UNMAPPED) => {
                // O endereço buscado é o que interessa; o PC pode ter ficado na instrução anterior.
                let target = self.uc.get_data().last_fault.unwrap_or(stopped_at);
                Ok(classify_fetch(target))
            }
            Err(uc_error::READ_UNMAPPED) | Err(uc_error::WRITE_UNMAPPED) => {
                Ok(StopReason::MemoryFault {
                    addr: self.uc.get_data().last_fault.unwrap_or(stopped_at),
                    pc: self.uc.get_data().last_fault_pc.unwrap_or(stopped_at),
                })
            }
            // Instrução inválida é o mesmo tipo de desfecho de uma exceção: diz onde o guest
            // se perdeu, e derrubar o emulador por isso esconde justamente essa informação.
            Err(uc_error::EXCEPTION) | Err(uc_error::INSN_INVALID) => {
                Ok(StopReason::Exception { pc: stopped_at })
            }
            Err(err) => Err(uc_err(err)),
        }
    }
}

/// Decide o que significa um fetch em endereço não mapeado.
fn classify_fetch(addr: u32) -> StopReason {
    if addr == RETURN_MAGIC {
        StopReason::Returned
    } else if (API_BASE..API_BASE.saturating_add(API_SIZE)).contains(&addr) {
        StopReason::ApiCall { addr }
    } else {
        StopReason::MemoryFault { addr, pc: addr }
    }
}

fn uc_err(err: uc_error) -> CpuError {
    CpuError(format!("{err:?}"))
}

#[cfg(test)]
mod tests_support {
    use super::*;

    /// Monta uma CPU com uma única região de código já escrita.
    pub fn cpu_with(code: &[u8]) -> UnicornCpu {
        let mut mem = GuestMemory::new();
        let mut bytes = code.to_vec();
        bytes.resize(0x1000, 0);
        mem.map("code", 0, bytes, false).unwrap();
        mem.map_zeroed("stack", 0x2000_0000, 0x1000).unwrap();
        let mut cpu = UnicornCpu::new().unwrap();
        cpu.reset(&mem).unwrap();
        cpu.write_reg(Reg::Sp, 0x2000_0f00);
        cpu
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::cpu_with;
    use super::*;

    #[test]
    fn executa_instrucao_e_le_registrador() {
        // mov r0, #0x37
        let mut cpu = cpu_with(&0xe3a0_0037u32.to_le_bytes());
        let stop = cpu.run(0, 1).unwrap();
        assert_eq!(stop, StopReason::Budget);
        assert_eq!(cpu.read_reg(Reg::R0), 0x37);
    }

    #[test]
    fn salto_para_a_faixa_de_api_vira_chamada() {
        // mov r0, #0xf0000000 (ror) ; bx r0   -> salta para API_BASE
        let code = [0xe3a0_020fu32.to_le_bytes(), 0xe12f_ff10u32.to_le_bytes()].concat();
        let mut cpu = cpu_with(&code);
        let stop = cpu.run(0, 10).unwrap();
        assert_eq!(stop, StopReason::ApiCall { addr: API_BASE });
    }

    #[test]
    fn retorno_para_o_sentinela_e_reconhecido() {
        // bx lr, com lr = RETURN_MAGIC
        let mut cpu = cpu_with(&0xe12f_ff1eu32.to_le_bytes());
        cpu.write_reg(Reg::Lr, RETURN_MAGIC);
        assert_eq!(cpu.run(0, 10).unwrap(), StopReason::Returned);
    }
}

/// Medida de vazão do núcleo, para separar "o jogo faz muita conta" de "o emulador está lento".
///
/// Fica fora dos testes normais (`#[ignore]`) porque mede tempo, e tempo não é resultado
/// estável de teste. Roda com:
///
/// ```text
/// cargo test --release cpu::unicorn::speed -- --ignored --nocapture
/// ```
#[cfg(test)]
mod speed {
    use super::tests_support::cpu_with;
    use super::*;

    #[test]
    #[ignore]
    fn entradas_por_segundo() {
        // Quanto custa **entrar** no guest. Um jogo que chama a API a cada pixel paga este
        // preço milhões de vezes, e aí ele é que manda, não a vazão de instruções.
        let code = [
            0xe3a0_020fu32.to_le_bytes(), // mov r0, #0xf0000000
            0xe12f_ff10u32.to_le_bytes(), // bx r0  -> salto para a faixa de API
        ]
        .concat();
        let mut cpu = cpu_with(&code);
        let rounds = 20_000;
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            cpu.run(0, 1_000_000).unwrap();
        }
        let elapsed = start.elapsed();
        println!(
            "{rounds} entradas em {:.2?} = {:.1} µs cada",
            elapsed,
            elapsed.as_secs_f64() * 1e6 / rounds as f64
        );
    }

    #[test]
    #[ignore]
    fn acesso_a_registrador() {
        // Quanto custa ler e escrever registrador pela FFI do unicorn. Uma chamada de API faz
        // meia dúzia disso, e se cada uma custar microssegundo é ela que manda no despacho.
        let mut cpu = cpu_with(&[0u8; 8]);
        let rounds = 200_000;
        let start = std::time::Instant::now();
        let mut soma = 0u64;
        for _ in 0..rounds {
            soma += cpu.read_reg(Reg::R1) as u64;
        }
        let leitura = start.elapsed();
        let start = std::time::Instant::now();
        for i in 0..rounds {
            cpu.write_reg(Reg::R1, i);
        }
        let escrita = start.elapsed();
        println!(
            "leitura {:.0} ns cada, escrita {:.0} ns cada (soma={soma})",
            leitura.as_secs_f64() * 1e9 / rounds as f64,
            escrita.as_secs_f64() * 1e9 / rounds as f64
        );
    }

    #[test]
    #[ignore]
    fn com_e_sem_o_teto_de_instrucoes() {
        // O `uc_emu_start` com contagem instala um hook por instrução. Este teste mede o preço
        // dele: mesmo programa, mesmo caminho, só muda o teto.
        let code = [
            0xe3a0_0401u32.to_le_bytes(), // mov  r0, #0x01000000
            0xe250_0001u32.to_le_bytes(), // subs r0, r0, #1
            0x1aff_fffdu32.to_le_bytes(), // bne  -3
            0xea00_003bu32.to_le_bytes(), // b    0x100
        ]
        .concat();
        for count in [0usize, 100_000_000] {
            let mut cpu = cpu_with(&code);
            let start = std::time::Instant::now();
            cpu.uc.emu_start(0, 0x100, 0, count).unwrap();
            let elapsed = start.elapsed();
            let instructions = cpu.instructions() as f64;
            println!(
                "teto {count:>10}: {:.2?} = {:.1} M/s",
                elapsed,
                instructions / elapsed.as_secs_f64() / 1e6
            );
        }
    }

    #[test]
    #[ignore]
    fn instrucoes_por_segundo() {
        // Laço apertado: `subs r0,r0,#1` e `bne` de volta. Só CPU, sem tocar memória.
        let code = [
            0xe3a0_00ffu32.to_le_bytes(), // mov r0, #255
            0xe250_0001u32.to_le_bytes(), // subs r0, r0, #1
            0x1aff_fffdu32.to_le_bytes(), // bne -3
            0xeaff_fffbu32.to_le_bytes(), // b   -5 (recomeça)
        ]
        .concat();
        let mut cpu = cpu_with(&code);
        let budget = 20_000_000u64;
        let start = std::time::Instant::now();
        cpu.run(0, budget).unwrap();
        let elapsed = start.elapsed();
        println!(
            "{} instruções em {:.2?} = {:.1} M/s",
            cpu.instructions(),
            elapsed,
            cpu.instructions() as f64 / elapsed.as_secs_f64() / 1e6
        );
    }
}
