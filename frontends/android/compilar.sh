#!/usr/bin/env bash
# Compila o núcleo para o Android e monta a APK.
#
# Tudo o que o Android precisa mora no $HOME, sem sudo: ver `frontends/android/LEIAME.md`.
set -euo pipefail

AQUI="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RAIZ="$(cd "$AQUI/../.." && pwd)"
# O SDK: o do `$HOME`, ou o que o sistema já apontar. O `ANDROID_SDK_ROOT` é o que os runners
# do GitHub exportam, e aceitá-lo é o que permite este mesmo script rodar no CI sem uma segunda
# cópia dos caminhos dentro do workflow.
: "${ANDROID_SDK_HOME:=${ANDROID_SDK_ROOT:-$HOME/Android/sdk}}"
: "${ANDROID_NDK_VERSAO:=27.3.13750724}"
: "${ZEEBX_ANDROID_ABI:=arm64-v8a}"
: "${ZEEBX_ANDROID_PLATFORM:=android-35}"

export ANDROID_HOME="$ANDROID_SDK_HOME"
export ANDROID_NDK_HOME="$ANDROID_SDK_HOME/ndk/$ANDROID_NDK_VERSAO"
export ANDROID_NDK_ROOT="$ANDROID_NDK_HOME"
export ANDROID_NDK="$ANDROID_NDK_HOME"
export JAVA_HOME="${JAVA_HOME:-$HOME/Android/jdk}"
export ZEEBX_ANDROID_ABI ZEEBX_ANDROID_PLATFORM
export CMAKE_TOOLCHAIN_FILE="$AQUI/ndk-toolchain.cmake"
export CMAKE_MAKE_PROGRAM="$(command -v ninja)"

cd "$RAIZ"
JNI="$AQUI/apk/app/src/main/jniLibs"
mkdir -p "$JNI"
cargo ndk -t "$ZEEBX_ANDROID_ABI" --platform "${ZEEBX_ANDROID_PLATFORM#android-}" \
  -o "$JNI" build --release -p zeebx-android

# A STL do NDK é uma biblioteca à parte, e o C++ do dynarmic depende dela. Sem este arquivo
# dentro da APK o `dlopen` falha na abertura com "library libc++_shared.so not found" e a
# atividade morre antes de o `android_main` rodar.
SYSROOT="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib"
case "$ZEEBX_ANDROID_ABI" in
  arm64-v8a)   TRIPLA="aarch64-linux-android" ;;
  armeabi-v7a) TRIPLA="arm-linux-androideabi" ;;
  x86_64)      TRIPLA="x86_64-linux-android" ;;
  *) echo "ABI sem tripla conhecida: $ZEEBX_ANDROID_ABI" >&2; exit 1 ;;
esac
cp "$SYSROOT/$TRIPLA/libc++_shared.so" "$JNI/$ZEEBX_ANDROID_ABI/"

echo "== .so =="
ls -lh "$JNI/$ZEEBX_ANDROID_ABI/"

if [ "${1:-}" = "--apk" ]; then
  # O Gradle apontado, o que estiver no caminho, ou o do `$HOME`. Não há wrapper neste projeto:
  # o `apk/` é um módulo mínimo que só empacota o `.so`, e um `gradlew` versionado seria mais
  # uma coisa a manter atualizada do que a leitura de uma variável.
  if [ -z "${GRADLE:-}" ]; then
    GRADLE="$(command -v gradle || echo "$HOME/Android/gradle/bin/gradle")"
  fi
  cd "$AQUI/apk"
  "$GRADLE" --no-daemon assembleDebug

  # O Gradle nomeia pelo módulo e pela variante: `app-debug.apk`, igual em todo projeto que
  # começou pelo assistente. Ao lado dos pacotes de desktop — `zeebx-standalone-…`,
  # `zeebx-headless-…` — um `app-debug.apk` não diz de qual programa nem de qual aparelho é,
  # então sai uma cópia com o nome que a release usa. A do Gradle fica onde estava: é dela que
  # o `gradle installDebug` e o Android Studio se servem.
  SAIDA="$AQUI/apk/app/build/outputs/apk/debug"
  NOMEADA="$SAIDA/zeebx-android-$ZEEBX_ANDROID_ABI.apk"
  cp "$SAIDA/app-debug.apk" "$NOMEADA"

  echo "== APK =="
  ls -lh "$NOMEADA"
fi
