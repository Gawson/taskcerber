//! Aktualizacja firmware'u przez HTTPS.
//!
//! # Dlaczego to działa bez ruszania tablicy partycji
//!
//! `firmware/partitions.csv` ma od początku dwa sloty aplikacji po 4 MiB
//! (`ota_0` @ 0x010000, `ota_1` @ 0x410000) i `otadata` @ 0x009000. Aplikacja zajmuje
//! ~71% slotu, więc mieści się z zapasem.
//!
//! Właściwość uboczna tamtego układu okazuje się tutaj siatką bezpieczeństwa:
//! `otadata` leży w luce, którą `espflash --merge` wypełnia bajtami `0xFF` i którą
//! webflasher **zapisuje**. Każde przeflashowanie z przeglądarki kasuje więc
//! `otadata`, a bootloader wraca do `ota_0`. Jeśli OTA wgra coś zepsutego, wyjściem
//! awaryjnym jest kabel i strona flashera.
//!
//! # Cztery rzeczy, które stoją między tym kodem a cegłą
//!
//! 1. **Rollback bootloadera.** Przy `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y`
//!    świeżo wgrany obraz startuje w stanie `PENDING_VERIFY`. Jeśli nie zawoła
//!    `esp_ota_mark_app_valid_cancel_rollback()` przed kolejnym resetem, bootloader
//!    wraca do poprzedniego slotu. Wołamy to dopiero na końcu udanego cyklu — czyli
//!    po tym, jak obraz udowodnił, że potrafi dojść do deep sleepu.
//! 2. **Suma kontrolna z manifestu.** Liczymy SHA-256 w locie z pobieranego strumienia
//!    i porównujemy **przed** przestawieniem slotu startowego. `esp_ota_end()` sprawdza
//!    dodatkowo sumę wbudowaną w sam obraz, ale ta pilnuje wyłącznie spójności — nie
//!    tego, czy dostaliśmy obraz, o który prosiliśmy.
//! 3. **Licznik prób.** Gdyby manifest obiecywał wersję, której wgrany obraz nie
//!    raportuje, urządzenie kręciłoby OTA w kółko aż do rozładowania ogniwa.
//!    Po [`MAX_ATTEMPTS`] próbach tej samej wersji odpuszczamy aż do zmiany manifestu.
//! 4. **Próg zasilania.** Pobranie ~3 MB przez HTTPS trzyma radio na antenie o rząd
//!    wielkości dłużej niż zwykły cykl. Bramkuje to `Policy::should_update`.
//!
//! # Zatrzask błędu strumienia
//!
//! [`http::ResponseReader`] zapamiętuje pierwszy błąd odczytu i **nie wolno go
//! pominąć**. Urwane pobieranie wygląda dla pętli `read()` dokładnie jak koniec pliku,
//! a `esp_ota_end()` przyjęłoby taki obcięty obraz, gdyby tylko trafił w granicę
//! sektora. Sprawdzamy [`http::ResponseReader::error`] przed czymkolwiek innym.

use std::io::Read;

use anyhow::{bail, Context, Result};
use esp_idf_svc::ota::EspOta;
use esp_idf_svc::sys;
use log::{info, warn};
use serde::Deserialize;

use crate::net::http;
use crate::power::rtc_state::RtcState;

/// Ile razy wolno próbować tej samej wersji, zanim uznamy ją za niewgrywalną.
pub const MAX_ATTEMPTS: u8 = 3;

/// Manifest to kilkaset bajtów; wszystko powyżej znaczy, że pobieramy nie to co trzeba.
const MAX_MANIFEST_BYTES: usize = 4096;

/// Widełki rozsądnego rozmiaru obrazu aplikacji. Dolna granica odsiewa strony błędu
/// podane z kodem 200, górna — obraz, który i tak nie zmieści się w slocie.
const MIN_IMAGE_BYTES: usize = 256 * 1024;
const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;

/// Kawałek strumienia zapisywany jednym `esp_ota_write`.
const CHUNK: usize = 4096;

/// Opis dostępnej aktualizacji.
///
/// Publikowany obok webflashera, więc CI generuje go tym samym krokiem co obraz.
/// `sha256` jest wymagane — bez sumy kontrolnej odmawiamy aktualizacji.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub version: String,
    /// Adres obrazu. Może być względny — wtedy rozwiązujemy go względem katalogu,
    /// w którym leży manifest. Dzięki temu ten sam artefakt działa i z GitHub Pages,
    /// i z serwera w sieci lokalnej, bez przebudowy.
    pub url: String,
    /// SHA-256 obrazu aplikacji, zapis szesnastkowy.
    pub sha256: String,
    /// Rozmiar w bajtach. Opcjonalny, ale jeśli jest — sprawdzamy.
    #[serde(default)]
    pub size: Option<usize>,
}

