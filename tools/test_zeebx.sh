#!/bin/sh
# Measures Zeebx on the device and gathers everything into a log bundle.
#
# Usage: sh test_zeebx.sh [seconds_per_run] [roms_folder]
#        sh test_zeebx.sh 20 /roms/zeebo
#
# Each run plays a fixed number of frames and measures the wall-clock time: real frames per
# second. The log of each run is kept alongside, and the summary says, per run, WHICH setting the
# core applied -- so the result doesn't depend on believing the option took.
#
# The libretro option keys and values written below (`zeebx_perfil = "portatil"`,
# `zeebx_log = "informacao"`...) and the core log lines grepped in `check()` are the core's
# own text: they are matched as data and stay exactly as the core writes them.

set -u

SECS=${1:-20}
ROMDIR=${2:-}
HERE=$(cd "$(dirname "$0")" && pwd)
OUT="$HERE/logs-zeebx-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$OUT"
SUMMARY="$OUT/summary.txt"
: > "$SUMMARY"

say() { printf '%s\n' "$*" | tee -a "$SUMMARY"; }

# ------------------------------------------------------------------ find retroarch and the core

RA=$(command -v retroarch || true)
[ -n "$RA" ] || RA=$(ls /usr/local/bin/retroarch /usr/bin/retroarch 2>/dev/null | head -1)
[ -n "$RA" ] || { say "RETROARCH: could not find the executable"; exit 1; }

# **muOS needs a preparation that its launcher does and doesn't pass on.**
# `/opt/muos/script/launch/lr-general.sh` sets up the SDL environment and `HOME` before calling
# RetroArch, and **passes on no arguments at all** -- without that, `--max-frames` never gets
# there. So the script does the same preparation and calls the binary directly.
if [ -f /opt/muos/script/var/func.sh ]; then
    set +u
    . /opt/muos/script/var/func.sh 2>/dev/null || true
    command -v SETUP_SDL_ENVIRONMENT >/dev/null 2>&1 && SETUP_SDL_ENVIRONMENT >/dev/null 2>&1
    new_home=$(GET_VAR "device" "board/home" 2>/dev/null || true)
    [ -n "${new_home:-}" ] && HOME="$new_home" && export HOME
    set -u
fi

# The context's `HOME` isn't reliable (EmulationStation may run as root), so the ArkOS and muOS
# paths go into the list -- including the muOS one, which is a symlink to
# `/opt/muos/share/info/config`, where muOS keeps each core's options.
# **All** the plausible directories, and not just the first: ArkOS keeps the options in a global
# file and muOS per core, in different folders, and the script has no way of knowing which of the
# two applies without trying. Writing to all of them is what makes the test valid on both
# devices; if none accepts it, the summary warns instead of putting out a result that compares
# nothing.
DIRS=""
for c in "$HOME/.config/retroarch" "$HOME/RetroArch" /home/ark/.config/retroarch \
         /opt/muos/share/info/config /opt/muos/share/emulator/retroarch; do
    [ -d "$c" ] || continue
    r=$(readlink -f "$c" 2>/dev/null || echo "$c")
    case " $DIRS " in *" $r "*) ;; *) DIRS="$DIRS $r" ;; esac
done
CFG=$(echo "$DIRS" | awk '{print $1}')
[ -n "$CFG" ] || CFG="$HOME/.config/retroarch"

CORE=""
for c in "$HERE/zeebx_libretro.so" "$CFG/cores/zeebx_libretro.so" \
         "/opt/muos/share/core/zeebx_libretro.so" /usr/lib/libretro/zeebx_libretro.so; do
    [ -f "$c" ] && CORE="$c" && break
done
[ -n "$CORE" ] || { say "CORE: could not find zeebx_libretro.so"; exit 1; }

# On muOS the games go on the **ROMs card** (SD2), and the content system is a unionfs: what
# counts is `/mnt/union/ROMS`, which joins USB, SDCARD and the system card in that order. Looking
# through the union before the raw paths is what finds the games in both arrangements -- one card
# or two.
if [ -z "$ROMDIR" ]; then
    for d in /roms/zeebo /roms/Zeebo /roms/ROMS/Zeebo \
             /mnt/union/ROMS/Zeebo /mnt/union/ROMS/zeebo \
             /mnt/sdcard/ROMS/Zeebo /mnt/mmc/ROMS/Zeebo "$HOME/roms/zeebo"; do
        [ -d "$d" ] && ROMDIR="$d" && break
    done
