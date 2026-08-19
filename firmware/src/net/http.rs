//! Klient HTTPS na esp-idf-svc, z mostkiem do `std::io::Read`.
//!
//! **To jest powód, dla którego cały ten projekt stoi na ESP-IDF.** mbedTLS z ESP-IDF
//! niesie pełny bundle certyfikatów i weryfikuje dowolnego publicznego hosta.
//! Odpowiednik w `no_std` — `embedded-tls` — ma `MAX_SAN_DNS_NAMES = 3`, a certyfikat
//! `*.googleapis.com` wymienia tę nazwę jako szóstą z siedemnastu, więc weryfikacja
//! nazwy hosta nie ma prawa przejść.
//!
//! # Zatrzask błędu
//!
//! [`ResponseReader`] implementuje `std::io::Read`, ale **zapamiętuje pierwszy błąd**.
//! Powód jest konkretny: parsery strumieniowe mają skłonność do traktowania błędu
//! odczytu jak końca pliku. Urwane pobieranie kalendarza wygląda wtedy dokładnie tak
//! samo jak kalendarz, w którym nic już nie ma — i po cichu gubisz wydarzenia.
//! Po zakończeniu czytania **zawsze** sprawdź [`ResponseReader::error`].

use std::io::{self, Read};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use embedded_svc::http::client::Client;
use embedded_svc::io::Read as SvcRead;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection, FollowRedirectsPolicy};
use esp_idf_svc::sys::esp_crt_bundle_attach;
use log::{info, warn};

/// Limit czasu pojedynczego żądania. **Obowiązkowy**: pętla przekierowań w
/// esp-idf-svc nie ma własnego licznika skoków, więc jedynym ogranicznikiem
/// zapętlonego przekierowania jest czas.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(25);

/// Górny limit rozmiaru odpowiedzi, żeby patologiczny kanał nie zjadł sterty.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Otwiera połączenie HTTPS z weryfikacją certyfikatu z bundle'a ESP-IDF.
pub fn connection() -> Result<EspHttpConnection> {
    EspHttpConnection::new(&Configuration {
        // Pełny bundle CA z ESP-IDF — to on weryfikuje Google.
        crt_bundle_attach: Some(esp_crt_bundle_attach),
        timeout: Some(REQUEST_TIMEOUT),
        // Prywatny URL iCal Google przekierowuje; bez tego dostajesz 302 i nic więcej.
        follow_redirects_policy: FollowRedirectsPolicy::FollowGetHead,
        buffer_size: Some(4096),
        buffer_size_tx: Some(1024),
        ..Default::default()
    })
    .context("nie mogę utworzyć połączenia HTTP")
}

/// Wykonuje GET i oddaje czytnik treści.
///
/// Zwrócony czytnik trzyma połączenie; czytaj go do końca, a potem sprawdź
/// [`ResponseReader::error`].
pub fn get(url: &str) -> Result<ResponseReader> {
    let conn = connection()?;
    let mut client = Client::wrap(conn);

    let request = client
        .request(
            embedded_svc::http::Method::Get,
            url,
            &[
                ("Accept", "text/calendar, application/json, */*"),
                // Bez kompresji: rozpakowywanie w locie kosztuje pamięć, a różnica
                // w czasie radia nie jest warta dodatkowego bufora.
                ("Accept-Encoding", "identity"),
                ("Connection", "close"),
            ],
        )
        .context("nie mogę zbudować żądania")?;

    let response = request.submit().context("żądanie nie doszło")?;
    let status = response.status();

    if !(200..300).contains(&status) {
        bail!("serwer odpowiedział kodem {status}");
    }

    let content_length = response
        .header("Content-Length")
        .and_then(|v| v.parse::<usize>().ok());
    info!("HTTP {status}, Content-Length: {content_length:?}");

    Ok(ResponseReader {
        client,
        error: None,
        read_total: 0,
        content_length,
    })
}

/// Mostek między `embedded_svc::io::Read` a `std::io::Read`, z zatrzaskiem błędu.
pub struct ResponseReader {
    client: Client<EspHttpConnection>,
    error: Option<io::Error>,
    read_total: usize,
    content_length: Option<usize>,
}

impl ResponseReader {
    /// Pierwszy napotkany błąd odczytu, jeśli był.
    ///
    /// **Sprawdź to zawsze po zakończeniu parsowania.**
    pub fn error(&self) -> Option<&io::Error> {
        self.error.as_ref()
    }

    /// Ile bajtów faktycznie przeczytano.
    pub fn bytes_read(&self) -> usize {
        self.read_total
    }

    /// Czy przeczytano tyle, ile zapowiadał `Content-Length`.
    ///
    /// Zwraca `None`, gdy serwer nie podał długości — Google przy kanałach iCal
    /// nie podaje, więc jedynym dowodem kompletności jest `END:VCALENDAR`.
    pub fn length_matches(&self) -> Option<bool> {
        self.content_length
            .map(|expected| expected == self.read_total)
    }
}

impl Read for ResponseReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some(e) = &self.error {
            return Err(io::Error::new(e.kind(), "połączenie już zawiodło"));
        }
        if self.read_total >= MAX_BODY_BYTES {
            let e = io::Error::new(
                io::ErrorKind::InvalidData,
                "odpowiedź przekroczyła dopuszczalny rozmiar",
            );
            self.error = Some(io::Error::new(e.kind(), e.to_string()));
            return Err(e);
        }

        match SvcRead::read(&mut self.client.connection(), buf) {
            Ok(n) => {
                self.read_total += n;
                Ok(n)
            }
            Err(e) => {
                warn!("błąd odczytu HTTP po {} bajtach: {e:?}", self.read_total);
                let err = io::Error::new(io::ErrorKind::Other, format!("{e:?}"));
                self.error = Some(io::Error::new(err.kind(), err.to_string()));
                Err(err)
            }
        }
    }
}
