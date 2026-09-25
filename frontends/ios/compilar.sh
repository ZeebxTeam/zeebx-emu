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

# O zip que o CI publica. Não é o .app nu: quem baixa precisa do leia-me, e o
# .app do aparelho vai como IPA para o AltStore reassinar. Um .app na raiz do
# zip não cabe ao lado desses dois.
pacote() {
  local sim="$AQUI/build/iphonesimulator/Zeebx.app"
  local aparelho="$AQUI/build/iphoneos/Zeebx.app"
  local saida="${1:-$AQUI/build/zeebx-ios-simulator.zip}"
  local stage ipa_stage ipa
  if [ ! -d "$sim" ] || [ ! -d "$aparelho" ]; then
    echo "faltam os dois .app. Rode --app e --app-aparelho antes." >&2
    exit 1
  fi
  mkdir -p "$(dirname "$saida")"
  ipa="$(dirname "$saida")/zeebx-ios.ipa"
  stage="$(mktemp -d)"
  mkdir -p "$stage/simulador"
  ditto --norsrc "$sim" "$stage/simulador/Zeebx.app"
  # O IPA é um zip cuja raiz é Payload/, não o .app. O --keepParent é o que
  # põe essa pasta; sem ela o AltStore não reconhece o pacote.
  ipa_stage="$(mktemp -d)"
  mkdir -p "$ipa_stage/Payload"
  ditto --norsrc "$aparelho" "$ipa_stage/Payload/Zeebx.app"
  xattr -cr "$ipa_stage" "$stage/simulador"
  # O zip se chama simulador. O AltStore não abre o .app de dentro dele: instala
  # este IPA, que é o binário de aparelho. Ele sai também ao lado do zip, para
  # a release oferecer o arquivo direto.
  ditto -c -k --norsrc --keepParent "$ipa_stage/Payload" "$ipa"
  rm -rf "$ipa_stage"
  cp "$ipa" "$stage/zeebx-ios.ipa"
  cp "$AQUI/LEIA-ME.txt" "$stage/LEIA-ME.txt"
  rm -f "$saida"
  ditto -c -k --norsrc "$stage" "$saida"
  rm -rf "$stage"
  echo "== zip =="
  ls -lh "$saida" "$ipa"
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
  --pacote)
    pacote "${2:-$AQUI/build/zeebx-ios-simulator.zip}"
    ;;
  *)
    echo "uso: $0 [--simulador | --aparelho | --app | --app-aparelho | --pacote]" >&2
    exit 2
    ;;
esac
