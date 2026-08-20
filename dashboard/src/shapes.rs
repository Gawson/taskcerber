//! Minimalne prymitywy wektorowe z antyaliasingiem.
//!
//! Świadomie ręcznie pisane zamiast `tiny-skia`: cała grafika, jakiej potrzebuje ten
//! dashboard, to prostokąty, linie i zaokrąglone „pigułki". Analityczne pokrycie dla
//! zaokrąglonego prostokąta to kilkadziesiąt linii, a oszczędza dużą zależność, która
//! na Xtensie i tak nie ma SIMD i musiałaby przejść przez `esp-idf-sys`.

use crate::canvas::{Gray8, Rect, BILEVEL_THRESHOLD, BLACK, WHITE};

/// Wypełnia zaokrąglony prostokąt z antyaliasingiem na łukach.
///
/// `radius` jest obcinany do połowy krótszego boku, więc `radius: 9999` daje
/// stadion/pigułkę.
pub fn fill_round_rect(c: &mut Gray8, r: Rect, radius: f32, ink: u8) {
    if r.w <= 0 || r.h <= 0 {
        return;
    }
    let rad = radius.min(r.w as f32 / 2.0).min(r.h as f32 / 2.0).max(0.0);
    if rad <= 0.5 {
        c.fill_rect(r, ink);
        return;
    }

    // Środki czterech łuków.
    let cx0 = r.x as f32 + rad;
    let cx1 = r.right() as f32 - rad;
    let cy0 = r.y as f32 + rad;
    let cy1 = r.bottom() as f32 - rad;

    for y in r.y..r.bottom() {
        let py = y as f32 + 0.5;
        for x in r.x..r.right() {
            let px = x as f32 + 0.5;

            // Odległość do najbliższego środka łuku; poza narożnikami traktujemy
            // punkt jako w pełni wewnątrz.
            let dx = if px < cx0 {
                cx0 - px
            } else if px > cx1 {
                px - cx1
            } else {
                0.0
            };
            let dy = if py < cy0 {
                cy0 - py
            } else if py > cy1 {
                py - cy1
            } else {
                0.0
            };

            let coverage = if dx == 0.0 && dy == 0.0 {
                1.0
            } else {
                // Pokrycie liniowe na szerokości jednego piksela wokół promienia.
                let d = (dx * dx + dy * dy).sqrt();
                (rad + 0.5 - d).clamp(0.0, 1.0)
            };

            c.blend(x, y, ink, coverage);
        }
    }
}

/// Obrys zaokrąglonego prostokąta o grubości `stroke` (do wewnątrz).
pub fn stroke_round_rect(c: &mut Gray8, r: Rect, radius: f32, stroke: i32, ink: u8) {
    if stroke <= 0 || r.w <= 0 || r.h <= 0 {
        return;
    }
    let rad = radius.min(r.w as f32 / 2.0).min(r.h as f32 / 2.0).max(0.0);
    let inner = r.inset(stroke);
    let inner_rad = (rad - stroke as f32).max(0.0);

    let cx0 = r.x as f32 + rad;
    let cx1 = r.right() as f32 - rad;
    let cy0 = r.y as f32 + rad;
    let cy1 = r.bottom() as f32 - rad;

    let icx0 = inner.x as f32 + inner_rad;
    let icx1 = inner.right() as f32 - inner_rad;
    let icy0 = inner.y as f32 + inner_rad;
    let icy1 = inner.bottom() as f32 - inner_rad;

    for y in r.y..r.bottom() {
        let py = y as f32 + 0.5;
        for x in r.x..r.right() {
            let px = x as f32 + 0.5;

            let outer_cov = round_rect_coverage(px, py, cx0, cx1, cy0, cy1, rad);
            if outer_cov <= 0.0 {
                continue;
            }
            let outside_inner = inner.w <= 0
                || inner.h <= 0
                || px < inner.x as f32
                || px > inner.right() as f32
                || py < inner.y as f32
                || py > inner.bottom() as f32;
            let inner_cov = if outside_inner {
                0.0
            } else {
                round_rect_coverage(px, py, icx0, icx1, icy0, icy1, inner_rad)
            };

            let cov = (outer_cov - inner_cov).clamp(0.0, 1.0);
            c.blend(x, y, ink, cov);
        }
    }
}

fn round_rect_coverage(px: f32, py: f32, cx0: f32, cx1: f32, cy0: f32, cy1: f32, rad: f32) -> f32 {
    let dx = if px < cx0 {
        cx0 - px
    } else if px > cx1 {
        px - cx1
    } else {
        0.0
    };
    let dy = if py < cy0 {
        cy0 - py
    } else if py > cy1 {
        py - cy1
    } else {
        0.0
    };
    if dx == 0.0 && dy == 0.0 {
        return 1.0;
    }
    let d = (dx * dx + dy * dy).sqrt();
    (rad + 0.5 - d).clamp(0.0, 1.0)
}

