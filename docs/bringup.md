# Uruchomienie na sprzęcie

Nikt wcześniej nie sterował epdiy z Rusta. Wyszukiwanie w GitHubie po `epdiy` daje
same projekty w C i C++; po `epaper esp32 language:rust` — same panele SPI.

## Bez miernika, samym wgraniem

Większość pytań z tego dokumentu da się zamknąć mając wyłącznie płytkę, kabel
i monitor szeregowy. Firmware sam wypisuje raport przy zimnym starcie
(`firmware/src/diag.rs`), więc nie trzeba niczego interpretować z surowych liczb.

```bash
./tools/build-image.sh
python3 -m http.server -d dist 8000     # wgraj z przeglądarki
espflash monitor                        # albo „Logs & Console" na tej samej stronie
```

### Co powie pierwszy boot

```
=== raport bring-upu ===
I2C: znaleziono 6 urządzeń
  0x20  PCA9535   ekspander I/O
  0x51  PCF8563   zegar RTC
  0x55  BQ27220   licznik ogniwa
  0x5D  GT911     dotyk
  0x68  TPS65185  PMIC panelu
  0x6B  BQ25896   ładowarka
RTC: wariant Pcf8563
RTC: flaga VL czysta, czas wygląda na ciągły
PSRAM: 8388608 B — zgodnie z oczekiwaniem
boot do gotowości: 312 ms
=== koniec raportu ===
```

| Co widać | Co to znaczy |
|---|---|
| `CISZA` przy którymś adresie | ten układ nie odpowiada — albo magistrala, albo zasilanie sekcji |
| `CISZA` **przy 0x5D** | jedyny adres, przy którym cisza w raporcie nic nie znaczy: skan idzie przed sekwencją resetu GT911, a kontroler do tej chwili siedzi w resecie. Rozstrzyga dopiero linia `GT911: 911` z okna interaktywnego |
| `0x14  GT911 pod ADRESEM ZAPASOWYM` | sekwencja resetu ustawia `INT` odwrotnie; **dotyk i tak działa**, firmware sam się przełącza |
| `wybudzenie: Touch` | urządzenie obudziło się od dotknięcia ekranu, nie od BOOT-a ani timera |
| `budzenie: sam BOOT — T_INT stoi nisko` | tryb przerwania GT911 (rejestr `0x804D`) trzyma `INT` w dole w spoczynku, więc budzenie dotykiem musi zostać wyłączone — inaczej byłaby pętla wybudzeń |
| `GT911 niedostępny — dotyk nie zadziała` | kontroler nie wstał po sekwencji resetu. Pierwsze podejrzenie: `T_RST` został w zatrzasku po poprzednim śnie — patrz `power::shutdown::release_pin_holds` |
| `RTC: wariant Pcf85063` | sprzeczność ze schematu rozstrzygnięta na niekorzyść sterownika — trzeba zmienić mapę rejestrów |
| `RTC: flaga VL ustawiona` | bateryjka RTC pusta albo pierwszy start; czas przyjdzie z SNTP |
| `PSRAM: 0 B` | octal PSRAM nie wstało; epdiy alokuje przez `assert()`, więc następny krok to abort |
| `boot do gotowości > 600 ms` | pomiar 5: memtest PSRAM albo walidacja obrazu przy każdym wybudzeniu |
| `boot #2` i dalej rosnące | pomiar 4: `.rtc.data` przeżywa deep sleep, linker nie wyciął sekcji |

### Kiedy urządzenie śpi — widać to na ekranie

Przy `SLEEP_MARKER = true` (domyślnie, na czas bring-upu) urządzenie tuż przed
zaśnięciem zaczernia kwadracik 22 × 22 px w prawym dolnym rogu płótna i zostawia go
tam na cały sen — e-papier nie potrzebuje zasilania, żeby go utrzymać. Znika sam przy
pierwszym przerysowaniu po wybudzeniu.

**Czarny róg = śpi, dotyk nic nie da. Brak = czuwa.** Bez tego nie da się odróżnić
„urządzenie mnie ignoruje" od „urządzenie śpi", bo obraz na szkle jest w obu
wypadkach ten sam. Stała jest w `firmware/src/main.rs`.

### Budzenie dotykiem

`WAKE_ON_TOUCH` (też `main.rs`) decyduje, czy GT911 zostaje przy życiu na czas snu.
Przy `true` `T_RST` jest zatrzaskiwany w GÓRZE, a `T_INT` (GPIO3, pin RTC) dokłada
się do maski `ext1` obok BOOT-a — dotknięcie ekranu budzi urządzenie. **Kosztuje
prąd**: kontroler skanuje przez cały sen. Przy `false` kontroler idzie w reset,
prąd spada do zera i budzi wyłącznie BOOT.

