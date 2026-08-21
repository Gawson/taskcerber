//! Widok roczny — planer: dwanaście wierszy miesięcy, trzydzieści jeden kolumn dni.
//!
//! # Do czego ten widok służy, a do czego nie
//!
//! Nie do gęstości. Do **struktury**: w jaki dzień tygodnia wypada dana data, gdzie
//! leżą weekendy i święta, ile tygodni dzieli dwie daty, kiedy wypada termin.
//! Po godzinę idzie się do agendy, po listę dnia do miesiąca — tutaj chodzi
//! o kalendarz jako siatkę, nie jako treść.
//!
//! Pierwsza wersja tego modułu rysowała słupki gęstości i była odpowiedzią na
//! niezadane pytanie. Zostawiam to zapisane, bo błąd był pouczający: „widok roczny"
//! brzmi jak podsumowanie, a jest narzędziem nawigacyjnym.
//!
//! # Dlaczego NIE dwanaście miniaturek kalendarza
//!
//! Naturalny odruch to ułożyć dwanaście siatek 7 × 6 w tablicę 3 × 4. Na płótnie
//! 540 × 960 daje to kratkę dnia **25 × 30 px**. Dwucyfrowa liczba w podłodze
//! typograficznej ma ~20 px szerokości, więc formalnie się mieści — i to jest
//! pułapka, bo mieści się **504 razy**. Ekran z pięciuset liczbami w rozmiarze
//! granicznym nie jest widokiem rocznym, tylko ścianą cyfr.
//!
//! Układ „miesiąc w wierszu, dzień w kolumnie" to klasyczny planer ścienny i ma
//! własność, której siatka miesięcy nie ma: **weekendy układają się w ukośne pasy**.
//! Każdy taki pas to jeden tydzień, więc liczenie tygodni między dwiema datami
//! sprowadza się do policzenia pasów — bez numerów tygodni, bez arytmetyki.
//!
//! # Przeszłość NIE jest tu wyszarzana
//!
//! W widoku miesięcznym minione dni bledną, bo tam liczy się to, co przed nami.
//! Tutaj jest odwrotnie: sprawdzenie, w jaki dzień wypadał zeszłoroczny termin,
//! to jedno z zastosowań tego ekranu. Struktura roku jest pokazana w całości.
//!
//! Horyzontem ograniczone są wyłącznie **dane o wydarzeniach** — urządzenie pobiera
//! `HORIZON_DAYS` dni do przodu i tylko w tym oknie potrafi zaznaczyć zajęty dzień.
//! Sama siatka dat, weekendy i święta z kalendarza świąt nie zależą od pobrania.

use chrono::{Datelike, NaiveDate, Weekday};

use crate::canvas::{dither_rect, Gray8, Rect, BLACK, INK_FAINT, WHITE};
use crate::hit::Screen;
use crate::layout::{TEXT_BODY, TEXT_FLOOR, TEXT_HEAD};
use crate::model::{Model, SourceTag};
use crate::shapes::hline;
use crate::text::{Align, Fonts, Weight};

/// Wysokość dostępna dla treści: płótno bez pasa zakładek.
///
/// Widok rysuje się nad paskiem, a nie pod nim — bez tego stopka wpadałaby pod
/// zakładki i znikała.
fn body_h(c: &Gray8) -> i32 {
    c.height() as i32 - crate::nav::tabs_h(c)
}

const MONTHS: i32 = 12;
const MAX_DAYS: i32 = 31;

const HEAD_H: i32 = 78;
const FOOT_H: i32 = 52;
/// Szerokość kolumny z nazwą miesiąca.
const LABEL_W: i32 = 46;

const SKROTY: [&str; 12] = [
    "sty", "lut", "mar", "kwi", "maj", "cze", "lip", "sie", "wrz", "paź", "lis", "gru",
];

/// Geometria planera.
struct Grid {
    margin: i32,
    row_h: i32,
    col_w: i32,
    grid_x: i32,
    cell_h: i32,
}

