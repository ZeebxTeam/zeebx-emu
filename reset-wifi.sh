#!/usr/bin/env bash
# Reinicializa apenas a Realtek RTL8852BE deste computador.
set -euo pipefail
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
pci=0000:08:00.0
dev=/sys/bus/pci/devices/$pci
driver=/sys/bus/pci/drivers/rtw89_8852be
case "${1:-}" in
  --dry-run) dry=1 ;;
  '') dry=0 ;;
  *) echo "Uso: sudo bash $0 [--dry-run]" >&2; exit 2 ;;
esac
[[ -d $dev ]] || { echo 'Placa PCIe não encontrada.' >&2; exit 1; }
[[ $(cat "$dev/vendor") == 0x10ec && $(cat "$dev/device") == 0xb852 ]] || {
  echo 'O dispositivo não é a RTL8852BE esperada.' >&2; exit 1;
}
if [[ -L $dev/driver && $(readlink -f "$dev/driver") != "$driver" ]]; then
  echo 'Driver diferente do esperado; operação cancelada.' >&2; exit 1
fi
if (( dry )); then
  echo "Alvo: RTL8852BE PCIe $pci, driver rtw89_8852be."
  echo 'Salva logs, desvincula e vincula novamente o driver e aguarda a interface.'
  echo 'A conexão desta placa cai durante a recuperação. Nenhuma alteração feita.'
  exit 0
fi
(( EUID == 0 )) || { echo "Execute: sudo bash $0" >&2; exit 1; }
exec 9>/run/lock/reset-wifi-rtl8852be.lock
flock -n 9 || { echo 'Já existe uma recuperação em andamento.' >&2; exit 1; }
umask 077
log=$(mktemp /tmp/reset-wifi.XXXXXXXX.log)
journalctl -k -b --no-pager -n 1500 > "$log" 2>&1 || true
echo "Registro anterior ao reset: $log"
modprobe rtw89_8852be
# Tenta restabelecer o vínculo mesmo se houver interrupção ou erro.
restore() {
  if [[ -d $dev && ! -L $dev/driver ]]; then
    printf '%s' "$pci" > "$driver/bind" || true
  fi
}
trap restore EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
echo 'Reinicializando a placa; a conexão Wi-Fi dela será interrompida...'
if [[ -L $dev/driver ]]; then
  printf '%s' "$pci" > "$driver/unbind"
fi
sleep 2
printf '%s' "$pci" > "$driver/bind"
for ((i=0; i<20; i++)); do
  for net in "$dev"/net/*; do
    [[ -e $net ]] || continue
    iface=${net##*/}
    echo "Interface recuperada: $iface. O NetworkManager pode reconectar automaticamente."
    if command -v nmcli >/dev/null; then
      nmcli -f GENERAL.STATE,GENERAL.CONNECTION device show "$iface" || true
    fi
    echo 'Interface presente não garante conexão à internet. Se necessário, selecione sua rede no painel.'
    exit 0
  done
  sleep 1
done
echo 'A interface não reapareceu. Consulte journalctl -k -n 100; pode ser necessário reiniciar o computador.' >&2
exit 1
