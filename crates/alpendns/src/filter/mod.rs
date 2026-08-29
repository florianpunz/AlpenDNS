//! Blocklisten: Parser, Matcher, Block-Antworten, Aktualisierung.
//!
//! Der Filter hängt **vor** dem Cache (ARCHITECTURE.md §1). Das ist keine
//! Geschmacksfrage: der Cache hält die ungefilterte Antwort, damit sich alle
//! Clients einen Cache teilen können, ohne dass die Regeln des einen die
//! Antwort des anderen verändern.

pub mod block;
pub mod matcher;
pub mod parser;
pub mod source;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use arc_swap::ArcSwap;
use hickory_proto::op::Message;
use hickory_proto::rr::Name;
use tokio_util::sync::CancellationToken;

use crate::config::BlockingConfig;
use crate::resolve::{ResolveBackend, ResolveError};
use block::BlockMode;
use matcher::{Match, Matcher};
use source::{ListSpec, LoadError, Loader};

/// Der Regelstand, gegen den gerade geprüft wird.
#[derive(Debug, Default)]
pub struct FilterSet {
    pub block: Matcher,
    pub allow: Matcher,
}

/// Was der Filter zu einem Namen sagt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Auf keiner Liste.
    Pass,
    /// Auf einer Allowlist — ausdrücklich durchgelassen, auch wenn eine
    /// Blockliste ihn kennt.
    Allowed(Match),
    Blocked(Match),
}

#[derive(Debug, Default)]
struct Counters {
    blocked: AtomicU64,
    allowed: AtomicU64,
    passed: AtomicU64,
}

/// Zähler über die Lebensdauer des Prozesses. Nur Summen, keine Namen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterStats {
    pub blocked: u64,
    pub allowed: u64,
    pub passed: u64,
    /// Eindeutige Domains im aktuellen Regelstand.
    pub block_entries: usize,
    pub allow_entries: usize,
}

/// Prüft Namen gegen den aktuellen Regelstand.
#[derive(Debug)]
pub struct Filter {
    /// `ArcSwap` statt `RwLock`: Anfragen lesen ohne Lock, ein Update tauscht
    /// den Zeiger atomar. Laufende Anfragen sehen die alte Fassung zu Ende
    /// (ARCHITECTURE.md §3).
    sets: ArcSwap<FilterSet>,
    mode: BlockMode,
    sinkhole_v4: std::net::Ipv4Addr,
    sinkhole_v6: std::net::Ipv6Addr,
    counters: Counters,
}

impl Filter {
    pub fn new(set: FilterSet, config: &BlockingConfig) -> Self {
        Self {
            sets: ArcSwap::from_pointee(set),
            mode: config.mode,
            sinkhole_v4: config.sinkhole_ipv4,
            sinkhole_v6: config.sinkhole_ipv6,
            counters: Counters::default(),
        }
    }

    /// Tauscht den Regelstand. Kein Ausfall, keine Sperre im Anfragepfad.
    pub fn replace(&self, set: FilterSet) {
        self.sets.store(Arc::new(set));
    }

    /// Allowlist zuerst, dann Blocklisten.
    ///
    /// Die Reihenfolge ist die eigentliche Regel: eine Domain auf beiden Listen
    /// wird durchgelassen. Andersherum gäbe es keinen Weg, einen Fehlalarm einer
    /// fremden Liste zu übersteuern.
    pub fn verdict(&self, name: &Name) -> Verdict {
        let Some(normalized) = normalize_query(name) else {
            self.counters.passed.fetch_add(1, Ordering::Relaxed);
            return Verdict::Pass;
        };
        let sets = self.sets.load();

        if let Some(hit) = sets.allow.lookup(&normalized) {
            self.counters.allowed.fetch_add(1, Ordering::Relaxed);
            return Verdict::Allowed(hit);
        }
        if let Some(hit) = sets.block.lookup(&normalized) {
            self.counters.blocked.fetch_add(1, Ordering::Relaxed);
            return Verdict::Blocked(hit);
        }
        self.counters.passed.fetch_add(1, Ordering::Relaxed);
        Verdict::Pass
    }

    /// Baut die Antwort auf einen geblockten Namen.
    pub fn block_response(&self, request: &Message) -> Message {
        block::synthesize(request, self.mode, self.sinkhole_v4, self.sinkhole_v6)
    }

    /// Woher eine Regel kam, als Text für Logs und später für den Trace.
    /// Enthält Listennamen und Zeilennummer, keinen Query-Namen.
    pub fn describe(&self, hit: &Match, allow: bool) -> String {
        let sets = self.sets.load();
        let matcher = if allow { &sets.allow } else { &sets.block };
        let list = matcher.list_name(hit.rule.list).unwrap_or("?");
        format!("{list}:{}", hit.rule.line)
    }

