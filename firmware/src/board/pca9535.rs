//! Ekspander I/O PCA9535 pod adresem `0x20`.
//!
//! epdiy obsługuje **wyłącznie port 1** (sterowanie EPD: OE, MODE, PWRUP, VCOM_CTRL,
//! WAKEUP oraz odczyt PWR_GOOD i INT) — jego `pca9555_set_config`/`pca9555_set_value`
//! zawsze podają `high_port = 1`. Port 0 nie jest przez epdiy dotykany w ogóle,
//! a `lilygo_board_s3` ustawia `.gpio_write = NULL`, więc nie ma nawet API.
//!
//! **A na porcie 0, bit 0, siedzi `LORA_EN`** — zasilanie wspólnej szyny LoRa + GPS.
//! Rezystor `R21` (10 kΩ) podciąga `EN` tego LDO do VSYS, więc **szyna wstaje ZAŁĄCZONA
//! po zimnym starcie** i kosztuje 25–35 mA. To jest 40 godzin baterii. Dlatego pierwszą
//! transakcją I²C każdego bootu jest zgaszenie tego bitu.
//!
//! Mapy rejestrów PCA9535 / PCA9555 / XL9555 są identyczne, więc sterownik epdiy
//! działa na tej kostce bez zmian.

use anyhow::Result;

use crate::i2c::{I2cBus, I2cDevice};

pub const ADDRESS: u8 = 0x20;

// Rejestry standardowe PCA95xx.
const REG_INPUT_0: u8 = 0x00;
const REG_INPUT_1: u8 = 0x01;
const REG_OUTPUT_0: u8 = 0x02;
#[allow(dead_code)]
const REG_OUTPUT_1: u8 = 0x03;
const REG_CONFIG_0: u8 = 0x06;
const REG_CONFIG_1: u8 = 0x07;

/// Bit 0 portu 0: zasilanie szyny LoRa + GPS.
const BIT_LORA_EN: u8 = 1 << 0;

// Bity portu 1 — należą do epdiy, wymienione dla czytelności logów diagnostycznych.
pub mod port1 {
    pub const EPD_OE: u8 = 1 << 0;
    pub const EPD_MODE: u8 = 1 << 1;
    pub const BUTTON: u8 = 1 << 2;
    pub const TPS_PWRUP: u8 = 1 << 3;
    pub const VCOM_CTRL: u8 = 1 << 4;
    pub const TPS_WAKEUP: u8 = 1 << 5;
    pub const TPS_PWR_GOOD: u8 = 1 << 6;
    pub const TPS_INT: u8 = 1 << 7;
}

pub struct Pca9535 {
    dev: I2cDevice,
}

impl Pca9535 {
    pub fn new(bus: &I2cBus) -> Result<Self> {
        Ok(Self {
            dev: bus.device(ADDRESS)?,
        })
    }

    /// Gasi szynę LoRa/GPS. **Pierwsza rzecz po utworzeniu magistrali.**
    ///
    /// Modyfikuje wyłącznie bit 0 — reszta portu 0 jest nieudokumentowana i nie wolno
    /// jej ruszać.
    pub fn power_down_lora_gps(&self) -> Result<()> {
        // Bit 0 jako wyjście (0 = output w PCA95xx).
        self.dev.update_u8(REG_CONFIG_0, BIT_LORA_EN, 0)?;
        // Stan niski = LDO wyłączony.
        self.dev.update_u8(REG_OUTPUT_0, BIT_LORA_EN, 0)?;
        Ok(())
    }

    /// Załącza szynę LoRa/GPS. Potrzebne dopiero, gdy zaczniemy używać tych modułów.
    pub fn power_up_lora_gps(&self) -> Result<()> {
        self.dev.update_u8(REG_CONFIG_0, BIT_LORA_EN, 0)?;
        self.dev.update_u8(REG_OUTPUT_0, BIT_LORA_EN, BIT_LORA_EN)?;
        Ok(())
    }

    /// Przywraca bitowi przycisku kierunek „wejście". **Wołać po `Epd::new()`.**
    ///
    /// epdiy w `epd_board_init` robi:
    ///
    /// ```c
    /// // set all epdiy lines to output except TPS interrupt + PWR good
    /// pca9555_set_config(pca, CFG_PIN_PWRGOOD | CFG_PIN_INT, 1);
    /// ```
    ///
    /// czyli zapisuje **cały** rejestr konfiguracji portu 1, zostawiając jako wejścia
    /// wyłącznie bity 6 i 7. Bit 2 — nasz przycisk — staje się wyjściem, a
    /// `epd_set_config_register` wpisuje tam potem zero. Efekt: [`Self::button_pressed`]
    /// czyta poziom, który sama płytka wystawia, i zwraca `true` bez końca, niezależnie
    /// od tego, czy ktokolwiek coś nacisnął.
    ///
    /// Odzyskanie bitu jest bezpieczne, bo **epdiy z niego nie korzysta**: jego stałe
    /// `CFG_PIN_*` to bity 0, 1, 3, 4, 5, 6 i 7. Bit 2 wpada wyłącznie do tablicy
    /// `others[]`, którą epdiy zeruje i nigdy nie czyta. Rejestru konfiguracji nie
    /// dotyka też później — `epd_poweron`/`epd_poweroff` piszą do rejestru wyjściowego,
    /// a zapis do wyjścia pinu ustawionego jako wejście nie robi nic.
    pub fn reclaim_button_input(&self) -> Result<()> {
        // W PCA95xx 1 = wejście.
        self.dev
            .update_u8(REG_CONFIG_1, port1::BUTTON, port1::BUTTON)?;
        Ok(())
    }

    /// Czyta przycisk funkcyjny `S3` (port 1, bit 2, aktywny stanem niskim).
    ///
    /// Wymaga wcześniejszego [`Self::reclaim_button_input`], jeśli panel był już
    /// inicjalizowany — inaczej odczyt jest bez wartości.
    pub fn button_pressed(&self) -> Result<bool> {
        let p1 = self.dev.read_u8(REG_INPUT_1)?;
        Ok(p1 & port1::BUTTON == 0)
    }

    /// Surowe porty wejściowe — do logu diagnostycznego przy bring-upie.
    pub fn read_inputs(&self) -> Result<(u8, u8)> {
        Ok((
            self.dev.read_u8(REG_INPUT_0)?,
            self.dev.read_u8(REG_INPUT_1)?,
        ))
    }

    /// Czy TPS65185 zgłasza power-good.
    pub fn tps_power_good(&self) -> Result<bool> {
        let p1 = self.dev.read_u8(REG_INPUT_1)?;
        Ok(p1 & port1::TPS_PWR_GOOD != 0)
    }
}
