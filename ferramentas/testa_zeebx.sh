#!/bin/sh
# Mede o Zeebx no aparelho e junta tudo num pacote de log.
#
# Uso:   sh testa_zeebx.sh [segundos_por_rodada] [pasta_das_roms]
#        sh testa_zeebx.sh 20 /roms/zeebo
#
# Cada rodada roda um numero fixo de quadros e mede o tempo de parede: quadros por segundo de
# verdade. O log de cada rodada fica junto, e o resumo diz, por rodada, QUAL ajuste o nucleo
# aplicou -- assim o resultado nao depende de acreditar que a opcao pegou.

set -u

SEG=${1:-20}
ROMDIR=${2:-}
AQUI=$(cd "$(dirname "$0")" && pwd)
SAIDA="$AQUI/logs-zeebx-$(date +%Y%m%d-%H%M%S)"
mkdir -p "$SAIDA"
RESUMO="$SAIDA/resumo.txt"
: > "$RESUMO"

fala() { printf '%s\n' "$*" | tee -a "$RESUMO"; }

# ------------------------------------------------------------------ achar o retroarch e o core

RA=$(command -v retroarch || true)
[ -n "$RA" ] || RA=$(ls /usr/local/bin/retroarch /usr/bin/retroarch 2>/dev/null | head -1)
[ -n "$RA" ] || { fala "RETROARCH: nao achei o executavel"; exit 1; }

# **O muOS precisa de um preparo que o lancador dele faz e nao repassa.** O
# `/opt/muos/script/launch/lr-general.sh` monta o ambiente de SDL e o `HOME` antes de chamar o
# RetroArch, e **nao repassa argumento nenhum** -- sem isso, `--max-frames` nao chega la. Entao o
# script faz o mesmo preparo e chama o binario direto.
if [ -f /opt/muos/script/var/func.sh ]; then
    set +u
    . /opt/muos/script/var/func.sh 2>/dev/null || true
    command -v SETUP_SDL_ENVIRONMENT >/dev/null 2>&1 && SETUP_SDL_ENVIRONMENT >/dev/null 2>&1
    novo_home=$(GET_VAR "device" "board/home" 2>/dev/null || true)
    [ -n "${novo_home:-}" ] && HOME="$novo_home" && export HOME
    set -u
fi

# O `HOME` do contexto nao e confiavel (o EmulationStation pode rodar como root), entao os
# caminhos do ArkOS e do muOS entram na lista -- incluindo o do muOS, que e um symlink para
# `/opt/muos/share/info/config`, onde o muOS guarda as opcoes de cada core.
# **Todos** os diretorios plausiveis, e nao so o primeiro: o ArkOS guarda as opcoes num arquivo
# global e o muOS por core, em pastas diferentes, e o script nao tem como saber qual dos dois vale
# sem tentar. Escrever em todos e o que faz o teste valer nos dois aparelhos; se nenhum aceitar, o
# resumo avisa em vez de sair um resultado que nao compara nada.
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
for c in "$AQUI/zeebx_libretro.so" "$CFG/cores/zeebx_libretro.so" \
         "/opt/muos/share/core/zeebx_libretro.so" /usr/lib/libretro/zeebx_libretro.so; do
    [ -f "$c" ] && CORE="$c" && break
done
[ -n "$CORE" ] || { fala "CORE: nao achei o zeebx_libretro.so"; exit 1; }

# No muOS os jogos vao no **cartao de ROMs** (SD2), e o sistema de conteudo e um unionfs: o que
# vale e `/mnt/union/ROMS`, que junta USB, SDCARD e o cartao do sistema nessa ordem. Procurar pelo
# union antes dos caminhos crus e o que faz achar os jogos nos dois arranjos -- um cartao so ou
# dois.
if [ -z "$ROMDIR" ]; then
    for d in /roms/zeebo /roms/Zeebo /roms/ROMS/Zeebo \
             /mnt/union/ROMS/Zeebo /mnt/union/ROMS/zeebo \
             /mnt/sdcard/ROMS/Zeebo /mnt/mmc/ROMS/Zeebo "$HOME/roms/zeebo"; do
        [ -d "$d" ] && ROMDIR="$d" && break
    done
