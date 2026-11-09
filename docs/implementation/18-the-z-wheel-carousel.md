# 18 — The Z-Wheel carousel: from black screen to drawing

**Who this document is for:** anyone implementing the Z-Wheel (the Zeebo's shell/store, App ID
274755, applet `0x01070798`) in **any** Zeebo/BREW emulator and stuck on a black screen.

Almost everything here is **guest-side fact**: what `tectoy.mod` does, at which address, and what
answer it demands to keep going. That holds regardless of the emulator's language and
architecture. Where a decision shows up that is only ours, the text says it is ours.

What is **not** transferable: Zeebx file and function names. They are collected in the appendix, at
the end, so they don't clutter the rest.

> Status (2026-09-11): the wheel comes up, the roller is built, **drawn every frame and composited
> on screen**, and **turns with the key** — measured, see §5.8. It still **does not fully work**:
> section 8 lists what is still a hypothesis or broken. Nothing here promises that the whole path
> is right; it is the record of what got past each step.

---

## 1. Quick triage: the black screen has several causes

If the screen is black, the first question is **at which step the app stopped**. Each row below is
a symptom we saw, with the cause that was behind it.

| Symptom | Likely cause | How to confirm |
|---|---|---|
| Stops at `Could not create root form(20)` | the refused class is `0x01028e51`, **not** `0x01001011`; and its return value is **inverted** | see §3 |
| `SendEvent to get PrefsDB failed`, 11 times, then gives up | event sent to its own class **before** `EVT_APP_START` | see §2.1 |
| `Invalid database version`, `Failed to init Delayed Queue database: 42` | `tt_dlqueue.db` opened empty, without `DBINFO` | see §2.4 |
| `Arithmetic exception: Divide By Zero` | the "list step" answered with zero | see §6.3 |
| `Failure in call to CreateOwnerDrawWidget` | class `0x01028e14` refused | see §4 |
| `Unable to create roller widget in MainMenu form` | **widget slot 17 is missing** | see §5.4 |
| Runs all day, no error and no drawing, on a ~10 s pulse | a refused slot aborted the callback **silently** | see §5.1 and §4 |
| The screen shows a small rectangle instead of the 640×480 | the roller's surface (214×34) overwrote the screen bitmap | see §5.6 |
| The roller draws **once** and never again; the bottom bar stays empty | its widget was born at the screen bitmap's address, recycled after one `Release` too many | see §5.8 |
| The 3D comes out shifted / cut at the edge | the pbuffer was created with the screen size instead of the attributes' size | see §4 |
| Everything is built, but the screen stays white or with the intro on top | you are painting **all** live roots | see §6.1 |
| The wheel does not turn with any key | the key is being consumed by the wrong handler, or the key code is wrong | see §7 |
| Hard hang, with no API call at all | a handler that branches to itself: you did not return the **previous** one | see §5.5 |

---

## 2. Before dreaming of the wheel

None of these items is about the interface, but without each of them the build-up stops early.

### 2.1 The event the applet sends itself before it exists

The Z-Wheel sends event **`0x7b0a`** to **its own class**, asking for the preferences database
object, and the answer comes back written into `dwParam`. This happens **while the applet is being
constructed**, inside `CreateInstance` — before any `EVT_APP_START`.

If your `ISHELL_SendEvent` looks up the applet in a variable that is only filled in at start, the
event comes back "nobody handled it" and the app prints `SendEvent to get PrefsDB failed` eleven
times, then gives up on the whole configuration.

**Rule:** for the shell, the applet exists once it is **registered**, not once it is started.
`AEEApplet_New` has already written the output pointer before the game code runs — that is where to
read it from. This is not a Z-Wheel patch: it holds for any app that sends an event to itself during
construction.

### 2.2 Real SQLite

`AEECLSID_SQLMGR` really is SQLite, not a clone. The package's `tt_prefs.db` literally starts with
`SQLite format 3`, and the statements the module carries as text are the dialect (`INSERT OR
REPLACE`, `COLLATE NOCASE`, a JOIN between `GAMEINFO` and `TITLETEXT`, `PRAGMA integrity_check`).
Link an SQLite library and only write the bridge: open the named file, execute the statement and
return the rows **one call to the game's callback per row, with the values already as text**, like
`sqlite3_exec`.

The schema the interface shows:

```sql
CREATE TABLE GAMEINFO(game_id INTEGER PRIMARY KEY, class_id INTEGER, playcount INTEGER,
                      dt_download INTEGER, dt_lastplayed INTEGER, boxart_path TEXT,
                      flags INTEGER, size INTEGER, unique(game_id, class_id));
CREATE TABLE TITLETEXT(game_id INTEGER, lang_id INTEGER, titletext TEXT, unique(game_id, lang_id));
CREATE TABLE DLITEMINFO(item_id INTEGER PRIMARY KEY, price INTEGER, size INTEGER,
                        titletext TEXT, boxart_path TEXT, flags INTEGER, upgrade_id INTEGER);
CREATE TABLE PREFSINFO(name TEXT PRIMARY KEY, strValue TEXT, dwValue INTEGER, flags INTEGER);
```

### 2.3 The catalog has to be writable

The package's `tt_game_info` is the source of the official catalog and **must not be modified**.
Work on a profile copy. If you are going to inject the local ROMs as carousel items, mark what is
yours (we use a table of our own, `ZEEBX_LIBRARY`, with `class_id`/`game_id`) so that a new scan
deletes only your records, without touching what came from the package.

### 2.4 `tt_dlqueue.db` arrives empty and needs a schema

The file exists with 0 bytes. `DLQueueDB.c` queries `DBINFO` and fails with
`Invalid database version` / `Failed to init Delayed Queue database: 42`. When opening this
database, create:

```sql
CREATE TABLE IF NOT EXISTS DBINFO(version INTEGER, subversion INTEGER);
INSERT OR IGNORE INTO DBINFO values (1, 0);
CREATE TABLE IF NOT EXISTS DLITEMINFO(item_id INTEGER PRIMARY KEY, price INTEGER, size INTEGER,
                                      titletext TEXT, boxart_path TEXT, flags INTEGER,
                                      upgrade_id INTEGER);
```

### 2.5 `preloaded.cfg` is device state, not a package file

The Z-Wheel does `IFILEMGR_Test`, then `GetInfo`, then `OpenFile` on `preloaded.cfg`. It does not
come in the package: the console created it. Without the NAND, the faithful answer is **it exists
and is empty** — an empty list of preinstalled games. Answering "does not exist" to `Test` sends
the flow elsewhere.

A detail that bites: the file size is read at **`+0xc`** of the `FileInfo` (measured at `0x89068`).

---

## 3. The inverted return — the bug that looks like a "missing class"

`Could not create root form(20)` misleads twice. The `20` is `EUNSUPPORTED`, and the refused class
is **`0x01028e51`**, not `0x01001011`. The message is not referenced by literal but by `ADR`:
`tectoymain.c:1001` comes out at `0x7c478`, and the `ISHELL_CreateInstance` just above loads the
literal from `0x7c69c`.

Of this class the game uses **a single method**, **slot 3**, a generic accessor called by two
wrappers that pin the selector:

| Wrapper | Selector | What it does |
|---|---|---|
| `0x3f72c` | `0x800` | **reads** item number `id` and writes it into the third argument |
| `0x403c8` | `0x801` | **writes** item `id` |

And what decides everything is the return value: the wrappers do `cmp r0,#0; moveq r0,#3`. **Zero
becomes an error.** Answering `SUCCESS` (which is zero in BREW) means answering "failed" — and
`0x78acc` does not handle that failure: it jumps to the cleanup and dereferences the second child,
which was never filled in. Hence the invalid access at zero at `0x78b70`, which **looks like** the
point of failure and is only the error branch.

**In this class, non-zero is success.** `AddRef`/`Release` still return the count, as everywhere in
BREW.

### 3.1 The accessor's items are typed

Selector `0x800` **is not "get the child"**: it is "read the item", and the type of what comes out
depends on the number.

- Items **`>= 0x5000` hold objects** (`0x88338` reads `0x5000` and `0x5002` and uses both as
  widgets).
- Items **below `0x5000` hold numbers**. The image callback reads item `0x347`, **adds two** and
  writes it back; `0x11668` reads `0x414`, subtracts one and compares.

If you create an object for every item that is read, the game ends up adding two to **one of your
pointers** and comparing a pointer with a number. The symptom does not look like the cause: the
intro spins forever (we saw twelve million trips to the accessor and six million timers in one
run).

Storing an object in an item `>= 0x5000` is **hanging it**: that is how the form receives what it
shows (`0x8ed80` writes `0x5000` into the z-pad form). Treating this as a simple "numeric property"
breaks the tree into pieces, and no rule about "which is the current screen" gets it right after
that.

### 3.2 The class family that shares this accessor

They are numbering neighbors and show up together in a firmware table at `0x1035cf24`:

| Class | What it is |
|---|---|
| `0x01028e51` | the interface's root widget |
| `0x01028e05` | requested by the stage; refused, `CreateStageWidget` gives up |
| `0x01028e14` | the roller's `OwnerDrawWidget` |
| `0x01028e19`, `0x01028e2a` | "frame widget" of `AnimationVideo_Form.c:93`; `2a` is the one that **sets text** |
| `0x01028e26`, `0x01028e36` | accessor with colors and properties; without `36`, `Couldn't create z-pad instruction form (20)` |
| `0x01028e3f` | **bottom bar container** — important, see §6.1 |
| `0x01028e47` | the **form**: it has a handler and hangs content on item `0x5000` |

Watch out for one detail: **the same slot number means different things depending on the class.**
Slot 6 is "show/hide" in most of the family, and in `0x01028e2a` it is **set text**
(`slot6(this, text AECHAR, size, …)`).

