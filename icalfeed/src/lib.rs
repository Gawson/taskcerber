//! Pobieranie i parsowanie kanałów iCalendar dla urządzeń o ograniczonej pamięci.
//!
//! Crate celowo **nie zależy od ESP-IDF** — dzięki temu najtrudniejsza logika
//! projektu (składanie linii, strefy czasowe, rozwijanie reguł powtarzania,
//! nadpisania pojedynczych wystąpień) jest testowalna na hoście, na prawdziwych
//! plikach `.ics`, zamiast być pisana w ciemno i debugowana przez patrzenie
//! na ścianę.

pub mod dt;
pub mod feed;
pub mod parser;

pub use feed::{parse_feed, FeedError, Window};
pub use parser::{Property, PropertyReader};

/// Buduje z góry leniwe silniki wyrażeń regularnych, których używa parser reguł.
///
/// # Po co osobna funkcja na coś, co zrobi się samo
///
/// Bo robi się samo **na stosie wołającego**, a kosztuje tam ~27 KB. `rrule` trzyma
/// dwa `OnceLock<Regex>` (nazwa właściwości i format daty) i buduje je przy pierwszym
/// `RRuleSet::from_str`. Budowa schodzi przez `regex_automata::meta::strategy::new`,
/// której pojedyncza ramka to **13 632 B** — największa w całym obrazie firmware'u.
/// Razem z drogą do niej daje to 34 KB, czyli więcej niż miało całe zadanie `main`.
///
/// Tak wyglądał zdiagnozowany stack overflow: nie parser kalendarza go zjadał, tylko
/// jednorazowa budowa silnika regex, wykonana akurat w najgłębszym miejscu cyklu.
///
/// **Wołaj z osobnego wątku z jawnym `stack_size`.** Rozgrzewka na początku `main`
/// nic nie daje: `entry` rezerwuje ramkę `main` w chwili wejścia, więc te 27 KB
/// i tak musiałoby się zmieścić obok niej.
pub fn warm_up_rrule() {
    // Ten tekst dotyka OBU blokad naraz: nazwy właściwości (każda linia treści)
    // i formatu daty (DTSTART). Wynik jest nieistotny — liczy się skutek uboczny.
    let _ = "DTSTART:20200101T000000Z\nRRULE:FREQ=DAILY;COUNT=1".parse::<rrule::RRuleSet>();
}
