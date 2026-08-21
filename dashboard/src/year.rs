//! Widok roczny — dwanaście miesięcy, dni i święta. Kalendarz ścienny.
//!
//! # Co ten ekran pokazuje, a czego świadomie NIE
//!
//! Pokazuje **strukturę roku**: który dzień tygodnia to która data, gdzie wypadają
//! weekendy i święta, ile tygodni dzieli dwie daty. Nie pokazuje **nic o zajętości**
//! — żadnych kropek, słupków ani gęstości. Kalendarz na ścianie odpowiada na pytanie
//! „kiedy wypada", a nie „co mam zaplanowane"; od tego drugiego są agenda i miesiąc.
//!
//! Ten moduł przeszedł przez dwie ślepe uliczki i obie warto mieć zapisane, bo obie
//! wyglądały na rozsądne:
//!
//! 1. **Słupki gęstości.** Rok jako mapa tego, gdzie jest tłoczno. Odpowiedź na
//!    niezadane pytanie — i przy prawdziwym kalendarzu, gdzie prawie każdy dzień
//!    roboczy coś ma, po prostu szum.
//! 2. **Pasma miesięcy z rastrem weekendów.** Bliżej, ale wciąż nie kalendarz:
//!    żeby odczytać datę, trzeba było liczyć kolumny od podziałki.
//!
//! # Dlaczego „ściana cyfr" okazała się złym zarzutem
//!
//! Odrzuciłem kiedyś ten układ szacunkiem, że dwucyfrowa liczba w podłodze
//! typograficznej ma ~20 px, więc 504 z nich nie da się ułożyć czytelnie. **Pomiar
//! mówi co innego**: najszersza para cyfr w 22 px ma 18,5 px, a w 19 px — 16,0.
//!
//! Przy podziale 3 × 4 w pionie blok miesiąca dostaje 173 × 199 px, czyli kratkę
//! **24,7 × 24,5 px** — prawie kwadratową, z sześcioma pikselami luzu wokół liczby
//! pisanej stopniem 22, a więc POWYŻEJ podłogi. Zarzut opierał się na zgadywanej
//! szerokości, nie na zmierzonej.
//!
//! # Weekend jest strukturą, nie oznaczeniem
//!
//! W siatce zaczynającej się od poniedziałku sobota i niedziela to **zawsze dwie
//! ostatnie kolumny**. Nie trzeba ich więc podkreślać tonem — wystarczy kreska
//! oddzielająca piątek od soboty. To ważne, bo raster pod cyfrą jest na tym panelu
//! kosztowny: cyfra bez białej podkładki siada na kropkach i przestaje być cyfrą,
//! a przy kratce 24 px na podkładkę nie ma miejsca.
//!
//! # Święto to jedyne wypełnienie
//!
//! Pełna czarna kratka z liczbą w negatywie — jedyne miejsce w tym widoku rysowane
//! zalewką, więc czyta się z drugiego końca pokoju. Negatyw idzie `Bold`, reguła 4
//! z nagłówka [`crate::layout`].
//!
//! **Święta mają własny horyzont.** Sama siatka dat liczy się z kalendarza i
//! obowiązuje cały rok; święto jest wydarzeniem i musi przyjść z kanału ICS. Kanał
//! świąt pobierany jest więc na **cały rok**, a nie na czternaście dni co kanał
//! z treścią — kosztuje to tyle co nic, bo to ~13 wydarzeń całodniowych bez reguł
//! powtarzania. Mechanizm siedzi w `EventSource::horizon_days`.
//!
//! Dlatego ten widok czyta [`Model::holidays`], a nie `Model::days`: w `days`
//! leży wyłącznie horyzont treści, żeby agenda w sierpniu nie listowała 25 grudnia.
//! Zasięg wiedzy o świętach niesie [`Model::known_holidays`] i mówi go stopka —
//! przy nieskonfigurowanym kanale świąt ekran przyzna, że ich nie zna, zamiast
//! udawać, że listopad jest bez świąt.

use chrono::{Datelike, NaiveDate};

