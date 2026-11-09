# 08 — Files and resources

## The formats

| Format | Module | What it is |
|---|---|---|
| `.mod` | `loader/modfile.rs` | the module's ARM executable |
| `.mif` | `loader/miffile.rs` | the Module Information File: the applets' ClassIDs and the icons |
| `.bar` | `loader/resfile.rs` | resources — strings and images the game loads by id |
| `.zip` | `loader/archive.rs` | how titles usually circulate |

### `.mif`

The format is not public. What is in the code was worked out by comparing **24 real files**: the
SDK's (whose ClassIDs we know from the `.bid` files), the simulator's system modules, the
OpenZeebo homebrew and the console titles.

The section index has `n + 1` offsets — each section runs from `offs[i]` to `offs[i+1]`, and the
last value is the file size. The applet record is **20 bytes** and starts with the `AEECLSID`; in
every applet `.mif` examined there is exactly one section of that size.

The rule was validated against values known from another source: `mediaplayer.mif` returns
`0x01010EF6`, the same ClassID that the firmware reverse engineering recorded for the Media Player.

**The icons** live in sections that start with a `u16` length **of the header itself** (always
12) followed by the zero-terminated MIME type — `image/png`, `image/bmp` or `image/jpg`. Reading
that length as if it were only the text's leaves two bytes behind, and no decoder accepts the
image.

Real sizes: 16×16, 26×26 and 65×42 for most, 44×44 in Crash, **192×192** in Resident Evil 4.
**There is no cover art inside the ROMs.**

### `.bar`

Same container as `.mif`. The index entries are `(u16 type, u16 id, u16 a, u16 b)`, where ids
`id..=id+a` live in sections `b..=b+a`. Types: `1` string, `6` image (with an `AEEResBlob`
header), `0x5000` binary.

## The virtual file system

`brew/vfs.rs`. BREW gives each module its own directory, and that is where the game's paths
point:

```
file.dat             /mod/<module>/file.dat
fs:/~/file.dat       the "~" is the module's own directory
fs:/~/../id1/x       /mod/id1/x — Quake keeps its data this way
fs:/~0x01234567/x    a module's directory, by ClassID (today, always the module's own)
fs:/shared/x         shared area
fs:/mod/<folder>/x   an installed module's folder — fan-made ports use it this way
```

**The game never sees a host path.** The guest path first becomes a virtual path, as text only,
and `..` is resolved there: from the module folder it climbs up to `/mod/` and not one step
further; from the device it does not climb. A component with `:` or NUL is refused. Only then
does the virtual path become a host path, through one of two layers:

| Layer | Virtual | Host | Access |
|---|---|---|---|
| package | `/mod/...` | the `mod/` folder of the extracted package | read |
| writable | `/mod/...` | `saves/mod/`, the save store shared by all games | read and write |
| writable | the rest of `/` | `aparelho/`, shared by all games | read and write |

Reads look in the writable layer and then in the package; every write goes to the writable
layer, and a package resource opened for writing is copied first. Remove, rename, `MkDir` and
`RmDir` only touch the writable layer: the package cannot be altered or deleted. A symbolic link
between the layer root and the file makes the path be refused, and the save state only reopens
paths inside those roots.

Quake (`mod/id1/`), Quake 2 (`mod/quake2res/`), Prey Evil (`mod/preyresources/`) and NFS
(`mod/nfsresources/`) climb one level to read folders that come **in the same zip**; no game in
the library climbs two. That is why the neighboring folder is only visible when the module is in
a package's `mod/`. A loose `.mod` sees only its own folder: the one above belongs to the user —
if it were `~/Downloads`, the game could delete whatever was there.

There is one store, not one per content, because that is how the console does it: a game that
looks for another game's save in `fs:/mod/<dir>/` finds it. The price is that two versions of the
same game, with the same folder, share the save. Up to 0.5.0 there was a per-content overlay in
`saves/<hash>/`, and what went to `fs:/mod/` landed in `aparelho/mod/`;
`StoragePaths::migrate_to_shared_saves` copies both into the store when the game opens, without
overwriting, and marks the source with `.migrado-para-saves-mod`.

### The empty slash anchors at the module

