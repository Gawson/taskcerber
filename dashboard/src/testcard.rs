//! Karta tonów — jedyny sposób, żeby ustalić, co ten panel NAPRAWDĘ pokazuje.
//!
//! # Po co
//!
//! Płótno ma 8 bitów na piksel, `pack4` obcina to do 4 bitów, a panel deklaruje
//! „16 odcieni szarości". Z tego nie wynika, że szesnaście poziomów jest
//! **rozróżnialnych**, ani że są rozłożone równomiernie. Zawiesina elektroforetyczna
//! porusza się nieliniowo, a waveform producenta jest kompromisem dobranym pod
//! zdjęcia, nie pod interfejs z cienkimi kreskami.
//!
//! Objaw, od którego się to zaczęło: elementy rysowane `GRAY_20` (`0x33`, czyli
//! wartość, która powinna wyjść prawie czarna) wyglądały na szkle jak jasnoszary
//! artefakt. Dopóki nie wiadomo, które poziomy są widoczne, każdy wybór odcienia
//! w `layout` jest zgadywaniem.
//!
//! # Co karta rozstrzyga
//!
//! 1. **Drabina 16 poziomów** — w trzech postaciach naraz, bo panel traktuje je
//!    zupełnie inaczej: pełna plama, tekst (cienkie kreski) i linie 1/2/4 px.
//!    Poziom może być czytelny jako plama i niewidoczny jako litera.
//! 2. **Dither kontra półton** — obok siebie, przy tej samej gęstości czerni.
//!    Dithering używa wyłącznie czerni i bieli, czyli dwóch stanów, które waveform
//!    dowozi pewnie. Jeśli kwadrat ditherowany wygląda równiej niż półton o tej
//!    samej gęstości, to jest odpowiedź na pytanie „czym robić jasne wypełnienia".
//! 3. **Biel po pełnym odświeżeniu kontra biel po częściowym** — dwa pola, z których
//!    jedno zostaje nietknięte, a drugie dostaje impuls DU. Różnica między nimi to
//!    dokładnie ta, przez którą odświeżone prostokąty wyglądają na jaśniejsze od tła.
//!
//! # Jak czytać
//!
//! Karta jest rysowana **bez kwantyzacji** — to jedyne miejsce w projekcie, gdzie
//! półtony mają dojść do panelu takie, jakie są. Wnioski wpisuje się potem do palety
//! w `canvas`, a nie odwrotnie.

use crate::canvas::{dither_rect, Gray8, Rect, BLACK, WHITE};
use crate::shapes::{hline, stroke_round_rect};
use crate::text::{Align, Fonts, Weight};

/// Obrysowuje próbkę czarną kreską 1 px, żeby dało się ją znaleźć także wtedy,
/// gdy jej wypełnienie jest bielą.
fn outline(c: &mut Gray8, r: Rect) {
    hline(c, r.x, r.y, r.w, 1, BLACK);
    hline(c, r.x, r.bottom() - 1, r.w, 1, BLACK);
    for y in r.y..r.bottom() {
        c.set(r.x, y, BLACK);
        c.set(r.right() - 1, y, BLACK);
    }
}

/// Poziom 4-bitowy `n` rozciągnięty na pełny bajt — dokładnie to, co `pack4`
/// zobaczy i odeśle na panel.
fn level_byte(n: u8) -> u8 {
    (n << 4) | n
}

