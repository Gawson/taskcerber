//! Płótno w skali szarości 8-bit, z konwersją do formatu framebufferu epdiy.
//!
//! Konwencja jasności jest taka sama jak w epdiy: **0 = czarny, 255 = biały**.
//! Płótno startuje białe, bo e-papier to papier.

/// Szerokość panelu ED047TC1 w jego **natywnym** skanie.
///
/// `epd_width()` zwraca 960 niezależnie od ustawień. `epd_set_rotation()` tego nie
/// zmienia — nagłówek epdiy mówi wprost, że przestawia wyłącznie jego własne prymitywy
/// rysujące i fonty. My piszemy prosto do bufora z `epd_hl_get_framebuffer()`, więc
/// dla nas ta funkcja jest pustą instrukcją. Obrót robi [`Gray8::pack4`] i nikt inny.
pub const PANEL_WIDTH: usize = 960;
/// Wysokość panelu w natywnym skanie. Patrz [`PANEL_WIDTH`].
pub const PANEL_HEIGHT: usize = 540;

/// Rozmiar spakowanego framebufferu 4bpp, w bajtach — dokładnie tyle, ile zwraca
/// `epd_hl_get_framebuffer()`. Liczony z wymiarów **panelu**, bo panel się nie obraca.
pub const PACKED_LEN: usize = PANEL_WIDTH / 2 * PANEL_HEIGHT;

/// Orientacja treści na panelu.
///
/// Dwie i tylko dwie — to jest kontrola orientacji, a nie odwracanie do góry nogami.
/// Płótno zmienia przy tym proporcje: poziomo 960×540, pionowo 540×960. Dlatego rozmiar
/// płótna **nie jest stałą kompilacji** — bierze się go z [`Rotation::canvas_size`],
/// a układ graficzny czyta wymiary z samego płótna.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rotation {
    /// Poziomo — zgodnie ze skanem panelu, bez przekształcania współrzędnych.
    Landscape,
    /// Pionowo. Domyślne i sprawdzone na sprzęcie 2026-08-19: **USB-C na dole**.
    #[default]
    Portrait,
}

/// Czy odwrócić poziom o 180°.
///
/// Pion jest sprawdzony na sprzęcie, poziom **nie**. Której strony trzeba, zależy od
/// tego, w którą stronę obrócisz urządzenie z pionu — a tego nie da się wyprowadzić
/// ani ze schematu, ani z pionowego ustawienia. Jeśli poziom wyjdzie do góry nogami,
/// zmień tu na `true`. To jedyne miejsce, które o tym decyduje.
const LANDSCAPE_FLIPPED: bool = false;

impl Rotation {
    /// Rozmiar płótna dla tej orientacji.
    pub const fn canvas_size(self) -> (usize, usize) {
        match self {
            Self::Landscape => (PANEL_WIDTH, PANEL_HEIGHT),
            Self::Portrait => (PANEL_HEIGHT, PANEL_WIDTH),
        }
    }

    /// Czy płótno jest pionowe.
    pub const fn is_portrait(self) -> bool {
        matches!(self, Self::Portrait)
    }

    /// Druga orientacja. To jest to, co robi przycisk.
    pub const fn toggled(self) -> Self {
        match self {
            Self::Landscape => Self::Portrait,
            Self::Portrait => Self::Landscape,
        }
    }

    /// Postać do zapisu w NVS.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Landscape => "landscape",
            Self::Portrait => "portrait",
        }
    }

    /// Przelicza punkt ze współrzędnych **panelu** na współrzędne płótna.
    ///
    /// Dotyk przychodzi z GT911 w układzie panelu (960×540), a obszary z
    /// [`crate::hit::Screen`] są w układzie płótna, które w pionie ma 540×960.
    /// Bez tego przeliczenia dotknięcie lewego górnego rogu trafia w zupełnie
    /// inne miejsce układu.
    ///
    /// To jest **ta sama macierz**, którą `Gray8::panel_sample` stosuje przy
    /// pakowaniu — i musi nią być, bo opisuje jedno i to samo przyklejenie płótna
    /// do szkła. Trzymanie ich obok siebie w jednym pliku jest celowe: rozjazd
    /// objawiłby się jako dotyk chybiający o obrót, czyli objaw, który wygląda
    /// na błąd sterownika dotyku, a nie układu graficznego.
    /// Pilnuje tego test `dotyk_trafia_w_ten_sam_piksel_co_pakowanie`.
    pub const fn panel_to_canvas(self, px: i32, py: i32) -> (i32, i32) {
        match self {
            Self::Landscape if LANDSCAPE_FLIPPED => {
                (PANEL_WIDTH as i32 - 1 - px, PANEL_HEIGHT as i32 - 1 - py)
            }
            Self::Landscape => (px, py),
            // Płótno pionowe ma szerokość PANEL_HEIGHT.
            Self::Portrait => (PANEL_HEIGHT as i32 - 1 - py, px),
        }
    }

    /// Przelicza PROSTOKĄT z układu płótna na układ panelu.
    ///
    /// Ta sama macierz, co [`Rotation::panel_to_canvas`], tylko w drugą stronę i na
    /// całym obszarze. Potrzebna do częściowego odświeżenia: `epd_hl_update_area`
    /// przyjmuje obszar we współrzędnych panelu, a obszary dotykowe z
    /// [`crate::hit::Screen`] są w układzie płótna. Bez tego natychmiastowy feedback
    /// pod palcem odrysowywałby prostokąt obrócony o 90°, czyli w losowym miejscu.
    ///
    /// W pionie zamieniają się też wymiary: prostokąt `w × h` na płótnie jest
    /// `h × w` na panelu.
    pub const fn canvas_rect_to_panel(self, r: Rect) -> Rect {
        match self {
            Self::Landscape if LANDSCAPE_FLIPPED => Rect::new(
                PANEL_WIDTH as i32 - r.right(),
                PANEL_HEIGHT as i32 - r.bottom(),
                r.w,
                r.h,
            ),
            Self::Landscape => r,
            // panel_x = canvas_y, panel_y = PANEL_HEIGHT - 1 - canvas_x.
            Self::Portrait => Rect::new(r.y, PANEL_HEIGHT as i32 - r.right(), r.h, r.w),
        }
    }

    /// Odczyt z NVS. Cokolwiek nierozpoznanego daje wartość domyślną — konfiguracja
    /// z przyszłej wersji nie ma prawa zablokować bootu.
    ///
    /// `cw`/`ccw` to nazwy z wersji, w której obroty były dwa, ale oba pionowe;
    /// czytamy je, żeby urządzenie zaktualizowane przez OTA nie zgubiło ustawienia.
    pub fn parse(s: &str) -> Self {
        match s {
            "landscape" => Self::Landscape,
            "portrait" | "cw" | "ccw" => Self::Portrait,
            _ => Self::default(),
        }
    }
}

