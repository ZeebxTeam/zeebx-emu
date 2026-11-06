# 02 — The platform: Qualcomm BREW 4.0.2

The Zeebo runs **BREW 4.0.2** with customizations by Zeebo Inc./TecToy. Understanding BREW is 90%
of the work of an emulator.

## Execution model

BREW is a **COM-like environment in C** on top of the AEE (*Application Execution Environment*):

- Every API is an interface with a **vtable**: `IShell`, `IDisplay`, `IGraphics`, `IBitmap`,
  `IFileMgr`, `IFile`, `ISound`, `ISoundPlayer`, `IMedia`, `IHID`, `IHIDDevice`, `ISignal`,
  `IHeap`, `IDisplay`, `IImage`, `IFont`, `IWeb`, `INetMgr`…
- The application is a relocatable DLL (`.mod`) whose **first symbol in `.text` is `AEEMod_Load()`**.
  The AEE loads the module and calls `AEEMod_Load`, which registers the module's `CreateInstance`.
- Objects are born through `ISHELL_CreateInstance(shell, ClassID, &obj)` — the ClassID is a 32-bit
  number (e.g. Media Player = `0x01010EF6`). The project's `.bid` declares its own ClassIDs.
- **There is no `main()` and no loop of its own**: the app is *event-driven*. `HandleEvent(app, eCode, wParam, dwParam)`
  receives `EVT_APP_START`, `EVT_APP_STOP`, `EVT_APP_SUSPEND/RESUME`, `EVT_KEY`, `EVT_USER`…
  Continuous work is done by re-scheduling callbacks (`ISHELL_SetTimer`, `AEECallback`) — the app
  **hands control back to the AEE** every frame. That is great for the emulator: the "frame loop"
  lives on the host.
- No libc with I/O or heap: allocation is the AEE's `MALLOC`/`FREE`, files are `IFileMgr`/`IFile`,
  there is no `stdio`. C++ global constructors do not run automatically.
- Cooperative/single-threaded in the app core; the documentation (ch. 4.5 of the Developer Guide)
  discourages real multitasking.

Practical consequence: **an HLE emulator only needs an ARM interpreter + an implementation of the
vtables**. The game has no direct access to hardware registers — if it does, it is an exception.

## What is specific to the Zeebo

From the Developer Guide (chs. 6 and 7):

- **`IHID` / `IHIDDevice` + `ISignal`**: USB gamepads. Button and axis events, connect/disconnect,
  rumble, exclusive access, remapping. Two simultaneous controllers.
- **OpenGL ES 1.0+ Common Profile via a BREW extension** (`OpenGLES_Extension_1.5.3_For_BREW_SDK_4.x.x`):
  the app creates an EGL context on top of a BREW `IDisplay`/`IBitmap` and from then on uses
  standard `gl*`. Relevant extensions: ATITC, matrix palette, point sprites, `glDrawTexture`.
- **Fixed-point math**: a good share of the games use `GLfixed` — the implementation has to be
  correct in fixed point, not just convert to float.
- Audio: MIDI/wavetable, MP3, PCM, ADPCM, CMX, QCELP through `ISound`/`ISoundPlayer`/`IMedia`.
- Screen: 640×480 RGB565, composite output (the TV encoder is documented in `research/tvenc/`).

## API documentation available locally

| File | What it is for |
|---|---|
| `vendor/tripleoxygen/doc/BREWOemAPIReferenceforMSM.pdf` | **OEM API reference** — the closest thing to a spec of the interfaces |
| `vendor/tripleoxygen/doc/ZeeboDeveloperGuide0.97.pdf` | Official Zeebo guide (IHID, GL ES, memory, events) |
| `vendor/tripleoxygen/doc/503611776Inside_BREW.rar` | The book "Inside BREW" |
| `vendor/tripleoxygen/doc/11715848BREWProgrammingguide.rar` | BREW Programming Guide |
| `vendor/tripleoxygen/doc/BREW_and_OpenGLES-Astle.ppt`, `Tech-303_Ligon.pdf`, `TECH-606.pdf` | BREW/OpenGL ES talks |
| `vendor/tripleoxygen/doc/Consideracoes_sobre_o_QDSP5000_e_o_BREW_(v0.1).pdf` | QDSP5000 + BREW (audio), in Portuguese |
| `vendor/sdk/samples/unpacked/MSM7500_OGLES_*` | Official C samples, with `.mif`/`.bid`/`.mak` |
| `vendor/tripleoxygen/brew/OpenGL_ES_extension_1.0.3.zip` | Headers of the GL ES extension for BREW |
| `vendor/repos/zeebo_doom/src` | A real port (PrBoom) — shows the API subset a game actually uses |
| `vendor/repos/openzeebo/brew/{intro,backup}` | Minimal complete BREW apps (`.c`, `.mif`, `.bar`, `.sig`, `.bid`) |
| `vendor/sdk/zeebo-sdk-named/` | **Zeebo HID extension**: `AEEIHID.h`, `AEEIHIDDevice.h`, `AEEHIDButtons.{c,h}` (Tectoy), `AEEHIDThumbsticks.{c,h}` |
| `vendor/sdk/qxengine-named/` | **QXEngine** headers and libs + the original `AEEModGen.c`/`AEEAppGen.c` |
| **`vendor/sdk-brew/brew-402/BREW 4.0.2 SP19/sdk/inc/`** | **BREW SDK 4.0.2 headers — the Zeebo's exact version.** The canonical source of the vtables |
| `vendor/sdk-brew/brew-402/…/documentation/API Reference/` | API reference in HTML |
| `vendor/sdk-brew/toolset/…/elf2mod.exe` + `elf2mod.x` | The `.mod` generator and the official linker script |

### What the extracted headers provide

- `AEEIHID.h` (340 lines) and `AEEIHIDDevice.h` (1,010 lines) are the **Zeebo input API in
  full** — device enumeration, button/axis events, rumble, exclusive access.
  You can write the `IHID`/`IHIDDevice` vtable slot by slot straight from them.
- `AEEModGen.c`/`AEEAppGen.c` show exactly what the AEE expects from a module: `AEEMod_Load()`,
  `IModule` (`CreateInstance`, `FreeResources`), `AEEClsCreateInstance()`, and the base
  `AEEApplet` with `HandleEvent`. That is the contract the emulator's loader has to honor.
- QXEngine comes as an **ARM `.a`** — disassemblable, and it shows which BREW APIs the engine
  consumes.

## Real firmware vtables (an important find)

`vendor/tripleoxygen/research/brew/IFILEMGR_VTBL_Z200.txt` and `vtbl.ods` contain the **dump of
the `IFileMgr` vtable** extracted from the Zeebo firmware (Z200), mapping each slot to the address
of the function in the APPS image and to the API prototype:

```
offset 0x00 -> 0x101e66ad  IFILEMGR_AddRef
offset 0x08 -> 0x10d4c26f  IFILEMGR_OpenFile
offset 0x50 -> 0x105cc5c1  IFILEMGR_GetFreeSpaceEx
...
```

That is the answer key for (a) validating the slot order of each interface in your
reimplementation and (b) disassembling the original implementation in
`dump/nand/1.1.2/partitions/1.1.2_APPS.bin` when an API's behavior is ambiguous. **Slot order is
ABI**: getting one slot wrong breaks everything.
