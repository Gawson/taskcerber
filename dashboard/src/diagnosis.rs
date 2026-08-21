//! Ekran diagnozy — dokąd doszedł poprzedni cykl, zanim zamilkł.
//!
//! # Dlaczego to jest osobny ekran, a nie pasek nad agendą
//!
//! Bo cykl, który go maluje, **celowo pomija sieć**. Agenda pokazywałaby wtedy
//! nieaktualne dane obok komunikatu o awarii i trzeba by zgadywać, czy stare jest
//! stare, czy dopiero co pobrane. Ekran, który mówi jedną rzecz, nie ma tego problemu.
//!
//! # Przycisk konfiguracji nie jest dekoracją
//!
//! Najczęstsza awaria to `łączenie z WiFi`, a jej najczęstsza przyczyna to literówka
//! w haśle. Bez wyjścia do konfiguracji z tego ekranu jedyną drogą byłoby trafienie
//! w drobny napis w stopce agendy — na ekranie, którego ten cykl nie rysuje.
//!
//! # Skąd biorą się liczby
//!
//! Z okruszka zapisanego w NVS przez poprzedni cykl, tuż przed wejściem w dany etap.
//! Najważniejszy jest **wolny DRAM**: awaria w TLS-ie wygląda jak zawieszenie, a jest
//! brakiem pamięci — i widać to wyłącznie po tym, ile jej zostawało tuż przed.
//! Reszta uzasadnienia w nagłówku `devlogic::boot`.

use crate::canvas::{Gray8, Rect, BLACK, INK_DIM, WHITE};
use crate::hit::{Action, HitRegion, Screen};
use crate::layout::{TEXT_BODY, TEXT_FLOOR, TEXT_HEAD, TEXT_LEAD, TEXT_TITLE};
use crate::shapes::{hline, stroke_round_rect};
use crate::text::{Align, Fonts, Weight};

/// Co pokazać. Łańcuchy przychodzą gotowe, żeby `dashboard` nie zależał od `devlogic`.
pub struct Diagnosis<'a> {
    /// Nazwa etapu, na którym cykl zamilkł.
    pub step: &'a str,
    /// Zdanie o tym, co najprawdopodobniej jest nie tak.
    pub hint: &'a str,
    /// Ile milisekund od startu minęło, zanim ten etap się zaczął.
    pub ms: u32,
    /// Wolna pamięć wewnętrzna w tej samej chwili.
    pub dram_kb: u16,
    pub firmware: &'a str,
}

const MARGIN: i32 = 32;

