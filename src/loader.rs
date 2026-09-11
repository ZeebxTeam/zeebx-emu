//! Monta o ambiente de execução de um módulo BREW e chama `AEEMod_Load`.
//!
//! O linker script oficial (`elf2mod.x`) fixa `ro-base = 0x0`, então mapear a imagem no
//! endereço 0 do guest deixa todos os endereços absolutos corretos e dispensa relocação.
//!
//! Assinatura do ponto de entrada, de `sdk/inc/AEEModGen.h` do BREW SDK 4.0.2:
//!
//! ```c
//! int AEEMod_Load(IShell *ps, void *ph, IModule **pMod);
//! ```

use crate::brew::aee::{self, Interface};
use crate::cpu::CpuError;
use crate::cpu::mem::{GuestMemory, MemError};
use crate::modfile::ModImage;

/// Onde a imagem do módulo é carregada.
///
/// O linker script fixa `ro-base = 0`, mas o módulo **não** pode ficar no endereço 0: o stub
/// que o `elf2mod` coloca no início calcula a própria base por aritmética relativa ao PC e
/// depois lê duas palavras **antes** dela (`ldr r0, [r1, #-8]` e `[r1, #-4]`, com `r1` = base).
/// O carregador do AEE, portanto, reserva um prefixo antes da imagem. Como o stub é
/// position-independent, qualquer base serve — desde que exista memória válida logo abaixo.
pub const MODULE_BASE: u32 = 0x0001_0000;
/// Tamanho do prefixo reservado antes da imagem, para as palavras que o stub lê em offsets
/// negativos. Uma página é folga de sobra.
pub const MODULE_PREFIX: u32 = 0x1000;
/// Espaço zerado reservado depois da imagem, para a `.bss` do módulo.
///
/// O `.mod` guarda só as seções com conteúdo; a `.bss` existe em tempo de execução e não no
/// arquivo. Enquanto não soubermos ler o tamanho real dela do cabeçalho, reservamos uma folga
/// generosa — sobrar memória zerada é inofensivo.
pub const MODULE_BSS_SLACK: usize = 1024 * 1024;
/// Heap do módulo — onde `MALLOC` vai servir.
///
/// 64 MB porque o Quake mede a memória livre antes de carregar os `.pak` e desiste com
/// "Not enough free memory" se ela for pequena. O console tem 128 MB de RAM, e o tamanho aqui
/// é escolha nossa — só precisa ser folgado o bastante para o jogo reconhecer o aparelho.
pub const HEAP_BASE: u32 = 0x1000_0000;
pub const HEAP_SIZE: usize = 64 * 1024 * 1024;
/// Pilha. `sp` começa no topo porque a pilha do ARM cresce para baixo.
pub const STACK_BASE: u32 = 0x2000_0000;
pub const STACK_SIZE: usize = 1024 * 1024;
/// Objetos que o emulador expõe ao guest — cada um começa com o ponteiro de vtable.
///
/// Quatro megabytes dão 65.536 objetos, e o número não é capricho: com 64 KB eram **mil e vinte
/// e quatro**, e a Z-Wheel os esgotava numa execução. Depois disso tudo falha ao mesmo tempo —
/// `Couldn't open DB: 3`, `Unable to create vector model`, formulários com erro 3 — e nenhum
/// desses sintomas se parece com a causa.
///
/// Mil e vinte e quatro era limite **nosso**, não do console: lá os objetos saem do heap do
/// BREW, que tem dezenas de megabytes. A região é zerada e só ocupa o que for tocado, então a
/// folga não custa memória de verdade.
///
/// Isso **não** dispensa consertar quem vaza. Mas um teto de mil objetos transforma qualquer
/// vazamento pequeno numa falha em cascata, e falha em cascata esconde a causa.
pub const OBJECT_BASE: u32 = 0x3000_0000;
pub const OBJECT_SIZE: usize = 4 * 1024 * 1024;
/// Memória dos pixels das superfícies.
///
/// Precisa ficar no espaço do guest porque o `IDIB` entrega ao jogo o ponteiro do buffer para
/// ele desenhar direto — é assim que os jogos comerciais escrevem na tela.
pub const SURFACE_BASE: u32 = 0x4000_0000;
pub const SURFACE_SIZE: usize = 8 * 1024 * 1024;

