//! Testy integracyjne parsowania kanału na realistycznych plikach iCal.
//!
//! Przypadki odwzorowują to, co faktycznie emituje Google Calendar, w tym rzeczy,
//! na których kod kalendarzowy najczęściej się wykłada: przesunięte wystąpienia
//! cyklicznych spotkań, wykluczone terminy, wydarzenia całodniowe i złamane linie
//! ze znakami wielobajtowymi.

use std::io::Cursor;

use chrono::{NaiveDate, NaiveDateTime};
use dashboard::model::SourceTag;
use icalfeed::{parse_feed, FeedError, Window};

const WARSAW: chrono_tz::Tz = chrono_tz::Europe::Warsaw;

fn dt(y: i32, m: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(y, m, d)
        .unwrap()
        .and_hms_opt(h, mi, 0)
        .unwrap()
}

fn window(from: NaiveDateTime, days: i64) -> Window {
    Window {
        start: from,
        end: from + chrono::Duration::days(days),
    }
}

fn parse(ics: &str, w: Window) -> Vec<dashboard::model::CalEvent> {
    parse_feed(Cursor::new(ics), w, WARSAW, SourceTag::Primary).expect("kanał ma się sparsować")
}

fn wrap(body: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\n\
         CALSCALE:GREGORIAN\r\n{body}END:VCALENDAR\r\n"
    )
}

#[test]
fn pojedyncze_wydarzenie() {
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:abc123@google.com\r\n\
         DTSTART;TZID=Europe/Warsaw:20260818T090000\r\n\
         DTEND;TZID=Europe/Warsaw:20260818T100000\r\n\
         SUMMARY:Stand-up zespołu\r\n\
         LOCATION:Sala Kraków\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 18, 0, 0), 7));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].title, "Stand-up zespołu");
    assert_eq!(events[0].start, dt(2026, 8, 18, 9, 0));
    assert_eq!(events[0].end, dt(2026, 8, 18, 10, 0));
    assert_eq!(events[0].location.as_deref(), Some("Sala Kraków"));
    assert!(!events[0].all_day);
}

#[test]
fn czas_utc_jest_przeliczany_na_warszawe() {
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:utc@x\r\n\
         DTSTART:20260818T070000Z\r\n\
         DTEND:20260818T080000Z\r\n\
         SUMMARY:Z UTC\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 18, 0, 0), 2));
    assert_eq!(events.len(), 1);
    // Sierpień: Warszawa jest UTC+2.
    assert_eq!(events[0].start, dt(2026, 8, 18, 9, 0));
}

#[test]
fn wydarzenie_calodniowe() {
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:allday@x\r\n\
         DTSTART;VALUE=DATE:20260820\r\n\
         DTEND;VALUE=DATE:20260821\r\n\
         SUMMARY:Urlop\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 18, 0, 0), 7));
    assert_eq!(events.len(), 1);
    assert!(
        events[0].all_day,
        "VALUE=DATE musi dać wydarzenie całodniowe"
    );
    assert_eq!(
        events[0].start.date(),
        NaiveDate::from_ymd_opt(2026, 8, 20).unwrap()
    );
}

#[test]
fn cotygodniowe_rozwija_sie_w_oknie() {
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:weekly@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20260803T100000\r\n\
         DTEND;TZID=Europe/Warsaw:20260803T110000\r\n\
         RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
         SUMMARY:Poniedziałkowa retro\r\n\
         END:VEVENT\r\n",
    );
    // Okno obejmuje 4 poniedziałki: 3, 10, 17, 24 sierpnia.
    let events = parse(&ics, window(dt(2026, 8, 1, 0, 0), 28));
    assert_eq!(
        events.len(),
        4,
        "spodziewano się czterech poniedziałków, jest {}",
        events.len()
    );
    for e in &events {
        assert_eq!(e.title, "Poniedziałkowa retro");
        assert_eq!(
            e.start.time(),
            chrono::NaiveTime::from_hms_opt(10, 0, 0).unwrap()
        );
    }
}

#[test]
fn regula_bez_konca_nie_generuje_w_nieskonczonosc() {
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:daily@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20200101T080000\r\n\
         DTEND;TZID=Europe/Warsaw:20200101T081500\r\n\
         RRULE:FREQ=DAILY\r\n\
         SUMMARY:Codzienna\r\n\
         END:VEVENT\r\n",
    );
    // Reguła bez UNTIL/COUNT, startująca sześć lat wstecz. Okno ma siedem dni.
    let events = parse(&ics, window(dt(2026, 8, 18, 0, 0), 7));
    assert_eq!(
        events.len(),
        7,
        "okno tygodniowe ma dać siedem wystąpień, nie {}",
        events.len()
    );
}

