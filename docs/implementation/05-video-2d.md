# 05 — 2D video

## The screen

640×480, RGB565 — the Zeebo's video output. `video/display.rs` has the `Framebuffer` (pixels in
`u16`) and the operations: `set_pixel_native`, `fill_rect`, `draw_frame`, `draw_line` (Bresenham,
integers only, like the hardware of the time) and `blit`.

The game's `IBitmap` is an object of ours with an associated `Framebuffer`. The screen's is the
**device bitmap**, created the first time someone asks for `IDISPLAY_GetDeviceBitmap`.

## The DIB: where everything gets expensive

BREW lets the game ask for **direct access to the pixels**, via `QueryInterface(AEECLSID_DIB)`.
From then on the game has a pointer to the pixel memory and writes to it without going through us.
Commercial games do this all the time.

But `QueryInterface` is a permission, not a prerequisite. On the console a software `IBitmap`
**is** an `IDIB`: the struct starts with the `IBitmap` vtable and continues with public fields —
size, pitch, depth, pointer to the pixels — and nothing stops the game from reading them directly.
Peggle does exactly that with the bitmap that comes out of the PNG decoder, and that is why every
decoded bitmap already comes out with these fields filled in.

While they came out zeroed, the game read `-size 0/0` (the phrase is from its log) and built each
sprite as a square with zero side: **76,618 of the 77,208 triangles of a frame** were discarded for
zero area, and the screen stayed black with the game drawing all the time. It was the counter of
rejected triangles, by reason, that pointed to this — no API call had failed.

Only the decoder path publishes the pixels without being asked. In the others, `QueryInterface`
is still the moment to allocate, and every published surface enters the synchronization loop
below.

### The address gets reused, and the header has to keep up

Publishing the `IDIB` was done **once per object address** — and object addresses are recycled:
once the previous one is freed, the next bitmap is born in the same place, with another size.

Tekken 2 decodes nine images in sequence, freeing each one before the next; all of them were born
at the same address. The `IDIB` announced for all nine the size of the first, 200×112. The game
then created a 200×112 page for a 360×280 letter sheet, kept only its corner, and then asked for
each glyph by coordinates of the whole sheet: whatever fell outside became a block filled with the
texture. The whole menu came out with the words as orange rectangles.

Today the header is rewritten on each exposure, with the size and pitch of the image that is
there. The buffer is reused when it fits, which saves a buffer swap for each image.

**The surfaces region uses the same allocator as the game's heap.** At first it didn't recycle,
and Z-Wheel exhausted it in 27 s of browsing. Then it started recycling whole blocks — the smallest
free one that fit —, without splitting or merging neighbors: with varied sizes, the small holes
didn't serve large requests and the 8 MB ran out just the same, only more slowly, with the bitmap
being born without pixels. With the `Heap`, returned blocks merge and a large block serves a small
request. The EGL color buffer also goes through it and returns the old one when it grows.

The lesson is the same as other times: **what a game reads from a struct of ours counts as much as
what we return from a call.** Here no call failed, and the report came out clean.

**And the capacity annotation has to die along with the buffer.** When drawing on a game surface
we build an `IBitmap` of ours as the source and discard it right after. The discard removed the
buffer from the annotation, but left the capacity behind; the next bitmap to be born at that
address read the leftover, concluded that its buffer already fit and published an `IDIB` with a
**null** `pBmp`. Whoever read the pixels through there got zero: Bejeweled Twist's `BltIn` builds
the row table from `pBmp`. Today the temporary source is returned by `release_dib`, which lets go
of both things.

**The reused buffer still has the dead one's pixels.** Rewriting only the header leaves the
previous object's image in the buffer, and the new bitmap only has the right pixels on our side.
On Unicorn this didn't show up: the write watch said "the game didn't touch this", the import
didn't happen and our copy prevailed. Dynarmic has no watch and always answers "dirty" — which is
only safe if the buffer is never behind the host. Here it was, and the import brought the previous
image on top of the glyph sheet: in the window's engine, Tekken 2's letters came out as blocks
again, now dark ones.

