# Warstwa ekranów, widoki, zadania, powiadomienia — projekt

Dokument projektowy do zatwierdzenia. Powstał z ocenionych propozycji; wszystkie liczby
są albo zmierzone na sztuce (`docs/bringup.md`), albo policzone na hoście na tym kodzie,
albo wyprowadzone z tablicy przebiegów `waveforms/epdiy_ED047TC1.h`. Tam, gdzie liczba
jest szacunkiem, jest to napisane wprost.

**Nie ma tu implementacji.** Są typy, wymiary, koszty i uzasadnienia — tyle, żeby dało się
z tego pisać kod, i ani linijki więcej.

---

## 0. Ograniczenia, z których wynika cała reszta

Powtórzone tutaj, bo każda decyzja niżej odwołuje się do którejś z nich.

| Ograniczenie | Liczba | Skąd |
|---|---|---|
| Pełne odświeżenie | negatyw DU + GC16 = 35 klatek ≈ **250 ms** | `epd.rs:68` (`FULL_VIA_INVERSE = true`), `epd.rs:322-337` |
| Szybkie odświeżenie (MODE_DU) | 5 faz ≈ **35 ms**, zostawia duchy | `docs/bringup.md:116` |
| Obszar **nie skraca** przebiegu | `epd_hl_update_area` taktuje wszystkie bramki | `epd.rs:468` |
| MODE_DU napędza **tylko** poziomy 0 i 15 | `to = 1..14` → `{0,0,0,0}` we wszystkich 5 fazach | tablica przebiegów |
| Panel pod napięciem | ~115 mA; okno dotyku ~40 mA | `docs/power.md` |
| Nic nie przeżywa snu poza RTC i NVS | RTC-FAST: dziś ~48 B zajęte, RTC-SLOW 8 KB | `power/rtc_state.rs` |
| Płótno | `Gray8` = 540 × 960 × 1 B = **518 400 B**, PSRAM | `canvas.rs` |
| Stos zadania głównego | 32 768 B | `sdkconfig.defaults:133` |
| Zmierzona konsumpcja doby | **7,7 mAh/dobę**, średnio 319 µA | `docs/bringup.md:212` |

Z ostatniego wiersza wynika skala, do której trzeba przykładać każdą propozycję:
**doba to 27 720 mAs.** Jedno pełne odświeżenie to 28,8 mAs, czyli 0,1% doby. Jedno
20-sekundowe okno dotyku to 800 mAs, czyli **2,9% doby**. Sześć sekund trzymanych szyn
panelu to 690 mAs, czyli 2,5% doby. Odświeżenia nie są w tym budżecie pozycją — są nią
okna i szyny.

---

## 1. Warstwa ekranów

### 1.1. Co jest dziś i dlaczego to nie działa

Nie ma warstwy ekranów. Są dwie niezależne pętle, każda z własnym kompletem stanu:

| | `interactive_loop` (`main.rs:1089`) | `setup_screen` (`main.rs:1325`) |
|---|---|---|
| długość | 217 linii, w tym `match action` — 106 | 257 linii, w tym `match Applied` — ~20 |
| płótno | własne `canvas` | **własne, drugie** `canvas` |
| mapa dotyku | własne `screen` | własne `screen` |
| odniesienie panelu | `panel_synced` przekazywane przez `&mut` do `repaint` | brak — zaczyna od `Refresh::Full` i już |
| licznik duchów | brak | `du_since_full`, lokalny |
| sklejanie klatek | brak | `COALESCE_MS` / `MAX_DEFER_MS`, lokalne |
| trzymanie szyn | brak | `POWER_HOLD_IDLE_MS`, lokalne |
| okno bezczynności | `IDLE_MS` / `FRESH_IDLE_MS` | `SETUP_IDLE_MS` |
| droga na panel | `paint` → `present` | `repaint_setup` → `present` **albo** `present_areas` wprost |

Do tego pięć funkcji pomocniczych, które istnieją wyłącznie po to, żeby te dwie pętle
miały czym operować: `paint` (`:726`), `render_frame` (`:747`), `repaint` (`:1632`),
`flash_region` (`:1667`), `repaint_setup` (`:1702`), `render_setup_frame` (`:1590`).

Wszystkie pięć zgłoszonych wad to ta sama wada:

| Zgłoszona wada | Gdzie siedzi | Jak została „naprawiona" |
|---|---|---|
| brak wyjścia z konfiguracji | `setup_screen` kończy się tylko przez `Save` albo 90 s ciszy | **nie została** |
| brak potwierdzenia po zapisie | `interactive_loop:1219` | doklejonym blokiem: trzecie płótno + `render_saved` + `mark_going_to_sleep` |
| mignięcie odwracało zły kształt | cel dotykowy ≠ rysunek | dodaniem `HitRegion::visual` (`hit.rs:46-70`) i `flash_region` |
| bateria nieustawiona w modelu | `provisioning_model` | ręcznym dopisaniem pola, `main.rs:810-825` — z komentarzem na dziewięć linii |
| rysowanie ze starego płótna przy zasypianiu | `mark_going_to_sleep` dostaje `&mut canvas` | trzymaniem `canvas` żywego przez całą pętlę |

Każda naprawa jest lokalna, poprawna i **niekomunikowalna następnemu ekranowi**.
Bateria: `render_setup` (`layout.rs:1340`) bierze wyłącznie `&Setup` — na ekranie
konfiguracji stanu ogniwa i sieci nie ma nie dlatego, że ktoś zapomniał go ustawić,
tylko dlatego, że **nie ma jak go tam podać**. Trzeci ekran powtórzy ten sam błąd,
bo nie ma miejsca, w którym byłby raz rozwiązany.

Do tego dwa błędy, których nikt nie zgłosił, a które są w kodzie:

