//! Model interakcji dotykowej.
//!
//! Renderowanie zwraca listę obszarów reagujących na dotyk. Dzięki temu symulator
//! (mysz) i firmware (GT911) używają **tych samych** regionów — nie ma dwóch
//! implementacji, które mogą się rozjechać, i nie da się dodać przycisku w układzie
//! bez tego, żeby zadziałał w obu miejscach.

use crate::canvas::Rect;
use crate::setup::Field;

/// Co się dzieje po dotknięciu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Wymuś pobranie i odświeżenie teraz.
    RefreshNow,
    /// Następna strona agendy.
    NextPage,
    /// Poprzednia strona agendy.
    PrevPage,
    /// Rozwiń szczegóły wydarzenia o podanym indeksie globalnym.
    ShowEvent(usize),
    /// Wróć do widoku agendy.
    Back,
    /// Otwórz ekran konfiguracji.
    OpenSetup,

    // --- ekran konfiguracji -------------------------------------------------
    // Znak jest w akcji, a nie w indeksie klawisza, i to jest celowe: układ
    // klawiatury zmienia się między stronami i przy `⇧`, a obszar dotykowy ma
    // znaczyć to, co widać na klawiszu w momencie rysowania. Indeks wymagałby,
    // żeby odbiorca znał układ — czyli drugiej kopii tej samej wiedzy.
    /// Dopisz znak do edytowanego pola.
    Key(char),
    /// Skasuj ostatni znak.
    Backspace,
    /// Przełącz `⇧`: wyłączony -> jednorazowy -> blokada.
    Caps,
    /// Przełącz stronę klawiatury (litery / cyfry i symbole).
    KeyPage,
    /// Przejdź do edycji wskazanego pola.
    Focus(Field),
    /// Zapisz konfigurację i wyjdź.
    Save,
}

/// Prostokąt reagujący na dotyk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitRegion {
    pub rect: Rect,
    pub action: Action,
}

impl HitRegion {
    pub fn new(rect: Rect, action: Action) -> Self {
        Self { rect, action }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.rect.x && x < self.rect.right() && y >= self.rect.y && y < self.rect.bottom()
    }
}

/// Wynik renderowania: co narysowano i w co można dotknąć.
#[derive(Debug, Clone, Default)]
pub struct Screen {
    pub hits: Vec<HitRegion>,
    /// Ile stron ma agenda przy obecnych danych.
    pub pages: usize,
    /// Która strona jest pokazana (liczona od zera).
    pub page: usize,
}

impl Screen {
    /// Znajduje akcję pod podanym punktem.
    ///
    /// Regiony są sprawdzane w kolejności dodania, więc dodane później
    /// (rysowane na wierzchu) mają pierwszeństwo — dlatego iterujemy od tyłu.
    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        self.hits
            .iter()
            .rev()
            .find(|h| h.contains(x, y))
            .map(|h| h.action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trafienie_w_obszar() {
        let h = HitRegion::new(Rect::new(10, 10, 100, 50), Action::RefreshNow);
        assert!(h.contains(10, 10));
        assert!(h.contains(109, 59));
        assert!(!h.contains(110, 30), "prawa krawędź jest wyłączna");
        assert!(!h.contains(9, 30));
        assert!(!h.contains(50, 60));
    }

    #[test]
    fn wierzchni_region_wygrywa() {
        let screen = Screen {
            hits: vec![
                HitRegion::new(Rect::new(0, 0, 960, 540), Action::Back),
                HitRegion::new(Rect::new(100, 100, 50, 50), Action::RefreshNow),
            ],
            pages: 1,
            page: 0,
        };
        assert_eq!(screen.hit(120, 120), Some(Action::RefreshNow));
        assert_eq!(screen.hit(10, 10), Some(Action::Back));
        assert_eq!(screen.hit(-5, -5), None);
    }

    #[test]
    fn pusty_ekran_nie_trafia() {
        let screen = Screen::default();
        assert_eq!(screen.hit(100, 100), None);
    }
}
