//! Wyciąganie wydarzeń z kanału iCal, z rozwijaniem reguł powtarzania.
//!
//! Przepływ jest strumieniowy: właściwości lecą jedna po drugiej, `VEVENT`-y są
//! składane w pamięci pojedynczo, a zatrzymywane tylko te, które mogą trafić w okno
//! zainteresowania. Bloki `VTIMEZONE` są pomijane w całości — Google zawsze podaje
//! nazwy stref IANA w `TZID`, a jego definicje `VTIMEZONE` i tak bywają historycznie
//! błędne (obecna reguła rzutowana wstecz aż do 1970).
//!
//! # Co jest tu trudne
//!
//! **`RECURRENCE-ID`.** Crate `rrule` nie obsługuje tego w ogóle. Gdy przesuniesz
//! jedno wystąpienie cyklicznego spotkania, Google emituje osobny `VEVENT` z tym samym
//! `UID` i z `RECURRENCE-ID` wskazującym pierwotny termin. Bez obsługi tego pola
//! przesunięte spotkanie pokazuje się **dwa razy**: w starym i nowym terminie.
//! Tu nadpisania są kubełkowane po `UID` i stosowane po rozwinięciu reguły.
//!
//! **`EXDATE`.** Pojedyncze skasowane wystąpienia. Obsługiwane przez przekazanie
//! do `RRuleSet` razem z regułą.

use std::collections::HashMap;
use std::io::BufRead;

use chrono::{Duration, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use dashboard::model::{CalEvent, SourceTag};
use log::{debug, warn};

use crate::dt::{self, IcalTime};
use crate::parser::{Property, PropertyReader};

/// Maksymalna liczba wystąpień rozwijanych z jednej reguły w oknie.
/// Limit liczy wystąpienia **zachowane**; te przed oknem są iterowane i odrzucane
/// bez konsumowania limitu.
const MAX_OCCURRENCES: u16 = 200;

/// Twardy limit wydarzeń zatrzymywanych w pamięci, żeby patologiczny kanał
/// nie wyczerpał sterty.
const MAX_RETAINED: usize = 2000;

#[derive(Debug)]
pub enum FeedError {
    /// Błąd odczytu strumienia. **Nie** to samo, co pusty kalendarz.
    Io(std::io::Error),
    /// Strumień skończył się bez `END:VCALENDAR` — pobranie było niekompletne.
    Truncated,
}

impl std::fmt::Display for FeedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeedError::Io(e) => write!(f, "błąd odczytu kanału: {e}"),
            FeedError::Truncated => {
                write!(f, "kanał urwany — brak END:VCALENDAR, dane są niekompletne")
            }
        }
    }
}

impl std::error::Error for FeedError {}

/// Okno czasowe, którym się interesujemy.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
}

/// Surowy `VEVENT`, zanim rozwiniemy powtórzenia.
#[derive(Debug, Clone, Default)]
struct RawEvent {
    uid: String,
    start: Option<IcalTime>,
    end: Option<IcalTime>,
    duration_s: Option<i64>,
    summary: String,
    location: Option<String>,
    rrule: Vec<String>,
    exdate: Vec<String>,
    rdate: Vec<String>,
    recurrence_id: Option<IcalTime>,
    cancelled: bool,
    /// Wartość TZID z DTSTART — potrzebna do odtworzenia reguły dla `rrule`.
    start_tzid: Option<String>,
    start_is_date: bool,
}

