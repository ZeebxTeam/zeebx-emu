#!/usr/bin/env bash
# Compila o núcleo para o iOS e, com --app, monta o .app do simulador.
#
# O Xcode chama este mesmo script na fase de build (`--na-fase`), para não haver uma segunda
# cópia dos alvos dentro do projeto. Ver `frontends/ios/LEIAME.md`.
set -euo pipefail

AQUI="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RAIZ="$(cd "$AQUI/../.." && pwd)"
# A biblioteca sai sempre em `target/` do repositório. Um `CARGO_TARGET_DIR` herdado — de um
# sandbox, de um cache — faria o `cp` procurar o `.a` num lugar e o cargo gravá-lo noutro.
export CARGO_TARGET_DIR="$RAIZ/target"

export PATH="${HOME}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:${PATH}"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-16.0}"

# O `cargo` do PATH pode ser o do Homebrew, que só traz a biblioteca padrão do Mac. O alvo de
# iOS mora no rustup. Escolher o primeiro que tenha o `rustlib` evita compilar com o errado e
# morrer no meio, numa mensagem de "can't find crate for std".
escolhe_cargo() {
  local alvo="$1"
  local candidato bin sysroot
  for candidato in ${CARGO:-} cargo "${HOME}/.cargo/bin/cargo"; do
    [ -n "$candidato" ] || continue
    if ! command -v "$candidato" >/dev/null 2>&1; then
      continue
    fi
    # `cargo rustc --print` não é o sysroot: no stable o `--print` é flag do cargo e
    # recusa. O `rustc` ao lado do `cargo` é quem sabe dizer onde está a biblioteca
    # padrão, e é ela que tem (ou não) o alvo de iOS.
    bin="$(cd "$(dirname "$(command -v "$candidato")")" && pwd)"
    if [ ! -x "$bin/rustc" ]; then
      continue
    fi
    # O Xcode exporta `SDKROOT` do simulador. O `rustc` do host, perguntado com esse
    # SDK, pode recusar o `--print` antes de dizer onde mora.
    sysroot="$(env -u SDKROOT "$bin/rustc" --print sysroot 2>/dev/null || true)"
    if [ -n "$sysroot" ] && [ -d "${sysroot}/lib/rustlib/${alvo}" ]; then
      echo "$bin/cargo"
      return 0
    fi
  done
  return 1
}

compila() {
  local alvo="$1"
  local sdk="$2"
  local cargo
  if ! cargo="$(escolhe_cargo "$alvo")"; then
    echo "não há biblioteca padrão de Rust para ${alvo}." >&2
    echo "com rustup: rustup target add ${alvo}" >&2
    exit 1
  fi
  export SDKROOT="$(xcrun --sdk "$sdk" --show-sdk-path)"
  cd "$RAIZ"
  "$cargo" build --release --locked -p zeebx-ios --target "$alvo"
  echo "$RAIZ/target/${alvo}/release/libzeebx_ios.a"
}

copia_biblioteca() {
  local alvo="$1"
  local biblioteca="$RAIZ/target/${alvo}/release/libzeebx_ios.a"
  if [ -n "${BUILT_PRODUCTS_DIR:-}" ]; then
    cp "$biblioteca" "${BUILT_PRODUCTS_DIR}/libzeebx_ios.a"
  fi
}

xcode() {
  local sdk="$1"
  local destino="$2"
  local extra=()
  if [ -n "${DEVELOPMENT_TEAM:-}" ]; then
    extra+=(DEVELOPMENT_TEAM="$DEVELOPMENT_TEAM")
  else
    # Sem time de desenvolvimento o .app não instala num aparelho, mas o link fecha: é o que
    # o CI confere. Quem vai instalar no telefone exporta DEVELOPMENT_TEAM.
    extra+=(CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="")
  fi
  xcodebuild \
    -project "$AQUI/app/Zeebx.xcodeproj" \
    -target Zeebx \
    -sdk "$sdk" \
    -configuration Release \
    -destination "generic/platform=${destino}" \
    ARCHS=arm64 \
    ONLY_ACTIVE_ARCH=YES \
    CONFIGURATION_BUILD_DIR="$AQUI/build/${sdk}" \
    "${extra[@]}"
  echo "== .app =="
  ls -lh "$AQUI/build/${sdk}/Zeebx.app"
}

caso="${1:---simulador}"
case "$caso" in
  --na-fase)
    case "${PLATFORM_NAME:-}" in
      iphonesimulator) alvo=aarch64-apple-ios-sim; sdk=iphonesimulator ;;
      iphoneos) alvo=aarch64-apple-ios; sdk=iphoneos ;;
      *) echo "PLATFORM_NAME sem alvo: ${PLATFORM_NAME:-vazia}" >&2; exit 1 ;;
    esac
    compila "$alvo" "$sdk"
    copia_biblioteca "$alvo"
    ;;
  --simulador|"")
    compila aarch64-apple-ios-sim iphonesimulator
    ;;
  --aparelho)
    compila aarch64-apple-ios iphoneos
    ;;
  --app)
    xcode iphonesimulator "iOS Simulator"
    ;;
  --app-aparelho)
    xcode iphoneos iOS
    ;;
  *)
    echo "uso: $0 [--simulador | --aparelho | --app | --app-aparelho]" >&2
    exit 2
    ;;
esac