That is why a new object at an address that already has a buffer goes into `inherited_dibs`:
while it is there, the import is skipped and the first exposure publishes the host's pixels, even
when it fits. The rule is one of lifecycle — **the buffer belongs to the object**, and bytes of an
object that died are not the game's to be brought back.

### Only the rectangle that changed goes back to the game

Publishing the whole surface after each draw was the emulator's biggest cost in sprite games:
Pac-Mania draws 160 thousand sprites in five virtual seconds, and each `IIMAGE_Draw` rewrote the
surface's 600 KB — 15 of the 16 seconds of API. The `Framebuffer` now keeps the dirty box since
the last publication and a serial number; while the serial is the same one the game already
received, only the box is written (146 ms in the same stretch). The import resets the box,
because after it both sides are equal.

A diff of the API calls between the two engines is what told this apart from a CPU bug: 2.2
million identical calls (342 different lines, all of them timer delays). If the game asks for the
same thing and the frame comes out different, what differs is our side.

This forces two sides to be kept up to date:

```
  our Framebuffer  ◄── sync_from_guest ──  buffer in the guest's memory
                   ── sync_to_guest ────►
```

And there is the trap: **doing this around every call costs 1.2 MB round trip.**

### What Pac-Mania showed

The game draws its own images **pixel by pixel** through the API — 1.16 million `DrawPixel` in ten
frames — and queries the clip 112 thousand times every fifteen seconds. Charging the whole surface
for each of these calls:

| | real time |
|---|---|
| 110 frames, copy per call | **232 s** — of which 1.8 s of emulated CPU and 161 s of copying |
| after handling the pixel in place | 3.4 s |
| after skipping the copy on state settings | 20 virtual s in 26 real s |

Two changes, same 59 million instructions.

### The RLE-compressed BMP

The BMP decoder rejected anything that came compressed, and that rejection cost a game: **Disney
All Star Cards** stores almost all of its images in `BI_RLE8` and `BI_RLE4`. Without them the game
showed the map with an empty rectangle in place of the art and, soon after, had nothing left to do
— the session ended by itself.

The scheme is Windows': a `(count, value)` pair repeats the value; a zero count opens an escape —
`0` end of line, `1` end of image, `2` a `(dx, dy)` jump and, from `3` up, a literal run, always
ending on a word boundary. In `RLE4` each byte carries two pixels, alternating the high and low
nibble. Whatever the image doesn't cover stays at palette index zero, as Windows does.

### The clip is not a finishing touch

`IIMAGE_Draw` went through the whole image and checked pixel by pixel, without looking at the clip.
That is wrong twice.

Pac-Mania draws the **whole font sheet** and tightens the clip so that only the letter shows — that
is its text drawing. Without the clip in the loop, a letter cost the sheet's 193 thousand pixels:
20 thousand `Draw` calls read **3.9 billion pixels** to put 315 thousand on the screen. And, since
our pixel write doesn't know about the clip, what the game told us to hide went to the screen too.

Today the clip goes into the computation of the loop's bounds, before reading any pixel. Five
virtual seconds of Pac-Mania went from **174 to 62 seconds** of wall clock, and the language
selection screen shows up whole and correct.

The rule that stays: **in a drawing loop, the clip decides the size of the work, not the look of
the result.** Applying it per pixel, at the end, is paying for everything you'll throw away.

### How it is solved

`touches_whole_surface(name)` decides whether the call pays for the copy.

- `DrawPixel` and `GetPixel` deal with **their** pixel directly in the game's buffer — two bytes,
  not the surface.
- The state settings (`SetClipRect`, `SetColor`, `SetFont`, the `Get`/`Set` pairs of `IGraphics`)
  touch no pixel at all and pay nothing.
- Everything else pays.

The list is an **exclusion** list, and that is deliberate: forgetting a method that draws there
would give a wrong pixel, which is hard to notice; leaving out one that doesn't draw only costs the
copy, which shows up in the measurement. **When in doubt, copy.**

