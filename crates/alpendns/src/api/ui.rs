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
    fn the_stylesheet_keeps_to_one_accent_colour() {
        // Ein Akzentton, alles andere neutrale Graustufen (CLAUDE.md B.6).
        let accents = STYLE.matches("--accent:").count();
        assert_eq!(accents, 2, "erwartet: je einer für hell und dunkel");
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
