//! Konsola konfiguracyjna po USB-Serial-JTAG.
//!
//! # Po co ona jest
//!
//! Bez niej **nic sieciowego w tym urządzeniu nie da się włączyć**. `Store` umie
//! zapisać SSID, hasło, adres kanału iCal i adres manifestu OTA, ale do tej pory
//! nikt tych setterów nie wołał — urządzenie po wgraniu firmware'u pokazywało ekran
//! konfiguracji i nie miało żadnej drogi, którą ta konfiguracja mogłaby do niego
//! trafić. Kalendarz, SNTP i OTA były za tą samą jedną blokadą.
//!
//! # Dlaczego akurat konsola szeregowa
//!
//! Płytka ma natywne USB (`303A:1001`), więc ten sam kabel, którym się flashuje,
//! jest gotowym kanałem dwukierunkowym — bez dodatkowego radia, bez SoftAP,
//! bez serwera HTTP w obrazie i bez ani jednego bajtu wystawionego na zewnątrz.
//! Wpisać da się to wprost z konsoli w esp-web-tools (ta sama strona, z której
//! wgrywasz firmware) albo z `espflash monitor`.
//!
//! SoftAP z formularzem jest wygodniejszy dla kogoś, kto nie ma pod ręką kabla,
//! ale to osobny serwer HTTP i osobna strona w obrazie. Ma sens dopiero wtedy,
//! gdy ścieżka sieciowa jest sprawdzona — a żeby ją sprawdzić, trzeba najpierw
//! móc podać SSID.
//!
//! # Kiedy się otwiera
//!
//! Wyłącznie przy **fizycznie podłączonym hoście USB**. To nie jest oszczędność na
//! wyrost: urządzenie nieskonfigurowane budzi się co pół godziny i rysuje ekran
//! konfiguracji, więc bezwarunkowe okno 90 s kosztowałoby ~2400 mAs na cykl —
//! wielokrotność całego budżetu udanego cyklu z siecią (~360 mAs).
//!
//! Pytamy o to [`usb_serial_jtag_is_connected`], a nie ładowarkę BQ25896, i to jest
//! celowe. Po pierwsze, BQ25896 wisi na magistrali I²C, która na tej płytce jeszcze
//! nie jest zweryfikowana — a konsola ma działać właśnie po to, żeby dało się
//! zweryfikować resztę. Po drugie, ładowarka odpowiada na pytanie „czy jest
//! zasilanie", a nas interesuje „czy po drugiej stronie jest komputer": powerbank
//! nie wysyła pakietów SOF i słusznie nie liczy się jako host.

use std::io::Write;
use std::time::{Duration, Instant};

use devlogic::redact;
use esp_idf_svc::sys;
use log::{info, warn};

use crate::store::{Config, Store, MAX_VALUE};

/// Ile czekamy na pierwsze polecenie, gdy urządzenie nie ma jeszcze konfiguracji.
///
/// Po wgraniu firmware'u z przeglądarki trzeba jeszcze kliknąć „Logs & Console",
/// więc okno musi objąć ten ruch z zapasem.
const WINDOW_FRESH: Duration = Duration::from_secs(90);

/// To samo, gdy urządzenie jest już skonfigurowane — wtedy okno służy tylko do
/// tego, żeby dało się coś poprawić, i nie ma powodu, by trzymało dłużej.
const WINDOW_CONFIGURED: Duration = Duration::from_secs(10);

/// Każdy przyjęty bajt przedłuża okno o tyle. Bez tego dłuższe wklejenie adresu
/// iCal kończy się urwaniem w połowie.
const EXTEND: Duration = Duration::from_secs(30);

/// Odstęp odpytywania sterownika. 100 ms jest niżej niż próg zauważalności przy
/// pisaniu, a wyżej niż koszt budzenia zadania.
const POLL_MS: u32 = 100;

/// Najkrótszy dopuszczalny odstęp odświeżania. Poniżej minuty urządzenie nie zdąży
/// zasnąć na tyle, żeby to miało sens, a `align_to_minute` i tak zaokrągli w górę.
const MIN_INTERVAL_S: u32 = 60;

/// Czy po drugiej stronie kabla jest host USB.
///
/// Monitor połączenia w ESP-IDF startuje z **optymistycznym `true`** i dopiero po
/// kilku tickach bez pakietu SOF przestawia się na `false`
/// (`esp_driver_usb_serial_jtag/src/usb_serial_jtag_connection_monitor.c`). Przy
/// `configTICK_RATE_HZ = 100` to jeden–dwa ticki, czyli ~20 ms od startu systemu.
/// Wołanie tej funkcji zaraz po `app_main` zwróciłoby więc „podłączony" niezależnie
/// od stanu faktycznego. Do momentu, w którym ją wołamy, magistrala I²C i NVS są
/// już podniesione — czyli minęło dużo więcej — ale dokładamy jeszcze jedną próbkę,
/// bo koszt jest żaden, a pomyłka kosztuje okno 90 s na baterii.
pub fn host_attached() -> bool {
    // SAFETY: prosty getter z ESP-IDF, bez argumentów i bez stanu po naszej stronie.
    let first = unsafe { sys::usb_serial_jtag_is_connected() };
    std::thread::sleep(Duration::from_millis(30));
    // SAFETY: jw.
    let second = unsafe { sys::usb_serial_jtag_is_connected() };
    first && second
}

