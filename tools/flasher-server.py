#!/usr/bin/env python3
"""Serwer HTTPS dla webflashera.

Istnieje z jednego powodu: Web Serial działa tylko w bezpiecznym kontekście.
`localhost` jest bezpieczny z definicji, `http://192.168.x.x` nie jest — i żadna
flaga po stronie serwera tego nie zmieni. Trzeba HTTPS, choćby na certyfikacie
podpisanym przez samego siebie. Chrome pokaże ostrzeżenie, ale po jego przyjęciu
`isSecureContext` jest prawdziwe i `navigator.serial` istnieje.

`python3 -m http.server --tls-cert` umie to od Pythona 3.14. Ten plik robi to samo
na 3.8+, a przy okazji dokłada dwie rzeczy, które oszczędzają telefon:

  Cache-Control: no-store   Przeglądarka po drugiej stronie potrafi trzymać stary
                            firmware.bin i wgrywać poprzedni build bez słowa.
  przekierowanie z HTTP     Wpisany w pasku `192.168.1.152:8443` idzie po http
                            i kończy się pustą stroną. Nasłuch na osobnym porcie
                            odbija na https zamiast tego.
"""

from __future__ import annotations

import argparse
import http.server
import socket
import ssl
import sys
import threading


class NoStoreHandler(http.server.SimpleHTTPRequestHandler):
    """Zwykłe serwowanie plików, ale bez cache'owania i bez reklamowania Pythona."""

    server_version = "t5s3pro-flasher"
    sys_version = ""

    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store, must-revalidate")
        super().end_headers()

    def log_message(self, fmt: str, *args) -> None:
        sys.stderr.write("  %s  %s\n" % (self.address_string(), fmt % args))


class RedirectHandler(http.server.BaseHTTPRequestHandler):
    """Odbija http://host:N/cokolwiek na https://host:PORT/cokolwiek."""

    server_version = "t5s3pro-flasher"
    sys_version = ""
    https_port = 8443

    def do_GET(self) -> None:  # noqa: N802
        host = (self.headers.get("Host") or "").rsplit(":", 1)[0]
        if not host:
            host = self.server.server_address[0]
        if ":" in host and not host.startswith("["):  # goły IPv6
            host = f"[{host}]"
        self.send_response(308)
        self.send_header("Location", f"https://{host}:{self.https_port}{self.path}")
        self.send_header("Content-Length", "0")
        self.end_headers()

    do_HEAD = do_GET

    def log_message(self, fmt: str, *args) -> None:
        pass


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True
    # socketserver domyślnie kolejkuje 5 połączeń. Chrome otwiera do 6 na host,
    # a strona ciągnie install-button.js, dwa chunki, manifest i 3 MB obrazu —
    # przy piątce część SYN-ów leci do kosza i czeka na retransmisję.
    request_queue_size = 64


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--dir", default="dist")
    ap.add_argument("--bind", default="0.0.0.0")
    ap.add_argument("--port", type=int, default=8443)
    ap.add_argument("--cert", required=True)
    ap.add_argument("--key", required=True)
    ap.add_argument(
        "--redirect-port",
        type=int,
        default=8080,
        help="port HTTP przekierowujący na HTTPS; 0 wyłącza",
    )
    a = ap.parse_args()

    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(a.cert, a.key)

    def handler(*args, **kw):
        return NoStoreHandler(*args, directory=a.dir, **kw)

    try:
        httpd = Server((a.bind, a.port), handler)
    except OSError as e:
        print(f"nie mogę zająć {a.bind}:{a.port} — {e}", file=sys.stderr)
        return 1
    httpd.socket = ctx.wrap_socket(httpd.socket, server_side=True)

    if a.redirect_port:
        RedirectHandler.https_port = a.port
        try:
            redirect = Server((a.bind, a.redirect_port), RedirectHandler)
        except OSError as e:
            print(f"  (przekierowanie z portu {a.redirect_port} nieaktywne — {e})", file=sys.stderr)
        else:
            threading.Thread(target=redirect.serve_forever, daemon=True).start()

    print(f"serwuję {a.dir}/ na {a.bind}:{a.port} — Ctrl-C kończy", file=sys.stderr)
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        print("\nkoniec", file=sys.stderr)
    return 0


if __name__ == "__main__":
    socket.setdefaulttimeout(30)
    sys.exit(main())
