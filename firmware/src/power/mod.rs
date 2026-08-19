//! Polityka zasilania.
//!
//! Bazuje na pomiarach i szacunkach z `docs/power.md`. Krótko:
//! * Podłoga deep sleepu po pełnej sekwencji wyłączania: **~155 µA**.
//! * Jedno wybudzenie z siecią: **~360 mAs** (zoptymalizowane) do ~850 mAs (zimne).
//! * Budżet na tydzień z 1200 mAh użytecznych: **7,14 mA średnio**.
//!
//! Przy odświeżaniu co 30 minut w godzinach aktywnych i nocnej przerwie wychodzi
//! ~7 mAh/dobę, czyli grubo ponad sto dni. Tydzień to nie jest tu trudny cel —
//! trudne jest nie zepsuć go jednym z czterech błędów opisanych w `docs/power.md`.

pub mod rtc_state;
pub mod shutdown;

use chrono::{NaiveDateTime, NaiveTime, Timelike};

use crate::board::bq25896::PowerStatus;
use crate::board::bq27220::Fuel;

/// Tryb pracy wybierany przy każdym wybudzeniu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// USB podłączone — nie oszczędzamy.
    Usb,
    /// Bateria, godziny aktywne, ogniwo w porządku.
    Active,
    /// Bateria, okno nocne — jedno długie spanie do rana.
    Night,
    /// Ogniwo poniżej 40% — rzadziej.
    Frugal,
    /// Ogniwo poniżej 20% — jeszcze rzadziej, bez pełnych odświeżeń.
    Survival,
    /// Ogniwo poniżej 10% — ostatni ekran i zamrożenie.
    Hold,
}

/// Poniżej tego poziomu naładowania nie ruszamy OTA bez USB.
pub const MIN_BATTERY_FOR_OTA: u8 = 50;

/// Konfiguracja polityki. Docelowo czytana z NVS, żeby dało się zmienić
/// bez przeflashowania.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// Początek okna aktywnego.
    pub day_start: NaiveTime,
    /// Koniec okna aktywnego.
    pub day_end: NaiveTime,
    /// Co ile sekund odświeżać w trybie [`Mode::Active`].
    pub active_interval_s: u64,
    /// Co ile sekund w trybie [`Mode::Usb`].
    pub usb_interval_s: u64,
    /// Co ile sekund w trybie [`Mode::Frugal`].
    pub frugal_interval_s: u64,
    /// Co ile sekund w trybie [`Mode::Survival`].
    pub survival_interval_s: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            day_start: NaiveTime::from_hms_opt(7, 0, 0).unwrap(),
            day_end: NaiveTime::from_hms_opt(23, 0, 0).unwrap(),
            active_interval_s: 30 * 60,
            usb_interval_s: 5 * 60,
            frugal_interval_s: 60 * 60,
            survival_interval_s: 6 * 60 * 60,
        }
    }
}

impl Policy {
    /// Wybiera tryb na podstawie zasilania, stanu ogniwa i pory dnia.
    pub fn mode(&self, power: PowerStatus, fuel: Fuel, now: NaiveDateTime) -> Mode {
        if power.usb_present {
            return Mode::Usb;
        }

        match fuel.percent {
            Some(p) if p < 10 => return Mode::Hold,
            Some(p) if p < 20 => return Mode::Survival,
            Some(p) if p < 40 => return Mode::Frugal,
            _ => {}
        }

        if self.is_night(now.time()) {
            Mode::Night
        } else {
            Mode::Active
        }
    }

    fn is_night(&self, t: NaiveTime) -> bool {
        if self.day_start <= self.day_end {
            t < self.day_start || t >= self.day_end
        } else {
            // Okno przechodzące przez północ.
            t < self.day_start && t >= self.day_end
        }
    }

    /// Ile sekund spać po tym wybudzeniu.
    ///
    /// W trybie nocnym śpimy jednym ciągiem do początku okna aktywnego, zamiast
    /// budzić się co pół godziny po nic.
    pub fn sleep_seconds(&self, mode: Mode, now: NaiveDateTime) -> u64 {
        match mode {
            Mode::Usb => self.usb_interval_s,
            Mode::Active => self.active_interval_s,
            Mode::Frugal => self.frugal_interval_s,
            Mode::Survival => self.survival_interval_s,
            Mode::Hold => 24 * 60 * 60,
            Mode::Night => self.seconds_until_morning(now),
        }
    }

    fn seconds_until_morning(&self, now: NaiveDateTime) -> u64 {
        let today_start = now.date().and_time(self.day_start);
        let target = if now < today_start {
            today_start
        } else {
            now.date()
                .succ_opt()
                .map(|d| d.and_time(self.day_start))
                .unwrap_or(today_start)
        };
        let secs = (target - now).num_seconds();
        // Minimum minuta, żeby błąd zegara nie wpędził nas w pętlę natychmiastowych
        // wybudzeń.
        secs.max(60) as u64
    }

