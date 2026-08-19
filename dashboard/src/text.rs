//! Rasteryzacja tekstu z antyaliasingiem, na `ab_glyph`.
//!
//! Dlaczego nie `u8g2-fonts`: to bitmapy 1-bitowe. Panel ma 16 odcieni przy ~234 PPI —
//! renderowanie tekstu bez antyaliasingu wyrzuca do kosza jedyną rzecz, za którą się
//! na tym wyświetlaczu płaci.
//!
//! Dlaczego nie `fontdue`: `Font::from_bytes()` dekoduje zachłannie każdy glif
//! (`vec::from_elem(Glyph::default(), glyph_count)`), co przy pełnym kroju to megabajty
//! RAM-u. `ab_glyph` trzyma ~4 KB na krój i parsuje leniwie — a tytuły z kalendarza
//! Google mogą zawierać dowolny znak, więc krój musi być pełny, nie podzbiór runtime'owy.

use ab_glyph::{Font, FontRef, Glyph, PxScale, ScaleFont};

use crate::canvas::Gray8;

/// Waga kroju. Trzy warianty, bo więcej i tak nie jest czytelne na e-papierze.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weight {
    Regular,
    Medium,
    Bold,
}

/// Wyrównanie poziome względem podanego `x`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Kroje wbudowane w binarkę.
///
/// Podzbiór Latin + Latin Extended-A (czyli komplet polskich znaków) — ~30 KB na krój
/// zamiast ~620 KB pełnego Noto Sans. Skrypt generujący: `tools/subset-fonts.sh`.
pub struct Fonts<'a> {
    regular: FontRef<'a>,
    medium: FontRef<'a>,
    bold: FontRef<'a>,
}

static REGULAR_TTF: &[u8] = include_bytes!("../fonts/NotoSans-Regular.subset.ttf");
static MEDIUM_TTF: &[u8] = include_bytes!("../fonts/NotoSans-Medium.subset.ttf");
static BOLD_TTF: &[u8] = include_bytes!("../fonts/NotoSans-Bold.subset.ttf");

impl Fonts<'static> {
    /// Ładuje wbudowane kroje.
    ///
    /// # Panics
    /// Tylko jeśli wbudowane pliki są uszkodzone, co jest błędem builda, nie runtime'u.
    pub fn embedded() -> Self {
        Self {
            regular: FontRef::try_from_slice(REGULAR_TTF).expect("wbudowany Noto Sans Regular"),
            medium: FontRef::try_from_slice(MEDIUM_TTF).expect("wbudowany Noto Sans Medium"),
            bold: FontRef::try_from_slice(BOLD_TTF).expect("wbudowany Noto Sans Bold"),
        }
    }
}

