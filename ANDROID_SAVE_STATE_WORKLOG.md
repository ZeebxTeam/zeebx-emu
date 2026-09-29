# Android save-state work log

## 2026-09-23

### Goal

Expose reliable persistent save-state slots in the Zeebx Android frontend.

### Important upstream change discovered

While implementing a standalone serializer on top of the v0.2.1-era Android branch, Zeebx v0.3.0 was released.

v0.3.0 already contains a complete core save-state implementation:

- versioned `ZBXS` format with named sections;
- CRC32 integrity checking;
- module/content compatibility checks before state is applied;
- CPU registers, memory, heaps, objects, timers, callbacks and input queues;
- files/SQLite state and BREW object tables;
- software GL state and hardware-GL mirror reconstruction;
- textures, depth/stencil and drawing state;
- Libretro serialize/unserialize integration and round-trip tests.

Release v0.3.0 also merged our previous Android PR #20 (input/audio/performance fixes).

Because the upstream implementation is substantially more complete and better tested than the duplicate serializer being written locally, the duplicate implementation was stopped rather than creating a second save-state format.

### Preserved old WIP

The pre-v0.3.0 attempt was committed and preserved on:

`wip/save-states-pre-0.3.0`

Checkpoint commit:

`e96b012 Guarda a tentativa de save state anterior à 0.3.0`

It should not be merged. It exists only as a durable record of the earlier work.

### Current branch

`feature/android-save-slots`

Base:

`v0.3.0` / `6035a25`

### Current design

Do not serialize Machine state in the Android frontend.

Use the v0.3.0 public Session API directly:

- `Session::pode_salvar()`
- `Session::grava_estado()`
- `Session::restaura_estado()`

Android adds only:

1. five persistent slots;
2. atomic on-disk writes of the exact upstream ZBXS bytes;
3. per-content directories using the same BLAKE3 ContentId rule as Zeebx storage;
4. an optional 160x120 RGB565 thumbnail sidecar;
5. pause-menu UI for Save / Load / overwrite confirmation.

The state file is authoritative. A missing/corrupt thumbnail never makes a valid state unloadable.

### Storage

Android private application root:

`<internal>/states/<BLAKE3 content id>/slot-N.zbxstate`

Optional thumbnail:

`<internal>/states/<BLAKE3 content id>/slot-N.rgb565`

The ContentId hashes the original frontend-selected file (.zip/.7z/.mod), so different revisions of the same title do not share states accidentally.

State reads are capped at 128 MiB to avoid allocating an absurd size from a damaged/unexpected file.

Writes use tmp + fsync + backup/rename so overwriting a slot cannot destroy the previous valid state if the final rename fails.

### Android UI

The existing Back/pause dialog gains a Save states section with five slots.

For each slot:

- 96x72 thumbnail when available;
- slot number;
- empty/occupied + state size;
- Save or Overwrite button;
- Load button only for occupied slots;
- occupied slots require a second confirmation press before overwrite.

The list is scrollable with a 170-point maximum height. This was chosen from the Android frontend's own scale guarantee: a 960x544 handheld has roughly 340 egui points vertically, so a 300-point slot list would push the bottom controls off-screen.

All controls are ordinary egui buttons so the controller-focus behavior added in v0.3.0 applies automatically.

Translations added for:

- pt-BR
- en
- es
- es-MX

### Input after loading

After a successful Load, the Android physical Pad state is reset to neutral and immediately sent to the Session.

Reason: the button used to choose Load belongs to the frontend UI, not to the emulated instant. Without clearing it, that UI button/direction could leak into the first restored guest frame.

The Android rendering caches (`textura`, `quadro`, `quadro_565`) are invalidated after Load so the restored frame is uploaded again.

### Validation completed so far

- All four modified language JSON files parse successfully.
- `git diff --check`: clean.
- Android ARM64 cross-check completed successfully:

```
cargo ndk -t arm64-v8a --platform 35 check -p zeebx-android
```

Result:

`Finished dev profile ... target(s) in 7m 36s`

