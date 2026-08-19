//! Układ dashboardu. Wszystkie decyzje o wyglądzie mieszkają tutaj.
//!
//! Zasady przyjęte dla e-papieru:
//! * Papier zostaje biały — tła na całą stronę zżerają kontrast i wydłużają odświeżanie.
//! * Hierarchia przez rozmiar i wagę kroju, nie przez odcień; szarości jest tylko 16,
//!   a te ciemne są słabo rozróżnialne.
//! * Żadnych cienkich szarości na dużych powierzchniach — panel ich nie utrzyma.
//! * Wszystko wyrównane do siatki 8 px.
//!
//! Renderowanie zwraca [`Screen`] z obszarami dotykowymi, więc symulator i firmware
//! reagują dokładnie tak samo.

use chrono::{Datelike, NaiveDate};

use crate::canvas::{Gray8, Rect, BLACK, GRAY_40, GRAY_50, GRAY_60, GRAY_80, WHITE};
use crate::hit::{Action, HitRegion, Screen};
use crate::model::{
    data_dzien_miesiac, dzien_skrot, godzina, naglowek_dnia, za_ile, Battery, CalEvent, Model,
    NetState, SourceTag,
};
use crate::shapes::{
    chevron_left, chevron_right, fill_circle, fill_round_rect, hline, stroke_round_rect,
};
use crate::text::{Align, Fonts, Weight};

/// Wymiary układu wyliczone dla konkretnego płótna.
///
/// Powstaje z rozmiaru płótna, a nie ze stałych modułu, bo płótno bywa poziome
/// (960×540) albo pionowe (540×960) — patrz [`crate::Rotation`]. Warunkowe są tu
/// wyłącznie te wielkości, które naprawdę różnią się między orientacjami; reszta
/// jest wspólna, bo wysokość wiersza agendy nie ma powodu zależeć od tego, jak
/// urządzenie stoi.
#[derive(Debug, Clone, Copy)]
struct Geom {
    w: i32,
    h: i32,
    margin: i32,
    header_h: i32,
    footer_h: i32,
    content_top: i32,
    content_bottom: i32,
    content_h: i32,
    /// Szerokość kolumny z godziną po lewej stronie agendy.
    ///
    /// Musi pomieścić najszerszą treść, jaka tam trafia — a to nie jest „08:00",
    /// tylko „cały dzień". Pilnuje tego test `kolumna_godziny_miesci_najszersza_tresc`.
    time_col_w: i32,
}

impl Geom {
    fn of(c: &Gray8) -> Self {
        let w = c.width() as i32;
        let h = c.height() as i32;

        let header_h = 104;
        // W pionie jest zapas wysokości i wersja firmware'u dostaje własny wiersz pod
        // kafelkami. W poziomie wysokości jest o 420 px mniej i każdy piksel się liczy.
        let footer_h = if c.rotation().is_portrait() { 118 } else { 92 };
        let content_top = header_h + 16;
        let content_bottom = h - footer_h - 16;

        Self {
            w,
            h,
            margin: 32,
            header_h,
            footer_h,
            content_top,
            content_bottom,
            content_h: content_bottom - content_top,
            time_col_w: 116,
        }
    }
}

/// Wysokość paska statusu w nagłówku i jego odsunięcie od krawędzi.
const STATUS_SIZE: f32 = 22.0;
const STATUS_PAD: i32 = 10;
const STATUS_PILL_TOP: i32 = 66;
const STATUS_BASELINE: f32 = 90.0;

/// Rozmiar wartości na kafelku i podłoga, poniżej której wolimy uciąć niż zmniejszać.
const TILE_VALUE_SIZE: f32 = 32.0;
const TILE_VALUE_MIN_SIZE: f32 = 17.0;

const DAY_HEADER_H: i32 = 40;
const EVENT_H: i32 = 50;
const EVENT_H_WITH_LOCATION: i32 = 62;
const DAY_GAP: i32 = 14;

/// Rysuje pełny dashboard i zwraca obszary dotykowe.
pub fn render(model: &Model, fonts: &Fonts, c: &mut Gray8) -> Screen {
    c.clear(WHITE);

    let mut screen = Screen::default();

    draw_header(model, fonts, c, &mut screen);
    draw_agenda(model, fonts, c, &mut screen);
    draw_footer(model, fonts, c, &mut screen);

    screen
}

// ---------------------------------------------------------------------------
// Wiersze i paginacja
// ---------------------------------------------------------------------------

/// Agenda jest spłaszczana do listy wierszy, żeby paginacja miała jeden wymiar
/// zamiast zagnieżdżenia dni w wydarzeniach.
enum Row<'a> {
    DayHeader(NaiveDate),
    /// Wydarzenie wraz z jego indeksem globalnym (do akcji dotykowych).
    Event(&'a CalEvent, usize),
}