### The compatible bitmap is already an `IDIB`, and Zenonia presents through `ITransform`

Zenonia stayed with a black screen while drawing all the time, and it was two chained bugs.

**The canvas was born without a size.** The game creates a 320x240 canvas with
`CreateCompatibleBitmap` and reads `cx`, `cy` and `pBmp` directly from the struct, without
`QueryInterface` — in BREW the compatible bitmap is already an `IDIB`. Ours only published these
fields when someone asked for the interface, so the canvas was 0x0 for the game. Without `pBmp`,
its engine fell into the slow path, pixel by pixel through `SetPixels` (197 thousand calls in
thirty seconds). Now `CreateCompatibleBitmap` already exposes the DIB.

**The frame went to the screen through an interface that didn't exist.** To present, the game asks
the screen bitmap for IID `0x01001029` and calls slot 4 of the object with `(x, y, pSrc, xSrc,
ySrc, dx, dy, pMatrix, nComposite)` — `ITransform::TransformBltComplex`. That IID was treated as
"the third IID of `IDIB`", for being a neighbor of `AEEIID_DIB_20`, and was answered with the
bitmap itself: slot 4 fell into `NativeToRGB`.

The matrix is `{A, B, C, D}` in 8.8 applied around the center of the source rectangle, with
`(x, y)` at the corner it would have without transformation. Zenonia passes `x = 160`, `y = 120`,
a 320x240 canvas and scale 1.9: the result, 608x456, is centered on the 640x480 screen. Sampling is
nearest-pixel, going through the destination so as not to leave holes.

### Action Hero 3D: `TransformBltSimple`, the magenta and the `IImage` piece

The game stopped at the first presentation asking for `TransformBltSimple`, and once past it, the
menu came out covered with whole font sheets on a magenta background. It was three things.

**`TransformBltSimple`.** Same arguments as `Complex` up to `dy`, and instead of the matrix a
`uint16 unTransform`: rotation in quarter turns in bits 0-1, X mirror in bit 2 and scale from bit 3
up. The game passes `unTransform = 8` with the same `x = 160`, `y = 120` and 320x240 canvas as
Zenonia — the doubled frame centered on the screen —, so `8` is 2x scale. The matrix comes from the
flags and the drawing is `Complex`'s. Scales other than 1x and 2x haven't been seen yet and are
noted in the assumptions.

**The default transparent color is magenta.** The game draws the ready-made words from a
magenta-background atlas with `BitBlt(..., AEE_RO_TRANSPARENT)` without ever calling
`SetTransparencyColor`. It is BREW's `RGB_MASK_COLOR`. A bitmap with no requested color now uses
`0xF81F` in the transparent blit and announces this value in the `IDIB`'s `ncTransparent`.
`FillRect` still compares with zero, which is what Bejeweled Twist needs.

**`IPARM_SIZE`, `IPARM_OFFSET` and `IPARM_ROP`.** The dynamic text comes from an `IImage` over the
201x37 BMP of `font.bar`, three rows of 12 pixels. Each letter is `SetParm(0, cx, 12)`,
`SetParm(1, x, 0|12|24)` and `Draw(x, y)`, and the image is prepared with `SetParm(3, 7)`. The
numbers came from that usage — `SIZE` 0, `OFFSET` 1, `ROP` 3, neighbors of `CXFRAME` 2 and
`NFRAMES` 4 that we already knew. Ignored, each letter put the whole sheet on the screen; now
`Draw` reads only the piece, and with the transparent `ROP` it skips the magenta.

**The `IDIB` row pitch is aligned to 4 bytes.** After that the title screen logo and the story
portraits still came out cut. The game copies these images pixel by pixel through `pBmp`, and its
loop moves from one row to the next by `cx + 1` pixels when `cx` is odd (`tst cx, #1` before the
loop). The logo is 149 wide and the portraits 179, and our buffer stored the rows packed together,
with `cx * 2` bytes: each row read slipped one pixel. Now `nPitch` and the buffer use the width in
bytes rounded up to a multiple of 4, with one padding pixel at the end of an odd-width row. At even
width the pitch is still `cx * 2`, so the games that already worked read the same buffer as before.

