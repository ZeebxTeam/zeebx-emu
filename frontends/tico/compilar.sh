#!/usr/bin/env bash
# Monta o módulo do tico: tico-zeebx.nro mais o module.json, num zip cuja pasta
# de cima é zeebx/.
#
# Não é o core do RetroArch. Aquele continua em frontends/switch/compilar.sh e
# sai como libzeebx_libretro.a. Este aqui é o .nro que o tico abre com o jogo
# na linha de comando. O desenho é no processador: não há contexto de OpenGL
# neste processo. O dynarmic usa o mesmo cfg(zeebx_switch) do core; os
# switch_jit_* saem do hospedeiro.c, via o JIT do libnx, e não do ROMBundler.
#
# No Mac, a cadeia é a imagem oficial do devkitPro (ela não traz rustc; este
# script instala o stable no contêiner), a mesma do job tico.yml:
#   ./frontends/tico/compilar.sh
#
# Dentro dessa cadeia:
#   ./frontends/tico/compilar.sh --local [diretorio-de-saida]
set -euo pipefail

AQUI="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RAIZ="$(cd "$AQUI/../.." && pwd)"
# Tag datada, a mesma do tico.yml e do core-switch. ZEEBX_SWITCH_IMAGE ainda
# aceita a imagem local rombundler-switch, se alguém a tiver.
IMAGEM="${ZEEBX_SWITCH_IMAGE:-devkitpro/devkita64:20260219}"
export DEVKITPRO="${DEVKITPRO:-/opt/devkitpro}"
NX_BIN="${DEVKITPRO}/devkitA64/bin"
FERRAMENTAS="${DEVKITPRO}/tools/bin"
LIBNX="${DEVKITPRO}/libnx"

