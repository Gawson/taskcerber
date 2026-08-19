//! Ekran konfiguracji: stan edycji i zawartość klawiatury ekranowej.
//!
//! To jest **jedyna** droga wprowadzania danych do urządzenia. Nie ma tu konsoli
//! szeregowej ani serwera HTTP — jest panel dotykowy, który i tak musi działać,
//! bo bez niego nie da się nawet przełączyć strony agendy.
//!
//! Podział jest taki sam jak w reszcie crate'a: tutaj mieszka **stan i treść**
//! (które pole, jakie znaki ma klawiatura, co robi naciśnięcie), a cała geometria
//! i rysowanie są w [`crate::layout`]. Dzięki temu ten plik da się czytać jako opis
//! zachowania, a nie jako arytmetykę pikseli.
//!
//! # Dlaczego zakładki, a nie lista pól
//!
//! Pierwsza wersja pokazywała wszystkie sześć pól jedno pod drugim. W poziomie
//! (960×540) po odjęciu klawiatury zostaje ~200 px, czyli 33 px na pole — mniej niż
//! wysokość wiersza tekstu. Zakładki zajmują jeden wiersz niezależnie od liczby pól
//! i zostawiają edytowanej wartości tyle miejsca, żeby było widać, co się wpisuje.
//!
//! # Znaki, których tu nie ma
//!
//! Klawiatura ma litery łacińskie, polskie znaki diakrytyczne, cyfry i te symbole,
//! które faktycznie występują w adresach i hasłach. Nie ma nawiasów klamrowych,
//! znaków matematycznych ani niczego, co trafiłoby tu tylko po to, żeby klawiatura
//! wyglądała na kompletną — każdy klawisz zabiera szerokość pozostałym, a przy
//! 540 px w pionie klawisz ma ~48 px i nie ma z czego oddawać.

/// Maksymalna długość wartości.
///
/// **Musi się zgadzać z `Store::MAX_VALUE` w firmwarze** — po tamtej stronie jest to
/// limit pojedynczej wartości w NVS. Pilnuje tego stała asercja w `firmware/src/store.rs`;
/// gdyby ktoś podniósł limit tylko tutaj, wpisany adres dałby się wystukać i zniknął
/// przy zapisie.
pub const MAX_LEN: usize = 512;

/// Pole konfiguracji.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Field {
    #[default]
    Ssid,
    Password,
    Ics,
    Ics2,
    Timezone,
    Ota,
}

impl Field {
    /// Kolejność zakładek — ona wyznacza też kolejność wypełniania.
    pub const ALL: [Field; 6] = [
        Field::Ssid,
        Field::Password,
        Field::Ics,
        Field::Ics2,
        Field::Timezone,
        Field::Ota,
    ];

    /// Napis na zakładce. Krótki, bo sześć zakładek dzieli 540 px w pionie.
    pub fn tab(self) -> &'static str {
        match self {
            Field::Ssid => "sieć",
            Field::Password => "hasło",
            Field::Ics => "iCal",
            Field::Ics2 => "iCal 2",
            Field::Timezone => "strefa",
            Field::Ota => "OTA",
        }
    }

    /// Pełna nazwa nad polem wartości.
    pub fn label(self) -> &'static str {
        match self {
            Field::Ssid => "nazwa sieci WiFi",
            Field::Password => "hasło WiFi",
            Field::Ics => "adres kalendarza (iCal)",
            Field::Ics2 => "drugi kalendarz — opcjonalny",
            Field::Timezone => "strefa czasowa",
            Field::Ota => "manifest aktualizacji — opcjonalny",
        }
    }

    /// Podpowiedź pokazywana, gdy pole jest puste.
    pub fn hint(self) -> &'static str {
        match self {
            Field::Ssid => "dokładnie tak, jak widać na liście sieci",
            Field::Password => "puste = sieć otwarta",
            Field::Ics => "https://calendar.google.com/calendar/ical/…/basic.ics",
            Field::Ics2 => "np. święta albo kalendarz współdzielony",
            Field::Timezone => "puste = Europe/Warsaw",
            Field::Ota => "puste = aktualizacje wyłączone",
        }
    }

    /// Czy bez tego pola urządzenie nie ruszy.
    pub fn required(self) -> bool {
        matches!(self, Field::Ssid | Field::Ics)
    }

    fn index(self) -> usize {
        match self {
            Field::Ssid => 0,
            Field::Password => 1,
            Field::Ics => 2,
            Field::Ics2 => 3,
            Field::Timezone => 4,
            Field::Ota => 5,
        }
    }
}

