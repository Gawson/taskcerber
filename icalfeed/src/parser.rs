//! Strumieniowy parser właściwości iCalendar (RFC 5545).
//!
//! Świadomie własny, a nie crate `ical`, z trzech powodów:
//!
//! 1. **Zużycie pamięci musi być stałe.** Kanał iCal Google potrafi mieć setki
//!    kilobajtów; urządzenie ma 8 MB PSRAM, ale wewnętrzny DRAM jest ciasny i dzielony
//!    z buforami mbedTLS. Ten parser trzyma jedną rozwiniętą linię naraz.
//! 2. **Błędy wejścia nie mogą wyglądać jak koniec pliku.** Parser `ical` w
//!    `LineReader::next_line` robi `let line = line.ok()?;` — połknie każdy błąd I/O
//!    i zgłosi go jako czysty EOF. Urwane pobieranie wygląda wtedy dokładnie jak
//!    kompletny kalendarz, co daje ciche gubienie wydarzeń. Tutaj błąd jest błędem.
//! 3. Testowalność na hoście, bez ESP-IDF.
//!
//! # Składanie linii (unfolding)
//! RFC 5545 §3.1: linia dłuższa niż 75 oktetów jest łamana, a kontynuacja zaczyna się
//! pojedynczą spacją lub tabulatorem. Składamy je z powrotem przed parsowaniem —
//! inaczej długie tytuły i lokalizacje rozpadają się w środku, także w środku znaku
//! UTF-8.

use std::io::{self, BufRead};

/// Pojedyncza właściwość: nazwa, parametry i wartość.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    /// Nazwa w wielkich literach, np. `DTSTART`.
    pub name: String,
    /// Parametry jako pary (nazwa, wartość), np. `("TZID", "Europe/Warsaw")`.
    pub params: Vec<(String, String)>,
    /// Wartość po dwukropku, z rozwiniętymi sekwencjami escape.
    pub value: String,
}

impl Property {
    /// Wartość parametru o podanej nazwie (bez rozróżniania wielkości liter).
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Iterator po właściwościach, czytający strumieniowo.
///
/// Każdy element to `Result` — błąd wejścia **nie** jest cichym końcem strumienia.
pub struct PropertyReader<R: BufRead> {
    reader: R,
    /// Linia wczytana z wyprzedzeniem, jeszcze nieprzetworzona.
    pending: Option<String>,
    finished: bool,
    /// Czy zobaczyliśmy `END:VCALENDAR`. Jedyny dowód, że pobranie było kompletne.
    saw_end: bool,
}

impl<R: BufRead> PropertyReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            pending: None,
            finished: false,
            saw_end: false,
        }
    }

    /// Czy strumień zawierał `END:VCALENDAR`.
    ///
    /// **Sprawdź to po zakończeniu iteracji.** Urwane połączenie TCP daje poprawnie
    /// sparsowany, ale niekompletny kalendarz — i bez tej flagi nie da się tego odróżnić
    /// od kalendarza, w którym po prostu nic już nie ma.
    pub fn saw_end(&self) -> bool {
        self.saw_end
    }

    /// Czyta jedną fizyczną linię, obcinając zakończenia CRLF/LF.
    /// Zwraca `Ok(None)` na końcu strumienia.
    fn read_physical(&mut self) -> io::Result<Option<String>> {
        let mut buf = Vec::new();
        let n = self.reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            return Ok(None);
        }
        while matches!(buf.last(), Some(b'\n' | b'\r')) {
            buf.pop();
        }
        // Kanały Google są w UTF-8; pojedynczy uszkodzony bajt nie powinien zabijać
        // całego pobrania, więc podmieniamy zamiast zwracać błąd.
        Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
    }

    /// Czyta jedną **logiczną** linię, składając kontynuacje.
    fn read_logical(&mut self) -> io::Result<Option<String>> {
        let mut line = match self.pending.take() {
            Some(l) => l,
            None => match self.read_physical()? {
                Some(l) => l,
                None => return Ok(None),
            },
        };

        while let Some(next) = self.read_physical()? {
            if next.starts_with(' ') || next.starts_with('\t') {
                line.push_str(&next[1..]);
            } else {
                self.pending = Some(next);
                break;
            }
        }

        Ok(Some(line))
    }
}

impl<R: BufRead> Iterator for PropertyReader<R> {
    type Item = io::Result<Property>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        loop {
            match self.read_logical() {
                Err(e) => {
                    self.finished = true;
                    return Some(Err(e));
                }
                Ok(None) => {
                    self.finished = true;
                    return None;
                }
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match parse_line(&line) {
                        Some(p) => {
                            if p.name == "END" && p.value.eq_ignore_ascii_case("VCALENDAR") {
                                self.saw_end = true;
                            }
                            return Some(Ok(p));
                        }
                        // Linia bez dwukropka jest niezgodna ze specyfikacją;
                        // pomijamy zamiast przewracać całe pobranie.
                        None => continue,
                    }
                }
            }
        }
    }
}

/// Rozbija logiczną linię na `NAZWA;PARAM=wartość:wartość`.
fn parse_line(line: &str) -> Option<Property> {
    // Dwukropek kończący część nazwy — ale nie taki, który jest w cudzysłowie
    // wewnątrz wartości parametru (np. TZID="GMT+01:00").
    let mut in_quotes = false;
    let mut colon = None;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;

    let (head, rest) = line.split_at(colon);
    let value = &rest[1..];

    let mut parts = split_unquoted(head, ';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }

    let params = parts
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            let v = v.trim().trim_matches('"');
            Some((k.trim().to_ascii_uppercase(), v.to_string()))
        })
        .collect();

    Some(Property {
        name,
        params,
        value: unescape(value),
    })
}

