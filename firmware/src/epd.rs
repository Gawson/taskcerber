//! Otoczka na epdiy dla panelu ED047TC1.
//!
//! epdiy dostaje magistralę I²C, którą stworzyliśmy sami (patrz [`crate::i2c`]), i sam
//! obsługuje na niej TPS65185 oraz port 1 ekspandera PCA9535.
//!
//! # Dwie pułapki upstreamu, obchodzone konstrukcyjnie
//!
//! **1. Nigdy nie używaj profilu `epd_board_v7`,** mimo że README epdiy i przykłady
//! LilyGo tak sugerują. `epd_board_v7.c` definiuje `D15=GPIO47, D14=21, D13=14, D12=13,
//! D11=12, D10=11` — czyli MISO, SCK, MOSI i CS karty SD oraz `BL_EN`. A `lcd_driver.c`
//! robi w `epd_deinit()`:
//!
//! ```c
//! for (size_t i = (16 - lcd.config.bus_width); i < 16; i++) {
//!     gpio_reset_pin(lcd.config.bus.data[i]);
//! }
//! ```
//!
//! Przy `bus_width == 8` iteruje po `i = 8..15` **surowej, nieprzestawionej tablicy**,
//! więc pod v7 resetowałoby magistralę SD i podświetlenie.
//!
//! **2. Nigdy nie wołaj `epd_deinit()`.** Pod `lilygo_board_s3` ta sama pętla woła
//! `gpio_reset_pin(GPIO_NUM_0)` osiem razy — nieszkodliwie, ale bez sensu. I tak nie
//! mamy zastosowania dla deinitu: deep sleep i tak resetuje peryferia, a nasze
//! wyłączanie to `epd_poweroff()` plus jawna izolacja GPIO. **Dlatego ten typ celowo
//! nie ma `impl Drop`.**

use anyhow::{bail, Result};
use dashboard::{Gray8, PACKED_LEN};
use esp_idf_svc::sys;

use crate::i2c::I2cBus;

/// Tryb odświeżania panelu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// Pełne, 16 odcieni. ~1–1,5 s. Czyści duchy. Używać przy zmianie treści.
    Full,
    /// Szybkie, dwupoziomowe. ~0,2–0,35 s. Zostawia duchy, więc co N razy trzeba
    /// wtrącić [`Refresh::Full`].
    Fast,
}

/// Ile odświeżeń [`Refresh::Fast`] wolno zrobić, zanim wymusimy pełne.
pub const FAST_REFRESHES_BEFORE_FULL: u8 = 12;

pub struct Epd {
    hl: sys::EpdiyHighlevelState,
    powered: bool,
}

impl Epd {
    /// Inicjalizuje epdiy na profilu `lilygo_board_s3` i panelu `ED047TC1`.
    ///
    /// `bus` musi żyć dłużej niż ten obiekt — epdiy trzyma surowy uchwyt.
    ///
    /// Rozmiar LUT to `EPD_LUT_1K`, nie 64K: tablica przebiegów siedzi w **wewnętrznym**
    /// DRAM-ie (`MALLOC_CAP_INTERNAL`, `render.c`), a te 63 KB różnicy są potrzebne
    /// buforom mbedTLS.
    pub fn new(bus: &I2cBus) -> Result<Self> {
        // SAFETY: uchwyt magistrali jest ważny i przeżyje epdiy; statyczne symbole
        // `lilygo_board_s3` i `ED047TC1` pochodzą z epdiy.
        let hl = unsafe {
            let i2c_cfg = sys::EpdI2cConfig {
                bus_handle: bus.raw(),
            };
            let init_cfg = sys::EpdInitConfig { i2c: &i2c_cfg };

            sys::epd_init_with_config(
                &sys::lilygo_board_s3,
                &sys::ED047TC1,
                sys::EpdInitOptions_EPD_LUT_1K,
                &init_cfg,
            );

            // VCOM panelu to -1,60 V; epdiy przyjmuje wartość bezwzględną w mV.
            sys::epd_set_vcom(1600);

            // null = wbudowany waveform panelu (EPD_BUILTIN_WAVEFORM).
            sys::epd_hl_init(std::ptr::null())
        };

        Ok(Self { hl, powered: false })
    }

    /// Rozmiar panelu według epdiy — sprawdzane przy bring-upie względem naszych stałych.
    pub fn dimensions(&self) -> (i32, i32) {
        // SAFETY: proste gettery bez stanu.
        unsafe { (sys::epd_width(), sys::epd_height()) }
    }

    /// Bufor roboczy epdiy: 4 bpp, 960/2 × 540 bajtów.
    fn framebuffer(&mut self) -> &mut [u8] {
        // SAFETY: epdiy alokuje ten bufor w `epd_hl_init` i trzyma przez całe życie
        // stanu; rozmiar wynika ze stałych panelu.
        unsafe {
            let ptr = sys::epd_hl_get_framebuffer(&mut self.hl);
            std::slice::from_raw_parts_mut(ptr, PACKED_LEN)
        }
    }

