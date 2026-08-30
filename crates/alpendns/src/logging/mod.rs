//! Was mit dem Decision-Trace geschieht.
//!
//! Der Trace entsteht immer (ARCHITECTURE.md §2). **Hier** entscheidet sich, was
//! davon den Prozess überlebt — und nur hier dürfen Query-Namen überhaupt
//! auftauchen (B.1 Regel 3). Die vier Modi stammen aus
//! [ADR-0004](../../../docs/adr/0004-logging-default-aggregiert.md):
//!
//! | Modus | was gespeichert wird |
//! |---|---|
//! | `none` | nur globale Zähler |
//! | `aggregate` | zusätzlich Häufigkeiten in einer Zählertabelle ohne Namen; ein Name erscheint erst ab `aggregate_k` Treffern |
//! | `ring` | zusätzlich die letzten `ring_seconds` im RAM, nie auf Platte |
//! | `full` | zusätzlich strukturierte Zeilen auf Platte |
//!
//! Default ist `aggregate`. Wer `full` will, muss es hinschreiben.

pub mod counts;
pub mod ring;

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use hickory_proto::op::ResponseCode;
use serde::Deserialize;

use counts::Counts;
use ring::Ring;

use crate::trace::Step;

/// Wie viel eine Anfrage hinterlässt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Nur Zähler. Maximum an Zurückhaltung.
    None,
    /// Zähler plus Häufigkeiten über der k-Schwelle.
    #[default]
    Aggregate,
    /// Zusätzlich ein Zeitfenster im RAM, für Fehlersuche.
    Ring,
    /// Zusätzlich eine Datei. Bewusste Entscheidung des Betreibers.
    Full,
}

impl Mode {
    /// Alle Modi. Wie bei [`crate::filter::block::BlockMode::ALL`] gibt die
    /// Metrik jeden aus, damit über mehrere Installationen sichtbar wird,
    /// welche überhaupt jemand benutzt.
    pub const ALL: [Self; 4] = [Self::None, Self::Aggregate, Self::Ring, Self::Full];

    /// Ob in diesem Modus überhaupt Namen gespeichert werden dürfen.
    pub const fn keeps_names(self) -> bool {
        matches!(self, Self::Ring | Self::Full)
    }

    /// Wie der Modus in der Konfiguration heißt.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Aggregate => "aggregate",
            Self::Ring => "ring",
            Self::Full => "full",
        }
    }
}

/// Woran eine Anfrage gescheitert ist.
///
/// Abgeleitet aus dem Decision-Trace, nicht getrennt mitgeführt: der Trace ist
/// die Quelle der Wahrheit dafür, warum eine Antwort so ausfiel
/// (ARCHITECTURE.md §2). Eine zweite Buchführung daneben liefe irgendwann
/// auseinander.
///
/// Die Aufzählung ist bewusst offen für Phase 8: die Heuristiken aus
/// `alpendns-detect` (DGA, Tunneling, Rebinding) bekommen dann je eine
/// Variante, ohne dass Zähler, API oder UI sich ändern müssen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockReason {
    /// Eine Blockliste hat den Namen enthalten.
    Blocklist,
    /// Eine Regex-Regel der Policy hat gepasst.
    Regex,
    /// Ein Zeitplan war aktiv.
    Schedule,
    /// Geblockt, aber kein Schritt sagt warum. Sollte nicht vorkommen und wird
    /// deshalb sichtbar gezählt statt stillschweigend einem Grund zugeschlagen.
    Other,
}

impl BlockReason {
    /// Alle Gründe. Wie [`Mode::ALL`] gibt die Metrik jeden aus, auch mit Wert
    /// null — sonst verschwindet eine Kategorie aus der Ausgabe, sobald sie
    /// gerade nicht vorkommt, und ein Diagramm darüber bekommt Lücken.
    pub const ALL: [Self; 4] = [Self::Blocklist, Self::Regex, Self::Schedule, Self::Other];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Blocklist => "blocklist",
            Self::Regex => "regex",
            Self::Schedule => "schedule",
            Self::Other => "other",
        }
    }

    /// Der Grund, den ein Trace nennt.
    ///
    /// Es gewinnt der zuletzt eingetragene passende Schritt: die Kette wird von
    /// außen nach innen aufgebaut, und der letzte Treffer ist der, der die
    /// Entscheidung tatsächlich herbeigeführt hat.
    pub fn from_steps(steps: &[Step]) -> Self {
        steps
            .iter()
            .rev()
            .find_map(|step| match step {
                Step::BlocklistHit { .. } => Some(Self::Blocklist),
                Step::RegexHit { .. } => Some(Self::Regex),
                Step::ScheduleHit { .. } => Some(Self::Schedule),
                _ => None,
            })
            .unwrap_or(Self::Other)
    }
}