compilar_local() {
	local saida="${1:-$AQUI/saida}"
	if [[ ! -x "${NX_BIN}/aarch64-none-elf-gcc" ]]; then
		echo "falta o aarch64-none-elf-gcc em ${NX_BIN}." >&2
		exit 1
	fi
	if [[ ! -f "${LIBNX}/include/switch.h" ]]; then
		echo "falta o libnx em ${LIBNX}." >&2
		exit 1
	fi
	if [[ ! -x "${FERRAMENTAS}/elf2nro" || ! -x "${FERRAMENTAS}/nacptool" ]]; then
		echo "faltam elf2nro e nacptool em ${FERRAMENTAS}." >&2
		exit 1
	fi
	if ! command -v cargo >/dev/null 2>&1; then
		echo "falta o cargo; o rustc do host ou do contêiner tem de estar no PATH." >&2
		exit 1
	fi
	if ! command -v unzip >/dev/null 2>&1; then
		echo "falta o unzip, para conferir o zip do módulo." >&2
		exit 1
	fi
	mkdir -p "$saida" /tmp/zeebx-switch-stubs
	cp "$RAIZ/frontends/switch/stubs/libdl.ld" /tmp/zeebx-switch-stubs/libdl.a
	cp "$RAIZ/frontends/switch/stubs/libgcc_s.ld" /tmp/zeebx-switch-stubs/libgcc_s.a
	cp "$RAIZ/frontends/switch/stubs/librt.ld" /tmp/zeebx-switch-stubs/librt.a
	cp "$RAIZ/frontends/switch/stubs/libutil.ld" /tmp/zeebx-switch-stubs/libutil.a

	# Diretório próprio. O core Libretro e este .nro compilam o núcleo com
	# features diferentes; dividir o target faz um apagar o outro.
	export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$RAIZ/target/tico}"
	export CC_aarch64_unknown_linux_gnu="${NX_BIN}/aarch64-none-elf-gcc"
	export CXX_aarch64_unknown_linux_gnu="${NX_BIN}/aarch64-none-elf-g++"
	export AR_aarch64_unknown_linux_gnu="${NX_BIN}/aarch64-none-elf-gcc-ar"
	export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="${NX_BIN}/aarch64-none-elf-gcc"
	export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS="--cfg zeebx_switch -Lnative=/tmp/zeebx-switch-stubs -C panic=abort -C relocation-model=pic"
	export CMAKE_TOOLCHAIN_FILE_aarch64_unknown_linux_gnu="$RAIZ/frontends/switch/cmake/aarch64-none-elf.cmake"
	export CFLAGS_aarch64_unknown_linux_gnu="-I$RAIZ/frontends/switch/compat"
	export CXXFLAGS_aarch64_unknown_linux_gnu="-D_DEFAULT_SOURCE -I$RAIZ/frontends/switch/compat"
	export LIBSQLITE3_FLAGS="-DSQLITE_OMIT_WAL -DSQLITE_MAX_MMAP_SIZE=0 -DSQLITE_OMIT_LOAD_EXTENSION"

	cd "$RAIZ"
	rustup target add aarch64-unknown-linux-gnu >/dev/null
	cargo rustc --release --locked -p zeebx-tico --crate-type staticlib --target aarch64-unknown-linux-gnu
	local arquivo="$CARGO_TARGET_DIR/aarch64-unknown-linux-gnu/release/libzeebx_tico.a"
	if [[ ! -f "$arquivo" ]]; then
		echo "o rustc não deixou $arquivo" >&2
		exit 1
	fi
	bash "$RAIZ/frontends/switch/bundle-native-libs.sh" "$arquivo"

	local tmp
	tmp="$(mktemp -d)"
	# O switch.specs só mexe no link. Include e -lnx vêm do Makefile do
	# libnx (LIBDIRS → -I$LIBNX/include -L$LIBNX/lib -lnx), e o DEVKITPRO
	# precisa estar no ambiente: o specs resolve o linker script por getenv.
	#
	# O hospedeiro é C: o g++ trata `.c` como C++ e recusa o memalign e o
	# stdatomic. Compila com o gcc; a libstdc++ entra no link porque o
	# dynarmic e o {fmt} são C++ (sem ela faltam std::_Rb_tree_* e delete).
	local arch="-march=armv8-a+crc+crypto -mtune=cortex-a57 -mtp=soft -fPIE"
	# shellcheck disable=SC2086
	"${NX_BIN}/aarch64-none-elf-gcc" $arch -g -O2 -ffunction-sections -D__SWITCH__ \
		-I"${LIBNX}/include" -c "$AQUI/hospedeiro.c" -o "$tmp/hospedeiro.o"
	# O posix_faltante também fala com o libnx (csrng, svcSleepThread).
	# shellcheck disable=SC2086
	"${NX_BIN}/aarch64-none-elf-gcc" $arch -g -O2 -ffunction-sections -D__SWITCH__ \
		-I"${LIBNX}/include" -c "$AQUI/posix_faltante.c" -o "$tmp/posix_faltante.o"
	# Sem --whole-archive: a std linux-gnu puxaria todo objeto ocioso (egui,
	# lzma em paralelo…) e com ele símbolos da glibc que o Horizon não tem.
	# O arquivo já traz o dynarmic embutido; o linker puxa o que a raiz referencia.
	# shellcheck disable=SC2086
	if ! "${NX_BIN}/aarch64-none-elf-gcc" $arch \
		-specs="${LIBNX}/switch.specs" \
		"$tmp/hospedeiro.o" "$tmp/posix_faltante.o" \
		-L"${LIBNX}/lib" -L/tmp/zeebx-switch-stubs \
		"$arquivo" \
		-lnx -lstdc++ -ldl -lgcc_s -lrt -lutil -lm \
		-o "$tmp/zeebx.elf"; then
		echo "o link do .nro falhou; o zip não foi gerado." >&2
		rm -rf "$tmp"
		exit 1
	fi
	if ! "${NX_BIN}/aarch64-none-elf-objdump" -f "$tmp/zeebx.elf" | grep -q elf64-littleaarch64; then
		echo "zeebx.elf não é AArch64" >&2
		rm -rf "$tmp"
		exit 1
	fi
	"${FERRAMENTAS}/nacptool" --create "Zeebx" "ZeebxTeam" "0.5.1" "$tmp/zeebx.nacp"
	"${NX_BIN}/aarch64-none-elf-strip" -o "$tmp/zeebx.strip" "$tmp/zeebx.elf" || cp "$tmp/zeebx.elf" "$tmp/zeebx.strip"
	"${FERRAMENTAS}/elf2nro" "$tmp/zeebx.strip" "$tmp/tico-zeebx.nro" --nacp="$tmp/zeebx.nacp"

	local pacote="$tmp/zeebx"
	mkdir -p "$pacote/lang" "$pacote/gamelists" "$pacote/licenses"
	cp "$tmp/tico-zeebx.nro" "$pacote/"
	cp "$AQUI/modulo/module.json" "$AQUI/modulo/settings.json" "$AQUI/LEIAME.md" "$pacote/"
	cp "$AQUI/modulo/lang/"*.json "$pacote/lang/"
	cp "$AQUI/modulo/gamelists/zeebo.dat" "$pacote/gamelists/"
	cp "$RAIZ/LICENSE" "$RAIZ/LICENSE-GPL3" "$RAIZ/THIRD-PARTY-NOTICES.txt" "$pacote/licenses/"
	mkdir -p "$saida"
	rm -f "$saida/zeebx-tico-module.zip"
	( cd "$tmp" && zip -r "$saida/zeebx-tico-module.zip" zeebx )
	rm -rf "$tmp"

	if ! unzip -l "$saida/zeebx-tico-module.zip" | grep -q 'zeebx/module.json'; then
		echo "o zip não contém zeebx/module.json" >&2
		exit 1
	fi
	if ! unzip -l "$saida/zeebx-tico-module.zip" | grep -q 'zeebx/tico-zeebx.nro'; then
		echo "o zip não contém zeebx/tico-zeebx.nro" >&2
		exit 1
	fi
	echo "built: $saida/zeebx-tico-module.zip"
}

