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
use dashboard::{Gray8, Rect, PACKED_LEN};
use esp_idf_svc::sys;

use crate::i2c::I2cBus;

/// Zegar magistrali danych panelu w MHz. `None` = zostaw to, co deklaruje epdiy.
///
/// # Po co to jest przestawialne
///
/// Rzecz na czas bring-upu, jak `SLEEP_MARKER` w `main`. Na zdjęciach panelu widać
/// **jaśniejsze pasy biegnące prostopadle do linii bramkowych** — czyli na stałych
/// pozycjach w ścieżce danych, a nie za treścią. To wyklucza duchy po poprzednim
/// obrazie i wskazuje na magistralę.
///
/// Podejrzenie jest konkretne: `ED047TC1` deklaruje `bus_speed = 20`, ale dopóki
/// linia cache'u danych była domyślna (32 B), epdiy **po cichu połowiło zegar do
/// 10 MHz** (`lcd_driver.c`). Ustawienie `CONFIG_ESP32S3_DATA_CACHE_LINE_64B`
/// odblokowało pełne 20 MHz — i dopiero od tego momentu panel jedzie z prędkością,
/// przy której `line_front_porch = 4` takty mogą nie wystarczać na ustalenie się
/// poziomów na wyjściach sterowników źródłowych.
///
/// **Sprawdzone 2026-08-20: przy 10 MHz pasy są IDENTYCZNE.** Magistrala nie ma
/// z tym nic wspólnego i ten trop jest zamknięty — zegar wraca na deklarowane
/// 20 MHz. Przełącznik zostaje, bo koszt jest zerowy, a przy następnej zagadce
/// z obrazem to pierwsza rzecz, którą warto wykluczyć jednym wgraniem.
///
/// **Koszt**: czas odświeżania jest wprost proporcjonalny do okresu linii, więc przy
/// 10 MHz wszystko trwa DWA RAZY dłużej — DU ~68 ms zamiast ~35, pełne odświeżenie
/// ~1,8 s zamiast ~0,9.
const PANEL_BUS_MHZ: Option<i32> = None;

/// Do ilu pikseli wyrównujemy obszar częściowego odświeżenia. Patrz
/// [`Epd::present_area`].
const AREA_ALIGN: i32 = 8;

/// Otoczka prostokątna zbioru obszarów. Zakłada niepustą listę.
fn hull_of(areas: &[Rect]) -> Rect {
    let mut r = areas[0];
    for a in &areas[1..] {
        let x = r.x.min(a.x);
        let y = r.y.min(a.y);
        r = Rect::new(
            x,
            y,
            r.right().max(a.right()) - x,
            r.bottom().max(a.bottom()) - y,
        );
    }
    r
}

/// Przycina obszar do panelu i wyrównuje do [`AREA_ALIGN`]. `None`, gdy po przycięciu
/// nic nie zostaje — wtedy nie ma czego odświeżać.
fn clamp_area(r: Rect) -> Option<Rect> {
    let w = dashboard::PANEL_WIDTH as i32;
    let h = dashboard::PANEL_HEIGHT as i32;

    let x0 = (r.x.max(0) / AREA_ALIGN) * AREA_ALIGN;
    let y0 = r.y.clamp(0, h);
    let x1 = (r.right().min(w) + AREA_ALIGN - 1) / AREA_ALIGN * AREA_ALIGN;
    let x1 = x1.min(w);
    let y1 = r.bottom().clamp(0, h);

    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

/// Tryb odświeżania panelu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// Pełne, 16 odcieni. `epd_fullclear` + GC16 = **126 przebiegów** przez wszystkie
    /// bramki, ~0,9 s przy zegarze 20 MHz. Czyści duchy. Nigdy w środku interakcji.
    Full,
    /// Szybkie, dwupoziomowe. MODE_DU to **5 faz**, ~35 ms — czyli o rząd wielkości
    /// taniej niż `Full`, a nie „trochę". Zostawia duchy, więc co jakiś czas trzeba
    /// wtrącić [`Refresh::Full`] — ale w przerwie, nie w rytmie naciśnięć.
    Fast,
}

