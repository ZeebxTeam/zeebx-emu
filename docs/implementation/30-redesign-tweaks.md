# 30 — Redesign tweaks

Tweaks after the four phases ([27](27-window-frame.md), [28](28-game-in-the-main-window.md),
[29](29-settings-with-sidebar.md)), asked for after using the interface: the Z-Wheel icon, the
bottom of the window while the game runs, the Z-Pad and the Dragon on a single button, the Z-Wheel
and the scan disabled during the game, the question before closing the game, and the age rating
in the details inside the dialog.

## The Z-Wheel is home

The Z-Wheel button, on the toolbar and in the File menu, used Lucide's `disc-3`, a disc, which
does not say what it is. The Z-Wheel is the console's home screen: from there you pick a game and
start. The icon became `house`, and the label stays "Z-Wheel", which is the name the console gives
it. `disc-3` had no other use and left the font subset, which still has 33 icons.

## The bottom of the window during the game

With the game open, the bottom of the window had a strip that would not go away:

- **in the separate window**, a 24 px line with the title and where the game draws, with no option
  at all to remove it;
- **in the main window**, the status bar, which only went away through the View menu, and then
  went away from the library too. During the game it showed the game count and the key hint,
  which belong to the library.

Now both get out of the game's way:

- **The separate window's line no longer exists.** The game's title stays in the window title
  ("Zeebx — Double Dragon — software").
- **The status bar hides while the game runs in the main window**, and the game extends down to
  the bottom of the window. In the library it stays, and View → Status bar still applies. With the
  separate window, the library stays visible in the main window, and so does the bar, with the
  game on the right, as before.
- **The debug panel stays as it was**: the bottom strip, which shrinks the game, with its options
  under Debug.

### The metrics in the corner

What the bar and the line said about the game went to a box in the top right corner, over the
frame, turned on under Debug → "Show FPS and metrics in the game corner" (`debug.metricas`, off by
default). It does not depend on the panel and takes no space from the game, so it can stay on
while playing:

```
                                   31 FPS
                    103% of console speed
                                    Software
```

The frames per second are highlighted; below, the speed relative to the console and where the
game draws: "Software", or "GPU · 2× · llvmpipe", with the internal resolution and the GPU's name.
The width is at most half the frame's, and a long GPU name ends in an ellipsis. The lines come
from the same session sample the panel and the status bar use (`Session::sample`), and the core
builds them already translated (`Core::metrics`).

The screenshot and save state notices, which already lived in that corner, move down below the
box when it is showing.

## Z-Pad and Dragon on a single button

The two controllers have the same buttons, the same axes and the same drawing on the controls
page; what changes is the grip. The device choice had four buttons (Z-Pad, Dragon, Boomerang,
Keyboard), and now has three: **Controller**, Boomerang and Keyboard. With Controller chosen, a
"Model" box next to it picks between the Z-Pad and the Dragon.

**The model did not go away because the console sees it.** Each one announces its own USB
identifier (`1A5C:3033` the Z-Pad, `1EAA:0135` the Dragon), and the Boomerang Sports games change
their handling by it ([20](20-boomerang-and-wii-remote.md)). The saved configuration did not
change: the same `ZPad` and `Controle` in `settings.json`, and the same indices in
`cfg.deviceKind()`.

- Coming from the Boomerang or the keyboard, Controller picks the Dragon, which is the emulator's
  default.
- Clicking the already selected Controller does not change the model.
- **The three buttons are exclusive like radio buttons** (`autoExclusive`). Without that,
  clicking the already checked Controller unchecked it: Qt toggles the button's `checked` on
  click, and since the device did not change, the binding did not check it again.

## The Z-Wheel and the scan wait for the game

With the game in the main window, "Open the Z-Wheel" and "Scan for games again" are disabled, in
the File menu and on the toolbar, just as pause and restart are without a game. They are library
things, and the game is in the library's place: opening the Z-Wheel would swap the game for it
without any question. The button's tooltip says "Close the game to open the Z-Wheel.". The
controller's HOME button already did not open the Z-Wheel during the game: the library does not
listen to the controller while a game is open (`Main.overlaid`).

With the separate window, the library stays visible in the main window, and both still work, as
before: a game opened on top of another stays in the window where the previous one was
([28](28-game-in-the-main-window.md)).

## The question before closing the game

One Esc too many threw away whatever was not saved. Now closing the game asks first, like PCSX2's
"Confirm Shutdown": "Close the game?", with "Close the game" and "Keep playing". These go through
the question:

- Esc in the game (only the first press: held, the repeated Esc would close the question it
  opened itself);
- Close in the System menu and on the toolbar;
- closing the separate window;
- quitting Zeebx with a game open, through the File menu or by closing the main window ("Quit
  Zeebx?").

**The game pauses while the question is open**, and backing out returns it as it was: a game that
was already paused stays paused. Esc and a click outside the question back out, so two Escs in a
row close nothing.

**The question lives in the stage, not in the main window** (`GameStage.requestClose`), so it
appears where the game is: a dialog of the main window would open behind the separate window. The
main window's Close with the game in the separate window brings that window to the front. The one
asking is the stage; the ones closing are still the main window's `closeGame()` and the separate
window's `close()`, which no longer ask.

**These do not ask:** the Z-Wheel, which is the home screen and holds nothing that can be lost,
and a game that exits on its own. The "Ask before closing the game" option, under Graphics, next
to the separate window, turns all of this off (`graphics.confirmar_ao_fechar`, on by default). It
is global, like the window choice, and does not change per game.

## The age rating in the details

In a game's details (controller button 3, or C), the age rating is a wide strip: the age, the
theme and the content, side by side. The image only had the height set (64 px), and kept the
file's width: it went past the dialog's right edge, over the main window. Now it has the column's
width, and the height follows the aspect ratio, up to the same 64 px.

## How it was checked

In the Ubuntu 24.04 container, with the `.deb`'s Qt 6.4.2, on a virtual display driven by
`xdotool`:

- the house on the toolbar;
- Bad Dudes in the main window, with the metrics on: without the status bar, the game down to the
  bottom of the window, the box in the corner ("31 FPS · 103% · Software") and the F9 notice below
  it;
- Bad Dudes in the separate window, with the metrics and the panel on: without the 24 px line, the
  panel at the bottom and the box at the top; the main window with the status bar and the game on
  the right;
- the controls: Boomerang → Controller lands on the Dragon; Z-Pad through the box; Controller
  again keeps the Z-Pad and stays checked; Keyboard saves `Teclado` in `settings.json`;
- the new option on the Debug page, outside the panel group.
- with Bad Dudes in the main window, the house and the scan greyed out on the toolbar and in the
  File menu;
- Esc asked, with the game paused; the second Esc backed out and the game started running again;
  the toolbar's Close asked, and "Close the game" brought the library back;
- Quit in the File menu, with the game open, asked "Quit Zeebx?", and "Quit" ended the process;
- in the separate window, Esc and the Close on the main window's toolbar asked inside it;
- the Z-Wheel, opened through the house, went back to the library on Esc without asking;
- Bad Dudes' details with the rating inside the dialog.

**Not checked:** closing the separate window through the frame's button. The virtual display has
no window manager, and the path was only seen through Esc, which reaches the same question.