/// Otwiera konsolę i obsługuje polecenia, dopóki nie minie okno albo użytkownik
/// nie wyjdzie sam.
///
/// Zwraca `true`, jeśli cokolwiek zostało zapisane — wołający ma wtedy przeładować
/// [`Config`], żeby zmiany zadziałały jeszcze w tym cyklu, a nie dopiero za pół
/// godziny.
pub fn run(store: &mut Store, config: &Config) -> bool {
    if let Err(e) = install_driver() {
        warn!("konsola: nie mogę zainstalować sterownika USB-Serial-JTAG ({e:#})");
        return false;
    }

    let window = if config.is_provisioned() {
        WINDOW_CONFIGURED
    } else {
        WINDOW_FRESH
    };

    banner(config, window);

    let mut deadline = Instant::now() + window;
    // Wiersz zbieramy w BAJTACH, nie w `String`. Hasło WiFi bywa polskie, a filtr
    // po `is_ascii_graphic` wyciąłby z niego „ą" bez słowa wyjaśnienia.
    let mut line: Vec<u8> = Vec::new();
    let mut skip_lf = false;
    let mut changed = false;

    while Instant::now() < deadline {
        let mut buf = [0u8; 64];
        let n = read_bytes(&mut buf);
        if n == 0 {
            continue;
        }
        // Okno wolno PRZEDŁUŻYĆ, nigdy skrócić. Bajt, który przyszedł w drugiej
        // sekundzie okna 90-sekundowego, nie może przyciąć go do trzydziestu.
        deadline = deadline.max(Instant::now() + EXTEND);

        for &byte in &buf[..n] {
            // Terminal wysyła CR, CRLF albo LF. Bez tego CRLF daje pusty wiersz
            // i drugi znak zachęty po każdym poleceniu.
            if std::mem::take(&mut skip_lf) && byte == b'\n' {
                continue;
            }
            match byte {
                b'\r' | b'\n' => {
                    skip_lf = byte == b'\r';
                    say("");
                    let raw = std::mem::take(&mut line);
                    let command = match String::from_utf8(raw) {
                        Ok(text) => text.trim().to_string(),
                        Err(_) => {
                            say("wiersz nie jest poprawnym UTF-8 — powtórz");
                            prompt();
                            continue;
                        }
                    };
                    if command.is_empty() {
                        prompt();
                        continue;
                    }
                    match execute(&command, store) {
                        Verdict::Continue { wrote } => {
                            changed |= wrote;
                            prompt();
                        }
                        Verdict::Leave => {
                            say("konsola zamknięta, jadę dalej");
                            return changed;
                        }
                    }
                }
                // Backspace i DEL — terminale wysyłają jedno albo drugie.
                0x08 | 0x7F => {
                    // Kasujemy cały ZNAK, nie bajt: „ą" to dwa bajty i usunięcie
                    // jednego zostawiłoby w wierszu połówkę sekwencji UTF-8,
                    // przez którą całe polecenie przestałoby się dekodować.
                    let mut removed = false;
                    while let Some(b) = line.pop() {
                        removed = true;
                        if b & 0xC0 != 0x80 {
                            break;
                        }
                    }
                    if removed {
                        // Cofnij, zamaluj spacją, cofnij jeszcze raz.
                        echo(b"\x08 \x08");
                    }
                }
                // Ctrl-C: porzuć wpisywany wiersz, nie wychodź.
                0x03 => {
                    line.clear();
                    say("");
                    prompt();
                }
                // Wszystko od spacji w górę jest treścią — łącznie z bajtami UTF-8
                // powyżej 0x7F. Poniżej 0x20 zostają same sterujące, w tym
                // sekwencje strzałek: to edytor jednego wiersza, nie emulator VT100.
                b if b >= 0x20 => {
                    if line.len() < MAX_VALUE {
                        line.push(b);
                        echo(&[b]);
                    }
                }
                _ => {}
            }
        }
    }

    say("");
    info!("konsola: okno minęło, jadę dalej");
    changed
}

enum Verdict {
    Continue { wrote: bool },
    Leave,
}

