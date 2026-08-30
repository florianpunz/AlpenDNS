//! Binary: Argumente, Konfiguration, Laufzeit, Signale, Shutdown.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use alpendns::api::{ApiState, HistoryView, ListInfo, PolicyInfo, StatusSource};
use alpendns::caching::CachingBackend;
use alpendns::clock::{SystemClock, SystemWallClock};
use alpendns::config::{Config, ListConfig};
use alpendns::filter::Lists;
use alpendns::filter::source::{ListSpec, Loader, Source};
use alpendns::history::{History, Sample};
use alpendns::logging::QueryLog;
use alpendns::policy::{Blueprint, Engine, Explanation, PolicyBackend};
use alpendns::privacy;
use alpendns::ratelimit::RateLimiter;
use alpendns::router::ZoneRouter;
use alpendns::server::Server;
use alpendns::upstream::odoh::{OdohBackend, OdohTransport};
use alpendns::upstream::pool::{Pool, Upstream};
use alpendns::upstream::transport::Transport;
use alpendns::upstream::{Encrypted, ForwardBackend};
use anyhow::Context as _;
use tokio_util::sync::CancellationToken;

/// Abstand, in dem die Cache-Bilanz im Log erscheint.
///
/// Fünf Minuten sind ein Kompromiss: oft genug, um beim Zusehen etwas zu sehen,
/// selten genug, dass ein Dauerbetrieb das Log nicht zumüllt. Ein abfragbarer
/// Endpunkt dafür kommt in Phase 6.
const STATS_INTERVAL: Duration = Duration::from_secs(300);

const USAGE: &str = "alpendns — privacy-fokussierter DNS-Server

Aufruf:
  alpendns -c <datei>
      Server mit dieser Konfiguration starten

  alpendns -c <datei> check
      Prüft die Konfiguration und die Verzeichnisse, ohne den Server zu
      starten. Exit 0 heißt: dieser Start wird nicht an der Konfiguration
      scheitern. Läuft als ExecStartPre in der systemd-Unit.

  alpendns -c <datei> policy test <domain> [--client <name>]
      Zeigt, wie diese Domain für diesen Client entschieden würde, samt
      vollständiger Begründung. Ohne --client gilt die Default-Policy.

Optionen:
  -c, --config <datei>   Pfad zur Konfigurationsdatei (Pflicht)
  -h, --help             Diese Hilfe
  -V, --version          Version ausgeben
";

enum Args {
    Run(PathBuf),
    Check(PathBuf),
    PolicyTest {
        config: PathBuf,
        domain: String,
        client: Option<String>,
    },
    Print(String),
}

