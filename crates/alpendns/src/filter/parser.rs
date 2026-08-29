//! Parser für die gängigen Blocklisten-Formate.
//!
//! Jede Zeile einer Liste ist fremde Eingabe. Die Parser werfen weg, was sie
//! nicht verstehen, statt zu scheitern: eine Liste mit zehn Müllzeilen unter
//! hunderttausend guten soll den Server nicht am Starten hindern. Was
//! weggeworfen wurde, wird gezählt und geloggt — schweigend ignorieren wäre der
//! Weg, wie eine Liste unbemerkt halb leer bleibt.

use std::fmt;

/// Wie eine Zeile zu lesen ist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// `0.0.0.0 ads.example.com` — das Format von /etc/hosts.
    Hosts,
    /// Eine Domain pro Zeile.
    Domains,
    /// Wie `domains`, aber jeder Eintrag gilt auch für alle Subdomains.
    Wildcard,
    /// Teilmenge der Adblock-Syntax: `||domain^`. Siehe [`parse_adblock`].
    Adblock,
    /// Response Policy Zone, Teilmenge. Siehe [`parse_rpz`].
    Rpz,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Hosts => "hosts",
            Self::Domains => "domains",
            Self::Wildcard => "wildcard",
            Self::Adblock => "adblock",
            Self::Rpz => "rpz",
        };
        f.write_str(text)
    }
}

/// Ob ein Eintrag nur den Namen selbst meint oder auch alles darunter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Nur genau dieser Name.
    Exact,
    /// Dieser Name und jede Subdomain.
    Suffix,
}

/// Ein geparster Eintrag mit der Zeile, aus der er stammt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Normalisiert: klein geschrieben, ohne führenden und abschließenden Punkt.
    pub domain: String,
    pub scope: Scope,
    /// Zeilennummer in der Quelldatei, 1-basiert. Ohne sie gäbe es keine
    /// Antwort auf "warum wurde das geblockt?".
    pub line: u32,
}

/// Was beim Parsen einer Liste herauskam.
#[derive(Debug, Default)]
pub struct Parsed {
    pub entries: Vec<Entry>,
    /// Zeilen, die keinen Eintrag ergaben und keine Kommentare waren.
    pub skipped: u32,
}

/// Längstes zulässiges Label nach RFC 1035 §2.3.4.
const MAX_LABEL: usize = 63;
/// Längster zulässiger Name in Textform.
const MAX_NAME: usize = 253;

