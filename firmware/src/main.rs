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
mod diag;
mod epd;
mod i2c;
mod net;
mod power;
mod source;
mod store;

use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, NaiveDate, NaiveDateTime};
use dashboard::model::{Battery, DayGroup, NetState, SourceTag};
use dashboard::{Action, Fonts, Gray8, Model, Rotation};
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

/// Wersja pokazywana w stopce i **porównywana przez OTA**.
///
/// To nie jest `CARGO_PKG_VERSION`, tylko łańcuch z `tools/version.sh` z doklejonym
/// commitem: `0.1.0+g1a2b3c4`. Sam semver z `Cargo.toml` zmienia się raz na wydanie,
/// więc dopóki nikt go ręcznie nie podbije, urządzenie zawsze widzi w manifeście
/// swoją własną wersję i nie aktualizuje się nigdy — nie da się nawet sprawdzić,
/// czy OTA działa. Ten sam skrypt wypełnia `version` w `ota.json`.
///
/// Deskryptor aplikacji (`esp_app_desc!`) niesie dalej goły semver, bo makro
/// z esp-idf-sys czyta `CARGO_PKG_VERSION` na sztywno. `espflash` pokaże więc
/// „0.1.0", a OTA porównuje pełny łańcuch — `check-image.sh` pilnuje, żeby ten
/// z `ota.json` faktycznie był w obrazie.
const VERSION: &str = env!("T5_VERSION");

/// Ile dni do przodu pokazujemy.
const HORIZON_DAYS: i64 = 14;

