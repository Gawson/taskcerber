# t5s3pro — kalendarz na e-papierze

Firmware w Ruście dla **LilyGo T5 E-Paper S3 Pro**: pobiera kalendarz Google przez
HTTPS, rysuje go na panelu 960×540 w 16 odcieniach szarości i śpi. Wgrywa się
z przeglądarki.

```
┌─────────────────────────────────────────────────────────────────┐
│  dashboard/   render 960×540 Gray4 + model interakcji dotykowej  │  ← bez ESP-IDF
│  icalfeed/    strumieniowy parser iCal + rozwijanie RRULE        │  ← bez ESP-IDF
│  devlogic/    polityka zasilania, decyzja OTA, maskowanie adresów│  ← bez ESP-IDF
├─────────────────────────────────────────────────────────────────┤
│  preview/     zrzuty PNG na hoście          simulator/  okno,    │
│                                                mysz jako dotyk   │
├─────────────────────────────────────────────────────────────────┤
│  firmware/    ESP-IDF: WiFi, mbedTLS, epdiy, zasilanie, NVS      │  ← xtensa
└─────────────────────────────────────────────────────────────────┘
```

Podział jest celowy: **cała trudna logika żyje poza firmware'em i jest testowana
na hoście**. Układ graficzny, parsowanie kalendarza, strefy czasowe, reguły
powtarzania, paginacja i obsługa dotyku mają testy, które chodzą w sekundę na
zwykłym `cargo test` — zamiast być debugowane przez wgrywanie i patrzenie na ścianę.

---

## Szybki start bez płytki

Wszystko poniżej działa na zwykłym stabilnym Ruście, bez toolchainu Xtensa.

```bash
cargo test                              # 153 testy: render, iCal, dotyk, klawiatura, polityka, OTA
cargo run -p preview -- all             # zrzuty PNG do out/ (pionowo)
cargo run -p preview -- all landscape   # to samo poziomo, pliki z sufiksem
cargo run -p simulator                  # interaktywne okno, mysz jako dotyk
cargo run -p simulator -- --landscape   # ten sam kod w orientacji poziomej
cargo run -p simulator -- --ics "<twój prywatny adres iCal>"
```

Symulator renderuje **dokładnie ten sam kod**, który idzie na panel: ta sama
kwantyzacja do 16 poziomów, te same obszary dotykowe, odwzorowane czasy odświeżania
(GC16 ~1,2 s, DU ~0,28 s) i symulacja duchów po serii szybkich odświeżeń.

```
spacja  pełne odświeżenie      B  poziom baterii      S  zrzut PNG
← →     zmiana strony          N  stan sieci          1-4 scenariusze
Esc     powrót ze szczegółów   G  duchy wł/wył        K  konfiguracja
R       pobierz ponownie                              Q  wyjście
```

`K` otwiera **ekran konfiguracji z klawiaturą dotykową** — ten sam, który zobaczysz
na szkle. Myszą stuka się w klawisze dokładnie tak, jak palcem na panelu: to te
same regiony z `hit::Screen`, których użyje firmware po odczycie z GT911.

Podanie `--ics` z prawdziwym adresem to test integracji end-to-end: pobranie,
parsowanie, rozwinięcie reguł powtarzania i render — na komputerze, w sekundę.
Adres można też podać zmienną `T5_ICS_URL`, żeby nie lądował w historii powłoki.

---

## Firmware

### Wymagania

```bash
cargo install espup --locked
espup install --std --targets esp32s3 --export-file "$HOME/export-esp.sh"
. "$HOME/export-esp.sh"                 # w każdej powłoce
cargo install ldproxy --locked
cargo install espflash --version 4.5.0 --locked
```

Zweryfikowane wersje: espup 0.17.1, Xtensa Rust **1.95.0.0**, LLVM `esp-20.1.1_20250829`.
Pierwszy build ściąga ESP-IDF i narzędzia (~4,5 GB w `~/.espressif`); kolejne są przyrostowe.

> **ESP-IDF jest przypięte do `v5.5.4` i nie wolno tego podnieść bez sprawdzenia.**
> W `v5.5.5` struktura `sdmmc_host_t` dostała nowe pole, przez co `esp-idf-hal` 0.46.2
> (najnowsza wydana) nie kompiluje się wcale. Poprawka jest w `master`, ale niewydana.
>
> Zmiana `esp_idf_version` w `Cargo.toml` **nie unieważnia** builda `esp-idf-sys` —
> ani `cargo clean -p esp-idf-sys` nie pomaga. Trzeba usunąć katalog ręcznie:
> ```bash
> rm -rf firmware/target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-*
> ```

