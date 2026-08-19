//! Podgląd dashboardu na hoście.
//!
//! To jest ciasna pętla iteracji nad układem graficznym: `cargo run -p preview`
//! renderuje **dokładnie ten sam kod**, który pójdzie na urządzenie, i zapisuje PNG.
//! Bez płytki, bez flashowania, bez czekania na odświeżenie panelu.
//!
//! PNG jest kwantyzowany do 16 poziomów, bo tyle panel realnie pokazuje — podgląd w
//! pełnej skali szarości kłamałby na temat tego, co zobaczysz na szkle.
//!
//! ```text
//! cargo run -p preview                 # scenariusz "typowy tydzień"
//! cargo run -p preview -- empty        # pusty kalendarz
//! cargo run -p preview -- offline      # brak sieci
//! cargo run -p preview -- full         # przepełniona agenda
//! cargo run -p preview -- all          # wszystkie scenariusze naraz
//! ```

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime};
use dashboard::model::{Battery, CalEvent, DayGroup, NetState, SourceTag, Tile};
use dashboard::{render, Fonts, Gray8, Model, Rotation};

fn main() {
    let arg = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "week".to_string());

    // Druga pozycja wybiera orientację: `cargo run -p preview -- all landscape`.
    // Domyślnie pion, bo tak stoi urządzenie.
    let rotation = match std::env::args().nth(2).as_deref() {
        Some("landscape") | Some("poziomo") => Rotation::Landscape,
        _ => Rotation::Portrait,
    };
    let suffix = if rotation.is_portrait() {
        ""
    } else {
        "-landscape"
    };

    let fonts = Fonts::embedded();

    let scenarios: Vec<(&str, Model)> = match arg.as_str() {
        "all" => vec![
            ("week", scenario_week()),
            ("empty", scenario_empty()),
            ("offline", scenario_offline()),
            ("full", scenario_full()),
            ("full-page2", {
                let mut m = scenario_full();
                m.page = 1;
                m
            }),
            ("detail", {
                let mut m = scenario_week();
                m.focus = Some(1);
                m
            }),
            ("provisioning", scenario_provisioning()),
        ],
        "empty" => vec![("empty", scenario_empty())],
        "provisioning" => vec![("provisioning", scenario_provisioning())],
        "offline" => vec![("offline", scenario_offline())],
        "full" => vec![("full", scenario_full())],
        "detail" => vec![("detail", {
            let mut m = scenario_week();
            m.focus = Some(1);
            m
        })],
        _ => vec![("week", scenario_week())],
    };

    std::fs::create_dir_all("out").expect("nie mogę utworzyć katalogu out/");

    for (name, model) in scenarios {
        let started = std::time::Instant::now();
        let mut canvas = Gray8::new(rotation);
        render(&model, &fonts, &mut canvas);
        let render_us = started.elapsed().as_micros();

        // Tak jak na panelu: 16 poziomów, nie 256.
        canvas.quantize16();

        let path = format!("out/{name}{suffix}.png");
        write_png(&canvas, &path);

        let packed = canvas.to_packed();
        println!(
            "{path:<20} render {render_us:>6} µs   framebuffer {} B   atrament {:.1}%",
            packed.len(),
            ink_ratio(&canvas) * 100.0
        );
    }
}

fn ink_ratio(c: &Gray8) -> f32 {
    let dark = c.pixels().iter().filter(|&&p| p < 128).count();
    dark as f32 / c.pixels().len() as f32
}

fn write_png(c: &Gray8, path: impl AsRef<Path>) {
    let file = File::create(path.as_ref()).expect("nie mogę zapisać PNG");
    let mut encoder = png::Encoder::new(BufWriter::new(file), c.width() as u32, c.height() as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("nagłówek PNG");
    writer.write_image_data(c.pixels()).expect("dane PNG");
}

// ---------------------------------------------------------------------------
// Scenariusze
// ---------------------------------------------------------------------------

fn dt(d: u32, h: u32, m: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 8, d)
        .unwrap()
        .and_hms_opt(h, m, 0)
        .unwrap()
}

fn date(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 8, d).unwrap()
}