/// Was über eine beantwortete Anfrage bekannt ist.
#[derive(Debug, Clone)]
pub struct QueryEvent {
    pub name: String,
    pub query_type: String,
    pub client: Arc<str>,
    pub blocked: bool,
    /// Nur gesetzt, wenn `blocked`.
    pub reason: Option<BlockReason>,
    pub rcode: ResponseCode,
    /// Die Begründungskette, schon als Text.
    pub why: Vec<String>,
    pub elapsed: Duration,
}

/// Was über den Live-Strom hinausgeht.
///
/// In `none` und `aggregate` bleiben Name, Client und Begründung leer: der Puls
/// ist sichtbar, der Inhalt nicht. Das ist genau der Unterschied, den ADR-0004
/// beschreibt.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StreamEvent {
    pub at: String,
    pub blocked: bool,
    pub rcode: String,
    pub ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<Vec<String>>,
}

/// Ein Eintrag, wie ihn API und UI sehen.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LoggedQuery {
    pub at: String,
    pub name: String,
    #[serde(rename = "type")]
    pub query_type: String,
    pub client: String,
    pub blocked: bool,
    pub rcode: String,
    pub why: Vec<String>,
    pub ms: f64,
}

/// Zähler, die es in jedem Modus gibt.
#[derive(Debug, Default)]
struct Counters {
    queries: AtomicU64,
    blocked: AtomicU64,
    /// Nach RCODE, indiziert über die niederwertigen vier Bit.
    by_rcode: [AtomicU64; 16],
    /// Nach Block-Grund, in der Reihenfolge von [`BlockReason::ALL`].
    by_reason: [AtomicU64; BlockReason::ALL.len()],
}

/// Momentaufnahme der Zähler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogStats {
    pub queries: u64,
    pub blocked: u64,
    pub by_rcode: Vec<(String, u64)>,
    /// Geblockte Anfragen je Grund. Enthält jeden Grund, auch mit Wert null.
    pub by_reason: Vec<(BlockReason, u64)>,
    /// Namen im Ringpuffer. In `none` und `aggregate` immer 0.
    pub ring_entries: usize,
}

/// Eine Domain mit ihrer Häufigkeit — nur oberhalb der k-Schwelle.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TopDomain {
    pub name: String,
    pub count: u32,
    pub blocked: bool,
}

/// Die Häufigkeitsliste samt dem, was sie verschweigt.
///
/// Die verschwiegenen Anfragen werden **als Summe** ausgewiesen, nicht
/// weggelassen. Sonst ergäbe die Liste ein falsches Bild vom Verkehr: bei einem
/// frisch gestarteten Server steht fast alles unter der Schwelle, und eine
/// Ansicht, die das nicht sagt, sieht aus wie ein Server ohne Verkehr. Die
/// Summe verrät nichts über einzelne Namen — genau darin besteht der Handel
/// (ADR-0004).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TopReport {
    /// Die k-Schwelle, ab der ein Name überhaupt genannt werden darf.
    pub threshold: u32,
    pub domains: Vec<TopDomain>,
    /// Anfragen auf Namen, die die Schwelle nicht erreicht haben.
    pub below_threshold_queries: u64,
    /// Wie viele verschiedene Namen das sind.
    pub below_threshold_names: usize,
}

/// Nimmt Anfragen entgegen und behält davon, was der Modus zulässt.
#[derive(Debug)]
pub struct QueryLog {
    mode: Mode,
    aggregate_k: u32,
    counters: Counters,
    counts: Mutex<Counts>,
    /// Namen, die die Schwelle überschritten haben. Vorher steht ein Name
    /// **nirgends** — das ist der ganze Punkt.
    reportable: Mutex<HashMap<String, DomainCount>>,
    ring: Mutex<Ring<LoggedQuery>>,
    file: Option<Mutex<std::fs::File>>,
    /// Live-Strom für die UI. Wer nicht zuhört, kostet nichts.
    events: tokio::sync::broadcast::Sender<StreamEvent>,
}

