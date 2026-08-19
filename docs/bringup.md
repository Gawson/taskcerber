# Uruchomienie na sprzęcie

Nikt wcześniej nie sterował epdiy z Rusta. Wyszukiwanie w GitHubie po `epdiy` daje
same projekty w C i C++; po `epaper esp32 language:rust` — same panele SPI.

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
