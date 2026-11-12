//! Call trampoline for the BREW APIs.
//!
//! A BREW object is a pointer to a struct whose first word points to the vtable. Calling
//! `ISHELL_CreateInstance(shell, ...)` in the guest becomes, in ARM code, a load of the vtable
//! pointer, a load of the slot and a `blx` to the address read.
//!
//! We take advantage of that: the addresses we put in the slots sit in an **unmapped** range,
//! chosen so that the address itself encodes which method was called. The ARM core aborts the
//! fetch, and [`decode`] returns the interface and the slot.

use crate::brew::aee_helpers;
use crate::brew::aee_slots;
use crate::cpu::API_BASE;

/// How many bits of the address identify the interface.
const IFACE_SHIFT: u32 = 12;
/// Space reserved for each interface — 1024 slots, far more than needed.
const IFACE_STRIDE: u32 = 1 << IFACE_SHIFT;

/// The interface **as a number**, for the save state, and back.
///
/// The number is the enum's discriminant, which was already explicit in the code — it is not a
/// new table someone has to maintain in parallel. The way back is written by hand because Rust
/// does not undo an `as u32` by itself, and the test below checks the whole list: a new variant
/// with no entry here turns the test red, which is what you want from a format stored on disk.
pub fn code(iface: Interface) -> u32 {
    iface as u32
}

/// The interface back from its number. `None` for a number that belongs to no interface.
pub fn from_code(value: u32) -> Option<Interface> {
    Some(match value {
        0 => Interface::Shell,
        1 => Interface::Module,
        2 => Interface::Applet,
        3 => Interface::FileMgr,
        4 => Interface::File,
        5 => Interface::Display,
        6 => Interface::Helpers,
        7 => Interface::Bitmap,
        8 => Interface::Hid,
        9 => Interface::HidDevice,
        10 => Interface::Signal,
        11 => Interface::SignalCtl,
        12 => Interface::SignalCbFactory,
        13 => Interface::Graphics,
        14 => Interface::Sound,
        15 => Interface::License,
        16 => Interface::MemAStream,
        17 => Interface::Image,
        18 => Interface::Thread,
        19 => Interface::Egl,
        20 => Interface::Gles,
        21 => Interface::MediaUtil,
        22 => Interface::Media,
        23 => Interface::EglLegacy,
        24 => Interface::GlLegacy,
        25 => Interface::Web,
        26 => Interface::Hash,
        27 => Interface::CipherFactory,
        28 => Interface::Cipher,
        29 => Interface::Heap,
        30 => Interface::UnzipStream,
        31 => Interface::ImageDecoder,
        32 => Interface::ForceFeed,
        33 => Interface::EglSurfaceManip,
        34 => Interface::GlesImageonExt,
        35 => Interface::Probe,
        36 => Interface::SqlMgr,
        37 => Interface::SqlDatabase,
        38 => Interface::Collection,
        39 => Interface::SourceUtil,
        40 => Interface::Widget,
        41 => Interface::ZeeboMcp,
        42 => Interface::Config,
        43 => Interface::Source,
        44 => Interface::Peek,
        45 => Interface::Vector,
        46 => Interface::Class28e3c,
        47 => Interface::Cm,
        48 => Interface::SystemCtl,
        49 => Interface::Typeface,
        50 => Interface::SimCardCtl,
        51 => Interface::Control,
        52 => Interface::Transform,
        53 => Interface::Canvas,
        54 => Interface::Font,
        55 => Interface::Gles11Ext,
        56 => Interface::Gles10Ext,
        57 => Interface::EglGetPowerLevel,
        58 => Interface::EglOesSwapInterval,
        59 => Interface::EglGetColorBuffer,
        60 => Interface::Gles11ExtPak,
        61 => Interface::Joystick,
        63 => Interface::SoundPlayer,
        _ => return None,
    })
}

#[cfg(test)]
mod encoding_tests {
    use super::*;

    /// **Every** variant goes and comes back, and each one has its own number.
    ///
    /// The list is written by hand on purpose: if someone adds an interface and forgets the way
    /// back, the count below does not match and the test fails **before** a save state becomes
    /// unreadable.
    #[test]
    fn every_interface_has_a_number_and_round_trips() {
        let all: [(u32, &str); 63] = [
        (0, "Shell"),
        (1, "Module"),
        (2, "Applet"),
        (3, "FileMgr"),
        (4, "File"),
        (5, "Display"),
        (6, "Helpers"),
        (7, "Bitmap"),
        (8, "Hid"),
        (9, "HidDevice"),
        (10, "Signal"),
        (11, "SignalCtl"),
        (12, "SignalCbFactory"),
        (13, "Graphics"),
        (14, "Sound"),
        (15, "License"),
        (16, "MemAStream"),
        (17, "Image"),
        (18, "Thread"),
        (19, "Egl"),
        (20, "Gles"),
        (21, "MediaUtil"),
        (22, "Media"),
        (23, "EglLegacy"),
        (24, "GlLegacy"),
        (25, "Web"),
        (26, "Hash"),
        (27, "CipherFactory"),
        (28, "Cipher"),
        (29, "Heap"),
        (30, "UnzipStream"),
        (31, "ImageDecoder"),
        (32, "ForceFeed"),
        (33, "EglSurfaceManip"),
        (34, "GlesImageonExt"),
        (35, "Probe"),
        (36, "SqlMgr"),
        (37, "SqlDatabase"),
        (38, "Collection"),
        (39, "SourceUtil"),
        (40, "Widget"),
        (41, "ZeeboMcp"),
        (42, "Config"),
        (43, "Source"),
        (44, "Peek"),
        (45, "Vector"),
        (46, "Class28e3c"),
        (47, "Cm"),
        (48, "SystemCtl"),
        (49, "Typeface"),
        (50, "SimCardCtl"),
        (51, "Control"),
        (52, "Transform"),
        (53, "Canvas"),
        (54, "Font"),
        (55, "Gles11Ext"),
        (56, "Gles10Ext"),
        (57, "EglGetPowerLevel"),
        (58, "EglOesSwapInterval"),
        (59, "EglGetColorBuffer"),
        (60, "Gles11ExtPak"),
        (61, "Joystick"),
        (63, "SoundPlayer"),
        ];
        for (value, name) in all {
            let iface = from_code(value).unwrap_or_else(|| panic!("{name} ({value}) did not come back"));
            assert_eq!(code(iface), value, "{name} did not come back to the same number");
            assert_eq!(format!("{iface:?}"), name, "the variant for {value} is not {name}");
        }
        assert_eq!(from_code(9999), None, "a made-up number should return nothing");
    }
}

