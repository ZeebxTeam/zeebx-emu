# 06 — 3D video

## The contract

The console has an **Adreno 130** (formerly ATI Imageon). Here the GPU is the host's CPU. What
matters is the contract: the game hands over vertices in object coordinates, matrices, textures and
a handful of fixed states, and expects a frame back.

`video/rasterizer.rs` is the classic fixed pipeline, **without lighting**: it transforms, clips
against the near plane, divides by perspective, maps to the screen and fills triangles with
perspective-correct interpolation, depth test, blending and alpha test.

## EGL and the two forms

BREW exposes OpenGL ES in **two** ways, and the console's games use both:

| | What it is like |
|---|---|
| `IEGL`/`IGLES` | normal COM interface, with `this` in the first argument |
| old `AEEGL.h` form | no `this`, direct return — `Interface::EglLegacy` and `GlLegacy` |

`eglGetProcAddress` returns `aee::encode(Interface::Gles, slot)` after stripping the `gl` prefix,
so that a function obtained at runtime lands in the same dispatch as the others.

## The surface

Deduced from the largest `glViewport` the game asked for. And there is a rule that was costly to
discover:

> **Whoever never calls `glViewport` gets the default viewport, which OpenGL defines as the whole
> surface.**

Deducing the surface from an empty set of viewports gave 1×1, and the presentation stretched a
single pixel across the whole screen — it was the cause of Zeebo Sports Peteca's "white screen",
and I blamed the window before finding this.

