//! Decyzja o aktualizacji firmware'u: czy pobierać, skąd i pod jakim warunkiem.
//!
//! Sam transfer — HTTPS, zapis do slotu, SHA-256 liczone w locie — siedzi
//! w `firmware/src/net/ota.rs`, bo wymaga ESP-IDF. Tutaj jest wszystko, co da się
//! rozstrzygnąć bez sprzętu, i dzięki temu jest przetestowane.
//!
//! # Licznik prób musi przeżyć restart, i to jest ta pułapka
//!
//! Gdyby manifest obiecywał wersję, której wgrany obraz nie raportuje, urządzenie
//! kręciłoby OTA w kółko: pobierz 3 MB, zainstaluj, zrestartuj, zobacz że wersja
//! wciąż się nie zgadza, pobierz znowu — aż do rozładowania ogniwa. Stąd
//! [`Attempts`] i [`MAX_ATTEMPTS`].
//!
//! Pierwsza wersja tego licznika leżała w pamięci RTC i **nie działała**, choć
//! wyglądała poprawnie. Bootloader ESP-IDF przeładowuje segmenty RTC z obrazu przy
//! każdym resecie, który **nie** jest wybudzeniem z deep sleepu:
//!
//! ```c
//! // components/bootloader_support/src/esp_image_format.c, should_load()
//! bool load_rtc_memory = esp_rom_get_reset_reason(0) != RESET_REASON_CORE_DEEP_SLEEP;
//! ```
//!
//! `esp_restart()` po wgraniu obrazu jest resetem programowym, więc `.rtc.data`
//! wracało do wartości początkowych — z magicznym słowem zero. Nowy obraz widział
//! zimny start i zerował licznik dokładnie w tym jednym scenariuszu, przed którym
//! licznik miał chronić. Dlatego stan prób trzymamy w NVS, która przeżywa i reset,
//! i przeflashowanie z przeglądarki (partycja `nvs` leży za obiema partycjami
//! aplikacji).
//!
//! Pętla i tak nie chodziła co cykl, tylko co drugi, i to też warto wiedzieć:
//! `esp_ota_begin` odmawia z `ESP_ERR_OTA_ROLLBACK_INVALID_STATE`, dopóki działający
//! obraz jest w stanie `PENDING_VERIFY` (`esp_ota_ops.c`). Świeżo wgrany obraz
//! potwierdza się dopiero na końcu udanego cyklu, więc pierwsze podejście po
//! restarcie odbijało się od tej blokady. To spowalniało rozładowywanie ogniwa,
//! ale go nie zatrzymywało.

use serde::Deserialize;

/// Ile razy wolno próbować tej samej wersji, zanim uznamy ją za niewgrywalną.
pub const MAX_ATTEMPTS: u8 = 3;

/// Manifest to kilkaset bajtów; wszystko powyżej znaczy, że pobieramy nie to co trzeba.
pub const MAX_MANIFEST_BYTES: usize = 4096;

/// Widełki rozsądnego rozmiaru obrazu aplikacji. Dolna granica odsiewa strony błędu
/// podane z kodem 200, górna — obraz, który i tak nie zmieści się w slocie.
pub const MIN_IMAGE_BYTES: usize = 256 * 1024;
pub const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// Opis dostępnej aktualizacji.
///
/// Publikowany obok webflashera, więc CI generuje go tym samym krokiem co obraz.
/// `sha256` jest wymagane — bez sumy kontrolnej odmawiamy aktualizacji.
#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub version: String,
    /// Adres obrazu. Może być względny — wtedy rozwiązujemy go względem katalogu,
    /// w którym leży manifest. Dzięki temu ten sam artefakt działa i z GitHub Pages,
    /// i z serwera w sieci lokalnej, bez przebudowy.
    pub url: String,
    /// SHA-256 obrazu aplikacji, zapis szesnastkowy.
    pub sha256: String,
    /// Rozmiar w bajtach. Opcjonalny, ale jeśli jest — sprawdzamy, i wtedy kasujemy
    /// tylko tyle slotu, ile trzeba.
    #[serde(default)]
    pub size: Option<usize>,
}

/// Ile razy próbowaliśmy już wgrać którą wersję. Trzyma to NVS — patrz nagłówek modułu.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attempts {
    /// Wersja, której dotyczy licznik. Pusty łańcuch = żadnej.
    pub version: String,
    pub count: u8,
}

impl Attempts {
    /// Odnotowuje kolejne podejście do wskazanej wersji.
    pub fn record(&mut self, version: &str) {
        if self.version == version {
            self.count = self.count.saturating_add(1);
        } else {
            self.version = version.to_string();
            self.count = 1;
        }
    }