/// Parsuje kanał i zwraca wydarzenia mieszczące się w oknie, posortowane rosnąco.
pub fn parse_feed<R: BufRead>(
    reader: R,
    window: Window,
    home: Tz,
    tag: SourceTag,
) -> Result<Vec<CalEvent>, FeedError> {
    let mut props = PropertyReader::new(reader);

    let mut masters: Vec<RawEvent> = Vec::new();
    let mut overrides: Vec<RawEvent> = Vec::new();

    let mut current: Option<RawEvent> = None;
    let mut skip_depth = 0usize;
    let mut retained = 0usize;

    for item in props.by_ref() {
        let prop = item.map_err(FeedError::Io)?;

        // Bloki, których nie chcemy, pomijamy razem z zawartością.
        if prop.name == "BEGIN" {
            let component = prop.value.to_ascii_uppercase();
            if skip_depth > 0 {
                skip_depth += 1;
                continue;
            }
            match component.as_str() {
                "VEVENT" => current = Some(RawEvent::default()),
                "VTIMEZONE" | "VALARM" | "VTODO" | "VJOURNAL" | "VFREEBUSY" => skip_depth = 1,
                _ => {}
            }
            continue;
        }

        if skip_depth > 0 {
            if prop.name == "END" {
                skip_depth -= 1;
            }
            continue;
        }

        if prop.name == "END" && prop.value.eq_ignore_ascii_case("VEVENT") {
            if let Some(ev) = current.take() {
                if retained >= MAX_RETAINED {
                    warn!(
                        "osiągnięto limit {MAX_RETAINED} zatrzymanych wydarzeń — reszta pominięta"
                    );
                    continue;
                }
                if ev.recurrence_id.is_some() {
                    overrides.push(ev);
                    retained += 1;
                } else if should_retain(&ev, window) {
                    masters.push(ev);
                    retained += 1;
                }
            }
            continue;
        }

        if let Some(ev) = current.as_mut() {
            absorb(ev, &prop, home);
        }
    }

    if !props.saw_end() {
        return Err(FeedError::Truncated);
    }

    debug!(
        "kanał: {} wydarzeń bazowych, {} nadpisań",
        masters.len(),
        overrides.len()
    );

    let mut out = expand(masters, overrides, window, home, tag);
    out.sort_by_key(|e| (e.start, e.title.clone()));
    Ok(out)
}

/// Czy wydarzenie w ogóle może trafić w okno.
fn should_retain(ev: &RawEvent, window: Window) -> bool {
    // Cykliczne zatrzymujemy zawsze — dopiero rozwinięcie powie, czy trafia.
    if !ev.rrule.is_empty() || !ev.rdate.is_empty() {
        return true;
    }
    let Some(start) = ev.start else { return false };
    let start_dt = start.as_datetime();
    let end_dt = ev.end.map(|e| e.as_datetime()).unwrap_or(start_dt);
    // Nakładanie się przedziałów.
    start_dt < window.end && end_dt >= window.start
}

fn absorb(ev: &mut RawEvent, prop: &Property, home: Tz) {
    let is_date = prop
        .param("VALUE")
        .map(|v| v.eq_ignore_ascii_case("DATE"))
        .unwrap_or(false);
    let tzid = prop.param("TZID");

    match prop.name.as_str() {
        "UID" => ev.uid = prop.value.clone(),
        "SUMMARY" => ev.summary = prop.value.clone(),
        "LOCATION" if !prop.value.trim().is_empty() => ev.location = Some(prop.value.clone()),
        "DTSTART" => {
            ev.start = dt::parse(&prop.value, tzid, is_date, home);
            ev.start_tzid = tzid.map(|s| s.to_string());
            ev.start_is_date = is_date || ev.start.map(|t| t.is_all_day()).unwrap_or(false);
        }
        "DTEND" => ev.end = dt::parse(&prop.value, tzid, is_date, home),
        "DURATION" => ev.duration_s = parse_duration(&prop.value),
        "RRULE" => ev.rrule.push(prop.value.clone()),
        "RDATE" => ev.rdate.push(prop.value.clone()),
        "EXDATE" => ev.exdate.push(prop.value.clone()),
        "RECURRENCE-ID" => ev.recurrence_id = dt::parse(&prop.value, tzid, is_date, home),
        "STATUS" => ev.cancelled = prop.value.eq_ignore_ascii_case("CANCELLED"),
        _ => {}
    }
}