### Build i obraz dla webflashera

```bash
./tools/build-image.sh                  # firmware + sklejony obraz w dist/
python3 -m http.server -d dist 8000     # http://localhost:8000
```

### Wgrywanie

Otwórz **`http://localhost:8000`** w Chrome, Edge albo Firefoksie 151+ i kliknij
**Zainstaluj firmware**. Płytka ma natywne USB (`303A:1001`), więc sterowniki nie
są potrzebne.

Nie otwieraj `dist/index.html` z dysku. Po `file://` nie ma ani Web Serial
(to nie jest bezpieczny kontekst), ani ładowania modułów ES — strona wygląda,
jakby się wczytała, i nic nie robi.

Jeśli port się nie pojawia: przytrzymaj **BOOT**, kliknij **RST**, puść **RST**,
puść **BOOT**.

### Wgrywanie z innego komputera

```bash
./tools/serve-flasher.sh                # HTTPS na wszystkich adresach tej maszyny
```

Skrypt wypisze adresy — `https://192.168.1.152:8443/` i podobne — oraz odcisk
SHA-256 certyfikatu. Wpisanie adresu bez `https://` trafia na port 8080 i zostaje
przekierowane, więc nie trzeba pamiętać o schemacie.

**Dlaczego HTTPS w sieci domowej.** Web Serial jest dostępny wyłącznie
w bezpiecznym kontekście. `localhost` jest bezpieczny z definicji,
`http://192.168.x.x` nie jest — przeglądarka po prostu nie pokaże listy portów
i nie powie dlaczego. Bezpieczny kontekst zależy od schematu URL-a, nie od tego,
czy certyfikat jest zaufany, więc certyfikat podpisany przez samego siebie
załatwia sprawę: Chrome pokaże ostrzeżenie, po jego przyjęciu
(*Zaawansowane → Przejdź do…*) pasek adresu mówi „Niezabezpieczona", ale
`isSecureContext` jest prawdziwe i `navigator.serial` działa.

Klucz prywatny ląduje w `.flasher-tls/`, nie w `dist/` — `dist/` jedzie na
GitHub Pages jako artefakt CI. Certyfikat jest odtwarzany, gdy zmienią się adresy
maszyny albo zbliża się koniec ważności; poza tym wyjątek raz przyjęty
w przeglądarce zostaje.

Jeśli ostrzeżenie przeszkadza, są dwie drogi bez niego:

1. **Tunel SSH.** Przeglądarka widzi `localhost`, więc certyfikat w ogóle nie
   wchodzi w grę. Tutaj `python3 -m http.server -d dist 8000`, a na drugim
   komputerze:

   ```bash
   ssh -N -L 8000:127.0.0.1:8000 gawson@192.168.1.152   # potem http://localhost:8000
   ```

2. **Flaga w Chrome.** `chrome://flags/#unsafely-treat-insecure-origin-as-secure`
   na drugim komputerze, wpisany adres `http://192.168.1.152:8000`, restart
   przeglądarki. Wtedy wystarczy zwykłe `python3 -m http.server`.

### Konfiguracja urządzenia

Świeżo wgrane urządzenie nie ma ani danych WiFi, ani adresu kalendarza i pokazuje
ekran konfiguracji. Wszystko wpisuje się **dotykiem, na panelu** — nie ma konsoli
szeregowej, serwera HTTP ani aplikacji towarzyszącej.

Wejścia na ekran konfiguracji są dwa: plakietka „skonfiguruj urządzenie" w nagłówku
(na nieskonfigurowanym urządzeniu odświeżenie i tak nie ma czego pobrać) oraz numer
wersji w lewym dolnym rogu — dyskretny, ale z obszarem dotykowym wysokim na 44 px.

Sześć pól: `sieć`, `hasło`, `iCal`, `iCal 2`, `strefa`, `OTA`. Wymagane są dwa —
nazwa sieci i adres kalendarza; są oznaczone kropką na zakładce, a nad klawiaturą
widać, czego jeszcze brakuje. Pole wartości pokazuje **końcówkę** wpisywanego
tekstu, bo przy 120-znakowym adresie iCal to początek jest nieciekawy.