fn parse_args() -> anyhow::Result<Args> {
    let mut config = None;
    let mut rest: Vec<String> = Vec::new();
    let mut client = None;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Args::Print(USAGE.to_owned())),
            "-V" | "--version" => {
                return Ok(Args::Print(format!(
                    "alpendns {}\n",
                    env!("CARGO_PKG_VERSION")
                )));
            }
            "-c" | "--config" => {
                config = Some(PathBuf::from(
                    args.next()
                        .context("-c/--config erwartet einen Pfad als Argument")?,
                ));
            }
            "--client" => {
                client = Some(
                    args.next()
                        .context("--client erwartet einen Client-Namen")?,
                );
            }
            other if other.starts_with('-') => {
                anyhow::bail!("unbekannte Option '{other}'\n\n{USAGE}")
            }
            other => rest.push(other.to_owned()),
        }
    }

    let config = config.context(format!("keine Konfiguration angegeben\n\n{USAGE}"))?;
    match rest.as_slice() {
        [] => Ok(Args::Run(config)),
        [command] if command == "check" => Ok(Args::Check(config)),
        [command, action, domain] if command == "policy" && action == "test" => {
            Ok(Args::PolicyTest {
                config,
                domain: domain.clone(),
                client,
            })
        }
        _ => anyhow::bail!("unbekanntes Unterkommando\n\n{USAGE}"),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Vor dem Start steht der Logger womöglich noch nicht, deshalb hier
            // bewusst auf stderr statt über `tracing`.
            eprintln!("Fehler: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let path = match parse_args()? {
        Args::Print(text) => {
            print!("{text}");
            return Ok(());
        }
        Args::Check(config) => return check(&config),
        Args::PolicyTest {
            config,
            domain,
            client,
        } => return policy_test(&config, &domain, client.as_deref()),
        Args::Run(path) => path,
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::load(&path)?;
    // Vor dem Zerlegen der Konfiguration: der Blueprint liest quer über
    // Clients, Policies und Listennamen.
    let blueprint = Arc::new(Blueprint::from_config(&config)?);
    let client_count = config.client.len();
    let policy_count = config.policy.len();
    // Für die API zusammenstellen, solange die Konfiguration noch vollständig ist.
    let policy_infos: Vec<PolicyInfo> = config
        .policy
        .iter()
        .map(|entry| PolicyInfo {
            name: entry.name.clone(),
            blocklists: entry.blocklists.clone(),
            allowlists: entry.allowlists.clone(),
            regex: entry.regex.len(),
            schedules: entry.schedule.iter().map(|s| s.name.clone()).collect(),
            clients: config
                .client
                .iter()
                .filter(|client| client.policy == entry.name)
                .map(|client| client.name.clone())
                .collect(),
        })
        .collect();
    // Client-Name → Adresse, solange die Konfiguration noch vollständig ist.
    // Ausgewertet wird mit der ersten Adresse eines Clients — dieselbe Wahl wie
    // bei `alpendns policy test`, damit UI und Kommandozeile dasselbe sagen.
    let client_addrs: std::collections::HashMap<String, std::net::IpAddr> = config
        .client
        .iter()
        .filter_map(|entry| {
            let address = entry.matches.ip.first()?;
            let net = alpendns::config::parse_net(address).ok()?;
            Some((entry.name.clone(), net.addr()))
        })
        .collect();

    // Die Detektoren brauchen die forward_zone-Einträge (für den
    // Rebinding-Schutz) und damit die noch vollständige Konfiguration.
    let detectors = alpendns::detect::from_config(
        &config.detection,
        config.rebinding_allow_zones(),
        SystemClock,
        SystemWallClock,
    );
    let detector_settings = detectors.configured();

    let server_config = config.server;
    let cache_config = config.cache;
    let privacy_config = config.privacy;
    let privacy = privacy::Settings::from(&privacy_config);
    let api_config = config.api;
    let metrics_config = config.metrics;
    let timeout = server_config.query_timeout;

    // Ein Pool; mehrere auszuwählen ist Sache der Policies in Phase 5.
    let pool_config = config
        .upstream_pool
        .into_iter()
        .next()
        .context("Konfiguration ohne Upstream-Pool")?;
    let tls = Arc::new(Transport::default_tls_config()?);
    let upstream_names: Vec<String> = pool_config
        .resolver
        .iter()
        .map(|resolver| format!("{} ({})", resolver.name, resolver.addr.scheme()))
        .collect();
    // Mit ODoH geht jede Anfrage über den konfigurierten Proxy; die Validierung
    // hat schon sichergestellt, dass dann alle Resolver doh:// sprechen.
    let odoh_proxy = privacy_config
        .odoh
        .enabled
        .then(|| privacy_config.odoh.proxy.clone())
        .flatten();
    let upstreams: Vec<Upstream<Encrypted>> = pool_config
        .resolver
        .iter()
        .map(|resolver| -> anyhow::Result<Upstream<Encrypted>> {
            // Die Validierung stellt sicher, dass hier ein Name steht.
            let tls_name = resolver.tls_name.as_deref().unwrap_or_default();
            let backend = match (&odoh_proxy, &resolver.addr) {
                (Some(proxy), alpendns::config::UpstreamAddr::Doh { addr, path }) => {
                    Encrypted::Oblivious(OdohBackend::new(
                        OdohTransport::new(
                            proxy.clone(),
                            tls_name,
                            path,
                            *addr,
                            alpendns::upstream::odoh::well_known_config_url(tls_name),
                            timeout,
                            privacy,
                        )
                        .with_context(|| {
                            format!("ODoH-Transport für Resolver '{}'", resolver.name)
                        })?,
                    ))
                }
                _ => Encrypted::Direct(Transport::new(
                    resolver.addr.clone(),
                    tls_name,
                    timeout,
                    privacy,
                    Arc::clone(&tls),
                )),
            };
            Ok(Upstream::new(
                resolver.name.clone(),
                // Der Transport heißt in Statistik und Metrik weiterhin `doh`;
                // ob er über einen Proxy geht, ist eine Eigenschaft der
                // Konfiguration und steht dort, nicht je Upstream.
                resolver.addr.scheme(),
                backend,
            ))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let strategy = pool_config.strategy;
    let seed_rotation = pool_config.seed_rotation;
    let zones: Vec<(hickory_proto::rr::Name, ForwardBackend)> = config
        .forward_zone
        .iter()
        .map(|zone| {
            (
                zone.zone.0.clone(),
                ForwardBackend::new(zone.upstream.socket_addr(), timeout, privacy),
            )
        })
        .collect();
    let zone_names: Vec<String> = config
        .forward_zone
        .iter()
        .map(|zone| zone.zone.0.to_string())
        .collect();

    // Der Blueprint muss vor dem Zerlegen der Konfiguration gebaut werden.
    let blocking_config = config.blocking;
    let specs = to_specs(&config.blocklist, &config.allowlist);
    // Name → Format, um die geladenen Listen später der Metrik zuzuordnen.
    let list_formats: std::collections::HashMap<String, alpendns::filter::parser::Format> = specs
        .iter()
        .map(|spec| (spec.name.clone(), spec.format))
        .collect();
    // Kürzestes Intervall aller Listen; Details in filter::run_updater.
    let refresh = config
        .blocklist
        .iter()
        .chain(config.allowlist.iter())
        .filter(|list| list.enabled)
        .map(|list| list.refresh)
        .min()
        .unwrap_or(Duration::from_secs(24 * 60 * 60));
    let lists = Arc::new(Lists::new(
        Loader::new(blocking_config.cache_dir.clone())?,
        specs,
    ));

    // Die Runtime wird von Hand gebaut statt über #[tokio::main]: ein Fehler
    // beim Start soll eine Fehlermeldung geben, kein Panic (CLAUDE.md B.1).
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio-Runtime konnte nicht gestartet werden")?;

    runtime.block_on(async move {
        // Von außen nach innen: Cache → Zonen-Weiche → Pool bzw. LAN-Server.
        // Jede Schicht ist ein ResolveBackend, keine kennt die anderen.
        let pool = Arc::new(Pool::new(upstreams, strategy, SystemClock));
        let query_log = Arc::new(
            QueryLog::new(&privacy_config.logging).context("Query-Log konnte nicht geöffnet werden")?,
        );

        // Erststart ist strikt: lieber gar kein DNS als ungefiltertes DNS
        // (B.1 Regel 6). Spätere Ausfälle behandelt run_updater nachsichtig.
        let loaded = lists
            .load(true)
            .await
            .context("Blocklisten konnten beim Start nicht geladen werden")?;
        let entries = loaded.total_entries();
        let list_infos: Vec<ListInfo> = loaded
            .names()
            .map(|name| ListInfo {
                name: name.to_string(),
                entries: loaded.get(name).map_or(0, |matcher| matcher.len()),
                // Nur *geladene* Listen zählen; eine, die nicht erreichbar war,
                // steht in der Konfiguration, aber nicht in der Metrik.
                format: list_formats
                    .get(name.as_ref())
                    .map_or_else(|| "?".to_owned(), ToString::to_string),
            })
            .collect();
        let engine = Arc::new(
            Engine::new(
                blueprint.build(&loaded)?,
                entries,
                &blocking_config,
                SystemClock,
                SystemWallClock,
            )
            .with_detectors(detectors),
        );

        // Von außen nach innen: Filter → Cache → Zonen-Weiche → Pool.
        // Gefiltert wird vor dem Cache, damit dieser die ungefilterte Antwort
        // hält und alle Clients sie teilen können (ARCHITECTURE.md §4).
        let caching = CachingBackend::new(
            ZoneRouter::new(zones, Arc::clone(&pool)),
            &cache_config,
            SystemClock,
        );
        // Handle auf den Cache behalten, bevor das Backend in den Server wandert:
        // beim Herunterfahren soll die Trefferquote im Log stehen. Ein
        // Metrik-Endpunkt dafür kommt in Phase 6.
        let cache = Arc::clone(caching.cache());
        let backend = PolicyBackend::new(Arc::clone(&engine), caching);
        // Pflicht, bevor der Server irgendwo lauscht, wo er nicht nur sein
        // eigenes LAN sieht (CLAUDE.md B.5). Abschaltbar, aber per Default an.
        let limiter = RateLimiter::from_config(
            &server_config.rate_limit,
            Arc::new(SystemClock) as Arc<dyn alpendns::clock::Clock>,
        );
        let bound = Server::new(
            backend,
            server_config.edns.udp_payload_size,
            Arc::clone(&query_log),
        )
            .with_rate_limit(limiter.clone())
            .bind(&server_config)
            .await
            .context("Listener konnten nicht geöffnet werden")?;

        tracing::info!(
            udp = ?bound.udp_addrs(),
            tcp = ?bound.tcp_addrs(),
            upstreams = ?upstream_names,
            strategy = ?strategy,
            seed_rotation = ?seed_rotation,
            forward_zones = ?zone_names,
            cache_entries = cache_config.max_entries,
            serve_stale = cache_config.serve_stale,
            list_entries = entries,
            clients = client_count,
            policies = policy_count,
            blocking = ?blocking_config.mode,
            logging = ?privacy_config.logging.mode,
            dnssec = privacy.dnssec,
            odoh = odoh_proxy.is_some(),
            detectors = ?detector_settings
                .iter()
                .map(|(detector, action)| format!("{}={}", detector.as_str(), action.as_str()))
                .collect::<Vec<_>>(),
            rate_limit = limiter.as_ref().map_or_else(
                || "aus".to_owned(),
                |limiter| {
                    let (rate, burst) = limiter.limits();
                    format!("{rate:.0}/s, Burst {burst:.0}")
                }
            ),
            "AlpenDNS gestartet"
        );

        let shutdown = CancellationToken::new();

        // Der Seed von split_by_zone wird regelmäßig neu gezogen, damit kein
        // Anbieter über die Zeit ein stabiles Bild lernt (FEATURES.md P2).
        tokio::spawn(alpendns::upstream::pool::run_seed_rotation(
            Arc::clone(&pool),
            seed_rotation,
            shutdown.clone(),
        ));
        tokio::spawn(report_stats(
            Arc::clone(&cache),
            Arc::clone(&pool),
            Arc::clone(&engine),
            shutdown.clone(),
        ));
        // API und Metriken sind zwei Listener, weil sie verschiedene Zielgruppen
        // haben: die API zeigt Namen und braucht einen Token, die Metriken
        // enthalten keine und werden von einem Scraper ohne Token abgeholt.
        let history = Arc::new(Mutex::new(History::new(std::time::Instant::now())));
        let source: Arc<dyn StatusSource> = Arc::new(Runtime {
            cache: Arc::clone(&cache),
            pool: Arc::clone(&pool),
            engine: Arc::clone(&engine),
            log: Arc::clone(&query_log),
            lists: list_infos.clone(),
            policies: policy_infos.clone(),
            blocking_mode: blocking_config.mode,
            seed_rotation,
            dnssec: privacy.dnssec,
            started: std::time::Instant::now(),
            history: Arc::clone(&history),
            client_addrs: client_addrs.clone(),
            limiter: limiter.clone(),
        });
        tokio::spawn(sample_history(
            Arc::clone(&source),
            history,
            shutdown.clone(),
        ));

        if api_config.enabled {
            let token = read_or_create_token(&api_config.token_file)?;
            let state = ApiState::new(
                Arc::clone(&source),
                Arc::clone(&query_log),
                Arc::from(token.as_str()),
            );
            let listener = tokio::net::TcpListener::bind(api_config.listen)
                .await
                .with_context(|| format!("API-Listener auf {} ", api_config.listen))?;
            tracing::info!(listen = %api_config.listen, token_file = %api_config.token_file.display(), "API und Web-UI");
            let router = alpendns::api::router(state);
            let signal = shutdown.clone();
            tokio::spawn(async move {
                let _ = axum::serve(listener, router)
                    .with_graceful_shutdown(async move { signal.cancelled().await })
                    .await;
            });
        }

        if metrics_config.enabled {
            let state = ApiState::new(
                Arc::clone(&source),
                Arc::clone(&query_log),
                Arc::from(""),
            );
            let listener = tokio::net::TcpListener::bind(metrics_config.listen)
                .await
                .with_context(|| format!("Metrik-Listener auf {}", metrics_config.listen))?;
            tracing::info!(listen = %metrics_config.listen, path = %metrics_config.path, "Metriken");
            let router = alpendns::api::metrics_router(state, &metrics_config.path);
            let signal = shutdown.clone();
            tokio::spawn(async move {
                let _ = axum::serve(listener, router)
                    .with_graceful_shutdown(async move { signal.cancelled().await })
                    .await;
            });
        }

        tokio::spawn(alpendns::policy::run_updater(
            Arc::clone(&engine),
            Arc::clone(&blueprint),
            Arc::clone(&lists),
            refresh,
            shutdown.clone(),
        ));

        let signals = shutdown.clone();
        tokio::spawn(async move {
            wait_for_signal().await;
            tracing::info!("Signal empfangen, laufende Anfragen werden noch beantwortet");
            signals.cancel();
        });

        bound.run(shutdown).await;
        log_stats(&cache, &pool, &engine, "Cache-Bilanz");
        tracing::info!("beendet");
        Ok(())
    })
}

/// Schreibt die Cache-Bilanz regelmäßig ins Log, solange sich etwas getan hat.
///
/// Nur Summen, keine Namen — das ist unabhängig vom Log-Modus zulässig
/// (CLAUDE.md B.1, Regel 3). Ohne Verkehr wird nichts geschrieben, damit ein
/// Server im Leerlauf still bleibt.
/// Übersetzt die Konfiguration in das, was der Loader braucht.
///
/// Abgeschaltete Listen fallen hier heraus; die Validierung hat schon
/// sichergestellt, dass genau eine Quelle angegeben ist.
/// Zählt die geladenen Listen je Format, häufigstes zuerst.
///
/// Für die Metrik `alpendns_lists`: sie soll belegen, welche Formate im Betrieb
/// tatsächlich vorkommen — die Frage vor jeder weiteren Streichung.
fn count_formats(lists: &[ListInfo]) -> Vec<(String, u64)> {
    let mut counted: std::collections::HashMap<&str, u64> = std::collections::HashMap::new();
    for list in lists {
        *counted.entry(list.format.as_str()).or_default() += 1;
    }
    let mut counted: Vec<(String, u64)> = counted
        .into_iter()
        .map(|(format, count)| (format.to_owned(), count))
        .collect();
    counted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counted
}

fn to_specs(blocklists: &[ListConfig], allowlists: &[ListConfig]) -> Vec<ListSpec> {
    blocklists
        .iter()
        .chain(allowlists.iter())
        .filter(|list| list.enabled)
        .filter_map(|list| {
            let source = match (&list.url, &list.path) {
                (Some(url), _) => Source::Url(url.clone()),
                (None, Some(path)) => Source::File(path.clone()),
                (None, None) => return None,
            };
            Some(ListSpec {
                name: list.name.clone(),
                source,
                format: list.format,
            })
        })
        .collect()
}

async fn report_stats<C: alpendns::clock::Clock>(
    cache: Arc<alpendns::cache::Cache<C>>,
    pool: Arc<Pool<Encrypted, SystemClock>>,
    engine: Arc<PolicyEngine>,
    shutdown: CancellationToken,
) {
    let mut ticker = tokio::time::interval(STATS_INTERVAL);
    // Der erste Tick kommt sofort; den wollen wir nicht.
    ticker.tick().await;
    let mut last = cache.stats();
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let current = cache.stats();
        if current != last {
            log_stats(&cache, &pool, &engine, "Cache");
            last = current;
        }
    }
}

fn log_stats<C: alpendns::clock::Clock>(
    cache: &alpendns::cache::Cache<C>,
    pool: &Pool<Encrypted, SystemClock>,
    engine: &PolicyEngine,
    message: &'static str,
) {
    let stats = cache.stats();
    tracing::info!(
        hits = stats.hits,
        stale_hits = stats.stale_hits,
        misses = stats.misses,
        entries = cache.len(),
        hit_rate = format!("{:.1} %", stats.hit_rate() * 100.0),
        "{message}"
    );
    let filtered = engine.stats();
    tracing::info!(
        blocked = filtered.blocked,
        allowed = filtered.allowed,
        passed = filtered.passed,
        entries = filtered.entries,
        "Policy"
    );
    for upstream in pool.stats() {
        tracing::info!(
            upstream = %upstream.name,
            ok = upstream.successes,
            failed = upstream.failures,
            rtt = ?upstream.rtt,
            down = upstream.down,
            "Upstream"
        );
    }
}

/// Kurzform für den Engine-Typ, wie ihn das Binary benutzt.
type PolicyEngine = Engine<SystemClock, SystemWallClock>;

/// Was die API vom laufenden Server sieht.
///
/// Bündelt die konkreten Typen, damit das API-Modul nicht über Cache-, Uhr- und
/// Backend-Typ generisch sein muss.
struct Runtime {
    cache: Arc<alpendns::cache::Cache<SystemClock>>,
    pool: Arc<Pool<Encrypted, SystemClock>>,
    engine: Arc<PolicyEngine>,
    log: Arc<QueryLog>,
    lists: Vec<ListInfo>,
    policies: Vec<PolicyInfo>,
    blocking_mode: alpendns::filter::block::BlockMode,
    /// Abstand der Seed-Rotation aus der Konfiguration.
    seed_rotation: Duration,
    /// Ob die Signaturkette selbst nachgerechnet wird.
    dnssec: bool,
    started: std::time::Instant,
    /// Die Zeitreihe der letzten 24 Stunden. Hinter einem Mutex, aber außerhalb
    /// des Anfragepfads: hier schreibt nur der Sampler alle 30 Sekunden.
    history: Arc<Mutex<History>>,
    /// Client-Name → erste konfigurierte Adresse, für `/api/explain`.
    /// Dieselbe Auswahl wie bei `alpendns policy test`.
    client_addrs: std::collections::HashMap<String, std::net::IpAddr>,
    /// Fehlt, wenn die Drosselung abgeschaltet ist.
    limiter: Option<Arc<RateLimiter>>,
}

impl StatusSource for Runtime {
    fn snapshot(&self) -> alpendns::metrics::Snapshot {
        alpendns::metrics::Snapshot {
            log: self.log.stats(),
            cache: self.cache.stats(),
            cache_entries: self.cache.len(),
            policy: self.engine.stats(),
            upstreams: self.pool.stats(),
            uptime: self.started.elapsed(),
            blocking_mode: self.blocking_mode,
            logging_mode: self.log.mode(),
            list_formats: count_formats(&self.lists),
            privacy: alpendns::privacy::counters(),
            aggregate_k: self.log.aggregate_k(),
            zone_seed_rotation: self.seed_rotation,
            zone_seed_rotations: self.pool.rotations(),
            dnssec_enabled: self.dnssec,
            dnssec: alpendns::dnssec::counters(),
            detectors: self.engine.detectors(),
            detections: alpendns::detect::counters(),
            below_threshold_queries: self.log.top(0).below_threshold_queries,
            rate_limit: self.limiter.as_ref().map(|limiter| {
                let (rate, burst) = limiter.limits();
                alpendns::metrics::RateLimitStats {
                    per_client_qps: rate,
                    burst,
                    throttled: limiter.throttled(),
                    tracked: limiter.tracked(),
                }
            }),
        }
    }

    fn lists(&self) -> Vec<ListInfo> {
        self.lists.clone()
    }

    fn policies(&self) -> Vec<PolicyInfo> {
        self.policies.clone()
    }

    fn grant(&self, domain: &str, ttl: Duration) {
        self.engine.temporary().grant(domain, ttl);
    }

    fn revoke(&self, domain: &str) {
        self.engine.temporary().revoke(domain);
    }

    fn deny(&self, domain: &str, ttl: Duration) {
        self.engine.denied().grant(domain, ttl);
    }

    fn undeny(&self, domain: &str) {
        self.engine.denied().revoke(domain);
    }

    fn denials(&self) -> Vec<(String, Duration)> {
        self.engine.denied().active()
    }

    fn grants(&self) -> Vec<(String, Duration)> {
        self.engine.temporary().active()
    }

    fn history(&self) -> HistoryView {
        let history = self.history.lock().unwrap_or_else(PoisonError::into_inner);
        HistoryView {
            bucket_seconds: history.bucket_seconds(),
            upstreams: history.upstreams().to_vec(),
            buckets: history.buckets(),
        }
    }

    fn explain(&self, domain: &str, client: Option<&str>) -> Result<Explanation, String> {
        let peer = match client {
            None => std::net::IpAddr::from([127, 0, 0, 1]),
            Some(name) => *self
                .client_addrs
                .get(name)
                .ok_or_else(|| format!("kein Client namens '{name}'"))?,
        };
        alpendns::policy::explain(&self.engine, domain, peer)
    }
}

/// Schreibt alle 30 Sekunden den Zählerstand in die Zeitreihe.
///
/// Gemessen wird der Snapshot, nicht die einzelne Anfrage: die Zeitreihe kann
/// dadurch gar keinen Namen sehen, auch nicht versehentlich. 30 Sekunden sind
/// zehnmal feiner als ein Eimer — genug, dass ein Neustart des Samplers oder
/// ein verpasster Tick die Kurve nicht sichtbar verbiegt.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

async fn sample_history(
    source: Arc<dyn StatusSource>,
    history: Arc<Mutex<History>>,
    shutdown: CancellationToken,
) {
    let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = ticker.tick() => {}
        }
        let snapshot = source.snapshot();
        let names: Vec<String> = snapshot
            .upstreams
            .iter()
            .map(|upstream| upstream.name.clone())
            .collect();
        let sample = Sample {
            queries: snapshot.log.queries,
            blocked: snapshot.log.blocked,
            cache_hits: snapshot
                .cache
                .hits
                .saturating_add(snapshot.cache.stale_hits),
            cache_misses: snapshot.cache.misses,
            upstreams: snapshot
                .upstreams
                .iter()
                .map(|upstream| upstream.successes)
                .collect(),
        };
        history
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .observe(sample, &names, std::time::Instant::now());
    }
}

