//! Ekran „Dzisiaj" — jedna odpowiedź na pytanie „czy mogę teraz odejść od tej ściany".
//!
//! # Dlaczego to nie jest agenda przycięta do jednego dnia
//!
//! Agenda odpowiada na „co jest w czternastu dniach" i robi to wierszami po 50 px,
//! w których dzisiejsza czternasta wygląda dokładnie tak samo jak czwartkowa. Pytanie
//! „czy mam teraz wolne" jest inne i wymaga innej hierarchii: **co trwa** i **co
//! następne** muszą być czytelne z drugiego końca pokoju, a cała reszta dnia schodzi
//! do jednej linii podsumowania.
//!
//! Stąd trzy bloki zamiast listy. Blok „teraz" jest największy, bo odpowiada na
//! pytanie wprost; „potem" jest drugi, bo mówi, ile mam czasu; podsumowanie jest
//! najmniejsze, bo odpowiada na „czy to już wszystko".
//!
//! # Czas względny nie jest tu bohaterem, i to jest celowe
//!
//! „za 25 min" jest prawdziwe **wyłącznie w chwili namalowania klatki**, a między
//! wybudzeniami mija do godziny. Napis, który po pół godzinie kłamie o pół godziny,
//! nie może być największym elementem ekranu — dlatego bohaterem jest **tytuł
//! i godzina**, a czas względny idzie obok, mniejszym stopniem. Godzina jest prawdziwa
//! zawsze, względny czas tylko chwilowo.
//!
//! Z tego samego powodu ten ekran wymaga wybudzenia o północy (patrz
//! `devlogic::policy::sleep_seconds`): bez niego między północą a rankiem stałby tu
//! ekran o nazwie „dzisiaj" z wczorajszą datą, co jest gorsze niż brak ekranu.
//!
//! # Ton
//!
//! Kończymy `quantize_ink`, tak jak agenda, miesiąc i rok — czyli ekran jest
//! pięciopoziomowy i **nie nadaje się pod częściowe odświeżenie**. To nie jest
//! przeoczenie, tylko ta sama reguła, co wszędzie indziej.

use chrono::{Datelike, NaiveDate, NaiveDateTime};

use crate::canvas::{Gray8, BLACK, INK_DIM, WHITE};
use crate::hit::Screen;
use crate::layout::{TEXT_BODY, TEXT_HEAD, TEXT_LEAD, TEXT_TITLE};
use crate::model::{godzina, za_ile, CalEvent, Model};
use crate::shapes::hline;
use crate::text::{Align, Fonts, Weight};

/// Wysokość ciała ekranu — bez pasa zakładek, który rysuje `layout::render`.
fn body_h(c: &Gray8) -> i32 {
    c.height() as i32 - crate::nav::tabs_h(c)
}

const MARGIN: i32 = 28;

/// Stopnie i odstępy zależne od orientacji.
///
/// W poziomie ciała ekranu jest 486 px zamiast 894, czyli **niecałe 55 %** — same
/// mniejsze marginesy by nie wystarczyły, więc schodzi też stopień tytułu bloku.
struct Miara {
    tytul_bloku: f32,
    odstep_bloku: i32,
    odstep_wiersza: i32,
    naglowek_h: i32,
}

impl Miara {
    fn of(c: &Gray8) -> Self {
        if c.rotation().is_portrait() {
            Self {
                tytul_bloku: TEXT_TITLE,
                odstep_bloku: 46,
                odstep_wiersza: 40,
                naglowek_h: 128,
            }
        } else {
            Self {
                tytul_bloku: TEXT_HEAD,
                odstep_bloku: 26,
                odstep_wiersza: 32,
                naglowek_h: 96,
            }
        }
    }
}

/// Wydarzenia dzisiejszego dnia, rozdzielone na całodniowe i godzinowe.
///
/// `days` niesie tylko to, o co urządzenie pytało, więc brak dzisiejszej grupy jest
/// zwyczajną sytuacją (pusty dzień), a nie awarią.
fn dzisiaj(model: &Model, dzis: NaiveDate) -> (Vec<&CalEvent>, Vec<&CalEvent>) {
    let Some(grupa) = model.days.iter().find(|d| d.date == dzis) else {
        return (Vec::new(), Vec::new());
    };
    let mut caly_dzien = Vec::new();
    let mut godzinowe = Vec::new();
    for e in &grupa.events {
        if e.all_day {
            caly_dzien.push(e);
        } else {
            godzinowe.push(e);
        }
    }
    godzinowe.sort_by_key(|e| e.start);
    (caly_dzien, godzinowe)
}