// Założenia, na których stoi obrót w `pack4`. Sprawdzane przy kompilacji.
const _: () = assert!(PANEL_WIDTH > PANEL_HEIGHT, "panel skanuje poziomo");
const _: () = assert!(
    PANEL_WIDTH.is_multiple_of(2),
    "pakowanie mieści dwa piksele w bajcie, więc szerokość panelu musi być parzysta"
);
const _: () = assert!(
    PANEL_HEIGHT.is_multiple_of(2),
    "w pionie wysokość panelu staje się szerokością płótna — też musi być parzysta"
);

// ---------------------------------------------------------------------------
// Paleta — ZMIERZONA NA SZKLE, nie wyprowadzona z noty katalogowej
// ---------------------------------------------------------------------------
//
// Panel deklaruje szesnaście poziomów. Karta tonów (`crate::testcard`, wgrana
// 2026-08-20) pokazuje, że jako **kanał informacji** działa z tego najwyżej jedna
// trzecia, a charakterystyka jest mocno przesunięta w stronę bieli:
//
//   poziom 0-3 (0x00-0x33)   atrament. Nadaje się na tekst, kreski i grafikę.
//                            Poziom 4 (0x44) jest granicą — jeszcze się czyta,
//                            ale już nie w cienkim kroju.
//   poziom 5-8  (0x55-0x88)  tylko tła i plamy. Widać, że coś tam jest; nie da
//                            się na tym postawić litery ani kreski 1 px.
//   poziom 9    (0x99)       zewnętrzna granica. Czasem widać, czasem nie.
//   poziom 10-15 (0xAA-0xFF) do olania. Sens mają WYŁĄCZNIE jako ogon
//                            antyaliasingu albo w grafice o płynnym tonie —
//                            nigdy jako samodzielny element interfejsu.
//
// Czyli z szesnastu deklarowanych poziomów użyteczna jest POŁOWA, a na tekst
// i kreski — pierwsza ćwiartka.
//
// Poprzednia paleta była nazwana procentami jasności i to wprowadzało w błąd:
// `GRAY_20` sugerowało „20% szarości, czyli prawie czarne", a na szkle wychodziła
// z tego średnia szarość. `GRAY_50`, `GRAY_60` i `GRAY_80` były po prostu
// NIEWIDOCZNE — a wisiały na nich pasek baterii, kropki stron, linie włosowe
// i tytuły wydarzeń minionych. Nazwy mówią teraz, do czego wolno użyć, a nie ile
// procent czerni to miało być.
//
// Jasne wypełnienia robi się [`dither_rect`], nie półtonem: dithering używa
// wyłącznie czerni i bieli, czyli dwóch stanów, które waveform dowozi pewnie.
// Przy tej samej gęstości wychodzi wyraźnie ciemniejszy i równiejszy od półtonu —
// widać to na karcie gołym okiem, a przy odświeżeniu częściowym (MODE_DU, które
// półtonów nie umie w ogóle) jest jedyną działającą opcją.

pub const BLACK: u8 = 0x00;

/// Atrament drugoplanowy. Poziom 2 — wyraźnie lżejszy od czerni, wciąż czytelny.
pub const INK_DIM: u8 = 0x22;

/// Granica czytelności. Poziom 3. Poniżej tego tekst przestaje być tekstem,
/// a staje się tym, co użytkownik nazwał „artefaktem".
pub const INK_FAINT: u8 = 0x33;

/// Najciemniejsze wypełnienie, które nie udaje atramentu. Poziom 5.
pub const FILL_DARK: u8 = 0x55;

/// Wypełnienie średnie. Poziom 6.
pub const FILL: u8 = 0x66;

/// Najjaśniejsze wypełnienie, które jeszcze pewnie widać. Poziom 8.
pub const FILL_LIGHT: u8 = 0x88;

/// Powyżej tej wartości panel nie odróżnia niczego od bieli.
///
/// Pilnuje tego test `nic_czytelnego_nie_jest_jasniejsze_niz_granica`. Jeśli
/// kiedykolwiek zajdzie potrzeba użycia wyższej wartości jako samodzielnego
/// elementu — to znaczy, że element ma być ditherowany, nie rozjaśniony.
pub const LIGHTEST_VISIBLE: u8 = 0x99;

pub const WHITE: u8 = 0xFF;

/// Jak szesnaście poziomów wygląda NA SZKLE — zmierzone ze zdjęć karty tonów.
///
/// Czytać tak: poziom 0 nie jest czernią, tylko ciemnym grafitem; od poziomu 10
/// w górę wszystko jest bielą. Pomiędzy jest ściśnięta, wypukła krzywa — stąd
/// wrażenie, że „szarości są w chuj blade".
const PANEL_RESPONSE: [u8; 16] = [
    0x18, 0x28, 0x40, 0x58, 0x70, 0x88, 0x9C, 0xB0, 0xC4, 0xD8, 0xEE, 0xF4, 0xF8, 0xFB, 0xFD, 0xFF,
];

