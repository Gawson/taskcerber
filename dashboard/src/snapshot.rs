//! Migawka kalendarza — to, co urządzenie ma pokazać, gdy nie sięgnęło po sieć.
//!
//! # Dlaczego to w ogóle musi istnieć
//!
//! Wydarzenia żyły dotąd wyłącznie w RAM-ie, a deep sleep gasi RAM. Każde wybudzenie
//! musiało więc pobrać kalendarz od nowa, inaczej ekran byłby pusty — i to jest korzeń
//! trzech osobnych problemów naraz:
//!
//! * **Wybudzenie dotykiem trwało kilkanaście sekund.** Zanim panel w ogóle powstał,
//!   trzeba było przepchnąć 1,18 MB przez radio. Człowiek stał przed urządzeniem,
//!   które go ignorowało.
//! * **Bez kabla nie dało się nic pokazać.** Wypięcie kabla kasowało treść.
//! * **Pominięcie pobrania kasowało ekran.** Każdy warunek w rodzaju „dane są świeże,
//!   nie pobieraj" dawał pusty model zamiast oszczędności.
//!
//! Migawka rozcina ten węzeł: treść przeżywa sen, więc pobranie staje się czynnością
//! **odświeżającą**, a nie warunkiem narysowania czegokolwiek.
//!
//! # Format jest wersjonowany i celowo ręczny
//!
//! Bajty lądują w NVS i czyta je następna wersja firmware'u, więc format jest częścią
//! umowy, a nie szczegółem. Pierwszy bajt to numer wersji; przy niezgodności migawkę
//! **odrzucamy w całości** zamiast zgadywać — stara treść pokazana jako świeża jest
//! gorsza niż jej brak.
//!
//! Ręczny zapis, a nie `serde`, bo idzie do pamięci liczonej w kilobajtach i zapisywanej
//! z ograniczoną liczbą cykli. JSON tej samej treści jest kilkukrotnie większy, a jedyne,
//! co by dał, to wygoda pisania — raz.
//!
//! # Czasy jako sekundy epoki, mimo że są naiwne
//!
//! [`chrono::NaiveDateTime`] nie ma strefy, ale ma jednoznaczne odwzorowanie na liczbę
//! sekund, jeśli konsekwentnie czytać ją jako UTC. Zapis i odczyt robią dokładnie to,
//! więc obieg jest bezstratny — a nie płacimy za przechowywanie strefy, która i tak
//! jest jedna na całe urządzenie.

use chrono::{DateTime, NaiveDate, NaiveDateTime};

use crate::model::{CalEvent, SourceTag};

/// Numer wersji formatu. Bump przy KAŻDEJ zmianie układu bajtów.
pub const WERSJA: u8 = 2;

/// Górny limit długości tekstu w bajtach.
///
/// Tytuł i tak jest przycinany przy rysowaniu — najszersza kratka mieści kilkadziesiąt
/// znaków. Limit chroni przed kanałem, w którym ktoś wkleił do opisu całą umowę.
const MAX_TEKST: usize = 200;

/// Górny limit liczby wydarzeń w migawce.
///
/// Przy horyzoncie czternastu dni realny kalendarz mieści się w kilkudziesięciu.
/// Limit jest zabezpieczeniem przed rozdęciem zapisu, a nie oczekiwaniem.
const MAX_WYDARZEN: usize = 400;

/// Wszystko, co ekran potrzebuje wiedzieć bez sięgania po sieć.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub events: Vec<CalEvent>,
    pub holidays: Vec<NaiveDate>,
    /// Zakres dni, o które pytał kanał z treścią.
    pub known: Option<(NaiveDate, NaiveDate)>,
    /// Zakres dni, o które pytał kanał świąt — osobno, bo ma inny horyzont.
    pub known_holidays: Option<(NaiveDate, NaiveDate)>,
    /// Kiedy ta treść została pobrana. `None` = nie wiadomo.
    ///
    /// Znacznik mieszka W MIGAWCE, a nie w pamięci RTC, i to jest poprawka konkretnej
    /// pomyłki: `RtcState` nie przeżywa resetu innego niż wybudzenie z deep sleepu,
    /// więc po każdym restarcie przez USB pole zerowało się i ekran pokazywał „z 01:00",
    /// czyli epokę. Wiek treści jest własnością treści, więc leży tam, gdzie ona.
    pub saved_at: Option<NaiveDateTime>,
}

