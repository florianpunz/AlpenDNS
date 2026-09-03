//! Wer darf was — und warum.
//!
//! Diese Schicht ersetzt den globalen Filter aus Phase 4. Statt einer Regel für
//! alle bekommt jeder Client eine Policy, und jede Policy ihre eigenen Listen,
//! Regex-Regeln und Zeitfenster.
//!
//! Sie liegt **vor** dem Cache (ARCHITECTURE.md §1). Das ist keine
//! Geschmacksfrage: der Cache hält die ungefilterte Antwort, damit sich alle
//! Clients einen Cache teilen können, ohne dass die Regeln des einen die Antwort
//! des anderen verändern.

pub mod clients;
pub mod rules;
pub mod schedule;
pub mod temporary;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use arc_swap::ArcSwap;
use hickory_proto::op::Message;
use hickory_proto::rr::Name;

use crate::clock::{Clock, WallClock};
use crate::config::{BlockingConfig, ConfigError};
use crate::filter::LoadedLists;
use crate::filter::block::{self, BlockMode};
use crate::filter::matcher::Matcher;
use crate::resolve::{ResolveBackend, ResolveError};
use crate::trace::{Ctx, Step};
use clients::Clients;
use rules::RegexRules;
use schedule::Schedule;
use temporary::Temporary;

/// Was für einen Client gilt.
#[derive(Debug)]
pub struct Policy {
    pub name: Arc<str>,
    pub blocklists: Vec<Arc<Matcher>>,
    pub allowlists: Vec<Arc<Matcher>>,
    pub regex: Arc<RegexRules>,
    pub schedules: Vec<Schedule>,
}

/// Der aktuelle Regelstand: Clients, ihre Policies, die geladenen Listen.
#[derive(Debug)]
pub struct PolicySet {
    clients: Clients,
    policies: HashMap<Arc<str>, Arc<Policy>>,
    /// Greift, wenn eine Policy in der Konfiguration fehlt. Blockt nichts.
    fallback: Arc<Policy>,
}

impl PolicySet {
    fn policy(&self, name: &str) -> Arc<Policy> {
        self.policies
            .get(name)
            .map_or_else(|| Arc::clone(&self.fallback), Arc::clone)
    }
}

/// Das Ergebnis einer Auswertung. Die Begründung steht im [`Ctx`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Block,
}

/// Eine Auswertung samt ihrer Begründungskette, für Menschen.
///
/// Erzeugt von [`explain`] und benutzt von genau zwei Aufrufern: `alpendns
/// policy test` auf der Kommandozeile und `/api/explain` für den Klick auf eine
/// Zeile im Protokoll. Beide sollen dieselbe Antwort geben — deshalb ist es
/// eine Funktion und nicht zweimal dieselbe Schleife.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Explanation {
    pub domain: String,
    pub client: String,
    pub blocked: bool,
    /// Ein Schritt je Zeile, in der Reihenfolge der Entscheidung.
    pub steps: Vec<String>,
}

