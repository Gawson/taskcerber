# LilyGo T5 E‑Paper S3 Pro — hardware reference

Zebrane ze schematu i repo producenta, zweryfikowane 2026‑08‑18. Źródła na końcu.
Ten dokument jest niezależny od wybranej architektury firmware'u — to po prostu prawda o płytce.

---

## 1. Warianty płytki — przeczytaj zanim cokolwiek wgrasz

| Nazwa handlowa | TPS65185 | GPS | LoRa | Gałąź firmware vendora |
|---|:--:|:--:|:--:|---|
| **T5 E‑Paper S3 Pro** | ✅ | ✅ | ✅ | `H752-01` |
| **T5 E‑Paper S3 Pro Lite** | ✅ | ❌ | ❌ | `H752-01` |
| H752 (wycofany) | ❌ | ❌ | ✅ | `H752` |

Vendor: *„The `Lite` version and the `Pro` version share the same schematic diagram,
but the `Lite` version does not have LoRa and GPS."* → jeden firmware obsłuży oba,
ale LoRa/GPS muszą być wykrywane runtime'owo (probe‑and‑degrade).

> ⚠️ **Pułapka nazewnicza.** Samo „T5 S3" zwykle oznacza **starszy T5 4.7" V2.3 / LilyGo‑EPD47**
> — też ESP32‑S3, też ED047TC1, ale **inny pinout**, bez PCA9535 i bez TPS65185 w tym układzie.
> Inne crate'y Rust (`lilygo-epd47` vs `lilygo-t5s3paperpro`), **niekompatybilne pinowo**.
> Sprawdź nadruk na płytce.

---

## 2. Rdzeń

| Element | Wartość |
|---|---|
| Moduł | **ESP32‑S3‑WROOM‑1** (Xtensa LX7 dual‑core, 240 MHz) |
| Flash | **16 MB**, QIO @ 80 MHz |
| PSRAM | **8 MB, Octal (OPI)** — `memory_type: qio_opi` |
| SRAM wewn. | ~327 680 B użytkowych |
| USB | **natywne USB‑Serial‑JTAG / CDC**, VID:PID `303A:1001`, D− = GPIO19, D+ = GPIO20 |
| Bateria | Li‑Po 3.7 V 1500 mAh |
| Reset | pin `EN` (nie GPIO); BOOT = GPIO0 |

**Wejście w tryb download.** Vendor podaje procedurę „przytrzymaj BOOT → kliknij RST
z tyłu → puść RST → puść BOOT" i przepisuje ją z README płytek z zewnętrznym mostkiem
USB-UART. **Na tej płytce nie jest potrzebna** i zostało to sprawdzone na sztuce:
ESP32-S3 ma natywne USB-Serial-JTAG, więc host sam wymusza reset w tryb download —
wystarczy wpiąć kabel i wgrywać. Procedura ręczna zostaje jako ratunek, gdyby
firmware zawiesił kontroler USB.

**Ważne dla builda:** PSRAM jest **octal**, a domyślna konfiguracja esp‑hal to `quad`.
Bez `ESP_HAL_CONFIG_PSRAM_MODE = "octal"` inicjalizacja PSRAM zwróci zły rozmiar albo padnie.

---

## 3. Panel e‑papieru

| Element | Wartość |
|---|---|
| Panel / TCON | **ED047TC1** |
| Przekątna | 4.7" |
| Rozdzielczość | **960 × 540** |
| Odcienie | **16 (4 bpp, `Gray4`)** |
| Interfejs | **8‑bitowa magistrala równoległa** — na ESP32‑S3 przez **LCD_CAM w trybie i8080 + GDMA**, plus 4 osobne linie timingu |
| Pakowanie na magistrali | **4 piksele/bajt (2 bity/piksel)** → 240 bajtów na linię |
| Framebuffer po stronie hosta | 4 bpp → 960/2 × 540 = **259 200 B** — **musi być w PSRAM** |
| Waveform LUT | ~325–425 KB — też PSRAM/flash |
| Zegar magistrali | vendor: 26.6 MHz (`PLL160M`); crate Rust: 20 MHz |
| VCOM | −1600 mV |
| EPD PMIC | **TPS65185 / TPS651851RSLR**, I²C `0x68` |
| Sterowanie gate/OE | **nie na pinach MCU** — przez ekspander **PCA9535** |

