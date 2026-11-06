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
pub mod miffile;
pub mod modfile;
pub mod resfile;

use crate::brew::aee::{self, Interface};
use crate::cpu::CpuError;
use crate::cpu::mem::{GuestMemory, MemError};
use crate::loader::modfile::ModImage;

/// Where the module image is loaded.
///
/// The linker script sets `ro-base = 0`, but the module **cannot** sit at address 0: the stub
/// that `elf2mod` puts at the start computes its own base with PC-relative arithmetic and then
/// reads two words **before** it (`ldr r0, [r1, #-8]` and `[r1, #-4]`, with `r1` = base). The
/// AEE loader therefore reserves a prefix before the image. Since the stub is
/// position-independent, any base works — as long as there is valid memory right below it.
pub const MODULE_BASE: u32 = 0x0001_0000;
/// Size of the prefix reserved before the image, for the words the stub reads at negative
/// offsets. One page is more than enough slack.
pub const MODULE_PREFIX: u32 = 0x1000;
/// Zeroed space reserved after the image, for the module's `.bss`.
///
/// The `.mod` stores only the sections with content; the `.bss` exists at run time and not in
/// the file. Until we know how to read its real size from the header, we reserve a generous
/// slack — leftover zeroed memory is harmless.
pub const MODULE_BSS_SLACK: usize = 1024 * 1024;
/// The module's heap — where `MALLOC` serves from.
///
/// 64 MB because Quake measures free memory before loading the `.pak` files and gives up with
/// "Not enough free memory" if it is small. The console has 128 MB of RAM
/// (`docs/01-hardware.md`), and the size here is our choice — it only needs to be roomy enough
/// for the game to accept the device.
///
/// On the Vita the whole process fits in 232 MiB, and the zeroed `Vec` is really reserved:
/// newlib does not have the desktop's overcommit. With 64 MiB here, the frontend's next frame
/// could not find the temporary block of ~1.2 MiB (640×480 at 32 bits): vitaGL copies that
/// pointer even when the allocation comes back null, and Zeetris, already open, died in a data
/// abort inside `sceClibMemcpy`. 48 MiB is still the heap the Z-Wheel reads as "how much is
/// free". The internal region adds another 16 MiB reserved on the device. I did not measure
/// whether 48 is enough for Quake there.
///
/// **Measured on 22/09/2026, and the number stayed:** doubling to 128 MB does not move the
/// Z-Wheel. It allocates ~64 MiB in its own pool (its `memcheck` prints `Free(67001392)`, the
/// same number at both sizes) and then asks for **all that is left** — 67 001 488 with 64 MB,
/// 134 110 352 with 128 MB. The request follows the heap instead of being a need of its own, so
/// growing the heap only moves the target. What remained from that measurement is the honest
/// accounting of "how much is free": see [`crate::brew::heap::Heap::largest_block`].
pub const HEAP_BASE: u32 = 0x1000_0000;
pub const HEAP_SIZE: usize = if cfg!(target_os = "vita") {
    48 * 1024 * 1024
} else {
    64 * 1024 * 1024
};
/// Stack. `sp` starts at the top because the ARM stack grows downwards.
pub const STACK_BASE: u32 = 0x2000_0000;
pub const STACK_SIZE: usize = 1024 * 1024;
/// Objects the emulator exposes to the guest — each one starts with the vtable pointer.
///
/// Four megabytes give 65,536 objects, and the number is not a whim: with 64 KB it was **one
/// thousand and twenty-four**, and the Z-Wheel used them up in a single run. After that
/// everything fails at once — `Couldn't open DB: 3`, `Unable to create vector model`, forms with
/// error 3 — and none of those symptoms looks like the cause.
///
/// One thousand and twenty-four was **our** limit, not the console's: there, objects come out of
/// the BREW heap, which has tens of megabytes. The region is zeroed and only takes up what gets
/// touched, so the slack does not cost real memory.
///
/// This does **not** excuse fixing whatever leaks. But a ceiling of a thousand objects turns any
/// small leak into a cascading failure, and a cascading failure hides the cause.
pub const OBJECT_BASE: u32 = 0x3000_0000;
pub const OBJECT_SIZE: usize = 4 * 1024 * 1024;
/// Memory for the surfaces' pixels.
///
/// It must live in the guest's space because `IDIB` hands the game the buffer pointer so it can
/// draw directly — that is how the commercial games write to the screen.
pub const SURFACE_BASE: u32 = 0x4000_0000;
pub const SURFACE_SIZE: usize = 8 * 1024 * 1024;

