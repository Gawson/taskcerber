//! Konfiguracja trwała w NVS.
//!
//! Partycja `nvs` leży **za** obiema partycjami aplikacji (0x810000), i to jest
//! świadoma decyzja: `espflash --merge` zapisuje obraz jako
//! `[bootloader@0x0][luka 0xFF][tablica@0x8000][luka 0xFF][aplikacja@ota_0]`,
//! a te luki są faktycznie zapisywane. Wszystko przed końcem aplikacji ginie przy
//! każdym przeflashowaniu z przeglądarki — wszystko za nią przeżywa.
//!
//! Efekt: możesz zaktualizować firmware webflasherem i **nie stracić** ani danych
//! WiFi, ani adresu kalendarza.

use anyhow::{Context, Result};
use dashboard::snapshot::Snapshot;
use dashboard::Rotation;
use devlogic::boot::{BootStep, Crumb};
use devlogic::ota::Attempts;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

const NAMESPACE: &str = "t5cal";

const KEY_SSID: &str = "wifi_ssid";
const KEY_PASSWORD: &str = "wifi_pass";
const KEY_ICS_URL: &str = "ics_url";
const KEY_ICS_URL_2: &str = "ics_url2";
const KEY_TIMEZONE: &str = "tz";
const KEY_INTERVAL: &str = "interval_s";
const KEY_ROTATION: &str = "rotation";
const KEY_OTA_URL: &str = "ota_url";

// Licznik prób OTA. Leży w NVS, a nie w pamięci RTC, i to jest poprawka konkretnego
// błędu: bootloader przeładowuje segmenty RTC z obrazu przy każdym resecie, który
// nie jest wybudzeniem z deep sleepu — a po wgraniu nowego obrazu wołamy
// `esp_restart()`. Licznik zerował się więc dokładnie w tym scenariuszu, przed
// którym miał chronić. Pełne wyjaśnienie: nagłówek `devlogic::ota`.
// Okruszek startowy. W NVS z dokładnie tego samego powodu co licznik prób OTA
// powyżej: panika, watchdog i `esp_restart()` kasują `.rtc.data`, a to są właśnie
// te trzy przypadki, o których okruszek ma opowiedzieć. Jeden klucz, nie trzy —
// NVS liczy zapisy, a nie bajty. Pełne wyjaśnienie: nagłówek `devlogic::boot`.
const KEY_BOOT_CRUMB: &str = "boot_crumb";

// Migawka kalendarza. Zapisujemy ją TYLKO wtedy, gdy zmieniło się CRC treści —
// patrz `Store::save_snapshot`. Bez tego warunku byłby to zapis kilku kilobajtów
// do flasha co pół godziny, czyli kilkadziesiąt tysięcy cykli rocznie za nic.
const KEY_SNAPSHOT: &str = "cal_snap";

// Kiedy ostatnio udało się zsynchronizować zegar. Jeden zapis na dobę, więc
// zużycie flasha jest bez znaczenia.
const KEY_SNTP: &str = "sntp_unix";

// Czas ostatniego UDANEGO pobrania. W NVS, a nie tylko w RtcState, bo tamta ginie
// przy każdym resecie innym niż wybudzenie z deep sleepu — a wtedy ocena świeżości
// widzi „nigdy nie pobierano" i puszcza pobranie, choć dane mają minutę.
const KEY_FETCH: &str = "fetch_unix";

const KEY_OTA_TRY_VER: &str = "ota_try_ver";
const KEY_OTA_TRY_N: &str = "ota_try_n";

/// Maksymalna długość wartości tekstowej. Adresy iCal Google mają ~120 znaków;
/// zapas jest na wypadek innych źródeł.
pub const MAX_VALUE: usize = 512;

/// Górny limit rozmiaru migawki. Partycja `nvs` ma 128 KB na wszystko, a realny
/// kalendarz na czternaście dni mieści się w kilku — szesnaście to zapas, nie plan.
const MAX_SNAPSHOT: usize = 16 * 1024;

