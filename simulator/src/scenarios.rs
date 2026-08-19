//! Scenariusze demonstracyjne — do pracy nad układem graficznym bez sieci.
//!
//! `edge_cases` istnieje po to, żeby przypadki, które psują układ, były na wyciągnięcie
//! jednego klawisza: bardzo długie tytuły, znaki spoza podstawowej łacinki, wydarzenia
//! o zerowym czasie trwania, nachodzące na siebie, przez północ.

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use dashboard::model::{Battery, CalEvent, DayGroup, NetState, SourceTag, Tile};
use dashboard::Model;

/// Bieżąca data z realnym „teraz", żeby nagłówki „dziś/jutro" miały sens.
fn today() -> NaiveDate {
    Local::now().date_naive()
}

fn now() -> NaiveDateTime {
    Local::now().naive_local()
}

fn at(day_offset: i64, h: u32, m: u32) -> NaiveDateTime {
    (today() + Duration::days(day_offset))
        .and_hms_opt(h, m, 0)
        .unwrap()
}

fn ev(day: i64, sh: u32, sm: u32, eh: u32, em: u32, title: &str) -> CalEvent {
    CalEvent {
        start: at(day, sh, sm),
        end: at(day, eh, em),
        all_day: false,
        title: title.to_string(),
        location: None,
        source: SourceTag::Primary,
    }
}

fn with_loc(mut e: CalEvent, loc: &str) -> CalEvent {
    e.location = Some(loc.to_string());
    e
}

fn tagged(mut e: CalEvent, t: SourceTag) -> CalEvent {
    e.source = t;
    e
}

fn base() -> Model {
    let mut m = Model::empty(now());
    m.battery = Battery {
        percent: Some(78),
        millivolts: Some(3920),
        charging: false,
    };
    m.firmware = "symulator".to_string();
    m
}

/// Typowy tydzień pracy.
pub fn week() -> Model {
    let mut m = base();
    m.days = vec![
        DayGroup {
            date: today(),
            events: vec![
                ev(0, 8, 30, 9, 0, "Stand-up zespołu"),
                with_loc(
                    ev(0, 11, 0, 12, 30, "Przegląd architektury — kwartał"),
                    "Sala Kraków, 2. piętro",
                ),
                ev(0, 14, 0, 15, 0, "1:1 z Łukaszem"),
                with_loc(
                    ev(0, 18, 30, 20, 0, "Trening — ćwiczenia siłowe"),
                    "Siłownia Wrocławska 12",
                ),
            ],
        },
        DayGroup {
            date: today() + Duration::days(1),
            events: vec![
                tagged(
                    ev(1, 9, 0, 10, 30, "Warsztat: strategia produktu"),
                    SourceTag::Secondary,
                ),
                with_loc(
                    ev(1, 13, 0, 14, 0, "Obiad z Agnieszką"),
                    "Bistro Świętojańska",
                ),
                ev(1, 16, 0, 17, 0, "Retrospektywa"),
            ],
        },
        DayGroup {
            date: today() + Duration::days(2),
            events: vec![CalEvent {
                start: (today() + Duration::days(2)).and_hms_opt(0, 0, 0).unwrap(),
                end: (today() + Duration::days(3)).and_hms_opt(0, 0, 0).unwrap(),
                all_day: true,
                title: "Wyjazd — Gdańsk".to_string(),
                location: None,
                source: SourceTag::Holiday,
            }],
        },
    ];
    m.tiles = vec![
        Tile::new("pogoda", "21").with_unit("°C"),
        Tile::new("wschód", "05:42"),
        Tile::new("zachód", "20:01"),
    ];
    m
}

/// Pusty kalendarz.
pub fn empty() -> Model {
    let mut m = base();
    m.tiles = vec![Tile::new("pogoda", "17").with_unit("°C")];
    m
}

/// Bardzo zajęty tydzień — sprawdza paginację.
pub fn busy() -> Model {
    let mut m = base();
    let titles = [
        "Synchronizacja z zespołem platformy",
        "Przegląd kodu — moduł płatności",
        "Rozmowa rekrutacyjna",
        "Planowanie sprintu",
        "Prezentacja dla zarządu",
        "Warsztat z klientem",
        "Szkolenie BHP",
    ];
    m.days = (0..7)
        .map(|d| DayGroup {
            date: today() + Duration::days(d),
            events: (0..6)
                .map(|i| {
                    let e = ev(
                        d,
                        8 + i * 2,
                        0,
                        9 + i * 2,
                        0,
                        titles[(i as usize + d as usize) % titles.len()],
                    );
                    if i % 2 == 0 {
                        with_loc(e, "Sala konferencyjna B")
                    } else {
                        e
                    }
                })
                .collect(),
        })
        .collect();
    m.tiles = vec![Tile::new("pogoda", "28").with_unit("°C")];
    m
}

/// Przypadki brzegowe, które psują układ, jeśli coś się przeoczy.
pub fn edge_cases() -> Model {
    let mut m = base();
    m.battery = Battery {
        percent: Some(7),
        millivolts: Some(3480),
        charging: false,
    };
    m.net = NetState::Stale {
        since: now() - Duration::hours(9),
    };

    m.days = vec![
        DayGroup {
            date: today(),
            events: vec![
                // Bardzo długi tytuł.
                with_loc(
                    ev(0, 9, 0, 10, 0, "Spotkanie w sprawie ujednolicenia procesu zatwierdzania faktur kosztowych w oddziałach zagranicznych"),
                    "Bardzo długa nazwa sali konferencyjnej na którą nie ma miejsca w kolumnie",
                ),
                // Znaki spoza podstawowej łacinki.
                ev(0, 10, 30, 11, 0, "Żółć — gęślą jaźń ĄĆĘŁŃÓŚŹŻ"),
                // Zerowy czas trwania.
                ev(0, 12, 0, 12, 0, "Przypomnienie (zero minut)"),
                // Nachodzące na siebie.
                ev(0, 13, 0, 15, 0, "Blok A"),
                ev(0, 14, 0, 16, 0, "Blok B nachodzący na A"),
                // Bardzo krótkie.
                ev(0, 16, 5, 16, 10, "Pięć minut"),
                // Przez północ.
                CalEvent {
                    start: at(0, 23, 30),
                    end: at(1, 1, 30),
                    all_day: false,
                    title: "Wdrożenie nocne".to_string(),
                    location: None,
                    source: SourceTag::Secondary,
                },
                // Pusty tytuł po stronie źródła.
                ev(0, 20, 0, 21, 0, "(bez tytułu)"),
            ],
        },
        DayGroup {
            date: today() + Duration::days(1),
            events: vec![
                // Kilka całodniowych naraz.
                CalEvent {
                    start: (today() + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap(),
                    end: (today() + Duration::days(2)).and_hms_opt(0, 0, 0).unwrap(),
                    all_day: true,
                    title: "Urlop".to_string(),
                    location: None,
                    source: SourceTag::Holiday,
                },
                CalEvent {
                    start: (today() + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap(),
                    end: (today() + Duration::days(2)).and_hms_opt(0, 0, 0).unwrap(),
                    all_day: true,
                    title: "Święto państwowe".to_string(),
                    location: None,
                    source: SourceTag::Holiday,
                },
            ],
        },
    ];
    m.tiles = vec![
        Tile::new("bardzo długa etykieta kafelka", "123456").with_unit("jednostek"),
        Tile::new("ok", "1"),
    ];
    m
}

#[allow(dead_code)]
fn _unused(_: chrono::Utc) {}

#[allow(dead_code)]
fn _tz_marker() {
    let _ = chrono::Local.timestamp_opt(0, 0);
}
