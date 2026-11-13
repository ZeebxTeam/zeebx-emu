//! Sets up the execution environment for a BREW module and calls `AEEMod_Load`.
//!
//! The official linker script (`elf2mod.x`) sets `ro-base = 0x0`, so mapping the image at guest
//! address 0 leaves every absolute address correct and makes relocation unnecessary.
//!
//! Entry point signature, from `sdk/inc/AEEModGen.h` in the BREW SDK 4.0.2:
//!
//! ```c
//! int AEEMod_Load(IShell *ps, void *ph, IModule **pMod);
//! ```

pub mod archive;
pub mod seven_z;
// Os formatos BREW (`.mod`, `.mif`, `.bar`) vêm do `zeebx-core`; reexportados para manter os caminhos.
pub use zeebx_core::formats::{miffile, modfile, resfile};

use crate::brew::aee::Interface;
use crate::cpu::CpuError;
use crate::cpu::mem::{GuestMemory, MemError};
use crate::loader::modfile::ModImage;

// O mapa de memória do guest e as tabelas de vtables/helpers são o contrato do AEE, e moram no
// `zeebx-core`. Aqui fica só quem mapeia de fato e chama `AEEMod_Load`.
pub use zeebx_core::layout::*;

/// The size of the guest heap. The core only fixes where it starts; the size is our choice.
///
/// Defined here, it takes precedence over the deprecated `HEAP_SIZE` the glob above brings in.
// O Vita reserva de fato o `Vec` zerado (newlib não tem overcommit): com 64 MiB o vitaGL não
// achava o bloco temporário do frame seguinte e o Zeetris morria em `sceClibMemcpy`.
pub const HEAP_SIZE: usize = if cfg!(target_os = "vita") {
    48 * 1024 * 1024
} else {
    zeebx_core::layout::DEFAULT_HEAP_SIZE
};

/// Zeebo `bgdi` newlib grows through `_sbrk` from here, just after the BREW heap.
///
/// It stays out of the heap: the Z-Wheel measures free RAM with `GetRAMFree`, and a larger
/// heap makes it ask for more than fits. `bgdi` uses `0x14000000..0x18000000`. Without this
/// map the first `malloc` after the title screen writes to an unmapped address.
pub const SBRK_BASE: u32 = 0x1400_0000;
pub const SBRK_SIZE: usize = if cfg!(target_os = "vita") || cfg!(zeebx_switch) {
    0
} else {
    64 * 1024 * 1024
};

/// An extension module to map together with the main one.
pub struct ExtensionImage {
    pub image: ModImage,
    /// The classes its `.mif` declares it provides.
    pub classes: Vec<u32>,
}

/// An extension module already mapped, waiting for the first request.
pub struct LoadedExtension {
    /// Address of its `AEEMod_Load`.
    pub entry: u32,
    /// Where it must write the `IModule*` it creates.
    pub out_module: u32,
    pub classes: Vec<u32>,
}

/// A module ready to run.
pub struct LoadedModule {
    pub mem: GuestMemory,
    /// Address of `AEEMod_Load` in the guest's space.
    pub entry: u32,
    /// Pointer to the `IShell` we pass in `r0`.
    pub shell: u32,
    /// Helpers pointer we pass in `r1`.
    pub helpers: u32,
    /// Where `AEEMod_Load` must write the `IModule*` it creates.
    pub out_module: u32,
    /// The package's extension modules, already mapped and not yet loaded.
    pub extensions: Vec<LoadedExtension>,
}