impl<'a> Fonts<'a> {
    fn face(&self, w: Weight) -> &FontRef<'a> {
        match w {
            Weight::Regular => &self.regular,
            Weight::Medium => &self.medium,
            Weight::Bold => &self.bold,
        }
    }

    /// Czy krój ma glif dla tego znaku.
    ///
    /// **Brakujący glif rysuje się jako NIC** — nie ma tofu, nie ma pustego
    /// prostokąta, nie ma ostrzeżenia. `ab_glyph` zwraca `None` z `outline_glyph`
    /// i pętla rysująca po prostu nic nie robi, więc klawisz wygląda na pusty,
    /// a układ jest poprawny co do piksela.
    ///
    /// To nie jest hipotetyczne: `tools/subset-fonts.sh` wymienia w zakresie
    /// `U+2190-2193` (strzałki), ale źródłowy Noto Sans ich nie ma, więc pyftsubset
    /// je po cichu pomija. Klawisz kasowania z napisem „←" był pustym prostokątem.
    /// Pilnuje tego teraz test `wszystkie_znaki_interfejsu_maja_glify`.
    pub fn has_glyph(&self, ch: char, weight: Weight) -> bool {
        use ab_glyph::Font as _;
        self.face(weight).glyph_id(ch).0 != 0
    }

    /// Szerokość napisu w pikselach, z kerningiem — bez rysowania czegokolwiek.
    pub fn measure(&self, s: &str, size: f32, weight: Weight) -> f32 {
        let scaled = self.face(weight).as_scaled(PxScale::from(size));
        let mut width = 0.0;
        let mut prev = None;
        for ch in s.chars() {
            let g = scaled.scaled_glyph(ch);
            if let Some(p) = prev {
                width += scaled.kern(p, g.id);
            }
            width += scaled.h_advance(g.id);
            prev = Some(g.id);
        }
        width
    }

    /// Wysokość linii (ascent - descent + line gap) dla danego rozmiaru.
    pub fn line_height(&self, size: f32, weight: Weight) -> f32 {
        let scaled = self.face(weight).as_scaled(PxScale::from(size));
        scaled.height() + scaled.line_gap()
    }

    /// Odległość od góry pola tekstowego do linii bazowej.
    pub fn ascent(&self, size: f32, weight: Weight) -> f32 {
        self.face(weight).as_scaled(PxScale::from(size)).ascent()
    }

    /// Rysuje jedną linię tekstu. `y` to **linia bazowa**, nie góra.
    ///
    /// Zwraca szerokość narysowanego tekstu.
    // Osiem parametrów to dużo, ale każdy jest niezależnym wymiarem typografii
    // i opakowanie ich w strukturę stylu dodałoby ceremonii w każdym miejscu
    // wywołania bez zysku na czytelności.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        c: &mut Gray8,
        s: &str,
        x: f32,
        baseline: f32,
        size: f32,
        weight: Weight,
        ink: u8,
        align: Align,
    ) -> f32 {
        let width = self.measure(s, size, weight);
        let start_x = match align {
            Align::Left => x,
            Align::Center => x - width / 2.0,
            Align::Right => x - width,
        };

        let font = self.face(weight);
        let scaled = font.as_scaled(PxScale::from(size));
        let mut pen = start_x;
        let mut prev = None;

        for ch in s.chars() {
            let mut g: Glyph = scaled.scaled_glyph(ch);
            if let Some(p) = prev {
                pen += scaled.kern(p, g.id);
            }
            let advance = scaled.h_advance(g.id);
            g.position = ab_glyph::point(pen, baseline);

            if let Some(outlined) = font.outline_glyph(g.clone()) {
                let bounds = outlined.px_bounds();
                let ox = bounds.min.x as i32;
                let oy = bounds.min.y as i32;
                outlined.draw(|gx, gy, coverage| {
                    c.blend(ox + gx as i32, oy + gy as i32, ink, coverage);
                });
            }

            pen += advance;
            prev = Some(g.id);
        }

        width
    }

    /// Skraca napis tak, by zmieścił się w `max_width`, dopisując wielokropek.
    ///
    /// Iteruje po `char`, nie po bajtach — inaczej „Śniadanie" ucięte w złym miejscu
    /// to panika na granicy UTF-8, a nie brzydki napis.
    pub fn truncate(&self, s: &str, max_width: f32, size: f32, weight: Weight) -> String {
        if self.measure(s, size, weight) <= max_width {
            return s.to_string();
        }
        let ellipsis = '…';
        let ell_w = self.measure("…", size, weight);
        if ell_w > max_width {
            return String::new();
        }

        let mut out = String::new();
        let mut width = 0.0;
        let scaled = self.face(weight).as_scaled(PxScale::from(size));
        let mut prev = None;

        for ch in s.chars() {
            let g = scaled.scaled_glyph(ch);
            let mut step = scaled.h_advance(g.id);
            if let Some(p) = prev {
                step += scaled.kern(p, g.id);
            }
            if width + step + ell_w > max_width {
                break;
            }
            width += step;
            out.push(ch);
            prev = Some(g.id);
        }

        // Nie zostawiaj wiszącej spacji przed wielokropkiem.
        while out.ends_with(' ') {
            out.pop();
        }
        out.push(ellipsis);
        out
    }

    /// Łamie tekst na linie mieszczące się w `max_width`, po granicach słów.
    /// Słowo dłuższe niż linia jest skracane wielokropkiem, a nie łamane w środku.
    pub fn wrap(
        &self,
        s: &str,
        max_width: f32,
        size: f32,
        weight: Weight,
        max_lines: usize,
    ) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();

        for word in s.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };

            if self.measure(&candidate, size, weight) <= max_width {
                current = candidate;
                continue;
            }

            if !current.is_empty() {
                lines.push(core::mem::take(&mut current));
                if lines.len() == max_lines {
                    break;
                }
            }

            if self.measure(word, size, weight) <= max_width {
                current = word.to_string();
            } else {
                current = self.truncate(word, max_width, size, weight);
                lines.push(core::mem::take(&mut current));
                if lines.len() == max_lines {
                    break;
                }
            }
        }

        if !current.is_empty() && lines.len() < max_lines {
            lines.push(current);
        }

        // Jeśli coś zostało obcięte, zaznacz to na ostatniej linii.
        if lines.len() == max_lines {
            let consumed: usize = lines.iter().map(|l| l.chars().count()).sum();
            if consumed < s.chars().count().saturating_sub(max_lines) {
                if let Some(last) = lines.last_mut() {
                    *last = self.truncate(&format!("{last} …"), max_width, size, weight);
                }
            }
        }

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::{BLACK, WHITE};

    fn fonts() -> Fonts<'static> {
        Fonts::embedded()
    }

    #[test]
    fn wbudowane_kroje_sie_parsuja() {
        let _ = fonts();
    }

    #[test]
    fn polskie_znaki_maja_niezerowa_szerokosc() {
        let f = fonts();
        for ch in [
            'ą', 'ć', 'ę', 'ł', 'ń', 'ó', 'ś', 'ź', 'ż', 'Ą', 'Ć', 'Ę', 'Ł', 'Ń', 'Ó', 'Ś', 'Ź',
            'Ż',
        ] {
            let w = f.measure(&ch.to_string(), 32.0, Weight::Regular);
            assert!(
                w > 1.0,
                "znak {ch} nie ma szerokości — brakuje go w podzbiorze kroju"
            );
        }
    }

    #[test]
    fn polskie_znaki_faktycznie_zostawiaja_piksele() {
        let f = fonts();
        let mut c = Gray8::new(crate::canvas::Rotation::default());
        f.draw(
            &mut c,
            "ŻÓŁĆ gęślą jaźń",
            20.0,
            60.0,
            40.0,
            Weight::Bold,
            BLACK,
            Align::Left,
        );
        let ink = c.pixels().iter().filter(|&&p| p < 200).count();
        assert!(
            ink > 500,
            "spodziewano się atramentu na płótnie, jest {ink} pikseli"
        );
    }

    #[test]
    fn skracanie_nie_lamie_utf8() {
        let f = fonts();
        // Wąskie pole wymusza cięcie w środku wyrazu z polskimi znakami.
        for w in 10..200 {
            let t = f.truncate(
                "Śniadanie z Łukaszem — ćwiczenia",
                w as f32,
                28.0,
                Weight::Regular,
            );
            assert!(t.is_char_boundary(t.len()));
        }
    }

    #[test]
    fn skracanie_miesci_sie_w_limicie() {
        let f = fonts();
        let max = 180.0;
        let t = f.truncate(
            "Bardzo długi tytuł spotkania który się nie mieści",
            max,
            28.0,
            Weight::Regular,
        );
        assert!(f.measure(&t, 28.0, Weight::Regular) <= max);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn krotki_tekst_nie_jest_ruszany() {
        let f = fonts();
        let t = f.truncate("OK", 500.0, 28.0, Weight::Regular);
        assert_eq!(t, "OK");
    }

    #[test]
    fn zawijanie_respektuje_szerokosc_i_limit_linii() {
        let f = fonts();
        let lines = f.wrap(
            "Spotkanie zespołu w sprawie planu na przyszły kwartał i budżetu",
            300.0,
            24.0,
            Weight::Regular,
            3,
        );
        assert!(!lines.is_empty());
        assert!(lines.len() <= 3);
        for l in &lines {
            assert!(
                f.measure(l, 24.0, Weight::Regular) <= 300.0 + 1.0,
                "linia za szeroka: {l}"
            );
        }
    }

    #[test]
    fn wyrownanie_do_prawej_konczy_sie_na_x() {
        let f = fonts();
        let mut c = Gray8::new(crate::canvas::Rotation::default());
        f.draw(
            &mut c,
            "12:30",
            900.0,
            100.0,
            30.0,
            Weight::Medium,
            BLACK,
            Align::Right,
        );
        // Na prawo od punktu kotwiczenia nie powinno być atramentu.
        for y in 60..110 {
            for x in 905..960 {
                assert_eq!(c.get(x, y), WHITE);
            }
        }
    }
}
