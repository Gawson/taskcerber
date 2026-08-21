//! Kalendarz z prywatnego URL iCal Google.
//!
//! Adres ma postać:
//! `https://calendar.google.com/calendar/ical/<id>/private-<klucz>/basic.ics`
//!
//! Zalety: żadnego OAuth, żadnego ekranu zgody, żadnej pułapki z siedmiodniowym
//! wygasaniem tokenu odświeżającego w aplikacjach o statusie „Testing".
//!
//! Wada, którą trzeba znać: to jest **stały bearer do całego kalendarza**, bez
//! zakresu i bez terminu ważności. Kto ma link, ma kalendarz. Dlatego adres siedzi
//! w NVS, a nie w binarce — patrz [`crate::store`].
//!
//! Ograniczenie techniczne: Google **nie pozwala** filtrować tego kanału po dacie
//! ani nie wystawia ETag / Last-Modified (nagłówki mówią `no-cache, no-store`),
//! więc warunkowe GET-y nie wchodzą w grę. Za każdym razem płacimy za pełne pobranie
//! i dopiero po stronie urządzenia odsiewamy okno. Stąd parser strumieniowy.

use std::io::{BufReader, Read};

use anyhow::{bail, Context, Result};
use chrono::NaiveDateTime;
use chrono_tz::Tz;
use dashboard::model::{CalEvent, SourceTag};
use devlogic::redact;
use icalfeed::{parse_feed, FeedError, Window};
use log::{info, warn};

use crate::net::http;
use crate::power::rtc_state::crc32;

use super::{Downloaded, EventSource};

/// Ile bajtów rezerwujemy z góry na treść kanału.
///
/// Google nie podaje `Content-Length` (odpowiedź jest kawałkowana), więc bez rezerwacji
/// megabajtowy bufor realokowałby się kilkanaście razy, kopiując za każdym razem całość.
const REZERWA_BUFORA: usize = 512 * 1024;

/// Górny limit treści kanału. Dwa źródła po tyle wciąż mieszczą się w 8 MB PSRAM-u.
const MAX_BODY: usize = 2 * 1024 * 1024;

pub struct IcsSource {
    url: String,
    home: Tz,
    tag: SourceTag,
    label: String,
    /// Ile dni do przodu pobierać z tego konkretnego kanału — patrz
    /// [`crate::source::EventSource::horizon_days`].
    horizon_days: i64,
}

impl IcsSource {
    pub fn new(
        url: impl Into<String>,
        home: Tz,
        tag: SourceTag,
        label: impl Into<String>,
        horizon_days: i64,
    ) -> Self {
        Self {
            url: url.into(),
            home,
            tag,
            label: label.into(),
            horizon_days,
        }
    }
}

impl EventSource for IcsSource {
    fn name(&self) -> &str {
        &self.label
    }

    fn horizon_days(&self) -> i64 {
        self.horizon_days
    }

    fn download(&self) -> Result<Downloaded> {
        info!("pobieram kanał iCal: {}", redact(&self.url));

        let mut reader = http::get(&self.url).context("nie mogę pobrać kanału")?;

        // Zbieramy CAŁOŚĆ do pamięci, zamiast parsować w locie. Bufor rośnie ponad
        // 4 KB, więc `SPIRAM_MALLOC_ALWAYSINTERNAL` przenosi go do PSRAM-u — a tam
        // 1,18 MB to 15% z ośmiu megabajtów i nikomu nie wchodzi w drogę.
        // Rezerwacja z góry, bo Google nie podaje Content-Length (odpowiedź jest
        // kawałkowana), a rośnięcie od zera realokowałoby megabajt kilkanaście razy.
        let mut body: Vec<u8> = Vec::with_capacity(REZERWA_BUFORA);
        let mut kawalek = [0u8; 4096];
        loop {
            let n = reader.read(&mut kawalek).context("błąd odczytu kanału")?;
            if n == 0 {
                break;
            }
            if body.len() + n > MAX_BODY {
                bail!(
                    "kanał przekroczył {} MB — nie pobieram dalej",
                    MAX_BODY / (1024 * 1024)
                );
            }
            body.extend_from_slice(&kawalek[..n]);
        }

        // Zatrzask błędu strumienia MUSI być sprawdzony: urwane pobranie wygląda
        // dla pętli `read` dokładnie jak koniec pliku.
        if let Some(e) = reader.error() {
            bail!("połączenie zawiodło w trakcie pobierania: {e}");
        }
        if let Some(false) = reader.length_matches() {
            warn!("liczba przeczytanych bajtów nie zgadza się z Content-Length");
        }

        let mut hasher = StreamingCrc::new();
        hasher.update(&body);
        if !hasher.saw_vcalendar() {
            // Podpowiedź celowo OPISUJE adres, zamiast pokazywać jego wzór. Dosłowny
            // szablon wpada w skaner sekretów z tools/check-image.sh, który nie
            // odróżnia przykładu od prawdziwego klucza — i słusznie, bo nie ma jak.
            bail!(
                "to nie jest kanał iCal — serwer odpowiedział czymś innym, \
                 najczęściej stroną logowania. Weź adres z: Google Kalendarz -> \
                 Ustawienia kalendarza -> Integracja kalendarza -> \
                 „Prywatny adres w formacie iCal”. Musi kończyć się na basic.ics; \
                 link skopiowany z paska przeglądarki NIE jest kanałem iCal."
            )
        }

        info!("kanał {}: {} B pobrane", self.label, body.len());
        Ok(Downloaded {
            content_crc: hasher.finish(),
            body,
        })
    }