impl Grid {
    fn of(c: &Gray8) -> Self {
        let (w, h) = (c.width() as i32, body_h(c));
        let margin = 12;
        let grid_x = margin + LABEL_W;
        let row_h = (h - HEAD_H - FOOT_H) / MONTHS;
        Self {
            margin,
            row_h,
            col_w: (w - margin - grid_x) / MAX_DAYS,
            grid_x,
            // Kratka niższa niż wiersz: prześwit między miesiącami sprawia, że
            // ukośny pas weekendu czyta się jako ciąg kratek, a nie jako plama.
            cell_h: row_h - 6,
        }
    }

    fn row_y(&self, month: i32) -> i32 {
        HEAD_H + month * self.row_h
    }

    fn col_x(&self, day: i32) -> i32 {
        self.grid_x + day * self.col_w
    }

    fn cell(&self, month: i32, day: i32) -> Rect {
        Rect::new(
            self.col_x(day) + 1,
            self.row_y(month) + 2,
            self.col_w - 2,
            self.cell_h,
        )
    }
}

/// Co urządzenie wie o jednym dniu.
#[derive(Clone, Copy, Default)]
struct DayInfo {
    /// Dzień leży w pobranym oknie — bez tego nie wiadomo, czy brak święta
    /// oznacza dzień roboczy, czy tylko brak danych.
    znany: bool,
    swieto: bool,
}

/// Zbiera informacje o dniach roku, indeksowane `[miesiąc-1][dzień-1]`.
fn scan(model: &Model, year: i32) -> [[DayInfo; 31]; 12] {
    let mut out = [[DayInfo::default(); 31]; 12];
    let zakres = crate::month::covered_range(model);

    if let Some((a, z)) = zakres {
        let mut d = a;
        while d <= z {
            if d.year() == year {
                out[(d.month() - 1) as usize][(d.day() - 1) as usize].znany = true;
            }
            match d.succ_opt() {
                Some(n) => d = n,
                None => break,
            }
        }
    }

    for day in &model.days {
        if day.date.year() != year {
            continue;
        }
        let slot = &mut out[(day.date.month() - 1) as usize][(day.date.day() - 1) as usize];
        slot.znany = true;
        // Interesują nas TYLKO święta. Spotkania nie: pierwsza wersja stawiała
        // kropkę przy każdym zajętym dniu i w kalendarzu, w którym prawie każdy
        // dzień roboczy coś ma, kropki układały się w ciągłą linię pod miesiącem.
        // Czytało się to jak linijka, a nie jak dane — i zagłuszało pasy weekendów,
        // czyli jedyną rzecz, dla której ten ekran istnieje.
        slot.swieto |= day.events.iter().any(|e| e.source == SourceTag::Holiday);
    }
    out
}