/// Interfaces the emulator knows. The numeric value goes into the trampoline address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Interface {
    Shell = 0,
    Module = 1,
    Applet = 2,
    FileMgr = 3,
    File = 4,
    Display = 5,
    Bitmap = 7,
    /// Zeebo gamepad — a console extension, declared in its SDK.
    Hid = 8,
    HidDevice = 9,
    /// Signals: the mechanism by which BREW tells the app that something happened.
    Signal = 10,
    SignalCtl = 11,
    SignalCbFactory = 12,
    /// BREW's 2D API: lines, circles, polygons, viewport.
    Graphics = 13,
    /// BREW's basic sound: tones, vibration and volume.
    Sound = 14,
    /// License of the running module: type, expiration and purchase method.
    License = 15,
    /// Asynchronous stream over a block of memory — it is how BREW feeds the image
    /// decoders.
    MemAStream = 16,
    /// Image decoded from a stream (PNG, in the case of the console's games).
    Image = 17,
    /// BREW's cooperative thread: there is no preemption, the guest yields control by itself.
    Thread = 18,
    /// EGL — the bridge between OpenGL ES and the console's screen.
    Egl = 19,
    /// OpenGL ES 1.1, in the form BREW exposes.
    Gles = 20,
    /// Factory of media objects.
    MediaUtil = 21,
    /// Audio and video playback — on the console, the soundtrack's `.mp3` files.
    Media = 22,
    /// EGL in the old `AEEGL.h` form, without `this` and with a direct return.
    EglLegacy = 23,
    /// OpenGL ES 1.0 in the old `AEEGL.h` form.
    GlLegacy = 24,
    /// BREW's HTTP. The console is always offline here, but the object has to exist: the
    /// games create the three network ones in sequence and do not check the return.
    Web = 25,
    /// Cryptographic digest — `AEECLSID_MD5`.
    Hash = 26,
    /// Cipher factory, from `inc/AEEICipherFactory.h`.
    CipherFactory = 27,
    /// Block cipher, from `inc/AEEICipher1.h`.
    Cipher = 28,
    /// System memory, from `sdk/inc/AEEHeap.h`.
    Heap = 29,
    /// Stream that inflates deflate, from `sdk/inc/AEEUnzipStream.h`.
    UnzipStream = 30,
    /// Image decoder, from `inc/AEEIImageDecoder.h`. It is the standard interface of the decoding
    /// classes — `AEECLSID_PNGDecoderBREW` among them.
    ImageDecoder = 31,
    /// A decoder's data input, from `inc/AEEIForceFeed.h`. The game asks the decoder for this
    /// interface and writes the file into it, piece by piece.
    ForceFeed = 32,
    /// Scaling, rotation and transparency of the EGL surface, from `sdk/inc/AEEEGLSurfaceManip.h`.
    /// It is the console's `EGL_QUALCOMM_surface_scale`.
    EglSurfaceManip = 33,
    /// ATI Imageon's extras on top of OpenGL ES, from `sdk/inc/AEEGLESImageonEXT.h`.
    GlesImageonExt = 34,
    /// `AEEIID_GLES11EXT`: the OpenGL ES 1.1 OES extensions. See [`aee_slots::GLES11_EXT`].
    ///
    /// Prey Evil only draws if it exists: without the interface, it builds matrices and textures
    /// and stops — eleven GL methods in the report, no `Draw` and a black screen.
    Gles11Ext = 55,
    /// `AEEIID_GLES10EXT`: `QueryMatrixxOES`. See [`aee_slots::GLES10_EXT`].
    Gles10Ext = 56,
    /// `AEEIID_EGLGETPOWERLEVEL`: the device's power level. See
    /// [`aee_slots::EGL_GET_POWER_LEVEL`].
    EglGetPowerLevel = 57,
    /// `AEEIID_EGLOESSWAPINTERVAL`: the frame pacing, by interface. See
    /// [`aee_slots::EGL_OES_SWAP_INTERVAL`].
    EglOesSwapInterval = 58,
    /// `AEEIID_EGLGETCOLORBUFFER`: the EGL color buffer, by interface. See
    /// [`aee_slots::EGL_GET_COLOR_BUFFER`].
    EglGetColorBuffer = 59,
    /// `AEEIID_GLES11EXTPAK`: `TexGen`, separate blending and framebuffer objects. See
    /// [`aee_slots::GLES11_EXT_PAK`].
    Gles11ExtPak = 60,
    /// `AEECLSID_IJOYSTICK`: the USB joystick. See [`aee_slots::JOYSTICK`].
    Joystick = 61,
    /// The `0x01001039`: MD5 with the context in the game's memory. See [`aee_slots::HASH_CTX`].
    HashCtx = 62,
    /// `AEECLSID_SOUNDPLAYER`: BREW 1.x's file player, deprecated by `IMedia`. See
    /// [`aee_slots::SOUND_PLAYER`].
    SoundPlayer = 63,
    /// The console's `AEECLSID_SQLMGR`: opens SQLite databases. See [`crate::brew::sql`].
    SqlMgr = 36,
    /// A database opened by [`Interface::SqlMgr`].
    SqlDatabase = 37,
    /// The Z-Wheel's generic collection (`0x0100104f`): it stores items and is iterated.
    Collection = 38,
    /// `0x01001011` = **`AEECLSID_SOURCEUTIL`**, BREW's `ISource` factory.
    ///
    /// It used to be called `RootForm` here, and the name was wrong: it came from the Z-Wheel's
    /// `Could not create root form` message, which is actually about [`Interface::Widget`].
    /// The right identification came from two games that use it in ways that looked
    /// incompatible — and are not, once the slots have the names from `AEESource.h`:
    ///
    /// | slot | method | who uses it |
    /// |---|---|---|
    /// | 3 | `PeekSourceFromSource(po, ISource*, nMax, IPeek**)` | Z-Wheel, to read `tectoy.cfg` line by line |
    /// | 5 | `SourceFromMemory(po, pBuf, nSize, pfn, pUser, ISource**)` | Zeeboids, to wrap the POST body |
    /// | 6 | `SourceFromFile(po, IFile*, ISource**)` | Z-Wheel, over the freshly opened file |
    ///
    /// `SourceFromMemory` is what settles it: it has **six** parameters, and the output pointer
    /// is the second on the stack — exactly where Zeeboids' code read it, in a stretch we had
    /// named "send". It sends nothing: it wraps the body to hand it to `IWeb`. That is where the
    /// Zeeboids bridge takes the body from, and that is why it works.
    ///
    /// There are seven methods, and the firmware vtable at `0x10a785e4` also has seven.
    SourceUtil = 39,
    /// `0x01028e51`, the Z-Wheel's interface widget — the root form included.
    ///
    /// The class is not in the `1.1.2_APPS.bin` table, so there is no firmware vtable to copy.
    /// What is known about it came from the game's code, and it is little and clear: the only
    /// method used is **slot 3**, a generic accessor `slot3(this, selector, id, value)`. The game
    /// calls it through two wrappers, and both say what the selector is:
    ///
    /// - `0x3f72c(obj, id, output)` calls `slot3(obj, 0x800, id, output)` — **gets the child**
    ///   number `id` and writes the pointer to `output`.
    /// - `0x403c8(obj, value)` calls `slot3(obj, 0x801, 0x130, value)` — **sets** the property
    ///   `0x130`.
    ///
    /// The return is the opposite of BREW's: **non-zero is success**. Both wrappers do
    /// `cmp r0,#0; moveq r0,#3`, that is, they turn zero into `EBADCLASS`. Answering `SUCCESS`
    /// here — which is zero — means "failed", and that is exactly what made
    /// `tectoymain.c:1001` print `Could not create root form(20)` and then die on a null.
    ///
    /// **The inverted return applies only to the accessor.** Slot 2 is an ordinary
    /// `QueryInterface`, and there zero is success: at `0x11c58` the game does
    /// `movs r5,r0; bne <error>`. Mixing up the two conventions would be easy, and that is why
    /// they are written side by side here. Slot 12 follows the same convention as 2, and the same
    /// `(IID, &output)` form.
    Widget = 40,
    /// Basic control used by Zenonia (`AEECLSID 0x01003109`).
    Control = 51,
    /// Scaling and rotation of one bitmap over another, from `AEETransform.h` (`AEEIID_TRANSFORM`).
    ///
    /// It is how Zenonia presents the frame: it draws at 320x240 and asks the screen bitmap to
    /// scale the canvas to it.
    Transform = 52,
    /// A bitmap's "canvas": the interface `0x0101e443`, which the Z-Wheel asks of the bitmap
    /// where it draws a widget subtree. The only method used is slot 7, which hands over an
    /// `IDisplay` drawing on that bitmap. Name and number came from usage, not from a header.
    Canvas = 53,
    /// `0x01006c05`, the **ZEEBOMCP** — the single object the Z-Wheel asks for at every launch.
    ///
    /// The name comes from the game itself: `Tectoy.c` prints `Cannot create instance of ZEEBOMCP`
    /// when creation fails. In the firmware it is the singleton at `0x11085cb8`, which allocates
    /// eight bytes — vtable and count — and, if it already exists, only increments the count.
    ///
    /// The constructor is `0x11085cb8` and the vtable is `0x102d47a8`, with eight methods — in the
    /// same stretch that loads `fs:/card3` and `fs:/mcp/`, which is what an MCP would do.
    ///
    /// It was this class that revealed an error in the reading of the firmware's class table.
    /// `firmware.py` read the entries shifted by one word and returned, for each CLSID, the
    /// constructor of the **previous** entry — here, `LCT_SIMCardCtl_New`, which starts by
    /// comparing the received CLSID with `0x01006c01`. That comparison is what gave the shift
    /// away. The tool is fixed, and the table now points to the same constructor I had found by
    /// hand.
    ///
    /// The first three slots were read: count, count, and a `QueryInterface` that compares the
    /// IID with `0x01000001` and with `0x01006c05`. The remaining five are left unnamed on
    /// purpose — the Z-Wheel, so far, only creates and releases the object, and a call to one of
    /// them is something to show up in the report, not to be answered by guesswork.
    ZeeboMcp = 41,
    /// `0x01001027`, BREW's `IConfig` — the device's configuration items.
    ///
    /// The Z-Wheel's `Tectoy_SetLanguagePref` calls **slot 3** with `(0x3f, pointer, 4)`, which
    /// is the shape of the SDK's `ICONFIG_SetItem(pMe, nItem, pBuff, nSize)`; slot 2 is the
    /// matching `GetItem`. That is what is implemented: the items are stored by number, and
    /// whoever writes reads back what it wrote.
    ///
    /// **The firmware vtable did not work, and the reason changed after I understood it.** I had
    /// copied a table of twelve methods in which nine were `movs r0,#0x14; bx lr` — they return
    /// `EUNSUPPORTED` and nothing else, `SetItem` included —, and the result was the game
    /// trading `Unable to create instance of IConfig, error 20` for `Unable to set language to
    /// config, error 20`: the same twenty, one step further.
    ///
    /// That table belonged to another class: `firmware.py` read the entries shifted by one word.
    /// With the reading fixed, what is left for `0x01001027` is an entry with flags `0xffff0008`
    /// and a "constructor" whose vtable has a single method, which is not even an address — that
    /// is, a **false positive**: this class is not registered in this firmware.
    ///
    /// That is why the names come from the SDK, and only the four slots the Z-Wheel exercises.
    /// The eight above are left out: there is no source at all about them.
    Config = 42,
    /// A BREW `ISource`: bytes with a cursor. Created by [`Interface::SourceUtil`].
    Source = 43,
    /// A BREW `IPeek`: line-by-line reading over an [`Interface::Source`].
    ///
    /// Of `IPeek` we only know **slot 8**, because it is the only one the Z-Wheel calls: it passes
    /// the address of a `{pointer, size}` pair and the number 3, and expects to receive the next
    /// line. The others stay unnamed — a call to them needs to show up in the report.
    Peek = 44,
    /// `0x01028e35`, the Z-Wheel's generic list — what the game calls a "vector model".
    ///
    /// It is not in the firmware's class table, so the slots came from the game's code, and each
    /// one has two readings that agree: the `tectoy.cfg` loader at `0x88338` fills the list, and
    /// the loop at `0x7f164` iterates it.
    ///
    /// | slot | method | where it is read |
    /// |---|---|---|
    /// | 5 | size | `0x7f170`, and the result becomes the loop's bound |
    /// | 6 | get at | `0x7f190`, with `(index, &output)`; the game tests whether the text starts with `#` |
    /// | 7 | replace at | `0x38ec4`, with `(index, item)`, in an insertion sort |
    /// | 8 | insert at | `0x884f0`, with index `-1` — insert at the end |
    /// | 9 | remove at | `0x7d788`, with index `0`, in the loop that empties the list item by item |
    /// | 10 | clear | `0x80010`, once, right before `Release` |
    /// | 12 | set freer | `0x88458`, receiving a **function pointer from the module** |
    ///
    /// The unnamed slots were never called. Leaving them unnamed is what makes an unexpected call
    /// show up in the report instead of passing as implemented.
    Vector = 45,
    /// `0x01028e3c`: an **`IValueModel`** — a value and a list of listeners notified when it
    /// changes. It is the model that links the game grid to the action of opening the game.
    ///
    /// | slot | method | where it is read |
    /// |---|---|---|
    /// | 3 | `AddListener(pl)` | `0x8588c`: the game puts the function at `pl+8` and the context at `pl+0xc` **before** calling — BREW's 0x18-byte `ModelListener` |
    /// | 5 | `SetValue(pv, nLen, pfnFree)` | `0x20e08` clears it with three zeros |
    /// | 6 | `GetValue(&nLen)` | the listener `0x37704` checks event `0x1000` and gets the item through here, with `NULL` |
    ///
    /// This slot 3 was, for a long time, a "query" that zeroed 0x18 bytes, and the answer
    /// **erased the function and the context of every listener** the Z-Wheel registered.
    Class28e3c = 46,
    /// `0x01011810`, what the Z-Wheel calls **ICM** — BREW's call manager.
    ///
    /// `tectoymain.c:1037` gives up initialization if it cannot create it. What it wants from it
    /// is a single thing, and the code says what: `0x87c40` zeroes a buffer of `0x340` bytes,
    /// calls **slot 28** with `(buffer, 0x340)` and returns the word at `+0xc`. At `0x77564` the
    /// caller compares that word with **5**.
    ///
    /// Five is `SYS_OPRT_MODE_ONLINE` of Qualcomm's radio operating mode, and the field matches
    /// `oprt_mode` in `AEECMPhInfo`. That is: the question is "is the radio on the air?", and
    /// here the answer is yes. It is a hypothesis, and it is recorded as such in the report — but
    /// a hypothesis with two independent supports, the value and the position.
    Cm = 47,
    /// `0x01006c02`, the console's **system control** — the firmware's `OEM_LCTSystemCtl.c`.
    ///
    /// `tectoymain.c:1759` prints `ERROR: Failed to create system control` without it. What it
    /// controls is written in the strings next to the implementation: `Pipe OFF`,
    /// `Pipe Half Bright`, `Pipe Slow Pulsing`, `Button ON` — the device's lights — and an
    /// `LCT_SystemCtl_SystemControl nSystemMode=%d, nDownloadMode=%d`.
    ///
    /// The vtable is `0x10691ea8`, with seven methods, and the constructor `0x10e9f93e` checks the
    /// received CLSID against `0x01006c02` — it is this class's own. The first three are
    /// implemented; the ones that touch the lights stay as placeholders until someone calls them.
    ///
    /// Slot 6 is the exception, because the Z-Wheel calls it in a loop. In the firmware it is a
    /// three-instruction shell over a hardware call, with no argument at all; on the game's side,
    /// at `0x81768`, zero is "go on" and non-zero branches away. We answer zero — the device we do
    /// not have has nothing to complain about.
    SystemCtl = 48,
    /// `0x01035156`, the console's TrueType font.
    ///
    /// The name comes from the message `tectoymain.c:1269` prints when creation fails:
    /// `Unable to create instance of TrueType TYPEFACE`. It is not in this firmware's class
    /// table.
    ///
    /// Like [`Interface::Class28e3c`], the Z-Wheel creates and keeps it — at `+0x34b8` — and so
    /// far calls no method at all. Here that is less surprising than it looks: the emulator
    /// already draws text with the `tectoy.ttf` the package itself carries, so the rendering path
    /// does not go through this object.
    Typeface = 49,
    /// `IFont` — the system bitmap font, created by the `AEECLSID_FONT_*` classes.
    ///
    /// See [`crate::brew::aee_slots::FONT`] and [`crate::machine::font`].
    Font = 54,
    /// `0x01006c01`, `LCT_SIMCardCtl` — the console's SIM card control.
    ///
    /// **It is implemented and not offered, and the reason is the game.** `0x78544` creates this
    /// class to ask for the card check; when creation **fails**, it sets the state to `0x27` —
    /// and `0x27` is precisely what `0x82464` forwards to the transition that opens the main
    /// menu. Refusing **is** the path, for a reason that took three rounds to become clear.
    ///
    /// `0x78544` distinguishes three outcomes, and `0x82464` does different things with each:
    ///
    /// - `CreateInstance` fails → state `0x27`. `0x82464` calls `0x1f7b4`, which is the routine
    ///   that **advances the interface**: it reaches `0x7ed10`, the launch of the z-pad
    ///   instructions form. It is the only one of the three that moves the game forward.
    /// - Slot 3 returns zero → state `0x28`. `0x82464` **does nothing** with it: it compares,
    ///   branches to the end and returns. The screen stays where it is, silent.
    /// - Slot 3 returns non-zero → state `1`, and the game shows `Showing SIM Error dialog`.
    ///
    /// The repeated `Unable to create instance of AEECLSID_LCT_SIMCARDCTL` is the game taking the
    /// right path many times, not an error to silence. Offering the class silences the message
    /// and, along with it, the progress: that is how the welcome screen got stuck.
    ///
    /// There is also the leak, if this is ever offered: `0x785a8` returns without `Release` and
    /// the pointer dies on the stack. On the console, what releases the object is the answer of
    /// the asynchronous check. Without releasing, the game created one control per loop
    /// iteration and exhausted the pool at 65,506 objects, taking the z-pad form down with it.
    ///
    /// The vtable is `0x113cf854`, with four methods, and the constructor `0x11267d04` checks the
    /// received CLSID against `0x01006c01` — it is this class's own. It was this constructor, by
    /// the way, that gave away the shifted reading of the firmware's class table.
    ///
    /// Slot 3 receives `(this, text, applet)` and, in the firmware, stores a pair at `+0xc4` and
    /// `+0xc8`: it is a callback registration, of the "check the card and let me know" kind.
    SimCardCtl = 50,
    /// Object of a class we do not know yet, created at the request of `--probe`.
    ///
    /// It implements no interface at all: it exists to **find out which one it is**. Every call is
    /// logged with the slot and the arguments, and answers `SUCCESS`, so that the game goes as far
    /// as it can and shows what it expects from the object. That is how the console's `IHID` was
    /// identified, by hand; this is the same idea with a tool.
    Probe = 35,
    /// Table of BREW stdlib functions (`MALLOC`, `STRLEN`, …), which dynamic modules access
    /// through a pointer handed over by the loader — not through an object vtable.
    Helpers = 6,
}