/// Która strona klawiatury jest pokazana.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Letters,
    Symbols,
}

impl Page {
    pub fn toggled(self) -> Self {
        match self {
            Page::Letters => Page::Symbols,
            Page::Symbols => Page::Letters,
        }
    }

    /// Napis na klawiszu przełączającym — pokazuje stronę, na którą się przejdzie.
    pub fn switch_label(self) -> &'static str {
        match self {
            Page::Letters => "?123",
            Page::Symbols => "abc",
        }
    }

    /// Wiersze znakowe tej strony.
    ///
    /// Strony mają różną liczbę wierszy i to jest w porządku — klawiatura jest
    /// dosunięta do dolnej krawędzi, więc rośnie w górę.
    pub fn rows(self) -> &'static [&'static str] {
        match self {
            Page::Letters => &["qwertyuiop", "asdfghjkl", "zxcvbnm"],
            // Wiersz drugi to komplet znaków z adresu iCal Google; trzeci to reszta
            // tego, co trafia do haseł. Polskie znaki są na osobnym wierszu, bo
            // w SSID-ach się zdarzają, a bez nich nie byłoby ich jak wpisać w ogóle.
            Page::Symbols => &["1234567890", "./:-_?=&%@", "+#!*$,;'\"", "ąćęłńóśźż"],
        }
    }

    /// Czy na tej stronie klawisz `⇧` cokolwiek zmienia.
    pub fn has_letters(self) -> bool {
        match self {
            Page::Letters => true,
            // `ą` -> `Ą` działa, cyfry i symbole zostają bez zmian.
            Page::Symbols => true,
        }
    }
}

/// Stan klawisza `⇧`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Caps {
    #[default]
    Off,
    /// Jedna wielka litera, potem z powrotem.
    Once,
    /// Zablokowane do odwołania.
    Lock,
}

impl Caps {
    /// Naciśnięcie `⇧`: wył. -> jednorazowo -> blokada -> wył.
    pub fn pressed(self) -> Self {
        match self {
            Caps::Off => Caps::Once,
            Caps::Once => Caps::Lock,
            Caps::Lock => Caps::Off,
        }
    }

    pub fn is_active(self) -> bool {
        !matches!(self, Caps::Off)
    }
}

/// Co się stało po zastosowaniu akcji — wołający wybiera na tej podstawie
/// tryb odświeżenia panelu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// Zmieniła się wyłącznie edytowana wartość.
    Edited,
    /// Zmieniło się coś w układzie: pole, strona klawiatury, stan `⇧`.
    Relayout,
    /// Użytkownik nacisnął „zapisz".
    Save,
    /// Akcja nie należy do tego ekranu.
    Ignored,
}

/// Stan ekranu konfiguracji.
#[derive(Debug, Clone, Default)]
pub struct Setup {
    field: Field,
    values: [String; 6],
    page: Page,
    caps: Caps,
}

