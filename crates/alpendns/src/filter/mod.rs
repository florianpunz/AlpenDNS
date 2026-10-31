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

use futures_util::StreamExt as _;

use matcher::Matcher;
use source::{Age, ListSpec, LoadError, Loaded, Loader};

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

    /// Das Format einer konfigurierten Liste, für die Statusanzeige.
    ///
    /// `None` heißt: die Liste steht nicht (mehr) in der aktuellen
    /// Konfiguration — nach einem Reload kann der geladene Stand älter sein.
    pub fn format_of(&self, name: &str) -> Option<String> {
        self.specs
            .iter()
            .find(|spec| spec.name == name)
            .map(|spec| spec.format.to_string())
    }

    /// Lädt alle Listen.
    ///
    /// `strict` entscheidet, was eine unerreichbare Liste bedeutet: beim
    /// Erststart ein Fehler (lieber kein DNS als ungefiltertes DNS, B.1 Regel 6),
    /// bei einem späteren Update nur eine Warnung — dann gilt weiter, was schon
    /// geladen war.
    ///
    /// Geladen wird mit begrenzter Nebenläufigkeit: ein Haushalt hat selten mehr
    /// als eine Handvoll Listen, und nacheinander summiert sich die Wartezeit
    /// jedes Servers. Verarbeitet wird danach in Konfigurationsreihenfolge,
    /// damit Logs und der strikte Fehler nicht davon abhängen, welche Liste
    /// zuerst antwortet.
    pub async fn load(&self, strict: bool) -> Result<LoadedLists, LoadError> {
        const CONCURRENCY: usize = 4;

        let results: Vec<(usize, Result<Loaded, LoadError>)> =
            futures_util::stream::iter(self.specs.iter().cloned().enumerate())
                .map(|(index, spec)| {
                    let loader = self.loader.clone();
                    async move { (index, loader.load(&spec).await) }
                })
                .buffer_unordered(CONCURRENCY)
                .collect()
                .await;

        let mut by_index: Vec<Option<Result<Loaded, LoadError>>> =
            (0..self.specs.len()).map(|_| None).collect();
        for (index, result) in results {
            if let Some(slot) = by_index.get_mut(index) {
                *slot = Some(result);
            }
        }

        let mut by_name = HashMap::with_capacity(self.specs.len());
        for (spec, result) in self.specs.iter().zip(by_index) {
            let Some(result) = result else {
                continue;
            };
            match result {
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
