//! Pobieranie i parsowanie kanałów iCalendar dla urządzeń o ograniczonej pamięci.
//!
//! Crate celowo **nie zależy od ESP-IDF** — dzięki temu najtrudniejsza logika
//! projektu (składanie linii, strefy czasowe, rozwijanie reguł powtarzania,
//! nadpisania pojedynczych wystąpień) jest testowalna na hoście, na prawdziwych
//! plikach `.ics`, zamiast być pisana w ciemno i debugowana przez patrzenie
//! na ścianę.

pub mod dt;
pub mod feed;
pub mod parser;

pub use feed::{parse_feed, FeedError, Window};
pub use parser::{Property, PropertyReader};
