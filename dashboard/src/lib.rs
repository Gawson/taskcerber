//! Renderowanie dashboardu na panel ED047TC1 (16 odcieni szarości).
//!
//! Płótno jest **pionowe** (540×960) — tak, jak stoi urządzenie. Panel skanuje
//! poziomo (960×540); obrót robi [`Gray8::pack4`] przy pakowaniu do framebufferu.
//!
//! Ten crate **nie wie nic o ESP-IDF ani o sprzęcie**. Kompiluje się identycznie na
//! hoście i na `xtensa-esp32s3-espidf`, dzięki czemu układ graficzny rozwija się w
//! pętli `cargo run -p preview` (sekundy), a nie `build → flash → patrz na ścianę`
//! (minuty).
//!
//! ```no_run
//! use dashboard::{Fonts, Gray8, Model, Rotation, render};
//! # use chrono::NaiveDate;
//! let fonts = Fonts::embedded();
//! let mut canvas = Gray8::new(Rotation::default());
//! let now = NaiveDate::from_ymd_opt(2026, 8, 18).unwrap().and_hms_opt(12, 0, 0).unwrap();
//! let screen = render(&Model::empty(now), &fonts, &mut canvas);
//! let framebuffer = canvas.to_packed(); // 259 200 B, wprost do epd_hl_get_framebuffer()
//! ```

pub mod canvas;
pub mod hit;
pub mod layout;
pub mod model;
pub mod setup;
pub mod shapes;
pub mod text;

pub use canvas::{Gray8, Rect, Rotation, PACKED_LEN, PANEL_HEIGHT, PANEL_WIDTH};
pub use hit::{Action, HitRegion, Screen};
pub use layout::{render, render_setup};
pub use model::{Battery, CalEvent, DayGroup, Model, NetState, SourceTag, Tile};
pub use setup::{Applied, Caps, Field, Setup};
pub use text::{Align, Fonts, Weight};