    pub fn stats(&self) -> FilterStats {
        let sets = self.sets.load();
        FilterStats {
            blocked: self.counters.blocked.load(Ordering::Relaxed),
            allowed: self.counters.allowed.load(Ordering::Relaxed),
            passed: self.counters.passed.load(Ordering::Relaxed),
            block_entries: sets.block.len(),
            allow_entries: sets.allow.len(),
        }
    }
}

/// Bringt einen Namen aus einer Anfrage in die Form des Matchers.
fn normalize_query(name: &Name) -> Option<String> {
    let text = name.to_ascii();
    let trimmed = text.trim_end_matches('.');
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// Die konfigurierten Listen und wie sie geladen werden.
#[derive(Debug)]
pub struct Lists {
    loader: Loader,
    block: Vec<ListSpec>,
    allow: Vec<ListSpec>,
}

impl Lists {
    pub const fn new(loader: Loader, block: Vec<ListSpec>, allow: Vec<ListSpec>) -> Self {
        Self {
            loader,
            block,
            allow,
        }
    }

    /// Lädt alle Listen und baut daraus einen Regelstand.
    ///
    /// `strict` entscheidet, was eine unerreichbare Liste bedeutet: beim
    /// Erststart ein Fehler (lieber kein DNS als ungefiltertes DNS, B.1 Regel 6),
    /// bei einem späteren Update nur eine Warnung — dann gilt weiter, was schon
    /// geladen war.
    pub async fn load(&self, strict: bool) -> Result<FilterSet, LoadError> {
        Ok(FilterSet {
            block: self.build(&self.block, strict, "blocklist").await?,
            allow: self.build(&self.allow, strict, "allowlist").await?,
        })
    }

    async fn build(
        &self,
        specs: &[ListSpec],
        strict: bool,
        kind: &str,
    ) -> Result<Matcher, LoadError> {
        let mut builder = matcher::Builder::new();
        for spec in specs {
            match self.loader.load(spec).await {
                Ok(loaded) => {
                    let parsed = parser::parse(&loaded.text, spec.format);
                    let entries = parsed.entries.len();
                    if parsed.skipped > 0 {
                        tracing::warn!(
                            list = %spec.name,
                            skipped = parsed.skipped,
                            entries,
                            "Zeilen in der Liste nicht verstanden"
                        );
                    }
                    tracing::info!(
                        list = %spec.name,
                        kind,
                        entries,
                        origin = ?loaded.origin,
                        "Liste geladen"
                    );
                    builder.add(&spec.name, &parsed);
                }
                Err(error) if strict => return Err(error),
                Err(error) => {
                    tracing::error!(list = %spec.name, %error, "Liste nicht geladen, alter Stand gilt weiter");
                }
            }
        }
        Ok(builder.build())
    }
}

/// Lädt die Listen regelmäßig neu und tauscht den Regelstand aus.
///
/// Alle Listen werden gemeinsam erneuert, mit dem kürzesten konfigurierten
/// Intervall. Getrennte Zeitpläne wären genauer, aber ein Abruf mit ETag kostet
/// bei unveränderter Liste nur ein 304 — der Aufwand lohnt die Komplexität nicht.
pub async fn run_updater(
    filter: Arc<Filter>,
    lists: Arc<Lists>,
    interval: Duration,
    shutdown: CancellationToken,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await; // der erste Tick kommt sofort
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        // Nicht strikt: ein Ausfall jetzt darf den laufenden Betrieb nicht
        // ungefiltert machen (B.1 Regel 6).
        match lists.load(false).await {
            Ok(set) => {
                let entries = set.block.len();
                filter.replace(set);
                tracing::info!(entries, "Blocklisten aktualisiert");
            }
            Err(error) => tracing::error!(%error, "Aktualisierung fehlgeschlagen"),
        }
    }
}

/// Der Filter als Schicht vor dem Rest der Pipeline.
#[derive(Debug)]
pub struct FilterBackend<B> {
    filter: Arc<Filter>,
    inner: B,
}

impl<B: ResolveBackend> FilterBackend<B> {
    pub const fn new(filter: Arc<Filter>, inner: B) -> Self {
        Self { filter, inner }
    }
}

impl<B: ResolveBackend> ResolveBackend for FilterBackend<B> {
    fn resolve(
        &self,
        request: &Message,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let verdict = request
            .queries
            .first()
            .map(|query| self.filter.verdict(query.name()));

        async move {
            match verdict {
                Some(Verdict::Blocked(hit)) => {
                    // Kein Query-Name im Log (B.1 Regel 3) — nur, welche Regel
                    // aus welcher Liste zugeschlagen hat.
                    tracing::debug!(rule = %self.filter.describe(&hit, false), "geblockt");
                    Ok(self.filter.block_response(request))
                }
                _ => self.inner.resolve(request).await,
            }
        }
    }
}
