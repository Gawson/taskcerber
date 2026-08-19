//! Sterowniki układów na płytce, poza tymi, które obsługuje epdiy.
//!
//! epdiy bierze na siebie TPS65185 (PMIC e-papieru) i port 1 ekspandera PCA9535.
//! Wszystko poniżej jest nasze.

pub mod bq25896;
pub mod bq27220;
pub mod gt911;
pub mod pca9535;
pub mod pcf8563;

use anyhow::Result;

use crate::i2c::I2cBus;

/// Wszystkie układy na współdzielonej magistrali, zebrane w jedno.
pub struct Board {
    pub expander: pca9535::Pca9535,
    pub charger: bq25896::Bq25896,
    pub fuel: bq27220::Bq27220,
    pub rtc: pcf8563::Pcf8563,
}

impl Board {
    /// Otwiera wszystkie urządzenia i **natychmiast gasi szynę LoRa/GPS**.
    ///
    /// Kolejność jest istotna: ta szyna wstaje załączona po zimnym starcie przez
    /// podciągnięcie R21 i kosztuje 25–35 mA, czyli około czterdziestu godzin baterii.
    pub fn open(bus: &I2cBus) -> Result<Self> {
        let expander = pca9535::Pca9535::new(bus)?;
        expander.power_down_lora_gps()?;

        Ok(Self {
            expander,
            charger: bq25896::Bq25896::new(bus)?,
            fuel: bq27220::Bq27220::new(bus)?,
            rtc: pcf8563::Pcf8563::new(bus)?,
        })
    }
}
