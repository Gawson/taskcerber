//! Aktualizacja firmware'u przez HTTPS — transport.
//!
//! Decyzja „czy w ogóle, którą wersję i skąd" siedzi w [`devlogic::ota`] i jest
//! przetestowana na hoście. Tutaj zostaje to, czego bez ESP-IDF nie ma: pobranie
//! strumienia, liczenie SHA-256 sprzętowym mbedTLS-em i zapis do wolnego slotu.
//!
//! # Dlaczego to działa bez ruszania tablicy partycji
//!
//! `firmware/partitions.csv` ma od początku dwa sloty aplikacji po 4 MiB
//! (`ota_0` @ 0x010000, `ota_1` @ 0x410000) i `otadata` @ 0x009000. Aplikacja zajmuje
//! ~73% slotu, więc mieści się z zapasem.
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
//! 3. **Licznik prób w NVS.** Manifest obiecujący wersję, której wgrany obraz nie
//!    raportuje, zapętliłby pobieranie 3 MB aż do rozładowania ogniwa. Dlaczego
//!    akurat NVS, a nie pamięć RTC, w której ten licznik był najpierw — nagłówek
//!    [`devlogic::ota`].
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
use devlogic::ota::{self, Decision, Plan};
use esp_idf_svc::ota::EspOta;
use esp_idf_svc::sys;
use log::{info, warn};

use crate::net::http;
use crate::store::Store;

/// Kawałek strumienia zapisywany jednym `esp_ota_write`.
const CHUNK: usize = 4096;

#[derive(Debug)]
pub enum Outcome {
    /// Manifest podaje tę samą wersję, która działa.
    UpToDate,
    /// Jest nowsza, ale nie teraz — powód w polu.
    Skipped(String),
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
    store: &mut Store,
) -> Result<Outcome> {
    let manifest = fetch_manifest(manifest_url).context("nie mogę pobrać manifestu OTA")?;
    let mut attempts = store.ota_attempts();

    let plan = match ota::decide(&manifest, manifest_url, running_version, &attempts) {
        Decision::UpToDate => {
            info!("OTA: wersja {running_version} jest aktualna");
            // Porażka kasowania licznika nie jest powodem, żeby przerwać cykl —
            // w najgorszym razie zostanie nieaktualny wpis w NVS.
            if let Err(e) = store.clear_ota_attempts() {
                warn!("OTA: nie mogę wyzerować licznika prób: {e:#}");
            }
            return Ok(Outcome::UpToDate);
        }
        Decision::Refuse(reason) => {
            warn!("OTA: odpuszczam — {reason}");
            return Ok(Outcome::Skipped(reason));
        }
        Decision::Download(plan) => plan,
    };

    info!(
        "OTA: {running_version} -> {}, pobieram {}",
        plan.version, plan.image_url
    );

    // Licznik prób zapisujemy PRZED pobraniem. Stąd do końca funkcji jest wiele
    // ścieżek błędu i każda z nich musi zostawić ślad — inaczej wersja, która
    // wywraca się w połowie pobierania, próbowałaby się w nieskończoność.
    attempts.record(&plan.version);
    store
        .set_ota_attempts(&attempts)
        .context("nie mogę zapisać licznika prób OTA")?;

    let written = download_into_slot(&plan)?;
    info!("OTA: wgrane {written} B, wersja {}", plan.version);

    Ok(Outcome::Installed {
        version: plan.version,
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

fn fetch_manifest(url: &str) -> Result<ota::Manifest> {
    let mut reader = http::get(url)?;
    let mut body = Vec::new();
    reader
        .by_ref()
        .take(ota::MAX_MANIFEST_BYTES as u64)
        .read_to_end(&mut body)
        .context("nie mogę wczytać manifestu")?;

    if let Some(e) = reader.error() {
        bail!("pobieranie manifestu przerwane: {e}");
    }
    if body.len() >= ota::MAX_MANIFEST_BYTES {
        bail!(
            "manifest większy niż {} B — to nie jest manifest",
            ota::MAX_MANIFEST_BYTES
        );
    }

    ota::parse_manifest(&body).map_err(|e| anyhow::anyhow!(e))
}

/// Pobiera obraz prosto do wolnego slotu, licząc po drodze SHA-256.
///
/// Obraz nie mieści się w RAM-ie (3 MB przy 8 MB PSRAM zajętej w większości przez
/// bufory panelu i mbedTLS), więc idzie strumieniem do flasha. Konsekwencja: sumę
/// kontrolną znamy dopiero na końcu, kiedy dane są już zapisane. Dlatego slot
/// startowy przestawiamy **po** weryfikacji — do tego momentu wgrany obraz jest
/// martwy i nieszkodliwy.
fn download_into_slot(plan: &Plan) -> Result<usize> {
    let mut reader = http::get(&plan.image_url).context("nie mogę pobrać obrazu")?;

    let mut esp_ota = EspOta::new().context("nie mogę otworzyć OTA")?;

    // Rozmiar z manifestu przekłada się wprost na to, ile flasha kasujemy.
    // `initiate_update` przekazuje `OTA_SIZE_UNKNOWN`, a wtedy `esp_ota_begin` kasuje
    // **cały** slot 4 MiB, zanim przyjdzie pierwszy bajt. Przy obrazie ~3 MB to
    // megabajt kasowania w zamian za nic. Znany rozmiar zaokrągla się w górę do
    // sektora i tyle właśnie znika.
    //
    // Cena jest taka, że rozmiar musi być PRAWDZIWY: zapis poza skasowany obszar
    // dałby uszkodzony obraz. Pilnuje tego twardy limit w pętli niżej i porównanie
    // sumy bajtów na końcu.
    let mut update = match plan.size {
        Some(size) => esp_ota.initiate_update_with_known_size(size),
        None => esp_ota.initiate_update(),
    }
    .context("nie mogę rozpocząć zapisu do slotu OTA")?;

    let limit = plan.size.unwrap_or(ota::MAX_IMAGE_BYTES);
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
        if total > limit {
            let _ = update.abort();
            bail!("obraz przekroczył {limit} B");
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
    match plan.size {
        // Manifest podał rozmiar, więc mamy z czym porównać — i musimy, bo tyle
        // sektorów skasowaliśmy.
        Some(size) if total != size => {
            let _ = update.abort();
            bail!("pobrano {total} B, a manifest obiecywał {size} B");
        }
        // Bez rozmiaru w manifeście zostaje tylko dolna granica rozsądku: strona
        // błędu podana z kodem 200 nie ma 256 kB.
        None if total < ota::MIN_IMAGE_BYTES => {
            let _ = update.abort();
            bail!("pobrano tylko {total} B — to nie jest obraz aplikacji");
        }
        _ => {}
    }

    let actual = hasher.finish();
    if actual != plan.sha256 {
        let _ = update.abort();
        bail!(
            "SHA-256 się nie zgadza: obraz {}, manifest {}",
            ota::hex(&actual),
            ota::hex(&plan.sha256)
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