use crate::canvas::{Gray8, Rect, BLACK, INK_DIM, WHITE};
use crate::hit::Screen;
use crate::layout::{TEXT_BODY, TEXT_FLOOR, TEXT_HEAD};
use crate::model::Model;
use crate::shapes::{hline, stroke_round_rect};
use crate::text::{Align, Fonts, Weight};

const MONTHS: usize = 12;
const COLS: i32 = 7;
const WEEKS: i32 = 6;

const SKROTY: [&str; MONTHS] = [
    "styczeń",
    "luty",
    "marzec",
    "kwiecień",
    "maj",
    "czerwiec",
    "lipiec",
    "sierpień",
    "wrzesień",
    "październik",
    "listopad",
    "grudzień",
];

/// Inicjały dni tygodnia, od poniedziałku.
const DNI: [&str; 7] = ["p", "w", "ś", "c", "p", "s", "n"];

/// Wysokość dostępna dla treści: płótno bez pasa zakładek.
fn body_h(c: &Gray8) -> i32 {
    c.height() as i32 - crate::nav::tabs_h(c)
}

/// Rozkład dwunastu bloków i geometria jednego z nich.
struct Plan {
    kolumny: i32,
    margin: i32,
    blok_w: i32,
    blok_h: i32,
    top: i32,
    cell_w: i32,
    cell_h: i32,
    /// Wysokość nazwy miesiąca plus wiersza inicjałów, licząc od góry bloku.
    naglowek_h: i32,
    stopien: f32,
}

impl Plan {
    fn of(c: &Gray8, fonts: &Fonts) -> Self {
        let w = c.width() as i32;
        let h = body_h(c);
        let pion = c.rotation().is_portrait();

        // 3 x 4 w pionie, 6 x 2 w poziomie. Wybór jest wymuszony przez wysokość
        // kratki, nie przez estetykę: w poziomie na treść zostaje 486 px, więc przy
        // czterech kolumnach na blok przypada 136 px, a po odjęciu nagłówka wychodzi
        // 15 px na wiersz tygodnia — mniej niż stopień pisma. Sześć kolumn daje 25 px.
        let kolumny = if pion { 3 } else { 6 };
        let wiersze = MONTHS as i32 / kolumny;

        let margin = 10;
        let head_h = if pion { 56 } else { 46 };
        let foot_h = if pion { 40 } else { 32 };

        let blok_w = (w - 2 * margin) / kolumny;
        // Rynna między miesiącami. Bez niej niedziela stycznia sąsiaduje wprost
        // z poniedziałkiem lutego i dwanaście bloków czyta się jak jeden pas —
        // widać to było zwłaszcza w poziomie, gdzie na blok przypada 156 px.
        let rynna = if pion { 8 } else { 10 };
        let blok_h = (h - head_h - foot_h) / wiersze;
        let naglowek_h = if pion { 48 } else { 42 };
        let cell_w = (blok_w - rynna) / COLS;
        let cell_h = (blok_h - naglowek_h) / WEEKS;

        // Największy stopień, w którym najszersza para cyfr mieści się w kratce
        // z oddechem. Mierzymy „28" — to ona, a nie „31", jest najszersza w tym
        // kroju. Bez doboru układ poziomy by się rozjechał, a pionowy marnowałby
        // sześć pikseli luzu na kratkę.
        let luz = 4.0;
        let stopien = [TEXT_BODY, 20.0, TEXT_FLOOR, 17.0]
            .into_iter()
            .find(|&sz| fonts.measure("28", sz, Weight::Bold) + luz <= cell_w as f32)
            .unwrap_or(15.0);

        Self {
            kolumny,
            margin,
            blok_w,
            blok_h,
            top: head_h,
            cell_w,
            cell_h,
            naglowek_h,
            stopien,
        }
    }

    fn blok(&self, m: usize) -> Rect {
        let i = m as i32;
        Rect::new(
            self.margin + (i % self.kolumny) * self.blok_w,
            self.top + (i / self.kolumny) * self.blok_h,
            self.blok_w,
            self.blok_h,
        )
    }
}

