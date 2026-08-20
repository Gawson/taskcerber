use dashboard::{Fonts, Weight};

fn main() {
    let f = Fonts::embedded();
    let probe = |s: &str, size: f32, w: Weight| {
        println!("{:>8.1} {:<8} {:>7.1}px  {:?}", size, format!("{:?}", w), f.measure(s, size, w), s);
    };

    println!("== wysokosc linii / ascent ==");
    for size in [19.0f32, 22.0, 27.0, 34.0, 44.0, 76.0] {
        for w in [Weight::Regular, Weight::Medium, Weight::Bold] {
            println!("size {:>4} {:<8} lh={:>6.2} asc={:>6.2}", size, format!("{:?}", w), f.line_height(size, w), f.ascent(size, w));
        }
    }

    println!("\n== cyfry dnia (1..31) ==");
    for size in [19.0f32, 22.0, 27.0, 34.0, 44.0] {
        for w in [Weight::Medium, Weight::Bold] {
            let w1 = f.measure("8", size, w);
            let w2 = f.measure("28", size, w);
            println!("size {:>4} {:<8} '8'={:>6.2}  '28'={:>6.2}", size, format!("{:?}", w), w1, w2);
        }
    }

    println!("\n== skroty dni tygodnia ==");
    for size in [19.0f32, 22.0, 27.0] {
        for w in [Weight::Medium, Weight::Bold] {
            let mut max = 0.0f32;
            let mut maxs = "";
            for s in ["pon","wto","śro","czw","pią","sob","nie","P","W","Ś","C","P","S","N","PN","WT","ŚR","CZ","PT","SB","ND"] {
                let m = f.measure(s, size, w);
                if m > max { max = m; maxs = s; }
            }
            println!("size {:>4} {:<8} najszerszy={:>6.2} ({})", size, format!("{:?}", w), max, maxs);
        }
        for w in [Weight::Medium, Weight::Bold] {
            println!("  3-lit: {:?} pon={:.2} śro={:.2} czw={:.2} pią={:.2}", w,
                f.measure("pon", size, w), f.measure("śro", size, w),
                f.measure("czw", size, w), f.measure("pią", size, w));
            println!("  2-lit: {:?} PN={:.2} WT={:.2} ŚR={:.2} CZ={:.2} PT={:.2} SB={:.2} ND={:.2}", w,
                f.measure("PN", size, w), f.measure("WT", size, w), f.measure("ŚR", size, w),
                f.measure("CZ", size, w), f.measure("PT", size, w), f.measure("SB", size, w), f.measure("ND", size, w));
        }
    }

    println!("\n== godziny ==");
    for size in [19.0f32, 22.0, 27.0] {
        for w in [Weight::Medium, Weight::Bold] {
            probe("08:30", size, w);
            probe("8:30", size, w);
            probe("18:30", size, w);
        }
    }

    println!("\n== typowe tytuly ==");
    let tytuly = [
        "Stand-up zespołu",
        "Przegląd architektury — kwartał",
        "1:1 z Łukaszem",
        "Trening — ćwiczenia siłowe",
        "Warsztat: strategia produktu",
        "Obiad z Agnieszką",
        "Wyjazd — Gdańsk",
        "Dentysta",
        "Urodziny Ani",
    ];
    for size in [19.0f32, 22.0] {
        for w in [Weight::Medium, Weight::Regular] {
            println!("--- size {} {:?}", size, w);
            for t in tytuly {
                println!("   {:>7.1}px ({} zn., {:.2} px/zn) {}", f.measure(t, size, w), t.chars().count(), f.measure(t, size, w)/t.chars().count() as f32, t);
            }
        }
    }

    println!("\n== srednia szerokosc znaku, proba jezykowa ==");
    let proba = "Spotkanie zespołu w sprawie planu na przyszły kwartał i budżetu, Warsztat strategia produktu, Obiad z Agnieszką, Trening ćwiczenia siłowe";
    for size in [19.0f32, 22.0, 27.0, 34.0] {
        for w in [Weight::Regular, Weight::Medium, Weight::Bold] {
            let n = proba.chars().count() as f32;
            println!("size {:>4} {:<8} avg={:.3} px/zn", size, format!("{:?}", w), f.measure(proba, size, w)/n);
        }
    }

    println!("\n== ile znakow wchodzi w szerokosc N px ==");
    for szer in [56i32, 60, 64, 66, 68, 72, 76, 84, 92, 100, 112, 120, 128, 136] {
        for size in [19.0f32, 22.0] {
            for w in [Weight::Medium] {
                // ile znakow proby zmiesci sie
                let mut acc = 0.0;
                let mut n = 0;
                for ch in proba.chars() {
                    let cw = f.measure(&ch.to_string(), size, w);
                    if acc + cw > szer as f32 { break; }
                    acc += cw; n += 1;
                }
                println!("szer {:>4} size {:>4} {:?} -> ~{} znakow", szer, size, w, n);
            }
        }
    }

    println!("\n== nazwy miesiecy ==");
    for size in [19.0f32, 22.0, 27.0, 34.0, 44.0] {
        for w in [Weight::Bold] {
            let mut max = 0.0f32; let mut maxs = "";
            for m in ["styczeń","luty","marzec","kwiecień","maj","czerwiec","lipiec","sierpień","wrzesień","październik","listopad","grudzień"] {
                let x = f.measure(m, size, w); if x > max { max = x; maxs = m; }
            }
            let mut max3 = 0.0f32; let mut max3s = "";
            for m in ["sty","lut","mar","kwi","maj","cze","lip","sie","wrz","paź","lis","gru"] {
                let x = f.measure(m, size, w); if x > max3 { max3 = x; max3s = m; }
            }
            println!("size {:>4} {:?} pelna_max={:.1} ({})  skrot_max={:.1} ({})", size, w, max, maxs, max3, max3s);
        }
    }

    println!("\n== liczniki i etykiety ==");
    for size in [19.0f32, 22.0] {
        for w in [Weight::Bold, Weight::Medium] {
            probe("+3", size, w);
            probe("+12", size, w);
            probe("12", size, w);
            probe("cały dzień", size, w);
            probe("dziś", size, w);
        }
    }
}
