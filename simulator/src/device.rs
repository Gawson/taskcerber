//! Model urządzenia: stan, który na prawdziwej płytce siedzi w NVS i pamięci RTC,
//! plus symulacja zachowania panelu e-papierowego.
//!
//! Celem jest, żeby symulator **kłamał tylko tam, gdzie musi**. Render jest ten sam
//! co na urządzeniu. Kwantyzacja do 16 poziomów jest ta sama. Czasy odświeżania
//! odpowiadają zmierzonym na panelu ED047TC1. Różni się to, czego z definicji nie
//! da się odtworzyć: prawdziwy prąd, prawdziwy dotyk, prawdziwe duchy.

use std::time::{Duration, Instant};

use chrono::NaiveDateTime;
use dashboard::model::{Battery, DayGroup, NetState};
use dashboard::{Action, Fonts, Gray8, Model, Rotation, Screen};

/// Tryb odświeżania, ten sam podział co w firmware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refresh {
    /// Pełne, 16 odcieni. Na panelu ~1,0–1,5 s, z widocznym miganiem.
    Full,
    /// Szybkie, dwupoziomowe. ~0,2–0,35 s, zostawia duchy.
    Fast,
}

impl Refresh {
    /// Czas trwania odświeżenia na prawdziwym panelu.
    pub fn duration(self) -> Duration {
        match self {
            // Wartości z pomiarów społeczności dla ED047TC1; producent podaje 630 ms,
            // realia są bliżej 1,5 s.
            Refresh::Full => Duration::from_millis(1200),
            Refresh::Fast => Duration::from_millis(280),
        }
    }
}

/// Faza animacji odświeżania — odtwarza to, co widać na szkle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    /// Pełne odświeżenie: panel miga na czarno i biało, zanim pokaże treść.
    Flashing,
    /// Treść już się wywołuje, ale panel jeszcze się ustala.
    Settling,
}

/// Ile szybkich odświeżeń przed wymuszeniem pełnego — jak w firmware.
const FAST_BEFORE_FULL: u8 = 12;

pub struct Device {
    pub model: Model,
    pub screen: Screen,
    fonts: Fonts<'static>,

    /// Bufor tego, co faktycznie „świeci" na panelu.
    panel: Gray8,
    /// Bufor przygotowany do wypchnięcia.
    pending: Gray8,

    refresh_started: Option<Instant>,
    refresh_mode: Refresh,
    fast_count: u8,

    /// Symulacja duchów po szybkim odświeżaniu.
    pub simulate_ghosting: bool,
    ghost: Option<Gray8>,

    pub stats: Stats,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub renders: u32,
    pub full_refreshes: u32,
    pub fast_refreshes: u32,
    pub last_render_us: u128,
}

impl Device {
    pub fn new(model: Model, rotation: Rotation) -> Self {
        let fonts = Fonts::embedded();
        let mut dev = Self {
            model,
            screen: Screen::default(),
            fonts,
            panel: Gray8::new(rotation),
            pending: Gray8::new(rotation),
            refresh_started: None,
            refresh_mode: Refresh::Full,
            fast_count: 0,
            simulate_ghosting: true,
            ghost: None,
            stats: Stats::default(),
        };
        dev.render_and_push(Refresh::Full);
        dev
    }

    /// Szerokość panelu w bieżącej orientacji.
    pub fn width(&self) -> usize {
        self.panel.width()
    }

    /// Wysokość panelu w bieżącej orientacji.
    pub fn height(&self) -> usize {
        self.panel.height()
    }

    /// Renderuje bieżący model i rozpoczyna odświeżanie panelu.
    pub fn render_and_push(&mut self, mode: Refresh) {
        let started = Instant::now();
        self.screen = dashboard::render(&self.model, &self.fonts, &mut self.pending);
        self.stats.last_render_us = started.elapsed().as_micros();
        self.stats.renders += 1;

        // Panel pokazuje 16 poziomów — kwantyzujemy dokładnie tak jak `pack4`.
        self.pending.quantize16();

        // Wymuszenie pełnego odświeżenia po serii szybkich, jak w firmware.
        let mode = if self.fast_count >= FAST_BEFORE_FULL {
            Refresh::Full
        } else {
            mode
        };

        match mode {
            Refresh::Full => {
                self.fast_count = 0;
                self.ghost = None;
                self.stats.full_refreshes += 1;
            }
            Refresh::Fast => {
                self.fast_count += 1;
                self.stats.fast_refreshes += 1;
                if self.simulate_ghosting {
                    // Zapamiętaj poprzednią zawartość jako źródło duchów.
                    let mut g = Gray8::new(self.panel.rotation());
                    g.pixels_mut().copy_from_slice(self.panel.pixels());
                    self.ghost = Some(g);
                }
            }
        }

        self.refresh_mode = mode;
        self.refresh_started = Some(started);
    }