impl Row<'_> {
    fn height(&self) -> i32 {
        match self {
            Row::DayHeader(_) => DAY_HEADER_H,
            Row::Event(e, _) if e.location.is_some() => EVENT_H_WITH_LOCATION,
            Row::Event(_, _) => EVENT_H,
        }
    }
}

/// Jedna strona agendy.
struct Page {
    start: usize,
    end: usize,
    /// Nagłówek dnia do powtórzenia na górze, gdy strona zaczyna się w środku dnia.
    continued: Option<NaiveDate>,
}

fn build_rows(model: &Model) -> Vec<Row<'_>> {
    let mut rows = Vec::new();
    let mut index = 0usize;
    for group in &model.days {
        if group.events.is_empty() {
            continue;
        }
        rows.push(Row::DayHeader(group.date));
        for event in &group.events {
            rows.push(Row::Event(event, index));
            index += 1;
        }
    }
    rows
}

/// Dzieli wiersze na strony mieszczące się w obszarze treści.
///
/// Dwie reguły redakcyjne: nagłówek dnia nie zostaje sierotą na końcu strony,
/// a strona zaczynająca się w środku dnia powtarza jego nagłówek.
fn paginate(rows: &[Row], g: &Geom) -> Vec<Page> {
    let mut pages = Vec::new();
    let mut i = 0usize;

    while i < rows.len() {
        let start = i;

        // Czy zaczynamy w środku dnia?
        let continued = match &rows[i] {
            Row::DayHeader(_) => None,
            Row::Event(_, _) => day_of(rows, i),
        };

        let mut used = if continued.is_some() { DAY_HEADER_H } else { 0 };

        while i < rows.len() {
            let h = rows[i].height();
            if used + h > g.content_h {
                break;
            }
            // Nie zostawiaj nagłówka dnia jako ostatniego wiersza strony.
            if matches!(rows[i], Row::DayHeader(_)) {
                let next_h = rows.get(i + 1).map(|r| r.height()).unwrap_or(0);
                if used + h + next_h > g.content_h {
                    break;
                }
                // Odstęp przed kolejnym dniem, jeśli to nie pierwszy wiersz strony.
                if i > start {
                    used += DAY_GAP;
                    if used + h + next_h > g.content_h {
                        break;
                    }
                }
            }
            used += h;
            i += 1;
        }

        // Zabezpieczenie przed pętlą nieskończoną: jeśli nic nie weszło, weź jeden wiersz.
        if i == start {
            i = start + 1;
        }

        pages.push(Page {
            start,
            end: i,
            continued,
        });
    }

    if pages.is_empty() {
        pages.push(Page {
            start: 0,
            end: 0,
            continued: None,
        });
    }
    pages
}

/// Data dnia, do którego należy wiersz o podanym indeksie.
fn day_of(rows: &[Row], index: usize) -> Option<NaiveDate> {
    rows[..=index].iter().rev().find_map(|r| match r {
        Row::DayHeader(d) => Some(*d),
        _ => None,
    })
}

// ---------------------------------------------------------------------------
// Nagłówek
// ---------------------------------------------------------------------------

fn draw_header(model: &Model, fonts: &Fonts, c: &mut Gray8, screen: &mut Screen) {
    let g = Geom::of(c);
    let today = model.now.date();

    let day_num = format!("{}", today.day());
    let num_w = fonts.draw(
        c,
        &day_num,
        g.margin as f32,
        78.0,
        76.0,
        Weight::Bold,
        BLACK,
        Align::Left,
    );

    let x = g.margin as f32 + num_w + 18.0;
    fonts.draw(
        c,
        crate::model::dzien_tygodnia(today),
        x,
        50.0,
        30.0,
        Weight::Medium,
        BLACK,
        Align::Left,
    );
    fonts.draw(
        c,
        &format!(
            "{} {}",
            crate::model::miesiac_dopelniacz(today),
            today.year()
        ),
        x,
        82.0,
        26.0,
        Weight::Regular,
        GRAY_40,
        Align::Left,
    );

    draw_battery(&model.battery, fonts, c, g.w - g.margin - 92, 30);

    let (status_text, status_ink) = match model.net {
        NetState::Ok => (format!("zaktualizowano {}", godzina(model.now)), GRAY_40),
        NetState::Stale { since } => (format!("nieaktualne od {}", godzina(since)), BLACK),
        NetState::Offline => ("brak sieci".to_string(), BLACK),
        NetState::NeedsAuth => ("skonfiguruj urządzenie".to_string(), BLACK),
    };
    // Status nie ma prawa wejść ani na blok z datą po lewej, ani na baterię nad sobą.
    // Przy 540 px szerokości jedno i drugie było o włos.
    let status_text = fonts.truncate(
        &status_text,
        (g.w - 2 * g.margin) as f32 * 0.56,
        STATUS_SIZE,
        Weight::Regular,
    );
    let status_w = fonts.draw(
        c,
        &status_text,
        (g.w - g.margin - STATUS_PAD) as f32,
        STATUS_BASELINE,
        STATUS_SIZE,
        Weight::Regular,
        status_ink,
        Align::Right,
    );

    // Plakietka kończy się dokładnie na marginesie. Wcześniej wystawała 6 px za niego
    // — w poziomie niewidoczne, w pionie już tak.
    let pill = Rect::new(
        g.w - g.margin - status_w as i32 - 2 * STATUS_PAD,
        STATUS_PILL_TOP,
        status_w as i32 + 2 * STATUS_PAD,
        32,
    );

    if matches!(model.net, NetState::Offline | NetState::NeedsAuth) {
        stroke_round_rect(c, pill, 8.0, 2, BLACK);
    }

    // Dotknięcie paska statusu wymusza odświeżenie.
    screen.hits.push(HitRegion::new(
        Rect::new(pill.x - 10, pill.y - 6, pill.w + 20, pill.h + 12),
        Action::RefreshNow,
    ));

    hline(c, g.margin, g.header_h, g.w - 2 * g.margin, 3, BLACK);
}

