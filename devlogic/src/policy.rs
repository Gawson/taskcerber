//! Polityka zasilania.
//!
//! Bazuje na pomiarach i szacunkach z `docs/power.md`. Krótko:
//! * Podłoga deep sleepu po pełnej sekwencji wyłączania: **~155 µA**.
//! * Jedno wybudzenie z siecią: **~360 mAs** (zoptymalizowane) do ~850 mAs (zimne).
//! * Budżet na tydzień z 1200 mAh użytecznych: **7,14 mA średnio**.
//!
//! Przy odświeżaniu co 30 minut w godzinach aktywnych i nocnej przerwie wychodzi
//! ~7 mAh/dobę, czyli grubo ponad sto dni. Tydzień to nie jest tu trudny cel —
//! trudne jest nie zepsuć go jednym z czterech błędów opisanych w `docs/power.md`.
//!
//! # Dlaczego argumenty są prymitywami, a nie strukturami z płytki
//!
//! `mode` bierze `usb_present: bool` i `battery_percent: Option<u8>`, a nie
//! `PowerStatus` i `Fuel` z modułów sterowników. Tamte struktury ciągną za sobą
//! ESP-IDF, a wtedy cała ta polityka wróciłaby do firmware'u i przestałaby być
//! testowana. `None` w poziomie ogniwa to nie brak informacji do zignorowania —
//! to licznik ogniwa, który nie odpowiedział, i traktujemy go jak zły stan.

use chrono::{NaiveDateTime, NaiveTime, Timelike};

/// Tryb pracy wybierany przy każdym wybudzeniu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// USB podłączone — nie oszczędzamy.
    Usb,
    /// Bateria, godziny aktywne, ogniwo w porządku.
    Active,
    /// Bateria, okno nocne — jedno długie spanie do rana.
    Night,
    /// Ogniwo poniżej 40% — rzadziej.
    Frugal,
    /// Ogniwo poniżej 20% — jeszcze rzadziej, bez pełnych odświeżeń.
    Survival,
    /// Ogniwo poniżej 10% — ostatni ekran i zamrożenie.
    Hold,
}

/// Poniżej tego poziomu naładowania nie ruszamy OTA bez USB.
pub const MIN_BATTERY_FOR_OTA: u8 = 50;

/// Konfiguracja polityki. Docelowo czytana z NVS, żeby dało się zmienić
/// bez przeflashowania.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// Początek okna aktywnego.
    pub day_start: NaiveTime,
    /// Koniec okna aktywnego.
    pub day_end: NaiveTime,
    /// Co ile sekund odświeżać w trybie [`Mode::Active`].
    ///
    /// Godzina, nie pół. Kalendarz zmienia się rzadko, a każde pobranie to 1,18 MB
    /// przez radio i kilkanaście sekund anteny w górze — czyli ponad połowa całego
    /// dobowego budżetu energii idzie właśnie tam. Świeższą treść zawsze da się
    /// wymusić dotknięciem „odśwież".
    pub active_interval_s: u64,
    /// Co ile sekund w trybie [`Mode::Usb`].
    ///
    /// Pół godziny, nie pięć minut. Na kablu energia nie boli, ale pobranie to i tak
    /// 1,18 MB i ~10 s radia — a kalendarz nie zmienia się częściej. Krótki odstęp
    /// dawał tylko złudzenie, że urządzenie „ciągle coś robi".
    pub usb_interval_s: u64,
    /// Co ile sekund w trybie [`Mode::Frugal`].
    pub frugal_interval_s: u64,
    /// Co ile sekund w trybie [`Mode::Survival`].
    pub survival_interval_s: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            day_start: NaiveTime::from_hms_opt(7, 0, 0).unwrap(),
            day_end: NaiveTime::from_hms_opt(23, 0, 0).unwrap(),
            active_interval_s: 60 * 60,
            usb_interval_s: 30 * 60,
            frugal_interval_s: 60 * 60,
            survival_interval_s: 6 * 60 * 60,
        }
    }
}