    fn exhausted(&self, version: &str, max: u8) -> bool {
        self.version == version && self.count >= max
    }
}

/// Co zrobić z manifestem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Manifest podaje tę samą wersję, która działa.
    UpToDate,
    /// Nie aktualizujemy, i to jest powód.
    Refuse(String),
    /// Pobierz i wgraj.
    Download(Plan),
}

/// Wszystko, czego potrzebuje warstwa transportu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Adres obrazu, już rozwiązany względem adresu manifestu.
    pub image_url: String,
    pub sha256: [u8; 32],
    /// Zadeklarowany rozmiar, jeśli manifest go podał.
    pub size: Option<usize>,
    pub version: String,
}

/// Rozstrzyga, czy i co pobrać.
///
/// Nie dotyka ani sieci, ani flasha — dostaje gotowy manifest i stan licznika prób,
/// oddaje decyzję. Wołający odnotowuje próbę **przed** pobraniem, nie po: między
/// startem pobierania a końcem jest wiele ścieżek błędu i każda z nich musi
/// zostawić ślad.
pub fn decide(
    manifest: &Manifest,
    manifest_url: &str,
    running_version: &str,
    attempts: &Attempts,
) -> Decision {
    if manifest.version == running_version {
        return Decision::UpToDate;
    }

    if attempts.exhausted(&manifest.version, MAX_ATTEMPTS) {
        return Decision::Refuse(format!(
            "wersja {} próbowana już {MAX_ATTEMPTS} razy bez skutku",
            manifest.version
        ));
    }

    let sha256 = match parse_hex32(&manifest.sha256) {
        Ok(h) => h,
        Err(e) => {
            return Decision::Refuse(format!("zła suma SHA-256 w manifeście: {e}"));
        }
    };

    if let Some(size) = manifest.size {
        if !(MIN_IMAGE_BYTES..=MAX_IMAGE_BYTES).contains(&size) {
            return Decision::Refuse(format!(
                "manifest podaje rozmiar {size} B — poza dopuszczalnym zakresem"
            ));
        }
    }

    Decision::Download(Plan {
        image_url: resolve_url(manifest_url, &manifest.url),
        sha256,
        size: manifest.size,
        version: manifest.version.clone(),
    })
}

/// Rozwiązuje adres obrazu względem adresu manifestu.
///
/// Adres bezwzględny zostaje bez zmian. Względny doklejamy do katalogu manifestu —
/// czyli tego, co zostaje po obcięciu wszystkiego za ostatnim ukośnikiem. To celowo
/// nie jest pełna implementacja RFC 3986: manifest jest naszym plikiem i leży obok
/// obrazu, a pełny parser URL-i to kod, którego nie ma tu czego pilnować.
pub fn resolve_url(manifest_url: &str, target: &str) -> String {
    if target.starts_with("http://") || target.starts_with("https://") {
        return target.to_string();
    }
    // Ucinamy zapytanie i fragment — w ścieżce bazowej nie mają czego szukać.
    let base = manifest_url
        .split(['?', '#'])
        .next()
        .unwrap_or(manifest_url);
    match base.rfind('/') {
        Some(i) => format!("{}{}", &base[..=i], target.trim_start_matches('/')),
        None => target.to_string(),
    }
}

/// Parsuje 64 znaki szesnastkowe na 32 bajty.
pub fn parse_hex32(s: &str) -> Result<[u8; 32], String> {
    let s = s.trim();
    if s.len() != 64 {
        return Err(format!(
            "oczekiwano 64 znaków szesnastkowych, jest {}",
            s.len()
        ));
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .map_err(|_| format!("niepoprawny bajt na pozycji {i}"))?;
    }
    Ok(out)
}