Konsekwencja: to **nie jest** wyświetlacz SPI. `epd-waveshare`, `ssd1677`, `uc8179`
i cała reszta driverów SPI e‑ink są tutaj bezużyteczne.

---

## 4. Mapa GPIO (piny bezpośrednio na MCU)

Zgodne w trzech niezależnych źródłach: `docs/pin_define.md`, `docs/pinmap.md`
(wyprowadzone ze schematu `T5 E-paper S3 Pro V1.0 24-12-24.pdf`) oraz makro `pin_config!`
w crate `lilygo-t5s3paperpro`.

| GPIO | Net | Funkcja | Podsystem | Rola i80 | Uwagi |
|---:|---|---|---|---|---|
| 0 | `IO0` | przycisk BOOT | system | vendor: dummy DC | Low przy resecie ⇒ tryb download |
| 1 | `LORA_RST` | reset LoRa | SX1262 | — | |
| 2 | `RTC_INT` | przerwanie RTC | RTC | — | |
| 3 | `T_INT` | przerwanie dotyku | GT911 | — | |
| 4 | `EP_CKH` | zegar źródłowy EPD | EPD | **WR / PCLK** | |
| 5 | `EP_D0` | dane EPD 0 | EPD | linia danych | |
| 6 | `EP_D1` | dane EPD 1 | EPD | linia danych | |
| 7 | `EP_D2` | dane EPD 2 | EPD | linia danych | |
| 8 | `EP_D7` | dane EPD **7** | EPD | linia danych | ⚠️ poza kolejnością numeryczną |
| 9 | `T_RST` | reset dotyku | GT911 | — | |
| 10 | `LORA_IRQ` | DIO1 / IRQ | SX1262 | — | |
| 11 | `BL_EN` | podświetlenie (PWM) | frontlight | — | steruje `PT4103B23F EN` |
| 12 | `SD_CS` | chip select microSD | SD | — | |
| 13 | `MOSI` | SPI MOSI | SPI współdz. | — | LoRa + SD |
| 14 | `SCK` | SPI SCLK | SPI współdz. | — | LoRa + SD |
| 15 | `EP_D3` | dane EPD 3 | EPD | linia danych | |
| 16 | `EP_D4` | dane EPD 4 | EPD | linia danych | |
| 17 | `EP_D5` | dane EPD 5 | EPD | linia danych | |
| 18 | `EP_D6` | dane EPD 6 | EPD | linia danych | |
| 19 | `DM` | USB D− | USB | — | natywne USB |
| 20 | `DP` | USB D+ | USB | — | natywne USB |
| 21 | `MISO` | SPI MISO | SPI współdz. | — | LoRa + SD |
| 35–37 | — | brak netu na schemacie | — | — | **zajęte przez octal PSRAM — nie używać** |
| 38 | `PCA_INT` | przerwanie ekspandera | PCA9535 | — | active‑low, open‑drain |
| 39 | `II2C_SDA` | I²C SDA | **wspólna magistrala I²C** | — | wszystkie układy I²C |
| 40 | `II2C_SCL` | I²C SCL | **wspólna magistrala I²C** | — | wszystkie układy I²C |
| 41 | `EP_STH` | impuls startu źródła | EPD | vendor: CS / crate: **DC** | patrz §7 |
| 42 | `EP_LE` / `EP_LEH` | latch enable | EPD | zwykłe GPIO, impuls | |
| 43 | `U0TXD` → `GPS_RX` | UART do GPS | GNSS | — | przez `R11`; to też klasyczny pad UART0 TX |
| 44 | `U0RXD` ← `GPS_TX` | UART z GPS | GNSS | — | przez `R12` |
| 45 | `EP_STV` | impuls startu bramki | EPD | zwykłe GPIO | **pin strapujący (VDD_SPI)** |
| 46 | `LORA_CS` | chip select LoRa | SX1262 | — | **pin strapujący** |
| 47 | `LORA_BUSY` | busy LoRa | SX1262 | — | |
| 48 | `EP_CKV` | zegar bramki | EPD | GPIO **lub wyjście RMT** | crate Rust używa RMT dla precyzji impulsów |
| — | `EN` | reset | system | — | nie jest GPIO |
| — | `VRTC` | bateria podtrzymująca RTC | RTC | — | koszyk `J12` |

