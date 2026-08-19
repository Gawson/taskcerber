//! Symulator LilyGo T5 E-Paper S3 Pro.
//!
//! Okno pokazuje **dokładnie to**, co pojawi się na panelu: ten sam kod renderujący
//! z crate'a `dashboard`, ta sama kwantyzacja do 16 poziomów szarości, te same
//! obszary dotykowe. Myszka udaje palec.
//!
//! ```text
//! cargo run -p simulator                        # dane demonstracyjne
//! cargo run -p simulator -- --ics <URL>         # prawdziwy kanał iCal
//! cargo run -p simulator -- --file kalendarz.ics
//! cargo run -p simulator -- --scale 2           # powiększenie (domyślnie 1)
//! ```
//!
//! Klawiatura:
//! ```text
//!   spacja   pełne odświeżenie          B  cykl poziomu baterii
//!   ←  →     zmiana strony              N  cykl stanu sieci
//!   Esc      powrót z widoku szczegółów G  duchy po szybkim odświeżaniu wł/wył
//!   R        ponowne pobranie kanału    S  zrzut PNG do out/simulator.png
//!   1–4      scenariusze demonstracyjne Q  wyjście
//! ```

mod device;
mod feed;
mod scenarios;

use std::time::Duration;

use dashboard::model::{Battery, NetState};
use dashboard::{Action, Rotation};
use minifb::{Key, MouseButton, MouseMode, Window, WindowOptions};

use device::{Device, Phase, Refresh};

/// Wysokość paska stanu pod panelem — to nie jest część urządzenia,
/// tylko przyrządy pomiarowe symulatora.
const CHROME_H: usize = 64;