/// Pokrycie, poniżej którego piksel idzie na biel.
///
/// 38/255 to ~15%. Niżej i tak nie ma czym tego pokazać — najjaśniejszy użyteczny
/// poziom atramentu to 4, a między nim a bielą panel nie ma nic.
pub const INK_CUTOFF: u8 = 38;

/// Odwzorowuje wartość płótna na jeden z pięciu poziomów atramentu albo na biel.
///
/// Patrz [`Gray8::quantize_ink`].
pub fn ink_level(v: u8) -> u8 {
    let coverage = 255u16 - v as u16;
    if coverage < INK_CUTOFF as u16 {
        return WHITE;
    }
    // `t` rośnie od 0 przy progu do 1 przy pełnym pokryciu.
    let span = 255u16 - INK_CUTOFF as u16;
    let t = coverage - INK_CUTOFF as u16;
    // Pięć stopni: pełne pokrycie -> 0 (czerń), próg -> 4.
    let level = 4 - ((t * 4 + span / 2) / span).min(4);
    (level as u8) * 0x11
}

/// Macierz Bayera 4×4, wartości 0-15.
///
/// Rozkład uporządkowany: sąsiednie piksele dostają progi maksymalnie od siebie
/// oddalone, więc wzór jest drobny i nie tworzy pasm. Szesnaście progów odwzorowuje
/// dokładnie tyle gęstości, ile poziomów deklaruje panel.
const BAYER4: [[u8; 4]; 4] = [
    [0, 8, 2, 10],
    [12, 4, 14, 6],
    [3, 11, 1, 9],
    [15, 7, 13, 5],
];

/// Wypełnia prostokąt ditherem o gęstości `level`/16 czerni.
///
/// **To jest właściwy sposób na jasne wypełnienie na tym panelu**, a nie półton.
/// Dithering używa wyłącznie czerni i bieli — dwóch stanów, które waveform dowozi
/// pewnie — więc przy tej samej gęstości wychodzi ciemniejszy i równiejszy niż
/// prawdziwa szarość. Przy poziomach, na których półton jest już nieodróżnialny od
/// bieli (2/16, 3/16), dither wciąż wyraźnie widać.
///
/// `level = 0` zostawia biel, `level = 16` daje pełną czerń.
pub fn dither_rect(c: &mut Gray8, r: Rect, level: u8) {
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            let t = BAYER4[(y.rem_euclid(4)) as usize][(x.rem_euclid(4)) as usize];
            c.set(x, y, if t < level { BLACK } else { WHITE });
        }
    }
}

/// Próg, przy którym [`Gray8::quantize2`] rozstrzyga czerń od bieli.
///
/// 0xA8 to pokrycie ~34%, nie 50%. Uzasadnienie przy `quantize2`.
pub const BILEVEL_THRESHOLD: u8 = 0xA8;

/// Prostokąt w pikselach. Przechowywany jako `i32`, żeby układ mógł liczyć poza
/// krawędziami bez paniki — obcinanie dzieje się przy rysowaniu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// Zmniejsza prostokąt o `d` z każdej strony.
    pub const fn inset(&self, d: i32) -> Self {
        Self::new(self.x + d, self.y + d, self.w - 2 * d, self.h - 2 * d)
    }
}

/// Bufor 8-bitowej skali szarości o wymiarach panelu.
pub struct Gray8 {
    rotation: Rotation,
    w: usize,
    h: usize,
    px: Vec<u8>,
}

impl Default for Gray8 {
    fn default() -> Self {
        Self::new(Rotation::default())
    }
}

impl Gray8 {
    /// Nowe płótno, całe białe, o wymiarach właściwych dla podanej orientacji.
    ///
    /// Orientacja jest **przechowywana w płótnie**, a nie podawana dopiero przy
    /// pakowaniu. Inaczej dałoby się spakować pionowe płótno tak, jakby było poziome,
    /// i dostać na panelu paski — a to jest błąd, którego nic by nie złapało.
    pub fn new(rotation: Rotation) -> Self {
        let (w, h) = rotation.canvas_size();
        Self {
            rotation,
            w,
            h,
            px: vec![WHITE; w * h],
        }
    }

    /// Szerokość płótna. Zależy od orientacji — nie ma stałej `WIDTH`.
    pub fn width(&self) -> usize {
        self.w
    }

    /// Wysokość płótna.
    pub fn height(&self) -> usize {
        self.h
    }

    /// Orientacja, dla której to płótno powstało.
    pub fn rotation(&self) -> Rotation {
        self.rotation
    }

    pub fn pixels(&self) -> &[u8] {
        &self.px
    }

