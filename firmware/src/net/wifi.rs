//! Łączenie z WiFi, z buforowaniem punktu dostępowego.
//!
//! Największa pojedyncza dźwignia w budżecie energetycznym: asocjacja z zapamiętanym
//! BSSID i kanałem trwa poniżej pół sekundy, a pełne skanowanie 2–4 sekundy przy
//! ~110 mA. To różnica około 300 mAs na każde wybudzenie, przy budżecie 360 mAs
//! na całe pobranie.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{
    AuthMethod, BlockingWifi, ClientConfiguration, Configuration as WifiConfiguration, EspWifi,
};
use log::{info, warn};

use crate::power::rtc_state::RtcState;

/// Twardy limit na całe podłączenie. Po jego przekroczeniu wracamy do snu —
/// nieudana próba kosztuje 200–480 mAs, czyli więcej niż udane pobranie,
/// więc nie wolno jej przeciągać ani powtarzać w obrębie jednego wybudzenia.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// Limit na zamknięcie radia.
///
/// Krótki, bo tu już nie ma czego ratować: jeśli sterownik nie potwierdzi
/// rozłączenia, i tak zaraz idziemy spać, a deep sleep gasi radio twardo.
/// Chodzi wyłącznie o to, żeby nie stać tu w nieskończoność.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

pub struct Wifi<'a> {
    inner: BlockingWifi<EspWifi<'a>>,
}

impl<'a> Wifi<'a> {
    /// Podnosi radio i łączy się z siecią.
    ///
    /// Gdy `state` ma zbuforowany punkt dostępowy, próbuje najpierw jego; przy
    /// niepowodzeniu unieważnia bufor i próbuje raz przez zwykłe wyszukiwanie.
    pub fn connect(
        modem: Modem<'a>,
        sysloop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
        ssid: &str,
        password: &str,
        state: &mut RtcState,
    ) -> Result<Self> {
        // Znaczniki kroków. Ścieżka sieciowa jest jedynym miejscem, w którym
        // urządzenie potrafiło stanąć bez śladu — a wtedy ostatnia wypisana linia
        // jest jedyną informacją o tym, GDZIE stanęło. Każdy krok mówi, ile trwał.
        let started = Instant::now();
        let krok = |co: &str| info!("sieć[{} ms]: {co}", started.elapsed().as_millis());

        krok("inicjalizuję sterownik");
        let esp_wifi = EspWifi::new(modem, sysloop.clone(), Some(nvs))
            .context("nie mogę zainicjalizować WiFi")?;
        let mut wifi = BlockingWifi::wrap(esp_wifi, sysloop).context("nie mogę opakować WiFi")?;
        krok("sterownik gotowy");

        if state.ap_cached {
            krok("próbuję zapamiętanego AP");
            match Self::try_connect(
                &mut wifi,
                ssid,
                password,
                Some(state.ap_bssid),
                Some(state.ap_channel),
                CONNECT_TIMEOUT.saturating_sub(started.elapsed()),
            ) {
                Ok(()) => {
                    info!("połączono z bufora w {} ms", started.elapsed().as_millis());
                    return Ok(Self { inner: wifi });
                }
                Err(e) => {
                    krok("zapamiętany AP odpadł, zatrzymuję sterownik");
                    warn!("zapamiętany AP nie odpowiedział ({e:#}), unieważniam bufor");
                    state.invalidate_ap();
                    // Z limitem:  z esp-idf-svc czeka bez końca.
                    Self::stop_bounded(
                        &mut wifi,
                        CONNECT_TIMEOUT.saturating_sub(started.elapsed()),
                    );
                }
            }
        }

        // Druga próba dostaje to, co ZOSTAŁO — nie świeże osiem sekund. Komentarz
        // przy `CONNECT_TIMEOUT` mówi wprost: limit dotyczy całego podłączenia
        // i nie wolno go powtarzać w obrębie jednego wybudzenia.
        let left = CONNECT_TIMEOUT.saturating_sub(started.elapsed());
        if left.is_zero() {
            bail!("budżet {CONNECT_TIMEOUT:?} na podłączenie wyczerpany przez zapamiętany AP");
        }
        krok("szukam sieci");
        Self::try_connect(&mut wifi, ssid, password, None, None, left)
            .context("nie mogę połączyć się z siecią")?;
        krok("połączony");

        // Zapamiętaj, z czym się udało.
        if let Ok(info) = wifi.wifi().sta_netif().get_ip_info() {
            info!("adres IP: {}", info.ip);
        }
        if let Ok(ap) = wifi.wifi().driver().get_configuration() {
            if let WifiConfiguration::Client(c) = ap {
                if let (Some(bssid), Some(channel)) = (c.bssid, c.channel) {
                    state.cache_ap(bssid, channel);
                }
            }
        }

        info!("połączono w {} ms", started.elapsed().as_millis());
        Ok(Self { inner: wifi })
    }

