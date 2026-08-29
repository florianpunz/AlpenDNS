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
use crate::filter::block::{self, BlockMode};
use crate::filter::matcher::Matcher;
use crate::filter::{Lists, LoadedLists};
use crate::resolve::{ResolveBackend, ResolveError};
use crate::trace::{Ctx, Step};
use clients::Clients;
use rules::RegexRules;
use schedule::Schedule;
use temporary::TemporaryAllows;

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
    temporary: TemporaryAllows<C>,
    wall: W,
    mode: BlockMode,
    sinkhole_v4: std::net::Ipv4Addr,
    sinkhole_v6: std::net::Ipv6Addr,
    entries: std::sync::atomic::AtomicUsize,
    counters: Counters,
}

impl<C: Clock, W: WallClock> Engine<C, W> {
    pub fn new(set: PolicySet, entries: usize, config: &BlockingConfig, clock: C, wall: W) -> Self {
        Self {
            set: ArcSwap::from_pointee(set),
            temporary: TemporaryAllows::new(clock),
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
    pub const fn temporary(&self) -> &TemporaryAllows<C> {
        &self.temporary
    }

    /// Wertet eine Anfrage aus und schreibt die Begründung in den Kontext.
    ///
    /// **Reihenfolge** (ARCHITECTURE.md §1): befristete Freigabe → Allowlist →
    /// Blocklisten → Regex → Zeitplan. Die Allowlist steht vorn, weil sie sonst
    /// keinen Zweck hätte: sie ist das Mittel, einen Fehlalarm einer fremden
    /// Liste zu übersteuern.
    pub fn evaluate(&self, name: &Name, peer: IpAddr, ctx: &Ctx) -> Decision {
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

        // Der Zeitplan steht zuletzt: wer bis hierher gekommen ist, steht auf
        // keiner Allowlist — und genau das ist die Bedingung von
        // `block_all_except_allowlist`.
        if let Some(active) = schedule::first_active(&policy.schedules, &self.wall.now()) {
            ctx.record(Step::ScheduleHit {
                schedule: Arc::clone(&active.name),
                effect: active.effect,
            });
            return self.block(ctx);
        }

        self.counters.passed.fetch_add(1, Ordering::Relaxed);
        Decision::Allow
    }

    fn block(&self, ctx: &Ctx) -> Decision {
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
pub async fn run_updater<C: Clock, W: WallClock>(
    engine: Arc<Engine<C, W>>,
    blueprint: Arc<Blueprint>,
    lists: Arc<Lists>,
    interval: std::time::Duration,
    shutdown: tokio_util::sync::CancellationToken,
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

impl<B: ResolveBackend, C: Clock, W: WallClock> PolicyBackend<B, C, W> {
    pub const fn new(engine: Arc<Engine<C, W>>, inner: B) -> Self {
        Self { engine, inner }
    }
}

impl<B: ResolveBackend, C: Clock, W: WallClock> ResolveBackend for PolicyBackend<B, C, W> {
    fn resolve(
        &self,
        request: &Message,
        ctx: &Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let decision = request
            .queries
            .first()
            .map(|query| self.engine.evaluate(query.name(), ctx.peer.ip(), ctx));

        async move {
            if decision == Some(Decision::Block) {
                // Kein Query-Name im Log (B.1 Regel 3). Die vollständige
                // Begründung steht im Trace; was damit geschieht, entscheidet
                // die Logging-Schicht in Phase 6.
                tracing::debug!("geblockt");
                return Ok(self.engine.block_response(request));
            }
            self.inner.resolve(request, ctx).await
        }
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
        )
    })
}
