//! Raport z bring-upu — wszystko, co da się rozstrzygnąć **samym wgraniem firmware'u**.
//!
//! Ten moduł powstał pod konkretne ograniczenie: jest płytka i jest kabel, ale nie ma
//! miernika ani możliwości obejrzenia oznaczeń na kostkach. Okazuje się, że to wciąż
//! wystarcza na większość pytań z `docs/bringup.md` — pod warunkiem, że urządzenie
//! samo powie, co widzi, zamiast wypisywać surowe liczby do interpretacji.
//!
//! Stąd dwie rzeczy:
//!
//! * **Skan I²C jest interpretowany, nie wypisywany.** Lista `[20, 51, 55, ...]`
//!   wymaga trzymania obok `docs/hardware.md` §6; lista z nazwami i słowem CISZA przy
//!   brakujących mówi wprost, co jest nie tak.
//! * **Zużycie energii liczymy z licznika kulombów BQ27220**, który jest na płytce.
//!   Nie zastąpi PPK2 w rozdzielczości, ale odpowiada na jedyne pytanie, które
//!   naprawdę boli — czy dobowe zużycie mieści się w budżecie — i robi to po dobie,
//!   a nie po tygodniu.
//!
//! Czego **nie** da się stąd wyczytać: prądu chwilowego w deep sleepie (MCU śpi, więc
//! nie ma kto czytać), szczytów przy nadawaniu (BQ27220 uśrednia), różnicy z kartą SD
//! i bez niej. To zostaje dla miernika.

use log::{info, warn};

use crate::board::{
    bq25896::PowerStatus,
    bq27220::{self, Fuel},
    Board,
};
use crate::i2c::I2cBus;
use crate::power::rtc_state::RtcState;

/// Układy, których spodziewamy się na magistrali — `docs/hardware.md` §6.
const EXPECTED: &[(u8, &str)] = &[
    (0x20, "PCA9535   ekspander I/O"),
    (0x51, "PCF8563   zegar RTC"),
    (0x55, "BQ27220   licznik ogniwa"),
    (0x5D, "GT911     dotyk"),
    (0x68, "TPS65185  PMIC panelu"),
    (0x6B, "BQ25896   ładowarka"),
];

/// Adres, pod którym GT911 ląduje, gdy `INT` był w GÓRZE przy zwolnieniu `RST`.
///
/// Widok tego adresu zamiast `0x5D` to jednoznaczna diagnoza: sekwencja resetu
/// w `board::gt911` ustawiła `INT` odwrotnie, niż zakłada.
const GT911_ALT: u8 = 0x14;

