//! Model danych, który dashboard renderuje.
//!
//! Ta struktura jest jedyną granicą między „skąd biorę dane" a „jak to wygląda".
//! Firmware wypełnia ją z iCal/HTTP, podgląd na hoście z fixture'ów — a `render()`
//! nie wie i nie musi wiedzieć, które to było.

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

/// Który widok jest pokazywany.
///
/// Kolejność wariantów jest kolejnością zakładek na ekranie i to jest jedyne
/// miejsce, gdzie ta kolejność jest zapisana — [`View::ALL`] karmi zarówno
/// rysowanie paska, jak i testy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum View {
    /// Lista najbliższych wydarzeń — widok domyślny.
    #[default]
    Agenda,
    /// Siatka miesiąca.
    Month,
    /// Planer roczny.
    Year,
}

impl View {
    pub const ALL: [View; 3] = [View::Agenda, View::Month, View::Year];

    /// Napis na zakładce. Krótki, bo segment ma 180 px w pionie.
    pub fn label(self) -> &'static str {
        match self {
            View::Agenda => "agenda",
            View::Month => "miesiąc",
            View::Year => "rok",
        }
    }
}

/// Kompletny stan do wyrenderowania jednej klatki.
#[derive(Debug, Clone)]
pub struct Model {
    /// Czas lokalny w momencie renderowania.
    pub now: NaiveDateTime,
    /// Wydarzenia pogrupowane po dniach, posortowane rosnąco.
    pub days: Vec<DayGroup>,
    pub battery: Battery,
    pub net: NetState,
    /// Kafelki na dole — miejsce na „kilka innych" źródeł.
    pub tiles: Vec<Tile>,
    /// Wersja firmware'u, pokazywana drobnym drukiem w stopce.
    pub firmware: String,
    /// Która strona agendy jest pokazana (liczona od zera).
    pub page: usize,
    /// Gdy ustawione, zamiast agendy pokazywany jest widok szczegółów
    /// wydarzenia o tym indeksie globalnym.
    pub focus: Option<usize>,
    /// O jakie dni urządzenie w ogóle PYTAŁO — zakres domknięty obustronnie.
    ///
    /// To nie to samo co zakres dni, w których coś się dzieje, i ta różnica jest
    /// widoczna na ekranie. `days` zawiera wyłącznie dni Z WYDARZENIAMI, bo
    /// `group_by_day` nie tworzy pustych grup — więc wyprowadzanie „co wiemy"
    /// z pierwszego i ostatniego wpisu daje kłamstwo w obie strony: wolny wtorek
    /// w środku horyzontu wygląda jak dzień, o który nie pytano.
    ///
    /// `None` = urządzenie nic nie pobrało; wtedy nie wie nic o żadnym dniu.
    pub known: Option<(NaiveDate, NaiveDate)>,
    /// Który widok jest na ekranie.
    pub view: View,
}

impl Model {
    /// Pusty model na podany moment — przydatny jako ekran startowy i w testach.
    pub fn empty(now: NaiveDateTime) -> Self {
        Self {
            now,
            days: Vec::new(),
            battery: Battery::default(),
            net: NetState::Ok,
            tiles: Vec::new(),
            firmware: String::new(),
            page: 0,
            known: None,
            focus: None,
            view: View::default(),
        }
    }

    /// Wydarzenie o podanym indeksie globalnym (numeracja ciągła przez wszystkie dni).
    pub fn event_at(&self, index: usize) -> Option<&CalEvent> {
        self.days.iter().flat_map(|d| d.events.iter()).nth(index)
    }

    /// Liczba wszystkich wydarzeń.
    pub fn event_count(&self) -> usize {
        self.days.iter().map(|d| d.events.len()).sum()
    }
}

/// Jeden dzień z listą wydarzeń.
#[derive(Debug, Clone)]
pub struct DayGroup {
    pub date: NaiveDate,
    pub events: Vec<CalEvent>,
}

/// Pojedyncze wydarzenie z kalendarza.
#[derive(Debug, Clone)]
pub struct CalEvent {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    /// Wydarzenie całodniowe — rysowane bez godziny.
    pub all_day: bool,
    pub title: String,
    pub location: Option<String>,
    /// Z którego źródła przyszło; steruje znacznikiem po lewej.
    pub source: SourceTag,
}

impl CalEvent {
    /// Czy wydarzenie już trwa w chwili `now`.
    pub fn is_now(&self, now: NaiveDateTime) -> bool {
        !self.all_day && self.start <= now && now < self.end
    }

    /// Czy wydarzenie już się skończyło.
    pub fn is_past(&self, now: NaiveDateTime) -> bool {
        if self.all_day {
            self.end.date() < now.date()
        } else {
            self.end <= now
        }
    }