impl Policy {
    /// Wybiera tryb na podstawie zasilania, stanu ogniwa i pory dnia.
    pub fn mode(&self, usb_present: bool, battery_percent: Option<u8>, now: NaiveDateTime) -> Mode {
        if usb_present {
            return Mode::Usb;
        }

        match battery_percent {
            Some(p) if p < 10 => return Mode::Hold,
            Some(p) if p < 20 => return Mode::Survival,
            Some(p) if p < 40 => return Mode::Frugal,
            _ => {}
        }

        if self.is_night(now.time()) {
            Mode::Night
        } else {
            Mode::Active
        }
    }

    fn is_night(&self, t: NaiveTime) -> bool {
        if self.day_start <= self.day_end {
            t < self.day_start || t >= self.day_end
        } else {
            // Okno przechodzące przez północ.
            t < self.day_start && t >= self.day_end
        }
    }

    /// Ile sekund spać po tym wybudzeniu.
    ///
    /// W trybie nocnym śpimy do początku okna aktywnego, zamiast budzić się co pół
    /// godziny po nic — z jednym przystankiem o północy, żeby data na ekranie zdążyła
    /// się zmienić. Patrz [`Self::seconds_until_midnight`].
    pub fn sleep_seconds(&self, mode: Mode, now: NaiveDateTime) -> u64 {
        let nominalny = match mode {
            Mode::Usb => self.usb_interval_s,
            Mode::Active => self.active_interval_s,
            Mode::Frugal => self.frugal_interval_s,
            Mode::Survival => self.survival_interval_s,
            Mode::Hold => 24 * 60 * 60,
            Mode::Night => self.seconds_until_morning(now),
        };

        // `Hold` broni ogniwa, które jest już prawie puste — jego doba snu jest
        // ważniejsza niż poprawna data na szkle. Wszędzie indziej przycinamy sen
        // do najbliższej północy.
        if matches!(mode, Mode::Hold) {
            return nominalny;
        }
        nominalny.min(self.seconds_until_midnight(now))
    }

    /// Sekundy do najbliższej północy — a właściwie do 00:01.
    ///
    /// # Dlaczego to w ogóle istnieje
    ///
    /// Bo data na ekranie zmienia się o północy, a żaden inny mechanizm tego nie
    /// zauważa: sumę kontrolną liczymy z treści kanału, a ta się o północy nie
    /// zmienia. Bez tego przycięcia `Mode::Night` śpi jednym ciągiem do rana i między
    /// północą a siódmą na szkle stoi ekran o nazwie „dzisiaj" z **wczorajszą** datą.
    /// To samo dotyczy „dziś" i „jutro" w nagłówkach agendy.
    ///
    /// Koszt: jedno dodatkowe wybudzenie na dobę, czyli boot i jedna pełna klatka —
    /// rzędu 42 mAs, poniżej dwóch dziesiątych procenta doby.
    ///
    /// # Dlaczego 00:01, a nie 00:00
    ///
    /// Zegar potrafi obudzić urządzenie ułamek sekundy za wcześnie, a wtedy `now`
    /// jest jeszcze wczorajsze i cała operacja idzie na marne — z drugim wybudzeniem
    /// sekundę później. Minuta zapasu nic nie kosztuje i zamyka tę klasę pomyłek.
    fn seconds_until_midnight(&self, now: NaiveDateTime) -> u64 {
        let cel = now
            .date()
            .succ_opt()
            .map(|d| d.and_hms_opt(0, 1, 0).unwrap_or(d.and_time(self.day_start)));
        let Some(cel) = cel else {
            // Ostatni reprezentowalny dzień w kalendarzu — nie ma jutra, więc nie ma
            // czego przycinać. Zdarzy się nigdy, ale `unwrap` tu nie jest potrzebny.
            return u64::MAX;
        };
        (cel - now).num_seconds().max(60) as u64
    }

    fn seconds_until_morning(&self, now: NaiveDateTime) -> u64 {
        let today_start = now.date().and_time(self.day_start);
        let target = if now < today_start {
            today_start
        } else {
            now.date()
                .succ_opt()
                .map(|d| d.and_time(self.day_start))
                .unwrap_or(today_start)
        };
        let secs = (target - now).num_seconds();
        // Minimum minuta, żeby błąd zegara nie wpędził nas w pętlę natychmiastowych
        // wybudzeń.
        secs.max(60) as u64
    }

    /// Czy w tym trybie w ogóle sięgamy po sieć.
    pub fn should_fetch(&self, mode: Mode) -> bool {
        !matches!(mode, Mode::Hold)
    }