/// Parsuje `DURATION` z RFC 5545, np. `PT1H30M`, `P1D`, `-PT15M`.
fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    let (negative, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let rest = rest.strip_prefix('P')?;

    let mut total = 0i64;
    let mut number = String::new();
    let mut in_time = false;

    for ch in rest.chars() {
        match ch {
            'T' => in_time = true,
            '0'..='9' => number.push(ch),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                total += match (unit, in_time) {
                    ('W', _) => n * 7 * 86400,
                    ('D', _) => n * 86400,
                    ('H', true) => n * 3600,
                    ('M', true) => n * 60,
                    ('S', true) => n,
                    _ => return None,
                };
            }
        }
    }

    if !number.is_empty() {
        return None;
    }
    Some(if negative { -total } else { total })
}

fn expand(
    masters: Vec<RawEvent>,
    overrides: Vec<RawEvent>,
    window: Window,
    home: Tz,
    tag: SourceTag,
) -> Vec<CalEvent> {
    // Nadpisania kubełkowane po UID; klucz wewnętrzny to pierwotny termin wystąpienia.
    let mut by_uid: HashMap<String, Vec<&RawEvent>> = HashMap::new();
    for o in &overrides {
        by_uid.entry(o.uid.clone()).or_default().push(o);
    }

    let mut out = Vec::new();

    for master in &masters {
        let Some(start) = master.start else { continue };
        if master.cancelled {
            continue;
        }

        let duration = event_duration(master);
        let occurrences = occurrences_of(master, start, window, home);

        let overrides_for_uid = by_uid.get(&master.uid);

        for occ_start in occurrences {
            // Czy to wystąpienie zostało nadpisane?
            if let Some(list) = overrides_for_uid {
                if let Some(ovr) = list
                    .iter()
                    .find(|o| o.recurrence_id.map(|r| r.as_datetime()) == Some(occ_start))
                {
                    if ovr.cancelled {
                        continue; // wystąpienie odwołane
                    }
                    if let Some(ev) = build_from_override(ovr, master, window, tag) {
                        out.push(ev);
                    }
                    continue;
                }
            }

            let occ_end = occ_start + Duration::seconds(duration);
            if occ_start >= window.end || occ_end < window.start {
                continue;
            }
            out.push(CalEvent {
                start: occ_start,
                end: occ_end,
                all_day: start.is_all_day(),
                title: title_of(master),
                location: master.location.clone(),
                source: tag,
            });
        }
    }

    // Nadpisania wydarzeń, których bazy nie zatrzymaliśmy (np. baza poza oknem,
    // a przesunięte wystąpienie w oknie) — dodajemy osobno.
    let master_uids: std::collections::HashSet<&str> =
        masters.iter().map(|m| m.uid.as_str()).collect();
    for o in &overrides {
        if master_uids.contains(o.uid.as_str()) || o.cancelled {
            continue;
        }
        if let Some(ev) = build_standalone(o, window, tag) {
            out.push(ev);
        }
    }

    out
}

fn title_of(ev: &RawEvent) -> String {
    if ev.summary.trim().is_empty() {
        "(bez tytułu)".to_string()
    } else {
        ev.summary.clone()
    }
}

fn event_duration(ev: &RawEvent) -> i64 {
    if let Some(d) = ev.duration_s {
        return d.max(0);
    }
    match (ev.start, ev.end) {
        (Some(s), Some(e)) => (e.as_datetime() - s.as_datetime()).num_seconds().max(0),
        // Domyślnie: całodniowe trwa dobę, punktowe zero.
        (Some(s), None) if s.is_all_day() => 86400,
        _ => 0,
    }
}

fn build_from_override(
    ovr: &RawEvent,
    master: &RawEvent,
    window: Window,
    tag: SourceTag,
) -> Option<CalEvent> {
    let start = ovr.start.or(master.start)?;
    let start_dt = start.as_datetime();
    let duration = if ovr.start.is_some() {
        event_duration(ovr)
    } else {
        event_duration(master)
    };
    let end_dt = start_dt + Duration::seconds(duration);

    if start_dt >= window.end || end_dt < window.start {
        return None;
    }

    Some(CalEvent {
        start: start_dt,
        end: end_dt,
        all_day: start.is_all_day(),
        title: if ovr.summary.trim().is_empty() {
            title_of(master)
        } else {
            ovr.summary.clone()
        },
        location: ovr.location.clone().or_else(|| master.location.clone()),
        source: tag,
    })
}