fn execute(command: &str, store: &mut Store) -> Verdict {
    let (verb, rest) = match command.split_once(char::is_whitespace) {
        Some((v, r)) => (v, r.trim()),
        None => (command, ""),
    };
    let verb = verb.to_ascii_lowercase();

    // Wartości tekstowe biorą CAŁĄ resztę wiersza, bez dzielenia na tokeny.
    // Hasła WiFi i nazwy sieci potrafią mieć spacje, a adres iCal ma ~120 znaków —
    // każde „mądrzejsze" parsowanie kończy się obcięciem czyjegoś hasła.
    let mut wrote = false;
    match verb.as_str() {
        "?" | "help" | "pomoc" => help(),
        "show" | "stan" => show(&store.load()),

        "ssid" => wrote = set_text(store, rest, "SSID", |s, v| s.set_ssid(v)),
        "pass" | "haslo" => wrote = set_text(store, rest, "hasło", |s, v| s.set_password(v)),
        "tz" | "strefa" => {
            if rest.parse::<chrono_tz::Tz>().is_err() {
                say(&format!(
                    "nieznana strefa `{rest}` — podaj nazwę IANA, np. Europe/Warsaw"
                ));
            } else {
                wrote = set_text(store, rest, "strefa", |s, v| s.set_timezone(v));
            }
        }

        "ics" | "ics2" | "ota" => {
            if !rest.starts_with("http://") && !rest.starts_with("https://") {
                say("adres musi zaczynać się od http:// albo https://");
            } else {
                if rest.starts_with("http://") {
                    say("UWAGA: http:// nie uwierzytelnia źródła. Do testów w LAN-ie w porządku, na stałe — nie.");
                }
                wrote = match verb.as_str() {
                    "ics" => set_text(store, rest, "kalendarz", |s, v| s.set_ics_url(v)),
                    "ics2" => set_text(store, rest, "kalendarz 2", |s, v| {
                        s.set_ics_url_secondary(v)
                    }),
                    _ => set_text(store, rest, "manifest OTA", |s, v| s.set_ota_url(v)),
                };
            }
        }

        "interval" | "odstep" => match rest.parse::<u32>() {
            Ok(s) if s >= MIN_INTERVAL_S => match store.set_interval(s) {
                Ok(()) => {
                    say(&format!("odstęp: {s} s"));
                    wrote = true;
                }
                Err(e) => say(&format!("nie zapisałem: {e:#}")),
            },
            Ok(_) => say(&format!("odstęp musi być co najmniej {MIN_INTERVAL_S} s")),
            Err(_) => say("odstęp podaje się w sekundach, np. `interval 1800`"),
        },

        "clear" | "kasuj" => match store.clear(rest) {
            Ok(true) => {
                say(&format!("`{rest}` skasowane"));
                wrote = true;
            }
            Ok(false) => say(&format!("nieznane pole `{rest}` — spróbuj `?`")),
            Err(e) => say(&format!("nie skasowałem: {e:#}")),
        },

        "reboot" | "reset" => {
            say("restart");
            // Bezpieczny moment: panel jeszcze nie dostał zasilania (Epd::new jest
            // dużo niżej), a radio nie jest podniesione. Reset przy podniesionych
            // szynach TPS65185 potrafi uszkodzić panel.
            let _ = std::io::stdout().flush();
            std::thread::sleep(Duration::from_millis(100));
            // SAFETY: prosty restart z ESP-IDF.
            unsafe { sys::esp_restart() };
        }

        "done" | "exit" | "quit" | "q" | "koniec" => return Verdict::Leave,

        other => say(&format!("nie znam `{other}` — spróbuj `?`")),
    }

    Verdict::Continue { wrote }
}

fn set_text(
    store: &mut Store,
    value: &str,
    label: &str,
    write: impl FnOnce(&mut Store, &str) -> anyhow::Result<()>,
) -> bool {
    if value.is_empty() {
        say(&format!(
            "{label}: brak wartości (skasować: `clear <pole>`)"
        ));
        return false;
    }
    if value.len() > MAX_VALUE {
        say(&format!(
            "{label}: {} znaków, limit to {MAX_VALUE}",
            value.len()
        ));
        return false;
    }
    match write(store, value) {
        Ok(()) => {
            say(&format!("{label}: zapisane"));
            true
        }
        Err(e) => {
            say(&format!("{label}: nie zapisałem — {e:#}"));
            false
        }
    }
}

fn banner(config: &Config, window: Duration) {
    say("");
    say("=========================================================");
    say(" t5s3pro — konfiguracja");
    say("=========================================================");
    if !config.is_provisioned() {
        say(" Urządzenie nie ma jeszcze SSID albo adresu kalendarza.");
        say(" Minimum to dwa polecenia:");
        say("   ssid <nazwa sieci>");
        say("   pass <hasło>");
        say("   ics  <adres kanału iCal>");
    }
    say(&format!(
        " Okno: {} s. Każde polecenie je przedłuża. `?` — pomoc.",
        window.as_secs()
    ));
    say("");
    show(config);
    prompt();
}