/// Pełny raport, wypisywany przy zimnym starcie.
///
/// Każda linia odpowiada na jedno pytanie z `docs/bringup.md`, żeby dało się je
/// odhaczać z monitora zamiast zgadywać z rozsypanych logów.
pub fn cold_boot_report(bus: &I2cBus, hw: &Board, boot_ms: u128) {
    info!("=== raport bring-upu ===");

    // --- magistrala I²C ----------------------------------------------------
    let found = bus.scan();
    info!("I2C: znaleziono {} urządzeń", found.len());
    for (addr, name) in EXPECTED {
        if found.contains(addr) {
            info!("  0x{addr:02X}  {name}");
        } else {
            warn!("  0x{addr:02X}  {name}  <-- CISZA");
        }
    }
    if found.contains(&GT911_ALT) {
        warn!(
            "  0x{GT911_ALT:02X}  GT911 pod ADRESEM ZAPASOWYM — sekwencja resetu \
             ustawiła INT odwrotnie niż zakłada board::gt911"
        );
    }
    for addr in &found {
        if !EXPECTED.iter().any(|(a, _)| a == addr) && *addr != GT911_ALT {
            warn!("  0x{addr:02X}  <-- NIEOCZEKIWANY, nie ma go w docs/hardware.md §6");
        }
    }

    // --- RTC: rozstrzygnięcie sprzeczności w źródłach vendora ---------------
    // README vendora mówi PCF85063, schemat mówi PCF8563, mapy rejestrów się różnią.
    // `probe_variant` odpowiada na to bez patrzenia na kostkę.
    info!("RTC: wariant {:?}", hw.rtc.probe_variant());
    match hw.rtc.voltage_low() {
        Ok(true) => warn!("RTC: flaga VL ustawiona — zegar stracił zasilanie, czas jest śmieciem"),
        Ok(false) => info!("RTC: flaga VL czysta, czas wygląda na ciągły"),
        Err(e) => warn!("RTC: nie mogę odczytać flagi VL: {e:#}"),
    }

    // --- ekspander ---------------------------------------------------------
    match hw.expander.read_inputs() {
        Ok((p0, p1)) => info!("PCA9535: port0={p0:#010b} port1={p1:#010b}"),
        Err(e) => warn!("PCA9535: brak odczytu: {e:#}"),
    }

    // --- PSRAM: epdiy alokuje przez assert(), więc zła konfiguracja to abort --
    match psram_size() {
        size if size == 8 * 1024 * 1024 => info!("PSRAM: {size} B — zgodnie z oczekiwaniem"),
        0 => warn!("PSRAM: 0 B — octal PSRAM nie wstało, epdiy zaraz padnie na assert"),
        size => warn!("PSRAM: {size} B, oczekiwano 8388608 — sprawdź CONFIG_SPIRAM_MODE_OCT"),
    }

    // Pomiar 5 z bringup.md: boot do gotowości. >600 ms oznacza memtest PSRAM albo
    // walidację obrazu przy każdym wybudzeniu.
    info!("boot do gotowości: {boot_ms} ms");

    info!("=== koniec raportu ===");
}

/// Jedna linia na wybudzenie: zasilanie, ogniwo, i uśredniony pobór od linii bazowej.
///
/// Uśrednienie jest jedynym sposobem, żeby wycisnąć coś sensownego z licznika
/// o rozdzielczości 1 mAh: przy 155 µA pojedyncze wybudzenie to 0,08 mAh, czyli
/// poniżej najmniejszego kroku. Dopiero po kilku godzinach różnica przestaje być
/// szumem — i wtedy odpowiada na pomiary 10 i 11 z `docs/bringup.md`, bez miernika.
pub fn energy_line(state: &mut RtcState, power: PowerStatus, fuel: Fuel, now_unix: i64) {
    info!(
        "zasilanie: USB={} ogniwo={:?}% {:?} mV {:?} mA {:?} °C",
        power.usb_present, fuel.percent, fuel.millivolts, fuel.milliamps, fuel.temperature_c
    );

    let Some(remaining) = fuel.remaining_mah else {
        return;
    };

    // Linia bazowa: brak, albo ogniwo zostało w międzyczasie podładowane. Ładowanie
    // unieważnia pomiar — od tego momentu liczymy od nowa.
    if state.energy_start_unix == 0 || remaining > state.energy_start_mah || now_unix <= 0 {
        state.energy_start_unix = now_unix;
        state.energy_start_mah = remaining;
        info!("pomiar energii: linia bazowa {remaining} mAh");
        return;
    }

    let seconds = now_unix - state.energy_start_unix;
    let used = state.energy_start_mah.saturating_sub(remaining);
    if seconds <= 0 {
        return;
    }

    let hours = seconds as f32 / 3600.0;
    if used == 0 {
        info!("pomiar energii: {hours:.1} h, zużycie poniżej rozdzielczości licznika (<1 mAh)");
        return;
    }

    let per_day = used as f32 * 24.0 / hours;
    let average_ua = used as f32 * 1_000_000.0 / (hours * 1000.0);
    info!(
        "pomiar energii: {used} mAh przez {hours:.1} h  ->  {per_day:.1} mAh/dobę, \
         średnio {average_ua:.0} µA"
    );

    // Progi z bringup.md, pomiar 10. Warto je mieć w logu, a nie w pamięci czytającego.
    if hours >= 6.0 {
        if per_day > 25.0 {
            warn!("pomiar energii: >25 mAh/dobę — coś w sekwencji wyłączania nie ląduje");
        } else if per_day > 10.0 {
            warn!("pomiar energii: {per_day:.1} mAh/dobę, budżet zakłada ~7");
        }
    }
}

