//! Współdzielona magistrala I²C na GPIO39/40, na nowym sterowniku ESP-IDF.
//!
//! **Dlaczego nie `esp_idf_hal::i2c`:** `I2cDriver` używa starego API
//! (`i2c_param_config` / `i2c_driver_install`), a epdiy 2.1.3 wymaga
//! `i2c_master_bus_handle_t` z nowego sterownika. Te dwa sterowniki nie mogą
//! współistnieć na jednym porcie. Więc bierzemy magistralę na własność tutaj
//! i podajemy uchwyt do `epd_init_with_config()`.
//!
//! Na tej magistrali wiszą: PCA9535 `0x20`, RTC `0x51`, BQ27220 `0x55`,
//! GT911 `0x5D`, TPS65185 `0x68`, BQ25896 `0x6B`. TPS65185 i PCA9535 obsługuje
//! epdiy; resztą zajmujemy się sami.
//!
//! # Podciągnięcia magistrali
//!
//! Płytka MA rezystory zewnętrzne — sprawdzone doświadczalnie 2026-08-26: obraz
//! zbudowany z całkowicie wyłączonymi podciągnięciami wewnętrznymi znalazł wszystkie
//! pięć układów na magistrali, bez jednego błędu transmisji. Wcześniej było to
//! twierdzenie w komentarzu, bez źródła w żadnym dokumencie.
//!
//! Wewnętrzne zostawiamy mimo to włączone, jako siatkę bezpieczeństwa na wypadek
//! magistrali zawieszonej resetem w środku transakcji. Równolegle do zewnętrznych
//! są elektrycznie nieszkodliwe: ~45 kΩ obok kilku kiloomów praktycznie nie zmienia
//! sumy, a nota ESP-IDF sama nazywa je zbyt słabymi, żeby coś zepsuć.

use std::ptr;

use anyhow::{bail, Result};
use esp_idf_svc::sys::{
    self, i2c_device_config_t, i2c_master_bus_add_device, i2c_master_bus_config_t,
    i2c_master_bus_handle_t, i2c_master_dev_handle_t, i2c_master_transmit,
    i2c_master_transmit_receive, i2c_new_master_bus, i2c_port_num_t, ESP_OK,
};

/// SDA magistrali współdzielonej.
pub const SDA: i32 = 39;
/// SCL magistrali współdzielonej.
pub const SCL: i32 = 40;
/// Przerwanie z ekspandera PCA9535 (active-low, open-drain).
pub const PCA_INT: i32 = 38;

const PORT: i2c_port_num_t = 0;
const TIMEOUT_MS: i32 = 100;

/// Uchwyt do magistrali. Trzymany przez całe życie programu — epdiy dostaje
/// surowy wskaźnik i zakłada, że magistrala żyje dłużej niż on.
pub struct I2cBus {
    handle: i2c_master_bus_handle_t,
}

// Magistrala jest używana z jednego wątku aplikacji; epdiy dokłada swoje
// transakcje wyłącznie w obrębie wywołań, które robimy synchronicznie.
unsafe impl Send for I2cBus {}

impl I2cBus {
    /// Tworzy magistralę master na GPIO39/40 z wewnętrznymi podciągnięciami.
    ///
    /// Wywołać **raz**, na samym początku bootu, przed `epd_init_with_config()`.
    pub fn new() -> Result<Self> {
        let cfg = i2c_master_bus_config_t {
            i2c_port: PORT,
            sda_io_num: SDA,
            scl_io_num: SCL,
            // W wiązaniach `clk_source` jest w anonimowej unii, nie polem wprost:
            //   pub union i2c_master_bus_config_t__bindgen_ty_1 { pub clk_source: i2c_clock_source_t }
            __bindgen_anon_1: sys::i2c_master_bus_config_t__bindgen_ty_1 {
                clk_source: sys::soc_periph_i2c_clk_src_t_I2C_CLK_SRC_DEFAULT,
            },
            glitch_ignore_cnt: 7,
            // Podciągnięcia wewnętrzne deklarujemy TUTAJ, a nie dopisujemy po fakcie.
            //
            // Sterownik zapamiętuje tę flagę i przy KAŻDYM konfigurowaniu pinów robi
            // z niej użytek (`i2c_common.c:338` i `:352`): gdy jest fałszywa, woła
            // jawnie `gpio_pullup_dis()` na SDA i SCL. Poprzednia wersja włączała je
            // własnym `gpio_set_pull_mode` PO utworzeniu magistrali — działało, ale
            // każde ponowne skonfigurowanie pinów przez sterownik cicho by je zdjęło.
            // Ta sama flaga wycisza ostrzeżenie „Please check pull-up resistances",
            // które sterownik wypisuje dokładnie wtedy, gdy jest fałszywa
            // (`i2c_master.c:1066`) — u nas na każdym boocie.
            flags: {
                let mut f = sys::i2c_master_bus_config_t__bindgen_ty_2::default();
                f.set_enable_internal_pullup(1);
                f
            },
            intr_priority: 0,
            trans_queue_depth: 0, // 0 = tryb synchroniczny
            ..Default::default()
        };

        let mut handle: i2c_master_bus_handle_t = ptr::null_mut();
        // SAFETY: cfg jest poprawnie wypełniona, handle jest ważnym wskaźnikiem wyjściowym.
        let err = unsafe { i2c_new_master_bus(&cfg, &mut handle) };
        if err != ESP_OK {
            bail!("i2c_new_master_bus zwróciło {err}");
        }

        Ok(Self { handle })
    }

