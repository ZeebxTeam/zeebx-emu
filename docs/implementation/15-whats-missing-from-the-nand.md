# 15 — What is still missing from the NAND

This is the list that was asked for: **what the console's firmware has and the emulator does not
yet**. It became quick to answer after `tools/firmware.py` gained the `class` command; before,
each class cost a dozen manual steps.

## How the list is put together

`1.1.2_APPS.bin` has a class table with sixteen-byte entries:

```
<u32 constructor> <u32 CLSID> <u32 flags> <u32 zero>
```

The command finds the entry, disassembles the constructor looking for the literal it writes into
`[obj]` — the vtable — and lists the methods up to the first one that is not a Thumb address:

```
$ python3 tools/firmware.py class 0x01001011
  constructor 0x112e399c  (flags 0x8)
  vtable      0x10a785e4  (7 methods)
```

Three filters separate the real entry from the other mentions of the same CLSID — `0x01001011`
shows up twenty-six times in the file: the constructor has the Thumb bit 0 set, falls in a
loadable segment, and starts with `push`. The first two alone still let ASCII text read as a
number through.

**A remaining doubt, said up front**: entries with flags `0xffff0000` are suspect. The confirmed
ones carry small values (`0x4`, `0x8`, `0x9`, `0x20000`), and the two with `0xffff0000` may be
chance matches. Where that shows up below, it is marked.

## What is already read and implemented

| Class | What it is | State |
|---|---|---|
| `0x01001011` | dispatcher with a queue (it is not a form) | **implemented** — 7 methods, and slot 5 is the send that made the network work |
| `0x0100104f` | generic collection | implemented |
| `0x0102c4e8` | `SQLMGR`, on top of real SQLite | implemented |

## What the firmware delivers and is still to be implemented

| Class | Methods | What it unlocks |
|---|---:|---|
| `0x01006c05` | 8 | requested by the Z-Wheel; constructor `0x11267d04`, vtable `0x102d47a8` |
| `0x0103475a` | 39 | the **inner** object of `0x01001011` — slots 4, 5 and 6 delegate to it. Flags `0xffff0000`: check before trusting |
| `0x01003109` | 40 | Zenonia's text subsystem (`CWBLText::Create() failed!`). Constructor `0x108ee2ec`, vtable `0x10e15c04` |
| `0x0100102e` | 59 | the network class Opera Mini asks for. Constructor `0x10b9b778`, vtable `0x10a783e4` |

### The constructor that does not write the vtable

The last two rows had a `?` under "methods" for a long time, and not because the information was
missing: **their constructor does not write the vtable.** It allocates the object and hands the
job to an initialization function, reached by a long jump — `ldr pc, [pc, #-4]`, the trampoline
the linker puts between distant segments. The whole constructor disassembles without a single
`str` into `[obj]`, and following the call by hand led to the trampoline, whose disassembly is
garbage.

`0x0100102e` shows the whole pattern:

```
0x10b9b786  ldr  r1, [pc, #0x44]   ; = 0x01039270   <- first creates the class it depends on
0x10b9b790  blx  r3                                 <- ISHELL_CreateInstance
0x10b9b798  movs r0, #0x30
0x10b9b79a  bl   #0x10d6f944                        <- malloc(0x30)
0x10b9b7b0  blx  #0x10b98744                        <- init, and this is where the vtable goes in
0x10b9b7b4  str  r5, [r4]                           <- *out = obj
```

`0x10b98744` is the trampoline; behind it, at `0x105c7dd4`, is the real init:

```
0x105c7ddc  ldr  r0, [pc, #0x20]   ; = 0x10a783e4   <- the vtable
0x105c7dde  str  r0, [r4]
```

`firmware.py` does this on its own now: when the start of the constructor does not yield the
literal, it follows the calls — crossing the trampoline — **one level**. One level covers these
classes, and each extra level raises the chance of finding the vtable *of something else*: the
constructor also calls `malloc` and the `CreateInstance` of the object it depends on.

It is worth recording where the other wrong number came from: the method count stopped at a
**ceiling of 16** that nobody saw, and the ceiling went into the table as if it were the size of
the interface. `0x0103475a` has 39 methods, not 16. Today the ceiling is 64, adjustable
(`class <clsid> <ceiling>`), and the tool warns when the count hits it.

The reading gives one more thing for free: `0x0100102e` **creates `0x01039270` before anything
else** and returns its error if it fails. That is a clue about what the class is, and not an
obstacle of ours — the firmware is not executed here, we implement the class on the host.
`0x01039270`, in turn, is not in this partition's table, which puts it on the same list as the
other missing ones, below.

And it is worth being precise about Opera Mini's error: the `EFAILED` it gets in the applet's
`CreateInstance` comes from **us** not implementing `0x0100102e` — its `ISHELL_CreateInstance`
refuses, and the applet's constructor gives up. It is not the firmware's constructor failing; that
one we only read.

## What the firmware does **not** have

| Class | Who asks for it |
|---|---|
| `0x01028e35` | Z-Wheel — and it is its next obstacle, called in slot 12 |
| `0x01028e51` | Z-Wheel and Zeebo App |
| `0x01035156` | the Z-Wheel's TrueType font |
| `0x01001031` | shows up in `IWeb`'s `QueryInterface` |
| `0x01039270` | the class `0x0100102e` depends on — Opera Mini, as a consequence |

None is in the APPS partition's table, and the likely explanation is the usual one: we only
extracted that partition. The rest of the file system is behind the EFS2, whose reader stopped at
the current generation of the page table (see [14](14-z-wheel-and-efs2.md)).

## What is not a class problem

Worth separating, because it changes the order of the work:

- **The EFS2 reader** is what unlocks the four classes above at once, and also the extension
  modules (`widgets.mod`, `forms.mod`, `isql.mod`) the console loaded. What is missing is
  reproducing the change journal of the log-structured system.
- **`0x01001011` was not a form**, even though the Z-Wheel's error message says "root form". The
  vtable showed a dispatcher with a queue. This is worth keeping as a warning: the name an
  application gives a class describes its use, not the class.

## Suggested order

1. **`0x0103475a`**, the inner object — it is what `0x01001011` needs to stop being a stub, and it
   is the class both targets use.
2. **`0x01006c05`**, which is already read and is small (4 methods).
3. **The EFS2 reader**, which unlocks the four missing ones at once instead of one by one.