The second rule: **the `y` of `glViewport` counts from the bottom.** Both rasterizers store the
frame top to bottom, and for a long time they used `y` as if it counted from the top. With a
full-screen viewport (`y = 0`, height equal to the surface's) the two readings coincide, and that
is why almost no game showed a difference. Crash Nitro Kart did: it draws the next stretch of track
through a portal, with the viewport and the clip on the portal's rectangle. The stretch ended up
mirrored in the bottom half of the screen, the portal area stayed empty, and when the kart went
through and the portal became full screen, the scenery "rose from the bottom up". It looked like
draw distance, and the game's portal recursion depth limit had nothing to do with it.

The conversion lives in one place only, `top_down_viewport()`, with `y_top = surface_height − y −
height`, used in drawing and in `draw_texture`. The internal viewport that the graphics card's
`import_rgb565_changes` builds is already counted from the top and doesn't go through it.

`glDepthRange` is also served in both rasterizers: the normalized `z` goes to
`near + (far − near) × (z/w × 0.5 + 0.5)`. Crash alternates ranges to put the kart's glow on top
of everything else, and before, the call was silently ignored.

## Two texture units — in both rasterizers

The Adreno 130 has two units, and OpenGL ES 1.1 requires at least two. For a long time the
pipeline drew only unit 0 and answered `GL_MAX_TEXTURE_UNITS` = 1; today it has both
(`TextureUnit`): each with a bound texture, on/off switch, environment (`TexEnv`) and coordinate
array, and the vertex carries `uv1`. Unit 1 acts on what came out of unit 0 — in `GL_COMBINE`,
`GL_PREVIOUS` is unit 0's output and `GL_PRIMARY_COLOR` the vertex color. It is sampled without
mipmaps.

**The binding and the parameters belong to the active unit.** Resident Evil 4 ends each block on
unit 1, turning off the texture and swapping the bound one; when the graphics card applied that to
the base texture, the village came out white. `bound_texture` answers for the active unit, which is
the one a `glTexImage2D` reaches.

Answering just one was what hid Dragon Vs Chicken's characters — see its section.

## The texture matrix

`GL_TEXTURE` **must** be applied to the `uv` coordinates. Games send UV in fixed point (32767 is
common) and count on the matrix to bring it into the right range. Without it Peteca's textures
turned into noise — and the first diagnosis I gave, "missing mipmap", was wrong.

## Texture formats

| Module | Format | Who uses it |
|---|---|---|
| `video/atc.rs` | ATITC (`GL_AMD_compressed_ATC_texture`) | the Adreno's native format; Boomerang Sports Dodgeball loads **everything** this way, without a single `glTexImage2D` |
| `video/paltex.rs` | `OES_compressed_paletted_texture` | a 16- or 256-color palette followed by the indices |

Two traps recorded in tests:

- **The `paltex` palette is little-endian.** Reading it as big-endian gave rainbow stripes on the
  Double Dragon logo — plausible enough to go unnoticed.
- **The `level` of the paletted formats is non-positive**: zero is just the base level, negative
  says how many mipmaps come after. Since we only use the base, what matters is always the first
  stretch of indices after the palette.

ATITC blocks are 4×4 texels, like DXT1: two colors and two index bits per texel. The difference is
in the highest bit of the first color, which chooses between interpolating the palette's four
colors or reserving the first one for black.

### The decoder's PNG reaches the texture in its own format

Many games build their textures from the PNG `IImageDecoder`: they take the bitmap, read the
`IDIB` fields and copy `pBmp` into a `glTexImage2D`. **Alien Breaker Deluxe** shows the contract in
the disassembly: with `nDepth` 8 it uses the palette; otherwise it copies `nDepth / 8` bytes per
pixel and picks `GL_RGB` for 3 and `GL_RGBA` for the rest. Our bitmap announced RGB565, 16 bits,
and the game copied two bytes per pixel thinking they were four: the logos came out white.

The decoder's bitmap now publishes the `IDIB` in the PNG's own format — 32 bits in RGBA when the
image has alpha, 24 in RGB when it doesn't, contiguous rows and 8 bits per channel. The RGB565 copy
stays in the surface map for our blits, and that buffer stays out of synchronization. The same bug
was **Heavy Weapon**'s "images full of glitches", which now shows the mission map, and **Tork and
Kral**'s wrong images.

### The current color is clamped to [0, 1]

OpenGL ES 1.1 clamps the color of `glColor4f`/`glColor4x` to [0, 1] at the moment it is set.
Alien Breaker Deluxe paints with `glColor4f(255, 255, 255, a)` — it is the byte value in a
floating-point parameter —, and on the console that is pure white, which leaves the texture
intact. Storing 255, our modulator multiplied the texture by 255, and the title and menus came out
blown out to white, with only the dark edges showing.

### `glScissor` really clips

It was accepted and ignored, on the grounds that drawing too much is the least visible error. It
isn't, when the game **counts** on the clip: Peggle draws the whole font sheet and tightens the
scissor around one letter, the same trick Pac-Mania does with `IDisplay`'s clip. Ignored, each
letter put the whole sheet on the screen — the menu and the game table came out covered with
alphabets.

In both engines the rectangle goes in together with the viewport, and with the same convention:
the `y` of `glScissor` counts from the bottom up, and our surface counts from the top. In the
software rasterizer it tightens each triangle's box, and so it costs nothing per pixel; on the
graphics card it is the card's own `glScissor`, multiplied by the attachment's scale.

**And it goes through the same conversion as the viewport in widescreen.** The scissor arrives in
console pixels; the attachment is wider. While it went in raw, a full-screen scissor — which is
what Resident Evil 4 and Crash Nitro Kart turn on in-game — cut everything beyond the console's
640, and the sides that widescreen had just opened stayed in the attachment's background color.
Games that don't use the scissor, like Raging Thunder 2, never saw the strip.

**On the graphics card, the scissor's `y` is also converted to the top** (`scissor_on_attachment`,
in `video/gpu.rs`). The viewport went through `top_down_viewport` and the scissor went in raw; on
the full screen the two readings coincide, and only a rectangle showed the difference. Crash Nitro
Kart's portal is one: viewport and scissor on its rectangle, and the scissor fell on the mirrored
strip — the portal came out empty and the scenery went back to "rising out of nowhere" when going
through it, the same symptom the viewport once had, ever since the scissor started being honored.
The software rasterizer already converted it.

And, in widescreen, the scissor **shifts** by `extra` and only grows up to the edges it already
touched. Widening it by the viewport's ratio, as the first version did, worked on the full screen
and made a portal's rectangle leak outside the frame.

## Fog

`glFog*` was silently served and `GL_FOG` ignored. Resident Evil 4 asks for linear fog from ten to
seven hundred and eighty units to separate what is near from what is far, and without it the whole
scene came out with the same brightness.