```bash
cargo run -p simulator      # K otwiera ten ekran, mysz działa jak palec
cargo run -p preview -- setup   # zrzuty PNG obu wariantów
```

**Ekran konfiguracji jest wyraźnie wygodniejszy w poziomie.** Klawisz ma wtedy
87 px zamiast 48 — przy 234 DPI panelu to różnica między 9,8 a 5,2 mm. Orientację
przestawia przycisk custom (`S3`); w pionie da się pisać, ale adres iCal warto
wstukać poziomo.

### Aktualizacja przez sieć (OTA)

`./tools/build-image.sh` publikuje obok sklejki dla webflashera dwa dodatkowe pliki:

| Plik | Co to |
|---|---|
| `dist/firmware.bin` | sklejka `[bootloader][tablica][aplikacja]` pod offset 0 — **dla webflashera** |
| `dist/firmware-ota.bin` | sama aplikacja — **dla OTA** |
| `dist/ota.json` | wersja, względny adres obrazu, SHA-256, rozmiar |

**Wersja bierze się z gita, nie z `Cargo.toml`.** `tools/version.sh` wypisuje
`0.1.0+g1a2b3c4` (albo `0.1.0+g1a2b3c4.d5f9a2` przy brudnym drzewie) i jest jedynym
miejscem, w którym się ją liczy: `build-image.sh` eksportuje wynik jako `T5_VERSION`,
`build.rs` wkompilowuje go w obraz, a ten sam łańcuch ląduje w `ota.json`. Bez tego
OTA nie ma jak wystartować — semver z `Cargo.toml` zmienia się raz na wydanie, więc
urządzenie zawsze widziałoby w manifeście własną wersję i nie aktualizowało się
nigdy. Skrót brudnego drzewa jest tam dlatego, że bring-up robi się
na niescommitowanych zmianach, a dwa buildy o tej samej wersji wyglądają dokładnie
jak zepsute OTA.

`check-image.sh` sprawdza, że wersja z `ota.json` **faktycznie jest w obrazie** —
rozjazd tych dwóch to urządzenie pobierające 3 MB w kółko.

Pomylenie tych dwóch obrazów to urządzenie, które nie wstaje: `esp_ota_write` pisze
do slotu aplikacji, więc sklejka wylądowałaby tam razem z bootloaderem i tablicą
partycji. `tools/check-image.sh` sprawdza to jawnie — obraz OTA nie może mieć tablicy
partycji na 0x8000.

Włączenie na urządzeniu to wpisanie adresu manifestu w polu `OTA` na ekranie
konfiguracji (klucz NVS `ota_url`). Bez niego OTA jest wyłączone i to jest
**domyślne**: urządzenie na baterii nie powinno samo sięgać po nowy firmware,
dopóki ktoś świadomie nie wskaże, skąd.

Adres obrazu w manifeście jest **względny**, rozwiązywany względem adresu manifestu,
więc ten sam artefakt działa z GitHub Pages i z serwera w LAN-ie bez przebudowy.

> **Do testów po LAN-ie użyj zwykłego HTTP**, nie `serve-flasher.sh`. Urządzenie
> weryfikuje certyfikaty bundle'em CA z ESP-IDF i certyfikat podpisany przez samego
> siebie odrzuci — to nie przeglądarka, nie ma tam przycisku „przejdź dalej".
> `python3 -m http.server -d dist 8000` i `ota_url = http://192.168.1.152:8000/ota.json`.
> Po HTTP nie ma uwierzytelnienia źródła, a SHA-256 przyjeżdża tym samym kanałem co
> obraz, więc to jest konfiguracja **testowa**. Do prawdziwego użycia — HTTPS.

**Co stoi między tym a cegłą** (szczegóły w `firmware/src/net/ota.rs`):

* **Rollback bootloadera** — `CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y`. Świeżo wgrany
  obraz startuje w `PENDING_VERIFY` i musi zawołać
  `esp_ota_mark_app_valid_cancel_rollback()` przed kolejnym resetem. Wołamy to
  **na końcu udanego cyklu**, nie na starcie — obraz ma najpierw udowodnić, że
  potrafi narysować ekran i zasnąć.
