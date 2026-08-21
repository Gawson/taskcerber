//! Logika urządzenia, która nie potrzebuje ESP-IDF.
//!
//! # Po co osobny crate
//!
//! `firmware/` jest **osobnym workspace'em** celującym w `xtensa-esp32s3-espidf`
//! (patrz komentarz przy `exclude` w głównym `Cargo.toml`). Konsekwencja, którą
//! łatwo przeoczyć: `#[cfg(test)] mod tests` w firmwarze **nigdy się nie kompiluje
//! ani nie uruchamia** — `cargo test` w tamtym katalogu budowałby testy na Xtensę,
//! a `cargo build` testów nie dotyka. Testy potrafiły tam siedzieć miesiącami
//! z błędem kompilacji i nikt się o tym nie dowiadywał.
//!
//! Wszystko, co da się rozstrzygnąć bez sprzętu — kiedy sięgać po sieć, kiedy wolno
//! wgrać nowy obraz, jak rozwiązać adres z manifestu — mieszka więc tutaj i jest
//! testowane na hoście, tak samo jak `dashboard` i `icalfeed`.

pub mod boot;
pub mod ota;
pub mod policy;

mod redact;

pub use redact::redact;

/// CRC32 (IEEE) — do wykrywania, czy treść kalendarza się zmieniła.
///
/// Własna implementacja tablicowa zamiast crate'a: to dwadzieścia linii, a każda
/// zależność w buildzie firmware'u kosztuje minuty kompilacji ESP-IDF.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_zgodne_z_referencja() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
    }

    #[test]
    fn crc_wykrywa_zmiane() {
        assert_ne!(crc32(b"BEGIN:VEVENT"), crc32(b"BEGIN:VEVEN_"));
    }
}
