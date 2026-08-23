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
//! Sekwencję trzeba powtarzać **przy każdym wybudzeniu**: sekwencja wyłączania
//! zatrzaskuje `T_RST` (GPIO9) w dole, żeby kontroler nie pobierał prądu przez sen.
//! Zatrzask trzeba najpierw zdjąć — robi to `power::shutdown::release_pin_holds`
//! na starcie i jeszcze raz [`reset_sequence`], bo ani `gpio_reset_pin`, ani
//! `gpio_set_level` same go nie ruszają.
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
/// Początek tablicy konfiguracji; pod tym adresem siedzi jej WERSJA.
const REG_CONFIG: u16 = 0x8047;
/// `Module_Switch1` — bity 1:0 to tryb zgłaszania przerwania.
const REG_INT_MODE: u16 = 0x804D;
/// Suma kontrolna tablicy konfiguracji.
const REG_CONFIG_CHECKSUM: u16 = 0x80FF;
/// Zapis `1` tutaj każe kontrolerowi przyjąć nową konfigurację.
const REG_CONFIG_FRESH: u16 = 0x8100;
/// Ile bajtów obejmuje suma kontrolna: od `REG_CONFIG` do `REG_CONFIG_CHECKSUM - 1`.
const CONFIG_LEN: usize = (REG_CONFIG_CHECKSUM - REG_CONFIG) as usize;

/// Tryb przerwania: poziom niski utrzymany, dopóki palec dotyka.
///
/// Nie zbocze. Maska wybudzenia `ext1` próbkuje poziom przy zasypianiu i budzi na
/// stanie niskim — krótki impuls zboczowy potrafi się w to okno nie trafić, poziom
/// utrzymany trafia zawsze. Tę samą wartość ustawia firmware producenta.
const INT_MODE_LOW_LEVEL: u8 = 0b10;

const REG_PRODUCT_ID: u16 = 0x8140;
/// Stan bufora: bit 7 = są nowe dane, bity 0-3 = liczba punktów.
const REG_STATUS: u16 = 0x814E;
/// Pierwszy punkt dotyku, 8 bajtów: `[track ID, x lo, x hi, y lo, y hi, size lo,
/// size hi, rezerwa]`.
///
/// **To jest `0x814F`, nie `0x8150`.** Bloki punktów zaczynają się bezpośrednio
/// za rejestrem stanu i kolejne leżą co 8 bajtów (`0x8157`, `0x815F`, ...). Zgodnie
/// mówią o tym: `GT911Constants.h` z SensorLib vendora (`GT911_POINT_1 (0X814F)`),
/// `TouchDrvGT911::getPoint`, który czyta 39 bajtów od `{0x81, 0x4F}`, oraz crate
/// `lilygo-t5s3paperpro` (`ed047tc1.rs:65`).
///
/// Przesunięcie o jeden bajt nie daje ciszy ani błędu — daje **liczby**. Dotknięcie
/// (100, 200) czyta się wtedy jako (51200, ~15000): każde trafienie wypada poza
/// każdy obszar `Screen::hit`, więc dotyk wygląda na całkowicie martwy, mimo że
/// kontroler odpowiada i zgłasza palec.
const REG_POINT1: u16 = 0x814F;

/// Rozdzielczość skonfigurowana w kontrolerze: X pod `0x8146`, Y pod `0x8148`,
/// oba little-endian. Rejestry informacyjne, tylko do odczytu.
const REG_X_RESOLUTION: u16 = 0x8146;
const REG_Y_RESOLUTION: u16 = 0x8148;

const STATUS_READY: u8 = 0x80;
const STATUS_COUNT: u8 = 0x0F;

// ---------------------------------------------------------------------------
// Osie kontrolera względem panelu — BIERZEMY Z KONTROLERA, NIE ZGADUJEMY
// ---------------------------------------------------------------------------
// GT911 sam mówi, w jakim układzie liczy: rejestry `0x8146`/`0x8148` niosą
// rozdzielczość wpisaną w jego konfigurację. Na tej płytce spotykane są obie
// wartości i to jest jedyna rzecz, która rozstrzyga obrót:
//
//   960 x 540  -> kontroler liczy tak jak skanuje panel, nic nie przeliczamy
//   540 x 960  -> kontroler jest zamontowany o 90° i trzeba obrócić
//
// Nie jest to nasz wynalazek. Crate `lilygo-t5s3paperpro` dla dokładnie tej płytki
// ma na to osobną gałąź włączaną tym samym warunkiem (`src/ed047tc1.rs`, funkcja
// czytająca punkty): przy `(540, 960)` liczy `x = raw_y`, `y = 540 - 1 - raw_x`,
// a poza tym skaluje surowe wartości do wymiarów panelu. Robimy to samo, bo dzięki
// temu **ta sama binarka działa na obu wariantach** i nie ma czego zgadywać.
//
// Lustra z rozdzielczości nie wynikają — sam obrót nie mówi, czy szkło jest
// przyklejone tą, czy drugą stroną. Te dwie stałe zostają na wypadek, gdyby
// dotknięcie rogu pokazało odbicie; `interactive_loop` wypisuje przy każdym
// dotknięciu surowe współrzędne panelu obok przeliczonych na płótno:
//
//   lewy górny róg zwraca ~(0, 0)     -> nic nie zmieniaj
//   lewy górny róg zwraca ~(0, 540)   -> FLIP_Y
//   lewy górny róg zwraca ~(960, 0)   -> FLIP_X
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
    /// Rozdzielczość z konfiguracji kontrolera; `(0, 0)`, gdy odczyt się nie udał.
    /// Decyduje o obrocie w [`Gt911::orient`].
    resolution: (u16, u16),
}

