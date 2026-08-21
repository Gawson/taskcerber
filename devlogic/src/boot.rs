//! Okruszek startowy — dokąd doszedł poprzedni cykl, zanim zamilkł.
//!
//! # Po co, skoro jest log szeregowy
//!
//! Bo log wymaga kabla, a kabel jest dokładnie tym, od czego to urządzenie ma
//! uwalniać. Gorzej: na tej płytce **radio wstaje tylko przy USB**, więc awaria
//! sieciowa zdarza się wyłącznie wtedy, gdy ktoś siedzi przy komputerze — a jak
//! nie siedzi, urządzenie po prostu zamiera z nieaktualnym ekranem i nie mówi nic.
//!
//! # Dlaczego NVS, a nie pamięć RTC
//!
//! Z tego samego powodu, dla którego mieszka tam licznik prób OTA: bootloader
//! przeładowuje segmenty RTC z obrazu przy każdym resecie, który **nie** jest
//! wybudzeniem z deep sleepu. Panika, watchdog i `esp_restart()` kasują więc
//! `.rtc.data` — czyli dokładnie te trzy przypadki, o których okruszek ma opowiedzieć.
//! Pełne wyjaśnienie w nagłówku [`crate::ota`].
//!
//! # Dlaczego to nie jest ślad na żywo
//!
//! Bo panelu i radia nie wolno trzymać naraz: epdiy zajmuje ~30 KB wewnętrznego
//! DRAM-u, którego mbedTLS potrzebuje na uścisk dłoni, a `epd_deinit()` jest na tej
//! płytce zakazane, więc raz zainicjalizowany panel trzyma tę pamięć do końca cyklu.
//! Pisanie kroków na ekran **powodowało awarię, którą miało diagnozować**.
//!
//! Okruszek omija ten konflikt w czasie zamiast w pamięci: cykl N zapisuje, dokąd
//! doszedł, a cykl N+1 — już bez radia — ma panel wyłącznie dla siebie i maluje
//! diagnozę. Kosztuje to jeden zapis do NVS na krok i zero bajtów DRAM-u.

use serde::{Deserialize, Serialize};

/// Etap cyklu startowego.
///
/// Numery są **częścią formatu zapisu w NVS** i muszą być stabilne — okruszek
/// zostawiony przez poprzednią wersję firmware'u czyta ta następna. Dokładanie
/// etapów tylko na końcu listy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum BootStep {
    /// Cykl doszedł do końca. To jedyna wartość, która znaczy „wszystko dobrze".
    Done = 0,
    /// Awaria z poprzedniego cyklu została już pokazana na ekranie.
    ///
    /// Bez tego stanu urządzenie utknęłoby na diagnozie: cykl diagnostyczny pomija
    /// sieć, więc sam nigdy nie doszedłby do `Done` i malowałby ten sam ekran w kółko.
    Reported = 1,
    RadioUp = 2,
    Sntp = 3,
    FetchPrimary = 4,
    FetchSecondary = 5,
    Ota = 6,
    RadioDown = 7,
    Paint = 8,
}

impl BootStep {
    pub fn code(self) -> u8 {
        self as u8
    }

    /// Nieznany numer daje `Done` — okruszek pochodzi z innej wersji firmware'u
    /// i lepiej pominąć diagnozę, niż zawiesić urządzenie na ekranie o niczym.
    pub fn from_code(v: u8) -> Self {
        match v {
            1 => BootStep::Reported,
            2 => BootStep::RadioUp,
            3 => BootStep::Sntp,
            4 => BootStep::FetchPrimary,
            5 => BootStep::FetchSecondary,
            6 => BootStep::Ota,
            7 => BootStep::RadioDown,
            8 => BootStep::Paint,
            _ => BootStep::Done,
        }
    }

    /// Nazwa na ekran diagnozy. Krótka, bo idzie w dużym stopniu typograficznym —
    /// z dwóch metrów czyta się rozmiar, nie precyzję.
    pub fn label(self) -> &'static str {
        match self {
            BootStep::Done => "zakończony",
            BootStep::Reported => "pokazany",
            BootStep::RadioUp => "łączenie z WiFi",
            BootStep::Sntp => "ustawianie zegara",
            BootStep::FetchPrimary => "pobieranie kalendarza",
            BootStep::FetchSecondary => "pobieranie 2. kalendarza",
            BootStep::Ota => "sprawdzanie aktualizacji",
            BootStep::RadioDown => "wyłączanie radia",
            BootStep::Paint => "rysowanie ekranu",
        }
    }

    /// Co najprawdopodobniej jest nie tak — zdanie dla człowieka, nie dla logu.
    pub fn hint(self) -> &'static str {
        match self {
            BootStep::RadioUp => "sprawdź nazwę sieci i hasło",
            BootStep::Sntp => "sieć działa, ale nie odpowiada serwer czasu",
            BootStep::FetchPrimary | BootStep::FetchSecondary => {
                "sprawdź adres iCal; przy mało wolnej pamięci to może być TLS"
            }
            BootStep::Ota => "sprawdź adres manifestu aktualizacji",
            BootStep::RadioDown | BootStep::Paint => "usterka po stronie urządzenia",
            BootStep::Done | BootStep::Reported => "",
        }
    }
}