#[derive(Debug)]
pub enum Outcome {
    /// Manifest podaje tę samą wersję, która działa.
    UpToDate,
    /// Jest nowsza, ale nie teraz — powód w polu.
    Skipped(&'static str),
    /// Wgrane i zaktywowane. Wołający ma zrestartować urządzenie.
    Installed { version: String },
}

/// Sprawdza manifest i — jeśli trzeba i wolno — wgrywa nowy obraz do wolnego slotu.
///
/// **Nie restartuje.** Zwraca [`Outcome::Installed`], a decyzję o restarcie zostawia
/// wołającemu, bo ten wie, czy radio jest już wyłączone i czy panel nie jest w trakcie
/// odświeżania. Reset przy podniesionych szynach TPS65185 potrafi uszkodzić panel.
pub fn check_and_apply(
    manifest_url: &str,
    running_version: &str,
    state: &mut RtcState,
) -> Result<Outcome> {
    let manifest = fetch_manifest(manifest_url).context("nie mogę pobrać manifestu OTA")?;

    if manifest.version == running_version {
        info!("OTA: wersja {running_version} jest aktualna");
        state.clear_ota_attempts();
        return Ok(Outcome::UpToDate);
    }

    if !state.ota_allowed(&manifest.version) {
        warn!(
            "OTA: {} próbowane już {MAX_ATTEMPTS} razy bez skutku — odpuszczam",
            manifest.version
        );
        return Ok(Outcome::Skipped("wyczerpany limit prób tej wersji"));
    }

    let expected = parse_hex32(&manifest.sha256)
        .with_context(|| format!("zła suma SHA-256 w manifeście: {}", manifest.sha256))?;

    if let Some(size) = manifest.size {
        if !(MIN_IMAGE_BYTES..=MAX_IMAGE_BYTES).contains(&size) {
            bail!("manifest podaje rozmiar {size} B — poza dopuszczalnym zakresem");
        }
    }

    let image_url = resolve_url(manifest_url, &manifest.url);
    info!(
        "OTA: {running_version} -> {}, pobieram {image_url}",
        manifest.version
    );
    state.record_ota_attempt(&manifest.version);
    // Licznik prób musi przeżyć nieudaną próbę, a stąd do końca funkcji jest wiele
    // ścieżek błędu — zapisujemy od razu.
    state.store();

    let written = download_into_slot(&image_url, expected)?;
    info!("OTA: wgrane {written} B, wersja {}", manifest.version);

    Ok(Outcome::Installed {
        version: manifest.version,
    })
}

/// Zaznacza działający obraz jako sprawny i odwołuje rollback.
///
/// Wołać **na końcu udanego cyklu**, nie na starcie. Cała wartość rollbacku polega
/// na tym, że obraz musi coś udowodnić, zanim uzna się go za dobry; wołanie tego
/// od razu po starcie zamienia zabezpieczenie w dekorację.
pub fn mark_running_valid() {
    match EspOta::new().and_then(|mut ota| ota.mark_running_slot_valid()) {
        Ok(()) => info!("OTA: bieżący slot potwierdzony jako sprawny"),
        // Przy `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=n` albo gdy slot nie jest
        // w stanie PENDING_VERIFY, to jest brak operacji — nie błąd.
        Err(e) => info!("OTA: potwierdzenie slotu nie było potrzebne ({e})"),
    }
}

fn fetch_manifest(url: &str) -> Result<Manifest> {
    let mut reader = http::get(url)?;
    let mut body = Vec::new();
    reader
        .by_ref()
        .take(MAX_MANIFEST_BYTES as u64)
        .read_to_end(&mut body)
        .context("nie mogę wczytać manifestu")?;

    if let Some(e) = reader.error() {
        bail!("pobieranie manifestu przerwane: {e}");
    }
    if body.len() >= MAX_MANIFEST_BYTES {
        bail!("manifest większy niż {MAX_MANIFEST_BYTES} B — to nie jest manifest");
    }

    serde_json::from_slice(&body).context("manifest OTA nie jest poprawnym JSON-em")
}

/// Pobiera obraz prosto do wolnego slotu, licząc po drodze SHA-256.
///
/// Obraz nie mieści się w RAM-ie (3 MB przy 8 MB PSRAM zajętej w większości przez
/// bufory panelu i mbedTLS), więc idzie strumieniem do flasha. Konsekwencja: sumę
/// kontrolną znamy dopiero na końcu, kiedy dane są już zapisane. Dlatego slot
/// startowy przestawiamy **po** weryfikacji — do tego momentu wgrany obraz jest
/// martwy i nieszkodliwy.
fn download_into_slot(url: &str, expected_sha: [u8; 32]) -> Result<usize> {
    let mut reader = http::get(url).context("nie mogę pobrać obrazu")?;

    let mut ota = EspOta::new().context("nie mogę otworzyć OTA")?;
    let mut update = ota
        .initiate_update()
        .context("nie mogę rozpocząć zapisu do slotu OTA")?;

    let mut hasher = Sha256::new()?;
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0usize;

    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                let _ = update.abort();
                bail!("odczyt obrazu przerwany po {total} B: {e}");
            }
        };

        total += n;
        if total > MAX_IMAGE_BYTES {
            let _ = update.abort();
            bail!("obraz przekroczył {MAX_IMAGE_BYTES} B");
        }

        hasher.update(&buf[..n]);
        if let Err(e) = update.write(&buf[..n]) {
            let _ = update.abort();
            bail!("zapis do slotu OTA nie powiódł się po {total} B: {e}");
        }
    }

    // Zatrzask błędu sprawdzamy PRZED wszystkim innym: urwane pobieranie wygląda dla
    // pętli wyżej dokładnie jak czysty koniec strumienia.
    if let Some(e) = reader.error() {
        let _ = update.abort();
        bail!("pobieranie obrazu przerwane po {total} B: {e}");
    }
    if let Some(false) = reader.length_matches() {
        let _ = update.abort();
        bail!("pobrano {total} B, a serwer zapowiadał inną długość");
    }
    if total < MIN_IMAGE_BYTES {
        let _ = update.abort();
        bail!("pobrano tylko {total} B — to nie jest obraz aplikacji");
    }

    let actual = hasher.finish();
    if actual != expected_sha {
        let _ = update.abort();
        bail!(
            "SHA-256 się nie zgadza: obraz {}, manifest {}",
            hex(&actual),
            hex(&expected_sha)
        );
    }

    update
        .complete()
        .context("nie mogę zamknąć i aktywować slotu OTA")?;

    Ok(total)
}