    /// Czas trwania w minutach.
    pub fn duration_minutes(&self) -> i64 {
        (self.end - self.start).num_minutes()
    }
}

/// Źródło wydarzenia. Rozróżnialne wizualnie, żeby dało się mieć kilka kalendarzy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceTag {
    Primary,
    Secondary,
    Holiday,
}

/// Stan baterii odczytany z BQ27220 / BQ25896.
#[derive(Debug, Clone, Copy, Default)]
pub struct Battery {
    pub percent: Option<u8>,
    pub millivolts: Option<u16>,
    /// USB podłączone (BQ25896 `PG_STAT`).
    pub charging: bool,
}

/// Stan sieci — na ścianie musi być widać, że dane są nieświeże.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetState {
    /// Ostatnie pobranie się udało.
    Ok,
    /// Dane pochodzą z pamięci; ostatni sukces o podanej godzinie.
    Stale { since: NaiveDateTime },
    /// Brak połączenia z siecią.
    Offline,
    /// Źródło odrzuciło poświadczenia — wymagana interwencja użytkownika.
    NeedsAuth,
}

/// Kafelek z dowolną wartością z innego źródła (pogoda, kurs, cokolwiek).
#[derive(Debug, Clone)]
pub struct Tile {
    pub label: String,
    pub value: String,
    pub unit: Option<String>,
}

impl Tile {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            unit: None,
        }
    }

    pub fn with_unit(mut self, unit: impl Into<String>) -> Self {
        self.unit = Some(unit.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Polskie formatowanie dat.
//
// chrono potrafi to tylko z feature'em `unstable-locales`, który — jak nazwa mówi —
// jest niestabilny. Osiemnaście stringów jest tańsze niż ta zależność.
// ---------------------------------------------------------------------------

const DNI_TYGODNIA: [&str; 7] = [
    "poniedziałek",
    "wtorek",
    "środa",
    "czwartek",
    "piątek",
    "sobota",
    "niedziela",
];

const DNI_SKROT: [&str; 7] = ["pon", "wto", "śro", "czw", "pią", "sob", "nie"];

const MIESIACE_DOPELNIACZ: [&str; 12] = [
    "stycznia",
    "lutego",
    "marca",
    "kwietnia",
    "maja",
    "czerwca",
    "lipca",
    "sierpnia",
    "września",
    "października",
    "listopada",
    "grudnia",
];

/// Pełna nazwa dnia tygodnia, np. „środa".
pub fn dzien_tygodnia(d: NaiveDate) -> &'static str {
    DNI_TYGODNIA[d.weekday().num_days_from_monday() as usize]
}

/// Trzyliterowy skrót dnia, np. „śro".
pub fn dzien_skrot(d: NaiveDate) -> &'static str {
    DNI_SKROT[d.weekday().num_days_from_monday() as usize]
}

/// Nazwa miesiąca w dopełniaczu, np. „sierpnia" — forma używana w datach.
pub fn miesiac_dopelniacz(d: NaiveDate) -> &'static str {
    MIESIACE_DOPELNIACZ[(d.month0()) as usize]
}
/// Nazwa miesiąca w MIANOWNIKU — „sierpień", nie „sierpnia".
///
/// Dopełniacz jest formą do daty („18 sierpnia"), a nagłówek widoku miesięcznego
/// nazywa sam miesiąc i wymaga mianownika. Dwie osobne funkcje, bo polski nie
/// pozwala tu na jedną.
pub fn miesiac_mianownik(d: NaiveDate) -> &'static str {
    const M: [&str; 12] = [
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
    M[(d.month() as usize - 1).min(11)]
}



/// „18 sierpnia"
pub fn data_dzien_miesiac(d: NaiveDate) -> String {
    format!("{} {}", d.day(), miesiac_dopelniacz(d))
}

/// „18 sierpnia 2026"
pub fn data_pelna(d: NaiveDate) -> String {
    format!("{} {} {}", d.day(), miesiac_dopelniacz(d), d.year())
}

/// „14:05"
pub fn godzina(t: NaiveDateTime) -> String {
    format!("{:02}:{:02}", t.hour(), t.minute())
}

/// Nagłówek grupy dziennej: „dziś", „jutro" albo nazwa dnia.
pub fn naglowek_dnia(date: NaiveDate, today: NaiveDate) -> String {
    let delta = (date - today).num_days();
    match delta {
        0 => "dziś".to_string(),
        1 => "jutro".to_string(),
        2 => "pojutrze".to_string(),
        _ => dzien_tygodnia(date).to_string(),
    }
}

