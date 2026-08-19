#!/usr/bin/env bash
# Buduje firmware i skleja jednoplikowy obraz dla webflashera.
#
# Kluczowe szczegóły, każdy kosztował kogoś godziny:
#
#   --skip-padding      Bez tego espflash dopycha obraz zerami 0xFF do rozmiaru flasha,
#                       czyli publikujesz 16 MB do pobrania zamiast ~2,6 MB. Gorzej:
#                       esp-web-tools woła writeFlash({eraseAll: false}), więc te
#                       dopchane sektory KASUJĄ partycję nvs.
#   offset 0            ESP32-S3 bootuje z 0x0, nie z 0x1000 jak klasyczny ESP32.
#                       Przykład w README esp-web-tools podaje 4096 i jest błędny dla S3.
#   --bootloader        Musi być bootloader ZBUDOWANY PRZEZ PROJEKT, bo tylko on ma
#                       inicjalizację octal PSRAM. Wbudowany w espflash jej nie ma
#                       i urządzenie wpadnie w pętlę bootowania.
#   --flash-mode dio    Poprawne mimo QIO. Kconfig ESP-IDF: bootloader flashuje się
#                       w dio, a potem sam przechodzi w tryb quad przy inicjalizacji.
set -euo pipefail

cd "$(dirname "$0")/.."
BIN_NAME=t5s3pro
TARGET=xtensa-esp32s3-espidf
OUT=dist

if [[ -z "${IDF_PATH:-}" && -f "$HOME/export-esp.sh" ]]; then
  # shellcheck disable=SC1091
  . "$HOME/export-esp.sh"
fi

# Wersja liczona RAZ, przekazana do cargo i użyta w manifestach. To jest ta
# gwarancja, że obraz i ota.json nie mają jak się rozjechać: build.rs bierze
# T5_VERSION ze środowiska, a nie liczy jej po swojemu.
T5_VERSION=$(./tools/version.sh)
export T5_VERSION
echo "==> wersja: $T5_VERSION"

echo "==> buduję firmware"
(cd firmware && cargo build --release)

ELF="firmware/target/$TARGET/release/$BIN_NAME"
BOOTLOADER="firmware/target/$TARGET/release/bootloader.bin"

for f in "$ELF" "$BOOTLOADER"; do
  [[ -f "$f" ]] || { echo "BŁĄD: brak $f" >&2; exit 1; }
done

mkdir -p "$OUT"

echo "==> sklejam obraz"
espflash save-image \
  --chip esp32s3 \
  --merge \
  --skip-padding \
  --flash-size 16mb \
  --flash-mode dio \
  --flash-freq 80mhz \
  --bootloader "$BOOTLOADER" \
  --partition-table firmware/partitions.csv \
  --partition-table-offset 0x8000 \
  --target-app-partition ota_0 \
  "$ELF" \
  "$OUT/firmware.bin"

# Obraz dla OTA to CO INNEGO niż obraz dla webflashera. Webflasher dostaje sklejkę
# [bootloader][tablica partycji][aplikacja] pod offset 0; OTA dostaje samą aplikację,
# bo bootloader i tablica już na urządzeniu są, a `esp_ota_write` pisze do slotu
# aplikacji. Wgranie sklejki przez OTA daje urządzenie, które nie wstaje.
echo "==> obraz dla OTA (sama aplikacja)"
espflash save-image \
  --chip esp32s3 \
  --flash-size 16mb \
  --flash-mode dio \
  --flash-freq 80mhz \
  "$ELF" \
  "$OUT/firmware-ota.bin"

# Adres obrazu jest WZGLĘDNY. Firmware rozwiązuje go względem adresu manifestu
# (`net::ota::resolve_url`), więc ten sam plik działa z GitHub Pages i z serwera
# w LAN-ie, bez przebudowy i bez zmiennych środowiskowych w CI.
OTA_SHA=$(sha256sum "$OUT/firmware-ota.bin" | cut -d" " -f1)
OTA_SIZE=$(stat -c%s "$OUT/firmware-ota.bin")
cat > "$OUT/ota.json" <<JSON
{
  "version": "$T5_VERSION",
  "url": "firmware-ota.bin",
  "sha256": "$OTA_SHA",
  "size": $OTA_SIZE
}
JSON

echo "==> kompletuję stronę"
./tools/stage-page.sh "$OUT"

ls -lh "$OUT"
echo

# Zły offset bootloadera albo dopchany obraz to awarie ciche — urządzenie
# po prostu nie wstaje. Lepiej dowiedzieć się tutaj niż z przeglądarki.
./tools/check-image.sh "$OUT/firmware.bin"

echo
echo "Test lokalny:   python3 -m http.server -d $OUT 8000   # http://localhost:8000"
echo "Z innego kompa: ./tools/serve-flasher.sh                # HTTPS po adresie w LAN-ie"