1. **Nieudane wypchnięcie zabija dotyk.** `repaint` (`main.rs:1632-1655`) w gałęzi
   błędu zwraca `(Gray8::new(rotation), Screen::default())` — czyli białe płótno
   i **pustą mapę dotykową**. Po jednym błędzie sterownika okno jest martwe do końca,
   choć na szkle wciąż widać poprzednią, całkowicie poprawną klatkę. Komentarz nad
   funkcją mówi coś przeciwnego („obszary dotykowe z poprzedniej klatki wciąż są
   prawdziwe") — i ma rację, tylko kod tego nie robi.
2. **Obrót zostawia martwe stuknięcia.** `layout.rs:511` klamruje
   `page_index = model.page.min(pages.len() - 1)` i zapisuje wynik do `screen.page`,
   ale nikt tego wyniku nie konsumuje z powrotem do modelu. Te same 60 wydarzeń daje
   5 stron w pionie i 12 w poziomie; ze strony 9 w poziomie po obrocie `model.page`
   zostaje 9, a stron jest 5. Pięć kolejnych stuknięć „wstecz" nie robi nic widocznego,
   a każde z nich wypycha `Refresh::Full` (28,75 mAs) i przedłuża okno o `IDLE_MS`.
   Koszt: 144 mAs samego panelu + do 4 000 mAs przedłużonego okna ≈ **1,15 mAh**,
   czyli 15% doby, na jedno naciśnięcie przycisku obrotu.

### 1.2. Kształt docelowy — dwie rzeczy, nie jedna

**`dashboard::screen`** — czysta warstwa ekranów, bez ESP-IDF, testowana na hoście
i wspólna z symulatorem. **`firmware::ui::glass`** — jedyny właściciel tego, co jest
na szkle. Granica między nimi jest ostra i przebiega tam, gdzie kończy się funkcja czysta,
a zaczyna czas i sprzęt.

#### `dashboard/src/screen.rs`

```rust
/// Wejście warstwy ekranów. Trzy warianty i ani jednego więcej.
pub enum Event {
    /// Stuknięcie, JUŻ zmapowane przez mapę klatki, która jest na szkle.
    /// `shape` to `HitRegion::visual` (albo `rect`, gdy rysunku nie ma).
    Tap { action: Action, shape: Rect },
    /// Przycisk S3 na ekspanderze.
    Rotate,
    /// Okno bezczynności TEGO ekranu minęło.
    Idle,
}

/// Co ekran chce zrobić ze szkłem. Nie „odśwież", tylko „tyle mnie to kosztuje".
pub enum Paint<'a> {
    None,                 // nie dotykaj panelu
    Full,                 // negatyw + GC16, ~250 ms, czyści duchy
    Fast,                 // cała klatka MODE_DU, ~35 ms, zostawia duchy
    Patch(&'a [Rect]),    // te prostokąty, MODE_DU, jedno podniesienie szyn
}

/// Czego ekran chce od świata poza panelem. Sam tego NIE ROBI.
pub enum Request { None, SaveConfig, FetchSoon, StoreRotation(Rotation) }

pub enum Open { Detail(usize), Setup }
pub enum NavCmd { Stay, Push(Open), Pop, Sleep }

/// Wyjście jednego zdarzenia.
pub struct Out<'a> {
    pub paint: Paint<'a>,
    pub nav: NavCmd,
    /// Odwróć kształt pod palcem ZANIM zrobisz `paint`.
    pub ack: bool,
    pub req: Request,
}

/// Co ekrany czytają, a czego nie posiadają.
pub struct Ctx<'a> {
    pub model: &'a Model,          // treść, bateria, sieć, wersja — JEDEN egzemplarz
    pub stored: [&'a str; 6],      // NVS w kolejności `Field::ALL`
    pub rotation: Rotation,
}

pub enum Scene {
    Splash,                 // 0 B stanu
    Agenda { page: usize },
    Detail { index: usize },
    Setup(SetupView),       // { setup: Setup, pressed: Option<Rect>, dirty: Vec<Rect> }
}

impl Scene {
    pub fn on(&mut self, ev: Event, ctx: &Ctx) -> Out<'_>;
    pub fn render(&self, fonts: &Fonts, ctx: &Ctx, c: &mut Gray8) -> Screen;
    pub fn render_patch(&self, fonts: &Fonts, ctx: &Ctx, c: &mut Gray8);
    pub fn idle_ms(&self) -> u32;
    pub fn open(what: Open, ctx: &Ctx) -> Scene;
}

/// Stos ekranów o USTALONEJ głębokości dwóch.
pub struct Nav { root: Scene, over: Option<Scene> }

impl Nav {
    pub fn top(&self) -> &Scene;
    pub fn top_mut(&mut self) -> &mut Scene;
    pub fn walk(&mut self, cmd: NavCmd) -> Walk;   // Stayed | Moved | Sleep
}
```

**Dlaczego enum, a nie `dyn View`.** Nie z powodu sterty — `Screen` i tak niesie
`Vec<HitRegion>` na każdą klatkę, a `Box<dyn View>` byłby jedną alokacją na
*przejście*, nie na klatkę. Powód jest inny: zbiór ekranów jest zamknięty i ma taki
zostać, a `match` na enumie **wymusza obsłużenie każdego z nich** przy każdej zmianie
sygnatury. To jest jedyna rzecz, która nie pozwoli następnemu ekranowi zapomnieć
o baterii.

**Dlaczego nie ma traitu `View`.** Przy jednym implementerze (`Scene` sam ze sobą) trait
nie kupuje niczego poza jednym miejscem na `unreachable!` w domyślnym `render_patch` —
a `unreachable!` na tym urządzeniu to reset. `render_patch` jest `match`em, w którym
każde ramię poza `Setup` woła po prostu `render`: nie ma paniki, nie ma cichego błędu.
Trait wraca tego dnia, gdy pojawi się drugi implementer.

**Dlaczego głębokość dwa.** Każdy ekran otwiera się z agendy i wraca do agendy.
Trzeciego poziomu nie ma jak dziś wywołać — `hit::Action` nie ma akcji, która by go
otwierała. `Nav` to dwa pola, nie `Vec`; `Push` przy zajętym `over` to `debug_assert!`
plus `warn!` plus zachowanie jak `Stay` (przypadek dziś nieosiągalny, więc to wyłącznie
asekuracja, nie mechanizm).

**Dlaczego `Open`, a nie gotowa `Scene` w `NavCmd`.** Ekran konfiguracji trzeba zasiać
tym, co leży w NVS, a warstwa ekranów NVS-u nie widzi. `Scene::open(Open::Setup, ctx)`
jest funkcją totalną, bierze zasiew z `ctx.stored` i dzięki temu identycznie działa
w symulatorze (puste łańcuchy) i w firmwarze.

**Czego w `Ctx` NIE ma i dlaczego.** Nie ma `pages`. Liczba stron jest znana dopiero
**po** renderze, więc ekran decydowałby na podstawie poprzedniej klatki — czyli
dokładnie ten drugi stan, który cały ten refaktor ma usunąć. Nie trzeba: `draw_pager`
(`layout.rs:1096, 1114, 1127`) wystawia region `NextPage`/`PrevPage` **tylko wtedy, gdy
strona istnieje**. Istnienie obszaru dotykowego jest strażnikiem i to wystarczy.

#### `firmware/src/ui/glass.rs`

```rust
/// Jedyny właściciel tego, co jest na szkle. Bierze `&mut Epd` w metodach,
/// NIE posiada go. Konkretna struktura, bez generyka.
pub struct Glass {
    fonts: Fonts<'static>,     // jeden egzemplarz na całe wybudzenie
    shown: Gray8,              // to, co jest na szkle
    hits: dashboard::Screen,   // mapa TEJ zawartości, nierozłączna
    synced: bool,              // czy `back_fb` epdiy opisuje prawdę
    du_since_full: u8,
}

impl Glass {
    pub fn new(rotation: Rotation) -> Self;

    /// Doprowadza szkło do stanu, którego chce `scene`. JEDYNA droga do panelu.
    pub fn apply(&mut self, scene: &Scene, out: &Out, ctx: &Ctx, epd: &mut Epd, t_c: i32);

    /// Punkt PANELU -> obszar. Mapa zawsze pochodzi z klatki, która JEST na szkle.
    pub fn event_at(&self, px: i32, py: i32) -> Option<HitRegion>;

    /// Mignięcie pod palcem i jego cofnięcie — obydwa na płótnie, które jest na szkle.
    pub fn ack(&mut self, epd: &mut Epd, t_c: i32, region: &HitRegion) -> bool;

    /// Znacznik snu rysowany w TYM SAMYM płótnie, które jest na szkle.
    pub fn mark_asleep(&mut self, epd: &mut Epd, t_c: i32);

    /// Nazbierało się duchów. Glass wie CZY, wołający decyduje KIEDY.
    pub fn wants_ghost_clean(&self, idle_ms: u32) -> bool;
}
```

Cztery pola są prywatne i zmienia je **dokładnie jedna metoda**, w jednym wyrażeniu:
albo `render` oddaje nowe płótno i nową mapę naraz, albo `render_patch` zmienia płótno
przy niezmienionej mapie. Stanu pośredniego nie ma.

**Trzy reguły degradacji, które mieszkają wyłącznie tutaj.** To są trzy różne reguły,
nie jedna, i pomylenie ich kosztuje ćwierć sekundy migotania na każde stuknięcie:

1. `Paint::Full | Fast | Patch` przy `!synced` schodzą na `Refresh::Full`.
   `back_fb` epdiy jest po boocie biały i **kłamie** — różnica wyszłaby z niewłaściwego
   odniesienia i na szkle byłaby suma starej i nowej klatki (`epd.rs:290-313`).
2. **`ack` przy `!synced` NIE schodzi.** Odwrócony prostokąt różni się od białego
   `back_fb` na każdym pikselu, więc napędza się poprawnie — to dziś działa i jest
   opisane w `main.rs:1180-1187`. Bez tego wyjątku urządzenie na kablu z niezmienioną
   treścią dostaje +245 ms na każde stuknięcie.
3. `mark_asleep` też nie schodzi, z tego samego powodu.

**Nieudane wypchnięcie nie kasuje stanu.** Glass zostawia `shown` i `hits` nietknięte,
zeruje wyłącznie `synced` i loguje. To jedna linia różnicy wobec dzisiejszego `repaint`
i naprawia błąd opisany w 1.1 pkt 1.

**Czego w `Glass` NIE ma.** Nie ma generyka po `Surface`. Trójmetodowy trait
(`push_full` / `push_fast` / `push_areas`) nie obejmuje ani `hold_power`, ani sklejania
klatek, ani odkładania czyszczenia duchów — czyli ~235 z 257 linii `setup_screen`.
Trait, który pokrywa tę część, która i tak działa, nie zarabia na siebie. Wraca do
rozmowy, gdy `simulator/src/device.rs` będzie miał być przepisany na wspólny model
urządzenia; wtedy musi mieć czwartą metodę i wstrzykiwany zegar, bo sklejanie i duchy
są funkcjami **czasu**.

Nie ma też `hold_power`. Zostaje tam, gdzie jest — w pętli konfiguracji. Rachunek:
utrzymanie szyn oszczędza dwie sekwencje `poweron`/`poweroff`, czyli 40–80 ms × 115 mA
= **4,6–9,2 mAs** na stuknięcie, a przy progu `POWER_HOLD_IDLE_MS = 6 s` kosztuje
6 s × 115 mA = **690 mAs**. To 75–150-krotna strata. Próg opłacalności trzymania szyn to
40–80 ms; człowiek nie stuka co 80 ms. Na klawiaturze się to broni, bo tam odstępy
między znakami są rzędu 200–400 ms i klatka i tak przychodzi; na agendzie nie ma nawet
o czym mówić.

### 1.3. Co zostaje w pętli firmware'u

Pętla ma po zmianie ~50 linii, nie sześć. To jest uczciwa liczba i trzeba ją podać,
bo to, co zostaje, to **polityka czasu**, a `Scene::on` ma być bezczasowe:

| Zostaje w firmwarze | Stała | Dlaczego nie w warstwie ekranów |
|---|---|---|
| sklejanie naciśnięć | `COALESCE_MS` 15 / `MAX_DEFER_MS` 220 (`:1841`, `:1847`) | N naciśnięć ma dać JEDNĄ klatkę; jedno `on()` → jedno `Out` dałoby jedno DU na znak |
| odłożone czyszczenie duchów | `DU_BEFORE_FULL` 12 / `FULL_AFTER_IDLE_MS` 2000 (`:1816`, `:1824`) | wyzwalane czasem bez dotyku; w `Event` nie ma trzeciego zegara, a `Idle` znaczy „zamknij ekran" |
| trzymanie szyn | `POWER_HOLD_IDLE_MS` 6000 (`:1833`) | j.w., plus ostrzeżenie z `epd.rs:257` |
| martwy kontroler | `Touch::Dead`, `TOUCH_ERRORS_BEFORE_GIVING_UP` 8 | to nie jest zdarzenie ekranu, tylko awaria sprzętu |
| próbkowanie i zbocze | `SAMPLE_MS` 10, `STABLE_SAMPLES` 3 | j.w. |
| pierwsza klatka wybudzenia | `synced` | należy do `Glass`, nie do ekranu |

Ta lista ma trafić do doc-komentarza modułu `screen` jako **„czego ta warstwa NIE ma
prawa rozstrzygać"**. Bez tego z czterech rozsypanych miejsc konsoliduje się jedno
i zostawia trzy.

### 1.4. Ekrany po przeniesieniu

| Ekran | Stan | `idle_ms` | `Idle` → | Dziś |
|---|---|---|---|---|
| `Splash` | 0 B | 60 s (`FRESH_IDLE_MS`) | `Sleep` | `provisioning_model` + agenda |
| `Agenda` | `{ page: usize }` | 20 s (`IDLE_MS`) | `Sleep` | `interactive_loop` + `model.page` |
| `Detail` | `{ index: usize }` | 20 s | `Sleep` | `model.focus` |
| `Setup` | `SetupView` ≈ 196 B | 90 s (`SETUP_IDLE_MS`) | **`Pop`** | osobna pętla, 257 linii |

Żadnych nowych stałych czasu. `Setup::Idle → Pop` to utrwalenie zachowania, które dziś
istnieje jako obejście: `interactive_loop:1219-1235` przestawia `deadline` po powrocie
z konfiguracji, bo inaczej urządzenie zasypia w chwili, w której użytkownik dopiero
zobaczył agendę. W nowym modelu `Walk::Moved` zawsze przelicza termin z
`nav.top().idle_ms()` i nie ma czego obchodzić.

Deklaracje malowania, wprost — one są całą treścią kontraktu:

| Ekran / akcja | `Paint` | `ack` | Dlaczego |
|---|---|---|---|
| agenda, zmiana strony | `Full` | nie | mignięcie podwoiłoby 250 ms, a strona i tak się przemaluje |
| agenda, `OpenSetup` | `Full` | **tak** | akcja trwa ponad sekundę |
| agenda, `RefreshNow` | `None` | **tak** | mignięcie jest JEDYNĄ odpowiedzią — pobranie idzie na następne wybudzenie |
| szczegóły, `Back` | `Full` | nie | duża zmiana treści, duchy po DU byłyby widoczne |
| konfiguracja, znak | `Patch(&dirty)` | nie | pole wartości + klawisz pod palcem + poprzedni, który gaśnie |
| konfiguracja, `⇧`/`?123`/pole | `Fast` | nie | **inna mapa dotykowa**, więc `Patch` zakazane; ale nie `Full` — pełne w rytmie pisania to sekunda migotania i objaw, przez który klawiatura była nie do użycia |
| konfiguracja, `Back` | `Full` + `Pop` | nie | wracamy na agendę |
| konfiguracja, `Save` | `Full` + `Sleep` | nie | „Zapisano" i koniec okna |

### 1.5. `Paint` jako kontrakt tonalny — jedyna rzecz, która czyni ten enum nośnym

Cztery warianty nabierają sensu dopiero z regułą, którą sprzęt już narzucił:

> Ekran kończący render przez `quantize_ink` (pięć poziomów) wolno wypchnąć
> **wyłącznie** jako `Full`. `Fast` i `Patch` wolno zadeklarować tylko ekranowi,
> który kończy render przez `quantize2`.

Powód jest w tablicy przebiegów, nie w estetyce. MODE_DU dla `to = 1..14` ma
`{0x00,0x00,0x00,0x00}` we wszystkich pięciu fazach — piksel o docelowym poziomie
1–14 **nie dostaje żadnego impulsu** i zostaje na szkle tym, czym był. To nie jest
„duch" ani „inny odcień", tylko piksel nienarysowany. Histogram agendy
(3 dni × 4 wydarzenia, po `quantize_ink`):

```
poziom  0:   5 940 px  (1,15%)
poziom  1:  12 658 px  (2,44%)   <- bez impulsu
poziom  2:   6 267 px  (1,21%)   <- bez impulsu
poziom  3:   5 060 px  (0,98%)   <- bez impulsu
poziom  4:   2 669 px  (0,52%)   <- bez impulsu
poziom 15: 485 806 px (93,71%)
atrament razem 32 594 px, z tego 26 654 px = 81,8% BEZ IMPULSU
```

Agenda na `Fast` narysowałaby 18% atramentu nowej strony i zostawiła 82% starej.
Ta reguła jest dziś zapisana w trzech komentarzach (`canvas.rs:576`, `layout.rs:213`,
`shapes.rs:141`) i w żadnym teście. Ma zostać zapisana raz, mechanicznie:

```
dla każdego ekranu, który kiedykolwiek zwraca Fast albo Patch:
    assert!(canvas.pixels().iter().all(|&p| p == BLACK || p == WHITE))
```

Jedno zdanie, łapie każdy przyszły ekran, koduje pomiar zamiast go powtarzać w prozie.

Drugi test — poprawność `Patch` — kopiuje kształt istniejącego
`przyrostowe_odrysowanie_zgadza_sie_z_pelnym` (`layout.rs:2389`) i musi mieć **dwa
stany**: render S0 → patch do S1 → porównanie z pełnym renderem S1 piksel w piksel na
sumie prostokątów, plus `assert_eq!(hits(S0), hits(S1))`. Wersja jednostanowa
przechodzi trywialnie i dawałaby fałszywe poczucie bezpieczeństwa.

### 1.6. Model traci `page` i `focus`

W **tym samym** commicie, razem z `layout.rs`, `preview`, `simulator` i testami.
Zasięg policzony greppem: `layout.rs` :490, :511, :1056 + 6 miejsc w testach;
`preview` :69, :74, :100; `simulator/device.rs` 12 miejsc + 6 asercji;
`firmware` :1238, :1251, :1264, :1277. Około **30 punktów**. To jest zmiana publicznego
API `dashboard` i musi wejść jednym ruchem — inaczej są dwa źródła prawdy o numerze
strony i wracamy do punktu wyjścia.

`simulator/src/device.rs` przestaje mieć własne `apply` i zaczyna wołać `Scene::on`.
Tu umierają trzy zmierzone rozjazdy między symulatorem a urządzeniem: przycinanie
strony, `RefreshNow` i pusty `Setup` przy `OpenSetup`.

**Kolizja nazw:** `preview/src/main.rs:30` ma już własny `enum Scene`
(`Dash` / `Config` / `Month` / `TestCard` / `Uniformity`). Albo go wchłonąć, albo
nazwać nowy inaczej. To decyzja do podjęcia przed pierwszą linijką kodu.

### 1.7. Bilans i kryteria odbioru

**Odświeżeń nie przybywa ani nie ubywa.** To jedyna zgodna pozycja bilansu i tak trzeba
tę zmianę sprzedać: kupuje porządek i dwa naprawione błędy, nie milisekundy.

Złożoność, uczciwie: `screen.rs` (Scene + Nav + Event + Out + Paint + Request + Ctx)
≈ 250–300 linii, `glass.rs` ≈ 110–130, minus ~120 z `main.rs` (znikają `paint`,
`render_frame`, `repaint`, `repaint_setup`, `render_setup_frame`, `flash_region`),
minus ~101 z `simulator/device.rs`, plus dwa testy. Razem plus ~150 linii netto
i zmiana publicznego API `dashboard`.

**Kryteria odbioru, sprawdzalne bez sprzętu:**
* „wpisz sześć znaków w konfiguracji" daje tyle samo wypchnięć co dziś — jedno na serię,
  nie jedno na znak,
* `hold_power(false)` nadal pada po 6 s ciszy,
* `Back` na konfiguracji wraca na agendę bez zapisu (dziś niemożliwe),
* strona agendy przeżywa wejście w szczegóły i powrót.

**Kryteria odbioru na sprzęcie** (`docs/bringup.md:132-142`):
* dokładnie jedna linia `klatka: N ms (render …, panel …, K obszarów)` na znak,
* zero linii `czyszczenie duchów w przerwie` **między** znakami,
* dokładnie jedna sekwencja odświeżenia na stuknięcie w oknie agendy.

Jeśli którekolwiek z tych się zmieni, refaktor zapłacił energią za porządek i trzeba
go cofnąć.

---

## 2. Widoki kalendarza

### 2.1. Horyzont — fakt, który przesądza o wszystkich trzech widokach

`main.rs:69` ustawia `HORIZON_DAYS = 14`, a `main.rs:613` liczy `from = dziś 00:00`,
`to = from + 14 dni`. Urządzenie zna **wyłącznie** okno [dziś 00:00, dziś+14).
Przeszłości nie zna (deep sleep gasi PSRAM), dalszej przyszłości też nie.

Dziś `month.rs:115` (`covered`) wyprowadza „co wiemy" z `model.days.first()` i `.last()`,
a `group_by_day` (`main.rs:710`) **nie tworzy `DayGroup` dla dni bez wydarzeń**.
Skutek: wolny dzień wewnątrz horyzontu dostaje woal „nie pytano", czyli kłamstwo
w drugą stronę niż to, przed którym broni się komentarz przy `covered`.

**Bierzemy — trzy zmiany, zero nowego rysunku, zero kosztu odświeżania:**

1. `Model` dostaje `known: Option<(NaiveDate, NaiveDate)>` — „o tych dniach urządzenie
   pytało". `Model::empty` daje `None`. `build_model` (`main.rs:696`) przekazuje
   `from.date()` i `(to − 1 dzień).date()`. `covered` czyta `model.known`, a `first()`/
   `last()` zostaje fallbackiem. **~15 linii, jeden test** („dzień bez wydarzeń wewnątrz
   horyzontu NIE dostaje woalu").
2. `draw_day` (`month.rs:~232`) dostaje `past: bool`. Trzy stany wychodzą wtedy z samej
   daty i horyzontu, bez ani jednego dodatkowego piksela atramentu:

   | Warunek | Rysunek | Znaczenie |
   |---|---|---|
   | data < dziś | numer w `INK_FAINT`, czysto | minione — nie interesuje nas |
   | dziś ≤ data < dziś+14 | numer `BLACK`, paski = gęstość | **zmierzone**; brak pasków = nic nie ma |
   | data ≥ dziś+14 | woal `dither_rect(…, 1)` (już jest, `month.rs:245`) | **nie pytano** |

   ~8 linii, jeden test.
3. Nic więcej. Bez linii horyzontu (jej rolę pełni krawędź woalu, w tym samym miejscu),
   bez etykiety „koniec danych" (stopka `month.rs:208` już mówi „znane: dd.mm – dd.mm",
   a etykieta w 22 px Bold ma ~175 px = 2,4 kratki), bez kreski bazowej (po pkt 1 woal
   już rozróżnia „pusto" od „nie wiem"; kreska byłaby trzecim zapisem tej samej rzeczy).

### 2.2. Widok miesięczny — istnieje, ale jest nieosiągalny

`render_month` (`month.rs`) jest kompletny i wywoływany **wyłącznie z `preview`**.
W `hit::Action` nie ma wariantu, który by go otwierał, `dashboard::render` do niego nie
dyspozycjonuje, a sam widok wystawia **zero obszarów dotykowych** (`month.rs:124`).

Zmierzona geometria (z `Grid::of`, `month.rs:67-80`, `margin` 12, `HEAD_H` 96,
`FOOT_H` 56):

| Orientacja | usable | kratka |
|---|---|---|
| pion 540 × 960 | 516 × 808 | **73 × 134** |
| poziom 960 × 540 | 936 × 388 | **133 × 64** |

Żeby widok trafił na szkło, potrzeba trzech rzeczy i **wszystkie trzy wymagają warstwy
ekranów**: `Action::OpenMonth`, obszary dotykowe w `month.rs`, droga powrotna. Kolejność
jest sztywna: najpierw warstwa ekranów, potem to. Widok miesięczny **nie jest dostawą
refaktoru** i nie wolno go tak sprzedawać.

Otwarte: czy miesiąc to trzeci poziom stosu, czy zamiana korzenia (`agenda ⇄ miesiąc`).
Zamiana korzenia zostawia `Nav` dwuelementowy i jest tańsza; przy `Push` z agendy stos
i tak ma głębokość dwa. Rozstrzygnięcie należy do właściciela.

Widok miesięczny robi się przy tym uczciwy dopiero przy horyzoncie ≥ 31 dni. Dziś,
pierwszego dnia miesiąca, pokrywa 45% kratek.

### 2.3. Czy podnieść `HORIZON_DAYS` — tak, ale po jednym pomiarze

Rozszerzenie okna **nie kosztuje ani jednego bajtu transmisji** (ICS przychodzi w całości
niezależnie od okna). Koszt to wyłącznie ekspansja RRULE i RAM. Zmierzone na hoście
(release, fikstura 74 268 B, 312 `VEVENT`):

| Okno | Wydarzeń | Parsowanie (host) | `CalEvent` 80 B | Łańcuchy |
|---|---:|---:|---:|---:|
| 14 dni | 80 | 0,44 ms | 6 160 B | 1 661 B |
| 42 dni (szac.) | ~170 | ~0,6 ms | ~13 600 B | ~4 000 B |
| 366 dni | 1517 | 0,92 ms | 118 480 B | 36 171 B |

Ekspansja to ułamek pracy — dominuje przepuszczenie 74 KB przez parser linii, identyczne
w obu oknach. Przy pesymistycznym przeliczniku 50× na ESP32-S3 okno roczne to +24 ms
i +2,9 mAs na wybudzenie wobec ~360 mAs, czyli **+0,8%**. Energia nie jest tu problemem
i nie wolno tak tego uzasadniać.

Problemem jest **wewnętrzny DRAM**. `CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL=4096`
przypina każdą alokację poniżej 4 KB do wewnętrznego DRAM-u — a łańcuchy tytułów to
tysiące takich alokacji, równocześnie z mbedTLS. Przy 366 dniach to ~36 KB samych
łańcuchów (realnie 55–70 KB z narzutem alokatora); przy 42 dniach ~4 KB, czyli rząd
wielkości poniżej progu ryzyka.

**Decyzja: 42 dni jest tanie, ale wchodzi dopiero po jednym wgraniu z podbitą stałą
i odczytaniu `wolny_dram_kb()` z logu przy każdym źródle.** Okno roczne odrzucone.

Uwaga na marginesie: `FREQ=DAILY` bez `UNTIL` przy oknie 366 dni daje 200 wystąpień
z 366 (`MAX_OCCURRENCES`, `feed.rs:34`), sygnalizowane wyłącznie przez `warn!`.
Przy 42 dniach limit nie jest ruszany.

### 2.4. Widok tygodniowy — pion tak, siatka godzinowa nie

**Pion 540 × 960: siedem pasm.** Minione dni tygodnia zwijają się do skoku 40 px
(sam numer w `INK_FAINT` + skrót dnia), reszta dzieli resztę. Obszar pasm y 120..870
= 750 px, skok wiersza wydarzenia 26 px, wyściółki 10/18:

| Minionych dni | Skok pasma | Wydarzeń w paśmie |
|---:|---:|---:|
| 0 (poniedziałek) | 107 px | 3 |
| 1 | 118 px | 3 |
| 2 | 134 px | 4 |
| 3 | 157 px | 4 |
| 4 | 196 px | 6 |
| 5 | 275 px | 9 |
| 6 (niedziela) | 510 px | 18 |

Wnętrze pasma: rynna daty x 32..124 (92 px) — numer `TEXT_HEAD` 34 Bold, skrót dnia
`TEXT_BODY` 22 Medium `INK_DIM`; kolumna godziny x 132..180 (48 px, „08:30" przy 22
Medium = 41,4 px); tytuł x 188..508 = **320 px ≈ 40 znaków** przy 22 Medium.

Tytuł nagłówka: „18–24 sierpnia" przy `TEXT_TITLE` 44 Bold = 228,5 px, mieści się
w 368 px. Tydzień przez granicę miesiąca **musi** skracać nazwy („31 sie – 6 wrz"),
bo pełne „31 sierpnia – 6 września" ma 378,7 px i nie wchodzi.

**Poziom 960 × 540: siatka godzinowa — odrzucona.** Arytmetyka: użyteczna wysokość
540 − 104 − 56 = 380 px na 07:00–23:00, czyli **23,75 px na godzinę**. Wydarzenie
półgodzinne to 11,9 px — poniżej `TEXT_FLOOR` 19, więc **nie da się go podpisać**.
Kolumna dnia to (960 − 64)/7 = 128 px ≈ 18 znaków przy 19 px Medium; dwa nakładające
się wydarzenia dzielą kolumnę na pół → 64 px ≈ 9 znaków. Siatka godzinowa, w której
połowa bloków nie ma podpisu, to ozdoba, nie widok. Jeśli tydzień ma być w poziomie,
to jako te same siedem pasm obrócone, nie jako siatka.

**Ten widok nie ma dziś zamówienia ani drogi wejścia** (`Action` bez `OpenWeek`).
Jego przyjęcie to otwarta decyzja, nie część tego planu.

### 2.5. Mapa gęstości w RTC pod widok roczny — odrzucona (sam widok: zbudowany)

Propozycja: drugi, szeroki przebieg parsera zasilający licznik `[u8; 366]` (366 B, albo
183 B pakowane po 4 bity) w pamięci RTC, przeżywający deep sleep.

Odrzucona z czterech powodów, z których każdy wystarczy:
* wymaga okna 366 dni, czyli 55–70 KB wewnętrznego DRAM-u obok mbedTLS (2.3),
* `MAX_OCCURRENCES = 200` obcina reguły dzienne przy 366 dniach, więc mapa byłaby
  systematycznie zaniżona i nie byłoby tego jak zauważyć,
* wymaga bumpa `MAGIC` w `RtcState`, co jednorazowo kasuje zbuforowany AP
  (~300 mAs — największa pojedyncza dźwignia w budżecie),
* bramkowanie szerokiego przebiegu przez `last_content_crc` nie działa: CRC jest znane
  dopiero **po** pobraniu i sparsowaniu, a mapa ma powstać z tego samego przebiegu.

#### Sprostowanie: odrzucona była mapa, nie widok

Ta sekcja odrzucała **mapę gęstości**, a wnioskiem objęła cały widok roczny. To był
błąd w rozumowaniu i wyszedł dopiero, gdy padło pytanie, do czego ten ekran ma służyć.

Widok roczny nie służy do gęstości. Służy do **struktury**: gdzie wypadają weekendy
i święta, w jaki dzień tygodnia jest dana data, ile tygodni dzieli dwie daty. Cała
ta treść, poza świętami, jest **liczona z kalendarza**, a nie z pobranych danych —
nie potrzebuje więc ani okna 366 dni, ani `[u8; 366]`, ani bumpa `MAGIC`. Wszystkie
cztery powody odrzucenia dotyczyły mapy i żaden nie dotyczy siatki.

Zbudowany widok jest w `dashboard/src/year.rs`: dwanaście wierszy miesięcy, trzydzieści
jeden kolumn dni, weekendy w rastrze (niedziela ciemniejsza od soboty, żeby pas miał
kierunek), kreska na początku poniedziałku, święto pełnym atramentem, dzisiaj ramką.

#### Co z tego naprawdę potrzebuje szerokiego okna

Tylko **święta**. I to jest tania rzecz, bo kanał świąt to osobne źródło
(`SourceTag::Holiday`) liczące ~13 wydarzeń rocznie — całodniowych, bez reguł
powtarzania, więc `MAX_OCCURRENCES` go nie obcina. Szerokie okno dla tego jednego
źródła nie ma nic wspólnego z 55–70 KB, które pochłonęłoby okno roczne kalendarza
głównego.

Kierunek do rozważenia: **horyzont per źródło** zamiast jednego globalnego —
`HORIZON_DAYS` dla kalendarzy z treścią, pełny rok dla kanału świąt. Do czasu takiej
zmiany widok roczny rysuje prawdziwą siatkę i prawdziwe weekendy, a święta pokazuje
tylko w pobranym oknie, co stopka mówi wprost.

---

## 3. Zadania

### 3.1. Czego się nie da: OAuth na szkle

Google Tasks API przez OAuth2 z sekretami wpisywanymi na urządzeniu — **odrzucone**,
i to nie z powodu energii ani DRAM-u, tylko z powodu wpisywania.

Przebieg policzony na `Setup::apply` i `tail_that_fits` na prawdziwych danych:

| Pozycja | Liczba |
|---|---:|
| stuknięć na komplet `client_id` + `client_secret` + `refresh_token` | **282** |
| z tego wywołujących pełne przemalowanie ekranu | **142** |
| sam `refresh_token` (104 znaki base64url) | 156 stuknięć, 102 pełne klatki |
| znaków tokenu widocznych w polu po wpisaniu (pion, `TEXT_TITLE` 44) | **23 ze 104** |

Powód liczby przemalowań: `setup.rs:295-301` — `Action::Caps` zwraca
`Applied::Relayout`, a konsumpcja `Caps::Once` zwraca `Applied::Relayout` **drugi raz**.
Token base64url ma ~50% wielkich liter, więc każda z nich to dwa stuknięcia i dwie pełne
klatki.

Powód liczby widocznych znaków: `draw_edit_field` (`layout.rs:1519`) pokazuje **ogon**,
a `layout.rs:1532` mówi wprost, że „pole nie ma nawigacji w środku tekstu i to jest
świadome". 81 znaków tokenu nigdy nie da się obejrzeć po wpisaniu. Jedyna naprawa
literówki to 104 backspace'y i od nowa.

Zamknięcie: literówka daje `invalid_grant`. Pułapka siedmiu dni bez odświeżenia też daje
`invalid_grant`. **Ten sam błąd** — urządzenie na ścianie nie ma jak powiedzieć, czy
Google unieważnił token, czy pomyliłeś znak 47.

Do tego układ: `Field::ALL` to `[Field; 6]`; trzy pola więcej to dziewięć zakładek,
a `tab_w` w pionie spada z 74 px na 46 px i `fonts.truncate` robi z etykiet `cli…`,
`se…`, `to…`. Siódma zakładka daje ~64 px i jest na granicy — **przed dołożeniem
siódmego pola trzeba to obejrzeć w `cargo run -p preview -- setup`**, ósmej nie ma
gdzie postawić bez przeprojektowania układu.

I rzecz rozstrzygająca: zgody i tak **nie da się udzielić na urządzeniu** — nie ma
przeglądarki. Token trzeba wybić na komputerze. Skoro komputer i tak musi być w pętli,
to nie ma żadnego powodu, żeby sekrety z niego schodziły na szkło.

### 3.2. Co się da: zadania jako feed

Trzy drogi, w kolejności taniości:

**(a) `VTODO` w kanale ICS.** `icalfeed` dziś jawnie pomija `VTODO`
(`feed.rs:115` — razem z `VTIMEZONE`, `VALARM`, `VJOURNAL`, `VFREEBUSY`). Nextcloud,
Radicale, Fastmail i każdy serwer CalDAV publikują listy zadań jako `VTODO` z `DUE`,
`STATUS`, `PERCENT-COMPLETE` i — co ważne — z tym samym `RRULE`, który parser już umie
rozwijać. Koszt: rozszerzenie parsera o drugi typ komponentu, typ `Task` w modelu,
sekcja w renderze. Zero nowych sekretów, zero nowych pól konfiguracji, jeśli zadania
wejdą przez istniejący drugi adres iCal.

**(b) Przekaźnik.** Google Tasks **nie pojawia się w eksporcie ICS** kalendarza Google
w żadnej postaci, więc dla Google droga (a) nie istnieje. Jedyna droga to maszyna,
która i tak trzyma poświadczenia (komputer albo funkcja w chmurze) i publikuje
przetłumaczony feed pod adresem, którego nie da się zgadnąć. Urządzenie trzyma wtedy
jeden URL w NVS — dokładnie to, co już umie. **To jest decyzja właściciela, nie
inżynierska**: albo godzi się utrzymywać przekaźnik, albo zadania są ograniczone do
dostawców z (a).

**(c) Nic.** Też jest odpowiedzią. Urządzenie pokazuje kalendarz; zadania bez terminu
nie mają czego robić w widoku, który jest osią czasu.

### 3.3. Jak zadania mają wyglądać, jeśli wejdą

Zadanie nie ma godziny rozpoczęcia — ma termin. Wpuszczenie go w drabinę godzin agendy
znaczyłoby, że kolumna godziny raz znaczy „zaczyna się", a raz „ma być zrobione".
Wobec tego: osobna sekcja pod agendą albo osobny ekran, ograniczona do zadań z terminem
wewnątrz horyzontu, posortowana po terminie, z liczbą pozostałych w podpisie.
Zadania przeterminowane — `INK_FAINT`, tak jak minione dni w widoku miesięcznym,
żeby ton znaczył jedno i to samo w całym urządzeniu.

**Do rozstrzygnięcia przed pisaniem kodu:** czy zadania dzielą horyzont z kalendarzem
(14/42 dni), czy mają własny, dłuższy. Zadanie z terminem za trzy miesiące bywa
ważniejsze niż spotkanie za trzy dni.

---

## 4. Powiadomienia

### 4.1. Czym ten sprzęt w ogóle dysponuje

| Kanał | Jest? | Uwagi |
|---|---|---|
| e-papier | tak | pasywny — działa tylko wtedy, gdy ktoś patrzy |
| podświetlenie (frontlight) | tak, `BL_EN` = GPIO11, PWM, `PT4103B23F` | **jedyny aktywny kanał uwagi**; pobór niezmierzony |
| brzęczyk | **nie** | nie ma go na płytce |
| wibracja | **nie** | j.w. |
| dioda sterowalna z MCU | **nie** | dioda ładowania należy do BQ25896 |
| budzenie o zadanej porze | tak | timer ESP albo alarm PCF8563 (`RTC_INT` → GPIO2, domena RTC) |
| budzenie dotykiem | tak | `WAKE_ON_TOUCH = true`, `T_INT` → GPIO3 |
| połączenie trwałe / push | **nie** | radio i panel nigdy naraz; urządzenie śpi |

Wszystko poza samym obrazem wymaga, żeby urządzenie **nie spało**. A śpi prawie zawsze.

### 4.2. Co z tego wynika dla opóźnienia

Domyślna kadencja to `active_interval_s = 1800` w oknie 07:00–23:00 i jedno długie
spanie w nocy (`devlogic/src/policy.rs`). Najgorsze opóźnienie „dowiedzenia się"
o czymkolwiek to **30 minut**. Skrócenie kadencji do minuty kosztuje
1440 × 360 mAs = 518 400 mAs = **144 mAh/dobę**, czyli 19× zmierzoną konsumpcję —
ogniwo na osiem dni zamiast pięciu miesięcy. Odpada.

Ale powiadomienie o wydarzeniu z kalendarza **nie wymaga sieci**: godzina wydarzenia
jest znana już przy poprzednim pobraniu. Wymaga tylko, żeby przeżyła sen — a treść snu
nie przeżywa. Zostaje pamięć RTC.

### 4.3. Kształt, który jest wykonalny

Przy każdym wybudzeniu, po zbudowaniu modelu, do `RtcState` idzie znacznik najbliższego
wydarzenia; czas snu to `min(polityka, start_wydarzenia − wyprzedzenie)`. Na wybudzeniu
powiadamiającym urządzenie **nie sięga po sieć** — rysuje ekran wyłącznie z tego, co
leży w RTC.

Budżet RTC: 8 B (unix startu) + 1 B (flagi) + do 64 B (tytuł) = **73 B**. Dziś zajęte
jest ~48 B z „kilkuset dostępnych"; miejsca jest dość. Jednorazowy koszt: bump `MAGIC`
w `rtc_state.rs`, czyli utrata zbuforowanego AP przy pierwszym wybudzeniu po wgraniu
≈ **300 mAs**.

Koszt bieżący jednego powiadomienia:

| Pozycja | mAs |
|---|---:|
| boot do gotowości (312 ms × ~45 mA) | 13,5 |
| pełne odświeżenie (250 ms × 115 mA) | 28,8 |
| **razem, bez radia** | **~42** |
| pięć powiadomień na dobę | 210 mAs = 0,06 mAh |
| dla porównania: cała zmierzona doba | 27 720 mAs = 7,7 mAh |

Czyli **0,8% doby na pięć powiadomień**. Powiadomienia z RTC są tanie i to jest
zaskakująco dobra wiadomość — droga jest sieć i okno dotyku, nie wybudzenie.

### 4.4. Czego to nie umie i o czym trzeba zdecydować

* **Powiadomienie na e-papierze jest niewidoczne, dopóki nikt nie patrzy.** Nie ma
  „ping". Jedyna forma, która ma szansę zadziałać, to zmiana **całej góry ekranu** —
  pasek w negatywie, `TEXT_HERO`/`TEXT_TITLE`. Mała plakietka w rogu nie jest
  powiadomieniem, tylko dekoracją.
* **Podświetlenie jest jedynym kanałem działającym bez patrzenia** — i wyłącznie
  w ciemnym pomieszczeniu. Jego pobór **nie jest zmierzony**; szacunek dla tej klasy
  sterownika to 20–60 mA, czyli impuls 3 s = 60–180 mAs, dwa do sześciu razy drożej niż
  całe wybudzenie z odświeżeniem. **Nie wolno tego obiecywać przed pomiarem** —
  a pomiar jest łatwy, bo licznik kulombów jest na płytce.
* **Noc.** `Mode::Night` śpi jednym ciągiem do 07:00. Powiadomienie o wydarzeniu
  o 07:15 wymaga wybudzenia w oknie nocnym, czyli wyjątku w polityce. Wyjątek trzeba
  zamówić świadomie — inaczej urządzenie zacznie się budzić w nocy i nikt nie będzie
  wiedział dlaczego.
* **Wybudzenie powiadamiające pokazuje treść sprzed do 30 minut.** Odwołane spotkanie
  zostanie zapowiedziane. To jest cena za brak radia w tej ścieżce i trzeba ją przyjąć
  świadomie albo dopłacić 360 mAs za pobranie przy każdym powiadomieniu.

---

## 5. Budżet

### 5.1. Energia

| Operacja | Koszt | % doby (27 720 mAs) |
|---|---:|---:|
| Odświeżenie DU (35 ms × 115 mA) | 4,0 mAs | 0,014% |
| Pełne odświeżenie (250 ms × 115 mA) | 28,8 mAs | 0,10% |
| Mignięcie pod palcem (`ack`) | 4,0 mAs | 0,014% |
| Boot do gotowości (312 ms) | 13,5 mAs | 0,05% |
| Wybudzenie z siecią, zoptymalizowane | 360 mAs | 1,3% |
| Wybudzenie z siecią, zimne (pełny skan) | 850 mAs | 3,1% |
| Nieudana próba sieciowa | 200–480 mAs | 0,7–1,7% |
| **Okno dotyku, agenda (20 s × 40 mA)** | **800 mAs** | **2,9%** |
| **Okno dotyku, konfiguracja (90 s × 40 mA)** | **3 600 mAs** | **13,0%** |
| **Szyny trzymane 6 s (`POWER_HOLD_IDLE_MS`)** | **690 mAs** | **2,5%** |
| Powiadomienie z RTC (boot + Full) | 42 mAs | 0,15% |
| Impuls podświetlenia 3 s (**niezmierzone**) | 60–180 mAs | 0,2–0,6% |
| Bump `MAGIC` w `RtcState` (jednorazowo) | ~300 mAs | 1,1% |
| Suma powyższych pozycji (**model, nie pomiar**) | 27 720 mAs | 100% |
| Ogniwo użyteczne (1200 mAh) | 4 320 000 mAs | 155 dób |

> **Ta tabela nie zgadza się z rzeczywistością i to jest jej najważniejsza informacja.**
> Suma modelu daje 7,7 mAh na dobę, czyli średnio **0,32 mA**. Obserwacja na sprzęcie to
> **16–30 mA**, czyli pięćdziesiąt do stu razy więcej — przy takim prądzie doba wynosiłaby
> 1,4–2,6 Ah, a więc więcej niż całe ogniwo. Producent zmierzył na tej samej płytce
> **873 µA** (`vendor/README.md:171`, Victor 8246A), co jest jedyną liczbą z miernika,
> jaką ktokolwiek ma dla tego sprzętu.
>
> Model opisuje więc koszt tego, co robimy świadomie, i w tej roli jest użyteczny.
> Nie opisuje natomiast prądu spoczynkowego — a to on decyduje o tygodniu pracy.
> Dopóki nie wiemy, co pobiera te kilkanaście miliamperów, **żadna pozycja poniżej nie
> jest dźwignią**: wszystkie razem toną w tle, którego nie rozumiemy.

Wniosek, który trzeba trzymać przy każdej decyzji projektowej: **dobór Full/Fast/Patch
nie jest decyzją energetyczną.** Jedna klatka to 1/138 do 1/620 kosztu okna, w którym
się wydarza. Jest to decyzja o czasie odpowiedzi i o duchach. Decyzją energetyczną jest
długość okna i trzymanie szyn.

### 5.2. Pamięć

| Pozycja | Dziś | Po zmianie |
|---|---:|---:|
| `Gray8` (1 płótno) | 518 400 B | 518 400 B |
| Szczyt płócien jednocześnie | **3 × = 1 555 200 B** | **1 × = 518 400 B** |
| epdiy (`front_fb` + `back_fb`) | 518 400 B | 518 400 B |
| PSRAM łącznie / dostępne | ~2,1 MB / 8 MB (26%) | ~1,0 MB / 8 MB (13%) |
| `Scene` / `Nav` (stos, host) | — | 152 B / 304 B |
| `Nav` na xtensa (wskaźnik 32-bit) | — | ~170 B = 0,5% stosu 32 768 B |
| `Out` | — | 24 B (+16 B przy `Patch`) |
| `Fonts` | 8 472 B na egzemplarz, dziś tworzone wielokrotnie | jeden na wybudzenie |
| Okno 14 dni: `CalEvent` + łańcuchy | 7,8 KB | 7,8 KB (42 dni: ~17,6 KB) |
| `RtcState` | ~48 B z „kilkuset" | +73 B przy powiadomieniach |

Oszczędność 1,04 MB PSRAM jest realna, ale **nie wolno jej wpisywać w uzasadnienie**:
zasób jest w 74% pusty, `CONFIG_SPIRAM_MALLOC_ALWAYSINTERNAL=4096` i tak nie wpuszcza
518 KB do wewnętrznego DRAM-u, a konflikt epdiy ↔ mbedTLS jest tą zmianą nietknięty
w obie strony. Zysk = 0. Wartością jest jedno źródło prawdy, nie megabajt.

### 5.3. Dane i czas CPU

| Pozycja | Wartość |
|---|---|
| Kanał ICS (fikstura) | 74 268 B, 312 `VEVENT` |
| Transmisja na wybudzenie | pełne 74 KB (okno nie zmienia transmisji) |
| Przy 33 wybudzeniach na dobę | ~2,4 MB/dobę |
| Parsowanie, okno 14 dni (host) | 0,44 ms → ~22 ms na S3 (×50) |
| Parsowanie, okno 42 dni (host, szac.) | ~0,6 ms → ~30 ms na S3 |
| Pełny render agendy | 9 ms zmierzone dla 3 obszarów; pełna klatka ~45 ms |
| Pełny render konfiguracji (host) | 1,04 ms pion / 1,15 ms poziom |
| Render przyrostowy konfiguracji (host) | 0,207 ms pion / 0,262 ms poziom (**5,0×** taniej) |
| Klatka konfiguracji na sprzęcie | 58 ms (render 9, panel 49, 3 obszary) |

### 5.4. Trwałość

| Pozycja | Stan |
|---|---|
| Cykle ogniwa | 1200 mAh / 7,7 mAh na dobę = **155 dób na cykl**; 300–500 cykli to poza horyzontem projektu |
| HV na szkle bez odświeżania | `epd.rs:257`: „nie służy panelowi" — dlatego `hold_power` musi mieć próg, a nie zostawać do końca okna |
| Duchy po DU | czyszczone `Refresh::Full` w przerwie ≥ 2 s; przy 12 klatkach DU |
| Liczba pełnych odświeżeń na dobę | ~33 (jedno na wybudzenie z odświeżeniem) |
| Wytrzymałość panelu na odświeżenia | **niezmierzone i nieudokumentowane** dla ED047TC1 |
| NVS | zapis wyłącznie przy `Save` (6 kluczy) i przy zmianie obrotu — rzędy wielkości poniżej wytrzymałości sektora |
| `.rtc.data` | ginie przy naciśnięciu RESET (`EN` resetuje domenę RTC) — patrz `docs/hardware.md §5b` |

---

## 6. Czego NIE robimy

Ta lista jest po to, żeby ktoś nie stracił tygodnia. Każda pozycja była policzona.

1. **Nie schodzimy z agendy na `Refresh::Fast` / MODE_DU.** MODE_DU nie sprowadza
   szarości do dwóch poziomów — **on ich nie rusza w ogóle** (`to = 1..14` →
   `{0,0,0,0}` we wszystkich pięciu fazach). 81,8% atramentu agendy jest na poziomach
   1–4. Efekt to nie „lekki duch", tylko suma starej i nowej strony. Warunkiem wejścia
   DU do nawigacji jest `quantize2` na agendzie, a to odwrócenie decyzji podjętej na
   pomiarze ze szkła (`layout.rs:195-214`): przy `TEXT_BODY` 22 i `TEXT_FLOOR` 19
   czytelność bez wygładzenia **spada**. Zmierzone straty przy dwóch poziomach:
   „Stand-up zespołu" 19 px Medium −20% atramentu, „08:30" 19 px −25%, ~10% kresek
   jednopikselowych; przy 27 px szkody znikają (0,6%). Dwupoziomowa agenda jest możliwa
   dopiero po skasowaniu `TEXT_BODY` i `TEXT_FLOOR` z drabiny.
   *Uwaga na symulator:* `simulator/src/device.rs:136-201` modeluje DU jako „poprawna
   klatka + przyciemnienie do 18%", więc **pokaże to jako działające**. Nie jest to
   dowód niczego.
2. **Nie dzielimy jednej zmiany na dwa wypchnięcia.** Obszar nie skraca przebiegu
   (`epd_hl_update_area` taktuje wszystkie bramki), więc dwa wypchnięcia to podwójny
   koszt. Ta sama arytmetyka zabija timer gaszący negatyw pod palcem — został już raz
   usunięty i ma nie wrócić (`main.rs:1440-1455`).
3. **Nie wpisujemy sekretów OAuth na szkle.** 282 stuknięcia, 142 pełne przemalowania,
   23 ze 104 znaków widoczne, `invalid_grant` nieodróżnialny od literówki. Szczegóły
   w 3.1.
4. **Nie dokładamy `trait Surface` teraz.** Trzy metody nie obejmują `hold_power`,
   sklejania klatek ani odkładania duchów, czyli ~235 z 257 linii `setup_screen`.
   Warunek powrotu: czwarta metoda i wstrzykiwany zegar.
5. **Nie robimy `Glass<S: Surface>` generycznego „żeby kompilował się na hoście".**
   Dopóki `simulator/src/device.rs` nie jest przepisany na `Glass`, kupuje to samą
   kompilację.
6. **Nie robimy `Patches { [Rect; 12], u8 }`.** Argument pamięciowy przeciw `Vec` jest
   nieprawdziwy (`clear()` zachowuje pojemność), a ścieżka eskalacji po przepełnieniu
   to kod bez odbiorcy: `MAX_DEFER_MS` = 220 ms wymusza wypchnięcie, a przez 220 ms
   nikt nie naciśnie dwunastu różnych klawiszy. `Paint::Patch(&[Rect])` pożycza listę
   ze stanu ekranu i kosztuje 16 B na stosie.
7. **Nie dodajemy `ctx.pages`.** Liczba stron jest znana dopiero po renderze — pole
   niosłoby stan z poprzedniej klatki, czyli dokładnie ten drugi stan, który usuwamy.
   Strażnikiem jest istnienie obszaru `NextPage`.
8. **Nie dodajemy `debug_assert!(ruch ⇒ Paint::Full)`.** Kod **już świadomie łamie tę
   regułę**: zmiana układu klawiatury wypycha pełną klatkę konfiguracji w trybie DU
   (`main.rs:1512-1526`), i komentarz obok tłumaczy, że `Full` w tym miejscu było wadą.
   Dla dzisiejszych ekranów reguła zachodzi przypadkiem. Komentarz — tak; assert,
   z którym przyszły ekran będzie się bił — nie.
9. **Nie przenosimy `hold_power` poza klawiaturę.** 690 mAs za 4,6–9,2 mAs zysku,
   stosunek 75–150× na stratę.
10. **Nie robimy `trait View` z domyślnym `render_patch` panikującym.** `unreachable!`
    na tym urządzeniu to reset.
11. **Nie robimy `NavCmd::Replace` ani `Scene::Saved`.** „Zapisano" to ostatnia klatka
    przed snem, nie ekran na stosie; po jego usunięciu `Replace` nie ma wołającego.
12. **Nie robimy mapy gęstości w RTC ani okna 366 dni.** Cztery niezależne powody
    w 2.5.
13. **Nie robimy siatki godzinowej w poziomie.** 23,75 px na godzinę; wydarzenie
    półgodzinne = 11,9 px, poniżej `TEXT_FLOOR` 19, więc bez podpisu.
14. **Nie dodajemy `Event::Tick`.** Dziś żaden ekran nie zmienia się sam z siebie.
    Wchodzi dopiero, gdy taki ekran zostanie zamówiony.
15. **Nie liczymy na `RtcState::needs_full_refresh`.** Ta funkcja (`rtc_state.rs:158`,
    czytająca `FAST_REFRESHES_BEFORE_FULL` z `epd.rs`) **nie ma ani jednego wołającego**,
    a pole `fast_refreshes` jest inkrementowane i nigdy czytane. To martwy kod, nie
    mechanizm — albo go usunąć, albo podłączyć, ale nie powoływać się na niego jako na
    istniejące zabezpieczenie.

---

## 7. Kolejność wdrażania

**Etap 0 — dziś, niezależnie od wszystkiego, ~20 linii.**
* `model.page = screen.page;` po `matchu` na akcję w `interactive_loop`. Kasuje pięć
  martwych stuknięć po obrocie (1,15 mAh na naciśnięcie przycisku) i całą klasę tego
  błędu, bo `layout.rs:511` już klamruje.
* Test na hoście na ten przypadek: render w Landscape (12 stron) → `page = 9` → render
  w Portrait (5 stron) → `assert_eq!(screen.page, model.page)`. Dziś wywala się na
  `left: 5, right: 0`.
* `Action::Back` w dolnym rzędzie klawiatury (`layout.rs:1830-1831`, obok „usuń"
  i „zapisz") + ramię w `setup_screen` zwracające `false`. Sześć linii, wada nr 1
  znika przed refaktorem.

**Etap 1 — warstwa ekranów, czysty host.** `dashboard/src/screen.rs`: `Event`, `Out`,
`Paint`, `Request`, `Scene`, `Nav`, `Open`, `NavCmd`. `Model` traci `page` i `focus`
(~30 miejsc, **w tym samym commicie**). `simulator/src/device.rs` woła `Scene::on`.
Dwa testy tonalne z 1.5. Wszystko testowalne bez sprzętu i bez ESP-IDF.
*Przed pierwszą linijką:* rozstrzygnąć kolizję nazwy z `preview::Scene`.

**Etap 2 — `Glass` i przepisanie obu pętli firmware'u.** `firmware/src/ui/glass.rs`,
`interactive_loop` i `setup_screen` biorą `&mut Glass`. Znikają `paint`, `render_frame`,
`repaint`, `repaint_setup`, `render_setup_frame`, `flash_region`. Kryteria odbioru
z 1.7 — sprawdzane na sprzęcie, na logu.

Etapy 3–6 są **wzajemnie niezależne** i mogą iść w dowolnej kolejności po etapie 1
(a etap 3 i 6 nawet przed nim):

**Etap 3 — horyzont w modelu.** `Model.known`, `past` w `draw_day`, dwa testy.
Nie zależy od warstwy ekranów. ~25 linii.

**Etap 4 — pomiar DRAM-u i ewentualne `HORIZON_DAYS = 42`.** Jedno wgranie z podbitą
stałą, odczyt `wolny_dram_kb()` z logu przy każdym źródle. Jeśli zapas jest, stała
zostaje podbita; jeśli nie, wraca 14 i wiemy dlaczego.

**Etap 5 — widok miesięczny na szkle.** Wymaga etapu 1 i 2. `Action::OpenMonth`,
obszary dotykowe w `month.rs` (dziś zero), droga powrotna, decyzja `Push` czy zamiana
korzenia. Sensowny dopiero po etapie 3 i 4.

**Etap 6 — powiadomienia.** `RtcState` + 73 B, bump `MAGIC`, wyliczenie czasu snu
z `min(polityka, wydarzenie − wyprzedzenie)`, minimalny ekran rysowany wyłącznie z RTC.
Nie dotyka warstwy ekranów ani panelu. Przed obietnicą podświetlenia: **pomiar poboru
frontlightu licznikiem kulombów.**

**Etap 7 — zadania.** `VTODO` w parserze + typ w modelu + sekcja w renderze. Zależy od
decyzji z 3.2, nie od reszty planu.

**Etap 8 — widok tygodniowy.** Tylko jeśli zostanie zamówiony. Wymaga etapów 1–2.

---

## 8. Otwarte decyzje

Pytania, na które nie da się odpowiedzieć z kodu.

1. **`Back` na ekranie konfiguracji: porzucać zmiany od razu, czy pytać o
   potwierdzenie?** Potwierdzenie to trzeci poziom stosu i osobny ekran; nie dobudowuję
   go bez zamówienia.
2. **Gdzie w konfiguracji rysuje się „wróć": w pasku zakładek czy w pasku klawiatury?**
   W klawiaturze 1,0 jednostki w pionie = 45 px, a „wróć" przy `TEXT_BODY` 22 ma ~48 px
   tekstu; podłogą jest 1,5 jednostki = 71 px, a wtedy „zapisz" schodzi do 71 px przy
   ~66 px napisu — mieści się na styk. Pasek zakładek jest bezpieczniejszy.
3. **Widok miesięczny to trzeci poziom stosu, czy zamiana korzenia (`agenda ⇄
   miesiąc`)?** Od tego zależy, czy `Nav` zostaje dwuelementowy.
4. **Czy `SLEEP_MARKER` zostaje na stałe, czy to była rzecz bring-upowa?** Wpływa na to,
   czy `Glass::mark_asleep` jest częścią kontraktu, czy tymczasem.
5. **Czy `HORIZON_DAYS` ma iść z 14 na 42?** Pytanie o decyzję, nie o pomiar — pomiar
   jest w etapie 4.
6. **Czy „nie wiem" ma być w ogóle pokazywane, czy widok miesięczny ma się urywać na
   horyzoncie?** Druga opcja jest tańsza i też uczciwa, ale wygląda na uszkodzoną.
7. **Zadania: przekaźnik czy tylko `VTODO`?** Jeśli przekaźnik — kto go utrzymuje i pod
   jakim adresem. Jeśli tylko `VTODO` — Google Tasks nie wejdzie nigdy.
8. **Zadania: własny horyzont czy wspólny z kalendarzem?**
9. **Powiadomienia: czy urządzenie ma się budzić w oknie nocnym?** Bez wyjątku
   w `Mode::Night` powiadomienie o wydarzeniu przed 07:00 nie ma jak przyjść.
10. **Powiadomienia: podświetlenie czy sam obraz?** Odpowiedź warunkowa — najpierw
    pomiar poboru frontlightu, potem decyzja.
11. **Ile realnie wydarzeń ma Twój kanał ICS w skali roku?** Jedno uruchomienie
    na hoście na prawdziwym pliku zamyka całą niepewność wokół widoku rocznego
    i wielkości okna.