/// Tabela de helpers da stdlib do BREW.
///
/// Módulos dinâmicos não linkam contra libc: eles chamam `MALLOC`, `STRLEN` e companhia por
/// uma tabela de ponteiros de função que o carregador entrega. O código gerado faz
/// `ldr r1, [r0, #<offset>]` seguido de `bx r1`, onde `r0` é o ponteiro dessa tabela — o mesmo
/// padrão aparece em `helloworld.mod` e em `tectoy.mod`, byte a byte.
///
/// Preenchemos cada entrada com um endereço-trampolim, então qualquer helper usado aparece
/// como `AEEHelpers::slot[n]` no log em vez de saltar para o vazio.
pub const HELPERS_BASE: u32 = 0x3100_0000;
/// Quantos ponteiros a tabela expõe. `struct AEEHelperFuncs` tem 117 campos; arredondamos
/// para cima para o caso de alguma build do Zeebo ter alguns a mais no fim.
const HELPERS_SLOTS: u32 = 256;

/// Trechos de código ARM que o emulador escreve para o guest executar.
///
/// Servem os helpers que devolvem sempre a mesma coisa. Atender um deles pelo trampolim custa
/// parar e religar o núcleo ARM, e o `GetAppInstance` sozinho responde por mais da metade das
/// chamadas de API do Quake — os jogos do BREW guardam os globais dentro do applet, então
/// cada acesso a um global passa por ele. Como código, a chamada nem sai da CPU.
pub const STUB_BASE: u32 = 0x3200_0000;
pub const STUB_SIZE: usize = 0x1000;

/// Vtables: memória de leitura cujos slots apontam para a faixa-trampolim.
pub const VTABLE_BASE: u32 = 0xe000_0000;
/// Versão da tabela de helpers, lida pelo módulo via `GET_HELPER_VER()`.
const HELPER_VERSION: u32 = 1;

/// Quantos slots reservamos por vtable. Nenhuma interface do BREW chega perto disso.
const SLOTS_PER_VTABLE: u32 = 256;
/// Interfaces cujos objetos são criados já na carga.
const BOOT_INTERFACES: [Interface; 2] = [Interface::Shell, Interface::Module];

/// Um módulo pronto para executar.
pub struct LoadedModule {
    pub mem: GuestMemory,
    /// Endereço de `AEEMod_Load` no espaço do guest.
    pub entry: u32,
    /// Ponteiro para o `IShell` que passamos em `r0`.
    pub shell: u32,
    /// Ponteiro de helpers que passamos em `r1`.
    pub helpers: u32,
    /// Onde `AEEMod_Load` deve gravar o `IModule*` que criar.
    pub out_module: u32,
}

