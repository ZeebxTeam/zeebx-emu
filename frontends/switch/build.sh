#!/usr/bin/env bash
# Builds libzeebx_libretro.a for Horizon.
#
# On the Mac, the toolchain is in the rombundler-switch Docker image (devkitA64 + rustc):
#   ./frontends/switch/build.sh
#
# Inside that toolchain (the libretro.yml job uses the devkitpro/devkita64 image):
#   ./frontends/switch/build.sh --local [output-directory]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
IMAGE="${ZEEBX_SWITCH_IMAGE:-rombundler-switch}"
NX_BIN="/opt/devkitpro/devkitA64/bin"

build_local() {
	local out="${1:-$HERE/out}"
	mkdir -p "$out" /tmp/zeebx-switch-stubs
	cp "$HERE/stubs/libdl.ld" /tmp/zeebx-switch-stubs/libdl.a
	cp "$HERE/stubs/libgcc_s.ld" /tmp/zeebx-switch-stubs/libgcc_s.a
	cp "$HERE/stubs/librt.ld" /tmp/zeebx-switch-stubs/librt.a
	cp "$HERE/stubs/libutil.ld" /tmp/zeebx-switch-stubs/libutil.a
	export CC_aarch64_unknown_linux_gnu="${NX_BIN}/aarch64-none-elf-gcc"
	export CXX_aarch64_unknown_linux_gnu="${NX_BIN}/aarch64-none-elf-g++"
	export AR_aarch64_unknown_linux_gnu="${NX_BIN}/aarch64-none-elf-gcc-ar"
	export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="${NX_BIN}/aarch64-none-elf-gcc"
	export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="--cfg zeebx_switch -Lnative=/tmp/zeebx-switch-stubs"
	export CMAKE_TOOLCHAIN_FILE_aarch64_unknown_linux_gnu="$HERE/cmake/aarch64-none-elf.cmake"
	export CFLAGS_aarch64_unknown_linux_gnu="-I$HERE/compat"
	export CXXFLAGS_aarch64_unknown_linux_gnu="-D_DEFAULT_SOURCE -I$HERE/compat"
	export LIBSQLITE3_FLAGS="-DSQLITE_OMIT_WAL -DSQLITE_MAX_MMAP_SIZE=0 -DSQLITE_OMIT_LOAD_EXTENSION"
	cd "$ROOT"
	cargo rustc --release --locked -p zeebx-libretro --crate-type staticlib --target aarch64-unknown-linux-gnu
	bash "$HERE/bundle-native-libs.sh"
	cp -f "$ROOT/target/aarch64-unknown-linux-gnu/release/libzeebx_libretro.a" "$out/"
	echo "built: $out/libzeebx_libretro.a"
}

if [[ "${1:-}" == "--local" ]]; then
	shift
	build_local "$@"
	exit 0
fi

OUT="${1:-$HERE/out}"
mkdir -p "$OUT"

docker run --rm --platform linux/amd64 \
	-u "$(id -u):$(id -g)" \
	-e HOME=/tmp \
	-v "$ROOT:/src" \
	-w /src \
	"$IMAGE" \
	bash -lc "set -euo pipefail
cd /src
./frontends/switch/build.sh --local /src/frontends/switch/out
"

if [[ "$OUT" != "$HERE/out" ]]; then
	cp -f "$HERE/out/libzeebx_libretro.a" "$OUT/"
	echo "built: $OUT/libzeebx_libretro.a"
fi
