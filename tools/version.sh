#!/usr/bin/env bash
# Wypisuje wersję buildu — JEDYNE miejsce, w którym się ją liczy.
#
# Wersja jest tym, co OTA porównuje. `Cargo.toml` sam nie wystarczy: firmware
# raportuje `CARGO_PKG_VERSION`, manifest bierze tę samą wartość, więc dopóki nikt
# ręcznie nie podbije semvera, urządzenie zawsze widzi „0.1.0 == 0.1.0" i nie
# aktualizuje się NIGDY. Nie da się nawet sprawdzić, czy OTA działa.
#
# Stąd doklejony commit:
#
#     0.1.0+g1a2b3c4            czysty katalog roboczy
#     0.1.0+g1a2b3c4.d5f9a2     brudny — skrót różnicy względem HEAD
#
# Skrót brudnego drzewa nie jest ozdobą. Bring-up robi się na niescommitowanych
# zmianach, a bez niego dwa kolejne buildy miałyby tę samą wersję i urządzenie
# odmówiłoby wzięcia drugiego — co wygląda dokładnie jak zepsute OTA.
#
# Ograniczenie, o którym trzeba wiedzieć: skrót liczy się z `git diff HEAD` i listy
# `git status --porcelain`. Zmiana TREŚCI pliku, który nie jest jeszcze śledzony,
# nie zmieni wersji. `git add` załatwia sprawę.
#
# build.rs woła ten skrypt, a `build-image.sh` eksportuje wynik jako T5_VERSION,
# więc obraz i `ota.json` nie mają jak się rozjechać.
set -euo pipefail

cd "$(dirname "$0")/.."

pkg=$(grep -m1 '^version' firmware/Cargo.toml | cut -d'"' -f2)

if ! sha=$(git rev-parse --short=7 HEAD 2>/dev/null); then
    # Nie repozytorium albo brak commitów — zostaje sam semver.
    printf '%s' "$pkg"
    exit 0
fi

dirty=""
status=$(git status --porcelain 2>/dev/null || true)
if [[ -n "$status" ]]; then
    dirty=".$( { git diff HEAD 2>/dev/null || true; printf '%s' "$status"; } | sha1sum | cut -c1-6)"
fi

printf '%s+g%s%s' "$pkg" "$sha" "$dirty"