* **SHA-256 z manifestu** liczone w locie i sprawdzane **przed** przestawieniem slotu.
* **Kasujemy tyle slotu, ile trzeba.** Z rozmiarem w manifeście `esp_ota_begin`
  kasuje zaokrąglone w górę 3 MB zamiast całych 4 MiB. Cena: rozmiar musi być
  prawdziwy, więc pobranie ma twardy limit i porównanie sumy bajtów na końcu.
* **Licznik prób w NVS** — po trzech nieudanych podejściach do tej samej wersji
  odpuszczamy, zamiast pobierać 3 MB co cykl aż do rozładowania ogniwa.

  Był najpierw w pamięci RTC i **nie działał**. Bootloader przeładowuje segmenty RTC
  z obrazu przy każdym resecie, który nie jest wybudzeniem z deep sleepu
  (`should_load()` w `esp_image_format.c`), a po wgraniu obrazu wołamy `esp_restart()`.
  Licznik zerował się więc dokładnie w tym jednym scenariuszu, przed którym miał
  chronić. NVS przeżywa i reset, i przeflashowanie z przeglądarki.
* **Próg zasilania** — OTA tylko na USB albo powyżej 50% naładowania.
* **Kabel jako wyjście awaryjne** — `otadata` leży w luce, którą webflasher zapisuje,
  więc przeflashowanie z przeglądarki zawsze wraca do `ota_0`.

Aktualizacja idzie **po** pobraniu kalendarza i **przed** wyłączeniem radia. Restart
następuje przy zgaszonym radiu i nietkniętym panelu — reset przy podniesionych szynach
TPS65185 potrafi uszkodzić panel.

#### Jak to sprawdzić po LAN-ie

```bash
./tools/build-image.sh                       # wersja A — wgraj ją z przeglądarki
python3 -m http.server -d dist 8000          # ten sam katalog serwuje ota.json

# na urządzeniu: dotknij wersji w stopce -> zakładka OTA ->
#   http://192.168.1.152:8000/ota.json -> zapisz

git commit -am "cokolwiek" && ./tools/build-image.sh   # wersja B, nowa wersja z gita
```

Urządzenie musi być na USB albo powyżej 50% naładowania (`Policy::should_update`),
bo inaczej świadomie pominie aktualizację. W logu szukaj kolejno:
`OTA: <A> -> <B>, pobieram`, `OTA: wgrane N B`, `restart do nowego obrazu`,
a po restarcie `OTA: bieżący slot potwierdzony jako sprawny` — to ostatnie pojawia
się dopiero na końcu **udanego** cyklu i jest dowodem, że rollback został odwołany.

Warto zrobić też przebieg negatywny: podmień jeden znak w `sha256` w `ota.json`
i sprawdź, czy w logu jest `SHA-256 się nie zgadza` i czy urządzenie **nie**
przestawiło slotu.

### esp-web-tools jest w repozytorium

`web/vendor/esp-web-tools/` to rozpakowane `npm pack esp-web-tools@10.4.0`.
Strona nie odwołuje się do żadnego CDN-u — to strona, która wgrywa firmware do
urządzenia, więc kod flashera nie powinien przyjeżdżać z cudzego hosta w momencie
kliknięcia. Efekt uboczny: działa bez internetu.

Podbicie wersji to podmiana zawartości `package/dist/web/`. Bundle jest
code-splitowany, więc trzeba przenieść **cały** katalog — sam `install-button.js`
dociąga `esp32s3-*.js` i `stub_flasher_32s3-*.js` po ścieżkach względnych.

---

## Decyzje, które warto znać

**ESP-IDF, nie `no_std`.** Nie z sympatii, tylko dlatego, że `embedded-tls` ma
`MAX_SAN_DNS_NAMES = 3`, a certyfikat `*.googleapis.com` wymienia tę nazwę jako
szóstą z siedemnastu. Weryfikacja nazwy hosta nie ma prawa przejść. mbedTLS z ESP-IDF
niesie pełny bundle CA i po prostu działa. To był argument rozstrzygający.

**Rozmiar płótna nie jest stałą kompilacji.** Urządzenie ma dwie orientacje — pionową
(540×960) i poziomą (960×540) — i to ten sam kod układu rysuje obie. `layout.rs` nie
zna stałych `WIDTH`/`HEIGHT`; wymiary i wszystko, co z nich wynika, bierze ze struktury
`Geom` policzonej z płótna. Warunkowe są tylko te wielkości, które naprawdę się różnią
(wysokość stopki), bo wysokość wiersza agendy nie ma powodu zależeć od tego, jak
urządzenie stoi.

