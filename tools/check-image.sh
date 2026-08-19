#!/usr/bin/env bash
# Weryfikacja obrazu przed publikacją.
#
# Sprawdza dwie rzeczy, których nie widać po samym „build się udał":
#   1. Czy obraz ma poprawną strukturę — zły offset bootloadera na S3 to klasyczny
#      błąd, po którym urządzenie po prostu nie wstaje, bez żadnego komunikatu.
#   2. Czy nie wyciekły do niego sekrety. Opublikowany obraz może pobrać każdy,
#      a `strings` na nim to całość ataku.
set -euo pipefail

cd "$(dirname "$0")/.."
IMG=${1:-dist/firmware.bin}
BOOTLOADER=firmware/target/xtensa-esp32s3-espidf/release/bootloader.bin

fail() { echo "BŁĄD: $*" >&2; exit 1; }
ok()   { echo "  ok   $*"; }

[[ -f "$IMG" ]] || fail "brak $IMG — uruchom najpierw tools/build-image.sh"

echo "== struktura =="

at() { od -A n -t x1 -N "$2" -j "$1" "$IMG" | tr -d ' \n'; }

# ESP32-S3 bootuje z 0x0, nie z 0x1000 jak klasyczny ESP32. Przykład w README
# esp-web-tools podaje dla S3 offset 4096 i jest błędny.
[[ "$(at 0 1)" == "e9" ]] || fail "brak magic 0xE9 na offsecie 0 — bootloader nie jest tam, gdzie S3 go szuka"
ok "bootloader na 0x0 (magic 0xE9)"

# Bajt 2 to tryb SPI: 02 = DIO. Poprawny mimo QIO — Kconfig ESP-IDF mówi, że
# bootloader flashuje się w dio i sam przechodzi w quad przy inicjalizacji.
mode=$(at 2 1)
[[ "$mode" == "02" ]] || echo "  UWAGA tryb SPI bootloadera to 0x$mode, spodziewano się 02 (dio)"
[[ "$mode" == "02" ]] && ok "tryb flasha DIO"

# Bajt 3: starszy półbajt = rozmiar flasha (4 = 16 MB), młodszy = częstotliwość (f = 80 MHz).
sz=$(at 3 1)
[[ "$sz" == "4f" ]] || echo "  UWAGA bajt rozmiar/częstotliwość to 0x$sz, spodziewano się 4f (16 MB / 80 MHz)"
[[ "$sz" == "4f" ]] && ok "flash 16 MB @ 80 MHz"

[[ "$(at 32768 2)" == "aa50" ]] || fail "brak magic tablicy partycji (0xAA50) na 0x8000"
ok "tablica partycji na 0x8000"

[[ "$(at 65536 1)" == "e9" ]] || fail "brak obrazu aplikacji na 0x10000 (ota_0)"
ok "aplikacja na 0x10000"

# Bez --skip-padding espflash dopycha obraz do rozmiaru flasha. Efekt: 16 MB do
# pobrania zamiast ~3 MB, a przy okazji dopchane sektory kasują partycję nvs,
# bo esp-web-tools woła writeFlash({eraseAll: false}).
size=$(stat -c%s "$IMG")
(( size < 8 * 1024 * 1024 )) || fail "obraz ma $size B — wygląda na dopchany do rozmiaru flasha (brak --skip-padding)"
ok "rozmiar $(numfmt --to=iec "$size") — bez dopychania"

if [[ -f "$BOOTLOADER" ]]; then
    bl_size=$(stat -c%s "$BOOTLOADER")
    if cmp -s <(head -c "$bl_size" "$IMG") "$BOOTLOADER"; then
        ok "bootloader pochodzi z projektu (ma inicjalizację octal PSRAM)"
    else
        fail "bootloader w obrazie NIE jest tym z projektu — zapasowy z espflash nie inicjalizuje octal PSRAM i urządzenie wpadnie w pętlę bootowania"
    fi
fi

# `strings` raz, do pliku. Pod `set -o pipefail` konstrukcja `strings ... | grep -q`
# jest pułapką: grep zamyka potok po pierwszym trafieniu, strings dostaje SIGPIPE
# i cały potok raportuje porażkę mimo znalezienia szukanego wzorca.
STR=$(mktemp)
trap 'rm -f "$STR"' EXIT
strings "$IMG" > "$STR"

echo "== deskryptor aplikacji =="
grep -qx "t5s3pro" "$STR" || fail "brak nazwy projektu w deskryptorze aplikacji"
ok "deskryptor obecny"

echo "== sekrety =="

# Nagłówki PEM (-----BEGIN RSA PRIVATE KEY-----) są w każdym obrazie z mbedTLS
# jako format stringi do parsowania i NIE są wyciekiem. Szukamy materiału,
# który faktycznie mógłby wyciec: prywatnych adresów iCal i poświadczeń WiFi
# wkompilowanych przez env!().
found=0

if grep -qE 'calendar\.google\.com/calendar/ical/.*/private-' "$STR"; then
    echo "  ZNALEZIONO prywatny adres iCal wkompilowany w obraz" >&2
    found=1