/// Dzieli po separatorze, ignorując te wewnątrz cudzysłowów.
fn split_unquoted(s: &str, sep: char) -> impl Iterator<Item = &str> {
    let mut parts = Vec::new();
    let mut in_quotes = false;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c == sep && !in_quotes => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts.into_iter()
}

/// Rozwija sekwencje escape z RFC 5545 §3.3.11.
fn unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some('\\') => out.push('\\'),
            Some(';') => out.push(';'),
            Some(',') => out.push(','),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn props(s: &str) -> Vec<Property> {
        PropertyReader::new(Cursor::new(s))
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn prosta_wlasciwosc() {
        let p = props("SUMMARY:Spotkanie\r\n");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].name, "SUMMARY");
        assert_eq!(p[0].value, "Spotkanie");
        assert!(p[0].params.is_empty());
    }

    #[test]
    fn parametry_sa_parsowane() {
        let p = props("DTSTART;TZID=Europe/Warsaw;VALUE=DATE-TIME:20260818T090000\r\n");
        assert_eq!(p[0].name, "DTSTART");
        assert_eq!(p[0].param("TZID"), Some("Europe/Warsaw"));
        assert_eq!(
            p[0].param("tzid"),
            Some("Europe/Warsaw"),
            "nazwa parametru bez wielkości liter"
        );
        assert_eq!(p[0].param("VALUE"), Some("DATE-TIME"));
        assert_eq!(p[0].value, "20260818T090000");
    }

    #[test]
    fn skladanie_zlamanych_linii() {
        // Kontynuacja zaczyna się spacją; ta spacja NIE jest częścią wartości.
        let ics = "SUMMARY:Bardzo dlugi tytul spotkania kt\r\n ory zostal zlamany\r\n";
        let p = props(ics);
        assert_eq!(p.len(), 1);
        assert_eq!(
            p[0].value,
            "Bardzo dlugi tytul spotkania ktory zostal zlamany"
        );
    }

    #[test]
    fn skladanie_nie_lamie_utf8() {
        // Znak wielobajtowy przecięty przez złamanie linii — dokładnie ten przypadek,
        // przez który parsowanie po bajtach się wykłada.
        let ics = "SUMMARY:Śniadanie z Łuk\r\n aszem — ćwiczenia\r\n";
        let p = props(ics);
        assert_eq!(p[0].value, "Śniadanie z Łukaszem — ćwiczenia");
    }

    #[test]
    fn skladanie_tabulatorem() {
        let ics = "DESCRIPTION:linia\r\n\tdalej\r\n";
        let p = props(ics);
        assert_eq!(p[0].value, "liniadalej");
    }

    #[test]
    fn sekwencje_escape() {
        let p = props("DESCRIPTION:pierwsza\\nDruga\\, trzecia\\; czwarta\\\\koniec\r\n");
        assert_eq!(p[0].value, "pierwsza\nDruga, trzecia; czwarta\\koniec");
    }

    #[test]
    fn dwukropek_w_cudzyslowie_nie_konczy_naglowka() {
        let p = props("DTSTART;TZID=\"GMT+01:00\":20260818T090000\r\n");
        assert_eq!(p[0].name, "DTSTART");
        assert_eq!(p[0].param("TZID"), Some("GMT+01:00"));
        assert_eq!(p[0].value, "20260818T090000");
    }

    #[test]
    fn linie_bez_dwukropka_sa_pomijane() {
        let p = props("SUMMARY:ok\r\nSMIECI BEZ DWUKROPKA\r\nLOCATION:tez ok\r\n");
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].name, "SUMMARY");
        assert_eq!(p[1].name, "LOCATION");
    }

    #[test]
    fn puste_linie_sa_pomijane() {
        let p = props("SUMMARY:a\r\n\r\n\r\nLOCATION:b\r\n");
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn brak_zakonczenia_jest_wykrywany() {
        let mut r = PropertyReader::new(Cursor::new("BEGIN:VCALENDAR\r\nSUMMARY:x\r\n"));
        for _ in r.by_ref() {}
        assert!(!r.saw_end(), "urwany kalendarz nie może udawać kompletnego");

        let mut r = PropertyReader::new(Cursor::new("BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n"));
        for _ in r.by_ref() {}
        assert!(r.saw_end());
    }

    #[test]
    fn blad_wejscia_nie_udaje_konca_pliku() {
        /// Strumień, który zawsze zwraca błąd — udaje urwane połączenie TCP.
        struct Failing;
        impl io::Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::ConnectionReset, "urwane"))
            }
        }
        impl BufRead for Failing {
            fn fill_buf(&mut self) -> io::Result<&[u8]> {
                Err(io::Error::new(io::ErrorKind::ConnectionReset, "urwane"))
            }
            fn consume(&mut self, _: usize) {}
        }

        let mut r = PropertyReader::new(Failing);
        let first = r.next().expect("spodziewano się elementu");
        assert!(
            first.is_err(),
            "błąd I/O musi wyjść jako Err, nie jako koniec strumienia"
        );
    }

    #[test]
    fn nazwa_wlasciwosci_jest_normalizowana() {
        let p = props("summary:x\r\n");
        assert_eq!(p[0].name, "SUMMARY");
    }
}
