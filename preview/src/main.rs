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
//! cargo run -p preview -- setup        # ekran konfiguracji (klawiatura dotykowa)
//! cargo run -p preview -- all          # wszystkie scenariusze naraz
//! ```

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime};
use dashboard::model::{Battery, CalEvent, DayGroup, NetState, SourceTag, Tile};
use dashboard::setup::Field;
use dashboard::{render, render_setup, Action, Fonts, Gray8, Model, Rotation, Setup};

/// Co renderujemy. Ekran konfiguracji nie jest `Model`-em — ma własny stan — więc
/// podgląd musi umieć jedno i drugie.
enum Scene {
    /// Karta tonów — nie ma modelu, rysuje się sama.
    TestCard,
    /// Widok miesięczny.
    Month(Box<Model>),
    /// Widok roczny.
    Year(Box<Model>),
    /// Ekran diagnozy po cichym zgonie poprzedniego cyklu.
    Diagnoza,
    /// Karta jednorodności tła.
    Uniformity,
    Dash(Box<Model>),
    Config(Box<Setup>),
}

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

    let dash = |m: Model| Scene::Dash(Box::new(m));
    let scenarios: Vec<(&str, Scene)> = match arg.as_str() {
        "all" => vec![
            ("week", dash(scenario_week())),
            ("empty", dash(scenario_empty())),
            ("offline", dash(scenario_offline())),
            ("pobieram", dash(scenario_pobieram())),
            ("full", dash(scenario_full())),
            ("full-page2", {
                let mut m = scenario_full();
                m.page = 1;
                dash(m)
            }),
            ("detail", {
                let mut m = scenario_week();
                m.focus = Some(1);
                dash(m)
            }),
            ("provisioning", dash(scenario_provisioning())),
            ("setup", Scene::Config(Box::new(scenario_setup_pusty()))),
            (
                "setup-adres",
                Scene::Config(Box::new(scenario_setup_adres())),
            ),
        ],
        "empty" => vec![("empty", dash(scenario_empty()))],
        "provisioning" => vec![("provisioning", dash(scenario_provisioning()))],
        "pobieram" | "fetching" => vec![("pobieram", dash(scenario_pobieram()))],
        "offline" => vec![("offline", dash(scenario_offline()))],
        "full" => vec![("full", dash(scenario_full()))],
        "setup" => vec![
            ("setup", Scene::Config(Box::new(scenario_setup_pusty()))),
            (
                "setup-adres",
                Scene::Config(Box::new(scenario_setup_adres())),
            ),
        ],
        "miesiac" | "month" => vec![("miesiac", Scene::Month(Box::new(scenario_miesiac())))],
        "rok" | "year" => vec![("rok", Scene::Year(Box::new(scenario_rok())))],
        "diagnoza" => vec![("diagnoza", Scene::Diagnoza)],
        "tony" | "testcard" => vec![("tony", Scene::TestCard)],
        "jednorodnosc" | "uniformity" => vec![("jednorodnosc", Scene::Uniformity)],
        "detail" => vec![("detail", {
            let mut m = scenario_week();
            m.focus = Some(1);
            dash(m)
        })],
        _ => vec![("week", dash(scenario_week()))],
    };

    std::fs::create_dir_all("out").expect("nie mogę utworzyć katalogu out/");

    for (name, scene) in scenarios {
        let started = std::time::Instant::now();
        let mut canvas = Gray8::new(rotation);
        let hits = match &scene {
            Scene::Dash(model) => render(model, &fonts, &mut canvas).hits.len(),
            Scene::Config(setup) => render_setup(setup, &fonts, &mut canvas).hits.len(),
            Scene::TestCard => {
                dashboard::render_test_card(&fonts, &mut canvas);
                0
            }
            Scene::Month(model) => dashboard::render(model, &fonts, &mut canvas).hits.len(),
            Scene::Year(model) => dashboard::render(model, &fonts, &mut canvas).hits.len(),
            Scene::Diagnoza => dashboard::render_diagnosis(
                &dashboard::Diagnosis {
                    // Najgorszy realny przypadek: najdłuższa nazwa etapu i liczby
                    // z prawdziwego zgłoszenia ze sprzętu.
                    step: "pobieranie 2. kalendarza",
                    hint: "sprawdź adres iCal; przy mało wolnej pamięci to może być TLS",
                    ms: 4700,
                    dram_kb: 62,
                    firmware: "taskcerber 0.1.0+gd8acb08",
                },
                &fonts,
                &mut canvas,
            )
            .hits
            .len(),
            Scene::Uniformity => {
                dashboard::render_uniformity_card(&fonts, &mut canvas);
                0
            }
        };
        let render_us = started.elapsed().as_micros();

        // Tak jak na panelu: nie tylko 16 poziomów zamiast 256, ale i ZMIERZONA
        // charakterystyka tego panelu. Sam  pokazywał równą skalę,
        // której na szkle nie ma, i przez to zrzuty obiecywały kontrast, którego
        // urządzenie nie dowozi. Patrz `Gray8::simulate_panel`.
        canvas.simulate_panel();

        let path = format!("out/{name}{suffix}.png");
        write_png(&canvas, &path);

        let packed = canvas.to_packed();
        println!(
            "{path:<26} render {render_us:>6} µs   framebuffer {} B   atrament {:.1}%   obszarów dotyku {hits}",
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
    m.firmware = "taskcerber 0.1.0".to_string();
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
    m.firmware = "taskcerber 0.1.0".to_string();
    m.net = NetState::NeedsAuth;
    m.tiles = vec![
        Tile::new("krok 1", "Skonfiguruj"),
        Tile::new("krok 2", "wpisz sieć"),
        Tile::new("krok 3", "wpisz adres iCal"),
    ];
    m
}

/// Ekran konfiguracji zaraz po wgraniu firmware'u: nic nie wpisane, litery.
fn scenario_setup_pusty() -> Setup {
    Setup::new()
}

/// Ten sam ekran w najtrudniejszym momencie: wpisywanie 120-znakowego adresu iCal
/// na stronie z symbolami. To jest scenariusz, który rozstrzyga, czy pole wartości
/// pokazuje właściwy fragment i czy klawiatura ma z czego to złożyć.
fn scenario_setup_adres() -> Setup {
    let mut s = Setup::new();
    s.set(Field::Ssid, "Dom");
    s.set(
        Field::Ics,
        "https://calendar.google.com/calendar/ical/ktos%40gmail.com/private-9f2c",
    );
    s.apply(Action::Focus(Field::Ics));
    s.apply(Action::KeyPage);
    s
}

fn scenario_empty() -> Model {
    let mut m = base(dt(23, 9, 5));
    m.tiles = vec![Tile::new("pogoda", "17").with_unit("°C")];
    m
}

/// Treść z migawki, a pobranie trwa. Widoczne przez kilkanaście sekund między
/// wstaniem panelu a zejściem radia — patrz `NetState::Fetching`.
fn scenario_pobieram() -> Model {
    let mut m = scenario_full();
    m.net = NetState::Fetching {
        since: Some(dt(18, 6, 42)),
    };
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

/// Miesiąc z NIERÓWNĄ gęstością — to jest realny przypadek, nie „każdy dzień
/// wygląda tak samo". Zakres pobranych dni celowo kończy się przed końcem miesiąca,
/// żeby było widać różnicę między „nic nie ma" a „nie wiem".
fn scenario_miesiac() -> Model {
    // Widok jest wybierany przez model, nie przez wywołanie — dokładnie tak,
    // jak robi to firmware.
    let mut m = base(dt(18, 7, 15));
    let gestosc: [(u32, usize); 12] = [
        (10, 1),
        (11, 2),
        (12, 5),
        (14, 1),
        (17, 3),
        (18, 4),
        (19, 2),
        (20, 1),
        (21, 7),
        (24, 2),
        (25, 1),
        (26, 3),
    ];
    m.known = Some((date(10), date(26)));
    m.days = gestosc
        .iter()
        .map(|&(d, ile)| DayGroup {
            date: date(d),
            events: (0..ile)
                .map(|i| ev(d, 8 + i as u32, 0, 9 + i as u32, 0, "Spotkanie"))
                .collect(),
        })
        .collect();
    m.view = dashboard::View::Month;
    m
}

/// Rok z ROCZNYM horyzontem — bo przy dzisiejszych czternastu dniach widok roczny
/// pokrywa 4% kratek i pokazywałby głównie własną niewiedzę. Ten scenariusz mówi,
/// jak by wyglądał, gdyby dane były.
fn scenario_rok() -> Model {
    let mut m = base(dt(18, 7, 15));

    // Święta państwowe 2026, w tym ruchome liczone od Wielkanocy (5 kwietnia):
    // Poniedziałek Wielkanocny 6.04, Zielone Świątki 24.05, Boże Ciało 4.06.
    let daty = [
        (1u32, 1u32),
        (1, 6),
        (4, 5),
        (4, 6),
        (5, 1),
        (5, 3),
        (5, 24),
        (6, 4),
        (8, 15),
        (11, 1),
        (11, 11),
        (12, 25),
        (12, 26),
    ];
    m.holidays = daty
        .iter()
        .map(|&(mies, d)| chrono::NaiveDate::from_ymd_opt(2026, mies, d).unwrap())
        .collect();

    let start = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
    m.known_holidays = Some((start, start + chrono::Duration::days(364)));
    m.view = dashboard::View::Year;
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
