//! Stan przeżywający deep sleep, trzymany w pamięci RTC-FAST.
//!
//! Deep sleep gasi PSRAM, więc framebuffer i cały sterta znikają. Zostaje 8 KB pamięci
//! RTC-SLOW i pamięć RTC-FAST — dość na kilkaset bajtów semantyki, dzięki którym
//! kolejne wybudzenie nie musi zaczynać od zera:
//!
//! * **Zbuforowany BSSID i kanał AP** — asocjacja bez pełnego skanu to różnica
//!   ~300 mAs na wybudzenie, czyli największa pojedyncza dźwignia w budżecie.
//! * **CRC ostatniej pobranej treści** — jeśli kalendarz się nie zmienił, pomijamy
//!   odświeżenie panelu (360 mAs -> 210 mAs).
//! * **Licznik szybkich odświeżeń** — po N trzeba wtrącić pełne, inaczej zostają duchy.
//! * **Licznik kolejnych porażek sieci** — wykładnicze wycofanie.
//!
//! ## Uwaga na `--gc-sections`
//! Sekcja `.rtc.data` bywa wycinana przez linker, jeśli nic jej wprost nie używa.
//! Test przeżywalności [`RtcState::load`] sprawdza magiczne słowo i przy niezgodności
//! po prostu zaczyna od zera — czyli w najgorszym razie tracimy optymalizacje,
//! a nie poprawność.

use log::info;

// Bump przy KAŻDEJ zmianie układu `RtcState`. Stary stan w pamięci RTC ma inny
// rozmiar i przesunięcia pól; bez zmiany magii zostałby odczytany jako śmieci.
const MAGIC: u32 = 0x5435_5F32; // "T5_2"

/// Ile razy z rzędu sieć musi zawieść, zanim wydłużymy odstępy.
pub const FAILURES_BEFORE_BACKOFF: u8 = 3;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RtcState {
    magic: u32,
    /// Ile razy urządzenie wstało od zimnego startu.
    pub boot_count: u32,
    /// Ostatni znany czas unix — awaryjne źródło, gdy RTC zawiedzie.
    pub last_known_unix: i64,
    /// CRC32 ostatniej pobranej treści kalendarza.
    pub last_content_crc: u32,
    /// Ile szybkich odświeżeń od ostatniego pełnego.
    pub fast_refreshes: u8,
    /// Ile kolejnych porażek sieci.
    pub net_failures: u8,
    /// Czy zbuforowany AP jest ważny.
    pub ap_cached: bool,
    /// Kanał zbuforowanego AP.
    pub ap_channel: u8,
    /// BSSID zbuforowanego AP.
    pub ap_bssid: [u8; 6],
    /// Czas ostatniego udanego pobrania (unix), 0 = nigdy.
    pub last_success_unix: i64,
    /// CRC32 wersji, którą próbujemy wgrać przez OTA. 0 = żadnej.
    pub ota_target_crc: u32,
    /// Ile razy próbowaliśmy wgrać tę wersję.
    pub ota_attempts: u8,
}

// SAFETY: struktura jest `repr(C)` i złożona wyłącznie z typów POD; leży w pamięci
// RTC-FAST, do której dostęp mamy jednowątkowo, zanim wystartują inne zadania.
#[link_section = ".rtc.data"]
static mut RTC_STATE: RtcState = RtcState {
    magic: 0,
    boot_count: 0,
    last_known_unix: 0,
    last_content_crc: 0,
    fast_refreshes: 0,
    net_failures: 0,
    ap_cached: false,
    ap_channel: 0,
    ap_bssid: [0; 6],
    last_success_unix: 0,
    ota_target_crc: 0,
    ota_attempts: 0,
};

impl RtcState {
    /// Czyta stan z pamięci RTC. Przy zimnym starcie (albo gdy linker wyciął sekcję)
    /// zwraca świeży stan i zaznacza to w logu.
    pub fn load() -> Self {
        // SAFETY: jednowątkowy dostęp na początku bootu, przed startem innych zadań.
        let mut state = unsafe { core::ptr::read_volatile(&raw const RTC_STATE) };

        if state.magic != MAGIC {
            info!("stan RTC pusty lub nieważny — zimny start");
            state = Self {
                magic: MAGIC,
                boot_count: 0,
                last_known_unix: 0,
                last_content_crc: 0,
                fast_refreshes: 0,
                net_failures: 0,
                ap_cached: false,
                ap_channel: 0,
                ap_bssid: [0; 6],
                last_success_unix: 0,
                ota_target_crc: 0,
                ota_attempts: 0,
            };
        }

        state.boot_count = state.boot_count.wrapping_add(1);
        state
    }

