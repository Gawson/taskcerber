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
use devlogic::boot::BootStep;
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
/// Domyślny adres manifestu OTA, gdy w NVS nie ma własnego.
///
/// Wskazuje na GitHub Pages tego repozytorium — tam publikuje job `pages` z CI,
/// tym samym artefaktem, który powstaje z `tools/build-image.sh`. Dzięki temu
/// urządzenie aktualizuje się bez wklepywania czegokolwiek na ekranowej klawiaturze,
/// a wpis w konfiguracji zostaje jako NADPISANIE dla własnego serwera.
///
/// Zadziała dopiero, gdy repozytorium będzie publiczne: na darmowym planie
/// GitHub Pages nie obsługuje repozytoriów prywatnych.
const DEFAULT_OTA_URL: &str = "https://gawson.github.io/taskcerber/ota.json";

/// Jak często synchronizować zegar. Doba.
///
/// Patrz uzasadnienie przy wywołaniu: czekanie na SNTP kosztuje do dziesięciu sekund
/// z podniesioną anteną, a dryf PCF8563 jest dla kalendarza bez znaczenia.
const SNTP_INTERVAL_S: i64 = 24 * 60 * 60;

const HORIZON_DAYS: i64 = 14;

/// Horyzont kanału świąt — pełny rok z zapasem na przestępny.
///
/// Tyle wolno, bo święta to ~13 wydarzeń całodniowych rocznie, bez reguł
/// powtarzania, więc `MAX_OCCURRENCES` ich nie obcina, a pamięci zajmują tyle co nic.
/// Kalendarz roczny bez tego pokazywałby święta z dwóch tygodni i pusty listopad.
const HOLIDAY_HORIZON_DAYS: i64 = 366;

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
        "=== taskcerber {VERSION} === boot #{}, powód: {:?}",
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
    // Czytamy to na samym początku: rejestry źródła wybudzenia opisują stan sprzed
    // tego bootu i nie ma powodu, żeby cokolwiek zdążyło je zmienić.
    let wakeup = shutdown::wakeup_source();
    info!("wybudzenie: {wakeup:?}");

    let peripherals = Peripherals::take().context("nie mogę przejąć peryferiów")?;
    let sysloop = EspSystemEventLoop::take().context("nie mogę przejąć pętli zdarzeń")?;
    let nvs_partition = EspDefaultNvsPartition::take().context("nie mogę przejąć partycji NVS")?;

    // --- 0. Zwolnij zatrzaski GPIO z poprzedniego snu --------------------------
    // Musi być przed `Epd::new()` i przed pierwszym `Gt911::open`. Bez tego po każdym
    // wybudzeniu z deep sleepu panel dostaje śmieci zamiast obrazu, a GT911 zostaje
    // przybity do resetu — pełne wyjaśnienie przy `release_pin_holds`.
    shutdown::release_pin_holds();

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

    // Decyzja o oknie dotyku zapada TUTAJ, przed siecią, bo od niej zależy, czy warto
    // w ogóle budzić kontroler dotyku. Zależy wyłącznie od rzeczy, które są już znane:
    // konfiguracji, zasilania, licznika bootów i tego, co nas wybudziło.
    let woke_by_button = wakeup.by_human();
    let boot_count = state.boot_count;

    // Okruszek z poprzedniego cyklu. Jeśli tamten zamilkł w środku, TEN cykl
    // pomija sieć i oddaje panel diagnozie — bo panelu i radia nie wolno trzymać
    // naraz, a pisanie kroków na ekran w trakcie TLS-a powodowało dokładnie tę
    // awarię, którą miało pokazać. Pełne wyjaśnienie: nagłówek `devlogic::boot`.
    let okruszek = store.boot_crumb();
    let diagnoza = okruszek.is_failure();
    if diagnoza {
        warn!(
            "poprzedni cykl zamilkł na {:?} po {} ms, wolny DRAM {} KB — pomijam sieć",
            okruszek.step, okruszek.ms, okruszek.dram_kb
        );
        // Znacznik idzie PRZED malowaniem: gdyby padło samo malowanie, następny
        // cykl ma spróbować sieci, a nie utknąć na tej samej diagnozie w kółko.
        store.mark_boot_step(BootStep::Reported, ms_od_startu(), wolny_dram_kb() as u16);
    }

    // Na diagnozie okno dotyku otwieramy bezwarunkowo — bez niego przycisk
    // „Konfiguracja" byłby rysunkiem, a to on jest zwykle lekarstwem: najczęstsza
    // awaria to `łączenie z WiFi`, czyli najczęściej literówka w haśle.
    let interact = diagnoza
        || wants_interaction(
            config.is_provisioned(),
            power_status.usb_present,
            boot_count,
            woke_by_button,
        );

    // Czytnik dotyku wstaje PRZED siecią, nie po niej.
    //
    // Otwierany dopiero w oknie interaktywnym oznaczał, że przez cały czas pobierania
    // — przy kanale 1,18 MB kilkanaście sekund — nikt nie rozmawiał z GT911.
    // Stuknięcia z tego okresu nie ginęły w oprogramowaniu, tylko w kontrolerze:
    // trzyma on jeden punkt i flagę w rejestrze 0x814E, której nikt nie kasował.
    // A `Gt911::new` zaczyna od twardego resetu, więc samo otwarcie kontrolera
    // kasowało to dotknięcie, które przed chwilą wybudziło urządzenie.
    //
    // Koszt jest znany i zmierzony: wątek to ~4 KB stosu w WEWNĘTRZNYM DRAM-ie plus
    // transakcja I²C co `SAMPLE_MS`. Konkuruje więc z mbedTLS o tę samą pamięć —
    // ale log ze sprzętu pokazuje 190 KB wolnego w chwili pobierania, a sterownik
    // `i2c_master` serializuje dostęp semaforem, więc współdzielenie magistrali
    // z TPS65185 i ekspanderem jest bezpieczne. Gdyby to jednak destabilizowało
    // krok sieciowy, dowód będzie w logu: `krok5: pobieram ... DRAM {} KB`.
    let reader = if interact && TOUCH_BEFORE_NET {
        board::gt911::open(&bus, boot_count <= 1)
            .and_then(|touch| TouchReader::spawn(touch).map_err(|e| warn!("{e:#}")).ok())
    } else {
        None
    };

    // Silniki regex parsera reguł budujemy TERAZ, na osobnym wątku z własnym stosem.
    //
    // To jest naprawa zdiagnozowanego stack overflow. Budowa idzie przez
    // `regex_automata::meta::strategy::new` — ramka 13 632 B, największa w obrazie —
    // i razem z drogą do niej potrzebuje ~27 KB. Wykonana leniwie, przy pierwszym
    // parsowaniu reguły, lądowała na zadaniu `main` w najgłębszym punkcie cyklu:
    // pod nią leżały już ramki `main` (11 584 B) i `fetch_everything` (5 104 B).
    //
    // Przed radiem, żeby te 40 KB nie konkurowało z mbedTLS o wewnętrzny DRAM.
    // Rozmiar MUSI być jawny: `CONFIG_PTHREAD_TASK_STACK_SIZE_DEFAULT` to 8192.
    match std::thread::Builder::new()
        .stack_size(40 * 1024)
        .spawn(icalfeed::warm_up_rrule)
    {
        Ok(h) => {
            let _ = h.join();
            info!("parser reguł rozgrzany · stos {} B", zapas_stosu_b());
        }
        // Bez propagacji: gdyby wątek nie wstał, budowa i tak wydarzy się leniwie —
        // po prostu tam, gdzie boli.
        Err(e) => warn!("nie mogę rozgrzać parsera reguł: {e}"),
    }

    // --- 5. Sieć ---------------------------------------------------------------
    //
    // `Epd` powstaje tu TYLKO wtedy, gdy ślad na panelu jest włączony — bo tylko
    // wtedy jest do czego rysować. Domyślnie nie powstaje i to jest istotne:
    // epdiy trzymane przez czas TLS-a wywraca budżet wewnętrznego DRAM-u.
    // Pełne wyjaśnienie przy [`NET_TRACE_ON_PANEL`].
    let mut epd_early = if NET_TRACE_ON_PANEL {
        Some(Epd::new(&bus).context("nie mogę zainicjalizować panelu")?)
    } else {
        None
    };

    let requested = std::mem::take(&mut state.fetch_requested);
    if requested {
        info!("pobranie na życzenie z poprzedniego cyklu");
    }

    let mut net_state = NetState::Ok;

    // Migawka z poprzedniego cyklu. Wchodzi do modelu OD RAZU, jeszcze przed decyzją
    // o sieci — dzięki temu ekran ma co pokazać nawet wtedy, gdy pobrania nie będzie
    // wcale: bez kabla, przy pominięciu ze względu na świeżość albo przy awarii sieci.
    // Wydarzenia żyły dotąd wyłącznie w RAM-ie, a deep sleep gasi RAM, więc każde
    // wybudzenie musiało pobierać wszystko od nowa albo pokazać pustkę.
    let migawka = store.load_snapshot();
    let migawka_swieza = migawka
        .as_ref()
        .is_some_and(|m| dashboard::snapshot::wciaz_uzyteczna(m, now.date()));
    if migawka.is_some() && !migawka_swieza {
        warn!("migawka dotyczy dni, które już minęły — nie używam jej");
    }

    let (mut events, mut known, mut known_holidays) = match &migawka {
        Some(m) if migawka_swieza => (m.events.clone(), m.known, m.known_holidays),
        _ => (Vec::new(), None, None),
    };
    // CRC treści, która JEST na szkle. Trzymamy je osobno, bo `record_success`
    // nadpisuje `state.last_content_crc` zaraz po udanym pobraniu — porównanie
    // z polem stanu byłoby wtedy porównaniem wartości z samą sobą.
    let painted_crc = state.last_content_crc;
    let mut content_crc = state.last_content_crc;
    let mut fetched = false;

    // Radio podnosimy dopiero wtedy, gdy naprawdę trzeba — i na czas bring-upu
    // wyłącznie na kablu. Patrz [`RADIO_ONLY_ON_USB`].
    let radio_allowed = !RADIO_ONLY_ON_USB || power_status.usb_present;

    // OTA sprawdzamy TYLKO na kablu albo na wyraźne życzenie.
    //
    // Sprawdzenie manifestu to osobny uścisk TLS i osobne pobranie przy każdym
    // wybudzeniu — koszt bez adresata, bo nowa wersja pojawia się raz na kilka dni,
    // a nie co godzinę.
    //
    // Kabel zostaje jako SIATKA BEZPIECZEŃSTWA i to jest jego główna rola: gdyby
    // wydanie okazało się zepsute, podłączenie kabla wystarczy, żeby urządzenie
    // samo sięgnęło po poprawkę. Bez tego jedyną drogą byłby webflasher.
    let ota_dozwolone =
        (power_status.usb_present || requested) && power::may_update(&policy, mode, fuel);

    if diagnoza {
        // Sieć pominięta świadomie — patrz wyżej. Stan zgłaszamy jako nieaktualny,
        // żeby ewentualne późniejsze ekrany nie udawały świeżych danych.
        if state.last_success_unix > 0 {
            net_state = NetState::Stale {
                since: unix_to_local(state.last_success_unix, home_tz).unwrap_or(now),
            };
        } else {
            net_state = NetState::Offline;
        }
    } else if !config.is_provisioned() {
        warn!("urządzenie nieskonfigurowane — pokazuję ekran konfiguracji");
        net_state = NetState::NeedsAuth;
    } else if !radio_allowed {
        warn!(
            "radio wstrzymane: RADIO_ONLY_ON_USB, a USB nieobecne (vbus={}, chrg={})",
            power_status.vbus_stat, power_status.chrg_stat
        );
        if state.last_success_unix > 0 {
            net_state = NetState::Stale {
                since: unix_to_local(state.last_success_unix, home_tz).unwrap_or(now),
            };
        } else {
            net_state = NetState::Offline;
        }
    // Dotknięcie „odśwież" w poprzednim cyklu wymusza pobranie niezależnie od trybu,
    // także w nocy. Flagę zdejmujemy tutaj, a nie po udanym pobraniu: gdyby sieć
    // zawiodła, powtarzanie życzenia sprzed godziny nie jest już tym, o co ktoś prosił.
    // `fetch_is_due` dokłada do warunku ŚWIEŻOŚĆ, której do tej pory nie było.
    // Bez niej wybudzenie dotykiem ściągało kanał od nowa niezależnie od tego, że
    // pobraliśmy go przed chwilą — a to jest te kilkanaście sekund, w których panel
    // jeszcze nie istnieje i dotyku nikt nie czyta. Żądanie użytkownika
    // (`RefreshNow`) omija ten warunek celowo: „odśwież teraz" ma znaczyć teraz.
    } else if requested
        || (policy.should_fetch(mode)
            && policy.fetch_is_due(mode, net::time::now_unix(), state.last_success_unix)
            && !matches!(mode, Mode::Night))
    {
        // Ślad na panelu tylko na kablu — patrz [`NetTrace`].
        let mut trace = (epd_early.is_some() && power_status.usb_present)
            .then(|| NetTrace::begin(rotation, temperature));
        let wynik_sieci = fetch_everything(
            peripherals.modem,
            sysloop,
            nvs_partition,
            &config,
            &hw,
            &mut state,
            &mut store,
            home_tz,
            now,
            ota_dozwolone,
            migawka_swieza,
            epd_early.as_mut().zip(trace.as_mut()),
        );
        // Powrót z tej funkcji — obojętne czy z sukcesem, czy z błędem — znaczy,
        // że cykl PRZEŻYŁ krok sieciowy. Okruszek ma łapać ciche zgony (panika,
        // watchdog, brownout), a nie obsłużone błędy w rodzaju 404 na kanale.
        store.mark_boot_step(BootStep::Done, ms_od_startu(), wolny_dram_kb() as u16);
        match wynik_sieci {
            Ok(out) => {
                // Restart natychmiast: radio jest już wyłączone, a szyny panelu są
                // opuszczone — `Epd` istnieje od kroku 5, ale `present` gasi je po
                // każdym odświeżeniu. Reset przy podniesionych szynach TPS65185
                // potrafi uszkodzić panel, więc kolejność ma znaczenie.
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
                known = out.known;
                known_holidays = out.known_holidays;
                if out.unchanged {
                    // Treść bez zmian: `events` z migawki zostają nietknięte,
                    // a migawki nie przepisujemy — CRC i tak jest to samo.
                    info!("kanał bez zmian — zostaję przy migawce");
                } else {
                    events = out.events;
                    fetched = true;

                    // Zapis PRZED `record_success`, bo ta funkcja nadpisuje
                    // `last_content_crc` — a to z nim porównujemy, żeby nie zapisywać
                    // do flasha kalendarza, który się nie zmienił.
                    let snap = dashboard::Snapshot {
                        events: events.clone(),
                        holidays: swieta_z_wydarzen(&events),
                        known,
                        known_holidays,
                    };
                    store.save_snapshot(&snap, content_crc, state.last_content_crc);
                }

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
    // `Epd` powstaje TUTAJ, nie wcześniej, i to nie jest kwestia porządku. epdiy
    // alokuje część swoich buforów w WEWNĘTRZNYM DRAM-ie, a ten jest ciasny i dzieli
    // go z mbedTLS — dlatego `Epd::new` używa `EPD_LUT_1K` zamiast 64K, żeby zostawić
    // 63 KB handshake'owi. Trzymanie epdiy przy życiu przez czas TLS-a wywraca ten
    // budżet: na sprzęcie objawiło się to resetem przy `pobieram kalendarz główny`,
    // czyli dokładnie w szczycie zapotrzebowania na pamięć.
    let mut epd = match epd_early.take() {
        Some(epd) => epd,
        None => Epd::new(&bus).context("nie mogę zainicjalizować panelu")?,
    };

    // epdiy właśnie przestawiło cały port 1 ekspandera na wyjścia — łącznie z bitem
    // przycisku, którego samo nie używa. Bez tego odczyt przycisku zwraca „wciśnięty"
    // bez końca. Szczegóły: `Pca9535::reclaim_button_input`.
    if let Err(e) = hw.expander.reclaim_button_input() {
        warn!("nie mogę odzyskać bitu przycisku na ekspanderze: {e:#}");
    }
    //
    // Porównujemy z CRC sprzed pobrania, nie z polem stanu — patrz `painted_crc`.
    // Przeciw `state.last_content_crc` warunek był tożsamościowo fałszywy po każdym
    // udanym pobraniu, czyli panel nie odświeżał się już nigdy po pierwszym boocie,
    // a razem z nim nie otwierało się okno dotyku.
    let content_changed = content_crc != painted_crc || !fetched;
    // Wybudzenie przyciskiem zawsze rysuje. Ktoś nacisnął, więc czegoś od urządzenia
    // chce — a za chwilę może chcieć obrócić ekran, co bez świeżej klatki nie ma sensu.
    // Cykl diagnostyczny maluje ZAWSZE: jego jedynym produktem jest ten ekran.
    let needs_paint = diagnoza
        || content_changed
        || state.boot_count <= 1
        || net_state != NetState::Ok
        || woke_by_button;

    // Dotyk NIE zależy od tego, czy akurat malujemy klatkę — zależność idzie
    // w drugą stronę: to dotknięcie powoduje przerysowanie. Mapa obszarów dotykowych
    // powstaje z samego `dashboard::render`, czyli z czystego CPU (0,4–1,4 ms), więc
    // okno da się otworzyć bez podnoszenia szyn panelu. Wcześniej pętla interaktywna
    // siedziała w środku `if needs_paint` i skonfigurowane urządzenie na kablu,
    // z niezmienioną treścią, nie dostawało okna w ogóle.
    // --- Karta tonów: bring-up, nie funkcja produktu ---------------------------
    if BRING_UP_CARD != BringUpCard::None {
        show_bring_up_card(&mut epd, temperature, rotation);
        info!("karta bring-upowa wyrysowana — obraz zostaje na szkle przez cały sen");
        // Ta sama sekwencja zasypiania, co na normalnej ścieżce. Bez niej BOOT
        // przestałby budzić (brak `enable_wakeup`), a magistrala panelu poszłaby
        // w sen niezaizolowana — czyli karta pomiarowa kłamałaby o prądzie
        // spoczynkowym i o tym, czy urządzenie w ogóle da się obudzić.
        shutdown::prepare_for_deep_sleep(&mut epd, &hw, WAKE_ON_TOUCH);
        if let Err(e) = shutdown::enable_wakeup(WAKE_ON_TOUCH) {
            warn!("nie mogę włączyć budzenia: {e:#}");
        }
        return Ok(TEST_CARD_SLEEP_S);
    }

    if needs_paint || interact {
        let mut model = if config.is_provisioned() {
            build_model(
                now,
                events,
                fuel,
                power_status.usb_present,
                net_state,
                known,
                known_holidays,
            )
        } else {
            provisioning_model(now, fuel, power_status.usb_present)
        };

        // Urządzenie budzi się w tym widoku, w którym je zostawiono. Bez tego
        // miesiąc czy rok znikały przy pierwszym przemalowaniu i nie dało się
        // zrobić z nich stałego ekranu ściennego.
        model.view = dashboard::View::from_u8(state.view);

        // Pierwsze rysowanie w tym wybudzeniu MUSI być pełne — `back_fb` epdiy
        // powstał przed chwilą wyzerowany do bieli i nie wie nic o tym, co zostało
        // na szkle. Dopiero kolejne, w oknie interaktywnym, mogą być szybkie.
        //
        // `panel_synced` niesie tę wiedzę dalej: dopóki jest fałszywe, epdiy nie ma
        // prawdziwego punktu odniesienia, więc ani szybkie odświeżenie, ani tym
        // bardziej częściowe (feedback pod palcem) nie dałoby poprawnej różnicy.
        let (canvas, screen, panel_synced) = if diagnoza {
            let fonts = Fonts::embedded();
            let mut cv = Gray8::new(rotation);
            let sc = dashboard::render_diagnosis(
                &dashboard::Diagnosis {
                    step: okruszek.step.label(),
                    hint: okruszek.step.hint(),
                    ms: okruszek.ms,
                    dram_kb: okruszek.dram_kb,
                    firmware: VERSION,
                },
                &fonts,
                &mut cv,
            );
            match present(&mut epd, &cv, &mut state, temperature, Refresh::Full) {
                Ok(()) => (cv, sc, true),
                Err(e) => {
                    error!("nie mogę pokazać diagnozy: {e:#}");
                    (cv, sc, false)
                }
            }
        } else if needs_paint {
            match paint(
                &mut epd,
                &model,
                &mut state,
                temperature,
                rotation,
                Refresh::Full,
            ) {
                Ok((canvas, screen)) => (canvas, screen, true),
                Err(e) => {
                    // Nieudane malowanie nie zwalnia nas z poprawnego zaśnięcia.
                    error!("rysowanie nie powiodło się: {e:#}");
                    let (canvas, screen) = render_frame(&model, rotation);
                    (canvas, screen, false)
                }
            }
        } else {
            info!("treść bez zmian — panel zostaje, otwieram samo okno dotyku");
            let (canvas, screen) = render_frame(&model, rotation);
            (canvas, screen, false)
        };
        let mut canvas = canvas;

        if interact {
            let changed = interactive_loop(
                &mut epd,
                &hw,
                &mut store,
                &mut state,
                &model,
                canvas,
                screen,
                panel_synced,
                temperature,
                rotation,
                if config.is_provisioned() {
                    IDLE_MS
                } else {
                    FRESH_IDLE_MS
                },
                reader,
            );
            if changed {
                // Świeżo wpisana konfiguracja ma zadziałać teraz, a nie za pół godziny.
                info!("konfiguracja zmieniona — pobieram przy najbliższym wybudzeniu");
                state.request_fetch();
            }
        } else if panel_synced {
            // Urządzenie narysowało klatkę i wraca spać bez okna dotyku — znacznik
            // ma się pojawić także tutaj, bo z zewnątrz to jest ten sam stan.
            mark_going_to_sleep(&mut epd, &mut canvas, rotation, temperature);
        }
    } else {
        info!("treść bez zmian i nikogo przy urządzeniu — pomijam odświeżenie panelu");
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

    shutdown::prepare_for_deep_sleep(&mut epd, &hw, WAKE_ON_TOUCH);
    if let Err(e) = shutdown::enable_wakeup(WAKE_ON_TOUCH) {
        warn!("nie mogę włączyć budzenia: {e:#}");
    }

    Ok(sleep_s)
}

/// Wypisuje postęp kroku sieciowego WPROST NA PANEL.
///
/// # Po co, skoro jest log
///
/// Bo panel jest jedynym wyjściem, które zawsze jest. Log wymaga kabla i otwartego
/// monitora, a krok sieciowy to jedyne miejsce, w którym to urządzenie potrafiło
/// stanąć bez śladu: krok 5 idzie PRZED krokiem 6, więc przy zawieszeniu panel
/// zostaje z poprzednią klatką i nie wiadomo nawet, czy urządzenie wstało.
///
/// Ostatni wypisany krok jest wtedy jedyną informacją o tym, gdzie stanęło.
///
/// # Dlaczego to jest ograniczone do USB
///
/// Nagłówek tego pliku zabrania trzymać radio i szyny panelu włączone naraz: panel
/// ciągnie ~115 mA, szczyt nadajnika ~340 mA, a razem przez LDO na zużytym ogniwie
/// to brownout — i reset w trakcie odświeżania z podniesionymi szynami TPS65185
/// jest jedyną naprawdę szkodliwą awarią tej płytki.
///
/// Ta reguła chroni OGNIWO. Na kablu prądu zwykle wystarcza, a radio i tak wstaje
/// teraz wyłącznie przy USB (patrz [`RADIO_ONLY_ON_USB`]), więc podczas bring-upu
/// można rysować w trakcie. Na baterii ten ślad się NIE WŁĄCZA i to nie jest opcja.
///
/// # Ale i tak trzeba go trzymać tanio
///
/// Pierwsza wersja czyściła panel PEŁNYM odświeżeniem — 35 przelotów przez wszystkie
/// bramki, każdy piksel napędzany — i robiła to tuż przed podniesieniem radia. Na
/// sprzęcie objawiło się to migotaniem czerni i resetami w losowych miejscach kroku
/// sieciowego, czyli dokładnie tym, przed czym nagłówek ostrzega. Narzędzie
/// diagnostyczne zaburzało to, co mierzy.
///
/// Teraz nie ma ani czyszczenia, ani pełnego odświeżenia: każdy krok to jeden wiersz
/// i jedno odświeżenie CZĘŚCIOWE. `back_fb` epdiy jest po wybudzeniu biały, więc
/// różnicą są wyłącznie czarne piksele napisu — reszta panelu nie dostaje impulsu.
/// Kroki nadpisują to, co akurat jest na szkle, i to jest świadoma cena za to, żeby
/// pomiar nie zmieniał wyniku.
struct NetTrace {
    canvas: Gray8,
    fonts: Fonts<'static>,
    rotation: Rotation,
    temperature_c: i32,
    band: dashboard::Rect,
    line: i32,
    cleared: bool,
    started: std::time::Instant,
}

impl NetTrace {
    /// Ile kroków mieści się w pasie, zanim zacznie się od góry.
    const LINES: i32 = 8;
    /// Wysokość jednego wiersza. Dobrana pod `TEXT_HEAD`, bo to ma być czytelne
    /// z drugiego końca biurka, a nie z nosem przy szkle.
    const LINE_H: i32 = 46;

    fn begin(rotation: Rotation, temperature_c: i32) -> Self {
        let canvas = Gray8::new(rotation);
        let w = canvas.width() as i32;
        let h = canvas.height() as i32;
        let band_h = Self::LINES * Self::LINE_H;

        Self {
            fonts: Fonts::embedded(),
            canvas,
            rotation,
            temperature_c,
            band: dashboard::Rect::new(0, (h - band_h) / 2, w, band_h),
            line: 0,
            cleared: false,
            started: std::time::Instant::now(),
        }
    }

    /// Czyści sam pas — raz, przy pierwszym kroku.
    ///
    /// epdiy napędza wyłącznie piksele różne od `back_fb`, a ten po wybudzeniu jest
    /// biały. Samo wypełnienie pasa bielą nie skasuje więc niczego: różnicy nie ma,
    /// impulsu nie ma, stara treść zostaje. Trzeba przejść przez czerń.
    ///
    /// Kosztuje to dwa odświeżenia częściowe, czyli **dziesięć faz** — wobec
    /// trzydziestu pięciu, które kosztowało pełne odświeżenie w pierwszej wersji.
    /// I dotyczy wyłącznie pasa, nie całego ekranu. Tyle wolno wydać na to, żeby
    /// ślad dało się przeczytać.
    fn clear_band(&mut self, epd: &mut Epd) {
        let area = self.rotation.canvas_rect_to_panel(self.band);

        self.canvas.fill_rect(self.band, dashboard::canvas::BLACK);
        if let Err(e) = epd.present_area(&self.canvas, area, self.temperature_c) {
            warn!("nie mogę zaczernić pasa śladu: {e:#}");
            return;
        }
        self.canvas.fill_rect(self.band, dashboard::canvas::WHITE);
        if let Err(e) = epd.present_area(&self.canvas, area, self.temperature_c) {
            warn!("nie mogę wyczyścić pasa śladu: {e:#}");
        }
    }

    /// Dopisuje krok i wypycha SAM jego wiersz.
    fn step(&mut self, epd: &mut Epd, co: &str) {
        let ms = self.started.elapsed().as_millis();
        info!("krok5[{ms} ms]: {co}");

        if !self.cleared {
            self.cleared = true;
            self.clear_band(epd);
        }

        let y = self.band.y + (self.line % Self::LINES) * Self::LINE_H;
        let row = dashboard::Rect::new(self.band.x, y, self.band.w, Self::LINE_H);
        self.line += 1;

        dashboard::layout::draw_net_step(&self.fonts, &mut self.canvas, row, co, ms);
        let area = self.rotation.canvas_rect_to_panel(row);
        if let Err(e) = epd.present_area(&self.canvas, area, self.temperature_c) {
            warn!("nie mogę wypisać kroku na panel: {e:#}");
        }
    }
}

/// Wynik fazy sieciowej.
struct Fetched {
    events: Vec<dashboard::model::CalEvent>,
    crc: u32,
    /// O które dni zapytał kanał z treścią. `None` = nie udało się go pobrać.
    known: Option<(NaiveDate, NaiveDate)>,
    /// O które dni zapytał kanał świąt — osobno, bo ma inny horyzont.
    known_holidays: Option<(NaiveDate, NaiveDate)>,
    /// Treść identyczna z poprzednią — `events` jest puste i NIE WOLNO nim
    /// nadpisać tego, co przyszło z migawki.
    unchanged: bool,
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
    // `migawka_uzyteczna`: czy w NVS leży migawka, na którą można się wycofać.
    // Warunkuje pominięcie parsowania przy niezmienionej treści — bez niej byłoby
    // to pokazanie pustego ekranu zamiast oszczędności.
    migawka_uzyteczna: bool,
    // Panel i ślad — obecne tylko przy włączonym `NET_TRACE_ON_PANEL`.
    slad: Option<(&mut Epd, &mut NetTrace)>,
) -> Result<Fetched> {
    let ssid = config.ssid.as_deref().unwrap_or_default();
    let password = config.password.as_deref().unwrap_or_default();

    let mut slad = slad;
    // Jeden zapis kroku: do logu zawsze, na panel gdy ślad jest włączony.
    macro_rules! krok {
        ($co:expr) => {
            match slad.as_mut() {
                Some((epd, t)) => t.step(epd, $co),
                None => info!("krok5: {}", $co),
            }
        };
    }

    // Okruszek zapisujemy TUŻ PRZED etapem, nie po nim: ma przeżyć to, co się
    // w tym etapie stanie, a właśnie te etapy potrafią zabrać ze sobą cały układ.
    macro_rules! okruszek {
        ($etap:expr) => {
            store.mark_boot_step($etap, ms_od_startu(), wolny_dram_kb() as u16);
        };
    }

    okruszek!(BootStep::RadioUp);
    krok!(&format!("podnoszę radio · DRAM {} KB", wolny_dram_kb()));
    let wifi = net::wifi::Wifi::connect(modem, sysloop, nvs, ssid, password, state)?;
    krok!(&format!("radio gotowe · DRAM {} KB", wolny_dram_kb()));
    if let Some(rssi) = wifi.rssi() {
        info!("RSSI: {rssi} dBm");
    }

    // Czas synchronizujemy RAZ NA DOBĘ, a nie przy każdym wybudzeniu.
    //
    // To jest kalendarz, nie synchronizator kamer. PCF8563 dryfuje rzędu sekund na
    // tydzień; nawet pół roku bez korekty nie przesunie zadania przypisanego do dnia
    // ani powiadomienia z dokładnością do pół godziny. A czekanie kosztuje do
    // `SNTP_TIMEOUT` z PODNIESIONĄ ANTENĄ — w logach ze sprzętu dwa razy zeszło
    // pełne dziesięć sekund i skończyło się na „zostaję przy czasie z RTC".
    //
    // Wyjątki, przy których synchronizujemy mimo wszystko:
    //  * flaga VL zegara — stracił zasilanie, więc jego czas jest śmieciem;
    //  * brak zapisanej synchronizacji — świeże urządzenie nie wie, która godzina.
    let zegar_zgubiony = hw.rtc.voltage_low().unwrap_or(true);
    let od_ostatniej = now_unix_lub_zero() - store.last_sntp_unix();
    let trzeba_sntp =
        zegar_zgubiony || store.last_sntp_unix() <= 0 || od_ostatniej >= SNTP_INTERVAL_S;

    if trzeba_sntp {
        okruszek!(BootStep::Sntp);
        krok!("SNTP");
        match net::time::sync_sntp(&hw.rtc, home_tz) {
            Ok(zrodlo) => {
                info!("źródło czasu po synchronizacji: {zrodlo:?}");
                store.set_last_sntp_unix(net::time::now_unix());
                krok!("czas ustalony");
            }
            Err(e) => warn!("SNTP zawiódł: {e:#}"),
        }
    } else {
        info!(
            "SNTP pominięty — zsynchronizowany {} h temu",
            od_ostatniej / 3600
        );
    }

    let now = net::time::now_local(home_tz).unwrap_or(now);
    let from = now.date().and_hms_opt(0, 0, 0).unwrap_or(now);

    let mut events = Vec::new();
    let mut crc = 0u32;

    let mut sources: Vec<IcsSource> = Vec::new();
    if let Some(url) = &config.ics_url {
        sources.push(IcsSource::new(
            url,
            home_tz,
            SourceTag::Primary,
            "kalendarz główny",
            HORIZON_DAYS,
        ));
    }
    // Drugi kanał jest kanałem ŚWIĄT: tag `Holiday` i roczny horyzont. Jego opis
    // w konfiguracji od początku brzmiał „np. święta albo kalendarz współdzielony",
    // a święta są jedyną treścią, która ma sens na całym roku i jednocześnie nic
    // nie kosztuje. Kalendarz roczny czyta wyłącznie ten tag.
    if let Some(url) = &config.ics_url_secondary {
        sources.push(IcsSource::new(
            url,
            home_tz,
            SourceTag::Holiday,
            "kalendarz świąt",
            HOLIDAY_HORIZON_DAYS,
        ));
    }

    let mut any_ok = false;
    let mut last_error = None;
    // Surowa treść czeka tu na PARSOWANIE PO ZGASZENIU RADIA. Bufory leżą w PSRAM-ie,
    // bo przekraczają `SPIRAM_MALLOC_ALWAYSINTERNAL`.
    let mut pobrane: Vec<(usize, NaiveDateTime, crate::source::Downloaded)> = Vec::new();
    let mut known = None;
    let mut known_holidays = None;
    for (i, src) in sources.iter().enumerate() {
        okruszek!(if i == 0 {
            BootStep::FetchPrimary
        } else {
            BootStep::FetchSecondary
        });
        krok!(&format!(
            "pobieram {} · DRAM {} KB · stos {} B",
            src.name(),
            wolny_dram_kb(),
            zapas_stosu_b()
        ));
        // Okno liczone POD ŹRÓDŁO — to jest cały sens `horizon_days`.
        let to = from + ChronoDuration::days(src.horizon_days());
        match src.download() {
            Ok(dl) => {
                crc ^= dl.content_crc;
                pobrane.push((i, to, dl));
                any_ok = true;
                // O które dni to źródło faktycznie zapytało. Bez tego widoki
                // rastrują całą siatkę jako „nie wiem" — a `Model::known`
                // NIE BYŁO dotąd w ogóle ustawiane, więc tak właśnie wyglądało
                // to na urządzeniu.
                let zakres = (from.date(), (to - ChronoDuration::days(1)).date());
                if src.horizon_days() > HORIZON_DAYS {
                    known_holidays = Some(zakres);
                } else {
                    known = Some(zakres);
                }
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
    krok!(&format!(
        "pobrane · DRAM {} KB · stos {} B",
        wolny_dram_kb(),
        zapas_stosu_b()
    ));
    okruszek!(BootStep::Ota);
    let mut ota_installed = false;
    if ota_allowed {
        // Własny adres z konfiguracji ma pierwszeństwo; brak wpisu znaczy
        // „bierz z oficjalnych wydań", a nie „nie aktualizuj się".
        let manifest = config.ota_url.as_deref().unwrap_or(DEFAULT_OTA_URL);
        if config.ota_url.is_none() {
            info!("OTA: brak adresu w NVS, biorę domyślny {DEFAULT_OTA_URL}");
        }
        match net::ota::check_and_apply(manifest, VERSION, store) {
            Ok(net::ota::Outcome::Installed { version }) => {
                info!("OTA: wgrana wersja {version}, restart po wyłączeniu radia");
                ota_installed = true;
            }
            Ok(net::ota::Outcome::UpToDate) => {}
            Ok(net::ota::Outcome::Skipped(reason)) => info!("OTA pominięte: {reason}"),
            Err(e) => warn!("OTA nie powiodło się: {e:#}"),
        }
    }

    // Radio w dół ZANIM dotkniemy panelu.
    okruszek!(BootStep::RadioDown);
    wifi.shutdown();

    if !any_ok {
        return Err(last_error.unwrap_or_else(|| anyhow::anyhow!("brak skonfigurowanych źródeł")));
    }

    // Treść bez zmian — parsowania nie ma po co robić W OGÓLE.
    //
    // CRC znamy teraz PRZED parsowaniem, bo pobranie jest od niego oddzielone. Wcześniej
    // liczyliśmy je w locie ze strumienia, czyli dowiadywaliśmy się o braku zmian dopiero
    // po zapłaceniu za mielenie 1,18 MB. Warunek `migawka_uzyteczna` jest konieczny:
    // bez niej pominięcie parsowania dałoby pusty ekran zamiast oszczędności.
    let bez_zmian = crc != 0 && crc == state.last_content_crc && migawka_uzyteczna;
    if bez_zmian {
        krok!("bez zmian — pomijam parsowanie");
        return Ok(Fetched {
            events: Vec::new(),
            crc,
            known,
            known_holidays,
            unchanged: true,
            ota_installed,
        });
    }

    krok!(&format!(
        "parsuję bez radia · DRAM {} KB · stos {} B",
        wolny_dram_kb(),
        zapas_stosu_b()
    ));
    for (i, to, dl) in &pobrane {
        match sources[*i].parse(&dl.body, from, *to) {
            Ok(mut e) => events.append(&mut e),
            Err(e) => warn!("źródło `{}` nie sparsowało się: {e:#}", sources[*i].name()),
        }
    }
    krok!(&format!(
        "sparsowane · DRAM {} KB · stos {} B",
        wolny_dram_kb(),
        zapas_stosu_b()
    ));

    events.sort_by_key(|e| e.start);
    Ok(Fetched {
        events,
        crc,
        known,
        known_holidays,
        unchanged: false,
        ota_installed,
    })
}

#[allow(clippy::too_many_arguments)]
/// Wyciąga z listy wydarzeń same daty świąteczne, posortowane i bez powtórzeń.
///
/// Wydzielone, bo liczy to zarówno model do narysowania, jak i migawka zapisywana
/// do NVS — a rozjazd między nimi znaczyłby, że po wybudzeniu z migawki znikają
/// święta, które przed uśpieniem były.
fn swieta_z_wydarzen(events: &[dashboard::model::CalEvent]) -> Vec<NaiveDate> {
    let mut out: Vec<NaiveDate> = events
        .iter()
        .filter(|e| e.source == SourceTag::Holiday)
        .map(|e| e.start.date())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn build_model(
    now: NaiveDateTime,
    events: Vec<dashboard::model::CalEvent>,
    fuel: board::bq27220::Fuel,
    charging: bool,
    net: NetState,
    known: Option<(NaiveDate, NaiveDate)>,
    known_holidays: Option<(NaiveDate, NaiveDate)>,
) -> Model {
    let mut model = Model::empty(now);
    model.firmware = format!("taskcerber {VERSION}");
    model.battery = Battery {
        percent: fuel.percent,
        millivolts: fuel.millivolts,
        charging,
    };
    model.net = net;

    // Rozdział jest wymuszony różnymi horyzontami. Kanał świąt sięga roku, kanał
    // z treścią dwóch tygodni — gdyby wszystko poszło do `days`, agenda w sierpniu
    // listowałaby 25 grudnia, a miesiąc rysowałby pasek w kratce oznaczonej rastrem
    // „nie pytałem o ten dzień".
    model.holidays = swieta_z_wydarzen(&events);

    // Do `days` tylko to, co mieści się w horyzoncie TREŚCI. Bliskie święta
    // przechodzą przez to sito i pojawiają się w agendzie jak każde inne
    // wydarzenie całodniowe — i o to chodzi.
    let koniec = now.date() + ChronoDuration::days(HORIZON_DAYS);
    let bliskie: Vec<_> = events
        .into_iter()
        .filter(|e| e.start.date() < koniec)
        .collect();
    model.days = group_by_day(bliskie);
    // Te dwa zakresy to jedyne, co odróżnia „wolny dzień" od „nie pytałem o ten
    // dzień". Do tej pory NIE BYŁY ustawiane w ogóle, więc widok miesięczny
    // rastrował na urządzeniu całą siatkę jako niewiadomą.
    model.known = known;
    model.known_holidays = known_holidays;
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
) -> Result<(Gray8, dashboard::Screen)> {
    let (canvas, screen) = render_frame(model, rotation);
    present(epd, &canvas, state, temperature_c, mode)?;
    Ok((canvas, screen))
}

/// Rysuje klatkę do pamięci i **nie dotyka panelu**.
///
/// Rozdzielenie tych dwóch rzeczy jest tym, co pozwala otworzyć okno dotyku bez
/// odświeżania ekranu: mapa obszarów dotykowych jest produktem ubocznym renderowania,
/// a renderowanie to czysty CPU. Panel kosztuje ~115 mA i sekundę, render — ułamek
/// milisekunda i nic.
///
/// Płótno wraca razem z mapą, bo feedback pod palcem odrysowuje jego fragment.
fn render_frame(model: &Model, rotation: Rotation) -> (Gray8, dashboard::Screen) {
    let fonts = Fonts::embedded();
    let mut canvas = Gray8::new(rotation);

    let started = std::time::Instant::now();
    let screen = dashboard::render(model, &fonts, &mut canvas);
    info!("render: {} ms", started.elapsed().as_millis());

    (canvas, screen)
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
fn provisioning_model(now: NaiveDateTime, fuel: board::bq27220::Fuel, charging: bool) -> Model {
    let mut model = Model::empty(now);
    model.firmware = format!("taskcerber {VERSION}");
    // Stan ogniwa MUSI tu być, choć ekran jest „pusty". Wcześniej go nie było i przez
    // to wskaźnik baterii nie wskazywał niczego przez cały bring-up: `Model::empty`
    // daje `percent: None`, więc rysowała się sama ramka. Dane z licznika są w tym
    // momencie odczytane i leżą u wołającego — po prostu nie trafiały do modelu.
    //
    // Na tym ekranie ta informacja jest POTRZEBNIEJSZA niż na agendzie: urządzenie
    // jeszcze nie umie się nic pobrać, więc jedyne, co da się z niego wyczytać, to
    // czy żyje i czy się ładuje.
    model.battery = Battery {
        percent: fuel.percent,
        millivolts: fuel.millivolts,
        charging,
    };
    model.net = NetState::NeedsAuth;
    model.tiles = vec![
        dashboard::model::Tile::new("krok 1", "Skonfiguruj"),
        dashboard::model::Tile::new("krok 2", "wpisz sieć"),
        dashboard::model::Tile::new("krok 3", "wpisz adres iCal"),
    ];
    model
}

fn unix_to_local(unix: i64, tz: chrono_tz::Tz) -> Option<NaiveDateTime> {
    use chrono::TimeZone;
    tz.timestamp_opt(unix, 0).single().map(|d| d.naive_local())
}

/// Czy wolno podnieść radio wyłącznie przy podłączonym USB.
///
/// # Po co, i dlaczego akurat teraz
///
/// Rzecz na czas bring-upu, jak `SLEEP_MARKER`. Ścieżka sieciowa — WiFi, SNTP,
/// HTTPS, parser iCal — **nigdy nie chodziła na sprzęcie**, a pierwsza próba
/// zakończyła się tym, że urządzenie przestało dochodzić do ekranu.
///
/// Kluczowa obserwacja: dopóki brakowało adresu iCal, `Config::is_provisioned`
/// zwracało fałsz i radio NIE WSTAWAŁO w ogóle. Śmierć zbiega się więc dokładnie
/// z pierwszym w życiu tego urządzenia użyciem nadajnika — a nagłówek tego pliku
/// ostrzega przed tym wprost: szczyt nadajnika to ~340 mA i na zużytym ogniwie
/// przez LDO robi z tego brownout, czyli reset bez śladu na ekranie.
///
/// Ta stała odcina tę zmienną. Na kablu brownoutu być nie może, więc jeśli przy
/// `true` ścieżka sieciowa przechodzi, przyczyną było zasilanie; jeśli nadal pada,
/// przyczyna jest w kodzie i log wskaże gdzie. Bez tego rozdzielenia każdy kolejny
/// pomiar miesza dwie rzeczy naraz.
///
/// Poza bring-upem to jest też sensowna reguła sama w sobie: polityka OTA już teraz
/// traktuje nieznany stan ogniwa jak za mało energii (`Policy::should_ota`), a stan
/// ogniwa na tym egzemplarzu JEST nieznany — BQ27220 nie był nigdy uruchomiony
/// i `battery_percent` bywa `None`, co polityka trybu przepuszcza jak pełną baterię.
/// # Dlaczego już `false`
///
/// Oba powody, dla których ta stała stała na `true`, upadły na dowodach:
///
/// * **„Nie wiadomo, czy pada od zasilania, czy od kodu".** Wiadomo: od kodu.
///   Awaria była powtarzalnym `LoadProhibited` w mbedTLS, wywołanym przez
///   `CONFIG_MBEDTLS_DYNAMIC_BUFFER` na ścieżce TLS 1.3. Po wyłączeniu tej opcji
///   pobranie przechodzi. Brownout nie miał z tym nic wspólnego.
/// * **„Stan ogniwa jest nieznany, bo BQ27220 nigdy nie chodził".** Chodzi. Log ze
///   sprzętu podaje procent, napięcie, prąd i temperaturę, a `Policy::mode` schodzi
///   przy niskim stanie do `Frugal`/`Survival`/`Hold` — i `should_fetch(Hold)` jest
///   fałszem, więc ochrona przed nadawaniem na wyczerpanym ogniwie już działa.
///
/// Zostaje ryzyko szczytu nadajnika (~340 mA) na zużytym ogniwie, ale to jest ryzyko
/// normalnej pracy tego urządzenia, a nie niewiadoma bring-upu — i pilnuje go polityka
/// trybu, a nie ta stała.
const RADIO_ONLY_ON_USB: bool = false;

/// Czy budzić kontroler dotyku PRZED krokiem sieciowym.
///
/// Włączone rozwiązuje realny problem: przy kanale 1,18 MB przez kilkanaście sekund
/// nikt nie rozmawia z GT911, więc stuknięcia z tego okresu giną w kontrolerze,
/// a twardy reset przy otwieraniu kasuje to dotknięcie, które wybudziło urządzenie.
///
/// **Domyślnie WYŁĄCZONE, i to jest cofnięcie po zgłoszeniu ze sprzętu.** Zaraz po
/// włączeniu tej ścieżki cykl zaczął umierać na `RadioUp` i `FetchPrimary` — czyli
/// dokładnie tam, gdzie wątek dotyku wchodzi w drogę: ~4 KB stosu w wewnętrznym
/// DRAM-ie obok mbedTLS i transakcja I²C co `SAMPLE_MS` w trakcie uścisku TLS.
/// Korelacja nie jest dowodem, ale urządzenie ma działać, dopóki dowodu nie ma.
///
/// Włącz z powrotem, gdy log ze sprzętu pokaże, że krok sieciowy przechodzi przy
/// `true` — decyduje linia `krok5: pobieram ... DRAM {} KB` i to, czy po niej
/// pojawia się `kanał ... : N wydarzeń`.
const TOUCH_BEFORE_NET: bool = false;

/// Czy wypisywać postęp kroku sieciowego wprost na panel.
///
/// # Domyślnie WYŁĄCZONE, i to nie z ostrożności
///
/// Żeby rysować w trakcie kroku sieciowego, `Epd` musi istnieć — a epdiy alokuje
/// część buforów w **wewnętrznym DRAM-ie**, tym samym, który jest potrzebny
/// mbedTLS-owi na handshake. Cały projekt jest zbudowany tak, żeby te dwie rzeczy
/// nigdy nie istniały jednocześnie: `Epd::new` bierze `EPD_LUT_1K` zamiast 64K
/// właśnie po to, żeby zostawić 63 KB buforom TLS-a, a `Epd` powstaje dopiero
/// w kroku 6, po wyłączeniu radia.
///
/// Włączenie tej stałej przenosi `Epd::new` przed krok sieciowy i ten budżet
/// wywraca. Na sprzęcie objawiło się to **resetem przy `pobieram kalendarz główny`**,
/// czyli dokładnie w szczycie zapotrzebowania na pamięć — po tym, jak WiFi i SNTP
/// przechodziły bez problemu.
///
/// Ślad zrobił swoje: pokazał, gdzie kończy się krok sieciowy. Zostaje w kodzie,
/// bo jest jedynym narzędziem działającym bez kabla i bez przeglądarki, ale włączać
/// go wolno **wyłącznie ze świadomością, że sam może być przyczyną awarii**, której
/// się szuka. Przy `false` te same kroki idą do logu.
const NET_TRACE_ON_PANEL: bool = false;

/// Czy urządzenie ma zostawiać kontroler dotyku przy życiu na czas snu, żeby
/// dotknięcie mogło je obudzić.
///
/// Kosztuje prąd — GT911 skanuje dalej — i to jest świadoma wymiana: urządzenie,
/// którego nie da się obudzić dotknięciem ekranu, jest w użyciu nieodróżnialne
/// od zepsutego. Wyłączenie tej stałej wraca do wybudzania samym BOOT-em.
const WAKE_ON_TOUCH: bool = true;

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

/// Czytnik dotyku chodzący we WŁASNYM wątku.
///
/// # Dlaczego to musi być osobny wątek
///
/// Wypchnięcie klatki na panel blokuje wołającego na cały przebieg fali — na tym
/// panelu ~0,3 s dla `MODE_DU`. Dopóki odczyt GT911 siedział w tej samej pętli,
/// przez ten czas **nikt nie odpytywał kontrolera**, a GT911 trzyma w rejestrze
/// dokładnie jeden punkt: każde naciśnięcie poza ostatnim przepadało bezpowrotnie.
/// Żadne kolejkowanie po stronie rysowania tego nie naprawi, bo kolejkować nie ma
/// czego — zdarzenia giną w kontrolerze, zanim ktokolwiek je zobaczy.
///
/// Wątek czyta więc niezależnie od rysowania i wrzuca zbocza do kanału. Klawisze
/// naciśnięte w trakcie odświeżania czekają w kolejce i wchodzą do modelu, gdy tylko
/// pętla główna wróci — na ekranie pojawiają się wszystkie naraz.
///
/// # Współdzielenie magistrali
///
/// Wątek gada z GT911 po tej samej magistrali I²C, po której epdiy obsługuje
/// TPS65185 i ekspander. Jest to bezpieczne, bo sterownik `i2c_master` serializuje
/// transakcje semaforem `bus_lock_mux`.
enum Touch {
    /// Nic nowego w kolejce.
    Idle,
    /// Naciśnięcie, we współrzędnych panelu.
    Point(board::gt911::TouchPoint),
    /// Wątek się poddał — kontroler przestał odpowiadać.
    Dead,
}

struct TouchReader {
    rx: std::sync::mpsc::Receiver<board::gt911::TouchPoint>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl TouchReader {
    /// Zabiera kontroler na własność i uruchamia wątek.
    fn spawn(touch: board::gt911::Gt911) -> Result<Self> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc};

        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();

        let join = std::thread::Builder::new()
            .stack_size(4096)
            .spawn(move || {
                let mut finger = FingerEdge::default();
                let mut errors = 0u8;
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(TOUCH_POLL_MS));
                    match finger.press(&touch) {
                        Ok(Some(point)) => {
                            errors = 0;
                            if tx.send(point).is_err() {
                                return;
                            }
                        }
                        Ok(None) => errors = 0,
                        Err(e) => {
                            errors += 1;
                            if errors >= TOUCH_ERRORS_BEFORE_GIVING_UP {
                                warn!("dotyk nie odpowiada, zamykam czytnik: {e:#}");
                                return;
                            }
                        }
                    }
                }
            })
            .context("nie mogę uruchomić wątku dotyku")?;

        Ok(Self {
            rx,
            stop,
            join: Some(join),
        })
    }

    /// Kolejne zdarzenie z kolejki, bez czekania.
    fn poll(&self) -> Touch {
        match self.rx.try_recv() {
            Ok(point) => Touch::Point(point),
            Err(std::sync::mpsc::TryRecvError::Empty) => Touch::Idle,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Touch::Dead,
        }
    }
}

impl Drop for TouchReader {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Odstęp odpytywania GT911 w wątku dotyku.
///
/// Kontroler skanuje szkło co 5–15 ms, więc częściej nie ma sensu. To jest jedyne
/// miejsce, gdzie ten odstęp cokolwiek znaczy — pętla rysująca nie czeka już na dotyk.
const TOUCH_POLL_MS: u64 = 8;

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
    hw: &Board,
    store: &mut Store,
    state: &mut RtcState,
    model: &Model,
    mut canvas: Gray8,
    mut screen: dashboard::Screen,
    mut panel_synced: bool,
    temperature_c: i32,
    mut rotation: Rotation,
    window_ms: u64,
    // Czytnik przychodzi Z ZEWNĄTRZ, otwarty przed krokiem sieciowym.
    //
    // Otwierany tutaj oznaczał, że przez cały czas pobierania — a przy kanale 1,18 MB
    // to kilkanaście sekund — nikt nie rozmawiał z GT911. Stuknięcia z tego okresu
    // nie ginęły w oprogramowaniu, tylko w kontrolerze: trzyma on jeden punkt
    // i flagę w rejestrze 0x814E, której nikt nie kasował.
    //
    // Gorzej: `Gt911::new` zaczyna od twardego resetu, więc otwarcie kontrolera
    // KASOWAŁO dotknięcie, które przed chwilą wybudziło urządzenie. Stąd skarga
    // „ma w dupie dotyk, w szczególności ten co go wybudził".
    reader: Option<TouchReader>,
) -> bool {
    use std::time::{Duration, Instant};
    let mut custom = Button::new(hw.expander.button_pressed().unwrap_or(false));
    let mut model = model.clone();

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
            (canvas, screen) = repaint(
                epd,
                &model,
                state,
                temperature_c,
                rotation,
                Refresh::Full,
                &mut panel_synced,
                (canvas, screen),
            );
            // Liczba stron zależy od orientacji: te same wydarzenia dają 5 stron
            // w pionie i 12 w poziomie. Bez tego klamrowania `model.page` zostaje
            // poza zakresem i kolejne stuknięcia „wstecz" nic nie robią, płacąc
            // za każdym razem pełnym odświeżeniem.
            model.page = model.page.min(screen.pages.saturating_sub(1));
            deadline = Instant::now() + Duration::from_millis(window_ms);
            continue;
        }

        let Some(reader) = reader.as_ref() else {
            continue;
        };

        let point = match reader.poll() {
            Touch::Point(point) => point,
            Touch::Idle => continue,
            Touch::Dead => {
                warn!("dotyk nie odpowiada, zamykam okno");
                break;
            }
        };

        // Dotyk przychodzi w układzie PANELU, obszary są w układzie płótna.
        //
        // Obie pary współrzędnych idą do logu i to jest celowe: dopóki orientacja osi
        // GT911 jest założeniem, a nie pomiarem, dotknięcie czterech rogów i odczytanie
        // tych linii jest jedynym sposobem, żeby ją ustawić. Patrz `SWAP_XY` w
        // `board::gt911`.
        let (x, y) = rotation.panel_to_canvas(point.x, point.y);
        let Some(region) = screen.hit_region(x, y).copied() else {
            info!(
                "dotyk: panel ({}, {}) -> płótno ({x}, {y}), brak obszaru",
                point.x, point.y
            );
            continue;
        };
        let action = region.action;
        info!(
            "dotyk: panel ({}, {}) -> płótno ({x}, {y}) -> {action:?}",
            point.x, point.y
        );
        deadline = Instant::now() + Duration::from_millis(window_ms);

        // Mignięcie pod palcem TYLKO tam, gdzie akcja sama z siebie nie da odpowiedzi.
        //
        // Każde wypchnięcie na panel kosztuje pełny przebieg DU niezależnie od
        // wielkości obszaru — epdiy taktuje wszystkie bramki tak czy owak, patrz
        // `Epd::present_areas`. Miganie przed akcją, która i tak przemaluje ekran,
        // podwaja więc czas reakcji zamiast go skrócić: przewrócenie strony trwałoby
        // dwa przebiegi zamiast jednego.
        //
        // Zostaje tam, gdzie naprawdę jest potrzebne: `RefreshNow` nie zmienia na
        // ekranie nic, a wejście w konfigurację trwa ponad sekundę.
        //
        // Mignięcie jest poprawne także wtedy, gdy `panel_synced` jest fałszywe,
        // i to nie przypadkiem: `back_fb` epdiy jest wtedy biały, więc różnica na tym
        // prostokącie to „wszystko, co nie jest bielą" — a my zalewamy go czernią,
        // czyli każdy piksel obszaru dostaje impuls i wychodzi czarny niezależnie od
        // tego, co tam było.
        let slow_or_silent = matches!(action, Action::OpenSetup | Action::RefreshNow);
        let flashed = slow_or_silent
            && flash_region(
                epd,
                &mut canvas,
                region.rect,
                region.visual,
                rotation,
                temperature_c,
            );
        let mut repainted = false;

        match action {
            Action::OpenSetup => {
                if setup_screen(epd, reader, store, state, temperature_c, rotation) {
                    // Po zapisie nie ma na co czekać — prawdziwa treść wymaga sięgnięcia
                    // po sieć, czyli osobnego wybudzenia. Ale WYJŚCIE BEZ SŁOWA było
                    // błędem: na szkle zostawała klawiatura, bo `mark_going_to_sleep`
                    // wypycha sam kwadracik w rogu. Naciśnięcie „zapisz" wyglądało
                    // dokładnie tak samo jak brak reakcji.
                    let fonts = Fonts::embedded();
                    let mut done = Gray8::new(rotation);
                    dashboard::render_saved(&fonts, &mut done);
                    if let Err(e) = epd.present(&done, Refresh::Full, temperature_c) {
                        error!("nie mogę pokazać potwierdzenia zapisu: {e:#}");
                    }
                    canvas = done;
                    mark_going_to_sleep(epd, &mut canvas, rotation, temperature_c);
                    return true;
                }
                // Ekran konfiguracji ma własne, DŁUŻSZE okno bezczynności (90 s), więc
                // po wyjściu z niego termin okna agendy jest z definicji przeterminowany
                // — pętla kończyłaby się natychmiast po przerysowaniu i urządzenie
                // zasypiałoby w chwili, w której użytkownik dopiero zobaczył ekran
                // główny. Wyglądało to dokładnie jak martwy dotyk.
                deadline = Instant::now() + Duration::from_millis(window_ms);
                panel_synced = true;
                (canvas, screen) = repaint(
                    epd,
                    &model,
                    state,
                    temperature_c,
                    rotation,
                    Refresh::Full,
                    &mut panel_synced,
                    (canvas, screen),
                );
                // Liczba stron zależy od orientacji: te same wydarzenia dają 5 stron
                // w pionie i 12 w poziomie. Bez tego klamrowania `model.page` zostaje
                // poza zakresem i kolejne stuknięcia „wstecz" nic nie robią, płacąc
                // za każdym razem pełnym odświeżeniem.
                model.page = model.page.min(screen.pages.saturating_sub(1));
                repainted = true;
            }
            Action::SetView(v) => {
                // Przełączenie widoku zeruje nawigację WEWNĄTRZ widoku: numer strony
                // agendy i rozwinięte wydarzenie nie znaczą nic w siatce miesiąca,
                // a zostawione wracałyby przy powrocie w miejsce, którego użytkownik
                // już nie pamięta.
                //
                // Stuknięcie w AKTYWNĄ zakładkę też tu trafia i ma jedno działanie:
                // wyjście ze szczegółów wydarzenia na wierzch bieżącego widoku.
                model.view = v;
                model.focus = None;
                model.page = 0;
                // Wybór ląduje w strukturze teraz, a w pamięci RTC dopiero przy
                // `RtcState::store()` przed uśpieniem — i to wystarczy, bo `.rtc.data`
                // i tak nie przeżywa resetu innego niż wybudzenie z deep sleepu.
                state.view = v.as_u8();
                (canvas, screen) = repaint(
                    epd,
                    &model,
                    state,
                    temperature_c,
                    rotation,
                    Refresh::Full,
                    &mut panel_synced,
                    (canvas, screen),
                );
                repainted = true;
            }
            Action::NextPage => {
                model.page += 1;
                (canvas, screen) = repaint(
                    epd,
                    &model,
                    state,
                    temperature_c,
                    rotation,
                    Refresh::Full,
                    &mut panel_synced,
                    (canvas, screen),
                );
                // Liczba stron zależy od orientacji: te same wydarzenia dają 5 stron
                // w pionie i 12 w poziomie. Bez tego klamrowania `model.page` zostaje
                // poza zakresem i kolejne stuknięcia „wstecz" nic nie robią, płacąc
                // za każdym razem pełnym odświeżeniem.
                model.page = model.page.min(screen.pages.saturating_sub(1));
                repainted = true;
            }
            Action::PrevPage => {
                model.page = model.page.saturating_sub(1);
                (canvas, screen) = repaint(
                    epd,
                    &model,
                    state,
                    temperature_c,
                    rotation,
                    Refresh::Full,
                    &mut panel_synced,
                    (canvas, screen),
                );
                // Liczba stron zależy od orientacji: te same wydarzenia dają 5 stron
                // w pionie i 12 w poziomie. Bez tego klamrowania `model.page` zostaje
                // poza zakresem i kolejne stuknięcia „wstecz" nic nie robią, płacąc
                // za każdym razem pełnym odświeżeniem.
                model.page = model.page.min(screen.pages.saturating_sub(1));
                repainted = true;
            }
            Action::ShowEvent(i) => {
                model.focus = Some(i);
                (canvas, screen) = repaint(
                    epd,
                    &model,
                    state,
                    temperature_c,
                    rotation,
                    Refresh::Full,
                    &mut panel_synced,
                    (canvas, screen),
                );
                // Liczba stron zależy od orientacji: te same wydarzenia dają 5 stron
                // w pionie i 12 w poziomie. Bez tego klamrowania `model.page` zostaje
                // poza zakresem i kolejne stuknięcia „wstecz" nic nie robią, płacąc
                // za każdym razem pełnym odświeżeniem.
                model.page = model.page.min(screen.pages.saturating_sub(1));
                repainted = true;
            }
            Action::Back => {
                if model.focus.take().is_some() {
                    (canvas, screen) = repaint(
                        epd,
                        &model,
                        state,
                        temperature_c,
                        rotation,
                        Refresh::Full,
                        &mut panel_synced,
                        (canvas, screen),
                    );
                    // Liczba stron zależy od orientacji: te same wydarzenia dają 5 stron
                    // w pionie i 12 w poziomie. Bez tego klamrowania `model.page` zostaje
                    // poza zakresem i kolejne stuknięcia „wstecz" nic nie robią, płacąc
                    // za każdym razem pełnym odświeżeniem.
                    model.page = model.page.min(screen.pages.saturating_sub(1));
                    repainted = true;
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

        // Akcja, która nic nie przerysowała, zostawiłaby czarny prostokąt pod palcem.
        // Odrysowujemy sam ten obszar z czystego renderu — to znowu ułamek klatki.
        if flashed && !repainted {
            let (fresh, fresh_screen) = render_frame(&model, rotation);
            canvas = fresh;
            screen = fresh_screen;
            let restore = region.visual.map_or(region.rect, |v| v.rect);
            let area = rotation.canvas_rect_to_panel(restore);
            if let Err(e) = epd.present_area(&canvas, area, temperature_c) {
                warn!("nie mogę przywrócić obszaru pod palcem: {e:#}");
            }
        }
    }

    // Dotąd docieramy wyłącznie po wyczerpaniu okna bezczynności albo po awarii
    // dotyku — czyli bez zapisanej konfiguracji.
    mark_going_to_sleep(epd, &mut canvas, rotation, temperature_c);
    false
}

/// Ekran konfiguracji: jedyna droga wprowadzania danych do urządzenia.
///
/// Zostajemy na jawie, dopóki użytkownik nie naciśnie „zapisz" albo nie przestanie
/// dotykać na [`SETUP_IDLE_MS`]. Zwraca `true`, jeśli cokolwiek zapisano.
fn setup_screen(
    epd: &mut Epd,
    reader: &TouchReader,
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
    let fonts = Fonts::embedded();
    // Płótno żyje przez cały ekran konfiguracji i jest przerysowywane PRZYROSTOWO.
    // Pełna klatka to alokacja 518 KB w PSRAM, wyczyszczenie jej do bieli i narysowanie
    // sześćdziesięciu klawiszy — przy każdym znaku. Panel przy DU to pięć faz po
    // 540 linii, czyli rząd wielkości mniej, niż się wydawało: `frame_time` z waveformu
    // jest używany tylko w ścieżce I2S, a ta płytka idzie przez LCD. Wąskim gardłem
    // było więc rysowanie, nie szkło.
    let (mut canvas, mut screen) = repaint_setup(
        epd,
        &setup,
        state,
        temperature_c,
        rotation,
        Refresh::Full,
        None,
    );

    // Wykrywanie zbocza siedzi w wątku czytającym i jest ciągłe przez oba ekrany,
    // więc palec wciąż leżący na szkle po wejściu tutaj nie generuje fantomowego
    // naciśnięcia, a po powrocie nie gubi się pierwsze stuknięcie.
    let mut deadline = Instant::now() + Duration::from_millis(SETUP_IDLE_MS);

    // Stan wyświetlania trzymamy jako różnicę dwóch rzeczy: co panel POKAZUJE i co
    // POWINIEN pokazywać. Wypchnięcie zdarza się wtedy i tylko wtedy, gdy te dwie
    // rzeczy się rozjeżdżają, a nie za każdym naciśnięciem.
    let mut want_pressed: Option<dashboard::Rect> = None;
    let mut pending_since: Option<Instant> = None;
    let mut force_push = false;
    let mut last_input = Instant::now();
    // Prostokąty tknięte od ostatniego wypchnięcia — klawisze, które zmieniły stan.
    let mut touched: Vec<dashboard::Rect> = Vec::new();
    // Ile odświeżeń DU poszło od ostatniego pełnego — patrz czyszczenie duchów niżej.
    let mut du_since_full = 0u8;

    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(SAMPLE_MS));

        // --- 1. Opróżnij kolejkę do dna ---------------------------------------
        //
        // Wątek czytający zbierał zdarzenia także wtedy, gdy stalismy w sterowniku
        // panelu, więc po dłuższym odświeżeniu czeka ich tu kilka. Bierzemy WSZYSTKIE
        // za jednym razem — po to jest ta kolejka. Brane po jednym na obrót pętli
        // dokładałyby po `SAMPLE_MS` opóźnienia na znak.
        let mut got_input = false;
        loop {
            let point = match reader.poll() {
                Touch::Point(point) => point,
                Touch::Idle => break,
                Touch::Dead => {
                    warn!("dotyk nie odpowiada, zamykam konfigurację");
                    epd.hold_power(false);
                    return false;
                }
            };

            let (x, y) = rotation.panel_to_canvas(point.x, point.y);
            let Some(region) = screen.hit_region(x, y).copied() else {
                info!(
                    "dotyk: panel ({}, {}) -> płótno ({x}, {y}), brak obszaru",
                    point.x, point.y
                );
                continue;
            };
            got_input = true;
            deadline = Instant::now() + Duration::from_millis(SETUP_IDLE_MS);
            last_input = Instant::now();

            match setup.apply(region.action) {
                Applied::Save => {
                    epd.hold_power(false);
                    let saved = save_setup(store, &setup);
                    info!("konfiguracja zapisana: {saved}");
                    return saved;
                }
                Applied::Relayout => {
                    // Inna strona klawiatury albo inne pole — zmienia się pół ekranu
                    // i mapa obszarów dotykowych. Mapę odświeżamy OD RAZU, w pamięci,
                    // żeby kolejne zdarzenia z tej samej kolejki trafiały już w nowy
                    // układ; na panel to i tak pójdzie niżej, jedną klatką.
                    force_push = true;
                    screen = render_setup_frame(&setup, rotation, None).1;
                }
                Applied::Edited => {}
                Applied::Ignored => {}
            }

            if let Some(prev) = want_pressed {
                remember(&mut touched, prev);
            }
            want_pressed = Some(region.rect);
            remember(&mut touched, region.rect);
            pending_since.get_or_insert_with(Instant::now);
        }
        if got_input {
            continue;
        }

        // --- 2. Negatyw pod palcem NIE gaśnie na własnym timerze --------------
        //
        // Wcześniej stało tu odliczanie, które po 250 ms bezczynności wypychało DRUGĄ
        // klatkę tylko po to, żeby odbarwić klawisz. Kosztowało to dokładnie tyle samo
        // co klatka ze znakiem — bo obszar nie skraca przebiegu, patrz `Epd::present_area`
        // — czyli podwajało pracę panelu na każdy wpisany znak, nie wnosząc ani jednej
        // nowej informacji: znak pojawił się już w poprzedniej klatce.
        //
        // Negatyw gaśnie teraz wyłącznie PRZY OKAZJI: następne naciśnięcie dokłada
        // poprzedni klawisz do `touched` (krok 1) i odrysowuje go w tej samej klatce,
        // w której rysuje nowy znak. Jeśli następnego naciśnięcia nie ma, klawisz
        // zostaje zaczerniony — i dobrze, bo pokazuje, co urządzenie ostatnio przyjęło.
        // Sprząta go czyszczenie duchów w przerwie albo wyjście z ekranu.

        // --- 3. Czy już wypychać ---------------------------------------------
        let Some(since) = pending_since else {
            // Duchy po DU: przebieg dwupoziomowy nie resetuje cząstek do końca, więc
            // piksel przepędzony czerń→biel zostaje ciemniejszy od takiego, którego
            // nikt nie ruszał. Na klawiaturze widać to wprost — naciśnięty i zwolniony
            // klawisz zostaje szarawy, a kursor zostawia ślad na każdej pozycji,
            // z której zniknął.
            //
            // Jedynym lekarstwem jest pełne odświeżenie i wcześniej nie było na nie
            // stać: kosztowało ~1,5 s i wchodziło w słowo. Po przejściu na odniesienie
            // z negatywu treści kosztuje ~250 ms, czyli tyle co siedem klatek DU —
            // a odłożone do przerwy w pisaniu nie wchodzi w drogę nikomu.
            if du_since_full >= DU_BEFORE_FULL
                && last_input.elapsed() >= Duration::from_millis(FULL_AFTER_IDLE_MS)
            {
                du_since_full = 0;
                let started = Instant::now();
                // Negatyw ostatniego klawisza gaśnie przy okazji — to jedyne miejsce
                // poza następnym naciśnięciem, które go sprząta.
                want_pressed = None;
                let (fresh, fresh_screen) = repaint_setup(
                    epd,
                    &setup,
                    state,
                    temperature_c,
                    rotation,
                    Refresh::Full,
                    None,
                );
                canvas = fresh;
                screen = fresh_screen;
                touched.clear();
                info!(
                    "czyszczenie duchów w przerwie: {} ms",
                    started.elapsed().as_millis()
                );
                continue;
            }
            if last_input.elapsed() >= Duration::from_millis(POWER_HOLD_IDLE_MS) {
                epd.hold_power(false);
            }
            continue;
        };

        // Czekamy na ciszę, ale nie dłużej niż `MAX_DEFER_MS`. Przy pisaniu ciągiem
        // daje to jedną klatkę na kilka znaków zamiast jednej na znak; przy
        // pojedynczym stuknięciu opóźnienie jest o rząd wielkości mniejsze niż sam
        // przebieg DU, więc niewidoczne.
        let quiet = last_input.elapsed() >= Duration::from_millis(COALESCE_MS);
        let overdue = since.elapsed() >= Duration::from_millis(MAX_DEFER_MS);
        if !(force_push || quiet || overdue) {
            continue;
        }

        // --- 4. Jedna klatka nadrabiająca wszystko, co się wydarzyło ----------
        let started = Instant::now();
        epd.hold_power(true);
        // `force_push` to zmiana układu (inna strona klawiatury, inne pole) — pół
        // ekranu i cała mapa dotykowa. Idzie jako pełna klatka, ale w trybie DU.
        //
        // Pełnego odświeżenia (`Refresh::Full`) TU NIE MA i to jest sedno poprawki.
        // Kosztuje ono `epd_fullclear` + GC16, czyli 126 przebiegów bramek — grubo
        // ponad sekundę czarno-białego migotania całym ekranem. Wpuszczone w środek
        // pisania co N znaków dawało dokładnie ten objaw, przez który ta klawiatura
        // była nie do użycia: znak, znak, znak, ZAMARCIE. Czyszczenie duchów jest
        // teraz odkładane do przerwy w pisaniu — patrz `deferred_full` niżej.
        if force_push {
            let mode = Refresh::Fast;
            let (fresh, fresh_screen) = repaint_setup(
                epd,
                &setup,
                state,
                temperature_c,
                rotation,
                mode,
                want_pressed,
            );
            canvas = fresh;
            screen = fresh_screen;
            force_push = false;
            info!("klatka pełna: {} ms", started.elapsed().as_millis());
        } else {
            // Przyrostowo: pole wartości i te klawisze, które zmieniły stan.
            // Mapa obszarów dotykowych się nie zmienia — układ klawiatury jest ten sam,
            // a zmiana układu idzie gałęzią wyżej.
            let box_rect = dashboard::layout::redraw_setup_value(&setup, &fonts, &mut canvas);
            dashboard::layout::redraw_setup_keys(
                &setup,
                &fonts,
                &mut canvas,
                &touched,
                want_pressed,
            );
            let rendered = started.elapsed().as_millis();

            let mut areas = Vec::with_capacity(touched.len() + 1);
            areas.push(rotation.canvas_rect_to_panel(box_rect));
            for rect in &touched {
                areas.push(rotation.canvas_rect_to_panel(*rect));
            }

            if let Err(e) = epd.present_areas(&canvas, &areas, temperature_c) {
                warn!("nie mogę odrysować klawiatury: {e:#}");
            }
            let total = started.elapsed().as_millis();
            info!(
                "klatka: {total} ms (render {rendered} ms, panel {} ms, {} obszarów)",
                total - rendered,
                areas.len()
            );
        }

        du_since_full = du_since_full.saturating_add(1);
        touched.clear();
        pending_since = None;
    }

    info!("konfiguracja: brak aktywności, wracam bez zapisu");
    epd.hold_power(false);
    false
}

/// Dopisuje prostokąt do listy, jeśli jeszcze go tam nie ma.
fn remember(list: &mut Vec<dashboard::Rect>, rect: dashboard::Rect) {
    if !list.contains(&rect) {
        list.push(rect);
    }
}

/// Renderuje ekran konfiguracji do pamięci, bez dotykania panelu.
fn render_setup_frame(
    setup: &dashboard::setup::Setup,
    rotation: Rotation,
    pressed: Option<dashboard::Rect>,
) -> (Gray8, dashboard::Screen) {
    let fonts = Fonts::embedded();
    let mut canvas = Gray8::new(rotation);
    let screen = dashboard::layout::render_setup_pressed(setup, &fonts, &mut canvas, pressed);
    (canvas, screen)
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
    synced: &mut bool,
    // Klatka, która JEST na szkle. Zwracana bez zmian, gdy wypchnięcie zawiedzie.
    poprzednia: (Gray8, dashboard::Screen),
) -> (Gray8, dashboard::Screen) {
    // Dopóki panel nie dostał w tym wybudzeniu pełnej klatki, `back_fb` epdiy kłamie
    // — więc pierwsze przerysowanie idzie pełne niezależnie od tego, o co prosi
    // wołający. Pełne wyjaśnienie przy `Epd::present`.
    let mode = if *synced { mode } else { Refresh::Full };
    match paint(epd, model, state, temperature_c, rotation, mode) {
        Ok(out) => {
            *synced = true;
            out
        }
        Err(e) => {
            // Zwracamy POPRZEDNIĄ klatkę, nie pustą. Wcześniej szło tu
            // `Screen::default()`, czyli **pusta mapa dotykowa** — po jednym błędzie
            // sterownika okno stawało się martwe do końca, mimo że na szkle wciąż
            // widać poprawną klatkę. Komentarz nad tą funkcją mówił zresztą dokładnie
            // to, czego kod nie robił.
            //
            // Świeżo wyrenderowana klatka też jest zła: skoro wypchnięcie zawiodło,
            // na szkle została STARA treść i to jej mapa jest prawdziwa.
            error!("przerysowanie nie powiodło się: {e:#}");
            poprzednia
        }
    }
}

/// Natychmiastowa odpowiedź na dotknięcie: zaczernia trafiony obszar i odświeża
/// **sam ten prostokąt**.
///
/// To jest oddzielone od wykonania akcji i takie ma zostać. Akcja pod przyciskiem
/// bywa droga (pełna klatka, wejście w konfigurację, zapis do NVS) albo w ogóle
/// niewidoczna (`RefreshNow` tylko odkłada życzenie na następne wybudzenie) —
/// a człowiek musi wiedzieć, że urządzenie go usłyszało, zanim cokolwiek z tego
/// się wydarzy. Na e-papierze brak odpowiedzi jest nieodróżnialny od zepsutego dotyku.
///
/// Zwraca `false`, gdy nie było czego mignąć — wtedy nie ma też czego przywracać.
fn flash_region(
    epd: &mut Epd,
    canvas: &mut Gray8,
    rect: dashboard::Rect,
    visual: Option<dashboard::Visual>,
    rotation: Rotation,
    temperature_c: i32,
) -> bool {
    // Odwrócenie, nie zalanie czernią: pod palcem ma zostać widoczne to, w co
    // użytkownik trafił. Zakryty czarnym prostokątem guzik wygląda jak usterka.
    //
    // Kształt bierze się z `HitRegion::visual`, nie z obszaru dotykowego. Cel dotyku
    // bywa celowo większy od rysunku (plakietka statusu ma +10/+6 px, żeby dało się
    // w nią trafić), a odwracanie prostokąta OPISANEGO zapalało zaokrąglony guzik
    // jako ostry prostokąt, w dodatku większy od czegokolwiek, co widać.
    let (shape, radius) = match visual {
        Some(v) => (v.rect, v.radius as f32),
        None => (rect, 0.0),
    };
    dashboard::shapes::invert_round_rect(canvas, shape, radius);
    let area = rotation.canvas_rect_to_panel(shape);
    let started = std::time::Instant::now();
    match epd.present_area(canvas, area, temperature_c) {
        Ok(()) => {
            info!("feedback dotyku: {} ms", started.elapsed().as_millis());
            true
        }
        Err(e) => {
            warn!("nie mogę odrysować obszaru pod palcem: {e:#}");
            false
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn repaint_setup(
    epd: &mut Epd,
    setup: &dashboard::setup::Setup,
    state: &mut RtcState,
    temperature_c: i32,
    rotation: Rotation,
    mode: Refresh,
    pressed: Option<dashboard::Rect>,
) -> (Gray8, dashboard::Screen) {
    let (canvas, screen) = render_setup_frame(setup, rotation, pressed);
    if let Err(e) = present(epd, &canvas, state, temperature_c, mode) {
        error!("rysowanie konfiguracji nie powiodło się: {e:#}");
    }
    (canvas, screen)
}

/// Którą kartę bring-upową wyrysować zamiast normalnego ekranu.
#[derive(PartialEq, Eq)]
enum BringUpCard {
    /// Normalna praca.
    None,
    /// Drabina szesnastu poziomów, dither kontra półton, biel pełna kontra częściowa.
    Tones,
    /// Prawie puste tło z podziałkami we współrzędnych panelu — do pasów.
    Uniformity,
}

/// Ile razy dodatkowo przepędzić panel czyszczeniem przed rysowaniem karty.
///
/// Każdy przebieg to ~0,7 s i 96 przelotów przez wszystkie bramki. Zero, bo pytanie
/// „czy to historia" jest już rozstrzygnięte — jaśniejsze prostokąty leżą dokładnie
/// tam, gdzie robiono odświeżenia częściowe, i są odrobinę większe od obiektów, bo
/// `clamp_area` wyrównuje wypychany prostokąt do ośmiu pikseli. Lekarstwem jest
/// `FINISH_FULL_WITH_DU` w `epd`, nie kolejne czyszczenia.
const EXTRA_FULLCLEARS: u32 = 0;

const BRING_UP_CARD: BringUpCard = BringUpCard::None;

/// Jak długo urządzenie śpi po wyrysowaniu karty.
///
/// E-papier trzyma obraz bez zasilania, więc karta zostaje na szkle przez cały sen
/// i można ją oglądać bez pośpiechu. Godzina jest po to, żeby płytka zostawiona
/// na biurku nie budziła się w kółko.
const TEST_CARD_SLEEP_S: u64 = 3_600;

/// Rysuje wybraną kartę bring-upową i wypycha jej pole „DU" osobno.
///
/// Kolejność jest istotna i jest w niej cały trzeci pomiar: najpierw CAŁA karta idzie
/// pełnym odświeżeniem (`epd_fullclear` + GC16), a dopiero potem jeden prostokąt
/// dostaje odświeżenie częściowe (MODE_DU). Oba pola są bielą i w płótnie są
/// identyczne co do bitu — więc każda różnica, jaką widać na szkle, pochodzi
/// wyłącznie z tego, czym je odświeżono. To jest ta różnica, przez którą odświeżone
/// fragmentarycznie prostokąty wyglądają na jaśniejsze od tła.
fn show_bring_up_card(epd: &mut Epd, temperature_c: i32, rotation: Rotation) {
    let fonts = Fonts::embedded();
    let mut canvas = Gray8::new(rotation);

    // Czyszczenie PRZED narysowaniem czegokolwiek — inaczej mierzylibyśmy stan
    // panelu po naszym własnym rysowaniu, a nie po samym czyszczeniu. Przy zerze
    // `deep_clean` wraca od razu.
    epd.deep_clean(EXTRA_FULLCLEARS, temperature_c);

    let du_box = match BRING_UP_CARD {
        BringUpCard::Uniformity => dashboard::render_uniformity_card(&fonts, &mut canvas),
        _ => dashboard::render_test_card(&fonts, &mut canvas),
    };

    if let Err(e) = epd.present(&canvas, Refresh::Full, temperature_c) {
        error!("nie mogę wyrysować karty tonów: {e:#}");
        return;
    }
    let area = rotation.canvas_rect_to_panel(du_box);
    if let Err(e) = epd.present_area(&canvas, area, temperature_c) {
        warn!("nie mogę wypchnąć pola DU karty: {e:#}");
    }
    epd.ensure_powered_off();
}

/// Czy rysować znacznik zasypiania. Rzecz na czas bring-upu: bez niego nie da się
/// odróżnić „urządzenie mnie ignoruje" od „urządzenie śpi", bo e-papier trzyma obraz
/// tak samo w obu wypadkach — a śpi prawie zawsze.
const SLEEP_MARKER: bool = true;

/// Bok kwadracika i jego odstęp od krawędzi płótna.
const SLEEP_MARKER_SIZE: i32 = 22;
const SLEEP_MARKER_MARGIN: i32 = 12;

/// Zaczernia róg ekranu tuż przed zaśnięciem.
///
/// Znacznik zostaje na szkle przez cały sen, bo e-papier nie potrzebuje zasilania,
/// żeby go utrzymać, i znika sam przy pierwszym przerysowaniu po wybudzeniu.
/// Czarny kwadrat w rogu = urządzenie śpi i dotyk nic nie da. Brak = czuwa.
fn mark_going_to_sleep(epd: &mut Epd, canvas: &mut Gray8, rotation: Rotation, temperature_c: i32) {
    if !SLEEP_MARKER {
        return;
    }
    let rect = dashboard::Rect::new(
        canvas.width() as i32 - SLEEP_MARKER_MARGIN - SLEEP_MARKER_SIZE,
        canvas.height() as i32 - SLEEP_MARKER_MARGIN - SLEEP_MARKER_SIZE,
        SLEEP_MARKER_SIZE,
        SLEEP_MARKER_SIZE,
    );
    canvas.fill_rect(rect, dashboard::canvas::BLACK);
    let areas = [rotation.canvas_rect_to_panel(rect)];
    if let Err(e) = epd.present_areas(canvas, &areas, temperature_c) {
        warn!("nie mogę narysować znacznika snu: {e:#}");
    }
}

/// Ile odświeżeń DU wolno nazbierać, zanim w najbliższej przerwie posprzątamy duchy.
///
/// Duch po DU narasta z każdym przebiegiem, więc próg jest kompromisem między
/// czystością a liczbą przerw. Dwanaście to mniej więcej jedno słowo.
const DU_BEFORE_FULL: u8 = 12;

/// Jak długa musi być przerwa w pisaniu, żeby wtrącić pełne odświeżenie.
///
/// Dwie sekundy to znacznie więcej niż odstęp między znakami, a wyraźnie mniej niż
/// czas potrzebny na znalezienie kolejnego pola. Wcześniej próg wynosił 1,2 s przy
/// odświeżeniu za 1,5 s i to i tak wchodziło w słowo — teraz odświeżenie trwa
/// ~250 ms, więc nawet trafione w niewłaściwy moment nie jest dotkliwe.
const FULL_AFTER_IDLE_MS: u64 = 2_000;

/// Po jakiej bezczynności opuszczamy szyny panelu w czasie pisania.
///
/// Było 1500 ms i to była wartość dobrana pod tempo pisania, którego nikt nie
/// osiąga: przy odstępie między znakami większym niż próg KAŻDY klawisz płacił
/// pełną sekwencję `epd_poweron` — rozmowę po I²C z TPS65185 i pętlę czekającą na
/// power-good. Próg ma rozstrzygać „człowiek skończył pisać", a nie „człowiek
/// szuka litery", więc jest liczony w sekundach.
const POWER_HOLD_IDLE_MS: u64 = 6_000;

/// Ile ciszy na dotyku czekamy przed wypchnięciem klatki.
///
/// Przebieg DU trwa ~0,3 s i przez ten czas nie da się odpytywać dotyku — więc
/// rysowanie na każde naciśnięcie ustawia twardy sufit: jeden znak na jedną klatkę,
/// a wszystko, co użytkownik nacisnął w międzyczasie, przepada. Zamiast tego
/// zbieramy naciśnięcia i rysujemy raz, gdy palec na chwilę odpuści.
const COALESCE_MS: u64 = 15;

/// …ale nie dłużej niż tyle od pierwszej niewypchniętej zmiany.
///
/// Bez tego pisanie równym rytmem szybszym niż `COALESCE_MS` odkładałoby klatkę
/// w nieskończoność i ekran stałby martwy.
const MAX_DEFER_MS: u64 = 220;

/// Odstęp próbkowania przycisków i dotyku.
///
/// GT911 skanuje szkło co 5–15 ms, więc rzadsze odpytywanie dokłada się wprost do
/// opóźnienia między dotknięciem a reakcją. Dziesięć milisekund kosztuje trochę
/// ruchu na I²C, ale tylko na jawie — a na jawie i tak świeci panel.
const SAMPLE_MS: u64 = 10;

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

/// Wolny WEWNĘTRZNY DRAM w kilobajtach.
///
/// To jest zasób, o który biją się epdiy i mbedTLS, i jedyny, którego na tej płytce
/// realnie brakuje — PSRAM-u jest osiem megabajtów. Handshake TLS do Google
/// z pełnym pakietem certyfikatów to szczyt zapotrzebowania w całym cyklu, więc
/// liczba tuż przed nim mówi, ile marginesu naprawdę zostało.
/// Milisekundy od startu układu — do okruszka startowego.
///
/// `esp_timer_get_time` liczy od resetu, więc jest odporny na to, że w chwili
/// zapisu zegar kalendarzowy może być jeszcze nieustawiony (SNTP dopiero przed nami).
fn ms_od_startu() -> u32 {
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } / 1000).clamp(0, u32::MAX as i64) as u32
}

/// Ile bajtów stosu zadania `main` NIGDY nie zostało użyte.
///
/// `uxTaskGetStackHighWaterMark(NULL)` podaje minimum wolnego miejsca od startu
/// zadania. Przy przepełnieniu backtrace jest bezużyteczny — same adresy z obsługi
/// paniki i `|<-CORRUPTED` — więc jedyny sposób, żeby dowiedzieć się, KTÓRY krok
/// zjada stos, to zmierzyć zapas przed nim i po nim.
fn zapas_stosu_b() -> u32 {
    // SAFETY: `NULL` znaczy „bieżące zadanie"; wywołanie jest tylko odczytem.
    unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut()) }
}

/// Czas unix albo zero, gdy zegar jeszcze nic nie wie.
fn now_unix_lub_zero() -> i64 {
    net::time::now_unix().max(0)
}

fn wolny_dram_kb() -> u32 {
    // SAFETY: prosty getter z ESP-IDF, bez stanu.
    let bytes =
        unsafe { esp_idf_svc::sys::heap_caps_get_free_size(esp_idf_svc::sys::MALLOC_CAP_INTERNAL) };
    (bytes / 1024) as u32
}

/// Ile milisekund minęło od startu układu — pomiar 5 z `docs/bringup.md`.
fn uptime_ms() -> u128 {
    // SAFETY: prosty getter z ESP-IDF, zwraca mikrosekundy od bootu.
    (unsafe { esp_idf_svc::sys::esp_timer_get_time() } as u128) / 1000
}
