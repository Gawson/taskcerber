//! GT911 — pojemnościowy kontroler dotyku, adres I²C `0x5D`.
//!
//! To jest jedyne wejście urządzenia. Bez niego nie da się ani skonfigurować
//! WiFi, ani przewinąć agendy — patrz `dashboard::setup`.
//!
//! # Sekwencja resetu wybiera adres i to nie jest opcjonalne
//!
//! GT911 czyta poziom na `INT` w momencie zwolnienia `RST` i **na tej podstawie
//! ustala swój adres I²C**: `INT` w dole daje `0x5D`, `INT` w górze daje `0x14`.
//! Nie ma pinu strapującego ani wpisu w OTP — jeśli zaraz po starcie zapytasz
//! pod `0x5D` bez przeprowadzenia tej sekwencji, dostaniesz albo ciszę, albo
//! układ, który jeszcze nie wstał.
//!
//! Sekwencję trzeba powtarzać **przy każdym wybudzeniu**. Deep sleep sprowadza
//! `T_RST` (GPIO9) do stanu domyślnego, czyli trzyma kontroler w resecie.
//!
//! # Rejestry są 16-bitowe
//!
//! W przeciwieństwie do reszty układów na tej magistrali (PCA9535, BQ25896,
//! BQ27220, PCF8563 — wszystkie z 8-bitowym adresem rejestru) GT911 adresuje
//! rejestry dwoma bajtami, starszym najpierw. Stąd własne `read_at`/`write_at`
//! zamiast `I2cDevice::read_u8`.
//!
//! # Czego tu nie ma
//!
//! Nie zapisujemy konfiguracji (`0x8047`+): panel przyjeżdża skonfigurowany, a zły
//! wpis wraz z sumą kontrolną potrafi zostawić kontroler w stanie, z którego wychodzi
//! się tylko przez ponowne wgranie poprawnej tablicy. Nie czytamy też drugiego punktu
//! dotyku — całe `hit::Screen` operuje pojedynczym punktem, a gesty nie są tu do niczego
//! potrzebne.
//!
//! **Nic z tego nie było uruchomione na sprzęcie.** Szczególnie orientacja osi
//! względem panelu jest założeniem, nie pomiarem — patrz [`Gt911::read`].

use anyhow::{bail, Result};
use esp_idf_svc::sys;
use log::{info, warn};

use crate::i2c::{I2cBus, I2cDevice};

/// Adres po sekwencji resetu z `INT` w dole.
const ADDRESS: u8 = 0x5D;

/// Adres, na którym kontroler ląduje, gdy `INT` był w GÓRZE przy zwolnieniu `RST`.
///
/// Próbujemy go po `0x5D` nie z ostrożności, tylko dlatego, że polaryzacja `INT`
/// w tej sekwencji jest jedyną rzeczą, której nie da się sprawdzić inaczej niż na
/// sztuce. Jeśli układ odezwie się tutaj, urządzenie **działa** i jednocześnie mówi
/// w logu, co poprawić.
const ADDRESS_ALT: u8 = 0x14;

/// Reset kontrolera dotyku.
pub const RST: i32 = 9;
/// Przerwanie kontrolera dotyku. Pin RTC — może w przyszłości budzić z deep sleepu.
pub const INT: i32 = 3;

/// Identyfikator produktu, cztery bajty ASCII: `911\0`.
const REG_PRODUCT_ID: u16 = 0x8140;
/// Stan bufora: bit 7 = są nowe dane, bity 0-3 = liczba punktów.
const REG_STATUS: u16 = 0x814E;
/// Pierwszy punkt dotyku, 8 bajtów.
const REG_POINT1: u16 = 0x8150;

const STATUS_READY: u8 = 0x80;
const STATUS_COUNT: u8 = 0x0F;

// ---------------------------------------------------------------------------
// Osie kontrolera względem panelu — ZAŁOŻENIE, NIE POMIAR
// ---------------------------------------------------------------------------
// Zakładamy, że GT911 raportuje w tym samym układzie, w którym panel skanuje:
// `x` wzdłuż 960 pikseli, `y` wzdłuż 540. Oba układy są przylepione do tego samego
// szkła, więc jest to najbardziej prawdopodobne — ale nie zostało sprawdzone.
//
// Poprawka po pierwszym uruchomieniu sprowadza się do przestawienia tych trzech
// stałych i nie wymaga ruszania niczego poza tym plikiem. `interactive_loop` wypisuje
// przy każdym dotknięciu surowe współrzędne panelu obok przeliczonych na płótno,
// więc dotknięcie czterech rogów wystarczy, żeby je ustawić:
//
//   lewy górny róg zwraca ~(0, 0)          -> wszystko false, tak jak teraz
//   lewy górny róg zwraca ~(0, 540)        -> FLIP_Y
//   lewy górny róg zwraca ~(960, 0)        -> FLIP_X
//   x rośnie przy ruchu w dół ekranu       -> SWAP_XY
const SWAP_XY: bool = false;
const FLIP_X: bool = false;
const FLIP_Y: bool = false;

