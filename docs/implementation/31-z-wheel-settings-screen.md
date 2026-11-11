# 31 — The Z-Wheel settings screen: what breaks, option by option

**Who this document is for:** whoever picks up the Z-Wheel's "Configurar" (Setup) screen — the
balance options, the language, registration, the keyboard. It is a survey, measured on
2026-11-11 with the binary built from `5f2d525`, before any fix. The carousel itself is in
[18-the-z-wheel-carousel.md](18-the-z-wheel-carousel.md); read §3 (the accessor), §5.5 (handlers)
and §7 (key routing) there first.

## How to reproduce

"Configurar" only exists in the wheel with end-of-life off (§7.3 of doc 18). Windowless, one run
per option, each fits in about 13 s of wall clock:

```bash
zeebx session roms/Z-Wheel.zip --seconds=30 --no-end-of-life \
  --dump=out/s.bmp --serial=out/s.serial \
  --keys=16000:left,18000:left,20000:b1,23000:down,24500:b1 --shots=26000,29500
```

- The main menu is up at ~15 s. The wheel reads `Jogar · zeebo · Shop · Navegar · … ·
  Configurar`, with `zeebo` focused: **two lefts** reach Configurar, `b1` opens it.
- `--dump` saves a frame 500 ms after each key, numbered (`s.1.bmp`, `s.2.bmp`…).
- On open, the focus is on the **Z-Credits** row (second row). One `down` goes to Language,
  two to More Settings.
- **The profile is persistent.** A language confirmed in one run is the language of the next
  one (`PREFSINFO.Lang`, `538997872` = `"pt  "`, `538996325` = `"en  "`). Restore it at the end.

The screen, in Portuguese:

| Row | Roller items |
|---|---|
| `# do Zeebo` | text field with the console number |
| `Z-Credits 35` | Pré-pago (refill card) · **Comprar** (buy) · Verificar (check balance) |
| `Idioma` | Español · **Português** · English |
| `Mais Opções` | Ajuda (help) · **Cadastro** (registration) · Apagar (delete games) |

The forms behind them, from the source file names in `tectoy.mod`'s strings: `SetUp_Form.c`,
`Credit_Action.c`, `Buy_Form.c`, `Choose_Amount_Form.c`, `Boleto_*_Form.c`,
`ScratchCard_Number_Confirmation_Form.c`, `Registration_Form.c`, `DeleteGameForm.c`,
`tectoy_kbdform.c`, `tectoy_keyboard.c`, `tectoy_listwidget.c`.

## Option by option

| Option | Result | Cause |
|---|---|---|
| **Language** (change + confirm) | **Works.** Writes `Lang`, the Z-Wheel restarts in place and comes back in the new language ("Play", "My Z-Credits"). But every time: `invalid access at 0xa7a7a7a4 at pc 0xa7a7a7a4` | see §1 |
| **Buy / Check balance**, and the main wheel's **Shop** | Nothing changes on screen; the app waits forever | see §2 |
| **Refill card** | Nothing changes | probably the same as §2 (`Credit_Action.c`, `ScratchCard_*`); not isolated |
| **Registration** | A **magenta** rectangle, and a jump to `0x0` | see §3 — also the path to the keyboard |
| **Delete games** | **Works**: the grid with sizes, sorted by Title / Size / Least played | — |
| **Help** | not tested from this screen; it is the same FAQ as the wheel's, which works | — |

### 1. The language restart jumps into freed memory

Confirming on the Language row is not "close the screen": on the **release** of the key the
Z-Wheel saves `Lang`, tears down EGL (`eglDestroyContext`, `eglDestroySurface`,
`eglTerminate`), creates the root `0x01028e51` again, reopens every database and reconnects the
joystick — the game log shows `Joystick.c`/`MDN` twice. That is the app's own soft restart, and
it works.