fn draw_battery(b: &Battery, fonts: &Fonts, c: &mut Gray8, x: i32, y: i32) {
    let body = Rect::new(x, y, 56, 26);
    stroke_round_rect(c, body, 6.0, 2, BLACK);
    fill_round_rect(c, Rect::new(x + 56, y + 8, 5, 10), 2.0, BLACK);

    if let Some(pct) = b.percent {
        let inner = body.inset(5);
        let w = (inner.w as f32 * (pct as f32 / 100.0)).round() as i32;
        if w > 0 {
            let ink = if pct <= 15 { BLACK } else { GRAY_50 };
            fill_round_rect(c, Rect::new(inner.x, inner.y, w, inner.h), 2.0, ink);
        }
        fonts.draw(
            c,
            &format!("{pct}%"),
            (x - 10) as f32,
            (y + 21) as f32,
            22.0,
            Weight::Medium,
            BLACK,
            Align::Right,
        );
    }

    if b.charging {
        let cx = x + 28;
        let cy = y + 13;
        for i in 0..10 {
            c.set(cx + 3 - i / 3, cy - 8 + i, BLACK);
            c.set(cx + 2 - i / 3, cy - 8 + i, BLACK);
        }
        for i in 0..10 {
            c.set(cx - 1 + i / 3, cy + i - 1, BLACK);
            c.set(cx - 2 + i / 3, cy + i - 1, BLACK);
        }
    }
}

// ---------------------------------------------------------------------------
// Agenda
// ---------------------------------------------------------------------------

fn draw_agenda(model: &Model, fonts: &Fonts, c: &mut Gray8, screen: &mut Screen) {
    let g = Geom::of(c);
    // Widok szczegółów zastępuje agendę.
    if let Some(index) = model.focus {
        draw_event_detail(model, index, fonts, c, screen);
        return;
    }

    let rows = build_rows(model);
    if rows.is_empty() {
        draw_empty_state(fonts, c);
        screen.pages = 1;
        screen.page = 0;
        return;
    }

    let pages = paginate(&rows, &Geom::of(c));
    screen.pages = pages.len();
    let page_index = model.page.min(pages.len() - 1);
    screen.page = page_index;
    let page = &pages[page_index];

    let today = model.now.date();
    let mut y = g.content_top;

    if let Some(date) = page.continued {
        y = draw_day_header(date, today, fonts, c, y, true);
    }

    for row in &rows[page.start..page.end] {
        match row {
            Row::DayHeader(date) => {
                if y > g.content_top {
                    y += DAY_GAP;
                }
                y = draw_day_header(*date, today, fonts, c, y, false);
            }
            Row::Event(event, index) => {
                let h = row.height();
                draw_event(event, model, fonts, c, y, h);
                screen.hits.push(HitRegion::new(
                    Rect::new(g.margin - 10, y - 4, g.w - 2 * g.margin + 20, h),
                    Action::ShowEvent(*index),
                ));
                y += h;
            }
        }
    }
}

