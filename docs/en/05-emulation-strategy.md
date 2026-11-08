# 05 — Proposed emulation strategy

## HLE, not LLE

| | LLE (run the firmware) | **HLE (reimplement BREW)** |
|---|---|---|
| Needs the console's NAND/AMSS | yes (legal and distribution issue) | no |
| Needs MSM7201A, Adreno 130, QDSP-5, MDP, modem emulation | yes | no |
| Needs ARM11 emulation | yes | yes (game code only) |
| Effort | years | months until “something runs” |
| Accuracy | high | depends on API fidelity |

The chosen approach is **HLE**. The game only talks to BREW vtables; we implement everything behind them. NAND dumps are references for disassembly when an API's behavior is ambiguous, not runtime dependencies.

## Emulator components

The implementation and the reasons behind its decisions are documented in the [implementation guide](../implementacao/README.md) (Portuguese).