**Kolejność linii danych na magistrali i80** (`kDataGpios[8]`, indeks = numer linii i80):
`D0=GPIO5, D1=GPIO6, D2=GPIO7, D3=GPIO15, D4=GPIO16, D5=GPIO17, D6=GPIO18, D7=GPIO8`

---

## 5. Ekspander PCA9535PW (I²C `0x20`, INT → GPIO38)

Rejestry standardowe PCA95xx: `0x00/0x01` input, `0x02/0x03` output,
`0x04/0x05` polarity inversion, `0x06/0x07` configuration.

| Bit | Nazwa | Kier. | Przeznaczenie |
|---|---|---|---|
| IO0_0 | `LORA_EN` | wy | **Zasilanie wspólnej szyny 3.3 V LoRa + GPS.** Domyślnie **wyłączone po boocie** |
| IO0_1..7 | — | — | niepodłączone |
| IO1_0 | `EPD_OE` | wy | output enable sterownika źródłowego EPD |
| IO1_1 | `EPD_MODE` | wy | wybór trybu sterownika bramek |
| IO1_2 | `BUTTON` | **we** | przycisk funkcyjny `S3` na płytce (active low) |
| IO1_3 | `TPS_PWRUP` | wy | wyzwolenie sekwencji power‑up TPS65185 |
| IO1_4 | `VCOM_CTRL` | wy | włączenie VCOM |
| IO1_5 | `TPS_WAKEUP` | wy | wybudzenie TPS65185 |
| IO1_6 | `TPS_PWR_GOOD` | **we** | status power‑good |
| IO1_7 | `TPS_INT` | **we** | przerwanie TPS65185 |

Maska bezpiecznego wyłączenia (vendor): wyzeruj bity 0, 1, 3, 4, 5 portu 1.

> 🔌 **Pułapka:** `LORA_EN` jest **wyłączone po starcie**. Jeśli spróbujesz sondować
> SX1262 albo GPS przed jego podniesieniem, uznasz że radio jest martwe.

---

## 5b. Przyciski — gdzie który ląduje

Płytka ma pięć przycisków, ale tylko **jeden** trafia na pin MCU. To rozstrzyga,
który może budzić z deep sleepu, a nie żadna preferencja projektowa.

| Przycisk | Ląduje na | Firmware może zauważyć? |
|---|---|---|
| **BOOT** | **GPIO0** — pin MCU, w domenie RTC | **tak** — `ext1` budzi, `gpio_get_level` czyta na jawie |
| **RESET** | pin `EN` (CHIP_PU), nie GPIO | **nie** — chip startuje od zera, patrz niżej |
| **PWR** | `QON` w BQ25896 | **nie** — to cykl ścieżki zasilania, nie wybudzenie |
| **custom / `S3`** | PCA9535 `IO1_2` (I²C) | tylko na jawie |
| **HOME** | klawisz kontrolera GT911 (I²C) | dopiero ze sterownikiem dotyku |

Wybudzenie z deep sleepu przez `ext0`/`ext1` wymaga pinu **zdolnego do RTC**, czyli
na ESP32‑S3 GPIO0–21. Przycisk custom wisi na ekspanderze I²C, którego MCU nie ma jak
odpytywać we śnie, a `PCA_INT` ekspandera idzie na **GPIO38** — poza domeną RTC.
Custom buttonem nie da się obudzić urządzenia bez przelutowania.