None of these classes is registered in the firmware dump — they come from extensions
(`widgets.mod`, `forms.mod`, `framewidget.mod`) that are not in the file system. Everything above
came from reading the game's code. The strongest hypothesis, which explains the inverted return and
the shape of the slots in one go, is that this is the **BREW 4.0 widget framework**: slot 3 would be
`IWIDGET_HandleEvent` (which returns a "handled" boolean), slot 4 `SetHandler`, slots 7/5
`SetExtent`/`GetExtent`. If your friend has access to the headers `AEEWidget.h`, `AEEContainer.h`,
`AEEForm.h` and `AEEModel.h`, that is worth more than any reverse engineering of ours — that is how
`0x01001011` went from "root form" to its right name, `ISourceUtil` (`AEESource.h`).

---

## 4. The stage (the 3D wheel)

`CreateStageWidget` builds the stage, and it draws in GL ES **into a pbuffer**, not into the
window.

| What the game does | What to require from you |
|---|---|
| `IWidget::slot16(this)` at `0x22d58`, right after the stage exists | accept it. Refused, execution **does not come back** to `0x22d5c`: the menu build-up stops there, and the app sits on a 10 s pulse reading points and the download queue without drawing anything |
| creates `0x01028e14` and then `0x01028e05` | serve both; refusing takes the games wheel down with it |
| `eglChooseConfig`, `CreatePbufferSurface`, `CreateContext`, `MakeCurrent` | **the pbuffer has the size from the attribute list** (640×330, the area of the wheel's widget), not the screen's. With the wrong size, the cylinder runs past the edge and the bottom panel is cut off |
| `eglGetColorBufferQUALCOMM` | return the **raw pixel address**, in RGB565. Zero is failure: the `cmp r0,#0` right after branches to `0x76e30` and the stage gives up. There is no size output — the game already keeps width/height at `[r4+0x18]`/`[r4+0x1c]` |
| two `0x01028e3c` classes, created in `tectoymain.c` and stored at `+0x354`/`+0x358` | see below |

`0x01028e3c` has two methods that **kill the cycle silently** if refused:

- **`slot3(this, &out)`**, called at `0x8588c` and `0x858a8`: two objects in sequence fill pieces
  of the same structure, and the caller bails out if either returns non-zero. It was **the end of
  the attract cycle** — 1722 callbacks aborted in the first second, and then silence. We zero the
  `0x18` bytes between the two destinations (`r4+8` and `r4+0x20`) and return success. It is a
  hypothesis.
- **`slot5(this, &out, 0, 0)`**, at `0x86238`: the caller reads a **halfword** and stores it at
  `[r5+0x10]`. It looks like a measurement. We answer zero — and zero already brought the game down
  once (§6.3), so if a loop or a division by zero shows up, look here first.

Other classes `tectoymain.c` requires, with the name taken from the error message itself:
`0x01006c05` (ZEEBOMCP), `0x01006c02` (system control: its slot 6 is called in a loop, and zero
means "go on"), `0x01011810` (`ICM`: slot 28 receives `(buffer, 0x340)` and the caller compares
`+0xc` with **5**, which is `SYS_OPRT_MODE_ONLINE` — answer "the radio is on the air"),
`0x01001027` (`IConfig`), `0x0100104f` (generic collection), `0x01028e35` (generic list).

**And one you should refuse on purpose:** `0x01006c01`, the `LCT_SIMCardCtl`. `0x78544` creates it
to request a card check; when creation **fails**, it sets the state to `0x27` — and `0x27` is
exactly what `0x82464` forwards to the transition that opens the main menu. Implementing it breaks
the transition. We mapped the class in one commit and removed it in the next, for that reason.

---

## 5. The bottom roller (the slider): the missing steps

This is the part that solved the startup. In order of discovery.

### 5.1 Refusing a slot aborts the callback **silently**

Before anything else, a lesson in method: in our emulator, an unimplemented slot became an error
and the game's callback died whole **with nothing in the log** — because the one who aborted was
us, not the game. We had to log the **outcome of each callback** to see that an old report line
(`I28e3c::slot[3]`) was what killed the chain.

If your friend has a black screen and a clean log, this is the first instrumentation to have: per
callback, "entered / returned / aborted, and why".

### 5.2 Slot 13 returns a surface

At `0x24100..0x24198` of `tectoy.mod`, `tectoy_rollerwidget.c` calls **slot 13 of a widget** with
`(&bitmap, width, height)` and then does `QueryInterface` on the returned bitmap. In other words:
the widget is used as if it were a bitmap factory.

Answering just "success" makes the build-up go on and leaves the roller **without a surface**. The
right thing is to treat it as a **`CreateCompatibleBitmap`**: create and return a surface of the
requested size. With that done, the **214×34** (roller) and **440×49** (tabs) surfaces appear.

> This only works if your widget `QueryInterface` is permissive (returns `this` for any IID). That
> is what we do, and it is noted as a hypothesis — it is also the suspicion behind an old recursion
> at `0x1177c`, because a self-referencing graph traps whoever walks it looking for a parent or a
> sibling.

### 5.3 The accessor is called with a child's address in place of the selector

Some classes call slot 3 passing the **address of a child widget** as the selector, to query or
toggle its visual state. If you treat that as an "unknown selector" and return zero, remember §3:
zero is **failure** here. Recognize the address and answer success.

### 5.4 **Slot 17 — this is the main blocker**

At `0x23860` and `0x23d0c` of `tectoy.mod`, the roller does `ldr r3, [r0, #68]`, that is
**`pWidget->Slot17(0x8000, pFont)`**: it attaches the font model returned by the fallback (or by
the font handler) to the roller widget.

`68 / 4 = 17`. If your widget vtable stops at slot 16, this call lands on "method not implemented"
and **the roller build-up aborts** — that is `Unable to create roller widget in MainMenu form`.

Two things:

1. Accepting it already unblocks. With the slot accepted, `StageWidget.c:256` passes, the carousel
   loads its **15 items**, and the roller and the tabs are generated.
2. **It is not a notification:** the module **releases `pFont`** right after building the roller.
   Whoever attaches it needs to hold a reference of its own (`AddRef` on the model, `Release` on the
   previous one, and release everything when the widget dies). Without that the font object dies
   and the address is reused by another object — a bug that shows up far from here.

### 5.5 Slot 4 and slot 16 **return the previous one**

Both register a callback by passing a pointer to a structure, and both **write the previous
registration back into that same structure**. That is how BREW chains: whoever registers keeps
there whoever was before, and forwards to it whatever it does not handle.

- **Slot 4, event handler**: `slot4(this, &{function, context, free_fn})`.
  `ZPad_Keyboard_Instructions_Form.c` does exactly that — `0x8f560` reads `[context+0x14]` and
  tail-jumps there when it does not handle the key. Without writing the previous one back, the
  structure keeps describing **the very** handler that just registered, and the forward becomes
  **infinite recursion**: 250 million instructions in a single frame and a frozen window, **with no
  error at all in the log** — because there is no error, there is a loop. With no previous handler,
  return zero: `0x8f564` recognizes it.
- **Slot 16, draw callback**: `CreateOwnerDrawWidget` at `0x22cd0` builds the triple in its own
  structure (`[r4+0x14] = 0x5250c`, `[r4+0x18] = r4`, `[r4+0x1c] = 0x52574`) and passes `r4+0x14`
  in `r1`. The one that draws is `0x5250c`, and it is **a link in a chain**: it first calls
  `[ctx+0x14]([ctx+0x18], r1, r2, r3)` — exactly where the previous one has to be — and then the
  roller's own drawing, at `[ctx+0x00]([ctx+0x08], …)`. Without the write-back, it calls itself.

### 5.6 The screen bitmap must not go back to the free list

Symptom: the roller creates the 214×34 surface and **that is what appears in the window** instead
of the 640×480.

Cause: whoever asks for the screen bitmap releases what it got, as convention demands — but its
**owner** is the display, not whoever asked. Released, the address goes back to the free list and
the next `CreateCompatibleBitmap` writes the new surface over the screen. The display bitmap needs
a reference of yours, one that is never released.

**And a permanent reference is not enough**, as we found out later: if any path hands out that
pointer **without** counting — which was the case with `GetDestination` here —, the game releases
what was never counted, the permanent reference is consumed and the address goes back to the free
list all the same. See §5.8, which is the continuation of this one.

The same principle holds elsewhere: when the game hangs an image on a widget, it **releases its
reference to it right after**. If the widget does not hold it, the object dies with the decoded
image inside and what is left to paint is nothing. **Whoever keeps, holds; whoever keeps,
releases.**

### 5.7 The events the applet offers the root must come back "not handled"

The applet offers some events **to the root widget first**, at `0x7b418`. If the root answers true,
dispatch **stops at `0x7b420`**, before the applet's own handlers — which are the ones that fill
the font and resource outputs.

We used to answer "handled" and swallow the event. Today `0x101`, `0x7b0a`, `0x7b0e` and `0x7b0f`
come back **false** through the root's accessor, and the applet takes care of it. (Remember that
here "false" is zero, and in this accessor zero would be "failed" for the wrapper — that is why the
distinction matters: it is the **value of the event's boolean**, not the creation error code.)

### 5.8 The screen bitmap must not be recycled — this is what blanked the bar

This was the last step, and it is the most transferable of all, because it has nothing to do with
the Z-Wheel: it is reference-counting discipline in `IDisplay`.

**Symptom:** the roller builds, draws **once** — the 214×34 surface comes out with the "Jogar"
(Play) item rendered — and never again. The bottom strip of the screen stays empty, and no key
changes a single pixel.

**The chain, measured with the serial and the surface dump in the same run:**

1. `IDISPLAY_GetDestination` returns the target's `IBitmap *`. By BREW convention, **whoever
   receives releases** — and the game releases. We returned the pointer **without `AddRef`**, while
   `GetDeviceBitmap` right next to it counted correctly. Each `GetDestination` therefore took away a
   reference nobody had put in.
2. With that the screen bitmap **reaches zero**. The address goes back to the free list — but the
   surface stays alive in our bitmap map, and the field that points to "the screen" keeps pointing
   there.
3. The next object is born at that address. In the Z-Wheel, it was exactly the roller's
   `OwnerDrawWidget`: `<created 0x01028e14 -> 0x30000290 reuse true live-surface true is-screen
   true>`.
4. The roller registers its drawing on that object (`Slot16`), and shortly after the object is
   released and **leaves the widget table** — taking the draw registration with it. From then on
   nobody calls the roller's drawing anymore, and the bar disappears.

**The fix, at both ends:** `GetDestination` now returns **with a count** (in `IDisplay` and in
`IGraphics`), and the bitmap's `Release` **does not let the screen bitmap reach zero** — its owner
is the display, not whoever asked. The lock holds even with the right count: it is the screen's
address that must not be recycled, and any new path that hands it out without counting would reopen
the hole.

**What changed, measured** (Z-Wheel alone, 13 virtual s, no window):

| | before | after |
|---|---|---|
| live surfaces | 7 | 9 (three of 440×49, the tabs) |
| colors on screen | 1684 | 4097 |
| instructions in 1765 turns | 207 M | 674 M — the roller started drawing every frame |
| bottom bar | empty | "Jogar / Ajuda" (Play / Help) drawn |
| one `0xe034` key | **zero** bytes change | 25.4% of the screen's pixels change |

With the key, the cylinder moves one position (`zeebo │ Obrigado │ GAMEVIL` → `Obrigado │ GAMEVIL │
EA`) and the stage switches to the box art and the render of the selected game. Two runs **without**
a key come out byte-for-byte identical, so the difference comes from the key, not from noise — that
control is worth running before celebrating anything.

**The rule that stays**, and that holds for any BREW emulator: *every* function that hands out an
object by return value or through an output pointer counts a reference, and objects the system
owns — the screen is the example — must not die from the count of whoever borrowed them. The price
of getting it wrong is not a leak: it is one address serving two owners, and the symptom shows up
far from the cause.

---

## 6. Drawing: where the black screen becomes the right screen

**On the console, the one that draws the interface is the widgets extension, which nobody has.**
The emulator's widgets keep the tree; someone has to decide when to call the game to paint and what
to paint on their own. This section is the most "ours" part of the document — but the symptoms
repeat in any implementation.

### 6.1 The game **never destroys** what it built, and the bar is not a child of the form

Two measured facts that break naive drawing rules:

1. **The Z-Wheel never hides anything** and builds a new form on every turn of the cycle, without
   releasing the previous one: **1722 live roots in fifteen seconds**, each one a whole form.
   Painting all of them stacks the 640×480 welcome screen — which is the **oldest** root — on top
   of everything. The console shows **one** form at a time, and it is the **last one built**.
2. **The bottom bar is not a child of the selected form.** It lives in a container of class
   `0x01028e3f`, which is a loose root. Filtering the drawing "by the current form's tree" **erases
   the bar** even with it visible. And since the shell rebuilds the bar on every turn, including
   *all* instances brings the old trees back to life: only the **newest one with children** will
   do.

The same goes for the main menu: its form **does not receive** the content through item `0x5000`,
is left with zero children, and what it shows becomes a loose root (container `0x30000c10`, with
the status bar and the stage inside).

Our rule, after all this: **registered drawing is explicit** — whoever registered slot 16 wants to
be called —, so we call the draw callback of **every visible widget with registered drawing**,
without filtering by tree. For images and text (which we paint ourselves), there yes we use the
current form's tree **plus** the subtree of the newest `0x01028e3f`.

### 6.2 Coordinates, order and the cost of not deduplicating

- The position comes from `AdicionarFilho` (slot 5), whose third argument points to six words:
  **`{x, y, flag, width, height, object}`**. The intro is hung with `{0, 0, 1, 640, 480, …}` and
  the z-pad pieces with real coordinates (`{100,21}`, `{148,20}`, `{365,20}`). While this was
  ignored, everything was painted at the origin and the screen was a heap in the corner. Width and
  height come **zeroed** when the widget measures itself — do not write zero over what slot 7
  already said.
- **The same slot 5 serves five classes** and not every call has this shape: some pass a function
  in `r2` and an object in `r3`. Only read the position when `r2 == 0`.
- Drawing order is **child over parent** (depth in the tree), with creation order as the tiebreak.
  Using the object's address works until the allocator gets a free list — then the 640×480
  background falls on top of everything again.
- With thousands of live trees, **the same label, in the same place, in the same color, shows up
  thousands of times**. Painting them all gives exactly the same frame and made one frame take more
  than a minute: deduplicate by `(x, y, text, color)`.
- When switching forms, **clear the surface**; it is persistent. But clear it **only on the
  switch**, not every frame: whoever draws through `IDisplay` (most games) has no form at all and
  must not have the screen erased underneath.
- The text color comes from property **`0x140`**, in `RRGGBBAA` format (the Z-Wheel writes
  `0x444444ff`, the gray of the welcome screen). The alpha is discarded: the console's surface has
  no such channel.

### 6.3 Two zeros that crash

- **The list step.** Slot 5 with **`r2 == 4`** is not "hang a child", it is a **getter**:
  `slot5(this, &out, 4)`, and the output is **two halfwords**. `0x8fa04` makes the call, adds the
  two at `0x8fa20`, divides by them at `0x10a18` and multiplies back — it is a list's step, and the
  sum `(height - 20) / step * step + 10` says how many items fit. Writing nothing gives a zero sum
  and **`Arithmetic exception: Divide By Zero`** through semihosting. We return the font's line
  height; it is a hypothesis, but it cannot be zero.
- **The text's `GetExtent`.** In the text/image classes, slot 5 with a pointer that is neither a
  widget nor an image is `IWIDGET_GetExtent(&{cx, cy})`. The status bar uses the label's `cx` to
  position the credits: it reads `cx` at `0x857ec` and adds 375 at `0x85828`. Leaving it zeroed
  writes "10" on top of "Meus Z-Credits" (My Z-Credits).

And a pointer that is not a pointer: when the tree grows, the game calls slot 7 (`SetExtent`) with
`r1` pointing outside the map. Reading from there brings the ARM core down. Ignore what cannot be
read — a size that did not arrive is a size that does not change.

---

## 7. The wheel only turns if the key reaches the right handler

- **The codes are those of `AEEVCodes.h`**: `0xe030` `CLR`, `0xe031`/`0xe032` up/down,
  `0xe033`/`0xe034` left/right, `0xe035` `SELECT`; `0xe064` is the controller's confirm. The proof
  is in the module: `0x44914` is the Z-Wheel's own translation of the analog stick into keys, and it
  maps up, down, left and right to `0xe031` through `0xe034`; the handlers at `0x11888` and
  `0x158c0` treat `0xe030` the same as `0xe04a` and `0xe035` the same as `0xe064`. The controller's
  D-pad sends the four arrows.
- **The key arrives as `EVT_KEY` to the applet**, not through `IHID`. The intro form, for example,
  only moves with `AVK_0` or `AVK_CLR`, which no controller button produces.
- **Dispatch order.** First the handlers of the current screen, from newest to oldest, then the ones
  outside it. The reason is concrete: the z-pad form brings **`0x77300`**, a handler that answers
  "handled" for **any** key. While it came first, no key reached the menu.
- **Which is the current screen**, given §6.1: the newer of "the newest form with children" and
  "the newest root with children". Choosing only among forms leaves the z-pad one as current
  forever. And a root **must have a child**: the game creates loose widgets that never receive
  anything, and the newest of all is usually one of those — painting from it gives a white screen.
- **A confusing line in the report, which is not the problem.** Once the key is sent, the applet
  offers event `0x100` to the root widget's accessor (`mov lr, pc; bx ip` at `0x7b414`, with
  `LR = 0x7b41c`) and tests `cmp r0,#0; bne` — non-zero means "already handled, stop". On refusal,
  it goes on to its own `switch` at `0x7b424`, which compares the **event** against `0`, `1`, `2`,
  `3`, `8`, `0x110`, `0x400` and `0x4ec`: **`0x100` is not there**, and the applet discards it. On
  the console, what would route the key to the focused widget is the interface extension. Here the
  wheel turns anyway because **we hand the key directly to the handlers of the current screen's
  widgets**, without going through that routing. It is worth knowing that the line
  `IWidget::Acessador selector 0x100` in the report is a consequence of this design, and not the
  cause of a key that doesn't work — I already mistook one for the other.

