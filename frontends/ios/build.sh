#!/usr/bin/env bash
# Builds the core for iOS and, with --app, assembles the simulator .app.
#
# Xcode calls this same script in the build phase (`--build-phase`), so there is no second copy
# of the targets inside the project. See `frontends/ios/README.md`.
#
# The Portuguese flags from before the translation (`--na-fase`, `--simulador`, `--aparelho`,
# `--app-aparelho`, `--pacote`) still work as aliases.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# The library always comes out in the repository's `target/`. An inherited `CARGO_TARGET_DIR` —
# from a sandbox, from a cache — would make `cp` look for the `.a` in one place and cargo write it
# in another.
export CARGO_TARGET_DIR="$ROOT/target"

export PATH="${HOME}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:${PATH}"
export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-16.0}"

# The `cargo` on the PATH may be Homebrew's, which only ships the Mac standard library. The iOS
# target lives in rustup. Picking the first one that has the `rustlib` avoids building with the
# wrong one and dying midway, with a "can't find crate for std" message.
pick_cargo() {
  local target="$1"
  local candidate bin sysroot
  for candidate in ${CARGO:-} cargo "${HOME}/.cargo/bin/cargo"; do
    [ -n "$candidate" ] || continue
    if ! command -v "$candidate" >/dev/null 2>&1; then
      continue
    fi
    # `cargo rustc --print` is not the sysroot: on stable `--print` is a cargo flag and it
    # refuses. The `rustc` next to `cargo` is the one that can tell where the standard library
    # is, and that is what has (or doesn't have) the iOS target.
    bin="$(cd "$(dirname "$(command -v "$candidate")")" && pwd)"
    if [ ! -x "$bin/rustc" ]; then
      continue
    fi
    # Xcode exports the simulator's `SDKROOT`. The host `rustc`, asked with that SDK, may refuse
    # `--print` before saying where it lives.
    sysroot="$(env -u SDKROOT "$bin/rustc" --print sysroot 2>/dev/null || true)"
    if [ -n "$sysroot" ] && [ -d "${sysroot}/lib/rustlib/${target}" ]; then
      echo "$bin/cargo"
      return 0
    fi
  done
  return 1
}

build_lib() {
  local target="$1"
  local sdk="$2"
  local cargo
  if ! cargo="$(pick_cargo "$target")"; then
    echo "there is no Rust standard library for ${target}." >&2
    echo "with rustup: rustup target add ${target}" >&2
    exit 1
  fi
  export SDKROOT="$(xcrun --sdk "$sdk" --show-sdk-path)"
  cd "$ROOT"
  "$cargo" build --release --locked -p zeebx-ios --target "$target"
  echo "$ROOT/target/${target}/release/libzeebx_ios.a"
}

copy_library() {
  local target="$1"
  local library="$ROOT/target/${target}/release/libzeebx_ios.a"
  if [ -n "${BUILT_PRODUCTS_DIR:-}" ]; then
    cp "$library" "${BUILT_PRODUCTS_DIR}/libzeebx_ios.a"
  fi
}

xcode() {
  local sdk="$1"
  local destination="$2"
  local extra=()
  if [ -n "${DEVELOPMENT_TEAM:-}" ]; then
    extra+=(DEVELOPMENT_TEAM="$DEVELOPMENT_TEAM")
  else
    # Without a development team the .app doesn't install on a device, but the link closes: that
    # is what CI checks. Whoever is going to install on the phone exports DEVELOPMENT_TEAM.
    extra+=(CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="")
  fi
  xcodebuild \
    -project "$HERE/app/Zeebx.xcodeproj" \
    -target Zeebx \
    -sdk "$sdk" \
    -configuration Release \
    -destination "generic/platform=${destination}" \
    ARCHS=arm64 \
    ONLY_ACTIVE_ARCH=YES \
    CONFIGURATION_BUILD_DIR="$HERE/build/${sdk}" \
    "${extra[@]}"
  echo "== .app =="
  ls -lh "$HERE/build/${sdk}/Zeebx.app"
}

