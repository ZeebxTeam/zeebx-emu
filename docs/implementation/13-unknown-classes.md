# 13 — Unknown classes: how to find out what they are

BREW identifies each class by a number. When a game asks for a number we do not know, the right
answer is `ECLASSNOTSUPPORT` — and the game usually gives up. The question left over is always the
same: **what interface is this?**

Without the header, the traditional path is to guess from the neighbouring name and test. That is
how the console's `IHID` was identified, and it was expensive. `--probe` turns this into a
procedure.

## What the probe does

`--probe=0xCLSID[,0xCLSID…]` makes the class be served with an observation object instead of being
refused. That object implements no interface at all: it **records** every call — the slot, the
arguments and the text of any argument that points to text — and answers `SUCCESS`.

Three decisions make the difference between a tool and a toy:

**Answer success to everything.** The probe does not try to get the behaviour right; it tries to
make the game **move**, because it is by moving that the game shows what it expects. An honest
`EFAILED` would stop the investigation at the first call.

**Hand out another probe in the output pointer.** A method that returns an object writes the
pointer into an argument, and answering success without writing anything makes the game carry on
with garbage and die on first use — which is exactly what the Z-Wheel did. With the probe handing
out probes, the observation reaches the **whole family**: the manager returns a database, the
database returns whatever comes.

It only writes where it is safe: an aligned address, inside the game's memory and **holding
zero**. An argument that already points to something is not an output destination, and writing
to it would ruin game data.

**Read the text of the arguments.** That is what turns `slot[3] (0x5373c, 0x10002f28)` into
`slot[3] ("tt_prefs.db", output)`. Without it, you would have to find out where the module was
mapped in order to fetch the string from the file — and the first attempt at doing that gave a
wrong base and a meaningless string.

## The case that motivated it

The Z-Wheel — which is not a game, it is the console's store application — asked for `0x0102c4e8`
and refused to budge. Its log already said the name: `No SQLMGR: 20`, twenty-one thousand times,
until the stack overflowed.

One run with the probe gave the whole interface:

```
class 0x0102c4e8, object 0x30000050:
  slot[ 3] ("tt_prefs.db", 0x10002f28, 0x0)
class 0x0102c4e8, object 0x30000090:
  slot[ 3] ("PRAGMA integrity_check", 0x94ce8, 0x10002f20)
  slot[ 1] (…)
```

