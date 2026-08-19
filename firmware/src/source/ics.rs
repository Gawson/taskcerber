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
}

impl IcsSource {
    pub fn new(url: impl Into<String>, home: Tz, tag: SourceTag, label: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            home,
            tag,
            label: label.into(),
        }
    }
}

impl EventSource for IcsSource {
    fn name(&self) -> &str {
        &self.label
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

/// Ukrywa tajny fragment adresu w logach.
fn redact(url: &str) -> String {
    match url.find("/private-") {
        Some(i) => format!("{}/private-***", &url[..i]),
        None => url.to_string(),
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
}

impl StreamingCrc {
    fn new() -> Self {
        Self {
            state: 0xFFFF_FFFF,
            len: 0,
        }
    }

    fn update(&mut self, data: &[u8]) {
        self.len += data.len();
        for &byte in data {
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