    /// Odświeża bez pobierania — np. po zmianie strony.
    pub fn repaint(&mut self) {
        self.render_and_push(Refresh::Fast);
    }

    /// Postępuje o klatkę animacji. Zwraca `true`, gdy obraz się zmienił.
    pub fn tick(&mut self) -> bool {
        let Some(started) = self.refresh_started else {
            return false;
        };
        let elapsed = started.elapsed();
        let total = self.refresh_mode.duration();

        if elapsed >= total {
            self.panel
                .pixels_mut()
                .copy_from_slice(self.pending.pixels());
            self.apply_ghosting();
            self.refresh_started = None;
            return true;
        }
        true
    }

    /// Nakłada duchy po szybkim odświeżaniu — im więcej ich z rzędu, tym mocniejsze.
    fn apply_ghosting(&mut self) {
        if self.refresh_mode != Refresh::Fast {
            return;
        }
        let Some(ghost) = &self.ghost else { return };

        // Siła rośnie z liczbą szybkich odświeżeń; po pełnym wraca do zera.
        let strength = (self.fast_count as f32 / FAST_BEFORE_FULL as f32).min(1.0) * 0.18;
        if strength <= 0.0 {
            return;
        }

        let ghost_px: Vec<u8> = ghost.pixels().to_vec();
        for (dst, &src) in self.panel.pixels_mut().iter_mut().zip(ghost_px.iter()) {
            // Duch pojawia się tam, gdzie poprzednio był atrament, a teraz jest papier.
            if src < 128 && *dst > 200 {
                let v = *dst as f32 * (1.0 - strength) + 128.0 * strength;
                *dst = v as u8;
            }
        }
    }

    /// Bieżąca faza odświeżania, do rysowania animacji.
    pub fn phase(&self) -> Phase {
        let Some(started) = self.refresh_started else {
            return Phase::Idle;
        };
        let elapsed = started.elapsed();
        let total = self.refresh_mode.duration();

        match self.refresh_mode {
            // Pełne odświeżenie ED047TC1 przechodzi serię inwersji przed ustaleniem.
            Refresh::Full if elapsed < total.mul_f32(0.65) => Phase::Flashing,
            _ if elapsed < total => Phase::Settling,
            _ => Phase::Idle,
        }
    }

    /// Postęp bieżącego odświeżania, 0.0..=1.0.
    pub fn progress(&self) -> f32 {
        let Some(started) = self.refresh_started else {
            return 1.0;
        };
        let total = self.refresh_mode.duration().as_secs_f32();
        (started.elapsed().as_secs_f32() / total).min(1.0)
    }

    /// Zawartość panelu do wyświetlenia.
    pub fn panel(&self) -> &Gray8 {
        &self.panel
    }

    pub fn refresh_mode(&self) -> Refresh {
        self.refresh_mode
    }

    /// Obsługuje dotknięcie w podanym punkcie panelu.
    ///
    /// Zwraca akcję, jeśli w coś trafiono. Logika jest identyczna z tą, którą
    /// firmware zastosuje do zdarzeń z GT911.
    pub fn touch(&mut self, x: i32, y: i32) -> Option<Action> {
        // W trakcie odświeżania panel nie reaguje — tak samo jak prawdziwy.
        if self.refresh_started.is_some() {
            return None;
        }

        let action = self.screen.hit(x, y)?;
        self.apply(action);
        Some(action)
    }

    /// Stosuje akcję do modelu i odświeża.
    pub fn apply(&mut self, action: Action) {
        match action {
            Action::NextPage => {
                if self.model.page + 1 < self.screen.pages {
                    self.model.page += 1;
                    self.repaint();
                }
            }
            Action::PrevPage => {
                if self.model.page > 0 {
                    self.model.page -= 1;
                    self.repaint();
                }
            }
            Action::ShowEvent(i) => {
                self.model.focus = Some(i);
                self.repaint();
            }
            Action::Back => {
                if self.model.focus.take().is_some() {
                    self.repaint();
                }
            }
            Action::RefreshNow => {
                self.model.focus = None;
                self.render_and_push(Refresh::Full);
            }
        }
    }