    /// Jedna próba asocjacji, ograniczona `budget`.
    ///
    /// `budget` jest tym, co ZOSTAŁO z całego limitu na podłączenie — nie limitem
    /// na tę próbę. Dzięki temu druga próba nie może podwoić czasu.
    fn try_connect(
        wifi: &mut BlockingWifi<EspWifi<'_>>,
        ssid: &str,
        password: &str,
        bssid: Option<[u8; 6]>,
        channel: Option<u8>,
        budget: Duration,
    ) -> Result<()> {
        let ssid = ssid
            .try_into()
            .map_err(|_| anyhow::anyhow!("SSID dłuższy niż 32 znaki"))?;
        let password = password
            .try_into()
            .map_err(|_| anyhow::anyhow!("hasło dłuższe niż 64 znaki"))?;

        wifi.set_configuration(&WifiConfiguration::Client(ClientConfiguration {
            ssid,
            password,
            bssid,
            channel,
            // UWAGA: `auth_method` to PRÓG minimalnego bezpieczeństwa, nie deklaracja.
            // Domyślne `WPA2Personal` po cichu odmawia połączenia z sieciami otwartymi
            // i WPA-only, co przy provisioningu w terenie wygląda jak zepsute WiFi.
            auth_method: AuthMethod::None,
            ..Default::default()
        }))?;

        let deadline = Instant::now() + budget;

        // NIE UŻYWAMY tu `BlockingWifi::start()` ani `connect()`, i to jest sedno
        // całej tej funkcji.
        //
        // `start()`, `stop()` i `disconnect()` w esp-idf-svc 0.52.1 wołają
        // `wifi_wait_while(..., None)` — czyli czekają na zdarzenie **bez żadnego
        // limitu**, aż po `xSemaphoreTake(..., portMAX_DELAY)`. Zawieszenie tam jest
        // dosłownie wieczne: task stoi w `Blocked`, więc nie ma paniki ani resetu,
        // a task watchdog tego nie łapie z definicji — zablokowany task oddaje
        // procesor i idle karmi WDT normalnie.
        //
        // `connect()` limit niby ma (15 s), ale to limit CISZY, nie termin — patrz
        // [`wait_bounded`]. Przy sieci sypiącej `STA_DISCONNECTED` stoi dowolnie długo.
        //
        // Dlatego wołamy nieblokujący sterownik i czekamy sami, odpytując flagę.
        info!("  · start sterownika");
        wifi.wifi_mut()
            .start()
            .context("nie mogę wystartować WiFi")?;
        wait_bounded(wifi, deadline, "start sterownika WiFi", |w| {
            w.wifi().is_started().map(|s| !s)
        })?;

        info!("  · asocjacja");
        wifi.wifi_mut()
            .connect()
            .context("asocjacja nie powiodła się")?;
        wait_bounded(wifi, deadline, "asocjacja", |w| {
            w.wifi().is_connected().map(|c| !c)
        })?;
        info!("  · czekam na adres");

        // Wcześniej stało tu `wait_netif_up()`, a nasz `deadline` był sprawdzany
        // DOPIERO PO jego powrocie — czyli nie ograniczał niczego, tylko meldował,
        // że czas już minął. Potem było `ip_wait_while(..., Some(left))`, co też nie
        // pomogło, bo tamten „limit" jest limitem CISZY, nie terminem. Szczegóły
        // przy [`wait_bounded`], które jako jedyne trzyma tu prawdziwy termin.
        wait_bounded(wifi, deadline, "uzyskanie adresu", |w| {
            w.is_up().map(|up| !up)
        })?;
        Ok(())
    }

    /// Zatrzymuje sterownik z limitem czasu.
    ///
    /// `BlockingWifi::stop()` czeka bez limitu — patrz komentarz w [`Wifi::try_connect`].
    fn stop_bounded(wifi: &mut BlockingWifi<EspWifi<'_>>, budget: Duration) {
        if let Err(e) = wifi.wifi_mut().stop() {
            warn!("zatrzymanie WiFi zwróciło błąd: {e:#}");
            return;
        }
        let deadline = Instant::now() + budget;
        if let Err(e) = wait_bounded(wifi, deadline, "zatrzymanie sterownika WiFi", |w| {
            w.wifi().is_started()
        }) {
            warn!("{e:#}");
        }
    }

