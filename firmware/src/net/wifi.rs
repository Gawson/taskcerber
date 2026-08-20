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
        let esp_wifi = EspWifi::new(modem, sysloop.clone(), Some(nvs))
            .context("nie mogę zainicjalizować WiFi")?;
        let mut wifi = BlockingWifi::wrap(esp_wifi, sysloop).context("nie mogę opakować WiFi")?;

        let started = Instant::now();

        if state.ap_cached {
            info!("próbuję zapamiętanego AP na kanale {}", state.ap_channel);
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
                    warn!("zapamiętany AP nie odpowiedział ({e:#}), unieważniam bufor");
                    state.invalidate_ap();
                    // Z limitem:  z esp-idf-svc czeka bez końca.
                    Self::stop_bounded(&mut wifi, CONNECT_TIMEOUT.saturating_sub(started.elapsed()));
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
        Self::try_connect(&mut wifi, ssid, password, None, None, left)
            .context("nie mogę połączyć się z siecią")?;

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

        // `BlockingWifi::start()` NIE MA LIMITU CZASU — w esp-idf-svc 0.52.1 woła
        // `wifi_wait_while(..., None)`. Jeśli zdarzenie `STA_START` nigdy nie
        // przyjdzie, blokuje się **na zawsze**: bez paniki, bez resetu, bez śladu.
        // Dokładnie tak wyglądał objaw ze sprzętu — na kablu urządzenie stało
        // w kroku sieciowym i nigdy nie dochodziło nawet do znacznika snu.
        //
        // To samo dotyczy `stop()` i `disconnect()`. Limit ma tylko `connect()`
        // (15 s, zaszyte w esp-idf-svc). Dlatego wołamy nieblokujący sterownik
        // i czekamy SAMI, z naszym budżetem.
        wifi.wifi_mut()
            .start()
            .context("nie mogę wystartować WiFi")?;
        wait_bounded(wifi, deadline, "start sterownika WiFi", |w| {
            w.wifi().is_started().map(|s| !s)
        })?;

        wifi.connect().context("asocjacja nie powiodła się")?;

        // `BlockingWifi::wait_netif_up` ma WŁASNY limit 15 s zaszyty w esp-idf-svc
        // i nie przyjmuje naszego. Wcześniej stało tu właśnie ono, a nasz `deadline`
        // był sprawdzany DOPIERO PO POWROCIE — czyli nie ograniczał niczego, tylko
        // meldował, że czas już minął. Przy dwóch próbach (zapamiętany AP, potem
        // zwykłe wyszukiwanie) dawało to minutę stania na samym WiFi, przy ośmiu
        // sekundach obiecanych w dokumentacji tej stałej.
        //
        // `ip_wait_while` to ta sama funkcja, do której `wait_netif_up` deleguje,
        // tylko przyjmuje limit — więc podajemy nasz, i to ten, który ZOSTAŁ.
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            bail!("budżet na podłączenie wyczerpany przed uzyskaniem adresu");
        }
        let w: &BlockingWifi<EspWifi<'_>> = wifi;
        w.ip_wait_while(|| w.is_up().map(|up| !up), Some(left))
            .context("interfejs sieciowy nie wstał w wyznaczonym czasie")?;
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

/// Czeka, aż `matcher` przestanie być prawdziwy, ale nie dłużej niż do `deadline`.
///
/// Istnieje, bo `BlockingWifi` w esp-idf-svc 0.52.1 czeka **bez limitu** w `start`,
/// `stop` i `disconnect` (`wifi_wait_while(..., None)`), a limit ma wyłącznie
/// `connect`. Blokada w którymkolwiek z tych trzech to zawieszenie bez paniki
/// i bez resetu — czyli urządzenie, które stoi w nieskończoność z ostatnią klatką
/// na szkle.
fn wait_bounded<F>(
    wifi: &BlockingWifi<EspWifi<'_>>,
    deadline: Instant,
    co: &str,
    matcher: F,
) -> Result<()>
where
    F: Fn(&BlockingWifi<EspWifi<'_>>) -> Result<bool, esp_idf_svc::sys::EspError>,
{
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        bail!("budżet wyczerpany przed: {co}");
    }
    wifi.wifi_wait_while(|| matcher(wifi), Some(left))
        .with_context(|| format!("{co} nie zdążyło w {left:?}"))
}