    /// Przepisuje płótno do bufora epdiy, obracając je po drodze zgodnie z orientacją
    /// zapisaną w samym płótnie.
    ///
    /// Obrót jest tutaj, a nie w `epd_set_rotation`, bo tamta funkcja przestawia
    /// wyłącznie prymitywy rysujące samego epdiy — a my piszemy prosto do
    /// framebuffera i tych prymitywów nie dotykamy.
    pub fn blit(&mut self, canvas: &Gray8) -> Result<()> {
        let fb = self.framebuffer();
        if fb.len() != PACKED_LEN {
            bail!(
                "epdiy zwróciło framebuffer o długości {}, oczekiwano {PACKED_LEN}",
                fb.len()
            );
        }
        canvas.pack4(fb);
        Ok(())
    }

    /// Zeruje bufor do bieli (0xFF = biały w kodowaniu epdiy).
    pub fn clear_buffer(&mut self) {
        self.framebuffer().fill(0xFF);
    }

    fn power_on(&mut self) {
        if !self.powered {
            // SAFETY: epdiy zainicjalizowane.
            unsafe { sys::epd_poweron() };
            self.powered = true;
        }
    }

    fn power_off(&mut self) {
        if self.powered {
            // SAFETY: jw.
            unsafe { sys::epd_poweroff() };
            self.powered = false;
        }
    }

    /// Rysuje klatkę na panelu — czyszczenie (jeśli trzeba), przepisanie bufora
    /// i wypchnięcie, wszystko na **jednym** podniesieniu szyn.
    ///
    /// `temperature_c` powinno pochodzić z BQ27220 — `epd_board_ambient_temperature()`
    /// w profilu `lilygo_board_s3` zwraca zaszyte na sztywno 20 °C, a przebiegi
    /// e-papieru są mocno zależne od temperatury.
    ///
    /// Szyny są podnoszone tylko na czas odświeżania. **Nigdy nie rób tego równolegle
    /// z nadawaniem WiFi** — panel ciągnie ~115 mA, a szczyt nadajnika to 283–340 mA;
    /// razem przez LDO przy rozładowanym ogniwie to generator brownoutów.
    ///
    /// # Dlaczego [`Refresh::Full`] czyści panel przy KAŻDYM wybudzeniu
    ///
    /// `epd_hl_update_screen` nie rysuje klatki — rysuje **różnicę** między `front_fb`
    /// a `back_fb`. Do `epd_draw_base` idą tablice `dirty_lines`/`dirty_columns`, więc
    /// niezmienione linie i kolumny są pomijane w całości, a piksel biały→biały nie
    /// dostaje żadnego przebiegu.
    ///
    /// `back_fb` powstaje w `epd_hl_init` i jest zerowany do `0xFF`, czyli do bieli.
    /// Oba bufory leżą w PSRAM (`MALLOC_CAP_SPIRAM`), a deep sleep gasi PSRAM. Każde
    /// wybudzenie to świeży `epd_hl_init`, więc epdiy zawsze startuje z założeniem
    /// „panel jest biały" — podczas gdy panel fizycznie trzyma poprzednią klatkę,
    /// bo e-papier nie potrzebuje zasilania, żeby ją utrzymać.
    ///
    /// Skutek jest widoczny gołym okiem: nowy tusz ląduje poprawnie, ale stary tusz
    /// tam, gdzie nowa klatka jest biała, nie dostaje przebiegu i zostaje. Na panelu
    /// widać **sumę** starej i nowej klatki. To nie są duchy — to nietknięte piksele.
    ///
    /// `epd_fullclear` przepędza panel fizycznie (`epd_clear_area` = 3 cykle po 22
    /// pchnięcia pełnym ekranem) i zostawia `back_fb` biały zgodnie z prawdą, więc
    /// następujący po nim GC16 rysuje pełną klatkę poprawnie.
    ///
    /// [`Refresh::Fast`] pomija czyszczenie i jest poprawne **wyłącznie jako drugie
    /// i kolejne rysowanie w obrębie tego samego wybudzenia** — dopiero wtedy `back_fb`
    /// naprawdę opisuje to, co jest na panelu.
    pub fn present(&mut self, canvas: &Gray8, mode: Refresh, temperature_c: i32) -> Result<()> {
        self.power_on();

        if mode == Refresh::Full {
            // SAFETY: epdiy zainicjalizowane, szyny podniesione. Ścieżka pustej różnicy
            // (front == back == biel) zwraca EPD_DRAW_SUCCESS, więc wewnętrzny assert
            // w epd_fullclear nie ma jak strzelić.
            unsafe { sys::epd_fullclear(&mut self.hl, temperature_c) };
        }

        let epd_mode = match mode {
            Refresh::Full => sys::EpdDrawMode_MODE_GC16,
            Refresh::Fast => sys::EpdDrawMode_MODE_DU,
        };

        let blitted = self.blit(canvas);
        let drawn = if blitted.is_ok() {
            // SAFETY: epdiy zainicjalizowane, szyny podniesione, bufor jego własny.
            unsafe { sys::epd_hl_update_screen(&mut self.hl, epd_mode, temperature_c) }
        } else {
            sys::EpdDrawError_EPD_DRAW_SUCCESS
        };

        // Cokolwiek się wyżej stało, szyny mają zejść.
        self.power_off();

        blitted?;
        if drawn != sys::EpdDrawError_EPD_DRAW_SUCCESS {
            bail!("epd_hl_update_screen zwróciło błąd {drawn}");
        }
        Ok(())
    }

    /// Gasi szyny panelu. Wołane w sekwencji przed snem, na wypadek gdyby
    /// [`Epd::flush`] wyszło ścieżką błędu.
    pub fn ensure_powered_off(&mut self) {
        self.power_off();
    }
}