/// Memory that **the emulator** requests in the guest's space, outside the game's heap.
///
/// `IThread` stacks, the `IPeek` line buffer, the `AEEImageInfo`, the `eglQueryString` strings
/// and the out pointers we pass to the game's callbacks. They used to live in the heap,
/// interleaved with what the game requests, and that made the address of every game structure
/// depend on the **Zeebx version**: one extra block of ours and everything after it shifted.
/// RetroAchievements reads the heap by address, and an achievement cannot break because the
/// emulator started allocating differently. With the heap belonging only to the game, the
/// addresses depend only on what the game requests.
///
/// What the game requests **through an API** — `STRDUP`, `LoadResData`, `IHeap_Malloc` — stays
/// in the heap: there it is the system allocating on the game's behalf, and the size comes from
/// the game, not from us.
///
/// 16 MB because `IPeek` reserves the size of the whole font and each thread takes at least
/// 64 KB. On the desktop the region is zeroed and only takes up what gets touched. On the Vita
/// the whole block is reserved at startup: newlib does not hand out pages on demand.
pub const INTERNAL_BASE: u32 = 0x3400_0000;
pub const INTERNAL_SIZE: usize = 16 * 1024 * 1024;

/// The address belongs to a block of the internal region, and not to the game's heap.
pub fn is_internal(addr: u32) -> bool {
    (INTERNAL_BASE..INTERNAL_BASE + INTERNAL_SIZE as u32).contains(&addr)
}

/// Where the **extension** modules are mapped.
///
/// An extension module is a `.mod` like any other: same image linked at base zero, same
/// `AEEMod_Load`, same two helper words right before the base. What changes is who calls it —
/// the console loads it when a game asks for a class it provides, and not because the user
/// opened a title.
///
/// Mapping them at load time, and not when they are requested, is a simplicity decision: the
/// core has no API to map a region after `reset`, and the modules are small (90 KB and 164 KB in
/// the two packages we have). What is left for request time is **calling** `AEEMod_Load`.
pub const EXT_MODULE_BASE: u32 = 0x0800_0000;
/// Distance between two extensions. It must fit the image, the prefix and the `.bss`.
pub const EXT_MODULE_SPACING: u32 = 0x0100_0000;
/// How many extensions fit before running into the guest heap, at `0x10000000`.
pub const MAX_EXTENSIONS: usize = 8;

/// BREW's stdlib helper table.
///
/// Dynamic modules do not link against libc: they call `MALLOC`, `STRLEN` and friends through a
/// table of function pointers that the loader hands them. The generated code does
/// `ldr r1, [r0, #<offset>]` followed by `bx r1`, where `r0` is the pointer to that table — the
/// same pattern appears in `helloworld.mod` and in `tectoy.mod`, byte for byte.
///
/// We fill each entry with a trampoline address, so any helper used shows up as
/// `AEEHelpers::slot[n]` in the log instead of jumping into the void.
pub const HELPERS_BASE: u32 = 0x3100_0000;
/// How many pointers the table exposes. `struct AEEHelperFuncs` has 117 fields; we round up in
/// case some Zeebo build has a few more at the end.
const HELPERS_SLOTS: u32 = 256;

/// Snippets of ARM code that the emulator writes for the guest to execute.
///
/// They serve the helpers that always return the same thing. Answering one of them through the
/// trampoline costs stopping and restarting the ARM core, and `GetAppInstance` alone accounts for
/// more than half of Quake's API calls — BREW games keep their globals inside the applet, so
/// every access to a global goes through it. As code, the call never even leaves the CPU.
pub const STUB_BASE: u32 = 0x3200_0000;
pub const STUB_SIZE: usize = 0x1000;

/// Vtables: read-only memory whose slots point into the trampoline range.
pub const VTABLE_BASE: u32 = 0xe000_0000;
/// Version of the helper table, read by the module via `GET_HELPER_VER()`.
const HELPER_VERSION: u32 = 1;

/// How many slots we reserve per vtable. No BREW interface comes close to this.
const SLOTS_PER_VTABLE: u32 = 256;
/// Interfaces whose objects are created right at load time.
const BOOT_INTERFACES: [Interface; 2] = [Interface::Shell, Interface::Module];

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

/// Builds the helper table: each entry is the trampoline of the corresponding helper.
fn build_helper_table() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HELPERS_SLOTS as usize * 4);
    for slot in 0..HELPERS_SLOTS {
        bytes.extend_from_slice(&aee::encode(Interface::Helpers, slot).to_le_bytes());
    }
    bytes
}

/// Address of an interface's vtable.
pub fn vtable_addr(iface: Interface) -> u32 {
    VTABLE_BASE + (iface as u32) * SLOTS_PER_VTABLE * 4
}

/// Builds all the vtables: each slot holds the trampoline address of the corresponding method.
fn build_vtables() -> Vec<u8> {
    let mut bytes = Vec::new();
    // Walks the canonical list, which is indexed by the enum value — the same index that
    // `vtable_addr` uses to compute the address.
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
                "module", "nulo", "heap", "stack", "objects", "helpers", "stubs", "surfaces",
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