    fn parse(&self, body: &[u8], from: NaiveDateTime, to: NaiveDateTime) -> Result<Vec<CalEvent>> {
        let window = Window {
            start: from,
            end: to,
        };
        match parse_feed(
            BufReader::with_capacity(4096, body),
            window,
            self.home,
            self.tag,
        ) {
            Ok(events) => {
                info!("kanał {}: {} wydarzeń", self.label, events.len());
                Ok(events)
            }
            Err(FeedError::Truncated) => {
                bail!("pobieranie urwane — brak END:VCALENDAR, dane byłyby niekompletne")
            }
            Err(FeedError::Io(e)) => bail!("błąd odczytu kanału: {e}"),
        }
    }
}

/// Czytnik przepuszczający dane i liczący po drodze CRC.
struct Tee<'a, R> {
    inner: &'a mut R,
    crc: &'a mut StreamingCrc,
}

impl<R: std::io::Read> std::io::Read for Tee<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.crc.update(&buf[..n]);
        Ok(n)
    }
}

/// CRC32 liczone przyrostowo.
struct StreamingCrc {
    state: u32,
    len: usize,
    /// Ile bajtów `BEGIN:VCALENDAR` dopasowano do tej pory.
    ///
    /// Licznik, a nie flaga na buforze, bo strumień przychodzi kawałkami po 4 KB
    /// i nagłówek może wypaść na granicy dwóch odczytów.
    naglowek: usize,
}

impl StreamingCrc {
    fn new() -> Self {
        Self {
            state: 0xFFFF_FFFF,
            len: 0,
            naglowek: 0,
        }
    }

    /// Czy w strumieniu pojawiło się `BEGIN:VCALENDAR`.
    ///
    /// Odróżnia „pobranie się urwało" od „to w ogóle nie był kalendarz". Bez tego
    /// obie sytuacje dają ten sam komunikat o braku `END:VCALENDAR`, a prowadzą do
    /// zupełnie różnych działań: pierwsza do ponowienia, druga do poprawienia adresu.
    fn saw_vcalendar(&self) -> bool {
        self.naglowek >= Self::IGLA.len()
    }

    const IGLA: &'static [u8] = b"BEGIN:VCALENDAR";

    fn update(&mut self, data: &[u8]) {
        self.len += data.len();
        for &byte in data {
            if self.naglowek < Self::IGLA.len() {
                // Bez cofania: kanał iCal zaczyna się tym nagłówkiem, więc fałszywy
                // start w rodzaju „BEGIN:BEGIN:VCALENDAR" nas nie interesuje —
                // pytanie brzmi „czy to w ogóle kalendarz", nie „gdzie dokładnie".
                self.naglowek = if byte == Self::IGLA[self.naglowek] {
                    self.naglowek + 1
                } else if byte == Self::IGLA[0] {
                    1
                } else {
                    0
                };
            }
            self.state ^= byte as u32;
            for _ in 0..8 {
                let mask = (self.state & 1).wrapping_neg();
                self.state = (self.state >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }

    fn finish(&self) -> u32 {
        !self.state
    }

    fn len(&self) -> usize {
        self.len
    }
}

#[allow(dead_code)]
fn _crc_agreement_check(data: &[u8]) -> bool {
    let mut s = StreamingCrc::new();
    s.update(data);
    s.finish() == crc32(data)
}