impl Interface {
    /// All interfaces, in the exact order of the enum values.
    ///
    /// The vtable table is built from this list and addressed by the enum value, so the two have
    /// to agree — hence the list exists in one place only, with a test that checks the
    /// correspondence. When they diverged, an object received another interface's vtable and the
    /// call ended up in the wrong method, with the symptom miles away from the cause.
    pub const ALL: [Interface; 64] = [
        Self::Shell,
        Self::Module,
        Self::Applet,
        Self::FileMgr,
        Self::File,
        Self::Display,
        Self::Helpers,
        Self::Bitmap,
        Self::Hid,
        Self::HidDevice,
        Self::Signal,
        Self::SignalCtl,
        Self::SignalCbFactory,
        Self::Graphics,
        Self::Sound,
        Self::License,
        Self::MemAStream,
        Self::Image,
        Self::Thread,
        Self::Egl,
        Self::Gles,
        Self::MediaUtil,
        Self::Media,
        Self::EglLegacy,
        Self::GlLegacy,
        Self::Web,
        Self::Hash,
        Self::CipherFactory,
        Self::Cipher,
        Self::Heap,
        Self::UnzipStream,
        Self::ImageDecoder,
        Self::ForceFeed,
        Self::EglSurfaceManip,
        Self::GlesImageonExt,
        Self::Probe,
        Self::SqlMgr,
        Self::SqlDatabase,
        Self::Collection,
        Self::SourceUtil,
        Self::Widget,
        Self::ZeeboMcp,
        Self::Config,
        Self::Source,
        Self::Peek,
        Self::Vector,
        Self::Class28e3c,
        Self::Cm,
        Self::SystemCtl,
        Self::Typeface,
        Self::SimCardCtl,
        Self::Control,
        Self::Transform,
        Self::Canvas,
        Self::Font,
        Self::Gles11Ext,
        Self::Gles10Ext,
        Self::EglGetPowerLevel,
        Self::EglOesSwapInterval,
        Self::EglGetColorBuffer,
        Self::Gles11ExtPak,
        Self::Joystick,
        Self::HashCtx,
        Self::SoundPlayer,
    ];

