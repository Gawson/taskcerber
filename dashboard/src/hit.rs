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
    /// Przełącz na wskazany widok.
    ///
    /// Wskazany, a nie „następny": przy trzech widokach cykl kazałby stukać dwa razy
    /// i płacić dwoma pełnymi odświeżeniami za przejście, które jest jednym ruchem.
    SetView(crate::model::View),

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

/// Kształt, który pod tym obszarem **narysowano**.
///
/// To nie to samo co cel dotykowy i dlatego jest osobno. Cel bywa celowo większy —
/// plakietka statusu ma obszar rozszerzony o 10 px na boki i 6 px w pionie, żeby
/// dawało się w nią trafić palcem. Mignięcie ma odwzorować RYSUNEK, nie cel:
/// odwrócenie prostokąta opisanego zapalało zaokrąglony guzik jako ostry prostokąt,
/// i to o kilkanaście pikseli większy, niż cokolwiek widać.
///
/// Promień jest całkowity, bo `HitRegion` jest `Eq` — a `f32` nie jest. Promienie
/// w tym układzie i tak są całkowite (8 dla plakietki, 14 dla przycisku).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visual {
    pub rect: Rect,
    pub radius: i32,
}

/// Prostokąt reagujący na dotyk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitRegion {
    pub rect: Rect,
    pub action: Action,
    /// Kształt do mignięcia pod palcem. `None` — obszar nie ma własnego rysunku
    /// (np. całe tło zamykające szczegóły) i nie ma czym mignąć.
    pub visual: Option<Visual>,
}

impl HitRegion {
    pub fn new(rect: Rect, action: Action) -> Self {
        Self {
            rect,
            action,
            visual: None,
        }
    }

    /// Obszar dotykowy razem z kształtem, który pod nim narysowano.
    pub fn shaped(rect: Rect, action: Action, visual: Rect, radius: i32) -> Self {
        Self {
            rect,
            action,
            visual: Some(Visual {
                rect: visual,
                radius,
            }),
        }
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
    /// Prostokąt pola, w którym widać wpisywaną wartość — tylko na ekranie
    /// konfiguracji.
    ///
    /// Jest tu, bo to **jedyny** fragment ekranu, który zmienia się przy wpisaniu
    /// znaku. Dzięki temu firmware odświeża po naciśnięciu klawisza dwa małe
    /// prostokąty zamiast całej klatki, a znak pojawia się od razu, zamiast czekać
    /// na pełne przemalowanie ekranu.
    pub edit_box: Option<Rect>,
}

impl Screen {
    /// Znajduje akcję pod podanym punktem.
    ///
    /// Regiony są sprawdzane w kolejności dodania, więc dodane później
    /// (rysowane na wierzchu) mają pierwszeństwo — dlatego iterujemy od tyłu.
    pub fn hit(&self, x: i32, y: i32) -> Option<Action> {
        self.hit_region(x, y).map(|h| h.action)
    }

    /// To samo, ale zwraca cały region — z prostokątem.
    ///
    /// Prostokąt jest potrzebny firmware'owi do natychmiastowego feedbacku: po
    /// trafieniu odrysowuje się sam ten obszar, zanim wykona się akcja pod spodem.
    pub fn hit_region(&self, x: i32, y: i32) -> Option<&HitRegion> {
        self.hits.iter().rev().find(|h| h.contains(x, y))
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
            edit_box: None,
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