fi
# Last resort: look for a Zeebo folder on any mounted card. Better to find it than to say there
# is no game with the folder right there.
if [ -z "$ROMDIR" ]; then
    ROMDIR=$(ls -d /mnt/*/ROMS/Zeebo /mnt/*/ROMS/zeebo /mnt/*/Zeebo 2>/dev/null | head -1)
fi
[ -n "$ROMDIR" ] || { say "ROMS: could not find the folder; pass it as the second argument"; exit 1; }

# ------------------------------------------------------------------ time, with or without nanos

if [ "$(date +%N 2>/dev/null | wc -c)" -ge 10 ]; then
    now_ms() { date +%s%N | cut -c1-13; }
else
    now_ms() { echo $(( $(date +%s) * 1000 )); }
fi

# ------------------------------------------------------------------ the core options

# **Zeebx's options live in two files, and the device uses the global one.** With
# `global_core_options = true` (the ArkOS default) RetroArch reads and writes everything in
# `retroarch-core-options.cfg`; the per-core `.opt` only applies with that option off. Writing to
# both is what makes the test valid in both arrangements -- and, if only one applied, the runs
# would all come out the same and the comparison would say nothing.
BEFORE="$OUT/options-before"
NEW="$OUT/zeebx-for-the-run.cfg"

save_options() {
    mkdir -p "$BEFORE"
    n=0
    for d in $DIRS; do
        n=$((n + 1))
        [ -f "$d/retroarch-core-options.cfg" ] && cp "$d/retroarch-core-options.cfg" "$BEFORE/global-$n.cfg"
        [ -f "$d/config/Zeebx/Zeebx.opt" ] && cp "$d/config/Zeebx/Zeebx.opt" "$BEFORE/core-$n.opt"
    done
    return 0
}
restore_options() {
    n=0
    for d in $DIRS; do
        n=$((n + 1))
        [ -f "$BEFORE/global-$n.cfg" ] && cp "$BEFORE/global-$n.cfg" "$d/retroarch-core-options.cfg"
        if [ -f "$BEFORE/core-$n.opt" ]; then cp "$BEFORE/core-$n.opt" "$d/config/Zeebx/Zeebx.opt"; fi
    done
    return 0
}

put() { printf '%s = "%s"\n' "$1" "$2" >> "$NEW"; }

# `log = informacao` (info) in every run on purpose: it is the level at which the core says which
# rasterizer went in and whether tile discard stayed on. Without it, the summary would have no
# way to prove the run's setting took.
# **Each key goes out only once.** The first version wrote the default and then the choice on top,
# and RetroArch keeps the first value it reads: the run with tile discard on ran with it off, and
# the whole test would measure nothing.
options() {
    profile=padrao; rast=auto; scale=1; tiles=desligado; skip=desligado; fps=desligado; level=informacao
    for choice in $1; do
        case "$choice" in
            handheld)  profile=portatil ;;
            tiles)     tiles=ligado ;;
            software)  rast=software ;;
            debug)     level=depuracao ;;
        esac
    done
    : > "$NEW"
    put zeebx_perfil "$profile"
    put zeebx_rasterizador "$rast"
    put zeebx_resolucao_interna "$scale"
    put zeebx_descarte_de_tiles "$tiles"
    put zeebx_frameskip "$skip"
    put zeebx_limite_fps "$fps"
    put zeebx_log "$level"
    # In each directory found: the global one (preserving the other cores' options, since the
    # file keeps them all) and Zeebx's `.opt`.
    wrote=0
    for d in $DIRS; do
        g="$d/retroarch-core-options.cfg"
        if [ -f "$g" ]; then
            { grep -v '^zeebx_' "$g" 2>/dev/null; cat "$NEW"; } > "$OUT/global.tmp"
            cp "$OUT/global.tmp" "$g" 2>/dev/null && wrote=1
            rm -f "$OUT/global.tmp"
        fi
        mkdir -p "$d/config/Zeebx" 2>/dev/null
        cp "$NEW" "$d/config/Zeebx/Zeebx.opt" 2>/dev/null && wrote=1
    done
    if [ "$wrote" = 0 ]; then
        say "WARNING: could not write the options to any destination ($DIRS)."
        say "         Without that, every run uses the factory setting and the test compares nothing."
    fi
}

# ------------------------------------------------------------------ what the core said

check() {
    log=$1
    # **The last line, and not the first.** The core applies the options at startup (with the
    # defaults) and only then the run's choice: taking the first occurrence would say "off" in
    # every run.
    rast=$(grep -o 'drawing on the GPU\|rasterizer pinned to the processor' "$log" 2>/dev/null | tail -1)
    tiles=$(grep -o 'after the frame: [a-z]*' "$log" 2>/dev/null | tail -1 | sed 's/.*: //')
    [ -n "$tiles" ] || tiles='(not said)'
    gpu=$(grep -o 'GL real vendor=[^;]*' "$log" 2>/dev/null | tail -1 | sed 's/GL real vendor=//' | cut -c1-40)
    size=$(grep -m1 -o "frame [0-9]*x[0-9]*, outside the console's 640x480" "$log" 2>/dev/null)
    blit=$(grep -m1 -c 'does not return what it was asked to copy' "$log" 2>/dev/null)
    printf '    setting: %s | tiles: %s | gpu: %s | wrong frame: %s | blit failed: %s\n' \
        "${rast:-?}" "$tiles" "${gpu:-?}" "${size:-no}" "$blit" | tee -a "$SUMMARY"
}

# ------------------------------------------------------------------ one run

run() {
    label=$1; game=$2; config=$3
    rom=$(ls "$ROMDIR/$game"*.zip 2>/dev/null | head -1)
    if [ -z "$rom" ]; then
        say "$label [$config] SKIPPED: could not find $game"
        return 0
    fi
    options "$config"
    frames=$((SECS * 60))
    t0=$(now_ms)
    timeout $((SECS * 4 + 60)) "$RA" -L "$CORE" "$rom" --max-frames="$frames" --verbose \
        > "$OUT/$label.log" 2>&1
    status=$?
    t1=$(now_ms)
    spent=$((t1 - t0))
    secs=$(awk "BEGIN{printf \"%.1f\", $spent/1000}")
    fps=$(awk "BEGIN{printf \"%.1f\", $frames/($spent/1000)}")
    warning=''
    [ "$status" = 124 ] && warning='  <-- DID NOT FINISH: stopped at the time limit'
    say "$label [$config] ${secs}s for $frames frames -> $fps f/s$warning"
    check "$OUT/$label.log"
}

# ------------------------------------------------------------------ the header

say "Zeebx: on-device test $(date)"
say "retroarch: $RA"
say "config:    ${CFG:-(not found)}"
say "core:      $CORE"
say "roms:      $ROMDIR"
say "duration:  $SECS s per run"
say "----------------------------------------"
{
    echo "== device =="; uname -a
    cat /etc/os-release 2>/dev/null | head -4
    grep -m2 -iE 'hardware|model name|revision' /proc/cpuinfo
    echo "cores: $(grep -c ^processor /proc/cpuinfo)"
    free -m 2>/dev/null | head -2
    echo "glibc: $(getconf GNU_LIBC_VERSION 2>/dev/null || ldd --version 2>/dev/null | head -1)"
    echo "df:"; df -h "$OUT" 2>/dev/null | tail -2
    echo "== retroarch =="; "$RA" --version 2>&1 | head -3
    echo "== core =="; ls -la "$CORE"; sha256sum "$CORE" 2>/dev/null
    echo "== options before =="
    for d in $DIRS; do
        echo "-- $d"
        grep '^zeebx_' "$d/retroarch-core-options.cfg" 2>/dev/null
        cat "$d/config/Zeebx/Zeebx.opt" 2>/dev/null
    done
    echo "== gpu =="; cat /sys/class/drm/card*/device/uevent 2>/dev/null | head -6
    dmesg 2>/dev/null | grep -iE 'mali|panfrost|drm' | tail -6
} > "$OUT/00-device.txt" 2>&1

save_options

# ------------------------------------------------------------------ the runs

say "--- 3D on the GPU: tile discard and scale ---"
run base-crash       "Crash Bandicoot Nitro Kart 3D" base
run base-rally       "Rally Master Pro"              base
run handheld-rally   "Rally Master Pro"              handheld
run tiles-rally      "Rally Master Pro"              tiles
run tiles-crash      "Crash Bandicoot Nitro Kart 3D" tiles

say "--- 2D, arcade and heavy ones ---"
run base-quake       "Quake"                         base
run base-dd          "Double Dragon"                 base
run base-mdrop       "Magical Drop III"              base
run base-zeeboids    "Zeeboids"                      base
run base-peteca      "Zeebo Sports Peteca"           base

say "--- Zenonia: only draws 2D ---"
run base-zenonia     "Zenonia"                       base
run sw-zenonia       "Zenonia"                       software

say "--- diagnostics: the core telling everything ---"
run diag-base        "Rally Master Pro"              debug
run diag-tiles       "Rally Master Pro"              "tiles debug"
run diag-handheld    "Rally Master Pro"              "handheld debug"

restore_options

say "----------------------------------------"
say "done. Send this folder back: $OUT"
say "the summary is in summary.txt and the log of each run in the file named after it."