impl Setup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn field(&self) -> Field {
        self.field
    }

    pub fn page(&self) -> Page {
        self.page
    }

    pub fn caps(&self) -> Caps {
        self.caps
    }

    pub fn value(&self, field: Field) -> &str {
        &self.values[field.index()]
    }

    /// Wartość edytowanego pola.
    pub fn current(&self) -> &str {
        self.value(self.field)
    }

    /// Wstawia wartość — używane przy wejściu na ekran, żeby pokazać to,
    /// co już jest w NVS.
    pub fn set(&mut self, field: Field, value: impl Into<String>) {
        let mut value: String = value.into();
        truncate_to(&mut value, MAX_LEN);
        self.values[field.index()] = value;
    }

    /// Czy komplet wymaganych pól jest wypełniony.
    ///
    /// Ta sama reguła co `Config::is_provisioned` w firmwarze: bez SSID i bez adresu
    /// kalendarza nie ma po co budzić radia.
    pub fn is_complete(&self) -> bool {
        Field::ALL
            .iter()
            .filter(|f| f.required())
            .all(|f| !self.value(*f).is_empty())
    }

    /// Pierwsze wymagane pole, które jest jeszcze puste.
    pub fn first_missing(&self) -> Option<Field> {
        Field::ALL
            .into_iter()
            .find(|f| f.required() && self.value(*f).is_empty())
    }

    /// Stosuje akcję dotykową.
    pub fn apply(&mut self, action: crate::hit::Action) -> Applied {
        use crate::hit::Action;

        match action {
            Action::Key(ch) => {
                let ch = if self.caps.is_active() {
                    // `to_uppercase` bywa wieloznakowe (nie dla polskich liter, ale
                    // reguła jest ogólna) — bierzemy pierwszy znak, bo pole i tak
                    // przechowuje pojedyncze wciśnięcie.
                    ch.to_uppercase().next().unwrap_or(ch)
                } else {
                    ch
                };

                let value = &mut self.values[self.field.index()];
                // Limit liczymy w BAJTACH, bo taki jest limit po stronie NVS.
                // „ą" zajmuje dwa i musi się z tego rozliczyć.
                if value.len() + ch.len_utf8() > MAX_LEN {
                    return Applied::Ignored;
                }
                value.push(ch);

                if self.caps == Caps::Once {
                    self.caps = Caps::Off;
                    return Applied::Relayout;
                }
                Applied::Edited
            }

            Action::Backspace => {
                if self.values[self.field.index()].pop().is_some() {
                    Applied::Edited
                } else {
                    Applied::Ignored
                }
            }

            Action::Caps => {
                self.caps = self.caps.pressed();
                Applied::Relayout
            }

            Action::KeyPage => {
                self.page = self.page.toggled();
                Applied::Relayout
            }

            Action::Focus(field) => {
                if field == self.field {
                    return Applied::Ignored;
                }
                self.field = field;
                Applied::Relayout
            }

            Action::Save => Applied::Save,

            _ => Applied::Ignored,
        }
    }
}