/// Rysuje planer roczny dla roku, w którym leży `model.now`.
pub fn render_year(model: &Model, fonts: &Fonts, c: &mut Gray8) -> Screen {
    c.clear(WHITE);
    let screen = Screen::default();
    let g = Grid::of(c);
    let w = c.width() as i32;
    let today = model.now.date();
    let year = today.year();
    let info = scan(model, year);

    // --- nagłówek ----------------------------------------------------------
    fonts.draw(
        c,
        &year.to_string(),
        g.margin as f32,
        44.0,
        TEXT_HEAD,
        Weight::Bold,
        BLACK,
        Align::Left,
    );

    // Podziałka dni: co piąty. Trzydzieści jeden liczb w kolumnie 15 px byłoby
    // ścianą cyfr, którą ten układ miał właśnie zastąpić.
    for d in [0, 4, 9, 14, 19, 24, 29] {
        fonts.draw(
            c,
            &(d + 1).to_string(),
            (g.col_x(d) + g.col_w / 2) as f32,
            (HEAD_H - 10) as f32,
            TEXT_FLOOR,
            Weight::Medium,
            INK_FAINT,
            Align::Center,
        );
    }
    hline(c, g.margin, HEAD_H - 3, w - 2 * g.margin, 2, BLACK);

    // --- siatka ------------------------------------------------------------
    for m in 0..MONTHS {
        let miesiac = (m + 1) as u32;
        let dni = dni_miesiaca(year, miesiac);
        let biezacy = miesiac == today.month();
        let y = g.row_y(m);

        fonts.draw(
            c,
            SKROTY[m as usize],
            g.margin as f32,
            // Podstawa pisma w połowie kratki plus pół wysokości wersalika:
            // etykieta ma stać na wysokości swojego wiersza w obu orientacjach,
            // a te różnią się wysokością wiersza dwukrotnie (62 px wobec 27 px).
            (y + g.cell_h / 2 + 8) as f32,
            TEXT_BODY,
            if biezacy {
                Weight::Bold
            } else {
                Weight::Medium
            },
            if biezacy { BLACK } else { INK_FAINT },
            Align::Left,
        );

        // Podstawa wiersza: bez niej oko gubi się w poziomie przy trzydziestu jeden
        // kolumnach i „sie" przestaje się wiązać z kolumną 18.
        hline(c, g.grid_x, y + g.row_h - 3, g.col_w * dni, 1, INK_FAINT);

        for d in 0..dni {
            let Some(data) = NaiveDate::from_ymd_opt(year, miesiac, (d + 1) as u32) else {
                continue;
            };
            let cell = g.cell(m, d);

            // Kreska na początku poniedziałku. Dzięki niej odpowiedź na „w jaki
            // dzień wypada 15 marca" to policzenie kratek od najbliższej kreski,
            // zamiast liczenia w tył od pasa weekendu. Przy okazji tydzień staje
            // się jednostką widoczną wprost, a nie wyprowadzoną z ukosu.
            if data.weekday() == Weekday::Mon {
                // Pełny atrament i trzy piksele. Poniedziałek nigdy nie jest
                // weekendem, więc kreska stoi zawsze na białym tle — a przy 234 DPI
                // dwa piksele w słabszym tonie nikną między kratkami rastru.
                // Rytm wiersza staje się przez to czytelny wprost:
                // kreska, pięć dni roboczych, para weekendowa.
                // Kreska sięga górnej części wiersza, nie całej wysokości: poniedziałek
                // wypada tuż za parą weekendową, a pełnowysokościowa kreska sklejała
                // się z nią w jedną plamę i tydzień znów trzeba było odgadywać.
                c.fill_rect(Rect::new(cell.x - 1, y + 1, 3, g.row_h * 2 / 5), BLACK);
            }

            draw_day(c, cell, data, info[m as usize][d as usize], data == today);
        }
    }

    // --- stopka ------------------------------------------------------------
    let foot = body_h(c) - FOOT_H;
    hline(c, g.margin, foot, w - 2 * g.margin, 1, INK_FAINT);
    legenda(c, fonts, &g, foot, model, year);

    c.quantize_ink();
    screen
}

/// Rysuje jedną kratkę dnia.
///
/// Kolejność jest hierarchią ważności, bo kratka ma ~15 × 60 px i nie zmieści
/// czterech niezależnych oznaczeń naraz: weekend to tło, święto je nadpisuje,
/// dzisiaj nadpisuje wszystko.
fn draw_day(c: &mut Gray8, cell: Rect, data: NaiveDate, info: DayInfo, dzis: bool) {
    // Dzień spoza pobranego okna: sama struktura (weekend, poniedziałek) jest
    // liczona z kalendarza i obowiązuje zawsze, ale o świętach nic nie wiadomo.
    // Rozróżnienie niesie stopka, nie kratka — dwanaście miesięcy w rastrze
    // „nie wiem" zamieniłoby ekran w szum.
    let _ = info.znany;

    match data.weekday() {
        // Niedziela ciemniejsza od soboty: dzięki temu ukośny pas ma kierunek
        // i widać, gdzie tydzień się kończy, a nie tylko że gdzieś tam jest.
        Weekday::Sun => dither_rect(c, cell, 8),
        Weekday::Sat => dither_rect(c, cell, 4),
        _ => {}
    }

    if info.swieto {
        // Święto: pełna kratka atramentu. Jedyne pole w tym widoku rysowane
        // na czarno bez rastra, więc czyta się z dystansu.
        c.fill_rect(cell, BLACK);
    }

    if dzis {
        // Dzisiaj: ramka, nie wypełnienie. Wypełnienie zjadłoby oznaczenie święta,
        // a 1 stycznia bywa jednym i drugim.
        obwodka(c, cell, 3);
    }
}

