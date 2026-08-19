//! Ukrywanie sekretów w adresach, zanim trafią do logu albo na ekran.
//!
//! Prywatny adres iCal Google jest **stałym bearerem do całego kalendarza** — bez
//! zakresu i bez terminu ważności. Wypisanie go w logu z konsoli szeregowej albo
//! na ekranie konfiguracji jest równoważne z podaniem hasła.

/// Ukrywa tajny fragment prywatnego adresu iCal.
///
/// Adresy Google przycinamy tuż za `/private-`, bo dokładnie tam zaczyna się sekret.
/// Dla wszystkiego innego pokazujemy sam host: nie wiemy, gdzie w takim adresie
/// siedzi materiał uwierzytelniający, więc zakładamy, że wszędzie.
pub fn redact(url: &str) -> String {
    match url.find("/private-") {
        Some(i) => format!("{}/private-***", &url[..i]),
        None => match url.split('/').nth(2) {
            Some(host) if !host.is_empty() => host.to_string(),
            _ => "(adres)".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn ukrywa_tajny_fragment_adresu() {
        let url = "https://calendar.google.com/calendar/ical/ktos%40gmail.com/private-abc123def/basic.ics";
        let r = redact(url);
        assert!(
            !r.contains("abc123def"),
            "tajny klucz nie może wyciec do logu: {r}"
        );
        assert!(r.contains("calendar.google.com"));
    }

    #[test]
    fn inne_adresy_pokazuja_tylko_host() {
        assert_eq!(redact("https://przyklad.pl/kalendarz.ics"), "przyklad.pl");
    }

    #[test]
    fn adres_bez_hosta_nie_panikuje() {
        assert_eq!(redact("firmware-ota.bin"), "(adres)");
        assert_eq!(redact(""), "(adres)");
        assert_eq!(redact("http://"), "(adres)");
    }
}
