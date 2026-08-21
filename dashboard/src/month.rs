//! Widok miesięczny — siatka 7 × 6.
//!
//! # Co ten widok może pokazać, a czego nie
//!
//! Komórka dnia w pionie ma **74 × 118 px**. Przy [`TEXT_FLOOR`] (19 px, podłoga
//! reguły 1) mieści to około **siedmiu znaków** w linii — czyli nie mieści tytułu
//! żadnego prawdziwego wydarzenia. Każdy projekt, w którym w kratce miesiąca stoi
//! nazwa spotkania, jest projektem z telefonu, nie z tego panelu.
//!
//! Dlatego kratka niesie wyłącznie **gęstość**: numer dnia i do trzech pasków,
//! po jednym na wydarzenie, plus licznik nadmiaru. To wystarcza do jedynego
//! pytania, które ten widok ma sensownie odpowiadać — „kiedy mam wolne" — a po
//! szczegóły idzie się stuknięciem w dzień.
//!
//! # Dlaczego paski, a nie kropki
//!
//! Kropka o średnicy 5 px to przy zmierzonej charakterystyce panelu kilkanaście
//! pikseli atramentu, w większości na krawędziach — czyli w poziomach 3-4, ledwo
//! widocznych. Pasek 3 px wysokości na całą szerokość kratki to atrament w poziomie
//! 0 i widać go z dystansu, z jakiego patrzy się na kalendarz na ścianie.
//!
//! # Przeszłość NIE blednie, sąsiednie miesiące owszem
//!
//! Wcześniejsza wersja przygaszała dni minione i nie rysowała im pasków. Wyszło
//! z tego, że pierwsza połowa ekranu była wyblakła przez większość miesiąca —
//! wyróżnienie, które obejmuje połowę powierzchni, przestaje być wyróżnieniem.
//! Miniony dzień wygląda więc jak każdy inny.
//!
//! Blednie za to to, co naprawdę jest tu tylko gościem: **dni z sąsiednich
//! miesięcy**, które wpadają w siatkę, żeby tydzień był ciągły. Ich jest najwyżej
//! kilkanaście i nie należą do tematu ekranu.
//!
//! Uwaga na paletę: do przygaszenia tekstu służy [`LIGHTEST_VISIBLE`], a **nie**
//! `INK_FAINT`. Mimo nazwy `INK_FAINT` (0x33) po [`Gray8::quantize_ink`] ląduje na
//! poziomie 1, czyli praktycznie na czerni — stałe palety są wartościami płótna
//! sprzed kwantyzacji, nie poziomami panelu.
//!
//! # Dzień dzisiejszy jest w negatywie
//!
//! Bo to jedyne wyróżnienie, które na tym panelu działa pewnie: ton ma cztery
//! stopnie i wszystkie są zajęte przez zwykłą treść. Numer w negatywie idzie
//! `Bold` — reguła 4 z nagłówka `layout`.

use chrono::{Datelike, NaiveDate};

use crate::canvas::{
    dither_rect, Gray8, Rect, BLACK, FILL_LIGHT, INK_FAINT, LIGHTEST_VISIBLE, WHITE,
};
use crate::hit::Screen;
use crate::layout::{TEXT_BODY, TEXT_FLOOR, TEXT_HEAD, TEXT_LEAD};
use crate::model::Model;
use crate::shapes::hline;
use crate::text::{Align, Fonts, Weight};

/// Wysokość dostępna dla treści: płótno bez pasa zakładek.
///
/// Widok rysuje się nad paskiem, a nie pod nim — bez tego stopka wpadałaby pod
/// zakładki i znikała.
fn body_h(c: &Gray8) -> i32 {
    c.height() as i32 - crate::nav::tabs_h(c)
}

/// Ile tygodni rysujemy zawsze.
///
/// Sześć, nie „tyle, ile trzeba": miesiąc rozkłada się na pięć albo sześć tygodni
/// zależnie od tego, w jaki dzień wypada pierwszy. Zmienna liczba wierszy znaczyłaby
/// zmienną wysokość kratki, a wtedy ta sama data raz jest wyżej, raz niżej i oko
/// musi jej szukać przy każdym miesiącu. Stała siatka jest warta pustego wiersza.
const WEEKS: i32 = 6;
const COLS: i32 = 7;