**RESET jest nierozróżnialny od podłączenia zasilania.** W mapowaniu ESP‑IDF dla
ESP32‑S3 (`components/esp_system/port/soc/esp32s3/reset_reason.c`) **nie ma w ogóle
przypadku `ESP_RST_EXT`** — reset przez `EN` wchodzi jako `RESET_REASON_CHIP_POWER_ON`
→ `ESP_RST_POWERON`. Firmware nie odróżni naciśnięcia RST od wpięcia kabla.

Kosztuje przy tym całą pamięć RTC: `EN` w dół resetuje domenę RTC, więc `RtcState`
startuje od zera. Ginie zbuforowany BSSID (następna asocjacja to pełny skan, ~300 mAs
— największa pojedyncza dźwignia w budżecie energii), CRC ostatniej treści (wymuszone
przerysowanie) i licznik prób OTA.

> ⚠️ **Pułapka:** po wybudzeniu przez `ext1` pad GPIO0 zostaje pod kontrolą **RTC_IO**,
> a cyfrowa ścieżka wejściowa jest odcięta. `gpio_get_level(0)` zwraca wtedy zero
> niezależnie od tego, czy ktoś trzyma przycisk — samo `gpio_set_direction` +
> `gpio_set_pull_mode` tego nie odkręca. Potrzebne jest `rtc_gpio_deinit(0)` przed
> konfiguracją pinu jako wejścia. Kosztowało to jedną rundę na sprzęcie: licznik
> przytrzymania BOOT dobijał do progu przy każdym wybudzeniu i cofał zmianę
> orientacji dwie sekundy po tym, jak użytkownik ją zrobił.

> 💡 **Do wykorzystania później:** `T_INT` (GT911) siedzi na **GPIO3**, a `RTC_INT`
> (PCF8563) na **GPIO2** — oba w domenie RTC. Czyli **dotyk może budzić urządzenie**;
> `power::shutdown::prepare_for_deep_sleep` ma już na to parametr `keep_touch_alive`.
> Sterownik GT911 (`board/gt911.rs`) już jest, ale `T_INT` wykorzystuje wyłącznie
> jako pin sekwencji resetu — budzenie dotykiem to wciąż rzecz do zrobienia i to ona
> zdejmie z użytkownika konieczność naciskania BOOT, żeby cokolwiek dotknąć.
> Alarm z PCF8563 może z kolei zastąpić budzenie timerem ESP.

---

## 6. Mapa adresów I²C (jedna magistrala, GPIO39/40)

| Adres | Układ | Rola |
|---|---|---|
| `0x20` | PCA9535PW | ekspander I/O (sterowanie EPD + zasilanie LoRa/GPS) |
| `0x51` | PCF8563TS **lub** PCF85063 | RTC — **konflikt źródeł, patrz §7** |
| `0x55` | BQ27220YZFR | fuel gauge |
| `0x5D` | GT911 | dotyk pojemnościowy (2 punkty) |
| `0x68` | TPS651851RSLR | PMIC e‑papieru |
| `0x6B` | BQ25896 | ładowarka / PMIC baterii (robi też pełny shutdown) |

Rejestry TPS65185 używane przez vendora i przez crate Rust:
`0x01` = ENABLE (`0x3F` = wszystkie szyny), `0x03`/`0x04` = VCOM low/high,
`0x0F` = power‑good (maska `0xFA`).

---

## 7. Sprzeczności w źródłach — nie rozstrzygnięte, do weryfikacji na sztuce

1. **Model RTC.** README vendora mówi `PCF85063 (0x51)`; `docs/pinmap.md` vendora mówi
   wprost, że schemat strona 3 / U3 pokazuje `PCF8563TS`, i zaleca ufać schematowi.
   Strona produktowa lilygo.cc też mówi PCF8563. Mapy rejestrów tych układów **nie są
   identyczne**. → **Odczytaj oznaczenie na kostce.** Crate'y Rust istnieją dla obu
   (`pcf8563` 0.2.1, `pcf85063a` 0.1.1).
