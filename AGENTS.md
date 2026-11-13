# Guide for agents

This file is for an AI that is going to work on this repository. It says what you can't find out
by reading one file at a time: where things live, what must not break, and how this project
writes.

Humans can read it too — nothing here is a secret of the code.

## What Zeebx is

An emulator for the Zeebo, the console TecToy launched in 2009. The device runs **Qualcomm BREW
4.0.2**, so the emulation is high-level: the game is an ARM binary that never touches hardware, it
talks to the system through tables of function pointers. What has to be faithful is the
conversation, not the silicon. The overall design is in [`ARCHITECTURE.md`](ARCHITECTURE.md), and
each subsystem has its own document in [`docs/implementation/`](docs/implementation/README.md).

## The map

```
Cargo.toml          the `zeebx` library — the whole emulator, with no interface
src/                CPU, the BREW interfaces (`machine/`), video, audio, loader, session
src/ui/             screens and state shared between frontends (see the warning below)
frontends/
  qt-standalone/          the interface: the `zeebx-qt` binary, in Qt Quick
  cli/                    the command line: the `zeebx` binary (`run`, `session`, `bench`...), no Qt
  headless/               no interface, configured by `config.ini`
  libretro/               the RetroArch core
  android/                the app, without a single line of Java
  ios/                    the iOS app: UIKit on top of a static library
  switch/                 the static `.a` that ROMBundler links
  vita/                   the static `.a` that the Vita's RetroArch links
  vita-standalone/        the VPK that opens on its own, without RetroArch
tools/              Python scripts: catalog, scan, core installation
docs/               documentation; `patch-notes/` keeps each version's notes
assets/             icons, fonts and `lang/` — the built-in languages
```

