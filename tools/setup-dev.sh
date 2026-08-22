#!/usr/bin/env bash
# Przygotowanie stanowiska deweloperskiego.
#
# Robi dwie niezależne rzeczy:
#   1. regułę udev, żeby port płytki był zapisywalny bez ręcznego chmod
#   2. tryb bypass uprawnień Claude Code dla tego projektu
#
# Idempotentny — można puszczać wielokrotnie.
#
# Użycie:  ./tools/setup-dev.sh
set -uo pipefail

PROJEKT=$(cd "$(dirname "$0")/.." && pwd)
REGULA=/etc/udev/rules.d/99-esp32.rules
USTAWIENIA="$PROJEKT/.claude/settings.local.json"

echo "=============================================="
echo " taskcerber — przygotowanie stanowiska"
echo " projekt: $PROJEKT"
echo "=============================================="
echo

# --- 1. udev -----------------------------------------------------------------
# Port płytki wstaje jako 660 root:dialout. Bez reguły każde wybudzenie z deep
# sleepu re-enumeruje urządzenie i przywraca te prawa, więc ręczny chmod trzeba
# byłoby powtarzać w kółko.
#
# Zawężone do identyfikatora producenta Espressif (303a) — nie otwieramy na
# oślep każdego urządzenia szeregowego, które ktoś wepnie.
echo "[1/2] reguła udev dla portu płytki"

if [ -f "$REGULA" ] && grep -q '303a' "$REGULA" 2>/dev/null; then
    echo "      już istnieje: $REGULA"
else
    echo "      potrzebny root — sudo poprosi o hasło"
    if sudo sh -c "cat > $REGULA" <<'UDEV'
# Płytki Espressif (ESP32-S3 USB-JTAG i mostki szeregowe) dostępne dla zwykłego
# użytkownika. Bez tego espflash dostaje odmowę zapisu po każdym wybudzeniu.
SUBSYSTEM=="tty", ATTRS{idVendor}=="303a", MODE="0666"
SUBSYSTEM=="usb", ATTRS{idVendor}=="303a", MODE="0666"
UDEV
    then
        sudo udevadm control --reload-rules && sudo udevadm trigger --subsystem-match=tty
        echo "      zapisana i przeładowana"
    else
        echo "      NIE UDAŁO SIĘ — brak sudo albo odmowa. Pomijam."
    fi
fi

# Reguła zadziała dopiero przy następnej enumeracji, a płytka bywa już wpięta ze
# starymi prawami — bieżący egzemplarz otwieramy od razu.
for p in /dev/ttyACM* /dev/ttyUSB*; do
    if [ -e "$p" ] && [ ! -w "$p" ]; then
        sudo chmod 666 "$p" 2>/dev/null && echo "      doraźnie otwarty: $p"
    fi
done
echo

# --- 2. uprawnienia Claude Code ----------------------------------------------
echo "[2/2] tryb bypass uprawnień dla projektu"

mkdir -p "$PROJEKT/.claude"

if [ -f "$USTAWIENIA" ]; then
    cp "$USTAWIENIA" "$USTAWIENIA.bak"
    echo "      kopia poprzednich ustawień: settings.local.json.bak"
fi

cat > "$USTAWIENIA" <<'JSON'
{
  "permissions": {
    "defaultMode": "bypassPermissions",
    "allow": [
      "Bash(rustc --version)",
      "Bash(cargo --version)",
      "Bash(timeout 500 cargo test --workspace)"
    ]
  }
}
JSON

echo "      zapisane: .claude/settings.local.json"
echo "      (jest w .gitignore, więc nie trafi do publicznego repo)"
echo

# --- podsumowanie ------------------------------------------------------------
echo "=============================================="
echo " Stan po zmianach"
echo "=============================================="
if ls /dev/ttyACM* >/dev/null 2>&1; then
    ls -l /dev/ttyACM* | sed 's/^/  /'
else
    echo "  port nieobecny — płytka śpi albo jest odpięta"
    echo "  (reguła zadziała sama, gdy się wybudzi)"
fi
echo
echo "  Tryb bypass obowiązuje od NASTĘPNEGO startu Claude Code,"
echo "  nie w sesji, która już chodzi."