/// Jak krótko śpimy po dotknięciu „odśwież".
///
/// Pobranie wymaga radia, a radia nie wolno podnosić przy podniesionych szynach
/// panelu. Zamiast tego zasypiamy na chwilę i wracamy normalną ścieżką, w której
/// kolejność jest właściwa.
const FETCH_SOON_S: u64 = 5;

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
        // Pełny raport tylko przy zimnym starcie: skan magistrali kosztuje ponad sto
        // transakcji, a przy każdym wybudzeniu to zmarnowany czas z włączonym radiem
        // peryferiów. Przy okazji jest to jedyne miejsce, w którym urządzenie mówi
        // wprost, co widzi na płytce — patrz `diag`.
        diag::cold_boot_report(&bus, &hw, uptime_ms());
    }

    // --- 2. Konfiguracja ------------------------------------------------------
    let mut store = Store::open(nvs_partition.clone()).context("nie mogę otworzyć NVS")?;
    let config = store.load();
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

    // Zamiennik miernika: licznik kulombów BQ27220 uśredniony od linii bazowej.
    diag::energy_line(&mut state, power_status, fuel, net::time::now_unix());

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
    let mode = power::mode_from_hardware(&policy, power_status, fuel, now);
    info!(
        "tryb: {mode:?}, ogniwo: {:?}%, USB: {}",
        fuel.percent, power_status.usb_present
    );

    // --- 5. Sieć ---------------------------------------------------------------
    let requested = std::mem::take(&mut state.fetch_requested);
    if requested {
        info!("pobranie na życzenie z poprzedniego cyklu");
    }

    let mut net_state = NetState::Ok;
    let mut events = Vec::new();
    let mut content_crc = state.last_content_crc;
    let mut fetched = false;

    if !config.is_provisioned() {
        warn!("urządzenie nieskonfigurowane — pokazuję ekran konfiguracji");
        net_state = NetState::NeedsAuth;
    // Dotknięcie „odśwież" w poprzednim cyklu wymusza pobranie niezależnie od trybu,
    // także w nocy. Flagę zdejmujemy tutaj, a nie po udanym pobraniu: gdyby sieć
    // zawiodła, powtarzanie życzenia sprzed godziny nie jest już tym, o co ktoś prosił.
    } else if requested || (policy.should_fetch(mode) && !matches!(mode, Mode::Night)) {
        match fetch_everything(
            peripherals.modem,
            sysloop,
            nvs_partition,
            &config,
            &hw,
            &mut state,
            &mut store,
            home_tz,
            now,
            power::may_update(&policy, mode, fuel),
        ) {
            Ok(out) => {
                // Restart natychmiast: radio jest już wyłączone, a panelu jeszcze
                // nie dotykaliśmy — `Epd::new` jest niżej. Reset przy podniesionych
                // szynach TPS65185 potrafi uszkodzić panel, więc kolejność ma znaczenie.
                //
                // `state.store()` tutaj NIE MA i to nie jest przeoczenie: bootloader
                // przeładowuje segmenty RTC z obrazu przy każdym resecie, który nie
                // jest wybudzeniem z deep sleepu, więc cokolwiek byśmy zapisali,
                // nowy obraz i tak zobaczy zimny start. Cena jest znana i jednorazowa:
                // jeden pełny skan AP i jedno odświeżenie panelu bez porównania CRC.
                // Licznik prób OTA, który jako jedyny NIE MOŻE tego przeżyć bez szkody,
                // siedzi w NVS — patrz nagłówek `devlogic::ota`.
                if out.ota_installed {
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
        // Pierwsze rysowanie w tym wybudzeniu MUSI być pełne — `back_fb` epdiy
        // powstał przed chwilą wyzerowany do bieli i nie wie nic o tym, co zostało
        // na szkle. Dopiero kolejne, w oknie interaktywnym, mogą być szybkie.
        let screen = match paint(
            &mut epd,
            &model,
            &mut state,
            temperature,
            rotation,
            Refresh::Full,
        ) {
            Ok(screen) => screen,
            Err(e) => {
                // Nieudane malowanie nie zwalnia nas z poprawnego zaśnięcia.
                error!("rysowanie nie powiodło się: {e:#}");
                dashboard::Screen::default()
            }
        };

        let boot_count = state.boot_count;
        if wants_interaction(
            config.is_provisioned(),
            power_status.usb_present,
            boot_count,
            woke_by_button,
        ) {
            let changed = interactive_loop(
                &mut epd,
                &bus,
                &hw,
                &mut store,
                &mut state,
                &model,
                screen,
                temperature,
                rotation,
                if config.is_provisioned() {
                    IDLE_MS
                } else {
                    FRESH_IDLE_MS
                },
                boot_count <= 1,
            );
            if changed {
                // Świeżo wpisana konfiguracja ma zadziałać teraz, a nie za pół godziny.
                info!("konfiguracja zmieniona — pobieram przy najbliższym wybudzeniu");
                state.request_fetch();
            }
        }
    } else {
        info!("treść bez zmian — pomijam odświeżenie panelu");
    }

    // --- 7. Sekwencja wyłączania i sen -----------------------------------------
    let sleep_s = if state.fetch_requested {
        FETCH_SOON_S
    } else {
        let base = policy.sleep_seconds(mode, now);
        power::align_to_minute(now, base.saturating_mul(state.backoff_multiplier()))
    };

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
    store: &mut Store,
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
            Some(url) => match net::ota::check_and_apply(url, VERSION, store) {
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

/// Renderuje agendę i wypycha ją na panel, oddając obszary dotykowe.
fn paint(
    epd: &mut Epd,
    model: &Model,
    state: &mut RtcState,
    temperature_c: i32,
    rotation: Rotation,
    mode: Refresh,
) -> Result<dashboard::Screen> {
    let fonts = Fonts::embedded();
    let mut canvas = Gray8::new(rotation);

    let started = std::time::Instant::now();
    let screen = dashboard::render(model, &fonts, &mut canvas);
    info!("render: {} ms", started.elapsed().as_millis());

    present(epd, &canvas, state, temperature_c, mode)?;
    Ok(screen)
}

/// Wypycha gotowe płótno na panel.
fn present(
    epd: &mut Epd,
    canvas: &Gray8,
    state: &mut RtcState,
    temperature_c: i32,
    mode: Refresh,
) -> Result<()> {
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

    // PIERWSZE rysowanie w danym wybudzeniu musi być pełne, i to nie z ostrożności.
    //
    // Każde wybudzenie to świeży boot: deep sleep gasi PSRAM, więc `epd_hl_init`
    // alokuje `back_fb` od nowa i zeruje go do bieli. epdiy nie ma więc żadnej wiedzy
    // o tym, co faktycznie zostało na panelu, a rysuje wyłącznie różnicę względem tego
    // założenia. Bez czyszczenia stary tusz zostaje wszędzie tam, gdzie nowa klatka
    // jest biała. Szczegóły i cała mechanika: `Epd::present`.
    //
    // `Refresh::Fast` jest poprawne dopiero dla DRUGIEGO i kolejnego rysowania
    // w obrębie tego samego wybudzenia — wtedy `back_fb` opisuje prawdę. Dokładnie
    // to robi okno interaktywne: wpisany znak, zmiana strony, ekran szczegółów.
    // Za to, żeby nie zacząć od `Fast`, odpowiada wołający.
    let started = std::time::Instant::now();
    let result = epd.present(canvas, mode, temperature_c);
    info!("odświeżenie {mode:?}: {} ms", started.elapsed().as_millis());

    // Cokolwiek się stało, szyny mają zejść.
    epd.ensure_powered_off();
    result?;

    state.record_refresh(mode == Refresh::Full);
    Ok(())
}

/// Ekran startowy dla urządzenia bez konfiguracji.
///
/// Pokazywany, dopóki w NVS nie ma danych WiFi i adresu kalendarza — czyli zaraz
/// po wgraniu firmware'u z przeglądarki.
fn provisioning_model(now: NaiveDateTime) -> Model {
    let mut model = Model::empty(now);
    model.firmware = format!("t5s3pro {VERSION}");
    model.net = NetState::NeedsAuth;
    model.tiles = vec![
        dashboard::model::Tile::new("krok 1", "dotknij ekranu"),
        dashboard::model::Tile::new("krok 2", "wpisz sieć"),
        dashboard::model::Tile::new("krok 3", "wpisz adres iCal"),
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

/// Ile milisekund bez zdarzenia zamyka okno interaktywne agendy.
const IDLE_MS: u64 = 20_000;

/// To samo na urządzeniu bez konfiguracji.
///
/// Dwadzieścia sekund wystarczy komuś, kto wie, gdzie stuknąć. Po pierwszym wgraniu
/// firmware'u trzeba jeszcze przeczytać ekran i zorientować się, że plakietka jest
/// przyciskiem — a urządzenie bez konfiguracji i tak wisi wtedy na kablu.
const FRESH_IDLE_MS: u64 = 60_000;

/// To samo dla ekranu konfiguracji. Dłużej, bo wstukanie 120-znakowego adresu iCal
/// to kilka minut, a przerwa na sprawdzenie hasła w telefonie jest normalna.
const SETUP_IDLE_MS: u64 = 90_000;

/// Po tylu nieudanych odczytach z rzędu uznajemy kontroler dotyku za nieobecny.
/// Odpytywanie martwej magistrali przez całe okno to czysto stracona energia.
const TOUCH_ERRORS_BEFORE_GIVING_UP: u8 = 8;

/// Ile znaków wolno wpisać między pełnymi odświeżeniami.
///
/// Każde naciśnięcie klawisza to odświeżenie DU (~0,28 s), które zostawia duchy.
/// Ten sam próg co dla agendy — patrz [`epd::FAST_REFRESHES_BEFORE_FULL`].
const EDITS_BEFORE_FULL: u8 = epd::FAST_REFRESHES_BEFORE_FULL;

/// Czy po narysowaniu ekranu warto zostać na jawie i czekać na dotyk.
///
/// Okno kosztuje ~40 mA przez cały swój czas, więc na baterii otwiera się tylko
/// wtedy, gdy coś wskazuje na obecność człowieka: naciśnięty przycisk, wpięty kabel
/// albo urządzenie bez konfiguracji przy zimnym starcie — bo inaczej nie ma jak tej
/// konfiguracji wprowadzić.
///
/// Urządzenie bez konfiguracji na baterii, wybudzone timerem, okna **nie** dostaje.
/// Rysuje ekran startowy i wraca spać; drogą wejścia jest wtedy przycisk BOOT, który
/// jako jedyny na tej płytce potrafi wybudzić z deep sleepu.
fn wants_interaction(provisioned: bool, usb: bool, boot_count: u32, woke_by_button: bool) -> bool {
    woke_by_button || usb || (!provisioned && boot_count <= 1)
}

/// Wykrywanie zbocza dotyku.
///
/// GT911 raportuje palec **przy każdym cyklu skanowania**, więc bez tego
/// przytrzymanie klawisza wpisałoby go kilkadziesiąt razy.
#[derive(Default)]
struct FingerEdge {
    down: bool,
}

impl FingerEdge {
    /// Zwraca punkt wyłącznie w momencie położenia palca na szkle.
    fn press(&mut self, touch: &board::gt911::Gt911) -> Result<Option<board::gt911::TouchPoint>> {
        use board::gt911::Report;
        match touch.read()? {
            Report::Down(point) if !self.down => {
                self.down = true;
                Ok(Some(point))
            }
            Report::Down(_) => Ok(None),
            Report::Up => {
                self.down = false;
                Ok(None)
            }
            Report::Idle => Ok(None),
        }
    }
}

/// Okno, w którym urządzenie reaguje na dotyk i na przycisk.
///
/// # Podział ról między przyciskami a dotykiem
///
/// * **BOOT (GPIO0)** budzi z deep sleepu. Jest pinem RTC, więc może być źródłem
///   `ext1` — i jest jedynym przyciskiem na tej płytce, który to potrafi.
/// * **S3 (PCA9535 `IO1_2`, aktywny niskim)** obraca ekran. Wisi na ekspanderze I²C,
///   a INT ekspandera idzie na **GPIO38**, który na ESP32-S3 **nie jest pinem RTC**,
///   więc daje się odczytać wyłącznie na jawie.
/// * **Dotyk (GT911)** robi resztę: strony, szczegóły, wejście w konfigurację.
///
/// Obrót celowo **nie** ma przycisku na ekranie: to ustawienie sprzętowe, a nie
/// element treści, i gdyby był kafelkiem, zabierałby miejsce w każdym układzie.
///
/// Uwaga na opis na płytce: nadruk przy custom buttonie sugeruje `IO48`, ale GPIO48
/// to `EP_CKV` — zegar bramki panelu. Mapowanie na `IO1_2` pochodzi z
/// `docs/hardware.md`, zgodnie w trzech źródłach vendora.
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
/// Zwraca `true`, jeśli konfiguracja się zmieniła — wołający wie wtedy, że warto
/// przy najbliższej okazji sięgnąć po sieć.
#[allow(clippy::too_many_arguments)]
fn interactive_loop(
    epd: &mut Epd,
    bus: &I2cBus,
    hw: &Board,
    store: &mut Store,
    state: &mut RtcState,
    model: &Model,
    mut screen: dashboard::Screen,
    temperature_c: i32,
    mut rotation: Rotation,
    window_ms: u64,
    verbose: bool,
) -> bool {
    use std::time::{Duration, Instant};

    let touch = board::gt911::open(bus, verbose);
    let mut finger = FingerEdge::default();
    let mut custom = Button::new(hw.expander.button_pressed().unwrap_or(false));
    let mut model = model.clone();
    let mut errors = 0u8;

    info!("okno interaktywne: {window_ms} ms");
    let mut deadline = Instant::now() + Duration::from_millis(window_ms);

    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(SAMPLE_MS));

        if custom.pressed(hw.expander.button_pressed().unwrap_or(false)) {
            rotation = rotation.toggled();
            info!("obrót ekranu -> {rotation:?}");
            if let Err(e) = store.set_rotation(rotation) {
                warn!("nie mogę zapisać obrotu: {e:#}");
            }
            screen = repaint(epd, &model, state, temperature_c, rotation, Refresh::Full);
            deadline = Instant::now() + Duration::from_millis(window_ms);
            continue;
        }

        let Some(touch) = touch.as_ref() else {
            continue;
        };

        let point = match finger.press(touch) {
            Ok(point) => {
                errors = 0;
                point
            }
            Err(e) => {
                errors += 1;
                if errors >= TOUCH_ERRORS_BEFORE_GIVING_UP {
                    warn!("dotyk nie odpowiada, zamykam okno: {e:#}");
                    break;
                }
                continue;
            }
        };
        let Some(point) = point else {
            continue;
        };

        // Dotyk przychodzi w układzie PANELU, obszary są w układzie płótna.
        //
        // Obie pary współrzędnych idą do logu i to jest celowe: dopóki orientacja osi
        // GT911 jest założeniem, a nie pomiarem, dotknięcie czterech rogów i odczytanie
        // tych linii jest jedynym sposobem, żeby ją ustawić. Patrz `SWAP_XY` w
        // `board::gt911`.
        let (x, y) = rotation.panel_to_canvas(point.x, point.y);
        let Some(action) = screen.hit(x, y) else {
            info!(
                "dotyk: panel ({}, {}) -> płótno ({x}, {y}), brak obszaru",
                point.x, point.y
            );
            continue;
        };
        info!(
            "dotyk: panel ({}, {}) -> płótno ({x}, {y}) -> {action:?}",
            point.x, point.y
        );
        deadline = Instant::now() + Duration::from_millis(window_ms);

        match action {
            Action::OpenSetup => {
                if setup_screen(epd, touch, store, state, temperature_c, rotation) {
                    // Po zapisie nie ma na co czekać: ekran pod spodem pokazuje stan
                    // sprzed konfiguracji, a prawdziwa treść wymaga sięgnięcia po sieć,
                    // czyli osobnego wybudzenia. Wychodzimy od razu, zamiast trzymać
                    // urządzenie na jawie przez pełne okno bezczynności.
                    return true;
                }
                screen = repaint(epd, &model, state, temperature_c, rotation, Refresh::Full);
            }
            Action::NextPage => {
                model.page += 1;
                screen = repaint(epd, &model, state, temperature_c, rotation, Refresh::Fast);
            }
            Action::PrevPage => {
                model.page = model.page.saturating_sub(1);
                screen = repaint(epd, &model, state, temperature_c, rotation, Refresh::Fast);
            }
            Action::ShowEvent(i) => {
                model.focus = Some(i);
                screen = repaint(epd, &model, state, temperature_c, rotation, Refresh::Fast);
            }
            Action::Back => {
                if model.focus.take().is_some() {
                    screen = repaint(epd, &model, state, temperature_c, rotation, Refresh::Fast);
                }
            }
            Action::RefreshNow => {
                // Pobranie wymaga radia, a radio jest już wyłączone i panel ma
                // podniesione szyny. Zamiast ryzykować brownout, zapisujemy życzenie:
                // najbliższe wybudzenie sięgnie po sieć niezależnie od trybu.
                info!("odświeżenie na życzenie — przy najbliższym wybudzeniu");
                state.request_fetch();
            }
            // Akcje ekranu konfiguracji nie mają tu obszarów dotykowych.
            _ => {}
        }
    }

    // Dotąd docieramy wyłącznie po wyczerpaniu okna bezczynności albo po awarii
    // dotyku — czyli bez zapisanej konfiguracji.
    false
}

/// Ekran konfiguracji: jedyna droga wprowadzania danych do urządzenia.
///
/// Zostajemy na jawie, dopóki użytkownik nie naciśnie „zapisz" albo nie przestanie
/// dotykać na [`SETUP_IDLE_MS`]. Zwraca `true`, jeśli cokolwiek zapisano.
fn setup_screen(
    epd: &mut Epd,
    touch: &board::gt911::Gt911,
    store: &mut Store,
    state: &mut RtcState,
    temperature_c: i32,
    rotation: Rotation,
) -> bool {
    use dashboard::setup::{Applied, Field, Setup};
    use std::time::{Duration, Instant};

    let config = store.load();
    let mut setup = Setup::new();
    setup.set(Field::Ssid, config.ssid.clone().unwrap_or_default());
    setup.set(Field::Password, config.password.clone().unwrap_or_default());
    setup.set(Field::Ics, config.ics_url.clone().unwrap_or_default());
    setup.set(
        Field::Ics2,
        config.ics_url_secondary.clone().unwrap_or_default(),
    );
    setup.set(Field::Timezone, config.timezone.clone().unwrap_or_default());
    setup.set(Field::Ota, config.ota_url.clone().unwrap_or_default());

    info!("ekran konfiguracji");
    let mut screen = repaint_setup(epd, &setup, state, temperature_c, rotation, Refresh::Full);

    let mut finger = FingerEdge::default();
    let mut edits = 0u8;
    let mut errors = 0u8;
    let mut deadline = Instant::now() + Duration::from_millis(SETUP_IDLE_MS);

    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(SAMPLE_MS));

        let point = match finger.press(touch) {
            Ok(point) => {
                errors = 0;
                point
            }
            Err(e) => {
                errors += 1;
                if errors >= TOUCH_ERRORS_BEFORE_GIVING_UP {
                    warn!("dotyk nie odpowiada, zamykam konfigurację: {e:#}");
                    return false;
                }
                continue;
            }
        };
        let Some(point) = point else {
            continue;
        };

        let (x, y) = rotation.panel_to_canvas(point.x, point.y);
        let Some(action) = screen.hit(x, y) else {
            info!(
                "dotyk: panel ({}, {}) -> płótno ({x}, {y}), brak obszaru",
                point.x, point.y
            );
            continue;
        };
        deadline = Instant::now() + Duration::from_millis(SETUP_IDLE_MS);

        match setup.apply(action) {
            Applied::Ignored => {}
            Applied::Save => {
                let saved = save_setup(store, &setup);
                info!("konfiguracja zapisana: {saved}");
                return saved;
            }
            Applied::Edited | Applied::Relayout => {
                let mode = if edits >= EDITS_BEFORE_FULL {
                    edits = 0;
                    Refresh::Full
                } else {
                    edits += 1;
                    Refresh::Fast
                };
                screen = repaint_setup(epd, &setup, state, temperature_c, rotation, mode);
            }
        }
    }

    info!("konfiguracja: brak aktywności, wracam bez zapisu");
    false
}

/// Przenosi zawartość ekranu do NVS.
///
/// Puste pole zapisujemy jako pusty łańcuch, a nie pomijamy: użytkownik, który
/// wyczyścił drugi kalendarz, chce go mieć wyczyszczonego. `Store::load` i tak
/// traktuje pusty łańcuch jak brak wartości.
fn save_setup(store: &mut Store, setup: &dashboard::setup::Setup) -> bool {
    use dashboard::setup::Field;

    let writes: [(Field, fn(&mut Store, &str) -> Result<()>); 6] = [
        (Field::Ssid, |s, v| s.set_ssid(v)),
        (Field::Password, |s, v| s.set_password(v)),
        (Field::Ics, |s, v| s.set_ics_url(v)),
        (Field::Ics2, |s, v| s.set_ics_url_secondary(v)),
        (Field::Timezone, |s, v| s.set_timezone(v)),
        (Field::Ota, |s, v| s.set_ota_url(v)),
    ];

    let mut ok = true;
    for (field, write) in writes {
        if let Err(e) = write(store, setup.value(field)) {
            warn!("nie mogę zapisać pola {}: {e:#}", field.tab());
            ok = false;
        }
    }
    ok
}

/// Przerysowanie agendy z logowaniem błędu zamiast przerywania pętli.
///
/// W oknie interaktywnym nieudane odświeżenie nie jest powodem, żeby przestać
/// reagować — obszary dotykowe z poprzedniej klatki wciąż są prawdziwe.
fn repaint(
    epd: &mut Epd,
    model: &Model,
    state: &mut RtcState,
    temperature_c: i32,
    rotation: Rotation,
    mode: Refresh,
) -> dashboard::Screen {
    match paint(epd, model, state, temperature_c, rotation, mode) {
        Ok(screen) => screen,
        Err(e) => {
            error!("przerysowanie nie powiodło się: {e:#}");
            dashboard::Screen::default()
        }
    }
}

fn repaint_setup(
    epd: &mut Epd,
    setup: &dashboard::setup::Setup,
    state: &mut RtcState,
    temperature_c: i32,
    rotation: Rotation,
    mode: Refresh,
) -> dashboard::Screen {
    let fonts = Fonts::embedded();
    let mut canvas = Gray8::new(rotation);
    let screen = dashboard::render_setup(setup, &fonts, &mut canvas);

    if let Err(e) = present(epd, &canvas, state, temperature_c, mode) {
        error!("rysowanie konfiguracji nie powiodło się: {e:#}");
    }
    screen
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

/// Ile milisekund minęło od startu układu — pomiar 5 z `docs/bringup.md`.
fn uptime_ms() -> u128 {
    // SAFETY: prosty getter z ESP-IDF, zwraca mikrosekundy od bootu.
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } as u128) / 1000
}
