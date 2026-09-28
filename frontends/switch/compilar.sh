#!/usr/bin/env bash
# Monta libzeebx_libretro.a para o Horizon.
#
# No Mac, a cadeia está na imagem Docker rombundler-switch (devkitA64 + rustc):
#   ./frontends/switch/compilar.sh
#
# Dentro dessa cadeia (o job do libretro.yml usa a imagem devkitpro/devkita64):
#   ./frontends/switch/compilar.sh --local [diretorio-de-saida]
set -euo pipefail

AQUI="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RAIZ="$(cd "$AQUI/../.." && pwd)"
IMAGEM="${ZEEBX_SWITCH_IMAGE:-rombundler-switch}"
NX_BIN="/opt/devkitpro/devkitA64/bin"

compilar_local() {
	local saida="${1:-$AQUI/saida}"
	mkdir -p "$saida" /tmp/zeebx-switch-stubs
	cp "$AQUI/stubs/libdl.ld" /tmp/zeebx-switch-stubs/libdl.a
	cp "$AQUI/stubs/libgcc_s.ld" /tmp/zeebx-switch-stubs/libgcc_s.a
	cp "$AQUI/stubs/librt.ld" /tmp/zeebx-switch-stubs/librt.a
	cp "$AQUI/stubs/libutil.ld" /tmp/zeebx-switch-stubs/libutil.a
	export CC_aarch64_unknown_linux_gnu="${NX_BIN}/aarch64-none-elf-gcc"
	export CXX_aarch64_unknown_linux_gnu="${NX_BIN}/aarch64-none-elf-g++"
	export AR_aarch64_unknown_linux_gnu="${NX_BIN}/aarch64-none-elf-gcc-ar"
	export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="${NX_BIN}/aarch64-none-elf-gcc"
	export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="--cfg zeebx_switch -Lnative=/tmp/zeebx-switch-stubs"
	export CMAKE_TOOLCHAIN_FILE_aarch64_unknown_linux_gnu="$AQUI/cmake/aarch64-none-elf.cmake"
	export CFLAGS_aarch64_unknown_linux_gnu="-I$AQUI/compat"
	export CXXFLAGS_aarch64_unknown_linux_gnu="-D_DEFAULT_SOURCE -I$AQUI/compat"
	export LIBSQLITE3_FLAGS="-DSQLITE_OMIT_WAL -DSQLITE_MAX_MMAP_SIZE=0 -DSQLITE_OMIT_LOAD_EXTENSION"
	cd "$RAIZ"
	cargo rustc --release --locked -p zeebx-libretro --crate-type staticlib --target aarch64-unknown-linux-gnu
	bash "$AQUI/bundle-native-libs.sh"
	cp -f "$RAIZ/target/aarch64-unknown-linux-gnu/release/libzeebx_libretro.a" "$saida/"
	echo "built: $saida/libzeebx_libretro.a"
}

if [[ "${1:-}" == "--local" ]]; then
	shift
	compilar_local "$@"
	exit 0
fi

SAIDA="${1:-$AQUI/saida}"
mkdir -p "$SAIDA"

docker run --rm --platform linux/amd64 \
	-u "$(id -u):$(id -g)" \
	-e HOME=/tmp \
	-v "$RAIZ:/src" \
	-w /src \
	"$IMAGEM" \
	bash -lc "set -euo pipefail
cd /src
./frontends/switch/compilar.sh --local /src/frontends/switch/saida
"

if [[ "$SAIDA" != "$AQUI/saida" ]]; then
	cp -f "$AQUI/saida/libzeebx_libretro.a" "$SAIDA/"
	echo "built: $SAIDA/libzeebx_libretro.a"
fi