/// Maksymalna liczba pasków w kratce. Czwarte wydarzenie zamienia się w licznik.
const MAX_BARS: usize = 3;

/// Wysokość paska wydarzenia i odstęp między paskami.
const BAR_H: i32 = 4;
const BAR_GAP: i32 = 3;

/// Nagłówek z nazwą miesiąca i wiersz skrótów dni.
const HEAD_H: i32 = 96;
/// Pas na dole: legenda nawigacji.
const FOOT_H: i32 = 56;

/// Geometria siatki wyliczona dla konkretnego płótna.
struct Grid {
    margin: i32,
    cell_w: i32,
    cell_h: i32,
    top: i32,
    left: i32,
}

impl Grid {
    fn of(c: &Gray8) -> Self {
        let w = c.width() as i32;
        let h = body_h(c);
        let margin = 12;
        let usable_w = w - 2 * margin;
        let usable_h = h - HEAD_H - FOOT_H;
        Self {
            margin,
            cell_w: usable_w / COLS,
            cell_h: usable_h / WEEKS,
            top: HEAD_H,
            left: margin + (usable_w - (usable_w / COLS) * COLS) / 2,
        }
    }

    fn cell(&self, col: i32, row: i32) -> Rect {
        Rect::new(
            self.left + col * self.cell_w,
            self.top + row * self.cell_h,
            self.cell_w,
            self.cell_h,
        )
    }
}

/// Ile wydarzeń przypada na każdy dzień miesiąca.
///
/// Zwraca tablicę indeksowaną dniem − 1. Liczymy z `model.days`, czyli z tego, co
/// urządzenie realnie pobrało — a pobiera [`HORIZON_DAYS`] dni do przodu. Miesiąc
/// jest więc z natury NIEPEŁNY i widok nie ma prawa udawać, że wie o całym miesiącu.
/// Patrz `pokryte` niżej.
fn counts_for(model: &Model, year: i32, month: u32) -> [u8; 31] {
    let mut out = [0u8; 31];
    for day in &model.days {
        if day.date.year() == year && day.date.month() == month {
            let i = (day.date.day() - 1) as usize;
            out[i] = day.events.len().min(u8::MAX as usize) as u8;
        }
    }
    out
}

/// Zakres dni, o których urządzenie cokolwiek wie.
///
/// Poza nim kratka jest pusta NIE dlatego, że nic się nie dzieje, tylko dlatego,
/// że nikt nie pytał. To są dwie różne rzeczy i widok musi je odróżniać — inaczej
/// pusty koniec miesiąca czyta się jako „mam wolne", co bywa nieprawdą.
///
/// Bierzemy to z [`Model::known`], a NIE z pierwszego i ostatniego wpisu w `days`.
/// Pierwsza wersja tego widoku wyprowadzała zakres z `days` i był to ten sam błąd,
/// tylko odwrócony: `group_by_day` nie tworzy grup dla dni bez wydarzeń, więc wolny
/// wtorek w środku horyzontu wychodził jako dzień, o który nie pytano.
///
/// `days` zostaje jako awaryjne źródło dla modeli budowanych ręcznie — w testach
/// i w podglądzie — które `known` mogą nie mieć.
pub(crate) fn covered_range(model: &Model) -> Option<(NaiveDate, NaiveDate)> {
    if let Some(zakres) = model.known {
        return Some(zakres);
    }
    let first = model.days.first()?.date;
    let last = model.days.last()?.date;
    Some((first, last))
}