/// Wertet einen Namen aus und gibt die Begründung zurück, ohne etwas zu ändern.
///
/// Die Auswertung ist eine Simulation: sie zählt in keiner Statistik mit und
/// hinterlässt nichts. Der Name kommt vom Aufrufer und geht an ihn zurück —
/// gespeichert wird er nirgends, auch nicht in den leisen Log-Modi.
pub fn explain<C: Clock + Clone, W: WallClock>(
    engine: &Engine<C, W>,
    domain: &str,
    peer: IpAddr,
) -> Result<Explanation, String> {
    let name = Name::from_str_relaxed(domain)
        .map_err(|error| format!("'{domain}' ist kein gültiger Domainname: {error}"))?;
    let mut ctx = Ctx::new(std::net::SocketAddr::new(peer, 0));
    let decision = engine.evaluate(&name, peer, &mut ctx);
    let client = ctx
        .steps()
        .iter()
        .find_map(|step| match step {
            Step::ClientMatched { client, .. } => Some(client.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "unknown".to_owned());

    Ok(Explanation {
        domain: domain.to_owned(),
        client,
        blocked: decision == Decision::Block,
        steps: ctx.steps().iter().map(ToString::to_string).collect(),
    })
}

#[derive(Debug, Default)]
struct Counters {
    blocked: AtomicU64,
    allowed: AtomicU64,
    passed: AtomicU64,
}

/// Zähler über die Lebensdauer des Prozesses. Nur Summen, keine Namen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyStats {
    pub blocked: u64,
    /// Ausdrücklich durchgelassen: Allowlist oder befristete Freigabe.
    pub allowed: u64,
    pub passed: u64,
    pub entries: usize,
}

/// Wertet Policies aus und beantwortet geblockte Anfragen selbst.
#[derive(Debug)]
pub struct Engine<C, W> {
    /// `ArcSwap` statt `RwLock`: Anfragen lesen ohne Lock, ein Listen-Update
    /// tauscht den Zeiger atomar (ARCHITECTURE.md §3).
    set: ArcSwap<PolicySet>,
    temporary: Temporary<C>,
    /// Der Gegenpart: befristete Sperren, gesetzt über die API.
    denied: Temporary<C>,
    /// Die Heuristiken aus Phase 8. Leer, wenn alle abgeschaltet sind.
    detectors: crate::detect::Detectors,
    wall: W,
    mode: BlockMode,
    sinkhole_v4: std::net::Ipv4Addr,
    sinkhole_v6: std::net::Ipv6Addr,
    entries: std::sync::atomic::AtomicUsize,
    counters: Counters,
}

impl<C: Clock + Clone, W: WallClock> Engine<C, W> {
    pub fn new(set: PolicySet, entries: usize, config: &BlockingConfig, clock: C, wall: W) -> Self {
        Self {
            set: ArcSwap::from_pointee(set),
            temporary: Temporary::new(clock.clone()),
            denied: Temporary::new(clock),
            detectors: crate::detect::Detectors::new(),
            wall,
            mode: config.mode,
            sinkhole_v4: config.sinkhole_ipv4,
            sinkhole_v6: config.sinkhole_ipv6,
            entries: std::sync::atomic::AtomicUsize::new(entries),
            counters: Counters::default(),
        }
    }

    /// Tauscht den Regelstand. Kein Ausfall, keine Sperre im Anfragepfad.
    pub fn replace(&self, set: PolicySet, entries: usize) {
        self.set.store(Arc::new(set));
        self.entries.store(entries, Ordering::Relaxed);
    }

    /// Zugriff auf die befristeten Freigaben — für die CLI und ab Phase 6 die API.
    /// Hängt die Heuristiken ein. Nur beim Aufbau, nicht im Betrieb.
    #[must_use]
    pub fn with_detectors(mut self, detectors: crate::detect::Detectors) -> Self {
        self.detectors = detectors;
        self
    }

    /// Was die Konfiguration über die Detektoren sagt — für Status und Metrik.
    pub fn detectors(&self) -> Vec<(crate::detect::Detector, crate::detect::Action)> {
        self.detectors.configured()
    }

    pub const fn denied(&self) -> &Temporary<C> {
        &self.denied
    }

    pub const fn temporary(&self) -> &Temporary<C> {
        &self.temporary
    }

    /// Wertet eine Anfrage aus und schreibt die Begründung in den Kontext.
    ///
    /// **Reihenfolge** (ARCHITECTURE.md §1): befristete Freigabe → Allowlist →
    /// Blocklisten → Regex → Zeitplan. Die Allowlist steht vorn, weil sie sonst
    /// keinen Zweck hätte: sie ist das Mittel, einen Fehlalarm einer fremden
    /// Liste zu übersteuern.
    pub fn evaluate(&self, name: &Name, peer: IpAddr, ctx: &mut Ctx) -> Decision {
        self.evaluate_typed(name, hickory_proto::rr::RecordType::A, peer, ctx)
    }

    /// Wie [`Self::evaluate`], aber mit dem Anfragetyp.
    ///
    /// Den braucht die Tunneling-Erkennung: der Anteil an TXT- und
    /// NULL-Anfragen je Zone ist eines ihrer fünf Signale (FEATURES.md D2).
    /// `evaluate` ohne Typ bleibt für `policy test` und `/api/explain`, wo es
    /// keine echte Anfrage gibt.
    pub fn evaluate_typed(
        &self,
        name: &Name,
        query_type: hickory_proto::rr::RecordType,
        peer: IpAddr,
        ctx: &mut Ctx,
    ) -> Decision {
        let set = self.set.load();
        let identity = set.clients.identify(peer);
        ctx.record(Step::ClientMatched {
            client: Arc::clone(&identity.client),
            by: identity.by,
        });
        let policy = set.policy(&identity.policy);
        ctx.record(Step::PolicyApplied {
            policy: Arc::clone(&policy.name),
        });

        let Some(query) = normalize(name) else {
            self.counters.passed.fetch_add(1, Ordering::Relaxed);
            return Decision::Allow;
        };

        if let Some(remaining) = self.temporary.check(&query) {
            ctx.record(Step::TemporaryAllow { remaining });
            self.counters.allowed.fetch_add(1, Ordering::Relaxed);
            return Decision::Allow;
        }

        // Die Sperre steht nach der Freigabe und vor den Listen: sie ist eine
        // ausdrückliche Anordnung eines Menschen und soll eine fremde Liste
        // übersteuern können — aber die Freigabe, die ebenfalls von einem
        // Menschen kommt, gewinnt gegen sie.
        if let Some(remaining) = self.denied.check(&query) {
            ctx.record(Step::TemporaryDeny { remaining });
            return self.block(ctx);
        }

        let allowlisted = policy
            .allowlists
            .iter()
            .find_map(|matcher| matcher.lookup(&query).map(|hit| (Arc::clone(matcher), hit)));
        if let Some((matcher, hit)) = allowlisted {
            ctx.record(Step::AllowlistHit {
                list: Arc::from(matcher.list_name(hit.rule.list).unwrap_or("?")),
                line: hit.rule.line,
                matched: hit.matched,
            });
            self.counters.allowed.fetch_add(1, Ordering::Relaxed);
            return Decision::Allow;
        }

        let blocklisted = policy
            .blocklists
            .iter()
            .find_map(|matcher| matcher.lookup(&query).map(|hit| (Arc::clone(matcher), hit)));
        if let Some((matcher, hit)) = blocklisted {
            ctx.record(Step::BlocklistHit {
                list: Arc::from(matcher.list_name(hit.rule.list).unwrap_or("?")),
                line: hit.rule.line,
                matched: hit.matched,
            });
            return self.block(ctx);
        }

        if let Some(rule) = policy.regex.matches(&query) {
            ctx.record(Step::RegexHit {
                policy: Arc::clone(&policy.name),
                pattern: Arc::clone(&rule.pattern),
            });
            return self.block(ctx);
        }

        // Der Zeitplan steht vor den Heuristiken: wer bis hierher gekommen ist,
        // steht auf keiner Allowlist — und genau das ist die Bedingung von
        // `block_all_except_allowlist`.
        if let Some(active) = schedule::first_active(&policy.schedules, &self.wall.now()) {
            ctx.record(Step::ScheduleHit {
                schedule: Arc::clone(&active.name),
                effect: active.effect,
            });
            return self.block(ctx);
        }

        // Die Heuristiken stehen ganz zuletzt (ARCHITECTURE.md §1). Sie sind
        // das unschärfste Mittel im Haus, und alles, was eine klare Regel
        // entscheiden kann, soll vorher entschieden sein — sonst stünde im
        // Trace ein Score, wo eine Zeile aus einer Liste hingehört.
        if self.inspect(&query, query_type, ctx) == Decision::Block {
            return self.block(ctx);
        }

        self.counters.passed.fetch_add(1, Ordering::Relaxed);
        Decision::Allow
    }

    /// Lässt die Namens-Detektoren laufen und trägt jeden Fund in den Trace.
    ///
    /// Gibt `Block` zurück, sobald einer davon auf `block` steht. Eingetragen
    /// werden **alle** Funde, auch die bloß gemeldeten: der Trace bildet ab,
    /// was passiert ist, und "frisch registriert *und* algorithmisch erzeugt"
    /// ist eine andere Aussage als jeder Teil für sich.
    fn inspect(
        &self,
        name: &str,
        query_type: hickory_proto::rr::RecordType,
        ctx: &mut Ctx,
    ) -> Decision {
        if self.detectors.is_empty() {
            return Decision::Allow;
        }
        let findings = self
            .detectors
            .inspect_name(&crate::detect::Observation { name, query_type });
        record_findings(&findings, ctx)
    }

    /// Prüft die Antwort — heute nur der Rebinding-Schutz.
    ///
    /// Läuft in [`PolicyBackend`] nach dem inneren Backend, weil es die Antwort
    /// braucht (ARCHITECTURE.md §1, Schicht 5).
    pub fn inspect_answer(&self, name: &str, response: &Message, ctx: &mut Ctx) -> Decision {
        if self.detectors.is_empty() {
            return Decision::Allow;
        }
        let findings = self.detectors.inspect_answer(name, response);
        let decision = record_findings(&findings, ctx);
        if decision == Decision::Block {
            ctx.record(Step::Synthesized { mode: self.mode });
            self.counters.blocked.fetch_add(1, Ordering::Relaxed);
        }
        decision
    }

    fn block(&self, ctx: &mut Ctx) -> Decision {
        Self::record_block(self, ctx)
    }

    fn record_block(&self, ctx: &mut Ctx) -> Decision {
        ctx.record(Step::Synthesized { mode: self.mode });
        self.counters.blocked.fetch_add(1, Ordering::Relaxed);
        Decision::Block
    }

    /// Baut die Antwort auf eine geblockte Anfrage.
    pub fn block_response(&self, request: &Message) -> Message {
        block::synthesize(request, self.mode, self.sinkhole_v4, self.sinkhole_v6)
    }

    pub fn stats(&self) -> PolicyStats {
        PolicyStats {
            blocked: self.counters.blocked.load(Ordering::Relaxed),
            allowed: self.counters.allowed.load(Ordering::Relaxed),
            passed: self.counters.passed.load(Ordering::Relaxed),
            entries: self.entries.load(Ordering::Relaxed),
        }
    }
}

fn normalize(name: &Name) -> Option<String> {
    let text = name.to_ascii();
    let trimmed = text.trim_end_matches('.');
    (!trimmed.is_empty()).then(|| trimmed.to_ascii_lowercase())
}

/// Die Policy-Konfiguration, getrennt von den geladenen Listen.
///
/// Beim Aktualisieren der Listen werden nur die Matcher ausgetauscht; Clients,
/// Zeitpläne und übersetzte Regex bleiben, wie sie sind.
#[derive(Debug)]
pub struct Blueprint {
    clients: Vec<clients::ClientRule>,
    default_policy: Arc<str>,
    policies: Vec<PolicyBlueprint>,
}

#[derive(Debug)]
pub struct PolicyBlueprint {
    pub name: Arc<str>,
    pub blocklists: Vec<Arc<str>>,
    pub allowlists: Vec<Arc<str>>,
    pub regex: Arc<RegexRules>,
    pub schedules: Vec<Schedule>,
}

#[derive(Debug, thiserror::Error)]
#[error("Policy '{policy}' verweist auf die Liste '{list}', die es nicht gibt")]
pub struct UnknownList {
    pub policy: String,
    pub list: String,
}

impl Blueprint {
    pub const fn new(
        clients: Vec<clients::ClientRule>,
        default_policy: Arc<str>,
        policies: Vec<PolicyBlueprint>,
    ) -> Self {
        Self {
            clients,
            default_policy,
            policies,
        }
    }

    /// Setzt Policies und geladene Listen zusammen.
    ///
    /// Ein Verweis auf eine unbekannte Liste ist ein Fehler und kein stilles
    /// Weglassen: sonst filtert eine Policy nach einem Tippfehler gar nicht mehr,
    /// ohne dass es jemand merkt (B.1 Regel 5).
    pub fn build(&self, lists: &LoadedLists) -> Result<PolicySet, UnknownList> {
        let mut policies = HashMap::with_capacity(self.policies.len());
        for blueprint in &self.policies {
            let resolve = |names: &[Arc<str>]| -> Result<Vec<Arc<Matcher>>, UnknownList> {
                names
                    .iter()
                    .map(|name| {
                        lists.get(name).ok_or_else(|| UnknownList {
                            policy: blueprint.name.to_string(),
                            list: name.to_string(),
                        })
                    })
                    .collect()
            };
            policies.insert(
                Arc::clone(&blueprint.name),
                Arc::new(Policy {
                    name: Arc::clone(&blueprint.name),
                    blocklists: resolve(&blueprint.blocklists)?,
                    allowlists: resolve(&blueprint.allowlists)?,
                    regex: Arc::clone(&blueprint.regex),
                    schedules: blueprint.schedules.clone(),
                }),
            );
        }

        Ok(PolicySet {
            clients: Clients::new(self.clients.clone(), Arc::clone(&self.default_policy)),
            policies,
            fallback: Arc::new(Policy {
                name: Arc::from("(keine)"),
                blocklists: Vec::new(),
                allowlists: Vec::new(),
                regex: Arc::new(RegexRules::default()),
                schedules: Vec::new(),
            }),
        })
    }
}

impl Blueprint {
    /// Baut die Policy-Struktur aus der Konfiguration.
    ///
    /// Sind keine Policies konfiguriert, entsteht eine namens `default`, die
    /// alle Listen benutzt. Damit verhält sich eine Konfiguration ohne
    /// `[[policy]]` genau wie vor Phase 5: eine Regel für alle.
    pub fn from_config(config: &crate::config::Config) -> Result<Self, ConfigError> {
        let all_lists = |lists: &[crate::config::ListConfig]| -> Vec<Arc<str>> {
            lists
                .iter()
                .filter(|list| list.enabled)
                .map(|list| Arc::from(list.name.as_str()))
                .collect()
        };

        let policies: Vec<PolicyBlueprint> = if config.policy.is_empty() {
            vec![PolicyBlueprint {
                name: Arc::from("default"),
                blocklists: all_lists(&config.blocklist),
                allowlists: all_lists(&config.allowlist),
                regex: Arc::new(RegexRules::default()),
                schedules: Vec::new(),
            }]
        } else {
            config
                .policy
                .iter()
                .map(|entry| {
                    let regex = RegexRules::compile(&entry.name, &entry.regex)
                        .map_err(|e| ConfigError::Invalid(e.to_string()))?;
                    let schedules = entry
                        .schedule
                        .iter()
                        .map(build_schedule)
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(PolicyBlueprint {
                        name: Arc::from(entry.name.as_str()),
                        blocklists: entry
                            .blocklists
                            .iter()
                            .map(|n| Arc::from(n.as_str()))
                            .collect(),
                        allowlists: entry
                            .allowlists
                            .iter()
                            .map(|n| Arc::from(n.as_str()))
                            .collect(),
                        regex: Arc::new(regex),
                        schedules,
                    })
                })
                .collect::<Result<Vec<_>, ConfigError>>()?
        };

        let client_rules = config
            .client
            .iter()
            .map(|entry| {
                let nets = entry
                    .matches
                    .ip
                    .iter()
                    .map(|text| crate::config::parse_net(text).map_err(ConfigError::Invalid))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(clients::ClientRule {
                    name: Arc::from(entry.name.as_str()),
                    nets,
                    policy: Arc::from(entry.policy.as_str()),
                })
            })
            .collect::<Result<Vec<_>, ConfigError>>()?;

        Ok(Self::new(client_rules, Arc::from("default"), policies))
    }
}

fn build_schedule(entry: &crate::config::ScheduleEntry) -> Result<Schedule, ConfigError> {
    let invalid = |what: &str| ConfigError::Invalid(format!("Zeitplan '{}': {what}", entry.name));
    let days = entry
        .days
        .iter()
        .map(|day| crate::config::parse_weekday(day).ok_or_else(|| invalid("unbekannter Tag")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Schedule {
        name: Arc::from(entry.name.as_str()),
        days,
        from: crate::config::parse_clock_time(&entry.from)
            .ok_or_else(|| invalid("from ist keine Uhrzeit"))?,
        to: crate::config::parse_clock_time(&entry.to)
            .ok_or_else(|| invalid("to ist keine Uhrzeit"))?,
        effect: match entry.action {
            crate::config::ScheduleAction::BlockAllExceptAllowlist => {
                crate::trace::ScheduleEffect::BlockAllExceptAllowlist
            }
        },
    })
}

/// Lädt die Listen regelmäßig neu und tauscht den Regelstand aus.
///
/// Liest Blueprint und Listenquellen je Zyklus aus [`crate::reload::PolicySource`],
/// damit ein Reload sofort greift statt auf den nächsten Refresh-Tick zu warten.
/// `wake` stößt genau diesen Zyklus an: der Reload signalisiert hier, sobald er
/// einen neuen Stand eingetauscht hat.
pub async fn run_updater<C: Clock + Clone, W: WallClock>(
    engine: Arc<Engine<C, W>>,
    source: Arc<crate::reload::PolicySource>,
    interval: std::time::Duration,
    wake: Arc<tokio::sync::Notify>,
    shutdown: tokio_util::sync::CancellationToken,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await; // der erste Tick kommt sofort
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = ticker.tick() => {}
            () = wake.notified() => {}
        }
        let blueprint = source.blueprint();
        let lists = source.lists();
        // Nicht strikt: ein Ausfall jetzt darf den laufenden Betrieb nicht
        // ungefiltert machen (B.1 Regel 6).
        match lists.load(false).await {
            Ok(loaded) => match blueprint.build(&loaded) {
                Ok(set) => {
                    let entries = loaded.total_entries();
                    engine.replace(set, entries);
                    tracing::info!(entries, "Listen aktualisiert");
                }
                Err(error) => tracing::error!(%error, "Regelstand nicht baubar, alter gilt weiter"),
            },
            Err(error) => tracing::error!(%error, "Aktualisierung fehlgeschlagen"),
        }
    }
}