    /// Nominalny odstęp między pobraniami w danym trybie.
    pub fn interval_s(&self, mode: Mode) -> u64 {
        match mode {
            Mode::Usb => self.usb_interval_s,
            Mode::Active | Mode::Night => self.active_interval_s,
            Mode::Frugal => self.frugal_interval_s,
            Mode::Survival | Mode::Hold => self.survival_interval_s,
        }
    }

    /// Czy dane są na tyle stare, żeby warto było po nie sięgnąć.
    ///
    /// # Po co to w ogóle jest
    ///
    /// [`Policy::should_fetch`] patrzy wyłącznie na TRYB, nigdy na czas — a odstęp
    /// między pobraniami wynikał do tej pory tylko z tego, jak długo urządzenie
    /// spało. Wybudzenie przez CZŁOWIEKA omijało więc ten odstęp w całości:
    /// dotknięcie ekranu sekundę po pobraniu ściągało kanał od nowa. Przy kanale
    /// ważącym 1,18 MB to kilkanaście sekund, w trakcie których panel jeszcze nie
    /// istnieje (nie może — konkuruje o DRAM z mbedTLS), więc ekran stoi z markerem
    /// uśpienia, a dotyku nikt nie czyta. Człowiek widzi urządzenie, które go
    /// zignorowało, i naciska cokolwiek innego.
    ///
    /// Świeżość mierzymy od OSTATNIEGO UDANEGO pobrania, bo tylko ono coś zmieniło.
    ///
    /// # Tolerancja
    ///
    /// Wybudzenie timerem wypada nominalnie po `interval_s`, ale zegar dryfuje,
    /// a `align_to_minute` przesuwa moment o kilkadziesiąt sekund. Bez marginesu
    /// pobranie wypadałoby czasem o sekundę za wcześnie i przesuwało się o CAŁY
    /// interwał — kalendarz odświeżałby się co drugi cykl zamiast co cykl.
    pub fn fetch_is_due(&self, mode: Mode, now_unix: i64, last_success_unix: i64) -> bool {
        // Nigdy nic nie pobraliśmy — nie ma czego oszczędzać.
        if last_success_unix <= 0 {
            return true;
        }
        let elapsed = now_unix - last_success_unix;
        // Zegar cofnięty (SNTP poprawił dryf w tył, wymiana ogniwa RTC). Wtedy
        // wiek danych jest nieznany, a nieznany wiek traktujemy jak stary.
        if elapsed < 0 {
            return true;
        }
        const TOLERANCJA_S: i64 = 90;
        elapsed >= self.interval_s(mode) as i64 - TOLERANCJA_S
    }

    /// Czy w tym trybie wolno pobrać aktualizację firmware'u.
    ///
    /// Pobranie ~3 MB przez HTTPS trzyma radio na antenie o rząd wielkości dłużej
    /// niż zwykły cykl, a na końcu dochodzi kasowanie i zapis całego slotu we
    /// flashu. Na zużytym ogniwie to nie jest coś, co warto robić w tle — stąd USB
    /// albo wyraźny zapas energii.
    ///
    /// Nieznany stan naładowania (`None`, np. gdy licznik ogniwa nie odpowiada)
    /// traktujemy jak za mało. Lepiej nie zaktualizować się przez tydzień, niż
    /// zgasnąć w połowie zapisu slotu.
    pub fn should_update(&self, mode: Mode, battery_percent: Option<u8>) -> bool {
        match mode {
            Mode::Usb => true,
            Mode::Active => matches!(battery_percent, Some(p) if p >= MIN_BATTERY_FOR_OTA),
            _ => false,
        }
    }
}

/// Ile sekund do najbliższej pełnej minuty — wyrównanie wybudzeń, żeby odświeżenie
/// wypadało o równych godzinach, a nie o 30 sekundach po.
pub fn align_to_minute(now: NaiveDateTime, seconds: u64) -> u64 {
    let target_unaligned = seconds + now.second() as u64;
    let remainder = target_unaligned % 60;
    if remainder == 0 {
        seconds
    } else {
        seconds + (60 - remainder)
    }
}

#[cfg(test)]
mod tests {