#[test]
fn exdate_usuwa_wystapienie() {
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:weekly@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20260803T100000\r\n\
         DTEND;TZID=Europe/Warsaw:20260803T110000\r\n\
         RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
         EXDATE;TZID=Europe/Warsaw:20260817T100000\r\n\
         SUMMARY:Retro\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 1, 0, 0), 28));
    assert_eq!(
        events.len(),
        3,
        "EXDATE ma usunąć jedno z czterech wystąpień"
    );
    assert!(
        !events
            .iter()
            .any(|e| e.start.date() == NaiveDate::from_ymd_opt(2026, 8, 17).unwrap()),
        "wykluczony termin nadal jest na liście"
    );
}

#[test]
fn recurrence_id_przesuwa_wystapienie_zamiast_dublowac() {
    // To jest przypadek, na którym wykłada się większość implementacji: przesunięte
    // spotkanie pokazuje się dwa razy — w starym i w nowym terminie.
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:weekly@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20260803T100000\r\n\
         DTEND;TZID=Europe/Warsaw:20260803T110000\r\n\
         RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
         SUMMARY:Retro\r\n\
         END:VEVENT\r\n\
         BEGIN:VEVENT\r\n\
         UID:weekly@x\r\n\
         RECURRENCE-ID;TZID=Europe/Warsaw:20260817T100000\r\n\
         DTSTART;TZID=Europe/Warsaw:20260817T140000\r\n\
         DTEND;TZID=Europe/Warsaw:20260817T150000\r\n\
         SUMMARY:Retro (przesunięta)\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 1, 0, 0), 28));

    assert_eq!(
        events.len(),
        4,
        "przesunięcie nie może zmieniać liczby wystąpień"
    );

    let on_17: Vec<_> = events
        .iter()
        .filter(|e| e.start.date() == NaiveDate::from_ymd_opt(2026, 8, 17).unwrap())
        .collect();
    assert_eq!(
        on_17.len(),
        1,
        "17 sierpnia ma być dokładnie jedno wystąpienie, jest {}",
        on_17.len()
    );
    assert_eq!(
        on_17[0].start,
        dt(2026, 8, 17, 14, 0),
        "ma być w nowym terminie"
    );
    assert_eq!(on_17[0].title, "Retro (przesunięta)");
}

#[test]
fn recurrence_id_z_status_cancelled_usuwa_wystapienie() {
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:weekly@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20260803T100000\r\n\
         DTEND;TZID=Europe/Warsaw:20260803T110000\r\n\
         RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
         SUMMARY:Retro\r\n\
         END:VEVENT\r\n\
         BEGIN:VEVENT\r\n\
         UID:weekly@x\r\n\
         RECURRENCE-ID;TZID=Europe/Warsaw:20260817T100000\r\n\
         DTSTART;TZID=Europe/Warsaw:20260817T100000\r\n\
         STATUS:CANCELLED\r\n\
         SUMMARY:Retro\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 1, 0, 0), 28));
    assert_eq!(events.len(), 3, "odwołane wystąpienie ma zniknąć");
    assert!(!events
        .iter()
        .any(|e| e.start.date() == NaiveDate::from_ymd_opt(2026, 8, 17).unwrap()));
}

#[test]
fn zlamane_linie_z_polskimi_znakami() {
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:folded@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20260818T090000\r\n\
         DTEND;TZID=Europe/Warsaw:20260818T100000\r\n\
         SUMMARY:Śniadanie z Łukaszem i omówienie ćwicz\r\n eń na przyszły tydzi\r\n eń\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 18, 0, 0), 2));
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].title,
        "Śniadanie z Łukaszem i omówienie ćwiczeń na przyszły tydzień"
    );
}

#[test]
fn vtimezone_i_valarm_sa_pomijane() {
    let ics = wrap(
        "BEGIN:VTIMEZONE\r\n\
         TZID:Europe/Warsaw\r\n\
         BEGIN:DAYLIGHT\r\n\
         TZOFFSETFROM:+0100\r\n\
         TZOFFSETTO:+0200\r\n\
         DTSTART:19700329T020000\r\n\
         SUMMARY:To nie jest wydarzenie\r\n\
         END:DAYLIGHT\r\n\
         END:VTIMEZONE\r\n\
         BEGIN:VEVENT\r\n\
         UID:withalarm@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20260818T090000\r\n\
         DTEND;TZID=Europe/Warsaw:20260818T100000\r\n\
         SUMMARY:Prawdziwe wydarzenie\r\n\
         BEGIN:VALARM\r\n\
         ACTION:DISPLAY\r\n\
         SUMMARY:Przypomnienie\r\n\
         TRIGGER:-PT10M\r\n\
         END:VALARM\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 18, 0, 0), 2));
    assert_eq!(events.len(), 1, "tylko VEVENT ma się liczyć");
    assert_eq!(
        events[0].title, "Prawdziwe wydarzenie",
        "SUMMARY z VALARM nie może nadpisać tytułu"
    );
}

#[test]
fn wydarzenia_poza_oknem_sa_odrzucane() {
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:past@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20200101T090000\r\n\
         DTEND;TZID=Europe/Warsaw:20200101T100000\r\n\
         SUMMARY:Dawno temu\r\n\
         END:VEVENT\r\n\
         BEGIN:VEVENT\r\n\
         UID:future@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20300101T090000\r\n\
         DTEND;TZID=Europe/Warsaw:20300101T100000\r\n\
         SUMMARY:Kiedyś\r\n\
         END:VEVENT\r\n\
         BEGIN:VEVENT\r\n\
         UID:now@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20260819T090000\r\n\
         DTEND;TZID=Europe/Warsaw:20260819T100000\r\n\
         SUMMARY:W oknie\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 18, 0, 0), 7));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].title, "W oknie");
}