    /// Surowy uchwyt do przekazania epdiy przez `EpdI2cConfig`.
    pub fn raw(&self) -> i2c_master_bus_handle_t {
        self.handle
    }

    /// Dołącza urządzenie o podanym adresie 7-bitowym.
    pub fn device(&self, address: u8) -> Result<I2cDevice> {
        let cfg = i2c_device_config_t {
            dev_addr_length: sys::i2c_addr_bit_len_t_I2C_ADDR_BIT_LEN_7,
            device_address: address as u16,
            scl_speed_hz: 100_000,
            ..Default::default()
        };
        let mut dev: i2c_master_dev_handle_t = ptr::null_mut();
        // SAFETY: magistrala żyje, cfg poprawna.
        let err = unsafe { i2c_master_bus_add_device(self.handle, &cfg, &mut dev) };
        if err != ESP_OK {
            bail!("nie mogę dołączyć urządzenia I2C 0x{address:02X}: {err}");
        }
        Ok(I2cDevice {
            handle: dev,
            address,
        })
    }

    /// Skanuje magistralę i zwraca znalezione adresy. Diagnostyka bring-upu.
    pub fn scan(&self) -> Vec<u8> {
        let mut found = Vec::new();
        for addr in 0x08u8..0x78 {
            // SAFETY: probe jest bezpieczne dla dowolnego adresu.
            let err = unsafe { sys::i2c_master_probe(self.handle, addr as u16, 50) };
            if err == ESP_OK {
                found.push(addr);
            }
        }
        found
    }
}

/// Pojedyncze urządzenie na magistrali.
pub struct I2cDevice {
    handle: i2c_master_dev_handle_t,
    address: u8,
}

unsafe impl Send for I2cDevice {}

impl I2cDevice {
    pub fn address(&self) -> u8 {
        self.address
    }

    /// Zapisuje surowe bajty.
    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        // SAFETY: handle ważny, bufor żyje przez czas wywołania (synchroniczne).
        let err =
            unsafe { i2c_master_transmit(self.handle, bytes.as_ptr(), bytes.len(), TIMEOUT_MS) };
        if err != ESP_OK {
            bail!("zapis I2C do 0x{:02X} nie powiódł się: {err}", self.address);
        }
        Ok(())
    }

    /// Zapisuje adres rejestru i czyta odpowiedź.
    pub fn write_read(&self, out: &[u8], input: &mut [u8]) -> Result<()> {
        // SAFETY: jak wyżej; oba bufory żyją przez czas synchronicznego wywołania.
        let err = unsafe {
            i2c_master_transmit_receive(
                self.handle,
                out.as_ptr(),
                out.len(),
                input.as_mut_ptr(),
                input.len(),
                TIMEOUT_MS,
            )
        };
        if err != ESP_OK {
            bail!("odczyt I2C z 0x{:02X} nie powiódł się: {err}", self.address);
        }
        Ok(())
    }

    /// Czyta jeden rejestr 8-bitowy.
    pub fn read_u8(&self, reg: u8) -> Result<u8> {
        let mut buf = [0u8; 1];
        self.write_read(&[reg], &mut buf)?;
        Ok(buf[0])
    }

    /// Czyta parę rejestrów jako little-endian u16 (BQ27220 tak zwraca dane).
    pub fn read_u16_le(&self, reg: u8) -> Result<u16> {
        let mut buf = [0u8; 2];
        self.write_read(&[reg], &mut buf)?;
        Ok(u16::from_le_bytes(buf))
    }

    /// Zapisuje jeden rejestr 8-bitowy.
    pub fn write_u8(&self, reg: u8, value: u8) -> Result<()> {
        self.write(&[reg, value])
    }

    /// Read-modify-write pojedynczego rejestru.
    ///
    /// Używane wszędzie tam, gdzie dzielimy rejestr z epdiy albo gdzie część bitów
    /// jest nieudokumentowana i nie wolno jej wyzerować.
    pub fn update_u8(&self, reg: u8, mask: u8, value: u8) -> Result<u8> {
        let current = self.read_u8(reg)?;
        let updated = (current & !mask) | (value & mask);
        if updated != current {
            self.write_u8(reg, updated)?;
        }
        Ok(updated)
    }
}