impl LoadedModule {
    /// Where the object area can start without stepping on the pointers the loader reserved.
    ///
    /// These are the two words of the main module — the `IModule*` and the applet output — plus
    /// two per extension. Computing this here, and not in the caller, is what avoids the silent
    /// collision: the `ObjectStore` used to start right after the first two and run over the
    /// others.
    pub fn objects_reserved(&self) -> u32 {
        self.out_module + 8 + 8 * self.extensions.len() as u32 - OBJECT_BASE
    }
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

/// Builds the memory map with the module, stack, heap, vtables and initial objects.
///
/// This is the case for whoever has no extension module at all — the tests and whoever loads a
/// loose `.mod`, without the package around it.
#[cfg(test)]
pub fn load(image: &ModImage) -> Result<LoadedModule, LoadError> {
    load_with(image, &[])
}

/// The same, also mapping the package's extension modules.
pub fn load_with(
    image: &ModImage,
    loaded_extensions: &[ExtensionImage],
) -> Result<LoadedModule, LoadError> {
    let mut mem = GuestMemory::new();
    // The module region is writable: the image carries the `.data`, which the code modifies. It
    // starts before `MODULE_BASE` to accommodate the prefix read by the elf2mod stub.
    let mut module_bytes = vec![0u8; MODULE_PREFIX as usize];
    module_bytes.extend_from_slice(image.image());
    module_bytes.resize(module_bytes.len() + MODULE_BSS_SLACK, 0);
    mem.map("module", MODULE_BASE - MODULE_PREFIX, module_bytes, true)?;
    // **The null page is readable, and reads as zero.** The console has no memory protection,
    // and whatever lives at the low addresses is readable: a game that reads through a null
    // pointer gets some value and moves on. Aviãozinho, a fan-made Quake port, does this when
    // loading the first level: the map has a missing texture, the engine's fallback texture is
    // never created, and its name is read from address zero. With execution stopping there, the
    // game did not get past the menu. Writing is still an error, and so is executing: a jump to
    // address zero is a defect the report needs to show, not code to run through.
    mem.map_executable(
        "nulo",
        0,
        vec![0u8; (MODULE_BASE - MODULE_PREFIX) as usize],
        false,
        false,
    )?;
    mem.map_zeroed("heap", HEAP_BASE, HEAP_SIZE)?;
    if SBRK_SIZE > 0 {
        mem.map_zeroed("sbrk", SBRK_BASE, SBRK_SIZE)?;
    }
    mem.map_zeroed("stack", STACK_BASE, STACK_SIZE)?;
    mem.map_zeroed("objects", OBJECT_BASE, OBJECT_SIZE)?;
    // The helper table must be writable: `GetAppInstance` switches from trampoline to code as
    // soon as it has a fixed answer.
    mem.map("helpers", HELPERS_BASE, build_helper_table(), true)?;
    mem.map_zeroed("stubs", STUB_BASE, STUB_SIZE)?;
    mem.map_zeroed("surfaces", SURFACE_BASE, SURFACE_SIZE)?;
    mem.map_zeroed("internal", INTERNAL_BASE, INTERNAL_SIZE)?;
    mem.map("vtables", VTABLE_BASE, build_vtables(), false)?;

    // For now each object takes up just one word: the pointer to the vtable.
    let shell = OBJECT_BASE;
    mem.write_u32(shell, vtable_addr(Interface::Shell))?;

    // Space for the output `IModule*`, right after the initial objects.
    let out_module = OBJECT_BASE + 4 * BOOT_INTERFACES.len() as u32;
    mem.write_u32(out_module, 0)?;

    let helpers = HELPERS_BASE;
    // The SDK's `AEEStdLib.h` defines exactly these two fields before the module base:
    //   GET_HELPER()     = *((AEEHelperFuncs **)AEEMod_Load - 1)   -> base - 4
    //   GET_HELPER_VER() = *(uint32 *)(AEEMod_Load - 4 - 4)        -> base - 8
    mem.write_u32(MODULE_BASE - 8, HELPER_VERSION)?;
    mem.write_u32(MODULE_BASE - 4, helpers)?;

    // Each extension gets its own region and two output words in the object area, right after
    // those of the main module.
    let mut extensions = Vec::new();
    for (i, extension) in loaded_extensions.iter().take(MAX_EXTENSIONS).enumerate() {
        let base = EXT_MODULE_BASE + i as u32 * EXT_MODULE_SPACING;
        let mut bytes = vec![0u8; MODULE_PREFIX as usize];
        bytes.extend_from_slice(extension.image.image());
        bytes.resize(bytes.len() + MODULE_BSS_SLACK, 0);
        mem.map(
            EXT_REGION_NAMES[i],
            base - MODULE_PREFIX,
            bytes,
            true,
        )?;
        // The same two words that `AEEStdLib.h` reads before the module base. Without them the
        // extension finds the helper table at zero and dies on the first `MALLOC` call.
        mem.write_u32(base - 8, HELPER_VERSION)?;
        mem.write_u32(base - 4, helpers)?;
        // Two words per extension: its `IModule*` and the `CreateInstance` output. They start
        // after the two of the main module — `out_module` and the applet's.
        let out_module = OBJECT_BASE + 4 * (BOOT_INTERFACES.len() as u32 + 2) + 8 * i as u32;
        mem.write_u32(out_module, 0)?;
        mem.write_u32(out_module + 4, 0)?;
        extensions.push(LoadedExtension {
            entry: base + extension.image.entry(),
            out_module,
            classes: extension.classes.clone(),
        });
    }

    Ok(LoadedModule {
        mem,
        entry: MODULE_BASE + image.entry(),
        shell,
        helpers,
        out_module,
        extensions,
    })
}

/// Names of the extension regions. `GuestMemory::map` asks for a name, and one name per index
/// keeps the map printed by `run` readable — `ext0`, `ext1`, and not `ext` eight times.
const EXT_REGION_NAMES: [&str; MAX_EXTENSIONS] = [
    "ext0", "ext1", "ext2", "ext3", "ext4", "ext5", "ext6", "ext7",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brew::aee;

    /// Synthetic module: `AEEMod_Load` calls slot 3 of `IShell` from the pointer in r0.
    ///
    /// ```asm
    /// ldr r3, [r0]        ; r3 = shell vtable
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
    fn memory_map_has_the_expected_regions() {
        let module = load(&module_calling_shell_slot3()).unwrap();
        let names: Vec<_> = module.mem.regions().iter().map(|r| r.name).collect();
        assert_eq!(
            names,
            [
                "module", "nulo", "heap", "sbrk", "stack", "objects", "helpers", "stubs", "surfaces",
                "internal", "vtables"
            ]
        );
    }

    #[test]
    fn each_interface_has_its_own_vtable_in_the_right_place() {
        let module = load(&module_calling_shell_slot3()).unwrap();
        for iface in Interface::ALL {
            let addr = vtable_addr(iface);
            let first = module.mem.read_u32(addr).unwrap();
            assert_eq!(
                aee::decode(first),
                Some((iface, 0)),
                "the vtable of {} does not point to its trampolines",
                iface.name()
            );
        }
    }

    #[test]
    fn shell_points_to_its_own_vtable() {
        let module = load(&module_calling_shell_slot3()).unwrap();
        let vtable = module.mem.read_u32(module.shell).unwrap();
        assert_eq!(vtable, vtable_addr(Interface::Shell));
        // Slot 0 of the shell vtable is the trampoline for slot 0.
        assert_eq!(
            module.mem.read_u32(vtable).unwrap(),
            aee::encode(Interface::Shell, 0)
        );
    }
}