/// Dni roku będące świętami, indeksowane `[miesiąc-1][dzień-1]`.
///
/// Czytamy [`Model::holidays`], a nie `days`: tam leży pełny rok świąt, podczas gdy
/// `days` sięga horyzontu treści, czyli dwóch tygodni. Zajętość nie ma tu wstępu —
/// ten ekran z założenia nic o niej nie mówi.
fn swieta(model: &Model, year: i32) -> [[bool; 31]; MONTHS] {
    let mut out = [[false; 31]; MONTHS];
    for d in &model.holidays {
        if d.year() == year {
            out[(d.month() - 1) as usize][(d.day() - 1) as usize] = true;
        }
    }
    out
}

/// Rysuje kalendarz roczny dla roku, w którym leży `model.now`.
pub fn render_year(model: &Model, fonts: &Fonts, c: &mut Gray8) -> Screen {
    c.clear(WHITE);
    let screen = Screen::default();
    let p = Plan::of(c, fonts);
    let w = c.width() as i32;
    let today = model.now.date();
    let year = today.year();
    let sw = swieta(model, year);

    // --- nagłówek ----------------------------------------------------------
    fonts.draw(
        c,
        &year.to_string(),
        p.margin as f32,
        (p.top - 18) as f32,
        TEXT_HEAD,
        Weight::Bold,
        BLACK,
        Align::Left,
    );
    hline(c, p.margin, p.top - 10, w - 2 * p.margin, 2, BLACK);

    // --- dwanaście bloków --------------------------------------------------
    for (m, swieta_miesiaca) in sw.iter().enumerate() {
        draw_month(c, fonts, &p, p.blok(m), year, m, swieta_miesiaca, today);
    }

    // --- stopka ------------------------------------------------------------
    let foot = body_h(c) - 12;
    let podpis = match model.known_holidays {
        Some((a, z)) => format!(
            "święta znane {}.{:02}–{}.{:02} · siatka dat obowiązuje cały rok",
            a.day(),
            a.month(),
            z.day(),
            z.month()
        ),
        None => "brak pobranych świąt · siatka dat obowiązuje cały rok".to_string(),
    };
    fonts.draw(
        c,
        &podpis,
        p.margin as f32,
        foot as f32,
        TEXT_FLOOR,
        Weight::Medium,
        INK_DIM,
        Align::Left,
    );

    c.quantize_ink();
    screen
}