## The clip

`IDISPLAY_SetClipRect` was accepted and ignored, and that hid a big bug.

Pac-Mania draws the **whole font sheet** with `BitBlt` and counts on the clip so that only the
letter shows. Without the clip, the whole sheet went to the screen for each letter — the game's
opening became a mosaic of overlapping alphabets.

The cut moves the **origin in the source** along with it:

```rust
clip_blit(clip, dest, size, origin) -> Option<(dest, size, origin)>
```

Shrinking only the destination rectangle would show the wrong corner of the image — that is how a
font atlas becomes swapped letters. There is a test on exactly this property.

`None` means "nothing was left": the blit doesn't happen.

### No clip is the whole screen, not "nothing gets through"

`clip_blit` always knew this — with no clip, the blit goes through whole. `clip_rect`, next to it,
did the opposite: a `let clip = clip?;` on the first line, and every rectangle from whoever does
**not** set a clip was discarded.

The effect is silent and big: `IDISPLAY_DrawRect` never drew anything in any game that didn't call
`SetClipRect` first. It took a while to show up because almost all 2D drawing is blits, and blits
took the right path. And there was a test asserting the mistake — `clip_rect(None, r) == None`. A
wrong test protects a bug, and this one did.

## Clearing the screen is a `DrawRect`

There is no `IDISPLAY_ClearScreen` in the vtable: in the SDK it is a macro that calls

```c
IDISPLAY_DrawRect(p, NULL, RGB_NONE, RGB_NONE, IDF_RECT_FILL)
```

Three readings have to be right at the same time, and all three were wrong here:

| What arrives | What it means | What we did |
|---|---|---|
| `pRect == NULL` | the whole surface | nothing to draw |
| `RGB_NONE` | **the current color** from `SetColor` | "don't paint" |
| `flags` | what controls frame and fill | ignored |

Tekken 2 clears the screen this way **once per frame**: `SetColor(CLR_USER_BACKGROUND, black)` and
`ClearScreen`. While the clearing didn't happen, its menu was drawn on top of what was already on
the screen — the opening's "APERTE ❶ PARA COMEÇAR" kept showing under "MODO VERSUS". The `flags`
aren't a detail either: Quake asks for a frame alone (`IDF_RECT_FRAME`) passing black as the fill,
and honoring the color while ignoring the flag put a black rectangle it didn't ask for.

No flag at all (`flags == 0`) keeps the old behavior, drawing both. No game in the collection calls
it like that, and when in doubt it is better to keep drawing than to erase a screen because of a
reading that couldn't be checked.

## Blit

`Framebuffer::blit` copies with origin, size and an optional transparent color. The transparency
comes from `IBITMAP_SetTransparencyColor`, stored per bitmap.

`IBitmap::BltIn` and `BltOut` swap the roles of source and destination; `IDISPLAY_BitBlt` draws on
the display's current destination, which `SetDestination` may have changed.

## Text

`IDISPLAY_DrawText` writes with a TrueType font, chosen in this order:

1. **The game's**, when the package carries a `.ttf` — Z-Wheel carries `tectoy.ttf`.
2. **The device's**, at `fs:/shared/fonts/tectoy.ttf`, which is where the firmware looks for it.

The second doesn't come with the emulator, because it belongs to TecToy; it comes in the Z-Wheel
package, which is the console's system. When the device doesn't have it yet and Z-Wheel has been
opened at some point, it is copied from the cache to there (`archive::system_font`). With no font
at all, the text still goes only to the report.

Most games don't carry a font because on the console they didn't need to. Kingdom Hearts asks for
`0x8000`, BREW's `AEE_FONT_NORMAL`, and without the device's font it drew the five menu boxes and
none of the words — "New Game", "Load Game" and the others only showed up in the report.

`GetFontMetrics` and `MeasureTextEx` return numbers that are consistent with each other. Without
any, Pac-Mania doesn't even build the screen.
