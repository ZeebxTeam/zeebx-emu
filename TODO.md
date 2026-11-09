# Zeebx - next steps

## CPU diagnostics on Dynarmic

The options below depended on Unicorn's instruction and memory hooks. They were removed from the
command line together with the old backend, instead of staying accepted without producing data.
They need to be redesigned on top of Dynarmic's instrumentation and block invalidation APIs:

- **`--code=0xSTART:0xEND`** logged every instruction executed inside a range, along with `r0`
  and `lr`. It served to turn the disassembly, which shows every possible path, into the path the
  game actually took.
- **`--watch=0xADDR`** watched reads and writes around an address and logged the PC, the `lr`,
  the size and the value of each access. It served to find out who initialized, changed or just
  read a corrupted or unexpectedly null field. Not to be confused with `watch_dirty`, which stays
  in the backend for the internal invalidation of the renderer's surfaces.
- **`--profile`** counted the most executed ARM blocks and measured the time spent in the BREW
  API methods. It served to separate the game's hot loops from bottlenecks in the emulator's
  dispatcher and to guide optimizations with measurements.
- **`--wall=SECONDS`** stopped execution after a real-time limit. It served as a safeguard in
  diagnostics and automation when the virtual clock or the instruction limit did not quickly end
  a stuck game.

**`--trace[=filter]` is not part of this pending item**: it traces BREW calls in the `Machine`,
not backend instructions, and keeps working with Dynarmic.