#[test]
fn duration_zamiast_dtend() {
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:dur@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20260818T090000\r\n\
         DURATION:PT1H30M\r\n\
         SUMMARY:Z czasem trwania\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 18, 0, 0), 2));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].end, dt(2026, 8, 18, 10, 30));
}

#[test]
fn urwany_kanal_jest_bledem_a_nie_pustym_kalendarzem() {
    // Bez END:VCALENDAR — dokładnie to, co zostaje po urwanym połączeniu.
    let ics = "BEGIN:VCALENDAR\r\n\
               BEGIN:VEVENT\r\n\
               UID:x@x\r\n\
               DTSTART;TZID=Europe/Warsaw:20260818T090000\r\n\
               SUMMARY:Cokolwiek\r\n\
               END:VEVENT\r\n";
    let res = parse_feed(
        Cursor::new(ics),
        window(dt(2026, 8, 18, 0, 0), 7),
        WARSAW,
        SourceTag::Primary,
    );
    assert!(
        matches!(res, Err(FeedError::Truncated)),
        "urwane pobranie musi być błędem, nie cichym pustym wynikiem"
    );
}

#[test]
fn pusty_kalendarz_jest_poprawny() {
    let events = parse(&wrap(""), window(dt(2026, 8, 18, 0, 0), 7));
    assert!(events.is_empty());
}

#[test]
fn wydarzenia_sa_posortowane_rosnaco() {
    let ics = wrap(
        "BEGIN:VEVENT\r\nUID:c@x\r\nDTSTART;TZID=Europe/Warsaw:20260820T090000\r\n\
         DTEND;TZID=Europe/Warsaw:20260820T100000\r\nSUMMARY:Trzecie\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:a@x\r\nDTSTART;TZID=Europe/Warsaw:20260818T090000\r\n\
         DTEND;TZID=Europe/Warsaw:20260818T100000\r\nSUMMARY:Pierwsze\r\nEND:VEVENT\r\n\
         BEGIN:VEVENT\r\nUID:b@x\r\nDTSTART;TZID=Europe/Warsaw:20260819T090000\r\n\
         DTEND;TZID=Europe/Warsaw:20260819T100000\r\nSUMMARY:Drugie\r\nEND:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 18, 0, 0), 7));
    let titles: Vec<&str> = events.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, vec!["Pierwsze", "Drugie", "Trzecie"]);
}

#[test]
fn brak_tytulu_dostaje_zastepnik() {
    let ics = wrap(
        "BEGIN:VEVENT\r\nUID:notitle@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20260818T090000\r\n\
         DTEND;TZID=Europe/Warsaw:20260818T100000\r\nEND:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 8, 18, 0, 0), 2));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].title, "(bez tytułu)");
}

#[test]
fn zmiana_czasu_letniego_na_zimowy() {
    // Cotygodniowe spotkanie przechodzące przez zmianę czasu (25 października 2026).
    // Godzina lokalna ma zostać ta sama po obu stronach zmiany.
    let ics = wrap(
        "BEGIN:VEVENT\r\n\
         UID:dst@x\r\n\
         DTSTART;TZID=Europe/Warsaw:20261019T100000\r\n\
         DTEND;TZID=Europe/Warsaw:20261019T110000\r\n\
         RRULE:FREQ=WEEKLY;BYDAY=MO\r\n\
         SUMMARY:Przez zmianę czasu\r\n\
         END:VEVENT\r\n",
    );
    let events = parse(&ics, window(dt(2026, 10, 15, 0, 0), 21));
    assert!(events.len() >= 3);
    for e in &events {
        assert_eq!(
            e.start.time(),
            chrono::NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
            "godzina lokalna ma zostać 10:00 po obu stronach zmiany czasu, jest {} dnia {}",
            e.start.time(),
            e.start.date()
        );
    }
}
