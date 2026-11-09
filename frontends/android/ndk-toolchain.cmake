# Wrapper around the NDK toolchain, for the dependencies that compile C and C++ through CMake —
# dynarmic.
#
# `cmake-rs` builds the command line on its own: it already puts `CMAKE_SYSTEM_NAME=Android` and
# `--target=aarch64-linux-android35` in the flags, but doesn't tell the NDK toolchain the ABI.
# Without it the NDK assumes `armeabi-v7a` and adds `-march=armv7-a`, which clang rejects next to
# an aarch64 target — the build dies in the compiler check, before any code of ours.
#
# Here the ABI and the API are pinned before the NDK toolchain is read. They are picked by
# `build.sh`, through environment variables.
set(ANDROID_ABI "$ENV{ZEEBX_ANDROID_ABI}" CACHE STRING "" FORCE)
set(ANDROID_PLATFORM "$ENV{ZEEBX_ANDROID_PLATFORM}" CACHE STRING "" FORCE)
include("$ENV{ANDROID_NDK_HOME}/build/cmake/android.toolchain.cmake")