    /// Name used in the logs — it matches the SDK's naming.
    pub fn name(self) -> &'static str {
        match self {
            Self::Shell => "IShell",
            Self::Module => "IModule",
            Self::Applet => "IApplet",
            Self::FileMgr => "IFileMgr",
            Self::File => "IFile",
            Self::Display => "IDisplay",
            Self::Bitmap => "IBitmap",
            Self::Hid => "IHID",
            Self::HidDevice => "IHIDDevice",
            Self::Signal => "ISignal",
            Self::SignalCtl => "ISignalCtl",
            Self::SignalCbFactory => "ISignalCBFactory",
            Self::Graphics => "IGraphics",
            Self::Sound => "ISound",
            Self::License => "ILicense",
            Self::MemAStream => "IMemAStream",
            Self::Image => "IImage",
            Self::Thread => "IThread",
            Self::Egl => "IEGL11",
            Self::Gles => "IGLES11",
            Self::MediaUtil => "IMediaUtil",
            Self::Media => "IMedia",
            Self::EglLegacy => "IEGL",
            Self::GlLegacy => "IGL",
            Self::Web => "IWeb",
            Self::Hash => "IHash",
            Self::CipherFactory => "ICipherFactory",
            Self::Cipher => "ICipher1",
            Self::Heap => "IHeap",
            Self::UnzipStream => "IUnzipAStream",
            Self::ImageDecoder => "IImageDecoder",
            Self::ForceFeed => "IForceFeed",
            Self::EglSurfaceManip => "IEGLSurfaceManip",
            Self::GlesImageonExt => "IGLESImageonExt",
            Self::Gles11Ext => "IGLES11Ext",
            Self::Gles10Ext => "IGLES10Ext",
            Self::EglGetPowerLevel => "IEGLGetPowerLevel",
            Self::EglOesSwapInterval => "IEGLOESSwapInterval",
            Self::EglGetColorBuffer => "IEGLGetColorBuffer",
            Self::Gles11ExtPak => "IGLES11ExtPak",
            Self::Joystick => "IJoystick",
            Self::HashCtx => "IHashCTX",
            Self::SoundPlayer => "ISoundPlayer",
            Self::SqlMgr => "ISQLMgr",
            Self::SqlDatabase => "ISQLDatabase",
            Self::Collection => "IColecao",
            Self::SourceUtil => "ISourceUtil",
            Self::Widget => "IWidget",
            Self::Control => "IControl",
            Self::Transform => "ITransform",
            Self::Canvas => "ICanvas",
            Self::ZeeboMcp => "IZeeboMCP",
            Self::Config => "IConfig",
            Self::Source => "ISource",
            Self::Peek => "IPeek",
            Self::Vector => "IVetor",
            Self::Class28e3c => "I28e3c",
            Self::Cm => "ICM",
            Self::SystemCtl => "ILCTSystemCtl",
            Self::Typeface => "ITypeface",
            Self::Font => "IFont",
            Self::SimCardCtl => "ILCTSimCardCtl",
            Self::Probe => "ClasseDesconhecida",
            Self::Helpers => "AEEHelpers",
        }
    }

    /// Names of this interface's methods, in vtable order.
    fn slot_names(self) -> &'static [&'static str] {
        match self {
            Self::Shell => aee_slots::SHELL,
            Self::Module => aee_slots::MODULE,
            Self::Applet => aee_slots::APPLET,
            Self::FileMgr => aee_slots::FILEMGR,
            Self::File => aee_slots::FILE,
            Self::Display => aee_slots::DISPLAY,
            Self::Bitmap => aee_slots::BITMAP,
            Self::Hid => aee_slots::HID,
            Self::HidDevice => aee_slots::HIDDEVICE,
            Self::Signal => aee_slots::SIGNAL,
            Self::SignalCtl => aee_slots::SIGNALCTL,
            Self::SignalCbFactory => aee_slots::SIGNALCBFACTORY,
            Self::Graphics => aee_slots::GRAPHICS,
            Self::Sound => aee_slots::SOUND,
            Self::License => aee_slots::LICENSE,
            Self::MemAStream => aee_slots::MEMASTREAM,
            Self::Image => aee_slots::IMAGE,
            Self::Thread => aee_slots::THREAD,
            Self::Egl => aee_slots::EGL,
            Self::Gles => aee_slots::GLES,
            Self::MediaUtil => aee_slots::MEDIAUTIL,
            Self::Media => aee_slots::MEDIA,
            Self::EglLegacy => aee_slots::EGL_LEGACY,
            Self::GlLegacy => aee_slots::GL_LEGACY,
            Self::Web => aee_slots::WEB,
            Self::Hash => aee_slots::HASH,
            Self::CipherFactory => aee_slots::CIPHER_FACTORY,
            Self::Cipher => aee_slots::CIPHER,
            Self::Heap => aee_slots::HEAP,
            Self::UnzipStream => aee_slots::UNZIP_STREAM,
            Self::ImageDecoder => aee_slots::IMAGE_DECODER,
            Self::ForceFeed => aee_slots::FORCE_FEED,
            Self::EglSurfaceManip => aee_slots::EGL_SURFACE_MANIP,
            Self::GlesImageonExt => aee_slots::GLES_IMAGEON_EXT,
            Self::Gles11Ext => aee_slots::GLES11_EXT,
            Self::Gles10Ext => aee_slots::GLES10_EXT,
            Self::EglGetPowerLevel => aee_slots::EGL_GET_POWER_LEVEL,
            Self::EglOesSwapInterval => aee_slots::EGL_OES_SWAP_INTERVAL,
            Self::EglGetColorBuffer => aee_slots::EGL_GET_COLOR_BUFFER,
            Self::Gles11ExtPak => aee_slots::GLES11_EXT_PAK,
            Self::Joystick => aee_slots::JOYSTICK,
            Self::HashCtx => aee_slots::HASH_CTX,
            Self::SoundPlayer => aee_slots::SOUND_PLAYER,
            Self::SqlMgr => aee_slots::SQL_MGR,
            Self::SqlDatabase => aee_slots::SQL_DATABASE,
            Self::Collection => aee_slots::COLLECTION,
            Self::SourceUtil => aee_slots::SOURCE_UTIL,
            Self::Widget => aee_slots::WIDGET,
            Self::Control => aee_slots::CONTROL,
            Self::Transform => aee_slots::TRANSFORM,
            Self::Canvas => aee_slots::CANVAS,
            Self::ZeeboMcp => aee_slots::ZEEBO_MCP,
            Self::Config => aee_slots::CONFIG,
            Self::Source => aee_slots::SOURCE,
            Self::Peek => aee_slots::PEEK,
            Self::Vector => aee_slots::VECTOR,
            Self::Class28e3c => aee_slots::CLASS_28E3C,
            Self::Cm => aee_slots::CM,
            Self::SystemCtl => aee_slots::SYSTEM_CTL,
            Self::Typeface => aee_slots::TYPEFACE,
            Self::Font => aee_slots::FONT,
            Self::SimCardCtl => aee_slots::SIM_CARD_CTL,
            // The probe has no table: `method` answers for it before getting here.
            Self::Probe => &[],
            Self::Helpers => aee_helpers::HELPERS,
        }
    }

    /// Name of the method in a slot, when known.
    pub fn method(self, slot: u32) -> Option<&'static str> {
        // The probe accepts any slot: what matters about it is the number, not the name, and
        // refusing would make the game stop exactly at what we want to observe.
        if matches!(self, Self::Probe) {
            return Some("sonda");
        }
        self.slot_names().get(slot as usize).copied()
    }

    /// The same as `from_index`, for whoever only has the stored number — the API profile.
    pub fn from_index_public(index: u32) -> Option<Self> {
        Self::from_index(index)
    }

    fn from_index(index: u32) -> Option<Self> {
        Some(match index {
            0 => Self::Shell,
            1 => Self::Module,
            2 => Self::Applet,
            3 => Self::FileMgr,
            4 => Self::File,
            5 => Self::Display,
            7 => Self::Bitmap,
            8 => Self::Hid,
            9 => Self::HidDevice,
            10 => Self::Signal,
            11 => Self::SignalCtl,
            12 => Self::SignalCbFactory,
            13 => Self::Graphics,
            14 => Self::Sound,
            15 => Self::License,
            16 => Self::MemAStream,
            17 => Self::Image,
            18 => Self::Thread,
            19 => Self::Egl,
            20 => Self::Gles,
            21 => Self::MediaUtil,
            22 => Self::Media,
            23 => Self::EglLegacy,
            24 => Self::GlLegacy,
            25 => Self::Web,
            26 => Self::Hash,
            27 => Self::CipherFactory,
            28 => Self::Cipher,
            29 => Self::Heap,
            30 => Self::UnzipStream,
            31 => Self::ImageDecoder,
            32 => Self::ForceFeed,
            33 => Self::EglSurfaceManip,
            34 => Self::GlesImageonExt,
            55 => Self::Gles11Ext,
            56 => Self::Gles10Ext,
            57 => Self::EglGetPowerLevel,
            58 => Self::EglOesSwapInterval,
            59 => Self::EglGetColorBuffer,
            60 => Self::Gles11ExtPak,
            61 => Self::Joystick,
            62 => Self::HashCtx,
            63 => Self::SoundPlayer,
            35 => Self::Probe,
            36 => Self::SqlMgr,
            37 => Self::SqlDatabase,
            38 => Self::Collection,
            39 => Self::SourceUtil,
            40 => Self::Widget,
            41 => Self::ZeeboMcp,
            42 => Self::Config,
            43 => Self::Source,
            44 => Self::Peek,
            45 => Self::Vector,
            46 => Self::Class28e3c,
            47 => Self::Cm,
            48 => Self::SystemCtl,
            49 => Self::Typeface,
            54 => Self::Font,
            50 => Self::SimCardCtl,
            51 => Self::Control,
            52 => Self::Transform,
            53 => Self::Canvas,
            6 => Self::Helpers,
            _ => return None,
        })
    }
}