### Osie dotyku — jedyny sposób, żeby je ustawić

**Obrót nie jest już zgadywany.** Kontroler sam mówi, w jakim układzie liczy —
firmware czyta jego rejestry rozdzielczości przy zimnym starcie i wypisuje:

```
GT911: 911
GT911: rozdzielczość 540x960 -> kontroler obrócony o 90°, przeliczam
```

`960x540` znaczy, że kontroler liczy tak jak skanuje panel i nic nie trzeba
przeliczać. `540x960` znaczy, że jest zamontowany o 90° — firmware sam wtedy
przelicza (`x = raw_y`, `y = 539 - raw_x`), dokładnie tak jak crate
`lilygo-t5s3paperpro` dla tej samej płytki. **Ta sama binarka obsługuje oba warianty.**

Zostają lustra, bo te z rozdzielczości nie wynikają: sam obrót nie mówi, którą
stroną przyklejono szkło. Rozstrzyga je dotknięcie czterech rogów.

```
dotyk: panel (12, 8) -> płótno (531, 12) -> Focus(Ssid)
dotyk: panel (12, 8) -> płótno (531, 12), brak obszaru
```

Linia `feedback dotyku: N ms` obok mówi, ile trwało częściowe odświeżenie obszaru
pod palcem. To jest ta odpowiedź, która ma przyjść, zanim zacznie się cokolwiek dziać
pod przyciskiem; jeśli zbliża się do pełnego DU (~280 ms), obszar dotykowy jest
za duży albo `epd_hl_update_area` dostaje prostokąt na całą szerokość panelu.

Interesuje pierwsza para — punkt **po przeliczeniu do układu panelu**. Lewy górny
róg fizycznego ekranu powinien dać coś bliskiego `(0, 0)`. Jeśli daje co innego,
poprawka to dwie stałe w `firmware/src/board/gt911.rs`:

| Lewy górny róg zwraca | Ustaw |
|---|---|
| ~(0, 0) | nic, jest dobrze |
| ~(0, 540) | `FLIP_Y = true` |
| ~(960, 0) | `FLIP_X = true` |
| ~(960, 540) | `FLIP_X` i `FLIP_Y` |

Liczby powyżej zakresu panelu (`x` rzędu 50 000, `y` rzędu 15 000) to **nie** kwestia
osi, tylko przesunięcia w mapie rejestrów: blok pierwszego punktu zaczyna się pod
`0x814F`, nie `0x8150`. Patrz `REG_POINT1`.

### Energia bez PPK2

Licznik kulombów BQ27220 jest na płytce i firmware go czyta przy każdym wybudzeniu:

```
pomiar energii: linia bazowa 1187 mAh
pomiar energii: 3 mAh przez 9,4 h  ->  7,7 mAh/dobę, średnio 319 µA
```

Rozdzielczość to 1 mAh, więc pojedyncze wybudzenie (0,08 mAh) jest poniżej szumu —
ale po kilku godzinach średnia przestaje kłamać i **odpowiada na pomiary 10 i 11**
z tabel niżej. Przy przekroczeniu progów firmware sam się odzywa ostrzeżeniem.
Podłączenie ładowarki unieważnia linię bazową i pomiar zaczyna się od nowa, więc
zostaw urządzenie na baterii.

**Czego to nie zastąpi:** prądu chwilowego w deep sleepie (MCU śpi, nie ma kto
czytać licznika), szczytów przy nadawaniu WiFi, różnicy z kartą SD i bez niej,
i rozbicia zużycia na składniki. To zostaje dla PPK2 albo INA228.

---

## Co jest już załatwione (2026-08-19)

Część, która była największą niewiadomą, **jest sprawdzona**:

* Toolchain instaluje się i działa: espup 0.17.1, Xtensa Rust 1.95.0.0,
  LLVM `esp-20.1.1_20250829`, target `xtensa-esp32s3-espidf`.
* `[[package.metadata.esp-idf-sys.extra_components]]` **zbiera epdiy 2.1.3**
  z rejestru Espressif i wciąga go do builda bez ręcznego CMake'a.
* W `bindings.rs` są `lilygo_board_s3`, `ED047TC1`, `epd_init_with_config`,
  `EpdI2cConfig`, `EpdInitConfig`. **Wszystkie nazwy enumów zgadzają się
  z tym, co zakładał kod** — `EpdInitOptions_EPD_LUT_1K`, `EpdDrawMode_MODE_GC16`,
  `EpdDrawMode_MODE_DU`, `EpdDrawError_EPD_DRAW_SUCCESS`. Nie było ani jednej
  poprawki w tej warstwie.
