#!/usr/bin/env bash
# Builds libzeebx_libretro.a for the PlayStation Vita.
#
# The Vita's RetroArch is linked statically: this file doesn't run on its own.
# Whoever consumes it copies the `.a` to `libretro_vita.a` in the RetroArch tree
# and builds the VPK with the vitasdk. Video, audio and input remain the
# frontend's — the core delivers RGB565, PCM16 at 44.1 kHz and reads the
# RetroPad. Drawing stays on the CPU: vitaGL doesn't provide OpenGL ES 3. The
# core is the interpreter, because dynarmic doesn't emit ARMv7.
#
# On the Mac, the toolchain is in the vitasdk/vitasdk Docker image (it doesn't
# ship rustc; this script installs stable in the container):
#   ./frontends/vita/build.sh
#
# Inside that toolchain (the libretro.yml job uses the same image):
#   ./frontends/vita/build.sh --local [output-directory]
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

	local bin="${VITASDK}/bin"
	mkdir -p "$out"
	# The target is tier 3: the compiler knows the triple, but the std comes
	# from source with `-Z build-std`. `panic=abort` is what the rustc book
	# documents for this target — libunwind is not part of the vitasdk contract.
	#
	# Its own directory: the container's cargo is Linux and, in the default
	# directory, would write the host build scripts over the Mac's.
	export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/vita}"
	export PATH="${bin}:${PATH}"
	export CC_${TARGET//-/_}="${bin}/arm-vita-eabi-gcc"
	export CXX_${TARGET//-/_}="${bin}/arm-vita-eabi-g++"
	export AR_${TARGET//-/_}="${bin}/arm-vita-eabi-gcc-ar"
	export CARGO_TARGET_ARMV7_SONY_VITA_NEWLIBEABIHF_LINKER="${bin}/arm-vita-eabi-gcc"
	export CARGO_TARGET_ARMV7_SONY_VITA_NEWLIBEABIHF_RUSTFLAGS="-C panic=abort"
	# The vitasdk gcc already targets the Cortex-A9. `-mword-relocations` is
	# what RetroArch's Makefile.vita uses so the object fits in the large
	# executable. `compat/` covers the `sys/ioctl.h` that newlib doesn't ship
	# and sqlite includes.
	local cflags="-march=armv7-a -mfpu=neon -mfloat-abi=hard -mword-relocations -DVITA -I${HERE}/compat"
	export CFLAGS_armv7_sony_vita_newlibeabihf="${cflags}"
	export CXXFLAGS_armv7_sony_vita_newlibeabihf="${cflags}"
	# No mmap and no WAL: newlib has no `sys/mman.h`. Temporary tables in
	# memory avoid a temp directory the device doesn't offer in the form sqlite
	# expects. `fdatasync` exists in the libc, so it isn't turned off.
	export LIBSQLITE3_FLAGS="-DSQLITE_OMIT_WAL -DSQLITE_MAX_MMAP_SIZE=0 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_TEMP_STORE=3"
	export RUSTC_BOOTSTRAP=1

	cd "$ROOT"
	cargo rustc -Z build-std=std,panic_abort --release --locked \
		-p zeebx-libretro --crate-type staticlib --target "$TARGET"

	local artifact="${CARGO_TARGET_DIR}/${TARGET}/release/libzeebx_libretro.a"
	if [[ ! -f "$artifact" ]]; then
		echo "rustc did not leave $artifact" >&2
		exit 1
	fi
	cp -f "$artifact" "$out/libzeebx_libretro.a"

	# The sqlite amalgamation takes the address of `fchown` and `readlink`.
	# The Vita's newlib declares both and doesn't define them. Without this
	# object RetroArch doesn't link the `.a`. At run time the process is not
	# root, so sqlite doesn't even call `fchown`; `readlink` returning an error
	# means "not a symlink".
	local tmp_stub
	tmp_stub="$(mktemp -d)"
	"${bin}/arm-vita-eabi-gcc" -c -O2 ${cflags} \
		-o "${tmp_stub}/posix_missing.o" "$HERE/compat/posix_missing.c"
	"${bin}/arm-vita-eabi-gcc-ar" r "$out/libzeebx_libretro.a" "${tmp_stub}/posix_missing.o"
	rm -rf "$tmp_stub"

	local tmp
	tmp="$(mktemp -d)"
	( cd "$tmp" && "${bin}/arm-vita-eabi-ar" x "$out/libzeebx_libretro.a" )
	local bad
	bad="$("${bin}/arm-vita-eabi-objdump" -f "$tmp"/*.o | grep 'file format' | grep -v elf32-littlearm || true)"
	if [[ -n "$bad" ]]; then
		echo "the .a mixed in an object that is not 32-bit ARM" >&2
		echo "$bad" >&2
		rm -rf "$tmp"
		exit 1
	fi
	# No `-q`: a grep that stops at the first match closes the pipe, `nm` dies
	# with SIGPIPE and `pipefail` treats that as a failure even with the symbol
	# there.
	if ! "${bin}/arm-vita-eabi-nm" -g "$out/libzeebx_libretro.a" | grep ' retro_api_version$' >/dev/null; then
		echo "libzeebx_libretro.a does not export retro_api_version" >&2
		rm -rf "$tmp"
		exit 1
	fi
	rm -rf "$tmp"
	echo "built: $out/libzeebx_libretro.a"
}

if [[ "${1:-}" == "--local" ]]; then
	shift
	build_local "$@"
	exit 0
fi

OUT="${1:-$HERE/out}"
mkdir -p "$OUT" "$ROOT/target"

# The official image is Ubuntu with the vitasdk, without rustc. Stable goes
# into the container; `rust-src` stays because this target's std is not
# published.
docker run --rm \
	-e HOME=/tmp \
	-e CARGO_HOME=/src/target/vita-cargo \
	-e RUSTUP_HOME=/src/target/vita-rustup \
	-e HOST_UID="$(id -u)" \
	-e HOST_GID="$(id -g)" \
	-v "$ROOT:/src" \
	-w /src \
	"$IMAGE" \
	bash -lc "set -euo pipefail
if ! command -v curl >/dev/null 2>&1 || ! command -v cc >/dev/null 2>&1 || ! command -v pkg-config >/dev/null 2>&1; then
  apt-get update
  apt-get install -y --no-install-recommends ca-certificates curl build-essential pkg-config
fi
# rustup lives in CARGO_HOME, which is not on the image's PATH. Without
# sourcing the env, the second run would think rustc doesn't exist and
# install it again.
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
./frontends/vita/build.sh --local /src/frontends/vita/out
chown -R \"\${HOST_UID}:\${HOST_GID}\" /src/frontends/vita/out /src/target/vita-cargo /src/target/vita-rustup /src/target/vita || true
"

if [[ "$OUT" != "$HERE/out" ]]; then
	cp -f "$HERE/out/libzeebx_libretro.a" "$OUT/"
	echo "built: $OUT/libzeebx_libretro.a"
fi
