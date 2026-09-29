# Cross toolchain for cargo crates that invoke CMake (dynarmic) while
# targeting aarch64-unknown-linux-gnu inside the devkitA64 image.
set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR aarch64)

set(CMAKE_C_COMPILER /opt/devkitpro/devkitA64/bin/aarch64-none-elf-gcc)
set(CMAKE_CXX_COMPILER /opt/devkitpro/devkitA64/bin/aarch64-none-elf-g++)
set(CMAKE_ASM_COMPILER /opt/devkitpro/devkitA64/bin/aarch64-none-elf-gcc)
set(CMAKE_AR /opt/devkitpro/devkitA64/bin/aarch64-none-elf-gcc-ar CACHE FILEPATH "" FORCE)

# dynarmic turns tests on when it is the top-level project. Catch2 does not
# build with this C++ library, and the core does not need the test binary.
set(BUILD_TESTING OFF CACHE BOOL "" FORCE)
set(DYNARMIC_TESTS OFF CACHE BOOL "" FORCE)
set(DYNARMIC_USE_PRECOMPILED_HEADERS OFF CACHE BOOL "" FORCE)
# Horizon requires W^X. Dynarmic's default RWX code pages are not mappable.
set(DYNARMIC_ENABLE_NO_EXECUTE_SUPPORT ON CACHE BOOL "" FORCE)

# newlib hides fileno, fdopen, and putc_unlocked unless a feature macro is set.
# {fmt} calls all three. Applied after project(), so it is not overwritten by
# cmake-rs passing -DCMAKE_CXX_FLAGS.
set(CMAKE_PROJECT_INCLUDE "${CMAKE_CURRENT_LIST_DIR}/aarch64-none-elf-flags.cmake")