/// Względny czas do rozpoczęcia: „za 25 min", „za 3 h", „teraz".
pub fn za_ile(start: NaiveDateTime, now: NaiveDateTime) -> String {
    let mins = (start - now).num_minutes();
    if mins <= 0 {
        return "teraz".to_string();
    }
    if mins < 60 {
        return format!("za {mins} min");
    }
    let hours = mins / 60;
    if hours < 24 {
        let rem = mins % 60;
        if rem == 0 {
            format!("za {hours} h")
        } else {
            format!("za {hours} h {rem} min")
        }
    } else {
        format!("za {} dni", hours / 24)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;

    fn dt(y: i32, m: u32, d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap()
    }

    #[test]
    fn poniedzialek_jest_pierwszy() {
        // 2026-08-17 to poniedziałek.
        let pon = NaiveDate::from_ymd_opt(2026, 8, 17).unwrap();
        assert_eq!(dzien_tygodnia(pon), "poniedziałek");
        assert_eq!(dzien_skrot(pon), "pon");
        let nie = NaiveDate::from_ymd_opt(2026, 8, 23).unwrap();
        assert_eq!(dzien_tygodnia(nie), "niedziela");
    }

    #[test]
    fn miesiace_w_dopelniaczu() {
        let d = NaiveDate::from_ymd_opt(2026, 8, 18).unwrap();
        assert_eq!(data_dzien_miesiac(d), "18 sierpnia");
        assert_eq!(data_pelna(d), "18 sierpnia 2026");
        let d = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        assert_eq!(miesiac_dopelniacz(d), "marca");
        let d = NaiveDate::from_ymd_opt(2026, 12, 24).unwrap();
        assert_eq!(miesiac_dopelniacz(d), "grudnia");
    }

    #[test]
    fn wszystkie_miesiace_maja_nazwe() {
        for m in 1..=12u32 {
            let d = NaiveDate::from_ymd_opt(2026, m, 1).unwrap();
            assert!(!miesiac_dopelniacz(d).is_empty());
        }
    }

    #[test]
    fn naglowki_wzgledne() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 18).unwrap();
        assert_eq!(naglowek_dnia(today, today), "dziś");
        assert_eq!(naglowek_dnia(today.succ_opt().unwrap(), today), "jutro");
        let za_tydzien = today + chrono::Duration::days(7);
        assert_eq!(naglowek_dnia(za_tydzien, today), "wtorek");
    }

    #[test]
    fn czas_wzgledny() {
        let now = dt(2026, 8, 18, 12, 0);
        assert_eq!(za_ile(dt(2026, 8, 18, 12, 25), now), "za 25 min");
        assert_eq!(za_ile(dt(2026, 8, 18, 15, 0), now), "za 3 h");
        assert_eq!(za_ile(dt(2026, 8, 18, 15, 30), now), "za 3 h 30 min");
        assert_eq!(za_ile(dt(2026, 8, 18, 11, 0), now), "teraz");
        assert_eq!(za_ile(dt(2026, 8, 21, 12, 0), now), "za 3 dni");
    }

    #[test]
    fn trwajace_wydarzenie_jest_wykrywane() {
        let now = dt(2026, 8, 18, 12, 30);
        let e = CalEvent {
            start: dt(2026, 8, 18, 12, 0),
            end: dt(2026, 8, 18, 13, 0),
            all_day: false,
            title: "Stand-up".into(),
            location: None,
            source: SourceTag::Primary,
        };
        assert!(e.is_now(now));
        assert!(!e.is_past(now));
        assert_eq!(e.duration_minutes(), 60);
        assert!(e.is_past(dt(2026, 8, 18, 13, 1)));
    }

    #[test]
    fn calodniowe_nie_jest_nigdy_teraz() {
        let e = CalEvent {
            start: NaiveDate::from_ymd_opt(2026, 8, 18)
                .unwrap()
                .and_time(NaiveTime::MIN),
            end: NaiveDate::from_ymd_opt(2026, 8, 19)
                .unwrap()
                .and_time(NaiveTime::MIN),
            all_day: true,
            title: "Urlop".into(),
            location: None,
            source: SourceTag::Primary,
        };
        assert!(!e.is_now(dt(2026, 8, 18, 12, 0)));
        assert!(!e.is_past(dt(2026, 8, 18, 23, 0)));
    }

    #[test]
    fn godzina_ma_wiodace_zero() {
        assert_eq!(godzina(dt(2026, 8, 18, 9, 5)), "09:05");
        assert_eq!(godzina(dt(2026, 8, 18, 0, 0)), "00:00");
    }
}
