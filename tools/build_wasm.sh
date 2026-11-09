#!/usr/bin/env bash
# Static library of the core for Retroarch Web (`emcc -sSIDE_MODULE=1`).
#
# `-fPIC` makes sqlite relocatable; without it `wasm-ld -shared` rejects the object.
# `panic=abort` avoids unwinding: the main module doesn't export `_Unwind_RaiseException`.
# `sevenz-rust2` on wasm32 pulls in `wasm-bindgen`; Retroarch Web doesn't have those imports,
# so that JS bridge stays out. The `.7z` opens through `ArchiveReader::new`.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if [[ -z "${EMSDK:-}" || ! -f "${EMSDK}/emsdk_env.sh" ]]; then
  for candidate in "$root/../emsdk" "$HOME/emsdk"; do
    if [[ -f "$candidate/emsdk_env.sh" ]]; then
      EMSDK="$candidate"
      break
    fi
  done
fi
if [[ ! -f "${EMSDK:-}/emsdk_env.sh" ]]; then
  echo "could not find emsdk. Set EMSDK to the directory that contains emsdk_env.sh." >&2
  exit 1
fi
# shellcheck disable=SC1091
source "$EMSDK/emsdk_env.sh"

export CC_wasm32_unknown_emscripten="${CC_wasm32_unknown_emscripten:-emcc}"
export AR_wasm32_unknown_emscripten="${AR_wasm32_unknown_emscripten:-emar}"
export CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_LINKER="${CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_LINKER:-emcc}"
export CFLAGS_wasm32_unknown_emscripten="-fPIC ${CFLAGS_wasm32_unknown_emscripten:-}"
# rustup's bundled linker doesn't come with Homebrew; emsdk's emcc does the linking.
export CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_RUSTFLAGS="-C link-self-contained=no -C panic=abort ${CARGO_TARGET_WASM32_UNKNOWN_EMSCRIPTEN_RUSTFLAGS:-}"

if [[ ! -d "$(rustc --print sysroot)/lib/rustlib/src/rust/library/std" ]]; then
  echo "the rust-src component is required: the Wasm std is rebuilt with panic=abort." >&2
  exit 1
fi
export RUSTC_BOOTSTRAP=1

# The sevenz source has to be in the registry before the patch. On CI it hasn't been
# downloaded yet.
cargo fetch --locked

vendor="$root/target/vendor/sevenz-rust2"
if [[ ! -f "$vendor/.no-js" ]]; then
  src_dir=""
  for base in "${CARGO_HOME:-$HOME/.cargo}/registry/src"/*; do
    if [[ -d "$base/sevenz-rust2-0.23.0" ]]; then
      src_dir="$base/sevenz-rust2-0.23.0"
      break
    fi
  done
  if [[ -z "$src_dir" ]]; then
    echo "could not find sevenz-rust2-0.23.0 in the cargo registry." >&2
    exit 1
  fi
  rm -rf "$vendor"
  mkdir -p "$(dirname "$vendor")"
  cp -a "$src_dir" "$vendor"
  # The wasm-bindgen cfg sits on the line above. Without the lookahead the sed changes the
  # wrong file.
  perl -i -0777 -pe '
    s/#\[cfg\(target_arch = "wasm32"\)\]\nextern crate wasm_bindgen;/#\[cfg(all(target_arch = "wasm32", not(target_os = "emscripten")))]\nextern crate wasm_bindgen;/;
    s/#\[cfg\(all\(feature = "util", target_arch = "wasm32"\)\)\]\npub use util::wasm::\*;/#\[cfg(all(feature = "util", target_arch = "wasm32", not(target_os = "emscripten")))]\npub use util::wasm::*;/;
  ' "$vendor/src/lib.rs"
  perl -i -0777 -pe '
    s/#\[cfg\(target_arch = "wasm32"\)\]\npub\(crate\) mod wasm;/#\[cfg(all(target_arch = "wasm32", not(target_os = "emscripten")))]\npub(crate) mod wasm;/;
  ' "$vendor/src/util.rs"
  touch "$vendor/.no-js"
fi

# The patch doesn't go into the repository's Cargo.lock.
lock_copy=$(mktemp)
cp "$root/Cargo.lock" "$lock_copy"
status=0
cargo rustc -Z build-std=std,panic_abort --release \
  --config "patch.crates-io.sevenz-rust2.path=\"$vendor\"" \
  --target wasm32-unknown-emscripten \
  -p zeebx-libretro \
  --crate-type staticlib || status=$?
cp "$lock_copy" "$root/Cargo.lock"
rm -f "$lock_copy"
[[ $status -eq 0 ]]

out_dir="$root/target/wasm"
mkdir -p "$out_dir"
cp "${CARGO_TARGET_DIR:-$root/target}/wasm32-unknown-emscripten/release/libzeebx_libretro.a" \
  "$out_dir/zeebx_libretro_emscripten.a"
cp "$root/frontends/libretro/zeebx_libretro.info" "$out_dir/"

# The `.a` doesn't load in the browser. Retroarch Web does a `dlopen` of a side module.
emcc -sSIDE_MODULE=1 -O2 \
  -o "$out_dir/zeebx_libretro_emscripten.wasm" \
  -Wl,--whole-archive "$out_dir/zeebx_libretro_emscripten.a" -Wl,--no-whole-archive
echo "$out_dir/zeebx_libretro_emscripten.wasm"