    /// Podmienia dane kalendarza, zachowując resztę stanu.
    pub fn set_events(&mut self, days: Vec<DayGroup>, now: NaiveDateTime, net: NetState) {
        self.model.days = days;
        self.model.now = now;
        self.model.net = net;
        self.model.page = 0;
        self.model.focus = None;
        self.render_and_push(Refresh::Full);
    }

    pub fn set_battery(&mut self, b: Battery) {
        self.model.battery = b;
    }

    pub fn set_net(&mut self, net: NetState) {
        self.model.net = net;
        self.render_and_push(Refresh::Fast);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use dashboard::model::{CalEvent, SourceTag};

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, 18)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    fn model_with_events(n: usize) -> Model {
        let mut m = Model::empty(now());
        let date = NaiveDate::from_ymd_opt(2026, 8, 18).unwrap();
        m.days = vec![DayGroup {
            date,
            events: (0..n)
                .map(|i| CalEvent {
                    start: date.and_hms_opt(8 + (i as u32 % 12), 0, 0).unwrap(),
                    end: date.and_hms_opt(9 + (i as u32 % 12), 0, 0).unwrap(),
                    all_day: false,
                    title: format!("Wydarzenie {i}"),
                    location: None,
                    source: SourceTag::Primary,
                })
                .collect(),
        }];
        m
    }

    #[test]
    fn dotkniecie_wydarzenia_otwiera_szczegoly() {
        let mut dev = Device::new(model_with_events(3), Rotation::default());
        // Poczekaj na koniec odświeżania startowego.
        std::thread::sleep(Refresh::Full.duration());
        dev.tick();

        let hit = dev
            .screen
            .hits
            .iter()
            .find(|h| matches!(h.action, Action::ShowEvent(_)))
            .copied()
            .expect("wydarzenie ma być dotykalne");

        let action = dev.touch(hit.rect.x + 20, hit.rect.y + 10);
        assert!(matches!(action, Some(Action::ShowEvent(_))));
        assert!(
            dev.model.focus.is_some(),
            "dotknięcie ma otworzyć szczegóły"
        );
    }

    #[test]
    fn powrot_zamyka_szczegoly() {
        let mut dev = Device::new(model_with_events(3), Rotation::default());
        std::thread::sleep(Refresh::Full.duration());
        dev.tick();

        dev.apply(Action::ShowEvent(0));
        assert_eq!(dev.model.focus, Some(0));

        std::thread::sleep(Refresh::Fast.duration());
        dev.tick();

        dev.apply(Action::Back);
        assert_eq!(dev.model.focus, None);
    }

    #[test]
    fn panel_nie_reaguje_w_trakcie_odswiezania() {
        let mut dev = Device::new(model_with_events(3), Rotation::default());
        // Zaraz po utworzeniu trwa pełne odświeżenie.
        assert_eq!(
            dev.touch(400, 200),
            None,
            "panel w trakcie odświeżania ma nie reagować"
        );
    }

    #[test]
    fn strony_nie_wychodza_poza_zakres() {
        let mut dev = Device::new(model_with_events(60), Rotation::default());
        std::thread::sleep(Refresh::Full.duration());
        dev.tick();

        let pages = dev.screen.pages;
        assert!(pages > 1);

        for _ in 0..pages + 10 {
            dev.apply(Action::NextPage);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            dev.model.page < pages,
            "strona wyszła poza zakres: {}",
            dev.model.page
        );

        for _ in 0..pages + 10 {
            dev.apply(Action::PrevPage);
        }
        assert_eq!(dev.model.page, 0);
    }

    #[test]
    fn pelne_odswiezenie_kasuje_licznik_szybkich() {
        let mut dev = Device::new(model_with_events(3), Rotation::default());
        for _ in 0..3 {
            dev.render_and_push(Refresh::Fast);
        }
        assert!(dev.stats.fast_refreshes >= 3);
        dev.render_and_push(Refresh::Full);
        assert_eq!(dev.fast_count, 0);
    }

    #[test]
    fn po_serii_szybkich_wymuszane_jest_pelne() {
        let mut dev = Device::new(model_with_events(3), Rotation::default());
        for _ in 0..FAST_BEFORE_FULL + 1 {
            dev.render_and_push(Refresh::Fast);
        }
        assert_eq!(
            dev.refresh_mode(),
            Refresh::Full,
            "po {FAST_BEFORE_FULL} szybkich odświeżeniach ma wejść pełne"
        );
    }
}
