# 27 — The main window's frame

The second phase of the interface redesign, along the lines of PCSX2 2.x: the library window gets
a menu bar, a toolbar and a status bar. **The grid and the slider stay as they were**; what
changes is what goes around them. The first phase was the themes ([26](26-themes.md)); the next
ones, the game drawn inside the main window ([28](28-game-in-the-main-window.md)) and the settings
with a category sidebar ([29](29-settings-with-sidebar.md)).

The row of loose buttons at the top (Z-Wheel, search, Saves, Settings, Unlock sync) and the
count row with "Scan again" are gone: each thing went into a menu, the toolbar or the status bar.

## The menus

```text
File           Open the Z-Wheel · Scan for games again · Quit
System         Pause/Resume · Restart · Fast-forward · Screenshot · Save state ▸ ·
               Load state ▸ · Undo load state · Close the game
Settings       General · Controls · Shortcuts · Graphics · Speed · Audio · Debug ·
               Settings for this game…
View           Grid · Slider · Toolbar · Status bar · Fullscreen
Tools          Saves… · Run log · Unlock the Zeeboids sync
Help           GitHub repository · Discord server · Check for updates · About Zeebx
```

**The key next to the item is only text.** The `shortcut` of a QML menu item applies to the whole
window: a P typed in the search would pause the game. The one handling the keys is still the game
window, and the item shows the saved shortcut (the screenshot, restart and the save state ones
are changeable, so the text comes from the Shortcuts tab).

The System menu acts on the open game, which in this phase still opens in its own window. The
actions go through the same functions as the game window's keyboard (`Game.pause()`,
`restart()`, `screenshot()`), and the pause symbol in the middle of the screen shows up either
way. Without a game, the whole menu is disabled.

- **Fast-forward is a checkable item**, which turns it on and off. A menu item cannot be held,
  and feeding the shortcut a continuous "pressed" does not work: in hold mode it would turn on,
  but in toggle mode it would become a single press, and releasing would not turn it off.
  `RunningGame` has a separate switch (`toggle_fast_forward_lock`), which applies on top of both
  modes; the shortcut stays as it was.
- **Rewind is not in the menu.** Going back only makes sense while holding, and it stays on F8.
- **Save state and Load state** show the ten slots, with each one's size or "Empty", and Load
  disables the empty ones. Picking a slot makes it the current one, as in PCSX2: the following F1
  and F3 go to it. The current slot shows the key. The list is read from disk when the System menu
  opens, not on every draw.
- **The slot submenus have a minimum width.** Qt Quick's `Menu` measures the items the first time
  it opens, and did not grow when an empty slot became used: the "Used · 14.1 MB" ran into the
  key.

The checkable items (Grid, Slider, the bars, the log, fast-forward) are re-read when the menu
opens, not bound: a click on a checkable item breaks its binding to the setting, and a change made
in the settings would not show up in the menu.

**With a menu open, the library does not listen to the controller**, just as it already did not
with another window on top (`Main.overlaid`).

### Check for updates

At startup, the check for a new version only shows up when it finds one. From the Help menu,
whoever asked wants the answer either way: "up to date" and the failure come back in their own
dialog (`Core::update_check_result`), and a new version opens the usual notice, with the
installer.

### About

The settings' About tab became the Help > About Zeebx dialog, with the logo, the version and the
two addresses. Android keeps its own About screen.

## The toolbar

Z-Wheel and Scan again; Pause, Restart and Close, enabled with a game open; Settings and
Controls; Fullscreen (the game's, if it is open, and the library's, if not). On the right, the
grid and the slider, like PCSX2's grid/list pair, and the search, with the magnifier icon and the
clear button inside the field.

**The buttons do not take keyboard focus**, like the earlier ones: once clicked, a Qt Quick button
keeps it, and the arrows stopped going to the grid.

With the window narrower than 1200 points, the labels disappear and the name goes to the
tooltip. The width is what the Portuguese labels call for, the longest of the four languages.

### The icons

The icons are from [Lucide](https://lucide.dev), drawn as glyphs of a font, not as SVG: the color
is the text's, and follows the theme without an image to recolor; and Qt does not need the QtSvg
module, which the `.deb`, `windeployqt` and `macdeployqt` would have to bring along.

The full font has 2150 icons and 900 KB. [`tools/icons.py`](../../tools/icons.py) cuts out only
the ones the interface uses (33, at 13 KB, in `assets/icons/lucide.ttf`, embedded in the binary)
and writes the name table of `Icon.qml`. To add an icon, the name goes into the script's list and
it runs again, over the `lucide-static` package of the same version. The license is ISC, and MIT
for the icons that came from Feather; both are in `tools/licenses/lucide-LICENSE` and in
`THIRD-PARTY-NOTICES.txt`.

## The status bar

On the left, the game count and the key hint that used to be on the second row. With a game open,
on the right, its name, where it draws (GPU or software) and the speed of the last sample, the
same as the debug panel's: "Double Dragon — software · 100% · 60 FPS".

Both bars can be hidden through the View menu, and the choice is saved (`barras` in
`settings.json`). After the phases, the status bar started hiding while the game runs in the main
window, and what it said about the game went to the metrics box in the corner
([30](30-redesign-tweaks.md)).

## How it was checked

In a container with Ubuntu 24.04 and Qt 6.4.2, the `.deb`'s: `zeebx-qt` opened on a virtual
display (Xvfb), driven by `xdotool`, with a screenshot at each step. With Double Dragon open, the
System menu enabled the actions, saving to slot 3 through the submenu wrote the file and showed
the notice in the game, Load state only allowed picking slot 3, loading showed "State loaded from
slot 3. F4 undoes it.", and the status bar showed the game and the speed. The frame was also seen
in the Orange and Dark themes, and the narrow toolbar, with icons only.