---

## 7.1 Confirm: container focus, and the game list that opens

Confirming on "Jogar" (Play) did nothing. The chain that was missing, in order of discovery:

| Step | What it is | How it was read |
|---|---|---|
| selector `0x711` | sets a container's **focused child** | writes a widget (`0x30000d90`) into the roller's container |
| selector `0x713` | **reads** the focused child | done on the same container, with `&out`; `0x4eb5c` compares it with `[[r4+0x24]+0xb0]` and only acts if it matches |
| selector `0x700` | marks focus on a widget, and with that **on its parent** | the grid never writes `0x711`; it calls `0x700` on the item (`0x399d0`) and asks the item's parent for `0x713` |
| selector `0x702` | "enabled for focus", **one byte** | read with `ldrb`; the action and the `0x700` only happen if it is true. **Hypothesis** |
| `aee_makepath` | joins directory and file | used in two passes: measure, allocate, build |
| `IVector` slot 7 | `ReplaceAt(index, item)` | insertion sort at `0x38e5c`; it is the BREW slot between `GetAt` and `InsertAt` |
| interface `0x0101e443` | a bitmap's canvas; slot 7 hands out an `IDisplay` that draws on it | what comes out is used with `SetDestination`/`GetDestination`/`GetClipRect`; refused, `0x41d68` dereferenced null at `0x41dbc`. **Read from usage** |

With that, confirming on "Jogar" opens the "Novos / Recentes" (New / Recent) ring and the **grid of
games from the local library**, with the box art.

### 7.2 From the grid to the launch

