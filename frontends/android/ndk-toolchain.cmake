# Invólucro do toolchain do NDK, para as dependências que compilam C e C++ pelo CMake — o
# dynarmic.
#
# O `cmake-rs` monta a linha de comando sozinho: ele já põe `CMAKE_SYSTEM_NAME=Android` e
# `--target=aarch64-linux-android35` nas flags, mas não diz a ABI ao toolchain do NDK. Sem ela
# o NDK assume `armeabi-v7a` e acrescenta `-march=armv7-a`, que o clang recusa junto de um alvo
# aarch64 — o build morre no teste do compilador, antes de qualquer código nosso.
#
# Aqui a ABI e a API são fixadas antes de o toolchain do NDK ser lido. Quem as escolhe é o
# `compilar.sh`, pelas variáveis de ambiente.
set(ANDROID_ABI "$ENV{ZEEBX_ANDROID_ABI}" CACHE STRING "" FORCE)
set(ANDROID_PLATFORM "$ENV{ZEEBX_ANDROID_PLATFORM}" CACHE STRING "" FORCE)
include("$ENV{ANDROID_NDK_HOME}/build/cmake/android.toolchain.cmake")