#[derive(Debug)]
pub enum LoadError {
    Memory(MemError),
    Cpu(CpuError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Memory(e) => write!(f, "{e}"),
            Self::Cpu(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<MemError> for LoadError {
    fn from(e: MemError) -> Self {
        Self::Memory(e)
    }
}

impl From<CpuError> for LoadError {
    fn from(e: CpuError) -> Self {
        Self::Cpu(e)
    }
}

/// Monta o mapa de memória com o módulo, pilha, heap, vtables e objetos iniciais.
pub fn load(image: &ModImage) -> Result<LoadedModule, LoadError> {
    let mut mem = GuestMemory::new();
    // A região do módulo é gravável: a imagem carrega a `.data`, que o código altera. Ela
    // começa antes de `MODULE_BASE` para acomodar o prefixo lido pelo stub do elf2mod.
    let mut module_bytes = vec![0u8; MODULE_PREFIX as usize];
    module_bytes.extend_from_slice(image.image());
    module_bytes.resize(module_bytes.len() + MODULE_BSS_SLACK, 0);
    mem.map("module", MODULE_BASE - MODULE_PREFIX, module_bytes, true)?;
    mem.map_zeroed("heap", HEAP_BASE, HEAP_SIZE)?;
    mem.map_zeroed("stack", STACK_BASE, STACK_SIZE)?;
    mem.map_zeroed("objects", OBJECT_BASE, OBJECT_SIZE)?;
    // A tabela de helpers precisa ser gravável: o `GetAppInstance` troca de trampolim para
    // código assim que passa a ter uma resposta fixa.
    mem.map("helpers", HELPERS_BASE, build_helper_table(), true)?;
    mem.map_zeroed("stubs", STUB_BASE, STUB_SIZE)?;
    mem.map_zeroed("surfaces", SURFACE_BASE, SURFACE_SIZE)?;
    mem.map("vtables", VTABLE_BASE, build_vtables(), false)?;

    // Cada objeto ocupa uma palavra só por enquanto: o ponteiro para a vtable.
    let shell = OBJECT_BASE;
    mem.write_u32(shell, vtable_addr(Interface::Shell))?;

    // Espaço para o `IModule*` de saída, logo depois dos objetos iniciais.
    let out_module = OBJECT_BASE + 4 * BOOT_INTERFACES.len() as u32;
    mem.write_u32(out_module, 0)?;

    let helpers = HELPERS_BASE;
    // O `AEEStdLib.h` do SDK define exatamente estes dois campos antes da base do módulo:
    //   GET_HELPER()     = *((AEEHelperFuncs **)AEEMod_Load - 1)   -> base - 4
    //   GET_HELPER_VER() = *(uint32 *)(AEEMod_Load - 4 - 4)        -> base - 8
    mem.write_u32(MODULE_BASE - 8, HELPER_VERSION)?;
    mem.write_u32(MODULE_BASE - 4, helpers)?;

    Ok(LoadedModule {
        mem,
        entry: MODULE_BASE + image.entry(),
        shell,
        helpers,
        out_module,
    })
}

/// Gera a tabela de helpers: cada entrada é o trampolim do helper correspondente.
fn build_helper_table() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HELPERS_SLOTS as usize * 4);
    for slot in 0..HELPERS_SLOTS {
        bytes.extend_from_slice(&aee::encode(Interface::Helpers, slot).to_le_bytes());
    }
    bytes
}

/// Endereço da vtable de uma interface.
pub fn vtable_addr(iface: Interface) -> u32 {
    VTABLE_BASE + (iface as u32) * SLOTS_PER_VTABLE * 4
}

/// Gera todas as vtables: cada slot contém o endereço-trampolim do método correspondente.
fn build_vtables() -> Vec<u8> {
    let mut bytes = Vec::new();
    // Percorre a lista canônica, que é indexada pelo valor do enum — o mesmo índice que
    // `vtable_addr` usa para calcular o endereço.
    for iface in Interface::ALL {
        for slot in 0..SLOTS_PER_VTABLE {
            bytes.extend_from_slice(&aee::encode(iface, slot).to_le_bytes());
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Módulo sintético: `AEEMod_Load` chama o slot 3 de `IShell` a partir do ponteiro em r0.
    ///
    /// ```asm
    /// ldr r3, [r0]        ; r3 = vtable do shell
    /// ldr r3, [r3, #12]   ; r3 = slot 3
    /// bx  r3
    /// ```
    fn module_calling_shell_slot3() -> ModImage {
        let code = [
            0xe590_3000u32.to_le_bytes(),
            0xe593_300cu32.to_le_bytes(),
            0xe12f_ff13u32.to_le_bytes(),
        ]
        .concat();
        ModImage::parse(code).unwrap()
    }

    #[test]
    fn mapa_de_memoria_tem_as_regioes_esperadas() {
        let module = load(&module_calling_shell_slot3()).unwrap();
        let names: Vec<_> = module.mem.regions().iter().map(|r| r.name).collect();
        assert_eq!(
            names,
            [
                "module", "heap", "stack", "objects", "helpers", "stubs", "surfaces", "vtables"
            ]
        );
    }

    #[test]
    fn cada_interface_tem_a_propria_vtable_no_lugar_certo() {
        let module = load(&module_calling_shell_slot3()).unwrap();
        for iface in Interface::ALL {
            let addr = vtable_addr(iface);
            let first = module.mem.read_u32(addr).unwrap();
            assert_eq!(
                aee::decode(first),
                Some((iface, 0)),
                "a vtable de {} não aponta para os trampolins dela",
                iface.name()
            );
        }
    }

    #[test]
    fn shell_aponta_para_a_propria_vtable() {
        let module = load(&module_calling_shell_slot3()).unwrap();
        let vtable = module.mem.read_u32(module.shell).unwrap();
        assert_eq!(vtable, vtable_addr(Interface::Shell));
        // O slot 0 da vtable do shell é o trampolim do slot 0.
        assert_eq!(
            module.mem.read_u32(vtable).unwrap(),
            aee::encode(Interface::Shell, 0)
        );
    }
}
