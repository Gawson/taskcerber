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

use crate::canvas::{
    dither_rect, Gray8, Rect, Rotation, BLACK, FILL_DARK, INK_DIM, INK_FAINT, WHITE,
};
use crate::hit::{Action, HitRegion, Screen};
use crate::model::{
    data_dzien_miesiac, dzien_skrot, godzina, naglowek_dnia, za_ile, Battery, CalEvent, Model,
    NetState, SourceTag, View,
};
// `Page` w tym module to strona AGENDY (paginacja). Strona klawiatury
// przychodzi pod aliasem, żeby te dwa pojęcia nie udawały jednego.
use crate::setup::Page as KeyboardPage;
use crate::setup::{Caps, Field, Setup};
use crate::shapes::{
    chevron_left, chevron_right, fill_circle, fill_round_rect, hline, stroke_round_rect, vline,
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
        // `h` to wysokość CIAŁA ekranu, a nie płótna: pas zakładek jest odjęty raz,
        // tutaj, i dzięki temu cała reszta geometrii — stopka, treść, paginacja —
        // nie musi o nim wiedzieć.
        let h = c.height() as i32 - crate::nav::tabs_h(c);

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

// ---------------------------------------------------------------------------
// Typografia na e-papierze: dolna granica jest twarda
// ---------------------------------------------------------------------------
// Panel ma 4.7" i 960×540, czyli ~234 DPI — ale to nie jest gęstość LCD, tylko
// elektroforetyczna zawiesina. Cienki krój w małym rozmiarze nie wychodzi
// „drobny", tylko **poszczerbiony**: część kresek nie dostaje pełnego przebiegu
// i znika.
//
// Stąd trzy reguły, sprawdzone na sztuce:
//
//   1. Nic poniżej 19 px. Cokolwiek mniejszego jest do odczytania wyłącznie z nosem
//      przy szkle i wygląda na uszkodzone.
//   2. Poniżej ~26 px krój ma być `Medium` albo `Bold`, nigdy `Regular` — grubsza
//      kreska nadrabia to, czego panel nie utrzymuje.
//   3. **Tekst wyłącznie w poziomach 0-3**, czyli `BLACK`, `INK_DIM`, `INK_FAINT`.
//      To nie jest preferencja estetyczna, tylko wynik pomiaru z karty tonów: od
//      poziomu 5 w górę litera przestaje być literą, a od 10 nie ma jej wcale.
//      Cała mechanika i krzywa odpowiedzi — przy palecie w `canvas`.
//   4. **Tekst w NEGATYWIE (biel na czerni) zawsze `Bold`**, niezależnie od rozmiaru.
//      `BILEVEL_THRESHOLD` czerni piksel przy pokryciu powyżej 34%, ale bieli dopiero
//      powyżej 66% — w negatywie kreska chudnie dokładnie tak, jak w pozytywie tyje.
//      Zmierzone: „strefa" 22 px Medium ma 46% kresek jednopikselowych, Bold — 3%.
//
// Konsekwencja, z której trzeba sobie zdawać sprawę projektując: **ton jest tu
// kanałem o czterech stopniach, nie o szesnastu.** Hierarchię niosą rozmiar
// i grubość kroju; jasne wypełnienia robi `dither_rect`, nie szarość.
// ---------------------------------------------------------------------------
// Drabina rozmiarów
// ---------------------------------------------------------------------------
//
// Sześć stopni na oba ekrany. Wcześniej w tym pliku było **piętnaście** różnych
// rozmiarów, przy czym cztery poziomy hierarchii mieściły się w rozpiętości dwóch
// pikseli (25 / 26 / 26 / 27). Na e-papierze różnica dwóch pikseli jest niewidoczna,
// więc płaciło się złożonością za hierarchię, której nie widać.
//
// Stosunek między stopniami trzyma się ~1,25-1,3, czyli tyle, ile trzeba, żeby skok
// był widoczny z odległości, z jakiej patrzy się na kalendarz na ścianie.
//
// Orientacja: rozmiar zależny od `compact` zostaje TYLKO tam, gdzie poziomy pas
// naprawdę nie mieści stopnia z pionu — i wtedy obowiązuje jedna zasada,
// „w kompakcie jeden stopień niżej". Pozostałe warunki zniknęły.

/// Numer dnia. Jedyny element czytelny z drugiego końca pokoju.
pub(crate) const TEXT_HERO: f32 = 76.0;

/// To, po co się na ten ekran patrzy: tytuł w szczegółach, stan pusty, wpisywana
/// wartość w pionie.
pub(crate) const TEXT_TITLE: f32 = 44.0;

/// Nagłówek ekranu i przycisk główny: dzień tygodnia, „Konfiguracja", „Skonfiguruj",
/// klawisze znakowe.
pub(crate) const TEXT_HEAD: f32 = 34.0;

/// Treść pierwszoplanowa: tytuł wydarzenia, godzina startu, nagłówek dnia,
/// etykieta pola.
pub(crate) const TEXT_LEAD: f32 = 27.0;

/// Drugi plan, czytany z ręki: lokalizacja, jednostka, zakładki, podpowiedź,
/// lista „zapamiętane", plakietki, pasek klawiatury.
pub(crate) const TEXT_BODY: f32 = 22.0;

/// **Nie jest stopniem projektowym**, tylko podłogą reguły 1.
///
/// Wolno go użyć wyłącznie tam, gdzie tekst ma ISTNIEĆ, a nie być czytany, i gdzie
/// [`TEXT_BODY`] zmierzalnie nie wchodzi. Dwa takie miejsca: wersja firmware'u
/// w rogu i nagłówek listy „zapamiętane" — ten drugi ma w pionie **1 px** zapasu
/// (`HEAD_H` 24 + 6 × 28 = 192 przy 193 dostępnych), więc wyższy nagłówek to
/// zniknięcie całej listy.
pub(crate) const TEXT_FLOOR: f32 = 19.0;

// Pasek statusu w nagłówku: rozmiar pisma, odsunięcie od krawędzi i pozycja pigułki.
const STATUS_SIZE: f32 = TEXT_BODY;
const STATUS_PAD: i32 = 10;
const STATUS_PILL_TOP: i32 = 66;
const STATUS_BASELINE: f32 = 90.0;

/// Rozmiar wartości na kafelku i podłoga, poniżej której wolimy uciąć niż zmniejszać.
const TILE_VALUE_SIZE: f32 = 32.0;
const TILE_VALUE_MIN_SIZE: f32 = 22.0;

/// Jednostka przy wartości kafelka („°C", „%").
///
/// Sztywna, bo skalowanie od rozmiaru wartości (`size * 0.62`) łamało reguły
/// typografii z nagłówka tego pliku. Przy pełnej wartości dawało 19,8 px **Regular**
/// — reguła 2 (poniżej 26 px nigdy Regular) łamana na KAŻDYM kafelku z jednostką.
/// Gdy wartość zwężała się do [`TILE_VALUE_MIN_SIZE`], schodziło do 13,6 px, czyli
/// dodatkowo reguła 1. Podłoga `.max(12.0)` była martwa: 22 × 0,62 = 13,64 > 12.
///
/// Zmierzone przez realny potok (`quantize_ink` + `simulate_panel`): 20 px Medium
/// daje o 13-17% więcej widocznego atramentu niż 19,8 px Regular, a szerokość „°C"
/// rośnie o 0,02 px. Zysk jest darmowy, bo miejsce i tak było zarezerwowane.
///
/// **Rezerwacja miejsca niżej MUSI liczyć tymi samymi stałymi.** Gdy liczyła
/// `20.0, Regular`, a rysunek szedł Medium, najszersza jednostka („jednostek"
/// ze scenariusza stresowego) wychodziła 1,6 px poza kolumnę kafelka.
const TILE_UNIT_SIZE: f32 = TEXT_BODY;
const TILE_UNIT_WEIGHT: Weight = Weight::Medium;

const DAY_HEADER_H: i32 = 40;
const EVENT_H: i32 = 50;
const EVENT_H_WITH_LOCATION: i32 = 62;
const DAY_GAP: i32 = 14;

/// Rysuje pełny dashboard i zwraca obszary dotykowe.
/// Rysuje ekran główny — ten widok, który jest w [`Model::view`], plus pasek zakładek.
///
/// To jedyny punkt, w którym wybiera się widok. Firmware woła wyłącznie tę funkcję
/// i nie wie, że istnieje więcej niż jeden ekran; `preview` i symulator też.
pub fn render(model: &Model, fonts: &Fonts, c: &mut Gray8) -> Screen {
    let mut screen = match model.view {
        View::Agenda => render_agenda_screen(model, fonts, c),
        View::Month => crate::month::render_month(model, fonts, c),
        View::Year => crate::year::render_year(model, fonts, c),
    };

    crate::nav::draw_tabs(model.view, fonts, c, &mut screen);

    // Kwantyzujemy SAM PAS, bo widok zrobił już swoją treść i `ink_level`
    // **nie jest idempotentna**: 0x22 wychodzi jako 0x11, a 0x11 jako 0x00.
    // Drugi przebieg po całym płótnie zwęgliłby wszystkie tony do czerni —
    // wyszłoby to dopiero na szkle, bo w podglądzie różnica poziomu 1 od 0
    // jest ledwo widoczna.
    let pas = Rect::new(
        0,
        c.height() as i32 - crate::nav::tabs_h(c),
        c.width() as i32,
        crate::nav::tabs_h(c),
    );
    c.quantize_ink_rect(pas);

    screen
}

fn render_agenda_screen(model: &Model, fonts: &Fonts, c: &mut Gray8) -> Screen {
    c.clear(WHITE);

    let mut screen = Screen::default();

    draw_header(model, fonts, c, &mut screen);
    draw_agenda(model, fonts, c, &mut screen);
    draw_footer(model, fonts, c, &mut screen);

    // Agenda jest DWUPOZIOMOWA, tak jak ekran konfiguracji — i to jest zmiana
    // podyktowana pomiarem, nie estetyką.
    //
    // Zmierzone na zrzucie przepuszczonym przez `Gray8::simulate_panel`: **31%
    // atramentu tego ekranu lądowało w poziomach, których panel prawie nie pokazuje**
    // — 20,5% ledwo widocznych, 10,7% niewidocznych w ogóle. Prawie w całości był to
    // antyaliasing krawędzi liter, bo `Fonts::draw` blenduje pokryciem.
    //
    // Na zwykłym wyświetlaczu ta obwódka nadaje kresce wagę. Tutaj po prostu znika,
    // więc litera traci jedną trzecią atramentu i wygląda na cienką i wypłowiałą —
    // dokładnie tak, jak to zgłoszono ze sprzętu. Po kwantyzacji czytelne jest 100%
    // atramentu, a jego ilość spada tylko o 2%, bo próg `BILEVEL_THRESHOLD` jest
    // przesunięty w stronę czerni i większość obwódki zostaje czernią zamiast zniknąć.
    // Ale samo progowanie do dwóch poziomów zabiera krawędziom gładkość, a przy
    // małych rozmiarach to ONA nadaje literze kształt — też zgłoszone ze sprzętu.
    // Dlatego nie dwa poziomy, tylko PIĘĆ: tyle, ile panel realnie ma na atrament.
    // Pełne wyjaśnienie przy [`Gray8::quantize_ink`].
    //
    // Agenda może sobie na to pozwolić, bo jest odświeżana przez GC16. Klawiatura nie
    // może i zostaje przy `quantize2`, bo każde naciśnięcie klawisza idzie przez DU.
    c.quantize_ink();

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
        TEXT_HERO,
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
        TEXT_HEAD,
        Weight::Bold,
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
        TEXT_LEAD,
        Weight::Medium,
        INK_DIM,
        Align::Left,
    );

    draw_battery(&model.battery, fonts, c, g.w - g.margin - 92, 30);

    let (status_text, status_ink) = match model.net {
        NetState::Ok => (format!("zaktualizowano {}", godzina(model.now)), INK_DIM),
        // „Dane z", nie „nieaktualne od". Ten stan pojawia się w sytuacji zupełnie
        // NORMALNEJ — przy wybudzeniu, w którym pobranie nie było należne, bo treść
        // ma minutę. Słowo „nieaktualne" sugerowało wtedy usterkę tam, gdzie jej nie
        // ma, a pełny atrament dokładał alarmu. Wiek treści niesie sama godzina;
        // ocenę zostawiamy patrzącemu.
        NetState::Stale { since } => (format!("dane z {}", godzina(since)), INK_DIM),
        NetState::Offline => ("brak sieci".to_string(), BLACK),
        NetState::NeedsAuth => ("skonfiguruj urządzenie".to_string(), BLACK),
        // Pełnym atramentem, bo to komunikat „poczekaj", a nie tło. Wiek treści
        // dopisujemy tylko wtedy, gdy go znamy — wcześniejsza wersja brała go
        // z pamięci RTC, która nie przeżywa restartu, i wypisywała epokę: „z 01:00".
        NetState::Fetching { since: Some(t) } => {
            (format!("pobieram… · treść z {}", godzina(t)), BLACK)
        }
        NetState::Fetching { since: None } => ("pobieram…".to_string(), BLACK),
    };
    // Status nie ma prawa wejść ani na blok z datą po lewej, ani na baterię nad sobą.
    // Przy 540 px szerokości jedno i drugie było o włos.
    let status_text = fonts.truncate(
        &status_text,
        (g.w - 2 * g.margin) as f32 * 0.56,
        STATUS_SIZE,
        Weight::Medium,
    );
    let status_w = fonts.draw(
        c,
        &status_text,
        (g.w - g.margin - STATUS_PAD) as f32,
        STATUS_BASELINE,
        STATUS_SIZE,
        Weight::Medium,
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

    // Dotknięcie paska statusu wymusza odświeżenie — z jednym wyjątkiem.
    // Na nieskonfigurowanym urządzeniu plakietka mówi „skonfiguruj urządzenie",
    // więc jedyne sensowne, co może zrobić, to otworzyć konfigurację. Odświeżenie
    // i tak nie ma czego pobrać, bo nie ma ani sieci, ani adresu kalendarza.
    let pill_action = if matches!(model.net, NetState::NeedsAuth) {
        Action::OpenSetup
    } else {
        Action::RefreshNow
    };
    // Cel dotykowy jest większy od plakietki, ale mignąć ma sama plakietka — patrz
    // [`Visual`]. Promień 8 zgadza się ze `stroke_round_rect` wyżej.
    screen.hits.push(HitRegion::shaped(
        Rect::new(pill.x - 10, pill.y - 6, pill.w + 20, pill.h + 12),
        pill_action,
        pill,
        8,
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
            let ink = if pct <= 15 { BLACK } else { FILL_DARK };
            fill_round_rect(c, Rect::new(inner.x, inner.y, w, inner.h), 2.0, ink);
        }
        fonts.draw(
            c,
            &format!("{pct}%"),
            (x - 10) as f32,
            (y + 21) as f32,
            TEXT_BODY,
            Weight::Medium,
            BLACK,
            Align::Right,
        );
    }

    if b.charging {
        draw_bolt(c, x + 22, y + 4);
    }
}

/// Błyskawica ładowania: wypełniona bryła z białą otoczką.
///
/// # Dlaczego nie kreski
///
/// Poprzednia wersja składała ją z pojedynczych pikseli — dwie ukośne kreski po
/// **2 px szerokości**. Na 234 DPI kreska 2 px nie jest kształtem, tylko paprochem,
/// i dokładnie tak została zgłoszona ze sprzętu. To ta sama zasada, przez którą
/// paski gęstości w widoku miesięcznym mają 3 px pełnego atramentu zamiast kropek.
///
/// # Dlaczego otoczka
///
/// Błyskawica leży NA WYPEŁNIENIU baterii, którego ton zależy od stanu naładowania
/// ([`FILL_DARK`], a poniżej 15% [`BLACK`]) — więc czarny kształt na czarnym tle
/// znika. Biała otoczka o grubości 1 px odcina go od każdego tła, tak samo jak
/// biała podkładka pod numerem dnia w `crate::month`.
fn draw_bolt(c: &mut Gray8, x: i32, y: i32) {
    // Wiersz po wierszu: górny klin schodzi w dół-lewo, dolny jest wobec niego
    // przesunięty w prawo i schodzi tak samo. To przesunięcie JEST błyskawicą —
    // bez niego wychodzi zwykły ukośny pasek.
    const H: i32 = 18;
    let biegi: [(i32, i32); H as usize] = {
        let mut t = [(0, 0); H as usize];
        let mut i = 0;
        while i < H as usize {
            let dy = i as i32;
            t[i] = if dy < 9 {
                // Górny klin: lewa krawędź pełznie w lewo, szerokość stała.
                (7 - dy * 5 / 9, 5)
            } else {
                // Dolny klin zaczyna się o 3 px w prawo od końca górnego.
                (8 - (dy - 9) * 5 / 9, 5)
            };
            i += 1;
        }
        t
    };

    // Najpierw otoczka: ten sam kształt rozdmuchany o piksel w każdą stronę.
    for (dy, (bx, bw)) in biegi.iter().enumerate() {
        c.fill_rect(Rect::new(x + bx - 1, y + dy as i32 - 1, bw + 2, 3), WHITE);
    }
    for (dy, (bx, bw)) in biegi.iter().enumerate() {
        c.fill_rect(Rect::new(x + bx, y + dy as i32, *bw, 1), BLACK);
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
        // Na urządzeniu bez konfiguracji pusty kalendarz nie jest informacją —
        // informacją jest to, że nie ma czego pokazać, dopóki ktoś nie wpisze sieci.
        if matches!(model.net, NetState::NeedsAuth) {
            draw_setup_cta(fonts, c, screen);
        } else {
            draw_empty_state(fonts, c);
        }
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
        TEXT_LEAD,
        Weight::Bold,
        BLACK,
        Align::Left,
    );
    let label_w = fonts.measure(&label, TEXT_LEAD, Weight::Bold);

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
        TEXT_BODY,
        Weight::Medium,
        BLACK,
        Align::Left,
    );

    let sub_w = fonts.measure(&sub, TEXT_BODY, Weight::Medium);
    let line_x = g.margin + (label_w + sub_w) as i32 + 30;
    let line_w = g.w - g.margin - line_x;
    if line_w > 20 {
        hline(c, line_x, y + 14, line_w, 1, INK_DIM);
    }

    y + DAY_HEADER_H
}