/// Rysuje ramkę o zadanej grubości wewnątrz prostokąta.
fn obwodka(c: &mut Gray8, r: Rect, t: i32) {
    c.fill_rect(Rect::new(r.x, r.y, r.w, t), BLACK);
    c.fill_rect(Rect::new(r.x, r.y + r.h - t, r.w, t), BLACK);
    c.fill_rect(Rect::new(r.x, r.y, t, r.h), BLACK);
    c.fill_rect(Rect::new(r.x + r.w - t, r.y, t, r.h), BLACK);
}

/// Legenda plus uczciwe zdanie o tym, dokąd sięgają dane.
fn legenda(c: &mut Gray8, fonts: &Fonts, g: &Grid, foot: i32, model: &Model, year: i32) {
    let w_pelne = c.width() as i32;
    let y = foot + 30;
    let mut x = g.margin;
    let próbka = 16;

    let wpis = |c: &mut Gray8, x: &mut i32, rysuj: &dyn Fn(&mut Gray8, Rect), tekst: &str| {
        let r = Rect::new(*x, y - 14, próbka, 18);
        rysuj(c, r);
        fonts.draw(
            c,
            tekst,
            (*x + próbka + 6) as f32,
            y as f32,
            TEXT_FLOOR,
            Weight::Medium,
            INK_FAINT,
            Align::Left,
        );
        *x += próbka + 6 + fonts.measure(tekst, TEXT_FLOOR, Weight::Medium) as i32 + 14;
    };

    wpis(c, &mut x, &|c, r| dither_rect(c, r, 8), "weekend");
    wpis(c, &mut x, &|c, r| c.fill_rect(r, BLACK), "święto");
    wpis(c, &mut x, &|c, r| obwodka(c, r, 3), "dziś");
    wpis(
        c,
        &mut x,
        &|c, r| c.fill_rect(Rect::new(r.x, r.y, 3, r.h), BLACK),
        "pon.",
    );

    // Zasięg danych: weekendy są liczone z kalendarza i obowiązują cały rok,
    // ale święta i zajętość znamy tylko z pobranego okna. Bez tego zdania pusty
    // listopad wyglądałby jak listopad bez świąt.
    let zasieg = match crate::month::covered_range(model) {
        Some((a, z)) if a.year() <= year && z.year() >= year => format!(
            "święta znane {}.{:02}–{}.{:02}",
            a.day(),
            a.month(),
            z.day(),
            z.month()
        ),
        _ => "brak pobranych danych o świętach".to_string(),
    };
    fonts.draw(
        c,
        &zasieg,
        (w_pelne - g.margin) as f32,
        y as f32,
        TEXT_FLOOR,
        Weight::Medium,
        INK_FAINT,
        Align::Right,
    );
}

