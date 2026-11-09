# 25 — Save states on desktop

The Qt interface's game window saves and loads the console state in ten slots per game, through
keyboard shortcuts, like PCSX2. Up to 0.4.3 only Android (five slots, since 0.4.0) and the
Libretro core (through RetroArch's slots, since 0.3.0) did this. In Qt the state was only used by
the rewind.

The state format does not change: it is the ZBXS of [`src/save_state.rs`](../../src/save_state.rs),
with a version, named sections and `crc32`, the same as Android's, the core's and the rewind's.
What this work adds is what surrounds it: where the file lives, who writes it, and the keys.

## 1. The slots

```text
<config>/states/<ContentId>/slot-N.zbxstate   the state
<config>/states/<ContentId>/slot-N.rgb565     the thumbnail, optional
```

The module is [`src/ui/save_slots.rs`](../../src/ui/save_slots.rs). **It was born on Android**,
as `frontends/android/src/state.rs`, and had nothing Android-specific in it: naming the slots,
writing atomically, reading the thumbnail. It moved into the core library, and Android now uses
it from there. Both write the same layout, and a state copied from the phone to the computer
shows up in the same slot.

- **The folder is the [`ContentId`](../../src/storage.rs)**, the BLAKE3 of the file the frontend
  opened (the `.zip`, the `.7z` or the `.mod`), before extraction. Two versions of the same title
  do not share state: a state is the memory of one binary, and in the other's binary it makes no
  sense. The session already computed that hash to find the saves folder; now it keeps it
  (`Session::content`), and Qt does not read the whole package a second time for every game
  opened, nor for every one the Z-Wheel launches.
- **Ten slots**, as in PCSX2. Android had five; it shows the slots in a scrolling list and so its
  layout did not change. The `slot-1` to `slot-5` from before stay where they were.
- **The write is atomic**: the temporary file with `fsync`, the old state to `.bak`, the new one
  in place, and the `.bak` deleted. A crash midway leaves only the `.bak`, which the next read puts
  back in place. Overwriting a slot never destroys the last valid state.
- **The thumbnail is a separate file**, 160×120 at most, and it is decoration: broken or missing,
  the slot shows up without an image and loads the same. Saving the state and failing the
  thumbnail is a success.
- The state read is capped at 128 MiB, so a strange file cannot ask for an absurd allocation. The
  measured ones are tens of MB.

On desktop, `<config>` is `config_dir()` (`~/.config/zeebx` on Linux), the same root as the saves
and the cache.

## 2. Saving and loading

`save_slots::save` asks `Session::can_save` (it refuses in the middle of a `glBegin` without a
`glEnd`), notifies the GPU state mirror (`resume_context`: whoever shares the context with the
engine may have painted on it) and **brings the GPU frame to the screen before saving**
(`materialize_gl_frame`). It is the rewind's lesson, in
[24](24-speed.md#the-black-screen-and-the-step-forward): the last frame stays in the texture, the
texture is not part of the state, and without this the loaded state came back with the screen of
an old frame, or black in a game that only draws through GL. Android's save state did not do
this, and now it does.

**In Qt, both ends run with the game's context current** (`in_context` in `emu_core.rs`). The
thumbnail comes from the texture, and the texture can only be read there. Loading rebuilds the
textures on the GPU, and outside it they would end up in Qt Quick's context. That was exactly the
defect that kept the rewind from working in Qt.

What the window does after the console goes back to another instant lives in `RunningGame`
(`after_restore`), and also applies to undo:

- the rewind points are forgotten, because they belong to another timeline;
- the input already delivered to the game is forgotten, and whatever is held now arrives again as
  a press on the first restored frame;
- the real time that passed during the load is not for the game to make up;
- **pause stays as it was.** Loading with the game paused shows the loaded instant, paused.

In the bridge, the key of the image currently in the item is forgotten: the restored screen may
have the same series and the same writes as the one before, and the next pass would leave the old
image on display.

What the game wrote to disk does not come back, as in the rewind: loading a state from before a
save does not undo the save.

## 3. Undo load

An accidental load throws away the progress since the last save, and it is the accident that
costs dearly. Before every load, `RunningGame` saves the current state **in memory**
(`before_load`). The undo shortcut gives it back, once only: undoing again does not redo. There is
no new file on disk, and the copy goes away with the play session.

If `can_save` refuses at the moment of the load, the load goes ahead without undo: the player
asked for it. An undo the engine rejected (only through a defect of its own, because the state is
from the same session) keeps the copy stored for a second attempt.

Overwriting a slot has no undo. The on-screen notice names the slot, and PCSX2 does the same. A
copy of the previous file would double the disk space of the used slots, and was left out.

## 4. The shortcuts

| | Windows and Linux | macOS |
|---|---|---|
| Save to the current slot | F1 | ⌘S |
| Load from the current slot | F3 | ⌘L |
| Next slot | F2 | ⌘] |
| Previous slot | Shift+F2 | ⌘[ |
| Undo load | F4 | ⌘Z |

Outside the Mac it is PCSX2's. **On the Mac the F keys are brightness, volume and Mission
Control**, and only reach the program with Fn; F11 shows the desktop. There the default is the
system's convention: ⌘S and ⌘L are save and open in every program, and OpenEmu does the same. No
game uses ⌘, so nothing collides with the buttons. The earlier shortcuts (F8 to F11) did not
change, and have the same problem on the Mac.

All five are keyboard only, like the screenshot and restart: a lone controller button that saves
or loads a state is easy to press by accident, and the Zeebo controller already uses almost all of
them. The right way is a combination with a held hotkey button, like RetroArch's *Hotkey Enable*,
and it would serve all the shortcuts.

### The combination

The earlier shortcuts store a single key, by the `egui::Key` name. The save state ones store a
[`KeyCombo`](../../src/ui/settings.rs): the key with the modifiers in front, as text
(`Shift+F2`, `Cmd+S`). A lone key is written as it always was, and the egui names have no `+`
(the plus key's is `Plus`).

**The combination only matches with the same modifiers.** F2 moves forward and Shift+F2 moves
back; accepting extra modifiers would make Shift+F2 move forward too. That is why the game window
checks the save state shortcuts **before** the others: the screenshot looks only at the key, and a
save state Shift+F9 must not turn into the screenshot's F9.

**Qt swaps the names on the Mac**: what it calls `ControlModifier` is ⌘, and `MetaModifier` is
Control. The bridge's `qt_modifiers` undoes the swap. Outside the Mac, Meta is the system key
(the Windows key, Super), stored in the same field as ⌘. The screen shows the Mac symbols there
(`⌘]`) and `Shift+F2` elsewhere.

The **Shortcuts** tab rejects what it already rejected, taking the modifiers into account: Esc, P
and F11 on their own and Alt+Enter belong to the game window; the same combination cannot serve
two shortcuts; and the lone key of a Zeebo button is rejected, as with the screenshot. With a
modifier it does not collide with any button, because the game does not read modifiers.

### Whoever already had F1 on a button

F1 to F4 are not on any button of the default keyboard mapping, but someone who remapped may have
put them on one, and the shortcut, checked before the game, would steal the button. It is the
case of the restart R, and the rule is the same (`Settings::adopt_new_shortcuts`): a
`settings.json` that did not know the shortcut yet, with its default key on a button (in the
general settings or in some game's), gets the shortcut empty. A combination with a modifier
always arrives.

## 5. How it was checked

```bash
XDG_CONFIG_HOME=$(mktemp -d) ZEEBX_TEST_ROM=path/to/game.mod \
  cargo test --release -p zeebx --lib a_save_state_round_trips_in_a_running_game -- --nocapture
```

The two tests (`ui::running_game`) open the game, run three seconds, save to slot 5, run two
more, load, undo and run again. At each step they check the game's clock and the screen's
signature, one on the software rasterizer and the other on the GPU, in an offscreen context.
`XDG_CONFIG_HOME` matters: the session saves with the `config_dir` profile, and without it the
test would touch the real saves.

In Double Dragon, the screen at the saved instant and the one from before the load are different
(3083 ms and 5249 ms on the software rasterizer; 3216 ms and 5216 ms on the GPU), and each one
came back at its step. In Magical Drop III both screens are the same at those instants, and there
only the clock proves anything.

The `ui::save_slots` tests cover the disk without any game: the slot that round-trips, the
out-of-range slot, the state with a flipped byte (rejected by the `crc32`), the broken thumbnail
and the `.bak` of an interrupted write.

**Not checked**: the keys in a real window and ⌘ on a Mac. ⌘] and ⌘[ depend on where the keyboard
puts the brackets, and on a non-US keyboard (the Mac's ABNT2, for example) Qt may deliver another
key with ⌘ held.

## What is missing

- The panel with each slot's thumbnail and date, in the System menu of the interface redesign.
- The saves manager (`SavesWindow.qml`) does not list the states, and they cannot be deleted from
  it.
- The held hotkey button, so the shortcuts accept a controller button.
