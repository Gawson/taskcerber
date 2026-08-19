#!/usr/bin/env bash
# Kompletuje stronę webflashera w dist/ — bez dotykania firmware'u.
#
# Wydzielone z build-image.sh, bo strona i binarka mają różne cykle życia:
# poprawka w tekście albo podbicie esp-web-tools nie wymaga przebudowy firmware'u,
# a serve-flasher.sh musi mieć pewność, że serwuje aktualne web/, nie kopię
# sprzed trzech commitów.
#
# NIE kopiuje niczego spoza web/ — dist/ ląduje na GitHub Pages jako artefakt CI,
# więc wszystko, co tu trafi, jest publiczne.
set -euo pipefail

cd "$(dirname "$0")/.."
OUT=${1:-dist}
mkdir -p "$OUT"

cp web/index.html "$OUT/index.html"

# Bundle esp-web-tools jest code-splitowany — kopiujemy cały katalog, bo
# install-button.js dociąga chunki dynamicznie po ścieżkach względnych.
rm -rf "$OUT/vendor"
cp -a web/vendor "$OUT/vendor"

# Wersja w manifeście to wersja firmware'u — esp-web-tools pokazuje ją w dialogu.
# Ta sama, którą raportuje obraz i którą porównuje OTA. `build-image.sh` podaje ją
# w środowisku; przy samodzielnym uruchomieniu liczymy ją tym samym skryptem.
VERSION=${T5_VERSION:-$(./tools/version.sh)}
sed "s/\"version\": \"[^\"]*\"/\"version\": \"$VERSION\"/" web/manifest.json > "$OUT/manifest.json"

echo "  strona: $OUT/index.html + manifest.json ($VERSION) + vendor/ ($(find "$OUT/vendor" -type f | wc -l) plików)"
