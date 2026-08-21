#!/usr/bin/env bash
# Wystawia webflashera pod adresem w sieci lokalnej, żeby dało się wgrać firmware
# z innego komputera niż ten, na którym się buduje.
#
# Dlaczego HTTPS, skoro to sieć domowa: Web Serial jest dostępny wyłącznie
# w bezpiecznym kontekście. `localhost` jest bezpieczny z definicji,
# `http://192.168.x.x` — nie. Przeglądarka po prostu nie pokaże listy portów.
# Certyfikat podpisany przez samego siebie wystarcza: po przyjęciu ostrzeżenia
# Chrome uznaje stronę za bezpieczny kontekst i `navigator.serial` działa.
#
# Klucz prywatny leży w .flasher-tls/, NIE w dist/ — dist/ jedzie na GitHub Pages.
set -euo pipefail

cd "$(dirname "$0")/.."

PORT=${PORT:-8443}
REDIRECT_PORT=${REDIRECT_PORT:-8080}
OUT=${OUT:-dist}
TLS_DIR=.flasher-tls
CRT="$TLS_DIR/cert.pem"
KEY="$TLS_DIR/key.pem"
SAN_FILE="$TLS_DIR/san.txt"

[[ -f "$OUT/firmware.bin" ]] || {
  echo "BŁĄD: brak $OUT/firmware.bin — uruchom najpierw ./tools/build-image.sh" >&2
  exit 1
}

echo "==> kompletuję stronę"
./tools/stage-page.sh "$OUT"

# ── Adresy, pod którymi ta maszyna jest widoczna ────────────────────────────────
# Chrome ignoruje CN i patrzy wyłącznie na SAN, a dla adresu wpisanego liczbami
# musi tam być wpis typu IP:, nie DNS:. Bez tego dostaniesz błąd, którego nie da
# się kliknąć na bok (ERR_CERT_COMMON_NAME_INVALID bez opcji „mimo to").
mapfile -t IPS < <(ip -4 -o addr show scope global 2>/dev/null | awk '{sub(/\/.*/,"",$4); print $4}')
HOST=$(hostname)

SANS=("DNS:localhost" "IP:127.0.0.1" "DNS:$HOST" "DNS:$HOST.local")
for ip in "${IPS[@]}"; do SANS+=("IP:$ip"); done
if TS=$(tailscale status --json 2>/dev/null | python3 -c \
        'import sys,json; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))' 2>/dev/null) \
   && [[ -n "$TS" ]]; then
  SANS+=("DNS:$TS")
fi
SAN=$(printf '%s\n' "${SANS[@]}" | sort -u | paste -sd, -)

# ── Certyfikat ─────────────────────────────────────────────────────────────────
# Regeneracja tylko wtedy, gdy naprawdę trzeba. Nowy certyfikat = nowe ostrzeżenie
# w przeglądarce po drugiej stronie, więc zmiana adresu IP jest jedynym powodem,
# dla którego warto zmusić kogoś do ponownego klikania.
need_cert=0
if [[ ! -f "$CRT" || ! -f "$KEY" ]]; then
  need_cert=1; why="brak certyfikatu"
elif [[ ! -f "$SAN_FILE" ]] || [[ "$(cat "$SAN_FILE")" != "$SAN" ]]; then
  need_cert=1; why="zmieniły się adresy tej maszyny"
elif ! openssl x509 -in "$CRT" -noout -checkend $((30 * 24 * 3600)) >/dev/null 2>&1; then
  need_cert=1; why="certyfikat wygasa w ciągu 30 dni"
fi

if (( need_cert )); then
  echo "==> generuję certyfikat ($why)"
  mkdir -p "$TLS_DIR"; chmod 700 "$TLS_DIR"
  # 397 dni: poniżej limitu 398, którego Chrome pilnuje. Krzywa P-256 zamiast RSA
  # — szybciej się generuje i jest akceptowana wszędzie, gdzie działa Web Serial.
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 \
    -sha256 -days 397 -nodes \
    -keyout "$KEY" -out "$CRT" \
    -subj "/CN=taskcerber flasher ($HOST)" \
    -addext "subjectAltName=$SAN" \
    -addext "basicConstraints=critical,CA:FALSE" \
    -addext "keyUsage=critical,digitalSignature,keyEncipherment" \
    -addext "extendedKeyUsage=serverAuth" 2>/dev/null
  chmod 600 "$KEY"
  printf '%s' "$SAN" > "$SAN_FILE"
fi

FP=$(openssl x509 -in "$CRT" -noout -fingerprint -sha256 | cut -d= -f2)

# ── Co użytkownik ma zrobić ────────────────────────────────────────────────────
echo
echo "Otwórz na drugim komputerze (Chrome, Edge albo Firefox 151+):"
for ip in "${IPS[@]}"; do
  case "$ip" in
    100.*) echo "    https://$ip:$PORT/   (tailscale)" ;;
    *)     echo "    https://$ip:$PORT/" ;;
  esac
done
echo "    https://$HOST.local:$PORT/   (jeśli działa mDNS)"
echo
echo "Chrome pokaże ostrzeżenie o certyfikacie — to oczekiwane, certyfikat jest"
echo "podpisany przez samego siebie. Kliknij Zaawansowane → Przejdź do..."
echo "Po przyjęciu wyjątku Web Serial działa normalnie."
echo
echo "Odcisk SHA-256 (do porównania w oknie certyfikatu, jeśli chcesz sprawdzić):"
echo "    $FP"
echo
echo "Wpisanie samego adresu bez https:// wpada na port $REDIRECT_PORT i zostaje"
echo "przekierowane — nie trzeba pamiętać o schemacie."
echo

exec python3 tools/flasher-server.py \
  --dir "$OUT" --port "$PORT" --redirect-port "$REDIRECT_PORT" \
  --cert "$CRT" --key "$KEY"