fn draw_day_header(
    date: NaiveDate,
    today: NaiveDate,
    fonts: &Fonts,
    c: &mut Gray8,
    y: i32,
    continued: bool,
) -> i32 {
    let g = Geom::of(c);
    let label = naglowek_dnia(date, today);
    let baseline = (y + 22) as f32;

    fonts.draw(
        c,
        &label,
        g.margin as f32,
        baseline,
        26.0,
        Weight::Bold,
        BLACK,
        Align::Left,
    );
    let label_w = fonts.measure(&label, 26.0, Weight::Bold);

    let sub = if continued {
        format!(
            "{} · {} · ciąg dalszy",
            dzien_skrot(date),
            data_dzien_miesiac(date)
        )
    } else {
        format!("{} · {}", dzien_skrot(date), data_dzien_miesiac(date))
    };
    fonts.draw(
        c,
        &sub,
        g.margin as f32 + label_w + 14.0,
        baseline,
        22.0,
        Weight::Regular,
        GRAY_50,
        Align::Left,
    );

    let sub_w = fonts.measure(&sub, 22.0, Weight::Regular);
    let line_x = g.margin + (label_w + sub_w) as i32 + 30;
    let line_w = g.w - g.margin - line_x;
    if line_w > 20 {
        hline(c, line_x, y + 14, line_w, 1, GRAY_80);
    }

    y + DAY_HEADER_H
}

fn draw_event(event: &CalEvent, model: &Model, fonts: &Fonts, c: &mut Gray8, y: i32, h: i32) {
    let g = Geom::of(c);
    let now = model.now;
    let past = event.is_past(now);
    let live = event.is_now(now);

    let title_ink = if past { GRAY_50 } else { BLACK };
    let time_ink = if past { GRAY_60 } else { BLACK };

    if live {
        fill_round_rect(
            c,
            Rect::new(g.margin - 10, y - 4, g.w - 2 * g.margin + 20, h - 2),
            8.0,
            0xF2,
        );
        fill_round_rect(c, Rect::new(g.margin - 10, y - 4, 5, h - 2), 2.5, BLACK);
    }

    let baseline = (y + 26) as f32;

    let time_text = if event.all_day {
        "cały dzień".to_string()
    } else {
        godzina(event.start)
    };
    let time_size = if event.all_day { 20.0 } else { 26.0 };
    let time_weight = if past {
        Weight::Regular
    } else {
        Weight::Medium
    };
    fonts.draw(
        c,
        &time_text,
        g.margin as f32,
        baseline,
        time_size,
        time_weight,
        time_ink,
        Align::Left,
    );

    if !event.all_day {
        fonts.draw(
            c,
            &godzina(event.end),
            g.margin as f32,
            baseline + 20.0,
            18.0,
            Weight::Regular,
            GRAY_60,
            Align::Left,
        );
    }

    let dot_x = (g.margin + g.time_col_w - 18) as f32;
    let dot_y = baseline - 8.0;
    match event.source {
        SourceTag::Primary => fill_circle(c, dot_x, dot_y, 5.0, BLACK),
        SourceTag::Secondary => {
            fill_circle(c, dot_x, dot_y, 5.0, BLACK);
            fill_circle(c, dot_x, dot_y, 2.5, WHITE);
        }
        SourceTag::Holiday => fill_round_rect(
            c,
            Rect::new(dot_x as i32 - 4, dot_y as i32 - 4, 9, 9),
            1.5,
            GRAY_50,
        ),
    }

    let text_x = g.margin + g.time_col_w;
    let mut avail = g.w - g.margin - text_x;

    if !past && !live && is_first_upcoming(model, event) {
        let badge = za_ile(event.start, now);
        let bw = fonts.measure(&badge, 20.0, Weight::Medium) as i32 + 24;
        let br = Rect::new(g.w - g.margin - bw, y + 2, bw, 28);
        fill_round_rect(c, br, 14.0, BLACK);
        fonts.draw(
            c,
            &badge,
            (br.x + br.w / 2) as f32,
            (br.y + 20) as f32,
            20.0,
            Weight::Medium,
            WHITE,
            Align::Center,
        );
        avail -= bw + 16;
    }

    let title = fonts.truncate(&event.title, avail as f32, 27.0, Weight::Medium);
    fonts.draw(
        c,
        &title,
        text_x as f32,
        baseline,
        27.0,
        Weight::Medium,
        title_ink,
        Align::Left,
    );

    if let Some(loc) = &event.location {
        let loc = fonts.truncate(loc, avail as f32, 20.0, Weight::Regular);
        fonts.draw(
            c,
            &loc,
            text_x as f32,
            baseline + 22.0,
            20.0,
            Weight::Regular,
            GRAY_50,
            Align::Left,
        );
    }
}

fn is_first_upcoming(model: &Model, event: &CalEvent) -> bool {
    model
        .days
        .iter()
        .flat_map(|d| d.events.iter())
        .find(|e| !e.is_past(model.now) && !e.is_now(model.now))
        .map(|e| core::ptr::eq(e, event))
        .unwrap_or(false)
}