/// Ile odświeżeń [`Refresh::Fast`] wolno zrobić, zanim wymusimy pełne.
pub const FAST_REFRESHES_BEFORE_FULL: u8 = 12;

/// Definicja panelu przekazywana do `epd_init_with_config`.
///
/// Zwraca wprost statyk epdiy, chyba że [`PANEL_BUS_MHZ`] każe przestawić zegar —
/// wtedy oddaje własną kopię z podmienioną jedną wartością.
///
/// # Dlaczego to musi być `'static`, a nie zmienna lokalna
///
/// `epd_init_with_config` NIE kopiuje struktury, tylko zapamiętuje wskaźnik
/// (`display = disp;` w `epdiy.c`), a `epd_lcd_init` czyta z niego `bus_speed`
/// przy każdym przeliczaniu taktowania. Struktura na stosie zwisłaby natychmiast
/// po powrocie z tej funkcji. `Box::leak` jest tu właściwym narzędziem, a nie
/// obejściem: obiekt ma żyć do końca działania programu, `Epd` istnieje w jednym
/// egzemplarzu, a urządzenie i tak kończy pracę deep sleepem, który zeruje RAM.
fn display_definition() -> &'static sys::EpdDisplay_t {
    // SAFETY: `ED047TC1` to stała inicjalizowana przed startem programu.
    let base = unsafe { std::ptr::read(std::ptr::addr_of!(sys::ED047TC1)) };

    match PANEL_BUS_MHZ {
        Some(mhz) if mhz != base.bus_speed => {
            log::warn!(
                "zegar magistrali panelu przestawiony z {} na {mhz} MHz — bring-up, \
                 odświeżanie będzie proporcjonalnie wolniejsze",
                base.bus_speed
            );
            let mut over = base;
            over.bus_speed = mhz;
            Box::leak(Box::new(over))
        }
        // SAFETY: bierzemy adres stałej o statycznym czasie życia.
        _ => unsafe { &*std::ptr::addr_of!(sys::ED047TC1) },
    }
}

pub struct Epd {
    hl: sys::EpdiyHighlevelState,
    powered: bool,
    /// Blokada zejścia szyn między odświeżeniami — patrz [`Epd::hold_power`].
    power_hold: bool,
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
        let display = display_definition();

        let hl = unsafe {
            let i2c_cfg = sys::EpdI2cConfig {
                bus_handle: bus.raw(),
            };
            let init_cfg = sys::EpdInitConfig { i2c: &i2c_cfg };

            sys::epd_init_with_config(
                &sys::lilygo_board_s3,
                display,
                sys::EpdInitOptions_EPD_LUT_1K,
                &init_cfg,
            );

            // VCOM panelu to -1,60 V; epdiy przyjmuje wartość bezwzględną w mV.
            sys::epd_set_vcom(1600);

            // null = wbudowany waveform panelu (EPD_BUILTIN_WAVEFORM).
            sys::epd_hl_init(std::ptr::null())
        };

        Ok(Self {
            hl,
            powered: false,
            power_hold: false,
        })
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

    /// Zostawia szyny panelu podniesione między kolejnymi odświeżeniami.
    ///
    /// Sekwencja `epd_poweron`/`epd_poweroff` to rozmowa po I²C z TPS65185 i czekanie
    /// na power-good — kilkadziesiąt milisekund w każdą stronę. Przy pełnej klatce
    /// to nic; przy wpisywaniu z klawiatury, gdzie samo odświeżenie obszaru trwa tyle
    /// samo, to podwaja czas odpowiedzi na każdy klawisz.
    ///
    /// **Nie zostawiać podniesionych szyn na dłuższą bezczynność** — panel ciągnie
    /// wtedy ~115 mA, a HV na szkle bez odświeżania nie służy panelowi. Wołający ma
    /// zwolnić blokadę, gdy tylko przestaje pisać; [`Epd::ensure_powered_off`] robi
    /// to bezwarunkowo.
    pub fn hold_power(&mut self, hold: bool) {
        self.power_hold = hold;
        if !hold {
            self.power_off();
        }
    }