fn main() {
    let args = Args::parse();

    let (model, source_label) = match (&args.ics, &args.file) {
        (Some(url), _) => match feed::from_url(url, args.days) {
            Ok(m) => (m, format!("iCal: {}", feed::redact(url))),
            Err(e) => {
                eprintln!("nie mogę pobrać kanału: {e}");
                eprintln!("wracam do danych demonstracyjnych");
                (scenarios::week(), "demo (pobieranie nieudane)".to_string())
            }
        },
        (_, Some(path)) => match feed::from_file(path, args.days) {
            Ok(m) => (m, format!("plik: {path}")),
            Err(e) => {
                eprintln!("nie mogę wczytać pliku: {e}");
                (scenarios::week(), "demo (wczytanie nieudane)".to_string())
            }
        },
        _ => (scenarios::week(), "demo".to_string()),
    };

    println!("źródło danych: {source_label}");
    println!("wydarzeń: {}", model.event_count());

    let rotation = if args.landscape {
        Rotation::Landscape
    } else {
        Rotation::Portrait
    };
    let mut dev = Device::new(model, rotation);
    let (width, height) = (dev.width(), dev.height());

    let scale = args.scale.clamp(1, 3);
    let win_w = width * scale;
    let win_h = height * scale + CHROME_H;

    let mut window = match Window::new(
        "T5 E-Paper S3 Pro — symulator",
        win_w,
        win_h,
        WindowOptions {
            resize: false,
            ..WindowOptions::default()
        },
    ) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("nie mogę otworzyć okna: {e}");
            eprintln!();
            eprintln!("Symulator potrzebuje sesji graficznej (X11 lub Wayland).");
            eprintln!("Bez niej użyj podglądu statycznego: cargo run -p preview -- all");
            std::process::exit(1);
        }
    };
    window.set_target_fps(60);

    let mut buffer = vec![0u32; win_w * win_h];
    let mut battery_step = 0usize;
    let mut net_step = 0usize;
    let mut mouse_was_down = false;
    let mut last_action: Option<Action> = None;

    while window.is_open() && !window.is_key_down(Key::Q) {
        // --- wejście: mysz jako dotyk ---------------------------------------
        let mouse_down = window.get_mouse_down(MouseButton::Left);
        if mouse_down && !mouse_was_down {
            if let Some((mx, my)) = window.get_mouse_pos(MouseMode::Discard) {
                let px = mx as i32 / scale as i32;
                let py = my as i32 / scale as i32;
                if py >= 0 && (py as usize) < height {
                    if let Some(action) = dev.touch(px, py) {
                        last_action = Some(action);
                        println!("dotyk ({px}, {py}) -> {action:?}");
                    } else {
                        println!("dotyk ({px}, {py}) -> brak akcji");
                    }
                }
            }
        }
        mouse_was_down = mouse_down;

        // --- wejście: klawiatura --------------------------------------------
        window
            .get_keys_pressed(minifb::KeyRepeat::No)
            .into_iter()
            .for_each(|key| match key {
                Key::Space => dev.apply(Action::RefreshNow),
                // Na urządzeniu wchodzi się w konfigurację dotykiem (wersja w stopce
                // albo plakietka „skonfiguruj urządzenie"); tutaj skrótem, bo
                // trafianie myszą w 15-pikselowy napis jest testem cierpliwości.
                Key::K => dev.apply(Action::OpenSetup),
                Key::Right => dev.apply(Action::NextPage),
                Key::Left => dev.apply(Action::PrevPage),
                Key::Escape => dev.apply(Action::Back),
                Key::B => {
                    battery_step = (battery_step + 1) % BATTERY_STEPS.len();
                    let (pct, charging) = BATTERY_STEPS[battery_step];
                    dev.set_battery(Battery {
                        percent: Some(pct),
                        millivolts: Some(3600),
                        charging,
                    });
                    dev.repaint();
                }
                Key::N => {
                    net_step = (net_step + 1) % 4;
                    let net = match net_step {
                        0 => NetState::Ok,
                        1 => NetState::Stale {
                            since: dev.model.now - chrono::Duration::hours(5),
                        },
                        2 => NetState::Offline,
                        _ => NetState::NeedsAuth,
                    };
                    dev.set_net(net);
                }
                Key::G => {
                    dev.simulate_ghosting = !dev.simulate_ghosting;
                    println!(
                        "symulacja duchów: {}",
                        if dev.simulate_ghosting { "wł" } else { "wył" }
                    );
                }
                Key::S => match save_png(&dev) {
                    Ok(path) => println!("zapisano {path}"),
                    Err(e) => eprintln!("nie mogę zapisać PNG: {e}"),
                },
                Key::R => {
                    if let Some(url) = &args.ics {
                        println!("pobieram ponownie…");
                        match feed::from_url(url, args.days) {
                            Ok(m) => {
                                dev.set_events(m.days, m.now, NetState::Ok);
                                println!("pobrano {} wydarzeń", dev.model.event_count());
                            }
                            Err(e) => {
                                eprintln!("pobieranie nieudane: {e}");
                                dev.set_net(NetState::Offline);
                            }
                        }
                    } else {
                        println!("brak --ics, nie ma czego pobierać");
                    }
                }
                Key::Key1 => load_scenario(&mut dev, scenarios::week()),
                Key::Key2 => load_scenario(&mut dev, scenarios::empty()),
                Key::Key3 => load_scenario(&mut dev, scenarios::busy()),
                Key::Key4 => load_scenario(&mut dev, scenarios::edge_cases()),
                _ => {}
            });

        // --- odświeżanie panelu ---------------------------------------------
        dev.tick();

        // --- rysowanie okna --------------------------------------------------
        draw(&mut buffer, win_w, &dev, scale, &source_label, last_action);
        if window.update_with_buffer(&buffer, win_w, win_h).is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

const BATTERY_STEPS: [(u8, bool); 6] = [
    (78, false),
    (42, false),
    (15, false),
    (5, false),
    (60, true),
    (100, true),
];

fn load_scenario(dev: &mut Device, model: dashboard::Model) {
    let now = model.now;
    let net = model.net;
    dev.model.tiles = model.tiles.clone();
    dev.model.battery = model.battery;
    dev.model.firmware = model.firmware.clone();
    dev.set_events(model.days, now, net);
}

fn save_png(dev: &Device) -> Result<String, Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;
    let path = "out/simulator.png".to_string();
    let file = std::fs::File::create(&path)?;
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(file),
        dev.width() as u32,
        dev.height() as u32,
    );
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(dev.panel().pixels())?;
    Ok(path)
}

