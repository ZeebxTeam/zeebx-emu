#!/usr/bin/env bash
# Biblioteca estática do core para o Retroarch Web (`emcc -sSIDE_MODULE=1`).
#
# `-fPIC` deixa o sqlite relocável; sem isso o `wasm-ld -shared` recusa o objeto.
# `panic=abort` evita o unwind: o módulo principal não exporta `_Unwind_RaiseException`.
# O `sevenz-rust2` em wasm32 puxa `wasm-bindgen`; o Retroarch Web não tem esses imports,
# então essa ponte JS fica de fora. O `.7z` abre por `ArchiveReader::new`.
set -euo pipefail

raiz="$(cd "$(dirname "$0")/.." && pwd)"
cd "$raiz"

if [[ -z "${EMSDK:-}" || ! -f "${EMSDK}/emsdk_env.sh" ]]; then
  for candidato in "$raiz/../emsdk" "$HOME/emsdk"; do
    if [[ -f "$candidato/emsdk_env.sh" ]]; then
      EMSDK="$candidato"
      break
    fi
  done
fi
if [[ ! -f "${EMSDK:-}/emsdk_env.sh" ]]; then
  echo "não achei o emsdk. Defina EMSDK com o diretório que contém emsdk_env.sh." >&2
  exit 1
fi
# shellcheck disable=SC1091
source "$EMSDK/emsdk_env.sh"

export CC_wasm32_unknown_emscripten="${CC_wasm32_unknown_emscripten:-emcc}"
export AR_wasm32_unknown_emscripten="${AR_wasm32_unknown_emscripten:-emar}"
export CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_LINKER="${CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_LINKER:-emcc}"
export CFLAGS_wasm32_unknown_emscripten="-fPIC ${CFLAGS_wasm32_unknown_emscripten:-}"
# O ligador embutido do rustup não vem no Homebrew; o emcc do emsdk faz o link.
export CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_RUSTFLAGS="-C link-self-contained=no -C panic=abort ${CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_RUSTFLAGS:-}"

if [[ ! -d "$(rustc --print sysroot)/lib/rustlib/src/rust/library/std" ]]; then
  echo "precisa do componente rust-src: a std do Wasm é recompilada com panic=abort." >&2
  exit 1
fi
export RUSTC_BOOTSTRAP=1

# O fonte do sevenz tem de estar no registro antes do patch. No CI ele ainda não foi baixado.
cargo fetch --locked

vendor="$raiz/target/vendor/sevenz-rust2"
if [[ ! -f "$vendor/.sem-js" ]]; then
  origem=""
  for base in "${CARGO_HOME:-$HOME/.cargo}/registry/src"/*; do
    if [[ -d "$base/sevenz-rust2-0.23.0" ]]; then
      origem="$base/sevenz-rust2-0.23.0"
      break
    fi
  done
  if [[ -z "$origem" ]]; then
    echo "não achei sevenz-rust2-0.23.0 no registro do cargo." >&2
    exit 1
  fi
  rm -rf "$vendor"
  mkdir -p "$(dirname "$vendor")"
  cp -a "$origem" "$vendor"
  # O cfg do wasm-bindgen fica na linha de cima. Sem o lookahead o sed troca o arquivo errado.
  perl -i -0777 -pe '
    s/#\[cfg\(target_arch = "wasm32"\)\]\nextern crate wasm_bindgen;/#\[cfg(all(target_arch = "wasm32", not(target_os = "emscripten")))]\nextern crate wasm_bindgen;/;
    s/#\[cfg\(all\(feature = "util", target_arch = "wasm32"\)\)\]\npub use util::wasm::\*;/#\[cfg(all(feature = "util", target_arch = "wasm32", not(target_os = "emscripten")))]\npub use util::wasm::*;/;
  ' "$vendor/src/lib.rs"
  perl -i -0777 -pe '
    s/#\[cfg\(target_arch = "wasm32"\)\]\npub\(crate\) mod wasm;/#\[cfg(all(target_arch = "wasm32", not(target_os = "emscripten")))]\npub(crate) mod wasm;/;
  ' "$vendor/src/util.rs"
  touch "$vendor/.sem-js"
fi

# O patch não entra no Cargo.lock do repositório.
trava=$(mktemp)
cp "$raiz/Cargo.lock" "$trava"
status=0
cargo rustc -Z build-std=std,panic_abort --release \
  --config "patch.crates-io.sevenz-rust2.path=\"$vendor\"" \
  --target wasm32-unknown-emscripten \
  -p zeebx-libretro \
  --crate-type staticlib || status=$?
cp "$trava" "$raiz/Cargo.lock"
rm -f "$trava"
[[ $status -eq 0 ]]

pacote="$raiz/target/wasm"
mkdir -p "$pacote"
cp "${CARGO_TARGET_DIR:-$raiz/target}/wasm32-unknown-emscripten/release/libzeebx_libretro.a" \
  "$pacote/zeebx_libretro_emscripten.a"
cp "$raiz/frontends/libretro/zeebx_libretro.info" "$pacote/"

# O `.a` não carrega no navegador. O Retroarch Web faz `dlopen` de um side module.
emcc -sSIDE_MODULE=1 -O2 \
  -o "$pacote/zeebx_libretro_emscripten.wasm" \
  -Wl,--whole-archive "$pacote/zeebx_libretro_emscripten.a" -Wl,--no-whole-archive
echo "$pacote/zeebx_libretro_emscripten.wasm"
