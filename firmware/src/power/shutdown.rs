//! Sekwencja wyłączania przed każdym `esp_deep_sleep_start()`.
//!
//! Kolejność nie jest kosmetyczna. To jest różnica między ~155 µA a ~873 µA — a przy
//! nieudanym kroku 1 nawet między 155 µA a **235 mA**, czyli między dwoma miesiącami
//! a pięcioma godzinami.
//!
//! Numeracja kroków odpowiada `docs/power.md`.

use anyhow::Result;
use esp_idf_svc::sys;
use log::warn;

use crate::board::Board;
use crate::epd::Epd;

/// Piny magistrali równoległej panelu plus linie timingu.
///
/// **Krok 1 i najważniejszy.** Sterownik źródłowy panelu ma diody zaciskające na
/// wejściach; zostawiony wysoki poziom na którejkolwiek z tych linii po zgaszeniu
/// szyn oddaje prąd z powrotem do panelu. epdiy #136 zmierzył tą drogą **235 mA**
/// na pokrewnej płytce T5-4.7. Izolujemy je **przed czymkolwiek innym**.
const EPD_PINS: [i32; 13] = [
    5, 6, 7, 15, 16, 17, 18, 8,  // D0..D7
    4,  // CKH / WR
    41, // STH
    42, // LEH
    45, // STV (pin strapujący — patrz niżej)
    48, // CKV
];

/// Podświetlenie panelu.
const BL_EN: i32 = 11;
/// Reset kontrolera dotyku GT911.
const TOUCH_RST: i32 = 9;

/// Zdejmuje zatrzaski z magistrali panelu. **Woła się przy każdym starcie, przed
/// dotknięciem panelu.**
///
/// [`isolate_epd_bus`] woła `rtc_gpio_isolate()`, a ta funkcja — cytat z `rtc_io.h` —
/// „disables input, output, pullup, pulldown, and **enables hold feature** for an
/// RTC IO". Zatrzask przeżywa deep sleep i reset po wybudzeniu; zwalnia go dopiero
/// jawne `rtc_gpio_hold_dis()`.
///
/// Zdolne do RTC na ESP32-S3 są GPIO0–21, czyli z naszej listy: D0..D7 i CKH.
/// epdiy zwalnia z nich **jeden** — `epd_board_init` robi `gpio_hold_dis(CKH)`
/// z komentarzem „free CKH after wakeup". D0..D7 zostają zatrzaśnięte.
///
/// Skutek: LCD_CAM steruje magistralą przez matrycę GPIO, ale pady nie przepuszczają
/// zmian, więc panel dostaje śmieci. Na szkle wychodzą z tego pasy — po **każdym**
/// wybudzeniu z deep sleepu, nie tylko po przycisku. Pierwszy start po wgraniu
/// firmware'u jest czysty, bo reset z zasilania żadnych zatrzasków nie zostawia,
/// i to właśnie maskuje błąd przy bring-upie.
///
/// STH, LEH, STV i CKV nie są pinami RTC, więc `isolate_epd_bus` ich nie zatrzaskuje,
/// a `gpio_deep_sleep_hold_en()` działa — zgodnie z `gpio.h` — wyłącznie na padach,
/// dla których wołano wcześniej `gpio_hold_en()`. Te wołamy tylko dla podświetlenia
/// i resetu dotyku, i te zostają trzymane celowo.
pub fn release_epd_bus_hold() {
    for pin in EPD_PINS {
        // SAFETY: numery pinów są stałe i poprawne dla tej płytki. Dla pinów spoza
        // domeny RTC `gpio_hold_dis` zwraca błąd, który tu nie ma znaczenia — te piny
        // i tak nigdy nie zostały zatrzaśnięte.
        unsafe {
            if sys::rtc_gpio_is_valid_gpio(pin) {
                sys::rtc_gpio_hold_dis(pin);
            } else {
                sys::gpio_hold_dis(pin);
            }
        }
    }
}

