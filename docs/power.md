# Budżet energetyczny

Analiza wykonana 2026-08-18 na podstawie not katalogowych, zgłoszeń społeczności
i schematu płytki. **Żadna z tych liczb nie została zmierzona na tym egzemplarzu** —
pomiary do wykonania są wypisane na końcu.

> Wniosek w jednym zdaniu: tydzień pracy to nie jest trudny cel na tej płytce
> (nawet nieustrojony deep sleep daje ~32 dni przy odświeżaniu co 15 minut).
> Trudne jest nie zepsuć go jednym z czterech błędów, z których każdy kosztuje
> 10–200× cały budżet.

---

# VERDICT: light sleep + one week on a 1500 mAh T5 S3 Pro

## 0. Two premise corrections before the arithmetic

**(a) "Light sleep so a clock can tick" is a non-sequitur.** Timekeeping does not need light sleep. This board has a PCF8563TS + MS412FE backup cell that keeps time at ~0.3 µA whether the ESP32 is running, light-sleeping, deep-sleeping, or dead. The ESP32-S3 also keeps RTC_SLOW powered in deep sleep. **You never lose the time.** What you lose in deep sleep is the *PSRAM framebuffer* — and the corpus already solves that: keep ~300 bytes of semantic state (`last_time_str`, `du_refreshes_since_gc16`, cached BSSID, TLS ticket) in `.rtc.data` and re-render the clock rectangle from it. Cost: 38 mAs/tick versus 24.5 mAs/tick in light sleep. That 13.5 mAs boot delta is the *entire* thing light sleep buys you.

**(b) The corpus's own headline — "one week is achievable but NOT with light sleep" — is overstated, and I'm not going to repeat it as written.** Run the numbers and light sleep clears one week at any network cadence of 5 min or slower, even at pessimistic floors. The correct criticism of light sleep on this board is not "it can't reach a week." It's "it costs you 0.41–2.0 mA of pure floor to save 0.22 mA of boot energy, and it puts you one board-level defect away from a 5-hour battery." That's a different — and more useful — argument.

---

## 1. Is "light sleep + one week" possible? The arithmetic.

### The budget

| Basis | Capacity | Budget for 168 h |
|---|---|---|
| Nameplate 1500 mAh, 100% usable | 1500 mAh | 8.93 mA |
| **Realistic: 80% usable (ageing + LDO dropout cutoff)** | **1200 mAh** | **7.14 mA** |

The 80% derate is not conservatism theatre. The 3.3 V rail comes from an **ME6217C33 LDO** (`U12`), dropout 100 mV typ / **180 mV max @ 300 mA**. Under a 300 mA EPD refresh pulse the rail collapses at **VBAT ≈ 3.48 V**, i.e. 8–12% SoC still nominally in the cell. Because it's an LDO, `I_battery ≈ I_rail` to within ~1%, so every mA below is directly comparable — but the wasted watt-hours are real and show up as the derate.

**All figures below use 1200 mAh / 7.14 mA.**

### Light-sleep floor, itemised

| Contributor | µA | Source |
|---|---:|---|
| ESP32-S3 light-sleep base | 240 | DS v2.2 Table 5-10 |
| + 8 MB **octal** PSRAM @3.3 V (N16R8 ⇒ ESP32-S3R8, 3.3 V part) | 140 | DS Table 5-10 fn.1 |
| + SPI flash standby | 10–30 | ESP-IDF |
| + flash CS pull-up (`ESP_SLEEP_FLASH_LEAKAGE_WORKAROUND`) | 10 | Kconfig help |
| + PSRAM CS pull-up (`ESP_SLEEP_PSRAM_LEAKAGE_WORKAROUND`) | 10 | Kconfig help |
| **Chip subtotal, datasheet-ideal** | **410–430** | |
| ME6217C33 quiescent (always on, EN tied to VSYS) | 100 | datasheet, 100 typ / 130 max |
| BQ25896 (BATFET closed, battery monitor off) | 32 | datasheet |
| BQ27220 in SLEEP | 9 | datasheet (50 µA in NORMAL) |
| TPS65185 in SLEEP | ~5 | datasheet 3.5–10 µA (130 µA in standby) |
| PCA9535/XL9555 standby | ~1 | datasheet 0.9 typ |
| PCF8563TS + backup | 0.3 | |
| GT911 held in RST | ~0 | (70–120 µA if merely "sleep") |
| LoRa/GPS LDO gated off | 0.1 | |
| **Board subtotal, fully tuned** | **~147** | |
| **LIGHT-SLEEP FLOOR (ideal)** | **~567 µA = 0.567 mA** | |

