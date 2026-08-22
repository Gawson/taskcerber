//! Ładowarka / PMIC baterii BQ25896 pod adresem `0x6B`.
//!
//! Potrzebujemy z niej dwóch rzeczy: czy jest USB (co przełącza politykę zasilania
//! w tryb „nie oszczędzaj") i wyłączenia ciągłego ADC przed snem — ciągła konwersja
//! trzyma przy życiu wewnętrzny regulator REGN.

use anyhow::Result;

use crate::i2c::{I2cBus, I2cDevice};

pub const ADDRESS: u8 = 0x6B;

const REG02: u8 = 0x02; // ADC control
const REG03: u8 = 0x03; // konfiguracja: OTG_CONFIG, CHG_CONFIG, SYS_MIN
const REG07: u8 = 0x07; // watchdog i timery ładowania
const REG0B: u8 = 0x0B; // VBUS/CHRG status

const REG02_CONV_RATE: u8 = 1 << 6;
const REG03_OTG_CONFIG: u8 = 1 << 5;
const REG07_WATCHDOG: u8 = 0b11 << 4;
const REG0B_PG_STAT: u8 = 1 << 2;

/// `VBUS_STAT` mówiący, że kostka **sama wystawia 5 V** na VBUS z ogniwa.
const VBUS_STAT_OTG: u8 = 0b111;

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

impl PowerStatus {
    /// Czy ładowarka pracuje jako **podwyższalnik BAT→VBUS**, czyli oddaje prąd
    /// z ogniwa na gniazdo zamiast go pobierać.
    ///
    /// To nie jest sytuacja teoretyczna: pin `OTG` (8) jest na tej płytce sprzętowo
    /// podciągnięty do VSYS przez R25 10K, więc jedyną bramką boosta jest bit
    /// `OTG_CONFIG` w REG03. Firmware producenta kasuje go jawnie przy każdym starcie
    /// (`main.cpp:497`), my nigdy nie dotknęliśmy tego rejestru.
    pub fn boost_running(&self) -> bool {
        self.vbus_stat == VBUS_STAT_OTG
    }
}

/// Konfiguracja ładowarki — wyłącznie do logu, nic tu nie sterujemy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChargerConfig {
    pub reg03: u8,
    pub reg07: u8,
}

impl ChargerConfig {
    /// Czy załączony jest boost BAT→VBUS.
    pub fn otg_enabled(&self) -> bool {
        self.reg03 & REG03_OTG_CONFIG != 0
    }

    /// Nastawa watchdoga (REG07[5:4]). Zero znaczy wyłączony.
    ///
    /// Przy nastawie niezerowej kostka co ~40 s przywraca REG00–REG07 do wartości
    /// fabrycznych, więc **każda konfiguracja ładowarki jest nietrwała** — a ponieważ
    /// nie ustawiamy tu niczego, dziś objawia się to tylko tym, że stan REG03 nie jest
    /// nasz, lecz fabryczny.
    pub fn watchdog(&self) -> u8 {
        (self.reg07 & REG07_WATCHDOG) >> 4
    }
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

    /// Odczyt konfiguracji do raportu diagnostycznego. Nie zapisuje niczego.
    pub fn config(&self) -> Result<ChargerConfig> {
        Ok(ChargerConfig {
            reg03: self.dev.read_u8(REG03)?,
            reg07: self.dev.read_u8(REG07)?,
        })
    }

    /// Wyłącza ciągłą konwersję ADC. Wołane w sekwencji przed deep sleepem.
    pub fn disable_continuous_adc(&self) -> Result<()> {
        self.dev.update_u8(REG02, REG02_CONV_RATE, 0)?;
        Ok(())
    }
}