pub struct Store {
    nvs: EspNvs<NvsDefault>,
}

/// Komplet konfiguracji odczytanej przy starcie.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub ssid: Option<String>,
    pub password: Option<String>,
    /// Główny kanał kalendarza.
    pub ics_url: Option<String>,
    /// Opcjonalny drugi kanał — np. święta albo kalendarz współdzielony.
    pub ics_url_secondary: Option<String>,
    /// Nazwa strefy IANA; domyślnie `Europe/Warsaw`.
    pub timezone: Option<String>,
    /// Nadpisanie odstępu między pobraniami, w sekundach.
    pub interval_s: Option<u32>,
    /// Adres manifestu OTA. Brak = aktualizacje wyłączone, i to jest domyślne:
    /// urządzenie na baterii nie powinno samo sięgać po nowy firmware, dopóki ktoś
    /// świadomie nie wskaże, skąd.
    pub ota_url: Option<String>,
    /// Obrót ekranu. Przestawiany przyciskiem `S3`, więc musi przeżyć deep sleep
    /// **i** odcięcie zasilania — RTC memory nie wystarczy.
    pub rotation: Rotation,
}

impl Config {
    /// Czy urządzenie ma komplet danych, żeby w ogóle spróbować pobrać kalendarz.
    pub fn is_provisioned(&self) -> bool {
        self.ssid.as_ref().is_some_and(|s| !s.is_empty())
            && self.ics_url.as_ref().is_some_and(|s| !s.is_empty())
    }

    /// Strefa czasowa albo Europe/Warsaw, gdy nieustawiona lub nierozpoznana.
    pub fn tz(&self) -> chrono_tz::Tz {
        self.timezone
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(chrono_tz::Europe::Warsaw)
    }
}

impl Store {
    pub fn open(partition: EspDefaultNvsPartition) -> Result<Self> {
        let nvs = EspNvs::new(partition, NAMESPACE, true)
            .with_context(|| format!("nie mogę otworzyć przestrzeni NVS `{NAMESPACE}`"))?;
        Ok(Self { nvs })
    }

    fn get_string(&self, key: &str) -> Option<String> {
        // MAX_VALUE + 1, nie MAX_VALUE. `nvs_get_str` wymaga miejsca na kończące
        // zero i przy buforze co do bajta zwraca ESP_ERR_NVS_INVALID_LENGTH —
        // czyli wartość dokładnie maksymalnej długości dałaby się ZAPISAĆ,
        // a przy odczycie zniknęłaby po cichu jako `None`.
        let mut buf = [0u8; MAX_VALUE + 1];
        match self.nvs.get_str(key, &mut buf) {
            Ok(Some(s)) if !s.is_empty() => Some(s.to_string()),
            _ => None,
        }
    }

    pub fn load(&self) -> Config {
        Config {
            ssid: self.get_string(KEY_SSID),
            password: self.get_string(KEY_PASSWORD),
            ics_url: self.get_string(KEY_ICS_URL),
            ics_url_secondary: self.get_string(KEY_ICS_URL_2),
            timezone: self.get_string(KEY_TIMEZONE),
            interval_s: self.nvs.get_u32(KEY_INTERVAL).ok().flatten(),
            ota_url: self.get_string(KEY_OTA_URL),
            rotation: self
                .get_string(KEY_ROTATION)
                .map(|s| Rotation::parse(&s))
                .unwrap_or_default(),
        }
    }

    pub fn set_ssid(&mut self, ssid: &str) -> Result<()> {
        self.nvs.set_str(KEY_SSID, ssid).context("zapis SSID")
    }

    pub fn set_password(&mut self, password: &str) -> Result<()> {
        self.nvs
            .set_str(KEY_PASSWORD, password)
            .context("zapis hasła")
    }