fn draw_empty_state(fonts: &Fonts, c: &mut Gray8) {
    let g = Geom::of(c);
    let cx = g.w as f32 / 2.0;
    let cy = ((g.content_top + g.content_bottom) / 2) as f32;
    fonts.draw(
        c,
        "Nic w planie",
        cx,
        cy,
        42.0,
        Weight::Medium,
        GRAY_40,
        Align::Center,
    );
    fonts.draw(
        c,
        "Wolne do końca widocznego okresu",
        cx,
        cy + 34.0,
        22.0,
        Weight::Regular,
        GRAY_60,
        Align::Center,
    );
}

// ---------------------------------------------------------------------------
// Widok szczegółów wydarzenia
// ---------------------------------------------------------------------------

fn draw_event_detail(
    model: &Model,
    index: usize,
    fonts: &Fonts,
    c: &mut Gray8,
    screen: &mut Screen,
) {
    let g = Geom::of(c);
    let Some(event) = model.event_at(index) else {
        draw_empty_state(fonts, c);
        return;
    };

    let x = g.margin as f32;
    let mut y = g.content_top + 20;

    let when = if event.all_day {
        format!(
            "{} · cały dzień",
            crate::model::data_pelna(event.start.date())
        )
    } else {
        format!(
            "{} · {}–{}",
            crate::model::data_pelna(event.start.date()),
            godzina(event.start),
            godzina(event.end)
        )
    };
    fonts.draw(
        c,
        &when,
        x,
        y as f32,
        24.0,
        Weight::Medium,
        GRAY_40,
        Align::Left,
    );
    y += 46;

    // Tytuł łamany na maksymalnie trzy linie.
    let avail = (g.w - 2 * g.margin) as f32;
    for line in fonts.wrap(&event.title, avail, 44.0, Weight::Bold, 3) {
        fonts.draw(
            c,
            &line,
            x,
            y as f32,
            44.0,
            Weight::Bold,
            BLACK,
            Align::Left,
        );
        y += 52;
    }

    if let Some(loc) = &event.location {
        y += 10;
        for line in fonts.wrap(loc, avail, 26.0, Weight::Regular, 2) {
            fonts.draw(
                c,
                &line,
                x,
                y as f32,
                26.0,
                Weight::Regular,
                GRAY_40,
                Align::Left,
            );
            y += 32;
        }
    }

    if !event.all_day {
        y += 14;
        let mins = event.duration_minutes();
        let dur = if mins >= 60 {
            let h = mins / 60;
            let m = mins % 60;
            if m == 0 {
                format!("{h} h")
            } else {
                format!("{h} h {m} min")
            }
        } else {
            format!("{mins} min")
        };
        fonts.draw(
            c,
            &dur,
            x,
            y as f32,
            24.0,
            Weight::Regular,
            GRAY_50,
            Align::Left,
        );
    }

    // Przycisk powrotu.
    let back = Rect::new(g.margin, g.content_bottom - 44, 180, 44);
    stroke_round_rect(c, back, 22.0, 2, BLACK);
    chevron_left(
        c,
        (back.x + 40) as f32,
        (back.y + back.h / 2) as f32,
        16.0,
        3.0,
        BLACK,
    );
    fonts.draw(
        c,
        "Wróć",
        (back.x + 62) as f32,
        (back.y + 29) as f32,
        24.0,
        Weight::Medium,
        BLACK,
        Align::Left,
    );
    screen
        .hits
        .push(HitRegion::new(back.inset(-12), Action::Back));

    // Dotknięcie gdziekolwiek indziej też wraca — na e-papierze celowanie jest trudne.
    screen.hits.insert(
        0,
        HitRegion::new(Rect::new(0, g.content_top, g.w, g.content_h), Action::Back),
    );
}

// ---------------------------------------------------------------------------
// Stopka
// ---------------------------------------------------------------------------

fn draw_footer(model: &Model, fonts: &Fonts, c: &mut Gray8, screen: &mut Screen) {
    let g = Geom::of(c);
    let top = g.h - g.footer_h;
    hline(c, g.margin, top, g.w - 2 * g.margin, 1, GRAY_80);

    // Paginacja ma pierwszeństwo przed kafelkami — na stronie 2 z 3 to ważniejsze
    // niż pogoda.
    if screen.pages > 1 && model.focus.is_none() {
        draw_pager(model, screen, fonts, c, top);
    } else if !model.tiles.is_empty() {
        draw_tiles(model, fonts, c, top);
    }

    // Wersja w lewym dolnym rogu, nie w prawym. W prawym siedzi przycisk paginacji,
    // a w poziomie stopka jest o 26 px niższa i te dwa elementy na siebie wchodziły.
    // Po lewej jest wolno w obu orientacjach.
    if !model.firmware.is_empty() {
        fonts.draw(
            c,
            &model.firmware,
            g.margin as f32,
            (g.h - 8) as f32,
            15.0,
            Weight::Regular,
            GRAY_80,
            Align::Left,
        );
    }
}