In the list, up and down turn the pages and move the focus between the tab bar and the box art;
left and right switch the tab or move across the box art, depending on the focus. `0xe035` or
`0xe064` opens. The path to the game, step
by step:

| Step | What it is |
|---|---|
| `0x01028e3c` = `IValueModel` | the grid does `SetValue(model, item)`, and listener `0x37704` reads the item with `GetValue`. Slot 3 (`AddListener`) was a "query" that zeroed 0x18 bytes and **erased the function and context of every listener** |
| getter `0x713` with `AddRef` | the grid's handler releases what it receives; without the reference, the count went to zero |
| `IContainer` on the containers | slot 6 `Remove(child)` and slot 7 `GetWidget(ref, bNext, bWrap)`, told apart from the "visible" and "size" reads by the arguments. Without them, `0x80490` ran forever |
| `IROOTFORM_RemoveForm(root, FORM_LAST)` | root slot 6 with `1`. Takes the top form off **without** `Release` and without zeroing the `parent` |
| `CanStartApplet` boolean | it answered `SUCCESS`, which is "cannot" |
| `EnumAppletInit`/`EnumNextApplet` | the Z-Wheel uses the `.mif` name (the module id) of the chosen game |
| `ZeeboMCP` slots 3, 5 and 7 | `ModDataCopyFromENAND`, `ModDataRemoveFromMCP`, `UserDataCopyToENAND` — the copy between `fs:/card3/mod/` and `fs:/mcp/mod/`, which does not exist here |
| `ObjectStore::add_ref` | does not resurrect a released object: it duplicated the address in the free list, and the `IGraphics` of the launch notice was born on top of a bitmap |
| `EVT_WDG_MOVEFOCUS` (`0x711`) with `1` to `4` | `FIRST`, `LAST`, `NEXT`, `PREV`: focus moves among the children that accept focus (`0x702`), and whoever leaves and whoever enters receives `EVT_WDG_SETFOCUS` (`0x700`). That notice is what draws the selection. The key no longer goes to a branch without focus — without this the arrow moved across the box art and switched the tab along with it |
| `IDISPLAY_DrawText` | honors `nChars`, the rectangle and the alignments (`0x20` center, `0x200` middle; checked against the use at `0x30434`). The tab bar is a carousel of centered 214 px labels |
| `utf8towstr` | the terminator only goes in if it fits: the Z-Wheel passes the size without it (`0x8afc8`), and reserving it ate the last letter of every name |
| 16-bit BMP | the box art of nine games is 5-5-5 without a mask |

Without a window, `--installed=0xCLSID:id` registers installed games to test the launch.

What is still missing on this screen:

- **Opening the game — solved.** The last step was slot 3 of `ILCTSystemCtl` (`0x01006c02`): it
  reads the mode through slot 6 and writes it through 3 (modes 0 to 3, hardware flags in the
  firmware). With it, the Z-Wheel clears the forms, writes the `ttgmrun.tmp` marker, arms the 2 s
  timer for `0x81ebc` and calls `ISHELL_StartApplet` with the chosen game's class. Measured without
  a window: with `bench roms/Z-Wheel.zip --seconds=43 --installed=0x010a2337:279369
  --key-names=30500:0xe064,33000:0xe032,35000:0xe034,36500:0xe034,38000:0xe064` (open the list, go
  down to the box art, right twice, confirm), the report says
  `launch:    the shell asked to open 0x010a2337` — Alien Breaker. The bench saves a frame with
  `--dump` half a second after each key, and accepts `--dump-surfaces`.
- **In the window, choosing a game closed the Z-Wheel and nothing opened.** Three causes, found by
  replaying without a window the inputs recorded in the window's report (`zeebx session`, which uses
  the same `Session`, the same library and the same controller mapping):
  1. **The Z-Wheel launches by exiting.** On confirm, it writes `ttgmrun.tmp` and
     `StringLastAppRan`, tears down the screens and exits on the `0x81c38` timer. The console
     reopens it, and at startup (`0x81904`) it sees the marker and opens the game two seconds
     later. The window now reopens the Z-Wheel when it exits on its own (`session::Z_WHEEL`). A
     marker left by a run that crashed is picked up on the next startup — that is why the Z-Wheel
     sometimes opened a game by itself.
  2. **Widgets died without their releasers.** The triple `{function, context, releaser}` lost its
     third word, `RemoveForm` did not release the root's reference and the children were
     released only by the count. The tab bar's roller stayed alive with its 40 ms timer (`0x2fc88`)
     armed over memory the Z-Wheel had already released, and the next tick jumped to address zero.
  3. **The surface region did not recycle.** Box art and 440/441×49 bitmaps every frame used up the
     8 MB after 27 s of browsing; from then on canvases and message boxes were left without pixels.
- **The 3D stage came out gray, and then with streaks.** Two defects in sequence:
  1. The Z-Wheel composites the stage by copying the pbuffer to the screen with the helper's
     `MEMMOVE`, and the write watch only saw writes from ARM code. The copies requested from the
     helper (`memmove`, `memset`) now mark the range (`CpuBackend::mark_dirty`).
  2. The pbuffer is 640×330, but the rasterizer stayed at 640×480 and the Z-Wheel never calls
     `glViewport`. The export and the import of `eglGetColorBufferQUALCOMM` went through scaling
     and did not come back to the same rows: the background the game copies every frame did not
     reach a third of them, and the animated sword left dotted trails. On `eglMakeCurrent` of a
     surface with a size other than the screen's, the GL surface and viewport now become its own.
- **The home screen wheel has two items in this ROM, and that is right.** `tectoy.cfg` carries
  `EOL=1` and `zeebomenu_hide=1`, which set bits `0x2000` and `0x4000` of `app+0x3614`; the build-up
  at `0x4f620` removes the Zeebo and Configurações (Settings) items and replaces the last one with
  "Ajuda" (Help). Screenshots with more items are from versions before the end of life.
- **Focus frame.** `MOVEFOCUS` with an explicit widget (`0x711`) now notifies whoever leaves and
  whoever enters with `EVT_WDG_SETFOCUS`, through the game's handler. The roller's (`0x6089c`)
  writes the focus at `[+0xac]`, which is what `DrawRollerExt` tests to draw the blue frame.
- **"Ajuda" opens, with a placeholder.** The FAQ screen needs the HTML widget (`0x0102dd32`). The
  class joined the widget family, and the build-up only passed once the widget's property `0x161`
  started returning an object: `0x31c34` does a `QueryInterface(0x102d691)` on it without checking
  for zero, and aborted silently. There is no HTML renderer: the widget shows the text of
  `aparelho/z-wheel/ajuda.html`, created with default text and freely editable (block tags become
  paragraphs, the rest disappears).
- **A covered form does not draw.** The owner-draw widgets were all called, and the menu's stage
  fell on top of the help and showed up behind the game list. Whatever belongs to a form that is not
  the top one is now left out; what is not under a form, like the status bar, stays.
- **Back is button 2**, as the Z-Wheel's help says, and it sends `AVK_CLR` — measured on the help
  screen.
- **Widget background color.** Property `0x130` is the background, in `RRGGBBAA` like the text's
  `0x140`. The Z-Wheel writes zero alpha on most widgets and `0xd0d0d0ff` on the container each form
  hangs on `0x5000`: it is the light gray behind the stage, the wheel and the game list. It is now
  painted every frame, before the images, and that also covered what was left over from one frame
  to the next — the gray square where the grid's page arrow sits and the little blue dashes under
  the tab bar.
- **The box art frame only on the sides is right.** The selection image in `tectoyli.brf`
  (178×267) is just that: two blue columns with a transparent middle. The wheel's frame (214×47)
  also checks out.
- **The "Aguarde" (Please wait) screen on launch.** When exiting to open a game, the Z-Wheel swaps
  files in its own directory (`0x81db0`): if `gamestartrgb.sav` exists, it renames
  `zeebosplash.rgb565.raw` to `.sav` and `gamestartrgb.sav` to `zeebosplash.rgb565.raw`. On
  reopening it undoes the swap in the first millisecond (`0x822bc`). The one that reads the file in
  that interval is the console, when reopening it, and the image (640×480 RGB565) is "Aguarde
  enquanto o aplicativo é carregado" (Please wait while the application loads). The reopened
  Z-Wheel does not draw during the two seconds until `StartApplet`, and the game starts with the
  screen it left. The session now paints this file when opening the Z-Wheel, and the game it opens
  inherits the last screen (`Session::inherit_screen`) — before, both intervals were a black screen.
  At boot the file is the "Bem-Vindo ao Zeebo" (Welcome to Zeebo), and the Z-Wheel's intro draws its
  own version on top.

### 7.3 Bottom wheel, tabs, return to the menu and transitions

- **The bottom wheel's items are 214×34 images from the resources**, built at `0x4f620`: Jogar
  (Play, `0x138f`), the zeebo logo (`0x138e`, in `tectoyli.brf`, if `0x4000` is clear), Comprar
  (Buy, `0x138d`, if `0x2000` is clear), Navegar (Browse, `0x13ef`, bit `1`, from `enable_browser`),
  Desligar (Power off, `0x13f0`, bit `0x40`) and, last, Ajuda (Help, `0x13f1`) with `0x2000` or
  Configurar (Setup, `0x13a2`) without it. The cfg read at `0x7fe80` confirms it: `EOL` sets
  `0x2000`, `zeebomenu_hide` sets `0x4000`. The "End-of-life Z-Wheel" option (General tab) hands the
  Z-Wheel a copy of `tectoy.cfg` in the device profile with both at zero.
- **`SourceFromFile` reread the file by name.** The cfg is read line by line through an `ISource`
  made from the `IFile`, and the `ISource` was created by resolving the guest name again — which
  pointed to the package's cfg, not to the opened copy. `OpenFile` now keeps the real path.
- **Translucent selection frame.** The selection image (`214×47`) has a blue middle with alpha 51.
  The PNG was reduced to opaque/transparent by the cutoff at 128; alpha with half-tones is now kept
  and blended when drawing the image.