/// Rysuje widok miesięczny dla miesiąca, w którym leży `model.now`.
pub fn render_month(model: &Model, fonts: &Fonts, c: &mut Gray8) -> Screen {
    c.clear(WHITE);
    let mut screen = Screen::default();
    let g = Grid::of(c);
    let w = c.width() as i32;

    let today = model.now.date();
    let (year, month) = (today.year(), today.month());

    // --- nagłówek ----------------------------------------------------------
    fonts.draw(
        c,
        crate::model::miesiac_mianownik(today),
        g.margin as f32,
        44.0,
        TEXT_HEAD,
        Weight::Bold,
        BLACK,
        Align::Left,
    );
    fonts.draw(
        c,
        &format!("{year}"),
        (w - g.margin) as f32,
        44.0,
        TEXT_HEAD,
        Weight::Medium,
        INK_FAINT,
        Align::Right,
    );

    // Skróty dni. Poniedziałek pierwszy — tydzień roboczy zaczyna się w poniedziałek
    // i tak samo wygląda każdy papierowy kalendarz w tym kraju.
    const DNI: [&str; 7] = ["pn", "wt", "śr", "cz", "pt", "so", "nd"];
    for (i, d) in DNI.iter().enumerate() {
        let cell = g.cell(i as i32, 0);
        fonts.draw(
            c,
            d,
            (cell.x + cell.w / 2) as f32,
            (HEAD_H - 14) as f32,
            TEXT_BODY,
            Weight::Medium,
            INK_FAINT,
            Align::Center,
        );
    }
    hline(c, g.margin, HEAD_H - 6, w - 2 * g.margin, 2, BLACK);

    // --- siatka ------------------------------------------------------------
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(today);
    let offset = first.weekday().num_days_from_monday() as i32;
    let counts = counts_for(model, year, month);
    let zakres = covered_range(model);

    for row in 0..WEEKS {
        for col in 0..COLS {
            let idx = row * COLS + col - offset;
            // Dni z sąsiednich miesięcy NIE są pomijane: pusta kratka w rogu siatki
            // każe się domyślać, gdzie miesiąc się zaczyna, a wyblakła data mówi to
            // wprost. Poza tym 31 marca i 1 kwietnia bywają tym samym tygodniem
            // i dziura między nimi jest myląca.
            let Some(data) = first.checked_add_signed(chrono::Duration::days(idx as i64)) else {
                continue;
            };
            let obcy = data.month() != month;
            let cell = g.cell(col, row);
            let wie = zakres.is_some_and(|(a, b)| data >= a && data <= b);
            let liczba = if obcy {
                obcy_count(model, data)
            } else {
                counts[idx as usize]
            };
            draw_day(
                fonts,
                c,
                cell,
                data,
                liczba,
                data == today,
                wie,
                obcy,
                today,
            );
        }
    }

    // --- stopka ------------------------------------------------------------
    let foot = body_h(c) - FOOT_H;
    hline(c, g.margin, foot, w - 2 * g.margin, 1, INK_FAINT);
    let podpis = match zakres {
        Some((a, b)) => format!(
            "znane: {}.{:02} – {}.{:02}",
            a.day(),
            a.month(),
            b.day(),
            b.month()
        ),
        None => "brak pobranych danych".to_string(),
    };
    fonts.draw(
        c,
        &podpis,
        g.margin as f32,
        (foot + 34) as f32,
        TEXT_BODY,
        Weight::Medium,
        INK_FAINT,
        Align::Left,
    );

    c.quantize_ink();
    screen.pages = 1;
    screen
}