pub(crate) fn dni_miesiaca(year: i32, month: u32) -> i32 {
    let (ny, nm) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    match (
        NaiveDate::from_ymd_opt(ny, nm, 1),
        NaiveDate::from_ymd_opt(year, month, 1),
    ) {
        (Some(a), Some(b)) => (a - b).num_days() as i32,
        _ => 30,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Rotation;
    use crate::model::DayGroup;

    fn pusty(rok: i32, mies: u32, dzien: u32) -> Model {
        Model::empty(
            NaiveDate::from_ymd_opt(rok, mies, dzien)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        )
    }

    #[test]
    fn siatka_miesci_dwanascie_miesiecy_i_trzydziesci_jeden_dni() {
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let c = Gray8::new(rot);
            let g = Grid::of(&c);
            let ost = g.cell(MONTHS - 1, MAX_DAYS - 1);
            assert!(
                ost.y + ost.h <= body_h(&c) - FOOT_H,
                "{rot:?}: grudzień wchodzi w stopkę"
            );
            assert!(
                ost.x + ost.w <= c.width() as i32 - g.margin,
                "{rot:?}: 31 dzień wychodzi poza margines"
            );
            assert!(g.col_w >= 12, "{rot:?}: kolumna {} px za wąska", g.col_w);
        }
    }

    /// Weekendy muszą tworzyć ukośne pasy — na tym opiera się liczenie tygodni.
    /// Test sprawdza własność układu, nie piksele: w kolejnych miesiącach kolumna
    /// pierwszej soboty przesuwa się, a odstęp między sobotami to zawsze 7 dni.
    #[test]
    fn soboty_ukladaja_sie_w_ukosne_pasy() {
        let rok = 2026;
        let mut poprzednia = None;
        let mut przesuniecia = 0;
        for m in 1..=12u32 {
            let pierwsza = (1..=7)
                .filter_map(|d| NaiveDate::from_ymd_opt(rok, m, d))
                .find(|d| d.weekday() == Weekday::Sat)
                .expect("każdy miesiąc ma sobotę w pierwszym tygodniu");
            for d in [pierwsza.day() as i32, pierwsza.day() as i32 + 7] {
                if let Some(x) = NaiveDate::from_ymd_opt(rok, m, d as u32 + 6) {
                    assert_eq!(x.weekday(), Weekday::Fri, "odstęp sobót to 7 dni");
                }
            }
            if let Some(p) = poprzednia {
                if p != pierwsza.day() {
                    przesuniecia += 1;
                }
            }
            poprzednia = Some(pierwsza.day());
        }
        assert!(
            przesuniecia >= 10,
            "pas musi się przesuwać, a nie stać pionowo"
        );
    }

    #[test]
    fn swieto_znaczy_swieto_a_spotkanie_nie() {
        let mut m = pusty(2026, 1, 15);
        m.known = Some((
            NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
        ));
        let nowy_rok = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        m.days = vec![DayGroup {
            date: nowy_rok,
            events: vec![crate::model::CalEvent {
                start: nowy_rok.and_hms_opt(0, 0, 0).unwrap(),
                end: nowy_rok.and_hms_opt(23, 59, 0).unwrap(),
                all_day: true,
                title: "Nowy Rok".into(),
                location: None,
                source: SourceTag::Holiday,
            }],
        }];
        let info = scan(&m, 2026);
        assert!(info[0][0].swieto, "1 stycznia powinno być świętem");
        assert!(
            !info[0][1].swieto,
            "2 stycznia nie jest świętem, choć leży w oknie"
        );
        assert!(
            info[0][14].znany,
            "15 stycznia leży w oknie, więc jest znany"
        );
        assert!(!info[0][14].swieto);
    }

    #[test]
    fn przeszlosc_nie_jest_wyszarzana() {
        // Widok roczny służy m.in. do dat historycznych — struktura roku musi być
        // narysowana w całości, także przed dzisiaj.
        let m = pusty(2026, 12, 20);
        let fonts = Fonts::embedded();
        let mut c = Gray8::new(Rotation::Portrait);
        render_year(&m, &fonts, &mut c);
        let g = Grid::of(&c);
        // Niedziela 4 stycznia 2026 — dawno miniona, a rastrowana jak każda inna.
        let cell = g.cell(0, 3);
        assert_eq!(
            NaiveDate::from_ymd_opt(2026, 1, 4).unwrap().weekday(),
            Weekday::Sun
        );
        let atrament = (cell.y..cell.y + cell.h)
            .flat_map(|y| (cell.x..cell.x + cell.w).map(move |x| (x, y)))
            .filter(|&(x, y)| c.get(x, y) < WHITE)
            .count();
        assert!(atrament > 0, "miniona niedziela musi być oznaczona");
    }

    #[test]
    fn renderuje_sie_w_obu_orientacjach_bez_atramentu_na_krawedziach() {
        let mut m = pusty(2026, 8, 18);
        m.known = Some((
            NaiveDate::from_ymd_opt(2026, 8, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 26).unwrap(),
        ));
        let fonts = Fonts::embedded();
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            render_year(&m, &fonts, &mut c);
            let (w, h) = (c.width() as i32, body_h(&c));
            for y in 0..h {
                assert_eq!(c.get(0, y), WHITE, "{rot:?}: atrament na lewej krawędzi");
                assert_eq!(c.get(w - 1, y), WHITE, "{rot:?}: na prawej");
            }
        }
    }
}
