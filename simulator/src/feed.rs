//! Wczytywanie prawdziwych danych kalendarzowych do symulatora.
//!
//! To jest ta część, dla której warto mieć symulator: testujesz integrację ze swoim
//! **prawdziwym** kalendarzem, na komputerze, bez płytki. Jeśli twój kanał ma coś,
//! czego parser nie ogarnia — dowiesz się tutaj, w sekundę, a nie po dwóch minutach
//! flashowania i patrzeniu na ścianę.
//!
//! Używany jest ten sam crate `icalfeed`, który idzie do firmware'u, więc wynik
//! jest identyczny.

use std::io::BufReader;

use chrono::{Duration, Local, NaiveDateTime};
use dashboard::model::{DayGroup, SourceTag, Tile};
use dashboard::Model;
use icalfeed::{parse_feed, Window};

/// Strefa domowa symulatora. Ta sama, co domyślna w firmware.
const HOME_TZ: chrono_tz::Tz = chrono_tz::Europe::Warsaw;

/// Pobiera kanał iCal przez HTTPS.
pub fn from_url(url: &str, days: i64) -> Result<Model, Box<dyn std::error::Error>> {
    let response = ureq::get(url).call()?;
    let reader = response.into_body().into_reader();
    build(BufReader::new(reader), days)
}

/// Wczytuje kanał z pliku na dysku.
pub fn from_file(path: &str, days: i64) -> Result<Model, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    build(BufReader::new(file), days)
}

fn build<R: std::io::BufRead>(reader: R, days: i64) -> Result<Model, Box<dyn std::error::Error>> {
    let now = Local::now().naive_local();
    let from = now.date().and_hms_opt(0, 0, 0).unwrap_or(now);
    let to = from + Duration::days(days);

    let events = parse_feed(
        reader,
        Window {
            start: from,
            end: to,
        },
        HOME_TZ,
        SourceTag::Primary,
    )?;

    let mut model = Model::empty(now);
    model.firmware = format!("symulator {}", env!("CARGO_PKG_VERSION"));
    model.days = group_by_day(events);
    model.battery = dashboard::model::Battery {
        percent: Some(78),
        millivolts: Some(3920),
        charging: false,
    };
    model.tiles = vec![Tile::new("wydarzeń", model.event_count().to_string())];
    Ok(model)
}

/// Grupowanie po dniach — identyczne z tym w firmware.
fn group_by_day(events: Vec<dashboard::model::CalEvent>) -> Vec<DayGroup> {
    let mut groups: Vec<DayGroup> = Vec::new();
    for event in events {
        let date = event.start.date();
        match groups.last_mut() {
            Some(g) if g.date == date => g.events.push(event),
            _ => groups.push(DayGroup {
                date,
                events: vec![event],
            }),
        }
    }
    groups
}

/// Maskowanie adresów mieszka w `devlogic`, bo ten sam kod ukrywa sekret
/// w logu firmware'u (konsola konfiguracyjna) i tutaj, w pasku symulatora.
pub use devlogic::redact;

#[allow(dead_code)]
fn _unused(_: NaiveDateTime) {}