/// Die Policy-Auswertung als Schicht vor dem Rest der Pipeline.
#[derive(Debug)]
pub struct PolicyBackend<B, C, W> {
    engine: Arc<Engine<C, W>>,
    inner: B,
}

impl<B: ResolveBackend, C: Clock + Clone, W: WallClock> PolicyBackend<B, C, W> {
    pub const fn new(engine: Arc<Engine<C, W>>, inner: B) -> Self {
        Self { engine, inner }
    }
}

impl<B: ResolveBackend, C: Clock + Clone, W: WallClock> ResolveBackend for PolicyBackend<B, C, W> {
    fn resolve(
        &self,
        request: &Message,
        ctx: &mut Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        // Die Adresse vorher herausziehen: `evaluate` braucht den Kontext
        // exklusiv, und `ctx.peer` im selben Ausdruck wäre ein zweiter Zugriff.
        let peer = ctx.peer.ip();
        let question = request.queries.first();
        let decision = question.map(|query| {
            self.engine
                .evaluate_typed(query.name(), query.query_type(), peer, ctx)
        });
        // Für den Rebinding-Schutz weiter unten, bevor der Borrow endet.
        let asked =
            question.map(|query| query.name().to_ascii().trim_end_matches('.').to_lowercase());

        async move {
            if decision == Some(Decision::Block) {
                // Kein Query-Name im Log (B.1 Regel 3). Die vollständige
                // Begründung steht im Trace; was damit geschieht, entscheidet
                // die Logging-Schicht in Phase 6.
                tracing::debug!("geblockt");
                return Ok(self.engine.block_response(request));
            }

            let response = self.inner.resolve(request, ctx).await?;

            // Post-Processing (ARCHITECTURE.md §1, Schicht 5): erst hier gibt
            // es eine Antwort, die der Rebinding-Schutz ansehen kann.
            if let Some(name) = asked
                && self.engine.inspect_answer(&name, &response, ctx) == Decision::Block
            {
                tracing::debug!("Antwort verworfen: private Adresse für einen öffentlichen Namen");
                return Ok(self.engine.block_response(request));
            }
            Ok(response)
        }
    }
}

/// Trägt alle Funde in den Trace und sagt, ob einer davon blockt.
fn record_findings(
    findings: &[(crate::detect::Finding, crate::detect::Action)],
    ctx: &mut Ctx,
) -> Decision {
    let mut blocked = false;
    for (finding, action) in findings {
        crate::detect::record(finding.detector);
        ctx.record(Step::Detected {
            detector: finding.detector,
            score: finding.score,
            reason: Arc::clone(&finding.reason),
            action: *action,
        });
        blocked |= action.blocks();
    }
    if blocked {
        Decision::Block
    } else {
        Decision::Allow
    }
}

/// Kurzform für Log-Zeilen: welcher Schritt hat die Entscheidung getragen.
pub fn deciding_step(steps: &[Step]) -> Option<&Step> {
    steps.iter().rev().find(|step| {
        matches!(
            step,
            Step::BlocklistHit { .. }
                | Step::AllowlistHit { .. }
                | Step::RegexHit { .. }
                | Step::ScheduleHit { .. }
                | Step::TemporaryAllow { .. }
                | Step::TemporaryDeny { .. }
                | Step::Detected { .. }
        )
    })
}
