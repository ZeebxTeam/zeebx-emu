//! The device: framebuffer, controller, speaker and the `ux0:` folder.
//!
//! Filling goes to the GPU when vitaGL comes up. vitaGL translates OpenGL ES 2, and the core's
//! shader is that version. If it does not come up, the frame goes back to being the RGB565
//! painted into the kernel framebuffer. dynarmic does not emit ARMv7 — the core here is the
//! interpreter.

#[path = "gpu.rs"]
mod gpu;

use std::ffi::{c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use zeebx::audio::MidiBackend;
use zeebx::config::ZWheel;
use zeebx::input::{self, Pad};
use zeebx::library::{self, Game};
use zeebx::session::{self, MAX_SLICE, Session, Step};
use zeebx::storage::StoragePaths;
use zeebx::speed::{Frameskip, Pace};

use crate::presentation::{self, Frame};
use crate::host_input::{self, Reading};
use crate::list;
use crate::sound::{self, Ring};

const ROOT: &str = "ux0:data/zeebx";
const ROMS: &str = "ux0:data/zeebx/roms";
const LOG: &str = "ux0:data/zeebx/zeebx.log";
const LOG_C: &[u8] = b"ux0:data/zeebx/zeebx.log\0";

const WIDTH: usize = 960;
const HEIGHT: usize = 544;
const STEP: usize = 960;
/// A 960×544 frame at 32 bits fits in 2 MiB. Uncached `PHYCONT` only comes in multiples of
/// 1 MiB; CDRAM, in multiples of 256 KiB. 2 MiB works for both.
const ALIGNMENT: u32 = 1024 * 1024;

/// Physically contiguous, uncached scanout. The CPU does not paint here: every uncached store
/// crosses the bus. Drawing goes to `Screen::composite`, which is cached, and `sceDmacMemcpy`
/// delivers it.
const SCREEN_MEMORY: u32 = 0x0D80_8060;
/// Where the Vita's frame usually lives. On this device there are 112 MiB free.
const CDRAM_RW: u32 = 0x0940_8060;

/// 1×, and automatic draw skipping when the session measures lag.
///
/// Without skipping, a frame that goes past 16 ms delays the game by the same amount: the turn
/// draws all of them. The automatic mode stops filling the frame that does not go to the screen,
/// as long as a drawn one still fits in the slice — it is the calculation the session already
/// does. The Portable profile's half resolution is left out: it measured 22% on that other
/// device and makes the 3D blocky.
const PACE: Pace = Pace {
    ratio: Some(1.0),
    frameskip: Frameskip::Auto,
    half_rate_present: false,
    mute_sound: false,
};

/// newlib reads this symbol to size `malloc`. The block is requested once and does not grow.
/// 192 MiB fit in the extended mode that `ATTRIBUTE2=12` in `param.sfo` requests (365 MiB of MAIN).
///
/// What goes past 1 MiB does not live here: the guest heap on the Vita is 48 MiB, and Double
/// Dragon died with `without memory` at this number. Those blocks go through [`LargeAllocs`].
///
/// Nothing in this binary reads the value: `#[used]` stops LLVM from deleting the symbol, and the
/// `-Wl,-u` in `build.rs` stops `--gc-sections` from deleting it again. `#[used(linker)]` would do
/// both, and in rustc 1.99 it is still unstable.
#[used]
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
pub static _newlib_heap_size_user: u32 = 192 * 1024 * 1024;

#[repr(C)]
#[allow(dead_code)]
struct SceKernelAllocMemBlockOpt {
    size: u32,
    attr: u32,
    alignment: u32,
    uid_base: u32,
    name: *const c_char,
    flags: i32,
    reserved: [i32; 10],
}

#[repr(C)]
struct SceDisplayFrameBuf {
    size: u32,
    base: *mut c_void,
    pitch: u32,
    pixelformat: u32,
    width: u32,
    height: u32,
}

#[repr(C)]
struct SceCtrlData {
    time_stamp: u64,
    buttons: u32,
    lx: u8,
    ly: u8,
    rx: u8,
    ry: u8,
    rest: [u8; 16],
}

unsafe extern "C" {
    fn sceCtrlSetSamplingMode(mode: i32) -> i32;
    fn sceCtrlPeekBufferPositive(port: i32, pad: *mut SceCtrlData, count: i32) -> i32;
    fn sceCtrlReadBufferPositive(port: i32, pad: *mut SceCtrlData, count: i32) -> i32;
    fn sceDisplaySetFrameBuf(fb: *const SceDisplayFrameBuf, sync: i32) -> i32;
    fn sceDisplayWaitVblankStart() -> i32;
    fn sceAudioOutOpenPort(kind: i32, len: i32, freq: i32, mode: i32) -> i32;
    fn sceAudioOutOutput(port: i32, buf: *const c_void) -> i32;
    fn sceAudioOutSetVolume(port: i32, ch: i32, vol: *mut i32) -> i32;
    fn sceKernelAllocMemBlock(
        name: *const c_char,
        kind: u32,
        size: u32,
        opt: *mut SceKernelAllocMemBlockOpt,
    ) -> i32;
    fn sceKernelGetMemBlockBase(uid: i32, base: *mut *mut c_void) -> i32;
    fn sceKernelFreeMemBlock(uid: i32) -> i32;
    fn sceKernelGetFreeMemorySize(info: *mut FreeMemInfo) -> i32;
    fn sceKernelExitProcess(res: i32) -> i32;
    fn sceDmacMemcpy(dst: *mut c_void, src: *const c_void, size: u32) -> i32;
    fn scePowerGetArmClockFrequency() -> i32;
    fn scePowerGetBusClockFrequency() -> i32;
    fn scePowerSetArmClockFrequency(freq: i32) -> i32;
    fn scePowerSetBusClockFrequency(freq: i32) -> i32;
    fn sceKernelGetThreadId() -> i32;
    fn sceKernelChangeThreadPriority(thid: i32, priority: i32) -> i32;
}

/// The maximum the system accepts. Without this the process stays at the clock the shell left it
/// at.
const CLOCK_ARM_MHZ: i32 = 444;
const CLOCK_BUS_MHZ: i32 = 222;

/// Below the main thread's default, and the same on both threads: the game does not get ahead
/// of the sound. A lower number is more urgent.
const PRIORITY: i32 = 64;

const _: () = assert!(std::mem::size_of::<SceDisplayFrameBuf>() == 0x18);
const _: () = assert!(std::mem::size_of::<SceCtrlData>() == 0x20);
const _: () = assert!(std::mem::size_of::<SceKernelAllocMemBlockOpt>() == 0x40);
const _: () = assert!(cfg!(target_pointer_width = "32"));

fn die(code: i32) -> ! {
    unsafe { sceKernelExitProcess(code) };
    loop {}
}

fn out_of_memory(layout: std::alloc::Layout) {
    // No `std::fs`: the hook runs because an allocation already failed, and opening the file
    // through Rust would allocate again. newlib's `open`, if it does not fit, returns an error
    // instead of reentering here. The number is the size that did not fit, so the next log does
    // not just say "ran out".
    let mut msg = [0u8; 48];
    let prefix = b"out of memory ";
    msg[..prefix.len()].copy_from_slice(prefix);
    let mut n = layout.size();
    let mut digits = [0u8; 20];
    let mut i = 0usize;
    if n == 0 {
        digits[0] = b'0';
        i = 1;
    } else {
        while n > 0 && i < digits.len() {
            digits[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
    }
    let mut p = prefix.len();
    while i > 0 {
        i -= 1;
        msg[p] = digits[i];
        p += 1;
    }
    msg[p] = b'\n';
    p += 1;
    unsafe {
        let fd = open(LOG_C.as_ptr().cast(), 1 | 0x8 | 0x200, 0o644i32);
        if fd >= 0 {
            write(fd, msg.as_ptr().cast(), p);
            close(fd);
        }
    }
}

#[repr(C)]
struct FreeMemInfo {
    size: i32,
    user: i32,
    cdram: i32,
    phycont: i32,
}

fn note_free_memory() {
    let mut info = FreeMemInfo {
        size: std::mem::size_of::<FreeMemInfo>() as i32,
        user: 0,
        cdram: 0,
        phycont: 0,
    };
    let rc = unsafe { sceKernelGetFreeMemorySize(&mut info) };
    if rc < 0 {
        note(&format!("free memory query refused ({rc})"));
        return;
    }
    note(&format!(
        "free user {} cdram {} phycont {}",
        info.user, info.cdram, info.phycont
    ));
}

const BLOCKS: usize = 256;
const PAGE: usize = 256 * 1024;
/// `SCE_KERNEL_MEMBLOCK_TYPE_USER_RW`. Cached: the interpreter reads this all the time.
/// The on-screen frame uses another type, uncached, because the display reads it by DMA.
const USER_RW: u32 = 0x0c20_d060;
const BLOCK_NAME: &[u8] = b"zeebx\0";

#[derive(Clone, Copy)]
struct Slot {
    ptr: usize,
    uid: i32,
}

/// Allocations of 1 MiB or more, outside newlib's fixed heap.
///
/// The 192 MiB heap is requested whole at startup and does not give what is left back to the
/// system. The guest map (48 MiB of heap, 16 of the internal region, 8 of surface, 4 of objects)
/// fits in it and leaves little contiguous memory for the texture and the sound. Each large block
/// asks the kernel for its own memory; `malloc` keeps what is small. If the kernel refuses, it
/// falls back to `malloc` as before.
pub struct LargeAllocs {
    lock: std::sync::atomic::AtomicBool,
    slot_table: std::cell::UnsafeCell<[Slot; BLOCKS]>,
}

unsafe impl Sync for LargeAllocs {}

impl LargeAllocs {
    pub const NEW: Self = Self {
        lock: std::sync::atomic::AtomicBool::new(false),
        slot_table: std::cell::UnsafeCell::new([Slot { ptr: 0, uid: 0 }; BLOCKS]),
    };

    fn lock(&self) {
        while self.lock.swap(true, std::sync::atomic::Ordering::Acquire) {
            core::hint::spin_loop();
        }
    }

    fn release(&self) {
        self.lock
            .store(false, std::sync::atomic::Ordering::Release);
    }

    fn block(&self, size: u32) -> Option<*mut u8> {
        let uid = unsafe {
            sceKernelAllocMemBlock(
                BLOCK_NAME.as_ptr().cast(),
                USER_RW,
                size,
                std::ptr::null_mut(),
            )
        };
        if uid < 0 {
            return None;
        }
        let mut base: *mut c_void = std::ptr::null_mut();
        if unsafe { sceKernelGetMemBlockBase(uid, &mut base) } < 0 || base.is_null() {
            unsafe { sceKernelFreeMemBlock(uid) };
            return None;
        }
        self.lock();
        let slot_table = unsafe { &mut *self.slot_table.get() };
        let slot = slot_table.iter_mut().find(|slot| slot.ptr == 0);
        let Some(slot) = slot else {
            self.release();
            unsafe { sceKernelFreeMemBlock(uid) };
            return None;
        };
        slot.ptr = base as usize;
        slot.uid = uid;
        self.release();
        Some(base.cast())
    }

    fn free_block(&self, ptr: *mut u8) -> bool {
        self.lock();
        let slot_table = unsafe { &mut *self.slot_table.get() };
        let Some(slot) = slot_table.iter_mut().find(|slot| slot.ptr == ptr as usize) else {
            self.release();
            return false;
        };
        let uid = slot.uid;
        slot.ptr = 0;
        slot.uid = 0;
        self.release();
        unsafe { sceKernelFreeMemBlock(uid) };
        true
    }
}

unsafe impl std::alloc::GlobalAlloc for LargeAllocs {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        if layout.size() >= 1024 * 1024
            && layout.align() <= PAGE
            && let Some(size) = round_up(layout.size())
            && let Some(ptr) = self.block(size)
        {
            return ptr;
        }
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        if self.free_block(ptr) {
            return;
        }
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

fn round_up(n: usize) -> Option<u32> {
    let full = n.checked_add(PAGE - 1)?;
    u32::try_from(full & !(PAGE - 1)).ok()
}

unsafe extern "C" {
    fn open(path: *const c_char, flags: i32, ...) -> i32;
    fn write(fd: i32, buf: *const c_void, n: usize) -> isize;
    fn close(fd: i32) -> i32;
}

fn note(text: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG)
    {
        use std::io::Write;
        let _ = writeln!(file, "{text}");
    }
}

fn flush_log() {
    let lines = zeebx::logging::drain();
    if lines.is_empty() {
        return;
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG)
    {
        use std::io::Write;
        for line in lines {
            let _ = writeln!(
                file,
                "[{}] {}: {}",
                line.level.tag(),
                line.target,
                line.text
            );
        }
    }
}

fn reading_from(data: &SceCtrlData) -> Reading {
    Reading {
        buttons: data.buttons,
        lx: data.lx,
        ly: data.ly,
        rx: data.rx,
        ry: data.ry,
    }
}

fn read_blocking() -> Reading {
    let mut data = SceCtrlData {
        time_stamp: 0,
        buttons: 0,
        lx: 128,
        ly: 128,
        rx: 128,
        ry: 128,
        rest: [0; 16],
    };
    let n = unsafe { sceCtrlReadBufferPositive(0, &mut data, 1) };
    if n < 1 {
        Reading {
            lx: 128,
            ly: 128,
            rx: 128,
            ry: 128,
            ..Reading::default()
        }
    } else {
        reading_from(&data)
    }
}

fn read_now() -> Reading {
    let mut data = SceCtrlData {
        time_stamp: 0,
        buttons: 0,
        lx: 128,
        ly: 128,
        rx: 128,
        ry: 128,
        rest: [0; 16],
    };
    let n = unsafe { sceCtrlPeekBufferPositive(0, &mut data, 1) };
    if n < 1 {
        Reading {
            lx: 128,
            ly: 128,
            rx: 128,
            ry: 128,
            ..Reading::default()
        }
    } else {
        reading_from(&data)
    }
}

struct Screen {
    base: *mut u32,
    /// The frame the CPU paints. The usable start is aligned to 16: the DMA requires it, and
    /// `Vec<u32>` only guarantees 4.
    composite: Vec<u32>,
    start: usize,
    index: usize,
    stride: usize,
    width: usize,
    step: usize,
    height: usize,
    already_warned: bool,
    already_warned_dma: bool,
    /// vitaGL took over the screen. The composite is uploaded as a texture; there is no kernel
    /// scanout.
    on_gpu: bool,
}

fn aligned_composite(words: usize) -> (Vec<u32>, usize) {
    let composite = vec![presentation::BLACK; words + 3];
    let addr = composite.as_ptr() as usize;
    let remaining = (16 - (addr & 15)) & 15;
    (composite, remaining / 4)
}

fn alloc_screen(kind: u32, size: u32) -> Result<i32, i32> {
    // Without `opt`. Requesting alignment in the struct (`attr` 0x4) returns
    // `0x80020005` on the device, with 256 KiB and with 1 MiB. Vita3K accepts the
    // struct. The kernel already aligns the block by type when the pointer is null.
    let uid = unsafe {
        sceKernelAllocMemBlock(c"zeebx-fb".as_ptr(), kind, size, std::ptr::null_mut())
    };
    if uid < 0 { Err(uid) } else { Ok(uid) }
}

impl Screen {
    fn open(on_gpu: bool) -> Result<Self, String> {
        unsafe { sceCtrlSetSamplingMode(1) };
        if on_gpu {
            let (composite, start) = aligned_composite(STEP * HEIGHT);
            let mut screen = Self {
                base: std::ptr::null_mut(),
                composite,
                start,
                index: 0,
                stride: 0,
                width: WIDTH,
                step: STEP,
                height: HEIGHT,
                already_warned: false,
                already_warned_dma: false,
                on_gpu: true,
            };
            screen.present(|q| presentation::clear(q, presentation::BLACK));
            screen.present(|q| presentation::clear(q, presentation::BLACK));
            return Ok(screen);
        }
        let bytes = ((STEP * HEIGHT * 4) as u32).div_ceil(ALIGNMENT) * ALIGNMENT;
        let size = bytes * 2;
        let uid = match alloc_screen(SCREEN_MEMORY, size) {
            Ok(uid) => uid,
            Err(phy) => match alloc_screen(CDRAM_RW, size) {
                Ok(uid) => {
                    note(&format!("screen in cdram; phycont refused ({phy})"));
                    uid
                }
                Err(cdram) => {
                    return Err(format!(
                        "could not get the screen memory (phycont {phy}, cdram {cdram})"
                    ));
                }
            },
        };
        let mut base: *mut c_void = std::ptr::null_mut();
        let rc = unsafe { sceKernelGetMemBlockBase(uid, &mut base) };
        if rc < 0 || base.is_null() {
            return Err(format!("the screen memory has no address ({rc})"));
        }
        let (composite, start) = aligned_composite(STEP * HEIGHT);
        let mut screen = Self {
            base: base.cast(),
            composite,
            start,
            index: 0,
            stride: bytes as usize,
            width: WIDTH,
            step: STEP,
            height: HEIGHT,
            already_warned: false,
            already_warned_dma: false,
            on_gpu: false,
        };
        screen.present(|q| presentation::clear(q, presentation::BLACK));
        screen.present(|q| presentation::clear(q, presentation::BLACK));
        Ok(screen)
    }

    fn present(&mut self, draw: impl FnOnce(&mut Frame<'_>)) {
        let n = self.step * self.height;
        let start = self.start;
        {
            let pixels = &mut self.composite[start..start + n];
            let mut frame = Frame {
                pixels,
                width: self.width,
                step: self.step,
                height: self.height,
            };
            draw(&mut frame);
        }
        if self.on_gpu {
            gpu::show_rgba(
                &self.composite[start..start + n],
                self.width as i32,
                self.height as i32,
            );
            return;
        }
        let offset = self.index * (self.stride / 4);
        let dst = unsafe { self.base.add(offset) };
        let src = unsafe { self.composite.as_ptr().add(start) };
        let bytes = (n * 4) as u32;
        let rc = unsafe { sceDmacMemcpy(dst.cast(), src.cast(), bytes) };
        if rc < 0 {
            if !self.already_warned_dma {
                self.already_warned_dma = true;
                note(&format!("the screen DMA failed ({rc}); copying with the CPU"));
            }
            unsafe { std::ptr::copy_nonoverlapping(src, dst, n) };
        }
        let fb = SceDisplayFrameBuf {
            size: 0x18,
            base: dst.cast(),
            pitch: self.step as u32,
            pixelformat: 0,
            width: self.width as u32,
            height: self.height as u32,
        };
        let rc = unsafe { sceDisplaySetFrameBuf(&fb, 1) };
        if rc < 0 && !self.already_warned {
            self.already_warned = true;
            note(&format!("the screen refused the frame: {rc}"));
        }
        unsafe { sceDisplayWaitVblankStart() };
        self.index ^= 1;
    }
}

struct Sound {
    ring: Arc<Ring>,
    port: i32,
}

impl Sound {
    fn open() -> Self {
        let ring = Arc::new(Ring::create(8_192));
        let port = unsafe { sceAudioOutOpenPort(0, sound::GRAIN as i32, sound::RATE as i32, 1) };
        if port < 0 {
            note(&format!("no sound: the port returned {port}"));
            return Self { ring, port: -1 };
        }
        let mut volume = [0x8000i32, 0x8000];
        unsafe { sceAudioOutSetVolume(port, 3, volume.as_mut_ptr()) };
        let ring_for_thread = Arc::clone(&ring);
        let created = std::thread::Builder::new()
            .name("som".into())
            .stack_size(64 * 1024)
            .spawn(move || audio_thread(port, ring_for_thread));
        if let Err(error) = created {
            note(&format!("no sound: {error}"));
            return Self { ring, port: -1 };
        }
        Self { ring, port }
    }
}

fn audio_thread(port: i32, ring: Arc<Ring>) {
    set_priority();
    // Two blocks: the port reads what it just received until the next call returns. Filling the
    // same block again before that replaces the sound that is still playing.
    let mut blocks = [
        sound::Block {
            samples: [0; sound::GRAIN * 2],
        },
        sound::Block {
            samples: [0; sound::GRAIN * 2],
        },
    ];
    let mut which = 0;
    loop {
        ring.pull(&mut blocks[which].samples);
        let rc = unsafe { sceAudioOutOutput(port, blocks[which].samples.as_ptr().cast()) };
        if rc < 0 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        which ^= 1;
    }
}

struct Game {
    session: Session,
    mixer: zeebx::audio::Mixer,
    prev_pad: Pad,
    last: Instant,
    clock: u32,
    last_sound: Instant,
    mix: Vec<f32>,
    pcm: Vec<i16>,
    exit_hold: Option<Instant>,
    via_z_wheel: bool,
    first: bool,
    /// The menu is still in the composite frame. The game's first presentation clears the strip.
    dirty_band: bool,
}

struct App {
    games: Vec<Game>,
    installed: Vec<(u32, String)>,
    storage: StoragePaths,
    cursor: usize,
    error: String,
    game: Option<Game>,
    saw_frame: bool,
    exits_without_frame: u32,
}

impl App {
    fn create() -> Self {
        let storage =
            StoragePaths::for_frontend(Path::new("ux0:data"), Some(Path::new("ux0:data")));
        let mut error = String::new();
        if let Err(failure) = storage.create_dirs() {
            error = format!("could not create the folders: {failure}");
        }
        let _ = std::fs::create_dir_all(ROMS);
        Self {
            games: Vec::new(),
            installed: Vec::new(),
            storage,
            cursor: 0,
            error,
            game: None,
            saw_frame: false,
            exits_without_frame: 0,
        }
    }

    fn scan(&mut self) {
        self.games = library::scan(Path::new(ROMS));
        self.installed = self
            .games
            .iter()
            .filter_map(|game| Some((game.clsid?, library::module_id(&game.path)?)))
            .collect();
        if self.games.is_empty() {
            self.cursor = 0;
        } else if self.cursor >= self.games.len() {
            self.cursor = self.games.len() - 1;
        }
    }

    fn class_path(&self, class_id: u32) -> Option<PathBuf> {
        self.games
            .iter()
            .find(|game| game.clsid == Some(class_id))
            .map(|game| game.path.clone())
    }

    fn open_path(
        &mut self,
        path: &Path,
        via_z_wheel: bool,
        sound: &Sound,
    ) -> Result<(), String> {
        let installed = self.installed.clone();
        // With vitaGL up, the list shows. The game's frame, in this Vita3K,
        // does not come back from the framebuffer: drawing on the GPU and just swapping the
        // buffers republishes the list. Without the readback, the game goes out on the CPU.
        let guard = gpu::keeps_frame();
        let context = guard.then(gpu::context).flatten();
        let on_gpu = context.is_some();
        match Session::start_with_storage_installed_policy(
            path,
            zeebx::DEFAULT_PORTS,
            None,
            on_gpu,
            context,
            ZWheel::default(),
            &self.storage,
            &installed,
            MidiBackend::Wavetable,
        ) {
            Ok(mut session) => {
                let clock = session.clock_ms();
                let mixer = session.record_audio(sound::RATE);
                sound.ring.clear();
                self.saw_frame = false;
                self.game = Some(Game {
                    session,
                    mixer,
                    prev_pad: Pad::default(),
                    last: Instant::now(),
                    clock,
                    last_sound: Instant::now(),
                    mix: Vec::new(),
                    pcm: Vec::new(),
                    exit_hold: None,
                    via_z_wheel,
                    first: true,
                    dirty_band: true,
                });
                note(&format!("opened {}", path.display()));
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn open_selected(&mut self, sound: &Sound) -> Result<(), String> {
        let path = self
            .games
            .get(self.cursor)
            .ok_or("no game selected")?
            .path
            .clone();
        self.open_path(&path, false, sound)
    }

    fn back_to_menu(&mut self, sound: &Sound, error: String) {
        self.game = None;
        sound.ring.clear();
        self.error = error;
        self.saw_frame = false;
        // The count is of consecutive exits, in the middle of the Z-Wheel reopening. Going back
        // to the list resets it: the next game the player picks starts clean.
        self.exits_without_frame = 0;
    }

    fn menu(&mut self, screen: &mut Screen) {
        let mut before = 0u32;
        let mut dir_prev = 0i32;
        let mut dir_since = Instant::now();
        let mut dir_step = Instant::now();
        let mut dirty = true;
        loop {
            flush_log();
            if dirty {
                let titles: Vec<&str> =
                    self.games.iter().map(|game| game.title.as_str()).collect();
                let error = self.error.clone();
                let cursor = self.cursor;
                screen.present(|q| presentation::draw_menu(q, &titles, cursor, &error));
                dirty = false;
            }
            let reading = read_blocking();
            let action = list::touch(reading.buttons, before);
            let dir = list::direction(reading.buttons);
            let border = if dir != dir_prev { dir } else { 0 };
            if dir != dir_prev {
                dir_prev = dir;
                dir_since = Instant::now();
                dir_step = Instant::now();
            }
            let delta = list::list_step(border, dir, dir_since.elapsed(), dir_step.elapsed());
            if delta != 0 && border == 0 {
                dir_step = Instant::now();
            }
            before = reading.buttons;
            match action {
                list::Press::Quit => die(0),
                list::Press::Refresh => {
                    screen.present(|q| presentation::draw_menu(q, &[], 0, "Reading the folder..."));
                    self.error.clear();
                    self.scan();
                    dirty = true;
                }
                list::Press::Confirm if !self.games.is_empty() => return,
                _ => {}
            }
            if delta != 0 {
                self.cursor = list::move_cursor(self.cursor, self.games.len(), delta);
                dirty = true;
            }
        }
    }

    fn frame(&mut self, vita_screen: &mut Screen, sound: &Sound) {
        let mut quit = false;
        let mut step = Step::Running;
        let mut request = None;
        let mut normal = false;
        let mut reason = String::new();
        let mut class_id = 0u32;
        let mut opened_via_z_wheel = false;
        let mut presented = false;
        {
            let game = self.game.as_mut().expect("frame without a game");
            let reading = read_now();
            if host_input::is_exit_combo(reading.buttons) {
                if game.exit_hold.is_none() {
                    game.exit_hold = Some(Instant::now());
                }
                if game
                    .exit_hold
                    .is_some_and(|since| host_input::exit_hold_elapsed(since.elapsed()))
                {
                    quit = true;
                }
            } else {
                game.exit_hold = None;
            }
            if !quit {
                let mut for_game = reading;
                for_game.buttons = host_input::filter_exit(reading.buttons);
                let pad = host_input::to_pad(&for_game);
                if game.first {
                    game.prev_pad = pad;
                    game.first = false;
                }
                let before = game.prev_pad;
                game.session.set_port_pad(0, pad);
                for (avk, pressed) in input::controller_key_events(&before, &pad) {
                    game.session.set_key(avk, pressed);
                }
                game.prev_pad = pad;

                let frame_now = Instant::now();
                let slice = frame_now
                    .saturating_duration_since(game.last)
                    .min(MAX_SLICE);
                game.last = frame_now;
                if game.dirty_band {
                    game.dirty_band = false;
                    vita_screen.present(|q| presentation::clear(q, presentation::BLACK));
                    // The clear changes the viewport and the program. The GPU mirror still thinks
                    // the previous state is in effect, and the next frame would cover the whole
                    // screen.
                    if gpu::is_active() {
                        game.session.resume_context();
                    }
                }
                if gpu::is_active() {
                    gpu::prepare_frame();
                }
                let intermediate = game.session.show_intermediate_frame();
                if !intermediate {
                    step = game.session.advance(slice, &PACE);
                }
                if step == Step::Presented {
                    presented = true;
                }
                request = game.session.take_launch_request();
                if step == Step::Stopped {
                    normal = game.session.exited_on_its_own();
                    reason = game.session.stopped_reason().unwrap_or_default();
                    class_id = game.session.class_id();
                    opened_via_z_wheel = game.via_z_wheel;
                }
                // The image goes out before the mix. The sound does not change the frame, and
                // mixing it before the swap delayed the command by the render time.
                if intermediate || step == Step::Presented {
                    // The target texture, 1:1 in the center. Without the frame readback the
                    // session is on the CPU, and the image goes out through the CPU.
                    let texture = match gpu::keeps_frame() && !intermediate {
                        true => game.session.frame_texture(),
                        false => None,
                    };
                    if let Some(id) = texture {
                        gpu::show_texture(id, 160, 32, 640, 480);
                        game.session.resume_context();
                    } else {
                        vita_screen.present(|q| {
                            // The copy only writes the center. Without this the list stays in the
                            // strip.
                            presentation::clear(q, presentation::BLACK);
                            let screen = game.session.screen();
                            presentation::copy_rgb565(
                                q,
                                screen.pixels(),
                                screen.width(),
                                screen.height(),
                            );
                        });
                    }
                } else if step == Step::Ahead {
                    // When ahead, the turn returns right away. Without this wait the loop spins
                    // idle.
                    unsafe { sceDisplayWaitVblankStart() };
                }
                if sound.port >= 0 {
                    let now_ms = game.session.clock_ms();
                    let virt = u64::from(now_ms.wrapping_sub(game.clock));
                    game.clock = now_ms;
                    let real = game.last_sound.elapsed().as_millis() as u64;
                    game.last_sound = Instant::now();
                    let due = sound::samples_due(real, virt, sound::RATE);
                    game.mixer.render_into(due, &mut game.mix);
                    game.pcm.clear();
                    game.pcm.extend(
                        game.mix
                            .iter()
                            .map(|sample| (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16),
                    );
                    sound.ring.push(&game.pcm);
                }
            }
        }
        flush_log();
        if quit {
            note("back to the list");
            self.back_to_menu(sound, String::new());
            return;
        }
        if let Some(class_id) = request {
            match self.class_path(class_id) {
                Some(path) => {
                    let via_z_wheel = class_id != session::Z_WHEEL;
                    if let Err(error) = self.open_path(&path, via_z_wheel, sound) {
                        note(&error);
                        self.back_to_menu(sound, error);
                    }
                }
                None => {
                    let error = format!("The shell asked for {class_id:#010x}, which is not in the folder");
                    note(&error);
                    self.back_to_menu(sound, error);
                }
            }
            return;
        }
        if presented {
            self.saw_frame = true;
            self.exits_without_frame = 0;
        }
        if step != Step::Stopped {
            return;
        }
        if !normal {
            note(&reason);
            self.back_to_menu(sound, reason);
            return;
        }
        if !self.saw_frame {
            self.exits_without_frame += 1;
        } else {
            self.exits_without_frame = 0;
        }
        let reopen = class_id == session::Z_WHEEL || opened_via_z_wheel;
        if reopen && self.exits_without_frame < 3 {
            if let Some(path) = self.class_path(session::Z_WHEEL) {
                if let Err(error) = self.open_path(&path, false, sound) {
                    note(&error);
                    self.back_to_menu(sound, error);
                }
                return;
            }
        }
        self.back_to_menu(sound, String::new());
    }
}

fn raise_clock() {
    unsafe {
        let before_arm = scePowerGetArmClockFrequency();
        let before_bus = scePowerGetBusClockFrequency();
        let arm = scePowerSetArmClockFrequency(CLOCK_ARM_MHZ);
        let bus = scePowerSetBusClockFrequency(CLOCK_BUS_MHZ);
        let now_arm = scePowerGetArmClockFrequency();
        let now_bus = scePowerGetBusClockFrequency();
        note(&format!(
            "clock {before_arm}/{before_bus} MHz -> {now_arm}/{now_bus} MHz (requested {arm}/{bus})"
        ));
    }
}

fn set_priority() {
    unsafe {
        let id = sceKernelGetThreadId();
        let rc = sceKernelChangeThreadPriority(id, PRIORITY);
        if rc < 0 {
            note(&format!("priority {PRIORITY} refused ({rc})"));
        }
    }
}

pub fn run() -> ! {
    let _ = std::fs::create_dir_all(ROOT);
    raise_clock();
    set_priority();
    let log = LOG.to_string();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            use std::io::Write;
            let _ = writeln!(file, "panic: {info}");
        }
    }));
    // Running out of memory aborts without going through the panic. The hook is a `fn`, not a
    // closure: in the middle of a refused allocation it cannot capture anything.
    std::alloc::set_alloc_error_hook(out_of_memory);
    unsafe { std::env::set_var("HOME", ROOT) };
    zeebx::logging::from_env();
    // 8 MiB of already decoded sound, the Portable profile's ceiling. Above that the cache
    // competes with the game for the device's RAM.
    zeebx::machine::set_sound_cache_cap(8 * 1024 * 1024);
    note("Zeebx");
    note_free_memory();

    let on_gpu = match gpu::open() {
        gpu::Opening::Ready => true,
        gpu::Opening::NoGpu => false,
        gpu::Opening::TookScreen(error) => {
            note(&error);
            die(1);
        }
    };
    let mut screen = match Screen::open(on_gpu) {
        Ok(screen) => screen,
        Err(error) => {
            note(&error);
            die(1);
        }
    };
    let sound = Sound::open();
    let mut app = App::create();
    screen.present(|q| presentation::draw_menu(q, &[], 0, "Reading the folder..."));
    app.scan();
    flush_log();
    loop {
        app.menu(&mut screen);
        if let Err(error) = app.open_selected(&sound) {
            note(&error);
            app.error = error;
            continue;
        }
        while app.game.is_some() {
            app.frame(&mut screen, &sound);
        }
    }
}
