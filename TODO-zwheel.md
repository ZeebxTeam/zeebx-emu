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
  - [ ] **typing**: keys reach only the form's handler (`0x49b38`), which forwards to the previous
        one expecting the default form behavior to route to the focused child
        (`0x711` on `0x30001a50` → `0x30002250`). Blocked on item 2
  - [ ] "CANC." where the console shows "CANCELAR" — probably our font measures wider and the
        game picks the short string
  - [ ] the keyboard's per-frame 500×180 bitmap ends with one reference too many
        (`Release -> 1` at `0x45860`): 77 alive after 1.5 s
  - [ ] draw the list's items (who calls the `0x163` callback, and with what)
- [ ] **2. Key routing by focus.** Today the key goes to every handler of the current screen
      (`flush_keys`). It should go down the focus chain: form → content (`0x5000`) → focused child
      (`0x711`), with each handler forwarding to the previous one. Fixes the Setup rows turning
      together, the missing first frame, and typing on the keyboard.
- [ ] **3. `IDOWNLOAD` failing like a console without network**, so Buy / Check balance /
      Refill card / Shop show the connection error instead of hanging.
- [ ] **4. The language restart's swallowed fault** (`pc 0xa7a7a7a4`). Instrumented: the serial now
      logs `<fault …>` with `lr`, and `<callback … did not return>` with the callback delivered.

Instruments added on the way: `<focus …>` lines for `0x700`/`0x711`/`0x713`,
`<drawing … did not return>`, and `zeebx session --dump-surfaces=DIR`.
