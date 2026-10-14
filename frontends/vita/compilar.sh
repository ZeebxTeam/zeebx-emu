#!/usr/bin/env bash
# Monta libzeebx_libretro.a para a PlayStation Vita.
#
# O RetroArch da Vita é ligado estático: este arquivo não roda sozinho. Quem o
# consome copia o `.a` para `libretro_vita.a` na árvore do RetroArch e monta o
# VPK com o vitasdk. Vídeo, áudio e entrada continuam do frontend — o core
# entrega RGB565, PCM16 a 44,1 kHz e lê o RetroPad. O desenho fica no
# processador: o vitaGL não entrega OpenGL ES 3. O núcleo é o interpretador,
# porque o dynarmic não emite ARMv7.
#
# No Mac, a cadeia está na imagem Docker vitasdk/vitasdk (ela não traz rustc;
# este script instala o stable no contêiner):
#   ./frontends/vita/compilar.sh
#
# Dentro dessa cadeia (o job do libretro.yml usa a mesma imagem):
#   ./frontends/vita/compilar.sh --local [diretorio-de-saida]
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

	local bin="${VITASDK}/bin"
	mkdir -p "$saida"
	# O alvo é tier 3: o compilador sabe o triple, mas a std sai do fonte com
	# `-Z build-std`. O `panic=abort` é o que o livro do rustc documenta para
	# este alvo — a libunwind não faz parte do contrato do vitasdk.
	#
	# Diretório próprio: o cargo do contêiner é Linux e, no diretório padrão,
	# escreveria os build scripts do host por cima dos do Mac.
	export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$RAIZ/target/vita}"
	export PATH="${bin}:${PATH}"
	export CC_${ALVO//-/_}="${bin}/arm-vita-eabi-gcc"
	export CXX_${ALVO//-/_}="${bin}/arm-vita-eabi-g++"
	export AR_${ALVO//-/_}="${bin}/arm-vita-eabi-gcc-ar"
	export CARGO_TARGET_ARMV7_SONY_VITA_NEWLIBEABIHF_LINKER="${bin}/arm-vita-eabi-gcc"
	export CARGO_TARGET_ARMV7_SONY_VITA_NEWLIBEABIHF_RUSTFLAGS="-C panic=abort"
	# O gcc do vitasdk já aponta o Cortex-A9. `-mword-relocations` é o que o
	# Makefile.vita do RetroArch usa para o objeto caber no executável grande.
	# `compat/` cobre o `sys/ioctl.h` que a newlib não traz e o sqlite inclui.
	local cflags="-march=armv7-a -mfpu=neon -mfloat-abi=hard -mword-relocations -DVITA -I${AQUI}/compat"
	export CFLAGS_armv7_sony_vita_newlibeabihf="${cflags}"
	export CXXFLAGS_armv7_sony_vita_newlibeabihf="${cflags}"
	# Sem mmap e sem WAL: a newlib não tem `sys/mman.h`. Tabela temporária em
	# memória evita um diretório de temp que o aparelho não oferece no formato
	# que o sqlite espera. `fdatasync` existe na libc, então não se desliga.
	export LIBSQLITE3_FLAGS="-DSQLITE_OMIT_WAL -DSQLITE_MAX_MMAP_SIZE=0 -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_TEMP_STORE=3"
	export RUSTC_BOOTSTRAP=1

	cd "$RAIZ"
	cargo rustc -Z build-std=std,panic_abort --release --locked \
		-p zeebx-libretro --crate-type staticlib --target "$ALVO"

	local artefato="${CARGO_TARGET_DIR}/${ALVO}/release/libzeebx_libretro.a"
	if [[ ! -f "$artefato" ]]; then
		echo "o rustc não deixou $artefato" >&2
		exit 1
	fi
	cp -f "$artefato" "$saida/libzeebx_libretro.a"

	# O amalgamation do sqlite toma o endereço de `fchown` e `readlink`. A
	# newlib da Vita declara os dois e não os define. Sem este objeto o
	# RetroArch não liga o `.a`. Em execução o processo não é root, então o
	# sqlite nem chama o `fchown`; `readlink` devolver erro é "não é symlink".
	local tmp_stub
	tmp_stub="$(mktemp -d)"
	"${bin}/arm-vita-eabi-gcc" -c -O2 ${cflags} \
		-o "${tmp_stub}/posix_faltante.o" "$AQUI/compat/posix_faltante.c"
	"${bin}/arm-vita-eabi-gcc-ar" r "$saida/libzeebx_libretro.a" "${tmp_stub}/posix_faltante.o"
	rm -rf "$tmp_stub"

	local tmp
	tmp="$(mktemp -d)"
	( cd "$tmp" && "${bin}/arm-vita-eabi-ar" x "$saida/libzeebx_libretro.a" )
	local ruim
	ruim="$("${bin}/arm-vita-eabi-objdump" -f "$tmp"/*.o | grep 'file format' | grep -v elf32-littlearm || true)"
	if [[ -n "$ruim" ]]; then
		echo "o .a misturou objeto que não é ARM de 32 bits" >&2
		echo "$ruim" >&2
		rm -rf "$tmp"
		exit 1
	fi
	# Sem `-q`: o grep que para no primeiro acerto fecha o pipe, o `nm` morre
	# com SIGPIPE e o `pipefail` trata isso como falha mesmo com o símbolo lá.
	if ! "${bin}/arm-vita-eabi-nm" -g "$saida/libzeebx_libretro.a" | grep ' retro_api_version$' >/dev/null; then
		echo "libzeebx_libretro.a não exporta retro_api_version" >&2
		rm -rf "$tmp"
		exit 1
	fi
	rm -rf "$tmp"
	echo "built: $saida/libzeebx_libretro.a"
}

if [[ "${1:-}" == "--local" ]]; then
	shift
	compilar_local "$@"
	exit 0
fi

SAIDA="${1:-$AQUI/saida}"
mkdir -p "$SAIDA" "$RAIZ/target"

# A imagem oficial é Ubuntu com o vitasdk, sem rustc. O stable entra no
# contêiner; o `rust-src` fica porque a std deste alvo não é publicada.
docker run --rm \
	-e HOME=/tmp \
	-e CARGO_HOME=/src/target/vita-cargo \
	-e RUSTUP_HOME=/src/target/vita-rustup \
	-e HOST_UID="$(id -u)" \
	-e HOST_GID="$(id -g)" \
	-v "$RAIZ:/src" \
	-w /src \
	"$IMAGEM" \
	bash -lc "set -euo pipefail
if ! command -v curl >/dev/null 2>&1 || ! command -v cc >/dev/null 2>&1 || ! command -v pkg-config >/dev/null 2>&1; then
  apt-get update
  apt-get install -y --no-install-recommends ca-certificates curl build-essential pkg-config
fi
# O rustup fica em CARGO_HOME, que não está no PATH da imagem. Sem ler o env,
# a segunda execução acharia que o rustc não existe e instalaria de novo.
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
./frontends/vita/compilar.sh --local /src/frontends/vita/saida
chown -R \"\${HOST_UID}:\${HOST_GID}\" /src/frontends/vita/saida /src/target/vita-cargo /src/target/vita-rustup /src/target/vita || true
"

if [[ "$SAIDA" != "$AQUI/saida" ]]; then
	cp -f "$AQUI/saida/libzeebx_libretro.a" "$SAIDA/"
	echo "built: $SAIDA/libzeebx_libretro.a"
fi
