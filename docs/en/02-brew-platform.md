# 02 — The platform: Qualcomm BREW 4.0.2

The Zeebo runs **BREW 4.0.2** with customizations by Zeebo Inc./TecToy. Understanding BREW is 90% of the work in an emulator.

## Execution model

BREW is a **COM-like C environment** over the AEE (*Application Execution Environment*):

- Every API is an interface with a **vtable**: `IShell`, `IDisplay`, `IGraphics`, `IBitmap`, `IFileMgr`, `IFile`, `ISound`, `ISoundPlayer`, `IMedia`, `IHID`, `IHIDDevice`, `ISignal`, `IHeap`, `IImage`, `IFont`, `IWeb`, `INetMgr`, and others.
- An application is a relocatable DLL (`.mod`) whose **first `.text` symbol is `AEEMod_Load()`**. The AEE loads the module, calls `AEEMod_Load`, and the module registers its `CreateInstance` function.
- Objects are created through `ISHELL_CreateInstance(shell, ClassID, &obj)`; a ClassID is a 32-bit number assigned by Qualcomm or the platform vendor.

The game calls these interfaces rather than hardware. That boundary is what makes high-level emulation practical.