/// Odwraca zawartość ZAOKRĄGLONEGO prostokąta — dwupoziomowo.
///
/// # Dlaczego nie `Gray8::invert_rect`
///
/// Mignięcie pod palcem odwracało prostokąt OPISANY na przycisku, więc zaokrąglony
/// guzik zapalał się jako ostry prostokąt. Kształt musi iść za rysunkiem, a rysunek
/// zna tylko ten, kto go narysował — stąd promień wędruje w [`crate::hit::Visual`].
///
/// # Dlaczego DWUPOZIOMOWO, a nie „255 minus wartość"
///
/// To jest drugi błąd w tym samym miejscu i mniej oczywisty. Płótno agendy przechodzi
/// przez `Gray8::quantize_ink`, więc atrament leży na poziomach 0-4. Odwrócenie przez
/// odejmowanie przenosi je na poziomy 11-15 — czyli **dokładnie w pasmo, którego panel
/// nie odróżnia od bieli**. Antyaliasowane krawędzie liter w negatywie po prostu
/// znikały, a litera robiła się rozmyta i za gruba.
///
/// Nie da się tego naprawić lepszym odwzorowaniem: na czarnym tle jasne półtony
/// musiałyby leżeć blisko bieli, a tam ten panel nie ma ani jednego użytecznego
/// stopnia. **W negatywie nie ma antyaliasingu, bo nie ma go z czego zrobić.**
/// Maska narożnika też jest twarda, z tego samego powodu.
pub fn invert_round_rect(c: &mut Gray8, r: Rect, radius: f32) {
    if r.w <= 0 || r.h <= 0 {
        return;
    }
    let rad = radius.min(r.w as f32 / 2.0).min(r.h as f32 / 2.0).max(0.0);
    let cx0 = r.x as f32 + rad;
    let cx1 = r.right() as f32 - rad;
    let cy0 = r.y as f32 + rad;
    let cy1 = r.bottom() as f32 - rad;

    for y in r.y..r.bottom() {
        let py = y as f32 + 0.5;
        for x in r.x..r.right() {
            let px = x as f32 + 0.5;
            if round_rect_coverage(px, py, cx0, cx1, cy0, cy1, rad) < 0.5 {
                continue;
            }
            let v = c.get(x, y);
            c.set(x, y, if v < BILEVEL_THRESHOLD { WHITE } else { BLACK });
        }
    }
}

/// Wypełnione koło z antyaliasingiem.
pub fn fill_circle(c: &mut Gray8, cx: f32, cy: f32, radius: f32, ink: u8) {
    if radius <= 0.0 {
        return;
    }
    let x0 = (cx - radius - 1.0).floor() as i32;
    let x1 = (cx + radius + 1.0).ceil() as i32;
    let y0 = (cy - radius - 1.0).floor() as i32;
    let y1 = (cy + radius + 1.0).ceil() as i32;

    for y in y0..y1 {
        for x in x0..x1 {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            let cov = (radius + 0.5 - d).clamp(0.0, 1.0);
            c.blend(x, y, ink, cov);
        }
    }
}

/// Odcinek o zadanej grubości, z antyaliasingiem.
///
/// Liczy odległość punktu od odcinka analitycznie — dla kilku krótkich kresek
/// na ekran to tańsze niż rasteryzator, a wynik jest gładki.
pub fn thick_line(c: &mut Gray8, x0: f32, y0: f32, x1: f32, y1: f32, thickness: f32, ink: u8) {
    let half = thickness / 2.0;
    let min_x = (x0.min(x1) - half - 1.0).floor() as i32;
    let max_x = (x0.max(x1) + half + 1.0).ceil() as i32;
    let min_y = (y0.min(y1) - half - 1.0).floor() as i32;
    let max_y = (y0.max(y1) + half + 1.0).ceil() as i32;

    let dx = x1 - x0;
    let dy = y1 - y0;
    let len_sq = dx * dx + dy * dy;

    for y in min_y..max_y {
        for x in min_x..max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            // Rzut punktu na odcinek, obcięty do jego końców.
            let t = if len_sq <= f32::EPSILON {
                0.0
            } else {
                (((px - x0) * dx + (py - y0) * dy) / len_sq).clamp(0.0, 1.0)
            };
            let cx = x0 + t * dx;
            let cy = y0 + t * dy;
            let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();

            let cov = (half + 0.5 - d).clamp(0.0, 1.0);
            c.blend(x, y, ink, cov);
        }
    }
}

