# 03 — API dispatch

## The trampoline

A BREW object is a pointer to a struct whose first word points to the vtable. Calling
`ISHELL_CreateInstance(shell, …)` becomes, in ARM, a load of the vtable pointer, a load of the slot
and a `blx` to the address read.

We take advantage of that: the addresses we put in the slots sit in an **unmapped** range, chosen
so that the address itself encodes which method was called.

```
address = 0xf000_0000 + (interface << 12) + (slot * 4)
```

The core aborts the fetch, `aee::decode` undoes the math, and the dispatch knows exactly what to
call. **There is not a single line of ARM glue code anywhere** — the fetch fault *is* the call
mechanism.

Decoding by hand, looking at a log:

```
0xf000_7038 - 0xf000_0000 = 0x7038
0x7038 >> 12              = 7      -> Interface::Bitmap
(0x7038 & 0xfff) / 4      = 14     -> slot 14 = SetTransparencyColor
```

`aee::describe` does this and returns `"IBitmap::SetTransparencyColor"`, which is what shows up in
the trace and in error messages.

## The slots

`brew/aee_slots.rs` lists the name of each method **in the order it occupies in the vtable**. The
order comes from the `INHERIT_IXxx` macros in the SDK headers — it is the only source that
guarantees the right index, and an offset of one slot makes the game call the wrong method with no
warning at all.

`brew/aee_helpers.rs` does the same for the stdlib: `struct AEEHelperFuncs`, field by field. The
index is the offset in words — `malloc` is slot 26, offset `0x68`, which is the value real modules
ask for.

## `Machine::dispatch`

```rust
dispatch(addr)
  ├─ aee::decode(addr)                    -> (interface, slot)
  ├─ count the call                       -> the summary at the end
  ├─ note_spin(iface, slot)               -> see 04-time.md
  ├─ trace, if enabled
  ├─ dispatch_inner(iface, slot)          -> the real work
  └─ bad pointer becomes EBADPARM, not a panic
```

A bad pointer coming from the game **must not bring the emulator down**: it becomes `EBADPARM`,
which is what BREW answers, and it is recorded to show up in the report. A game with a bug is no
reason for the emulator to crash.

`dispatch_inner` splits by interface. Some families have their own function (`file_call`,
`gles_call`, `media_call`, `bitmap_call`…), and the `IDisplay`/`IGraphics`/`IBitmap` arm has a
costly peculiarity described in [05-video-2d.md](05-video-2d.md).

Returning `None` means **"not implemented yet"** — the loop stops with `Outcome::Unimplemented`,
which names the method, the arguments and where the call came from. That message is what turns
"the game doesn't open" into "`IFileMgr::EnumInit` is missing".

## How a new method gets in

1. Run the game with `--seconds=6` and read the line `stopped on pass N at unimplemented API`.
2. Find the signature — in this order: the SDK header in `docs/vendor/.../sdk/inc/`, then the HTML
   reference in `documentation/API Reference/`.
3. Implement it in the family's `*_call`, returning what BREW would return (including the error).
4. Test. If the method has geometry or a format, the test is about that and not about the call.
5. Run the game again and check that it got past that point.

Step 2 is not optional. **Guessing a signature gives you an emulator that works by accident** —
and the accident ends three games later.

## When BREW has no good answer

Not every method needs to do what it promises. It is fine to answer what is true:

- `ISHELL_Prompt` returns **false**: "I didn't create the dialog". BREW provides for this, and the
  game goes down the path of someone who couldn't ask.
- `IBitmap::QueryInterface` for a class we don't have returns `ECLASSNOTSUPPORT`, which is the
  correct answer — BREW expects the app to have an alternative path.
- What changes nothing (`Backlight`, `SetAnnunciators`) returns `SUCCESS` and ignores it. Refusing
  would make games give up over a setting that changes neither the sound nor the picture.

What is **not** fine is making up data. An unknown class goes into `unknown_classes` and shows up
in the report; an assumption goes into `assumptions` and shows up too.

## The class that isn't ours: extension modules

There is a case where the right answer is neither implementing the class nor refusing it: when it
comes **inside the game's own package**, written in ARM. It is BREW's extension module mechanism,
and two titles we have depend on it:

| title | applet | extension | class |
|---|---|---|---|
| Action Hero 3D | `mod/274259/a3d.mod` | `mod/12875/imicro3d.mod` | `0x010292c3` |
| Kingdom Hearts | `Kingdon Hearts/kh.mod` | `Kingdon Hearts_/swv21brew.mod` | `0x0102bbfc` |

Both showed up in the survey as "doesn't run", and the cause was the same: they asked for a class
the console doesn't have in any partition — because it isn't the console's, it's the game's.

### How the extension is recognized

The `.mif` stores an 8-byte record per class, `<u32 ClassID> <u32 zero>`, and it appears
**identical on both sides of the relationship**: in the game's manifest, declaring the dependency,
and in the extension's, declaring what it provides. What tells them apart is the surrounding file:
a `.mif` **without** an applet record is not a title's, it is an extension's. Without this rule the
game's manifest would offer itself to serve the very class it is asking for.

The `.mod` ↔ `.mif` pairing is by the module's **folder name**, which is the BREW layout —
`<name>/` with the module inside and `<name>.mif` next to it. It is narrower than the search that
finds the applet's manifest, and on purpose: there, getting it wrong gives "manifest not found";
here, it would give the class served by the wrong module.