#[derive(Debug, Clone, Copy)]
struct DomainCount {
    count: u32,
    blocked: bool,
}

impl QueryLog {
    pub fn new(config: &crate::config::LoggingConfig) -> std::io::Result<Self> {
        let file = if config.mode == Mode::Full {
            if let Some(parent) = config.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Some(Mutex::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&config.path)?,
            ))
        } else {
            None
        };

        Ok(Self {
            mode: config.mode,
            aggregate_k: config.aggregate_k,
            counters: Counters::default(),
            counts: Mutex::new(Counts::new()),
            reportable: Mutex::new(HashMap::new()),
            ring: Mutex::new(Ring::new(config.ring_seconds)),
            file,
            // Puffer für Zuhörer, die kurz nicht hinterherkommen. Wer weiter
            // zurückfällt, verliert Ereignisse — ein Live-Strom soll den Server
            // nicht bremsen.
            events: tokio::sync::broadcast::channel(256).0,
        })
    }

    pub const fn mode(&self) -> Mode {
        self.mode
    }

    /// Die k-Schwelle dieser Instanz.
    pub const fn aggregate_k(&self) -> u32 {
        self.aggregate_k
    }

    /// Ob Anfragen auf die Platte geschrieben werden.
    ///
    /// Die UI behauptet "Daten nur im RAM" — diese Behauptung muss aus dem
    /// laufenden Prozess kommen und nicht aus der Annahme, dass schon niemand
    /// `full` eingeschaltet haben wird.
    pub const fn writes_to_disk(&self) -> bool {
        self.file.is_some()
    }

    /// Nimmt eine beantwortete Anfrage auf.
    pub fn record(&self, event: &QueryEvent) {
        self.counters.queries.fetch_add(1, Ordering::Relaxed);
        if event.blocked {
            self.counters.blocked.fetch_add(1, Ordering::Relaxed);
            let reason = event.reason.unwrap_or(BlockReason::Other);
            if let Some(index) = BlockReason::ALL.iter().position(|&r| r == reason)
                && let Some(counter) = self.counters.by_reason.get(index)
            {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        }
        let rcode = usize::from(u16::from(event.rcode) & 0x0f);
        if let Some(counter) = self.counters.by_rcode.get(rcode) {
            counter.fetch_add(1, Ordering::Relaxed);
        }

        self.publish(event);

        if self.mode == Mode::None {
            return;
        }

        // Ab `aggregate`: die Häufigkeit zählt mit, der Name aber erst, wenn er
        // die Schwelle überschritten hat.
        {
            let count = self
                .counts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .add(&event.name);
            // k = 0 wäre sonst ein stiller Weg, die Schwelle abzuschalten.
            if self.aggregate_k > 0 && count >= self.aggregate_k {
                self.reportable
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(
                        event.name.clone(),
                        DomainCount {
                            count,
                            blocked: event.blocked,
                        },
                    );
            }
        }

        if !self.mode.keeps_names() {
            return;
        }

        let entry = LoggedQuery {
            at: jiff::Zoned::now()
                .strftime("%Y-%m-%dT%H:%M:%S%:z")
                .to_string(),
            name: event.name.clone(),
            query_type: event.query_type.clone(),
            client: event.client.to_string(),
            blocked: event.blocked,
            rcode: format!("{:?}", event.rcode),
            why: event.why.clone(),
            ms: event.elapsed.as_secs_f64() * 1000.0,
        };

        self.ring
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Instant::now(), entry.clone());

        if let Some(file) = &self.file
            && let Ok(line) = serde_json::to_string(&entry)
        {
            let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
            // Ein Schreibfehler darf keine Anfrage scheitern lassen.
            let _ = writeln!(file, "{line}");
        }
    }

    /// Wer den Live-Strom mitlesen will.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<StreamEvent> {
        self.events.subscribe()
    }

    /// Schickt das Ereignis in den Live-Strom — mit Namen nur dort, wo der
    /// Modus es zulässt.
    fn publish(&self, event: &QueryEvent) {
        if self.events.receiver_count() == 0 {
            return;
        }
        let detailed = self.mode.keeps_names();
        // Fehlt nur, wenn niemand mehr zuhört.
        let _ = self.events.send(StreamEvent {
            at: jiff::Zoned::now().strftime("%H:%M:%S").to_string(),
            blocked: event.blocked,
            rcode: format!("{:?}", event.rcode),
            ms: event.elapsed.as_secs_f64() * 1000.0,
            name: detailed.then(|| event.name.clone()),
            client: detailed.then(|| event.client.to_string()),
            why: detailed.then(|| event.why.clone()),
        });
    }

    pub fn stats(&self) -> LogStats {
        let by_rcode = self
            .counters
            .by_rcode
            .iter()
            .enumerate()
            .filter_map(|(code, counter)| {
                let value = counter.load(Ordering::Relaxed);
                (value > 0).then(|| {
                    let code = u8::try_from(code).unwrap_or(0);
                    (format!("{:?}", ResponseCode::from(0, code)), value)
                })
            })
            .collect();
        let by_reason = BlockReason::ALL
            .iter()
            .enumerate()
            .map(|(index, &reason)| {
                let value = self
                    .counters
                    .by_reason
                    .get(index)
                    .map_or(0, |counter| counter.load(Ordering::Relaxed));
                (reason, value)
            })
            .collect();
        LogStats {
            queries: self.counters.queries.load(Ordering::Relaxed),
            blocked: self.counters.blocked.load(Ordering::Relaxed),
            by_rcode,
            by_reason,
            ring_entries: self
                .ring
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .len(),
        }
    }

    /// Die häufigsten Domains — ausschließlich solche über der k-Schwelle.
    pub fn top_domains(&self, limit: usize) -> Vec<TopDomain> {
        self.top(limit).domains
    }

    /// Die Häufigkeitsliste mit der Summe dessen, was unter der Schwelle bleibt.
    pub fn top(&self, limit: usize) -> TopReport {
        if self.mode == Mode::None {
            // Ohne Zählung gibt es auch nichts zu verschweigen.
            return TopReport {
                threshold: self.aggregate_k,
                domains: Vec::new(),
                below_threshold_queries: 0,
                below_threshold_names: 0,
            };
        }

        let reportable = self
            .reportable
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let mut found: Vec<TopDomain> = reportable
            .iter()
            .map(|(name, entry)| TopDomain {
                name: name.clone(),
                count: entry.count,
                blocked: entry.blocked,
            })
            .collect();
        let named_queries: u64 = found.iter().map(|entry| u64::from(entry.count)).sum();
        let named_names = found.len();
        drop(reportable);

        found.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        found.truncate(limit);

        let counts = self.counts.lock().unwrap_or_else(PoisonError::into_inner);
        TopReport {
            threshold: self.aggregate_k,
            domains: found,
            below_threshold_queries: counts.total().saturating_sub(named_queries),
            below_threshold_names: counts.tracked().saturating_sub(named_names),
        }
    }

    /// Die jüngsten Anfragen. Leer in `none` und `aggregate`.
    pub fn recent(&self, limit: usize) -> Vec<LoggedQuery> {
        if !self.mode.keeps_names() {
            return Vec::new();
        }
        self.ring
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .recent(Instant::now(), limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_block_reason_comes_from_the_last_matching_step() {
        // Die Kette wird von außen nach innen aufgebaut; entschieden hat der
        // letzte Treffer. Eine Anfrage, die erst an einer Regex hängen bleibt
        // und dann auf einer Liste steht, ist ein Listentreffer.
        let steps = [
            Step::RegexHit {
                policy: Arc::from("kinder"),
                pattern: Arc::from("^ads"),
            },
            Step::BlocklistHit {
                list: Arc::from("stevenblack"),
                line: 7,
                matched: "ads.example.com".to_owned(),
            },
        ];
        assert_eq!(BlockReason::from_steps(&steps), BlockReason::Blocklist);
    }

    #[test]
    fn a_block_without_a_named_step_is_counted_as_other() {
        // Lieber sichtbar in einer Restkategorie als still einem Grund
        // zugeschlagen, der es nicht war.
        let steps = [Step::PolicyApplied {
            policy: Arc::from("default"),
        }];
        assert_eq!(BlockReason::from_steps(&steps), BlockReason::Other);
    }

    #[test]
    fn every_reason_has_a_stable_name() {
        // Die Namen stehen in Metrik-Labels und in der UI; sie sind Teil der
        // Schnittstelle, nicht bloß Debug-Ausgabe.
        let names: Vec<&str> = BlockReason::ALL.iter().map(|r| r.as_str()).collect();
        assert_eq!(names, ["blocklist", "regex", "schedule", "other"]);
    }
}
