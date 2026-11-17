# Z-Wheel — Setup screen

The survey behind each item is in
[docs/implementation/31-z-wheel-settings-screen.md](docs/implementation/31-z-wheel-settings-screen.md).

- [ ] **1. Registration and the keyboard.**
  - [x] `0x01028e1b`, the list widget (`CreateListWidget`, `0x20fcc`): in the family, slot 13
        hangs the model instead of creating a bitmap
  - [x] `0x01028e2f`, the text field of the "hidden menu form" (`0x4a414`): in the family; its
        model is the **game's** object, counted through its own vtable
  - [x] the keyboard draws whole: each `IGraphics` has its own destination — the shift's
        `SetDestination(gfx, NULL)` pointed the display back at the screen and every key after it
        was lost
  - [x] **typing**: the field's item `0x224` is its controller, where the keyboard hangs its key
        handler (`0x461b8`); `QueryInterface(0x1015946)` is its text model (`ReplaceSel` 5,
        `GetTextInfo` 7, `SetSel` 8). Measured: right, confirm, up, confirm types "HY"
  - [x] backspace: it sends `AVK_CLR` to the controller's previous handler (`0x465a4`); slot 4
        now hands out a default `HandleEvent` (slot 18, `DefaultHandler`) that deletes
  - [x] OK goes to the birth date: `0x341` is a container's `{child, visible}` — keyboard out,
        date lists in
  - [x] the date lists (`0x01028e1b` as ListWidget): the arrows move `0x165` through the list's
        default handler, the game's item callback (`0x163`, `(ctx, index, state)`) gives the
        text, painted in the middle of the list (declared substitute: current value only). The
        list holds its item widget (`IDECORATOR_SetWidget`). Measured: 16 → 17 → 18, confirm,
        "Julho"
  - [x] the choices reach the game: the list's `0x161` is its view model; the game's listener
        (`0x4d3d4`) keeps the index from `EVT_MDL_FOCUS_CHANGE` (`0x1007`). Measured: month,
        year and sex now show up in the field above the list
  - [x] a hidden container hides what it holds (`shown`): the previous question's list no
        longer stays painted under the next one
  - [ ] the summary — the window reaches it; confirm that day, year, sex and the newsletter
        answer appear now (the CEP/e-mail script is long, not run headless)
  - [ ] long text widgets do not wrap ("Deseja receber notícias e ofertas…" runs past the panel):
        wrap only where the widget's height has room for two lines, so the status bar labels,
        which our font measures wider than the console's, do not break
  - [ ] "Confirmar" sends the registration and waits forever on the spinner: network, same
        family as item 3
  - [ ] the lists' extent is a pointer (`805314384` = `0x30001e50`): slot 7 on `0x01028e1b`
        reads the wrong word
  - [x] "CANC." is the ROM's own string (`tectoy_pt.brf` of 1.0.727); the screenshot is from
        another version
  - [ ] the keyboard's per-frame 500×180 bitmap: 77 alive after 1.5 s before the `IGraphics`
        fix; after it, `Release -> 0` at `0x45860`. Re-count with `--dump-surfaces`
  - [ ] draw the list's items (who calls the `0x163` callback, and with what)
- [x] **2. Focus on the Setup screen.** A container that never set a focus hands it to its first
      child with a handler (`ensure_first_focus`), only when two or more compete. The rows no longer
      turn together and Z-Credits shows its frame on open. Main menu and game grid measured
      unchanged (grid frames byte-identical).
  - [ ] the full chain (slot 4 returning the widget's default `HandleEvent` instead of `(0, 0)`)
        is still not modeled; nothing measured needs it yet
- [x] **3. Shop / Buy / Check balance / Refill card "hanging".** Not the network: the press builds
      the "Não foi possível conectar à rede" box (the game already detects the missing network),
      and the **release** of the same key reached the box's OK button (`0x515ec`), which fires on
      `EVT_KEY_RELEASE` without checking it saw the press — the box closed 240 ms later. A release
      now goes only to the handlers that got the press and are still on screen
      (`press_receivers`). The box's text was under its frame: static texts are now painted after
      the owner-draws. The language box ("Deseja alterar o idioma…? Sim / Não") showed up by the
      same fix — before, the release confirmed it on its own.
  - [ ] the box paints over a white screen; on the console it is probably an overlay
  - [ ] long text does not wrap, and the `\n` of the language box shows as `??`
- [x] **4. The language restart's swallowed fault** (`0xa7a7a700`). "Sim" restarts the applet
      inside the release delivery; the next handler in the list was the freed box's `0x51a6c`.
      A release delivery now stops when the current form changes (only the release: on the press,
      `0x49b38` after the keyboard's `0x461b8` is what tears down the setup screen's timer).
- [x] **5. Help / News faulted at `pc 0` every frame.** Draw handlers get an `ICanvas`
      (`PFNDRAWHANDLER`), not the bitmap; `0x30bdc` calls `GetBitmap` (slot 3) and `SetBitmap`
      (slot 4) on it. A canvas over the screen is now passed (names in `zeebx-core`'s `CANVAS`).
- [x] **6. `IModel_AddListener` on the HTML models** (`0x31c34`, `0x31ca4`) answered as
      "missing": registered now; nothing notifies them yet.
- [ ] **7. Navegar** ends in `Failure attempting to launch app, error(1)`: the browser is looked
      up in the Shop catalog (`browser_itemid=19354124`). Open the Opera Mini package if
      installed, or hide the item (`enable_browser=0` in the cfg copy).

Survey: `python3 tools/zwheel_survey.py --zip roms/Z-Wheel.zip` runs every screen in a throwaway
profile and lists what is missing, scenario by scenario.

Instruments added on the way: `<focus …>` lines for `0x700`/`0x711`/`0x713`, `<painting form …>`,
the observed classes' calls in the session log,
`<drawing … did not return>`, and `zeebx session --dump-surfaces=DIR`.
