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
    /// # Czego o tej szynie NIE wiemy
    ///
    /// Wcześniejsza wersja tego komentarza twierdziła, że szyna „wstaje załączona przez
    /// podciągnięcie R21" i „kosztuje 25–35 mA". Render arkusza 1 w 300 dpi pokazuje coś
    /// innego: R21 10K jest **szeregowy** między `LORA_EN` a wejściem `EN` układu U7,
    /// a na samym `EN` nie ma ani podciągnięcia, ani ściągnięcia. Stan po zimnym starcie
    /// jest więc **nieokreślony**, a nie „załączony", i liczba 25–35 mA nie ma źródła
    /// w żadnym dokumencie, jaki mamy.
    ///
    /// Gasimy ją mimo to i dalej jako pierwszą rzecz — stan nieokreślony trzeba
    /// rozstrzygnąć, a to jedyny moment, w którym można to zrobić przed resztą startu.
    /// Dopóki nie zmierzymy VCC3V3 na J1 pin 9 podczas snu, koszt tej szyny pozostaje
    /// **niezmierzony**, a nie „wykluczony rozumowaniem".
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