/// Co kontroler ma do powiedzenia.
///
/// Rozróżnienie `Idle` od `Up` nie jest pedanterią. Kontroler raportuje palec
/// **przy każdym cyklu skanowania**, więc bez wykrywania zbocza przytrzymanie
/// klawisza wpisałoby go kilkadziesiąt razy. Zbocze da się wykryć tylko wtedy, gdy
/// „nie mam nic nowego" jest czymś innym niż „palec zdjęty".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
    /// Bufor pusty — od ostatniego odczytu nic się nie wydarzyło.
    Idle,
    /// Palec na szkle, w tym miejscu.
    Down(TouchPoint),
    /// Palec zdjęty.
    Up,
}

/// Punkt dotyku we współrzędnych **panelu** (960×540), nie płótna.
///
/// Przeliczenie na płótno robi [`dashboard::Rotation::panel_to_canvas`] — ta sama
/// macierz, którą pakowanie stosuje w drugą stronę.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TouchPoint {
    pub x: i32,
    pub y: i32,
}

pub struct Gt911 {
    dev: I2cDevice,
}

impl Gt911 {
    /// Przeprowadza sekwencję resetu i dołącza urządzenie do magistrali.
    ///
    /// Woła się to **przy każdym wybudzeniu**, przed pierwszym odczytem.
    pub fn new(bus: &I2cBus) -> Result<Self> {
        reset_sequence();

        // Najpierw adres, o który prosiliśmy sekwencją resetu; potem zapasowy.
        // Identyfikator jest jedynym sposobem odróżnienia „układ odpowiada" od
        // „pod tym adresem siedzi coś innego".
        for address in [ADDRESS, ADDRESS_ALT] {
            let Ok(dev) = bus.device(address) else {
                continue;
            };
            let touch = Self { dev };
            let Ok(id) = touch.product_id() else {
                continue;
            };
            if &id[..3] != b"911" {
                warn!(
                    "pod 0x{address:02X} odpowiada coś, co przedstawia się jako {:?}",
                    String::from_utf8_lossy(&id)
                );
                continue;
            }

            if address == ADDRESS_ALT {
                warn!(
                    "GT911 odpowiada pod adresem zapasowym 0x{ADDRESS_ALT:02X} — sekwencja \
                     resetu ustawia INT odwrotnie, niż zakłada. Dotyk działa, ale warto \
                     to poprawić w board::gt911::reset_sequence"
                );
            }
            return Ok(touch);
        }

        bail!("GT911 nie odpowiada ani pod 0x{ADDRESS:02X}, ani pod 0x{ADDRESS_ALT:02X}")
    }

    /// Cztery bajty identyfikatora produktu.
    pub fn product_id(&self) -> Result<[u8; 4]> {
        let mut buf = [0u8; 4];
        self.read_at(REG_PRODUCT_ID, &mut buf)?;
        Ok(buf)
    }

    /// Odczytuje stan dotyku.
    ///
    /// [`Report::Idle`] jest stanem normalnym i wołający odpytuje w pętli.
    ///
    /// # Orientacja osi jest ZAŁOŻENIEM
    ///
    /// Zakładamy, że GT911 raportuje w tym samym układzie, w którym panel skanuje:
    /// `x` wzdłuż 960 pikseli, `y` wzdłuż 540. Jest to najczęstsze ustawienie i zgadza
    /// się z tym, że oba układy są przylepione do tego samego szkła — ale nie zostało
    /// sprawdzone na sztuce. Jeśli po pierwszym uruchomieniu dotyk trafia w lustrzane
    /// odbicie albo w zamienione osie, poprawka jest jednoliniowa i **należy do tego
    /// miejsca**, nie do układu graficznego: przelicznik na płótno jest już wspólny
    /// dla dotyku i dla pakowania.
    pub fn read(&self) -> Result<Report> {
        let mut status = [0u8; 1];
        self.read_at(REG_STATUS, &mut status)?;
        let status = status[0];

        if status & STATUS_READY == 0 {
            return Ok(Report::Idle);
        }

        let report = if status & STATUS_COUNT == 0 {
            Report::Up
        } else {
            let mut raw = [0u8; 8];
            self.read_at(REG_POINT1, &mut raw)?;
            // Bajt 0 to identyfikator śledzenia, współrzędne zaczynają się od 1.
            Report::Down(orient(
                u16::from_le_bytes([raw[1], raw[2]]) as i32,
                u16::from_le_bytes([raw[3], raw[4]]) as i32,
            ))
        };

        // Flagę trzeba skasować, inaczej kontroler nie zgłosi kolejnego dotknięcia.
        // Robimy to ZAWSZE — także przy podniesieniu palca.
        self.write_at(REG_STATUS, 0)?;

        Ok(report)
    }
}