The factor comes from the **distance in eye coordinates** — `|z|`, as OpenGL allows instead of the
vector's length, and it is what every fixed-function implementation does. It is computed in the
vertex stage, which is shared by both rasterizers, and travels in the `Vertex` like the other
attributes; in the fragment it goes in **after the texture and before the alpha test**, touching
only the RGB, which is the OpenGL ES 1.1 order. On the graphics card it is one more vertex
attribute, and the shader does the blending.

In the `x` forms `GL_FOG_MODE` comes as an **integer**, not fixed point: it is an enumeration, and
converting it as a scale would give `0x2601/65536`, which is no mode at all.

`GL_FOG_COLOR` has **four** components and has to be in `gles::components`: outside it `glFogxv`
read only the first one, and Resident Evil 4's beige fog arrived with green and blue zeroed — red.

**The graphics settings switch belongs to the player, not the game.** On the console fog usually
hides what the draw distance didn't reach, and here the scene arrives whole; whoever prefers to see
far turns it off. It is on by default — the game asked for fog, and in many of them it is the
effect, not the patch.

## Dragon Vs Chicken, the SDK sample

The SDK 1.2.4 `conftest.mod` is a demo game on the QX engine (`.qxm` meshes, `.qxt` textures,
`.qxa` animations, `.qxp` particles), and the engine writes a `data/qx.log` with what it loaded.
It talks to GL **only through `IGL`**, the old interface without `this`, and asks for the buffer
functions through `eglGetProcAddress`. Three bugs of ours showed up with it:

- **`eglGetProcAddress` returned functions with `this`.** A `gl*` name resolved to the `IGLES11`
  slot, which reads the arguments starting from `r1`; whoever calls a C function pointer sends them
  from `r0`. `glGenBuffers(1, &name)` took the pointer as the count and wrote outside memory. Today
  the name is looked up in the `IGL` table, which gained at the end the functions `AEEGL.h` doesn't
  have (the floating-point ones, the buffer ones and the extension ones); the beginning, which is
  its vtable, didn't change.
- **The extension suffix falls back to the core function.** We announce
  `GL_ARB_vertex_buffer_object`, and QX, on finding it, asks for `glBindBufferARB` and the other
  five. The pointer was null; now it is `glBindBuffer`'s. The same goes for `OES`.
- **Zero offset in a buffer is a real array.** `gles_draw` gave up when the vertex pointer was
  zero, and with a buffer bound the pointer is an offset — QX issues
  `glVertexPointer(3, GL_FIXED, 0, 0)` for every mesh. Once it started using buffers, the whole
  scenery vanished, until this (`ArrayPointer::in_use`).

And `glTexEnv` gained **`GL_COMBINE`** (`TexEnv`, in both rasterizers): function, three sources and
operands for the RGB and for the alpha, scales and `GL_TEXTURE_ENV_COLOR`, on unit 0. Before, it
fell into `GL_MODULATE`. QX configures the combination with the source on the texture. Invalid
functions and sources are rejected, and the sources accept the crossbar — see the Bóia Cross
section.

**The characters vanished because we answered a single texture unit.** The engine asks for
`GL_MAX_TEXTURE_UNITS` at startup and stores the answer in the extension manager; the lighting
manager checks the number (`0x2df08`: if it is less than two, it sets bit `0x100`, which turns off
lighting), and its draws (`0x1ec20`) give up with the bit set. Without lighting, each mesh fell
into the per-vertex color path — pointed into the loaded mesh itself, where `dragon.qxm` stores
zeros —, and the texture in `GL_MODULATE` came out black and transparent, failing the alpha test.
The shadows showed up because they don't go through there.

With two units, the engine lights through the texture: `DOT3_RGB` between the bump map
(`*_b.qxt`) and the vertex color, which now carries the light direction, on unit 0; and
`ADD_SIGNED` with the color texture, alpha from `GL_TEXTURE_ENV_COLOR`, on unit 1. The dragon, the
chickens and the whole scenery — waterfall, rocks, the lighting — show up, in both rasterizers.

## Bóia Cross's water: invalid enum and the crossbar