/// Rysuje jedną kratkę.
#[allow(clippy::too_many_arguments)]
fn draw_day(
    fonts: &Fonts,
    c: &mut Gray8,
    cell: Rect,
    data: NaiveDate,
    events: u8,
    dzis: bool,
    wie: bool,
    // `obcy`: dzień z sąsiedniego miesiąca, w siatce dla ciągłości tygodnia.
    obcy: bool,
    today: NaiveDate,
) {
    // Dzień poza pobranym zakresem dostaje delikatny raster zamiast pustki —
    // „nie wiem" ma wyglądać inaczej niż „nic nie ma". Dni MINIONE rastru nie
    // dostają, choć urządzenie też o nich nie wie: przeszłość nie jest luką
    // w wiedzy, tylko czymś, co przestało być pytaniem. To jedyne miejsce, gdzie
    // przeszłość jest traktowana inaczej — sama data i jej paski wyglądają tak
    // samo jak wszędzie indziej.
    if !wie && data >= today {
        dither_rect(c, cell.inset(2), 1);
    }

    let pad = 6;
    let num_base = (cell.y + 30) as f32;

    // Podkładka pod numerem. Na kratce z rastrem cyfra bez niej siada na kropkach
    // i przestaje być cyfrą — a raster oznacza „nie wiem", więc pojawia się akurat
    // tam, gdzie data jest najbardziej potrzebna do orientacji.
    //
    // Numer jest ZAWSZE po lewej, także dzisiaj. Wyśrodkowanie tylko jednej kratki
    // sprawiało, że oko szukało dzisiejszej daty w innym miejscu niż wszystkich
    // pozostałych — wyróżnienie ma przyciągać wzrok, a nie przestawiać rytm.
    let numer = data.day().to_string();
    let num_w = fonts.measure(&numer, TEXT_LEAD, Weight::Bold).ceil() as i32;
    let plama = Rect::new(cell.x + 2, cell.y + 2, num_w + 2 * pad, 34);

    if dzis {
        // Negatyw: jedyne wyróżnienie, które na tym panelu działa pewnie.
        c.fill_rect(plama, BLACK);
    } else {
        c.fill_rect(plama, WHITE);
    }

    fonts.draw(
        c,
        &numer,
        (cell.x + pad + 2) as f32,
        num_base,
        TEXT_LEAD,
        if dzis { Weight::Bold } else { Weight::Medium },
        // Dzień z sąsiedniego miesiąca w LIGHTEST_VISIBLE, nie w INK_FAINT.
        // Wbrew nazwie `INK_FAINT` (0x33) po `quantize_ink` ląduje na poziomie 1,
        // czyli praktycznie na czerni — wyszarzenie nim jest niewidoczne.
        // Poziom 3 to najjaśniejszy ton, którym da się na tym panelu pisać.
        match (dzis, obcy) {
            (true, _) => WHITE,
            (false, true) => LIGHTEST_VISIBLE,
            (false, false) => BLACK,
        },
        Align::Left,
    );

    // Licznik nadmiaru stoi w WIERSZU NUMERU, po prawej — nie pod paskami.
    // Pod paskami mieścił się tylko w pionie: w poziomie kratka ma 64 px wysokości
    // (wobec 118 w pionie), więc licznik wypadał poza nią i lądował w cudzym dniu.
    // Jedno miejsce działające w obu orientacjach jest warte więcej niż ładniejsze
    // w jednej.
    if events as usize > MAX_BARS {
        let txt = format!("+{}", events as usize - MAX_BARS);
        let tw = fonts.measure(&txt, TEXT_FLOOR, Weight::Bold).ceil() as i32;
        c.fill_rect(
            Rect::new(cell.right() - tw - pad - 4, cell.y + 2, tw + pad, 30),
            WHITE,
        );
        fonts.draw(
            c,
            &txt,
            (cell.right() - pad - 2) as f32,
            (cell.y + 26) as f32,
            TEXT_FLOOR,
            Weight::Bold,
            BLACK,
            Align::Right,
        );
    }

    if events == 0 {
        return;
    }

    // Paski gęstości. Pełna szerokość kratki minus margines, żeby czytały się
    // z dystansu — patrz nagłówek modułu.
    let paski = (events as usize).min(MAX_BARS);
    let mut y = cell.y + 40;
    for _ in 0..paski {
        // Paski obcego miesiąca w słabszym atramencie: informacja zostaje, ale nie
        // konkuruje z miesiącem, który jest tematem ekranu.
        let ton = if obcy { FILL_LIGHT } else { BLACK };
        c.fill_rect(Rect::new(cell.x + pad, y, cell.w - 2 * pad, BAR_H), ton);
        y += BAR_H + BAR_GAP;
    }
}