**You cannot remove the 140 µA PSRAM term.** `CONFIG_ESP_SLEEP_POWER_DOWN_FLASH` is `depends on !SPIRAM || ESP_LDO_RESERVE_PSRAM || SOC_PSRAM_HAS_DEDICATED_LDO`; `SOC_PSRAM_HAS_DEDICATED_LDO` is absent from `esp32s3/soc_caps.h` and `ESP_LDO_RESERVE_PSRAM` is ESP32-P4. On S3 with `CONFIG_SPIRAM=y` the option is **not selectable at all**. VDD_SPI stays up. That is a hard Kconfig dependency, not a tuning knob.

**Real-world S3 light sleep runs 2–5× the datasheet.** Two independent reports: DroneBot (custom S3 PCB) ~**2 mA**; esp32.com #41048 ~**1.90 mA** against a claimed 240 µA. `CONFIG_PM_SLP_DISABLE_GPIO` help text claims it recovers "about 200~300 uA". So three scenarios:

- **L-ideal = 0.567 mA** (everything perfect)
- **L-real = 1.35 mA** (corpus's real-world model)
- **L-pessimistic = 2.15 mA** — *my labelled engineering estimate*: DroneBot's 2 mA chip-side + 147 µA board.

**Nobody — including LilyGo — has ever published a light-sleep measurement for this board.** The vendor's own `image-2.png` shows a Victor 8246A reading `00.8728` mA = **873 µA**, sleep mode unspecified, peripheral state unspecified. That's the only vendor number in existence and it doesn't even say which mode. Treat every light-sleep number here as unmeasured on this hardware.

### Per-event charge (mAs at the battery; 1 mAh = 3600 mAs)

| Event | mAs | Note |
|---|---:|---|
| Clock tick, light sleep (re-render rect + DU, no radio, **no boot**) | **24.5** | *derived*: 38 − 13.5 boot |
| Clock tick, deep sleep (boot + re-render + DU) | 38 | corpus composite |
| Deep-sleep boot to app-ready | 13.5 | 300 ms × 45 mA, ESTIMATE |
| Network wake, **optimized** (cached BSSID + static IP + TLS ticket + 9.6 KB gzip + GC16) | 360 | |
| Network wake, **measured-class** | 670 | TRMNL's instrumented 0.67 C capture — a real device, not arithmetic |
| Network wake, **cold** (full scan + full handshake) | 850 | |
| Fetch, body hash unchanged, **no repaint** | 210 | |

### The sums

Light sleep, 1-min clock ticks (9.80 mAh/day = 0.408 mA) + network at cadence:

| Floor | net 1 min | net 5 min | net 15 min | net 1 h |
|---|---:|---:|---:|---:|
| **L-ideal 0.567 mA** | 6.98 mA → **7.2 d** | 2.18 → 23.0 d | 1.38 → 36.4 d | 1.08 → 46.5 d |
| **L-real 1.35 mA** | 7.76 mA → **6.4 d** ✗ | 2.96 → 16.9 d | 2.16 → 23.2 d | 1.86 → 26.9 d |
| **L-pessim 2.15 mA** | 8.56 mA → **5.8 d** ✗ | 3.76 → 13.3 d | 2.96 → 16.9 d | 2.66 → 18.8 d |
| L-real, *measured-class* wake | 12.93 → **3.9 d** ✗ | 3.99 → 12.5 d | 2.50 → 20.0 d | 1.94 → 25.7 d |

Worked example, the recommended-ish case (L-real, 1-min clock, 15-min network, optimized wake):

```
  1.350 mA   light-sleep floor (chip 1.20 + board 0.147)
+ 0.408 mA   clock ticks   (1440/day × 24.5 mAs = 35,280 mAs = 9.80 mAh/day)
+ 0.400 mA   network       (   96/day × 360  mAs = 34,560 mAs =  9.60 mAh/day)
= 2.158 mA   →  1200 mAh / 2.158 mA = 556 h = 23.2 days
```

### Answer to Q1

**Yes, arithmetically — at 5-minute network cadence or slower, light sleep clears one week with 1.8–3.3× margin even at pessimistic floors. It fails one week only at 1-minute network cadence.**

But that is the wrong question to have asked, because:

**Light sleep is strictly dominated by deep sleep at every tick interval you would plausibly use.** Break-even, computed directly:

| Light-sleep floor | Floor penalty vs deep-tuned (155 µA) | Boot energy saved per tick | Break-even tick interval |
|---|---:|---:|---:|
| ideal 567 µA | 412 µA = 0.412 mAs/s | 13.5 mAs | **32.8 s** |
| real 1.35 mA | 1195 µA = 1.195 mAs/s | 13.5 mAs | **11.3 s** |
| pessimistic 2.15 mA | 1995 µA = 1.995 mAs/s | 13.5 mAs | **6.8 s** |

You want to tick once a minute. **Light sleep does not pay for itself unless you tick faster than every ~33 s in the best case, ~11 s realistically.** Same functionality, same clock, same everything — deep sleep is 42.1 days vs 23.2 days at the identical cadence. You would be spending 45% of your battery life on a sleep mode that gives you nothing you can see.

---

## 2. What IS achievable with light sleep — the honest numbers

Read the table above; the short version:

- **1-min clock + 1-min network fetch: 3.9–7.2 days.** This is the configuration that actually fails. At L-real with measured-class wakes it's **93 hours**.
- **1-min clock + 5-min network: 10.4–23.0 days.**
- **1-min clock + 15-min network: 15.1–36.4 days.**
- **1-min clock, no network at all (offline clock):** L-real 1.758 mA → **28.4 days**.
- **Light sleep with WiFi *associated*** (auto-light-sleep, so you could receive a push): DTIM3 = 1.62 mA + board + clock + 15-min net ≈ 2.43 mA → **20.6 days**; DTIM10 ≈ 18–19 days. Technically fine, but nothing pushes to this device, so you're paying for a mailbox nobody writes to.
- **A seconds-resolution clock: 80 hours = 3.3 days**, in *any* sleep mode. 1 Hz × ~15 mAs per small-rect DU = 15 mA average. And it's worse than a power problem: 86,400 panel refreshes/day against a ~10⁶-cycle e-paper life is **11.6 days until the glass is worn out**. If "so a clock can tick" secretly means seconds, the answer is no, permanently, and the battery is not the binding constraint.

For comparison, deep sleep at the same cadences (1-min clock tick + network):

| Floor | 1 min | 5 min | 15 min |
|---|---:|---:|---:|
| tuned 155 µA | 7.4 d | 25.1 d | **42.1 d** |
| untuned 415 µA | 7.1 d | 22.2 d | 34.5 d |
| vendor out-of-box 873 µA | 6.7 d | 18.5 d | 26.2 d |

Note the bottom-right cell: **even the completely untuned, out-of-the-box 873 µA board clears one week by 3.7× at 15-min network.** One week is not a hard target on this hardware. Which brings us to the actual point:

> **One week is not lost to the budget. It is lost to bugs.** Four ways to miss it, all of which I've quantified:
>
> | Failure | Result |
> |---|---|
> | EPD GPIO back-feed through the panel driver's clamping diodes after `epd_poweroff` (epdiy #136, measured **235 mA** on T5-4.7 — same mechanism, same driver, different board) | **5.1 hours** |
> | LoRa/GPS LDO left enabled (its `EN` has a **10 k pull-up to VSYS via R21**, so it comes up ON at cold boot and stays on until firmware clears XL9555 PORT0 bit 0) | **40 hours** |
> | WiFi retry storm: 3 retries × 10 s @ 110 mA on a 1-min cadence | **1320 mAh/day — dead in under a day** |
> | Seconds-resolution clock | 3.3 days, panel dead in 11.6 days |
>
> Every one of these is 10–200× your entire power budget. The difference between light sleep and deep sleep (0.4–2.0 mA) is noise next to any of them. **Spend your engineering time on the shutdown sequence and the WiFi failure path, not on the sleep mode.**

---

## 3. The exact configuration that gives one week — and what you lose

### Configuration

**Deep sleep only. No light sleep anywhere in the battery path.**

```
Sleep mode        : deep sleep, RTC_SLOW retained (.rtc.data), RTC_PERIPH powered down
Clock tick        : every 60 s, 07:00–23:00, radio OFF, MODE_DU on the clock rect only
Network fetch     : every 15 min, 07:00–23:00, batched (all sources in one association)
De-ghost          : full MODE_GC16 every ~20 DU ticks or on content change
Night             : 23:00–07:00, ONE deep sleep to 07:00. No ticks. No network.
                    GC16 repaint at 07:00.
Waveform LUT      : EPD_LUT_1K (not 64K) — frees 64 KB DRAM for mbedTLS
Bus clock         : 80 MHz octal PSRAM (NOT the vendor's 120 MHz)
```

Mandatory shutdown sequence before **every** `esp_deep_sleep_start()`, in this order — this is where 873 µA → 155 µA lives:

1. **Isolate the EPD bus first.** D0–D7 (GPIO 5,6,7,15,16,17,18,8), CKH=4, STH=41, LEH=42, STV=45, CKV=48 → input/no-pull, then `rtc_gpio_isolate()` where available. *This is the 235 mA path. Do this before anything else.*
2. XL9555/PCA9535 **PORT1** bits 0,1,3,4,5 → 0 (EPD_OE, EPD_MODE, TPS_PWRUP, VCOM_CTRL, TPS_WAKEUP). TPS65185 130 µA → 3.5–10 µA.
3. XL9555 **PORT0 bit 0** (`LORA_EN`) → 0. LoRa/GPS LDO → 0.1 µA. (epdiy never touches PORT0 and `lilygo_board_s3` sets `.gpio_write = NULL`, so you drive this yourself over the shared bus.)
4. `BL_EN` (GPIO11) → 0.
5. GT911: drive `RST` (GPIO9) low + `gpio_hold_en()`. **Not** the I²C sleep command — that's 70–120 µA.
6. BQ27220 → SLEEP (50 → 9 µA).
7. BQ25896 `CONV_RATE = 0` (REG02[6]). Continuous ADC keeps REGN alive.
8. `gpio_deep_sleep_hold_en()`, then `esp_sleep_pd_config(ESP_PD_DOMAIN_RTC_PERIPH, OFF)`.
9. Never `esp_sleep_pd_config(ESP_PD_DOMAIN_VDDSDIO, OFF)` — destroys PSRAM (and is a no-op benefit in deep sleep anyway).

### Result

```
floor  0.155 mA × 24 h                        =  3.72 mAh/day
ticks  960/day × 38 mAs                       = 10.13 mAh/day
net     64/day × 360 mAs                      =  6.40 mAh/day
                                                --------------
                                                 20.25 mAh/day
                                                 = 0.844 mA average
                                                 = 59.2 days on 1200 mAh
```

**7-day draw: 142 mAh = 11.8% of usable capacity, 9.5% of nameplate.** That is an **8.5× margin** on the one-week requirement, which is the right size of margin for a device whose largest single term (per-wake charge) has a 2.4× spread between the optimistic and cold cases.

Even if you botch the entire shutdown sequence and sit at the vendor's 873 µA: 37.5 mAh/day → **32 days**. Still 4.6×.

### What you actually lose versus "light sleep, everything on"

| Lost | Quantified cost of keeping it |
|---|---|
| **Sub-minute clock resolution.** Minute granularity only. | Seconds would be 3.3 days + panel death in 11.6 days. Non-negotiable. |
| **~0.5–0.7 s wake-to-visible latency** (boot ~300 ms → 200 ms with `CONFIG_BOOTLOADER_SKIP_VALIDATE_IN_DEEP_SLEEP`, + render + DU 200–350 ms) vs ~250 ms in light sleep. | This is the only genuine UX loss, and it's ~400 ms on a device that repaints in 0.2–1.5 s anyway. |
| **PSRAM contents each wake** (~1.01 MB of epdiy framebuffers). You re-render from `.rtc.data` semantics instead of retaining pixels. | Already counted in the 38 mAs tick. |
| **Live TCP/TLS session across sleep.** | Google's ICS endpoint sends `cache-control: no-cache, no-store` and closes; there's nothing to keep. Zero real loss. |
| **Clock frozen 23:00–07:00** (night window). | Re-enabling night ticks costs +8.4 mAh/day → 59.2 d drops to ~40 d. Affordable if the user hates it. |
| **Touch-to-wake while asleep** (GT911 held in reset). | Leaving GT911 in its own sleep mode costs 70–120 µA = **+2.9 mAh/day** → 59.2 d → 51.7 d. **This is affordable — buy it if touch matters.** What you must *not* do is leave it in Green (3.3 mA) or Doze (0.78 mA) mode. |
| Nothing else. | |

---

## 4. Hybrid power policy

Source of truth for mode selection, read once per wake over I²C before anything else:
- **USB present**: BQ25896 `REG0B[7:5] VBUS_STAT` / `PG_STAT` (0x6B)
- **SoC / voltage / temperature**: BQ27220 (0x55) — also use its temperature for `epd_hl_update_screen()`, because `epd_board_ambient_temperature()` on `lilygo_board_s3` returns a **hard-coded 20 °C** (`lilygo_board_s3.c:249`) and e-paper waveforms are strongly temperature-dependent.

| Mode | Entry condition | Clock tick | Network | Sleep | Extras | Draw |
|---|---|---|---|---|---|---|
| **U — USB / charging** | `PG_STAT=1` | 1 min | **1–2 min** | none (or light sleep between events) | touch ON, frontlight available, GT911 powered, full GC16 freely | Irrelevant — mains-powered. Budget ~45–70 mA active, ~1.5 mA idle. Do **not** let the charger current fight the load. |
| **A — Active** | battery, 07:00–23:00, SoC > 40% | 1 min | 15 min | deep | GC16 every 20 DU, hash-skip repaint | **20.25 mAh/d · 0.844 mA · 59 days** |
| **N — Night** | battery, 23:00–07:00, SoC > 20% | **none** | **none** | one deep sleep to 07:00 | GC16 repaint on wake | **0.155 mA for 8 h = 1.24 mAh** |
| **B — Frugal** | battery, SoC 40–20% | 5 min | 60 min | deep | DU only; GC16 once/day | **7.35 mAh/d · 0.306 mA · 163 days** |
| **C — Survival** | battery, SoC 20–10% | 15 min | 6 h | deep | no GC16; render "stale since HH:MM" | **4.70 mAh/d · 0.196 mA · 256 days** |
| **D — Hold** | battery, SoC < 10% | none | none | deep, button-wake only | one final GC16 "LOW BATTERY — CHARGE ME", then freeze | **3.72 mAh/d · 0.155 mA · 323 days** |
| **S — Ship** | user long-press "off" | — | — | BQ25896 BATFET off | wake only via QON button or USB | **12–23 µA. Never enter this on a timer — there is no timer wake out of it. `shutdown` without a return path is a brick.** |

### Adaptive rules layered on top (each with its number)

**R1 — WiFi failure backoff. Mandatory, not optional.**
A failed association costs 200–480 mAs (2–4 s at 100–120 mA), i.e. *more* than a successful optimized wake. At 15-min cadence, a permanently-dead AP burns **12.8 mAh/day** — 63% of your entire active-mode budget, for zero information. Worse, a naive 3×10 s retry loop on a 1-min cadence is **1320 mAh/day: the battery is gone in under 24 hours.**
Policy: 1 attempt per wake, hard 8 s timeout, no in-wake retries. After 3 consecutive failures → 1 h cadence. After 6 → 6 h cadence. Reset on first success. Render a "no network since HH:MM" badge so the failure is visible.

**R2 — Body-hash repaint skip.**
Google's ICS sends **no ETag, no Last-Modified, no Content-Length**, and `cache-control: no-cache, no-store, must-revalidate`. There is no conditional GET; you always pay the fetch. But you can hash the decompressed 92,775 B body and skip the panel refresh: 360 → **210 mAs**. If 80% of fetches are no-change (realistic for a calendar at 15-min cadence), active-mode network drops **9.60 → 6.40 mAh/day**, and total 20.25 → **17.05 mAh/day = 70 days**.

**R3 — Never overlap radio and panel.**
`esp_wifi_stop()` + `esp_wifi_deinit()` **before** `epd_poweron()`. Panel rail is ~115 mA @3.6 V sustained; WiFi TX peaks at 283–340 mA. Overlapping them through an LDO at VBAT 3.5 V is a brownout generator, and a reset mid-refresh with the TPS65185 rails up is the one genuinely damaging failure on this board. Keep `CONFIG_ESP_BROWNOUT_DET_LVL_SEL_7` (2.44 V, the **lowest** setting) so normal transients don't cause a reset loop, and do the real cutoff in software from the BQ27220.

**R4 — Batch all sources into one association.**
Association is 55 mAs (cached) and is paid once; only the ~80 mAs TLS handshakes multiply. Three sources ≈ 360 + 2×(80+40) = **600 mAs/wake**; at 15-min active-hours cadence that's 10.7 mAh/day → total 24.5 mAh/day → **49 days**. Still fine. But don't spread them across separate wakes.

**R5 — Charge-integral guard.**
Log BQ27220 accumulated charge per day to NVS. If any 24 h window exceeds **35 mAh** in mode A, log it and drop to mode B automatically. This is your in-field regression detector for the GPIO back-feed, the LoRa rail, and the WiFi storm — all three of which announce themselves as a step change in daily mAh long before the battery dies.

---

## 5. Measurements to take in the first week, with expected values

Instrument: Nordic **PPK2** (or INA228 + logger) in series with the battery. Not a bench DMM — you need the µA floor *and* the 340 mA peaks in the same capture, and you need charge integration.

### Tier 1 — do these before writing any application code

| # | Measurement | Expected | Red flag → diagnosis |
|---|---|---|---|
| 1 | **Deep-sleep current at VBAT after the full §3 shutdown sequence** | **150–200 µA** | 400–900 µA → sequence incomplete: check TPS65185 in SLEEP (130 µA), BQ27220 in SLEEP (50 µA), GT911 in RST (70–120 µA). **>1 mA → EPD GPIO back-feed (epdiy #136).** Bisect by isolating the 13 panel pins in groups. **20–40 mA → LoRa/GPS LDO still on** (R21 pull-up); clear XL9555 PORT0 bit 0. **>200 mA → panel rail never came down.** |
| 2 | **Same, with and without a microSD inserted** | Delta **<100 µA** | 0.5–1 mA → the card is not idling. **There is no power gate for the SD slot on the schematic.** If it's this bad, don't ship with a card, or add one. |
| 3 | **Light-sleep current at VBAT** (if you insist on evaluating it) | 570 µA best case, **1.2–2.2 mA realistically** | >2.5 mA → `CONFIG_PM_SLP_DISABLE_GPIO` not active (its help claims 200–300 µA). **>50 mA → the epdiy back-feed is live in light sleep too. Abandon light sleep at that point; it is not a tuning problem.** |
| 4 | **`esp_psram_get_size()` at boot** | **8388608** | Anything else → octal PSRAM misconfigured. epdiy's `heap_caps_aligned_alloc(..., MALLOC_CAP_SPIRAM)` calls are bare `assert()`s — you'll get an abort, not an error. |
| 5 | **`.rtc.data` survives deep sleep** | Magic word round-trips | Fails → `--gc-sections` ate it under the esp-idf-sys linker script. Fall back to `.rtc_noinit`. Binary pass/fail; test it on day one because the whole clock-tick design depends on it. |

### Tier 2 — per-event energy, once the app runs

| # | Measurement | Expected | Red flag → diagnosis |
|---|---|---|---|
| 6 | **Charge per network wake** (PPK2 integral, sleep-to-sleep) | **360 mAs** optimized · 670 mAs measured-class | **>1000 mAs → you are cold-connecting every time.** BSSID+channel cache or static IP not taking effect. This is the single biggest lever in the budget: cold vs optimized is 2.4×. |
| 7 | **Charge per clock tick** (no radio) | **38 mAs** deep sleep | >60 mAs → boot too slow (`CONFIG_BOOTLOADER_SKIP_VALIDATE_IN_DEEP_SLEEP`, PSRAM memtest) or you're doing GC16 instead of DU. |
| 8 | **`mbedtls_ssl_handshake` wall time, cold vs resumed session ticket** | **~800 ms cold, <300 ms resumed** | **>3 s → esp-idf #10523 class problem.** Verify `CONFIG_MBEDTLS_HARDWARE_AES/SHA/MPI=y`. This is the widest error bar in the whole budget: 80 mAs vs 590 mAs. |
| 9 | **WiFi associate → IP assigned** | **<500 ms** with cached BSSID + channel + static IP | 2–4 s → full scan or DHCP still running. Worth ~300 mAs/wake. |
| 10 | **GC16 and DU wall-clock on real ED047TC1** | **0.5–1.0 s / 0.2–0.35 s** (derived from epdiy's `phase_times[30]` / `[15]`, **not from a scope**) | >2 s GC16 → bus clock or waveform mismatch. Note the community measured ~1.5 s GC16 on this panel vs LilyGo's 630 ms claim; expect the pessimistic end. |
| 11 | **Panel rail current during refresh** | **~115 mA @3.6 V** | >200 mA sustained → check VCOM (`-1600 mV`) and waveform selection. |
| 12 | **Boot to app-ready from deep sleep** | **~300 ms**, ~200 ms with skip-validate | >600 ms → PSRAM memtest on, or bootloader image validation on every wake. |

### Tier 3 — the week-long integral (this is the one that actually answers the question)

| # | Measurement | Expected | Red flag |
|---|---|---|---|
| 13 | **Daily mAh from BQ27220 accumulated charge, logged to NVS** | **20 ± 4 mAh/day** in mode A (17 mAh/day with hash-skip working) | **>40 mAh/day on day 1 → something in the shutdown sequence is not landing.** Don't wait a week to find out; this number is visible in 24 h. |
| 14 | **SoC after 7 days, from full** | **~88–90% remaining** (142 mAh of 1200 usable) | <70% → you're at ≥2.5× budget. Below 50% → go back to #1. |
| 15 | **Reset-reason ring buffer** (`ResetReason::get()` persisted to NVS) | **Zero unexpected resets in 7 days** | Any `Brownout` → radio/panel overlap, or end-of-charge VBAT below LDO dropout. Any `TaskWatchdog` → set `CONFIG_ESP_TASK_WDT_PANIC=y` and `CHECK_IDLE_TASK_CPU0=n` (a blocking EPD frame push starves idle). N consecutive resets → enter safe mode that skips network + panel entirely, or the reset loop drains the cell. |
| 16 | **VBAT at the moment of death** (log every wake) | **~3.48 V**, at **8–12% reported SoC** | Dies at 25–30% SoC → BQ27220 needs a learning cycle, or LDO dropout is worse than the 180 mV spec. Dies at 3.7 V → something is sagging the rail hard; suspect a refresh/TX overlap. |
| 17 | **WiFi failure counter and cumulative radio-on seconds per day** | **<200 s/day** at 15-min cadence (64 wakes × ~2.5 s) | >600 s/day → retry loop. This costs more than everything else combined; see R1. |

---

## Bottom line

- **Light sleep + one week is achievable at ≥5 min network cadence (10–23 days), and fails at 1 min (3.9–7.2 days).** So the honest answer to the literal question is "yes, if you slow the cadence" — but you'd be choosing the worse of two options that both work.
- **Light sleep buys 13.5 mAs per tick and costs 412–1995 µA of floor. It breaks even at a ~33 s tick in the best case and ~11 s realistically. You want 60 s. It loses.** Deep sleep gives 42.1 days where light sleep gives 23.2 at the identical cadence and identical user-visible behaviour.
- **The clock does not need light sleep.** PCF8563 + `.rtc.data` + a DU on the clock rectangle gives you a minute clock for 38 mAs, on a 20 mAh/day budget.
- **Recommended: deep sleep, 1-min tick and 15-min fetch during 07:00–23:00, dark at night → 20.25 mAh/day = 0.844 mA = 59 days = 8.5× the requirement.** Even the untuned out-of-box board hits 32 days.
- **Your risk is not the budget; it is four bugs that each cost 10–200× the budget:** the epdiy GPIO back-feed (5.1 h), the LoRa/GPS LDO left on by its own pull-up (40 h), a WiFi retry storm (<1 day), and a seconds clock (3.3 days plus a dead panel in 12). Measure #1, #13 and #17 first; everything else is bookkeeping.
- **Explicit corpus gaps, my estimates labelled:** no light-sleep measurement exists for this board from anyone including LilyGo (the only vendor number, 873 µA, doesn't even name the mode); the 2.15 mA pessimistic floor is my estimate from a third-party S3 PCB; the 24.5 mAs light-sleep tick is my derivation from the 38 mAs composite minus a 13.5 mAs boot estimate; the 235 mA back-feed is measured on a T5-4.7, not this board, though the mechanism and driver are identical; GC16/DU durations are derived from epdiy waveform tables, not a scope.