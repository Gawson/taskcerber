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

    /// Pobiera wydarzenia z okna `[from, to)`.
    fn fetch(&self, from: NaiveDateTime, to: NaiveDateTime) -> Result<FetchResult>;

    /// Ile dni do przodu ma sens pobierać z TEGO źródła.
    ///
    /// Horyzont jest własnością źródła, a nie globalną stałą, bo źródła różnią się
    /// o rząd wielkości. Kalendarz z treścią daje przy roku ~1500 wydarzeń i ponad
    /// 100 KB w PSRAM-ie, więc trzyma się dwóch tygodni. Kanał świąt to ~13 wydarzeń
    /// całodniowych rocznie, bez reguł powtarzania — pobieranie go na czternaście dni
    /// znaczyłoby, że kalendarz roczny pokazuje święta z dwóch tygodni i pusty listopad.
    fn horizon_days(&self) -> i64;
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