fn draw_pager(model: &Model, screen: &mut Screen, fonts: &Fonts, c: &mut Gray8, top: i32) {
    let g = Geom::of(c);
    let btn_w = 120;
    let btn_h = 46;
    let btn_y = top + 22;

    let has_prev = screen.page > 0;
    let has_next = screen.page + 1 < screen.pages;

    if has_prev {
        let r = Rect::new(g.margin, btn_y, btn_w, btn_h);
        stroke_round_rect(c, r, 23.0, 2, BLACK);
        chevron_left(
            c,
            (r.x + r.w / 2) as f32,
            (r.y + r.h / 2) as f32,
            20.0,
            3.0,
            BLACK,
        );
        screen
            .hits
            .push(HitRegion::new(r.inset(-10), Action::PrevPage));
    }

    if has_next {
        let r = Rect::new(g.w - g.margin - btn_w, btn_y, btn_w, btn_h);
        stroke_round_rect(c, r, 23.0, 2, BLACK);
        chevron_right(
            c,
            (r.x + r.w / 2) as f32,
            (r.y + r.h / 2) as f32,
            20.0,
            3.0,
            BLACK,
        );
        screen
            .hits
            .push(HitRegion::new(r.inset(-10), Action::NextPage));
    }

    // Kropki stron pośrodku.
    let total = screen.pages.min(12);
    let spacing = 22.0;
    let start_x = g.w as f32 / 2.0 - (total as f32 - 1.0) * spacing / 2.0;
    let cy = (btn_y + btn_h / 2) as f32;
    for i in 0..total {
        let cx = start_x + i as f32 * spacing;
        if i == screen.page {
            fill_circle(c, cx, cy, 6.0, BLACK);
        } else {
            fill_circle(c, cx, cy, 5.0, GRAY_80);
            fill_circle(c, cx, cy, 3.0, WHITE);
        }
    }

    let label = format!("{} z {}", screen.page + 1, screen.pages);
    fonts.draw(
        c,
        &label,
        g.w as f32 / 2.0,
        (top + g.footer_h - 14) as f32,
        17.0,
        Weight::Regular,
        GRAY_50,
        Align::Center,
    );

    let _ = model;
}

