# 28 — The game in the main window

The third phase of the interface redesign. Up to 0.4.3 the game always opened in a window of its
own, next to the library. Now, as in PCSX2, it is drawn in the main window itself, in place of the
library, with the menu bar, the toolbar and the status bar around it ([27](27-window-frame.md)).
Closing the game brings the library back. The separate window still exists: it is the Settings >
Graphics > "Game in a separate window" option (`graphics.jogo_separado`), PCSX2's "Render to
Separate Window".

## One stage, two homes

What used to be the whole `Game.qml` (the frame, the notices over it, the pause symbol, the debug
panel, the keyboard and the step on every frame) moved out to `GameStage.qml`, a component
without a window. The main window has one stage, and the separate window has another. **Only the
stage where the game is advances**: each one's `FrameAnimation` runs with its `active`. There is
a single core, and with both stages active it would advance twice per frame.

That is why **where the game appears is only decided with no game running**. A game opened on top
of another, through the library with the separate window or through the Z-Wheel, stays where the
previous one was. The option applies on the next game.

What belongs to the window, like closing and going fullscreen, leaves the stage as a signal
(`closeRequested`, `fullScreenRequested`), and whoever hosts it decides what to do. The main
window's System menu, toolbar and status bar talk to the active stage (`Main.stage`), and work
with the game in either of the two.

**Two stages, not one that changes window.** A Qt Quick item can change window through `parent`,
but the `FrameItem` keeps the frame texture in the scene graph of the window it is in, and the
move would have to rebuild that node in the middle of the game. With two stages there is no move
at all: each one lives in its window, and the idle one does not draw.

## In the main window

- **The library leaves and the stage comes in**, taking the whole area, without the library's
  margins. The search and the grid and slider buttons disappear from the toolbar, and Ctrl+F does
  not go to the search: during the game, the letters belong to the game.
- **The window stays in the mode configured for the game** (`graphics.janela_do_jogo`: windowed,
  maximized or fullscreen) while it runs, and goes back to what it was when it closes. The main
  window's own mode, changed in the settings during the game, waits for the game to close.
- **In fullscreen, the three bars go away**, as in PCSX2: the screen belongs to the game. F11 and
  Alt+Enter toggle.
- **The status line below the frame does not exist**: what tells the title and where the game
  draws is the status bar. The window title becomes "Zeebx — Double Dragon — software". Later, the
  status bar started hiding during the game, and the line left the separate window too
  ([30](30-redesign-tweaks.md)).
- **The run log** stays on top of the window the game is in.
- Losing focus releases the keys, as in the separate window.

## The texture in another window

On the GPU, the frame is a texture of the rasterizer's own GL context, and the `FrameItem` shows
it without going back to the CPU ([21](21-qt-migration.md)). The context is shared with Qt
Quick's (`AA_ShareOpenGLContexts`), and that is why the same texture serves the main window as it
served the separate window. Checked with Alien Breaker, which draws in 3D, on Mesa's GPU driver
(llvmpipe).

## Coming from 0.4.x

The new option does not exist in the old `settings.json`, and its default is the game in the main
window: whoever updates sees the redesign, and the separate window is one click away.
`graphics.janela_do_jogo` still applies, now to the main window while the game runs.

## How it was checked

In the Ubuntu 24.04 container, with the `.deb`'s Qt 6.4.2, on a virtual display driven by
`xdotool`:

- Double Dragon opened from the grid appears in the main window, P pauses (the symbol appears and
  the button becomes "Resume") and Esc brings the library back, with the search and the view
  buttons;
- Alien Breaker, on the GPU, appears in the main window drawn by llvmpipe; F11 removes the three
  bars and the second F11 brings them back;
- with the separate window on, the game opens in its own window, with the status line below; the
  main window's System menu follows its pause, and Esc in the game window closes it.
