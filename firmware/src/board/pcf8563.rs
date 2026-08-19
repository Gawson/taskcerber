//! Zegar czasu rzeczywistego PCF8563 pod adresem `0x51`.
//!
//! **Uwaga na rozbieżność źródeł.** README producenta podaje `PCF85063`, ale
//! `docs/pinmap.md` tego samego repo mówi wprost, że schemat (strona 3, U3) pokazuje
//! `PCF8563TS`, i zaleca ufać schematowi. Strona produktowa też mówi PCF8563.
//! Mapy rejestrów tych układów **nie są identyczne** — patrz `docs/hardware.md` §7.
//!
//! Ten sterownik implementuje **PCF8563**. Funkcja [`Pcf8563::probe_variant`] sprawdza,
//! który układ faktycznie siedzi na płytce, i jest wołana przy bring-upie.
//!
//! Po co w ogóle RTC, skoro jest SNTP: żeby urządzenie znało czas **natychmiast po
//! wybudzeniu**, przed podniesieniem radia. Bez tego każde wybudzenie musiałoby czekać
//! na sieć, zanim cokolwiek narysuje — a przy `CONFIG_MBEDTLS_HAVE_TIME_DATE=y`
//! również zanim cokolwiek pobierze.

use anyhow::{bail, Result};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};

use crate::i2c::{I2cBus, I2cDevice};

pub const ADDRESS: u8 = 0x51;

// Rejestry PCF8563.
const REG_CONTROL_1: u8 = 0x00;
const REG_CONTROL_2: u8 = 0x01;
const REG_SECONDS: u8 = 0x02;

/// Bit VL w rejestrze sekund: napięcie spadło poniżej progu, czas jest niewiarygodny.
const SECONDS_VL: u8 = 0x80;

/// Który układ faktycznie siedzi pod 0x51.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Pcf8563,
    /// Wykryto zachowanie zgodne z PCF85063 — mapa rejestrów jest inna,
    /// ten sterownik NIE jest dla niego poprawny.
    Pcf85063,
    Unknown,
}

pub struct Pcf8563 {
    dev: I2cDevice,
}

impl Pcf8563 {
    pub fn new(bus: &I2cBus) -> Result<Self> {
        Ok(Self {
            dev: bus.device(ADDRESS)?,
        })
    }

    /// Próbuje rozróżnić PCF8563 od PCF85063.
    ///
    /// W PCF8563 rejestr `0x00` (Control_1) ma bity 0, 1, 2, 4 i 6 na stałe zerowe.
    /// W PCF85063 pod `0x00` jest Control_1 o innym układzie, gdzie bit 0 (`CAP_SEL`)
    /// i bit 4 (`SR`) są zapisywalne. Heurystyka, nie pewnik — **potwierdź oznaczeniem
    /// na kostce**, zanim uznasz odczyty czasu za wiarygodne.
    pub fn probe_variant(&self) -> Variant {
        let Ok(ctrl1) = self.dev.read_u8(REG_CONTROL_1) else {
            return Variant::Unknown;
        };
        // PCF8563 trzyma te bity na zero niezależnie od tego, co zapiszesz.
        if ctrl1 & 0b0101_0111 == 0 {
            Variant::Pcf8563
        } else {
            Variant::Pcf85063
        }
    }

    /// Czy zegar zgłasza utratę zasilania (bit VL). Jeśli tak, czas jest śmieciem.
    pub fn voltage_low(&self) -> Result<bool> {
        Ok(self.dev.read_u8(REG_SECONDS)? & SECONDS_VL != 0)
    }

    /// Odczytuje czas lokalny. Zwraca `None`, gdy zegar zgłasza utratę zasilania
    /// albo gdy pola BCD nie składają się w prawidłową datę.
    pub fn now(&self) -> Result<Option<NaiveDateTime>> {
        let mut buf = [0u8; 7];
        self.dev.write_read(&[REG_SECONDS], &mut buf)?;

        if buf[0] & SECONDS_VL != 0 {
            return Ok(None);
        }

        let sec = bcd_to_dec(buf[0] & 0x7F);
        let min = bcd_to_dec(buf[1] & 0x7F);
        let hour = bcd_to_dec(buf[2] & 0x3F);
        let day = bcd_to_dec(buf[3] & 0x3F);
        // buf[4] to dzień tygodnia — liczymy go sami z daty, więc ignorujemy.
        let century = buf[5] & 0x80 != 0;
        let month = bcd_to_dec(buf[5] & 0x1F);
        let year2 = bcd_to_dec(buf[6]);

        let year = if century {
            1900 + year2 as i32
        } else {
            2000 + year2 as i32
        };

        let Some(date) = NaiveDate::from_ymd_opt(year, month as u32, day as u32) else {
            return Ok(None);
        };
        let Some(dt) = date.and_hms_opt(hour as u32, min as u32, sec as u32) else {
            return Ok(None);
        };
        Ok(Some(dt))
    }

    /// Zapisuje czas lokalny do zegara.
    pub fn set(&self, t: NaiveDateTime) -> Result<()> {
        let year = t.year();
        if !(1900..2100).contains(&year) {
            bail!("rok {year} poza zakresem PCF8563");
        }
        let (century_bit, year2) = if year >= 2000 {
            (0u8, (year - 2000) as u8)
        } else {
            (0x80u8, (year - 1900) as u8)
        };

        let payload = [
            REG_SECONDS,
            dec_to_bcd(t.second() as u8), // czyszczy też VL
            dec_to_bcd(t.minute() as u8),
            dec_to_bcd(t.hour() as u8),
            dec_to_bcd(t.day() as u8),
            t.weekday().num_days_from_sunday() as u8,
            dec_to_bcd(t.month() as u8) | century_bit,
            dec_to_bcd(year2),
        ];
        self.dev.write(&payload)
    }

    /// Kasuje flagi alarmu/timera. Wołane raz przy zimnym starcie.
    pub fn clear_flags(&self) -> Result<()> {
        self.dev.write_u8(REG_CONTROL_2, 0x00)
    }
}

fn bcd_to_dec(b: u8) -> u8 {
    (b >> 4) * 10 + (b & 0x0F)
}

fn dec_to_bcd(d: u8) -> u8 {
    ((d / 10) << 4) | (d % 10)
}

#[cfg(test)]
mod tests {
    use super::{bcd_to_dec, dec_to_bcd};

    #[test]
    fn bcd_w_obie_strony() {
        for d in 0u8..=99 {
            assert_eq!(bcd_to_dec(dec_to_bcd(d)), d, "wartość {d}");
        }
    }

    #[test]
    fn znane_wartosci_bcd() {
        assert_eq!(dec_to_bcd(0), 0x00);
        assert_eq!(dec_to_bcd(9), 0x09);
        assert_eq!(dec_to_bcd(10), 0x10);
        assert_eq!(dec_to_bcd(59), 0x59);
        assert_eq!(bcd_to_dec(0x23), 23);
    }
}