fi
# Ultimo recurso: procurar uma pasta Zeebo em qualquer cartao montado. Melhor achar do que dizer
# que nao ha jogo com a pasta ali do lado.
if [ -z "$ROMDIR" ]; then
    ROMDIR=$(ls -d /mnt/*/ROMS/Zeebo /mnt/*/ROMS/zeebo /mnt/*/Zeebo 2>/dev/null | head -1)
fi
[ -n "$ROMDIR" ] || { fala "ROMS: nao achei a pasta; passe como segundo argumento"; exit 1; }

# ------------------------------------------------------------------ o tempo, com ou sem nanos

if [ "$(date +%N 2>/dev/null | wc -c)" -ge 10 ]; then
    agora_ms() { date +%s%N | cut -c1-13; }
else
    agora_ms() { echo $(( $(date +%s) * 1000 )); }
fi

# ------------------------------------------------------------------ as opcoes do core

# **As opcoes do Zeebx vivem em dois arquivos, e o aparelho usa o global.** Com
# `global_core_options = true` (o padrao do ArkOS) o RetroArch le e escreve tudo em
# `retroarch-core-options.cfg`; o `.opt` por core so vale com a opcao desligada. Escrever nos dois
# e o que faz o teste valer nos dois arranjos -- e, se so um valesse, as rodadas sairiam todas
# iguais e a comparacao nao diria nada.
ANTES="$SAIDA/opcoes-antes"
NOVAS="$SAIDA/zeebx-da-rodada.cfg"

salva_opcoes() {
    mkdir -p "$ANTES"
    n=0
    for d in $DIRS; do
        n=$((n + 1))
        [ -f "$d/retroarch-core-options.cfg" ] && cp "$d/retroarch-core-options.cfg" "$ANTES/global-$n.cfg"
        [ -f "$d/config/Zeebx/Zeebx.opt" ] && cp "$d/config/Zeebx/Zeebx.opt" "$ANTES/core-$n.opt"
    done
    return 0
}
devolve_opcoes() {
    n=0
    for d in $DIRS; do
        n=$((n + 1))
        [ -f "$ANTES/global-$n.cfg" ] && cp "$ANTES/global-$n.cfg" "$d/retroarch-core-options.cfg"
        if [ -f "$ANTES/core-$n.opt" ]; then cp "$ANTES/core-$n.opt" "$d/config/Zeebx/Zeebx.opt"; fi
    done
    return 0
}

poe() { printf '%s = "%s"\n' "$1" "$2" >> "$NOVAS"; }

# `log = informacao` em todas as rodadas de proposito: e o nivel em que o nucleo diz qual
# rasterizador entrou e se o descarte de tiles ficou ligado. Sem isso, o resumo nao teria como
# provar que o ajuste da rodada pegou.
# **Cada chave sai uma vez so.** A primeira versao escrevia o padrao e depois a escolha por cima,
# e o RetroArch fica com o primeiro valor que le: a rodada com o descarte de tiles ligado corria
# com ele desligado, e o teste inteiro nao mediria nada.
opcoes() {
    perfil=padrao; rast=auto; escala=1; tiles=desligado; pulo=desligado; fps=desligado; nivel=informacao
    for escolha in $1; do
        case "$escolha" in
            portatil)  perfil=portatil ;;
            tiles)     tiles=ligado ;;
            software)  rast=software ;;
            depuracao) nivel=depuracao ;;
        esac
    done
    : > "$NOVAS"
    poe zeebx_perfil "$perfil"
    poe zeebx_rasterizador "$rast"
    poe zeebx_resolucao_interna "$escala"
    poe zeebx_descarte_de_tiles "$tiles"
    poe zeebx_frameskip "$pulo"
    poe zeebx_limite_fps "$fps"
    poe zeebx_log "$nivel"
    # Em cada diretorio encontrado: o global (preservando as opcoes dos outros cores, que o
    # arquivo guarda todas) e o `.opt` do Zeebx.
    escreveu=0
    for d in $DIRS; do
        g="$d/retroarch-core-options.cfg"
        if [ -f "$g" ]; then
            { grep -v '^zeebx_' "$g" 2>/dev/null; cat "$NOVAS"; } > "$SAIDA/global.tmp"
            cp "$SAIDA/global.tmp" "$g" 2>/dev/null && escreveu=1
            rm -f "$SAIDA/global.tmp"
        fi
        mkdir -p "$d/config/Zeebx" 2>/dev/null
        cp "$NOVAS" "$d/config/Zeebx/Zeebx.opt" 2>/dev/null && escreveu=1
    done
    if [ "$escreveu" = 0 ]; then
        fala "ATENCAO: nao deu para escrever as opcoes em nenhum destino ($DIRS)."
        fala "         Sem isso, todas as rodadas correm no ajuste de fabrica e o teste nao compara nada."
    fi
}

# ------------------------------------------------------------------ o que o nucleo disse

confere() {
    log=$1
    # **A ultima linha, e nao a primeira.** O nucleo aplica as opcoes no arranque (com os padroes)
    # e so depois a escolha da rodada: pegar a primeira ocorrencia diria "desligado" em toda rodada.
    rast=$(grep -o 'desenhando na placa\|desenha no processador\|rasterizador fixado no processador' "$log" 2>/dev/null | tail -1)
    tiles=$(grep -o 'depois do quadro: [a-z]*' "$log" 2>/dev/null | tail -1 | sed 's/.*: //')
    [ -n "$tiles" ] || tiles='(nao dito)'
    gpu=$(grep -o 'GL real vendor=[^;]*' "$log" 2>/dev/null | tail -1 | sed 's/GL real vendor=//' | cut -c1-40)
    tamanho=$(grep -m1 -o 'quadro [0-9]*x[0-9]*, fora dos 640x480' "$log" 2>/dev/null)
    blit=$(grep -m1 -c 'nao devolve o que mandaram copiar' "$log" 2>/dev/null)
    printf '    ajuste: %s | tiles: %s | gpu: %s | quadro errado: %s | blit reprovado: %s\n' \
        "${rast:-?}" "$tiles" "${gpu:-?}" "${tamanho:-nao}" "$blit" | tee -a "$RESUMO"
}

# ------------------------------------------------------------------ uma rodada

roda() {
    rotulo=$1; jogo=$2; config=$3
    rom=$(ls "$ROMDIR/$jogo"*.zip 2>/dev/null | head -1)
    if [ -z "$rom" ]; then
        fala "$rotulo [$config] PULADO: nao achei $jogo"
        return 0
    fi
    opcoes "$config"
    quadros=$((SEG * 60))
    t0=$(agora_ms)
    timeout $((SEG * 4 + 60)) "$RA" -L "$CORE" "$rom" --max-frames="$quadros" --verbose \
        > "$SAIDA/$rotulo.log" 2>&1
    saida=$?
    t1=$(agora_ms)
    gasto=$((t1 - t0))
    seg=$(awk "BEGIN{printf \"%.1f\", $gasto/1000}")
    fps=$(awk "BEGIN{printf \"%.1f\", $quadros/($gasto/1000)}")
    aviso=''
    [ "$saida" = 124 ] && aviso='  <-- NAO TERMINOU: parou no tempo limite'
    fala "$rotulo [$config] ${seg}s para $quadros quadros -> $fps q/s$aviso"
    confere "$SAIDA/$rotulo.log"
}

# ------------------------------------------------------------------ o cabecalho

fala "Zeebx: teste no aparelho $(date)"
fala "retroarch: $RA"
fala "config:    ${CFG:-(nao achei)}"
fala "core:      $CORE"
fala "roms:      $ROMDIR"
fala "duracao:   $SEG s por rodada"
fala "----------------------------------------"
{
    echo "== aparelho =="; uname -a
    cat /etc/os-release 2>/dev/null | head -4
    grep -m2 -iE 'hardware|model name|revision' /proc/cpuinfo
    echo "nucleos: $(grep -c ^processor /proc/cpuinfo)"
    free -m 2>/dev/null | head -2
    echo "glibc: $(getconf GNU_LIBC_VERSION 2>/dev/null || ldd --version 2>/dev/null | head -1)"
    echo "df:"; df -h "$SAIDA" 2>/dev/null | tail -2
    echo "== retroarch =="; "$RA" --version 2>&1 | head -3
    echo "== core =="; ls -la "$CORE"; sha256sum "$CORE" 2>/dev/null
    echo "== opcoes antes =="
    for d in $DIRS; do
        echo "-- $d"
        grep '^zeebx_' "$d/retroarch-core-options.cfg" 2>/dev/null
        cat "$d/config/Zeebx/Zeebx.opt" 2>/dev/null
    done
    echo "== gpu =="; cat /sys/class/drm/card*/device/uevent 2>/dev/null | head -6
    dmesg 2>/dev/null | grep -iE 'mali|panfrost|drm' | tail -6
} > "$SAIDA/00-aparelho.txt" 2>&1