The water on the first track (Santo André) came out blue-purple and opaque; in the longplay
captured from the console it is brown and murky. The trace of its drawing
(`zeebx session --trace=IGLES11`, game code at `0x3cf64`) shows QX building a bump effect on two
units:

```
unit 0  texture 0xca (normal map, the lilac-blue)
        GL_COMBINE_RGB = GL_DOT3_RGB
        GL_COMBINE_RGB = GL_TEXTURE0        <- meant GL_SRC0_RGB
        GL_SRC1_RGB    = GL_PRIMARY_COLOR   (the light vector, ~(0.5, 0.95, 0.7))
unit 1  texture 0xc9 (the water color, brown)
        GL_COMBINE_RGB = GL_COMBINE         <- is no function at all
        GL_SRC0_RGB    = GL_PREVIOUS
        GL_SRC1_RGB    = GL_TEXTURE1
        GL_COMBINE_ALPHA = GL_REPLACE, GL_SRC0_ALPHA = GL_TEXTURE1
```

Two bugs of ours added up:

- **We accepted any value.** In the driver, a function or source that doesn't exist is
  `GL_INVALID_ENUM` and the previous value stays: unit 0 stays on `DOT3`, unit 1 on the default
  `MODULATE`. Here `GL_TEXTURE0` became the function and fell into multiplication — the normal map
  times the light vector, which is the water's blue-purple color. `TexEnv::define` now rejects
  both.
- **`GL_TEXTURE0` and `GL_TEXTURE1` as a source** (`ARB_texture_env_crossbar`) fell into
  `GL_PREVIOUS`. Unit 1 multiplied the previous result by itself, and the brown texture was never
  sampled. The crossbar is not part of OpenGL ES 1.1, but the device accepts it: with the source
  rejected, unit 1 would stay on the default `GL_PREVIOUS`, with no brown at all, and the water in
  the video is brown.

In both rasterizers the two units now sample before either of them combines, and a source that
points to a unit that isn't drawing makes the unit pass the previous result along, which is the
crossbar rule. This last one wasn't measured on the device — no tested game exercises it.

## The Zeebo Extreme draw distance

The scenery that pops out of nowhere in Rolima **is the game's decision, not clipping of ours.**
Measured in the race on the first track:

- The projection is `glFrustumx` with `near = 1` and `far = 500`. With the far plane four times
  larger, the screenshots of the same instants come out identical pixel by pixel: the game sends
  nothing beyond 500.
- The race reads nothing back from GL — no `glGet*` —, so the choice of what to draw is entirely
  up to its own math, the same as on the device.
- It is some 58 `Draw*` and 3 thousand triangles per frame, stable.
- The software rasterizer clips at the near plane and resolves the far one pixel by pixel, in
  depth; the graphics card clips both. Neither path discards a whole triangle.

The criterion is in `pak0.pakz` (a `PACK` of LZMA files, with the index at the end in 64-byte
entries: 56 for the name, the position and the size). `infos/gamecartsinfo1p.txt` splits each
track into segments (`models/pista_serra0.xtreme` to `24`, each with its own `.octree`) and gives
the last waypoint of each: the game draws the segments around where the player is. The levels of
detail declared there (`400` near, `1000` far) are for the racers' meshes, three per racer
(`pers1_lod1` to `lod3`); the track doesn't have a separate low-quality version.

Extending the distance would be an adjustment of ours on top of the game — finding and changing
the segment window —, not a fidelity fix.

## `glReadPixels` and the top side

Zeeboids draws the character and **reads the frame back** to build the profile picture. The origin
of `glReadPixels` is the bottom-left corner, and the first row of the result is the bottom one of
the screen; our surfaces count from the top, so the read flips the rows.

This holds for both engines, and it is easy to get wrong on the graphics card: there the frame is
**also** stored with row 0 at the top, because Y is flipped in the vertex shader — that is what lets
the screen readback not need a mirror on the CPU. Whoever reads directly from the card's
`glReadPixels` therefore gets the image already in the surface's convention, which is the wrong
one for the game. Read raw, Zeeboids' picture came out upside down, and with it the character's
face in every game that uses it later. There is a test comparing the orientation in both
rasterizers.

## `GL_OES_draw_texture`

