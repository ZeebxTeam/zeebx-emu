#!/usr/bin/env bash
# Monta zeebx.vpk, o Zeebx que abre sozinho na PlayStation Vita.
#
# Não é o core do RetroArch. Aquele continua em frontends/vita/compilar.sh e sai
# como libzeebx_libretro.a. Este aqui é o aplicativo: eboot.bin, param.sfo e a
# LiveArea. Vídeo, áudio e controle são do próprio binário — o vitaGL apresenta
# a tela, a porta de som é a MAIN a 48 kHz, o controle é o sceCtrl. O
# preenchimento vai para a placa; o vitaGL traduz OpenGL ES 2, e o shader do
# núcleo tem essa versão. O núcleo é o interpretador, porque o dynarmic não
# emite ARMv7.
#
# No Mac, a cadeia está na imagem Docker vitasdk/vitasdk (ela não traz rustc;
# este script instala o stable no contêiner):
#   ./frontends/vita-standalone/compilar.sh
#
# Dentro dessa cadeia (o vita.yml usa a mesma imagem):
#   ./frontends/vita-standalone/compilar.sh --local [diretorio-de-saida]
set -euo pipefail

AQUI="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RAIZ="$(cd "$AQUI/../.." && pwd)"
IMAGEM="${ZEEBX_VITA_IMAGE:-vitasdk/vitasdk:2026.08}"
ALVO="armv7-sony-vita-newlibeabihf"