/// Składa zawartość okna: panel u góry, przyrządy symulatora na dole.
fn draw(
    buffer: &mut [u32],
    win_w: usize,
    dev: &Device,
    scale: usize,
    source: &str,
    last_action: Option<Action>,
) {
    let phase = dev.phase();
    let panel = dev.panel();
    let (width, height) = (dev.width(), dev.height());

    // Panel, ze skalowaniem przez powielanie pikseli — bez interpolacji, żeby
    // było widać dokładnie te piksele, które trafią na szkło.
    for y in 0..height * scale {
        let sy = y / scale;
        for x in 0..width * scale {
            let sx = x / scale;
            let mut v = panel.get(sx as i32, sy as i32);

            // Animacja odświeżania — to, co widać na prawdziwym panelu.
            v = match phase {
                Phase::Flashing => {
                    // Seria inwersji przy pełnym odświeżeniu.
                    let t = dev.progress();
                    let band = ((t * 6.0) as u32) % 2;
                    if band == 0 {
                        255 - v
                    } else {
                        v
                    }
                }
                Phase::Settling => {
                    // Panel dochodzi do docelowej jasności.
                    let t = dev.progress();
                    let mid = 160.0;
                    (v as f32 * t + mid * (1.0 - t)) as u8
                }
                Phase::Idle => v,
            };

            buffer[y * win_w + x] = gray_to_rgb(v);
        }
    }

    // Pasek przyrządów.
    let chrome_top = height * scale;
    for y in chrome_top..chrome_top + CHROME_H {
        for x in 0..win_w {
            buffer[y * win_w + x] = 0x00_1C_1C_1A;
        }
    }
    // Linia oddzielająca panel od przyrządów — żeby było jasne, co jest urządzeniem.
    for x in 0..win_w {
        buffer[chrome_top * win_w + x] = 0x00_44_44_40;
    }

    // Ekran konfiguracji nie ma paginacji — pokazywanie „strona 1/1" sugerowałoby,
    // że strzałki coś tam robią.
    let gdzie = if dev.setup_open() {
        "konfiguracja".to_string()
    } else {
        format!("strona {}/{}", dev.screen.page + 1, dev.screen.pages.max(1))
    };
    let line1 = format!(
        "{gdzie}   {}   render {:.1} ms   pełnych {}  szybkich {}",
        match dev.refresh_mode() {
            Refresh::Full => "GC16",
            Refresh::Fast => "DU",
        },
        dev.stats.last_render_us as f64 / 1000.0,
        dev.stats.full_refreshes,
        dev.stats.fast_refreshes,
    );
    let line2 = match last_action {
        Some(a) => format!("{source}   ·   ostatnia akcja: {a:?}"),
        None if dev.setup_open() => {
            format!("{source}   ·   stukaj w klawisze myszą   ·   zapisz=wyjście z konfiguracji")
        }
        None => format!(
            "{source}   ·   spacja=odśwież  ←→=strony  B=bateria  N=sieć  K=konfiguracja  S=PNG  Q=wyjście"
        ),
    };

    text::draw(buffer, win_w, 12, chrome_top + 14, &line1, 0x00_E8_E6_DF);
    text::draw(buffer, win_w, 12, chrome_top + 36, &line2, 0x00_9A_9A_94);
}

fn gray_to_rgb(v: u8) -> u32 {
    // Papier e-ink nie jest idealnie biały ani czarny; lekkie ocieplenie i
    // ograniczenie zakresu daje obraz bliższy temu, co widać na szkle.
    let t = v as f32 / 255.0;
    let r = (26.0 + t * (243.0 - 26.0)) as u32;
    let g = (26.0 + t * (241.0 - 26.0)) as u32;
    let b = (24.0 + t * (233.0 - 24.0)) as u32;
    (r << 16) | (g << 8) | b
}

// ---------------------------------------------------------------------------

struct Args {
    ics: Option<String>,
    file: Option<String>,
    scale: usize,
    days: i64,
    /// Poziomo zamiast domyślnego pionu — ta sama treść, drugi układ.
    landscape: bool,
}