fn tag_na_bity(t: SourceTag) -> u8 {
    match t {
        SourceTag::Primary => 0,
        SourceTag::Secondary => 1,
        SourceTag::Holiday => 2,
    }
}

fn bity_na_tag(b: u8) -> SourceTag {
    match b {
        1 => SourceTag::Secondary,
        2 => SourceTag::Holiday,
        _ => SourceTag::Primary,
    }
}

fn sekundy(t: NaiveDateTime) -> i64 {
    t.and_utc().timestamp()
}

fn z_sekund(s: i64) -> Option<NaiveDateTime> {
    DateTime::from_timestamp(s, 0).map(|d| d.naive_utc())
}

/// Przycina tekst do `MAX_TEKST` bajtów, nie łamiąc znaku wielobajtowego.
fn przytnij(s: &str) -> &str {
    if s.len() <= MAX_TEKST {
        return s;
    }
    let mut i = MAX_TEKST;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

fn wpisz_tekst(out: &mut Vec<u8>, s: &str) {
    let s = przytnij(s);
    out.push(s.len() as u8);
    out.extend_from_slice(s.as_bytes());
}

fn wpisz_zakres(out: &mut Vec<u8>, z: Option<(NaiveDate, NaiveDate)>) {
    match z {
        // Zero jako „brak" jest bezpieczne, bo prawdziwa data 1970-01-01 nie wystąpi
        // w kalendarzu, który pobieramy na czternaście dni do przodu.
        None => {
            out.extend_from_slice(&0i64.to_le_bytes());
            out.extend_from_slice(&0i64.to_le_bytes());
        }
        Some((a, b)) => {
            out.extend_from_slice(&dzien_na_sekundy(a).to_le_bytes());
            out.extend_from_slice(&dzien_na_sekundy(b).to_le_bytes());
        }
    }
}

fn dzien_na_sekundy(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0).map(sekundy).unwrap_or(0)
}

/// Zapisuje migawkę do bajtów.
pub fn encode(s: &Snapshot) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + s.events.len() * 80);
    out.push(WERSJA);

    // Zero znaczy „nie wiadomo": prawdziwa data pobrania nie wypadnie w 1970 roku.
    out.extend_from_slice(&s.saved_at.map(sekundy).unwrap_or(0).to_le_bytes());

    wpisz_zakres(&mut out, s.known);
    wpisz_zakres(&mut out, s.known_holidays);

    let swieta: Vec<&NaiveDate> = s.holidays.iter().take(u16::MAX as usize).collect();
    out.extend_from_slice(&(swieta.len() as u16).to_le_bytes());
    for d in swieta {
        out.extend_from_slice(&dzien_na_sekundy(*d).to_le_bytes());
    }

    let wyd: Vec<&CalEvent> = s.events.iter().take(MAX_WYDARZEN).collect();
    out.extend_from_slice(&(wyd.len() as u16).to_le_bytes());
    for e in wyd {
        out.extend_from_slice(&sekundy(e.start).to_le_bytes());
        out.extend_from_slice(&sekundy(e.end).to_le_bytes());
        let flagi = (e.all_day as u8) | (tag_na_bity(e.source) << 1);
        out.push(flagi);
        wpisz_tekst(&mut out, &e.title);
        match &e.location {
            Some(l) => wpisz_tekst(&mut out, l),
            None => out.push(0),
        }
    }
    out
}

