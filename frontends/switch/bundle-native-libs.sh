#!/usr/bin/env bash
# rustc --crate-type staticlib archives the crate and native libs marked
# static= (cc's wrapper). CMake libraries linked as plain
# cargo:rustc-link-lib=dynarmic stay outside the archive. Copy their objects
# in so one .a is enough for the Switch link.
set -euo pipefail

ar_bin=ar
if [[ -x /opt/devkitpro/devkitA64/bin/aarch64-none-elf-gcc-ar ]]; then
	ar_bin=/opt/devkitpro/devkitA64/bin/aarch64-none-elf-gcc-ar
fi

# Only the archive for this target. A host dynarmic left under target/release
# is x86_64 and must not be folded into the Switch staticlib.
natives=()
while IFS= read -r -d '' lib; do
	natives+=("${lib}")
done < <(find target/aarch64-unknown-linux-gnu/release -type f -path '*/out/lib/*.a' -print0 | sort -z)

if [[ ${#natives[@]} -eq 0 ]]; then
	exit 0
fi

arts=()
while IFS= read -r -d '' art; do
	arts+=("${art}")
done < <(find . -type f -name '*_libretro*.a' ! -path '*/out/*' -print0)

if [[ ${#arts[@]} -eq 0 ]]; then
	echo "bundle-native-libs: no libretro archive to extend" >&2
	exit 1
fi

for art in "${arts[@]}"; do
	tmp="$(mktemp -d)"
	i=0
	added=0
	for lib in "${natives[@]}"; do
		sub="${tmp}/l${i}"
		mkdir -p "${sub}"
		# Paths are relative to the core tree. Extract from here so ar can open them.
		"${ar_bin}" x --output "${sub}" "${lib}"
		renamed=()
		for obj in "${sub}"/*; do
			[[ -f "${obj}" ]] || continue
			dest="${sub}/n${i}_$(basename "${obj}")"
			mv "${obj}" "${dest}"
			renamed+=("${dest}")
		done
		if [[ ${#renamed[@]} -gt 0 ]]; then
			"${ar_bin}" r "${art}" "${renamed[@]}"
			added=$((added + ${#renamed[@]}))
		fi
		i=$((i + 1))
	done
	"${ar_bin}" s "${art}" >/dev/null
	rm -rf "${tmp}"
	echo "bundled ${added} objects from ${#natives[@]} native libs into ${art}"
done