pub fn render_today(model: &Model, fonts: &Fonts, c: &mut Gray8) -> Screen {
    c.clear(WHITE);
    let screen = Screen::default();
    let m = Miara::of(c);
    let w = c.width() as i32;
    let h = body_h(c);
    let szer = (w - 2 * MARGIN) as f32;

    let now = model.now;
    let dzis = now.date();
    let (caly_dzien, godzinowe) = dzisiaj(model, dzis);

    // --- nagłówek: dzień tygodnia i data -----------------------------------
    // Dzień tygodnia większy od daty, bo z dystansu to on odpowiada na „który dziś
    // jest"; liczba i miesiąc są potwierdzeniem, nie treścią główną.
    fonts.draw(
        c,
        crate::model::dzien_tygodnia(dzis),
        MARGIN as f32,
        56.0,
        TEXT_TITLE,
        Weight::Bold,
        BLACK,
        Align::Left,
    );
    fonts.draw(
        c,
        &crate::model::data_dzien_miesiac(dzis),
        (w - MARGIN) as f32,
        56.0,
        TEXT_HEAD,
        Weight::Medium,
        INK_DIM,
        Align::Right,
    );

    // Święto i wydarzenia całodniowe idą jedną linią pod datą — to kontekst dnia,
    // nie jego treść, więc nie zabierają miejsca blokom „teraz" i „potem".
    let mut kontekst: Vec<String> = Vec::new();
    if model.holidays.contains(&dzis) {
        kontekst.push("święto".to_string());
    }
    for e in &caly_dzien {
        kontekst.push(e.title.clone());
    }
    if !kontekst.is_empty() {
        let linia = fonts.truncate(&kontekst.join(" · "), szer, TEXT_BODY, Weight::Medium);
        fonts.draw(
            c,
            &linia,
            MARGIN as f32,
            92.0,
            TEXT_BODY,
            Weight::Medium,
            INK_DIM,
            Align::Left,
        );
    }

    hline(c, MARGIN, m.naglowek_h, w - 2 * MARGIN, 2, BLACK);

    let mut y = m.naglowek_h + m.odstep_bloku;

    // --- blok „teraz" ------------------------------------------------------
    let trwa: Vec<&&CalEvent> = godzinowe.iter().filter(|e| e.is_now(now)).collect();
    y = etykieta(c, fonts, MARGIN, y, "TERAZ");

    if let Some(e) = trwa.first() {
        let tytul = fonts.truncate(&e.title, szer, m.tytul_bloku, Weight::Bold);
        fonts.draw(
            c,
            &tytul,
            MARGIN as f32,
            y as f32,
            m.tytul_bloku,
            Weight::Bold,
            BLACK,
            Align::Left,
        );
        y += m.odstep_wiersza;
        // „do 15:30" jest prawdą zawsze; „zostało 25 min" tylko w chwili malowania —
        // dlatego godzina idzie pierwsza i to ona jest kotwicą.
        let zostalo = (e.end - now).num_minutes().max(0);
        fonts.draw(
            c,
            &format!("do {} · zostało {} min", godzina(e.end), zostalo),
            MARGIN as f32,
            y as f32,
            TEXT_LEAD,
            Weight::Medium,
            INK_DIM,
            Align::Left,
        );
        y += m.odstep_bloku;
        if trwa.len() > 1 {
            fonts.draw(
                c,
                &format!("i {} inne równolegle", trwa.len() - 1),
                MARGIN as f32,
                y as f32,
                TEXT_BODY,
                Weight::Medium,
                INK_DIM,
                Align::Left,
            );
            y += m.odstep_wiersza;
        }
    } else {
        fonts.draw(
            c,
            "nic nie trwa",
            MARGIN as f32,
            y as f32,
            m.tytul_bloku,
            Weight::Bold,
            INK_DIM,
            Align::Left,
        );
        y += m.odstep_bloku;
    }

    // --- blok „potem" ------------------------------------------------------
    y += m.odstep_bloku / 2;
    let nastepne: Vec<&&CalEvent> = godzinowe.iter().filter(|e| e.start > now).collect();
    y = etykieta(c, fonts, MARGIN, y, "POTEM");

    if let Some(e) = nastepne.first() {
        // Godzina i tytuł w jednym wierszu: godzina stałej szerokości po lewej,
        // tytuł dobiera resztę. Ta sama zasada, co w kolumnie godzin agendy.
        let godz = godzina(e.start);
        let godz_w = fonts.measure("00:00", TEXT_HEAD, Weight::Bold) + 18.0;
        fonts.draw(
            c,
            &godz,
            MARGIN as f32,
            y as f32,
            TEXT_HEAD,
            Weight::Bold,
            BLACK,
            Align::Left,
        );
        let tytul = fonts.truncate(&e.title, szer - godz_w, TEXT_HEAD, Weight::Bold);
        fonts.draw(
            c,
            &tytul,
            MARGIN as f32 + godz_w,
            y as f32,
            TEXT_HEAD,
            Weight::Bold,
            BLACK,
            Align::Left,
        );
        y += m.odstep_wiersza;
        fonts.draw(
            c,
            &za_ile(e.start, now),
            MARGIN as f32,
            y as f32,
            TEXT_LEAD,
            Weight::Medium,
            INK_DIM,
            Align::Left,
        );
        y += m.odstep_wiersza;
    } else {
        fonts.draw(
            c,
            "na dziś koniec",
            MARGIN as f32,
            y as f32,
            m.tytul_bloku,
            Weight::Bold,
            INK_DIM,
            Align::Left,
        );
        y += m.odstep_bloku;
    }

    // --- reszta dnia --------------------------------------------------------
    // Pierwsza wersja stawiała tu jedną linię „jeszcze 2 dzisiaj" przy dolnej
    // krawędzi. Na zrzucie widać było, dlaczego to za mało: w pionie zostawało
    // 350 px pustki, a w poziomie CAŁA prawa połowa dziewięciuset sześćdziesięciu.
    // Ekran ścienny, który ma pół powierzchni pustej, marnuje dokładnie to, po co
    // się na niego patrzy — a „jeszcze 2" i tak każe pytać „ale o której".
    //
    // Godziny wystarczą. To nadal nie jest agenda: bez znaczników źródła, bez
    // paginacji, bez celów dotykowych — sama odpowiedź na „kiedy dziś jeszcze coś
    // mam". Pierwsze z listy pomijamy, bo stoi wyżej jako „potem".
    let reszta: Vec<&&CalEvent> = nastepne.iter().skip(1).copied().collect();
    if !reszta.is_empty() {
        let poziomo = !c.rotation().is_portrait();
        // W poziomie druga kolumna, w pionie dalej w dół. Ta sama treść, inne miejsce.
        let (lx, mut ly, dostepne) = if poziomo {
            let lx = w / 2 + MARGIN / 2;
            (lx, m.naglowek_h + m.odstep_bloku, (w - MARGIN - lx) as f32)
        } else {
            (MARGIN, y + m.odstep_bloku, szer)
        };

        ly = etykieta(c, fonts, lx, ly, "RESZTA DNIA");

        let krok = if poziomo { 34 } else { 44 };
        // Dolna granica: nad stopką w pionie, nad pasem zakładek w poziomie.
        let limit = h - if poziomo { 24 } else { 40 };
        let godz_w = fonts.measure("00:00", TEXT_LEAD, Weight::Bold) + 16.0;

        let mut pokazane = 0;
        for e in &reszta {
            if ly + krok > limit {
                break;
            }
            fonts.draw(
                c,
                &godzina(e.start),
                lx as f32,
                ly as f32,
                TEXT_LEAD,
                Weight::Bold,
                BLACK,
                Align::Left,
            );
            let tytul = fonts.truncate(&e.title, dostepne - godz_w, TEXT_LEAD, Weight::Medium);
            fonts.draw(
                c,
                &tytul,
                lx as f32 + godz_w,
                ly as f32,
                TEXT_LEAD,
                Weight::Medium,
                BLACK,
                Align::Left,
            );
            ly += krok;
            pokazane += 1;
        }

        // Ucięcie musi być widoczne. Milczące urwanie listy na ekranie bez paska
        // przewijania czyta się jak „to wszystko", czyli kłamie.
        if pokazane < reszta.len() {
            fonts.draw(
                c,
                &format!("…i jeszcze {}", reszta.len() - pokazane),
                lx as f32,
                ly as f32,
                TEXT_BODY,
                Weight::Medium,
                INK_DIM,
                Align::Left,
            );
        }
    }

    // Kwantyzacja na końcu, dokładnie jak w agendzie, miesiącu i roku. `ink_level`
    // NIE jest idempotentna, więc to musi być jedyne wywołanie na tym płótnie.
    c.quantize_ink();
    screen
}

