# 14 — The Z-Wheel and the EFS2: where we stopped

This document exists so that the next round of work on the Z-Wheel starts from the right point,
and not from the beginning. It records what is done, what is measured, and which single link is
missing.

## What the Z-Wheel is

It is not a game: it is the **console's store application** — catalog, download queue, points
and telemetry. The App ID is 274755 and the applet's ClassID is `0x01070798`. The package carries
the server in plain text, in `tectoy.cfg`:

```
credit_server_url=https://aquila.tectoy.com.br:8443/WSM/wsm?wsdl
```

That makes it the natural target for a private server: pointing the line somewhere else is
enough, with no runtime patch. And it works **offline** on the console — the server was only for
buying and downloading.

## How far it gets today

It creates the applet, runs `EVT_APP_START`, sets up the preferences database, opens the catalog
databases and reaches:

```
tectoy_prefsDB.c:605   Unable to create vector model PREFSDB_GetRecords
tectoymain.c:338       Failed to load config lines.
```

Both depend on the same missing class, `0x01028e35`.

The `Could not create root form(20)` that used to stop everything **is gone**. The `20` was
`EUNSUPPORTED`, and the refused class was `0x01028e51` — not `0x01001011`. See the section on the
inverted return.

What was achieved along the way, and is in the emulator: `AEECLSID_SQLMGR` on top of real SQLite
([`brew/sql.rs`](https://github.com/ZeebxTeam/zeebx-core/blob/main/src/brew/sql.rs)), the generic collection `0x0100104f`, `ISHELL_SendEvent`,
`GETJULIANDATE`, `IDisplay::Clone`, and `DrawText` drawing with the `tectoy.ttf` that the app
packages itself.

## The missing classes, and where they are

| Class | What it is | How it was identified |
|---|---|---|
| `0x01001011` | root form | the error message the app prints when it is refused |
| `0x0100104f` | generic collection | **implemented** |
| `0x01035156` | TrueType font | sits between `Unable to create instance of TrueType TYPEFACE` and `TrueType Dictionary` in the literal pool |
| `0x01028e51` | interface widget | **implemented** — it is the one `tectoymain.c:1001` asks for, and slot 3 has to answer **true**: the wrapper at `0x3f72c` turns a boolean into an error, with `moveq r0, #3` |
| `0x01006c05` | ZEEBOMCP | **implemented** — the name comes from `Tectoy.c`, which prints `Cannot create instance of ZEEBOMCP` |
| `0x01001027` | `IConfig` | **implemented** — `GetItem`/`SetItem` |
| `0x01028e35`, `0x01028e3c`, `0x01011810` | unnamed | what is missing today |

They **are not BREW API that we need to write**. They are extensions the console loaded —
`widgets.mod`, `forms.mod`, `framewidget.mod`, `imenu.mod`, `icontrols.mod` —, and in this
firmware they are compiled into `1.1.2_APPS.bin`: `0x01001011` shows up 26 times there and none
in the file system.

## The inverted return of `0x01028e51`

The message `Could not create root form` is not referenced by a literal, but by `ADR` — searching
for its pointer in the module finds nothing. Scanning the `ADD rD, pc, #imm`, `tectoymain.c:1001`
comes out at `0x7c478`, and the `ISHELL_CreateInstance` just above it loads the literal at
`0x7c69c`: `0x01028e51`.

The class is not in the firmware's table, but the game teaches what it needs from it. The only
method used is **slot 3**, a generic accessor called by two wrappers that fix the selector:
`0x3f72c` passes `0x800` (get child number `id`) and `0x403c8` passes `0x801` (write the
property).

The detail that decided everything is the **return**: `cmp r0,#0; moveq r0,#3`. Zero becomes an
error. Answering `SUCCESS`, which is zero, was answering "failed" — and `0x78acc` does not handle
that failure: it jumps to the cleanup and dereferences the second child, which was never filled
in. That is where the invalid access to zero at `0x78b70` came from.

## The class table was being read shifted by one word

This one cost hours, and it is worth telling properly because the lesson is the opposite of the
one I wrote first.

`firmware.py` read the class table entries as `{constructor, CLSID, flags, zero}`. The true order
is `{CLSID, flags, zero, constructor}` — the constructor comes **after**. Reading shifted, each
CLSID got the constructor of the previous entry.

The error broke nothing visibly: it returned a real constructor, pointing to a real vtable, of the
wrong class. Twice I concluded the table was "a façade" when the problem was mine:

- For `0x01006c05` it gave `LCT_SIMCardCtl_New`, which starts by comparing the CLSID it receives
  with `0x01006c01`. **That comparison is what gave the shift away**: a constructor that refuses
  its own class is a sign that it is not that class's.
- For `0x01001027` it gave a twelve-method vtable in which nine were `movs r0,#0x14`. Copied, it
  made the game swap `Unable to create instance of IConfig` for `Unable to set language to
  config` — the same error 20, one step further.

With the reading fixed, the table points to constructor `0x11085cb8` for `0x01006c05`, which is
exactly the singleton I had found by hand through `fs:/mcp/`. And `0x01001027` no longer has an
entry: what is left for it is a false positive, with flags `0xffff0008` and a "constructor" whose
vtable has a method that is not even an address.

**The rule that stays**: disassemble the constructor before copying the vtable. If it tests a
CLSID, it has to be the one you asked for.

## The event that arrives before the start

The Z-Wheel printed `SendEvent to get PrefsDB failed` eleven times and gave up on the whole
configuration. The event is `0x7b0a`, which it sends **to its own class** asking for the
preferences database object; the answer comes back written into `dwParam`.

The handler was never called. `send_applet_event` read `current_applet`, and `current_applet` is
only filled in when `EVT_APP_START` is dispatched — but this event happens **before that**, during
the construction of the applet, inside `CreateInstance` itself. At that moment the object already
exists: `AEEApplet_New` wrote the output pointer before the game's code ran. For the shell, the
applet starts to exist when it is **registered**, not when it is started.

This is not a Z-Wheel hack: any game that sent an event to itself during construction was being
ignored.

## The Z-Wheel's data model

With the PrefsDB up, the game opens the databases and the whole schema shows up in the module's
strings. It is the catalog the interface shows:

```sql
CREATE TABLE GAMEINFO(game_id INTEGER PRIMARY KEY, class_id INTEGER, playcount INTEGER,
                      dt_download INTEGER, dt_lastplayed INTEGER, boxart_path TEXT,
                      flags INTEGER, size INTEGER, unique(game_id, class_id))
CREATE TABLE TITLETEXT(game_id INTEGER, lang_id INTEGER, titletext TEXT, unique(game_id, lang_id))
CREATE TABLE ASSETS(owner INTEGER, dslid INTEGER PRIMARY KEY, type INTEGER, version INTEGER,
                    path TEXT, language INTEGER, title TEXT, startdate INTEGER, enddate INTEGER)
CREATE TABLE DLITEMINFO(item_id INTEGER PRIMARY KEY, price INTEGER, size INTEGER,
                        titletext TEXT, boxart_path TEXT, flags INTEGER, upgrade_id INTEGER)
CREATE TABLE PREFSINFO(name TEXT PRIMARY KEY, strValue TEXT, dwValue INTEGER, flags INTEGER)
```

## Where the Z-Wheel is now

No unknown class at all. It sets up the `PrefsDB`, opens fourteen databases, reads the seventy
lines of `tectoy.cfg`, connects the joystick through `IHID`, registers for position reading,
creates the signals and starts building the interface.

The classes that went in, and where each name came from:

| Class | What it is | Source |
|---|---|---|
| `0x01001011` | `ISourceUtil` | `AEESource.h` — see below |
| `0x01001027` | `IConfig` | signature of the SDK's `ICONFIG_SetItem` |
| `0x01006c02` | system control | `OEM_LCTSystemCtl.c`, in the firmware strings |
| `0x01006c05` | ZEEBOMCP | the game's own message |
| `0x01011810` | `ICM` | `SYS_OPRT_MODE_ONLINE` and the field position |
| `0x01028e35` | generic list | game code, two readings that agree |
| `0x01028e3c` | created and stored | — |
| `0x01035156` | TrueType font | the game's own message |
| `0x01028e19/2a/3f/47/51` | widget family | a single accessor, measured |

## `0x01001011` was not the root form: it is `ISourceUtil`

Two games used it in ways that looked incompatible. With the names from `AEESource.h`, the three
uses fit on the first try:

```
slot 3  PeekSourceFromSource(po, ISource*, nMax, IPeek**)          Z-Wheel, reads tectoy.cfg
slot 5  SourceFromMemory(po, pBuf, nSize, pfn, pUser, ISource**)   Zeeboids, wraps the POST
slot 6  SourceFromFile(po, IFile*, ISource**)                      Z-Wheel
```

`SourceFromMemory` is what closes the case: **six** parameters, with the output pointer in the
second stack position — exactly where Zeeboids read it, in a stretch we had named "send". It does
not send anything; it wraps the POST body to hand to `IWeb`. That is where the bridge picks up the
body, and that is why it always worked.

## The event the Z-Wheel sends to itself before it exists

It printed `SendEvent to get PrefsDB failed` eleven times and gave up on the configuration. The
event is `0x7b0a`, sent **to its own class**, and the handler was never called: `current_applet`
is only filled in when `EVT_APP_START` is dispatched, and this event happens before, during the
construction of the applet. At that moment the object already exists — `AEEApplet_New` wrote the
output pointer before the game's code ran. For the shell, the applet starts to exist when it is
**registered**, not when it is started.

It is not a Z-Wheel hack: any game that sent an event to itself during construction was being
ignored.

## What is missing for the menu, and what has already been tried

The intro shows up. The menu does not, and the reason has been read, not assumed.

### The intro's state machine

`AnimationVideo_Form` **sustains itself**: `0x11528` arms an `ISHELL_SetTimer` of one thousand
milliseconds with the callback `0x114ac`, which is the tick itself. The console does not keep
poking the animation; it gives **one** notice and the rest is up to the game. The notice is the
pair `(0x801, 0x5064)` in the handler that slot 4 registered — the form that `0x11828` diverts to
`0x114ac`.

The state lives in `[form+0x2c]` and the counter in `+0x30`:

| state | what happens |
|---|---|
| 0 | counts up to thirty one-second ticks, then becomes state 3 |
| 1 | asks for the **player** through the widget's slot 8 and tells it to play |
| 2 | arms a timer and waits |
| 3 | schedules an `ISHELL_Resume` — **this is where the menu comes in** |

Measured: when the notice is delivered, the state **is already 1**. What sets it to 1 is the image
callback, at `0x4d484`: an image loaded is an image ready to play. In other words, the console's
path goes through the player, and state 0 (count thirty seconds) is the path for when there is no
image.

### The two attempts, and why both were undone

**Repeated notice.** I delivered the pair every hundred milliseconds. The machine left the
counting state, asked for the player, went down the null path and ended up in a `malloc` with a
**pointer in place of the size**, in an infinite loop.

**Single notice, with slot 8 returning the parent.** The single notice is the right reading — the
machine rearms itself. And returning the parent has support: the object that comes out of slot 8
then receives a `slot3(widget, 0, 0)` and is released, and slot 3 on a widget is the accessor,
which refuses an unknown selector without breaking anything. With that the Z-Wheel **started
building the menu** — 178 live objects, 723 million instructions.

But the log tells the rest: `Unable to create instance of AEECLSID_LCT_SIMCARDCTL` **486,101
times**, `SendEvent to get Tectoy Font failed`, `Couldn't create z-pad instruction form (6)`.
Implementing `LCT_SIMCardCtl` — which is a firmware class, with a checked constructor — removed
that error and swapped the loop for a **hard hang**: pure spinning, without a single API call,
indefinitely.

Both were undone. "The game moved forward" is not proof that it moved along the right path, and a
hang is worse than a stable intro.

### Why it stays on the welcome screen

The short answer: **the intro never gets the start**, and when I give it, the game asks for the
player, which we do not have.

The long answer is in the image callback, at `0x4d484`, and it corrects a reading of mine. That
stretch reads the widget's natural size, compares it with `0x280` — six hundred and forty, the
screen width — and, if it is equal or larger, goes down the full-screen path: it sets bit 2 of item
`0x347`, reads item `0x414` into `[form+0x24]` and **sets the state to 1**.

In other words, state 1 is not an accident of ours: it is the right state for an intro that takes
up the whole screen. And state 1 is precisely the one that asks for the player.

### The widget accessor is typed, and we do not have the type table

This was a real error, found here. Selector `0x800` **is not "get the child"**: it is "read the
item", and what comes out depends on the item number. The image callback reads item `0x347`, adds
two and writes it back — it is a number. While the read created a child for any item, what the
game added two to was **one of our pointers**, and what it wrote into `[form+0x24]` through item
`0x414` was another.

The cut is at `0x5000`, and where it goes wrong is known: `0x414` is below the line and still
holds a widget. Raising the line would make `0x347` receive a pointer again. The items are typed
and the type table belongs to the console.

### What is missing, with name and address

1. **The animation player**, from the widget's slot 8. It is not a widget: what comes out of it
   receives `slot3(widget, 0, 0)`, a signature the accessor does not have.
2. **The font.** `0x7bfc8` calls **slot 4** of `0x01035156` expecting a font object in the output
   pointer, and then hands it to slot 9 of another object. Without it, there is
   `SendEvent to get Tectoy Font failed` and the forms that depend on text fail with error 6.
3. **The roller widget** — `MainMenu_Form.c` prints `Unable to create roller widget in MainMenu
   form`, and it is the one that draws the carousel.
4. **Real painting**, with text and clipping. `paint_widgets` only throws a hanging image onto the
   screen.

All four are pieces of the console's interface extension. It is no longer one slot at a time: it
is rebuilding a subsystem, and every object invented along the way takes the game to a state the
device never reaches.

## Where it stops today, and why

In an **infinite recursion** at `0x1177c`, which is the handler dispatcher: it reads the function
at `+0x20` and the context at `+0x1c` — the same structure that the widget's slot 4 registers —
and calls it. The stack fills with `0x1179c` repeated.

The most likely suspect is ours, and it is specific: the widget graph we return is
**self-referential**. The widget's `QueryInterface` returns `this` for any IID, and the accessor
returns a child of the same class; a game that walks that structure looking for a parent or a
sibling never gets anywhere.

This marks the end of what can be solved by discovering one slot at a time. From here on we need
to know **which** interfaces each widget really implements — and that is a question about the
console's interface extension, not about the Z-Wheel.

## The Z-Wheel's data model

It is the catalog the interface shows:

```sql
CREATE TABLE GAMEINFO(game_id INTEGER PRIMARY KEY, class_id INTEGER, playcount INTEGER,
                      dt_download INTEGER, dt_lastplayed INTEGER, boxart_path TEXT,
                      flags INTEGER, size INTEGER, unique(game_id, class_id))
CREATE TABLE TITLETEXT(game_id INTEGER, lang_id INTEGER, titletext TEXT, unique(game_id, lang_id))
CREATE TABLE ASSETS(owner INTEGER, dslid INTEGER PRIMARY KEY, type INTEGER, version INTEGER,
                    path TEXT, language INTEGER, title TEXT, startdate INTEGER, enddate INTEGER)
CREATE TABLE DLITEMINFO(item_id INTEGER PRIMARY KEY, price INTEGER, size INTEGER,
                        titletext TEXT, boxart_path TEXT, flags INTEGER, upgrade_id INTEGER)
CREATE TABLE PREFSINFO(name TEXT PRIMARY KEY, strValue TEXT, dwValue INTEGER, flags INTEGER)
```

## The EFS2, and why it was not the way

The idea was: if the interface classes are not in the firmware's table, they must be in extension
modules in the file system — `widgets.mod`, `forms.mod`, `framewidget.mod`. To read them we had to
decode the EFS2. When we tackled it, three things turned up, and all three change the plan.

### The file system is listed, and has no extension at all

The directory nodes are in the clear in the dump — they do not need the page map. Scanning
everything and joining it up (`nand.py names`) gives the complete list: **317 names**, and none of
them is a BREW extension. Only the games' modules — `tectoy.mod`, `reksio.mod` — and data: `.qxt`,
`.qxm`, `.brf`, `.html`, BDF fonts, modem profiles, error logs.

The count confirms the log-structured model: `tectoy.mod` shows up **400 times**, once per
rewrite.

### The classes are not registered anywhere in the dump

Searching for `0x01028e51`, `0x0100104f` and `0x01035156` as a registry entry — in any of the
plausible formats, of eight, twelve or sixteen bytes — across the **whole 128 MB**: zero. They
show up as a literal four or five times each, always inside a code pool or a string table, never
as a registration.

### The class table was partial because we read it wrong — and even whole it does not have ours

`registration()` looked for an **isolated entry**, filtering by a list of flags. But flags are not
a handful of values: in `1.1.2_APPS.bin` there are `0x1`, `0x4`, `0x5`, `0x8`, `0x20004`,
`0x2000004`, `0xffff0000`. With the filter, 105 entries came out.

What identifies the table is the **run**: four or more consecutive entries whose first field falls
in the ClassID range. That way you get **84 tables, 704 entries, 425 distinct classes** — the
largest at `0x10c3a018`, with 62. `AEECLSID_FILEMGR` is there, with **both** pointers of the entry
filled in, which also clarifies the shape: `<CLSID> <flags> <a> <b>`, constructor in `b` and
sometimes a second pointer in `a`.

And then comes the part that settles the question. With the 84 tables read, these are still
**absent**:

| Class | What it is |
|---|---|
| `0x01028e51`, `0x01028e35`, `0x01028e3c`, … | the widget family |
| `0x0100104f` | the generic collection |
| `0x01035156` | the TrueType font |
| `0x0102c4e8` | `AEECLSID_SQLMGR` |
| `0x0106c411` | `AEECLSID_HID` — the gamepad |
| `0x01041207` | `AEECLSID_SignalCBFactory` |

It is not a coincidence: **it is the whole Zeebo-specific layer**. SQLite, the gamepad, the
signals, the interface. None of it is in `1.1.2_APPS.bin`, and none of it is in any of the other
partitions of the dump.

That explains retroactively where each identification we have of these classes came from:
`AEECLSID_SQLMGR` came from the game's own log, `AEECLSID_HID` came from `hid_devices.cfg` and the
SDK's `IHID.dll`, the widgets came from the Z-Wheel's code. **None came from the firmware, because
none is in it.**

### What it probably is: the BREW 4.0 widget framework

Putting together what was measured slot by slot, a design shows up that is not Zeebo's — it is
BREW's:

| what we measured | what it is in BREW |
|---|---|
| slot 3 with `(0x800 or 0x801, id, value)` | `IWIDGET_HandleEvent(evt, wParam, dwParam)`, with the read and write property events |
| inverted return, zero is an error | `HandleEvent` returns a **boolean**: handled or not handled |
| event `0x100` with codes `0xe030`, `0xe04a` | key event, with BREW's `AVK_` |
| slot 4 with `{function, context}` | `IWIDGET_SetHandler` |
| slot 7 with `{width, height}`, slot 5 reading it back | `IWIDGET_SetExtent` / `GetExtent` |
| the list with size, get-at, insert-at, remove-at | `IVectorModel` |

Each of these readings was made in isolation, and they all land in the same place. That is what
gives the hypothesis its strength: it **explains at once** what had been explained piece by piece
— including the inverted return, which was the oddity hardest to justify.

**What is missing to move from "probable" is not firmware, it is documentation**: the headers
`AEEWidget.h`, `AEEContainer.h`, `AEEForm.h` and `AEEModel.h` from the BREW 4.0 SDK would give the
exact slot order, instead of us deducing it one run at a time. That is how `ISourceUtil` went from
"root form" to the right name, with `AEESource.h`.

And it is worth saying what would **not** help: another NAND dump. The dump we have already has,
in the APPS itself, code that uses `0x01028e35` and its neighbors — the console uses these classes
and does not implement them anywhere that can be found. Looking for another dump is betting that
the problem is one of version; the evidence points to a layer that is simply not dumped by this
method.

### Independent check: `IFileMgr`

tripleoxygen has, in `research/brew/`, an `IFILEMGR_VTBL_Z200.txt` and a `vtbl.ods` — someone
else's work mapping vtables of the same firmware. It counts as an outside check, and it passed on
two fronts:

- the address of the `IFileMgr` vtable that our corrected reading finds is `0x113e0438`, the same
  as in their file;
- the **twenty-one** names in our `FILEMGR` table match, in order, the `INHERIT_IFileMgr` of the
  SDK header they transcribed — from `AddRef` to `GetFreeSpaceEx`.

It is not confirmation of what is missing, but it is confirmation of the **method**: reading
vtables in the firmware, the way `firmware.py` does today, gives the same result that someone else
got by another path.

### The Zeebo SDK does not carry the BREW headers

`ZeeboSDKPackage-1.2.4.zip` (50 MB) has the Zeebo installer, the OpenGL ES one, the Adreno
Profiler, the USB driver, the developer guide and the samples — including the source of
`conftest`. **No `AEE*.h` header**: the Zeebo SDK relies on Qualcomm's BREW SDK, which is
installed separately and is not there.

The BREW material in the `doc/` folder is from the 2 and 3 era — the samples carry `AEE.h`,
`AEENTP.h`, `AEEAddrBookExt.h`, and nothing of the widget framework, which is from BREW 3.1
onward. `BREWOemAPIReferenceforMSM.pdf` does not mention `IWidget` a single time either.

In other words: what is missing is **Qualcomm's BREW 4.x SDK**, with `AEEWidget.h` and company. It
is not Zeebo material, it is Qualcomm's, and it is not in this collection.

### What was confirmed for free: `ICM`

With the corrected reading, `AEECLSID_CM` (`0x01011810`) got a real vtable — `0x10a5e1f0`, with
at least thirty-four slots. **Slot 28** exists and is `0x10ca927c`, and what it does is:

```
0x10ca92b6  bl   #0x10ca46d0
0x10ca92c0  str  r0, [r2, #0xc]      ; the value goes to offset 0xc
0x10ca92ca  blx  #0x10b967f8         ; copies the structure to the caller's buffer
0x10ca92ce  movs r0, #0              ; zero is success
```

The `0xc` offset our implementation writes **is confirmed by the firmware**. The value five is
still an inference from the game's side, which compares against five at `0x77564` — but now only
half of the hypothesis is hypothesis.

### And the console's Z-Wheel is another build

The `tectoy.mod` installed in the dump has strings ours does not have: `SpinStage.c`,
`StageWidget.c`, `spin_ui_utils.c`, `tectoy_maskededit.c`. Comparing the page that contains
`0x01028e51` in both, they differ — the offset within the page is not even the same.

In other words, the dump is from 2011 and our package is from another generation. Even with the
EFS2 read in full, what would come out of it is not necessarily what our package expects.

## What is still missing from the EFS2, if we ever need it

```
EFS2  →  MIFs of the static modules  →  class registry  →  vtables
```

The first link is the missing one, and it is the first in the chain.

**What is solved in the EFS2** (see [`tools/nand.py`](../../tools/nand.py)):

- The partition table, checked: with 128 KB blocks, the computed APPS partition matches byte for
  byte the `1.1.2_APPS.bin` published separately.
- The dump has two files; the `_spare` one is the same content interleaved in 512 + 16 sectors.
  The out-of-band area only has ECC — the map is internal.
- The directory entry format: `<u8 size> <u8 type> <u32 date> <u8 zero> <name> <'i'> <u32 ref>`,
  with the size counting the name plus five.
- The map is a page table: `table[ref + n]` is the physical page of the n-th 2 KB chunk.
  **Confirmed**: assembling `tectoy.ttf` through the table at `0x4c23104`, the first thirteen pages
  come out identical to the known copy.

**What is missing**: the current generation of the table. Entry 13 of that copy points to `0x14f5`
and the correct one is `0x17a6` — it is not corruption, it is an old version. And the current one
does not exist as a contiguous block: searching for the real list of 94 pages of the font, the
5-page prefix shows up once and the 16-page one does not show up at all. The current map is the
base table **plus a journal**, and reproducing it is the work that is missing.

## Two traps that have already cost a lot

**A check that agrees with the hypothesis is not a check.** This bit three times, in different
forms: a mapping base calibrated by a string that was the wrong occurrence, a disassembly that
produced valid instructions from text, and an execution trace that kept the first instructions
when I read it as if they were the last. Disassembly almost never refuses to produce code, and
`find` in a dump returns the first occurrence, not the current one.

**A primary source beats inference.** I deduced from the games' binaries that the controller's `X`
axis was `0x0106c4ce`, with consistent evidence across dozens of titles. The console's
`hid_devices.original.cfg` says `0x0106C40C`. The deduction described **another device** well —
the PC controller, whose entry in the same file is in fact `c4d0`/`c4d1`.

## Material

It lives in `vendor/`, ignored by git — firmware and console content do not go into the
repository. Origin: `tripleoxygen.net/files/devices/zeebo/`.

## The "object 10" of event 0x7b0a

`0x885a4` asks its own applet, through `SendEvent(cls=0x01070798, evt=0x7b0a,
wParam=0xa, dwParam=&out)`, for an object it keeps in `[app+0x34e4]`. Without that
object `0x89518` exits early, `0x8f700` receives zero and the game writes
`Couldn't create z-pad instruction form (6)` — forever, thousands of times per
minute.

The handler is at `0x7bd4c`, which splits by `wParam`: `1 → 0x7bdac`,
`2 → 0x7bebc`, `4 → 0x7c134` (PrefsDB) and `0xa` falling into `0x7bdc0`. The branch for 10 reads
the **`Lang`** preference and enters a loop:

```asm
0x7bdf4  ldr r5,[r5,#0x4e4]   ; head of the language list
0x7bdf8  ldr r0,[sp,#8]       ; the value of Lang
0x7bea4  cmp r5,#0            ; end of the list -> give up
0x7bea8  bne 0x7be00
0x7be00  ldr r1,[r5]          ; tag of the current language
0x7be04  cmp r1,r0
0x7be08  bne 0x7bea0          ; not this one, next
```

`[r5]` is not an index: it is **four characters** read as a word. The trace
shows `0x20206e65`, `0x20207365` and `0x20207470` — `"en  "`, `"es  "` and `"pt  "`.
The `Lang` preference stores the tag in the same packed format.

The package's `tt_prefs.db` is born with `Lang = 0`, along with `TermsAccepted = 0` and
`IsRegistered = 0`: it is a profile that never went through first-time setup — the screen
that offers `language_english.bmp`, `language_portugese.bmp`, `language_spanish.bmp`
and `language_mexico.bmp`. Zero matches no tag, and that is why the loop
always ends in nothing.

Until we reach that screen, `choose_language` writes `"pt  "` the first time
the database is opened without a choice, and the report notes the hypothesis. With that the z-pad
error goes away and the game starts asking for `tectoy_pt.brf`.

The next wall is another one: `Service status is NOT available!`, at
`tectoy_ui_internal_utils.c:647`, repeated at the same beat as
`Unable to create instance of AEECLSID_LCT_SIMCARDCTL`.

## The service status, and the loop that remains

Slot 28 of `ICM` is not `GetPhoneInfo`: it is `ICM_GetSSInfo`, and it fills an `AEECMSSInfo`
of 0x340 bytes. Two parts of the game read the same result by different paths —
`0x87c90` wants the operating mode at `+0xc`, `0x696a0` wants the service status at
`+0` and, when it is 2, the signal strength at `+0x28`, which `0x69830` turns into
bars in bands of nine. We answered only the first, and the second repeated
`Service status is NOT available!` at the beat of the attract cycle.

What still does not move is the cycle itself. The Z-Wheel drives itself through `ISHELL_Resume`:
each lap calls `0x82464`, which does the card check at `0x78544`, gets `0x27`
("could not verify"), goes through `0x1f7b4` and reschedules itself. That is 1736 laps inside
**one** frame, with a handful of widgets leaking on each one.

What would break the loop is state `0x28`, and it requires `AEECLSID_LCT_SIMCARDCTL`
to exist: `0x78580` calls its slot 3 and, if the answer is zero, `0x82470` exits without
rescheduling. Offering the class was tested again here, and it is still worse: the game creates
one control per lap and never releases it — 65,506 live objects, the pool exhausted, and the z-pad
starting to fail with code 3. Slot 3 registers a callback pair at
`+0xc4`/`+0xc8`, and without someone answering "there is no card" the object is never freed.

Two ideas were tested and undone:

- **Deferring the resumes** that went past a quota per frame. The loop did drop from
  1736 laps to the quota, and the heap from 4 MB to 907 KB — but the game started repeating
  `ISHELL_Resume` 576 thousand times in the frame, waiting for the resume that never came.
  Deferring does not pause the game; it only swaps one loop for another.
- **Expiring the timers at the callback boundary**, and not only between frames. The motivation is
  real — a frame can last two virtual minutes, and in that time no timer expired.
  But the reentrancy breaks whoever does not expect it: Crash Bandicoot started dying on a
  null access at `0x435b0`.

## The welcome screen chain

It took three rounds to become clear, with a correction of mine in the middle.

1. `AnimationVideo_Form` plays `opening_low.gif`, which has **a single frame**: it is the
   welcome screen itself, 640×480.
2. Once it is done, the state machine at `0x114ac` goes to state 3 — the terminal one — and on
   each lap calls `HandleEvent(0x801)` on the widget and reschedules itself through
   `ISHELL_Resume`.
3. The resume enters at `0x11750`, which calls `[app+0x24]->slot6(1)` and falls into `0x82464`:
   the SIM card check.
4. The `0x27` outcome leads to `0x1f7b4`, which goes through `0x1f85c` and reaches
   `0x7ed10(app, 1)` — **the launch of the z-pad instructions form**. The
   message `Unable to launch z-pad intructions form: %d` comes from there.

The point that cost me a reversal: of the three outcomes of the check, only `0x27`
moves forward. `0x28` compares, branches to the end and returns — quiet because it does nothing.
When I offered `AEECLSID_LCT_SIMCARDCTL` and answered "card in order", the log came out clean and
the screen stood still forever. The repeated `Unable to create instance of
AEECLSID_LCT_SIMCARDCTL` is the game taking the right path many times.

State 0 of the same machine is the one that waits for the image to load — thirty attempts,
one second apart, and `waiting for image load to complete...` when it gives up. It is never
reached here: the GIF loads on the first try.

### The clock and the input

`skip_idle_time` jumped straight to the nearest timer. With the 120,000 ms of
inactivity that the welcome screen arms, the clock went to two minutes in one go and
fired the idle time before any key had a chance to arrive: the loop made
**six** laps in ninety seconds of real clock, and a key script never
hit the marked instant. The jump is now one frame at a time — 8398 laps.

### The position of the widgets

The third argument of `AdicionarFilho` always carried the position, and we always
threw it away. It is six words, `{x, y, flag, width, height, object}`: the
intro image comes in with `{0, 0, 1, 640, 480}` and the pieces of the z-pad form
with `{100, 21}`, `{148, 20}`, `{365, 20}`. The same slot serves five classes of the family,
and not every call has this shape — some pass a function in `r2` and an object in
`r3` —, so the position is only read when `r2` is zero.

The properties the game writes through selector `0x801`, measured: `0x130` (0 or 255),
`0x140` (a color, `0x444444ff`), `0x152` (objects), `0x153` (`0x10`, `0x410`, `0x420`,
`0x440`), `0x216` (0 or 1), `0x347` (2) and the `0x5000` range (objects). None is a
coordinate.

## Where the widgets come from

The question is where to get the implementation from, and the answer has a good part and a bad
one.

**The bad: the binary is not in the dump.** The APPS carries the strings
`fs:/mod/widgets/widgets.mod`, `fs:/mif/widgets.mif` and `fs:/mod/htmlwidget/htmlwidget.mod`
— the paths exist, the files do not. Searched for across the whole NAND, the 128 MB, the
string `widgets` shows up thirteen times and all of them inside the APPS, as a constant. The
`part_EFS2APPS.bin` has two modules, `reksio.mod` and `tectoy.mod`, and nothing else.

The classes are not in the class table either: `0x01028e19`, `0x01028e2a`,
`0x01028e35`, `0x01028e3f`, `0x01028e47` and `0x01028e51` do not show up in any of the 84
tables, and of the five occurrences they have in the NAND none has the shape of a registry
entry. They are all literal pools — the firmware **asks for** these classes just as much as the
Z-Wheel does.

**The good: the firmware is a huge user of them.** `0x01028e2a` shows up forty times,
`0x01028e3f` and `0x01028e47` nineteen, `0x01028e35` sixteen. Each use is a call
site with the signature written down, and reading call sites is what we have been doing with the
Z-Wheel — except that here there is much more code, and it is TecToy's own. The version is
written right nearby: `RMUI Ver: V0.1.196`.

Two confirmations have already come out of this, and both match what we had deduced:

- At `0x103587be`, the firmware calls `slot7(this, &{600, 40})` — `DefinirTamanho`, with
  the shape we were already using.
- `0x1035f222` is the property setter: `accessor(this, 0x801, id, value)`, with
  the inverted return convention. One of the calls is `(0x153, 0x10)`, exactly the pair
  we measured coming out of the Z-Wheel.

And something new came out: its sibling at `0x1035f23c` uses a **third selector**,
`0x711`, in the form `accessor(this, 0x711, 0, value)`. That was the "widget selector we
do not know" from the hypotheses. We still refuse it — what it means we do not know yet —, but
now the report says the number instead of saying "a selector".

The way to implement, then, is not to copy a vtable: it is to read the firmware's call sites,
which are many and are all in `1.1.2_APPS.bin`.

## What the firmware says about the widgets

Studying `1.1.2_APPS.bin` to decide how to implement, with a Thumb reader that simulates
`movs`/`adds`/`lsls`/`ldr literal` in a window before each `bl` to recover the arguments.

**`0x1035f222` is the RMUI property setter:**

```asm
0x1035f222  push {r4, lr}
0x1035f224  ldr  r3, [r0]        ; vtable
0x1035f226  ldr  r4, [r3, #0xc]  ; slot 3 — the accessor
0x1035f228  mov  r3, r2          ; value
0x1035f22a  mov  r2, r1          ; id
0x1035f22c  ldr  r1, = 0x801     ; write selector
0x1035f22e  blx  r4
0x1035f230  cmp  r0, #0          ; inverted return: != 0 becomes 0
```

It matches method by method what we had deduced from the Z-Wheel side — the selector, the order
of the arguments and the inverted convention. And its sibling at `0x1035f23c` uses a **third
selector, `0x711`**, in the form `accessor(this, 0x711, 0, value)`: that was the "selector we do
not know" that appeared in the hypotheses. It is still refused, but the report now says the
number.

Ninety-two calls to the setter give the property vocabulary the **firmware** uses:

| id | values seen |
|---|---|
| `0x100` | 0, 1 |
| `0x110` | `0xff` |
| `0x130` | 0, `0x6e280fff` |
| `0x135` | `0xffff`, `0x6e280fff` |
| `0x140` | 0, `0x220` |
| `0x153` | 3, `0x10`, `0x220`, `0x900210` |
| `0x163`, `0x165` | 0, 1, 2 |
| `0x219` | `0x10`, `0x80`, `0x2400004a` |
| `0x220` | 1 |
| `0x315` | `0x01012786`, `0x01012787` — **ClassIDs** |

The Z-Wheel uses `0x130`, `0x140`, `0x152`, `0x153`, `0x216`, `0x347` and the `0x5000` range. The
intersection — `0x130`, `0x140`, `0x153` — confirms it is the same vocabulary. `0x140` receives
`0x444444ff` in the Z-Wheel, which is a color with alpha; `0x315` receives a class.

Another confirmation: at `0x103587be` the firmware calls `slot7(this, &{600, 40})`, which is
`DefinirTamanho` in the shape we were already using.

**Text is not a property.** Tracing what `LoadResString` loads to where it stops, it becomes the
`arg1` of **slot 6** of the widgets of class `0x01028e2a` — `slot6(this, text, size, …)`. In
other words, the same slot number means different things in different classes of the family:
where we call `DefinirVisivel`, that class sets text.

### The way to implement

It is not copying a vtable, because it is not in the material. It is writing a toolkit that
behaves like RMUI, using the firmware's call sites as the specification — and there are many:
forty uses of `0x01028e2a`, nineteen of `0x01028e3f` and of `0x01028e47`, sixteen of
`0x01028e35`.

The first concrete step, which gives a screen: store slot 6's text per class and draw it in
`paint_widgets` with the font we already have, at the position `AdicionarFilho` brings and in the
color of `0x140`.

## The widget model, pictured

Dumping the whole tree at the moment a key arrives answered more than months of deduction. What
you see is **three trees**, not one:

```
root 0x30000150  class 0x1028e51  4 children     ← the applet's root form
  0x30000650     class 0x1028e47  handler 0x11828   (intro form)
  0x30000710     class 0x1028e47  handler 0x8f48c   (z-pad form)

root 0x30000790  class 0x1028e3f  5 children     ← what we draw
  0x300007d0     class 0x1028e3f  handler 0x77300  size 640×50   (status bar)
    0x30000810   class 0x1028e2a  pos (148,20)  text "3G"
    0x300008d0   class 0x1028e19  pos (100,21)  (the signal icon)
    0x30000950   class 0x1028e2a  pos (365,20)  text "Meus Z-Credits"
    0x300009d0   class 0x1028e2a  pos (375,20)  text "10"
  0x30000a10     class 0x1028e2a  pos (320,90)  text "Z-Pad"
  0x30000ad0     class 0x1028e19  pos (320,100) (the controller photo)
  0x30000b90     class 0x1028e26  size (805309136, -20)

root 0x300005d0  class 0x1028e3f  1 child
  0x30000690     class 0x1028e19  size (66048, 0)
```

Plus seven loose widgets of class zero — the ones the accessor creates on its own when reading an
item in the `0x5000` range.

### What this says

**The forms and what they draw are separate objects in our tree.** `0x30000710` is the z-pad
form, it has the key handler and **zero children**; the status bar and the controller photo hang
from another root. On the console the two are the same form, and the link between them goes
through some call that we still read as something else. That is why no rule about "the tree of
the current form" gets it right: the key and the drawing live in different trees.

**The family has roles, and they can be read in the tree:**

| class | role, by what it carries |
|---|---|
| `0x01028e51` | the applet's root form — it is what `[app+0x24]` keeps |
| `0x01028e47` | a form: has a handler, has no child |
| `0x01028e3f` | a container: has children and a real size (640×50) |
| `0x01028e2a` | a label: carries text |
| `0x01028e19` | holds an image |
| `0x01028e26` | receives a color through `0x186`, and a **pointer** through slot 7 |
| `0x01028e36` | accessor with property `0x156` |

**And slot 7 is ambiguous too.** `0x30000b90`, of class `0x01028e26`, has size
`(805309136, -20)` — and `805309136` is `0x30000bd0`, an object address. `0x30000690`, of
`0x01028e19`, has `(66048, 0)`. In the container classes slot 7 really is `{width, height}`; in
these two it is something else.

That closes a pattern that has shown up four times already: **slot 4** returns the previous
handler, **slot 5** is a getter when the third argument is 4, **slot 6** sets text on
`0x01028e2a` and visibility on the others, and now **slot 7**. A single slot table for the whole
family is the assumption that is wrong, and each of its errors cost an investigation.

### What is missing, in order

1. **Linking form and content.** As long as they are separate trees, neither the key nor the
   drawing has any way to know what the current screen is.
2. **Focus.** The key reaches the applet, which forwards it to the form, which is ours — and the
   one that answers is `0x77300`, a `mov r0,#1; bx lr` of the game that returns "handled" for
   everything. On the console the one that receives it is the widget with focus.
3. **Drawing through the toolkit.** We know how to paint images and text; backgrounds, frames and
   the layout the toolkit computes do not exist here. Hence the sparse screen.