**The host-independent BREW lives in the [`zeebx-core`](https://crates.io/crates/zeebx-core)
crate** ([ZeebxTeam/zeebx-core](https://github.com/ZeebxTeam/zeebx-core)): the `.mod`/`.mif`/`.bar`
parsers, the vtables and trampolines (`brew::aee*`), the guest memory map (`layout`), heap,
objects, VFS, crypto, `SQLMGR` and the save state format. `lib.rs` re-exports it as `crate::brew`
and `crate::save_state`, and the loader as `loader::{modfile, miffile, resfile}` and its constants.
A fix there means publishing a new version of the crate; while developing, point Cargo at a
local clone with `[patch.crates-io] zeebx-core = { path = "../zeebx-core" }`.

**`src/ui/` is not just interface.** `library`, `settings`, `saves`, `zwheel_metadata` and `i18n`
live there but are read **by the core** — `machine`, `loader` and `session` talk to them. Only
`ui::window` (the raw window of `zeebx run --window`), `ui::updater` and `ui::discord` are really
desktop-only; the interface's window lives in `frontends/qt-standalone`.

## The features, and why they exist

| Feature | What it brings | Who uses it |
|---|---|---|
| `desktop` | gilrs, minifb, rfd, Discord, glutin — implies `gpu`, `audio`, `soundfont` | Qt, command line, headless |
| `audio` | only `cpal`, which on Android talks to Oboe | Android, and `desktop` |
| `gl` | drawing on the GPU; **pulls in nothing from the host** | core, Android, and `desktop` |
| `gpu` | creating its own context (glutin) — implies `gl` | desktop |
| `soundfont` | MIDI through a sample bank (rustysynth, pure Rust) | core and desktop |
| `achievements` | RetroAchievements: rcheevos (submodule `rcheevos-sys/rcheevos`, `git submodule update --init`) and `ureq` | Qt, with `--features achievements`; **off in releases** until RA accepts the console |

**House rule: the Libretro core links no host library.** No window, no sound card, no
controller. The frontend delivers video, audio and input. `libretro.yml` enforces this with
`ldd`, and a new `dep:` in the wrong place breaks the core's build.

Before putting a `#[cfg(feature = "desktop")]` on something, ask whether it is **really**
desktop-only. Three wrong gates have already broken the Android build without anyone noticing,
because the APK is only built on the tag.

## Building and proving

The interface is the Qt frontend (`zeebx-standalone-qt`, binary `zeebx-qt`), and it is what ships
in the installers. The command line (`zeebx-cli`, binary `zeebx`) is the bench for measuring and
debugging, and doesn't need Qt. The egui interface was removed after 0.4.2. Qt requires **Qt 6**
(6.4 or newer), which the build finds through `qmake6`, `qmake` or `QMAKE`.
`python3 tools/check_build_deps.py` says what is missing. The Qt interface is described in
[`docs/implementation/21-qt-migration.md`](docs/implementation/21-qt-migration.md).

```bash
cargo build --release --locked -p zeebx-cli                 # the `zeebx` binary
cargo build --release --locked -p zeebx-standalone-qt       # the `zeebx-qt` binary
cargo test  --release --locked -p zeebx -p zeebx-cli -p zeebx-standalone-qt
timeout 60 ./target/release/zeebx controls                  # really starts up, no screen

cargo build --release --locked -p zeebx-headless
cargo build --release --locked -p zeebx-libretro            # on its own: see below
python3 tools/verify_core.py target/release/libzeebx_libretro.so
```

**The Libretro core builds in a command of its own.** Together with another package, Cargo
unifies the core features both ask for, and the core inherits the other one's `audio`: code that
only compiles with sound passes the joint command and breaks in `libretro.yml`, which builds it
alone. It already happened with `set_audio` in `ui::running_game`.

`verify_core.py` opens the library the way the frontend does (`dlopen`) and calls the ABI. It is
not `nm` over a list of symbols: it catches a dependency missing from the link, which reading
symbols doesn't.

**QML errors only show up at runtime**, and on a Linux desktop Qt sends its messages to journald
when stderr is not a terminal. To see them in a terminal or a log file, run with
`QT_FORCE_STDERR_LOGGING=1`, e.g.
`QT_QPA_PLATFORM=offscreen QT_FORCE_STDERR_LOGGING=1 timeout 12 ./target/release/zeebx-qt`.
The build's `qmlcachegen` catches some mistakes, not all.

**Android** needs the toolchain in `$HOME`, without sudo — the table is in
[`frontends/android/README.md`](frontends/android/README.md):

```bash
export PATH="$HOME/.cargo/bin:$HOME/Android/gradle/bin:$PATH"
export JAVA_HOME="$HOME/Android/jdk"
./frontends/android/build.sh --apk
```

**Switch** (the static `.a` that ROMBundler links) lives in
[`frontends/switch/build.sh`](frontends/switch/build.sh). That script's `cfg(zeebx_switch)` is
what shrinks dynarmic's code cache and raises the number of cached sounds. On a Mac the
`rombundler-switch` Docker image brings devkitA64 and rustc; the `core-switch` job in
`libretro.yml` runs the same script with `--local` inside the `devkitpro/devkita64` image. The
release does the same and publishes `zeebx_libretro-switch.zip`. The `.a` doesn't link on its
own: the `switch_jit_*` and `switch_spinlock_*` that the dynarmic patch calls come from ROMBundler.

**Vita** (the static `.a` that RetroArch links) lives in
[`frontends/vita/build.sh`](frontends/vita/build.sh). The target is
`armv7-sony-vita-newlibeabihf`, tier 3: std is rebuilt with `-Z build-std` and vitasdk provides
`arm-vita-eabi-gcc`. Dynarmic is left out — it doesn't emit ARMv7 —, so the core is the
interpreter, and drawing stays on the CPU because vitaGL doesn't provide OpenGL ES 3. On a Mac the
`vitasdk/vitasdk` Docker image brings the SDK; the `core-vita` job in `libretro.yml` runs the same
script with `--local` inside that image. The release does the same and publishes
`zeebx_libretro-vita.zip`. The `.a` doesn't open on its own: RetroArch consumes it renamed to
`libretro_vita.a`.

The app, which doesn't go through RetroArch, lives in
[`frontends/vita-standalone`](frontends/vita-standalone/README.md). It is a VPK: same target and
same Docker image, another binary. `vita.yml` is `workflow_dispatch`, like the APK; the release
publishes `zeebx-vita.zip`.

**Memory on the Vita is tight and has not been measured on the device.** The game heap there is
48 MiB (64 everywhere else), and the internal region adds 16 MiB more that newlib really
reserves. A new or larger region in `loader/mod.rs` comes out of that same budget. A heap of a
different size also means a Vita save state doesn't open on the other platforms, and vice versa.
See [`frontends/vita-standalone/README.md`](frontends/vita-standalone/README.md).

**iOS** needs a Mac with Xcode and the `aarch64-apple-ios` and `aarch64-apple-ios-sim` targets
in rustup. The step-by-step is in [`frontends/ios/README.md`](frontends/ios/README.md):

```bash
./frontends/ios/build.sh --app
```

## CI

**The tag is the only automatic trigger.** `release.yml` fires on `v0.0.0` and builds the release
as a draft. `ci.yml`, `libretro.yml`, `headless.yml`, `android.yml`, `ios.yml`, `vita.yml` and
`qt.yml` are `workflow_dispatch`: the default CI builds the command line — and with it the core
and the tests — on the six platforms and calls `qt.yml`, which builds the interface and the
installers. The release calls the same `qt.yml`: the Qt steps live in one place. Eleven jobs per
run is too expensive to spend on every push, and whoever asks decides. The exceptions are
`discord-issues.yml`, which builds nothing: it posts to Discord when an issue opens, closes or
changes assignee; and `sentinel.yml`, which watches the lockfile.

If you changed something only one of them covers — the APK, the core on an ARM target —, tell the
human it is worth dispatching that workflow before the tag. You can't dispatch it.

## What must not break

- **The binary names don't change.** The interface is `zeebx-qt` — it is what the shortcut, the
  `.desktop` file and the updater of anyone who already has it installed point to —, and the
  command line is `zeebx`, which scripts, CI and the documentation type. The package may move;
  what the user types may not.
- **What users and scripts already type keeps working.** The `zeebx` subcommands and options and
  the `ZEEBX_*` environment variables had Portuguese names before the codebase moved to English.
  The old spellings are hidden aliases: `LEGACY_ARGS` in `frontends/cli/src/main.rs` and
  `LEGACY_ENV` in `src/config.rs` (read through `zeebx::config::env_var`). Don't remove an entry;
  add one when you rename an option or a variable.
- **Stored names are frozen, even when they are Portuguese.** Renaming a Rust field or variant
  must not change what is on disk: `settings.json`, `catalog.json` and the other serde types keep
  their old keys through `#[serde(rename = "...")]` (e.g. `"tema"`, `"atalhos"`,
  `"direcional_nos_eixos"`); theme IDs (`escuro`, `claro`...); the folders under the config
  directory (`aparelho/`, `temas/`, `relatorios/`, `saves/mod`...); the paths the guest sees
  (`/perfil/aparelho`, `/perfil/saves/mod`); save-state section names (`gl.matrizes`,
  `relogio.us`...); libretro core option keys **and values** (`zeebx_perfil`, `desligado`...),
  which RetroArch stores in each user's `.opt`; the log targets (`registro`, `controles`,
  `midia`...), which every log line carries and which scripts grep; the `atualizador` Cargo
  feature. Translate the comment next
  to such a name, never the name.
- **Names that cross into QML need an explicit `cxx_name`.** cxx-qt 0.10 doesn't convert names:
  a bridge property, invokable or signal without `cxx_name` reaches QML with its Rust spelling.
  Multi-word names get `#[cxx_name = "camelCase"]`, and QML calls that name. Model role names,
  `image://zeebx/...` URLs and the engine's initial properties are strings on the Rust side and
  identifiers on the QML side: rename both together.
- **The core's `.so` and `zeebx_libretro.info` go together.** An old `.info` next to a new core
  makes RetroArch's scan mark everything `??`. And the `.info` capability fields must match what
  the ABI does.
- **Zeebx's code is GPL-2.0-or-later.** The default CPU backend is Dynarmic, so that GPLv3
  frontends such as Qt 6 can link the core without carrying a GPLv2-only dependency. Before
  proposing a new library, check its license and the features of the binary that will link it.
  **The binaries are already GPLv3 in practice**: they all link crates that are Apache-2.0 only,
  which doesn't mix with GPLv2. That's why a GPL-2.0-only dependency doesn't go into any binary.
- **`PUBLIC_KEY` in `src/ui/updater.rs` and the GitHub secret `ZEEBX_ATUALIZADOR_CHAVE` go
  together.** The secret signs the release packages; the embedded key is what the emulator
  accepts. Changing one without the other makes every automatic update be refused — and anyone
  who already has the old version installed only gets out of it by downloading by hand.
- **`THIRD-PARTY-NOTICES.txt` follows `Cargo.lock`.** Changed a dependency: run
  `python3 tools/third_party_notices.py` and commit them together; the release checks it.
- **The guest heap belongs to the game only.** Memory the emulator asks for itself in guest space
  comes from `alloc_internal` (region `0x3400_0000`), never from `malloc` or `heap.alloc`. A block
  of ours in the heap moves the address of everything the game allocates afterwards, and
  RetroAchievements reads the heap by address. The bases and sizes in `loader/mod.rs` are the same
  contract.
- **`Cargo.lock` is versioned** and CI uses `--locked`. A new workspace member goes into the lock,
  in the same commit.

## How this project writes

**English in the code and in the documentation.** Identifiers, comments, messages, commit messages
and docs are in English. The user-facing text of the interfaces goes through `assets/lang/`
(`pt-BR`, `en`, `es`, `es-MX`); a new UI string goes into the catalog, not into the code.

**The Portuguese originals are a frozen record.** `README.pt-BR.md` and `docs/pt-BR/` keep the
documentation as it was in v0.5.1, when the codebase moved to English. They are not updated:
change the English version only.

**A comment explains why, not what.** The code already says what it does. What gets lost is the
reason: the measurement that killed the hypothesis, the defect the line prevents, the path that
was tried and didn't work. There are long comments here, and they are like that on purpose.

**A claim comes with a measurement.** "34× faster", "from 79% to 284%", "1214 frames per virtual
millisecond". If you didn't measure, say you didn't measure — don't round it up to an impression.

**Commit messages follow Conventional Commits, on a single line, in English**:
`type(scope): what changed`, e.g. `fix(gl): rotate normals by the inverse transpose of the
modelview`. The types are the usual ones (`feat`, `fix`, `refactor`, `docs`, `build`, `ci`,
`test`, `perf`, `chore`); the scope is the subsystem.

## The README's contributor list

The README's contributor table is written, not fetched: the README runs nothing. It comes from
`python3 tools/contributors.py`, which reads the same API as the site (`docs/site.js`) and
rewrites the section between the `<!-- contributors -->` markers. Redo it when someone new joins.

## Release notes

Each version leaves its own in [`docs/patch-notes/`](docs/patch-notes/), written for the people
who use the emulator: what was fixed, in a player's language, not a programmer's.

## If you're not sure

This repository prefers a question to a well-written guess. If the reading doesn't add up — if you
can't tell whether a controller sends the d-pad as keys or as an axis, for example —, say which
instrument would settle it instead of picking the most likely path.