/// Czytnik bajtów, który nigdy nie panikuje na obciętym wejściu.
struct Czytnik<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Czytnik<'a> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.i)?;
        self.i += 1;
        Some(v)
    }
    fn u16(&mut self) -> Option<u16> {
        let s = self.b.get(self.i..self.i + 2)?;
        self.i += 2;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }
    fn i64(&mut self) -> Option<i64> {
        let s = self.b.get(self.i..self.i + 8)?;
        self.i += 8;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Some(i64::from_le_bytes(a))
    }
    fn tekst(&mut self) -> Option<String> {
        let n = self.u8()? as usize;
        let s = self.b.get(self.i..self.i + n)?;
        self.i += n;
        Some(String::from_utf8_lossy(s).into_owned())
    }
    fn zakres(&mut self) -> Option<Option<(NaiveDate, NaiveDate)>> {
        let a = self.i64()?;
        let b = self.i64()?;
        if a == 0 || b == 0 {
            return Some(None);
        }
        let (a, b) = (z_sekund(a)?.date(), z_sekund(b)?.date());
        Some(Some((a, b)))
    }
}

/// Odczytuje migawkę. `None`, gdy bajty są z innej wersji albo obcięte.
///
/// Obcięcie jest realne, nie teoretyczne: NVS potrafi oddać krótszy blob, gdy zapis
/// przerwał reset. Dlatego czytnik sprawdza granice przy każdym polu, zamiast ufać
/// deklarowanym licznikom.
pub fn decode(bytes: &[u8]) -> Option<Snapshot> {
    let mut c = Czytnik { b: bytes, i: 0 };
    if c.u8()? != WERSJA {
        return None;
    }

    let saved_at = match c.i64()? {
        0 => None,
        v => z_sekund(v),
    };
    let known = c.zakres()?;
    let known_holidays = c.zakres()?;

    let ile_swiat = c.u16()? as usize;
    let mut holidays = Vec::with_capacity(ile_swiat.min(400));
    for _ in 0..ile_swiat {
        holidays.push(z_sekund(c.i64()?)?.date());
    }

    let ile = c.u16()? as usize;
    if ile > MAX_WYDARZEN {
        return None;
    }
    let mut events = Vec::with_capacity(ile);
    for _ in 0..ile {
        let start = z_sekund(c.i64()?)?;
        let end = z_sekund(c.i64()?)?;
        let flagi = c.u8()?;
        let title = c.tekst()?;
        let loc = c.tekst()?;
        events.push(CalEvent {
            start,
            end,
            all_day: flagi & 1 != 0,
            title,
            location: if loc.is_empty() { None } else { Some(loc) },
            source: bity_na_tag((flagi >> 1) & 0b11),
        });
    }

    Some(Snapshot {
        events,
        holidays,
        known,
        known_holidays,
        saved_at,
    })
}

/// Czy migawka opisuje dzień, który jeszcze nie minął.
///
/// Migawka sprzed tygodnia jest gorsza niż pusty ekran: pokazuje spotkania, które
/// się odbyły, jako nadchodzące. Sprawdzamy `known`, a nie daty wydarzeń — pusty
/// weekend w środku horyzontu nie znaczy, że migawka jest stara.
pub fn wciaz_uzyteczna(s: &Snapshot, dzis: NaiveDate) -> bool {
    match s.known {
        Some((_, do_kiedy)) => do_kiedy >= dzis,
        None => false,
    }
}