**Obrót robi `Gray8::pack4`, nie epdiy.** Panel ED047TC1 skanuje 960×540 i `epd_width()`
zwraca 960 niezależnie od ustawień. `epd_set_rotation()` **tego nie zmienia** — nagłówek
epdiy mówi wprost, że przestawia wyłącznie własne prymitywy rysujące i fonty, a my
piszemy prosto do bufora z `epd_hl_get_framebuffer()`. Obrót siedzi więc w pakowaniu
do 4 bpp: jedno przejście zamiast dwóch i bez drugiego bufora 518 kB. Orientacja jest
przechowywana **w płótnie**, nie podawana przy pakowaniu — inaczej dałoby się spakować
pionowe płótno jak poziome i dostać paski.

**Orientację przestawia custom button, BOOT tylko budzi.** Custom siedzi na ekspanderze
PCA9535 (`IO1_2`), a jego INT idzie na GPIO38 — poza domeną RTC, więc nie może wybudzić
z deep sleepu. BOOT to jedyny przycisk na pinie MCU zdolnym do RTC. Pełna mapa
przycisków: [`docs/hardware.md`](docs/hardware.md#5b-przyciski--gdzie-który-ląduje).

**epdiy kradnie bit przycisku i trzeba go odzyskać.** `epd_board_init` woła
`pca9555_set_config(pca, CFG_PIN_PWRGOOD | CFG_PIN_INT, 1)`, czyli zapisuje **cały**
rejestr konfiguracji portu 1 i zostawia jako wejścia tylko bity 6 i 7. Bit 2 — przycisk
— staje się wyjściem i dostaje zero, więc odczyt zwraca „wciśnięty" bez końca.
Odzyskanie go jest bezpieczne, bo epdiy z tego bitu nie korzysta: jego stałe `CFG_PIN_*`
to bity 0, 1, 3, 4, 5, 6 i 7.

**epdiy jako komponent, nie własny sterownik.** `epdiy` 2.1.3 zawiera profil
`lilygo_board_s3`, którego pinout zgadza się ze schematem co do nogi
(`SDA=39, SCL=40, D0..D7 = 5,6,7,15,16,17,18,8, CKV=48, STH=41, LEH=42, STV=45,
CKH=4, vcom=1600`), oraz panel `ED047TC1` z własnym waveformem. Napisanie tego od
zera to 9700 linii C plus ręcznie pisany asembler Xtensa. Wciąga się jednym wpisem
w `Cargo.toml`.

**Panel czyści się przy każdym wybudzeniu, nie tylko przy pierwszym.**
`epd_hl_update_screen` nie rysuje klatki — rysuje różnicę względem `back_fb`, i pomija
całe niezmienione linie i kolumny. `back_fb` powstaje w `epd_hl_init` wyzerowany do
bieli i leży w PSRAM, którą deep sleep gasi. Każde wybudzenie to więc świeże
założenie „panel jest biały", podczas gdy panel fizycznie trzyma poprzednią klatkę —
e-papier nie potrzebuje zasilania, żeby ją utrzymać. Bez `epd_fullclear` stary tusz
zostaje wszędzie tam, gdzie nowa klatka jest biała, i na ekranie widać **sumę** obu.
Kosztuje to jeden przebieg czyszczący (3 cykle × 22 pchnięcia pełnym ekranem) na
odświeżenie. `Refresh::Fast` zostaje w kodzie, ale jest poprawne wyłącznie dla
drugiego i kolejnego rysowania w obrębie tego samego wybudzenia.

**Deep sleep, nie light sleep.** Light sleep kupuje 13,5 mAs na wybudzenie
i kosztuje 0,4–2,0 mA stałej podłogi. Opłaca się dopiero przy odświeżaniu
częstszym niż co ~11 s. Szczegóły i cała arytmetyka: [`docs/power.md`](docs/power.md).

**Partycja NVS leży za aplikacją** (0x810000). `espflash --merge` zapisuje luki
wypełnione `0xFF`, więc wszystko przed końcem aplikacji ginie przy każdym
przeflashowaniu z przeglądarki — a wszystko za nią przeżywa. Dzięki temu
aktualizacja firmware'u nie kasuje danych WiFi ani adresu kalendarza.

**OTA było przewidziane w tablicy partycji, zanim powstało.** Dwa sloty po 4 MiB
i `otadata` w luce, którą webflasher kasuje — dzięki temu wejście OTA nie wymagało
przepartycjonowania, a każde przeflashowanie z przeglądarki jest resetem do
`ota_0`. Aplikacja zajmuje 71% slotu, więc zapas jest.

**Prywatny adres iCal zamiast OAuth.** Zero ekranu zgody, zero pułapki
z siedmiodniowym wygasaniem tokenu odświeżającego w aplikacjach o statusie
„Testing". Cena: to stały bearer do całego kalendarza, bez zakresu i bez terminu
ważności. Adres siedzi w NVS, nie w binarce, a CI sprawdza, czy nie wyciekł do
opublikowanego obrazu. Przejście na OAuth to dodanie drugiej implementacji traitu
`EventSource` — reszta się nie zmienia.

---

## Stan projektu

| Element | Stan |
|---|---|
| Render, typografia, paginacja, dotyk | **zweryfikowane** — 43 testy, zrzuty w `out/` |
| Parser iCal, RRULE, strefy czasowe | **zweryfikowane** — 40 testów, w tym `RECURRENCE-ID`, `EXDATE`, zmiana czasu |
| Symulator | **zweryfikowany** — 8 testów logiki; okno wymaga sesji graficznej |
| Firmware — kompilacja | **zweryfikowana** — buduje się na Xtensa Rust 1.95.0.0 / ESP-IDF v5.5.4 |
| Wiązania do epdiy | **zweryfikowane** — `lilygo_board_s3`, `ED047TC1` i wszystkie enumy są w `bindings.rs` |
| Obraz dla webflashera | **zweryfikowany** — `tools/check-image.sh` sprawdza offsety, tryb flasha i bootloader |
| Boot i render na panelu | **uruchomione** — płytka wstaje, rysuje ekran konfiguracji |
| Czyszczenie panelu, zatrzaski magistrali | **poprawione i sprawdzone na szkle** — obraz czysty po wybudzeniu |
| Dwie orientacje | **napisane**, pion sprawdzony na szkle, poziom nie |
| Ekran konfiguracji i klawiatura | **zweryfikowane na hoście** — 77 testów, mysz w symulatorze; na szkle nie |
| Sterownik dotyku GT911 | **nie istnieje** — bez niego ekranu konfiguracji nie da się obsłużyć na płytce |
| Decyzja OTA i polityka zasilania | **zweryfikowane** — 27 testów w `devlogic`, chodzą na hoście |
| I²C, zasilanie, sieć, dotyk | **nieuruchomione** |
| OTA — transport | **napisany, nieuruchomiony** — włącza się konsolą, wersje z gita |

Największa niewiadoma projektu — czy `extra_components` w ogóle zbierze epdiy i czy
nazwy z bindgena się zgodzą — **jest już rozwiązana**, a płytka wstaje i rysuje.

Zostało zachowanie reszty peryferiów. Zanim pójdziesz dalej, przeczytaj
[`docs/bringup.md`](docs/bringup.md) — jest tam lista pomiarów z oczekiwanymi
wartościami i opis czterech błędów, z których każdy kosztuje 10–200× budżetu energii.

## Dokumentacja

* [`docs/hardware.md`](docs/hardware.md) — pełny pinout, mapa I²C, rejestry
  PCA9535 i TPS65185, sprzeczności między źródłami producenta
* [`docs/power.md`](docs/power.md) — budżet energetyczny, arytmetyka, cztery błędy,
  które kosztują 10–200× budżet
* [`docs/bringup.md`](docs/bringup.md) — spike, bramki, pomiary

## Licencje

Kod tego repozytorium: MIT lub Apache-2.0.

**epdiy jest na LGPL-3.0-or-later.** Statyczne linkowanie do firmware'u uruchamia
obowiązki z §4 licencji. Dla projektu osobistego to bez znaczenia; jeśli będziesz
publikować obraz, trzymaj epdiy jako niemodyfikowany komponent z rejestru
(tak jest teraz) i licz się z obowiązkiem udostępnienia źródeł.

Krój: Noto Sans (SIL Open Font License 1.1), przycięty do Latin + Latin Extended-A.