// ---------------------------------------------------------------------------
// SHA-256 z mbedTLS, które i tak jest w obrazie na potrzeby TLS-a
// ---------------------------------------------------------------------------

struct Sha256(sys::mbedtls_sha256_context);

impl Sha256 {
    fn new() -> Result<Self> {
        // SAFETY: kontekst jest inicjalizowany przed pierwszym użyciem i zwalniany
        // w `Drop`; nie jest przenoszony między wątkami.
        let mut ctx = unsafe { std::mem::zeroed::<sys::mbedtls_sha256_context>() };
        unsafe {
            sys::mbedtls_sha256_init(&mut ctx);
            if sys::mbedtls_sha256_starts(&mut ctx, 0) != 0 {
                sys::mbedtls_sha256_free(&mut ctx);
                bail!("nie mogę zainicjalizować SHA-256");
            }
        }
        Ok(Self(ctx))
    }

    fn update(&mut self, data: &[u8]) {
        // SAFETY: kontekst zainicjalizowany, bufor ważny na czas wywołania.
        unsafe {
            sys::mbedtls_sha256_update(&mut self.0, data.as_ptr(), data.len());
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let mut out = [0u8; 32];
        // SAFETY: jw.; `out` ma wymagane 32 bajty.
        unsafe {
            sys::mbedtls_sha256_finish(&mut self.0, out.as_mut_ptr());
        }
        out
    }
}

impl Drop for Sha256 {
    fn drop(&mut self) {
        // SAFETY: kontekst zainicjalizowany w `new`.
        unsafe { sys::mbedtls_sha256_free(&mut self.0) };
    }
}

/// Rozwiązuje adres obrazu względem adresu manifestu.
///
/// Adres bezwzględny zostaje bez zmian. Względny doklejamy do katalogu manifestu —
/// czyli tego, co zostaje po obcięciu wszystkiego za ostatnim ukośnikiem. To celowo
/// nie jest pełna implementacja RFC 3986: manifest jest naszym plikiem i leży obok
/// obrazu, a pełny parser URL-i to kod, którego nie ma tu czego pilnować.
fn resolve_url(manifest_url: &str, target: &str) -> String {
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

fn parse_hex32(s: &str) -> Result<[u8; 32]> {
    let s = s.trim();
    if s.len() != 64 {
        bail!("oczekiwano 64 znaków szesnastkowych, jest {}", s.len());
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
            .with_context(|| format!("niepoprawny bajt na pozycji {i}"))?;
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_w_obie_strony() {
        let s = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(hex(&parse_hex32(s).unwrap()), s);
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
    fn zla_dlugosc_sumy_jest_bledem() {
        assert!(parse_hex32("abc").is_err());
        assert!(parse_hex32(&"z".repeat(64)).is_err());
    }
}