It reads from top to bottom: the manager opens a database by name and returns an object; the
object receives an SQL statement, a pointer into the module's code range — a callback — and a
context. That is the shape of `sqlite3_exec`, and it was enough to implement
[`ISQLMgr`](https://github.com/ZeebxTeam/zeebx-core/blob/main/src/brew/sql.rs) without any header.

## The count is half the information

Each line of the log carries how many times that slot was called, and that number separates two
very different things: "the app called this" and "the app is **stuck** on this".

It was the count that closed the diagnosis of the Z-Wheel's graphical interface. With five classes
probed, the app gets past "Could not create root form" and creates fourteen objects — it looks like
progress. The count shows what it really is:

```
class 0x0100104f, object 0x30000510:
  slot[ 7] (…)   (14590410x)
  slot[ 4] (…)   (14590409x)
```

Two methods alternating fifteen million times: the app is in a loop waiting for an answer the
probe does not know how to give. Probing did not make it move, it made it spin — and without the
counter that would pass for progress.

## Arranging a slot's answer

`--probe-answer=0xCLSID:SLOT=VALUE` makes a slot answer whatever it is told, instead of success.
It exists because answering success to everything has a cost symmetric to its benefit: the game
moves further, but never reaches the end of a loop in which it expects to hear "done".

That is what separated hypothesis from fact in the Z-Wheel's interface. The fifteen-million-pass
loop alternates two methods of class `0x0100104f`; with slot 4 answering anything other than
zero, the loop **ends** and the app moves on — which proves that slot is the condition, and not
the other one. It is information that no amount of passive observation would give.

## The literal pool also identifies

The probe tells the **shape** of a method; what usually tells its **purpose** is the binary. An
ARM module keeps each function's constants in a pool next to its code, and the games' `dbgprintf`
leaves the file name and the error message in the same place. So a constant's neighbourhood is,
in practice, its name.

To use this you have to calibrate where the module was mapped. The cheap way: take a string
pointer that showed up in a trace — the Z-Wheel's `IFILEMGR_OpenFile` received `0x7afec` —, look
for its content in the file and subtract. In `tectoy.mod` that gave base `0xbc80`, and from there
any address becomes a position in the file.

That is how the three classes of the Z-Wheel's interface were identified without any header:

| Class | Where the constant appears | What it is |
|---|---|---|
| `0x01001011` | the error the app prints when it is refused | the root form |
| `0x0100104f` | 20 bytes before `Could not create root form`, and also in "app history instance" and `Tectoy_LaunchMainMenu` | a generic collection — used in three places that have nothing in common other than holding items |
| `0x01035156` | between `Unable to create instance of TrueType TYPEFACE` and `TrueType Dictionary` | the TrueType font |

## Where the probe ends

It takes the game up to the point where it **uses** the object for real, and stops there. In the
Z-Wheel that point is a `blx r0` instruction with `r0` equal to zero: the app read a **field** of
the object it received and called what was there. A probe is just a vtable — it has no fields, and
there is no arranged answer that solves that.

It is the sign that discovery is over and implementation begins. Knowing this avoids the mistake
of continuing to probe when what is missing is no longer information.

## The reading mistake that cost the most

Worth recording because it was not an emulation mistake, it was one of **method**.

I calibrated the module's mapping base by looking for a string in the file and subtracting. I
found the wrong occurrence, and disassembled everything shifted by 0x3380 bytes — which gives
plausible code: a sequence of valid instructions, with branches and calls, telling an entirely
false story. I chased that story across several runs.

What gave it away was the disagreement between the disassembly and the emulator's `--code`: the
trace showed `lr` becoming `0x78af0` at an instruction where the disassembly had no call at all.
**When static reading and execution disagree, the one that is wrong is the reading.**

The base needed no calibration at all: the loader maps the whole file at
`MODULE_BASE - MODULE_PREFIX` and fills the prefix with zeros, so byte zero of the file is address
`0x10000`. It was in `loader/mod.rs` the whole time.

## And when the probe is no longer the best path

The classes the Zeebo applications ask for are implemented **inside the console's firmware** —
`1.1.2_APPS.bin`, a 21 MB ARM ELF. They are not loose modules in the file system: looked for
there, they do not exist. `0x01001011` appears 26 times in the firmware.

That changes the instrument. The probe discovers one slot per run, by hypothesis and test; in the
firmware the method table **is written down**. `tools/firmware.py` reads the ELF segments, looks
for a constant and disassembles in ARM or Thumb.

Two traps, both already paid for:

**The firmware mixes ARM and Thumb**, and a good part of the application code is Thumb.
Disassembling in the wrong mode gives a meaningless sequence of `strb`/`movs` — recognizable.

**Not every pointer in a table is code.** A table of `(ClassID, pointer)` pairs looked like a
class registry; the pointers were log strings from the video recorder, and disassembling them
gave plausible, false code. The same kind of mistake the wrong mapping base produced before:
disassembly almost never refuses to produce instructions, so "code came out" confirms nothing.

What **is** confirmation: code that makes sense as an interface. This one came out of the
firmware and is a textbook `QueryInterface` — it accepts the base `AEEIID` and two more, and
refuses the rest with error 3:

```
push  {lr}
ldr   r3, =0x01000001      ; the base IID
cmp   r1, r3
beq   accept
ldr   r3, =0x01005002
cmp   r1, r3
beq   accept
adds  r3, r3, #2           ; 0x01005004
cmp   r1, r3
bne   refuse
accept:  str r0, [r2]; AddRef; return 0
refuse:  [r2] = 0; return 3
```


## The class the game uses without checking

BREW answers `ECLASSNOTSUPPORT` for a class that does not exist, and writes null to the output
pointer. That is correct, and almost every game handles it. **Powerboat Challenge** does not: it
keeps the pointer to `0x01001039` and calls one of its methods without looking at the return, and
the jump to address zero happened even before the language menu.

For these, the machine has a short list — `CLASSES_UNDER_OBSERVATION` — served with the same
object as the probe: it answers success to everything and records what was called. The
assumption shows up in the report, and what the game called comes out in the probe log, which is
how the class will be identified once the console's firmware has been read.

Powerboat now shows the language menu and the loading screen; it does not get past it yet, and
that is another problem.

## What it does not do

The probe does not tell **the name** of the method, only the slot number and the shape. What gives
the name is the usage: a `"tt_prefs.db"` string in a slot that returns an object is
`OpenDatabase`, and that is how the name enters the slot table — with a comment saying the source
was observation, not documentation.

And it lies by construction: it answers success to methods that on the console could fail. It is
for discovering, not for running. No normal run should depend on it.