/// Liest den API-Token oder legt einen an.
///
/// Ohne diesen Schritt müsste vor dem ersten Start jemand von Hand eine Datei
/// mit Zufallszeichen anlegen, nur um die UI überhaupt zu sehen. Die Datei
/// bekommt Rechte 0600 — sie ist das Passwort.
fn read_or_create_token(path: &std::path::Path) -> anyhow::Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let trimmed = existing.trim().to_owned();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    let token: String = (0..32)
        .map(|_| {
            let byte: u8 = rand::random_range(0..16);
            char::from_digit(u32::from(byte), 16).unwrap_or('0')
        })
        .collect();

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Verzeichnis für {} anlegen", path.display()))?;
    }
    std::fs::write(path, format!("{token}\n"))
        .with_context(|| format!("Token nach {} schreiben", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Rechte für {} setzen", path.display()))?;
    }
    tracing::info!(path = %path.display(), "neuen API-Token erzeugt");
    Ok(token)
}

/// `alpendns check`
///
/// Läuft als `ExecStartPre` vor jedem Start (ROADMAP Phase 9, Schritt 3). Der
/// Sinn ist nicht die Ausgabe, sondern der Exit-Code: schlägt die Prüfung fehl,
/// startet systemd den neuen Prozess gar nicht erst — und die alte Instanz
/// läuft weiter, statt beim Neustart über eine kaputte Datei zu stolpern.
///
/// Geprüft wird ausdrücklich **nichts, was Netz braucht**: kein Listen-Download,
/// keine Upstream-Verbindung. Ein Startskript, das auf das Internet wartet, ist
/// ein Startskript, das irgendwann hängt.
fn check(path: &std::path::Path) -> anyhow::Result<()> {
    let config = Config::load(path)?;
    // Der Blueprint liest quer über Clients, Policies und Listennamen; ohne ihn
    // fiele ein Verweis auf eine unbekannte Liste erst beim Start auf.
    Blueprint::from_config(&config)?;

    println!("Konfiguration: {}", path.display());
    println!(
        "  Listener:    UDP {:?}, TCP {:?}",
        config.server.listen_udp, config.server.listen_tcp
    );
    println!(
        "  Upstreams:   {}",
        config
            .upstream_pool
            .iter()
            .flat_map(|pool| pool.resolver.iter())
            .map(|resolver| format!("{} ({})", resolver.name, resolver.addr.scheme()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  Listen:      {} Blocklisten, {} Allowlisten",
        config.blocklist.iter().filter(|list| list.enabled).count(),
        config.allowlist.iter().filter(|list| list.enabled).count()
    );
    println!(
        "  Policies:    {} für {} Clients",
        config.policy.len(),
        config.client.len()
    );
    println!(
        "  Blocken:     {:?}, Logging: {:?}",
        config.blocking.mode, config.privacy.logging.mode
    );
    let limit = &config.server.rate_limit;
    if limit.enabled {
        println!(
            "  Drosselung:  {} Anfragen/s je Client, Burst {}",
            limit.per_client_qps, limit.burst
        );
    } else {
        println!("  Drosselung:  aus");
    }

    // Verzeichnisse: die häufigste Ursache für einen Start, der an der
    // Konfiguration nicht scheitert und trotzdem nicht funktioniert. Nach einem
    // Upgrade gehört ein Verzeichnis schnell wieder root statt dem Dienstuser.
    let mut problems: Vec<String> = Vec::new();
    writable(
        &config.blocking.cache_dir,
        "blocking.cache_dir",
        &mut problems,
    );
    if config.api.enabled
        && let Some(parent) = config.api.token_file.parent()
    {
        writable(parent, "Verzeichnis von api.token_file", &mut problems);
    }
    // Nur im Modus `full` wird überhaupt eine Datei geschrieben.
    if config.privacy.logging.mode == alpendns::logging::Mode::Full
        && let Some(parent) = config.privacy.logging.path.parent()
    {
        writable(
            parent,
            "Verzeichnis von privacy.logging.path",
            &mut problems,
        );
    }
    if !problems.is_empty() {
        anyhow::bail!("{}", problems.join("\n"));
    }

    // Kein Fehler, sondern ein Hinweis: wer bewusst öffentlich lauscht, hat
    // sich dafür entschieden. Ungefragt darf das nur nicht passieren
    // (ROADMAP Phase 9, Schritt 6).
    let public = config.public_listeners();
    if public.is_empty() {
        println!("\nOK — erreichbar nur aus dem eigenen Netz.");
    } else {
        println!("\nOK — mit einem Hinweis:");
        for addr in &public {
            println!(
                "  {addr} ist nicht auf eine private Adresse beschränkt. Wenn dieser Port \n\
                 aus dem Internet erreichbar ist, ist der Server ein offener Resolver."
            );
        }
        if !config.server.rate_limit.enabled {
            println!(
                "  Dazu steht server.rate_limit.enabled auf false. Ein offener Resolver \n\
                 ohne Drosselung ist ein Amplification-Reflektor (CLAUDE.md B.5)."
            );
        }
    }
    Ok(())
}

/// Prüft, ob in dieses Verzeichnis geschrieben werden kann.
///
/// Geprüft wird durch Hinschreiben, nicht durch Rechte-Rechnen: die effektiven
/// Rechte hängen an User, Gruppen, ACLs und den systemd-Direktiven zusammen,
/// und die einzige Antwort, auf die es ankommt, ist die des Kernels.
fn writable(dir: &std::path::Path, label: &str, problems: &mut Vec<String>) {
    if !dir.is_dir() {
        problems.push(format!(
            "{label}: {} gibt es nicht (oder ist kein Verzeichnis)",
            dir.display()
        ));
        return;
    }
    let probe = dir.join(".alpendns-check");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
        }
        Err(error) => problems.push(format!(
            "{label}: in {} kann nicht geschrieben werden ({error})",
            dir.display()
        )),
    }
}