* Firmware kompiluje się do końca. Aplikacja: 2 976 032 B, czyli 71% partycji
  `ota_0` (4 MiB). Obraz dla webflashera: 3,0 MB, zweryfikowany strukturalnie
  przez `tools/check-image.sh`.

**Jedna pułapka wersji, na którą trzeba uważać.** ESP-IDF musi być `v5.5.4`.
W `v5.5.5` `sdmmc_host_t` dostało pole `unaligned_multi_block_rw_max_chunk_size`
i `esp-idf-hal` 0.46.2 — najnowsza wydana — przestaje się kompilować. Moduł `sd`
nie jest za feature'em, więc nie da się go obejść. Poprawka jest w `master`
esp-idf-hal, ale niewydana. Do tego zmiana `esp_idf_version` w `Cargo.toml`
nie unieważnia builda `esp-idf-sys` i `cargo clean -p esp-idf-sys` też nie —
trzeba usunąć `firmware/target/xtensa-esp32s3-espidf/release/build/esp-idf-sys-*`
ręcznie.

## Co zostało: zachowanie na sprzęcie

To, że się kompiluje i że symbole są na miejscu, **nie znaczy, że panel się zapali**.
Sekwencja zasilania TPS65185, timing magistrali i8080, prąd spoczynkowy —
tego nie da się sprawdzić inaczej niż na płytce.

Poniższy spike nadal ma sens, ale w węższym zakresie: nie chodzi już o to,
czy wiązania Rusta działają (działają), tylko czy **hardware odpowiada tak,
jak zakłada profil `lilygo_board_s3`**.

---

## Spike: 3,5 godziny, twarda bramka

Sens jest taki: rozdzielić „epdiy nie działa na tej płytce z tą wersją IDF" od
„moje wiązania Rusta są złe". To dwa zupełnie różne problemy i mylenie ich
kosztuje dni.

### Godzina 0:00–1:30 — czysty C

```bash
. "$HOME/esp/esp-idf/export.sh"          # checkout IDF v5.5.4 (patrz wyżej)
idf.py create-project epdspike && cd epdspike
idf.py set-target esp32s3
idf.py add-dependency "vroland/epdiy^2.1.3"
cp ../t5s3pro/firmware/sdkconfig.defaults .
idf.py build flash monitor
```

`main/main.c`, około czterdziestu linii:

1. Utwórz magistralę `i2c_master` na SDA=39, SCL=40.
2. **Wyzeruj bit 0 portu 0 ekspandera PCA9535** (`0x20`, rejestr konfiguracji `0x06`,
   rejestr wyjściowy `0x02`) — to gasi szynę LoRa/GPS.
3. `epd_init_with_config(&lilygo_board_s3, &ED047TC1, EPD_LUT_1K, &cfg)`
4. `epd_set_vcom(1600)`
5. `epd_poweron()` → `epd_fullclear()` → `epd_write_default(&FiraSans, "SPIKE", &x, &y, fb)`
   → `epd_hl_update_screen(&hl, MODE_GC16, 20)` → `epd_poweroff()`
6. `printf("psram=%u\n", esp_psram_get_size());`

**Bramka 1:** panel czyści się i pokazuje napis, `psram=8388608`, żadnego abortu,
czas GC16 zalogowany.

Jeśli to nie przejdzie — problem jest w epdiy, IDF albo sprzęcie, i **żadna ilość
pracy w Ruście nie znalazłaby go szybciej**. Masz za to gotowe zgłoszenie do
upstreamu w czystym C.

### Godzina 1:30–2:00 — to samo z Rusta

Firmware w tym repo **już to robi i już się kompiluje**, więc ta część to nie
przepisywanie, tylko `./tools/build-image.sh` i wgranie. Warstwa wiązań została
sprawdzona i nie wymagała ani jednej poprawki.

**Bramka 2:** identyczny obraz na panelu jak ze spike'u w C.

Rozjazd między bramką 1 a 2 oznaczałby różnicę w sekwencji inicjalizacji, nie
w wiązaniach — najpierw porównaj kolejność wywołań i wartość VCOM.

### Godzina 3:00–3:30 — mina energetyczna

Z builda Rusta uruchom sekwencję wyłączania i przyłóż miernik do VBAT.

**Bramka 3: poniżej 200 µA.**

