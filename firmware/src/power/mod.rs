//! Polityka zasilania — cienka warstwa nad [`devlogic::policy`].
//!
//! Sama polityka (progi naładowania, okno nocne, długość snu, bramka OTA) mieszka
//! w `devlogic`, bo nie potrzebuje ESP-IDF i dzięki temu jest testowana na hoście.
//! Tutaj zostaje wyłącznie to, czego bez płytki nie ma: zamiana odczytów
//! z ładowarki i licznika ogniwa na prymitywy, które ta polityka przyjmuje.
//!
//! Cała arytmetyka energetyczna i cztery błędy, które kosztują 10–200× budżetu,
//! są w `docs/power.md`.

pub mod rtc_state;
pub mod shutdown;

pub use devlogic::policy::{align_to_minute, Mode, Policy};

use chrono::NaiveDateTime;

use crate::board::bq25896::PowerStatus;
use crate::board::bq27220::Fuel;

/// Wybiera tryb pracy z odczytów układów na płytce.
///
/// `fuel.percent` bywa `None`, gdy licznik ogniwa nie odpowiada na I²C, i polityka
/// traktuje to jak zły stan — nie jak brak informacji do pominięcia.
pub fn mode_from_hardware(
    policy: &Policy,
    power: PowerStatus,
    fuel: Fuel,
    now: NaiveDateTime,
) -> Mode {
    policy.mode(power.usb_present, fuel.percent, now)
}

/// Czy wolno pobrać aktualizację firmware'u przy tym stanie ogniwa.
pub fn may_update(policy: &Policy, mode: Mode, fuel: Fuel) -> bool {
    policy.should_update(mode, fuel.percent)
}
