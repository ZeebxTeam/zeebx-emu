# 22 — Screenshots

A shortcut in the game window (F9, rebindable) writes the frame on screen to a `.png`. The yardstick
is the [RetroAchievements screenshot guidelines](https://docs.retroachievements.org/guidelines/content/screenshot-guidelines.html):
the image comes from the emulator itself, at the resolution the game was drawn at, with no filter,
no watermark and no window upscaling. What the site accepts beyond native is 3D rendered larger
**by the emulator's internal resolution**, which is how DuckStation and PPSSPP do it.

Only the Qt window has the shortcut. Choosing the frame and writing it live in the core
(`src/ui/screenshot.rs` and `Session::capture`), with nothing of the window, so Android can gain a
button later without rewriting anything.

## 1. Which frame comes out

**The frame the window shows, at the size it exists at.** There are two:

| On screen | What comes out |
|---|---|
| 3D on the GPU, with nothing drawn on top in 2D, with the internal resolution above 1x (or the wide aspect ratio on) | The GPU texture, read whole: 1280×960 at 2x, 1920×1440 at 3x |
| Everything else: a 2D game, a HUD through `IDisplay`, a message box, the Z-Wheel, the software rasterizer, the internal resolution at 1x | The console screen, **640×480**, which is the Zeebo's resolution |

The rule for which of the two is on screen is that of `Session::gpu_frame`, the same one Qt's
`Core::frame` uses to choose between texture and image. **Calling `large_frame` directly does not
work**: it reads the GPU without asking whether there was 2D drawing after the last
`eglSwapBuffers`, and in a game with a HUD the screenshot would come out without the HUD. The HUD
is composed on the CPU, over the 640×480 screen, and there is no large version of it. That is why,
in that case, native comes out, and not an upscale: nearest-neighbor upscaling adds no detail at
all, and RetroAchievements does not accept it.

Checked with `zeebx session --gpu --scale=3 --shots=…`, which says for each shot whether the window
would show the large frame: in the three shots of Alien Breaker Deluxe (3D on the GPU) the
screenshot came out at 1920×1440, and in the three of Action Hero 3D it came out at 640×480. The
latter draws its 3D through the IMICRO3D engine, in software, on the 2D screen (zero GL frames up
to 19 s), and `large_frame` alone would have returned a 1920×1440 that was not what was on screen.

### The configured resolution applies, even above 3x

The site says "never 4x or more". The shortcut belongs to the emulator, not only to the site:
whoever set 6x wants to see 6x. Whoever is going to submit to RetroAchievements picks 2x or 3x in
the settings, as they would in DuckStation. The site's 6 MB limit does not pinch at 3x: the Alien
Breaker Deluxe title screen at 1920×1440 gave a 2.2 MB PNG. Above that it was not measured.
Reducing a 6x frame to 3x would mix pixels, which is exactly the image change the yardstick
forbids; and rendering an extra frame just for the screenshot would take a frame for real.

The experimental wide aspect ratio follows the same logic: the screenshot comes out **with the
sides**, the way it is on screen. The site forbids widescreen, but the aspect ratio is the choice of
whoever configures it, like the scale.

The stretched surface (Quake draws at 320×400 and Qualcomm stretches it to 640×480) comes out at
the surface's aspect ratio: 960×1200 at 3x. That is the "aspect uncorrected" the site asks for by
default.

### Eight bits per channel, no alpha

The console screen is RGB565, and that is how the Zeebo shows it: at native, each channel is
expanded by repeating the high bits (`0b11111` becomes 255, not 248), as in `Framebuffer::to_argb`.
The GPU frame is drawn at eight bits, and `Session::large_frame` returns it as a `Framebuffer`,
that is, RGB565: writing through it would throw away two or three bits per channel of what the GPU
had already computed, and gradient banding would show up in the sky. The screenshot reads the raw
RGBA (`Session::capture`).

The PNG is RGB, with no alpha channel. The texture's alpha is not image information: Double Dragon
and the other Data East games clear the background with zero alpha, and that is what leaked through
the Qt window (see "the frame's alpha" in [`21-qt-migration.md`](21-qt-migration.md)). Written as
it came, the background of those games would come out transparent in the screenshot.

## 2. Where it writes

```
<folder>/<game>/<game> - YYYY-MM-DD HH-MM-SS.png
```

- **The folder** is `config_dir()/screenshots/` by default (`~/.config/zeebx/screenshots` on
  Linux), and it can be changed in the General tab (`screenshots_dir` in `settings.json`). The name
  is in English, unlike `relatorios/` and `aparelho/`, because it is what people coming from
  RetroArch recognize. `~/.config` is hidden, and that is why the tab has the button to open the
  folder.
- **One subfolder per game**, with the library title (`Core::title`): whoever takes ten
  screenshots of each game finds them all together. Without a title, `Zeebx`.
- **The title loses what Windows refuses** (`< > : " / \ | ? *` and control characters), and the
  trailing dots and spaces, which Windows also deletes silently. The same file has to be able to go
  from one system to another.
- **The time is local, and comes from Qt** (`Qt.formatDateTime` in QML). The core has no time zone
  at all: the guest's clock is GMT, and pulling in a date dependency just for a file name would be
  disproportionate.
- **Two screenshots in the same second** get ` (2)`, ` (3)`… The name is reserved by the file
  system's `create_new`, not by a check beforehand: two F9 presses in a row write on different
  threads, and "is the name free?" followed by "create" would let both pick the same one.

## 3. The shortcut

**F9 by default, rebindable** in the Shortcuts section, at the end of the Controls tab
(`atalhos.screenshot` in `settings.json`, with the key name from `bindings`). Esc, P and F11 stay
fixed: the `Shortcuts` struct is born with a single field, but it is their place if they ever
become rebindable.

- **Keyboard only.** The Zeebo controller has no obviously free button, and a combo
  (Select+something) would require a per-button shortcut screen that does not exist.
- **Capturing the shortcut refuses** Esc, P and F11, and refuses a key that is already a Zeebo
  button on some port, saying which one ("F9 is already Button 1 on port 1"). Swapping the two
  would change a mapping without the user seeing it. For the same reason, capturing a Zeebo button
  refuses the shortcut's key.
- **In the game window, the shortcut wins.** An old `settings.json` may have F9 as a button;
  `Game.qml` first asks whether the key is the shortcut, and the game does not receive it.
- **Auto-repeat does not count.** Holding F9 takes one screenshot, not thirty.

## 4. The notice, and the thread

After the screenshot, a notice in the corner of the game window for two seconds, in the style of
the calibration notice: "Screenshot saved as: Quake - 2026-09-25 14-03-07.png". Only the file name,
not the resolution: whoever chose the internal resolution already knows which it is, and the folder
opens with a click on the notice. A failure (a folder without
permission, a full disk) shows in orange with the reason, and the path goes to `stderr`. It does not
go to the log window: that shows `Session::log`, which belongs to the game, and the frontend has
nowhere to write there.

- **The notice does not go into the PNG.** It is drawn by Qt on top of the frame, and the frame
  comes from the core. The site forbids watermarks, and that is also why there is no white flash: it
  would show up in a video recorded externally.
- **The read is on the interface thread; the compression is not.** The texture can only be read
  with the GL context current, and the context belongs to the interface thread (see the top of
  `emu_core.rs`). The PNG goes to a thread of its own, and the answer comes back through a channel
  on the game's next turn. How much compressing a 3840×2880 costs was not measured: the thread is a
  precaution, not a number.

## 5. What was left out

- **egui**, which goes away in phase 8 of the migration; **the Libretro core**, because RetroArch
  already has its own screenshot and the house rule is that whoever delivers the video is the
  frontend; **headless**, which already writes a `.png` per frame (`dump.rs`); and **Android**,
  which has no F9.
- **The save state thumbnail on Android** (`src/ui/save_slots.rs`) uses `large_frame` without the
  HUD question, and may come out without it. It is not part of this work; noted here.

## Note for the next version

The notes live in `docs/patch-notes/` and are written at release time. What this one goes in
saying:

> - **Screenshots.** F9 in the game window saves the screen as a `.png`, in a folder per game. With
>   3D on the GPU and the internal resolution at 2x or 3x, the image comes out at that size;
>   otherwise, it comes out at 640×480, the Zeebo's resolution. The shortcut and the folder can be
>   changed in the settings. If you had F9 as a controller button: in the game window it now takes a
>   screenshot — change the shortcut or the button.
