//! Parsowanie wartości daty i czasu z iCalendar oraz rozwiązywanie stref czasowych.
//!
//! To jest miejsce, w którym mieszka większość błędów w kodzie kalendarzowym.
//! Trzy przypadki, które trzeba rozróżniać:
//!
//! | Postać | Znaczenie |
//! |---|---|
//! | `20260818T090000Z` | czas UTC |
//! | `DTSTART;TZID=Europe/Warsaw:20260818T090000` | czas w podanej strefie |
//! | `DTSTART:20260818T090000` | czas „pływający" — lokalny wszędzie |
//! | `DTSTART;VALUE=DATE:20260818` | wydarzenie całodniowe |
//!
//! Czas pływający i `VALUE=DATE` **muszą** zostać przypisane do strefy domowej,
//! zanim trafią do rozwijania reguł powtarzania. Bez tego rozwiązują się do
//! `rrule::Tz::Local`, co na ESP-IDF oznacza UTC — i każde urodziny lądują o dzień
//! obok.

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use chrono_tz::Tz;

/// Wartość czasu wyciągnięta z właściwości iCal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IcalTime {
    /// Konkretna chwila, już przeliczona na strefę domową.
    DateTime(NaiveDateTime),
    /// Data bez godziny — wydarzenie całodniowe.
    Date(NaiveDate),
}

impl IcalTime {
    /// Reprezentacja jako `NaiveDateTime` w strefie domowej; dla dat to północ.
    pub fn as_datetime(&self) -> NaiveDateTime {
        match self {
            IcalTime::DateTime(dt) => *dt,
            IcalTime::Date(d) => d.and_time(NaiveTime::MIN),
        }
    }

    pub fn is_all_day(&self) -> bool {
        matches!(self, IcalTime::Date(_))
    }
}