/// Strzałka „w lewo" jako kształt wektorowy.
///
/// Świadomie nie glif: **Noto Sans nie ma znaków U+2190/U+2192**. Wypisane przez
/// krój dają poprawną szerokość i zero pikseli, czyli pusty przycisk — awaria cicha
/// i łatwa do przeoczenia. Kształt nie ma tego problemu.
pub fn chevron_left(c: &mut Gray8, cx: f32, cy: f32, size: f32, thickness: f32, ink: u8) {
    let h = size / 2.0;
    thick_line(c, cx + h * 0.55, cy - h, cx - h * 0.45, cy, thickness, ink);
    thick_line(c, cx - h * 0.45, cy, cx + h * 0.55, cy + h, thickness, ink);
}

/// Strzałka „w prawo" jako kształt wektorowy.
pub fn chevron_right(c: &mut Gray8, cx: f32, cy: f32, size: f32, thickness: f32, ink: u8) {
    let h = size / 2.0;
    thick_line(c, cx - h * 0.55, cy - h, cx + h * 0.45, cy, thickness, ink);
    thick_line(c, cx + h * 0.45, cy, cx - h * 0.55, cy + h, thickness, ink);
}

/// Pozioma linia o grubości `thickness` pikseli, rysowana w dół od `y`.
pub fn hline(c: &mut Gray8, x: i32, y: i32, w: i32, thickness: i32, ink: u8) {
    c.fill_rect(Rect::new(x, y, w, thickness.max(1)), ink);
}

/// Pionowa linia o grubości `thickness` pikseli, rysowana w prawo od `x`.
pub fn vline(c: &mut Gray8, x: i32, y: i32, h: i32, thickness: i32, ink: u8) {
    c.fill_rect(Rect::new(x, y, thickness.max(1), h), ink);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{BLACK, WHITE};

    #[test]
    fn pigulka_ma_puste_narozniki_i_pelny_srodek() {
        let mut c = Gray8::new(crate::canvas::Rotation::default());
        let r = Rect::new(100, 100, 200, 60);
        fill_round_rect(&mut c, r, 9999.0, BLACK);

        // Środek pełny.
        assert_eq!(c.get(200, 130), BLACK);
        // Lewy górny narożnik pigułki musi zostać papierem.
        assert_eq!(c.get(101, 101), WHITE);
        // Skrajnie lewy punkt na osi pionowej środka jest zamalowany.
        assert!(c.get(101, 130) < 40);
    }

    #[test]
    fn zerowy_promien_degeneruje_do_prostokata() {
        let mut c = Gray8::new(crate::canvas::Rotation::default());
        fill_round_rect(&mut c, Rect::new(10, 10, 20, 20), 0.0, BLACK);
        assert_eq!(c.get(10, 10), BLACK);
        assert_eq!(c.get(29, 29), BLACK);
    }

    #[test]
    fn obrys_zostawia_wnetrze_nietkniete() {
        let mut c = Gray8::new(crate::canvas::Rotation::default());
        stroke_round_rect(&mut c, Rect::new(50, 50, 100, 100), 12.0, 3, BLACK);
        assert_eq!(c.get(100, 100), WHITE, "wnętrze obrysu ma zostać papierem");
        assert!(c.get(100, 51) < 60, "górna krawędź ma być zamalowana");
    }

    #[test]
    fn strzalki_zostawiaja_atrament() {
        // Regresja: te kształty zastąpiły glify U+2190/U+2192, których Noto Sans
        // nie zawiera — wcześniej dawały pusty przycisk.
        for f in [
            chevron_left as fn(&mut Gray8, f32, f32, f32, f32, u8),
            chevron_right,
        ] {
            let mut c = Gray8::new(crate::canvas::Rotation::default());
            f(&mut c, 100.0, 100.0, 24.0, 3.0, BLACK);
            let ink = c.pixels().iter().filter(|&&p| p < 128).count();
            assert!(ink > 40, "strzałka narysowała tylko {ink} pikseli");
        }
    }

    #[test]
    fn odcinek_o_zerowej_dlugosci_nie_panikuje() {
        let mut c = Gray8::new(crate::canvas::Rotation::default());
        thick_line(&mut c, 50.0, 50.0, 50.0, 50.0, 4.0, BLACK);
        assert!(c.get(50, 50) < 200, "punktowy odcinek ma zostawić kropkę");
    }

    #[test]
    fn ksztalty_poza_plotnem_nie_panikuja() {
        let mut c = Gray8::new(crate::canvas::Rotation::default());
        fill_round_rect(&mut c, Rect::new(-50, -50, 100, 100), 20.0, BLACK);
        fill_circle(&mut c, 955.0, 535.0, 40.0, BLACK);
        fill_circle(&mut c, -10.0, -10.0, 5.0, BLACK);
        thick_line(&mut c, -50.0, -50.0, 2000.0, 2000.0, 5.0, BLACK);
        chevron_left(&mut c, 5.0, 5.0, 40.0, 3.0, BLACK);
    }
}