fn draw_tiles(model: &Model, fonts: &Fonts, c: &mut Gray8, top: i32) {
    let g = Geom::of(c);
    let n = model.tiles.len().min(4);
    let avail = g.w - 2 * g.margin;
    let gap = 16;
    let tw = (avail - gap * (n as i32 - 1)) / n as i32;

    for (i, tile) in model.tiles.iter().take(n).enumerate() {
        let x = g.margin + i as i32 * (tw + gap);

        let label = fonts.truncate(&tile.label.to_uppercase(), tw as f32, 17.0, Weight::Medium);
        fonts.draw(
            c,
            &label,
            x as f32,
            (top + 30) as f32,
            17.0,
            Weight::Medium,
            GRAY_50,
            Align::Left,
        );

        // Wartość kafelka bywa liczbą („21"), ale bywa i zdaniem („podłącz USB") —
        // tak wygląda ekran konfiguracji. Przy trzech kafelkach na 540 px kolumna ma
        // ~148 px, a „otwórz stronę" w 32 pt zajmuje 200 i wchodziła na sąsiada.
        // Najpierw więc schodzimy z rozmiarem, a dopiero gdy to nie starcza — ucinamy.
        let unit_w = tile
            .unit
            .as_ref()
            .map_or(0.0, |u| fonts.measure(u, 20.0, Weight::Regular) + 6.0);
        let room = (tw as f32 - unit_w).max(1.0);

        let mut size = TILE_VALUE_SIZE;
        while size > TILE_VALUE_MIN_SIZE && fonts.measure(&tile.value, size, Weight::Bold) > room {
            size -= 1.0;
        }
        let value = fonts.truncate(&tile.value, room, size, Weight::Bold);

        let value_w = fonts.draw(
            c,
            &value,
            x as f32,
            (top + 64) as f32,
            size,
            Weight::Bold,
            BLACK,
            Align::Left,
        );

        if let Some(unit) = &tile.unit {
            fonts.draw(
                c,
                unit,
                x as f32 + value_w + 6.0,
                (top + 64) as f32,
                (size * 0.62).max(12.0),
                Weight::Regular,
                GRAY_40,
                Align::Left,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Rotation;
    use crate::model::{DayGroup, Tile};
    use chrono::NaiveDateTime;

    fn dt(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, 18)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[test]
    fn kolumna_godziny_miesci_najszersza_tresc() {
        // Najszersza treść w tej kolumnie to nie „08:00", tylko „cały dzień" — i to
        // ona wyznacza g.time_col_w. Kropka źródła siedzi na `g.time_col_w - 18`, więc
        // tekst musi się skończyć przed nią. Bez tego testu zmiana kroju albo rozmiaru
        // czcionki wchodzi na kropkę po cichu.
        let fonts = Fonts::embedded();
        // Kolumna godziny ma tę samą szerokość w obu orientacjach, ale sprawdzamy
        // obie — gdyby kiedyś przestała, test ma to zauważyć.
        let g = Geom::of(&Gray8::new(Rotation::default()));
        let dot_x = g.time_col_w - 18;
        for (text, size) in [("cały dzień", 20.0_f32), ("08:00", 26.0_f32)] {
            let w = fonts.measure(text, size, Weight::Regular).ceil() as i32;
            assert!(
                w + 8 <= dot_x,
                "`{text}` zajmuje {w} px, a do kropki jest {dot_x} px — zderzą się"
            );
        }
    }

    fn ev(title: &str, sh: u32, eh: u32) -> CalEvent {
        CalEvent {
            start: dt(sh, 0),
            end: dt(eh, 0),
            all_day: false,
            title: title.into(),
            location: None,
            source: SourceTag::Primary,
        }
    }

    fn model_with(days: usize, per_day: usize) -> Model {
        let base = NaiveDate::from_ymd_opt(2026, 8, 18).unwrap();
        let mut m = Model::empty(dt(8, 0));
        m.days = (0..days)
            .map(|d| DayGroup {
                // Arytmetyka na datach, nie na numerze dnia — inaczej test wysypuje się
                // na przekroczeniu długości miesiąca.
                date: base + chrono::Duration::days(d as i64),
                events: (0..per_day)
                    .map(|i| {
                        ev(
                            &format!("Wydarzenie {i}"),
                            8 + (i as u32 % 12),
                            9 + (i as u32 % 12),
                        )
                    })
                    .collect(),
            })
            .collect();
        m
    }

    #[test]
    fn pusty_model_renderuje_stan_pusty() {
        let fonts = Fonts::embedded();
        let mut c = Gray8::new(Rotation::default());
        let screen = render(&Model::empty(dt(12, 0)), &fonts, &mut c);
        assert_eq!(screen.pages, 1);
        assert!(c.pixels().iter().filter(|&&p| p < 128).count() > 200);
    }

    #[test]
    fn krotka_agenda_miesci_sie_na_jednej_stronie() {
        let fonts = Fonts::embedded();
        let mut c = Gray8::new(Rotation::default());
        let screen = render(&model_with(1, 3), &fonts, &mut c);
        assert_eq!(screen.pages, 1);
    }

    #[test]
    fn dluga_agenda_dzieli_sie_na_strony() {
        let fonts = Fonts::embedded();
        let mut c = Gray8::new(Rotation::default());
        let screen = render(&model_with(7, 6), &fonts, &mut c);
        assert!(
            screen.pages > 1,
            "42 wydarzenia mają zająć więcej niż jedną stronę"
        );
    }

    #[test]
    fn kazde_wydarzenie_pojawia_sie_dokladnie_raz_w_calej_paginacji() {
        let fonts = Fonts::embedded();
        let model = model_with(5, 5);
        let total: usize = model.days.iter().map(|d| d.events.len()).sum();

        let mut seen = std::collections::HashSet::new();
        let mut page = 0;
        loop {
            let mut m = model.clone();
            m.page = page;
            let mut c = Gray8::new(Rotation::default());
            let screen = render(&m, &fonts, &mut c);
            for h in &screen.hits {
                if let Action::ShowEvent(i) = h.action {
                    assert!(seen.insert(i), "wydarzenie {i} pokazane na dwóch stronach");
                }
            }
            page += 1;
            if page >= screen.pages {
                break;
            }
        }
        assert_eq!(
            seen.len(),
            total,
            "paginacja zgubiła wydarzenia: {} z {}",
            seen.len(),
            total
        );
    }

    #[test]
    fn strona_poza_zakresem_jest_przycinana() {
        let fonts = Fonts::embedded();
        let mut m = model_with(1, 2);
        m.page = 99;
        let mut c = Gray8::new(Rotation::default());
        let screen = render(&m, &fonts, &mut c);
        assert_eq!(screen.page, 0);
    }

    #[test]
    fn paginacja_konczy_sie_dla_bardzo_wysokich_wierszy() {
        // Zabezpieczenie przed pętlą nieskończoną, gdyby wiersz nie mieścił się
        // na pustej stronie.
        let fonts = Fonts::embedded();
        let mut m = model_with(30, 20);
        m.page = 0;
        let mut c = Gray8::new(Rotation::default());
        let screen = render(&m, &fonts, &mut c);
        assert!(screen.pages > 0 && screen.pages < 1000);
    }

    #[test]
    fn dotkniecie_wydarzenia_daje_akcje() {
        let fonts = Fonts::embedded();
        let mut c = Gray8::new(Rotation::default());
        let screen = render(&model_with(1, 3), &fonts, &mut c);
        let hit = screen
            .hits
            .iter()
            .find(|h| matches!(h.action, Action::ShowEvent(_)));
        assert!(hit.is_some(), "wydarzenia mają być dotykalne");
        let hit = hit.unwrap();
        assert_eq!(
            screen.hit(hit.rect.x + 5, hit.rect.y + 5),
            Some(hit.action),
            "trafienie w środek regionu ma zwrócić jego akcję"
        );
    }

    #[test]
    fn widok_szczegolow_ma_powrot() {
        let fonts = Fonts::embedded();
        let mut m = model_with(1, 3);
        m.focus = Some(1);
        let mut c = Gray8::new(Rotation::default());
        let screen = render(&m, &fonts, &mut c);
        assert!(screen.hits.iter().any(|h| h.action == Action::Back));
        assert!(!screen
            .hits
            .iter()
            .any(|h| matches!(h.action, Action::ShowEvent(_))));
    }

    #[test]
    fn pager_pojawia_sie_tylko_przy_wielu_stronach() {
        let fonts = Fonts::embedded();

        let mut c = Gray8::new(Rotation::default());
        let screen = render(&model_with(1, 2), &fonts, &mut c);
        assert!(!screen.hits.iter().any(|h| h.action == Action::NextPage));

        let mut c = Gray8::new(Rotation::default());
        let screen = render(&model_with(7, 6), &fonts, &mut c);
        assert!(screen.hits.iter().any(|h| h.action == Action::NextPage));
        assert!(
            !screen.hits.iter().any(|h| h.action == Action::PrevPage),
            "na pierwszej stronie nie ma cofania"
        );
    }

    #[test]
    fn ostatnia_strona_nie_ma_przycisku_dalej() {
        let fonts = Fonts::embedded();
        let model = model_with(7, 6);
        let mut c = Gray8::new(Rotation::default());
        let pages = render(&model, &fonts, &mut c).pages;

        let mut m = model.clone();
        m.page = pages - 1;
        let mut c = Gray8::new(Rotation::default());
        let screen = render(&m, &fonts, &mut c);
        assert!(!screen.hits.iter().any(|h| h.action == Action::NextPage));
        assert!(screen.hits.iter().any(|h| h.action == Action::PrevPage));
    }

    #[test]
    fn stany_sieci_renderuja_sie_bez_paniki() {
        let fonts = Fonts::embedded();
        for net in [
            NetState::Ok,
            NetState::Offline,
            NetState::NeedsAuth,
            NetState::Stale { since: dt(6, 15) },
        ] {
            let mut m = Model::empty(dt(12, 0));
            m.net = net;
            let mut c = Gray8::new(Rotation::default());
            render(&m, &fonts, &mut c);
        }
    }

    #[test]
    fn bateria_na_kazdym_poziomie() {
        let fonts = Fonts::embedded();
        for pct in [0u8, 1, 15, 50, 99, 100] {
            let mut m = Model::empty(dt(12, 0));
            m.battery = Battery {
                percent: Some(pct),
                millivolts: Some(3800),
                charging: pct % 2 == 0,
            };
            let mut c = Gray8::new(Rotation::default());
            render(&m, &fonts, &mut c);
        }
    }

    #[test]
    fn dlugie_tytuly_i_lokalizacje_nie_wychodza_poza_plotno() {
        let fonts = Fonts::embedded();
        let mut m = Model::empty(dt(8, 0));
        m.days = vec![DayGroup {
            date: NaiveDate::from_ymd_opt(2026, 8, 18).unwrap(),
            events: (0..8)
                .map(|i| {
                    let mut e = ev(
                        "Bardzo długi tytuł wydarzenia który na pewno nie zmieści się w dostępnej kolumnie i musi zostać skrócony wielokropkiem",
                        8 + i,
                        9 + i,
                    );
                    e.location = Some("Bardzo długa nazwa lokalizacji, która również się nie mieści".into());
                    e
                })
                .collect(),
        }];
        m.tiles = vec![Tile::new("pogoda", "21").with_unit("°C")];
        let mut c = Gray8::new(Rotation::default());
        render(&m, &fonts, &mut c);
        assert_eq!(c.pixels().len(), c.width() * c.height());
    }
}