pub fn render_diagnosis(d: &Diagnosis, fonts: &Fonts, c: &mut Gray8) -> Screen {
    c.clear(WHITE);
    let mut screen = Screen::default();
    let w = c.width() as i32;
    let h = c.height() as i32;
    let poziomo = !c.rotation().is_portrait();

    // --- nagłówek ----------------------------------------------------------
    fonts.draw(
        c,
        "Start nie doszedł do końca",
        MARGIN as f32,
        76.0,
        TEXT_HEAD,
        Weight::Bold,
        BLACK,
        Align::Left,
    );
    hline(c, MARGIN, 100, w - 2 * MARGIN, 2, BLACK);

    // --- etap --------------------------------------------------------------
    fonts.draw(
        c,
        "zatrzymał się na",
        MARGIN as f32,
        (if poziomo { 150 } else { 176 }) as f32,
        TEXT_BODY,
        Weight::Medium,
        INK_DIM,
        Align::Left,
    );

    // Nazwa etapu w największym stopniu, jaki się mieści. „pobieranie 2. kalendarza"
    // w TEXT_TITLE nie wchodzi w 476 px szerokości użytecznej w pionie, a skracanie
    // z wielokropkiem zabrałoby akurat to słowo, które odróżnia pierwszy kanał od
    // drugiego. Zejście o stopień jest tańsze niż utrata sensu.
    let etap_y = if poziomo { 208 } else { 240 };
    let dostepne = (w - 2 * MARGIN) as f32;
    let stopien = [TEXT_TITLE, TEXT_HEAD, TEXT_LEAD]
        .into_iter()
        .find(|&s| fonts.measure(d.step, s, Weight::Bold) <= dostepne)
        .unwrap_or(TEXT_BODY);
    fonts.draw(
        c,
        d.step,
        MARGIN as f32,
        etap_y as f32,
        stopien,
        Weight::Bold,
        BLACK,
        Align::Left,
    );

    // --- liczby ------------------------------------------------------------
    // Czas i wolna pamięć w jednym wierszu, bo czyta się je razem: „stanął po
    // czterech sekundach, mając 60 KB" to inna diagnoza niż „po czterech, mając 8".
    let liczby_y = etap_y + if poziomo { 44 } else { 56 };
    fonts.draw(
        c,
        &format!("po {} ms · wolny DRAM {} KB", d.ms, d.dram_kb),
        MARGIN as f32,
        liczby_y as f32,
        TEXT_LEAD,
        Weight::Medium,
        BLACK,
        Align::Left,
    );

    if !d.hint.is_empty() {
        fonts.draw(
            c,
            d.hint,
            MARGIN as f32,
            (liczby_y + if poziomo { 36 } else { 44 }) as f32,
            TEXT_BODY,
            Weight::Medium,
            INK_DIM,
            Align::Left,
        );
    }

    // --- wyjście do konfiguracji -------------------------------------------
    let bw = 420.min(w - 2 * MARGIN);
    let bh = if poziomo { 78 } else { 96 };
    let btn = Rect::new(
        (w - bw) / 2,
        h - bh - if poziomo { 96 } else { 150 },
        bw,
        bh,
    );
    stroke_round_rect(c, btn, 14.0, 3, BLACK);
    fonts.draw(
        c,
        "Konfiguracja",
        (btn.x + btn.w / 2) as f32,
        (btn.y + btn.h / 2 + 12) as f32,
        TEXT_HEAD,
        Weight::Bold,
        BLACK,
        Align::Center,
    );
    screen.hits.push(HitRegion::new(btn, Action::OpenSetup));

    // --- stopka ------------------------------------------------------------
    fonts.draw(
        c,
        "sieć pominięta w tym cyklu · następny spróbuje ponownie",
        (w / 2) as f32,
        (h - if poziomo { 48 } else { 84 }) as f32,
        TEXT_BODY,
        Weight::Medium,
        INK_DIM,
        Align::Center,
    );
    if !d.firmware.is_empty() {
        fonts.draw(
            c,
            d.firmware,
            MARGIN as f32,
            (h - 20) as f32,
            TEXT_FLOOR,
            Weight::Medium,
            INK_DIM,
            Align::Left,
        );
    }

    c.quantize_ink();
    screen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Rotation;

    fn przyklad<'a>(step: &'a str, hint: &'a str) -> Diagnosis<'a> {
        Diagnosis {
            step,
            hint,
            ms: 4700,
            dram_kb: 62,
            firmware: "t5s3pro 0.1.0",
        }
    }

    #[test]
    fn ma_wyjscie_do_konfiguracji_w_obu_orientacjach() {
        // Najczęstsza awaria to złe hasło WiFi; ekran bez tego przycisku byłby
        // ślepą uliczką, bo agenda ze swoim drobnym wejściem nie jest rysowana.
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            let s = render_diagnosis(
                &przyklad("łączenie z WiFi", "sprawdź hasło"),
                &Fonts::embedded(),
                &mut c,
            );
            let btn = s
                .hits
                .iter()
                .find(|h| h.action == Action::OpenSetup)
                .unwrap_or_else(|| panic!("{rot:?}: brak wyjścia do konfiguracji"));
            assert!(btn.rect.h >= 44, "{rot:?}: przycisk za niski");
            assert!(
                btn.rect.bottom() <= c.height() as i32,
                "{rot:?}: przycisk poza ekranem"
            );
        }
    }

    /// Najdłuższa nazwa etapu musi się zmieścić bez skracania — to ona odróżnia
    /// pierwszy kanał od drugiego, więc wielokropek zjadłby całą informację.
    #[test]
    fn najdluzszy_etap_miesci_sie_w_szerokosci() {
        let fonts = Fonts::embedded();
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let c = Gray8::new(rot);
            let dostepne = (c.width() as i32 - 2 * MARGIN) as f32;
            for etap in [
                "pobieranie 2. kalendarza",
                "sprawdzanie aktualizacji",
                "łączenie z WiFi",
            ] {
                let ok = [TEXT_TITLE, TEXT_HEAD, TEXT_LEAD]
                    .into_iter()
                    .any(|s| fonts.measure(etap, s, Weight::Bold) <= dostepne);
                assert!(ok, "{rot:?}: \"{etap}\" nie mieści się w żadnym stopniu");
            }
        }
    }

    #[test]
    fn nie_wychodzi_atramentem_na_krawedzie() {
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            render_diagnosis(
                &przyklad("pobieranie 2. kalendarza", "sprawdź adres iCal"),
                &Fonts::embedded(),
                &mut c,
            );
            let (w, h) = (c.width() as i32, c.height() as i32);
            for y in 0..h {
                assert_eq!(c.get(0, y), WHITE, "{rot:?}: atrament na lewej krawędzi");
                assert_eq!(c.get(w - 1, y), WHITE, "{rot:?}: na prawej");
            }
        }
    }
}
