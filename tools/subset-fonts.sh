#!/usr/bin/env bash
# Przycina Noto Sans do Latin + Latin Extended-A (komplet polskich znaków).
# 620 KB -> ~30 KB na krój. Wymaga fonttools: pip install fonttools
set -euo pipefail
cd "$(dirname "$0")/.."

SRC=${1:-/usr/share/fonts/google-noto}
UNI='U+0020-007E,U+00A0-00FF,U+0100-017F,U+2010-2027,U+2030-205E,U+20AC,U+2122,U+2190-2193,U+2022,U+25A0-25CF,U+2600-26FF'

for f in Regular Medium Bold; do
  pyftsubset "$SRC/NotoSans-$f.ttf" \
    --unicodes="$UNI" \
    --layout-features='kern,liga,ccmp,mark,mkmk' \
    --no-hinting --desubroutinize \
    --output-file="dashboard/fonts/NotoSans-$f.subset.ttf"
done
ls -la dashboard/fonts/