The screen blit: a rectangle drawn **in window coordinates**, without going through the matrices.
It is the path an emulator uses to put its screen on the device's screen, and the console has the
extension.

The piece of the texture comes from `GL_TEXTURE_CROP_RECT_OES`, four **signed** integers — a
negative width or height mirrors the axis, which is how the extension flips the image. A zeroed
crop is the whole texture: drawing nothing would be worse than adopting the obvious default.

The OpenGL window has zero at the **bottom**, unlike our surface. Getting this flip wrong puts the
image upside down, and in an arcade emulator that passes for "it worked" — hence the test being
about exactly this orientation.

The eight forms (`s`, `i`, `x`, `f` and the vector ones) draw the same thing; only how the number
arrives changes. They are **not** part of the `IGLES` vtable: they sit at the end of the name
table, and the game reaches them through `eglGetProcAddress`.

## The console's extensions

Ten arcade ports — all on the same engine, a Neo Geo emulator — gave up on graphics initialization
and wrote **"InitGLExtensions failed"** on the screen. They don't use `eglGetProcAddress` by name:
they ask for the extensions through **`QueryInterface` on the EGL object**, the way the SDK's
`GLES_ext.c` shim does.

| Interface | What it gives |
|---|---|
| `IEGLSurfaceManip` | surface scaling, rotation, transparency and overlay |
| `IGLESImageonExt` | the ATI Imageon extras on top of OpenGL ES 1.0 |

Both have complete headers in the vendored SDK, with the whole method table. V2 is a superset of
V1 **with the same vtable prefix**, so one name table serves both IIDs.

Almost everything there answers "done" without doing anything, and that is deliberate: rotation,
transparency and layers don't change what the game draws, only how the console composites the
result. Refusing would make the game give up over a feature it doesn't even get to use. The ATI
and Qualcomm vertex buffers are the exception — there, answering success without storing anything
would make the next draw come out of garbage, and refusing is more honest.

**Scaling is the one that does real work.** In `SetSurfaceScale` the game declares the rectangle
it draws in, and that is worth more than the deduction from the viewport: here it *says* the size.
An arcade emulator draws at 320×224 and lets the console upscale.

### And the extensions have to be announced by name too

Having the interface wasn't enough. The games also look, **by substring**, for three names:

```
GL_OES_draw_texture          in glGetString(GL_EXTENSIONS)
GL_ATI_imageon_misc          same
EGL_QUALCOMM_surface_scale   in eglQueryString(EGL_EXTENSIONS)
```

In their binary all three appear **with a trailing space** — the search delimiter. While
`eglQueryString` returned empty, video didn't initialize no matter that the interfaces existed.

### The missing name was worth a whole game

The list has a fourth entry today, `GL_ATI_texture_compression_atitc`, and gained a fifth:
`GL_ARB_vertex_buffer_object`. That last one cost a game while it was left out, and it is worth
telling how, because the symptom didn't point to GL at all.

**Prey Evil** showed up in the survey as "stops in the loop — jumps to address zero
(lr 0x000161d8)". That address is a `blx r1` after `ldr r1, [r0, #0x274]`: it calls a function
pointer stored in a field of one of its objects. The field is null, and what leaves it null is the
game itself, at `0x1ddc4`:

```
0x1ddd4  bl   …            <- builds the extension list
0x1dde4  blx  r2           <- strstr(list, "GL_OES_draw_texture")     -> found
0x1de04  blx  r2           <- strstr(list, "ARB_vertex_buffer_object") -> 0
0x1de0c  beq  #0x1de3c     <- and then it **doesn't install** the function
0x1de1c  str  r0, [r4, #0x274]
```

Two searches, and it only installs the drawing function if it finds **both**. Then it calls the
pointer without checking. That is: the game doesn't tolerate the extension being missing, it
assumes it exists — the console has it.

With buffer objects implemented and the name on the list, it goes from "breaks on pass 0" to
**358 frames in six virtual seconds**, with the controller detected
(`gamepadmgr.cpp:319 — 1 Joysticks connected`). It still doesn't put geometry on the screen; that
is its next step, and it is another problem.