/// Parsuje wartość `DTSTART` / `DTEND` / `RECURRENCE-ID`.
///
/// * `value` — surowa wartość po dwukropku.
/// * `tzid` — zawartość parametru `TZID`, jeśli był.
/// * `is_date` — czy parametr `VALUE` miał wartość `DATE`.
/// * `home` — strefa domowa urządzenia; do niej przeliczane są wszystkie wyniki.
pub fn parse(value: &str, tzid: Option<&str>, is_date: bool, home: Tz) -> Option<IcalTime> {
    let value = value.trim();

    // Postać samej daty: 20260818
    if is_date || (value.len() == 8 && !value.contains('T')) {
        return parse_date(value).map(IcalTime::Date);
    }

    let (naive, utc) = parse_datetime(value)?;

    if utc {
        // Zapisane w UTC — przeliczamy na strefę domową.
        let dt = chrono_tz::UTC
            .from_utc_datetime(&naive)
            .with_timezone(&home);
        return Some(IcalTime::DateTime(dt.naive_local()));
    }

    match tzid.and_then(resolve_tz) {
        Some(tz) if tz != home => {
            // Czas w innej strefie — przez UTC do strefy domowej.
            let in_tz = tz.from_local_datetime(&naive).earliest()?;
            Some(IcalTime::DateTime(in_tz.with_timezone(&home).naive_local()))
        }
        // Brak TZID (czas pływający) albo TZID równy strefie domowej —
        // wartość jest już czasem lokalnym.
        _ => Some(IcalTime::DateTime(naive)),
    }
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    if s.len() < 8 {
        return None;
    }
    let year: i32 = s[0..4].parse().ok()?;
    let month: u32 = s[4..6].parse().ok()?;
    let day: u32 = s[6..8].parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// Zwraca `(czas, czy_utc)`.
fn parse_datetime(s: &str) -> Option<(NaiveDateTime, bool)> {
    let utc = s.ends_with('Z');
    let core = if utc { &s[..s.len() - 1] } else { s };

    let (date_part, time_part) = core.split_once('T')?;
    let date = parse_date(date_part)?;

    if time_part.len() < 6 {
        return None;
    }
    let hour: u32 = time_part[0..2].parse().ok()?;
    let min: u32 = time_part[2..4].parse().ok()?;
    let sec: u32 = time_part[4..6].parse().ok()?;

    // Sekundy przestępne (60) obcinamy do 59 — chrono ich nie przyjmie,
    // a różnica jest bez znaczenia dla kalendarza.
    let sec = sec.min(59);

    date.and_hms_opt(hour, min, sec).map(|dt| (dt, utc))
}

/// Rozwiązuje nazwę strefy z `TZID`.
///
/// Google zawsze emituje nazwy IANA (`Europe/Warsaw`), więc podstawowa ścieżka jest
/// prosta. Kalendarze z Outlooka/Exchange emitują nazwy windowsowe
/// (`Central European Standard Time`), których `chrono_tz` nie rozpozna — dla nich
/// jest mała tablica najczęstszych przypadków. Nierozpoznana strefa zwraca `None`,
/// a wołający traktuje wartość jako czas pływający.
pub fn resolve_tz(tzid: &str) -> Option<Tz> {
    let tzid = tzid.trim().trim_matches('"');

    if let Ok(tz) = tzid.parse::<Tz>() {
        return Some(tz);
    }

    // Kilka nazw windowsowych, które realnie się trafiają.
    let mapped = match tzid {
        "Central European Standard Time" | "Central Europe Standard Time" => "Europe/Warsaw",
        "W. Europe Standard Time" => "Europe/Berlin",
        "Romance Standard Time" => "Europe/Paris",
        "GMT Standard Time" => "Europe/London",
        "UTC" | "Coordinated Universal Time" => "UTC",
        "E. Europe Standard Time" | "FLE Standard Time" => "Europe/Kiev",
        "Eastern Standard Time" => "America/New_York",
        "Pacific Standard Time" => "America/Los_Angeles",
        _ => return None,
    };
    mapped.parse::<Tz>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const WARSAW: Tz = chrono_tz::Europe::Warsaw;

    fn ndt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    #[test]
    fn czas_utc_przeliczany_na_strefe_domowa() {
        // Sierpień = czas letni w Warszawie, UTC+2.
        let t = parse("20260818T070000Z", None, false, WARSAW).unwrap();
        assert_eq!(t, IcalTime::DateTime(ndt(2026, 8, 18, 9, 0)));
    }

    #[test]
    fn czas_utc_zima_ma_inne_przesuniecie() {
        // Styczeń = czas zimowy, UTC+1.
        let t = parse("20260115T070000Z", None, false, WARSAW).unwrap();
        assert_eq!(t, IcalTime::DateTime(ndt(2026, 1, 15, 8, 0)));
    }

    #[test]
    fn tzid_zgodny_ze_strefa_domowa_jest_brany_wprost() {
        let t = parse("20260818T090000", Some("Europe/Warsaw"), false, WARSAW).unwrap();
        assert_eq!(t, IcalTime::DateTime(ndt(2026, 8, 18, 9, 0)));
    }

    #[test]
    fn tzid_innej_strefy_jest_przeliczany() {
        // Londyn latem to UTC+1, Warszawa UTC+2 — różnica godziny.
        let t = parse("20260818T090000", Some("Europe/London"), false, WARSAW).unwrap();
        assert_eq!(t, IcalTime::DateTime(ndt(2026, 8, 18, 10, 0)));
    }

    #[test]
    fn czas_plywajacy_jest_lokalny() {
        let t = parse("20260818T090000", None, false, WARSAW).unwrap();
        assert_eq!(t, IcalTime::DateTime(ndt(2026, 8, 18, 9, 0)));
    }

    #[test]
    fn wartosc_date_daje_wydarzenie_calodniowe() {
        let t = parse("20260818", None, true, WARSAW).unwrap();
        assert_eq!(
            t,
            IcalTime::Date(NaiveDate::from_ymd_opt(2026, 8, 18).unwrap())
        );
        assert!(t.is_all_day());
        // Wykrywane też bez jawnego VALUE=DATE, po samej długości.
        let t = parse("20260818", None, false, WARSAW).unwrap();
        assert!(t.is_all_day());
    }

    #[test]
    fn nazwy_windowsowe_sa_mapowane() {
        assert_eq!(resolve_tz("Central European Standard Time"), Some(WARSAW));
        assert_eq!(resolve_tz("Europe/Warsaw"), Some(WARSAW));
        assert_eq!(resolve_tz("\"Europe/Warsaw\""), Some(WARSAW));
        assert_eq!(resolve_tz("Zupelnie Wymyslona Strefa"), None);
    }

    #[test]
    fn nierozpoznana_strefa_traktowana_jak_czas_plywajacy() {
        let t = parse("20260818T090000", Some("Wymyslona/Strefa"), false, WARSAW).unwrap();
        assert_eq!(t, IcalTime::DateTime(ndt(2026, 8, 18, 9, 0)));
    }

    #[test]
    fn smieci_nie_panikuja() {
        assert!(parse("", None, false, WARSAW).is_none());
        assert!(parse("nonsens", None, false, WARSAW).is_none());
        assert!(parse("2026", None, false, WARSAW).is_none());
        assert!(parse("20261332T990000Z", None, false, WARSAW).is_none());
        assert!(parse("20260818T99", None, false, WARSAW).is_none());
    }

    #[test]
    fn sekundy_przestepne_sa_obcinane() {
        let t = parse("20260818T235960Z", None, false, WARSAW);
        assert!(
            t.is_some(),
            "sekunda przestępna nie może wywalić parsowania"
        );
    }
}