# Two zips. Each one carries only what installs in that place, and the readme for that flow. The
# simulator one doesn't carry the IPA: whoever opens the wrong zip would install a binary that
# AltStore refuses, or the other way around.
package() {
  local sim="$HERE/build/iphonesimulator/Zeebx.app"
  local device="$HERE/build/iphoneos/Zeebx.app"
  local dir="${1:-$HERE/build}"
  local zip_sim="$dir/zeebx-ios-simulator.zip"
  local zip_device="$dir/zeebx-ios.zip"
  local stage ipa_stage
  if [ ! -d "$sim" ] || [ ! -d "$device" ]; then
    echo "both .app are missing. Run --app and --app-device first." >&2
    exit 1
  fi
  mkdir -p "$dir"
  # The old format left the IPA loose next to the zip. Whoever downloaded the release saw both and
  # didn't know which one opened where.
  rm -f "$dir/zeebx-ios.ipa"

  stage="$(mktemp -d)"
  mkdir -p "$stage/simulator"
  ditto --norsrc "$sim" "$stage/simulator/Zeebx.app"
  xattr -cr "$stage/simulator"
  cp "$HERE/README-simulator.txt" "$stage/README.txt"
  mkdir -p "$stage/licenses"
  cp "$ROOT/LICENSE" "$ROOT/LICENSE-GPL3" "$ROOT/THIRD-PARTY-NOTICES.txt" "$stage/licenses/"
  rm -f "$zip_sim"
  ditto -c -k --norsrc "$stage" "$zip_sim"
  rm -rf "$stage"

  # The IPA is a zip whose root is Payload/, not the .app. --keepParent is what puts that folder
  # there; without it AltStore doesn't recognize the package. This IPA goes inside the device zip,
  # next to the readme, and not loose.
  stage="$(mktemp -d)"
  ipa_stage="$(mktemp -d)"
  mkdir -p "$ipa_stage/Payload"
  ditto --norsrc "$device" "$ipa_stage/Payload/Zeebx.app"
  xattr -cr "$ipa_stage"
  ditto -c -k --norsrc --keepParent "$ipa_stage/Payload" "$stage/zeebx-ios.ipa"
  rm -rf "$ipa_stage"
  cp "$HERE/README-altstore.txt" "$stage/README.txt"
  mkdir -p "$stage/licenses"
  cp "$ROOT/LICENSE" "$ROOT/LICENSE-GPL3" "$ROOT/THIRD-PARTY-NOTICES.txt" "$stage/licenses/"
  rm -f "$zip_device"
  ditto -c -k --norsrc "$stage" "$zip_device"
  rm -rf "$stage"

  echo "== zip =="
  ls -lh "$zip_sim" "$zip_device"
}

mode="${1:---simulator}"
case "$mode" in
  --build-phase|--na-fase)
    case "${PLATFORM_NAME:-}" in
      iphonesimulator) target=aarch64-apple-ios-sim; sdk=iphonesimulator ;;
      iphoneos) target=aarch64-apple-ios; sdk=iphoneos ;;
      *) echo "PLATFORM_NAME with no target: ${PLATFORM_NAME:-empty}" >&2; exit 1 ;;
    esac
    build_lib "$target" "$sdk"
    copy_library "$target"
    ;;
  --simulator|--simulador|"")
    build_lib aarch64-apple-ios-sim iphonesimulator
    ;;
  --device|--aparelho)
    build_lib aarch64-apple-ios iphoneos
    ;;
  --app)
    xcode iphonesimulator "iOS Simulator"
    ;;
  --app-device|--app-aparelho)
    xcode iphoneos iOS
    ;;
  --package|--pacote)
    package "${2:-$HERE/build}"
    ;;
  *)
    echo "usage: $0 [--simulator | --device | --app | --app-device | --package]" >&2
    exit 2
    ;;
esac
