//! Binary: Argumente, Konfiguration, Laufzeit, Signale, Shutdown.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use alpendns::caching::CachingBackend;
use alpendns::clock::{SystemClock, SystemWallClock};
use alpendns::config::{Config, ListConfig};
use alpendns::filter::Lists;
use alpendns::filter::source::{ListSpec, Loader, Source};
use alpendns::policy::{Blueprint, Engine, PolicyBackend};
use alpendns::privacy;
use alpendns::router::ZoneRouter;
use alpendns::server::Server;
use alpendns::upstream::ForwardBackend;
use alpendns::upstream::pool::{Pool, Upstream};
use alpendns::upstream::transport::Transport;
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
    let server_config = config.server;
    let cache_config = config.cache;
    let privacy = privacy::Settings::from(&config.privacy);
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
    let upstreams: Vec<Upstream<Transport>> = pool_config
        .resolver
        .iter()
        .map(|resolver| {
            Upstream::new(
                resolver.name.clone(),
                Transport::new(
                    resolver.addr.clone(),
                    // Die Validierung stellt sicher, dass hier ein Name steht.
                    resolver.tls_name.as_deref().unwrap_or_default(),
                    timeout,
                    privacy,
                    Arc::clone(&tls),
                ),
            )
        })
        .collect();
    let strategy = pool_config.strategy;
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
        let pool = Arc::new(Pool::new(
            upstreams,
            strategy,
            pool_config.fanout,
            SystemClock,
        ));
        // Erststart ist strikt: lieber gar kein DNS als ungefiltertes DNS
        // (B.1 Regel 6). Spätere Ausfälle behandelt run_updater nachsichtig.
        let loaded = lists
            .load(true)
            .await
            .context("Blocklisten konnten beim Start nicht geladen werden")?;
        let entries = loaded.total_entries();
        let engine = Arc::new(Engine::new(
            blueprint.build(&loaded)?,
            entries,
            &blocking_config,
            SystemClock,
            SystemWallClock,
        ));

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
        let bound = Server::new(backend, server_config.edns.udp_payload_size)
            .bind(&server_config)
            .await
            .context("Listener konnten nicht geöffnet werden")?;

        tracing::info!(
            udp = ?bound.udp_addrs(),
            tcp = ?bound.tcp_addrs(),
            upstreams = ?upstream_names,
            strategy = ?strategy,
            forward_zones = ?zone_names,
            cache_entries = cache_config.max_entries,
            serve_stale = cache_config.serve_stale,
            list_entries = entries,
            clients = client_count,
            policies = policy_count,
            blocking = ?blocking_config.mode,
            "AlpenDNS gestartet"
        );

        let shutdown = CancellationToken::new();

        tokio::spawn(report_stats(
            Arc::clone(&cache),
            Arc::clone(&pool),
            Arc::clone(&engine),
            shutdown.clone(),
        ));
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
    pool: Arc<Pool<Transport, SystemClock>>,
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
    pool: &Pool<Transport, SystemClock>,
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

    let name = hickory_proto::rr::Name::from_str_relaxed(domain)
        .map_err(|e| anyhow::anyhow!("'{domain}' ist kein gültiger Domainname: {e}"))?;
    let ctx = alpendns::trace::Ctx::new(std::net::SocketAddr::new(peer, 0));
    let decision = engine.evaluate(&name, peer, &ctx);

    println!("Domain:   {domain}");
    println!("Client:   {} ({peer})", client.unwrap_or("(default)"));
    println!(
        "Verdikt:  {}",
        match decision {
            alpendns::policy::Decision::Block => "GEBLOCKT",
            alpendns::policy::Decision::Allow => "durchgelassen",
        }
    );
    println!("\nBegründung:");
    println!("{}", ctx.explain());
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
