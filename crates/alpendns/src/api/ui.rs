//! Auslieferung der Web-UI.
//!
//! Die Dateien liegen im Binary (`include_str!`). Kein Build-Schritt, kein
//! Verzeichnis, das zur Laufzeit da sein muss, und keine Möglichkeit, dass die
//! UI zu einer anderen Version gehört als der Server.
//!
//! Kein Token: die Seite ist statisch und enthält keine Daten. Sie fragt den
//! Token beim ersten Aufruf ab und schickt ihn dann selbst mit.

use axum::Router;
use axum::http::header;
use axum::response::{IntoResponse as _, Response};
use axum::routing::get;

const INDEX: &str = include_str!("../../../../web/index.html");
pub(super) const STYLE: &str = include_str!("../../../../web/app.css");
const SCRIPT: &str = include_str!("../../../../web/app.js");
/// Der Theme-Block, der vor dem ersten Zeichnen laufen muss. Eigene Datei statt
/// eines Inline-Blocks, damit die CSP kein `'unsafe-inline'` und keinen
/// Hash-Nachlauf braucht.
const THEME: &str = include_str!("../../../../web/theme.js");

pub fn router() -> Router<super::ApiState> {
    Router::new()
        .route("/", get(index))
        .route("/app.css", get(style))
        .route("/app.js", get(script))
        .route("/theme.js", get(theme))
}