if [[ "${1:-}" == "--local" ]]; then
	shift
	compilar_local "$@"
	exit 0
fi

SAIDA="${1:-$AQUI/saida}"
mkdir -p "$SAIDA" "$RAIZ/target"

docker run --rm --platform linux/amd64 \
	-e HOME=/tmp \
	-e CARGO_HOME=/src/target/tico-cargo \
	-e RUSTUP_HOME=/src/target/tico-rustup \
	-e CARGO_TARGET_DIR=/src/target/tico \
	-e HOST_UID="$(id -u)" \
	-e HOST_GID="$(id -g)" \
	-v "$RAIZ:/src" \
	-w /src \
	"$IMAGEM" \
	bash -lc "set -euo pipefail
# O cargo escreve como root no volume. Sem isto, uma compilação que quebra no meio deixa
# target/ com dono root e o dono da máquina não apaga.
trap 'chown -R \"\${HOST_UID}:\${HOST_GID}\" /src/frontends/tico/saida /src/target/tico-cargo /src/target/tico-rustup /src/target/tico 2>/dev/null || true' EXIT
if ! command -v curl >/dev/null 2>&1 || ! command -v cmake >/dev/null 2>&1 || ! command -v unzip >/dev/null 2>&1 || ! command -v zip >/dev/null 2>&1; then
  apt-get update
  apt-get install -y --no-install-recommends ca-certificates curl cmake git ninja-build pkg-config python3 unzip zip
fi
if [[ -f \"\${CARGO_HOME}/env\" ]]; then
  # shellcheck disable=SC1091
  . \"\${CARGO_HOME}/env\"
fi
export PATH=\"\${CARGO_HOME}/bin:\${PATH}\"
if ! command -v rustc >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal --target aarch64-unknown-linux-gnu
  # shellcheck disable=SC1091
  . \"\${CARGO_HOME}/env\"
fi
if ! rustup target list --installed | grep -q '^aarch64-unknown-linux-gnu\$'; then
  rustup target add aarch64-unknown-linux-gnu
fi
cd /src
./frontends/tico/compilar.sh --local /src/frontends/tico/saida
chown -R \"\${HOST_UID}:\${HOST_GID}\" /src/frontends/tico/saida /src/target/tico-cargo /src/target/tico-rustup /src/target/tico || true
"

if [[ "$SAIDA" != "$AQUI/saida" ]]; then
	cp -f "$AQUI/saida/zeebx-tico-module.zip" "$SAIDA/"
	echo "built: $SAIDA/zeebx-tico-module.zip"
fi