/// Liczba wydarzeń w dniu spoza bieżącego miesiąca.
///
/// `counts_for` buduje tablicę indeksowaną dniem miesiąca, więc dla 31 marca
/// widocznego w siatce kwietnia nie ma tam miejsca. Sąsiednich dni jest najwyżej
/// dwanaście, więc liniowe przejście po `model.days` jest tańsze niż drugi bufor.
fn obcy_count(model: &Model, data: NaiveDate) -> u8 {
    model
        .days
        .iter()
        .find(|d| d.date == data)
        .map_or(0, |d| d.events.len().min(u8::MAX as usize) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Rotation;

    fn model_na(rok: i32, mies: u32, dzien: u32) -> Model {
        let now = NaiveDate::from_ymd_opt(rok, mies, dzien)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        Model::empty(now)
    }

    #[test]
    fn siatka_zawsze_ma_szesc_tygodni() {
        // Zmienna liczba wierszy przesuwałaby te same daty między miesiącami.
        for (rok, mies) in [(2026, 2), (2026, 8), (2027, 1)] {
            let m = model_na(rok, mies, 1);
            let mut c = Gray8::new(Rotation::Portrait);
            let g = Grid::of(&c);
            assert_eq!(
                g.cell(0, WEEKS - 1).bottom() <= body_h(&c) - FOOT_H,
                true,
                "{rok}-{mies}: szósty tydzień wychodzi poza siatkę"
            );
            render_month(&m, &Fonts::embedded(), &mut c);
        }
    }

    #[test]
    fn kazdy_dzien_miesiaca_ma_swoja_kratke() {
        // Off-by-one w przesunięciu pierwszego dnia gubi 1 albo ostatni dzień —
        // a tego na gotowym obrazku nie widać.
        for (rok, mies, ile) in [(2026, 2, 28), (2028, 2, 29), (2026, 8, 31), (2026, 4, 30)] {
            assert_eq!(crate::year::dni_miesiaca(rok, mies), ile, "{rok}-{mies}");
            let first = NaiveDate::from_ymd_opt(rok, mies, 1).unwrap();
            let offset = first.weekday().num_days_from_monday() as i32;
            assert!(
                offset + ile <= WEEKS * COLS,
                "{rok}-{mies}: {ile} dni z przesunięciem {offset} nie mieści się w sześciu tygodniach"
            );
        }
    }

    #[test]
    fn wolny_dzien_w_horyzoncie_nie_dostaje_woalu() {
        // To jest błąd, który miał ten widok w pierwszej wersji: zakres „co wiemy"
        // brał się z `days`, a `group_by_day` nie tworzy grup dla dni bez wydarzeń.
        // Wolny wtorek w środku horyzontu wychodził więc jako dzień, o który nikt
        // nie pytał — czyli dokładnie to kłamstwo, przed którym woal miał bronić.
        let mut m = model_na(2026, 8, 10);
        m.known = Some((
            NaiveDate::from_ymd_opt(2026, 8, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 24).unwrap(),
        ));
        // Wydarzenia tylko 10 i 24 — środek horyzontu jest pusty, ale ZNANY.
        m.days = vec![];

        let zakres = covered_range(&m).expect("known ma pierwszeństwo przed days");
        assert_eq!(zakres.0, NaiveDate::from_ymd_opt(2026, 8, 10).unwrap());
        assert_eq!(zakres.1, NaiveDate::from_ymd_opt(2026, 8, 24).unwrap());

        let srodek = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        assert!(
            srodek >= zakres.0 && srodek <= zakres.1,
            "wolny dzień w środku horyzontu musi być uznany za ZNANY"
        );
    }

    #[test]
    fn renderuje_sie_w_obu_orientacjach_i_nie_dotyka_krawedzi() {
        let m = model_na(2026, 8, 18);
        let fonts = Fonts::embedded();
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            render_month(&m, &fonts, &mut c);
            let w = c.width() as i32;
            let h = body_h(&c);
            for y in 0..h {
                assert_eq!(c.get(0, y), WHITE, "{rot:?}: atrament na lewej krawędzi");
                assert_eq!(
                    c.get(w - 1, y),
                    WHITE,
                    "{rot:?}: atrament na prawej krawędzi"
                );
            }
        }
    }
}