fn draw_event(event: &CalEvent, model: &Model, fonts: &Fonts, c: &mut Gray8, y: i32, h: i32) {
    let g = Geom::of(c);
    let now = model.now;
    let past = event.is_past(now);
    let live = event.is_now(now);

    let title_ink = if past { INK_FAINT } else { BLACK };
    let time_ink = if past { INK_FAINT } else { BLACK };

    if live {
        // Ditherem, nie półtonem. Poprzednie `0xF2` to poziom 15 — na szkle
        // czysta biel, czyli podświetlenie „dzieje się teraz" nie istniało.
        dither_rect(
            c,
            Rect::new(g.margin - 10, y - 4, g.w - 2 * g.margin + 20, h - 2),
            2,
        );
        fill_round_rect(c, Rect::new(g.margin - 10, y - 4, 5, h - 2), 2.5, BLACK);
    }

    let baseline = (y + 26) as f32;

    let time_text = if event.all_day {
        "cały dzień".to_string()
    } else {
        godzina(event.start)
    };
    let time_size = if event.all_day { TEXT_BODY } else { TEXT_LEAD };
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
            TEXT_BODY,
            Weight::Medium,
            INK_DIM,
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
            BLACK,
        ),
    }

    let text_x = g.margin + g.time_col_w;
    let mut avail = g.w - g.margin - text_x;

    if !past && !live && is_first_upcoming(model, event) {
        let badge = za_ile(event.start, now);
        // Bold, bo to NEGATYW — patrz reguła 4 w nagłówku pliku. Zmierzone:
        // przy Medium 52% kresek tej plakietki było jednopikselowych, przy Bold 3%.
        let bw = fonts.measure(&badge, TEXT_BODY, Weight::Bold) as i32 + 24;
        let br = Rect::new(g.w - g.margin - bw, y + 2, bw, 28);
        fill_round_rect(c, br, 14.0, BLACK);
        fonts.draw(
            c,
            &badge,
            (br.x + br.w / 2) as f32,
            (br.y + 20) as f32,
            TEXT_BODY,
            Weight::Bold,
            WHITE,
            Align::Center,
        );
        avail -= bw + 16;
    }

    let title = fonts.truncate(&event.title, avail as f32, TEXT_LEAD, Weight::Medium);
    fonts.draw(
        c,
        &title,
        text_x as f32,
        baseline,
        TEXT_LEAD,
        Weight::Medium,
        title_ink,
        Align::Left,
    );

    if let Some(loc) = &event.location {
        let loc = fonts.truncate(loc, avail as f32, TEXT_BODY, Weight::Medium);
        fonts.draw(
            c,
            &loc,
            text_x as f32,
            baseline + 22.0,
            TEXT_BODY,
            Weight::Medium,
            INK_DIM,
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

/// Szerokość i wysokość przycisku wejścia w konfigurację.
///
/// 96 px to na tym panelu ponad 10 mm — cel dla palca, nie dla kursora.
const SETUP_BUTTON_W: i32 = 420;
const SETUP_BUTTON_H: i32 = 96;

/// Przycisk „Skonfiguruj" na urządzeniu, które nie ma jeszcze konfiguracji.
///
/// Kafelek w stopce mówi „krok 1 — dotknij ekranu", więc na ekranie musi być coś,
/// co wygląda jak przycisk i nim jest. Wcześniej jedynymi wejściami w konfigurację
/// były dwie plakietki po 44 px przy samych krawędziach — razem **3% powierzchni** —
/// więc człowiek, który przeczytał instrukcję i stuknął w środek ekranu, wyciągał
/// wniosek, że dotyk jest zepsuty. Napis na ekranie ma być prawdą.
fn draw_setup_cta(fonts: &Fonts, c: &mut Gray8, screen: &mut Screen) {
    let g = Geom::of(c);
    let w = SETUP_BUTTON_W.min(g.w - 2 * g.margin);
    let h = SETUP_BUTTON_H;
    let rect = Rect::new(
        (g.w - w) / 2,
        (g.content_top + g.content_bottom) / 2 - h / 2,
        w,
        h,
    );

    stroke_round_rect(c, rect, 14.0, 3, BLACK);
    fonts.draw(
        c,
        "Skonfiguruj",
        (rect.x + rect.w / 2) as f32,
        (rect.y + rect.h / 2 + 12) as f32,
        TEXT_HEAD,
        Weight::Bold,
        BLACK,
        Align::Center,
    );
    fonts.draw(
        c,
        "sieć i adres kalendarza",
        (rect.x + rect.w / 2) as f32,
        (rect.bottom() + 40) as f32,
        TEXT_LEAD,
        Weight::Medium,
        INK_DIM,
        Align::Center,
    );

    // Promień 14 zgadza się ze `stroke_round_rect` wyżej — bez tego przycisk zapalał
    // się pod palcem jako ostry prostokąt.
    screen
        .hits
        .push(HitRegion::shaped(rect, Action::OpenSetup, rect, 14));
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
        TEXT_TITLE,
        Weight::Bold,
        INK_DIM,
        Align::Center,
    );
    fonts.draw(
        c,
        "Wolne do końca widocznego okresu",
        cx,
        cy + 36.0,
        TEXT_LEAD,
        Weight::Medium,
        INK_DIM,
        Align::Center,
    );
}

/// Ekran potwierdzenia po zapisaniu konfiguracji.
///
/// # Po co osobny ekran
///
/// Po naciśnięciu „zapisz" urządzenie ma już komplet danych, ale jeszcze ŻADNEJ
/// treści — zasypia na kilka sekund i dopiero po przebudzeniu sięga po kalendarz.
/// Nie da się w tym momencie pokazać ani agendy (byłoby „Nic w planie", czyli
/// nieprawda), ani ekranu startowego (wyglądałby, jakby zapis nie przeszedł).
///
/// Wcześniej nie pokazywano nic: `setup_screen` wracał, a wołający wypychał tylko
/// znacznik snu — kwadracik 22 px w rogu. Na szkle zostawała ta sama klawiatura,
/// więc **naciśnięcie „zapisz" było wizualnie nieodróżnialne od braku reakcji**.
pub fn render_saved(fonts: &Fonts, c: &mut Gray8) -> Screen {
    c.clear(WHITE);
    let g = Geom::of(c);
    let cx = g.w as f32 / 2.0;
    let cy = ((g.content_top + g.content_bottom) / 2) as f32;

    fonts.draw(
        c,
        "Zapisano",
        cx,
        cy,
        TEXT_TITLE,
        Weight::Bold,
        BLACK,
        Align::Center,
    );
    // Dwie linie, nie jedna: w pionie zostaje 468 px szerokości, a cały komunikat
    // jednym ciągiem wymagałby zejścia poniżej `TEXT_LEAD`.
    for (i, line) in ["pobieram kalendarz", "przy najbliższym połączeniu"]
        .iter()
        .enumerate()
    {
        fonts.draw(
            c,
            line,
            cx,
            cy + 44.0 + i as f32 * 34.0,
            TEXT_LEAD,
            Weight::Medium,
            INK_DIM,
            Align::Center,
        );
    }

    c.quantize_ink();
    Screen::default()
}

/// Wpisuje jeden krok śladu sieciowego w JEDEN wiersz.
///
/// Bez czyszczenia tła: `back_fb` epdiy jest po wybudzeniu biały, więc na panel
/// pójdą wyłącznie czarne piksele napisu, a reszta nie dostanie impulsu. Napis
/// nadpisuje to, co akurat jest na szkle — świadoma cena za to, żeby narzędzie
/// diagnostyczne nie obciążało zasilania w chwili, w której radio jest na antenie.
/// Pełne uzasadnienie przy `NetTrace` w firmwarze.
pub fn draw_net_step(fonts: &Fonts, c: &mut Gray8, row: Rect, co: &str, ms: u128) {
    let g = Geom::of(c);
    // `TEXT_HEAD`, nie `TEXT_BODY`: to jest ekran diagnostyczny czytany z dystansu,
    // w dodatku nadpisany na tym, co zostało z poprzedniej klatki. Drugoplanowy
    // rozmiar był tu nie do odczytania i tak to zgłoszono ze sprzętu.
    let czas = format!("{ms} ms");
    let czas_w = fonts.measure(&czas, TEXT_LEAD, Weight::Medium);

    let dostepne = (g.w - 2 * g.margin) as f32 - czas_w - 14.0;
    let co = fonts.truncate(co, dostepne, TEXT_HEAD, Weight::Bold);

    let baseline = (row.y + 34) as f32;
    fonts.draw(
        c,
        &co,
        g.margin as f32,
        baseline,
        TEXT_HEAD,
        Weight::Bold,
        BLACK,
        Align::Left,
    );
    fonts.draw(
        c,
        &czas,
        (g.w - g.margin) as f32,
        baseline,
        TEXT_LEAD,
        Weight::Medium,
        BLACK,
        Align::Right,
    );
    c.quantize_ink_rect(row);
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
        TEXT_LEAD,
        Weight::Medium,
        INK_DIM,
        Align::Left,
    );
    y += 46;

    // Tytuł łamany na maksymalnie trzy linie.
    let avail = (g.w - 2 * g.margin) as f32;
    // Trzy linie mieszczą się tylko w pionie. W poziomie przy tytule na trzy linie
    // i lokalizacji na dwie czas trwania lądował na linii bazowej 430 — czyli
    // W ŚRODKU przycisku „Wróć" (`Rect(32, 388, 180, 44)`). To jest naprawa
    // istniejącego błędu, nie koszt tej zmiany.
    let title_lines = match c.rotation() {
        Rotation::Portrait => 3,
        Rotation::Landscape => 2,
    };
    for line in fonts.wrap(&event.title, avail, TEXT_TITLE, Weight::Bold, title_lines) {
        fonts.draw(
            c,
            &line,
            x,
            y as f32,
            TEXT_TITLE,
            Weight::Bold,
            BLACK,
            Align::Left,
        );
        y += 52;
    }

    if let Some(loc) = &event.location {
        y += 10;
        // Medium, nie Regular: 26 px to sama granica reguły 2, a `INK_DIM` dodatkowo
        // odbiera kresce atrament. Rozmiar bez zmian — sam krój.
        for line in fonts.wrap(loc, avail, TEXT_LEAD, Weight::Medium, 2) {
            fonts.draw(
                c,
                &line,
                x,
                y as f32,
                TEXT_LEAD,
                Weight::Medium,
                INK_DIM,
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
            TEXT_BODY,
            Weight::Medium,
            INK_DIM,
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
        TEXT_BODY,
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
    hline(c, g.margin, top, g.w - 2 * g.margin, 1, INK_DIM);

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
        let w = fonts.draw(
            c,
            &model.firmware,
            g.margin as f32,
            (g.h - 8) as f32,
            TEXT_FLOOR,
            Weight::Medium,
            INK_DIM,
            Align::Left,
        );

        // Wejście do konfiguracji na urządzeniu, które JEST już skonfigurowane —
        // inaczej zmiana hasła WiFi wymagałaby kasowania NVS przez przeflashowanie.
        // Obszar jest znacznie wyższy niż napis, bo piętnastopikselowy tekst nie
        // jest celem dla palca; sam napis zostaje dyskretny, żeby nie wyglądał
        // jak przycisk do przypadkowego naciśnięcia.
        screen.hits.push(HitRegion::new(
            Rect::new(g.margin - 10, g.h - 46, w as i32 + 20, 44),
            Action::OpenSetup,
        ));
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
            fill_circle(c, cx, cy, 5.0, BLACK);
            fill_circle(c, cx, cy, 3.0, WHITE);
        }
    }

    let label = format!("{} z {}", screen.page + 1, screen.pages);
    fonts.draw(
        c,
        &label,
        g.w as f32 / 2.0,
        (top + g.footer_h - 14) as f32,
        TEXT_BODY,
        Weight::Medium,
        INK_DIM,
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

        let label = fonts.truncate(
            &tile.label.to_uppercase(),
            tw as f32,
            TEXT_BODY,
            Weight::Medium,
        );
        fonts.draw(
            c,
            &label,
            x as f32,
            (top + 30) as f32,
            TEXT_BODY,
            Weight::Medium,
            INK_DIM,
            Align::Left,
        );

        // Wartość kafelka bywa liczbą („21"), ale bywa i zdaniem („podłącz USB") —
        // tak wygląda ekran konfiguracji. Przy trzech kafelkach na 540 px kolumna ma
        // ~148 px, a „otwórz stronę" w 32 pt zajmuje 200 i wchodziła na sąsiada.
        // Najpierw więc schodzimy z rozmiarem, a dopiero gdy to nie starcza — ucinamy.
        let unit_w = tile.unit.as_ref().map_or(0.0, |u| {
            fonts.measure(u, TILE_UNIT_SIZE, TILE_UNIT_WEIGHT) + 6.0
        });
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
                TILE_UNIT_SIZE,
                TILE_UNIT_WEIGHT,
                INK_DIM,
                Align::Left,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Ekran konfiguracji
// ---------------------------------------------------------------------------

/// Klawiatura jest dosunięta do dolnej krawędzi i rośnie w górę.
///
/// Strony mają różną liczbę wierszy (litery 3, symbole 4), więc kotwiczenie od góry
/// przesuwałoby cały pasek przy przełączeniu strony — a to na e-papierze jest
/// przemalowaniem połowy ekranu zamiast jednego wiersza.
const KB_MARGIN: i32 = 16;
const KB_GAP: i32 = 6;

/// Ile najwyżej wysokości ekranu wolno zająć klawiaturze.
const KB_MAX_HEIGHT_PCT: i32 = 42;

/// Klawisz niższy niż to jest nietrafialny palcem, wyższy — marnuje ekran.
/// Przy 234 DPI panelu 44 px to niecałe 5 mm, czyli tyle, co klawisz w telefonie.
const KEY_H_MIN: i32 = 44;
const KEY_H_MAX: i32 = 78;

/// Wymiary ekranu konfiguracji, policzone dla konkretnego płótna.
struct SetupGeom {
    w: i32,
    gap: i32,
    /// Szerokość klawisza w wierszu dziesięcioklawiszowym. Węższe wiersze centrujemy.
    unit: i32,
    key_h: i32,
    kb_top: i32,
    /// W poziomie nad klawiaturą zostaje ~280 px i blok treści musi się ścisnąć.
    compact: bool,
    content_margin: i32,
    title_base: f32,
    rule_y: i32,
    tabs_y: i32,
    tab_h: i32,
    label_base: f32,
    box_y: i32,
    box_h: i32,
    hint_base: f32,
    /// Pas na listę pól. Dolna granica jest liczona dla NAJWYŻSZEJ klawiatury,
    /// a nie dla obecnie pokazanej — patrz komentarz przy jej wyliczeniu.
    summary_top: i32,
    summary_bottom: i32,
}

impl SetupGeom {
    fn of(c: &Gray8, setup: &Setup) -> Self {
        let w = c.width() as i32;
        let h = c.height() as i32;
        let rows = setup.page().rows().len() as i32 + 1; // wiersze znakowe + pasek

        let key_h = ((h * KB_MAX_HEIGHT_PCT / 100 - (rows - 1) * KB_GAP) / rows)
            .clamp(KEY_H_MIN, KEY_H_MAX);
        let kb_h = rows * key_h + (rows - 1) * KB_GAP;
        let kb_top = h - KB_MARGIN - kb_h;

        let compact = kb_top < 320;

        let title_base = if compact { 38.0 } else { 56.0 };
        let rule_y = if compact { 52 } else { 78 };
        let tabs_y = rule_y + if compact { 12 } else { 22 };
        let tab_h = if compact { 44 } else { 56 };
        let label_base = (tabs_y + tab_h) as f32 + if compact { 26.0 } else { 36.0 };
        let box_y = label_base as i32 + if compact { 10 } else { 14 };
        // Pole wartości jest tym, co się czyta w trakcie wpisywania, i jako jedyne
        // musi być czytelne z odległości ręki trzymającej urządzenie. Reszta ekranu
        // może być drobna, to nie.
        let box_h = if compact { 64 } else { 86 };
        let hint_base = (box_y + box_h) as f32 + if compact { 24.0 } else { 32.0 };

        // Dolna granica listy pól NIE zależy od tego, która strona klawiatury jest
        // pokazana. Gdyby zależała, przełączenie na symbole — o wiersz wyższe —
        // kazałoby liście zniknąć, a na e-papierze zniknięcie bloku to przemalowanie
        // ćwiartki ekranu i wygląda jak usterka, nie jak zmiana strony.
        let max_rows = KeyboardPage::Letters
            .rows()
            .len()
            .max(KeyboardPage::Symbols.rows().len()) as i32
            + 1;
        let max_key_h = ((h * KB_MAX_HEIGHT_PCT / 100 - (max_rows - 1) * KB_GAP) / max_rows)
            .clamp(KEY_H_MIN, KEY_H_MAX);
        // 12 px odstępu, żeby ostatni wiersz listy nie dotykał klawiszy.
        let summary_bottom = h - KB_MARGIN - (max_rows * max_key_h + (max_rows - 1) * KB_GAP) - 12;

        Self {
            w,
            gap: KB_GAP,
            unit: (w - 2 * KB_MARGIN - 9 * KB_GAP) / 10,
            key_h,
            kb_top,
            compact,
            content_margin: if compact { 20 } else { 28 },
            title_base,
            rule_y,
            tabs_y,
            tab_h,
            label_base,
            box_y,
            box_h,
            hint_base,
            summary_top: hint_base as i32 + 16,
            summary_bottom,
        }
    }

    /// Szerokość klawisza obejmującego `units` pozycji siatki wraz z przerwami,
    /// które połyka. Dzięki temu pasek o sumie 10 jednostek ma dokładnie tę samą
    /// szerokość co wiersz dziesięciu klawiszy.
    fn span(&self, units: f32) -> i32 {
        (units * self.unit as f32 + (units - 1.0) * self.gap as f32).round() as i32
    }
}

/// Rysuje ekran konfiguracji i zwraca obszary dotykowe.
///
/// To jedyna droga wprowadzania danych do urządzenia — patrz [`crate::setup`].
pub fn render_setup(setup: &Setup, fonts: &Fonts, c: &mut Gray8) -> Screen {
    render_setup_pressed(setup, fonts, c, None)
}

/// To samo, ale z jednym klawiszem narysowanym **w negatywie**.
///
/// Wciśnięty klawisz nie jest zasłaniany czarnym prostokątem, tylko rysowany tym
/// samym stylem, co `⇧` w blokadzie: wypełnienie czarne, napis biały. Znak zostaje
/// widoczny pod palcem, a to jest cała różnica między „coś mignęło" a „widzę, który
/// klawisz nacisnąłem".
///
/// Klawisz, który i tak jest wypełniony (gotowy „zapisz", `⇧` w blokadzie), dostaje
/// zamiast tego grubą ramkę — inaczej naciśnięcie nie zmieniałoby niczego.
pub fn render_setup_pressed(
    setup: &Setup,
    fonts: &Fonts,
    c: &mut Gray8,
    pressed: Option<Rect>,
) -> Screen {
    c.clear(WHITE);

    let mut screen = Screen::default();
    let g = SetupGeom::of(c, setup);

    draw_setup_head(setup, fonts, c, &mut screen, &g);
    draw_setup_summary(setup, fonts, c, &mut screen, &g);
    draw_keyboard(setup, fonts, c, &mut screen, &g, pressed);

    // Ekran konfiguracji jest DWUPOZIOMOWY, bo każde naciśnięcie klawisza odświeża
    // go przez `MODE_DU`, a ten szarości nie umie. Pełne wyjaśnienie przy
    // [`Gray8::quantize2`]. Kwantyzacja jest tutaj, a nie w sterowniku panelu,
    // żeby symulator i zrzuty PNG pokazywały dokładnie to, co wyjdzie na szkle.
    c.quantize2();

    screen
}

/// Styl klawisza po uwzględnieniu tego, czy akurat jest pod palcem.
fn pressed_style(base: KeyStyle, rect: Rect, pressed: Option<Rect>) -> KeyStyle {
    if pressed != Some(rect) {
        return base;
    }
    match base {
        KeyStyle::Filled => KeyStyle::Marked,
        _ => KeyStyle::Filled,
    }
}

fn draw_setup_head(
    setup: &Setup,
    fonts: &Fonts,
    c: &mut Gray8,
    screen: &mut Screen,
    g: &SetupGeom,
) {
    let m = g.content_margin;

    fonts.draw(
        c,
        "Konfiguracja",
        m as f32,
        g.title_base,
        // Zostaje warunkowy: w poziomie nagłówek dzieli pas z zakładkami.
        if g.compact { TEXT_LEAD } else { TEXT_HEAD },
        Weight::Bold,
        BLACK,
        Align::Left,
    );

    // Po prawej stronie tytułu: czego jeszcze brakuje. To jedyne miejsce, w którym
    // widać postęp — zakładek jest sześć i cztery z nich są opcjonalne.
    let status_size = TEXT_BODY;
    let (status, ink) = match setup.first_missing() {
        Some(field) => (format!("brakuje: {}", field.tab()), BLACK),
        None => ("komplet — naciśnij zapisz".to_string(), INK_DIM),
    };
    fonts.draw(
        c,
        &status,
        (g.w - m) as f32,
        g.title_base,
        status_size,
        Weight::Medium,
        ink,
        Align::Right,
    );

    hline(c, m, g.rule_y, g.w - 2 * m, 2, BLACK);

    // --- zakładki pól ------------------------------------------------------
    let count = Field::ALL.len() as i32;
    let tab_gap = 8;
    let tab_w = (g.w - 2 * m - (count - 1) * tab_gap) / count;
    let tab_size = TEXT_BODY;

    for (i, field) in Field::ALL.into_iter().enumerate() {
        let r = Rect::new(m + i as i32 * (tab_w + tab_gap), g.tabs_y, tab_w, g.tab_h);
        let active = field == setup.field();

        if active {
            fill_round_rect(c, r, 10.0, BLACK);
        } else {
            stroke_round_rect(c, r, 10.0, 2, BLACK);
        }

        // Kropka przy polach wymaganych, które są jeszcze puste. Bez niej nie widać,
        // że „iCal" to nie jest opcja, dopóki nie kliknie się w zakładkę.
        let label = if field.required() && setup.value(field).is_empty() {
            format!("• {}", field.tab())
        } else {
            field.tab().to_string()
        };
        // Aktywna zakładka jest w NEGATYWIE, więc Bold — reguła 4. Dziś miała
        // najmniej atramentu ze wszystkich sześciu, czyli hierarchia była odwrócona:
        // 46% kresek jednopikselowych przy Medium, 3% przy Bold.
        //
        // Warunkowo, nie na wszystkich: efekt ma być RÓŻNICOWY. Gdyby nieaktywne
        // rosły razem z aktywną, różnica by zniknęła.
        let tab_weight = if active { Weight::Bold } else { Weight::Medium };
        // Ten sam krój do mierzenia i do rysowania — rozjazd tej pary to dokładnie
        // ta klasa błędu, którą opisuje komentarz przy `TILE_UNIT_SIZE`.
        let label = fonts.truncate(&label, (tab_w - 12) as f32, tab_size, tab_weight);

        fonts.draw(
            c,
            &label,
            (r.x + r.w / 2) as f32,
            (r.y + r.h / 2) as f32 + tab_size * 0.36,
            tab_size,
            tab_weight,
            if active { WHITE } else { BLACK },
            Align::Center,
        );

        screen.hits.push(HitRegion::new(r, Action::Focus(field)));
    }

    draw_edit_field(setup, fonts, c, screen, g);
}

/// Rysuje etykietę pola, ramkę z wpisywaną wartością, kursor i podpowiedź.
///
/// Wydzielone z [`draw_setup_head`], bo to **jedyny** fragment ekranu konfiguracji,
/// który zmienia się przy wpisaniu znaku. Firmware przerysowuje sam ten kawałek
/// w istniejącym płótnie zamiast całej klatki — patrz [`redraw_setup_value`].
fn draw_edit_field(
    setup: &Setup,
    fonts: &Fonts,
    c: &mut Gray8,
    screen: &mut Screen,
    g: &SetupGeom,
) {
    let m = g.content_margin;
    fonts.draw(
        c,
        setup.field().label(),
        m as f32,
        g.label_base,
        TEXT_LEAD,
        Weight::Medium,
        INK_DIM,
        Align::Left,
    );

    let boxr = Rect::new(m, g.box_y, g.w - 2 * m, g.box_h);
    stroke_round_rect(c, boxr, 8.0, 2, BLACK);
    // Z zapasem na kursor i grubość ramki — obszar idzie prosto do częściowego
    // odświeżenia, a urwana krawędź ramki wyglądałaby jak artefakt.
    screen.edit_box = Some(Rect::new(boxr.x - 4, boxr.y - 4, boxr.w + 8, boxr.h + 8));

    // Zostaje warunkowy: ramka wartości ma w poziomie 64 px zamiast 86.
    let size = if g.compact { TEXT_HEAD } else { TEXT_TITLE };
    let pad = 14;
    // 10 px zapasu na kursor, żeby nie wylazł za ramkę przy pełnym polu.
    let avail = (boxr.w - 2 * pad - 10) as f32;
    let shown = tail_that_fits(fonts, setup.current(), avail, size);
    let baseline = (boxr.y + boxr.h / 2) as f32 + size * 0.35;
    let drawn = fonts.draw(
        c,
        &shown,
        (boxr.x + pad) as f32,
        baseline,
        size,
        Weight::Regular,
        BLACK,
        Align::Left,
    );

    // Kursor. Stoi zawsze na końcu — pole nie ma nawigacji w środku tekstu i to
    // jest świadome: strzałki na e-papierze przy 0,28 s na odświeżenie to gorsze
    // narzędzie niż skasowanie i wpisanie od nowa.
    vline(
        c,
        boxr.x + pad + drawn as i32 + 3,
        boxr.y + 10,
        boxr.h - 20,
        3,
        BLACK,
    );

    // 18 px Regular było najgorszym miejscem w całym pliku: 55% kresek tego napisu
    // wychodziło jednopikselowych, czyli dokładnie „cienkie i wypłowiałe" ze
    // zgłoszenia. Przy 22 px Medium zostaje 4%, a atramentu jest 781 px zamiast 432.
    // Warunek na orientację znika — 22 px mieści się w obu.
    let hint = fonts.truncate(
        setup.field().hint(),
        (g.w - 2 * m) as f32,
        TEXT_BODY,
        Weight::Medium,
    );
    fonts.draw(
        c,
        &hint,
        m as f32,
        g.hint_base,
        TEXT_BODY,
        Weight::Medium,
        INK_DIM,
        Align::Left,
    );
}

/// Obszar, który obejmuje etykietę pola, ramkę wartości i podpowiedź.
fn edit_field_area(g: &SetupGeom) -> Rect {
    let top = g.label_base as i32 - 26;
    let bottom = g.hint_base as i32 + 8;
    Rect::new(0, top, g.w, bottom - top)
}

/// Przerysowuje w ISTNIEJĄCYM płótnie sam obszar edytowanej wartości.
///
/// Zwraca prostokąt do wypchnięcia na panel. Kosztuje ułamek pełnej klatki: nie ma
/// tu ani czyszczenia 518 KB płótna, ani rysowania sześćdziesięciu klawiszy.
pub fn redraw_setup_value(setup: &Setup, fonts: &Fonts, c: &mut Gray8) -> Rect {
    let g = SetupGeom::of(c, setup);
    let area = edit_field_area(&g);
    c.fill_rect(area, WHITE);
    let mut throwaway = Screen::default();
    draw_edit_field(setup, fonts, c, &mut throwaway, &g);
    // Dwupoziomowo, tak samo jak pełny render — inaczej odrysowany fragment miałby
    // antyaliasing, a reszta ekranu nie, i po `MODE_DU` widać by to było jako
    // jaśniejszy prostokąt. Pilnuje tego test `przyrostowe_odrysowanie_zgadza_sie_z_pelnym`.
    c.quantize2_rect(area);
    area
}

/// Przerysowuje w istniejącym płótnie wskazane klawisze.
///
/// `pressed` decyduje, który z nich idzie w negatywie. Klawisze spoza listy zostają
/// nietknięte — o to właśnie chodzi.
pub fn redraw_setup_keys(
    setup: &Setup,
    fonts: &Fonts,
    c: &mut Gray8,
    rects: &[Rect],
    pressed: Option<Rect>,
) {
    if rects.is_empty() {
        return;
    }
    let g = SetupGeom::of(c, setup);
    let mut throwaway = Screen::default();
    // Klawisz w negatywie zostawia po sobie czarne wypełnienie, więc pod spodem
    // trzeba wyczyścić — `draw_key` rysuje ramkę, nie tło.
    for rect in rects {
        c.fill_rect(*rect, WHITE);
    }
    draw_keyboard_filtered(setup, fonts, c, &mut throwaway, &g, pressed, Some(rects));
    // Jak w `redraw_setup_value`: fragment ma wyjść bit w bit tak, jak wyszedłby
    // z pełnego renderu.
    for rect in rects {
        c.quantize2_rect(*rect);
    }
}

/// Lista wszystkich pól z tym, co urządzenie ma zapamiętane.
///
/// Rysowana tylko wtedy, gdy nad klawiaturą zostaje miejsce — czyli w pionie.
/// W poziomie zostaje ~60 px i rolę „co już jest wpisane" pełnią same zakładki.
/// W pionie zostaje ~250 px i bez tego bloku nie dałoby się jednym spojrzeniem
/// sprawdzić konfiguracji, a ekran wyglądałby na niedokończony.
///
/// Wiersze są jednocześnie obszarami dotykowymi. Są znacznie większym celem niż
/// zakładka, więc na urządzeniu to one będą główną drogą przełączania pola.
fn draw_setup_summary(
    setup: &Setup,
    fonts: &Fonts,
    c: &mut Gray8,
    screen: &mut Screen,
    g: &SetupGeom,
) {
    const HEAD_H: i32 = 24;
    const LABEL_W: i32 = 120;
    /// Wiersz niższy niż to przestaje być celem dla palca; wyższy tylko rozciąga
    /// listę bez zysku.
    const ROW_H_MIN: i32 = 28;
    const ROW_H_MAX: i32 = 40;

    let m = g.content_margin;
    let top = g.summary_top;
    let count = Field::ALL.len() as i32;

    let avail = g.summary_bottom - top;
    let row_h = ((avail - HEAD_H) / count).clamp(ROW_H_MIN, ROW_H_MAX);
    if HEAD_H + count * row_h > avail {
        // W poziomie nad klawiaturą zostaje ~44 px i lista się nie mieści.
        return;
    }

    fonts.draw(
        c,
        "zapamiętane",
        m as f32,
        (top + 12) as f32,
        TEXT_FLOOR,
        Weight::Medium,
        INK_DIM,
        Align::Left,
    );
    hline(c, m, top + HEAD_H - 6, g.w - 2 * m, 1, INK_DIM);

    for (i, field) in Field::ALL.into_iter().enumerate() {
        let y = top + HEAD_H + i as i32 * row_h;
        let baseline = (y + row_h / 2) as f32 + 6.0;
        let active = field == setup.field();
        // Regular przy 20 px łamał regułę 2 na PIĘCIU z sześciu wierszy — zmierzone:
        // „Dom-WiFi-5GHz" miało 40% kresek jednopikselowych. Wiersz aktywny idzie
        // o stopień wyżej, żeby różnica została; nośnikiem jest krój, nie ton.
        let weight = if active { Weight::Bold } else { Weight::Medium };

        fonts.draw(
            c,
            field.tab(),
            m as f32,
            baseline,
            TEXT_BODY,
            weight,
            if active { BLACK } else { INK_DIM },
            Align::Left,
        );

        let value = summary_value(setup, field);
        let value = fonts.truncate(&value, (g.w - 2 * m - LABEL_W) as f32, TEXT_BODY, weight);
        fonts.draw(
            c,
            &value,
            (m + LABEL_W) as f32,
            baseline,
            TEXT_BODY,
            weight,
            BLACK,
            Align::Left,
        );

        screen.hits.push(HitRegion::new(
            Rect::new(m - 8, y, g.w - 2 * m + 16, row_h),
            Action::Focus(field),
        ));
    }
}

/// Wartość pokazywana na liście.
///
/// Hasło idzie w kropkach nie z powodu tajemnicy — edytowane widać w całości, bo
/// inaczej nie dałoby się go wpisać z klawiatury dotykowej i sprawdzić literówki —
/// tylko dlatego, że na liście nic by nie wnosiło, a urządzenie wisi na ścianie.
fn summary_value(setup: &Setup, field: Field) -> String {
    let raw = setup.value(field);
    if raw.is_empty() {
        return match field {
            // To samo domyślne, co `Config::tz()` w firmwarze.
            Field::Timezone => "Europe/Warsaw".to_string(),
            _ => "—".to_string(),
        };
    }
    if field == Field::Password {
        return "•".repeat(raw.chars().count().min(16));
    }
    raw.to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyStyle {
    Normal,
    /// `⇧` jednorazowy — grubsza ramka.
    Marked,
    /// Wypełniony: `⇧` zablokowany albo gotowy „zapisz".
    Filled,
}

fn draw_keyboard(
    setup: &Setup,
    fonts: &Fonts,
    c: &mut Gray8,
    screen: &mut Screen,
    g: &SetupGeom,
    pressed: Option<Rect>,
) {
    draw_keyboard_filtered(setup, fonts, c, screen, g, pressed, None);
}

/// Rysuje klawiaturę; przy `only = Some(..)` **rysuje wyłącznie wskazane klawisze**,
/// ale obszary dotykowe wypełnia tak samo dla wszystkich.
#[allow(clippy::too_many_arguments)]
fn draw_keyboard_filtered(
    setup: &Setup,
    fonts: &Fonts,
    c: &mut Gray8,
    screen: &mut Screen,
    g: &SetupGeom,
    pressed: Option<Rect>,
    only: Option<&[Rect]>,
) {
    let wanted = |r: Rect| only.is_none_or(|list| list.contains(&r));
    let rows = setup.page().rows();
    let caps = setup.caps().is_active();
    // Zostaje warunkowy: klawisz ma w poziomie 52 px wysokości zamiast 78.
    let size = if g.compact { TEXT_LEAD } else { TEXT_HEAD };

    for (r, row) in rows.iter().enumerate() {
        let n = row.chars().count() as i32;
        let total = n * g.unit + (n - 1) * g.gap;
        let x0 = (g.w - total) / 2;
        let y = g.kb_top + r as i32 * (g.key_h + g.gap);

        for (i, ch) in row.chars().enumerate() {
            let rect = Rect::new(x0 + i as i32 * (g.unit + g.gap), y, g.unit, g.key_h);
            // Na klawiszu widać to, co wpiszemy — a akcja niesie znak MAŁY.
            // Wielkość liter rozstrzyga `Setup::apply`, żeby stan `⇧` był
            // w jednym miejscu, a nie w dwóch.
            let label = if caps {
                ch.to_uppercase().collect::<String>()
            } else {
                ch.to_string()
            };
            if wanted(rect) {
                draw_key(
                    fonts,
                    c,
                    rect,
                    &label,
                    size,
                    pressed_style(KeyStyle::Normal, rect, pressed),
                );
            }
            screen.hits.push(HitRegion::new(rect, Action::Key(ch)));
        }
    }

    // Pasek dolny: `Aa`, przełącznik strony, spacja, kasowanie, zapis.
    //
    // Wszystkie napisy są słowami, nie symbolami, i to jest wymuszone: wbudowany
    // krój nie ma ANI `⇧`, ANI `⌫`, ANI nawet `←` — mimo że `tools/subset-fonts.sh`
    // wymienia zakres strzałek, źródłowy Noto Sans ich nie zawiera i pyftsubset je
    // pomija. Brakujący glif rysuje się jako nic, więc klawisz z „←" był po prostu
    // pustym prostokątem. Patrz `Fonts::has_glyph`.
    let bar_y = g.kb_top + rows.len() as i32 * (g.key_h + g.gap);
    let bar_w = 10 * g.unit + 9 * g.gap;
    let mut x = (g.w - bar_w) / 2;
    let bar_size = TEXT_BODY;

    let caps_style = match setup.caps() {
        Caps::Off => KeyStyle::Normal,
        Caps::Once => KeyStyle::Marked,
        Caps::Lock => KeyStyle::Filled,
    };
    let save_style = if setup.is_complete() {
        KeyStyle::Filled
    } else {
        KeyStyle::Normal
    };

    // Jednostki sumują się do 10, czyli dokładnie do szerokości wiersza klawiszy.
    let bar: [(f32, &str, Action, KeyStyle); 5] = [
        (1.5, "Aa", Action::Caps, caps_style),
        (
            1.5,
            setup.page().switch_label(),
            Action::KeyPage,
            KeyStyle::Normal,
        ),
        (3.0, "spacja", Action::Key(' '), KeyStyle::Normal),
        (2.0, "usuń", Action::Backspace, KeyStyle::Normal),
        (2.0, "zapisz", Action::Save, save_style),
    ];

    for (units, label, action, style) in bar {
        let w = g.span(units);
        let rect = Rect::new(x, bar_y, w, g.key_h);
        if wanted(rect) {
            draw_key(
                fonts,
                c,
                rect,
                label,
                bar_size,
                pressed_style(style, rect, pressed),
            );
        }
        screen.hits.push(HitRegion::new(rect, action));
        x += w + g.gap;
    }
}

fn draw_key(fonts: &Fonts, c: &mut Gray8, r: Rect, label: &str, size: f32, style: KeyStyle) {
    match style {
        // Ramka klawisza MUSI być czarna. Przy `INK_DIM` (0x66) klawiatura była
        // ledwo widoczna: panel słabo rozróżnia jasne półtony, a `MODE_DU` — czyli
        // każde odświeżenie w oknie interaktywnym — jest dwupoziomowy i tak czy owak
        // sprowadza szarość do bieli albo czerni. Objawiało się to tym, że klawisz
        // stawał się czytelny dopiero po naciśnięciu, bo dopiero wtedy przechodził
        // przez DU.
        KeyStyle::Normal => stroke_round_rect(c, r, 8.0, 2, BLACK),
        KeyStyle::Marked => stroke_round_rect(c, r, 8.0, 4, BLACK),
        KeyStyle::Filled => fill_round_rect(c, r, 8.0, BLACK),
    }

    let ink = if style == KeyStyle::Filled {
        WHITE
    } else {
        BLACK
    };
    let label = fonts.truncate(label, (r.w - 8) as f32, size, Weight::Medium);
    fonts.draw(
        c,
        &label,
        (r.x + r.w / 2) as f32,
        (r.y + r.h / 2) as f32 + size * 0.36,
        size,
        Weight::Medium,
        ink,
        Align::Center,
    );
}

/// Zwraca końcówkę tekstu mieszczącą się w `max_width`, poprzedzoną wielokropkiem.
///
/// [`Fonts::truncate`] ucina koniec, a tutaj potrzebny jest odwrotny kierunek:
/// przy wpisywaniu 120-znakowego adresu iCal trzeba widzieć to, co się właśnie
/// stuka, a nie początek sprzed dwóch minut.
fn tail_that_fits(fonts: &Fonts, s: &str, max_width: f32, size: f32) -> String {
    if fonts.measure(s, size, Weight::Regular) <= max_width {
        return s.to_string();
    }

    let ell_w = fonts.measure("…", size, Weight::Regular);
    let mut start = s.len();
    // Idziemy po granicach ZNAKÓW od końca — cięcie po bajtach rozwaliłoby „ż".
    for (i, _) in s.char_indices().rev() {
        if fonts.measure(&s[i..], size, Weight::Regular) + ell_w > max_width {
            break;
        }
        start = i;
    }
    format!("…{}", &s[start..])
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
        // Rozmiary i krój muszą być te SAME, co w `draw_event`, inaczej strażnik
        // pilnuje czegoś, czego na ekranie nie ma. Wcześniej mierzył 20/26 px krojem
        // Regular, a rysowane było Medium — czyli węższą wersję niż prawdziwa.
        for (text, size) in [("cały dzień", TEXT_BODY), ("08:00", TEXT_LEAD)] {
            let w = fonts.measure(text, size, Weight::Medium).ceil() as i32;
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

    // -----------------------------------------------------------------------
    // Ekran konfiguracji
    // -----------------------------------------------------------------------

    use crate::setup::{Applied, Field, Page, Setup};

    /// Każda orientacja razy każda strona klawiatury — cztery kombinacje, w których
    /// coś może wyjść poza płótno albo wejść na coś innego.
    fn kazdy_wariant() -> Vec<(Rotation, Page, Setup)> {
        let mut out = Vec::new();
        for rotation in [Rotation::Portrait, Rotation::Landscape] {
            for page in [Page::Letters, Page::Symbols] {
                let mut s = Setup::new();
                if page == Page::Symbols {
                    s.apply(Action::KeyPage);
                }
                out.push((rotation, page, s));
            }
        }
        out
    }

    #[test]
    fn klawiatura_miesci_sie_w_plotnie() {
        let fonts = Fonts::embedded();
        for (rotation, page, setup) in kazdy_wariant() {
            let mut c = Gray8::new(rotation);
            let screen = render_setup(&setup, &fonts, &mut c);
            let (w, h) = (c.width() as i32, c.height() as i32);

            for hit in &screen.hits {
                assert!(
                    hit.rect.x >= 0
                        && hit.rect.y >= 0
                        && hit.rect.right() <= w
                        && hit.rect.bottom() <= h,
                    "{rotation:?}/{page:?}: {:?} wychodzi poza {w}x{h}",
                    hit.rect
                );
            }
        }
    }

    #[test]
    fn kazdy_klawisz_ma_wlasny_obszar_dotykowy() {
        let fonts = Fonts::embedded();
        for (rotation, page, setup) in kazdy_wariant() {
            let mut c = Gray8::new(rotation);
            let screen = render_setup(&setup, &fonts, &mut c);

            for row in page.rows() {
                for ch in row.chars() {
                    assert!(
                        screen.hits.iter().any(|h| h.action == Action::Key(ch)),
                        "{rotation:?}/{page:?}: brak obszaru dla `{ch}`"
                    );
                }
            }
            // Pasek dolny jest na obu stronach.
            for action in [
                Action::Caps,
                Action::KeyPage,
                Action::Key(' '),
                Action::Backspace,
                Action::Save,
            ] {
                assert!(
                    screen.hits.iter().any(|h| h.action == action),
                    "{rotation:?}/{page:?}: brak {action:?}"
                );
            }
        }
    }

    #[test]
    fn obszary_dotykowe_nie_nachodza_na_siebie() {
        let fonts = Fonts::embedded();
        for (rotation, page, setup) in kazdy_wariant() {
            let mut c = Gray8::new(rotation);
            let screen = render_setup(&setup, &fonts, &mut c);

            for (i, a) in screen.hits.iter().enumerate() {
                for b in &screen.hits[i + 1..] {
                    let overlap = a.rect.x < b.rect.right()
                        && b.rect.x < a.rect.right()
                        && a.rect.y < b.rect.bottom()
                        && b.rect.y < a.rect.bottom();
                    assert!(
                        !overlap,
                        "{rotation:?}/{page:?}: {:?} nachodzi na {:?}",
                        a.action, b.action
                    );
                }
            }
        }
    }

    #[test]
    fn klawiatura_nie_wchodzi_na_pole_wartosci() {
        let fonts = Fonts::embedded();
        for (rotation, page, setup) in kazdy_wariant() {
            let mut c = Gray8::new(rotation);
            let g = SetupGeom::of(&c, &setup);
            let _ = render_setup(&setup, &fonts, &mut c);
            assert!(
                g.hint_base < g.kb_top as f32,
                "{rotation:?}/{page:?}: podpowiedź na linii {} wchodzi na klawiaturę od {}",
                g.hint_base,
                g.kb_top
            );
        }
    }

    #[test]
    fn zakladki_prowadza_do_wszystkich_pol() {
        let fonts = Fonts::embedded();
        let mut c = Gray8::new(Rotation::Portrait);
        let screen = render_setup(&Setup::new(), &fonts, &mut c);
        for field in Field::ALL {
            assert!(
                screen.hits.iter().any(|h| h.action == Action::Focus(field)),
                "brak zakładki dla {field:?}"
            );
        }
    }

    /// Pełna droga: narysuj, trafnij w środek klawisza, zastosuj akcję.
    /// To jest dokładnie to, co robi firmware po odczycie z GT911.
    #[test]
    fn dotkniecie_klawisza_wpisuje_znak() {
        let fonts = Fonts::embedded();
        let mut setup = Setup::new();

        for ch in "dom".chars() {
            let mut c = Gray8::new(Rotation::Portrait);
            let screen = render_setup(&setup, &fonts, &mut c);
            let key = screen
                .hits
                .iter()
                .find(|h| h.action == Action::Key(ch))
                .unwrap_or_else(|| panic!("brak klawisza `{ch}`"));

            let action = screen
                .hit(key.rect.x + key.rect.w / 2, key.rect.y + key.rect.h / 2)
                .expect("środek klawisza musi trafiać");
            assert_eq!(setup.apply(action), Applied::Edited);
        }

        assert_eq!(setup.value(Field::Ssid), "dom");
    }

    #[test]
    fn dotkniecie_zakladki_przelacza_pole() {
        let fonts = Fonts::embedded();
        let mut setup = Setup::new();
        let mut c = Gray8::new(Rotation::Landscape);
        let screen = render_setup(&setup, &fonts, &mut c);

        let tab = screen
            .hits
            .iter()
            .find(|h| h.action == Action::Focus(Field::Ics))
            .unwrap();
        let action = screen
            .hit(tab.rect.x + tab.rect.w / 2, tab.rect.y + tab.rect.h / 2)
            .unwrap();
        assert_eq!(setup.apply(action), Applied::Relayout);
        assert_eq!(setup.field(), Field::Ics);
    }

    #[test]
    fn dlugi_adres_pokazuje_koncowke_a_nie_poczatek() {
        let fonts = Fonts::embedded();
        let long =
            "https://calendar.google.com/calendar/ical/ktos%40gmail.com/private-abc123/basic.ics";
        let shown = tail_that_fits(&fonts, long, 300.0, 24.0);
        assert!(shown.starts_with('…'), "brak sygnału ucięcia: {shown}");
        assert!(
            long.ends_with(shown.trim_start_matches('…')),
            "pokazany fragment nie jest końcówką: {shown}"
        );
        assert!(fonts.measure(&shown, 24.0, Weight::Regular) <= 300.0);
    }

    #[test]
    fn krotki_adres_pokazuje_sie_w_calosci() {
        let fonts = Fonts::embedded();
        assert_eq!(tail_that_fits(&fonts, "dom", 300.0, 24.0), "dom");
        assert_eq!(tail_that_fits(&fonts, "", 300.0, 24.0), "");
    }

    #[test]
    fn wersja_w_stopce_otwiera_konfiguracje() {
        let fonts = Fonts::embedded();
        let mut model = Model::empty(dt(12, 0));
        model.firmware = "taskcerber 0.1.0+gabc1234".to_string();
        let mut c = Gray8::new(Rotation::Portrait);
        let screen = render(&model, &fonts, &mut c);

        let hit = screen
            .hits
            .iter()
            .find(|h| h.action == Action::OpenSetup)
            .expect("wersja w stopce musi być wejściem do konfiguracji");
        // Palec, nie kursor myszy.
        assert!(hit.rect.h >= 44, "obszar {} px jest za niski", hit.rect.h);
    }

    /// Ekran startowy obiecuje „krok 1 — dotknij ekranu". Ta obietnica ma pokrycie
    /// w przycisku na środku, a nie w plakietce 44 px przy krawędzi.
    /// Przerysowanie przyrostowe musi dać **te same piksele**, co pełny render.
    /// Inaczej ekran konfiguracji rozjeżdżałby się z tym, co firmware myśli, że rysuje.
    #[test]
    fn przyrostowe_odrysowanie_zgadza_sie_z_pelnym() {
        use crate::setup::Field;

        let fonts = Fonts::embedded();
        let mut przed = Setup::new();
        przed.set(Field::Ssid, "Dom".to_string());
        let mut po = Setup::new();
        po.set(Field::Ssid, "Doma".to_string());

        for rot in [Rotation::Portrait, Rotation::Landscape] {
            // Płótno w stanie „przed", potem przyrostowo doprowadzone do „po".
            let mut a = Gray8::new(rot);
            let screen = render_setup(&przed, &fonts, &mut a);
            let area = redraw_setup_value(&po, &fonts, &mut a);

            // Płótno wyrenderowane od zera w stanie „po".
            let mut b = Gray8::new(rot);
            let _ = render_setup(&po, &fonts, &mut b);

            for y in area.y..area.bottom() {
                for x in area.x..area.right() {
                    assert_eq!(
                        a.get(x, y),
                        b.get(x, y),
                        "{rot:?}: pole wartości, piksel ({x}, {y})"
                    );
                }
            }

            // To samo dla klawisza w negatywie.
            let klawisz = screen
                .hits
                .iter()
                .find(|h| matches!(h.action, Action::Key(_)))
                .expect("klawiatura ma klawisze")
                .rect;

            let mut c1 = Gray8::new(rot);
            let _ = render_setup(&po, &fonts, &mut c1);
            redraw_setup_keys(&po, &fonts, &mut c1, &[klawisz], Some(klawisz));

            let mut c2 = Gray8::new(rot);
            let _ = render_setup_pressed(&po, &fonts, &mut c2, Some(klawisz));

            for y in klawisz.y..klawisz.bottom() {
                for x in klawisz.x..klawisz.right() {
                    assert_eq!(
                        c1.get(x, y),
                        c2.get(x, y),
                        "{rot:?}: klawisz w negatywie, piksel ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn nieskonfigurowane_urzadzenie_ma_przycisk_konfiguracji_na_srodku() {
        let fonts = Fonts::embedded();
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut model = Model::empty(dt(12, 0));
            model.net = NetState::NeedsAuth;
            let mut c = Gray8::new(rot);
            let screen = render(&model, &fonts, &mut c);

            let srodek = (c.width() as i32 / 2, c.height() as i32 / 2);
            assert_eq!(
                screen.hit(srodek.0, srodek.1),
                Some(Action::OpenSetup),
                "{rot:?}: stuknięcie w środek ekranu ma otwierać konfigurację"
            );

            let przycisk = screen
                .hits
                .iter()
                .find(|h| h.action == Action::OpenSetup && h.rect.h >= SETUP_BUTTON_H)
                .expect("brak przycisku konfiguracji");
            assert!(
                przycisk.rect.w >= 300,
                "{rot:?}: przycisk {} px szerokości to za mało",
                przycisk.rect.w
            );
        }
    }

    /// Skonfigurowane urządzenie z pustym kalendarzem nie dostaje przycisku —
    /// jest co pokazać, tylko akurat nic nie ma w planie.
    #[test]
    fn skonfigurowane_urzadzenie_bez_wydarzen_nie_ma_przycisku_konfiguracji() {
        let fonts = Fonts::embedded();
        let model = Model::empty(dt(12, 0));
        let mut c = Gray8::new(Rotation::Portrait);
        let screen = render(&model, &fonts, &mut c);
        assert_eq!(
            screen.hit(c.width() as i32 / 2, c.height() as i32 / 2),
            None
        );
    }

    #[test]
    fn nieskonfigurowane_urzadzenie_prowadzi_do_konfiguracji_z_naglowka() {
        let fonts = Fonts::embedded();
        let mut model = Model::empty(dt(12, 0));
        model.net = NetState::NeedsAuth;
        let mut c = Gray8::new(Rotation::Portrait);
        let screen = render(&model, &fonts, &mut c);

        // Plakietka „skonfiguruj urządzenie" otwiera konfigurację zamiast wymuszać
        // pobranie, którego i tak nie ma z czego zrobić.
        assert!(screen.hits.iter().any(|h| h.action == Action::OpenSetup));
        assert!(
            !screen.hits.iter().any(|h| h.action == Action::RefreshNow),
            "na nieskonfigurowanym urządzeniu odświeżenie nie ma sensu"
        );
    }

    #[test]
    fn skonfigurowane_urzadzenie_zachowuje_odswiezenie_z_naglowka() {
        let fonts = Fonts::embedded();
        let model = Model::empty(dt(12, 0));
        let mut c = Gray8::new(Rotation::Portrait);
        let screen = render(&model, &fonts, &mut c);
        assert!(screen.hits.iter().any(|h| h.action == Action::RefreshNow));
    }

    /// Brakujący glif rysuje się jako NIC — bez tofu, bez ostrzeżenia, z poprawnym
    /// układem. Klawisz kasowania z napisem „←" był przez to pustym prostokątem,
    /// mimo że `tools/subset-fonts.sh` wymienia zakres strzałek: źródłowy Noto Sans
    /// ich nie ma i pyftsubset je po cichu pomija.
    ///
    /// Ten test przechodzi po każdym znaku, który interfejs może narysować.
    #[test]
    fn wszystkie_znaki_interfejsu_maja_glify() {
        use std::collections::BTreeSet;

        let fonts = Fonts::embedded();
        let mut znaki: BTreeSet<char> = BTreeSet::new();

        for page in [Page::Letters, Page::Symbols] {
            for row in page.rows() {
                for ch in row.chars() {
                    znaki.insert(ch);
                    // Klawiatura pokazuje wersje wielkie, gdy `⇧` jest włączony.
                    znaki.extend(ch.to_uppercase());
                }
            }
            znaki.extend(page.switch_label().chars());
        }

        for field in Field::ALL {
            znaki.extend(field.tab().chars());
            znaki.extend(field.label().chars());
            znaki.extend(field.hint().chars());
        }

        for napis in [
            "Konfiguracja",
            "Skonfiguruj",
            "sieć i adres kalendarza",
            "Nic w planie",
            "Wolne do końca widocznego okresu",
            "komplet — naciśnij zapisz",
            "brakuje:",
            "zapamiętane",
            "Europe/Warsaw",
            "Aa",
            "spacja",
            "usuń",
            "zapisz",
            "•",
            "…",
            "—",
        ] {
            znaki.extend(napis.chars());
        }

        for weight in [Weight::Regular, Weight::Medium, Weight::Bold] {
            for ch in &znaki {
                assert!(
                    fonts.has_glyph(*ch, weight),
                    "krój {weight:?} nie ma glifu `{ch}` (U+{:04X}) — narysuje się jako nic",
                    *ch as u32
                );
            }
        }
    }

    #[test]
    fn lista_pol_pokazuje_zapamietane_wartosci() {
        let mut s = Setup::new();
        s.set(Field::Ssid, "Dom");
        s.set(Field::Password, "tajne");

        assert_eq!(summary_value(&s, Field::Ssid), "Dom");
        // Hasło w kropkach, po jednej na znak.
        assert_eq!(summary_value(&s, Field::Password), "•••••");
        assert_eq!(summary_value(&s, Field::Ics), "—");
        // Puste pole strefy pokazuje to, co firmware faktycznie zastosuje.
        assert_eq!(summary_value(&s, Field::Timezone), "Europe/Warsaw");
    }

    #[test]
    fn lista_pol_jest_w_pionie_a_w_poziomie_jej_nie_ma() {
        let fonts = Fonts::embedded();
        let setup = Setup::new();

        // W pionie każde pole ma DWA wejścia: zakładkę i wiersz listy.
        let mut c = Gray8::new(Rotation::Portrait);
        let pion = render_setup(&setup, &fonts, &mut c);
        let wejscia = pion
            .hits
            .iter()
            .filter(|h| h.action == Action::Focus(Field::Ota))
            .count();
        assert_eq!(wejscia, 2, "w pionie ma być zakładka i wiersz listy");

        // ...i ma tam zostać po przełączeniu na stronę symboli, która jest o wiersz
        // wyższa. Znikająca lista to przemalowanie ćwiartki ekranu przy zmianie
        // strony klawiatury.
        let mut symbole = Setup::new();
        symbole.apply(Action::KeyPage);
        let mut c = Gray8::new(Rotation::Portrait);
        let pion_symbole = render_setup(&symbole, &fonts, &mut c);
        let wejscia = pion_symbole
            .hits
            .iter()
            .filter(|h| h.action == Action::Focus(Field::Ota))
            .count();
        assert_eq!(
            wejscia, 2,
            "lista pól nie może znikać po zmianie strony klawiatury"
        );

        // W poziomie nad klawiaturą nie ma na listę miejsca — zostaje sama zakładka.
        let mut c = Gray8::new(Rotation::Landscape);
        let poziom = render_setup(&setup, &fonts, &mut c);
        let wejscia = poziom
            .hits
            .iter()
            .filter(|h| h.action == Action::Focus(Field::Ota))
            .count();
        assert_eq!(wejscia, 1, "w poziomie ma zostać sama zakładka");
    }
    /// Błyskawica ładowania musi być BRYŁĄ, nie zadrapaniem.
    ///
    /// Poprzednia wersja składała ją z kresek 2 px i została zgłoszona ze sprzętu
    /// jako „paproch". Test pilnuje dwóch rzeczy naraz: że w ogóle coś przybywa
    /// przy ładowaniu, i że najszerszy ciągły bieg czerni ma co najmniej 4 px —
    /// czyli że nikt nie wrócił do rysowania pojedynczymi pikselami.
    #[test]
    fn blyskawica_ladowania_jest_bryla_a_nie_kreska() {
        let fonts = Fonts::embedded();
        let now = NaiveDate::from_ymd_opt(2026, 8, 22)
            .unwrap()
            .and_hms_opt(14, 0, 0)
            .unwrap();

        let render_z = |charging: bool| {
            let mut m = Model::empty(now);
            m.battery = Battery {
                percent: Some(78),
                millivolts: Some(4100),
                charging,
            };
            let mut c = Gray8::new(Rotation::Portrait);
            render(&m, &fonts, &mut c);
            c
        };

        let bez = render_z(false);
        let z = render_z(true);
        assert_ne!(
            bez.pixels(),
            z.pixels(),
            "ładowanie musi być w ogóle widoczne"
        );

        // Najszerszy ciągły bieg czerni w obszarze wskaźnika, wiersz po wierszu.
        let mut najszerszy = 0;
        for y in 0..z.height() as i32 {
            let mut biez = 0;
            for x in 0..z.width() as i32 {
                if z.get(x, y) == BLACK && bez.get(x, y) != BLACK {
                    biez += 1;
                    najszerszy = najszerszy.max(biez);
                } else {
                    biez = 0;
                }
            }
        }
        assert!(
            najszerszy >= 4,
            "najszerszy bieg czerni to {najszerszy} px — na 234 DPI to paproch, nie kształt"
        );
    }
}