    fn power_off(&mut self) {
        if self.power_hold {
            return;
        }
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

    /// Odświeża wskazany prostokąt panelu, w trybie DU.
    ///
    /// # Obszar NIE kupuje czasu na szkle — tylko czas procesora
    ///
    /// Wcześniej stało tu, że „obszar wielkości klawisza to ułamek czasu pełnej
    /// klatki, bo `epd_draw_base` dostaje krótszy zakres linii". **To była nieprawda
    /// i na tym nieporozumieniu stała cała konstrukcja odpowiedzi na dotyk.**
    ///
    /// `epd_draw_base` ustawia `render_context.lines_total = rounded_display_height()`
    /// bezwarunkowo, a `render_lcd.c` linii spoza obszaru nie pomija — wypełnia jej
    /// bufor zerami i i tak commituje. Bramki panelu są taktowane wszystkie, zawsze.
    /// Czas przebiegu zależy więc wyłącznie od liczby faz waveformu:
    ///
    /// | tryb | fazy | czas |
    /// |---|---|---|
    /// | `MODE_DU` | 5 | ~35 ms |
    /// | `MODE_GC16` | 30 | ~210 ms |
    /// | `epd_fullclear` (GC16 + `epd_clear`) | 96 | ~680 ms |
    ///
    /// (Liczby dla zegara piksela 20 MHz, czyli przy `CONFIG_ESP32S3_DATA_CACHE_LINE_64B`.
    /// Przy domyślnej linii cache'u 32 B epdiy PO CICHU połowi zegar do 10 MHz —
    /// `lcd_driver.c:405-424` — i wszystkie powyższe czasy się podwajają.)
    ///
    /// Wniosek praktyczny: dzielenie zmiany na dwa wypchnięcia (mignięcie pod palcem,
    /// a potem treść) **podwaja** czas, zamiast go skracać. Wolno to robić wyłącznie
    /// tam, gdzie akcja sama z siebie nic na ekranie nie zmienia.
    ///
    /// `area` jest we współrzędnych **panelu** — przelicza je
    /// [`dashboard::Rotation::canvas_rect_to_panel`]. Prostokąt jest przycinany do
    /// panelu i wyrównywany do ośmiu pikseli: przy 4 bpp jeden bajt to dwa piksele,
    /// a magistrala pakuje po cztery, więc obszar nierówny do bajtu potrafi zostawić
    /// pionowy szew na krawędzi.
    ///
    /// **Wolno wołać dopiero po pełnym odświeżeniu w tym samym wybudzeniu.** Wcześniej
    /// `back_fb` epdiy jest wyzerowany do bieli i nie opisuje tego, co jest na szkle —
    /// różnica wyszłaby wtedy z niewłaściwego punktu odniesienia. Pilnuje tego wołający.
    pub fn present_area(&mut self, canvas: &Gray8, area: Rect, temperature_c: i32) -> Result<()> {
        self.present_areas(canvas, &[area], temperature_c)
    }

    /// Kilka prostokątów na **jednym** podniesieniu szyn.
    ///
    /// Naciśnięcie klawisza zmienia dwa miejsca naraz — sam klawisz i pole, w którym
    /// pojawia się znak — a sekwencja `epd_poweron`/`epd_poweroff` kosztuje tyle samo
    /// co niejeden z tych prostokątów. Robienie tego dwa razy pod rząd byłoby
    /// zmarnowaniem połowy czasu odpowiedzi.
    pub fn present_areas(
        &mut self,
        canvas: &Gray8,
        areas: &[Rect],
        temperature_c: i32,
    ) -> Result<()> {
        let areas: Vec<Rect> = areas.iter().copied().filter_map(clamp_area).collect();
        if areas.is_empty() {
            return Ok(());
        }

        self.power_on();
        let mut blitted = Ok(());
        for area in &areas {
            // Pakujemy TYLKO te prostokąty. Przepakowanie całego płótna kosztuje
            // więcej niż samo odświeżenie — patrz `Gray8::pack4_rect`.
            if let Err(e) = self.blit_rect(canvas, *area) {
                blitted = Err(e);
                break;
            }
        }

        // JEDNO wywołanie na całą otoczkę, nie po jednym na prostokąt.
        //
        // `epd_hl_update_area` wygląda, jakby koszt zależał od obszaru, i tak jest
        // opisane w nagłówku epdiy. Nie jest: funkcja wylicza prostokąt zmian, po czym
        // **jawnie go wyrzuca** i woła `epd_draw_base(epd_full_screen(), ...)`, licząc
        // wyłącznie na tablicę `dirty_lines`. A w ścieżce wyjściowej LCD czysta linia
        // też dostaje wysłany bufor (`render_lcd.c`, gałąź `!drawn_lines[...]` wypełnia
        // go zerami i commituje) — bramki panelu są taktowane wszystkie, zawsze.
        //
        // Czyli: obszar wpływa na koszt PROCESORA (mniejsza różnica do policzenia,
        // mniej linii do przygotowania), ale czas przebiegu na szkle jest stały
        // i równy pełnemu DU. Trzy wywołania to trzy pełne przebiegi — przy pisaniu
        // z klawiatury dokładnie tyle, ile widać jako opóźnienie.
        let hull = hull_of(&areas);
        // SAFETY: epdiy zainicjalizowane, szyny podniesione, obszar przycięty
        // do wymiarów panelu i o dodatnich bokach.
        let drawn = unsafe {
            sys::epd_hl_update_area(
                &mut self.hl,
                sys::EpdDrawMode_MODE_DU,
                temperature_c,
                sys::EpdRect {
                    x: hull.x,
                    y: hull.y,
                    width: hull.w,
                    height: hull.h,
                },
            )
        };

        self.power_off();

        blitted?;
        if drawn != sys::EpdDrawError_EPD_DRAW_SUCCESS {
            bail!("epd_hl_update_area zwróciło błąd {drawn}");
        }
        Ok(())
    }

    /// Przepisuje do bufora epdiy sam wskazany prostokąt panelu.
    fn blit_rect(&mut self, canvas: &Gray8, area: Rect) -> Result<()> {
        let fb = self.framebuffer();
        if fb.len() != PACKED_LEN {
            bail!(
                "epdiy zwróciło framebuffer o długości {}, oczekiwano {PACKED_LEN}",
                fb.len()
            );
        }
        canvas.pack4_rect(fb, area);
        Ok(())
    }

    /// Powtarza `epd_fullclear` wskazaną liczbę razy, na jednym podniesieniu szyn.
    ///
    /// Narzędzie bring-upowe do jednego konkretnego pytania: **czy jaśniejsze pasy
    /// na tle da się w ogóle skasować?** Jeśli słabną z każdym przebiegiem, to jest
    /// nieskasowana historia poprzednich odświeżeń częściowych i trzeba czyścić
    /// agresywniej. Jeśli po pięciu przebiegach wyglądają tak samo — siedzą
    /// w szkle albo w VCOM-ie i czyszczeniem się ich nie ruszy.
    ///
    /// Jedno wywołanie to 96 przelotów przez wszystkie bramki, czyli ~0,7 s.
    pub fn deep_clean(&mut self, times: u32, temperature_c: i32) {
        if times == 0 {
            return;
        }
        self.power_on();
        for _ in 0..times {
            // SAFETY: epdiy zainicjalizowane, szyny podniesione.
            unsafe { sys::epd_fullclear(&mut self.hl, temperature_c) };
        }
        self.power_off();
    }

    /// Gasi szyny panelu. Wołane w sekwencji przed snem, na wypadek gdyby
    /// [`Epd::flush`] wyszło ścieżką błędu.
    pub fn ensure_powered_off(&mut self) {
        self.power_hold = false;
        self.power_off();
    }
}