### What happens at request time

Extensions are **mapped at load time**, each at `0x0800_0000 + i * 0x0100_0000`, with the two
helper words before the base — the same ones `AEEStdLib.h` reads. Mapping beforehand, and not at
request time, is because the core has no API to map a region after `reset`; the modules are small
(90 KB and 164 KB) and the cost is irrelevant.

What is left for request time are the two steps the console takes:

1. `AEEMod_Load(shell, helpers, &out)` the first time, and the extension hands over its
   `IModule*`.
2. `IModule::CreateInstance(module, shell, clsid, &out)` — slot 2, the same as the applet's.

The object that comes back is implemented in ARM by the extension. From then on the game talks
to it directly and **we don't need to know which interface it is** — neither `IMICRO3D` nor the
Superscape engine was implemented here; they simply run.

### The exception to the rule, and the rule that comes with it

This happens **inside** the dispatch of an API call, which is precisely the place where entering
the guest was forbidden — the normal `call_guest` writes to `r0..r3` and to `lr`, and whoever is
dispatching will read `lr` afterwards to know where to resume the game. Losing it sends execution
to the wrong place, usually to address zero.

`IIMAGE_Draw` on a game surface solved this by deferring the call to the boundary queue, and that
doesn't work here: the game expects the object pointer to be written before the return. So the
call is truly nested, with **the whole context saved and restored** — `r0..r12`, `sp` and `lr`.
The stack needs no care, because the nested call pushes below the current `sp`, which is space
nobody is using: the same guarantee an interrupt has.

### Where the two got to

Neither of them was just one class away — and it is worth recording, because it is the difference
between "unblocked" and "runs".

**Action Hero 3D** created the `IMICRO3D` object and the next obstacle showed up right away:
`0x01004001`, requested right after an `IMemAStream`. It is `AEECLSID_BMP`, and the identification
is not a guess — its resources are 67 BMPs inside a `.res` whose first entry is called `66.bmp`.
The `IImage` image decoder only handled PNG; today it looks at the magic bytes and handles PNG,
BMP and JPEG through the same path, which is the right thing anyway: the console has one class per
format, all fed by `IAStream`, and the format is written in the file. With that it creates the
applet, receives `EVT_APP_START` and runs the loop with a clean report — no unknown class, no
missing file, no assumption. It still doesn't put anything on the screen.

**Kingdom Hearts** needed one more thing, and it didn't belong to any class: the processor in
**user mode**, see [02-cpu-and-memory.md](02-cpu-and-memory.md). With both, it runs **405
passes of the loop and 418 million instructions** in six virtual seconds, with the screen drawn.
What's left is `0x0100a004`, which isn't in any `.mif` in the package nor in the firmware's class
table, and which it tolerates not having.

## The C functions of `AEEHelpers` follow C, corners included

The string and number helpers are rewritten in Rust, and the easy way to get them wrong is to
follow Rust instead of C. `strtoul` is the example that cost a whole game.

**Alice no País das Maravilhas** stopped with "invalid access at 0x0000000c". The game's log gave
the hint: `TTDMM ASSERT ... Error 7` and `Error Malloc - not enough free memory`. The game reserves
18 MB at once and manages the pool by itself; error 7 is the end of the free list without a block
large enough, that is, a real lack of memory, not corruption.

What was eating the pool was the game's grid reader. It reads lines of eight numbers with `strtoul`
and stores each line in a vector that doubles in size. One line had `-2 12 16 73 ...`, and our
`strtoul` didn't accept a sign: it returned zero without consuming anything. The game's loop treats
"nothing consumed" as an empty field, counts the field and carries on **at the same position** —
and since it only exits when the pointer reaches exactly the end of the text, it repeated the line
forever. A 149 KB file became a 4 MB vector, and the next copy of the vector didn't fit.

C's `strtoul` accepts `+` and `-` and, with `-`, returns the negated number. Now ours does too, and
the leading whitespace is `isspace`'s (`' '`, `\t`, `\n`, `\v`, `\f`, `\r`), not Unicode's, which
read as Latin-1 would count `0xA0`. The `0` of the octal prefix in base 0 is now consumed too.
With that Alice gets past the language, the title and the menu and reaches the level.

### The string ceiling, and the cut-off map

Reaching the level wasn't playing all of it. In level 2, the bottom part of the scenery came out
black, and Alice, carried through the crack into a room, fell forever into the void. The same was
true for the CPU rasterizer and for the graphics card: the game draws in software, and what was
missing was the map.

The levels are Lua code generated by an editor (`AliceEditor's Lua Exporter`), inside
`resources.pakz` — a `PACK` package of LZMA streams. Each layer's tile map is a single-line base64
string, with 8-byte entries (x, y, tile, flags) in row order: 15,426 characters in level 2, and
202,806 in `stage1-30`. The emulator's string read ceiling was 4,096 bytes, and `strlen` returned
4,096 for all of them. The game decoded only the beginning, and every tile after row 29 of level 2
vanished — the room's floor is on rows 56 to 61.

The ceiling exists against a pointer without a terminator, and the read goes in chunks up to the
zero: raising it to 1 MiB doesn't change the cost of a short string. Every level of the game has
lines above 4,096.