fn psram_size() -> usize {
    // SAFETY: prosty getter z ESP-IDF.
    unsafe { esp_idf_svc::sys::esp_psram_get_size() }
}

/// Konfiguracja ładowarki i licznika ogniwa — dwa pytania, na które nasz kod nigdy
/// nie odpowiadał, a od których zależy otwarte dochodzenie w sprawie prądu snu.
///
/// # Dlaczego osobno od [`cold_boot_report`]
///
/// Tamten drukuje się wyłącznie przy zimnym starcie, a przy pracy z kablem zimny start
/// zdarza się rzadko — i tak czy owak USB-CDC gubi pierwsze kilkaset milisekund wyjścia,
/// zanim host zdąży się podpiąć. Odczyty trafiały więc w próżnię dokładnie wtedy, gdy
/// były potrzebne. Pięć rejestrów kosztuje ułamek milisekundy, więc na kablu wołamy to
/// przy każdym wybudzeniu; na baterii zostaje przy zimnym starcie.
pub fn hardware_config_report(hw: &Board) {
    // --- ładowarka: czy sami nie wystawiamy 5 V na gniazdo ------------------
    // Pin OTG jest podciągnięty do VSYS przez R25 10K, więc boost BAT→VBUS blokuje
    // wyłącznie bit OTG_CONFIG w REG03. Producent kasuje go przy każdym starcie, my
    // tego rejestru nigdy nie dotknęliśmy — a pracujący podwyższalnik bez odbiornika
    // jest jedyną znaną nam pozycją zdolną wyjaśnić rząd wielkości prądu snu.
    match hw.charger.status() {
        Ok(s) => {
            if s.boost_running() {
                warn!(
                    "BQ25896: VBUS_STAT=0b{:03b} — kostka WYSTAWIA 5 V z ogniwa (OTG)",
                    s.vbus_stat
                );
            } else {
                info!(
                    "BQ25896: VBUS_STAT=0b{:03b} CHRG_STAT=0b{:02b} USB={}",
                    s.vbus_stat, s.chrg_stat, s.usb_present
                );
            }
        }
        Err(e) => warn!("BQ25896: brak odczytu stanu: {e:#}"),
    }
    match hw.charger.config() {
        Ok(c) => {
            info!(
                "BQ25896: REG03={:#04X} (OTG_CONFIG={}) REG07={:#04X} (watchdog={})",
                c.reg03,
                c.otg_enabled() as u8,
                c.reg07,
                c.watchdog()
            );
            if c.otg_enabled() {
                warn!("BQ25896: OTG_CONFIG załączony — to jest podejrzany numer jeden");
            }
        }
        Err(e) => warn!("BQ25896: brak odczytu konfiguracji: {e:#}"),
    }

    // --- licznik ogniwa: czy w ogóle wie, co obsługuje ----------------------
    let p = hw.fuel.provisioning();
    info!(
        "BQ27220: DesignCapacity={:?} mAh, FullCharge={:?} mAh, OperationStatus={:?}",
        p.design_mah,
        p.full_charge_mah,
        p.operation_status.map(|v| format!("{v:#06X}"))
    );
    match p.zna_ogniwo() {
        Some(false) => warn!(
            "BQ27220: profil NIE odpowiada ogniwu {} mAh — procent naładowania jest \
             zaniżony, a od niego zależą progi trybów pracy",
            bq27220::Provisioning::NOMINALNA_MAH
        ),
        Some(true) => info!("BQ27220: profil zgodny z ogniwem z tabliczki"),
        None => warn!("BQ27220: nie mogę odczytać DesignCapacity"),
    }
}