- **Coming back from help locked the top carousel.** The menu's handler (`0x4e0e8`) reads
  `SETPROPERTY(0x5064)`, `FID_ACTIVE`: with zero it stops the stage and the wheel (`0x4e3d0`),
  with one it gives back focus, stage and sound. The help exits with an explicit
  `RemoveForm(root, form)`, and the form that came back to the top never received the `1`. Both
  `RemoveForm` paths now notify the new top, **on the next turn of the loop** and only if it is
  still on top: when launching a game, `0x78a94` takes the top form off and the Z-Wheel tears down
  the rest in the same call. Notified on the spot, the menu reactivated in the middle of the
  teardown, the game list opened an empty message box and the game did not launch. In the window,
  the launch request is checked before the Z-Wheel's exit, for the same reason: the reopened one
  asks for the game and exits in the same turn.
- **The tabs show each one's page.** On a tab switch, `0x3220c` opens the file, makes an `ISource`
  out of it and hands it through slot 5 (`0x322e0`) of the object of property `0x161`. The content
  is kept per widget and drawn in place of the placeholder, with accent entities decoded; with no
  page, the placeholder applies. The arrows scroll the panel when it has focus, as the firmware's
  HTML widget would — the help container writes its own HTML widget (its grandchild) as the focus.
- **Transitions.** `0x798c4(app, type, 1)` decides whether the switch slides: a type in
  `SlideOnceToForm` only slides if it is not yet in the preferences' `HasSlidToForm`, which it marks
  the first time. The cfg carries `31` and the dump's `tt_prefs.db` already comes with `14` (Jogar,
  Configurar, zeebo), so nothing slid. The "Z-Wheel transitions on every screen change" option (on
  by default) puts `SlideOnceToForm=0` in the cfg copy. The animation itself:
  1. `0x74e20` takes a snapshot of the screen, creates an offscreen bitmap, does `SetDestination` to
     it, releases its own reference and pushes the form; it arms 400 ms for `0x74434`.
  2. `0x74468` takes the destination (the new screen), builds 640×1440 (old, the `transitions.png`
     background with the title, new), and sets the destination back to the screen.
  3. `0x74258` slides 5 pixels at a time **in a single loop**, with `IDISPLAY_Update` at each step;
     at the end it sends `0x7001` and reactivates the root.
  Two things were missing: `SetDestination` did not hold a reference (the bitmap died and the
  destination became another object — the transition stopped on the old menu forever), and the
  screens of each `Update` inside a callback never reached the window. The machine now keeps the
  screens of a turn with more than one `Update` and the session shows them, one per frame, before
  the game goes on.
- **`ISHELL_CloseApplet`** records the request; the session delivers `EVT_APP_STOP` and ends as a
  normal exit. A game opened by the Z-Wheel that exits this way gives the window back to the
  Z-Wheel.

## 7.5 The scripted proof of the launch (2026-09-22)

The cycle now closes **in the scan**, with no window and no RetroArch, with a single command:

```bash
ZEEBX_ROM="Z-Wheel (Brazil) (Es,Pt).zip" \
ZEEBX_ROM_MS=70000 ZEEBX_ROM_CAP=300 ZEEBX_ROM_INSTALLED=auto \
ZEEBX_ROM_KEYS=30500:k0xe064,33000:k0xe032,35000:k0xe034,36500:k0xe034,38000:k0xe064,45000:k0xe032,48000:k0xe064 \
cargo test --locked the_given_rom_progresses -- --nocapture
```

and the report answers with the outcome that does not lie — `Session::take_launch_request`, the same
hook the core uses to switch sessions:

```text
state: finished on its own
requested launch: 0x0108e356        (Alien Breaker Deluxe)
```

The script goes the whole way: confirm on "Jogar" (`0xe064`), go down to the box art (`0xe032`),
move two box arts to the right (`0xe034`), confirm again. The grid appears drawn from 37.6 s on,
with the local library's games in three columns.

**Two traps of the instrument, measured here:**

- **The `id` of `--installed` is mandatory**, and it is the **number of the module folder** inside
  the package (`fs:/mod/N`). Without it the Z-Wheel logs `Tectoy.c:2925 No mod number for this
  game!!!` and gives up — that is what happened with the class list without ids. That is what
  `ZEEBX_ROM_INSTALLED=auto` solves: it scans the ROM folder and builds the `0xCLSID:id` pairs from
  the packages.
- **The cache is game state.** The Z-Wheel writes `zeeboprefs.dat` and `ttgmrun.tmp` **inside the
  extracted package**, and on the next boot it reacts to them — launching the last game in 2 s,
  without going through the script. Measuring the fresh path requires clearing
  `~/.config/zeebx/cache` **and** the device profile before each run; without that the same command
  gives different results in consecutive runs.

**What still does not close, measured:** the Z-Wheel logs `check_malloc: Malloc failed in Tectoy.c
at line 570` after allocating ~64 MiB of its own pool (its `memcheck` prints `Free(67001392)`), and
exits. The failing request **tracks the heap size** — 67,001,488 with 64 MB, 134,110,352 with
128 MB —, so growing the heap only moves the target: it asks for whatever is left, not for a need of
its own. What remained from this measurement is honest accounting of "how much is free"
(`Heap::largest_block`), which used to add holes to what is left ahead and answer a number no
allocation can get.

### 7.6 The proprietary widget `0x01028e19`, measured from usage

The widget family is served as a block (by numbering proximity), and what tells one class from
another is not in any header. The per-class accessor census (`ZEEBX_ROM_SELECTORS=1`) answers
**from usage**, and in the Z-Wheel `0x01028e19` — the only one of the trio `0x01028e19` /
`0x01028e2a` / `0x01028e4b` that this script creates — receives two selectors, and nothing else:

```text
class 0x01028e19  selector 0x800  (5x)
class 0x01028e19  selector 0x801  (10x)
```

`0x800` is an item read and `0x801` the write; the items are **typed**, with the cutoff measured at
`0x5000` — from there up the item holds an object, below it holds a number. It is the same path the
whole family uses, so in this cycle **what the proprietary class asks for is the generic behavior**
— and that is what the emulator does.

**What is not reproduced, and stays on record:**

1. **The class's own drawing.** We paint images and text through a declared substitute, not by
   emulating the widgets extension — whatever the class would do differently when drawing is not
   measured here.
2. **Any behavior of it outside the paths this script exercises.** The census measures *usage*;
   absence of usage is not absence of behavior.

### 7.7 The core did not translate the controller into console keys

`input::controller_key_events` has always existed and is what turns the **controller** into the
**BREW keys** the applications read (`0xe031` to `0xe034` for the D-pad, `0xe064` for button 1).
The window and the desktop use it. The Libretro core **never called it** — it delivered the
controller through `set_port_pad` and stopped there. Measured on 2026-09-22, through the path
RetroArch uses:

```text
without the translation   the wheel animates forever: 13 to 14 distinct images in every 24
                          frames, on every step of the script, and no key is handled
with the translation      55 distinct images in the 100 frames before the key (animating) and
                          1 distinct image in the 100 frames after the confirm (handled)
```

It is the difference between "the controller reaches the guest" — which was measured — and "the
controller reaches the applet the way it reads it". On the device this is RetroArch handing the
controller to an application that only understands console keys.

**What this path still does not close, measured.** With the **same keys** and the same machine, the
scan requests (`requested launch: 0x0108e356`) and the core does not. Three measurements narrowed
down where the difference is, and none of them is the key:

1. **It is not patience.** After the script, another 6,000 frames (100 s of virtual clock) and the
   request is still zero: the wheel is **stuck** after the confirm (1 distinct image in 100 frames),
   not slow.
2. **The pacing is not the frame count.** Measured with a test clock: each `retro_run` advances
   **~26 ms** of virtual clock, not the 16 ms of a frame at 60 Hz, because the core's turn ends when
   the machine **presents** a frame. Counting frames, the script started at **50.6 s** instead of
   the 30.5 s of the scan's script — another point on the timeline, another screen, another outcome.
3. **And the same instant is not the same state.** Driving by the clock, at **30.5 s** the wheel on
   the core path is **still** (1 distinct image in 100 frames) while on the scan's it animates. In
   other words: the two paths are not at the same point of the timeline when the clock reads the
   same number — and that is what the next experiment has to separate (what the scan presents that
   the core does not, or the other way around).

The instruments stay in the test: `CURRENT_CLASS` (who is running) and `CLOCK` (at what instant),
and the same instants of the scan's script (30,500 / 33,000 / 35,000 / 36,500 / 38,000 ms) apply to
both paths.

**4. The virtual clock stops, and that is what traps the wheel.** Measured with a script that keys
every two and a half seconds between 25 s and 75 s: **517,036 frames delivered to the frontend with
the virtual clock at ~25.4 s** — 426 ms of virtual time in half a million frames. `run_frame` closes
each turn when the machine presents a frame *or* when it spends `MAX_STEPS_PER_FRAME` (200,000)
turns; with the guest in a loop that does not advance the clock, the turn budget runs out **before**
the next timer expires, and the clock — which is what fires the wheel's timers — never moves. The
wheel does not get slow: it stops.

**5. Releasing the key is not the factor.** In the scan's script the key is **pressed and never
released** (there is no `Solto` (released) step in the script that requests the launch, and
`ScriptStep::Key` only pushes `(avk, true)`), while on the core path the button was pressed and
released 300 ms later. Holding the button down in the core test, the outcome is **the same**: 1
distinct image in 100 frames and a zero request. The difference is not in the press/release pair.

**6. With serial capture in the core, the question became the focus.** The core got the same
instrument as the scan (`ZEEBX_CORE_SERIAL`), and it shows what the wheel does inside on both paths.
The difference is in one query:

```text
scan   [  30508 ms] ... AND GAMEINFO.class_id = -1 -> 0 row(s)
       [  38022 ms] ... AND GAMEINFO.class_id = 17359702 -> 2 row(s)   <- the focus, and the request goes out
core   [  50611 ms] ... AND GAMEINFO.class_id = -1 -> 0 row(s)
       [  53033 ms] ... AND GAMEINFO.class_id = -1 -> 0 row(s)        <- the key is handled, the focus does not come
```