    /// Wybudzenie przez człowieka nie może ściągać kanału od nowa sekundę po
    /// poprzednim pobraniu — to jest te kilkanaście sekund, przez które urządzenie
    /// wygląda, jakby zignorowało dotknięcie.
    #[test]
    fn swieze_dane_nie_sa_pobierane_ponownie() {
        let p = Policy::default();
        let teraz = 1_000_000i64;
        let interwal = p.usb_interval_s as i64;

        assert!(
            !p.fetch_is_due(Mode::Usb, teraz, teraz - 1),
            "pobranie sprzed sekundy jest świeże"
        );
        assert!(
            !p.fetch_is_due(Mode::Usb, teraz, teraz - interwal / 2),
            "połowa interwału to wciąż świeżo"
        );
        assert!(
            p.fetch_is_due(Mode::Usb, teraz, teraz - interwal),
            "po pełnym interwale pobieramy"
        );
        assert!(
            p.fetch_is_due(Mode::Usb, teraz, teraz - 10 * interwal),
            "dane sprzed godzin są bezdyskusyjnie stare"
        );
    }

    /// Tolerancja istnieje po to, żeby wybudzenie o sekundę za wczesne nie przesuwało
    /// pobrania o CAŁY interwał — inaczej kalendarz odświeżałby się co drugi cykl.
    #[test]
    fn wybudzenie_odrobine_za_wczesne_wciaz_pobiera() {
        let p = Policy::default();
        let teraz = 1_000_000i64;
        let interwal = p.usb_interval_s as i64;
        assert!(
            p.fetch_is_due(Mode::Usb, teraz, teraz - interwal + 60),
            "minuta przed czasem to dryf zegara, nie świeże dane"
        );
    }

    /// Brak historii i cofnięty zegar znaczą „nie wiem, ile to ma lat" — a nieznany
    /// wiek traktujemy jak stary, bo pusty ekran jest gorszy niż jedno pobranie.
    #[test]
    fn nieznany_wiek_danych_znaczy_stary() {
        let p = Policy::default();
        assert!(p.fetch_is_due(Mode::Usb, 1_000_000, 0), "nigdy nie pobrano");
        assert!(
            p.fetch_is_due(Mode::Usb, 1_000_000, 2_000_000),
            "zegar cofnięty"
        );
    }

    /// Każdy tryb ma swój interwał i żaden nie może przypadkiem zwrócić zera —
    /// zero znaczyłoby pobieranie przy każdym wybudzeniu, czyli stan sprzed poprawki.
    #[test]
    fn kazdy_tryb_ma_niezerowy_interwal() {
        let p = Policy::default();
        for mode in [
            Mode::Usb,
            Mode::Active,
            Mode::Night,
            Mode::Frugal,
            Mode::Survival,
            Mode::Hold,
        ] {
            assert!(p.interval_s(mode) > 0, "{mode:?} ma zerowy interwał");
        }
    }
    use super::*;
    use chrono::NaiveDate;