What the list did **not** gain is equally part of the decision: `point_size_array`, which several
games also look for, stays out because all that exists for it is a `SUCCESS` that does nothing.
Announcing what doesn't exist is what produces exactly the bug above, upside down.

### Buffer objects

`glGenBuffers`, `glBindBuffer`, `glBufferData`, `glBufferSubData`, `glDeleteBuffers`,
`glIsBuffer` and `glGetBufferParameteriv`. The contents stay **on the host**, in a map by name,
and not in the game's memory: that is where a real driver keeps them, and after `glBufferData` the
game has the right to reuse the pointer it passed — keeping our own copy is what makes that right
hold.

Two rules decide where an array comes from, and they are **not the same**:

- For the vertex, color, normal and coordinate arrays, what counts is the `GL_ARRAY_BUFFER` binding
  at the moment of `glVertexPointer`, not at the draw. That is why `ArrayPointer` stores the buffer
  name: a game that uploads three meshes binds each buffer, gives its pointers and only then draws —
  reading the current binding at the draw, all three would come from the last buffer.
- For the `glDrawElements` index list, what counts is the **current** `GL_ELEMENT_ARRAY_BUFFER`
  binding: it decides whether the last argument is a pointer or an offset.

With a buffer bound, the array's "pointer" is no address at all — it is an offset inside the
buffer. Confusing the two has a characteristic and misleading symptom: `glVertexPointer(…, 0)`
becomes a read of address zero, that is, it shows up as an invalid access at `0x00000000` and not
as a GL error. A request that goes past the end of the buffer comes out **zeroed** instead of
failing, because OpenGL leaves the result undefined there and bringing down the whole
`glDrawElements` because of one vertex would be worse — see `buffer_slice` and its tests.

## Texture and buffer names: Rally Master Pro

Rally Master Pro stopped after a few races, on the scene change, with "invalid access at
`0x0101010d`". The stop was deterministic: a windowless script that abandons the stage and reloads
it every 30 s always stopped at the same point, at 194 s, on the fifth or sixth cycle. The address
is `0x01010101 + 0x0c`: a field read from memory covered in `0x01` bytes.

What found the culprit was a temporary guard: 64 bytes filled in after each heap block, and a
write watch on the guards. All the out-of-place writes landed in the same block, a 248-byte array,
and they were `0x01` bytes right after its end. On the stop triggered by the watch, the registers
showed a `glBindTexture(GL_TEXTURE_2D, …)` on its way, and the previous write had been at index
`0xf8`: the game keeps one flag per texture, indexed by its **name**.

The names came from a counter that only grew. The specification allows it: `glGen*` only needs to
hand out names not in use. But each stage deletes and recreates the scene's textures, so the name
went past 248 within a few races, and the flag ended up in the neighboring heap object. With the
guard in the way, the writes landed in it and the game didn't stop, which closed the diagnosis.

Now the names in use live in a `GlNames`, one for textures and another for buffers, and `glGen*`
hands out the **lowest free name**: a deleted name comes out again. `glBind*` of a name never
generated marks it as used, because OpenGL creates the object on the first bind. With this, the
same script ran for 680 s, about 21 cycles, without stopping and without a single invalid access.

**Which name the Zeebo's driver hands out was not measured.** All that is known is that it reuses
them, because the game doesn't stop on the console. The lowest free one is what keeps names low,
and low names are what the 248 array assumes. Mesa, for example, hands out the highest in use plus
one, which also reuses when the whole scene goes away.

The save state stores both sets. A state from before this only has the counter, and then the
names from 1 up to it stay in use, which is what the counter guaranteed. The counter field is still
in the file, with the highest name in use: an old engine that opens a new state keeps counting
from there, without repeating a live name.

## Presentation

`eglSwapBuffers` is what marks a frame. The interface only redraws when the swap counter changes —
or, in a 2D game that never presents through GL, at each real-clock interval.

Redrawing on every pass of the loop drowned the window: in Peteca there are a million and a half
passes for a few dozen frames, and the window never got to be composited.

## What is missing

- **Mipmaps.** Nothing generates or consumes levels beyond the base.
- **Lighting.** No tested game asked for it, but it isn't there.
- **`--dump-gl=DIR`** writes the GL frames one by one; it is the way to check 3D without a window.