`17359702` is `0x0108E356`, Alien Breaker — the same one the scan asks to open. In the core the key
**is handled** (the wheel creates the `IValueModel` at `0x01028e3c`, hangs `0x5000` on the form and
sets `FID_ACTIVE`), but the focus never becomes a game: the query stays at `-1`. This is the
question for the next experiment — **where the wheel gets the class of the focused item from, and
why it comes out `-1` on the core path** —, and now there is a way to measure it, because the
instrument exists on both sides.

**7. The widget tree is the same, and the first confirm does the same — the divergence comes
later.** Comparing the two captures at the instant of the first key, the trees are **identical**,
line by line: same addresses, same classes, same handlers. The `0x300007d0` that §7.6 measures
(`class 0x1028e19`, 39×25 at (100,21), two children) is in both, hung on bar `0x1028e3f`, whose
handler is `0x77300` — the one that answers "handled" for any key.

And the **first** confirm does the same on both: `IWeb` (`0x0100550d`), display, and
`prop 0x5064=0x1` (`FID_ACTIVE`) — in the scan at 30,910 ms, in the core at 53,434 ms.

The divergence shows up **on the next key**: in the scan, the down arrow (33,023 ms) and the right
one (35,023 ms) produce activity (`IWeb` + display, once each); in the core, the following keys
produce **nothing** in the capture. Same tree, same first effect, and the second key gets lost —
which points to **event delivery**, and not to the screen state. It is the only variable that
differs between the two paths: there the key comes in as a console key, here it is translated from
the controller.

**8. And only **one** key reaches the machine.** The instrument that was missing — the delivery
line, in the dispatch itself — answered right away. In the whole run of the core path, the machine
received **a single pair**:

```text
[36612 ms] <key 0xe064 press to 2 handler(s) on screen and 0 off, handled>
[36771 ms] <key 0xe064 release to 2 handler(s) on screen and 4 off, handled>
```

The **seven following keys of the script do not reach the machine** — neither as press nor as
release. This is not the applet ignoring them: it is the event not arriving. With the first key
handled and the others absent, the question switches sides: either the **controller translation**
does not produce the following edges, or the machine's **queue** is not drained after that
dispatch. It is the next experiment, and it is about code, not applet behavior — the
instrumentation for it is already in the capture.

**9. And only one key arrives, in three variations.** The same instrument, run on both paths,
compares what enters the machine:

```text
scan  [30510] 0xe064 press   [33023] 0xe032 press   [35023] 0xe034 press
      [36508] 0xe034 press   [38024] 0xe064 press      <- the five of the script, all handled
core  [36612] 0xe064 press   [36771] 0xe064 release    <- a single pair, and nothing else
```

And the fact holds across three variations, all measured: different buttons, the **same** button
four times, and a press of **120 frames** (against 8) — in all of them, the machine receives **one**
key and the following ones do not show up, neither as press nor as release. In the scan, the
capture shows all five, one by one, each handled by two handlers.

This is not the applet: it is the **delivery**. After the first key the core path takes no more
keys to the machine, and that is the question for the next experiment — with the instrument already
on both sides (the delivery line in the capture) and the scan's script as the reference for what
should show up.

**10. The pad changes, the delivery does not arrive — and there is a shortcut that eats arrows.**
Measuring the translation directly (a temporary print of what `read_pad` returns per frame), the pad
changes **correctly** at every step:

```text
0x0 -> 0x1       press button 1 (the confirm)
0x1 -> 0x1  ...  hold
0x1 -> 0x0       release
0x0 -> 0x4000    press the D-pad down
```

In other words: the controller reading is right and the edges exist. What does not happen is the
**delivery** — the capture records one key and the others do not show up. And in the key dispatch
there is a path that **consumes the arrows before delivering them**: the focused HTML scrolling,
which returns "handled" for up/down when there is an HTML widget with focus, and leaves the loop
before calling any handler. Also worth a look: its `continue` does not go through the capture line,
so an arrow consumed there **leaves no trace** — which makes "the key did not arrive" and "the key
was consumed by the scrolling" look alike.

**11. What was refuted, so the path is not repeated.** Two hypotheses fell to measurement, and both
were good:

- **"the session stopped after the first key, and that is why the queue is not drained"** —
  refuted: the machine keeps executing **84,360 instructions per frame** after the confirm. A
  stopped session executes nothing.
- **"the HTML scrolling shortcut eats the arrows"** — insufficient: it only consumes **up and
  down**, and the **right** arrow does not reach the capture either.

What is left, measured: the pad **changes** at every step (`0x0->0x1`, `0x1->0x0`, `0x0->0x4000`),
the edges exist, `Session::set_key` delegates to the machine's queue with no intermediary — and the
capture records **one** key. Between "the queue received it" and "the dispatch delivered it" there
is nothing else on the path besides those two lines. The next experiment is to instrument exactly
those two points, in a single commit: a line in `Session::set_key` (entered the queue) and the line
that already exists in `flush_keys` moved to the **top** of the loop (left the queue, before any
shortcut consumes it).

**12. The picture that closes, with two facts that still do not fit.** Three measurements of the
same run:

- the pad **changes** at all seven steps (`0x1`, `0x4000`, `0x8000`, `0x8000`, `0x1`, `0x1`,
  `0x4000`);
- the translation **works in the core crate** — proven with hand-built pads:
  `translation of b1: [(57444, true)]`, which is `0xe064`, the confirm;
- and the capture records **one** pair of keys, with the virtual clock **stopping at 37,012 ms**
  after the confirm.

What **fits**: with the clock stopped, the guest no longer reaches the API boundaries, and those are
where the key queue is drained — the following keys stay **stuck in the queue**, and that is why
the capture does not show them. This explains the whole symptom (still screen, keys with no effect)
without needing any new defect.

What **does not fit yet**, and is the next experiment: the guest executes 84,360 instructions per
frame after the confirm, and a turn with that much work should return from `advance` and go through
`deliver_signals`. If it does not, the question is **where the turn gets stuck** — and the
measurement is a line in `advance_once_inner`, saying whether `deliver_signals` runs after the
confirm.

### 7.8 The cause was in the core: the intermediate `Update` screens were not consumed

When the guest calls `IDISPLAY_Update` **inside a callback** — and the Z-Wheel's transition does
that in a synchronous loop, some two hundred times —, the machine keeps one screen per call
(`update_frames`). The one that shows them is the frontend, **one per frame**, without advancing the
clock: the window consumes them (`ui/mod.rs`) and the windowless session does too (`main.rs`). **The
Libretro core did not consume them.**

With the queue full, `advance_once` returned "presented" **forever** — without running the guest,
without an API boundary, without draining the key queue. Measured, in the Z-Wheel through the core
path:

```text
before   clock stopped at 37,012 ms · 1 key pair in the whole run · launch request 0x0
after    clock reaches 78,983 ms    · the 8 keys of the script, one by one, each handled
```

And the cycle closes, on the path RetroArch uses:

```text
step 1: open 0x0108e356, 11 distinct signature(s)
after the key: 11 distinct images in 100 frames, clock 566 ms   <- the new session, the game running
launch request on the core path: 0x0108e356
6886 frames, 19,088,154 instructions in 61,814 virtual ms
```

`0x0108E356` is **Alien Breaker**, the same game the scan requests — and the clock at 566 ms is the
game's session, which started from zero: the core **opened and ran it**. The return to the wheel is
measured and not enforced: it depends on the game ending on its own, and what the grid puts in focus
is the first title in the folder.

**The return to the wheel, measured and not yet proven.** The core test's script became
configurable (`ZEEBX_CORE_KEYS=ms:id`, with the RetroPad `id` — `1` is `Y`, the confirm), so the
proof can be scripted as in the scan. With it, the obvious attempt — a folder with the Z-Wheel and
**a game that ends on its own** (`Zeebo Clube`, measured in the scan) — **does not launch**: the
wheel reacts to the confirm (50 distinct images) and the request stays at zero. The grid depends on
the **content** to place the focus, and with two titles the navigation script moves the focus away
from the only one that exists.

With the full folder (62 games), the doc's script launches and runs (`0x0108E356`), and the return
stays at `false` because **the focused game — Alien Breaker, the first in order after the
navigation — does not exit on its own**. It stays measured and on record: what is missing is a
script that opens, in the full folder, one of the two titles that end on their own.

**The return to the wheel: what the measurement narrowed down.** Three folders, the same test, the
same default script — and only one launches:

```text
62 games (the full folder)   launches and runs (0x0108E356); the return stays at false because the
                             focused game does not exit on its own
3 games (Action Hero 3D, Alice, Zeebo Clube)   does not launch: the first key produces 50 distinct
                             images (a transition) and the request stays at zero
2 games (Zeebo Clube)        does not launch, with the same symptom
```

In other words: **the grid only places a reachable focus with a large catalog**, and that is what
the script's navigation needs in order to choose — and not the number of titles as such, because the
list is built from the catalog. The return to the wheel therefore depends on two things at once: a
catalog where the script finds the focus, and a game that **exits on its own** at that focus.
`Zeebo Clube` and `Zeebo App` are the two that exit (measured in the scan), and neither is reached
by the script in the full folder.

Four attempts, all with the same outcome — the launch **only happens with the full folder**:

```text
2 games, the doc's script                      does not launch
3 games (Action Hero 3D, Alice, Zeebo Clube)   does not launch
3 games with a title that STOPS RIGHT AWAY     does not launch
4 games, the scan's instants (30.5 to 38 s)    does not launch
```

The third was the most promising: `Zeebo Channels - Opera Mini` **does not create the applet** and
the session stops right away, so the return to the wheel would be exercised for real. And the
grid's title comes from the **package name** (it is what the scan writes into the catalog), so I
renamed the copy to `Alien Z` so it would land in third alphabetical position — which is where the
script puts the focus in the full folder. Even so, no launch.