    fn at(h: u32, m: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 8, 18)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
    }

    #[test]
    fn usb_wygrywa_ze_wszystkim() {
        let p = Policy::default();
        assert_eq!(p.mode(true, Some(5), at(3, 0)), Mode::Usb);
    }

    #[test]
    fn niski_stan_ogniwa_wygrywa_z_pora_dnia() {
        let p = Policy::default();
        assert_eq!(p.mode(false, Some(5), at(12, 0)), Mode::Hold);
        assert_eq!(p.mode(false, Some(15), at(12, 0)), Mode::Survival);
        assert_eq!(p.mode(false, Some(30), at(12, 0)), Mode::Frugal);
    }

    #[test]
    fn okno_nocne() {
        let p = Policy::default();
        assert_eq!(p.mode(false, Some(80), at(2, 0)), Mode::Night);
        assert_eq!(p.mode(false, Some(80), at(23, 30)), Mode::Night);
        assert_eq!(p.mode(false, Some(80), at(6, 59)), Mode::Night);
        assert_eq!(p.mode(false, Some(80), at(7, 0)), Mode::Active);
        assert_eq!(p.mode(false, Some(80), at(22, 59)), Mode::Active);
    }

    /// Noc śpi do rana, ale z PRZYSTANKIEM O PÓŁNOCY.
    ///
    /// Poprzednia wersja tego testu wymagała jednego ciągu i była zgodna z kodem,
    /// tyle że oba były błędne: między północą a siódmą na szkle stał ekran „dzisiaj"
    /// z wczorajszą datą, a w agendzie „dziś" wskazywało wczoraj.
    #[test]
    fn noc_budzi_sie_o_polnocy_a_potem_spi_do_rana() {
        let p = Policy::default();

        // O 23:30 najbliższa granica to północ (a ściślej 00:01), czyli 31 minut.
        assert_eq!(p.sleep_seconds(Mode::Night, at(23, 30)), 31 * 60);

        // Po północy nic już nie stoi na drodze do rana: o 02:00 do 07:00 jest 5 h.
        assert_eq!(p.sleep_seconds(Mode::Night, at(2, 0)), 5 * 3600);
    }

    /// Przycięcie do północy obowiązuje we WSZYSTKICH trybach poza `Hold` — data
    /// na ekranie jest niepoprawna niezależnie od tego, ile zostało w ogniwie.
    #[test]
    fn polnoc_przycina_kazdy_tryb_procz_hold() {
        let p = Policy::default();
        // Godzinę przed północą żaden nominalny odstęp nie ma prawa jej przekroczyć.
        for mode in [Mode::Usb, Mode::Active, Mode::Frugal, Mode::Survival] {
            let s = p.sleep_seconds(mode, at(23, 0));
            assert!(s <= 61 * 60, "{mode:?}: sen {s} s przeskakuje północ");
        }
        // `Hold` broni ogniwa, które jest prawie puste — jego doba jest ważniejsza.
        assert_eq!(p.sleep_seconds(Mode::Hold, at(23, 0)), 24 * 3600);
    }

    /// Sen liczony tuż przed północą nie może wyjść zerowy ani ujemny — inaczej
    /// urządzenie wpadłoby w pętlę natychmiastowych wybudzeń.
    #[test]
    fn tuz_przed_polnoca_sen_ma_minimum() {
        let p = Policy::default();
        for minuta in [58, 59] {
            let s = p.sleep_seconds(Mode::Active, at(23, minuta));
            assert!(s >= 60, "o 23:{minuta} sen wyszedł {s} s");
        }
    }

    #[test]
    fn spanie_nigdy_nie_jest_zerowe() {
        let p = Policy::default();
        // Dokładnie o początku okna aktywnego — nie wolno zwrócić 0.
        let s = p.sleep_seconds(Mode::Night, at(7, 0));
        assert!(s >= 60);
    }

    #[test]
    fn tryb_hold_nie_siega_po_siec() {
        let p = Policy::default();
        assert!(!p.should_fetch(Mode::Hold));
        assert!(p.should_fetch(Mode::Active));
        assert!(p.should_fetch(Mode::Survival));
    }

    #[test]
    fn ota_tylko_na_usb_albo_z_zapasem_energii() {
        let p = Policy::default();
        assert!(p.should_update(Mode::Usb, Some(5)));
        assert!(p.should_update(Mode::Active, Some(MIN_BATTERY_FOR_OTA)));
        assert!(!p.should_update(Mode::Active, Some(MIN_BATTERY_FOR_OTA - 1)));
        // Milczący licznik ogniwa liczy się jak za mało — nie jak „pewnie w porządku".
        assert!(!p.should_update(Mode::Active, None));
        // Tryby oszczędne nie aktualizują się nigdy, choćby procent był wysoki.
        for mode in [Mode::Night, Mode::Frugal, Mode::Survival, Mode::Hold] {
            assert!(!p.should_update(mode, Some(100)), "{mode:?}");
        }
    }

    #[test]
    fn wyrownanie_do_pelnej_minuty() {
        let now = NaiveDate::from_ymd_opt(2026, 8, 18)
            .unwrap()
            .and_hms_opt(12, 0, 20)
            .unwrap();
        // 1800 s + 20 s przesunięcia -> dociągamy do 1840, żeby trafić w pełną minutę.
        assert_eq!(align_to_minute(now, 1800) % 60, 40);
        let exact = NaiveDate::from_ymd_opt(2026, 8, 18)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap();
        assert_eq!(align_to_minute(exact, 1800), 1800);
    }
}