    pub fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.px
    }

    pub fn clear(&mut self, level: u8) {
        self.px.fill(level);
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return WHITE;
        }
        self.px[y as usize * self.w + x as usize]
    }

    #[inline]
    pub fn set(&mut self, x: i32, y: i32, level: u8) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        self.px[y as usize * self.w + x as usize] = level;
    }

    /// Nakłada `ink` z pokryciem `coverage` (0.0..=1.0). To jest jedyna ścieżka
    /// rysowania z antyaliasingiem — glify i kształty obie z niej korzystają.
    #[inline]
    pub fn blend(&mut self, x: i32, y: i32, ink: u8, coverage: f32) {
        if coverage <= 0.0 || x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let idx = y as usize * self.w + x as usize;
        let dst = self.px[idx] as f32;
        let c = if coverage > 1.0 { 1.0 } else { coverage };
        let out = dst * (1.0 - c) + ink as f32 * c;
        self.px[idx] = out.round().clamp(0.0, 255.0) as u8;
    }

    /// Wypełnia prostokąt na sztywno (bez antyaliasingu — krawędzie są całkowite).
    /// Odwraca jasność pikseli w prostokącie: czarne robią się białe i odwrotnie.
    ///
    /// Do potwierdzenia dotknięcia. Zalanie obszaru czernią jest tańsze, ale zakrywa
    /// to, w co użytkownik właśnie trafił — a chce zobaczyć, że trafił **w to**.
    /// Odwrócenie działa na dowolnej treści, więc nie wymaga, żeby układ graficzny
    /// wiedział cokolwiek o stanie „wciśnięty".
    pub fn invert_rect(&mut self, r: Rect) {
        let x0 = r.x.max(0);
        let y0 = r.y.max(0);
        let x1 = r.right().min(self.w as i32);
        let y1 = r.bottom().min(self.h as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                self.set(x, y, 0xFF - self.get(x, y));
            }
        }
    }

    pub fn fill_rect(&mut self, r: Rect, level: u8) {
        let x0 = r.x.max(0);
        let y0 = r.y.max(0);
        let x1 = r.right().min(self.w as i32);
        let y1 = r.bottom().min(self.h as i32);
        for y in y0..y1 {
            let row = y as usize * self.w;
            self.px[row + x0 as usize..row + x1 as usize].fill(level);
        }
    }

    /// Pakuje do formatu, którego oczekuje `epd_hl_get_framebuffer()`.
    ///
    /// epdiy (`src/epdiy.c`, `epd_draw_pixel`) trzyma dwa piksele w bajcie:
    /// parzysty `x` w młodszym półbajcie, nieparzysty w starszym.
    ///
    /// ```c
    /// uint8_t* buf_ptr = &framebuffer[y * epd_width() / 2 + x / 2];
    /// if (x % 2) { *buf_ptr = (*buf_ptr & 0x0F) | (color & 0xF0); }
    /// else       { *buf_ptr = (*buf_ptr & 0xF0) | (color >> 4);   }
    /// ```
    ///
    /// Przy okazji **obraca** płótno o 90° — patrz [`ROTATION`]. Panel skanuje
    /// 960×540, płótno jest 540×960, więc pakowanie i obrót to i tak jedno przejście
    /// po tych samych bajtach; robienie z tego dwóch kroków kosztowałoby drugi bufor
    /// 518 kB w PSRAM.
    ///
    /// # Panics
    /// Jeśli `out.len() != PACKED_LEN`.
    /// Pakuje do bufora epdiy **wyłącznie wskazany prostokąt panelu**.
    ///
    /// `pack4` przechodzi 259 200 bajtów wyjścia, a każdy wymaga dwóch odczytów
    /// z płótna w poprzek jego wierszy — przy obrocie o 90° to jest chodzenie po
    /// PSRAM-ie krokiem równym szerokości płótna, czyli chybianie w każdej linii
    /// pamięci podręcznej. Przy pełnym odświeżeniu (~1,5 s) to ginie. Przy
    /// częściowym, gdzie sam panel robi swoje w kilkadziesiąt milisekund, to jest
    /// **cały koszt** — i to on sprawiał, że klawiatura przyjmowała jeden znak na
    /// kilkaset milisekund.
    ///
    /// Prostokąt jest w układzie panelu i zostaje rozszerzony do pełnych bajtów:
    /// jeden bajt to dwa sąsiednie piksele, a oba i tak czytamy z płótna, więc
    /// zapisanie całego bajtu jest poprawne, nie przybliżone.
    pub fn pack4_rect(&self, out: &mut [u8], panel: Rect) {
        assert_eq!(
            out.len(),
            PACKED_LEN,
            "bufor docelowy musi mieć dokładnie {PACKED_LEN} bajtów"
        );
        let stride = PANEL_WIDTH / 2;
        let xh0 = (panel.x.max(0) as usize / 2).min(stride);
        let xh1 = ((panel.right().max(0) as usize).div_ceil(2)).min(stride);
        let y0 = panel.y.clamp(0, PANEL_HEIGHT as i32) as usize;
        let y1 = panel.bottom().clamp(0, PANEL_HEIGHT as i32) as usize;

        for py in y0..y1 {
            let dst = py * stride;
            for xh in xh0..xh1 {
                let even = self.panel_sample(xh * 2, py);
                let odd = self.panel_sample(xh * 2 + 1, py);
                out[dst + xh] = (odd & 0xF0) | (even >> 4);
            }
        }
    }

    pub fn pack4(&self, out: &mut [u8]) {
        assert_eq!(
            out.len(),
            PACKED_LEN,
            "bufor docelowy musi mieć dokładnie {PACKED_LEN} bajtów"
        );
        // Pętla idzie po wierszach PANELU, żeby zapisy do `out` były ciągłe. Odczyty
        // z płótna lecą wtedy w poprzek, co na PSRAM kosztuje — ale to i tak ginie
        // przy ~1,5 s odświeżania panelu, a licznik jest w logu `present`.
        for py in 0..PANEL_HEIGHT {
            let dst = py * (PANEL_WIDTH / 2);
            for xh in 0..PANEL_WIDTH / 2 {
                let even = self.panel_sample(xh * 2, py);
                let odd = self.panel_sample(xh * 2 + 1, py);
                out[dst + xh] = (odd & 0xF0) | (even >> 4);
            }
        }
    }

    /// Piksel płótna widziany pod współrzędnymi panelu — jedyne miejsce z macierzą
    /// obrotu.
    ///
    /// Poziomo płótno leży zgodnie ze skanem, więc odwzorowanie jest tożsamością
    /// (albo obrotem o 180°, gdy [`LANDSCAPE_FLIPPED`]). Pionowo `panel(px, py) =
    /// canvas(W-1-py, px)`.
    ///
    /// Każde z tych odwzorowań jest bijekcją zbioru pikseli panelu na zbiór pikseli
    /// płótna, więc żaden piksel panelu nie zostaje bez źródła i żaden piksel płótna
    /// nie ginie. Pilnuje tego test `obrot_jest_bijekcja`.
    #[inline]
    fn panel_sample(&self, px: usize, py: usize) -> u8 {
        let (x, y) = match self.rotation {
            Rotation::Landscape if LANDSCAPE_FLIPPED => (self.w - 1 - px, self.h - 1 - py),
            Rotation::Landscape => (px, py),
            Rotation::Portrait => (self.w - 1 - py, px),
        };
        self.px[y * self.w + x]
    }

    /// Zwraca świeżo zaalokowany spakowany framebuffer.
    pub fn to_packed(&self) -> Vec<u8> {
        let mut out = vec![0u8; PACKED_LEN];
        self.pack4(&mut out);
        out
    }

    /// Redukuje płótno do 16 poziomów, które panel realnie wyświetla.
    ///
    /// Wywoływane przez podgląd na hoście, żeby PNG nie kłamał. Na urządzeniu jest
    /// zbędne — `pack4` i tak obcina do starszego półbajtu — ale wynik jest ten sam,
    /// więc test golden porównuje to samo, co zobaczysz na szkle.
    pub fn quantize16(&mut self) {
        for p in self.px.iter_mut() {
            let hi = *p >> 4;
            *p = (hi << 4) | hi;
        }
    }

    /// Odwzorowuje płótno tak, **jak wygląda na szkle**, a nie jak wygląda w pliku.
    ///
    /// Do podglądu na hoście i do symulatora. Bez tego cała pętla „rysuj na
    /// komputerze, patrz na PNG" kłamie w jedną, konkretną stronę: PNG pokazuje
    /// równomierną skalę szesnastu szarości, a panel — skalę mocno przesuniętą ku
    /// bieli, w której górna połowa poziomów w ogóle nie istnieje. Dokładnie na tym
    /// polegli autorzy poprzedniej palety: `GRAY_50` wyglądało na zrzucie jak
    /// porządna średnia szarość, a na urządzeniu było niewidoczne.
    ///
    /// Krzywa jest **zmierzona**, nie wyliczona: pochodzi ze zdjęć karty tonów
    /// ([`crate::testcard`]) zrobionych 2026-08-20. Jest przybliżeniem — chodzi
    /// o to, żeby zrzut przestał obiecywać kontrast, którego nie ma, a nie o
    /// fotometrię. Jeśli kiedyś zmieni się waveform albo egzemplarz panelu, wgraj
    /// kartę jeszcze raz i popraw tę tablicę.
    pub fn simulate_panel(&mut self) {
        for p in self.px.iter_mut() {
            *p = PANEL_RESPONSE[(*p >> 4) as usize];
        }
    }

    /// Ściska antyaliasing do poziomów, które panel NAPRAWDĘ pokazuje.
    ///
    /// # Trzecia droga między dwoma złymi
    ///
    /// Krawędzie liter powstają z pokrycia: `Fonts::draw` blenduje glif z tłem, więc
    /// obwódka jest szara. Na tym panelu prowadziło to do wyboru między dwiema
    /// wadami, obiema zgłoszonymi ze sprzętu:
    ///
    /// * **pełne 16 poziomów** — 31% atramentu ekranu lądowało powyżej poziomu 9,
    ///   czyli w bieli. Litery wychodziły cienkie i wypłowiałe.
    /// * **dwa poziomy** ([`Gray8::quantize2`]) — cały atrament widoczny, ale
    ///   krawędzie twarde. Przy małych rozmiarach czytelność SPADA, bo znika
    ///   wygładzenie, które nadaje kształt.
    ///
    /// Obie wady biorą się z tego samego błędu: rozkładania pokrycia na skalę, której
    /// panel nie ma. Ten panel ma na atrament **pięć stopni: 0-4**. Antyaliasing
    /// ściśnięty do tych pięciu jest jednocześnie widoczny w całości i gładki.
    ///
    /// # Jak to liczy
    ///
    /// Pokrycie poniżej [`INK_CUTOFF`] to szum — idzie na biel, bo i tak nie miałoby
    /// czym się wyświetlić. Reszta rozkłada się liniowo na pięć poziomów: pełne
    /// pokrycie daje czerń, ledwie przekroczony próg daje poziom 4.
    ///
    /// # Kiedy tego NIE używać
    ///
    /// Na ekranie odświeżanym przez `MODE_DU`. DU jest dwupoziomowy z definicji, więc
    /// zetnie te pięć stopni z powrotem do dwóch — i to niespójnie z pełnym
    /// odświeżeniem, które je pokaże. Klawiatura zostaje przy [`Gray8::quantize2`]
    /// właśnie dlatego, że każde naciśnięcie klawisza idzie przez DU.
    pub fn quantize_ink(&mut self) {
        for p in self.px.iter_mut() {
            *p = ink_level(*p);
        }
    }

    /// Sprowadza płótno do DWÓCH poziomów — czerni i bieli.
    ///
    /// # Dlaczego półtony na tym urządzeniu są kłamstwem
    ///
    /// Ekran jest odświeżany na dwa sposoby i tylko jeden z nich umie szarości.
    /// `MODE_GC16` (nasze [`Refresh::Full`]) daje 16 poziomów; `MODE_DU`
    /// ([`Refresh::Fast`]) jest **dwupoziomowy** — w słowach samego epdiy „go from
    /// any color to black for white only". A DU to każde naciśnięcie klawisza, każda
    /// zmiana strony i każde wejście w szczegóły.
    ///
    /// Skutek był widoczny gołym okiem i brany za nieposprzątane artefakty:
    ///
    /// * Piksel narysowany szarością wychodzi raz szary (po GC16), raz biały albo
    ///   czarny (po DU) — **ta sama treść wygląda inaczej w zależności od tego, czym
    ///   ją ostatnio odświeżono**.
    /// * epdiy odświeża CAŁE wiersze, które się zmieniły (`dirty_lines`), na pełnej
    ///   szerokości panelu. Wiersz, który dostał impuls DU, jest czystszy od
    ///   sąsiedniego, który go nie dostał — i przy szarościach widać to jako
    ///   **poziomy pas przez cały ekran, wysoki dokładnie na tyle, ile ma tekst**.
    /// * Antyaliasing kroju i zaokrąglonych ramek to też półtony: obwódka klawisza
    ///   po DU staje się jaśniejsza od tła, które impulsu nie dostało. Stąd „ramka
    ///   bielsza niż tło".
    ///
    /// Przy dwóch poziomach żaden z tych efektów nie ma z czego powstać: biel
    /// odświeżona i biel nieodświeżona wyglądają tak samo.
    ///
    /// # Próg jest przesunięty w stronę czerni, i to celowo
    ///
    /// Domyślne 0x80 (pokrycie 50%) ścieniłoby litery, a panel elektroforetyczny
    /// i tak gubi cienkie kreski — patrz reguły typograficzne w `layout`. Przy
    /// [`BILEVEL_THRESHOLD`] czarny zostaje każdy piksel o pokryciu powyżej ~34%,
    /// więc kreska raczej tyje, niż chudnie.
    pub fn quantize2(&mut self) {
        for p in self.px.iter_mut() {
            *p = if *p < BILEVEL_THRESHOLD { BLACK } else { WHITE };
        }
    }

    /// To samo, ale tylko na wskazanym prostokącie płótna — dla odrysowań
    /// przyrostowych, które dotykają kilku prostokątów zamiast całości.
    pub fn quantize2_rect(&mut self, r: Rect) {
        let x0 = r.x.clamp(0, self.w as i32) as usize;
        let x1 = r.right().clamp(0, self.w as i32) as usize;
        let y0 = r.y.clamp(0, self.h as i32) as usize;
        let y1 = r.bottom().clamp(0, self.h as i32) as usize;
        for y in y0..y1 {
            let row = y * self.w;
            for x in x0..x1 {
                let p = &mut self.px[row + x];
                *p = if *p < BILEVEL_THRESHOLD { BLACK } else { WHITE };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paleta_miesci_sie_w_zmierzonym_zakresie() {
        // Reguła z karty tonów: atrament tylko w poziomach 0-3, wypełnienia do 8,
        // poziom 9 to zewnętrzna granica. Cokolwiek jaśniejszego jest bielą i nie
        // wolno tego użyć jako samodzielnego elementu.
        for (nazwa, v) in [("INK_DIM", INK_DIM), ("INK_FAINT", INK_FAINT)] {
            assert!(v >> 4 <= 3, "{nazwa} = {v:#04X} jest poza zakresem atramentu");
        }
        for (nazwa, v) in [("FILL_DARK", FILL_DARK), ("FILL", FILL), ("FILL_LIGHT", FILL_LIGHT)] {
            let poziom = v >> 4;
            assert!(
                (4..=8).contains(&poziom),
                "{nazwa} = {v:#04X} jest poza zakresem wypełnień"
            );
        }
        assert!(LIGHTEST_VISIBLE >> 4 <= 9, "granica widoczności przesunięta za wysoko");
    }

    #[test]
    fn krzywa_panelu_jest_monotoniczna_i_saturuje_do_bieli() {
        for i in 1..16 {
            assert!(
                PANEL_RESPONSE[i] > PANEL_RESPONSE[i - 1],
                "krzywa panelu nie rośnie na poziomie {i}"
            );
        }
        assert_eq!(PANEL_RESPONSE[15], WHITE, "poziom 15 musi być bielą");
        assert!(
            PANEL_RESPONSE[0] > BLACK,
            "poziom 0 na tym panelu NIE jest czernią — to ciemny grafit i tak ma zostać"
        );
        // Sedno pomiaru: od poziomu 10 w górę nie ma już czego odróżniać.
        assert!(
            PANEL_RESPONSE[10] > 0xE0,
            "poziom 10 miał być praktycznie bielą"
        );
    }

    const ORIENTACJE: [Rotation; 2] = [Rotation::Landscape, Rotation::Portrait];

    /// Odwrotność [`Gray8::panel_sample`]: gdzie na panelu ląduje piksel płótna.
    fn canvas_to_panel(rotation: Rotation, x: usize, y: usize) -> (usize, usize) {
        let (w, h) = rotation.canvas_size();
        match rotation {
            Rotation::Landscape if LANDSCAPE_FLIPPED => (w - 1 - x, h - 1 - y),
            Rotation::Landscape => (x, y),
            Rotation::Portrait => (y, w - 1 - x),
        }
    }

    /// Półbajt piksela panelu ze spakowanego bufora — czyli to, co zobaczy epdiy.
    fn panel_nibble(packed: &[u8], px: usize, py: usize) -> u8 {
        let b = packed[py * (PANEL_WIDTH / 2) + px / 2];
        if px % 2 == 1 {
            b >> 4
        } else {
            b & 0x0F
        }
    }

    /// Prostokąt przeliczony na panel musi pokrywać DOKŁADNIE te piksele panelu,
    /// na które trafiają piksele płótna z wnętrza prostokąta — ani jednego mniej
    /// (zostałby nieodświeżony pasek), ani jednego więcej.
    #[test]
    fn prostokat_plotna_pokrywa_sie_z_obszarem_panelu() {
        for rotation in ORIENTACJE {
            let (cw, ch) = rotation.canvas_size();
            for rect in [
                Rect::new(0, 0, 10, 20),
                Rect::new(7, 11, 63, 29),
                Rect::new(cw as i32 - 30, ch as i32 - 40, 30, 40),
                Rect::new(0, 0, cw as i32, ch as i32),
            ] {
                let panel = rotation.canvas_rect_to_panel(rect);
                assert_eq!(
                    panel.w * panel.h,
                    rect.w * rect.h,
                    "{rotation:?}: pole prostokąta ma się zgadzać"
                );
                for x in rect.x..rect.right() {
                    for y in rect.y..rect.bottom() {
                        let (px, py) = canvas_to_panel(rotation, x as usize, y as usize);
                        let (px, py) = (px as i32, py as i32);
                        assert!(
                            px >= panel.x && px < panel.right(),
                            "{rotation:?}: płótno ({x},{y}) -> panel x={px} poza {panel:?}"
                        );
                        assert!(
                            py >= panel.y && py < panel.bottom(),
                            "{rotation:?}: płótno ({x},{y}) -> panel y={py} poza {panel:?}"
                        );
                    }
                }
            }
        }
    }

    /// Pakowanie prostokąta musi dawać dokładnie te same bajty, co pakowanie całości
    /// — inaczej częściowe odświeżenie zostawiałoby szew na krawędzi obszaru.
    #[test]
    fn pakowanie_prostokata_zgadza_sie_z_caloscia() {
        for rotation in ORIENTACJE {
            let mut c = Gray8::new(rotation);
            for i in 0..c.width() as i32 {
                c.set(i, i % c.height() as i32, BLACK);
                c.set(i, (i * 7) % c.height() as i32, 0x77);
            }
            let pelne = c.to_packed();

            for rect in [
                Rect::new(0, 0, PANEL_WIDTH as i32, PANEL_HEIGHT as i32),
                Rect::new(13, 29, 71, 43),
                Rect::new(0, 0, 8, 8),
                Rect::new(PANEL_WIDTH as i32 - 9, PANEL_HEIGHT as i32 - 5, 9, 5),
            ] {
                let mut czesciowe = vec![0u8; PACKED_LEN];
                c.pack4_rect(&mut czesciowe, rect);

                let stride = PANEL_WIDTH / 2;
                let xh0 = rect.x as usize / 2;
                let xh1 = (rect.right() as usize).div_ceil(2);
                for py in rect.y as usize..rect.bottom() as usize {
                    for xh in xh0..xh1 {
                        assert_eq!(
                            czesciowe[py * stride + xh],
                            pelne[py * stride + xh],
                            "{rotation:?} {rect:?}: bajt ({xh}, {py})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn odwrocenie_prostokata_jest_swoja_odwrotnoscia() {
        let mut c = Gray8::new(Rotation::Portrait);
        c.set(10, 10, BLACK);
        c.set(11, 10, 0x40);
        let rect = Rect::new(5, 5, 20, 20);
        c.invert_rect(rect);
        assert_eq!(c.get(10, 10), 0xFF);
        assert_eq!(c.get(11, 10), 0xBF);
        assert_eq!(c.get(6, 6), 0x00, "biel ma się zrobić czernią");
        assert_eq!(c.get(100, 100), 0xFF, "poza prostokątem bez zmian");
        c.invert_rect(rect);
        assert_eq!(c.get(10, 10), BLACK);
        assert_eq!(c.get(11, 10), 0x40);
    }

    #[test]
    fn nowe_plotno_jest_biale() {
        let c = Gray8::new(Rotation::default());
        assert!(c.pixels().iter().all(|&p| p == WHITE));
    }

    #[test]
    fn plotno_ma_wymiary_swojej_orientacji() {
        let l = Gray8::new(Rotation::Landscape);
        assert_eq!((l.width(), l.height()), (960, 540));

        let p = Gray8::new(Rotation::Portrait);
        assert_eq!((p.width(), p.height()), (540, 960));

        // Liczba pikseli musi być ta sama — obrót nie skaluje.
        assert_eq!(l.pixels().len(), p.pixels().len());
        assert_eq!(l.pixels().len(), PANEL_WIDTH * PANEL_HEIGHT);
    }

    #[test]
    fn pakowanie_zgodne_z_epdiy() {
        // Parzysty `x` panelu w młodszym półbajcie, nieparzysty w starszym — tak,
        // jak robi to `epd_draw_pixel` w epdiy.
        for rotation in ORIENTACJE {
            let (w, h) = rotation.canvas_size();
            for px in [0usize, 1] {
                let (cx, cy) = (0..w)
                    .flat_map(|x| (0..h).map(move |y| (x, y)))
                    .find(|&(x, y)| canvas_to_panel(rotation, x, y) == (px, 0))
                    .expect("każdy piksel panelu ma źródło na płótnie");

                let mut c = Gray8::new(rotation);
                c.set(cx as i32, cy as i32, BLACK);
                let packed = c.to_packed();

                assert_eq!(
                    panel_nibble(&packed, px, 0),
                    0x0,
                    "{rotation:?}: piksel panelu ({px}, 0) miał być czarny"
                );
                assert_eq!(
                    panel_nibble(&packed, px ^ 1, 0),
                    0xF,
                    "{rotation:?}: sąsiad w tym samym bajcie miał zostać biały"
                );
            }
        }
    }

    #[test]
    fn obrot_odwzorowuje_narozniki() {
        for rotation in ORIENTACJE {
            let (w, h) = rotation.canvas_size();
            for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
                let (px, py) = canvas_to_panel(rotation, x, y);
                assert!(
                    px < PANEL_WIDTH && py < PANEL_HEIGHT,
                    "{rotation:?}: ({px},{py}) poza panelem"
                );

                let mut c = Gray8::new(rotation);
                c.set(x as i32, y as i32, BLACK);
                assert_eq!(
                    panel_nibble(&c.to_packed(), px, py),
                    0x0,
                    "{rotation:?}: płótno ({x},{y}) miało wylądować na panelu ({px},{py})"
                );
            }
        }
    }

    #[test]
    fn obrot_jest_bijekcja() {
        // Każdy piksel panelu ma dokładnie jedno źródło na płótnie. Gdyby odwzorowanie
        // gubiło albo dublowało piksele, na ekranie byłyby pasy — a taki błąd łatwo
        // przeoczyć, bo tekst nadal wygląda „prawie dobrze".
        for rotation in ORIENTACJE {
            let (w, h) = rotation.canvas_size();
            let mut seen = vec![false; w * h];
            let c = Gray8::new(rotation);

            for py in 0..PANEL_HEIGHT {
                for px in 0..PANEL_WIDTH {
                    let (x, y) = match rotation {
                        Rotation::Landscape if LANDSCAPE_FLIPPED => (w - 1 - px, h - 1 - py),
                        Rotation::Landscape => (px, py),
                        Rotation::Portrait => (w - 1 - py, px),
                    };
                    let idx = y * w + x;
                    assert!(!seen[idx], "{rotation:?}: piksel płótna ({x},{y}) dwa razy");
                    seen[idx] = true;
                }
            }
            assert!(
                seen.iter().all(|&v| v),
                "{rotation:?}: nie każdy piksel płótna trafił na panel"
            );
            assert_eq!(c.to_packed().len(), PACKED_LEN);
        }
    }

    #[test]
    fn spakowany_rozmiar_zgadza_sie_z_panelem() {
        assert_eq!(PACKED_LEN, 259_200);
        for rotation in ORIENTACJE {
            assert_eq!(Gray8::new(rotation).to_packed().len(), PACKED_LEN);
        }
    }

    #[test]
    fn rysowanie_poza_plotnem_nie_panikuje() {
        for rotation in ORIENTACJE {
            let mut c = Gray8::new(rotation);
            let (w, h) = (c.width() as i32, c.height() as i32);
            c.set(-5, -5, BLACK);
            c.set(9999, 9999, BLACK);
            c.blend(-1, 0, BLACK, 1.0);
            c.fill_rect(Rect::new(-100, -100, 50, 50), BLACK);
            c.fill_rect(Rect::new(w - 10, h - 10, 200, 200), BLACK);
            assert_eq!(c.get(w - 5, h - 5), BLACK);
        }
    }

    #[test]
    fn kwantyzacja_daje_16_poziomow() {
        let mut c = Gray8::new(Rotation::default());
        for (i, p) in c.pixels_mut().iter_mut().enumerate() {
            *p = (i % 256) as u8;
        }
        c.quantize16();
        let mut levels: Vec<u8> = c.pixels().to_vec();
        levels.sort_unstable();
        levels.dedup();
        assert_eq!(levels.len(), 16);
    }

    #[test]
    fn nazwy_orientacji_w_obie_strony() {
        for rotation in ORIENTACJE {
            assert_eq!(Rotation::parse(rotation.as_str()), rotation);
        }
        // Nazwy z wersji, w której obroty były dwa, ale oba pionowe.
        assert_eq!(Rotation::parse("cw"), Rotation::Portrait);
        assert_eq!(Rotation::parse("ccw"), Rotation::Portrait);
        // Śmieci nie blokują bootu.
        assert_eq!(Rotation::parse("???"), Rotation::default());
    }

    #[test]
    fn przycisk_przelacza_tam_i_z_powrotem() {
        for rotation in ORIENTACJE {
            assert_ne!(rotation.toggled(), rotation);
            assert_eq!(rotation.toggled().toggled(), rotation);
        }
    }

    /// Dotyk i pakowanie opisują to samo przyklejenie płótna do szkła. Gdyby się
    /// rozjechały, objaw wyglądałby na błąd sterownika GT911, a nie układu.
    #[test]
    fn dotyk_trafia_w_ten_sam_piksel_co_pakowanie() {
        for rotation in [Rotation::Portrait, Rotation::Landscape] {
            let mut c = Gray8::new(rotation);
            // Każdy piksel dostaje wartość zależną od swojego położenia na PŁÓTNIE.
            for y in 0..c.height() as i32 {
                for x in 0..c.width() as i32 {
                    c.set(x, y, (((x * 7 + y * 13) % 16) * 17) as u8);
                }
            }

            let packed = c.to_packed();

            for (px, py) in [
                (0, 0),
                (PANEL_WIDTH as i32 - 1, 0),
                (0, PANEL_HEIGHT as i32 - 1),
                (PANEL_WIDTH as i32 - 1, PANEL_HEIGHT as i32 - 1),
                (123, 45),
                (777, 321),
            ] {
                let (x, y) = rotation.panel_to_canvas(px, py);
                assert!(
                    x >= 0 && y >= 0 && x < c.width() as i32 && y < c.height() as i32,
                    "{rotation:?}: panel ({px},{py}) wypada poza płótno jako ({x},{y})"
                );

                // Ten sam piksel widziany przez spakowany bufor.
                let byte = packed[py as usize * (PANEL_WIDTH / 2) + (px as usize) / 2];
                let nibble = if px % 2 == 0 { byte & 0x0F } else { byte >> 4 };

                assert_eq!(
                    nibble,
                    c.get(x, y) >> 4,
                    "{rotation:?}: dotyk z panelu ({px},{py}) wskazuje inny piksel niż pakowanie"
                );
            }
        }
    }
}