After an empty slash — `/` at the start or `//` in the middle —, `..` does not leave the module
folder. The Zeebo Extreme games write the track records to `./udata/trackinfo.txt`, but read them
back through their packer (`ttd_packer.cpp`), which builds `"%s/%s"` with the roots `./` and
empty: `.//../udata/trackinfo.txt` and `/../udata/trackinfo.txt`. Climbing up to the module root,
the reread never found what the game had just written. The `InfoManager` ended up with the track
count and without the track vector, and the Bóia Cross start read the null pointer (`0x6cce0`).
Without the empty slash nothing changes: NFS and Quake 2 still reach `../nfsresources` and
`../quake2res`.

### `fs:/mod/<dir>/` and `fs:/~/` are the same folder

On the console `fs:/mod/<dir>/` is where the module is installed, and fan-made ports open their
own data that way: OpenTyrian asks for `fs:/mod/opentyrian_zeebo/data/tyrian1.lvl`. Both forms
reach the same virtual path, and from there the same layers. The Z-Wheel keeps its own data in
`fs:/mod/274755/`, which now lands in the save store like everything else.

`resolve` refuses the module's own directory; `resolve_dir` accepts it. That is exactly the
difference: listing `fs:/~/` is legitimate, opening it as a file is not.

## Directory enumeration

`IFileMgr::EnumInit`/`EnumNext` unblocked five games. The whole listing comes out at once in
`EnumInit`, and the queue lives **inside the `IFileMgr`** — as in BREW, so that two managers
enumerate different directories at the same time.

The name returned is the **full path**, with the same prefix the game passed: that is what goes
back to `OpenFile` right after, and a bare name would open nothing.

After a false `EnumNext`, `GetLastError` returns `EFAILED` even when everything went fine. The
documentation calls this compatibility with the 1.0 client, and there is a game that checks it.

## `.zip`

The emulator **extracts to a cache** before running, instead of reading from inside the package.
Games write — Peteca has a `.sav` —, and writing back into a zip is not something you want to do.
Extracting solves that for free and keeps the rest of the emulator unaware that the zip exists.

The cache folder is named by the file plus its size and date, so that replacing the ROM
invalidates the old extraction.

When there is more than one `.mod` inside, the one in the console layout wins —
`<Title>/mod/<id>/<name>.mod` — even if it is the deepest: a package that ships the game and some
extra has the game's module exactly there. If that ties, the shortest path wins.

## `IUnzipAStream`

`AEECLSID_UNZIPSTREAM` decompresses a stream. Double Dragon reads its `data.ggz` this way, from
an `IFile` — and an `IFile` is also an `IAStream`. Decompression tries gzip, zlib and raw
deflate, in that order, because what arrives does not announce which it is.

## Game images

`video/icon.rs` decodes PNG (expanding the palette), **paletted 1/4/8-bit and direct 24/32-bit
BMP** and JPEG. The BMP is hand-written because what the `.mif` files carry is the simplest
subset of the format, and a whole dependency would cost more than the few dozen lines.

## BREW's PNG decoder

`AEECLSID_PNGDecoderBREW` (`0x01030766`) is a class separate from `AEECLSID_PNG`: its default
interface is `IImageDecoder`, not `IImage`. The path games use:

```
ISHELL_CreateInstance(AEECLSID_PNGDecoderBREW)   -> IImageDecoder
IIMAGEDECODER_QueryInterface(AEEIID_FORCEFEED)   -> IForceFeed
IFORCEFEED_Write(chunk, n) …                     -> the file, bit by bit
IFORCEFEED_Write(NULL, 0)                        -> end
IIMAGEDECODER_GetBitmap(&bmp)                    -> the finished IBitmap
IIMAGEDECODER_GetRop()                           -> what to draw it with
```

The `IForceFeed` is a **separate** object from the decoder. The two interfaces have different
methods in the same slot — `GetBitmap` and `Write` are both slot 3 —, and returning the same
pointer would make the game call the wrong method.

Decoding happens in `GetBitmap`, not in the end-of-stream write: doing it earlier would only
waste work if the game gave up halfway.

**`AEEIID_FORCEFEED = 0x0101eb0b` is not in the headers we have.** It was identified by use: the
class's `.bid` declares `IForceFeed` among the supported interfaces, that is the interface that
feeds a decoder, and that is the IID Heavy Weapon asks for right after creating the object. The
behavior confirms it — it starts decoding its images.

The format comes from the **file signature**, not the label: a `.mif` calls JPEG `image/jpg`, and
following the label to the letter would leave the icon out because of one letter.
