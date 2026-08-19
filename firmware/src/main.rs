//! Kalendarzowy dashboard e-papierowy na LilyGo T5 E-Paper S3 Pro.
//!
//! Cykl życia jest cykliczny i bardzo krótki: obudź się → ustal czas → pobierz
//! kalendarz → narysuj → uśpij. Nic nie działa w tle, bo nic nie może — deep sleep
//! gasi PSRAM i całą stertę.
//!
//! ```text
//!   wybudzenie
//!       │
//!       ├─ magistrala I²C  ──►  ZGAŚ SZYNĘ LoRa/GPS   (pierwsza transakcja, zawsze)
//!       ├─ czas z RTC                                  (przed radiem)
//!       ├─ stan zasilania i ogniwa  ──►  wybór trybu
//!       ├─ [jeśli tryb pozwala]  WiFi ─► SNTP ─► HTTPS ─► parsowanie iCal ─► WiFi STOP
//!       ├─ render dashboardu (ten sam kod, co podgląd na hoście)
//!       ├─ wypchnięcie na panel                        (radio już wyłączone)
//!       └─ sekwencja wyłączania ──► deep sleep
//! ```
//!
//! Kolejność dwóch rzeczy jest krytyczna i nie wolno jej zmieniać:
//! 1. **Szyna LoRa/GPS gaśnie jako pierwsza.** Podciągnięcie R21 podnosi ją przy
//!    zimnym starcie; zostawiona kosztuje 25–35 mA, czyli około czterdziestu godzin.
//! 2. **Radio wyłącza się przed podniesieniem szyn panelu.** Panel ciągnie ~115 mA,
//!    szczyt nadajnika to ~340 mA; razem przez LDO na zużytym ogniwie to brownout,
//!    a reset w trakcie odświeżania z podniesionymi szynami TPS65185 potrafi
//!    uszkodzić panel.

mod board;
mod console;
mod epd;
mod i2c;
mod net;
mod power;
mod source;
mod store;

use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, NaiveDate, NaiveDateTime};
use dashboard::model::{Battery, DayGroup, NetState, SourceTag};
use dashboard::{Fonts, Gray8, Model, Rotation};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::reset::ResetReason;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{error, info, warn};

use crate::board::Board;
use crate::epd::{Epd, Refresh};
use crate::i2c::I2cBus;
use crate::power::rtc_state::RtcState;
use crate::power::{shutdown, Mode, Policy};
use crate::source::{ics::IcsSource, EventSource};
use crate::store::{Config, Store};

/// Wersja pokazywana w stopce i raportowana przez deskryptor aplikacji.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Ile dni do przodu pokazujemy.
const HORIZON_DAYS: i64 = 14;

/// Ile czekamy na kolejne naciśnięcie `S3`, zanim wrócimy do snu.
const ORIENTATION_WINDOW_MS: u64 = 8_000;

esp_idf_svc::sys::esp_app_desc!();

fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let state = RtcState::load();
    info!(
        "=== t5s3pro {VERSION} === boot #{}, powód: {:?}",
        state.boot_count,
        ResetReason::get()
    );

    match run(state) {
        Ok(sleep_s) => {
            // Cykl doszedł do końca, czyli obraz potrafi wstać, narysować i zasnąć.
            // Dopiero teraz odwołujemy rollback — wcześniej byłby dekoracją.
            net::ota::mark_running_valid();
            info!("cykl zakończony, śpię {sleep_s} s");
            shutdown::deep_sleep_for(sleep_s);
        }
        Err(e) => {
            error!("cykl zakończony błędem: {e:#}");
            // Nawet po błędzie musimy zasnąć — pętla resetów rozładuje ogniwo
            // szybciej niż cokolwiek innego.
            shutdown::deep_sleep_for(15 * 60);
        }
    }
}