/// Etykieta sekcji: małe, rozrzedzone wersaliki. Zwraca `y` pierwszego wiersza treści.
fn etykieta(c: &mut Gray8, fonts: &Fonts, x: i32, y: i32, tekst: &str) -> i32 {
    fonts.draw(
        c,
        tekst,
        x as f32,
        y as f32,
        TEXT_BODY,
        Weight::Bold,
        INK_DIM,
        Align::Left,
    );
    y + if c.rotation().is_portrait() { 52 } else { 40 }
}

/// Czy dwa modele dałyby na tym ekranie **inną odpowiedź**.
///
/// # Po co to istnieje
///
/// Bo „Dzisiaj" zależy od `now`, a nie od treści kanału — i suma kontrolna kanału,
/// którą posługuje się reszta firmware'u, nie zauważy ani tego, że spotkanie właśnie
/// się zaczęło, ani tego, że zrobiła się północ. Bez tej funkcji ekran albo zamarza
/// na godzinę, albo przemalowuje się przy każdym wybudzeniu mimo braku zmian.
///
/// Porównujemy **semantykę, nie piksele**: datę, identyfikator trwającego wydarzenia,
/// identyfikator następnego i liczbę pozostałych. Zmiana samego „za 25 min" na
/// „za 24 min" celowo NIE jest zmianą — ten napis nie jest bohaterem ekranu i nie
/// jest wart pełnej klatki.
pub fn odcisk(model: &Model) -> Odcisk {
    let dzis = model.now.date();
    let (_, godzinowe) = dzisiaj(model, dzis);
    let trwa = godzinowe
        .iter()
        .find(|e| e.is_now(model.now))
        .map(|e| e.start);
    let nastepne = godzinowe
        .iter()
        .find(|e| e.start > model.now)
        .map(|e| e.start);
    Odcisk {
        dzien: dzis.num_days_from_ce(),
        trwa,
        nastepne,
        pozostalo: godzinowe.iter().filter(|e| e.start > model.now).count() as u16,
    }
}

