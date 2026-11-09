# 29 — Settings with a sidebar

The fourth phase of the interface redesign, and the last one in the plan. The settings window
swaps the tabs at the top for a column of categories with icons, on the left, and the explanation
of the options moves from below each one to a panel at the bottom, as in PCSX2. The pages
themselves (the options, the order, what each one saves) did not change.

## The sidebar

General, Controls, Shortcuts, Graphics, Speed, Audio and Debug, each with a Lucide icon
([27](27-window-frame.md#the-icons)): `settings`, `gamepad-2`, `keyboard`, `monitor`, `gauge`,
`volume-2` and `bug`. The same icons went into the main window's Settings menu.

**The column keeps the `id: tabs` and the `currentIndex` of the old `TabBar`.** Whoever opens the
window on a category (`open(1)`, the Settings menu items) and the pages that read the controller
while they are visible (`tabs.currentIndex === 1`, the controls one and the shortcuts one) ask for
them, and stayed the same.

The categories do not take keyboard focus on click, like the toolbar buttons: the key capture of
the controls and shortcuts pages needs it.

In single-game mode (a game's own settings), the categories that do not apply per game (General,
Shortcuts and Debug) disappear, and the column shows only Controls, Graphics, Speed and Audio.

## The explanation at the bottom

Below each option, the explanations stretched the pages until they no longer fit in the window.
Now the bottom panel shows the one for the option under the pointer, or with keyboard focus:
whoever moves with Tab reads what each option does. With nothing to explain, it says how to make
one appear. The height is fixed: one that followed the text would make the window jump at every
option hovered.

**What changes with the state stays next to the option**: the disconnected port, the capture
waiting for a key, no controller found, the calibration in progress. That is a notice, not an
explanation, and needs to be seen without searching.

### How the page finds the explanation

Three markers say what to explain:

- the **`SectionTitle`** of a group of options carries the group's `description`, which applies
  from the title to the next one. The fixed hints that came right below a title became its
  `description`;
- the **`Option`** (an on/off toggle) carries its `hint`, which used to sit below it, dimmed;
- the **`Explanation`** takes no space and explains what is right above it: a combo box, a row of
  buttons. The fixed hints that came after a control, and not after a title, became
  `Explanation`.

Each page (`SettingsPage`) has a `HoverHandler`. With the pointer at height `y`, it takes the
nearest marker above it (`describe`). An option without a hint shows its group title's. Above the
first title, `pageDescription` applies: that of the pages that opened with a general explanation
(Shortcuts, Graphics, Audio and Debug). Changing page clears the previous one's explanation.

**Only the page's direct children count.** The fixed hints that live inside a nested group (the
internal resolution and the antialias, inside the GPU rasterizer group; the rewind memory; the
turbo and the axes, on the controls page) stay below the option. Moving them would require each
group to be a page too, and was left for later.

An item's list of children is not iterable with `for...of` in Ubuntu 24.04's Qt 6.4: the loop
threw `TypeError` and the bottom panel stayed frozen. `describe` walks by index.

## The window

Bigger, to fit the column: 900 by 660, and at least 600 by 400 (it was 720 by 640, and 420 by
320).

## How it was checked

In the Ubuntu 24.04 container, with Qt 6.4.2, on a virtual display driven by `xdotool`: on the
General page, the pointer over the theme selector brought up the theme's explanation, and over
"End-of-life Z-Wheel", its own; on Speed, over the frame skip, the frame skip's, and over rewind,
its own; on Shortcuts, the page's. Opened through the X in the library, a game's settings window
showed only the four per-game categories, with the option to use its own settings at the top.