impl Args {
    fn parse() -> Self {
        let mut args = Self {
            ics: None,
            file: None,
            landscape: false,
            scale: 1,
            days: 14,
        };
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--ics" => args.ics = it.next(),
                "--file" => args.file = it.next(),
                "--scale" => args.scale = it.next().and_then(|v| v.parse().ok()).unwrap_or(1),
                "--days" => args.days = it.next().and_then(|v| v.parse().ok()).unwrap_or(14),
                // Symulator domyślnie stoi tak, jak urządzenie: pionowo.
                "--landscape" => args.landscape = true,
                "--help" | "-h" => {
                    println!("{}", HELP);
                    std::process::exit(0);
                }
                other => eprintln!("nieznany argument: {other}"),
            }
        }
        // Adres można też podać zmienną środowiskową, żeby nie lądował w historii powłoki.
        if args.ics.is_none() {
            args.ics = std::env::var("T5_ICS_URL").ok();
        }
        args
    }
}

const HELP: &str = "\
Symulator LilyGo T5 E-Paper S3 Pro

  --ics <URL>     pobierz prawdziwy kanał iCal (albo zmienna T5_ICS_URL)
  --file <plik>   wczytaj kanał z pliku .ics
  --scale <1..3>  powiększenie okna
  --days <n>      ile dni do przodu pokazać (domyślnie 14)

Klawiatura:
  spacja  pełne odświeżenie     B  cykl poziomu baterii
  ← →     zmiana strony         N  cykl stanu sieci
  Esc     powrót ze szczegółów  G  duchy wł/wył
  R       pobierz ponownie      S  zrzut PNG
  K       ekran konfiguracji    Q  wyjście
  1-4     scenariusze

Ekran konfiguracji obsługuje się myszą jak palcem — to te same regiony dotykowe,
które dostanie firmware z GT911. Na urządzeniu wchodzi się w niego dotknięciem
wersji w stopce albo plakietki \"skonfiguruj urządzenie\".";

/// Minimalny renderer tekstu 5x7 dla paska przyrządów.
///
/// Świadomie osobny od `dashboard::text` — pasek stanu **nie jest** częścią
/// urządzenia i nie powinien korzystać z jego kroju ani jego kodu, żeby nie było
/// wątpliwości, co jest symulacją, a co przyrządem.
mod text {
    const GLYPH_W: usize = 6;
    const GLYPH_H: usize = 7;

    pub fn draw(buffer: &mut [u32], win_w: usize, x0: usize, y0: usize, s: &str, color: u32) {
        let mut x = x0;
        for ch in s.chars() {
            let glyph = glyph_for(ch);
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        let px = x + col;
                        let py = y0 + row;
                        if px < win_w {
                            let idx = py * win_w + px;
                            if idx < buffer.len() {
                                buffer[idx] = color;
                            }
                        }
                    }
                }
            }
            x += GLYPH_W;
            if x + GLYPH_W >= win_w {
                break;
            }
        }
        let _ = GLYPH_H;
    }

    /// Bardzo mały krój 5x7. Znaki spoza zestawu rysowane są jako kropka —
    /// pasek stanu ma być czytelny, nie ładny.
    fn glyph_for(ch: char) -> [u8; 7] {
        match ch.to_ascii_lowercase() {
            '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
            '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
            '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
            '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
            '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
            '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
            '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
            '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
            '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
            '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
            'a' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
            'b' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
            'c' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
            'd' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
            'e' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
            'f' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
            'g' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
            'h' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
            'i' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
            'j' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
            'k' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
            'l' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
            'm' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
            'n' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
            'o' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
            'p' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
            'q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
            'r' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
            's' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
            't' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
            'u' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
            'v' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
            'w' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
            'x' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
            'y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
            'z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
            ' ' => [0; 7],
            '.' => [0, 0, 0, 0, 0, 0x0C, 0x0C],
            ',' => [0, 0, 0, 0, 0x0C, 0x0C, 0x08],
            ':' => [0, 0x0C, 0x0C, 0, 0x0C, 0x0C, 0],
            '/' => [0x01, 0x02, 0x02, 0x04, 0x08, 0x08, 0x10],
            '-' => [0, 0, 0, 0x1F, 0, 0, 0],
            '=' => [0, 0, 0x1F, 0, 0x1F, 0, 0],
            '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
            ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
            '·' => [0, 0, 0, 0x04, 0, 0, 0],
            '←' => [0x04, 0x08, 0x1F, 0x08, 0x04, 0, 0],
            '→' => [0x04, 0x02, 0x1F, 0x02, 0x04, 0, 0],
            _ => [0, 0, 0, 0x04, 0, 0, 0],
        }
    }
}