/// Semantyczny odcisk ekranu „Dzisiaj" — patrz [`odcisk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Odcisk {
    dzien: i32,
    trwa: Option<NaiveDateTime>,
    nastepne: Option<NaiveDateTime>,
    pozostalo: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Rotation;
    use crate::model::{CalEvent, DayGroup, SourceTag};
    use chrono::NaiveTime;

    fn dt(d: u32, h: u32, min: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, d)
            .unwrap()
            .and_hms_opt(h, min, 0)
            .unwrap()
    }

    fn ev(d: u32, od: (u32, u32), do_: (u32, u32), tytul: &str) -> CalEvent {
        CalEvent {
            start: dt(d, od.0, od.1),
            end: dt(d, do_.0, do_.1),
            all_day: false,
            title: tytul.to_string(),
            location: None,
            source: SourceTag::Primary,
        }
    }

    fn model_z(now: NaiveDateTime, wydarzenia: Vec<CalEvent>) -> Model {
        let mut m = Model::empty(now);
        if !wydarzenia.is_empty() {
            m.days = vec![DayGroup {
                date: now.date(),
                events: wydarzenia,
            }];
        }
        m.view = crate::model::View::Today;
        m
    }

    /// Ekran musi coś powiedzieć także wtedy, gdy nie ma nic — pusty dzień jest
    /// najczęstszym stanem weekendu, a biała kartka wygląda jak awaria.
    #[test]
    fn pusty_dzien_nie_jest_pusta_kartka() {
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            let m = model_z(dt(22, 10, 0), vec![]);
            render_today(&m, &Fonts::embedded(), &mut c);
            let atrament = c.pixels().iter().filter(|&&p| p != WHITE).count();
            assert!(
                atrament > 2000,
                "{rot:?}: pusty dzień dał tylko {atrament} px atramentu"
            );
        }
    }

    #[test]
    fn nie_wychodzi_atramentem_na_krawedzie() {
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            let m = model_z(
                dt(22, 14, 15),
                vec![
                    ev(
                        22,
                        (14, 0),
                        (15, 30),
                        "Bardzo długi tytuł spotkania, który się nie mieści",
                    ),
                    ev(22, (16, 0), (17, 0), "Odbiór dzieci"),
                    ev(22, (20, 0), (21, 0), "Kolacja"),
                ],
            );
            render_today(&m, &Fonts::embedded(), &mut c);
            let (w, h) = (c.width() as i32, c.height() as i32);
            for y in 0..h {
                assert_eq!(c.get(0, y), WHITE, "{rot:?}: atrament na lewej krawędzi");
                assert_eq!(c.get(w - 1, y), WHITE, "{rot:?}: na prawej");
            }
        }
    }

    /// Treść nie może wjechać w pas zakładek — ten rysuje się po nas i przykryłby ją.
    #[test]
    fn tresc_nie_wchodzi_w_pas_zakladek() {
        for rot in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rot);
            let m = model_z(
                dt(22, 14, 15),
                vec![
                    ev(22, (14, 0), (15, 30), "Spotkanie"),
                    ev(22, (16, 0), (17, 0), "Drugie"),
                    ev(22, (20, 0), (21, 0), "Trzecie"),
                ],
            );
            render_today(&m, &Fonts::embedded(), &mut c);
            let dol = body_h(&c);
            for y in dol..c.height() as i32 {
                for x in 0..c.width() as i32 {
                    assert_eq!(c.get(x, y), WHITE, "{rot:?}: atrament w pasie zakładek");
                }
            }
        }
    }

    /// Sedno tego ekranu: trwające wydarzenie musi być czymś INNYM niż następne.
    #[test]
    fn trwajace_i_nastepne_sa_rozroznione() {
        let m = model_z(
            dt(22, 14, 15),
            vec![
                ev(22, (14, 0), (15, 30), "Trwa teraz"),
                ev(22, (16, 0), (17, 0), "Potem"),
            ],
        );
        let o = odcisk(&m);
        assert_eq!(o.trwa, Some(dt(22, 14, 0)));
        assert_eq!(o.nastepne, Some(dt(22, 16, 0)));
        assert_eq!(o.pozostalo, 1);
    }

    /// Odcisk MUSI się zmienić, gdy zmienia się odpowiedź ekranu — inaczej klatka
    /// zamarza. I MUSI zostać ten sam, gdy zmienia się wyłącznie czas względny.
    #[test]
    fn odcisk_lapie_zmiane_odpowiedzi_a_nie_uplyw_minut() {
        let wyd = vec![
            ev(22, (14, 0), (15, 30), "Trwa"),
            ev(22, (16, 0), (17, 0), "Potem"),
        ];

        let a = odcisk(&model_z(dt(22, 14, 15), wyd.clone()));
        let b = odcisk(&model_z(dt(22, 14, 16), wyd.clone()));
        assert_eq!(
            a, b,
            "minuta później to ta sama odpowiedź, klatka niepotrzebna"
        );

        // Spotkanie się skończyło — to JEST inna odpowiedź.
        let c = odcisk(&model_z(dt(22, 15, 45), wyd.clone()));
        assert_ne!(a, c, "koniec wydarzenia musi wymusić klatkę");

        // Zmiana doby też, i to jest powód wybudzenia o północy.
        let jutro = odcisk(&model_z(
            NaiveDate::from_ymd_opt(2026, 8, 23)
                .unwrap()
                .and_time(NaiveTime::from_hms_opt(0, 1, 0).unwrap()),
            wyd,
        ));
        assert_ne!(a, jutro, "nowa doba musi wymusić klatkę");
    }

    /// Całodniowe nie mogą trafić do „teraz" ani do „potem" — inaczej „Urlop"
    /// wypchnąłby z ekranu spotkanie, które faktycznie trwa.
    #[test]
    fn calodniowe_ida_do_kontekstu_nie_do_blokow() {
        let mut caly = ev(22, (0, 0), (23, 59), "Urlop");
        caly.all_day = true;
        let m = model_z(
            dt(22, 14, 15),
            vec![caly, ev(22, (14, 0), (15, 30), "Spotkanie")],
        );
        let o = odcisk(&m);
        assert_eq!(
            o.trwa,
            Some(dt(22, 14, 0)),
            "całodniowe przesłoniło trwające"
        );
    }
}