fn run(mut state: RtcState) -> Result<u64> {
    let peripherals = Peripherals::take().context("nie mogę przejąć peryferiów")?;
    let sysloop = EspSystemEventLoop::take().context("nie mogę przejąć pętli zdarzeń")?;
    let nvs_partition = EspDefaultNvsPartition::take().context("nie mogę przejąć partycji NVS")?;

    // --- 0. Zwolnij zatrzaski magistrali panelu --------------------------------
    // Musi być przed `Epd::new()`. Bez tego po każdym wybudzeniu z deep sleepu panel
    // dostaje śmieci zamiast obrazu — pełne wyjaśnienie przy `release_epd_bus_hold`.
    shutdown::release_epd_bus_hold();

    // --- 1. Magistrala I²C i natychmiastowe zgaszenie szyny LoRa/GPS -----------
    let bus = I2cBus::new().context("nie mogę zestawić magistrali I2C")?;
    let hw = Board::open(&bus).context("nie mogę otworzyć układów na płytce")?;

    if state.boot_count <= 1 {
        // Skan magistrali tylko przy zimnym starcie — przy każdym wybudzeniu
        // to zmarnowany czas z włączonym zasilaniem.
        info!("urządzenia I2C: {:02X?}", bus.scan());
        if let Ok((p0, p1)) = hw.expander.read_inputs() {
            info!("ekspander PCA9535: port0={p0:#010b} port1={p1:#010b}");
        }
        info!("wariant RTC: {:?}", hw.rtc.probe_variant());
        if let Ok(size) = psram_size() {
            info!("PSRAM: {size} B");
            if size != 8 * 1024 * 1024 {
                warn!("PSRAM ma {size} B, oczekiwano 8388608 — sprawdź CONFIG_SPIRAM_MODE_OCT");
            }
        }
    }

    // --- 2. Konfiguracja i konsola --------------------------------------------
    let mut store = Store::open(nvs_partition.clone()).context("nie mogę otworzyć NVS")?;
    let mut config = store.load();

    // Konsola stoi PRZED wszystkim, co z konfiguracji korzysta: SSID wpisane przed
    // chwilą ma zadziałać w tym cyklu, a nie za pół godziny. Otwiera się wyłącznie
    // przy podłączonym hoście USB — szczegóły i arytmetyka w `console`.
    if console::host_attached() {
        if console::run(&mut store, &config) {
            config = store.load();
            info!("konfiguracja zmieniona z konsoli");
        }
    } else {
        info!("brak hosta USB — konsola konfiguracyjna pominięta");
    }

    let home_tz = config.tz();
    let rotation = config.rotation;

    // --- 3. Czas z RTC, przed radiem ------------------------------------------
    let time_source = net::time::seed_from_rtc(&hw.rtc, home_tz);
    info!("źródło czasu: {time_source:?}");

    // --- 4. Stan zasilania i wybór trybu --------------------------------------
    let power_status = hw.charger.status().unwrap_or_else(|e| {
        warn!("nie mogę odczytać ładowarki: {e:#}");
        board::bq25896::PowerStatus {
            usb_present: false,
            vbus_stat: 0,
            chrg_stat: 0,
        }
    });
    let fuel = hw.fuel.read();
    let temperature = hw.fuel.temperature_or_default();

    let mut policy = Policy::default();
    if let Some(interval) = config.interval_s {
        policy.active_interval_s = interval as u64;
    }

    let now = net::time::now_local(home_tz).unwrap_or_else(|| {
        NaiveDate::from_ymd_opt(2026, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
    });
    let mode = policy.mode(power_status, fuel, now);
    info!(
        "tryb: {mode:?}, ogniwo: {:?}%, USB: {}",
        fuel.percent, power_status.usb_present
    );

    // --- 5. Sieć ---------------------------------------------------------------
    let mut net_state = NetState::Ok;
    let mut events = Vec::new();
    let mut content_crc = state.last_content_crc;
    let mut fetched = false;

    if !config.is_provisioned() {
        warn!("urządzenie nieskonfigurowane — pokazuję ekran konfiguracji");
        net_state = NetState::NeedsAuth;
    } else if policy.should_fetch(mode) && !matches!(mode, Mode::Night) {
        match fetch_everything(
            peripherals.modem,
            sysloop,
            nvs_partition,
            &config,
            &hw,
            &mut state,
            home_tz,
            now,
            policy.should_update(mode, fuel),
        ) {
            Ok(out) => {
                // Restart natychmiast: radio jest już wyłączone, a panelu jeszcze
                // nie dotykaliśmy — `Epd::new` jest niżej. Reset przy podniesionych
                // szynach TPS65185 potrafi uszkodzić panel, więc kolejność ma znaczenie.
                if out.ota_installed {
                    state.last_known_unix = net::time::now_unix();
                    state.store();
                    info!("restart do nowego obrazu");
                    // SAFETY: prosty restart z ESP-IDF.
                    unsafe { esp_idf_svc::sys::esp_restart() };
                }

                content_crc = out.crc;
                events = out.events;
                fetched = true;
                state.record_success(net::time::now_unix(), content_crc);
            }
            Err(e) => {
                warn!("pobieranie nie powiodło się: {e:#}");
                state.record_failure();
                net_state = if state.last_success_unix > 0 {
                    NetState::Stale {
                        since: unix_to_local(state.last_success_unix, home_tz).unwrap_or(now),
                    }
                } else {
                    NetState::Offline
                };
            }
        }
    } else {
        info!("tryb {mode:?} nie sięga po sieć w tym cyklu");
        if state.last_success_unix > 0 {
            net_state = NetState::Stale {
                since: unix_to_local(state.last_success_unix, home_tz).unwrap_or(now),
            };
        }
    }

    // --- 6. Render i wypchnięcie na panel --------------------------------------
    // Radio jest już wyłączone — patrz komentarz na górze pliku.
    //
    // `Epd` powstaje TUTAJ, nie w `paint`, bo sekwencja wyłączania musi dostać go
    // w ręce po tym, jak rysowanie się skończy — także wtedy, gdy skończyło się
    // błędem. Panel z podniesionymi szynami wchodzący w deep sleep to 235 mA.
    let mut epd = Epd::new(&bus).context("nie mogę zainicjalizować panelu")?;

    // epdiy właśnie przestawiło cały port 1 ekspandera na wyjścia — łącznie z bitem
    // przycisku, którego samo nie używa. Bez tego odczyt przycisku zwraca „wciśnięty"
    // bez końca. Szczegóły: `Pca9535::reclaim_button_input`.
    if let Err(e) = hw.expander.reclaim_button_input() {
        warn!("nie mogę odzyskać bitu przycisku na ekspanderze: {e:#}");
    }

    let content_changed = content_crc != state.last_content_crc || !fetched;
    // Wybudzenie przyciskiem zawsze rysuje. Ktoś nacisnął, więc czegoś od urządzenia
    // chce — a za chwilę może chcieć obrócić ekran, co bez świeżej klatki nie ma sensu.
    let woke_by_button = woken_by_button();
    let needs_paint =
        content_changed || state.boot_count <= 1 || net_state != NetState::Ok || woke_by_button;

    if needs_paint {
        let model = if config.is_provisioned() {
            build_model(now, events, fuel, power_status.usb_present, net_state)
        } else {
            provisioning_model(now)
        };
        if let Err(e) = paint(&mut epd, &model, &mut state, temperature, rotation) {
            // Nieudane malowanie nie zwalnia nas z poprawnego zaśnięcia.
            error!("rysowanie nie powiodło się: {e:#}");
        }

        if woke_by_button {
            orientation_loop(
                &mut epd,
                &hw,
                &mut store,
                &model,
                &mut state,
                temperature,
                rotation,
            );
        }
    } else {
        info!("treść bez zmian — pomijam odświeżenie panelu");
    }

    // --- 7. Sekwencja wyłączania i sen -----------------------------------------
    let base = policy.sleep_seconds(mode, now);
    let sleep_s = base.saturating_mul(state.backoff_multiplier());
    let sleep_s = power::align_to_minute(now, sleep_s);

    state.last_known_unix = net::time::now_unix();
    state.store();

    shutdown::prepare_for_deep_sleep(&mut epd, &hw, false);
    if let Err(e) = shutdown::enable_button_wakeup() {
        warn!("nie mogę włączyć budzenia przyciskiem: {e:#}");
    }

    Ok(sleep_s)
}

/// Wynik fazy sieciowej.
struct Fetched {
    events: Vec<dashboard::model::CalEvent>,
    crc: u32,
    /// Nowy obraz leży już w wolnym slocie i slot startowy jest przestawiony.
    /// Wołający ma zrestartować — ale dopiero wtedy, gdy uzna, że wolno.
    ota_installed: bool,
}

/// Podnosi WiFi, synchronizuje czas, pobiera kalendarze, sprawdza aktualizację
/// i **wyłącza radio** przed powrotem.
#[allow(clippy::too_many_arguments)]
fn fetch_everything(
    modem: esp_idf_svc::hal::modem::Modem,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    config: &Config,
    hw: &Board,
    state: &mut RtcState,
    home_tz: chrono_tz::Tz,
    now: NaiveDateTime,
    ota_allowed: bool,
) -> Result<Fetched> {
    let ssid = config.ssid.as_deref().unwrap_or_default();
    let password = config.password.as_deref().unwrap_or_default();

    let wifi = net::wifi::Wifi::connect(modem, sysloop, nvs, ssid, password, state)?;
    if let Some(rssi) = wifi.rssi() {
        info!("RSSI: {rssi} dBm");
    }

    // Czas przed HTTPS — przy CONFIG_MBEDTLS_HAVE_TIME_DATE=y zły zegar to
    // odrzucony certyfikat.
    if let Err(e) = net::time::sync_sntp(&hw.rtc, home_tz) {
        warn!("SNTP zawiódł: {e:#}");
    }

    let now = net::time::now_local(home_tz).unwrap_or(now);
    let from = now.date().and_hms_opt(0, 0, 0).unwrap_or(now);
    let to = from + ChronoDuration::days(HORIZON_DAYS);

    let mut events = Vec::new();
    let mut crc = 0u32;

    let mut sources: Vec<IcsSource> = Vec::new();
    if let Some(url) = &config.ics_url {
        sources.push(IcsSource::new(
            url,
            home_tz,
            SourceTag::Primary,
            "kalendarz główny",
        ));
    }
    if let Some(url) = &config.ics_url_secondary {
        sources.push(IcsSource::new(
            url,
            home_tz,
            SourceTag::Secondary,
            "kalendarz dodatkowy",
        ));
    }

    let mut any_ok = false;
    let mut last_error = None;
    for src in &sources {
        match src.fetch(from, to) {
            Ok(result) => {
                crc ^= result.content_crc;
                events.extend(result.events);
                any_ok = true;
            }
            Err(e) => {
                warn!("źródło `{}` zawiodło: {e:#}", src.name());
                last_error = Some(e);
            }
        }
    }

    // Aktualizacja firmware'u — ostatnia rzecz przy podniesionym radiu.
    //
    // Po kalendarzu, bo kalendarz jest funkcją urządzenia, a aktualizacja tylko
    // utrzymaniem: nieudane albo długie OTA nie ma prawa zabrać ekranowi treści.
    let mut ota_installed = false;
    if ota_allowed {
        match config.ota_url.as_deref() {
            Some(url) => match net::ota::check_and_apply(url, VERSION, state) {
                Ok(net::ota::Outcome::Installed { version }) => {
                    info!("OTA: wgrana wersja {version}, restart po wyłączeniu radia");
                    ota_installed = true;
                }
                Ok(net::ota::Outcome::UpToDate) => {}
                Ok(net::ota::Outcome::Skipped(reason)) => info!("OTA pominięte: {reason}"),
                Err(e) => warn!("OTA nie powiodło się: {e:#}"),
            },
            None => info!("OTA: brak adresu manifestu w NVS — pomijam"),
        }
    }

    // Radio w dół ZANIM dotkniemy panelu.
    wifi.shutdown();

    if !any_ok {
        return Err(last_error.unwrap_or_else(|| anyhow::anyhow!("brak skonfigurowanych źródeł")));
    }

    events.sort_by_key(|e| e.start);
    Ok(Fetched {
        events,
        crc,
        ota_installed,
    })
}

fn build_model(
    now: NaiveDateTime,
    events: Vec<dashboard::model::CalEvent>,
    fuel: board::bq27220::Fuel,
    charging: bool,
    net: NetState,
) -> Model {
    let mut model = Model::empty(now);
    model.firmware = format!("t5s3pro {VERSION}");
    model.battery = Battery {
        percent: fuel.percent,
        millivolts: fuel.millivolts,
        charging,
    };
    model.net = net;
    model.days = group_by_day(events);
    model
}

/// Grupuje wydarzenia po dniach, zachowując kolejność chronologiczną.
fn group_by_day(events: Vec<dashboard::model::CalEvent>) -> Vec<DayGroup> {
    let mut groups: Vec<DayGroup> = Vec::new();
    for event in events {
        let date = event.start.date();
        match groups.last_mut() {
            Some(g) if g.date == date => g.events.push(event),
            _ => groups.push(DayGroup {
                date,
                events: vec![event],
            }),
        }
    }
    groups
}

/// Renderuje i wypycha na panel.
fn paint(
    epd: &mut Epd,
    model: &Model,
    state: &mut RtcState,
    temperature_c: i32,
    rotation: Rotation,
) -> Result<()> {
    let fonts = Fonts::embedded();
    let mut canvas = Gray8::new(rotation);

    let started = std::time::Instant::now();
    dashboard::render(model, &fonts, &mut canvas);
    info!("render: {} ms", started.elapsed().as_millis());

    // Porównanie idzie z wymiarami PANELU, nie płótna. Płótno jest pionowe (540×960),
    // panel skanuje poziomo (960×540) — obrót robi `pack4`.
    let (w, h) = epd.dimensions();
    if (w, h)
        != (
            dashboard::PANEL_WIDTH as i32,
            dashboard::PANEL_HEIGHT as i32,
        )
    {
        warn!(
            "epdiy raportuje panel {w}x{h}, spodziewano się {}x{}",
            dashboard::PANEL_WIDTH,
            dashboard::PANEL_HEIGHT
        );
    }

    // Zawsze pełne odświeżenie, i to nie z ostrożności.
    //
    // Każde wybudzenie to świeży boot: deep sleep gasi PSRAM, więc `epd_hl_init`
    // alokuje `back_fb` od nowa i zeruje go do bieli. epdiy nie ma więc żadnej wiedzy
    // o tym, co faktycznie zostało na panelu, a rysuje wyłącznie różnicę względem tego
    // założenia. Bez czyszczenia stary tusz zostaje wszędzie tam, gdzie nowa klatka
    // jest biała. Szczegóły i cała mechanika: `Epd::present`.
    //
    // `Refresh::Fast` odżyje dopiero przy rysowaniu w obrębie jednego wybudzenia
    // (dotyk, ekran szczegółów) — wtedy `back_fb` opisuje prawdę i licznik
    // `RtcState::needs_full_refresh` zacznie mieć sens.
    let mode = Refresh::Full;

    let started = std::time::Instant::now();
    let result = epd.present(&canvas, mode, temperature_c);
    info!("odświeżenie {mode:?}: {} ms", started.elapsed().as_millis());

    // Cokolwiek się stało, szyny mają zejść.
    epd.ensure_powered_off();
    result?;

    state.record_refresh(true);
    Ok(())
}

/// Ekran startowy dla urządzenia bez konfiguracji.
///
/// Pokazywany, dopóki w NVS nie ma danych WiFi i adresu kalendarza — czyli zaraz
/// po wgraniu firmware'u z przeglądarki. Drogą wyjścia z tego ekranu jest konsola
/// konfiguracyjna po USB — patrz [`console`].
fn provisioning_model(now: NaiveDateTime) -> Model {
    let mut model = Model::empty(now);
    model.firmware = format!("t5s3pro {VERSION}");
    model.net = NetState::NeedsAuth;
    model.tiles = vec![
        dashboard::model::Tile::new("krok 1", "podłącz USB"),
        dashboard::model::Tile::new("krok 2", "otwórz konsolę"),
        dashboard::model::Tile::new("krok 3", "ssid, pass, ics"),
    ];
    model
}

fn unix_to_local(unix: i64, tz: chrono_tz::Tz) -> Option<NaiveDateTime> {
    use chrono::TimeZone;
    tz.timestamp_opt(unix, 0).single().map(|d| d.naive_local())
}

/// Czy to wybudzenie przyszło z przycisku, czy z timera.
fn woken_by_button() -> bool {
    // SAFETY: prosty getter z ESP-IDF.
    let cause = unsafe { esp_idf_svc::sys::esp_sleep_get_wakeup_cause() };
    cause == esp_idf_svc::sys::esp_sleep_source_t_ESP_SLEEP_WAKEUP_EXT1
}

/// Po wybudzeniu przyciskiem zostajemy chwilę na nogach, żeby dało się obrócić ekran.
///
/// Podział ról między przyciskami nie jest wyborem projektowym, tylko konsekwencją
/// sprzętu:
///
/// * **BOOT (GPIO0)** budzi z deep sleepu. Jest pinem RTC, więc może być źródłem
///   `ext1` — i jest jedynym przyciskiem na tej płytce, który to potrafi.
/// * **S3 (PCA9535 `IO1_2`, aktywny niskim)** obraca ekran. Wisi na ekspanderze I²C,
///   a INT ekspandera idzie na **GPIO38**, który na ESP32-S3 **nie jest pinem RTC**
///   (RTC to GPIO0–21). Deep sleep nie ma go jak zauważyć, więc `S3` daje się odczytać
///   wyłącznie na jawie.
///
/// Uwaga na opis na płytce: nadruk przy custom buttonie sugeruje `IO48`, ale GPIO48
/// to `EP_CKV` — zegar bramki panelu, sterowany przez RMT. Mapowanie na `IO1_2`
/// pochodzi z `docs/hardware.md`, zgodnie w trzech źródłach vendora.
///
/// Pozostałe przyciski odpadają: `PWR` należy do PMIC-a, `RESET` restartuje układ,
/// a `HOME` pod ekranem to klawisz kontrolera dotyku GT911 — bez sterownika dotyku
/// nie ma go jak odczytać.
///
/// Stąd układ: BOOT budzi, custom przełącza orientację. Przez
/// [`ORIENTATION_WINDOW_MS`] każde naciśnięcie customa przestawia pion ↔ poziom
/// i przerysowuje; okno startuje od nowa po każdym przełączeniu.
///
/// # BOOT tu nie występuje, i to jest świadome
///
/// Wcześniejsza wersja czytała BOOT jako wyjście awaryjne, reagujące na przytrzymanie
/// ~2 s. **Nie działało i nie może działać.** Po wybudzeniu przez `ext1` pad GPIO0
/// zostaje pod kontrolą RTC_IO, a cyfrowa ścieżka wejściowa jest odcięta, więc
/// `gpio_get_level(0)` zwraca zero niezależnie od tego, czy ktoś trzyma przycisk.
/// Licznik przytrzymania dobijał więc do progu przy każdym wybudzeniu i przestawiał
/// orientację z powrotem, jakieś dwie sekundy po tym, jak użytkownik ją zmienił.
///
/// Odzyskanie GPIO0 wymagałoby `rtc_gpio_deinit(0)` przed konfiguracją jako wejście.
/// Nie robimy tego, bo custom button działa, a drugi przycisk robiący to samo jest
/// tylko drugą rzeczą, która może się zepsuć.
#[allow(clippy::too_many_arguments)]
fn orientation_loop(
    epd: &mut Epd,
    hw: &Board,
    store: &mut Store,
    model: &Model,
    state: &mut RtcState,
    temperature_c: i32,
    mut rotation: Rotation,
) {
    use std::time::{Duration, Instant};

    info!("wybudzenie przyciskiem — {ORIENTATION_WINDOW_MS} ms na obrót ekranu (S3)");

    if let Ok((_, p1)) = hw.expander.read_inputs() {
        info!("okno obrotu: port1 ekspandera = {p1:#010b}");
    }

    let mut deadline = Instant::now() + Duration::from_millis(ORIENTATION_WINDOW_MS);
    let mut custom = Button::new(hw.expander.button_pressed().unwrap_or(false));

    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(SAMPLE_MS));

        if custom.pressed(hw.expander.button_pressed().unwrap_or(false)) {
            rotation = rotation.toggled();
            info!("obrót ekranu -> {rotation:?}");

            if let Err(e) = store.set_rotation(rotation) {
                warn!("nie mogę zapisać obrotu: {e:#}");
            }
            if let Err(e) = paint(epd, model, state, temperature_c, rotation) {
                error!("przerysowanie po obrocie nie powiodło się: {e:#}");
            }

            deadline = Instant::now() + Duration::from_millis(ORIENTATION_WINDOW_MS);
        }
    }
}