// Kontroler jest oddawany na własność wątkowi czytającemu dotyk. Magistralę I²C
// dzieli wtedy z epdiy, które chodzi z wątku głównego — i to jest bezpieczne, bo
// sterownik `i2c_master` z ESP-IDF serializuje transakcje semaforem `bus_lock_mux`
// (`components/esp_driver_i2c/i2c_master.c`), niezależnie od tego, z ilu zadań
// przychodzą.
unsafe impl Send for Gt911 {}

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
            let mut touch = Self {
                dev,
                resolution: (0, 0),
            };
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

            // Rozdzielczość rozstrzyga obrót osi, więc czytamy ją raz, tutaj.
            // Nieudany odczyt zostawia `(0, 0)`, czyli przeliczanie tożsamościowe —
            // to samo, co robił firmware przed tą zmianą.
            match touch.read_resolution() {
                Ok(res) => touch.resolution = res,
                Err(e) => warn!("GT911: nie mogę odczytać rozdzielczości: {e:#}"),
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

    /// Rozdzielczość odczytana przy otwarciu kontrolera. `(0, 0)` = nieznana.
    pub fn resolution(&self) -> (u16, u16) {
        self.resolution
    }

    /// Czy kontroler jest zamontowany obrócony o 90° względem skanu panelu.
    pub fn rotated(&self) -> bool {
        let (x, y) = self.resolution;
        x > 0 && y > 0 && x < y
    }

    fn read_resolution(&self) -> Result<(u16, u16)> {
        let mut buf = [0u8; 2];
        self.read_at(REG_X_RESOLUTION, &mut buf)?;
        let x = u16::from_le_bytes(buf);
        self.read_at(REG_Y_RESOLUTION, &mut buf)?;
        Ok((x, u16::from_le_bytes(buf)))
    }

    /// Odczytuje stan dotyku.
    ///
    /// [`Report::Idle`] jest stanem normalnym i wołający odpytuje w pętli.
    ///
    /// # Orientacja osi
    ///
    /// Zwracany punkt jest już w układzie **panelu** (960×540), niezależnie od tego,
    /// jak liczy sam kontroler — obrót bierze się z jego rozdzielczości, patrz
    /// [`Gt911::orient`]. Przeliczenie panel → płótno robi dopiero
    /// `Rotation::panel_to_canvas`, tą samą macierzą, którą pakowanie stosuje
    /// w drugą stronę.
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
            Report::Down(self.orient(
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
    /// Ustawia zgłaszanie przerwania na „poziom niski, dopóki trwa dotyk".
    ///
    /// # Po co, skoro nagłówek tego pliku mówi, żeby konfiguracji nie ruszać
    ///
    /// Bo bez tego **wybudzanie dotykiem nie może działać** i nie działało: maska
    /// `ext1` czeka na stan niski na `T_INT`, a kontroler nikt nie poprosił, żeby ten
    /// stan kiedykolwiek wystawił. Płaciliśmy więc za trzymanie kontrolera przy życiu
    /// przez cały sen i nie dostawaliśmy z tego nic.
    ///
    /// # Dlaczego to jest bezpieczniejsze, niż brzmi
    ///
    /// **Nie ruszamy wersji konfiguracji** pod `REG_CONFIG`. Kontroler zapisuje
    /// tablicę do swojej pamięci trwałej tylko wtedy, gdy podana wersja jest wyższa
    /// od bieżącej; przy tej samej wersji zmiana żyje w RAM-ie i znika przy resecie.
    /// A my resetujemy kontroler przy KAŻDYM wybudzeniu ([`reset_sequence`]), więc:
    /// zły zapis kasuje się sam w następnym cyklu, a dobry trzeba odtwarzać za każdym
    /// razem. Oba wnioski prowadzą do tego samego: wołać to po każdym resecie.
    ///
    /// Sumę kontrolną liczymy z tablicy PRZECZYTANEJ z kontrolera, nie z własnej —
    /// dzięki temu nie musimy znać poprawnej zawartości ani jednego innego bajtu.
    ///
    /// Błąd na dowolnym kroku jest logowany i **nie przerywa niczego**: dotyk działa
    /// wtedy dalej przez odpytywanie w oknie interaktywnym, tak jak do tej pory.
    pub fn set_interrupt_low_level(&self) -> Result<bool> {
        let mut cfg = [0u8; CONFIG_LEN];
        self.read_at(REG_CONFIG, &mut cfg)?;

        let idx = (REG_INT_MODE - REG_CONFIG) as usize;
        if cfg[idx] & 0b11 == INT_MODE_LOW_LEVEL {
            info!("GT911: tryb przerwania już poprawny (poziom niski)");
            return Ok(false);
        }

        let stary = cfg[idx];
        cfg[idx] = (cfg[idx] & !0b11) | INT_MODE_LOW_LEVEL;

        // Suma kontrolna GT911: dopełnienie do dwóch z sumy bajtów tablicy.
        let suma = cfg.iter().fold(0u8, |a, &b| a.wrapping_add(b));
        let checksum = (!suma).wrapping_add(1);

        self.write_at(REG_INT_MODE, cfg[idx])?;
        self.write_at(REG_CONFIG_CHECKSUM, checksum)?;
        self.write_at(REG_CONFIG_FRESH, 1)?;

        // Odczyt kontrolny. Bez niego nie wiedzielibyśmy, czy kontroler przyjął
        // zapis, czy odrzucił go na sumie — a to jest różnica między działającym
        // wybudzaniem a cichą regresją, którą widać dopiero po dobie na baterii.
        let mut sprawdzenie = [0u8; 1];
        self.read_at(REG_INT_MODE, &mut sprawdzenie)?;
        if sprawdzenie[0] & 0b11 != INT_MODE_LOW_LEVEL {
            bail!(
                "GT911: zapis trybu przerwania odrzucony (0x804D = {:#04X}, chciałem {:#04X})",
                sprawdzenie[0],
                cfg[idx]
            );
        }

        info!(
            "GT911: tryb przerwania {stary:#04X} -> {:#04X} (poziom niski), suma {checksum:#04X}",
            cfg[idx]
        );
        Ok(true)
    }

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
            // Zatrzask z `power::shutdown::set_low_and_hold` przeżywa deep sleep,
            // a `gpio_reset_pin` go NIE zdejmuje — bez tego pad zostaje przybity
            // do zera i kontroler nigdy nie wychodzi z resetu. Główne zdjęcie
            // zatrzasków robi `release_pin_holds` na starcie; tutaj jest drugi raz,
            // bo to ten kod jest właścicielem obu pinów.
            sys::gpio_hold_dis(pin);
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

impl Gt911 {
    /// Sprowadza surowe współrzędne kontrolera do układu panelu (960×540).
    ///
    /// Obrót wynika z rozdzielczości kontrolera, lustra ze stałych `FLIP_*` —
    /// pełne wyjaśnienie przy tych stałych.
    fn orient(&self, raw_x: i32, raw_y: i32) -> TouchPoint {
        const W: i32 = dashboard::PANEL_WIDTH as i32;
        const H: i32 = dashboard::PANEL_HEIGHT as i32;
        let (rx, ry) = (self.resolution.0 as i32, self.resolution.1 as i32);

        let (mut x, mut y) = if rx < 2 || ry < 2 {
            // Rozdzielczości nie znamy — bierzemy surowe wartości, tak jak wcześniej.
            (raw_x, raw_y)
        } else if rx < ry {
            // Kontroler zamontowany o 90°: jego `y` biegnie wzdłuż długiego boku.
            (scale(raw_y, ry, W), scale(rx - 1 - raw_x, rx, H))
        } else {
            (scale(raw_x, rx, W), scale(raw_y, ry, H))
        };

        if FLIP_X {
            x = W - 1 - x;
        }
        if FLIP_Y {
            y = H - 1 - y;
        }
        TouchPoint { x, y }
    }
}

/// Przelicza wartość z zakresu `[0, from)` na `[0, to)`.
///
/// Rozdzielczość kontrolera zwykle jest równa wymiarowi panelu i wtedy to jest
/// tożsamość, ale nie ma na to gwarancji — crate vendora skaluje i my też.
fn scale(v: i32, from: i32, to: i32) -> i32 {
    v.clamp(0, from - 1) * (to - 1) / (from - 1)
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
                let (rx, ry) = touch.resolution();
                info!(
                    "GT911: rozdzielczość {rx}x{ry} -> {}",
                    if rx == 0 || ry == 0 {
                        "NIEZNANA, przeliczam tożsamościowo"
                    } else if touch.rotated() {
                        "kontroler obrócony o 90°, przeliczam"
                    } else {
                        "osie jak panel, bez przeliczania"
                    }
                );
            }

            // Tryb przerwania ustawiamy przy KAŻDYM otwarciu, bo `reset_sequence`
            // przy każdym wybudzeniu przywraca konfigurację z pamięci trwałej
            // kontrolera — a my celowo do niej nie piszemy. Bez tego wywołania
            // `T_INT` nigdy nie schodzi w dół i maska `ext1` czeka na sygnał,
            // którego nikt nie wystawia.
            match touch.set_interrupt_low_level() {
                Ok(_) => {}
                Err(e) => warn!(
                    "GT911: nie ustawiłem trybu przerwania — dotyk będzie działał \
                     tylko przez odpytywanie, nie wybudzi ze snu: {e:#}"
                ),
            }

            Some(touch)
        }
        Err(e) => {
            warn!("GT911 niedostępny — dotyk nie zadziała: {e:#}");
            None
        }
    }
}