compilar_local() {
	local saida="${1:-$AQUI/saida}"
	if [[ -z "${VITASDK:-}" || ! -x "${VITASDK}/bin/arm-vita-eabi-gcc" ]]; then
		echo "defina VITASDK com a raiz do vitasdk (arm-vita-eabi-gcc tem de existir)." >&2
		exit 1
	fi
	if [[ ! -d "$(rustc --print sysroot)/lib/rustlib/src/rust/library/std" ]]; then
		echo "precisa do componente rust-src: a std da Vita é recompilada, o rustup não a publica." >&2
		exit 1
	fi
	if ! command -v unzip >/dev/null 2>&1; then
		echo "falta o unzip, para conferir que o vpk leva o eboot.bin." >&2
		exit 1
	fi

	local bin="${VITASDK}/bin"
	mkdir -p "$saida"
	# Diretório próprio, separado do core Libretro. Os dois compilam o núcleo com
	# features diferentes; dividir o target faz um apagar o outro.
	export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$RAIZ/target/vita-app}"
	export PATH="${bin}:${PATH}"
	export CC_${ALVO//-/_}="${bin}/arm-vita-eabi-gcc"
	export CXX_${ALVO//-/_}="${bin}/arm-vita-eabi-g++"
	export AR_${ALVO//-/_}="${bin}/arm-vita-eabi-gcc-ar"
	export CARGO_TARGET_ARMV7_SONY_VITA_NEWLIBEABIHF_LINKER="${bin}/arm-vita-eabi-gcc"
	# O laço de pixel é escalar. Sem o Cortex-A9 o rustc gera ARMv7 genérico e não
	# usa o NEON que o aparelho tem; o gcc do vitasdk já aponta esse núcleo.
	export CARGO_TARGET_ARMV7_SONY_VITA_NEWLIBEABIHF_RUSTFLAGS="-C panic=abort -C target-cpu=cortex-a9"
	# O gcc do vitasdk já aponta o Cortex-A9. `-mword-relocations` é o que cabe
	# num executável grande. `compat/` cobre o `sys/ioctl.h` que a newlib não traz
	# e o sqlite inclui — o mesmo arranjo do core.
	local cflags="-march=armv7-a -mfpu=neon -mfloat-abi=hard -mword-relocations -DVITA -I${RAIZ}/frontends/vita/compat"
	export CFLAGS_armv7_sony_vita_newlibeabihf="${cflags}"
	export CXXFLAGS_armv7_sony_vita_newlibeabihf="${cflags}"
	export LIBSQLITE3_FLAGS="-DSQLITE_OMIT_WAL -DSQLITE_MAX_MMAP_SIZE=0 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_TEMP_STORE=3"
	export RUSTC_BOOTSTRAP=1

	cd "$RAIZ"
	cargo rustc -Z build-std=std,panic_abort --release --locked \
		-p zeebx-vita --target "$ALVO"

	local elf="${CARGO_TARGET_DIR}/${ALVO}/release/zeebx-vita.elf"
	# O alvo da Vita no rustc põe `.elf` no executável. Sem o sufixo o arquivo não existe,
	# e o `vita-elf-create` não tem o que converter.
	if [[ ! -f "$elf" ]]; then
		echo "o rustc não deixou $elf" >&2
		exit 1
	fi
	if ! "${bin}/arm-vita-eabi-objdump" -f "$elf" | grep -q elf32-littlearm; then
		echo "zeebx-vita não é ARM de 32 bits" >&2
		exit 1
	fi

	local tmp
	tmp="$(mktemp -d)"
	"${bin}/vita-elf-create" "$elf" "$tmp/zeebx.velf"
	# `-s` comprime o self. O ELF com relocação fica grande; o eboot que o aparelho lê, não.
	"${bin}/vita-make-fself" -s "$tmp/zeebx.velf" "$tmp/eboot.bin"
	# ATTRIBUTE2 é inteiro. 12 pede a memória estendida: é ela que cabe o heap de 192 MiB.
	"${bin}/vita-mksfoex" -s TITLE_ID=ZEEB00001 -s APP_VER=00.51 -d ATTRIBUTE2=12 \
		"Zeebx" "$tmp/param.sfo"
	# O Vita de verdade recusa o VPK com 0x8010113D se o ícone ou o LiveArea não forem
	# PNG de 256 cores (tipo 3, 8 bits). O Vita3K instala os mesmos arquivos em RGBA.
	python3 - "$AQUI/bubble/icon0.png" "$AQUI/bubble/bg.png" "$AQUI/bubble/startup.png" <<'PY'
import struct, sys
for caminho in sys.argv[1:]:
    with open(caminho, "rb") as f:
        sig = f.read(8)
        f.read(8)
        w, h, bit, color = struct.unpack(">IIBB", f.read(10))
    if sig != b"\x89PNG\r\n\x1a\n" or bit != 8 or color != 3:
        raise SystemExit(f"{caminho} não é PNG de 256 cores ({w}x{h}, bits {bit}, tipo {color})")
PY
	"${bin}/vita-pack-vpk" -s "$tmp/param.sfo" -b "$tmp/eboot.bin" \
		--add "$AQUI/bubble/icon0.png=sce_sys/icon0.png" \
		--add "$AQUI/bubble/bg.png=sce_sys/livearea/contents/bg.png" \
		--add "$AQUI/bubble/startup.png=sce_sys/livearea/contents/startup.png" \
		--add "$AQUI/bubble/template.xml=sce_sys/livearea/contents/template.xml" \
		"$saida/zeebx.vpk"
	rm -rf "$tmp"

	if ! unzip -l "$saida/zeebx.vpk" | grep -q 'eboot.bin'; then
		echo "zeebx.vpk não contém eboot.bin" >&2
		exit 1
	fi
	echo "built: $saida/zeebx.vpk"
}

if [[ "${1:-}" == "--local" ]]; then
	shift
	compilar_local "$@"
	exit 0
fi

SAIDA="${1:-$AQUI/saida}"
mkdir -p "$SAIDA" "$RAIZ/target"

docker run --rm \
	-e HOME=/tmp \
	-e CARGO_HOME=/src/target/vita-app-cargo \
	-e RUSTUP_HOME=/src/target/vita-app-rustup \
	-e HOST_UID="$(id -u)" \
	-e HOST_GID="$(id -g)" \
	-v "$RAIZ:/src" \
	-w /src \
	"$IMAGEM" \
	bash -lc "set -euo pipefail
# O cargo escreve como root no volume. Sem isto, uma compilação que quebra no meio deixa
# target/ com dono root e o dono da máquina não apaga.
trap 'chown -R \"\${HOST_UID}:\${HOST_GID}\" /src/frontends/vita-standalone/saida /src/target/vita-app-cargo /src/target/vita-app-rustup /src/target/vita-app 2>/dev/null || true' EXIT
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
./frontends/vita-standalone/compilar.sh --local /src/frontends/vita-standalone/saida
chown -R \"\${HOST_UID}:\${HOST_GID}\" /src/frontends/vita-standalone/saida /src/target/vita-app-cargo /src/target/vita-app-rustup /src/target/vita-app || true
"

if [[ "$SAIDA" != "$AQUI/saida" ]]; then
	cp -f "$AQUI/saida/zeebx.vpk" "$SAIDA/"
	echo "built: $SAIDA/zeebx.vpk"
fi
