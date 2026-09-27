#!/bin/bash
# Point Oaknut's code buffer at a RW/RX pair. No-op for CMake projects that are not dynarmic.
set -euo pipefail
root="${1:-}"
header="${root}/externals/oaknut/include/oaknut/code_block.hpp"
if [[ ! -f "${header}" ]]; then
	exit 0
fi
if grep -q ZEEBX_SWITCH_JIT "${header}"; then
	exit 0
fi
patch -p1 -d "${root}" < "$(dirname "$0")/dynarmic-jit.patch"
python3 - "${root}/src/dynarmic/common/spin_lock_arm64.cpp" <<'PY'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = """void EmitSpinLockLock(oaknut::CodeGenerator& code, oaknut::XReg ptr) {
    oaknut::Label start, loop;

    code.MOV(Wscratch1, 1);
    code.SEVL();
    code.l(start);
    code.WFE();
    code.l(loop);
    code.LDAXR(Wscratch0, ptr);
    code.CBNZ(Wscratch0, start);
    code.STXR(Wscratch0, Wscratch1, ptr);
    code.CBNZ(Wscratch0, loop);
}

void EmitSpinLockUnlock(oaknut::CodeGenerator& code, oaknut::XReg ptr) {
    code.STLR(WZR, ptr);
}
"""
new = """extern "C" void switch_spinlock_lock(volatile int*);
extern "C" void switch_spinlock_unlock(volatile int*);

void EmitSpinLockLock(oaknut::CodeGenerator& code, oaknut::XReg ptr) {
    (void)ptr;
    code.MOVP2R(Backend::Arm64::Xscratch0, (const void*)switch_spinlock_lock);
    code.BLR(Backend::Arm64::Xscratch0);
}

void EmitSpinLockUnlock(oaknut::CodeGenerator& code, oaknut::XReg ptr) {
    (void)ptr;
    code.MOVP2R(Backend::Arm64::Xscratch0, (const void*)switch_spinlock_unlock);
    code.BLR(Backend::Arm64::Xscratch0);
}
"""
if old not in text:
    raise SystemExit("spin lock emit block not found")
path.write_text(text.replace(old, new, 1))
PY