/// Zapis szesnastkowy — do komunikatów o niezgodnej sumie.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Parsuje manifest z surowej odpowiedzi HTTP.
pub fn parse_manifest(body: &[u8]) -> Result<Manifest, String> {
    serde_json::from_slice(body)
        .map_err(|e| format!("manifest OTA nie jest poprawnym JSON-em: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str) -> Manifest {
        Manifest {
            version: version.to_string(),
            url: "firmware-ota.bin".to_string(),
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
            size: Some(3_000_000),
        }
    }

    const URL: &str = "https://example.test/fw/ota.json";

    #[test]
    fn hex_w_obie_strony() {
        let s = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(hex(&parse_hex32(s).unwrap()), s);
    }

    #[test]
    fn zla_dlugosc_sumy_jest_bledem() {
        assert!(parse_hex32("abc").is_err());
        assert!(parse_hex32(&"z".repeat(64)).is_err());
    }

    #[test]
    fn wzgledny_adres_rozwiazuje_sie_wzgledem_manifestu() {
        assert_eq!(
            resolve_url("https://example.test/fw/ota.json", "firmware-ota.bin"),
            "https://example.test/fw/firmware-ota.bin"
        );
        assert_eq!(
            resolve_url("https://example.test/ota.json?v=2", "firmware-ota.bin"),
            "https://example.test/firmware-ota.bin"
        );
        // Bezwzględny zostaje nietknięty.
        assert_eq!(
            resolve_url("https://example.test/ota.json", "https://cdn.test/a.bin"),
            "https://cdn.test/a.bin"
        );
    }

    #[test]
    fn ta_sama_wersja_to_koniec_tematu() {
        let d = decide(&manifest("0.1.0"), URL, "0.1.0", &Attempts::default());
        assert_eq!(d, Decision::UpToDate);
    }

    #[test]
    fn nowa_wersja_daje_plan_z_rozwiazanym_adresem() {
        match decide(&manifest("0.2.0"), URL, "0.1.0", &Attempts::default()) {
            Decision::Download(plan) => {
                assert_eq!(plan.image_url, "https://example.test/fw/firmware-ota.bin");
                assert_eq!(plan.version, "0.2.0");
                assert_eq!(plan.size, Some(3_000_000));
            }
            other => panic!("spodziewałem się pobrania, jest {other:?}"),
        }
    }

    #[test]
    fn po_wyczerpaniu_prob_odpuszczamy() {
        let attempts = Attempts {
            version: "0.2.0".to_string(),
            count: MAX_ATTEMPTS,
        };
        assert!(matches!(
            decide(&manifest("0.2.0"), URL, "0.1.0", &attempts),
            Decision::Refuse(_)
        ));
        // ...ale licznik dotyczy JEDNEJ wersji. Nowa wersja w manifeście to nowa szansa,
        // inaczej jedna zepsuta publikacja zamrażałaby aktualizacje na zawsze.
        assert!(matches!(
            decide(&manifest("0.3.0"), URL, "0.1.0", &attempts),
            Decision::Download(_)
        ));
    }

    #[test]
    fn licznik_prob_liczy_od_nowa_dla_nowej_wersji() {
        let mut a = Attempts::default();
        a.record("0.2.0");
        a.record("0.2.0");
        assert_eq!(a.count, 2);
        a.record("0.3.0");
        assert_eq!(a.count, 1);
        assert_eq!(a.version, "0.3.0");
    }

    #[test]
    fn licznik_prob_nie_przekreca_sie() {
        let mut a = Attempts::default();
        for _ in 0..300 {
            a.record("0.2.0");
        }
        assert_eq!(a.count, u8::MAX);
        assert!(a.exhausted("0.2.0", MAX_ATTEMPTS));
    }

    #[test]
    fn zla_suma_kontrolna_blokuje_pobranie() {
        let mut m = manifest("0.2.0");
        m.sha256 = "nie-jest-sumą".to_string();
        assert!(matches!(
            decide(&m, URL, "0.1.0", &Attempts::default()),
            Decision::Refuse(_)
        ));
    }

    #[test]
    fn rozmiar_poza_widelkami_blokuje_pobranie() {
        for size in [1_000usize, MAX_IMAGE_BYTES + 1] {
            let mut m = manifest("0.2.0");
            m.size = Some(size);
            assert!(
                matches!(
                    decide(&m, URL, "0.1.0", &Attempts::default()),
                    Decision::Refuse(_)
                ),
                "rozmiar {size} powinien być odrzucony"
            );
        }
    }

    #[test]
    fn brak_rozmiaru_jest_dopuszczalny() {
        let mut m = manifest("0.2.0");
        m.size = None;
        assert!(matches!(
            decide(&m, URL, "0.1.0", &Attempts::default()),
            Decision::Download(_)
        ));
    }

    #[test]
    fn manifest_parsuje_sie_z_json_a() {
        let body = br#"{"version":"0.2.0","url":"firmware-ota.bin",
            "sha256":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "size":3044400}"#;
        let m = parse_manifest(body).unwrap();
        assert_eq!(m.version, "0.2.0");
        assert_eq!(m.size, Some(3_044_400));
    }

    #[test]
    fn manifest_bez_sumy_kontrolnej_nie_parsuje_sie() {
        // `sha256` nie ma `#[serde(default)]` i to jest celowe: bez sumy kontrolnej
        // nie mamy czym sprawdzić, czy dostaliśmy obraz, o który prosiliśmy.
        let body = br#"{"version":"0.2.0","url":"firmware-ota.bin"}"#;
        assert!(parse_manifest(body).is_err());
    }

    #[test]
    fn strona_bledu_podana_jako_manifest_nie_parsuje_sie() {
        assert!(parse_manifest(b"<!doctype html><title>404</title>").is_err());
    }
}