fn help() {
    say("  ssid <nazwa>          nazwa sieci WiFi");
    say("  pass <hasło>          hasło WiFi");
    say("  ics  <url>            kanał iCal (główny)");
    say("  ics2 <url>            kanał iCal (dodatkowy)");
    say("  ota  <url>            manifest OTA; bez niego OTA jest wyłączone");
    say("  tz   <IANA>           strefa, np. Europe/Warsaw");
    say("  interval <sekundy>    odstęp odświeżania na baterii");
    say("  clear <pole>          skasuj: ssid pass ics ics2 ota tz interval");
    say("  show                  pokaż konfigurację");
    say("  reboot                restart");
    say("  done                  wyjdź i jedź dalej");
    say("");
    say("  Wartość to CAŁA reszta wiersza — spacje w haśle są w porządku.");
}

fn show(config: &Config) {
    let or_none = |v: &Option<String>| v.clone().unwrap_or_else(|| "—".to_string());

    say(&format!("  sieć       : {}", or_none(&config.ssid)));
    say(&format!(
        "  hasło      : {}",
        match &config.password {
            Some(p) => format!("ustawione ({} znaków)", p.chars().count()),
            None => "—".to_string(),
        }
    ));
    // Adresy przechodzą przez `redact`: prywatny link iCal jest stałym bearerem
    // do całego kalendarza, a konsola bywa nagrywana razem z resztą logu.
    say(&format!(
        "  kalendarz  : {}",
        config
            .ics_url
            .as_deref()
            .map(redact)
            .unwrap_or_else(|| "—".to_string())
    ));
    say(&format!(
        "  kalendarz 2: {}",
        config
            .ics_url_secondary
            .as_deref()
            .map(redact)
            .unwrap_or_else(|| "—".to_string())
    ));
    say(&format!(
        "  strefa     : {}{}",
        config.tz().name(),
        if config.timezone.is_none() {
            " (domyślna)"
        } else {
            ""
        }
    ));
    say(&format!(
        "  odstęp     : {}",
        match config.interval_s {
            Some(s) => format!("{s} s"),
            None => "domyślny".to_string(),
        }
    ));
    // Manifest OTA nie jest sekretem — pokazujemy go w całości, bo pomyłka
    // w adresie jest tu najczęstszym błędem i musi być widoczna.
    say(&format!("  OTA        : {}", or_none(&config.ota_url)));
    say(&format!("  obrót      : {}", config.rotation.as_str()));
}

fn echo(bytes: &[u8]) {
    let mut out = std::io::stdout();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

fn say(text: &str) {
    println!("{text}");
    let _ = std::io::stdout().flush();
}

fn prompt() {
    print!("> ");
    let _ = std::io::stdout().flush();
}

/// Instaluje sterownik i przekierowuje przez niego stdio.
///
/// Bez sterownika odczyt z USB-Serial-JTAG jest nieblokujący i trzeba go odpytywać
/// w pętli; ze sterownikiem dostajemy kolejkę z timeoutem, czyli dokładnie to,
/// czego potrzebuje edytor jednego wiersza. `esp_vfs_usb_serial_jtag_use_driver`
/// jest wymagane w parze — inaczej `println!` pisze prosto do rejestrów peryferium,
/// obok bufora sterownika, i wyjście miesza się samo ze sobą.
fn install_driver() -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static INSTALLED: AtomicBool = AtomicBool::new(false);

    if INSTALLED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let mut config = sys::usb_serial_jtag_driver_config_t {
        tx_buffer_size: 1024,
        rx_buffer_size: 1024,
    };
    // SAFETY: konfiguracja żyje do końca wywołania, a sterownik kopiuje ją do siebie.
    let err = unsafe { sys::usb_serial_jtag_driver_install(&mut config) };
    if err != sys::ESP_OK {
        INSTALLED.store(false, Ordering::SeqCst);
        anyhow::bail!("usb_serial_jtag_driver_install: {err}");
    }
    // SAFETY: sterownik jest już zainstalowany, co jest jedynym wymaganiem tej funkcji.
    unsafe { sys::esp_vfs_usb_serial_jtag_use_driver() };
    Ok(())
}

fn read_bytes(buf: &mut [u8]) -> usize {
    let ticks = POLL_MS * sys::configTICK_RATE_HZ / 1000;
    // SAFETY: bufor jest ważny na czas wywołania, długość zgodna z jego rozmiarem.
    let n = unsafe {
        sys::usb_serial_jtag_read_bytes(
            buf.as_mut_ptr() as *mut core::ffi::c_void,
            buf.len() as u32,
            ticks,
        )
    };
    n.max(0) as usize
}
