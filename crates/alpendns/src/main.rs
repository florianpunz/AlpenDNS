//! Binary: Argumente, Konfiguration, Laufzeit, Signale, Shutdown.

use std::path::PathBuf;
use std::process::ExitCode;

use alpendns::config::Config;
use alpendns::server::Server;
use alpendns::upstream::ForwardBackend;
use anyhow::Context as _;
use tokio_util::sync::CancellationToken;

const USAGE: &str = "alpendns — privacy-fokussierter DNS-Server

Aufruf:
  alpendns -c <datei>    Server mit dieser Konfiguration starten

Optionen:
  -c, --config <datei>   Pfad zur Konfigurationsdatei (Pflicht)
  -h, --help             Diese Hilfe
  -V, --version          Version ausgeben
";

enum Args {
    Run(PathBuf),
    Print(String),
}

fn parse_args() -> anyhow::Result<Args> {
    let mut config = None;
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
            other => anyhow::bail!("unbekanntes Argument '{other}'\n\n{USAGE}"),
        }
    }
    config
        .map(Args::Run)
        .context(format!("keine Konfiguration angegeben\n\n{USAGE}"))
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
        Args::Run(path) => path,
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = Config::load(&path)?;
    let (upstream_name, upstream_addr) = {
        let upstream = config
            .single_upstream()
            .context("Konfiguration ohne Upstream")?;
        (upstream.name.clone(), upstream.addr.0)
    };
    let server_config = config.server;

    // Die Runtime wird von Hand gebaut statt über #[tokio::main]: ein Fehler
    // beim Start soll eine Fehlermeldung geben, kein Panic (CLAUDE.md B.1).
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("tokio-Runtime konnte nicht gestartet werden")?;

    runtime.block_on(async move {
        let backend = ForwardBackend::new(upstream_addr, server_config.query_timeout);
        let bound = Server::new(backend, server_config.edns.udp_payload_size)
            .bind(&server_config)
            .await
            .context("Listener konnten nicht geöffnet werden")?;

        tracing::info!(
            udp = ?bound.udp_addrs(),
            tcp = ?bound.tcp_addrs(),
            upstream = %upstream_name,
            "AlpenDNS gestartet"
        );
        // Phase 1 spricht Klartext-DNS nach außen. Das widerspricht B.1 Regel 7
        // und verschwindet mit Phase 3 — bis dahin soll es niemand übersehen.
        tracing::warn!(
            "Upstream läuft unverschlüsselt über UDP (Phase 1). \
             Nicht für den Dauerbetrieb — verschlüsselte Transporte kommen in Phase 3."
        );

        let shutdown = CancellationToken::new();
        let signals = shutdown.clone();
        tokio::spawn(async move {
            wait_for_signal().await;
            tracing::info!("Signal empfangen, laufende Anfragen werden noch beantwortet");
            signals.cancel();
        });

        bound.run(shutdown).await;
        tracing::info!("beendet");
        Ok(())
    })
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
