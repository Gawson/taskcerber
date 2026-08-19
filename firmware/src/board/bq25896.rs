//! Ładowarka / PMIC baterii BQ25896 pod adresem `0x6B`.
//!
//! Potrzebujemy z niej dwóch rzeczy: czy jest USB (co przełącza politykę zasilania
//! w tryb „nie oszczędzaj") i wyłączenia ciągłego ADC przed snem — ciągła konwersja
//! trzyma przy życiu wewnętrzny regulator REGN.

use anyhow::Result;

use crate::i2c::{I2cBus, I2cDevice};

pub const ADDRESS: u8 = 0x6B;

const REG02: u8 = 0x02; // ADC control
const REG0B: u8 = 0x0B; // VBUS/CHRG status

const REG02_CONV_RATE: u8 = 1 << 6;
const REG0B_PG_STAT: u8 = 1 << 2;

pub struct Bq25896 {
    dev: I2cDevice,
}

/// Stan zasilania widziany przez ładowarkę.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerStatus {
    /// Zasilanie zewnętrzne obecne i dobre (`PG_STAT`).
    pub usb_present: bool,
    /// Surowe `VBUS_STAT` (REG0B[7:5]) — do logu.
    pub vbus_stat: u8,
    /// Surowe `CHRG_STAT` (REG0B[4:3]).
    pub chrg_stat: u8,
}

impl Bq25896 {
    pub fn new(bus: &I2cBus) -> Result<Self> {
        Ok(Self {
            dev: bus.device(ADDRESS)?,
        })
    }

    pub fn status(&self) -> Result<PowerStatus> {
        let r = self.dev.read_u8(REG0B)?;
        Ok(PowerStatus {
            usb_present: r & REG0B_PG_STAT != 0,
            vbus_stat: (r >> 5) & 0b111,
            chrg_stat: (r >> 3) & 0b11,
        })
    }

    /// Wyłącza ciągłą konwersję ADC. Wołane w sekwencji przed deep sleepem.
    pub fn disable_continuous_adc(&self) -> Result<()> {
        self.dev.update_u8(REG02, REG02_CONV_RATE, 0)?;
        Ok(())
    }
}