What this narrows down: **it is not the title nor the number of files** — it is what the grid
builds. With few titles it does not offer the focus the script reaches, and with many it does, but
then the focus lands on a game that does not exit on its own. Both conditions have to hold at the
same time, and that is what is left to set up.

**The bar asks the list every frame, and we answer zero.** In the capture, the pair `(bar, list
address)` shows up **14,319 times**, once every 5 ms — a query loop from bar `0x01028e3f` to roller
`0x30000ad0`, with the **child's address in place of the selector**. It is the case §5.3 already
knows ("the accessor is called with a child's address in place of the selector"), and our answer is
the bare `OK` of that branch.

What makes this the next lead: **it is a repeated question**, and what the bar does with the answer
is not measured. If it asks every frame to know *which item is chosen* — or *how many* the list
has —, the zero we return is the difference between the grid choosing and not choosing.

**Where the game list is, measured in the tree.** The serial capture dumps the widget tree on the
first key, and in it the wheel has **two** rollers (class `0x01028e14`):

```text
0x30000ad0  handler 0x75724  640x330 at (0,50)     <- the game LIST
0x30000cd0  handler 0x6089c  440x49 at (100,405)   <- the bottom wheel (Jogar, Comprar, ...)
```

The bottom one is the one doc §7.3 already knows (it is `0x6089c` that draws the blue frame when it
receives `SETFOCUS`). The top one, `0x30000ad0`, with `640x330` — the whole grid area — and handler
`0x75724`, is the **game list**: that is where the choice lives that the confirm turns into the
launch's `class_id`.

It is the next experiment, and now with an address and a handler in hand: log what `0x30000ad0`
receives while the script navigates (the census already shows class `0x01028e14` receiving `0x801`
ten times) and what it does on the confirm.

**The last-played hypothesis fell.** Clearing the cache (where the wheel writes its state, inside
the extracted package) the launch is still `0x0108E356` — the same Alien Breaker. The choice is
**deterministic** and does not depend on the state previous runs left behind.

What still stands, measured: the grid's query returns 120 rows merging official and local entries,
and the game the script reaches is the same with or without the renamed copy I inserted. In other
words, the script reaches a **game**, not a **position** in the list — which only closes with a
reading of the grid itself, which stays as the next experiment.

**The list is merged, and the order is not alphabetical.** Measured in the catalog the wheel opens:
**59 official titles** (short — `Alice`, `Alien Breaker`) plus **61 local ones** (the package name —
`Alice no Pais das Maravilhas (Brazil) (Es,Pt)`), **120 rows** in the grid's query. Sorted by title,
the first six are:

```text
Action Hero 3D                                  (official)
Action Hero 3D - Wild Dog & IMICRO3D (...)      (local)
Alice                                           (official)
Alice no Pais das Maravilhas (...)              (local)
Alien A (Brazil) (Es,Pt)                        (the broken copy, renamed by me)
Alien Breaker                                   (official)
```

The launch, even so, was `0x0108E356` — **Alien Breaker**, the sixth. So the grid's focus **is not
the third row of the merged list**. What is left as a measured explanation is the wheel's own order:
it keeps `dt_lastplayed` and has the "Novos / Recentes" (New / Recent) tab, and `Alien Breaker` is
exactly the game **these runs have been opening** — the most recent is the first.

Fifth attempt, with the **full** folder and the broken title arranged into third position: symbolic
links to the 62 packages (without duplicating 705 MB) plus a real copy of `Opera Mini` renamed to
`Alien A`, which sorts **before** `Alien Breaker`. Result: the request is still `0x0108E356` —
**Alien Breaker**, not `Alien A`. The grid's focus **does not follow the title order** I assumed:
the report's `class_id` (`17359702`) is what the grid chooses on its own.

That closes what can be measured without going into the grid: launch and execution proven on both
paths, and the **return** depending on a focused item that exits on its own — with the grid not
obeying the order seen from outside.

It is the next experiment, with the target already narrow: **find out how the grid chooses the
focused item in the full folder** (the report says `class_id = 17359702`, the third in title order)
and reproduce that condition in a folder where the third is a title that exits — or accept, with the
measurement in hand, that the return to the wheel is only demonstrated on the device.

**13. Storage changes the instant, not the trap.** The scan's path uses the default storage (the
user's config) and the core's uses what the frontend hands it; pointing the core test at the user's
tree (`ZEEBX_CORE_SYSTEM=$HOME/.config/zeebx`), the same number of frames takes the wheel to
**36.6 s** instead of **50.6 s** — the device state changes when things happen, which confirms that
the two paths are not compared by clock. But the trap after the confirm stays the same (1 distinct
image in 100 frames, zero request). It is not an artifact of the folder.

The math, with the numbers that came out (the test got `INSTRUCTIONS` next to `CLOCK`; the
`Session::instructions` accessor already existed):

```text
before the key   1,060,495,350 instructions · 55,088 virtual ms · 55 distinct images in 100 frames
after the key    1,068,931,395 instructions · 51,012 virtual ms ·  1 distinct image in 100 frames
                 -> +8,436,045 instructions in 100 frames = 84,360 per frame, and 4.2 ms of clock
```

**The wheel is neither stuck nor idle: it runs and does not present.** 84 thousand instructions per
frame is real work (the same level as the scan, ~17 million per virtual second), and the clock moves
4.2 ms per frame instead of the 16.67 of a full frame — because the turn ends earlier, with the
machine handing back control without swapping the buffer. What it does in this state is **wait for
an event**, and that is what the RetroPad keys do not produce on the core path — and what the scan's
script does produce, because there the keys come in as console keys and not as a controller.

Add to that the cap on turns per frame (item 4 above): with the guest in a loop that hands back
control without presenting, `run_frame` spends the budget before the next timer expires.

The next experiment is this, and it has a single question: **how many turns each path spends per
virtual frame, and how many of them advance the clock**. The scan takes one turn per iteration and
the core accumulates up to the cap; if the cap is hit in a loop without clock, the difference
between the two paths is there, and not in the script or the key. The instrument for this
(counting engine turns per frame) does not exist in the core yet: `Session::machine` is private to
the engine, and exposing a public accessor is a change in `src/`, which costs the twelve CI jobs —
it is worth doing together with another change, not on its own.

### 7.9 Return to the wheel, observed in the serial capture

The capture of the core path records the return the old test could not assert through
`CURRENT_CLASS`: after the game's session, the Z-Wheel builds its objects again. In
`/tmp/who.txt`, measured with the full script, at `78611 ms` `0x01028e14`/`0x01028e05` and the
`IValueModel`s show up again; at `85451 ms` it rebuilds the bar, reads the list and queries
`GAMEINFO.class_id = 17359702`. The same stretch writes `StringLastAppRan = 279369`, updates
`playcount/dt_lastplayed` and destroys the game's EGL context:

```text
78611 ms  class 0x01028e14 · class 0x01028e05 · IValueModel · GAMEINFO
85452 ms  StringLastAppRan = 279369 · UPDATE GAMEINFO · eglDestroyContext · eglDestroySurface
```

This is the return to the shell observed in the guest: the chosen game exits, the core reopens the
Z-Wheel and it builds the library again. The core test still reports
`went back to the Z-Wheel after the game: false` when the waiting window ends before this outcome or
when the focus lands on a title that does not exit; the capture is the most direct evidence of the
path that really happened.

## 8. What still does not work

### 8.1 Measured on 2026-09-22: the wheel does not react to keys, and the cause was a class

With the scan's script (`ZEEBX_ROM_KEYS=30500:k0xe064,33000:k0xe032,…`) the wheel comes up, draws
and **does not react**: the five frames saved half a second after each key come out **identical**
(same `sha1`), 86% white, and its log ends in
`Shop_Action.c:77 Unable to create instance of IDOWNLOAD in ShopAction_Init` followed by
`GameLib_Form.c:1315 Unable to init ShopAction in Gamelib_CheckPreLoaded` — the game library is not
built, and without a grid no key has anything to move.

`IDOWNLOAD` is class **`0x01000000`**, which was refused as "only `QVERSION`". The 4.0.2 SDK's
`AEEClassIDs.h` says, in three consecutive lines: `QVERSION` is `0x01000000`, `AEECLSID_PRIV` is
`QVERSION` and **`AEECLSID_DOWNLOAD` is `AEECLSID_PRIV`**. The earlier reading stopped at the first
of the three.

Served by a probe, the abort **disappears** from the log and execution goes further (473,828 →
474,692 instructions; 7,353 → 7,394 API calls), but the wheel still **does not build the menu**: it
stays on the pulse described in §4 — one timer, 2,582 turns of 16.6 ms reading points and the
download queue, without drawing anything new. That is where the investigation is now.

**The widget tree at the first key explains the "does not react".** With serial capture on in the
scan (`ZEEBX_ROM_SERIAL`), the dump at the first key (30,533 ms) shows **three** widgets: the root
form `0x1028e51` with two children, plus two objects of class `0x0`, with no handler, no size and no
text. There is no stage, roller, bar or grid — there is nothing for a key to move. What is seen on
screen is the *splash* inherited from the session, not the wheel's drawing.

And the capture says what it does instead, in order:

```text
  0 ms  class 0x0100104f  ... opened tt_prefs.db, asset_cache, tt_game_info, tt_dlqueue.db
  0 ms  sql SELECT * FROM GAMEINFO, TITLETEXT WHERE ... AND GAMEINFO.class_id = -1 -> 0 row(s)
  0 ms  class 0x01028e47 (form) · 0x01028e19 · 0x01028e3f (bar) · 0x0100550d (IWeb) 2x
  0 ms  prop 0x216=0x1 on 0x30000650
  6000 ms  prop 0x216=0x0
  7000 ms  prop 0x216=0x0 · class 0x01006c01 (SIM card)
30533 ms  first key: 3 widgets
```