fn ev(day: u32, sh: u32, sm: u32, eh: u32, em: u32, title: &str) -> CalEvent {
    CalEvent {
        start: dt(day, sh, sm),
        end: dt(day, eh, em),
        all_day: false,
        title: title.to_string(),
        location: None,
        source: SourceTag::Primary,
    }
}

fn at(mut e: CalEvent, loc: &str) -> CalEvent {
    e.location = Some(loc.to_string());
    e
}

fn tagged(mut e: CalEvent, t: SourceTag) -> CalEvent {
    e.source = t;
    e
}

fn base(now: NaiveDateTime) -> Model {
    let mut m = Model::empty(now);
    m.battery = Battery {
        percent: Some(78),
        millivolts: Some(3920),
        charging: false,
    };
    m.firmware = "t5s3pro 0.1.0".to_string();
    m
}

fn scenario_week() -> Model {
    let mut m = base(dt(18, 11, 42));
    m.days = vec![
        DayGroup {
            date: date(18),
            events: vec![
                ev(18, 8, 30, 9, 0, "Stand-up zespołu"),
                at(
                    ev(18, 11, 0, 12, 30, "Przegląd architektury — kwartał"),
                    "Sala Kraków, 2. piętro",
                ),
                ev(18, 14, 0, 15, 0, "1:1 z Łukaszem"),
                at(
                    ev(18, 18, 30, 20, 0, "Trening — ćwiczenia siłowe"),
                    "Siłownia Wrocławska 12",
                ),
            ],
        },
        DayGroup {
            date: date(19),
            events: vec![
                tagged(
                    ev(19, 9, 0, 10, 30, "Warsztat: strategia produktu"),
                    SourceTag::Secondary,
                ),
                at(
                    ev(19, 13, 0, 14, 0, "Obiad z Agnieszką"),
                    "Bistro Świętojańska",
                ),
            ],
        },
        DayGroup {
            date: date(20),
            events: vec![CalEvent {
                start: date(20).and_hms_opt(0, 0, 0).unwrap(),
                end: date(21).and_hms_opt(0, 0, 0).unwrap(),
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
        Tile::new("następne", "za 3 h"),
    ];
    m
}

/// Ekran po wgraniu firmware'u, przed konfiguracją — pierwsze, co widać na płytce.
///
/// Musi się zgadzać z `provisioning_model` w firmware/src/main.rs. Data 2026-01-01
/// nie jest przypadkowa: to wartość zapasowa z `run()`, używana, dopóki RTC nie ma
/// czasu, a sieci jeszcze nie było.
fn scenario_provisioning() -> Model {
    let mut m = Model::empty(
        NaiveDate::from_ymd_opt(2026, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap(),
    );
    m.firmware = "t5s3pro 0.1.0".to_string();
    m.net = NetState::NeedsAuth;
    m.tiles = vec![
        Tile::new("krok 1", "podłącz USB"),
        Tile::new("krok 2", "otwórz stronę"),
        Tile::new("krok 3", "wpisz WiFi"),
    ];
    m
}

fn scenario_empty() -> Model {
    let mut m = base(dt(23, 9, 5));
    m.tiles = vec![Tile::new("pogoda", "17").with_unit("°C")];
    m
}

fn scenario_offline() -> Model {
    let mut m = scenario_week();
    m.net = NetState::Offline;
    m.battery = Battery {
        percent: Some(11),
        millivolts: Some(3520),
        charging: false,
    };
    m
}

fn scenario_full() -> Model {
    let mut m = base(dt(18, 7, 15));
    let mut days = Vec::new();
    for d in 18..25u32 {
        let events = (0..6)
            .map(|i| {
                let e = ev(
                    d,
                    8 + i * 2,
                    0,
                    9 + i * 2,
                    0,
                    "Bardzo długi tytuł wydarzenia, który na pewno nie zmieści się w kolumnie i musi zostać skrócony",
                );
                at(e, "Bardzo długa nazwa lokalizacji, która też się nie mieści")
            })
            .collect();
        days.push(DayGroup {
            date: date(d),
            events,
        });
    }
    m.days = days;
    m.tiles = vec![
        Tile::new("pogoda", "28").with_unit("°C"),
        Tile::new("wilgotność", "64").with_unit("%"),
    ];
    m
}

// Dodatkowe zrzuty do przeglądu paginacji i szczegółów — patrz `main`.