#[allow(clippy::too_many_arguments)]
fn draw_month(
    c: &mut Gray8,
    fonts: &Fonts,
    p: &Plan,
    blok: Rect,
    year: i32,
    m: usize,
    swieta: &[bool; 31],
    today: NaiveDate,
) {
    let miesiac = (m + 1) as u32;
    let biezacy = miesiac == today.month() && year == today.year();

    // Nazwa miesiąca. „październik" jest najdłuższa i przy bloku 158 px w poziomie
    // nie wchodzi w TEXT_BODY — schodzimy o stopień zamiast skracać, bo skrót
    // trzyliterowy myli się z „paź" i „lis" przy szybkim spojrzeniu.
    let nazwa = SKROTY[m];
    let stopien_nazwy = if fonts.measure(nazwa, TEXT_BODY, Weight::Bold) <= (blok.w - 4) as f32 {
        TEXT_BODY
    } else {
        TEXT_FLOOR
    };
    fonts.draw(
        c,
        nazwa,
        (blok.x + 2) as f32,
        (blok.y + 20) as f32,
        stopien_nazwy,
        Weight::Bold,
        BLACK,
        Align::Left,
    );

    // Inicjały dni tygodnia.
    let inicjaly_y = blok.y + p.naglowek_h - 8;
    for (i, d) in DNI.iter().enumerate() {
        let i = i as i32;
        let weekend = i >= 5;
        fonts.draw(
            c,
            d,
            (blok.x + i * p.cell_w + p.cell_w / 2) as f32,
            inicjaly_y as f32,
            TEXT_FLOOR,
            if weekend {
                Weight::Bold
            } else {
                Weight::Medium
            },
            if weekend { BLACK } else { INK_DIM },
            Align::Center,
        );
    }

    // Kreska między piątkiem a sobotą. Weekend jest w tym układzie strukturą —
    // zawsze dwie ostatnie kolumny — więc wystarczy go oddzielić, zamiast kłaść
    // raster pod cyframi, gdzie i tak nie ma miejsca na białą podkładkę.
    let siatka_y = blok.y + p.naglowek_h;
    c.fill_rect(
        Rect::new(blok.x + 5 * p.cell_w, siatka_y, 1, WEEKS * p.cell_h),
        INK_DIM,
    );

    let Some(pierwszy) = NaiveDate::from_ymd_opt(year, miesiac, 1) else {
        return;
    };
    let offset = pierwszy.weekday().num_days_from_monday() as i32;
    let dni = dni_miesiaca(year, miesiac);

    for idx in 0..dni {
        let kol = (idx + offset) % COLS;
        let wiersz = (idx + offset) / COLS;
        if wiersz >= WEEKS {
            break;
        }
        let cell = Rect::new(
            blok.x + kol * p.cell_w,
            siatka_y + wiersz * p.cell_h,
            p.cell_w,
            p.cell_h,
        );
        let dzien = (idx + 1) as u32;
        let swieto = swieta[idx as usize];
        let dzis = biezacy && dzien == today.day();

        if swieto {
            // Jedyna zalewka w tym widoku — dlatego święto widać z dystansu.
            c.fill_rect(cell.inset(1), BLACK);
        }
        if dzis {
            stroke_round_rect(c, cell.inset(1), 4.0, 2, if swieto { WHITE } else { BLACK });
        }

        fonts.draw(
            c,
            &dzien.to_string(),
            (cell.x + cell.w / 2) as f32,
            (cell.y + cell.h - (cell.h - p.stopien as i32) / 2 - 2) as f32,
            p.stopien,
            if swieto || dzis {
                Weight::Bold
            } else {
                Weight::Medium
            },
            if swieto { WHITE } else { BLACK },
            Align::Center,
        );
    }
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
    use crate::model::{CalEvent, DayGroup, SourceTag};

    fn model_na(rok: i32, mies: u32, dzien: u32) -> Model {
        Model::empty(
            NaiveDate::from_ymd_opt(rok, mies, dzien)
                .unwrap()
                .and_hms_opt(12, 0, 0)
                .unwrap(),
        )
    }

    fn wydarzenie(d: NaiveDate, tag: SourceTag) -> DayGroup {
        DayGroup {
            date: d,
            events: vec![CalEvent {
                start: d.and_hms_opt(0, 0, 0).unwrap(),
                end: d.and_hms_opt(23, 59, 0).unwrap(),
                all_day: true,
                title: "x".into(),
                location: None,
                source: tag,
            }],
        }
    }

    /// Ile atramentu w kratce danego dnia — do odróżnienia zalewki od samej cyfry.
    fn zalane_procent(c: &Gray8, p: &Plan, m: usize, dzien: u32, rok: i32) -> usize {
        let blok = p.blok(m);
        let pierwszy = NaiveDate::from_ymd_opt(rok, (m + 1) as u32, 1).unwrap();
        let offset = pierwszy.weekday().num_days_from_monday() as i32;
        let idx = dzien as i32 - 1;
        let cell = Rect::new(
            blok.x + ((idx + offset) % COLS) * p.cell_w,
            blok.y + p.naglowek_h + ((idx + offset) / COLS) * p.cell_h,
            p.cell_w,
            p.cell_h,
        );
        let czarne = (cell.y..cell.bottom())
            .flat_map(|y| (cell.x..cell.right()).map(move |x| (x, y)))
            .filter(|&(x, y)| c.get(x, y) == BLACK)
            .count();
        czarne * 100 / (cell.w * cell.h).max(1) as usize
    }

    /// Sześć wierszy musi wystarczyć na każdy możliwy miesiąc. Najgorszy przypadek
    /// to 31 dni zaczynające się w niedzielę: offset 6 + 31 = 37 kratek, czyli
    /// szósty wiersz jest konieczny, a siódmy nigdy.
    #[test]
    fn szesc_tygodni_zawsze_wystarcza() {
        for rok in 2024..2036 {
            for m in 1..=12u32 {
                let pierwszy = NaiveDate::from_ymd_opt(rok, m, 1).unwrap();
                let offset = pierwszy.weekday().num_days_from_monday() as i32;
                let potrzeba = offset + dni_miesiaca(rok, m);
                assert!(
                    potrzeba <= WEEKS * COLS,
                    "{rok}-{m}: {potrzeba} kratek nie mieści się w {}",
                    WEEKS * COLS
                );
            }
        }
    }

    #[test]
    fn dwanascie_blokow_miesci_sie_nad_zakladkami() {
        let fonts = Fonts::embedded();
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let c = Gray8::new(rot);
            let p = Plan::of(&c, &fonts);
            for m in 0..MONTHS {
                let b = p.blok(m);
                let dol = b.y + p.naglowek_h + WEEKS * p.cell_h;
                assert!(
                    dol <= body_h(&c),
                    "{rot:?}: miesiąc {m} kończy się na {dol}, a treść ma {} px",
                    body_h(&c)
                );
                assert!(
                    b.x + COLS * p.cell_w <= c.width() as i32,
                    "{rot:?}: miesiąc {m} wychodzi poza prawą krawędź"
                );
            }
        }
    }

    /// Dobrany stopień musi realnie mieścić najszerszą parę cyfr — to jest ten
    /// pomiar, którego brak kazał kiedyś odrzucić cały ten układ jako „ścianę cyfr".
    #[test]
    fn dwucyfrowa_liczba_miesci_sie_w_kratce() {
        let fonts = Fonts::embedded();
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let c = Gray8::new(rot);
            let p = Plan::of(&c, &fonts);
            let szer = fonts.measure("28", p.stopien, Weight::Bold);
            assert!(
                szer < p.cell_w as f32,
                "{rot:?}: \"28\" ma {szer:.1} px przy kratce {} px",
                p.cell_w
            );
            assert!(
                p.stopien >= 17.0,
                "{rot:?}: stopień {} zszedł poniżej czytelności",
                p.stopien
            );
        }
    }

    /// Święto zalewa kratkę; spotkanie nie robi NIC. Ten widok z założenia nie mówi
    /// o zajętości — gdyby mówił, wróciłby do wersji, którą trzeba było wyrzucić.
    #[test]
    fn swieto_zalewa_kratke_a_spotkanie_nie() {
        let fonts = Fonts::embedded();
        let nowy_rok = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();

        let mut ze_swietem = model_na(2026, 6, 15);
        ze_swietem.holidays = vec![nowy_rok];
        let mut c1 = Gray8::new(Rotation::Portrait);
        render_year(&ze_swietem, &fonts, &mut c1);

        let mut ze_spotkaniem = model_na(2026, 6, 15);
        ze_spotkaniem.days = vec![wydarzenie(nowy_rok, SourceTag::Primary)];
        // Ten sam dzień, ale jako spotkanie — kratka ma zostać pusta.
        let mut c2 = Gray8::new(Rotation::Portrait);
        render_year(&ze_spotkaniem, &fonts, &mut c2);

        let p = Plan::of(&c1, &fonts);
        let swieto = zalane_procent(&c1, &p, 0, 1, 2026);
        let spotkanie = zalane_procent(&c2, &p, 0, 1, 2026);
        assert!(
            swieto > 50,
            "święto pokryte w {swieto}%, oczekiwano zalewki"
        );
        assert!(
            spotkanie < 25,
            "spotkanie pokryte w {spotkanie}% — ten widok nie mówi o zajętości"
        );
    }

    #[test]
    fn nie_wychodzi_atramentem_na_krawedzie() {
        let fonts = Fonts::embedded();
        let m = model_na(2026, 8, 18);
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            render_year(&m, &fonts, &mut c);
            let (w, h) = (c.width() as i32, c.height() as i32);
            for y in 0..h {
                assert_eq!(c.get(0, y), WHITE, "{rot:?}: atrament na lewej krawędzi");
                assert_eq!(c.get(w - 1, y), WHITE, "{rot:?}: na prawej");
            }
        }
    }
}