/// Whether a slot name is a placeholder — `slot7` and the like.
///
/// The table needs these placeholders when a slot **higher up** is known: without them, slot 28
/// of `ICM` would have no way of sitting at index 28. But a placeholder is not an implementation,
/// and answering it with success would be precisely the lie these tables exist to avoid. The
/// dispatcher uses this to refuse, and then the call shows up in the report with the slot number.
pub fn is_placeholder(name: &str) -> bool {
    name.strip_prefix("slot")
        .is_some_and(|n| n.parse::<u32>().is_ok())
}

/// Trampoline address of a method.
pub fn encode(iface: Interface, slot: u32) -> u32 {
    API_BASE + (iface as u32) * IFACE_STRIDE + slot * 4
}

/// Version of [`encode`] that accepts the interface's numeric index, for whoever stored only the
/// number (the call log, for example).
pub fn encode_raw(iface_index: u32, slot: u32) -> u32 {
    API_BASE + iface_index * IFACE_STRIDE + slot * 4
}

/// Inverts [`encode`]. Returns `None` if the address does not match a known interface.
pub fn decode(addr: u32) -> Option<(Interface, u32)> {
    let offset = addr.checked_sub(API_BASE)?;
    let iface = Interface::from_index(offset / IFACE_STRIDE)?;
    Some((iface, (offset % IFACE_STRIDE) / 4))
}