/// Rysuje kartę tonów na całym płótnie.
///
/// Zwraca prostokąt pola „biel po częściowym odświeżeniu", które wołający ma
/// wypchnąć osobno, przez `present_area` — inaczej trzeci pomiar nie powstanie.
pub fn render_test_card(fonts: &Fonts, c: &mut Gray8) -> Rect {
    c.clear(WHITE);
    let w = c.width() as i32;
    let m = 16;

    fonts.draw(
        c,
        "karta tonów",
        m as f32,
        44.0,
        30.0,
        Weight::Bold,
        BLACK,
        Align::Left,
    );
    fonts.draw(
        c,
        "poziom · plama · tekst · linie 1/2/4 px",
        m as f32,
        70.0,
        20.0,
        Weight::Medium,
        BLACK,
        Align::Left,
    );
    hline(c, m, 78, w - 2 * m, 2, BLACK);

    // --- Drabina szesnastu poziomów --------------------------------------
    //
    // Etykieta ZAWSZE czarna. Gdyby była w mierzonym tonie, przy jasnych poziomach
    // nie dałoby się odczytać, który wiersz się właśnie ogląda.
    const ROW_H: i32 = 29;
    let top = 88;
    let patch_x = m + 74;
    let patch_w = 132;
    let text_x = patch_x + patch_w + 14;
    let line_x = text_x + 106;
    let line_w = w - m - line_x;

    for n in 0..16u8 {
        let y = top + n as i32 * ROW_H;
        let ink = level_byte(n);

        fonts.draw(
            c,
            &format!("{n:X}·{ink:02X}"),
            m as f32,
            (y + 20) as f32,
            19.0,
            Weight::Medium,
            BLACK,
            Align::Left,
        );

        // Obrys jest konieczny, nie ozdobny: bez niego jasne poziomy zlewają się
        // z tłem i nie widać, gdzie kończy się próbka, a zaczyna biel kartki.
        let patch = Rect::new(patch_x, y + 2, patch_w, ROW_H - 6);
        c.fill_rect(patch, ink);
        outline(c, patch);

        fonts.draw(
            c,
            "Agmk 24",
            text_x as f32,
            (y + 20) as f32,
            24.0,
            Weight::Medium,
            ink,
            Align::Left,
        );

        hline(c, line_x, y + 6, line_w, 1, ink);
        hline(c, line_x, y + 12, line_w, 2, ink);
        hline(c, line_x, y + 18, line_w, 4, ink);
    }

    // --- Dither kontra półton --------------------------------------------
    let dz_top = top + 16 * ROW_H + 12;
    fonts.draw(
        c,
        "dither / półton — ta sama gęstość",
        m as f32,
        (dz_top + 18) as f32,
        20.0,
        Weight::Medium,
        BLACK,
        Align::Left,
    );

    const CELL: i32 = 52;
    const PAIR_GAP: i32 = 4;
    let pair_w = CELL * 2 + PAIR_GAP;
    let densities: [u8; 8] = [1, 2, 3, 4, 6, 8, 10, 12];
    let per_row = 4;
    let grid_top = dz_top + 30;
    let slack = w - 2 * m - per_row * pair_w;
    let step = pair_w + slack / (per_row - 1).max(1);

    for (i, &d) in densities.iter().enumerate() {
        let col = (i % per_row as usize) as i32;
        let row = (i / per_row as usize) as i32;
        let x = m + col * step;
        let y = grid_top + row * (CELL + 26);

        let left = Rect::new(x, y, CELL, CELL);
        let right = Rect::new(x + CELL + PAIR_GAP, y, CELL, CELL);
        dither_rect(c, left, d);
        // Półton o tej samej gęstości: `d` szesnastych czerni to poziom `16 - d`.
        c.fill_rect(right, level_byte(16 - d));
        outline(c, left);
        outline(c, right);

        fonts.draw(
            c,
            &format!("{d}/16"),
            (x + pair_w / 2) as f32,
            (y + CELL + 19) as f32,
            19.0,
            Weight::Medium,
            BLACK,
            Align::Center,
        );
    }

    // --- Biel pełna kontra biel częściowa --------------------------------
    let wz_top = grid_top + 2 * (CELL + 26) + 10;
    fonts.draw(
        c,
        "biel: całość / fragment",
        m as f32,
        (wz_top + 18) as f32,
        20.0,
        Weight::Medium,
        BLACK,
        Align::Left,
    );

    let box_y = wz_top + 28;
    let box_h = (c.height() as i32 - 10 - box_y).min(70);
    let box_w = (w - 2 * m - 12) / 2;

    // Lewe pole zostaje takie, jakie wyszło z pełnego odświeżenia. Ramka jest po to,
    // żeby było co porównywać wzrokiem — samo pole jest bielą i bez obwódki nie
    // dałoby się go zlokalizować.
    let untouched = Rect::new(m, box_y, box_w, box_h);
    stroke_round_rect(c, untouched, 6.0, 2, BLACK);

    let refreshed = Rect::new(m + box_w + 12, box_y, box_w, box_h);
    stroke_round_rect(c, refreshed, 6.0, 2, BLACK);
    fonts.draw(
        c,
        "DU",
        (refreshed.x + refreshed.w / 2) as f32,
        (refreshed.y + refreshed.h / 2 + 8) as f32,
        22.0,
        Weight::Medium,
        BLACK,
        Align::Center,
    );

    refreshed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Rotation;

    #[test]
    fn dither_ma_zadana_gestosc() {
        // Bayer 4x4 przy prostokącie będącym wielokrotnością 4 daje DOKŁADNIE
        // `level`/16 czarnych pikseli — bez tego porównanie z półtonem nic nie znaczy.
        for level in 0..=16u8 {
            let mut c = Gray8::new(Rotation::Portrait);
            let r = Rect::new(0, 0, 16, 16);
            dither_rect(&mut c, r, level);
            let czarne = (0..16)
                .flat_map(|y| (0..16).map(move |x| (x, y)))
                .filter(|&(x, y)| c.get(x, y) == BLACK)
                .count();
            assert_eq!(
                czarne,
                level as usize * 16,
                "gęstość dla poziomu {level} się nie zgadza"
            );
        }
    }

    #[test]
    fn karta_miesci_sie_w_plotnie_i_nie_jest_pusta() {
        let fonts = Fonts::embedded();
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            let du = render_test_card(&fonts, &mut c);
            assert!(du.x >= 0 && du.right() <= c.width() as i32, "{rot:?}: DU poza płótnem");
            assert!(du.y >= 0 && du.bottom() <= c.height() as i32, "{rot:?}: DU poza płótnem");
            let atrament = c.pixels().iter().filter(|&&p| p != WHITE).count();
            assert!(atrament > 10_000, "{rot:?}: karta wyszła prawie pusta");
        }
    }

    #[test]
    fn wszystkie_szesnascie_poziomow_trafia_na_plotno() {
        let fonts = Fonts::embedded();
        let mut c = Gray8::new(Rotation::Portrait);
        render_test_card(&fonts, &mut c);
        // Każdy poziom 4-bitowy ma się pojawić jako plama — inaczej drabina kłamie.
        for n in 0..16u8 {
            let want = level_byte(n);
            assert!(
                c.pixels().contains(&want),
                "poziomu {n:X} ({want:02X}) nie ma na karcie"
            );
        }
    }
}
