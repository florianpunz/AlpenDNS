//! Blocklisten: Parser, Matcher, Block-Antworten, Aktualisierung.
//!
//! Hier wohnt alles, was mit Listen zu tun hat: Formate lesen, nachschlagen,
//! Block-Antworten bauen, Listen aktuell halten. *Wer* welche Liste bekommt,
//! entscheidet [`crate::policy`].

pub mod block;
pub mod matcher;
pub mod parser;
pub mod source;

use std::collections::HashMap;
use std::sync::Arc;

use matcher::Matcher;
use source::{Age, ListSpec, LoadError, Loader};

/// Alle geladenen Listen, nach Namen.
///
/// Eine Liste wird einmal geladen und geparst; Policies verweisen darauf. Zwei
/// Policies, die dieselbe Liste benutzen, teilen sich den Matcher — bei zwei
/// Millionen Einträgen ist das der Unterschied zwischen 135 und 270 MB.
#[derive(Debug, Default)]
pub struct LoadedLists {
    by_name: HashMap<Arc<str>, Arc<Matcher>>,
}

impl LoadedLists {
    /// Aus fertigen Matchern zusammensetzen — für Tests und Benchmarks, die
    /// keine Dateien laden wollen.
    pub fn from_matchers(matchers: Vec<(Arc<str>, Arc<Matcher>)>) -> Self {
        Self {
            by_name: matchers.into_iter().collect(),
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<Matcher>> {
        self.by_name.get(name).map(Arc::clone)
    }

    pub fn names(&self) -> impl Iterator<Item = &Arc<str>> {
        self.by_name.keys()
    }

    /// Summe der Einträge über alle Listen. Für Logs und Metriken.
    pub fn total_entries(&self) -> usize {
        self.by_name.values().map(|matcher| matcher.len()).sum()
    }
}

/// Die konfigurierten Listen und wie sie geladen werden.
#[derive(Debug)]
pub struct Lists {
    loader: Loader,
    specs: Vec<ListSpec>,
}

impl Lists {
    pub const fn new(loader: Loader, specs: Vec<ListSpec>) -> Self {
        Self { loader, specs }
    }

    /// Name und Alter jeder konfigurierten Liste, in Konfigurationsreihenfolge.
    ///
    /// Gefragt wird der Loader, nicht der Bestand im Speicher: die Liste lebt
    /// auf Platte, und ihr Datum ändert sich, ohne dass sich hier etwas rührt.
    ///
    /// Hier und nicht beim Aufrufer, weil nur diese Struktur Loader und Specs
    /// zusammen hat — und weil ein `SIGHUP` sie als Ganzes austauscht, sodass
    /// eine mitgeführte Kopie veralten würde.
    pub fn ages(&self) -> Vec<(String, Age)> {
        self.specs
            .iter()
            .map(|spec| (spec.name.clone(), self.loader.age(spec)))
            .collect()
    }

    /// Lädt alle Listen.
    ///
    /// `strict` entscheidet, was eine unerreichbare Liste bedeutet: beim
    /// Erststart ein Fehler (lieber kein DNS als ungefiltertes DNS, B.1 Regel 6),
    /// bei einem späteren Update nur eine Warnung — dann gilt weiter, was schon
    /// geladen war.
    pub async fn load(&self, strict: bool) -> Result<LoadedLists, LoadError> {
        let mut by_name = HashMap::with_capacity(self.specs.len());
        for spec in &self.specs {
            match self.loader.load(spec).await {
                Ok(loaded) => {
                    let parsed = parser::parse(&loaded.text, spec.format);
                    let entries = parsed.entries.len();
                    if parsed.skipped > 0 {
                        tracing::warn!(
                            list = %spec.name,
                            skipped = parsed.skipped,
                            entries,
                            "lines in the list not understood"
                        );
                    }
                    tracing::info!(
                        list = %spec.name,
                        entries,
                        origin = ?loaded.origin,
                        "list loaded"
                    );
                    let mut builder = matcher::Builder::new();
                    builder.add(&spec.name, &parsed);
                    by_name.insert(Arc::from(spec.name.as_str()), Arc::new(builder.build()));
                }
                Err(error) if strict => return Err(error),
                Err(error) => {
                    tracing::error!(list = %spec.name, %error, "list not loaded");
                }
            }
        }
        Ok(LoadedLists { by_name })
    }
}