    /// Zapisuje stan z powrotem. Wołać tuż przed zaśnięciem.
    pub fn store(&self) {
        // SAFETY: jw. — jednowątkowo, tuż przed deep sleepem.
        unsafe { core::ptr::write_volatile(&raw mut RTC_STATE, *self) };
    }

    /// Zapamiętuje AP, z którym się udało połączyć.
    pub fn cache_ap(&mut self, bssid: [u8; 6], channel: u8) {
        self.ap_bssid = bssid;
        self.ap_channel = channel;
        self.ap_cached = true;
    }

    /// Unieważnia bufor AP — po nieudanej próbie połączenia z zapamiętanym punktem.
    pub fn invalidate_ap(&mut self) {
        self.ap_cached = false;
    }

    /// Rejestruje udane pobranie.
    pub fn record_success(&mut self, unix: i64, content_crc: u32) {
        self.net_failures = 0;
        self.last_success_unix = unix;
        self.last_content_crc = content_crc;
    }

    /// Rejestruje porażkę sieci.
    pub fn record_failure(&mut self) {
        self.net_failures = self.net_failures.saturating_add(1);
    }

    /// Mnożnik odstępu przy kolejnych porażkach: 1x, potem 2x, potem 12x.
    ///
    /// Nieudana asocjacja kosztuje 200–480 mAs, czyli **więcej niż udane pobranie**.
    /// Martwy AP przy odpytywaniu co 30 minut to 12,8 mAh/dobę wypalone za zero
    /// informacji.
    pub fn backoff_multiplier(&self) -> u64 {
        match self.net_failures {
            0..=2 => 1,
            3..=5 => 2,
            _ => 12,
        }
    }

    /// Czy wymusić pełne odświeżenie zamiast szybkiego.
    pub fn needs_full_refresh(&self) -> bool {
        self.fast_refreshes >= crate::epd::FAST_REFRESHES_BEFORE_FULL
    }

    /// Czy wolno jeszcze próbować OTA do wskazanej wersji.
    ///
    /// Zabezpiecza przed pętlą, w której manifest obiecuje wersję, a wgrany obraz
    /// raportuje inną — bez licznika urządzenie pobierałoby 3 MB co cykl aż do
    /// rozładowania ogniwa.
    pub fn ota_allowed(&self, version: &str) -> bool {
        let crc = crc32(version.as_bytes());
        crc != self.ota_target_crc || self.ota_attempts < crate::net::ota::MAX_ATTEMPTS
    }

    /// Odnotowuje próbę wgrania wskazanej wersji.
    pub fn record_ota_attempt(&mut self, version: &str) {
        let crc = crc32(version.as_bytes());
        if crc == self.ota_target_crc {
            self.ota_attempts = self.ota_attempts.saturating_add(1);
        } else {
            self.ota_target_crc = crc;
            self.ota_attempts = 1;
        }
    }

    /// Zeruje licznik prób — wołane, gdy działamy już na wersji z manifestu.
    pub fn clear_ota_attempts(&mut self) {
        self.ota_target_crc = 0;
        self.ota_attempts = 0;
    }

    /// Odnotowuje wykonane odświeżenie.
    pub fn record_refresh(&mut self, full: bool) {
        if full {
            self.fast_refreshes = 0;
        } else {
            self.fast_refreshes = self.fast_refreshes.saturating_add(1);
        }
    }
}

/// CRC32 (IEEE) — do wykrywania, czy treść kalendarza się zmieniła.
///
/// Własna implementacja tablicowa zamiast crate'a: to dwadzieścia linii, a każda
/// zależność w tym buildzie kosztuje minuty kompilacji ESP-IDF.
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

    #[test]
    fn wycofanie_rosnie_z_porazkami() {
        let mut s = RtcState {
            magic: 0,
            boot_count: 0,
            last_known_unix: 0,
            last_content_crc: 0,
            fast_refreshes: 0,
            net_failures: 0,
            ap_cached: false,
            ap_channel: 0,
            ap_bssid: [0; 6],
            last_success_unix: 0,
        };
        assert_eq!(s.backoff_multiplier(), 1);
        for _ in 0..3 {
            s.record_failure();
        }
        assert_eq!(s.backoff_multiplier(), 2);
        for _ in 0..3 {
            s.record_failure();
        }
        assert_eq!(s.backoff_multiplier(), 12);
        s.record_success(1, 2);
        assert_eq!(s.backoff_multiplier(), 1);
    }

    #[test]
    fn licznik_porazek_nie_przekreca_sie() {
        let mut s = RtcState::load();
        for _ in 0..300 {
            s.record_failure();
        }
        assert_eq!(s.net_failures, u8::MAX);
    }
}
