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
use dashboard::Rotation;
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

/// Maksymalna długość wartości tekstowej. Adresy iCal Google mają ~120 znaków;
/// zapas jest na wypadek innych źródeł.
const MAX_VALUE: usize = 512;

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
        let mut buf = [0u8; MAX_VALUE];
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

    pub fn set_wifi(&mut self, ssid: &str, password: &str) -> Result<()> {
        self.nvs.set_str(KEY_SSID, ssid).context("zapis SSID")?;
        self.nvs
            .set_str(KEY_PASSWORD, password)
            .context("zapis hasła")?;
        Ok(())
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

    pub fn set_rotation(&mut self, rotation: Rotation) -> Result<()> {
        self.nvs
            .set_str(KEY_ROTATION, rotation.as_str())
            .context("zapis obrotu ekranu")
    }
}
