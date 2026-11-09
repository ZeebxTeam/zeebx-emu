#!/usr/bin/env bash
# Builds zeebx.vpk, the Zeebx that opens on its own on the PlayStation Vita.
#
# It is not the RetroArch core. That one remains in frontends/vita/build.sh and
# comes out as libzeebx_libretro.a. This one is the application: eboot.bin,
# param.sfo and the LiveArea. Video, audio and controller belong to the binary
# itself — vitaGL presents the screen, the sound port is MAIN at 48 kHz, the
# controller is sceCtrl. Filling goes to the GPU; vitaGL translates OpenGL ES 2,
# and the core's shader is that version. The core is the interpreter, because
# dynarmic doesn't emit ARMv7.
#
# On the Mac, the toolchain is in the vitasdk/vitasdk Docker image (it doesn't
# ship rustc; this script installs stable in the container):
#   ./frontends/vita-standalone/build.sh
#
# Inside that toolchain (vita.yml uses the same image):
#   ./frontends/vita-standalone/build.sh --local [output-directory]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
IMAGE="${ZEEBX_VITA_IMAGE:-vitasdk/vitasdk:2026.08}"
TARGET="armv7-sony-vita-newlibeabihf"

build_local() {
	local out="${1:-$HERE/out}"
	if [[ -z "${VITASDK:-}" || ! -x "${VITASDK}/bin/arm-vita-eabi-gcc" ]]; then
		echo "set VITASDK to the vitasdk root (arm-vita-eabi-gcc must exist)." >&2
		exit 1
	fi
	if [[ ! -d "$(rustc --print sysroot)/lib/rustlib/src/rust/library/std" ]]; then
		echo "the rust-src component is required: the Vita std is rebuilt, rustup doesn't publish it." >&2
		exit 1
	fi
	if ! command -v unzip >/dev/null 2>&1; then
		echo "unzip is missing; it is needed to check that the vpk carries eboot.bin." >&2
		exit 1
	fi

	local bin="${VITASDK}/bin"
	mkdir -p "$out"
	# Its own directory, separate from the Libretro core. The two build the core
	# with different features; sharing the target makes one wipe out the other.
	export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/vita-app}"
	export PATH="${bin}:${PATH}"
	export CC_${TARGET//-/_}="${bin}/arm-vita-eabi-gcc"
	export CXX_${TARGET//-/_}="${bin}/arm-vita-eabi-g++"
	export AR_${TARGET//-/_}="${bin}/arm-vita-eabi-gcc-ar"
	export CARGO_TARGET_ARMV7_SONY_VITA_NEWLIBEABIHF_LINKER="${bin}/arm-vita-eabi-gcc"
	# The pixel loop is scalar. Without the Cortex-A9, rustc generates generic
	# ARMv7 and doesn't use the NEON the device has; the vitasdk gcc already
	# targets that core.
	export CARGO_TARGET_ARMV7_SONY_VITA_NEWLIBEABIHF_RUSTFLAGS="-C panic=abort -C target-cpu=cortex-a9"
	# The vitasdk gcc already targets the Cortex-A9. `-mword-relocations` is what
	# fits a large executable. `compat/` covers the `sys/ioctl.h` that newlib
	# doesn't ship and sqlite includes — the same arrangement as the core.
	local cflags="-march=armv7-a -mfpu=neon -mfloat-abi=hard -mword-relocations -DVITA -I${ROOT}/frontends/vita/compat"
	export CFLAGS_armv7_sony_vita_newlibeabihf="${cflags}"
	export CXXFLAGS_armv7_sony_vita_newlibeabihf="${cflags}"
	export LIBSQLITE3_FLAGS="-DSQLITE_OMIT_WAL -DSQLITE_MAX_MMAP_SIZE=0 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_TEMP_STORE=3"
	export RUSTC_BOOTSTRAP=1

	cd "$ROOT"
	cargo rustc -Z build-std=std,panic_abort --release --locked \
		-p zeebx-vita --target "$TARGET"

	local elf="${CARGO_TARGET_DIR}/${TARGET}/release/zeebx-vita.elf"
	# rustc's Vita target puts `.elf` on the executable. Without the suffix the file doesn't
	# exist, and `vita-elf-create` has nothing to convert.
	if [[ ! -f "$elf" ]]; then
		echo "rustc did not leave $elf" >&2
		exit 1
	fi
	if ! "${bin}/arm-vita-eabi-objdump" -f "$elf" | grep -q elf32-littlearm; then
		echo "zeebx-vita is not 32-bit ARM" >&2
		exit 1
	fi

	local tmp
	tmp="$(mktemp -d)"
	"${bin}/vita-elf-create" "$elf" "$tmp/zeebx.velf"
	# `-s` compresses the self. The ELF with relocations is large; the eboot the device reads is not.
	"${bin}/vita-make-fself" -s "$tmp/zeebx.velf" "$tmp/eboot.bin"
	# ATTRIBUTE2 is an integer. 12 asks for extended memory: that is what fits the 192 MiB heap.
	"${bin}/vita-mksfoex" -s TITLE_ID=ZEEB00001 -s APP_VER=00.51 -d ATTRIBUTE2=12 \
		"Zeebx" "$tmp/param.sfo"
	# A real Vita rejects the VPK with 0x8010113D if the icon or the LiveArea are not
	# 256-color PNGs (type 3, 8 bits). Vita3K installs the same files in RGBA.
	python3 - "$HERE/bubble/icon0.png" "$HERE/bubble/bg.png" "$HERE/bubble/startup.png" <<'PY'
import struct, sys
for path in sys.argv[1:]:
    with open(path, "rb") as f:
        sig = f.read(8)
        f.read(8)
        w, h, bit, color = struct.unpack(">IIBB", f.read(10))
    if sig != b"\x89PNG\r\n\x1a\n" or bit != 8 or color != 3:
        raise SystemExit(f"{path} is not a 256-color PNG ({w}x{h}, bits {bit}, type {color})")
PY
	"${bin}/vita-pack-vpk" -s "$tmp/param.sfo" -b "$tmp/eboot.bin" \
		--add "$HERE/bubble/icon0.png=sce_sys/icon0.png" \
		--add "$HERE/bubble/bg.png=sce_sys/livearea/contents/bg.png" \
		--add "$HERE/bubble/startup.png=sce_sys/livearea/contents/startup.png" \
		--add "$HERE/bubble/template.xml=sce_sys/livearea/contents/template.xml" \
		"$out/zeebx.vpk"
	rm -rf "$tmp"

	if ! unzip -l "$out/zeebx.vpk" | grep -q 'eboot.bin'; then
		echo "zeebx.vpk does not contain eboot.bin" >&2
		exit 1
	fi
	echo "built: $out/zeebx.vpk"
}

if [[ "${1:-}" == "--local" ]]; then
	shift
	build_local "$@"
	exit 0
fi

OUT="${1:-$HERE/out}"
mkdir -p "$OUT" "$ROOT/target"

docker run --rm \
	-e HOME=/tmp \
	-e CARGO_HOME=/src/target/vita-app-cargo \
	-e RUSTUP_HOME=/src/target/vita-app-rustup \
	-e HOST_UID="$(id -u)" \
	-e HOST_GID="$(id -g)" \
	-v "$ROOT:/src" \
	-w /src \
	"$IMAGE" \
	bash -lc "set -euo pipefail
# cargo writes as root on the volume. Without this, a build that breaks midway leaves
# target/ owned by root and the machine's owner can't delete it.
trap 'chown -R \"\${HOST_UID}:\${HOST_GID}\" /src/frontends/vita-standalone/out /src/target/vita-app-cargo /src/target/vita-app-rustup /src/target/vita-app 2>/dev/null || true' EXIT
if ! command -v curl >/dev/null 2>&1 || ! command -v cc >/dev/null 2>&1 || ! command -v pkg-config >/dev/null 2>&1 || ! command -v unzip >/dev/null 2>&1; then
  apt-get update
  apt-get install -y --no-install-recommends ca-certificates curl build-essential pkg-config unzip
fi
if [[ -f \"\${CARGO_HOME}/env\" ]]; then
  # shellcheck disable=SC1091
  . \"\${CARGO_HOME}/env\"
fi
export PATH=\"\${CARGO_HOME}/bin:\${PATH}\"
if ! command -v rustc >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal --component rust-src
  # shellcheck disable=SC1091
  . \"\${CARGO_HOME}/env\"
fi
if ! rustup component list --installed | grep -q '^rust-src'; then
  rustup component add rust-src
fi
cd /src
./frontends/vita-standalone/build.sh --local /src/frontends/vita-standalone/out
chown -R \"\${HOST_UID}:\${HOST_GID}\" /src/frontends/vita-standalone/out /src/target/vita-app-cargo /src/target/vita-app-rustup /src/target/vita-app || true
"

if [[ "$OUT" != "$HERE/out" ]]; then
	cp -f "$HERE/out/zeebx.vpk" "$OUT/"
	echo "built: $OUT/zeebx.vpk"
fi