What does not belong is the swallowed fault, seen only on this path (`0xa7a7a7a4` in one run,
`0xa7a7a700` in another). `0xA7` is likely the fill the app's own `memcheck.c` writes over freed
blocks — **hypothesis**, not verified: the PC was loaded from a structure the Z-Wheel already
freed. The suspects are on our side: a widget handler (slot 4), a draw callback (slot 16) or a
timer still alive in our tables after the restart released its owner — the same class of defect
as §7.2.2 of doc 18 (the tab bar's 40 ms timer over freed memory).

**What is missing to pin it:** `swallowed_failures` is a `BTreeSet<String>` with no instant and
no `lr`. A serial line in the `MemoryFault` arm of `Machine::execute`
([`machine/mod.rs`](../../src/machine/mod.rs)), with the clock, `lr` and the callback being
delivered, answers who called it.

### 2. Shop, Buy and Check balance wait on an `IDOWNLOAD` that never answers

After the confirm the Shop reads `AsstUPDTAvail` and `UIUpgdAvail` from the preferences and goes
silent: no error in the log, no new screen. The path goes through `ShopAction_Init`, which
creates `IDOWNLOAD` (`0x01000000`) — served today by "an object that answers success to
everything" (it is in the report's assumptions). Success without ever calling the game's
callback leaves it waiting.

The module says what the console does offline:

```text
shop watchdog fired and dataend set, showing connection error
Failed getting credit info, leaving shop...
check balance request sending failed!
```

**What to do:** make `IDOWNLOAD` fail the way a console without network fails — an error
returned, or the callback called with an error — so that the watchdog fires and the message box
shows up. Which of the two needs the slot reading of `IDownload` (`AEEDownload.h`); the probe
(`--probe=0x01000000`) shows which slots the Shop calls.

### 3. Registration needs `0x01028e1b`, and that is the way to the keyboard

```text
[24566 ms] <class 0x01028e1b>
[24566 ms] *dbgprintf-4* ..\..\common\tectoy_listwidget.c:79
[24566 ms] Unable to create list decorator in CreateListWidget
```

`0x01028e1b` is not in `WIDGET_FAMILY` ([`machine/mod.rs`](../../src/machine/mod.rs)). The
form is half-built — the magenta is an unpainted surface — and the cleanup jumps to `0x0`.

**The keyboard is drawn by the Z-Wheel itself**, not by the console: `tectoy_keyboard.c` builds
the key bitmaps (`Unable to generate bitmap for extended key keyboard`,
`Unable to create keys vector model inCreateKeyboardWidget`). The reference from the console's
documentation is a QWERTY layout with a bottom row `123 · ESPAÇO · CANCELAR · OK`. What the
emulator owes is the classes it asks for, in the order they will show up:

1. `0x01028e1b` — the list decorator (`CreateListWidget`); the first refusal.
2. `IZeeboKeyboardCTL` — the module has `IZeeboKeyboardCTL_New: returning EFAILED`; class id
   still to be read.
3. The keyboard's roller (`Failed to create roller widget for keyboard wizard`) and its font
   (`Unable to create font in KeyboardWizard_CreateSmallListWidgets`).

Registration is the shortest path to the keyboard: fix one, the next refusal appears.

## Two bugs that hold for the whole screen

### Left and right turn all three rows at once

With the focus on Z-Credits, `right` also turns Language and More Settings. The serial:

```text
[23050 ms] <key 0xe034 press to 5 handler(s) on screen and 4 off
            (0x6089c 0x6089c 0x6089c 0x77300), handled>
```

`0x6089c` is the roller's handler, one per row, and all three act. `flush_keys`
([`machine/hid.rs`](../../src/machine/hid.rs)) hands the key to **every** handler of the
current screen, and `out_of_focus` ([`machine/widget.rs`](../../src/machine/widget.rs)) only
filters once some container has a focus written through `0x711`. Measured: after the first
`down`, `right` moves **only** the focused row — the down writes the focus, and from then on the
filter works. Before it, nothing is filtered.

### The initial focus is not drawn

The Z-Credits row has the focus on open (one `down` lands on Language) but no blue frame. Same
cause: the initial focus is set through a path we do not record, so neither the roller is
notified (`EVT_WDG_SETFOCUS`, `0x700`, which is what turns on its frame at `[+0xac]`) nor the
key filter knows about it.

**What is missing to pin both:** the serial only logs the accessor's writes (`0x801`). Logging
`0x700`, `0x711` and `0x713` with the widget, the container and the value shows how
`SetUp_Form` marks the first row — whether through `MOVEFOCUS(FIRST)`, through a direct
`0x700`, or through a container we do not link to the rows.

## Small visual defects

- The console number sits at the top of its field instead of the middle.
- The Language row uses a larger font than the other rows.
- The size in Delete games has a stray glyph before the number (`⊡29.29 MB`).

## The missing APIs the report lists, and why they are not the problem

```text
IWidget::Acessador selector 0x102  (class 0x1028e51, from 0x0007b41c, 0xe033, 0x0)
IWidget::Acessador selector 0x7001 (class 0x1028e51, from 0x0007b41c, 0x0, 0x0)
IWidget::Acessador selector 0x7b0b (class 0x1028e51, from 0x0007b41c, 0x1, 0x20207470)
```

All three come from `0x7b41c`, the applet offering an event to the root before its own
`switch` (§5.7 and §7 of doc 18). `0x7b0b` carries the new language in `dwParam`
(`0x20207470` = `"pt  "`). Answering "not handled" is the right answer there; they show up as
"missing" only because the root does not list them as known.

## Suggested order

1. **`0x01028e1b` in the widget family**, as a generic container/decorator. Unblocks Registration
   and opens the way to the keyboard. The cheapest change.
2. **Focus selectors in the serial** (`0x700`/`0x711`/`0x713`), then fix the initial focus. Fixes
   the rows turning together and the missing frame.
3. **`IDOWNLOAD` failing faithfully**, so Buy, Check balance, Refill card and Shop show the
   connection error instead of hanging.
4. **Instant and `lr` of each swallowed fault in the serial**, to find what survives the
   language restart.