/// Okruszek: etap, czas od startu i wolna pamięć wewnętrzna w chwili jego rozpoczęcia.
///
/// `dram_kb` jest tu najważniejszą liczbą i dlatego w ogóle istnieje ta struktura,
/// a nie sam numer kroku: awarie w TLS-ie wyglądają jak zawieszenie, a są brakiem
/// pamięci — i widać to wyłącznie po tym, ile jej zostawało tuż przed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crumb {
    pub step: BootStep,
    pub ms: u32,
    pub dram_kb: u16,
}

impl Crumb {
    pub fn new(step: BootStep, ms: u32, dram_kb: u16) -> Self {
        Self { step, ms, dram_kb }
    }

    /// Czy ten okruszek opisuje cykl, który się nie dokończył.
    ///
    /// `Reported` NIE jest awarią do pokazania: znaczy, że diagnoza już poszła na
    /// ekran i kolejny cykl ma normalnie spróbować sieci.
    pub fn is_failure(&self) -> bool {
        !matches!(self.step, BootStep::Done | BootStep::Reported)
    }

    /// Pakuje okruszek w jedno słowo, bo NVS liczy zapisy, a nie bajty —
    /// trzy osobne klucze to trzy skasowania strony flasha na każdy krok.
    pub fn pack(&self) -> u64 {
        self.step.code() as u64 | (self.ms as u64) << 8 | (self.dram_kb as u64) << 40
    }

    pub fn unpack(v: u64) -> Self {
        Self {
            step: BootStep::from_code((v & 0xFF) as u8),
            ms: ((v >> 8) & 0xFFFF_FFFF) as u32,
            dram_kb: ((v >> 40) & 0xFFFF) as u16,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pakowanie_jest_odwracalne_takze_na_krancach() {
        for c in [
            Crumb::new(BootStep::Done, 0, 0),
            Crumb::new(BootStep::FetchPrimary, 4700, 62),
            Crumb::new(BootStep::Paint, u32::MAX, u16::MAX),
        ] {
            assert_eq!(Crumb::unpack(c.pack()), c, "okruszek {c:?} nie przeżył");
        }
    }

    /// Pola nie mogą na siebie zachodzić — inaczej duży czas zmieniłby numer kroku
    /// i diagnoza wskazywałaby nie ten etap.
    #[test]
    fn pola_sie_nie_nakladaja() {
        let a = Crumb::new(BootStep::RadioUp, u32::MAX, 0);
        assert_eq!(Crumb::unpack(a.pack()).step, BootStep::RadioUp);
        let b = Crumb::new(BootStep::RadioUp, 0, u16::MAX);
        assert_eq!(Crumb::unpack(b.pack()).step, BootStep::RadioUp);
        assert_eq!(Crumb::unpack(b.pack()).ms, 0);
    }

    #[test]
    fn numery_etapow_sa_stabilne() {
        // Te wartości są formatem zapisu w NVS. Zmiana któregokolwiek sprawia,
        // że okruszek sprzed aktualizacji opisuje inny etap niż naprawdę.
        for (step, code) in [
            (BootStep::Done, 0),
            (BootStep::Reported, 1),
            (BootStep::RadioUp, 2),
            (BootStep::Sntp, 3),
            (BootStep::FetchPrimary, 4),
            (BootStep::FetchSecondary, 5),
            (BootStep::Ota, 6),
            (BootStep::RadioDown, 7),
            (BootStep::Paint, 8),
        ] {
            assert_eq!(step.code(), code);
            assert_eq!(BootStep::from_code(code), step);
        }
        assert_eq!(BootStep::from_code(200), BootStep::Done);
    }

    #[test]
    fn tylko_niedokonczony_cykl_jest_awaria() {
        assert!(!Crumb::new(BootStep::Done, 0, 0).is_failure());
        // Kluczowe dla wyjścia z pętli diagnostycznej.
        assert!(!Crumb::new(BootStep::Reported, 0, 0).is_failure());
        assert!(Crumb::new(BootStep::RadioUp, 0, 0).is_failure());
        assert!(Crumb::new(BootStep::FetchPrimary, 0, 0).is_failure());
    }

    #[test]
    fn kazdy_etap_awarii_ma_podpowiedz() {
        for code in 2..=8 {
            let s = BootStep::from_code(code);
            assert!(!s.label().is_empty(), "{s:?} bez nazwy");
            assert!(!s.hint().is_empty(), "{s:?} bez podpowiedzi");
        }
    }
}