Cokolwiek powyżej 1 mA to sprzężenie zwrotne przez piny panelu (epdiy #136) — i tę
informację chcesz mieć w pierwszym dniu, a nie w piątym tygodniu.

### Jeśli bramka 1 padnie

Kolejność ratunkowa:

1. Przypnij ESP-IDF niżej i powtórz. `v5.5.4` jest sprawdzone jako budowalne;
   `v5.2.3` to minimum epdiy i jednocześnie domyślna wartość `esp-idf-sys` 0.37.2.
2. Oddaj magistralę I²C epdiy — zrezygnuj z `epd_init_with_config`, zrób zapis do
   portu 0 przed `epd_init`, i pogódź się z tym, że nasze sterowniki muszą chodzić
   po magistrali epdiy.
3. Zwendoruj epdiy przez `component_dirs = ["components/epdiy"]` i łataj.

Wszystkie trzy są tanie — **pod warunkiem, że wiesz, której potrzebujesz**,
a to jest dokładnie po to, żeby zrobić spike w C.

---

## Pomiary pierwszego tygodnia

Przyrząd: **PPK2** albo INA228 z rejestratorem, szeregowo z baterią.
Nie zwykły multimetr — potrzebujesz podłogi w µA i szczytów 340 mA w jednym
przebiegu, plus całkowanie ładunku.

### Poziom 1 — zanim napiszesz kod aplikacji

| # | Pomiar | Oczekiwane | Czerwona flaga |
|---|---|---|---|
| 1 | Prąd deep sleep po pełnej sekwencji | **150–200 µA** | 400–900 µA: sekwencja niepełna. **>1 mA: sprzężenie przez piny EPD.** 20–40 mA: szyna LoRa/GPS wciąż żyje |
| 2 | To samo, z kartą SD i bez | różnica **<100 µA** | 0,5–1 mA: karta nie usypia. **Na schemacie nie ma bramkowania zasilania slotu SD** |
| 3 | `esp_psram_get_size()` | **8388608** | cokolwiek innego: zła konfiguracja octal. epdiy alokuje przez `assert()`, więc dostaniesz abort, nie błąd |
| 4 | Przeżywalność `.rtc.data` | magiczne słowo wraca | linker wyciął sekcję przez `--gc-sections` |
| 5 | Boot do gotowości | **~300 ms** | >600 ms: memtest PSRAM albo walidacja obrazu przy każdym wybudzeniu |

### Poziom 2 — energia na zdarzenie

| # | Pomiar | Oczekiwane | Czerwona flaga |
|---|---|---|---|
| 6 | Ładunek na wybudzenie z siecią | **360 mAs** (zoptymalizowane) | **>1000 mAs: łączysz się na zimno za każdym razem.** Bufor BSSID nie działa — to największa dźwignia w całym budżecie, 2,4× |
| 7 | Handshake TLS, zimny i wznowiony | ~800 ms / <300 ms | >3 s: sprawdź `CONFIG_MBEDTLS_HARDWARE_AES/SHA/MPI` |
| 8 | GC16 i DU na prawdziwym panelu | 0,5–1,0 s / 0,2–0,35 s | >2 s dla GC16: zegar magistrali albo waveform |
| 9 | Prąd szyny panelu przy odświeżaniu | ~115 mA @3,6 V | >200 mA ciągle: sprawdź VCOM |

### Poziom 3 — całka tygodniowa

| # | Pomiar | Oczekiwane | Czerwona flaga |
|---|---|---|---|
| 10 | mAh na dobę z BQ27220, logowane do NVS | **7 ± 3 mAh** przy odświeżaniu co 30 min | **>25 mAh w pierwszej dobie: coś w sekwencji nie ląduje.** Nie czekaj tygodnia — to widać po 24 h |
| 11 | Stan naładowania po 7 dniach | **>90%** | <70%: jesteś 2,5× nad budżetem |
| 12 | Powody resetów, pierścień w NVS | **zero nieoczekiwanych** | `Brownout`: nakładanie radia na panel. `TaskWatchdog`: zablokowane wypchnięcie klatki |
| 13 | Sekundy z włączonym radiem na dobę | **<150 s** | >600 s: pętla ponowień. To kosztuje więcej niż cała reszta razem |

---

## Zanim podłączysz płytkę

* **Sprawdź, którą wersję masz.** „T5 E-Paper S3 Pro" i „Pro Lite" mają ten sam
  schemat (Lite nie ma LoRa i GPS) — oba działają. Ale starszy **T5 4.7 V2.3**
  to inna płytka: inny pinout, bez TPS65185 i bez ekspandera. Ten firmware jej
  nie obsłuży.
* **Odczytaj oznaczenie na kostce RTC.** Źródła producenta same sobie przeczą:
  README mówi PCF85063, schemat i strona produktowa mówią PCF8563. Mapy rejestrów
  nie są zgodne. Sterownik w tym repo implementuje PCF8563 i ma funkcję
  `probe_variant()`, która to sprawdza przy pierwszym boocie — zobacz log.
* **Nie używaj `power::shutdown()` z timerem.** Odcięcie ścieżki baterii przez
  BQ25896 nie ma wybudzenia czasowego. Wyjście tylko przyciskiem PMIC albo USB.
  Shutdown bez drogi powrotnej to cegła.