fi

# Klucz prywatny z prawdziwą zawartością, a nie sam nagłówek.
if grep -A1 -E '^-----BEGIN .* PRIVATE KEY-----$' "$STR" | grep -qE '^[A-Za-z0-9+/]{40,}={0,2}$'; then
    echo "  ZNALEZIONO materiał klucza prywatnego" >&2
    found=1
fi

# Jeśli budujesz z --features devcreds, poświadczenia trafiają do binarki.
# Ten build nigdy nie powinien być publikowany.
if grep -qE '^DEVCREDS_MARKER$' "$STR"; then
    echo "  ZNALEZIONO znacznik builda z wkompilowanymi poświadczeniami" >&2
    found=1
fi

(( found == 0 )) || fail "obraz zawiera sekrety — nie publikuj go"
ok "brak sekretów"

# ---------------------------------------------------------------------------
# Obraz OTA — publikowany obok, ale to CO INNEGO niż sklejka dla webflashera.
# ---------------------------------------------------------------------------
OTA_IMG="$(dirname "$IMG")/firmware-ota.bin"
OTA_JSON="$(dirname "$IMG")/ota.json"

if [[ -f "$OTA_IMG" ]]; then
    echo
    echo "== obraz OTA =="

    [[ "$(od -A n -t x1 -N 1 -j 0 "$OTA_IMG" | tr -d ' \n')" == "e9" ]] \
        || fail "brak magic 0xE9 na początku obrazu OTA"
    ok "magic aplikacji"

    # Najgroźniejsza pomyłka: opublikowanie sklejki jako obrazu OTA. `esp_ota_write`
    # pisze do slotu aplikacji, więc sklejka wylądowałaby tam razem z bootloaderem
    # i tablicą partycji — urządzenie nie wstaje, a naprawa wymaga kabla.
    [[ "$(od -A n -t x1 -N 2 -j 32768 "$OTA_IMG" | tr -d ' \n')" != "aa50" ]] \
        || fail "obraz OTA ma tablicę partycji na 0x8000 — to sklejka dla webflashera, nie obraz aplikacji"
    ok "to obraz aplikacji, nie sklejka"

    ota_size=$(stat -c%s "$OTA_IMG")
    (( ota_size > 256 * 1024 )) || fail "obraz OTA ma tylko $ota_size B"
    (( ota_size < 4 * 1024 * 1024 )) || fail "obraz OTA ma $ota_size B — nie zmieści się w slocie 4 MiB"
    ok "rozmiar $(numfmt --to=iec "$ota_size") — mieści się w slocie"

    # Ten plik też jest publiczny.
    OTA_STR=$(mktemp)
    trap 'rm -f "$STR" "$OTA_STR"' EXIT
    strings "$OTA_IMG" > "$OTA_STR"
    grep -qx "t5s3pro" "$OTA_STR" || fail "brak deskryptora aplikacji w obrazie OTA"
    grep -qE 'calendar\.google\.com/calendar/ical/.*/private-' "$OTA_STR" \
        && fail "obraz OTA zawiera prywatny adres iCal"
    ok "deskryptor obecny, brak sekretów"

    if [[ -f "$OTA_JSON" ]]; then
        want=$(grep -o '"sha256"[^"]*"[0-9a-f]*"' "$OTA_JSON" | grep -o '[0-9a-f]\{64\}')
        have=$(sha256sum "$OTA_IMG" | cut -d' ' -f1)
        [[ "$want" == "$have" ]] || fail "SHA-256 w ota.json ($want) nie zgadza się z obrazem ($have)"
        ok "SHA-256 w ota.json zgadza się z obrazem"

        # Wersja z manifestu MUSI być tym, co obraz o sobie mówi. Rozjazd tych dwóch
        # to najgorsza awaria, jaką OTA potrafi wyprodukować: urządzenie pobiera
        # 3 MB, instaluje, restartuje, widzi w manifeście wciąż inną wersję niż
        # własna — i pobiera znowu. Licznik prób to zatrzyma, ale dopiero po trzech
        # podejściach, a na baterii to jest realny koszt.
        #
        # Jeśli tu padnie po zwykłym `cargo build`: build.rs nie przeliczył wersji,
        # bo nic go nie unieważniło. `touch firmware/build.rs` i jeszcze raz.
        want_ver=$(grep -o '"version"[[:space:]]*:[[:space:]]*"[^"]*"' "$OTA_JSON" | cut -d'"' -f4)
        [[ -n "$want_ver" ]] || fail "ota.json nie ma pola version"
        grep -qF "$want_ver" "$OTA_STR" \
            || fail "obraz OTA nie zawiera wersji '$want_ver' z ota.json — manifest i obraz się rozjechały"
        ok "wersja $want_ver zgadza się z obrazem"
    else
        echo "  UWAGA brak $OTA_JSON — urządzenie nie ma z czego czytać wersji"
    fi
fi

echo
echo "Obraz gotowy do publikacji: $IMG"