/// Ile dni migawki dotyczy przyszłości — do pokazania, jak bardzo treść się zestarzała.
pub fn dni_zapasu(s: &Snapshot, dzis: NaiveDate) -> i64 {
    match s.known {
        Some((_, do_kiedy)) => (do_kiedy - dzis).num_days().max(0),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(d: u32, h: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, d)
            .unwrap()
            .and_hms_opt(h, 0, 0)
            .unwrap()
    }

    fn przyklad() -> Snapshot {
        Snapshot {
            events: vec![
                CalEvent {
                    start: dt(18, 9),
                    end: dt(18, 10),
                    all_day: false,
                    title: "Spotkanie z zespołem — ąćęłńóśźż".into(),
                    location: Some("sala 3".into()),
                    source: SourceTag::Primary,
                },
                CalEvent {
                    start: dt(20, 0),
                    end: dt(20, 23),
                    all_day: true,
                    title: "Urlop".into(),
                    location: None,
                    source: SourceTag::Secondary,
                },
            ],
            holidays: vec![
                NaiveDate::from_ymd_opt(2026, 8, 15).unwrap(),
                NaiveDate::from_ymd_opt(2026, 11, 11).unwrap(),
            ],
            known: Some((
                NaiveDate::from_ymd_opt(2026, 8, 18).unwrap(),
                NaiveDate::from_ymd_opt(2026, 8, 31).unwrap(),
            )),
            known_holidays: Some((
                NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
                NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
            )),
            saved_at: Some(dt(18, 7)),
        }
    }

    #[test]
    fn obieg_jest_bezstratny() {
        let s = przyklad();
        let odczyt = decode(&encode(&s)).expect("migawka ma się odczytać");
        assert_eq!(odczyt, s, "zapis i odczyt muszą dać to samo");
    }

    #[test]
    fn pusta_migawka_tez_przezywa() {
        let s = Snapshot::default();
        assert_eq!(decode(&encode(&s)), Some(s));
    }

    /// NVS potrafi oddać krótszy blob, gdy zapis przerwał reset. Żaden prefiks
    /// nie ma prawa spanikować ani zwrócić wydarzeń wziętych z powietrza.
    #[test]
    fn obciety_zapis_nie_panikuje() {
        let bajty = encode(&przyklad());
        for n in 0..bajty.len() {
            let wynik = decode(&bajty[..n]);
            if let Some(s) = wynik {
                // Prefiks może być poprawną, KRÓTSZĄ migawką tylko wtedy, gdy urwał
                // się dokładnie na granicy — nigdy nie może udawać pełnej.
                assert!(s.events.len() <= przyklad().events.len());
            }
        }
    }

    #[test]
    fn inna_wersja_jest_odrzucana_w_calosci() {
        let mut b = encode(&przyklad());
        b[0] = WERSJA.wrapping_add(1);
        assert_eq!(decode(&b), None, "migawka z innej wersji nie może wejść");
    }

    #[test]
    fn dlugi_tytul_jest_przycinany_na_granicy_znaku() {
        let mut s = Snapshot::default();
        s.events.push(CalEvent {
            start: dt(18, 9),
            end: dt(18, 10),
            all_day: false,
            // Same znaki dwubajtowe — przycięcie w złym miejscu dałoby niepoprawny UTF-8.
            title: "ą".repeat(300),
            location: None,
            source: SourceTag::Primary,
        });
        let odczyt = decode(&encode(&s)).expect("ma się odczytać");
        let t = &odczyt.events[0].title;
        assert!(t.len() <= MAX_TEKST, "przycięte do {} B", t.len());
        assert!(t.chars().all(|c| c == 'ą'), "bez znaków zastępczych: {t}");
    }

    #[test]
    fn migawka_z_przeszlosci_jest_bezuzyteczna() {
        let s = przyklad();
        let w_horyzoncie = NaiveDate::from_ymd_opt(2026, 8, 20).unwrap();
        let po_horyzoncie = NaiveDate::from_ymd_opt(2026, 9, 5).unwrap();
        assert!(wciaz_uzyteczna(&s, w_horyzoncie));
        assert!(!wciaz_uzyteczna(&s, po_horyzoncie), "tydzień po — do kosza");
        assert_eq!(dni_zapasu(&s, w_horyzoncie), 11);
        assert_eq!(dni_zapasu(&s, po_horyzoncie), 0);
    }

    /// Rozmiar musi się mieścić w NVS z zapasem — partycja ma 128 KB na wszystko.
    #[test]
    fn realny_kalendarz_miesci_sie_w_kilku_kilobajtach() {
        let mut s = Snapshot::default();
        for i in 0..60 {
            s.events.push(CalEvent {
                start: dt(18, 9),
                end: dt(18, 10),
                all_day: false,
                title: format!("Spotkanie numer {i} z dosyć długim tytułem"),
                location: Some("Sala konferencyjna na drugim piętrze".into()),
                source: SourceTag::Primary,
            });
        }
        let n = encode(&s).len();
        assert!(
            n < 8 * 1024,
            "60 wydarzeń zajęło {n} B, oczekiwano poniżej 8 KB"
        );
    }
}
