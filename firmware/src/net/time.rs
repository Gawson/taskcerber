//! Ustalanie czasu: RTC sprzętowy, potem SNTP.
//!
//! Kolejność jest istotna. Urządzenie musi znać czas **zanim podniesie radio** —
//! inaczej każde wybudzenie zaczyna się od czekania na sieć, zanim cokolwiek narysuje.
//! Zegar PCF8563 z bateryjką podtrzymującą kosztuje ~0,3 µA i pamięta czas nawet
//! wtedy, gdy ogniwo główne padnie do zera.
//!
//! SNTP wchodzi dopiero po nawiązaniu połączenia i zapisuje wynik z powrotem do RTC,
//! ale tylko przy dryfie większym niż próg — zapis do RTC to transakcja I²C, a przy
//! kilkudziesięciu wybudzeniach dziennie nie ma powodu robić jej za każdym razem.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono::{NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use esp_idf_svc::sntp::{EspSntp, SntpConf, SyncStatus};
use log::{info, warn};

use crate::board::pcf8563::Pcf8563;

/// Powyżej tego dryfu zapisujemy czas z powrotem do RTC.
const RTC_WRITEBACK_THRESHOLD_S: i64 = 30;

/// Ile czekamy na synchronizację SNTP, zanim odpuścimy i pójdziemy dalej.
const SNTP_TIMEOUT: Duration = Duration::from_secs(10);

/// Zakres, w którym czas uznajemy za wiarygodny. Poza nim traktujemy odczyt jako śmieć.
const PLAUSIBLE_FROM: i64 = 1_767_225_600; // 2026-01-01
const PLAUSIBLE_TO: i64 = 2_398_291_200; // 2046-01-01

/// Skąd wziął się czas, którym dysponujemy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeSource {
    /// Zsynchronizowany z SNTP w tym wybudzeniu.
    Sntp,
    /// Odczytany ze sprzętowego RTC.
    Rtc,
    /// Odtworzony z pamięci RTC-FAST — najsłabsze źródło.
    LastKnown,
    /// Nie wiadomo która godzina.
    Unknown,
}

/// Wczytuje czas ze sprzętowego RTC do zegara systemowego.
///
/// Wołane na samym początku bootu, przed radiem.
pub fn seed_from_rtc(rtc: &Pcf8563, home: Tz) -> TimeSource {
    match rtc.now() {
        Ok(Some(local)) => {
            let Some(utc) = local_to_unix(local, home) else {
                warn!("czas z RTC nie daje się przeliczyć na UTC");
                return TimeSource::Unknown;
            };
            if !(PLAUSIBLE_FROM..PLAUSIBLE_TO).contains(&utc) {
                warn!("czas z RTC poza wiarygodnym zakresem ({utc}) — ignoruję");
                return TimeSource::Unknown;
            }
            set_system_time(utc);
            info!("czas z RTC: {local}");
            TimeSource::Rtc
        }
        Ok(None) => {
            warn!("RTC zgłasza utratę zasilania — czas nieznany");
            TimeSource::Unknown
        }
        Err(e) => {
            warn!("nie mogę odczytać RTC: {e:#}");
            TimeSource::Unknown
        }
    }
}

/// Synchronizuje z SNTP i przy istotnym dryfie zapisuje wynik do RTC.
///
/// Wymaga działającego połączenia sieciowego. Nie jest błędem krytycznym, gdy się
/// nie uda — mamy wtedy czas z RTC.
pub fn sync_sntp(rtc: &Pcf8563, home: Tz) -> Result<TimeSource> {
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let sntp = EspSntp::new(&SntpConf::default()).context("nie mogę wystartować SNTP")?;

    let deadline = Instant::now() + SNTP_TIMEOUT;
    while sntp.get_sync_status() != SyncStatus::Completed {
        if Instant::now() > deadline {
            warn!("SNTP nie zdążył w {SNTP_TIMEOUT:?} — zostaję przy czasie z RTC");
            return Ok(TimeSource::Rtc);
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let drift = (after - before).abs();
    info!("SNTP zsynchronizowany, dryf {drift} s");

    if drift > RTC_WRITEBACK_THRESHOLD_S {
        let local = home
            .timestamp_opt(after, 0)
            .single()
            .map(|d| d.naive_local());
        if let Some(local) = local {
            match rtc.set(local) {
                Ok(()) => info!("zapisano czas do RTC: {local}"),
                Err(e) => warn!("nie mogę zapisać czasu do RTC: {e:#}"),
            }
        }
    }

    Ok(TimeSource::Sntp)
}

/// Aktualny czas lokalny.
pub fn now_local(home: Tz) -> Option<NaiveDateTime> {
    let unix = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    if !(PLAUSIBLE_FROM..PLAUSIBLE_TO).contains(&unix) {
        return None;
    }
    home.timestamp_opt(unix, 0)
        .single()
        .map(|d| d.naive_local())
}

/// Aktualny czas unix.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn local_to_unix(local: NaiveDateTime, home: Tz) -> Option<i64> {
    home.from_local_datetime(&local)
        .earliest()
        .map(|d| d.timestamp())
}

fn set_system_time(unix: i64) {
    let tv = esp_idf_svc::sys::timeval {
        tv_sec: unix as _,
        tv_usec: 0,
    };
    // SAFETY: prosta struktura POD przekazana do settimeofday.
    unsafe {
        esp_idf_svc::sys::settimeofday(&tv, std::ptr::null());
    }
}