    pub fn set_ics_url(&mut self, url: &str) -> Result<()> {
        self.nvs
            .set_str(KEY_ICS_URL, url)
            .context("zapis adresu kalendarza")
    }

    pub fn set_ics_url_secondary(&mut self, url: &str) -> Result<()> {
        self.nvs
            .set_str(KEY_ICS_URL_2, url)
            .context("zapis drugiego adresu kalendarza")
    }

    pub fn set_timezone(&mut self, tz: &str) -> Result<()> {
        self.nvs
            .set_str(KEY_TIMEZONE, tz)
            .context("zapis strefy czasowej")
    }

    pub fn set_interval(&mut self, seconds: u32) -> Result<()> {
        self.nvs
            .set_u32(KEY_INTERVAL, seconds)
            .context("zapis odstępu")
    }

    pub fn set_ota_url(&mut self, url: &str) -> Result<()> {
        self.nvs
            .set_str(KEY_OTA_URL, url)
            .context("zapis adresu manifestu OTA")
    }

    /// Ile razy próbowaliśmy już wgrać którą wersję.
    /// Okruszek zostawiony przez poprzedni cykl.
    ///
    /// Brak wpisu czytamy jako `Done`: świeżo przeflashowane urządzenie nie ma
    /// awarii do pokazania, a ekran diagnozy o niczym byłby gorszy niż jego brak.
    /// Odczytuje zapisaną migawkę kalendarza.
    ///
    /// Każde niepowodzenie — brak wpisu, inna wersja formatu, obcięty blob — daje
    /// `None`. Migawka jest optymalizacją, więc jej brak nie ma prawa niczego zepsuć.
    pub fn load_snapshot(&self) -> Option<Snapshot> {
        let mut buf = vec![0u8; MAX_SNAPSHOT];
        match self.nvs.get_blob(KEY_SNAPSHOT, &mut buf) {
            Ok(Some(bajty)) => {
                let n = bajty.len();
                match dashboard::snapshot::decode(bajty) {
                    Some(s) => {
                        log::info!("migawka: {} B, {} wydarzeń", n, s.events.len());
                        Some(s)
                    }
                    None => {
                        log::warn!("migawka nieczytelna ({n} B) — pomijam");
                        None
                    }
                }
            }
            Ok(None) => None,
            Err(e) => {
                log::warn!("nie mogę odczytać migawki: {e}");
                None
            }
        }
    }

    /// Zapisuje migawkę, ale tylko gdy treść naprawdę się zmieniła.
    ///
    /// `crc` to suma kontrolna pobranej treści; `poprzednie_crc` to ta sama wartość
    /// z ostatniego udanego cyklu. Gdy są równe, kalendarz się nie zmienił i zapis
    /// byłby czystym zużyciem flasha — kilka kilobajtów co pół godziny to
    /// kilkadziesiąt tysięcy cykli rocznie w zamian za nic.
    pub fn save_snapshot(&mut self, snap: &Snapshot, crc: u32, poprzednie_crc: u32) {
        if crc == poprzednie_crc && crc != 0 {
            log::debug!("migawka bez zmian (CRC {crc:08x}) — nie zapisuję");
            return;
        }
        let bajty = dashboard::snapshot::encode(snap);
        if bajty.len() > MAX_SNAPSHOT {
            log::warn!("migawka {} B przekracza limit — nie zapisuję", bajty.len());
            return;
        }
        match self.nvs.set_blob(KEY_SNAPSHOT, &bajty) {
            Ok(()) => log::info!("migawka zapisana: {} B", bajty.len()),
            // Bez propagacji: nieudany zapis podręcznej kopii nie ma prawa wywrócić
            // cyklu, w którym pobranie się udało.
            Err(e) => log::warn!("nie mogę zapisać migawki: {e}"),
        }
    }

    /// Czas ostatniej udanej synchronizacji SNTP (unix). 0 = nigdy.
    pub fn last_sntp_unix(&self) -> i64 {
        self.nvs
            .get_u64(KEY_SNTP)
            .ok()
            .flatten()
            .map(|v| v as i64)
            .unwrap_or(0)
    }