2. **„Driver IC".** Strona produktowa podaje `ED047TC1`, wiki podaje `TPS65185`.
   To dwie różne rzeczy: ED047TC1 to panel/TCON, TPS65185 to zasilacz EPD. Wpis na wiki
   jest źle opisany.
3. **Rozdzielczość** nie jest podana na stronie produktowej; 960×540 potwierdza README
   vendora i `t5s3_epd_pins.h` (`kPanelWidth = 960; kPanelHeight = 540`).
4. **Moduł GPS.** README wymienia `MIA-M10Q / L76K` — to dwa różne układy (u‑blox vs
   Quectel), a katalog `hardware/` zawiera datasheety obu. Zwykłe NMEA zadziała z każdym,
   ale binarna konfiguracja UBX tylko z u‑bloxem. Który jest wlutowany — **niezweryfikowane**.
5. **GPIO35/36/37** opisane jako „nieużywane w tej rewizji" — to wniosek z braku nazwy netu,
   nie pozytywne stwierdzenie. Na WROOM‑1 z **octal** PSRAM i tak są zajęte wewnętrznie.
   **Nie używać.**
6. **GPIO41 (`EP_STH`)**: firmware vendora używa go jako **CS** magistrali i80, a crate Rust
   jako **DC**. Oba działają, ale inaczej. Vendor dodatkowo używa **GPIO0 (BOOT!) jako dummy DC**
   — crate Rust tego nie robi i to jest bezpieczniejsze.
7. **Swizzle linii danych.** Crate Rust celowo przestawia linie:
   `data0←D6, data1←D7, data2←D4, data3←D5, data4←D2, data5←D3, data6←D0, data7←D1`.
   To **nie jest bug** — kompensuje kolejność bajtów/bitów kontrolera i80.
   **Nie „naprawiać".** Oznacza to też, że przeniesienie stałych czasowych z kodu C vendora
   wprost do crate'a Rust da śmieci na ekranie.

---

## 8. Rzeczy do zmierzenia na prawdziwym urządzeniu

- Czy `ESP_HAL_CONFIG_PSRAM_MODE = "octal"` faktycznie wykrywa 8 MB — zaloguj rozmiar przy boocie.
- Oznaczenie RTC (PCF8563 vs PCF85063).
- Czy po tym, jak firmware podrive'uje GPIO45/46 (piny strapujące), płytka nadal
  niezawodnie wchodzi w tryb download.
- Prąd deep sleep. `power::shutdown()` w crate Rust odcina ścieżkę baterii przez BQ25896
  i **działa tylko na zasilaniu bateryjnym**; powrót wyłącznie przez przycisk PMIC/QON lub USB.
  Zweryfikuj, że zawsze da się wrócić — shutdown bez drogi powrotnej to cegła.
- Jakość szarości: crate ma w TODO *„Implement fuller waveform / LUT support"*, ciemne
  poziomy są słabo rozróżnialne. Jeśli projekt zależy od 16 realnych odcieni — sprawdź to najpierw.

---

## Źródła (pobrane 2026‑08‑18)

- [Xinyuan-LilyGO/T5S3-4.7-e-paper-PRO](https://github.com/Xinyuan-LilyGO/T5S3-4.7-e-paper-PRO) — gałąź `H752-01`;
  `docs/pin_define.md`, `docs/pinmap.md`, `boards/T5-ePaper-S3.json`,
  `examples/epd_60fps_probe/main/{t5s3_epd_pins.h,epd_video.cpp,pca9535_min.h}`,
  `hardware/{ED047TC1.pdf,tps65185.pdf,pca9535.pdf}`
- [LilyGo wiki — T5 E-Paper S3 Pro](https://wiki.lilygo.cc/products/t5-series/t5-e-paper-s3-pro/)
- [Strona produktowa](https://lilygo.cc/en-us/products/t5-e-paper-s3-pro)
- [azw413/lilygo-t5s3paperpro-rs](https://github.com/azw413/lilygo-t5s3paperpro-rs) — `src/lib.rs`, `src/display.rs`, `src/ed047tc1.rs`