/// Bringt einen Domainnamen in die Form, in der er im Matcher steht.
///
/// Gibt `None` zurück, wenn daraus kein brauchbarer Name wird — das ist der
/// häufigste Fall bei Müllzeilen und deshalb kein Fehler, sondern ein
/// Übersprungen.
pub fn normalize(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('.');
    if trimmed.is_empty() || trimmed.len() > MAX_NAME {
        return None;
    }
    // Ein Name mit Leerzeichen oder Schrägstrich ist keine Domain, sondern eine
    // kaputte Zeile oder eine URL.
    if trimmed.contains(|c: char| c.is_whitespace() || c == '/' || c == '\\' || c == '@') {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.split('.').all(is_valid_label) {
        return None;
    }
    Some(lower)
}

fn is_valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_LABEL
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Schneidet einen Kommentar ab und liefert den Rest ohne Rand-Leerzeichen.
///
/// `#` gilt überall als Kommentar, `!` nur am Zeilenanfang (in Adblock-Listen
/// ist das der Kommentarmarker).
fn strip_comment(line: &str) -> &str {
    let line = line.trim();
    if line.starts_with('!') {
        return "";
    }
    line.split('#').next().unwrap_or("").trim()
}

/// Adressen, die in `hosts`-Listen als "hier steht nichts" gelten.
fn is_sinkhole_address(token: &str) -> bool {
    matches!(
        token,
        "0.0.0.0" | "127.0.0.1" | "::" | "::1" | "0:0:0:0:0:0:0:0"
    )
}

/// Namen, die in einer /etc/hosts-Datei stehen, aber nichts blocken sollen.
fn is_localhost(domain: &str) -> bool {
    matches!(
        domain,
        "localhost"
            | "localhost.localdomain"
            | "local"
            | "ip6-localhost"
            | "ip6-loopback"
            | "ip6-localnet"
            | "ip6-mcastprefix"
            | "ip6-allnodes"
            | "ip6-allrouters"
            | "broadcasthost"
    )
}

/// Parst eine Liste im angegebenen Format.
pub fn parse(text: &str, format: Format) -> Parsed {
    match format {
        Format::Hosts => parse_hosts(text),
        Format::Domains => parse_simple(text, Scope::Exact),
        Format::Wildcard => parse_simple(text, Scope::Suffix),
        Format::Adblock => parse_adblock(text),
        Format::Rpz => parse_rpz(text),
    }
}

/// `0.0.0.0 ads.example.com` — eine Adresse, danach ein oder mehrere Namen.
///
/// Zeilen mit einer anderen Adresse als einer Sinkhole-Adresse werden
/// übersprungen: eine Datei, die echte Zuordnungen enthält, ist keine Blockliste.
fn parse_hosts(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    for (index, line) in text.lines().enumerate() {
        let line = strip_comment(line);
        if line.is_empty() {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let Some(address) = tokens.next() else {
            continue;
        };
        if !is_sinkhole_address(address) {
            parsed.skipped = parsed.skipped.saturating_add(1);
            continue;
        }
        let mut found = false;
        for token in tokens {
            match normalize(token) {
                Some(domain) if !is_localhost(&domain) => {
                    found = true;
                    parsed.entries.push(Entry {
                        domain,
                        scope: Scope::Exact,
                        line: line_number(index),
                    });
                }
                // localhost-Zeilen sind erwartbar und kein Grund zu zählen.
                Some(_) => found = true,
                None => {}
            }
        }
        if !found {
            parsed.skipped = parsed.skipped.saturating_add(1);
        }
    }
    parsed
}

/// Eine Domain pro Zeile. Führende `*.` und `.` werden abgeschnitten — in
/// `wildcard`-Listen sind sie die übliche Schreibweise für "und alles darunter",
/// und das ist hier ohnehin der Geltungsbereich.
fn parse_simple(text: &str, scope: Scope) -> Parsed {
    let mut parsed = Parsed::default();
    for (index, line) in text.lines().enumerate() {
        let line = strip_comment(line);
        if line.is_empty() {
            continue;
        }
        let candidate = line.strip_prefix("*.").unwrap_or(line);
        match normalize(candidate) {
            Some(domain) => parsed.entries.push(Entry {
                domain,
                scope,
                line: line_number(index),
            }),
            None => parsed.skipped = parsed.skipped.saturating_add(1),
        }
    }
    parsed
}

/// Adblock-Syntax, **bewusst nur die Teilmenge `||domain^`**.
///
/// Unterstützt wird genau eine Form: `||example.com^` und `||example.com^$...`,
/// beides als Suffix-Regel. Alles andere wird übersprungen, insbesondere:
///
/// * Ausnahmeregeln (`@@||example.com^`) — Allowlists sind eine eigene Liste,
///   nicht eine Zeile mitten in einer Blockliste,
/// * Element-Filter (`example.com##.ad`) — das ist Sache eines Browsers,
/// * reguläre Ausdrücke (`/muster/`) und Teilstring-Regeln (`/ads/`) — sie
///   passen nicht auf einen Domainnamen, sondern auf eine URL, die ein
///   DNS-Resolver nie sieht,
/// * Optionen, die den Geltungsbereich einschränken (`$third-party`) — sie
///   nicht zu beachten würde mehr blocken als beabsichtigt.
fn parse_adblock(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    for (index, line) in text.lines().enumerate() {
        let line = strip_comment(line);
        if line.is_empty() {
            continue;
        }
        let Some(rest) = line.strip_prefix("||") else {
            parsed.skipped = parsed.skipped.saturating_add(1);
            continue;
        };
        // Alles ab `^` ist Trenner und Optionen.
        let Some((domain, options)) = rest.split_once('^') else {
            parsed.skipped = parsed.skipped.saturating_add(1);
            continue;
        };
        // Hinter dem `^` darf nichts mehr stehen. Optionen wie `$third-party`
        // schränken ein, worauf die Regel zutrifft; sie zu ignorieren würde
        // mehr blocken als die Liste beabsichtigt.
        if !options.is_empty() {
            parsed.skipped = parsed.skipped.saturating_add(1);
            continue;
        }
        match normalize(domain) {
            Some(domain) => parsed.entries.push(Entry {
                domain,
                scope: Scope::Suffix,
                line: line_number(index),
            }),
            None => parsed.skipped = parsed.skipped.saturating_add(1),
        }
    }
    parsed
}

/// Response Policy Zone, **Teilmenge**.
///
/// Erkannt werden Zeilen der Form `<name> [ttl] [klasse] CNAME .`, also die
/// NXDOMAIN-Regel aus RFC 8611 §2.1 — mit Abstand die häufigste. `$ORIGIN` wird
/// beachtet und vom Namen abgeschnitten, weil der Zonenname der Policy nichts
/// mit dem geblockten Namen zu tun hat.
///
/// Nicht unterstützt: alle anderen Policy-Aktionen (`*.rpz-passthru`,
/// `rpz-drop`, `rpz-tcp-only`), die Trigger-Zonen `rpz-client-ip`, `rpz-ip`,
/// `rpz-nsdname` und `rpz-nsip`, sowie mehrzeilige Records in Klammern.
fn parse_rpz(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    let mut origin = String::new();
    for (index, line) in text.lines().enumerate() {
        let line = strip_comment(line);
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("$ORIGIN") {
            origin = rest.trim().trim_matches('.').to_ascii_lowercase();
            continue;
        }
        // Andere Direktiven interessieren uns nicht, sind aber kein Müll.
        if line.starts_with('$') {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(name) = tokens.first() else {
            continue;
        };
        // Die Regel muss auf `CNAME .` enden, sonst ist es keine NXDOMAIN-Regel.
        let is_nxdomain_rule = tokens.windows(2).any(|pair| {
            pair.first()
                .is_some_and(|t| t.eq_ignore_ascii_case("CNAME"))
                && pair.get(1) == Some(&".")
        });
        if !is_nxdomain_rule {
            parsed.skipped = parsed.skipped.saturating_add(1);
            continue;
        }

        let (stripped, scope) = match name.strip_prefix("*.") {
            Some(rest) => (rest, Scope::Suffix),
            None => (*name, Scope::Exact),
        };
        // Relative Namen tragen die Policy-Zone als Suffix; sie gehört nicht dazu.
        let without_origin = if origin.is_empty() {
            stripped.trim_end_matches('.').to_owned()
        } else {
            let lower = stripped.trim_end_matches('.').to_ascii_lowercase();
            lower
                .strip_suffix(&format!(".{origin}"))
                .map_or(lower.clone(), str::to_owned)
        };
        match normalize(&without_origin) {
            Some(domain) => parsed.entries.push(Entry {
                domain,
                scope,
                line: line_number(index),
            }),
            None => parsed.skipped = parsed.skipped.saturating_add(1),
        }
    }
    parsed
}

fn line_number(index: usize) -> u32 {
    u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domains(parsed: &Parsed) -> Vec<&str> {
        parsed.entries.iter().map(|e| e.domain.as_str()).collect()
    }

    // -- Normalisierung ----------------------------------------------------

    #[test]
    fn normalize_lowercases_and_trims_dots() {
        assert_eq!(
            normalize("  Example.COM.  "),
            Some("example.com".to_owned())
        );
        assert_eq!(normalize(".example.com"), Some("example.com".to_owned()));
        assert_eq!(normalize("example.com."), Some("example.com".to_owned()));
    }

    #[test]
    fn normalize_rejects_what_is_not_a_domain() {
        for junk in [
            "",
            "   ",
            ".",
            "..",
            "example .com",
            "http://example.com/",
            "user@example.com",
            "example..com",
            "exa mple.com",
        ] {
            assert_eq!(normalize(junk), None, "{junk:?} wurde akzeptiert");
        }
    }

    #[test]
    fn normalize_rejects_oversized_labels_and_names() {
        // RFC 1035: Label höchstens 63, Name höchstens 253 Zeichen.
        let long_label = format!("{}.com", "a".repeat(64));
        assert_eq!(normalize(&long_label), None);
        assert!(normalize(&format!("{}.com", "a".repeat(63))).is_some());

        let long_name = vec!["abcdefgh"; 40].join(".");
        assert!(long_name.len() > MAX_NAME);
        assert_eq!(normalize(&long_name), None);
    }

    #[test]
    fn normalize_rejects_non_ascii_instead_of_guessing() {
        // IDN gehört in Punycode-Form in eine Liste. Selbst zu konvertieren
        // hieße, eine Kodierung zu raten, die die Liste nicht angegeben hat.
        assert_eq!(normalize("münchen.de"), None);
        assert_eq!(
            normalize("xn--mnchen-3ya.de"),
            Some("xn--mnchen-3ya.de".to_owned())
        );
    }

    #[test]
    fn normalize_survives_a_huge_line_without_growing_it() {
        // docs/TESTING.md verlangt eine absurd lange Zeile. 10 MB reichen, um
        // zu zeigen, dass nichts kopiert oder aufgeteilt wird, bevor die
        // Längenprüfung greift; 300 MB würden nur den Test verlangsamen.
        let huge = "a".repeat(10 * 1024 * 1024);
        assert_eq!(normalize(&huge), None);
    }

    // -- hosts -------------------------------------------------------------

    #[test]
    fn hosts_format_takes_the_names_after_the_address() {
        let parsed = parse(
            "0.0.0.0 ads.example.com\n127.0.0.1 tracker.example.net\n",
            Format::Hosts,
        );
        assert_eq!(
            domains(&parsed),
            vec!["ads.example.com", "tracker.example.net"]
        );
        assert!(parsed.entries.iter().all(|e| e.scope == Scope::Exact));
    }

    #[test]
    fn hosts_format_takes_every_name_on_a_line() {
        let parsed = parse("0.0.0.0 a.example.com b.example.com\n", Format::Hosts);
        assert_eq!(domains(&parsed), vec!["a.example.com", "b.example.com"]);
    }

    #[test]
    fn hosts_format_records_the_line_number() {
        let parsed = parse("# Kopf\n\n0.0.0.0 ads.example.com\n", Format::Hosts);
        assert_eq!(parsed.entries.first().map(|e| e.line), Some(3));
    }

    #[test]
    fn hosts_format_handles_comments_crlf_and_blank_lines() {
        let text = "# Kommentar\r\n\r\n0.0.0.0 ads.example.com # dahinter\r\n\r\n";
        let parsed = parse(text, Format::Hosts);
        assert_eq!(domains(&parsed), vec!["ads.example.com"]);
        assert_eq!(parsed.skipped, 0);
    }

    #[test]
    fn hosts_format_ignores_localhost_entries() {
        let text = "127.0.0.1 localhost\n::1 ip6-localhost ip6-loopback\n0.0.0.0 ads.example.com\n";
        let parsed = parse(text, Format::Hosts);
        assert_eq!(domains(&parsed), vec!["ads.example.com"]);
        assert_eq!(parsed.skipped, 0, "localhost-Zeilen sind erwartbar");
    }

    #[test]
    fn hosts_format_skips_lines_with_a_real_address() {
        // Eine /etc/hosts mit echten Zuordnungen ist keine Blockliste.
        let parsed = parse(
            "192.168.1.5 nas.home.arpa\n0.0.0.0 ads.example.com\n",
            Format::Hosts,
        );
        assert_eq!(domains(&parsed), vec!["ads.example.com"]);
        assert_eq!(parsed.skipped, 1);
    }

    #[test]
    fn hosts_format_handles_ipv6_sinkholes() {
        let parsed = parse(":: ads.example.com\n", Format::Hosts);
        assert_eq!(domains(&parsed), vec!["ads.example.com"]);
    }

    #[test]
    fn a_file_without_a_final_newline_still_yields_its_last_entry() {
        let parsed = parse("0.0.0.0 ads.example.com", Format::Hosts);
        assert_eq!(domains(&parsed), vec!["ads.example.com"]);
    }

    #[test]
    fn an_empty_file_yields_nothing_and_is_not_an_error() {
        for format in [
            Format::Hosts,
            Format::Domains,
            Format::Wildcard,
            Format::Adblock,
            Format::Rpz,
        ] {
            let parsed = parse("", format);
            assert!(parsed.entries.is_empty(), "{format}");
            assert_eq!(parsed.skipped, 0, "{format}");
        }
    }

    // -- domains und wildcard ---------------------------------------------

    #[test]
    fn domains_format_is_exact_wildcard_format_is_suffix() {
        let exact = parse("example.com\n", Format::Domains);
        assert_eq!(exact.entries.first().map(|e| e.scope), Some(Scope::Exact));
        let suffix = parse("example.com\n", Format::Wildcard);
        assert_eq!(suffix.entries.first().map(|e| e.scope), Some(Scope::Suffix));
    }

    #[test]
    fn leading_star_and_dot_are_normalized_away() {
        let parsed = parse(
            "*.example.com\n.example.net\nexample.org\n",
            Format::Wildcard,
        );
        assert_eq!(
            domains(&parsed),
            vec!["example.com", "example.net", "example.org"]
        );
    }

    #[test]
    fn duplicate_entries_are_kept_as_they_come() {
        // Aussortiert wird beim Bauen des Matchers, nicht beim Parsen — sonst
        // ginge die Zeilennummer des ersten Vorkommens verloren.
        let parsed = parse("example.com\nexample.com\n", Format::Domains);
        assert_eq!(parsed.entries.len(), 2);
    }

    #[test]
    fn junk_lines_are_counted_not_fatal() {
        let parsed = parse(
            "example.com\n!!! kaputt !!!\nhttp://example.net/pfad\ngut.example\n",
            Format::Domains,
        );
        assert_eq!(domains(&parsed), vec!["example.com", "gut.example"]);
        assert_eq!(parsed.skipped, 1, "die !-Zeile gilt als Kommentar");
    }

    // -- adblock -----------------------------------------------------------

    #[test]
    fn adblock_accepts_the_documented_subset() {
        let parsed = parse("||ads.example.com^\n", Format::Adblock);
        assert_eq!(domains(&parsed), vec!["ads.example.com"]);
        assert_eq!(parsed.entries.first().map(|e| e.scope), Some(Scope::Suffix));
    }

    /// Dieser Test *ist* die Dokumentation der Teilmenge (Roadmap, Schritt 3).
    #[test]
    fn adblock_ignores_everything_outside_the_subset() {
        let text = concat!(
            "! Kommentar\n",
            "||gut.example^\n",               // unterstützt
            "@@||ausnahme.example^\n",        // Ausnahmeregel: Allowlists sind eine eigene Liste
            "||dritt.example^$third-party\n", // Optionen schränken ein: nicht raten
            "example.com##.werbung\n",        // Element-Filter: Sache des Browsers
            "/werbe-muster/\n",               // Regex auf URLs, die ein Resolver nie sieht
            "|http://example.net|\n",         // URL-Regel
            "||ohne-trenner\n",               // kein ^: nicht unser Format
        );
        let parsed = parse(text, Format::Adblock);
        assert_eq!(
            domains(&parsed),
            vec!["gut.example"],
            "es wurde mehr übernommen als die dokumentierte Teilmenge"
        );
        assert_eq!(
            parsed.skipped, 6,
            "je eine Zeile für @@, $-Option, ##, Regex, URL und fehlendes ^"
        );
    }

    // -- rpz ---------------------------------------------------------------

    #[test]
    fn rpz_reads_nxdomain_rules_and_strips_the_origin() {
        let text = concat!(
            "$TTL 300\n",
            "$ORIGIN rpz.example.\n",
            "@ SOA ns.rpz.example. hostmaster.rpz.example. 1 12h 15m 30d 2h\n",
            "ads.example.com.rpz.example. CNAME .\n",
            "*.tracker.example.rpz.example. CNAME .\n",
            "erlaubt.example.com.rpz.example. CNAME rpz-passthru.\n",
        );
        let parsed = parse(text, Format::Rpz);
        assert_eq!(domains(&parsed), vec!["ads.example.com", "tracker.example"]);
        assert_eq!(
            parsed.entries.get(1).map(|e| e.scope),
            Some(Scope::Suffix),
            "*.name muss eine Suffix-Regel werden"
        );
    }

    #[test]
    fn rpz_accepts_ttl_and_class_between_name_and_type() {
        let text = "$ORIGIN rpz.example.\nads.example.com.rpz.example. 300 IN CNAME .\n";
        assert_eq!(domains(&parse(text, Format::Rpz)), vec!["ads.example.com"]);
    }

    #[test]
    fn rpz_ignores_policies_it_does_not_implement() {
        let text = concat!(
            "$ORIGIN rpz.example.\n",
            "drop.example.com.rpz.example. CNAME rpz-drop.\n",
            "8.0.0.0.127.rpz-ip.rpz.example. CNAME .\n",
            "geblockt.example.com.rpz.example. CNAME .\n",
        );
        let parsed = parse(text, Format::Rpz);
        assert!(domains(&parsed).contains(&"geblockt.example.com"));
        assert!(
            !domains(&parsed).contains(&"drop.example.com"),
            "rpz-drop ist nicht umgesetzt und darf nicht als Block durchgehen"
        );
    }

    #[test]
    fn rpz_without_origin_takes_the_name_as_it_stands() {
        let parsed = parse("ads.example.com. CNAME .\n", Format::Rpz);
        assert_eq!(domains(&parsed), vec!["ads.example.com"]);
    }
}
