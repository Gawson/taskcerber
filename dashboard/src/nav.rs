//! Pasek zakładek — jedyna droga między widokami.
//!
//! # Dlaczego zakładki, a nie cykl
//!
//! Przełączanie „następny widok" jednym przyciskiem jest tańsze w kodzie i gorsze
//! w użyciu: żeby z agendy dostać się do roku, trzeba stuknąć dwa razy i **za każdym
//! stuknięciem zapłacić pełnym odświeżeniem**, bo widok zmienia całą treść ekranu.
//! Przy trzech widokach cykl kosztuje średnio jedno zbędne przemalowanie na przejście.
//!
//! Zakładki kosztują za to miejsce — pasek zabiera 66 px w pionie, czyli mniej więcej
//! jeden wiersz agendy. To dobry interes: wiersz agendy pokazuje jedno wydarzenie,
//! a pasek mówi, gdzie się jest i dokąd można pójść, na każdym ekranie.
//!
//! # Aktywna zakładka jest w negatywie
//!
//! Z tego samego powodu co dzisiejszy dzień w widoku miesięcznym: ton ma na tym panelu
//! cztery użyteczne stopnie i wszystkie są zajęte przez treść. Negatyw jest jedynym
//! wyróżnieniem, które działa pewnie z dystansu. Napis w negatywie idzie `Bold` —
//! reguła 4 z nagłówka [`crate::layout`].
//!
//! # Cel dotykowy
//!
//! Segment ma 180 × 66 px w pionie i 320 × 54 px w poziomie. Obie wartości są grubo
//! ponad progiem palca (~44 px), i to celowo: pasek jest przy krawędzi, gdzie kciuk
//! trafia najmniej dokładnie.

use crate::canvas::{Gray8, Rect, BLACK, INK_DIM, WHITE};
use crate::hit::{Action, HitRegion, Screen};
use crate::layout::TEXT_LEAD;
use crate::model::View;
use crate::shapes::hline;
use crate::text::{Align, Fonts, Weight};

/// Wysokość paska zakładek.
///
/// W poziomie niższy, bo tam wysokości jest o 420 px mniej i każdy piksel wraca
/// do treści — ta sama zasada, która rządzi `Geom::footer_h`.
pub fn tabs_h(c: &Gray8) -> i32 {
    if c.rotation().is_portrait() {
        66
    } else {
        54
    }
}

