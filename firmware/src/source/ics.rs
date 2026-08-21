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

use std::io::BufReader;

use anyhow::{bail, Context, Result};
use chrono::NaiveDateTime;
use chrono_tz::Tz;
use dashboard::model::SourceTag;
use devlogic::redact;
use icalfeed::{parse_feed, FeedError, Window};
use log::{info, warn};

use crate::net::http;
use crate::power::rtc_state::crc32;

use super::{EventSource, FetchResult};

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

    fn fetch(&self, from: NaiveDateTime, to: NaiveDateTime) -> Result<FetchResult> {
        info!("pobieram kanał iCal: {}", redact(&self.url));

        let reader = http::get(&self.url).context("nie mogę pobrać kanału")?;

        // Bufor 4 KB: parser i tak czyta liniami, a większy tylko zabiera DRAM
        // buforom mbedTLS.
        let mut buffered = BufReader::with_capacity(4096, reader);

        // CRC liczymy w locie na strumieniu, nie na zebranej całości —
        // kalendarz może mieć setki kilobajtów.
        let mut hasher = StreamingCrc::new();
        let tee = Tee {
            inner: &mut buffered,
            crc: &mut hasher,
        };

        let window = Window {
            start: from,
            end: to,
        };
        let events = match parse_feed(
            BufReader::with_capacity(4096, tee),
            window,
            self.home,
            self.tag,
        ) {
            Ok(e) => e,
            Err(FeedError::Truncated) => {
                // Brak END:VCALENDAR znaczy jedno z dwóch i warto je rozróżnić, bo
                // prowadzą do zupełnie różnych działań. Zero przeczytanych bajtów
                // z BEGIN:VCALENDAR to nie urwane pobranie, tylko ODPOWIEDŹ, KTÓRA
                // NIE JEST KALENDARZEM — typowo strona logowania Google, bo ktoś
                // wkleił link do kalendarza z paska przeglądarki zamiast adresu ICS.
                if !hasher.saw_vcalendar() {
                    // Podpowiedź celowo OPISUJE adres, zamiast pokazywać jego wzór.
                    // Dosłowny szablon z „/calendar/ical/.../private-" wpada w skaner
                    // sekretów z tools/check-image.sh, który nie odróżnia przykładu
                    // od prawdziwego klucza — i słusznie, bo nie ma jak.
                    bail!(
                        "to nie jest kanał iCal — serwer odpowiedział czymś innym, \
                         najczęściej stroną logowania. Weź adres z: Google Kalendarz -> \
                         Ustawienia kalendarza -> Integracja kalendarza -> \
                         „Prywatny adres w formacie iCal”. Musi kończyć się na basic.ics; \
                         link skopiowany z paska przeglądarki NIE jest kanałem iCal."
                    )
                }
                bail!("pobieranie urwane — brak END:VCALENDAR, dane byłyby niekompletne")
            }
            Err(FeedError::Io(e)) => bail!("błąd odczytu kanału: {e}"),
        };

        let crc = hasher.finish();
        let bytes = hasher.len();

        // Podwójne zabezpieczenie: parser sprawdza END:VCALENDAR, a tu sprawdzamy,
        // czy transport nie zatrzasnął błędu po drodze.
        let reader = buffered.into_inner();
        if let Some(e) = reader.error() {
            bail!("połączenie zawiodło w trakcie pobierania: {e}");
        }
        if let Some(false) = reader.length_matches() {
            warn!("liczba przeczytanych bajtów nie zgadza się z Content-Length");
        }

        info!(
            "kanał {}: {} wydarzeń z {} bajtów",
            self.label,
            events.len(),
            bytes
        );

        Ok(FetchResult {
            events,
            content_crc: crc,
            bytes,
        })
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