/// Przycina łańcuch do `max` **bajtów**, nie rozcinając znaku.
fn truncate_to(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hit::Action;

    #[test]
    fn pisanie_trafia_do_wybranego_pola() {
        let mut s = Setup::new();
        assert_eq!(s.field(), Field::Ssid);
        for ch in "dom".chars() {
            s.apply(Action::Key(ch));
        }
        assert_eq!(s.value(Field::Ssid), "dom");
        assert_eq!(s.value(Field::Password), "");

        s.apply(Action::Focus(Field::Password));
        for ch in "tajne".chars() {
            s.apply(Action::Key(ch));
        }
        assert_eq!(s.value(Field::Ssid), "dom");
        assert_eq!(s.value(Field::Password), "tajne");
    }

    #[test]
    fn shift_jednorazowy_gasnie_po_jednej_literze() {
        let mut s = Setup::new();
        s.apply(Action::Caps);
        assert_eq!(s.caps(), Caps::Once);
        s.apply(Action::Key('a'));
        s.apply(Action::Key('b'));
        assert_eq!(s.current(), "Ab");
        assert_eq!(s.caps(), Caps::Off);
    }

    #[test]
    fn shift_zablokowany_zostaje() {
        let mut s = Setup::new();
        s.apply(Action::Caps); // Once
        s.apply(Action::Caps); // Lock
        assert_eq!(s.caps(), Caps::Lock);
        for ch in "abc".chars() {
            s.apply(Action::Key(ch));
        }
        assert_eq!(s.current(), "ABC");
        s.apply(Action::Caps); // Off
        s.apply(Action::Key('d'));
        assert_eq!(s.current(), "ABCd");
    }

    #[test]
    fn shift_dziala_na_polskich_literach() {
        let mut s = Setup::new();
        s.apply(Action::Caps);
        s.apply(Action::Key('ż'));
        assert_eq!(s.current(), "Ż");
    }

    #[test]
    fn backspace_kasuje_caly_znak_wielobajtowy() {
        let mut s = Setup::new();
        s.apply(Action::Key('ż'));
        s.apply(Action::Key('a'));
        assert_eq!(s.current(), "ża");
        s.apply(Action::Backspace);
        s.apply(Action::Backspace);
        assert_eq!(s.current(), "");
        // Na pustym polu backspace nie jest zmianą — nie ma po co odświeżać panelu.
        assert_eq!(s.apply(Action::Backspace), Applied::Ignored);
    }

    #[test]
    fn limit_dlugosci_liczy_bajty_a_nie_znaki() {
        let mut s = Setup::new();
        s.set(Field::Ssid, "ż".repeat(MAX_LEN / 2));
        assert_eq!(s.current().len(), MAX_LEN);
        // Kolejny znak już się nie mieści i nie wolno go po cichu przyjąć —
        // po stronie NVS wartość ponad limit znika przy odczycie.
        assert_eq!(s.apply(Action::Key('a')), Applied::Ignored);
        assert_eq!(s.current().len(), MAX_LEN);
    }

    #[test]
    fn wstawiona_wartosc_jest_przycinana_na_granicy_znaku() {
        let mut s = Setup::new();
        // MAX_LEN jest parzyste, więc łańcuch z „ż" o długości MAX_LEN + 1 znaku
        // musi zostać ucięty tak, żeby nie rozciąć dwubajtowej sekwencji.
        s.set(Field::Ics, "ż".repeat(MAX_LEN));
        assert!(s.value(Field::Ics).len() <= MAX_LEN);
        assert!(s.value(Field::Ics).chars().all(|c| c == 'ż'));
    }

    #[test]
    fn komplet_to_siec_i_kalendarz() {
        let mut s = Setup::new();
        assert!(!s.is_complete());
        assert_eq!(s.first_missing(), Some(Field::Ssid));

        s.set(Field::Ssid, "dom");
        assert_eq!(s.first_missing(), Some(Field::Ics));
        assert!(!s.is_complete());

        s.set(Field::Ics, "https://przyklad.pl/k.ics");
        assert!(s.is_complete());
        assert_eq!(s.first_missing(), None);
        // Hasło nie jest wymagane: sieć bywa otwarta.
        assert!(s.value(Field::Password).is_empty());
    }

    #[test]
    fn przelaczanie_strony_klawiatury() {
        let mut s = Setup::new();
        assert_eq!(s.page(), Page::Letters);
        assert_eq!(s.apply(Action::KeyPage), Applied::Relayout);
        assert_eq!(s.page(), Page::Symbols);
        s.apply(Action::Key('/'));
        assert_eq!(s.current(), "/");
    }

    #[test]
    fn wybor_tego_samego_pola_nie_jest_zmiana() {
        let mut s = Setup::new();
        assert_eq!(s.apply(Action::Focus(Field::Ssid)), Applied::Ignored);
        assert_eq!(s.apply(Action::Focus(Field::Ics)), Applied::Relayout);
    }

    #[test]
    fn akcje_agendy_nie_dotyczą_tego_ekranu() {
        let mut s = Setup::new();
        assert_eq!(s.apply(Action::NextPage), Applied::Ignored);
        assert_eq!(s.apply(Action::ShowEvent(3)), Applied::Ignored);
    }

    #[test]
    fn klawiatura_ma_znaki_potrzebne_do_adresu_ical() {
        let dostepne: String = Page::Letters
            .rows()
            .iter()
            .chain(Page::Symbols.rows())
            .flat_map(|r| r.chars())
            .collect();
        // Dokładnie te znaki występują w prywatnym adresie iCal Google.
        for ch in "abcdefghijklmnopqrstuvwxyz0123456789:/.-_%@?=&".chars() {
            assert!(dostepne.contains(ch), "brakuje `{ch}` na klawiaturze");
        }
    }

    #[test]
    fn zaden_znak_nie_powtarza_sie_na_klawiaturze() {
        let mut widziane = std::collections::BTreeSet::new();
        for row in Page::Letters.rows().iter().chain(Page::Symbols.rows()) {
            for ch in row.chars() {
                assert!(widziane.insert(ch), "znak `{ch}` jest na dwóch klawiszach");
            }
        }
    }
}
