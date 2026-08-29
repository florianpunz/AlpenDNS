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
    fn there_is_exactly_one_accent_and_it_exists_in_both_schemes() {
        // Ein Akzentton, sonst neutrale Graustufen (CLAUDE.md B.6). Je eine
        // Festlegung für hell und dunkel — eine Farbe, die nur in einem Modus
        // existiert, fehlt im anderen.
        assert_eq!(STYLE.matches("--accent:").count(), 2);
        for forbidden in ["--brand:", "--ok:", "--warn:", "--info:"] {
            assert!(
                !STYLE.contains(forbidden),
                "{forbidden} ist ein zweiter Akzent"
            );
        }
    }

    #[test]
    fn the_accent_marks_only_blocked_things() {
        // Jede Regel, die var(--accent) benutzt, muss zu Geblocktem gehören
        // oder zu einer Fehlermeldung. Sonst markiert die Farbe irgendwann alles.
        for (index, _) in STYLE.match_indices("var(--accent)") {
            let selector = STYLE
                .get(..index)
                .and_then(|before| before.rfind('}').map(|end| end + 1))
                .and_then(|start| STYLE.get(start..index))
                .unwrap_or_default();
            assert!(
                selector.contains("is-blocked")
                    || selector.contains(".subject")
                    || selector.contains(".error"),
                "Akzent außerhalb von Geblocktem:{selector}"
            );
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
        for token in ["--s-1:", "--s-2:", "--s-3:", "--s-4:", "--s-6:", "--s-8:"] {
            assert!(STYLE.contains(token), "Abstandsstufe fehlt: {token}");
        }
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
            STYLE.contains("@media (prefers-reduced-motion: reduce)"),
            "prefers-reduced-motion wird nicht beachtet"
        );
        // Höchstens eine dezente Übergangsregel.
        assert!(
            STYLE.matches("transition:").count() <= 2,
            "mehr Übergänge als die eine erlaubte Regel"
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
