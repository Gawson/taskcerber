//! Źródła danych.
//!
//! [`EventSource`] to szew, przez który wymienia się sposób dostępu do kalendarza.
//! W wersji pierwszej jest to prywatny URL iCal (zero OAuth, zero ekranu zgody).
//! Przejście na OAuth2 to dodanie drugiej implementacji tego traitu — reszta
//! firmware'u się nie zmienia.

pub mod ics;

use anyhow::Result;
use chrono::NaiveDateTime;
use dashboard::model::{CalEvent, Tile};

/// Źródło wydarzeń kalendarzowych.
pub trait EventSource {
    /// Nazwa do logów i do ekranu błędu.
    fn name(&self) -> &str;

    /// Ściąga surową treść kanału do pamięci i liczy jej CRC. **Nie parsuje.**
    ///
    /// Rozdzielenie pobrania od parsowania jest celowe i ma trzy powody:
    ///
    /// 1. **Radio schodzi przed parsowaniem.** Dziś antena stoi podniesiona przez całe
    ///    mielenie reguł — przy kanale 1,18 MB to ~9,5 s. Stąd krok sieciowy kosztuje
    ///    ~1160 mAs zamiast budżetowanych 360 i stąd modem śpi 30% czasu zamiast ~75%.
    /// 2. **CRC znamy PRZED parsowaniem.** Jeśli kanał się nie zmienił, parsowania nie
    ///    ma po co robić w ogóle — treść leży w migawce. Licząc CRC w locie ze strumienia
    ///    dowiadywaliśmy się o tym dopiero po fakcie, czyli po zapłaceniu.
    /// 3. Bufor idzie do PSRAM-u (8 MB, `SPIRAM_MALLOC_ALWAYSINTERNAL = 4096`), a nie
    ///    do wewnętrznego DRAM-u, o który biją się mbedTLS i epdiy.
    fn download(&self) -> Result<Downloaded>;

    /// Parsuje wcześniej pobraną treść. Wołane już BEZ radia.
    fn parse(&self, body: &[u8], from: NaiveDateTime, to: NaiveDateTime) -> Result<Vec<CalEvent>>;

    /// Ile dni do przodu ma sens pobierać z TEGO źródła.
    ///
    /// Horyzont jest własnością źródła, a nie globalną stałą, bo źródła różnią się
    /// o rząd wielkości. Kalendarz z treścią daje przy roku ~1500 wydarzeń i ponad
    /// 100 KB w PSRAM-ie, więc trzyma się dwóch tygodni. Kanał świąt to ~13 wydarzeń
    /// całodniowych rocznie, bez reguł powtarzania — pobieranie go na czternaście dni
    /// znaczyłoby, że kalendarz roczny pokazuje święta z dwóch tygodni i pusty listopad.
    fn horizon_days(&self) -> i64;
}

/// Surowa treść kanału razem z sumą kontrolną.
pub struct Downloaded {
    /// Ciało odpowiedzi. Przy rozmiarach rzędu megabajta leży w PSRAM-ie.
    pub body: Vec<u8>,
    /// CRC32 treści — liczone w trakcie pobierania, bez parsowania.
    pub content_crc: u32,
}

/// Wynik pobrania.
pub struct FetchResult {
    pub events: Vec<CalEvent>,
    /// CRC treści przed parsowaniem — do pominięcia odświeżenia panelu,
    /// gdy nic się nie zmieniło.
    pub content_crc: u32,
    /// Ile bajtów faktycznie przeszło przez radio.
    pub bytes: usize,
}

/// Źródło dowolnych wartości do kafelków w stopce — miejsce na „kilka innych".
pub trait TileSource {
    fn name(&self) -> &str;
    fn fetch(&self) -> Result<Vec<Tile>>;
}