/// Przeprowadza pełną sekwencję wyłączania.
///
/// Błędy pojedynczych kroków są logowane, ale nie przerywają sekwencji — lepiej
/// wykonać pozostałe kroki i zasnąć z podwyższonym prądem, niż nie zasnąć wcale.
pub fn prepare_for_deep_sleep(epd: &mut Epd, board: &Board, keep_touch_alive: bool) {
    // 1. Zgaś szyny panelu i odizoluj jego magistralę. NAJPIERW.
    epd.ensure_powered_off();
    isolate_epd_bus();

    // 2. Port 1 ekspandera (EPD_OE, EPD_MODE, TPS_PWRUP, VCOM_CTRL, TPS_WAKEUP)
    //    należy do epdiy i został już opuszczony przez `epd_poweroff()`.
    //    Sprawdzamy tylko, czy TPS faktycznie zszedł.
    match board.expander.tps_power_good() {
        Ok(true) => warn!("TPS65185 nadal zgłasza power-good po epd_poweroff — szyna EPD stoi"),
        Ok(false) => {}
        Err(e) => warn!("nie mogę odczytać power-good z PCA9535: {e:#}"),
    }

    // 3. Szyna LoRa/GPS. Podciągnięcie R21 wstaje załączone przy zimnym starcie,
    //    ale po miękkim resecie stan mógł zostać — więc gasimy bezwarunkowo.
    if let Err(e) = board.expander.power_down_lora_gps() {
        warn!("nie mogę zgasić szyny LoRa/GPS: {e:#}");
    }

    // 4. Podświetlenie.
    set_low_and_hold(BL_EN);

    // 5. GT911 trzymany w resecie.
    //    Tryb uśpienia samego kontrolera kosztuje 70–120 µA; reset kosztuje zero,
    //    ale wtedy nie ma budzenia dotykiem.
    if !keep_touch_alive {
        set_low_and_hold(TOUCH_RST);
    }

    // 6. Licznik energii do trybu SLEEP: 50 µA -> 9 µA.
    //    (BQ27220 przechodzi w SLEEP automatycznie przy małym prądzie; jawnej
    //    komendy nie wysyłamy, żeby nie namieszać w konfiguracji licznika.)

    // 7. Ładowarka: wyłącz ciągłą konwersję ADC, bo trzyma przy życiu REGN.
    if let Err(e) = board.charger.disable_continuous_adc() {
        warn!("nie mogę wyłączyć ciągłego ADC w BQ25896: {e:#}");
    }

    // 8. Utrwal stany GPIO na czas snu i zgaś domenę peryferiów RTC.
    // SAFETY: wywołania bezstanowe z ESP-IDF, wołane raz przed samym snem.
    unsafe {
        sys::gpio_deep_sleep_hold_en();
        sys::esp_sleep_pd_config(
            sys::esp_sleep_pd_domain_t_ESP_PD_DOMAIN_RTC_PERIPH,
            sys::esp_sleep_pd_option_t_ESP_PD_OPTION_OFF,
        );
    }

    // 9. NIGDY nie gaś ESP_PD_DOMAIN_VDDSDIO — to niszczy PSRAM, a przy deep sleepie
    //    i tak nic nie daje.
}

/// Ustawia piny magistrali panelu w stan wysokiej impedancji.
fn isolate_epd_bus() {
    for pin in EPD_PINS {
        // SAFETY: numery pinów są stałe i poprawne dla tej płytki.
        unsafe {
            sys::gpio_set_direction(pin, sys::gpio_mode_t_GPIO_MODE_INPUT);
            sys::gpio_set_pull_mode(pin, sys::gpio_pull_mode_t_GPIO_FLOATING);

            // rtc_gpio_isolate odcina też wewnętrzne podciągnięcia domeny RTC.
            // Działa tylko dla pinów zdolnych do RTC; dla pozostałych zwraca błąd,
            // który tutaj świadomie ignorujemy.
            if sys::rtc_gpio_is_valid_gpio(pin) {
                sys::rtc_gpio_isolate(pin);
            }
        }
    }
    // GPIO45 (STV) jest pinem strapującym VDD_SPI. Zostawiamy go pływającego,
    // a nie podciągniętego — poziom przy resecie decyduje o napięciu VDD_SPI.
}

fn set_low_and_hold(pin: i32) {
    // SAFETY: numer pinu stały, wywołania bezstanowe.
    unsafe {
        sys::gpio_set_direction(pin, sys::gpio_mode_t_GPIO_MODE_OUTPUT);
        sys::gpio_set_level(pin, 0);
        sys::gpio_hold_en(pin);
    }
}

/// Zasypia na podaną liczbę sekund.
///
/// Nie wraca.
pub fn deep_sleep_for(seconds: u64) -> ! {
    // SAFETY: wywołania ESP-IDF; esp_deep_sleep_start nie wraca.
    unsafe {
        sys::esp_sleep_enable_timer_wakeup(seconds * 1_000_000);
        // Bindgen wygenerował to jako `-> !`, więc funkcja tu się kończy.
        sys::esp_deep_sleep_start()
    }
}

/// Włącza budzenie przyciskiem BOOT (GPIO0, aktywny stanem niskim).
///
/// Wołane przed [`deep_sleep_for`], żeby dało się obudzić urządzenie ręcznie
/// bez czekania na timer.
pub fn enable_button_wakeup() -> Result<()> {
    const BOOT: u64 = 1 << 0;
    // SAFETY: maska dotyczy istniejącego pinu zdolnego do RTC.
    unsafe {
        sys::esp_sleep_enable_ext1_wakeup_io(
            BOOT,
            sys::esp_sleep_ext1_wakeup_mode_t_ESP_EXT1_WAKEUP_ANY_LOW,
        );
    }
    Ok(())
}