/// `alpendns policy test <domain> [--client <name>]`
///
/// Beantwortet die Frage "warum wurde das geblockt?" ohne Blick ins Log — und
/// ohne dass der Server laufen muss.
fn policy_test(path: &std::path::Path, domain: &str, client: Option<&str>) -> anyhow::Result<()> {
    let config = Config::load(path)?;
    let blueprint = Blueprint::from_config(&config)?;

    // Der Client wird über seinen Namen gesucht; ausgewertet wird dann mit
    // seiner ersten konfigurierten Adresse — denn danach wird auch im Betrieb
    // entschieden.
    let peer = match client {
        None => std::net::IpAddr::from([127, 0, 0, 1]),
        Some(name) => {
            let entry = config
                .client
                .iter()
                .find(|entry| entry.name == name)
                .with_context(|| {
                    let known: Vec<&str> = config.client.iter().map(|c| c.name.as_str()).collect();
                    format!(
                        "kein Client namens '{name}'; konfiguriert sind: {}",
                        if known.is_empty() {
                            "keine".to_owned()
                        } else {
                            known.join(", ")
                        }
                    )
                })?;
            let address = entry
                .matches
                .ip
                .first()
                .context("dieser Client hat keine Adresse")?;
            alpendns::config::parse_net(address)
                .map_err(anyhow::Error::msg)?
                .addr()
        }
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("tokio-Runtime konnte nicht gestartet werden")?;
    let loaded = runtime.block_on(async {
        let lists = Lists::new(
            Loader::new(config.blocking.cache_dir.clone())?,
            to_specs(&config.blocklist, &config.allowlist),
        );
        // Nachsichtig: die Simulation soll auch ohne Netz etwas sagen können,
        // dann eben auf Basis der zwischengespeicherten Listen.
        lists.load(false).await
    })?;

    let entries = loaded.total_entries();
    let engine = Engine::new(
        blueprint.build(&loaded)?,
        entries,
        &config.blocking,
        SystemClock,
        SystemWallClock,
    );

    // Dieselbe Auswertung, die `/api/explain` benutzt — sonst könnten
    // Kommandozeile und UI verschiedene Antworten auf dieselbe Frage geben.
    let explanation =
        alpendns::policy::explain(&engine, domain, peer).map_err(anyhow::Error::msg)?;

    println!("Domain:   {domain}");
    println!("Client:   {} ({peer})", client.unwrap_or("(default)"));
    println!(
        "Verdikt:  {}",
        if explanation.blocked {
            "GEBLOCKT"
        } else {
            "durchgelassen"
        }
    );
    println!("\nBegründung:");
    for (index, step) in explanation.steps.iter().enumerate() {
        println!("  {}. {step}", index.saturating_add(1));
    }
    Ok(())
}

/// Wartet auf SIGINT oder SIGTERM.
async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(stream) => stream,
        Err(error) => {
            tracing::warn!(%error, "SIGTERM nicht abonnierbar, es zählt nur SIGINT");
            let _ = tokio::signal::ctrl_c().await;
            return;
        }
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}
