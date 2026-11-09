# 24 — Speed: frame limit, frameskip, fast-forward, turbo and rewind

The five features of issue #32 touch the same thing: the game's pace against the wall clock. The
game's clock is virtual (see [04-time.md](04-time.md)) and moves with the work the game does.
What ties it to the wall clock is the session, comparing the two on every pass. That is why the
logic of all five lives in the core library, in `src/speed/` and in `Session::advance`, and the
frontends only wire up keys and menus.

| Feature | Where it applies | Default |
|---|---|---|
| Frame limit (60 / 30 / off) | all frontends; in the core, the `zeebx_limite_fps` option | 60 |
| Frameskip (off / automatic / fixed 1–6) | all; in the core, `zeebx_frameskip` | off |
| Fast-forward (2x–10x, or unlimited) | Qt, egui, Android | 3x, F10 held |
| Turbo (classic, toggle, hold) | Qt, egui, Android (player 1) | off |
| Rewind (return points) | Qt, egui, Android | off, F8 |

The Libretro core does not reimplement fast-forward, turbo or rewind: RetroArch has all three.
See [What the core does not do](#what-the-core-does-not-do). iOS is left out of this work, and
that is why `Session::step(budget, speed_limit: bool)` still exists: it is `advance` with the
usual pace.

## The pace of a pass

The frontend hands the session a `Pace`: the ratio (how many times the console's speed; `None`
is unthrottled), the frameskip, the half-rate presentation of 30 FPS and whether the sound is
muted. The old `speed_limit: bool` is `Pace::CONSOLE` or `Pace::UNTHROTTLED`.

**Changing the ratio re-anchors the clock.** At 3x the virtual clock runs ahead of the real one.
Measured back at 1x from the same anchor, it showed up ahead by everything it had gained, and the
throttle held it at `Step::Ahead` until the world caught up. That was already what happened when
turning the limit back on after a while without it. The new anchor starts at the moment of the
change.

**Unthrottled, the pass runs until the slice is used up.** Before, with the limit off, `step`
returned at the first presented frame. The window's "off" was stuck at one game frame per
monitor refresh, and the F10 of PR #83 had the same ceiling. Now it runs frames until it spends
`UNTHROTTLED_SLICE` (12 ms), which leaves 4 ms for the window to draw in a 60 Hz refresh. The
number is the refresh arithmetic, not a measurement. Without that ceiling, a pass that spent the
whole slice would overrun the refresh, the next one would get twice as much, and within a few
passes the window would be at 10 frames per second.

**The frames-per-pass ceiling grows with the ratio.** At 1x it is the usual `FRAMES_PER_PASS`
(4). At 10x, a 16 ms pass needs ten frames just to keep up.

## The frameskip

Skipping **does not change the game's speed**: the logic runs in full, and only that frame's 3D
drawing and clear are left out (`Machine::set_skip_draw`). A game that reads the screen back
(`glReadPixels`) never skips, the same rule the core already had.

The decision is **per frame, at its start**. A frame can span two window passes, and deciding
again in the middle of it would draw half the scene. That is why the session stores the buffer
swap number at which it decided, and only the next swap opens the next decision.

**Automatic**, outside the core, skips the frame that does not need to reach the screen:

- **One frame drawn per pass is enough.** After it, skipping the following ones leaves on screen
  what it drew, a few frames older.
- Before it, a frame is only skipped if, skipped, it still leaves time in the slice for a drawn
  one. With the throttle on, it also has to still be behind, and the pass must not have hit the
  ceiling.
- The cost of a skipped frame and that of a drawn one are measured **separately**.

The three rules came out of measurements that knocked down the earlier versions:

| Rule | Rolima at 10x | Defect |
|---|---|---|
| Skip everything that does not hit the pass ceiling | 8.27x | the pass's last frame came out skipped almost always, and the screen froze |
| Skip only if a drawn one still fits, with a single measure (the last frame's) | unlimited: 3.95x | a heavy game never saw any slack, and drew everything |
| Skip only if a drawn one still fits, with both measures | 4.11x | too conservative |
| Both measures, **and** skip everything after the first drawn one | 5.76x (average across scenes) | — |

In the core, automatic is still RetroArch's audio buffer notification, as `libretro.h`
prescribes. The counter for fixed skip and for the half-rate presentation is the same in both
(`SkipCounter`).

**During fast-forward, automatic is active even with frameskip off.** Drawing 600 frames per
second to show 60 is what would keep the host from reaching the ratio. A chosen fixed frameskip
still applies.

### The game that falls behind

A report from 0.4.2: in Qt, on a machine running EndeavourOS, NFS froze (image and sound, with
the window responsive) with any frameskip other than automatic, and recovered on automatic. It
did not reproduce: here NFS runs at 1x in the race with every choice, switched midway. The
suspicion is that the game runs below real time on that machine, because automatic is the only
choice that skips a frame when the game falls behind.

So that the next report brings the data, the session watches the speed (`SpeedWatchdog`). When,
in one real second, the game advances less than half of what the pace asks for, the log says how
much it advanced, the frameskip, the half-rate presentation, the cost of the last drawn frame and
of the last skipped one, and how many passes there were and how many were ahead. One line per
episode, and another when it gets back to 90%. A pause or the rewind do not count: the
measurement restarts after a gap between passes. Truly slow shows up as a low speed with
expensive frames; frozen shows up as zero.

## The fast-forward

The speed is a fixed ratio from 2x to 10x, or "unlimited". The shortcut is F10 (the one from PR
#83, for those who already got used to it), changeable in the Shortcuts tab, by key or by a
button on any connected controller. The mode can be hold or toggle. While paused, fast-forward
does not apply.

Measured on 2026-10-10, software rasterizer, window simulated at 60 Hz, average of 4 interleaved
runs of 1.5 s (interleaved because the game changes scene over time, and measuring one whole mode
after the other compared different scenes):

| Game | 3x | 10x | Unlimited | Unlimited, no skip |
|---|---|---|---|---|
| Crash Nitro Kart | 2.97x | 9.60x | 23.97x | 8.11x |
| Zeebo Extreme Rolima | 2.95x | 5.76x | 8.48x | 3.97x |
| Double Dragon | 2.98x | 9.92x | 100.55x | 10.38x |

On release, all three went back to 0.99x–1.00x. The GPU path and Android were not measured.

### The sound

**The sound is pulled by the sound card's clock, not the game's.** The card asks for samples at
the world's pace (`cpal`). At 3x, the game played the music at 1x and delivered the effects three
times closer together. The end of each sound, which the game measures by the virtual clock,
arrived before the mixer had played it in full. And the PCM streams discarded the excess at the
half-second ceiling, with pops.

The mixer now multiplies the step of each voice and each stream by the speed
(`Mixer::set_fast_forward`). The sound follows the game, higher-pitched, like a fast-forwarded
tape. The speed used is the **reached** one, in a half-second moving average, and not the
requested one. With 10x requested and the host reaching 4x, a step of 10 would leave the sound
two and a half times ahead of the game. At 1x the pace is exactly 1, because following the
measurement's wobble would put the music out of tune for someone who is not even fast-forwarding.

**The average starts from the requested ratio, not from 1x.** The game speeds up on the press: in
Action Hero 3D at 3x, the first 100 ms already run at 2.8x. Starting from 1x, the average took a
second and a half to get close to 3x, and the sound and the ▶▶ label climbed slowly behind the
game. Fast-forward felt gradual. From the second press on, with the same ratio, it starts from
where it stopped last time. In the same game at 10x the host reaches 6.7x, and starting from 10
on every press made the pitch slide for a second and a half; measured again, the second press
starts at 6.56x. The label uses this same average (`Session::reached_speed`), and not the
panel's half-second sample.

"No sound" silences the mixer while fast-forwarding. "Unlimited" is always mute: the real speed
varies from a light frame to a heavy one, and sound that followed it would change pitch all the
time.

The right path, and a bigger one, is for the game to produce the samples by the virtual clock
into a ring that feeds the card, as the core already does (`render(due)`). It is the "a single
owner for audio" refactor of [21-audio-refactor.md](21-audio-refactor.md), and is left for it.

## The turbo

**The rhythm is the virtual clock's, not the host's.** The frontend delivers the controller once
per pass, and a pass can run several frames: four to catch up with the clock, ten at 10x. A turbo
that toggled the button on every pass would pulse ten times slower for the game during
fast-forward. That is why the frontend says **which** buttons pulse (`Session::set_turbo`), and
the session decides **when** they are pressed, before each pass of the event loop, by the phase
of the game's clock: half the period pressed, half released.

The default is 10 presses per second, the 6-frame period RetroArch uses out of the box, in a
range from 5 to 20. A game that reads the controller through `GetState` at 20 frames per second
may miss presses above 10 per second; the ones that read through the event queue do not.

The modes are RetroArch's, per player:

- **Classic**: holding the turbo key together with a button makes that button pulse.
- **Single button (toggle)**: one press of the turbo key makes the default button pulse while it
  is held, and another press undoes it. It does **not** fire on its own: a stuck autofire would
  carry through the menus, where button 1 is "confirm".
- **Single button (hold)**: while the turbo key is held, the default button pulses by itself.

The D-pad and HOME never pulse. RetroArch excludes the D-pad by default, and a pulsing HOME would
open and close the console's menu. The turbo key is one more input in each player's mapping
(`buttons["turbo"]`), and not a Zeebo button.

**The BREW key pulses too.** The controller reaches the game by two paths: the `IHID` state and
the `EVT_KEY` keys (button 1 is confirm, `0xe064`; button 2 is `AVK_CLR`), which the desktop
window derives from the controller. Pulsing only the state would leave a game that reads keys
without turbo. Instead of moving the whole derivation into the session, which would change five
frontends, the session **asserts on top** the key of the pulsing buttons and hands it back to the
raw state when they stop. That only happens when the frontend says it derives the keys
(`Session::turbo_on_keys`). The core and Android do not derive them, and there the session does
not invent a key nobody releases.

## The rewind

**Return points, not a smooth rewind.** A game state is 15 to 66 MB and takes 5 to 37 ms to save.
Saving every frame, as RetroArch's rewind does, does not fit in 16 ms. The rewind marks a point at
every interval of play (default 1 s), in a ring with a memory ceiling (256 MB on desktop, 128 MB
on Android). Holding F8 goes back one point every 250 real ms and shows its frame, with the sound
muted. On release, the game continues from the last point shown, and the ones newer than it have
already left the ring. While paused, each press goes back one point. Rewind beats fast-forward:
while going back, the game does not advance.

**Only the copy runs in the game loop**; the compression (fast deflate) goes to a separate thread.
Measured on 2026-10-10, one point per second of play:

| Game | Path | Capture in the loop (median / max) | Per point, compressed | Going back one point |
|---|---|---|---|---|
| Double Dragon | software | 11.5 / 12.4 ms | 3.39 MB | 23–30 ms |
| Double Dragon | GPU | 10.7 / 11.4 ms | 3.18 MB | 22–27 ms |
| Crash Nitro Kart | software | 12.6 / 15.2 ms | 4.11 MB | 33–39 ms |
| Crash Nitro Kart | GPU | 6.7 / 14.6 ms | 3.34 MB | 27–32 ms |
| Zeebo Extreme Rolima | software | 33.5 / 38.9 ms | 18.34 MB | 133–144 ms |
| Zeebo Extreme Rolima | GPU | 19.1 / 19.1 ms | 6.08 MB | 45–55 ms |

The capture is one hitch per point; in Rolima on the software rasterizer, two frames lost per
second. That is why rewind comes off by default. Quake and Android were not measured.

**A point is not made while a file is open for writing.** The state stores an open file by its
path and offset, not by its contents (see `machine/save.rs`). Going back to the middle of a write
would make the game keep writing from an old offset on top of a file that has already changed:
the player's save. A point the engine rejects (a file the game deleted afterwards, for example)
leaves the ring, and the previous one is tried. **What the game wrote to disk does not come
back**: going back to before a save does not undo the save, the same as the manual save state.

### Rewind in Qt

Rewind did not work in the Qt window. There it was the only path that restored a state, because
the window had no manual save state (it came later, see [25](25-desktop-save-states.md)), and
reading the shortcut ran outside the game's GL context.
Going back one point rebuilds the textures on the GPU: they were deleted and created in whatever
context was current, Qt Quick's or none. Reading now happens with the game's context current,
and the `RunningGame`, as before advancing, notifies the GPU state mirror
(`Session::resume_context`) before going back one point. Android sends the same notice. With the
software rasterizer the notice does nothing, and that is why no test without a GPU observes it.

### The black screen and the step forward

On the GPU, Magical Drop III and Um Jogo de Ovos stayed on a black screen for the whole rewind.
Two things added up. The last frame stays in the GPU texture until someone asks for it, and the
texture is not part of the state: the point came back with the screen of an old frame, or black
in a game that only draws through GL. And, once the state was restored, the screen's write
counter came back equal to the saved one, the screen looked untouched, and the window showed the
GPU texture, which comes back empty.

The point now brings the GPU frame to the screen before saving (`materialize_gl_frame`, with the
game's context current, right after it advances), and the restored machine says the image is the
screen's until the next buffer swap (`Machine::gpu_frame`). Measured with the `RunningGame` on
the GPU, in Magical Drop III: without the first half, each point's screen came out with 0 lit
pixels; with it, with about 237 thousand.

The same measurement showed the rewind going **forward**: 30,023 ms, back to 29,000, and the next
step back to 30,023. The point the thread was compressing when the game went back entered the
ring afterwards and became the newest. Removing a point now changes the ring's generation, like
clearing it, and whatever was in flight is discarded: 30,023, 29,000, 27,977, 26,954.

### The sound that did not go back

The mixer is not part of the save state, and restoring did not touch it. Going back in the rewind
left playing whatever the mixer was playing the instant before: the next scene's music over the
restored one, forever when it was a loop, because the restored game does not know about that
voice and never tells it to stop. Seen in Action Hero 3D, whose music is a looping MP3. The same
applied to the manual save state.

Now restoring silences the whole mixer and plays again what the state says is playing
(`Machine::rebuild_sound`). An effect comes back from the point the virtual clock marks, counting
the repetitions. A loop comes back from the start, because the instant it started is not stored.
A PCM stream comes back empty, and the game fills it again. A sound whose cache is not in memory,
in a state from another session, stays silent until the game plays it again, as it already did.

### The clock that did not go back

The rewind exposed an old save state defect. The game's clock is
`clock_us + instructions / 528`, and the state saved only the instructions. The comment said
`clock_us` "is the instruction counter divided by the rate", but it is not: it is the idle time
the emulator skips (the vsync, `skip_idle_time`, `MSLEEP`), and in the measured games it is most
of the clock. In Double Dragon, a state saved at 20,265 ms came back at 25,408 ms. The timers and
the next vsync, saved in absolute time, all expired at once. `clock_us` now goes into a separate
section, `relogio.us`, and the same state comes back at 20,032 ms saved and 20,032 ms restored.
The section is optional: an old state loads as it used to. This also applies to the Android and
core save states.

## The shortcuts

The Qt **Shortcuts** tab gathers all the game window's keys in one place, like Dolphin's *Hotkey
Settings*:

- **Changeable**: the screenshot (keyboard only, F9), fast-forward (F10), rewind (F8) and restart
  (keyboard only, R). Fast-forward and rewind accept a key or a button on any connected
  controller.
- **Fixed**, read-only: Esc, P, F11.

**Restart opens the same game again**, right away, without confirmation, like a console's reset
button (`Core::restart`). It is not a machine reset: opening is the only path that sets
everything up the way the game expects — its settings, the ports, the sound, the Z-Wheel it came
from. What the game wrote to disk stays. Like the screenshot, it rejects a key that is already a
Zeebo button, and is checked before the game. Someone who already used R on a button, in a file
from before the shortcut, gets restart **without a key** (`Settings::adopt_new_shortcuts`),
instead of losing the button. On Android, "Restart" is in the back dialog, next to pause.

Pause shows a **⏸** in the center of the screen, with the word below it; resuming shows a **▶**
for half a second, and restart a **↻**, which fade out. They are drawn with `Shape`, not with
characters: not every font has ⏸ and ↻.

A button that belongs to a shortcut **does not reach the game** (`Shortcuts::reserved`). If it is
also a Zeebo button, the editor warns, but lets you save. The defaults are keyboard only: the
Zeebo controller uses almost every button of a regular controller, and any button reserved out
of the box would steal one from some game.

Android has no F10. Fast-forward, rewind and turbo get **on-screen pieces**, next to HOME and
above the diamond, and a physical controller button can be bound to each one in the settings.
The button names are the desktop ones (`South`, `LeftTrigger`…), so the same `settings.json`
works on both. Each piece has a show/hide toggle. Turbo and rewind, on top of that, only appear
with the feature in use; fast-forward always appears.

## What the core does not do

The Libretro core only got the core library's types for the limit and the frameskip, with the
same option keys. Fast-forward, turbo and rewind belong to RetroArch. RetroArch's rewind, however,
serializes **every frame**, and with 15 to 66 MB states that take 5 to 37 ms to save it is no
good with this core. That is stated here, not promised.
