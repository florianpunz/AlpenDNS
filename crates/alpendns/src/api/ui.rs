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
const STYLE: &str = include_str!("../../../../web/app.css");
const SCRIPT: &str = include_str!("../../../../web/app.js");

pub fn router() -> Router<super::ApiState> {
    Router::new()
        .route("/", get(index))
        .route("/app.css", get(style))
        .route("/app.js", get(script))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Die Seite muss ohne Internet funktionieren (CLAUDE.md B.6). Ein Verweis
    /// auf eine fremde Herkunft wäre genau der Fehler, den niemand bemerkt,
    /// solange er selbst online ist.
    #[test]
    fn the_page_references_nothing_from_outside() {
        for (name, content) in [
            ("index.html", INDEX),
            ("app.css", STYLE),
            ("app.js", SCRIPT),
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
        // Die Vorgabe aus CLAUDE.md B.6.
        for heading in [
            "Läuft er?",
            "Warum wurde das geblockt?",
            "Was gerade passiert",
        ] {
            assert!(INDEX.contains(heading), "Überschrift fehlt: {heading}");
        }
    }

    #[test]
    fn the_palette_is_semantic_and_exists_in_both_schemes() {
        // Vier Bedeutungen plus die Marke, mehr Farbe gibt es nicht
        // (CLAUDE.md B.6). Je eine Festlegung für hell und dunkel — eine Farbe,
        // die nur in einem Modus existiert, fehlt im anderen.
        for token in ["--danger:", "--success:", "--warn:", "--muted:", "--brand:"] {
            assert_eq!(
                STYLE.matches(token).count(),
                2,
                "{token} ist nicht genau einmal hell und einmal dunkel festgelegt"
            );
        }
        // Ein weiterer Ton wäre wieder Dekoration.
        for forbidden in ["--accent:", "--info:", "--primary:"] {
            assert!(
                !STYLE.contains(forbidden),
                "{forbidden} ist eine Farbe ohne Bedeutung"
            );
        }
    }

    #[test]
    fn colour_only_appears_where_it_carries_meaning() {
        // Jede Regel, die --danger, --success oder --warn benutzt, muss zu einer
        // der vier Bedeutungen gehören. Sonst markiert die Farbe irgendwann
        // alles. (--muted ist die neutrale Textfarbe und steht überall.)
        for colour in ["var(--danger)", "var(--success)", "var(--warn)"] {
            for (index, _) in STYLE.match_indices(colour) {
                let selector = STYLE
                    .get(..index)
                    .and_then(|before| before.rfind('}').map(|end| end + 1))
                    .and_then(|start| STYLE.get(start..index))
                    .unwrap_or_default();
                assert!(
                    [
                        "is-blocked",
                        ".subject",
                        ".error",
                        ".ms-",
                        ".up-dot",
                        ".dot",
                        ".line-blocked", // geblockt, als Kurve
                        ".line-cache",   // Cache-Treffer, als Kurve
                        ".sw-blocked",   // dieselbe Bedeutung in der Legende
                        ".sw-cache",
                        ".is-persisting", // schreibt auf Platte
                    ]
                    .iter()
                    .any(|allowed| selector.contains(allowed)),
                    "{colour} außerhalb einer Bedeutung:{selector}"
                );
            }
        }
    }

    #[test]
    fn the_stylesheet_uses_a_scale_instead_of_ad_hoc_values() {
        // Typografie und Abstände liegen als Variablen fest (CLAUDE.md B.6).
        for token in [
            "--t-micro:",
            "--t-small:",
            "--t-body:",
            "--t-figure:",
            "--t-lead:",
        ] {
            assert!(STYLE.contains(token), "Skala unvollständig: {token}");
        }
        // Abstände auf 8er-Basis; --s-4 ist die einzige halbe Stufe.
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
    fn the_four_key_figures_are_cards() {
        // Genau vier Kennzahlen, alle mit derselben Behandlung: 1px Rahmen,
        // 12px Radius, abgesetzte Fläche.
        assert_eq!(
            INDEX.matches("class=\"card\"").count(),
            4,
            "es sind nicht vier Kennzahlenkarten"
        );
        assert!(STYLE.contains("--radius: 12px"), "kein 12px-Radius");
        assert!(
            STYLE.contains("border: 1px solid var(--line)"),
            "Karten ohne Haarlinie"
        );
    }

    #[test]
    fn the_sparkline_is_inline_svg_without_a_library() {
        // Eine Linie, kein Diagrammpaket: die Seite lädt nichts nach.
        assert!(INDEX.contains("<svg class=\"spark\""), "keine Sparkline");
        assert!(
            INDEX.contains("vector-effect=\"non-scaling-stroke\""),
            "die Linie würde beim Strecken mitwachsen"
        );
        // Der Pfad wird gesetzt, nicht erzeugt — kein createElementNS, kein
        // Namensraum-URI, der die Prüfung auf fremde Herkunft aufweichen würde.
        assert!(
            SCRIPT.contains("path.setAttribute(\"d\""),
            "die Sparkline wird nicht über das vorhandene <path> gezeichnet"
        );
        assert!(!SCRIPT.contains("createElementNS"));
    }

    #[test]
    fn empty_areas_explain_themselves() {
        // Kein leeres Rechteck: Zeichen plus ein Satz, warum hier nichts steht.
        assert_eq!(
            INDEX.matches("class=\"empty\"").count(),
            5,
            "nicht jede leere Fläche erklärt sich"
        );
        assert_eq!(INDEX.matches("class=\"empty-icon\"").count(), 5);
    }

    #[test]
    fn the_answer_badge_says_what_it_means_without_colour() {
        // Farbe wiederholt den Text, sie ersetzt ihn nicht — sonst ist die
        // Tabelle für Farbenblinde unlesbar.
        assert!(SCRIPT.contains("badge.textContent = \"BLOCKED\""));
        assert!(
            SCRIPT.contains("badge.title = event.rcode"),
            "RCODE geht verloren"
        );
    }

    #[test]
    fn latency_thresholds_are_shared_between_upstreams_and_log() {
        // Dieselbe Farbe darf an zwei Stellen nicht zweierlei heißen.
        assert!(SCRIPT.contains("const MS_FAST = 20;"));
        assert!(SCRIPT.contains("const MS_SLOW = 100;"));
        assert_eq!(
            SCRIPT.matches("latencyClass(").count(),
            3,
            "zweite Schwelle"
        );
    }

    #[test]
    fn nothing_decorative_crept_in() {
        // Die Verbote aus CLAUDE.md B.6, als Test statt als Vorsatz.
        for forbidden in [
            "linear-gradient",
            "radial-gradient",
            "backdrop-filter",
            "@import",
            "@font-face",
        ] {
            assert!(!STYLE.contains(forbidden), "verboten laut B.6: {forbidden}");
        }
    }

    #[test]
    fn domain_names_use_a_system_monospace_stack() {
        assert!(STYLE.contains("ui-monospace"), "kein System-Monospace");
        assert!(
            STYLE.contains("td.c-name {") && STYLE.contains("font-family: var(--mono)"),
            "Namen im Protokoll stehen nicht in Monospace"
        );
    }

    #[test]
    fn motion_is_reduced_on_request() {
        assert!(
            STYLE.contains("@media (prefers-color-scheme: dark)"),
            "kein dunkles Schema"
        );
        assert!(
            STYLE.contains("@media (prefers-reduced-motion: reduce)"),
            "prefers-reduced-motion wird nicht beachtet"
        );
        // Zwei dezente Übergänge — Eingabefelder und Protokollzeilen — plus
        // die Regel, die beide wieder abschaltet.
        assert!(
            STYLE.matches("transition:").count() <= 3,
            "mehr Übergänge als die zwei erlaubten Regeln"
        );
    }

    #[test]
    fn the_page_does_not_scroll_the_log_does() {
        // Die Leitidee: Statuszeile fest, Protokoll rollt. Nur so stehen die
        // drei Fragen gleichzeitig da.
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
    }

    /// Der aktive Log-Modus steht dauerhaft auf der Seite (ADR-0004).
    ///
    /// Drei Angaben, nicht eine: der Modus allein sagt nichts darüber, ab wann
    /// ein Name genannt wird und ob etwas auf der Platte landet.
    #[test]
    fn the_privacy_strip_states_mode_threshold_and_storage() {
        for id in ["id=\"p-mode\"", "id=\"p-k\"", "id=\"p-store\""] {
            assert!(INDEX.contains(id), "im Privacy-Streifen fehlt {id}");
        }
        assert!(
            SCRIPT.contains("status.aggregate_k"),
            "die k-Schwelle wird nicht angezeigt"
        );
        // "Daten nur im RAM" darf keine Behauptung sein, sondern muss aus dem
        // laufenden Prozess kommen — sonst steht sie auch im Modus full da.
        assert!(
            SCRIPT.contains("status.persists_to_disk"),
            "die Aussage über die Platte kommt nicht vom Server"
        );
        assert!(SCRIPT.contains("Daten nur im RAM"));
        assert!(SCRIPT.contains("schreibt auf Platte"));
    }

    /// Der Live-Strom ist als flüchtig gekennzeichnet.
    #[test]
    fn the_live_stream_says_that_it_keeps_nothing() {
        assert!(
            INDEX.contains("flüchtig"),
            "keine Kennzeichnung am Protokoll"
        );
        assert!(
            SCRIPT.contains("flüchtig"),
            "die Kennzeichnung folgt nicht dem Modus"
        );
    }

    /// Jede Kurve dieser Seite kommt aus Zählern.
    ///
    /// Der Test fixiert die Quelle, nicht das Aussehen: die Zeitreihe wird von
    /// `/api/history` geholt, und dieser Endpunkt liefert per Konstruktion nur
    /// Summen. Ein Diagramm, das aus `/api/recent` gezeichnet würde, wäre eine
    /// Namensauswertung mit anderem Anstrich.
    #[test]
    fn the_time_series_is_drawn_from_counters_only() {
        assert!(SCRIPT.contains("api(\"/api/history\")"), "keine Zeitreihe");
        for field in ["bucket.queries", "bucket.blocked", "bucket.cache_hits"] {
            assert!(SCRIPT.contains(field), "die Kurve benutzt {field} nicht");
        }
        // Alle Pfade laufen über setPath; kein Diagrammpaket, kein Namensraum.
        assert!(SCRIPT.contains("path.setAttribute(\"d\""));
        assert!(!SCRIPT.contains("createElementNS"));
        for id in ["id=\"day-queries\"", "id=\"day-cache\"", "id=\"split-1\""] {
            assert!(INDEX.contains(id), "das <path> für {id} fehlt im HTML");
        }
    }

    /// Die Aufteilung auf die Upstreams wird über Graustufen unterschieden.
    ///
    /// Vier Upstreams mit vier Farben wären vier Bedeutungen, die es laut B.6
    /// nicht gibt. Die Bänder trennen nur benachbarte Flächen; welches Band zu
    /// welchem Resolver gehört, sagt die Legende.
    #[test]
    fn stacked_areas_use_a_neutral_ramp_instead_of_colour() {
        for band in ["--band-1:", "--band-2:", "--band-3:", "--band-4:"] {
            assert_eq!(
                STYLE.matches(band).count(),
                2,
                "{band} fehlt in einem der beiden Schemata"
            );
        }
        for colour in ["var(--danger)", "var(--success)", "var(--warn)"] {
            assert!(
                !STYLE.contains(&format!(".band-1 {{ fill: {colour}")),
                "ein Band trägt eine Bedeutung, die es nicht hat"
            );
        }
    }

    /// Zahlen stehen im deutschen Format.
    #[test]
    fn numbers_are_formatted_in_german() {
        assert!(
            SCRIPT.contains("const LOCALE = \"de-AT\";"),
            "kein festgelegtes Zahlenformat"
        );
        // Jede Zahl, die jemand liest, geht durch die drei Hilfsfunktionen.
        // `toFixed` erzeugt einen Dezimalpunkt und keine Tausendertrennung —
        // in einer Koordinate ist das richtig, in einer Anzeige falsch.
        for line in SCRIPT.lines().filter(|line| line.contains("toFixed")) {
            assert!(
                !line.contains("textContent"),
                "eine angezeigte Zahl geht an der Formatierung vorbei:\n{line}"
            );
        }
        for helper in ["const thousands =", "const percent =", "const decimal ="] {
            assert!(SCRIPT.contains(helper), "{helper} fehlt");
        }
    }

    /// Ein Klick auf eine Zeile fragt dieselbe Auswertung wie die
    /// Kommandozeile.
    #[test]
    fn a_log_row_can_ask_for_its_decision_chain() {
        assert!(
            SCRIPT.contains("/api/explain?"),
            "kein Aufruf der Auswertung"
        );
        assert!(
            SCRIPT.contains("tr.classList.add(\"askable\")"),
            "die Zeile zeigt nicht, dass sie anklickbar ist"
        );
        // Ohne Namen gibt es nichts zu erklären; dann darf die Zeile auch nicht
        // so aussehen, als ließe sich etwas anklicken.
        assert!(
            SCRIPT.contains("if (event.name) {"),
            "auch namenlose Zeilen wären anklickbar"
        );
        assert!(
            STYLE.contains(".log-table tr.askable:focus-visible"),
            "die Zeile ist mit der Tastatur nicht erreichbar"
        );
    }

    #[test]
    fn the_logo_is_inline_and_needs_no_file() {
        // Eine Grafik als Datei wäre eine weitere Route und eine weitere
        // Möglichkeit, dass die Seite ohne Netz halb geladen aussieht.
        assert!(INDEX.contains("<svg"), "kein eingebettetes Logo");
        assert!(!INDEX.contains("<img"), "die Seite lädt eine Bilddatei");
    }

    #[test]
    fn the_hidden_attribute_is_not_overridden_by_a_display_rule() {
        // `.login { display: flex }` hat das hidden-Attribut geschlagen, und
        // das Anmeldefenster blieb nach dem Login stehen.
        assert!(
            STYLE.contains("[hidden] { display: none !important; }"),
            "ohne diese Regel kann ein display: … das hidden-Attribut aushebeln"
        );
    }

    #[test]
    fn numbers_are_tabular() {
        assert!(STYLE.contains("font-variant-numeric: tabular-nums"));
    }

    #[test]
    fn the_script_never_writes_foreign_data_as_markup() {
        // textContent statt innerHTML: ein Domainname aus dem Netz darf keine
        // Auszeichnung werden.
        assert!(!SCRIPT.contains("innerHTML"), "innerHTML im Skript");
        assert!(!SCRIPT.contains("outerHTML"));
        assert!(!SCRIPT.contains("insertAdjacentHTML"));
    }
}