/// Rysuje pasek zakładek przy dolnej krawędzi i dopisuje obszary dotykowe.
pub fn draw_tabs(active: View, fonts: &Fonts, c: &mut Gray8, screen: &mut Screen) {
    let h = tabs_h(c);
    let w = c.width() as i32;
    let top = c.height() as i32 - h;

    hline(c, 0, top, w, 2, BLACK);

    let n = View::ALL.len() as i32;
    for (i, view) in View::ALL.iter().enumerate() {
        let i = i as i32;
        // Ostatni segment dobiera resztę z dzielenia, żeby pasek kończył się dokładnie
        // na krawędzi. Przy 540 px i trzech zakładkach dzieli się równo, ale przy
        // 960 px w poziomie zostaje piksel — i bez tego widać go jako szczerbę.
        let x = i * w / n;
        let seg_w = (i + 1) * w / n - x;
        let seg = Rect::new(x, top + 2, seg_w, h - 2);

        if *view == active {
            c.fill_rect(seg, BLACK);
        } else if i > 0 {
            // Kreska tylko między dwoma nieaktywnymi: przy wypełnionym sąsiedzie
            // krawędź plamy sama rozdziela segmenty, a dodatkowy włos na czerni
            // i tak by zniknął.
            let poprzedni_aktywny = View::ALL[(i - 1) as usize] == active;
            if !poprzedni_aktywny {
                c.fill_rect(Rect::new(x, top + 12, 1, h - 24), INK_DIM);
            }
        }

        fonts.draw(
            c,
            view.label(),
            (x + seg_w / 2) as f32,
            (top + h / 2 + 10) as f32,
            TEXT_LEAD,
            if *view == active {
                Weight::Bold
            } else {
                Weight::Medium
            },
            if *view == active { WHITE } else { BLACK },
            Align::Center,
        );

        // Aktywna zakładka też jest klikalna i to nie jest przeoczenie: stuknięcie
        // w nią wraca ze szczegółów wydarzenia do listy, więc „jestem tutaj" i „wróć
        // na górę tego widoku" to ten sam gest.
        screen
            .hits
            .push(HitRegion::new(seg, Action::SetView(*view)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Rotation;

    #[test]
    fn segmenty_pokrywaja_caly_pas_bez_szczerb() {
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            let mut s = Screen::default();
            draw_tabs(View::Agenda, &Fonts::embedded(), &mut c, &mut s);
            let mut kraw = vec![];
            for hr in &s.hits {
                kraw.push((hr.rect.x, hr.rect.right()));
            }
            kraw.sort();
            assert_eq!(kraw[0].0, 0, "{rot:?}: pasek nie zaczyna się na krawędzi");
            assert_eq!(
                kraw.last().unwrap().1,
                c.width() as i32,
                "{rot:?}: pasek nie kończy się na krawędzi"
            );
            for para in kraw.windows(2) {
                assert_eq!(para[0].1, para[1].0, "{rot:?}: szczerba między segmentami");
            }
        }
    }

    #[test]
    fn kazdy_widok_ma_swoj_cel_i_jest_on_dosc_duzy() {
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            let mut s = Screen::default();
            draw_tabs(View::Month, &Fonts::embedded(), &mut c, &mut s);
            assert_eq!(s.hits.len(), View::ALL.len());
            for hr in &s.hits {
                assert!(hr.rect.h >= 44, "{rot:?}: cel {} px za niski", hr.rect.h);
                assert!(hr.rect.w >= 44, "{rot:?}: cel {} px za wąski", hr.rect.w);
            }
            for view in View::ALL {
                assert!(
                    s.hits.iter().any(|h| h.action == Action::SetView(view)),
                    "{rot:?}: brak zakładki {view:?}"
                );
            }
        }
    }

    /// Dyspozytor musi naprawdę zmieniać treść, a nie tylko podświetlać zakładkę.
    /// Bez tego testu regresja „render zawsze rysuje agendę" przechodzi niezauważona,
    /// bo pasek na dole wygląda wtedy poprawnie.
    #[test]
    fn render_rysuje_ten_widok_ktory_jest_w_modelu() {
        use crate::model::Model;
        let fonts = Fonts::embedded();
        let mut m = Model::empty(
            chrono::NaiveDate::from_ymd_opt(2026, 8, 18)
                .unwrap()
                .and_hms_opt(7, 15, 0)
                .unwrap(),
        );

        let mut odciski = Vec::new();
        for view in View::ALL {
            m.view = view;
            let mut c = Gray8::new(Rotation::Portrait);
            let screen = crate::layout::render(&m, &fonts, &mut c);

            // Zakładka do KAŻDEGO widoku, na każdym widoku — inaczej jakiś ekran
            // byłby ślepą uliczką na urządzeniu bez przycisków.
            for cel in View::ALL {
                assert!(
                    screen.hits.iter().any(|h| h.action == Action::SetView(cel)),
                    "{view:?}: brak drogi do {cel:?}"
                );
            }

            // Treść nad paskiem, bez samego paska — ten jest z definicji podobny.
            let do_pasa = (c.height() as i32 - tabs_h(&c)) as usize * c.width();
            let atrament: usize = (0..do_pasa).filter(|&i| c.pixels()[i] != WHITE).count();
            odciski.push((view, atrament));
        }

        for para in odciski.windows(2) {
            assert_ne!(
                para[0].1, para[1].1,
                "{:?} i {:?} dają identyczną treść — dyspozytor nie przełącza",
                para[0].0, para[1].0
            );
        }
    }

    /// Aktywna zakładka musi być w negatywie — inaczej nie widać, gdzie się jest.
    #[test]
    fn aktywna_zakladka_jest_zalana_atramentem() {
        let mut c = Gray8::new(Rotation::Portrait);
        let mut s = Screen::default();
        draw_tabs(View::Year, &Fonts::embedded(), &mut c, &mut s);
        let seg = s
            .hits
            .iter()
            .find(|h| h.action == Action::SetView(View::Year))
            .expect("zakładka roku")
            .rect;
        let czarne = (seg.y..seg.bottom())
            .flat_map(|y| (seg.x..seg.right()).map(move |x| (x, y)))
            .filter(|&(x, y)| c.get(x, y) == BLACK)
            .count();
        let pole = (seg.w * seg.h) as usize;
        assert!(
            czarne * 2 > pole,
            "aktywna zakładka pokryta w {}%, oczekiwano ponad połowy",
            czarne * 100 / pole
        );
    }
}