fn serve(content_type: &'static str, body: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            // Die UI gehört zur Binärversion; ein alter Stand im Browser wäre
            // schlimmer als ein Abruf mehr.
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

async fn index() -> Response {
    serve("text/html; charset=utf-8", INDEX)
}

async fn style() -> Response {
    serve("text/css; charset=utf-8", STYLE)
}

async fn script() -> Response {
    serve("text/javascript; charset=utf-8", SCRIPT)
}

async fn theme() -> Response {
    serve("text/javascript; charset=utf-8", THEME)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(selector: &str) -> String {
        let needle = format!("\n{selector} {{");
        let (_, rest) = STYLE.split_once(&needle).expect("Regel fehlt");
        let (body, _) = rest.split_once('}').expect("Regel ohne Ende");
        body.to_owned()
    }

    fn selectors_using(needle: &str) -> Vec<String> {
        let mut selectors = Vec::new();
        for (index, _) in STYLE.match_indices(needle) {
            let before = STYLE.get(..index).unwrap_or_default();
            let Some(open) = before.rfind('{') else {
                continue;
            };
            let Some(prefix) = before.get(..open) else {
                continue;
            };
            let start = prefix.rfind('}').map_or(0, |end| end + 1);
            selectors.push(prefix.get(start..).unwrap_or_default().trim().to_owned());
        }
        selectors
    }

    #[test]
    fn the_page_references_nothing_from_outside() {
        for (name, content) in [
            ("index.html", INDEX),
            ("app.css", STYLE),
            ("app.js", SCRIPT),
            ("theme.js", THEME),
        ] {
            for needle in [
                "http://",
                "https://",
                "//cdn",
                "fonts.googleapis",
                "unpkg",
                "jsdelivr",
                "@import url(",
            ] {
                assert!(!content.contains(needle), "{name} verweist auf '{needle}'");
            }
        }
    }

    #[test]
    fn the_page_answers_the_three_questions_without_a_click() {
        assert!(INDEX.contains("id=\"health\""), "kein Verdikt");
        assert!(INDEX.contains("Live queries"), "kein Protokoll");
        assert!(INDEX.contains("id=\"inspector\""), "kein Inspector");
        assert!(
            SCRIPT.contains("close"),
            "der Inspector lässt sich nicht schließen"
        );
    }

    #[test]
    fn the_palette_has_one_accent_and_four_meanings() {
        assert_eq!(
            STYLE.matches("--accent:").count(),
            2,
            "Akzent fehlt in einem Modus"
        );
        for token in ["--danger:", "--warn:", "--muted:", "--success:"] {
            assert_eq!(
                STYLE.matches(token).count(),
                2,
                "{token} steht nicht genau einmal hell und einmal dunkel"
            );
        }
        assert!(!STYLE.contains("--brand:"), "eine zweite Markenfarbe");
        assert!(!STYLE.contains("--info:"), "eine Farbe ohne Bedeutung");
    }

    #[test]
    fn colour_only_appears_where_it_carries_meaning() {
        let allowed = [
            (
                "var(--accent)",
                &[
                    "a",
                    ".wordmark",
                    ".logo",
                    ":focus-visible",
                    ".name-button",
                    ".link",
                    ".is-key",
                    ".primary",
                ][..],
            ),
            (
                "var(--danger)",
                &[
                    ".line-blocked",
                    ".sw-blocked",
                    ".is-blocked",
                    ".result.is-blocked",
                    ".subject",
                    ".verdict-badge",
                ][..],
            ),
            (
                "var(--warn)",
                &[".result.is-error", ".is-warn", ".error"][..],
            ),
            (
                "var(--success)",
                &[".dot.is-live", ".stream-flag", ".up-dot"][..],
            ),
        ];

        for (colour, selectors) in allowed {
            for selector in selectors_using(colour) {
                assert!(
                    selectors
                        .iter()
                        .any(|allowed| selector == *allowed || selector.contains(allowed)),
                    "{colour} außerhalb einer Bedeutung: {selector}"
                );
            }
        }
    }

    #[test]
    fn glass_is_reserved_for_floating_surfaces() {
        let mut selectors = selectors_using("backdrop-filter: var(--blur)");
        selectors.sort();
        assert_eq!(
            selectors,
            vec![".commandbar", ".sheet", ".topbar"],
            "Glas liegt auf einer Fläche, die nicht schwebt"
        );
    }

    #[test]
    fn the_log_head_is_opaque_instead_of_blurred() {
        let head = rule(".log-table th");
        assert!(head.contains("position: sticky"), "der Kopf klebt nicht");
        assert!(
            !head.contains("backdrop-filter"),
            "über dem neu zeichnenden Log wird geblurrt"
        );
    }

    #[test]
    fn the_stylesheet_uses_a_scale_instead_of_ad_hoc_values() {
        for token in [
            "--t-micro:",
            "--t-small:",
            "--t-body:",
            "--t-figure:",
            "--t-lead:",
        ] {
            assert!(STYLE.contains(token), "Skala unvollständig: {token}");
        }
        for token in [
            "--s-4:", "--s-8:", "--s-16:", "--s-24:", "--s-32:", "--s-48:",
        ] {
            assert!(STYLE.contains(token), "Abstandsstufe fehlt: {token}");
        }
    }

    #[test]
    fn the_content_sits_in_one_centred_container() {
        assert!(STYLE.contains("--page: 1400px"), "keine Maximalbreite");
        assert!(STYLE.contains("margin-inline: auto"), "nicht zentriert");
    }

    #[test]
    fn the_four_figures_share_one_strip() {
        assert_eq!(
            INDEX.matches("class=\"metric\"").count(),
            4,
            "es sind nicht vier Kennzahlen"
        );
        let strip = rule(".metrics");
        assert!(
            strip.contains("repeat(4"),
            "die Kennzahlen stehen nicht in einer Leiste"
        );
        assert!(
            !strip.contains("backdrop-filter"),
            "die Kennzahlenleiste ist ein schwebendes Element geworden"
        );
        assert!(!rule(".metric").contains("backdrop-filter"));
    }

    #[test]
    fn the_ground_is_flat_and_carries_contours() {
        assert!(STYLE.contains("--ground:"), "kein Grund");
        assert_eq!(
            STYLE.matches("--contour:").count(),
            2,
            "Kontur nur in einem Modus"
        );
        assert!(INDEX.contains("class=\"contours\""), "keine Höhenlinien");
        assert!(!STYLE.contains("gradient"), "ein Verlauf ohne Bedeutung");
    }

    #[test]
    fn the_log_table_has_six_columns_and_a_cap() {
        for column in ["c-time", "c-name", "c-type", "c-client", "c-result", "c-ms"] {
            assert!(
                INDEX.contains(&format!("th class=\"{column}\"")),
                "Spalte {column} fehlt"
            );
        }
        assert!(SCRIPT.contains("const MAX_ROWS = 200;"), "keine DOM-Grenze");
        assert!(
            SCRIPT.contains("while (entries.length > MAX_ROWS)"),
            "die Grenze wird nicht angewandt"
        );
    }

    #[test]
    fn the_filters_offer_all_blocked_and_errors() {
        for filter in ["all", "blocked", "errors"] {
            assert!(
                INDEX.contains(&format!("data-filter=\"{filter}\"")),
                "Filter {filter} fehlt"
            );
        }
        assert!(
            SCRIPT.contains("aria-pressed"),
            "der Filterzustand ist nicht angekündigt"
        );
        assert!(
            SCRIPT.contains("ERROR_RCODES"),
            "Fehler werden nicht als solche erkannt"
        );
    }

    #[test]
    fn identical_queries_are_collapsed() {
        assert!(SCRIPT.contains("function sameQuery"), "keine Gruppierung");
        assert!(
            SCRIPT.contains("×${entry.count}"),
            "die Anzahl steht nicht an der Zeile"
        );
    }

    #[test]
    fn the_answer_is_named_without_colour_and_cache_is_not_zero_milliseconds() {
        assert!(SCRIPT.contains("result.textContent = \"BLOCKED\""));
        assert!(
            SCRIPT.contains("cell.textContent = \"cache\""),
            "ein Cache-Treffer zeigt wieder eine Zahl"
        );
        assert!(
            SCRIPT.contains("cell.textContent = \"local\""),
            "eine lokale Antwort zeigt wieder eine Zahl"
        );
        assert!(STYLE.contains(".result.is-blocked {"));
        assert!(!STYLE.contains(".badge"), "Pills als Standardcontainer");
    }

    #[test]
    fn latency_thresholds_are_shared_between_upstreams_and_log() {
        assert!(SCRIPT.contains("const MS_WARN = 250;"));
        assert_eq!(
            SCRIPT.matches("MS_WARN").count(),
            3,
            "die Schwelle steht an mehr oder weniger als zwei Stellen"
        );
    }

    #[test]
    fn the_text_stays_readable_on_every_surface() {
        let (light, dark) = STYLE
            .split_once(":root[data-theme=\"dark\"]")
            .expect("kein dunkles Schema");

        let mut worst = f64::MAX;
        for (mode, css) in [("hell", light), ("dunkel", dark)] {
            let ground = parse_color(&token(css, "--ground:")).0;
            let surface = parse_color(&token(css, "--surface:")).0;
            let glass = over(parse_color(&token(css, "--glass:")), ground);

            for text in [
                "--text:",
                "--faint:",
                "--muted:",
                "--danger:",
                "--success:",
                "--warn:",
                "--accent:",
            ] {
                let colour = parse_color(&token(css, text)).0;
                let name = text.trim_end_matches(':');

                for (field, background) in [("Grund", ground), ("Fläche", surface), ("Glas", glass)]
                {
                    let ratio = contrast(colour, background);
                    worst = worst.min(ratio);
                    assert!(ratio >= 4.5, "{mode}: {name} auf {field} nur {ratio:.2}:1");
                }
            }
        }
        assert!(worst >= 4.5, "schlechtester Kontrast {worst:.2}:1");
    }

    #[derive(Clone, Copy)]
    struct Rgb(u8, u8, u8);

    fn parse_color(raw: &str) -> (Rgb, f64) {
        if let Some(hex) = raw.strip_prefix('#') {
            let byte = |at: usize| {
                u8::from_str_radix(hex.get(at..at + 2).expect("Hexziffern"), 16).expect("Hex")
            };
            return (Rgb(byte(0), byte(2), byte(4)), 1.0);
        }
        let inner = raw
            .strip_prefix("rgba(")
            .and_then(|rest| rest.strip_suffix(')'))
            .expect("weder Hex noch rgba()");
        let parts: Vec<f64> = inner
            .split(',')
            .map(|part| part.trim().parse().expect("Zahl"))
            .collect();
        let at = |index: usize| parts.get(index).copied().expect("vier Werte") as u8;
        (
            Rgb(at(0), at(1), at(2)),
            parts.get(3).copied().expect("Alpha"),
        )
    }

    fn token(css: &str, name: &str) -> String {
        let rest = css
            .split_once(name)
            .map(|(_, rest)| rest)
            .expect("Token fehlt");
        rest.split_once(';')
            .map(|(value, _)| value.trim().to_owned())
            .expect("kein Semikolon")
    }

    fn over((top, alpha): (Rgb, f64), under: Rgb) -> Rgb {
        let mix = |a: u8, b: u8| {
            let value = alpha.mul_add(f64::from(a), (1.0 - alpha) * f64::from(b));
            value.round().clamp(0.0, 255.0) as u8
        };
        Rgb(
            mix(top.0, under.0),
            mix(top.1, under.1),
            mix(top.2, under.2),
        )
    }

    fn luminance(colour: Rgb) -> f64 {
        let channel = |value: u8| {
            let value = f64::from(value) / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(colour.0) + 0.7152 * channel(colour.1) + 0.0722 * channel(colour.2)
    }

    fn contrast(one: Rgb, other: Rgb) -> f64 {
        let (a, b) = (luminance(one), luminance(other));
        let (high, low) = if a > b { (a, b) } else { (b, a) };
        (high + 0.05) / (low + 0.05)
    }

    #[test]
    fn domain_names_use_a_system_monospace_stack() {
        assert!(STYLE.contains("ui-monospace"), "kein System-Monospace");
        assert!(
            rule(".name-button").contains("font-family: var(--mono)"),
            "Namen im Protokoll stehen nicht in Monospace"
        );
        assert!(rule(".sheet .subject").contains("font-family: var(--mono)"));
    }

    #[test]
    fn motion_is_reduced_on_request() {
        assert!(STYLE.contains("@media (prefers-reduced-motion: reduce)"));
        assert!(
            STYLE.matches("transition:").count() <= 6,
            "mehr Übergänge als die zwei erlaubten Regeln"
        );
        assert!(STYLE.contains("@media (prefers-reduced-transparency: reduce)"));
    }

    #[test]
    fn the_page_does_not_scroll_the_log_does() {
        assert!(
            STYLE.contains("height: 100dvh"),
            "die Seite füllt keinen Bildschirm"
        );
        assert!(
            STYLE.contains("overflow-y: scroll"),
            "das Protokoll rollt nicht"
        );
        assert!(
            STYLE.contains("scrollbar-gutter: stable"),
            "ohne reservierte Bahn springt das Layout beim ersten Überlauf"
        );
        assert!(
            rule(".side").contains("overflow-y: auto"),
            "die Seitenspalte rollt nicht"
        );
    }

    #[test]
    fn the_mode_stands_in_the_status_line() {
        assert!(
            INDEX.contains("id=\"status-detail\""),
            "kein Status im Kopf"
        );
        assert!(
            SCRIPT.contains("logging mode ${next.logging_mode}"),
            "der Modus kommt nicht vom Server"
        );
    }

    #[test]
    fn a_dropped_answer_is_visible_and_can_be_reviewed() {
        assert!(INDEX.contains("id=\"pc-bogus\""), "kein Zähler dafür");
        assert!(
            INDEX.contains("id=\"bogus-review\""),
            "kein Weg zu den Namen"
        );
        assert!(SCRIPT.contains("next.dnssec.bogus"));
        assert!(SCRIPT.contains("setFilter(\"errors\")"));
    }

    #[test]
    fn the_live_stream_says_what_it_keeps() {
        assert!(
            INDEX.contains("id=\"stream-flag\""),
            "keine Kennzeichnung am Protokoll"
        );
        assert!(
            SCRIPT.contains("setStreamState"),
            "der Zustand folgt nicht dem Strom"
        );
        assert!(
            SCRIPT.contains("keeps no names"),
            "der leise Modus wird nicht erklärt"
        );
    }

    #[test]
    fn the_time_series_is_drawn_from_counters_only() {
        assert!(SCRIPT.contains("api(\"/api/history\")"), "keine Zeitreihe");
        assert!(SCRIPT.contains("item.queries"), "die Kurve zählt nicht");
        assert!(
            SCRIPT.contains("item.blocked"),
            "die Blockaden fehlen in der Kurve"
        );
        assert!(SCRIPT.contains("setAttribute(\"d\""));
        assert!(!SCRIPT.contains("createElementNS"));
        for id in ["id=\"day-queries\"", "id=\"day-blocked\""] {
            assert!(INDEX.contains(id), "das <path> für {id} fehlt im HTML");
        }
    }

    #[test]
    fn upstream_rows_show_status_share_and_percentiles() {
        assert!(SCRIPT.contains("up-dot"), "kein Status");
        assert!(SCRIPT.contains("up-bar"), "kein Anteil");
        assert!(
            SCRIPT.contains("computeShares"),
            "der Anteil wird nicht gerechnet"
        );
        assert!(SCRIPT.contains("upstream.p50_ms"), "kein Median");
        assert!(SCRIPT.contains("upstream.p95_ms"), "kein p95");
    }

    #[test]
    fn numbers_are_formatted_in_english() {
        assert!(SCRIPT.contains("const LOCALE = \"en-US\";"));
        for helper in ["const thousands =", "const percent =", "const decimal ="] {
            assert!(SCRIPT.contains(helper), "{helper} fehlt");
        }
        for line in SCRIPT.lines().filter(|line| line.contains("toFixed")) {
            assert!(
                !line.contains("textContent"),
                "eine angezeigte Zahl geht an der Formatierung vorbei:\n{line}"
            );
        }
    }

    #[test]
    fn a_log_row_can_ask_for_its_decision_chain() {
        assert!(SCRIPT.contains("/api/explain?"));
        assert!(SCRIPT.contains("name-button"), "die Domain ist kein Knopf");
        assert!(
            SCRIPT.contains("if (event.name) {"),
            "auch namenlose Zeilen wären anklickbar"
        );
        assert!(STYLE.contains(":focus-visible"), "kein sichtbarer Fokus");
    }

    #[test]
    fn the_default_client_is_not_sent_to_explain() {
        assert!(
            SCRIPT.contains("client !== \"default\""),
            "der Pseudo-Client \"default\" wird an /api/explain geschickt"
        );
    }

    #[test]
    fn incoming_events_are_drawn_once_per_frame() {
        assert!(SCRIPT.contains("requestAnimationFrame(flushRows)"));
        assert!(
            !SCRIPT.contains("offsetHeight"),
            "eine Höhe wird je Zeile gemessen und erzwingt Layout"
        );
    }

    #[test]
    fn a_stale_evaluation_does_not_replace_a_newer_one() {
        assert_eq!(
            SCRIPT.matches("sequence !== inspectSeq").count(),
            2,
            "eine späte Antwort überschreibt den zuletzt gewählten Namen"
        );
    }

    #[test]
    fn the_log_uses_one_listener_instead_of_two_per_row() {
        assert!(SCRIPT.contains("$(\"log\").addEventListener(\"click\""));
        assert!(SCRIPT.contains("closest(\".name-button\")"));
        assert!(
            !SCRIPT.contains("tr.addEventListener"),
            "jede Zeile bringt Zuhörer mit"
        );
    }

    #[test]
    fn a_hidden_page_stops_drawing_and_polling() {
        assert!(SCRIPT.contains("if (document.hidden) return;"));
        assert!(SCRIPT.contains("visibilitychange"));
        assert!(
            SCRIPT.contains("event.skipped ?? 0"),
            "der Puls verliert die ausgelassenen Anfragen"
        );
    }

    #[test]
    fn the_command_bar_opens_with_slash() {
        assert!(INDEX.contains("id=\"command\""), "keine Command-Bar");
        assert!(SCRIPT.contains("event.key !== \"/\""));
        assert!(SCRIPT.contains("showModal"));
    }

    #[test]
    fn clicking_beside_the_command_bar_closes_it() {
        assert!(
            SCRIPT.contains("event.target === dialog"),
            "ein Klick neben die Command-Bar schließt sie nicht"
        );
        assert!(SCRIPT.contains("dialog.close()"));
    }

    #[test]
    fn the_inspector_shows_a_vertical_pipeline() {
        assert!(INDEX.contains("class=\"pipeline\""));
        for part in ["describeStep", "step-label", "step-detail", "is-key"] {
            assert!(SCRIPT.contains(part), "{part} fehlt in der Pipeline");
        }
        assert!(
            SCRIPT.contains("Evaluated now against the current lists"),
            "die Herkunft der Auswertung fehlt"
        );
        assert!(STYLE.contains(".pipeline li::before"), "keine Stationen");
    }

    #[test]
    fn the_footer_carries_the_list_count_and_the_mode() {
        for id in [
            "id=\"foot-lists\"",
            "id=\"foot-mode\"",
            "id=\"foot-version\"",
        ] {
            assert!(INDEX.contains(id), "{id} fehlt");
        }
        assert!(SCRIPT.contains("blocklist entries"));
        assert!(SCRIPT.contains("persists_to_disk"));
    }

    #[test]
    fn every_empty_area_explains_itself() {
        assert_eq!(
            INDEX.matches("class=\"empty\"").count(),
            4,
            "nicht jede leere Fläche erklärt sich"
        );
        assert_eq!(INDEX.matches("class=\"empty-icon\"").count(), 4);
    }

    #[test]
    fn the_logo_is_inline_and_needs_no_file() {
        assert!(INDEX.contains("<svg"), "kein eingebettetes Logo");
        assert!(!INDEX.contains("<img"), "die Seite lädt eine Bilddatei");
    }

    #[test]
    fn the_hidden_attribute_is_not_overridden_by_a_display_rule() {
        assert!(rule("[hidden]").contains("display: none !important"));
    }

    #[test]
    fn numbers_are_tabular() {
        assert!(STYLE.contains("font-variant-numeric: tabular-nums"));
    }

    #[test]
    fn the_script_never_writes_foreign_data_as_markup() {
        assert!(!SCRIPT.contains("innerHTML"), "innerHTML im Skript");
        assert!(!SCRIPT.contains("outerHTML"));
        assert!(!SCRIPT.contains("insertAdjacentHTML"));
    }
}