    /// Czy w tym trybie w ogóle sięgamy po sieć.
    pub fn should_fetch(&self, mode: Mode) -> bool {
        !matches!(mode, Mode::Hold)
    }

    /// Czy w tym trybie wolno pobrać aktualizację firmware'u.
    ///
    /// Pobranie ~3 MB przez HTTPS trzyma radio na antenie o rząd wielkości dłużej
    /// niż zwykły cykl, a na końcu dochodzi zapis całego slotu do flasha. Na
    /// zużytym ogniwie to nie jest coś, co warto robić w tle — stąd USB albo
    /// wyraźny zapas energii.
    ///
    /// Nieznany stan naładowania (`None`, np. gdy licznik ogniwa nie odpowiada)
    /// traktujemy jak za mało. Lepiej nie zaktualizować się przez tydzień, niż
    /// zgasnąć w połowie zapisu slotu.
    pub fn should_update(&self, mode: Mode, fuel: Fuel) -> bool {
        match mode {
            Mode::Usb => true,
            Mode::Active => matches!(fuel.percent, Some(p) if p >= MIN_BATTERY_FOR_OTA),
            _ => false,
        }
    }
}

/// Ile sekund do najbliższej pełnej minuty — wyrównanie wybudzeń, żeby odświeżenie
/// wypadało o równych godzinach, a nie o 30 sekundach po.
pub fn align_to_minute(now: NaiveDateTime, seconds: u64) -> u64 {
    let target_unaligned = seconds + now.second() as u64;
    let remainder = target_unaligned % 60;
    if remainder == 0 {
        seconds
    } else {
        seconds + (60 - remainder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, 18)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    fn power(usb: bool) -> PowerStatus {
        PowerStatus {
            usb_present: usb,
            vbus_stat: 0,
            chrg_stat: 0,
        }
    }

    fn fuel(pct: u8) -> Fuel {
        Fuel {
            percent: Some(pct),
            millivolts: Some(3800),
            milliamps: None,
            temperature_c: Some(21),
        }
    }

    #[test]
    fn usb_wygrywa_ze_wszystkim() {
        let p = Policy::default();
        assert_eq!(p.mode(power(true), fuel(5), at(3, 0)), Mode::Usb);
    }

    #[test]
    fn niski_stan_ogniwa_wygrywa_z_pora_dnia() {
        let p = Policy::default();
        assert_eq!(p.mode(power(false), fuel(5), at(12, 0)), Mode::Hold);
        assert_eq!(p.mode(power(false), fuel(15), at(12, 0)), Mode::Survival);
        assert_eq!(p.mode(power(false), fuel(30), at(12, 0)), Mode::Frugal);
    }

    #[test]
    fn okno_nocne() {
        let p = Policy::default();
        assert_eq!(p.mode(power(false), fuel(80), at(2, 0)), Mode::Night);
        assert_eq!(p.mode(power(false), fuel(80), at(23, 30)), Mode::Night);
        assert_eq!(p.mode(power(false), fuel(80), at(6, 59)), Mode::Night);
        assert_eq!(p.mode(power(false), fuel(80), at(7, 0)), Mode::Active);
        assert_eq!(p.mode(power(false), fuel(80), at(22, 59)), Mode::Active);
    }

    #[test]
    fn noc_spi_do_rana_jednym_ciagiem() {
        let p = Policy::default();
        // O 23:30 do 07:00 jest 7,5 h.
        let s = p.sleep_seconds(Mode::Night, at(23, 30));
        assert_eq!(s, 7 * 3600 + 30 * 60);
        // O 02:00 do 07:00 jest 5 h.
        let s = p.sleep_seconds(Mode::Night, at(2, 0));
        assert_eq!(s, 5 * 3600);
    }

    #[test]
    fn spanie_nigdy_nie_jest_zerowe() {
        let p = Policy::default();
        // Dokładnie o początku okna aktywnego — nie wolno zwrócić 0.
        let s = p.sleep_seconds(Mode::Night, at(7, 0));
        assert!(s >= 60);
    }

    #[test]
    fn tryb_hold_nie_siega_po_siec() {
        let p = Policy::default();
        assert!(!p.should_fetch(Mode::Hold));
        assert!(p.should_fetch(Mode::Active));
        assert!(p.should_fetch(Mode::Survival));
    }

    #[test]
    fn wyrownanie_do_pelnej_minuty() {
        let now = NaiveDate::from_ymd_opt(2026, 8, 18)
            .unwrap()
            .and_hms_opt(12, 0, 20)
            .unwrap();
        // 1800 s + 20 s przesunięcia -> dociągamy do 1840, żeby trafić w pełną minutę.
        assert_eq!(align_to_minute(now, 1800) % 60, 40);
        let exact = NaiveDate::from_ymd_opt(2026, 8, 18)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        assert_eq!(align_to_minute(exact, 1800), 1800);
    }
}
