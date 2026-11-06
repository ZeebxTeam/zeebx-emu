# 26 — Themes

The Qt interface has color themes: System (the default, the operating system's palette), Light,
Dark, AMOLED, Orange, Blue, Ruby and Emerald. You pick one in Settings > General > Theme, and the
change applies right away, in every window. It is the first phase of the interface redesign,
which follows PCSX2's lead.

## Palette, not style

Qt Quick has two ways of changing the look. **The style** changes how the controls are drawn
(Fusion, Material, Universal, Imagine, FluentWinUI3). **The palette** changes only a style's
colors. PCSX2's themes are palettes over Fusion, and Zeebx's are too. The style was left out for
four reasons:

- the style is fixed when the program opens, and changing it requires a restart;
- Material and Universal take their colors from their own properties (`Material.theme`,
  `Material.accent`), not from the palette. The library grid and slider are hand-drawn with
  `palette.*`, and would end up with colors that do not match the rest;
- FluentWinUI3 only exists from Qt 6.8 on, and the `.deb` uses Ubuntu 24.04's Qt 6.4;
- every new style would have to ship in the AppImage, the `.exe` and the `.app`, and be checked
  in all three.

Fusion is still the style (`gl_qt.cpp`), for the same reason as before: it follows the palette in
every control, and Basic painted dark text on a dark background.

## The file

```text
assets/themes/<key>.json        the built-in ones, embedded in the binary
<executable folder>/temas/      a portable copy
<config>/temas/                 the ones anyone wants to add
```

Format 1 has `nome` per language, `escuro`, `cores` with the twenty `QPalette` roles (with the
names QML uses: `window`, `windowText`, `button`...), `desabilitado` with the text of disabled
controls, and `aviso` and `erro`, the two colors the QML used to hardcode (`#e0a030` and
`#d04040`). The reader is [`src/ui/themes.rs`](../../src/ui/themes.rs).

**Every role is required.** A missing role would get the system's color, and a light theme text
over a light system background is unreadable. A file with a missing role, a color that is not
`#rrggbb` or an unknown format is ignored, with the reason in the log; the program opens with the
others, and whoever had chosen it stays on the system palette until the file is valid again. A
file with the key of a built-in one takes its place.

The choice is saved by key (`"tema": "rubi"` in `settings.json`); `null` is System.

## How the palette reaches the windows

cxx-qt-lib does not have `QPalette`, so the assembly lives in C++
([`cpp/palette.cpp`](../../frontends/qt-standalone/src/qt/cpp/palette.cpp)). Rust passes color by
color and it applies to the whole program, through `QGuiApplication::setPalette`. To go back to
System, `setPalette(QPalette())`. On Qt 6.8 or newer, `styleHints()->setColorScheme` also tells
the system whether the theme is dark, so the title bar follows along where it knows how.

Three things were measured, because none is obvious from the documentation:

- **The windows follow the change with the program open**: the background, buttons, fields,
  labels and the `Rectangle`s bound to `palette.*`. Checked in a test program on Qt 6.12 and on
  Ubuntu 24.04's Qt 6.4.2, and then in `zeebx-qt`, changing the theme through the selector on a
  virtual display.
- **The theme is applied after `engine.load`**, not before. Applied before, only the loose items
  (the grid cards) followed it: the windows and the Fusion controls were born with the system
  palette. They only switch to the program's when it changes after they already exist. After
  `load` the windows exist and have not been shown yet, so nobody sees the change.
- **Outside `emu_core::with`.** `setPalette` notifies the windows immediately, the QML bindings
  are re-evaluated right there, and one that calls the core would find the `RefCell` borrowed.
  The theme is copied inside and applied outside (`theme::apply`).

The warning and error colors come from the QML `Theme` object, reloaded when the theme changes
(`SettingsModel.themeChanged`). The game window does not follow the theme: the background is
black, and the notices over the game stay dark, because they are read over the game, not over the
interface.

### The tooltips

The interface's tooltips replace the `ToolTip` content with their own `Label`, and a `Label`
paints with `windowText`. With the system palette this never showed, because the yellow tooltip
and the window's dark text go together. In the Light theme, with a dark tooltip and a light
window, the tooltip text vanished. The tooltip `Label`s paint with `palette.toolTipText`.

## The contrast

[`tools/check_themes.py`](../../tools/check_themes.py) checks the format and the contrast of the
pairs the interface actually draws, by the WCAG 2.1 rule: 4.5:1 for text and 3:1 for borders and
for the fields' placeholder text. Among the pairs are the title of the selected card (`highlight`
at 16% over `button`) and the border of the slider's selected box against its fixed light panel
(`#ececec`). The script's role list has to move together with `ROLES` in `themes.rs`.

```bash
python3 tools/check_themes.py                 # the built-in ones
python3 tools/check_themes.py my-theme.json   # any theme
```

In the dark themes, the highlight fits in a narrow band: dark enough for the text over it and
light enough for the border to show against the window and against the slider panel. The
highlights were found by varying only the lightness, at the chosen hue and saturation.

- **In AMOLED**, fields and buttons have their own background. The border Fusion draws is the
  window color darkened (`darker(140)`), and with a black window it comes out black too and
  disappears.
- **Orange and Blue come from the Zeebo.** Orange's highlight is the orange of the console's box
  (`#cd4114` in a photo; `#ca4014` in the theme). Blue's has the hue and saturation of the logo
  lit up on the console (225°), with the lightness that white text over it calls for.

## What was left out

- Choosing the style, for the reasons above.
- An accent color separate from the theme. Each theme brings its own, matched to the other
  colors; whoever wants another one makes a theme.
