#!/usr/bin/env bash
# Builds the core for Android and assembles the APK.
#
# Everything Android needs lives in $HOME, without sudo: see `frontends/android/README.md`.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# The SDK: the one in `$HOME`, or whatever the system already points to. `ANDROID_SDK_ROOT` is
# what the GitHub runners export, and accepting it is what lets this same script run in CI without
# a second copy of the paths inside the workflow.
: "${ANDROID_SDK_HOME:=${ANDROID_SDK_ROOT:-$HOME/Android/sdk}}"
# The NDK version. The workflows read the number from the `ANDROID_NDK_DEFAULT=` line below with
# grep, so it stays on a line of its own. `ANDROID_NDK_VERSAO` is the old name of the variable,
# still accepted.
ANDROID_NDK_DEFAULT=27.3.13750724
: "${ANDROID_NDK_VERSION:=${ANDROID_NDK_VERSAO:-$ANDROID_NDK_DEFAULT}}"
: "${ZEEBX_ANDROID_ABI:=arm64-v8a}"
: "${ZEEBX_ANDROID_PLATFORM:=android-35}"

export ANDROID_HOME="$ANDROID_SDK_HOME"
export ANDROID_NDK_HOME="$ANDROID_SDK_HOME/ndk/$ANDROID_NDK_VERSION"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
export ANDROID_NDK="$ANDROID_NDK_HOME"
export JAVA_HOME="${JAVA_HOME:-$HOME/Android/jdk}"
export ZEEBX_ANDROID_ABI ZEEBX_ANDROID_PLATFORM
export CMAKE_TOOLCHAIN_FILE="$HERE/ndk-toolchain.cmake"
export CMAKE_MAKE_PROGRAM="$(command -v ninja)"

cd "$ROOT"
JNI="$HERE/apk/app/src/main/jniLibs"
mkdir -p "$JNI"
cargo ndk -t "$ZEEBX_ANDROID_ABI" --platform "${ZEEBX_ANDROID_PLATFORM#android-}" \
  -o "$JNI" build --release -p zeebx-android

# The NDK's STL is a separate library, and dynarmic's C++ depends on it. Without this file inside
# the APK, `dlopen` fails on load with "library libc++_shared.so not found" and the activity dies
# before `android_main` runs.
SYSROOT="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib"
case "$ZEEBX_ANDROID_ABI" in
  arm64-v8a)   TRIPLE="aarch64-linux-android" ;;
  armeabi-v7a) TRIPLE="arm-linux-androideabi" ;;
  x86_64)      TRIPLE="x86_64-linux-android" ;;
  *) echo "ABI with no known triple: $ZEEBX_ANDROID_ABI" >&2; exit 1 ;;
esac
cp "$SYSROOT/$TRIPLE/libc++_shared.so" "$JNI/$ZEEBX_ANDROID_ABI/"

# The licenses go inside the APK, in `assets/licenses`: the GPL and the libraries' licenses ask
# for the text to accompany the binary. Gradle packages `src/main/assets` on its own.
LICENSES="$HERE/apk/app/src/main/assets/licenses"
mkdir -p "$LICENSES"
cp "$ROOT/LICENSE" "$ROOT/LICENSE-GPL3" "$ROOT/THIRD-PARTY-NOTICES.txt" "$LICENSES/"

echo "== .so =="
ls -lh "$JNI/$ZEEBX_ANDROID_ABI/"

if [ "${1:-}" = "--apk" ]; then
  # The Gradle pointed to, whatever is on the path, or the one in `$HOME`. There is no wrapper in
  # this project: `apk/` is a minimal module that only packages the `.so`, and a versioned
  # `gradlew` would be one more thing to keep up to date than reading a variable.
  if [ -z "${GRADLE:-}" ]; then
    GRADLE="$(command -v gradle || echo "$HOME/Android/gradle/bin/gradle")"
  fi
  cd "$HERE/apk"
  "$GRADLE" --no-daemon assembleDebug

  # Gradle names by module and variant: `app-debug.apk`, the same in every project that started
  # from the wizard. Next to the desktop packages — `zeebx-standalone-…`, `zeebx-headless-…` — an
  # `app-debug.apk` doesn't say which program or which device it is for, so a copy comes out with
  # the name the release uses. Gradle's stays where it was: it is the one `gradle installDebug`
  # and Android Studio use.
  OUTPUTS="$HERE/apk/app/build/outputs/apk/debug"
  NAMED="$OUTPUTS/zeebx-android-$ZEEBX_ANDROID_ABI.apk"
  cp "$OUTPUTS/app-debug.apk" "$NAMED"

  echo "== APK =="
  ls -lh "$NAMED"
fi