salva_opcoes

# ------------------------------------------------------------------ as rodadas

fala "--- 3D na placa: descarte de tiles e escala ---"
roda base-crash       "Crash Bandicoot Nitro Kart 3D" base
roda base-rally       "Rally Master Pro"              base
roda portatil-rally   "Rally Master Pro"              portatil
roda tiles-rally      "Rally Master Pro"              tiles
roda tiles-crash      "Crash Bandicoot Nitro Kart 3D" tiles

fala "--- 2D, arcade e pesados ---"
roda base-quake       "Quake"                         base
roda base-dd          "Double Dragon"                 base
roda base-mdrop       "Magical Drop III"              base
roda base-zeeboids    "Zeeboids"                      base
roda base-peteca      "Zeebo Sports Peteca"           base

fala "--- Zenonia: so desenha 2D ---"
roda base-zenonia     "Zenonia"                       base
roda sw-zenonia       "Zenonia"                       software

fala "--- diagnostico: o nucleo contando tudo ---"
roda diag-base        "Rally Master Pro"              depuracao
roda diag-tiles       "Rally Master Pro"              "tiles depuracao"
roda diag-portatil    "Rally Master Pro"              "portatil depuracao"

devolve_opcoes

fala "----------------------------------------"
fala "pronto. Mande esta pasta de volta: $SAIDA"
fala "o resumo esta em resumo.txt e o log de cada rodada no arquivo com o nome dela."
