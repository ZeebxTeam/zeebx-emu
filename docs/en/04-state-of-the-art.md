# 04 — State of the art

## Infuse (Tuxality) — the only functional emulator

- Repository: <https://github.com/Tuxality/Infuse> — **the repository is empty** (only `LICENSE`, `README.md`, and `.gitignore`; 1 KB, last push 2024-03-31). The code is **not public**; GitHub serves as a showcase and issue tracker. GitHub lists its license as “Other (NOASSERTION)”.
- Site/blog: <https://tuxality.net/projects/infuse_zeebo_emulator>
- Architecture as described by its author:
  - **HLE**: BREW subsystem reimplemented from scratch through clean-room reverse engineering;
  - CPU: **[dynarmic](https://github.com/merryhime/dynarmic)** (ARM JIT);
  - Graphics: **OpenGL ES 1.x**;
  - Audio: basic MIDI, PCM/ADPCM, MP3, and multiple-stream mixing with host-independent resampling.

The research and community landscape is documented in the Portuguese source at [`../04-estado-da-arte.md`](../04-estado-da-arte.md).
