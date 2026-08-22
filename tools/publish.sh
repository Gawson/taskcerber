#!/usr/bin/env bash
# Publikuje zawartość `dist/` na gałęzi `gh-pages`.
#
# # Dlaczego stąd, a nie z CI
#
# Obraz i tak powstaje lokalnie i lokalnie przechodzi `check-image.sh` — kontrolę
# offsetów, deskryptora, sumy SHA-256 i skan sekretów. Budowanie go drugi raz na
# runnerze GitHuba nic nie dodaje, a kosztuje 35-60 minut przy zimnym cache, bo
# job ściąga ~1,2 GB narzędzi ESP-IDF. Publikacja stąd kosztuje zero minut.
#
# Ubocznie znika cała klasa pomyłek: publikujemy DOKŁADNIE ten artefakt, który
# przeszedł kontrolę, a nie jego kuzyna zbudowanego gdzie indziej.
#
# # Dlaczego gałąź jednorazowa, a nie dopisywanie do historii
#
# `firmware.bin` waży 3,1 MB. Dopisywanie każdego wydania do historii `gh-pages`
# rozdęłoby repozytorium o tyle za każdym razem, a stare obrazy nie są nikomu
# potrzebne — urządzenie pobiera zawsze najnowszy. Dlatego gałąź jest za każdym
# razem budowana od zera z jednego commita i wypychana z `--force`.
#
# To jest jedyne miejsce w tym projekcie, gdzie `--force` jest poprawne: `gh-pages`
# to treść WYGENEROWANA, nie źródło. Nikt na niej nie pracuje i nie ma czego stracić.
#
# # Konfiguracja po stronie GitHuba, jednorazowa
#
#   Settings -> Pages -> Source: "Deploy from a branch" -> gh-pages / (root)
#
# Świadomie NIE „GitHub Actions" — ten wariant wymagałby uruchomionego workflow.
#
# Użycie:
#   ./tools/build-image.sh && ./tools/publish.sh
set -euo pipefail

cd "$(dirname "$0")/.."

GALAZ=gh-pages
DIST=dist

[[ -d "$DIST" ]] || { echo "BŁĄD: brak katalogu $DIST — najpierw ./tools/build-image.sh" >&2; exit 1; }

# Bez tych trzech plików publikacja jest bezużyteczna: `ota.json` mówi urządzeniu,
# co pobrać, `firmware-ota.bin` jest tym obrazem, a `index.html` to webflasher.
for plik in ota.json firmware-ota.bin firmware.bin index.html; do
    [[ -f "$DIST/$plik" ]] || { echo "BŁĄD: brak $DIST/$plik" >&2; exit 1; }
done

zdalny=$(git remote get-url origin 2>/dev/null) || {
    echo "BŁĄD: repozytorium nie ma zdalnego `origin`" >&2; exit 1; }

wersja=$(./tools/version.sh)
opis=$(git log -1 --pretty=%s)

echo "==> publikuję wersję $wersja na gałąź $GALAZ"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

cp -r "$DIST/." "$tmp/"

# GitHub Pages domyślnie przepuszcza treść przez Jekylla, który POMIJA pliki
# i katalogi zaczynające się od podkreślenia. Vendorowany bundle esp-web-tools
# takie ma, więc bez tego pliku webflasher wyszedłby połamany.
touch "$tmp/.nojekyll"

git -C "$tmp" init -q
git -C "$tmp" add -A
git -C "$tmp" -c user.email="$(git config user.email)" \
              -c user.name="$(git config user.name)" \
              commit -q -m "$wersja — $opis"
git -C "$tmp" push -q --force "$zdalny" HEAD:"$GALAZ"

uzytkownik=$(basename "$(dirname "$zdalny")" | sed 's/.*://')
repo=$(basename "$zdalny" .git)

echo
echo "Opublikowane. Adresy, gdy Pages będzie włączone:"
echo "  webflasher   https://${uzytkownik,,}.github.io/$repo/"
echo "  manifest OTA https://${uzytkownik,,}.github.io/$repo/ota.json"
echo
echo "Ten drugi jest już wkompilowany jako DEFAULT_OTA_URL, więc urządzenie"
echo "znajdzie go samo — bez wpisywania czegokolwiek w konfiguracji."