This proves the current Android slot/frontend code compiles against v0.3.0 and does not introduce an unsupported host dependency.

### Formatting note

A global `cargo fmt --check` on v0.3.0 reports many pre-existing formatting differences across upstream files with the locally installed rustfmt. Do not mass-format the repository as part of this feature. Only new/touched code should be kept readable and minimal.

### Remaining work

1. Run the upstream core save-state tests on v0.3.0.
2. Review Android slot persistence edge cases (existing thumbnail overwrite, state corruption, state-load GL context).
3. Build the actual ARM64 APK.
4. Verify APK signature/package.
5. If a suitable ROM is available, run a real save -> advance -> load -> continue test on the Android build/device.
6. Update this log with final APK path/hash and any known limitation.

### Core save-state validation completed

Full v0.3.0 core library suite with the Android-relevant feature set:

```
cargo test --release --locked -p zeebx --no-default-features --features gl,audio --lib
```

Result:

- 492 tests discovered
- 490 passed
- 0 failed
- 2 ignored

This run includes the upstream `machine::save::*`, `save_state::*`, GL-state round-trip, input/timer/file/database state, module-mismatch rejection, corruption/truncation checks, and the previously merged Android audio regression tests.

The Android ARM64 cross-check was rerun after switching thumbnails to `Session::quadro_grande()` for hardware-3D states and still passes.

### Final validation and artifact

Final core suite **after** the Session clock regression test was added:

```
cargo test --release --locked -p zeebx --no-default-features --features gl,audio --lib
```

Result:

- 493 tests discovered
- 491 passed
- 0 failed
- 2 ignored

This final run includes `session::tests::restaurar_estado_reancora_o_relogio_real_da_sessao`.

A standalone/Android pacing gap was found during review: upstream `Session::restaura_estado()` restored the console clock but retained the old host-time anchors (`started`, `clock_base`, speed-sampling window). This can make a state restored to an older virtual instant look artificially late, or a newer one look ahead and return `Step::Ahead`.

Fix added in `src/session.rs` after a successful machine restore:

- re-anchor `started` to `Instant::now()`;
- set `clock_base` to the restored virtual clock;
- restart the speed/FPS/IPS measurement window from the restored counters;
- clear old performance history/sample;
- clear host-only intermediate framebuffer and stopped flag.

Focused regression test:

`session::tests::restaurar_estado_reancora_o_relogio_real_da_sessao`

Result: 1 passed, 0 failed.

Full core suite after the runtime fix (before adding only the test body itself):

- 492 discovered
- 490 passed
- 0 failed
- 2 ignored

The focused test adds one additional test to the source tree and passes independently.

The Android slot layer also now recovers a `.zbxstate.bak` automatically if Android/app death happened after the previous state was moved aside but before the new state was published.

Final ARM64 native build succeeded with v0.3.0 plus this feature. A clean Gradle `clean assembleDebug` succeeded and the APK was inspected: it contains all required libraries:

- `lib/arm64-v8a/libzeebx_android.so`
- `lib/arm64-v8a/libc++_shared.so`
- `lib/arm64-v8a/libsevenz_rust2-4ea715df3fadb002.so`

APK signature verification:

- APK Signature Scheme v2: PASS
- package: `io.github.zeebxteam.zeebx`
- version: `0.3.0` (`versionCode=2`)

Final APK:

`Bakin/zeebx/zeebx-android-arm64-v8a-v0.3.0-save-states.apk`

Size: 17,091,784 bytes

SHA-256:

`539A16A7B33E91E55FA59B094F0121085AE3C294FDC606F05950FC9115E3A1CA`

### Remaining validation gap

`adb devices -l` reports no connected Android device, and there is no `.zip`, `.7z` or `.mod` package at the top level of `Bakin/zeebx` to drive a real title locally. Therefore the final physical interaction sequence (save -> advance game -> load -> continue) cannot be executed from this machine in this run.

Everything below that device/ROM layer is validated: upstream save-state core tests, the new Session host-clock regression, Android ARM64 cross-compilation, release native link, clean Gradle package, required native libraries, package metadata and APK signature.
