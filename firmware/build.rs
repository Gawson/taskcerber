fn main() {
    embuild::espidf::sysenv::output();
    emit_version();
}

/// Ustawia `T5_VERSION` — łańcuch, który firmware raportuje i który porównuje OTA.
///
/// Liczy go `tools/version.sh`, i to jest celowo JEDNO miejsce: `build-image.sh`
/// woła ten sam skrypt i wpisuje wynik do `ota.json`. Gdyby każde z nich liczyło
/// wersję po swojemu, rozjazd objawiłby się dopiero na urządzeniu, jako pętla
/// aktualizacji do wersji, której obraz nigdy nie zaraportuje.
///
/// `build-image.sh` przekazuje wynik przez zmienną środowiskową, więc w tej ścieżce
/// skrypt liczy się raz. Gołe `cargo build` woła go stąd.
fn emit_version() {
    // Zmiana T5_VERSION MUSI unieważniać build — inaczej w obrazie zostaje stary
    // łańcuch, a `check-image.sh` przyłapie to dopiero przy pakowaniu.
    println!("cargo:rerun-if-env-changed=T5_VERSION");
    println!("cargo:rerun-if-changed=../tools/version.sh");

    let version = std::env::var("T5_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(from_script)
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());

    println!("cargo:rustc-env=T5_VERSION={version}");
}

fn from_script() -> Option<String> {
    let output = std::process::Command::new("../tools/version.sh")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!version.is_empty()).then_some(version)
}