    /// Siła sygnału w dBm, jeśli da się odczytać.
    pub fn rssi(&self) -> Option<i8> {
        self.inner.wifi().driver().get_rssi().ok().map(|v| v as i8)
    }

    /// Wyłącza radio.
    ///
    /// **Wołaj to przed podniesieniem szyn panelu.** Panel ciągnie ~115 mA na stałe,
    /// a szczyt nadajnika WiFi to 283–340 mA; obie rzeczy naraz przez LDO przy
    /// rozładowanym ogniwie to generator brownoutów, a reset w trakcie odświeżania
    /// z podniesionymi szynami TPS65185 to jedyna naprawdę szkodliwa awaria tej płytki.
    pub fn shutdown(mut self) {
        // Ani `disconnect()`, ani `stop()` z `BlockingWifi` NIE MA LIMITU CZASU —
        // patrz komentarz w [`Wifi::try_connect`]. Zawieszenie akurat tutaj byłoby
        // najgorsze z możliwych: radio zostaje na antenie, a wołający czeka, żeby
        // dopiero po nim podnieść szyny panelu. Nagłówek `main.rs` mówi wprost, że
        // te dwie rzeczy nigdy nie mogą być włączone naraz.
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;

        if let Err(e) = self.inner.wifi_mut().disconnect() {
            warn!("rozłączenie WiFi zwróciło błąd: {e:#}");
        } else if let Err(e) = wait_bounded(&self.inner, deadline, "rozłączenie WiFi", |w| {
            w.wifi().is_connected()
        }) {
            warn!("{e:#}");
        }

        if let Err(e) = self.inner.wifi_mut().stop() {
            warn!("zatrzymanie WiFi zwróciło błąd: {e:#}");
        } else if let Err(e) = wait_bounded(&self.inner, deadline, "zatrzymanie WiFi", |w| {
            w.wifi().is_started()
        }) {
            warn!("{e:#}");
        }
    }
}

/// Czeka, aż `matcher` przestanie być prawdziwy — z TWARDYM terminem.
///
/// # Dlaczego odpytywanie, a nie `wifi_wait_while(..., Some(limit))`
///
/// Bo tamten limit nie jest terminem. `esp-idf-svc` czeka tak
/// (`private/waitable.rs`, `wait_timeout_while_and_get`):
///
/// ```ignore
/// loop {
///     if !condition(&state)? { return ... }
///     state = self.cvar.wait_timeout(state, dur)?;   // dur NIEZMIENIONE
/// }
/// ```
///
/// `dur` jest podawane od nowa przy każdym obrocie, więc to jest **limit CISZY**,
/// a nie limit całkowity: każde powiadomienie, które nie spełnia warunku, restartuje
/// pełne odliczanie. Przy sieci sypiącej `STA_DISCONNECTED` — na przykład przy złym
/// haśle — `connect()` z jego piętnastoma sekundami potrafi stać dowolnie długo.
///
/// To jest powód, dla którego poprzednia poprawka (przekazanie `Some(budget)`
/// zamiast `None`) NIE POMOGŁA: odziedziczyła dokładnie tę samą wadę.
///
/// Odpytywanie flagi jest tu tańsze, niż wygląda: `is_started`, `is_connected`
/// i `is_up` czytają pole struktury utrzymywane przez handler sterownika, więc
/// obrót pętli to kilka instrukcji i `vTaskDelay`. Za to termin jest terminem.
fn wait_bounded<F>(
    wifi: &BlockingWifi<EspWifi<'_>>,
    deadline: Instant,
    co: &str,
    matcher: F,
) -> Result<()>
where
    F: Fn(&BlockingWifi<EspWifi<'_>>) -> Result<bool, esp_idf_svc::sys::EspError>,
{
    /// Sto milisekund: dość rzadko, żeby nie kręcić rdzeniem, i dość gęsto, żeby
    /// nie dokładać zauważalnie do czasu z radiem na antenie.
    const POLL: Duration = Duration::from_millis(100);

    loop {
        if !matcher(wifi).with_context(|| format!("odczyt stanu przy: {co}"))? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("{co} nie zdążyło w wyznaczonym czasie");
        }
        std::thread::sleep(POLL);
    }
}
