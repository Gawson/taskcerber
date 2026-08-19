//! Licznik energii BQ27220 pod adresem `0x55`.
//!
//! Dwa zastosowania: stan naładowania do polityki zasilania i **temperatura do
//! `epd_hl_update_screen()`**. To drugie jest nieoczywiste, ale ważne: profil
//! `lilygo_board_s3` w epdiy ma `epd_board_ambient_temperature()` zwracające
//! **zaszyte na sztywno 20 °C**, a przebiegi e-papieru są silnie zależne od
//! temperatury. Podanie prawdziwej wartości to różnica między czystym odświeżeniem
//! a duchami przy 5 °C.

use anyhow::Result;

use crate::i2c::{I2cBus, I2cDevice};

pub const ADDRESS: u8 = 0x55;

const REG_TEMPERATURE: u8 = 0x06; // 0.1 K
const REG_VOLTAGE: u8 = 0x08; // mV
const REG_CURRENT: u8 = 0x0C; // mA, ze znakiem
const REG_STATE_OF_CHARGE: u8 = 0x2C; // %

pub struct Bq27220 {
    dev: I2cDevice,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Fuel {
    pub percent: Option<u8>,
    pub millivolts: Option<u16>,
    /// Prąd w mA; dodatni = ładowanie.
    pub milliamps: Option<i16>,
    pub temperature_c: Option<i16>,
}

impl Bq27220 {
    pub fn new(bus: &I2cBus) -> Result<Self> {
        Ok(Self {
            dev: bus.device(ADDRESS)?,
        })
    }

    /// Odczyt wszystkiego naraz. Pojedyncze błędy nie przewracają całości —
    /// dashboard woli pokazać część danych niż nic.
    pub fn read(&self) -> Fuel {
        Fuel {
            percent: self
                .dev
                .read_u16_le(REG_STATE_OF_CHARGE)
                .ok()
                .map(|v| v.min(100) as u8),
            millivolts: self.dev.read_u16_le(REG_VOLTAGE).ok(),
            milliamps: self.dev.read_u16_le(REG_CURRENT).ok().map(|v| v as i16),
            temperature_c: self
                .dev
                .read_u16_le(REG_TEMPERATURE)
                .ok()
                .map(decikelvin_to_celsius),
        }
    }

    /// Temperatura dla waveformu EPD; przy błędzie odczytu 20 °C, czyli to samo,
    /// co i tak zaszyte w epdiy.
    pub fn temperature_or_default(&self) -> i32 {
        self.dev
            .read_u16_le(REG_TEMPERATURE)
            .ok()
            .map(decikelvin_to_celsius)
            .map(i32::from)
            .unwrap_or(20)
    }
}

/// BQ27220 raportuje temperaturę w dziesiątych częściach kelwina.
fn decikelvin_to_celsius(dk: u16) -> i16 {
    // 0 °C = 273.15 K = 2731.5 dK; zaokrąglamy do najbliższego stopnia.
    ((dk as i32 - 2732) as f32 / 10.0).round() as i16
}

#[cfg(test)]
mod tests {
    use super::decikelvin_to_celsius;

    #[test]
    fn konwersja_temperatury() {
        assert_eq!(decikelvin_to_celsius(2932), 20); // 293.2 K
        assert_eq!(decikelvin_to_celsius(2732), 0);
        assert_eq!(decikelvin_to_celsius(2632), -10);
    }
}