fn build_standalone(ev: &RawEvent, window: Window, tag: SourceTag) -> Option<CalEvent> {
    let start = ev.start?;
    let start_dt = start.as_datetime();
    let end_dt = start_dt + Duration::seconds(event_duration(ev));
    if start_dt >= window.end || end_dt < window.start {
        return None;
    }
    Some(CalEvent {
        start: start_dt,
        end: end_dt,
        all_day: start.is_all_day(),
        title: title_of(ev),
        location: ev.location.clone(),
        source: tag,
    })
}

/// Zwraca terminy rozpoczęcia wystąpień w oknie.
///
/// Dla wydarzeń niecyklicznych to po prostu `DTSTART`. Dla cyklicznych składamy
/// tekst reguły z powrotem i oddajemy go crate'owi `rrule`, ograniczając zakres
/// do okna — dzięki czemu reguła bez `UNTIL`/`COUNT` nie generuje w nieskończoność.
fn occurrences_of(ev: &RawEvent, start: IcalTime, window: Window, home: Tz) -> Vec<NaiveDateTime> {
    if ev.rrule.is_empty() && ev.rdate.is_empty() {
        return vec![start.as_datetime()];
    }

    let Some(text) = rrule_text(ev, start, home) else {
        warn!(
            "nie mogę zbudować reguły dla UID {}, biorę tylko pierwsze wystąpienie",
            ev.uid
        );
        return vec![start.as_datetime()];
    };

    let set: rrule::RRuleSet = match text.parse() {
        Ok(s) => s,
        Err(e) => {
            warn!("nieparsowalna reguła dla UID {}: {e}", ev.uid);
            return vec![start.as_datetime()];
        }
    };

    let rtz = rrule::Tz::Tz(home);
    let after = match rtz.from_local_datetime(&window.start).earliest() {
        Some(d) => d,
        None => return vec![start.as_datetime()],
    };
    let before = match rtz.from_local_datetime(&window.end).earliest() {
        Some(d) => d,
        None => return vec![start.as_datetime()],
    };

    let result = set.after(after).before(before).all(MAX_OCCURRENCES);
    if result.limited {
        warn!(
            "reguła dla UID {} uderzyła w limit {MAX_OCCURRENCES} wystąpień",
            ev.uid
        );
    }

    result.dates.into_iter().map(|d| d.naive_local()).collect()
}

/// Odtwarza blok tekstowy, którego oczekuje `RRuleSet::from_str`.
fn rrule_text(ev: &RawEvent, start: IcalTime, home: Tz) -> Option<String> {
    let start_dt = start.as_datetime();

    // Czas pływający i VALUE=DATE dostają jawnie strefę domową. Bez tego rrule
    // rozwiązuje je do Tz::Local, co na ESP-IDF znaczy UTC — i całodniowe wydarzenia
    // lądują o dzień obok.
    let mut text = format!(
        "DTSTART;TZID={}:{}\n",
        home.name(),
        start_dt.format("%Y%m%dT%H%M%S")
    );

    for r in &ev.rrule {
        text.push_str("RRULE:");
        text.push_str(r);
        text.push('\n');
    }
    for r in &ev.rdate {
        text.push_str("RDATE;TZID=");
        text.push_str(home.name());
        text.push(':');
        text.push_str(r);
        text.push('\n');
    }
    for e in &ev.exdate {
        text.push_str("EXDATE;TZID=");
        text.push_str(home.name());
        text.push(':');
        text.push_str(e);
        text.push('\n');
    }

    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_rfc5545() {
        assert_eq!(parse_duration("PT1H"), Some(3600));
        assert_eq!(parse_duration("PT1H30M"), Some(5400));
        assert_eq!(parse_duration("P1D"), Some(86400));
        assert_eq!(parse_duration("P1W"), Some(604800));
        assert_eq!(parse_duration("PT45S"), Some(45));
        assert_eq!(parse_duration("-PT15M"), Some(-900));
        assert_eq!(parse_duration("P1DT2H30M"), Some(86400 + 9000));
        assert_eq!(parse_duration("nonsens"), None);
        assert_eq!(parse_duration("PT1"), None);
    }
}