/// Odstęp próbkowania przycisków.
const SAMPLE_MS: u64 = 20;

/// Ile kolejnych zgodnych próbek uznaje zmianę stanu za prawdziwą.
///
/// Trzy próbki po 20 ms to 60 ms — więcej niż drganie styku, mniej niż najkrótsze
/// świadome naciśnięcie. Chroni też przed pływającym wejściem, gdyby okazało się,
/// że custom button nie ma podciągnięcia.
const STABLE_SAMPLES: u8 = 3;

/// Przycisk z odbiciem, zgłaszający wyłącznie zbocze narastające.
struct Button {
    state: bool,
    candidate: bool,
    count: u8,
}

impl Button {
    /// Stan początkowy czytamy z rzeczywistości: użytkownik prawie na pewno wciąż
    /// trzyma BOOT, którym przed chwilą wybudził, i to nie ma być policzone jako
    /// naciśnięcie.
    fn new(initial: bool) -> Self {
        Self {
            state: initial,
            candidate: initial,
            count: STABLE_SAMPLES,
        }
    }

    /// Zwraca `true` tylko w momencie przejścia „puszczony → wciśnięty".
    fn pressed(&mut self, level: bool) -> bool {
        if level == self.candidate {
            self.count = self.count.saturating_add(1);
        } else {
            self.candidate = level;
            self.count = 1;
        }

        if self.count >= STABLE_SAMPLES && self.state != self.candidate {
            self.state = self.candidate;
            return self.state;
        }
        false
    }
}

fn psram_size() -> Result<usize> {
    // SAFETY: prosty getter z ESP-IDF.
    Ok(unsafe { esp_idf_svc::sys::esp_psram_get_size() })
}
