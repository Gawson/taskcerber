//! Sekwencja wyłączania przed każdym `esp_deep_sleep_start()`.
//!
//! Kolejność nie jest kosmetyczna. To jest różnica między ~155 µA a ~873 µA — a przy
//! nieudanym kroku 1 nawet między 155 µA a **235 mA**, czyli między dwoma miesiącami
//! a pięcioma godzinami.
//!
//! Numeracja kroków odpowiada `docs/power.md`.

use anyhow::Result;
use esp_idf_svc::sys;
use log::{info, warn};

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
/// Przerwanie kontrolera dotyku. Pin w domenie RTC, więc **może** budzić z deep sleepu.
const TOUCH_INT: i32 = 3;

/// Przycisk BOOT — drugie źródło `ext1`, obok `TOUCH_INT`.
const BOOT_BTN: i32 = 0;

/// Zdejmuje zatrzaski GPIO założone przed snem. **Woła się przy każdym starcie,
/// przed dotknięciem panelu i przed sekwencją resetu dotyku.**
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
/// STH, LEH, STV i CKV nie są pinami RTC, więc `isolate_epd_bus` ich nie zatrzaskuje.
///
/// # Druga połowa tej samej pułapki: `BL_EN` i `TOUCH_RST`
///
/// [`prepare_for_deep_sleep`] **celowo** zatrzaskuje te dwa piny w dole
/// (`set_low_and_hold`) — podświetlenie ma nie świecić, a GT911 w resecie kosztuje
/// zero zamiast 70–120 µA. Obydwa są pinami zdolnymi do RTC, więc `gpio_hold_en`
/// schodzi w ESP-IDF do `rtc_gpio_hold_en` (`esp_driver_gpio/src/gpio.c`), a taki
/// zatrzask **przeżywa deep sleep i wybudzenie**.
///
/// Nie zdejmuje go nic z tego, co wygląda, jakby powinno: `gpio_reset_pin` woła
/// `rtc_gpio_deinit`, a ta przełącza wyłącznie multiplekser funkcji na cyfrowy
/// (`esp_driver_gpio/src/rtc_io.c`) — bitu HOLD nie rusza. `gpio_set_direction`
/// i `gpio_set_level` zapisują rejestry, których pad w zatrzasku nie czyta.
///
/// Skutek dla dotyku jest dokładnie taki: **pierwszy start po wgraniu firmware'u
/// ma dotyk, każdy następny już nie.** Reset z zasilania czyści domenę RTC i maskuje
/// błąd przy bring-upie; po pierwszym własnym śnie `T_RST` zostaje przybity do zera,
/// GT911 nigdy nie wstaje i `Gt911::new` kończy się „nie odpowiada ani pod 0x5D,
/// ani pod 0x14". Przycisk obrotu działa dalej, bo wisi na ekspanderze I²C — czyli
/// objaw wygląda na zepsuty wyłącznie kontroler dotyku.
///
/// Firmware vendora robi to samo w pierwszych trzech liniach `setup()`:
/// `gpio_hold_dis(BOARD_TOUCH_RST); gpio_hold_dis(BOARD_LORA_RST); gpio_deep_sleep_hold_dis();`
pub fn release_pin_holds() {
    // Piny źródeł `ext1` — osobno i INNYM wywołaniem, bo mają inny problem niż reszta.
    //
    // `ext1_wakeup_prepare` w ESP-IDF przestawia KAŻDY pin z maski na funkcję RTC
    // (`rtcio_hal_function_select`), a że usypiamy z RTC_PERIPH wyłączoną, dokłada
    // do tego `rtcio_hal_hold_enable`. Oba piny wracają więc z deep sleepu
    // przemuxowane na RTC i zatrzaśnięte — a na tej liście ich do tej pory NIE BYŁO.
    //
    // Samo `hold_dis` by nie wystarczyło: zatrzask schodzi, ale mux zostaje na RTC,
    // dopóki ktoś nie zawoła `rtc_gpio_deinit` — a robi to dopiero `gpio_reset_pin`.
    // To ta sama para wywołań, której używa `gt911::reset_sequence`, i ta sama
    // pułapka, którą opisuje `docs/hardware.md` §5b.
    //
    // Skutek pominięcia był podstępny: próbka `T_INT` w [`enable_wakeup`] czytała
    // pad odcięty od ścieżki cyfrowej, dostawała fałszywe zero i po cichu WYCINAŁA
    // dotyk z maski wybudzeń. Od tej chwili budził wyłącznie BOOT, a dotyk wracał
    // dopiero po cyklu, w którym otworzyło się okno dotyku — bo dopiero ono woła
    // sekwencję resetu GT911 i przy okazji odkręca GPIO3. Z zewnątrz wyglądało to
    // jak „trzeba raz albo dwa razy wdusić BOOT, żeby zaczął reagować".
    //
    // NIE `gpio_reset_pin`, mimo że `gt911::reset_sequence` używa właśnie jej.
    // Tam pin zaraz potem staje się wyjściem i podciągnięcie nie ma znaczenia; tutaj
    // zostaje wejściem — a `gpio_reset_pin` WŁĄCZA podciągnięcie. GT911 próbkuje
    // `T_INT` przy wychodzeniu z resetu, żeby wybrać adres I²C, więc podciągnięty pin
    // sadzał kontroler pod 0x14 zamiast 0x5D. Diagnostyka złapała to sama:
    // „GT911 pod ADRESEM ZAPASOWYM — sekwencja resetu ustawiła INT odwrotnie".
    //
    // `rtc_gpio_deinit` przywraca ścieżkę cyfrową i NIE dotyka podciągnięć, czyli
    // robi dokładnie to, o co chodzi, i nic ponadto.
    for pin in [BOOT_BTN, TOUCH_INT] {
        // SAFETY: numery pinów stałe, oba w domenie RTC, wywołania bezstanowe.
        unsafe {
            sys::gpio_hold_dis(pin);
            sys::rtc_gpio_hold_dis(pin);
            sys::rtc_gpio_deinit(pin);
        }
    }

    for pin in EPD_PINS.into_iter().chain([BL_EN, TOUCH_RST]) {
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

    // Globalna zgoda na trzymanie padów CYFROWYCH przez sen, założona przez
    // `gpio_deep_sleep_hold_en()` w kroku 8. Zdejmuje ją tylko jawne wywołanie albo
    // reset z zasilania — inaczej STH, LEH, STV i CKV zostają w stanie sprzed snu.
    // SAFETY: wywołanie bezstanowe z ESP-IDF.
    unsafe {
        sys::gpio_deep_sleep_hold_dis();
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
    // Odczyt DWA RAZY, z przerwą — i to nie jest ostrożność na wyrost.
    //
    // `PG` czytane natychmiast po `epd_poweroff` ściga się z rozładowaniem szyny:
    // VPOS/VNEG schodzą przez rezystory upływowe, a nie w zerowym czasie, więc
    // pojedyncze „power-good = 1" nie odróżnia szyny STOJĄCEJ od szyny, która
    // właśnie schodzi. Ta różnica jest warta setek miliamperów przez cały sen,
    // więc nie ma sensu jej zgadywać.
    //
    // Drugi odczyt po 50 ms rozstrzyga: jeśli `PG` opadło, to był wyścig i wszystko
    // jest w porządku. Jeśli nadal stoi — szyna naprawdę nie zeszła i log mówi to
    // słowem, którego nie da się pomylić z ostrzeżeniem o niczym.
    match board.expander.tps_power_good() {
        Ok(true) => {
            std::thread::sleep(std::time::Duration::from_millis(50));
            match board.expander.tps_power_good() {
                Ok(true) => warn!(
                    "SZYNA EPD STOI: TPS65185 zgłasza power-good 50 ms po epd_poweroff. \
                     To jest pobór rzędu setek miliamperów przez cały sen"
                ),
                Ok(false) => {
                    info!("szyna EPD zeszła w ciągu 50 ms — pierwszy odczyt był wyścigiem")
                }
                Err(e) => warn!("nie mogę powtórzyć odczytu power-good: {e:#}"),
            }
        }
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

    // 5. GT911: reset albo praca dalej.
    //
    //    Reset kosztuje zero prądu, ale wtedy dotyk nie budzi — a urządzenie, którego
    //    nie da się obudzić dotknięciem, wygląda po prostu na zepsute. Zostawiony przy
    //    życiu kontroler skanuje dalej i to widać w budżecie: rząd wielkości więcej
    //    niż cała reszta uśpionego urządzenia.
    //
    //    Zatrzask jest konieczny w OBU wariantach. Bez niego pad wraca po zaśnięciu
    //    do stanu domyślnego i kontroler wpada w reset niezależnie od tego, czego
    //    chcieliśmy.
    if keep_touch_alive {
        set_high_and_hold(TOUCH_RST);
    } else {
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
    set_level_and_hold(pin, 0);
}

fn set_high_and_hold(pin: i32) {
    set_level_and_hold(pin, 1);
}

fn set_level_and_hold(pin: i32, level: u32) {
    // SAFETY: numer pinu stały, wywołania bezstanowe.
    unsafe {
        sys::gpio_set_direction(pin, sys::gpio_mode_t_GPIO_MODE_OUTPUT);
        sys::gpio_set_level(pin, level);
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

/// Włącza budzenie przyciskiem BOOT, a jeśli się da — także **dotknięciem ekranu**.
///
/// BOOT (GPIO0) jest jedynym przyciskiem tej płytki, który w ogóle trafia na pin MCU
/// zdolny do RTC — reszta wisi na I²C albo na `EN`, patrz `docs/hardware.md` §5b.
/// Ale `T_INT` z GT911 siedzi na GPIO3, też w domenie RTC, więc dotyk może być drugim
/// źródłem `ext1` i jest to jedyny sposób, żeby urządzenie dawało się obudzić bez
/// szukania przycisku po omacku.
///
/// # Dlaczego poziom `INT` jest sprawdzany, a nie zakładany
///
/// GT911 ma cztery tryby przerwania, wybierane w jego konfiguracji (`0x804D`), i dwa
/// z nich trzymają `INT` w **dole** w spoczynku. Przy takim trybie maska `ANY_LOW`
/// wyzwoliłaby się natychmiast po zaśnięciu i urządzenie wpadłoby w pętlę wybudzeń,
/// która rozładowuje ogniwo w kilka godzin i wygląda jak losowe restarty. Dlatego
/// pin jest odczytywany tuż przed snem i budzenie dotykiem włącza się **tylko wtedy,
/// gdy linia faktycznie stoi wysoko**. Gdy stoi nisko, zostaje sam BOOT i log mówi,
/// dlaczego.
///
/// `keep_touch_alive` musi być zgodne z tym, co dostał [`prepare_for_deep_sleep`] —
/// kontroler trzymany w resecie nie ma czym przerwać.
pub fn enable_wakeup(keep_touch_alive: bool) -> Result<()> {
    let mut mask = 1u64 << BOOT_BTN;

    if keep_touch_alive {
        // Podciągnięcie, zanim cokolwiek odczytamy. Bez niego `T_INT` bywa pinem
        // pływającym — gdy w tym cyklu nie otwierało się okno dotyku, kontroler nie
        // dostał sekwencji resetu i niczego nie steruje. Pływający pin z maską
        // `ANY_LOW` to generator losowych wybudzeń. Podciągnięcie nie maskuje przy tym
        // przypadku, przed którym się bronimy: kontroler trzymający `INT` w dole
        // w spoczynku przeciąga słabe podciągnięcie i nadal odczytamy zero.
        // SAFETY: numer pinu stały, wywołania bezstanowe.
        let idle_high = unsafe {
            sys::gpio_set_direction(TOUCH_INT, sys::gpio_mode_t_GPIO_MODE_INPUT);
            sys::gpio_set_pull_mode(TOUCH_INT, sys::gpio_pull_mode_t_GPIO_PULLUP_ONLY);
            std::thread::sleep(std::time::Duration::from_millis(2));
            sys::gpio_get_level(TOUCH_INT) != 0
        };
        if idle_high {
            mask |= 1 << TOUCH_INT;
            info!("budzenie: BOOT albo dotyk");
        } else {
            warn!(
                "budzenie: sam BOOT — T_INT stoi nisko w spoczynku, więc maska ANY_LOW \
                 wyzwoliłaby się natychmiast. Tryb przerwania GT911 jest w rejestrze 0x804D"
            );
        }
    } else {
        info!("budzenie: sam BOOT");
    }

    // SAFETY: maska dotyczy istniejących pinów zdolnych do RTC.
    unsafe {
        sys::esp_sleep_enable_ext1_wakeup_io(
            mask,
            sys::esp_sleep_ext1_wakeup_mode_t_ESP_EXT1_WAKEUP_ANY_LOW,
        );
    }
    Ok(())
}

/// Co nas obudziło: `ext1` z BOOT-a, `ext1` z dotyku, czy timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wakeup {
    Timer,
    Button,
    Touch,
}

impl Wakeup {
    /// Czy przy urządzeniu ktoś jest. Timer nie znaczy nic, jedno i drugie `ext1` — tak.
    pub fn by_human(self) -> bool {
        !matches!(self, Self::Timer)
    }
}

/// Odczytuje źródło wybudzenia.
pub fn wakeup_source() -> Wakeup {
    // SAFETY: proste gettery z ESP-IDF.
    let (cause, mask) = unsafe {
        (
            sys::esp_sleep_get_wakeup_cause(),
            sys::esp_sleep_get_ext1_wakeup_status(),
        )
    };
    if cause != sys::esp_sleep_source_t_ESP_SLEEP_WAKEUP_EXT1 {
        return Wakeup::Timer;
    }
    if mask & (1 << TOUCH_INT) != 0 {
        Wakeup::Touch
    } else {
        Wakeup::Button
    }
}