/// Readable description of a call not implemented yet, for the log that becomes our backlog.
pub fn describe(addr: u32) -> String {
    match decode(addr) {
        Some((iface, slot)) => match iface.method(slot) {
            Some(name) => format!("{}::{name}", iface.name()),
            None => format!("{}::slot[{slot}]", iface.name()),
        },
        None => format!("unknown API address {addr:#010x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Every interface the trampoline knows has a vtable.** The eight that came after `Canvas`
    /// went into `from_index` and not here: their vtable fell past the end of the region, on a
    /// zeroed page, and the object handed to the game had all its methods at zero. Ridge Racer,
    /// which used to get "unsupported class" from `IGLES11Ext` and carry on without it, started
    /// jumping to address zero when entering the menu.
    #[test]
    fn every_trampoline_interface_is_in_the_vtable_list() {
        for index in 0..=u8::MAX as u32 {
            if let Some(iface) = Interface::from_index(index) {
                assert!(
                    Interface::ALL.contains(&iface),
                    "{} (index {index}) is not in Interface::ALL and has no vtable",
                    iface.name()
                );
            }
        }
    }

    #[test]
    fn encodes_and_decodes_round_trip() {
        for (iface, slot) in [
            (Interface::Shell, 0),
            (Interface::FileMgr, 2),
            (Interface::File, 31),
        ] {
            assert_eq!(decode(encode(iface, slot)), Some((iface, slot)));
        }
    }

    #[test]
    fn different_interfaces_do_not_collide() {
        assert_ne!(encode(Interface::Shell, 1023), encode(Interface::Module, 0));
    }

    #[test]
    fn an_address_outside_known_interfaces_does_not_decode() {
        assert_eq!(decode(API_BASE + 99 * IFACE_STRIDE), None);
        assert_eq!(decode(0x1000), None);
    }

    #[test]
    fn describes_a_call_with_the_sdk_name() {
        assert_eq!(
            describe(encode(Interface::FileMgr, 2)),
            "IFileMgr::OpenFile"
        );
        assert_eq!(
            describe(encode(Interface::Shell, 2)),
            "IShell::CreateInstance"
        );
    }

    #[test]
    fn a_slot_without_a_known_name_shows_by_number() {
        assert_eq!(describe(encode(Interface::File, 99)), "IFile::slot[99]");
    }

    #[test]
    fn the_slot_order_matches_the_sdk() {
        // Checked against the INHERIT_* macros of BREW SDK 4.0.2.
        assert_eq!(Interface::Module.method(2), Some("CreateInstance"));
        assert_eq!(Interface::Applet.method(2), Some("HandleEvent"));
        assert_eq!(Interface::Display.method(7), Some("Update"));
    }
}