In other words: the query on `tt_game_info` for `class_id = -1` comes back **empty**, `IWeb` is
created twice and never called, and flag `0x216` is set and cleared between 6 s and 7 s. After
that the wheel stays still. That is where — and not at the input — item 8 is stuck.

### 8.1.1 Slot 5 of the system control: the first key is not inert

Pressing confirm **early** (3 s, not the 30 s of the doc's script) is not inert: the scan aborted
with

```text
error: loop stopped at Unimplemented { addr: 0xf0030014, args: [0x300006d0, 2, 0, 0xf0030014], caller: 0x81898 }
```

and the address decodes through the emulator's own scheme — `API_BASE + interface*0x1000 + slot*4`,
with `API_BASE = 0xf0000000` — to **`Interface::SystemCtl` (48), slot 5**. In other words: on
confirm, the Z-Wheel calls slot 5 of the system control, with the mode in `r1 = 2`, and we did not
have it.

The slot table had `"slot5"`, which is a **placeholder**: `aee::is_placeholder` makes the dispatch
return `None`, and execution stops there on purpose — "a hang is better than a wrong launch". The
name came from the firmware, disassembled from `1.1.2_APPS.bin` (`tools/firmware.py`, Thumb):

```text
vtable 0x10691ea8     slot[3] = 0x10e9fdb6   (DefinirModo)
                      slot[4] = 0x10e9fe7a
                      slot[5] = 0x10e9fe92   <- the one the wheel calls
                      slot[6] = 0x10e9ff84   (Consultar)
```

Slot 5 receives `(this, mode, option)` — the option with `-1` meaning "the device's", read from
`0x114287ec` — and **the last thing it does is `bl 0x10e9fdb6`**, which is the body of
`DefinirModo`. It is slot 3 with one more argument. Implemented that way, the abort **disappears**
and the wheel starts receiving the script's seven keys without stopping — but it **does not
redraw**: the seven frames stay identical (same `sha1`), and the launch request does not show up.

### 8.1.2 The cause: the SIM card class was being offered

`268d6fb` (09-21) put `AEECLSID_SIMCARDCTL` (`0x01006c01`) in the factory, to silence
`tectoymain.c:1668 ERROR: Unable to create instance of AEECLSID_LCT_SIMCARDCTL, cannot do SIM
check`. The long comment on `Interface::SimCardCtl` already said that **that log is the game taking
the right path**: with the class refused, the Z-Wheel sets the state to `0x27` and `0x82464` calls
`0x1f7b4`, which moves the interface forward; offered, slot 3 answers zero, the state becomes
`0x28`, and `0x82464` **does nothing with it**. It was the `0x28` — "the screen stays where it is,
silent".

The A/B, measured in the windowless `bench`, 43 s of virtual clock, same script on both:

```text
with the class offered    474,692 instructions · 2582 turns · 1 timer · 6 IMedia calls
                          5 IDENTICAL frames (same sha1) · 0 frames presented
with the class refused    750 million instructions · 6572 turns · 4 timers · 32 IMedia calls
                          5 DIFFERENT frames · 1 frame presented
```

And the scan now shows what the wheel draws — the **local library grid**, with our games in three
columns, from 37.6 s on, which is after the confirm on "Jogar":

```text
text drawn on screen:
    37605 ms  (427, 272)  Alien Breaker
    37622 ms  (42, 272)  Action Hero 3D
    37622 ms  (271, 272)  Alice
```

What is missing is the **launch request**: it still does not show up, and the new candidate is in
the report itself — `IWidget::Acessador selector 0x7001` joined the list of missing APIs, and
`0x7001` is the event the transition sends at the end (§7.3).

**The catalog bridge, and what it does not solve.** The wheel does not read the ROM folder: it reads
the profile's `tt_game_info`, and what links one to the other is the `catalog.json` the interface
writes (`library::sync_catalog`). The scan did not feed it — measured: with `ZEEBX_ROM_INSTALLED`,
the profile database opened with the package's 59 official rows and `ZEEBX_LIBRARY` with **zero**.
The scan now scans the ROM folder and syncs (63 rows now go in), which is necessary — but **not
sufficient**: the widget tree at the first key still has the same three, and the wheel still does
not build the stage (`0x01028e05`) nor the roller (`0x01028e14`), which are the two objects the menu
build-up goes through (§4 to §5.4). The `0x01028e19` that §2.2 mentions **is** created, once, right
after the form — and it is the only one of the trio.

Two instrument traps came out of this measurement, and both are fixed in the code:

- **A script step without the `k` was silently discarded.** Written in the bench's form
  (`30500:0xe064`), the whole script became "no key" without a line of warning, and the measurement
  said the wheel does not respond — when the one that pressed nothing was the script. Now the `k` is
  optional (the button comes first, because `up` and `down` are names of both) and whatever is
  discarded comes out as a warning.
- **The scan had no serial capture.** The widget tree at the first key, the classes created and the
  databases opened go to the serial capture, which is where the instrumentation writes without
  mixing with the game's log — and it only existed in `run` (`--serial=`). Without it, "who was this
  key for?" could only be answered with a window open, which is what the scan exists not to
  require. Now it is `ZEEBX_ROM_SERIAL=<file>`.
- **The scan had no probe and did not show it.** `ZEEBX_ROM_PROBE=0xCLSID,…` answers the class with
  an observation object, and the report got the section "what the game called on the probe
  classes", slot by slot, with the strings the arguments point to. That is how we saw that the wheel
  takes the object, asks slot 2 (`QueryInterface`), uses it and releases it.

### 8.2 What still rests on hypothesis

Honesty about the state: the wheel comes up, draws, composites on screen and turns — but a good
part of the path still rests on a recorded hypothesis, not on a confirmed reading. What **has
not been verified yet** after the §5.8 fix: whether the rotation goes through the fifteen items and
wraps around, whether the confirm (`0xe064`) enters the game, and whether the tab selection
follows the wheel.

- **Slots answered without knowing what they do**: `0x01028e3c` slots 3 and 5, the accessor's
  `0x711` selector, the `slot16(this)` at `0x22d58`.
- **The list step** is the font height. A wrong step gets the layout wrong.
- **The slot 17 font is kept and not used for drawing.** We draw with the `tectoy.ttf` the package
  itself ships, at a single size, because the **`fontsize.map`** the Z-Wheel looks for **does not
  exist anywhere in the dump** — not even in its package. It is what would say how many pixels each
  named BREW size is worth. (The font class the roller asks for is **`0x0102f67c`**, not
  `0x01035156`.)
- **The parent link of the menu form was never observed** — it is inferred from the "current
  screen" rule, not read.
- **Old trees are never destroyed**; the "newest only" filters and the deduplication work around it,
  but the live object count grows. It leaks fast if `Release` does not release the kept children: in
  attract mode, a thousand and twenty-four objects ended up in one turn of the loop.
- **Widget `QueryInterface` accepts everything.** Slot 13 depends on it, and it is the suspicion
  behind the recursion at `0x1177c`.
- **The animation player** (widget slot 8) is still unimplemented; the intro gets out through a
  shortcut.
- **Painting images/text is a declared substitute**, not emulation of the widgets extension.

Two lessons worth more than any slot: **"the game moved on" does not prove it moved on along the
right path** — one attempt of ours made the Z-Wheel build the menu and the log revealed
`Unable to create instance of AEECLSID_LCT_SIMCARDCTL` **486,101 times**. And **a hang is worse
than a stable intro**: undo the attempt.

---

## Appendix — where this is in Zeebx

For people working in this repository.

| What | Where |
|---|---|
| Widget slots (13, 16, 17, accessor, root events) | [`machine/widget.rs`](../../src/machine/widget.rs), `widget_call` |
| Slot names | [`brew/aee_slots.rs`](../../src/brew/aee_slots.rs), `WIDGET` |
| Widget state (`drawing`, `models`, `handler`, `serial`) | [`machine/mod.rs`](../../src/machine/mod.rs), `struct Widget` |
| Widget family, `0x0102f67c`, `0x01028e3c` | [`machine/mod.rs`](../../src/machine/mod.rs) |
| Tree, current form, bar `0x01028e3f` | `form_tree`, `current_form` |
| Intro start and z-pad shortcut | `start_animation`, `skip_wheel_instructions` |
| Pbuffer and `GetColorBufferQUALCOMM` | [`machine/egl.rs`](../../src/machine/egl.rs) |
| Profile databases and `tt_dlqueue.db` | [`machine/sql.rs`](../../src/machine/sql.rs), [`brew/sql.rs`](../../src/brew/sql.rs) |
| `preloaded.cfg` | [`machine/file.rs`](../../src/machine/file.rs) |
| Wheel keys | [`input/mod.rs`](../../src/input/mod.rs), `avk::LEFT`/`RIGHT` (the D-pad, in `ui/mod.rs`) |
| Display bitmap | [`machine/bitmap.rs`](../../src/machine/bitmap.rs), `device_bitmap` |
| Event before start | [`machine/signal.rs`](../../src/machine/signal.rs), `send_applet_event` |

Commits, in order: `3ca627f` (slot 13 accepted, child as selector, root events), `67a2305`
(removes `SIMCARDCTL` again), `8ad6906` (drawing stops filtering by tree), `794ed80` (container
`0x01028e3f` joins the tree), `c109a54` (`0x0102f67c` becomes a font), `d7a73d3` (slot 13 →
`CreateCompatibleBitmap`; root events → false), **`39de79a`** (**slot 17** and the
`tt_dlqueue.db` schema: the roller builds), `ee63799` (local library in the catalog), `9e012f6`
(`preloaded.cfg` and `0x7b0e`).

Wider context: [14-z-wheel-and-efs2.md](14-z-wheel-and-efs2.md) (what came before, and why the NAND
does not solve it), [15-whats-missing-from-the-nand.md](15-whats-missing-from-the-nand.md),
[13-unknown-classes.md](13-unknown-classes.md) (the probe, which is how several vtables came out).