impl Gt911 {
    fn read_at(&self, reg: u16, buf: &mut [u8]) -> Result<()> {
        let addr = reg.to_be_bytes();
        self.dev.write_read(&addr, buf)
    }

    fn write_at(&self, reg: u16, value: u8) -> Result<()> {
        let addr = reg.to_be_bytes();
        self.dev.write(&[addr[0], addr[1], value])
    }
}

/// Sekwencja resetu ustawiająca adres `0x5D`.
///
/// Czasy są z noty katalogowej: `RST` w dole przez >1 ms, `INT` ustawiony przed
/// zwolnieniem `RST` i utrzymany przez >5 ms po nim, potem >50 ms na wstanie
/// firmware'u kontrolera. Bierzemy z zapasem — to jest raz na wybudzenie.
fn reset_sequence() {
    // SAFETY: oba piny należą do nas; epdiy używa 4-8, 15-18, 41, 42, 45, 48 i I²C
    // 39/40, więc GPIO3 i GPIO9 nie kolidują z niczym.
    unsafe {
        for pin in [RST, INT] {
            sys::gpio_reset_pin(pin);
            sys::gpio_set_direction(pin, sys::gpio_mode_t_GPIO_MODE_OUTPUT);
        }

        // Kontroler w reset, INT w dole -> po zwolnieniu resetu adres to 0x5D.
        sys::gpio_set_level(RST, 0);
        sys::gpio_set_level(INT, 0);
        delay_ms(10);

        sys::gpio_set_level(RST, 1);
        delay_ms(10);

        // INT wraca do roli wejścia; od tej chwili to kontroler nim steruje.
        sys::gpio_set_direction(INT, sys::gpio_mode_t_GPIO_MODE_INPUT);
        sys::gpio_set_pull_mode(INT, sys::gpio_pull_mode_t_GPIO_FLOATING);
        delay_ms(60);
    }
}

/// Sprowadza współrzędne kontrolera do układu panelu — patrz `SWAP_XY` i spółka.
fn orient(x: i32, y: i32) -> TouchPoint {
    let (mut x, mut y) = if SWAP_XY { (y, x) } else { (x, y) };
    if FLIP_X {
        x = dashboard::PANEL_WIDTH as i32 - 1 - x;
    }
    if FLIP_Y {
        y = dashboard::PANEL_HEIGHT as i32 - 1 - y;
    }
    TouchPoint { x, y }
}

fn delay_ms(ms: u32) {
    std::thread::sleep(std::time::Duration::from_millis(ms as u64));
}

/// Otwiera kontroler dotyku, nie przerywając bootu, jeśli go nie ma.
///
/// Urządzenie bez dotyku jest bezużyteczne, ale **wstające** urządzenie bez dotyku
/// da się zdiagnozować z logu; padnięty boot nie mówi nic. Przy zimnym starcie
/// wypisujemy identyfikator, bo to jedyny sygnał z bring-upu tego układu.
pub fn open(bus: &I2cBus, verbose: bool) -> Option<Gt911> {
    match Gt911::new(bus) {
        Ok(touch) => {
            if verbose {
                match touch.product_id() {
                    Ok(id) => info!(
                        "GT911: {}",
                        String::from_utf8_lossy(&id).trim_end_matches('\0')
                    ),
                    Err(e) => warn!("GT911 odpowiedział, ale identyfikator nie: {e:#}"),
                }
            }
            Some(touch)
        }
        Err(e) => {
            warn!("GT911 niedostępny — dotyk nie zadziała: {e:#}");
            None
        }
    }
}