    pub fn set_last_sntp_unix(&mut self, unix: i64) {
        if unix <= 0 {
            return;
        }
        if let Err(e) = self.nvs.set_u64(KEY_SNTP, unix as u64) {
            log::warn!("nie mogę zapisać czasu synchronizacji: {e}");
        }
    }

    /// Czas ostatniego udanego pobrania (unix). 0 = nigdy.
    pub fn last_fetch_unix(&self) -> i64 {
        self.nvs
            .get_u64(KEY_FETCH)
            .ok()
            .flatten()
            .map(|v| v as i64)
            .unwrap_or(0)
    }

    pub fn set_last_fetch_unix(&mut self, unix: i64) {
        if unix <= 0 {
            return;
        }
        if let Err(e) = self.nvs.set_u64(KEY_FETCH, unix as u64) {
            log::warn!("nie mogę zapisać czasu pobrania: {e}");
        }
    }

    pub fn boot_crumb(&self) -> Crumb {
        match self.nvs.get_u64(KEY_BOOT_CRUMB).ok().flatten() {
            Some(v) => Crumb::unpack(v),
            None => Crumb::new(BootStep::Done, 0, 0),
        }
    }

    /// Odnotowuje wejście w etap. Woła się TUŻ PRZED nim, nie po — okruszek ma
    /// przeżyć to, co się w tym etapie stanie.
    pub fn mark_boot_step(&mut self, step: BootStep, ms: u32, dram_kb: u16) {
        let packed = Crumb::new(step, ms, dram_kb).pack();
        if let Err(e) = self.nvs.set_u64(KEY_BOOT_CRUMB, packed) {
            // Świadomie bez propagacji: diagnostyka nie ma prawa wywrócić cyklu,
            // który diagnozuje.
            log::warn!("nie mogę zapisać okruszka {step:?}: {e}");
        }
    }

    pub fn ota_attempts(&self) -> Attempts {
        Attempts {
            version: self.get_string(KEY_OTA_TRY_VER).unwrap_or_default(),
            count: self.nvs.get_u8(KEY_OTA_TRY_N).ok().flatten().unwrap_or(0),
        }
    }

    pub fn set_ota_attempts(&mut self, attempts: &Attempts) -> Result<()> {
        self.nvs
            .set_str(KEY_OTA_TRY_VER, &attempts.version)
            .context("zapis wersji próbowanej przez OTA")?;
        self.nvs
            .set_u8(KEY_OTA_TRY_N, attempts.count)
            .context("zapis licznika prób OTA")
    }

    /// Zeruje licznik prób — wołane, gdy działamy już na wersji z manifestu.
    ///
    /// Sprawdzenie „czy jest co kasować" nie jest mikrooptymalizacją: to jest
    /// ścieżka, którą urządzenie przechodzi przy **każdym udanym cyklu**, czyli
    /// kilkadziesiąt razy dziennie przez lata. Bezwarunkowy zapis byłby
    /// bezwarunkowym cyklem kasowania sektora we flashu.
    pub fn clear_ota_attempts(&mut self) -> Result<()> {
        if self
            .nvs
            .find_key(KEY_OTA_TRY_N)
            .context("sprawdzenie licznika prób OTA")?
            .is_none()
        {
            return Ok(());
        }
        self.nvs
            .remove(KEY_OTA_TRY_N)
            .context("kasowanie licznika prób OTA")?;
        self.nvs
            .remove(KEY_OTA_TRY_VER)
            .context("kasowanie wersji próbowanej przez OTA")?;
        Ok(())
    }

    pub fn set_rotation(&mut self, rotation: Rotation) -> Result<()> {
        self.nvs
            .set_str(KEY_ROTATION, rotation.as_str())
            .context("zapis obrotu ekranu")
    }
}
