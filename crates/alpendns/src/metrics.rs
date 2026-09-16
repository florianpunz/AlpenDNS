//! Prometheus-Ausgabe im Textformat.
//!
//! Von Hand erzeugt statt über eine Metrik-Bibliothek: das Format ist eine
//! Handvoll Zeilen, und die Zähler liegen ohnehin schon in den Strukturen, die
//! sie hochzählen. Eine Registry dazwischen hieße, jeden Wert zweimal zu führen.
//!
//! **Keine Namen.** Ein Label mit einer Domain wäre ein Query-Log mit anderem
//! Dateinamen — Prometheus behält jede Zeitreihe für immer. Labels gibt es nur
//! für Dinge aus der Konfiguration: Resolver-Namen und RCODEs.

use std::fmt::Write as _;

use crate::cache::Stats as CacheStats;
use crate::detect::{Action as DetectorAction, Detector as DetectorKind};
use crate::dnssec::{CounterSnapshot as DnssecCounters, Verdict};
use crate::filter::block::BlockMode;
use crate::logging::{LogStats, Mode as LogMode};
use crate::policy::PolicyStats;
use crate::privacy::CounterSnapshot as PrivacyCounters;
use crate::server::TcpCounters;
use crate::upstream::pool::UpstreamStats;

/// Alles, was der Endpunkt ausgibt.
#[derive(Debug)]
pub struct Snapshot {
    pub log: LogStats,
    pub cache: CacheStats,
    pub cache_entries: usize,
    pub policy: PolicyStats,
    pub upstreams: Vec<UpstreamStats>,
    pub uptime: std::time::Duration,
    /// Der eingestellte Block-Modus.
    pub blocking_mode: BlockMode,
    /// Der eingestellte Log-Modus.
    pub logging_mode: LogMode,
    /// Wie viele geladene Listen je Format, absteigend nach Häufigkeit.
    pub list_formats: Vec<(String, u64)>,
    /// Wie oft die Privacy-Mechanismen tatsächlich gegriffen haben.
    pub privacy: PrivacyCounters,
    /// Die k-Schwelle aus der Konfiguration.
    pub aggregate_k: u32,
    /// Anfragen auf Namen unterhalb der k-Schwelle.
    pub below_threshold_queries: u64,
    /// Abstand, in dem der Zonen-Seed neu gezogen wird. Null heißt: gar nicht.
    pub zone_seed_rotation: std::time::Duration,
    /// Wie oft er seit dem Start neu gezogen wurde.
    pub zone_seed_rotations: u64,
    /// Ob die Signaturkette selbst nachgerechnet wird.
    pub dnssec_enabled: bool,
    /// Wie die Prüfung ausgegangen ist.
    pub dnssec: DnssecCounters,
    /// Was für jeden Detektor eingestellt ist.
    pub detectors: Vec<(crate::detect::Detector, crate::detect::Action)>,
    /// Wie oft jeder Detektor angeschlagen hat.
    pub detections: Vec<(crate::detect::Detector, u64)>,
    /// Fehlt, wenn die Drosselung abgeschaltet ist.
    pub rate_limit: Option<RateLimitStats>,
    /// Was der TCP-Listener an Verbindungen abgewiesen oder aufgegeben hat.
    pub tcp: TcpCounters,
}

/// Was die Drosselung pro Client zu berichten hat.
#[derive(Debug)]
pub struct RateLimitStats {
    pub per_client_qps: f64,
    pub burst: f64,
    /// Verworfene Anfragen seit dem Start.
    pub throttled: u64,
    /// Clients, für die gerade Buch geführt wird.
    pub tracked: usize,
}

/// Maskiert, was in einem Label-Wert nicht vorkommen darf.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn counter(out: &mut String, name: &str, help: &str, value: u64) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} counter");
    let _ = writeln!(out, "{name} {value}");
}

fn gauge(out: &mut String, name: &str, help: &str, value: f64) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} gauge");
    let _ = writeln!(out, "{name} {value}");
}

/// Erzeugt die Antwort des `/metrics`-Endpunkts.
pub fn render(snapshot: &Snapshot) -> String {
    let mut out = String::with_capacity(2048);

    let _ = writeln!(
        out,
        "# HELP alpendns_build_info Version des laufenden Prozesses"
    );
    let _ = writeln!(out, "# TYPE alpendns_build_info gauge");
    let _ = writeln!(
        out,
        "alpendns_build_info{{version=\"{}\"}} 1",
        escape(env!("CARGO_PKG_VERSION"))
    );
    gauge(
        &mut out,
        "alpendns_uptime_seconds",
        "Laufzeit des Prozesses",
        snapshot.uptime.as_secs_f64(),
    );

    counter(
        &mut out,
        "alpendns_queries_total",
        "Beantwortete Anfragen",
        snapshot.log.queries,
    );
    counter(
        &mut out,
        "alpendns_queries_blocked_total",
        "Anfragen, die von einer Regel geblockt wurden",
        snapshot.log.blocked,
    );
    counter(
        &mut out,
        "alpendns_queries_allowed_total",
        "Anfragen, die eine Allowlist oder Freigabe ausdrücklich durchgelassen hat",
        snapshot.policy.allowed,
    );

    let _ = writeln!(out, "# HELP alpendns_responses_total Antworten nach RCODE");
    let _ = writeln!(out, "# TYPE alpendns_responses_total counter");
    for (rcode, value) in &snapshot.log.by_rcode {
        let _ = writeln!(
            out,
            "alpendns_responses_total{{rcode=\"{}\"}} {value}",
            escape(rcode)
        );
    }

    counter(
        &mut out,
        "alpendns_cache_hits_total",
        "Treffer auf einen gültigen Cache-Eintrag",
        snapshot.cache.hits,
    );
    counter(
        &mut out,
        "alpendns_cache_stale_hits_total",
        "Treffer auf einen abgelaufenen Eintrag im serve-stale-Fenster",
        snapshot.cache.stale_hits,
    );
    counter(
        &mut out,
        "alpendns_cache_misses_total",
        "Anfragen, die der Cache nicht beantworten konnte",
        snapshot.cache.misses,
    );
    gauge(
        &mut out,
        "alpendns_cache_entries",
        "Einträge im Cache",
        snapshot.cache_entries as f64,
    );
    gauge(
        &mut out,
        "alpendns_cache_hit_ratio",
        "Anteil der Anfragen, die der Cache beantwortet hat",
        snapshot.cache.hit_rate(),
    );
    gauge(
        &mut out,
        "alpendns_list_entries",
        "Eindeutige Domains in allen geladenen Listen",
        snapshot.policy.entries as f64,
    );

    // Was eingestellt ist, nicht nur was passiert. Ohne diese drei lässt sich
    // nicht belegen, ob eine Einstellung im Feld überhaupt jemand benutzt — und
    // genau das ist die Frage vor jeder weiteren Streichung. Keine Namen: die
    // Label-Werte stammen ausschließlich aus geschlossenen Aufzählungen.
    let _ = writeln!(
        out,
        "# HELP alpendns_blocking_mode 1 beim eingestellten Block-Modus, 0 bei den übrigen"
    );
    let _ = writeln!(out, "# TYPE alpendns_blocking_mode gauge");
    for mode in BlockMode::ALL {
        let _ = writeln!(
            out,
            "alpendns_blocking_mode{{mode=\"{}\"}} {}",
            mode.as_str(),
            u8::from(mode == snapshot.blocking_mode)
        );
    }

    let _ = writeln!(
        out,
        "# HELP alpendns_logging_mode 1 beim eingestellten Log-Modus, 0 bei den übrigen"
    );
    let _ = writeln!(out, "# TYPE alpendns_logging_mode gauge");
    for mode in LogMode::ALL {
        let _ = writeln!(
            out,
            "alpendns_logging_mode{{mode=\"{}\"}} {}",
            mode.as_str(),
            u8::from(mode == snapshot.logging_mode)
        );
    }

    let _ = writeln!(
        out,
        "# HELP alpendns_blocked_by_reason_total Geblockte Anfragen je Grund"
    );
    let _ = writeln!(out, "# TYPE alpendns_blocked_by_reason_total counter");
    for (reason, value) in &snapshot.log.by_reason {
        let _ = writeln!(
            out,
            "alpendns_blocked_by_reason_total{{reason=\"{}\"}} {value}",
            reason.as_str()
        );
    }

    // Die Wirkung der Privacy-Schicht, nicht ihre Einstellung. Ohne diese
    // Zähler ist "ECS wird entfernt" eine Behauptung in der Konfiguration;
    // mit ihnen ist es eine Zahl, die im Betrieb steigt.
    counter(
        &mut out,
        "alpendns_privacy_ecs_stripped_total",
        "Anfragen, aus denen eine ECS-Option entfernt wurde",
        snapshot.privacy.ecs_stripped,
    );
    counter(
        &mut out,
        "alpendns_privacy_padded_total",
        "Anfragen, die auf Blockgröße aufgefüllt wurden",
        snapshot.privacy.padded,
    );
    counter(
        &mut out,
        "alpendns_privacy_case_randomized_total",
        "Anfragen mit gewürfelter Groß-/Kleinschreibung (0x20)",
        snapshot.privacy.randomized,
    );
    counter(
        &mut out,
        "alpendns_privacy_cookies_total",
        "Anfragen mit gesetztem DNS-Cookie",
        snapshot.privacy.cookies,
    );

    // Die k-Schwelle gehört in die Metrik, weil sonst niemand nachvollziehen
    // kann, wie viel eine Häufigkeitsliste verschweigt.
    gauge(
        &mut out,
        "alpendns_aggregate_k",
        "Ab wie vielen Treffern ein Name überhaupt genannt werden darf",
        f64::from(snapshot.aggregate_k),
    );
    counter(
        &mut out,
        "alpendns_queries_below_threshold_total",
        "Anfragen auf Namen, die die k-Schwelle nicht erreicht haben",
        snapshot.below_threshold_queries,
    );

    // Was die eigene Validierung ergeben hat. Ohne `bogus` als eigene Zeile
    // wäre nicht zu sehen, ob eine Zone im Netz kaputt ist oder ein Upstream
    // lügt — und genau danach schaut man, wenn etwas nicht auflöst.
    gauge(
        &mut out,
        "alpendns_dnssec_enabled",
        "1, wenn AlpenDNS die Signaturkette selbst nachrechnet",
        f64::from(u8::from(snapshot.dnssec_enabled)),
    );
    let _ = writeln!(
        out,
        "# HELP alpendns_dnssec_total Geprüfte Antworten je Urteil"
    );
    let _ = writeln!(out, "# TYPE alpendns_dnssec_total counter");
    for verdict in Verdict::ALL {
        let _ = writeln!(
            out,
            "alpendns_dnssec_total{{result=\"{}\"}} {}",
            verdict.as_str(),
            snapshot.dnssec.get(verdict)
        );
    }

    // Die Rotation der Zonen-Zuordnung. Ohne diese beiden Zeilen ist "kein
    // Anbieter lernt ein stabiles Bild" eine Behauptung; mit ihnen steht da,
    // wie oft die Zuordnung tatsächlich gewechselt hat.
    gauge(
        &mut out,
        "alpendns_zone_seed_rotation_seconds",
        "Abstand, in dem die Zuordnung Domain → Upstream neu gewürfelt wird; 0 = nie",
        snapshot.zone_seed_rotation.as_secs_f64(),
    );
    counter(
        &mut out,
        "alpendns_zone_seed_rotations_total",
        "Wie oft die Zuordnung Domain → Upstream seit dem Start neu gewürfelt wurde",
        snapshot.zone_seed_rotations,
    );

    // Was für jeden Detektor eingestellt ist — dieselbe Form wie bei den
    // Modi: eine Zeile je Kombination, damit sich über viele Installationen
    // summieren lässt, wer welche Stufe benutzt. Ein Detektor, der nicht
    // eingehängt ist, erscheint als `off`.
    let _ = writeln!(
        out,
        "# HELP alpendns_detector_action 1 bei der eingestellten Stufe je Detektor, 0 sonst"
    );
    let _ = writeln!(out, "# TYPE alpendns_detector_action gauge");
    for detector in DetectorKind::ALL {
        let configured = snapshot
            .detectors
            .iter()
            .find_map(|(known, action)| (*known == detector).then_some(*action))
            .unwrap_or(DetectorAction::Off);
        for action in DetectorAction::ALL {
            let _ = writeln!(
                out,
                "alpendns_detector_action{{detector=\"{}\",action=\"{}\"}} {}",
                detector.as_str(),
                action.as_str(),
                u8::from(action == configured)
            );
        }
    }

    let _ = writeln!(
        out,
        "# HELP alpendns_detections_total Funde je Detektor, oberhalb seiner Schwelle"
    );
    let _ = writeln!(out, "# TYPE alpendns_detections_total counter");
    for (detector, count) in &snapshot.detections {
        let _ = writeln!(
            out,
            "alpendns_detections_total{{detector=\"{}\"}} {count}",
            detector.as_str()
        );
    }

    let _ = writeln!(out, "# HELP alpendns_lists Geladene Listen je Format");
    let _ = writeln!(out, "# TYPE alpendns_lists gauge");
    for (format, count) in &snapshot.list_formats {
        let _ = writeln!(
            out,
            "alpendns_lists{{format=\"{}\"}} {count}",
            escape(format)
        );
    }

    let _ = writeln!(
        out,
        "# HELP alpendns_upstream_queries_total Erfolgreiche Anfragen je Upstream"
    );
    let _ = writeln!(out, "# TYPE alpendns_upstream_queries_total counter");
    for upstream in &snapshot.upstreams {
        let _ = writeln!(
            out,
            "alpendns_upstream_queries_total{{resolver=\"{}\"}} {}",
            escape(&upstream.name),
            upstream.successes
        );
    }
    let _ = writeln!(
        out,
        "# HELP alpendns_upstream_transport 1 je Upstream beim benutzten Transport"
    );
    let _ = writeln!(out, "# TYPE alpendns_upstream_transport gauge");
    for upstream in &snapshot.upstreams {
        let _ = writeln!(
            out,
            "alpendns_upstream_transport{{resolver=\"{}\",transport=\"{}\"}} 1",
            escape(&upstream.name),
            upstream.scheme
        );
    }

    let _ = writeln!(
        out,
        "# HELP alpendns_upstream_failures_total Fehlgeschlagene Anfragen je Upstream"
    );
    let _ = writeln!(out, "# TYPE alpendns_upstream_failures_total counter");
    for upstream in &snapshot.upstreams {
        let _ = writeln!(
            out,
            "alpendns_upstream_failures_total{{resolver=\"{}\"}} {}",
            escape(&upstream.name),
            upstream.failures
        );
    }
    let _ = writeln!(
        out,
        "# HELP alpendns_upstream_rtt_seconds Gleitendes Mittel der Antwortzeit"
    );
    let _ = writeln!(out, "# TYPE alpendns_upstream_rtt_seconds gauge");
    for upstream in &snapshot.upstreams {
        if let Some(rtt) = upstream.rtt {
            let _ = writeln!(
                out,
                "alpendns_upstream_rtt_seconds{{resolver=\"{}\"}} {}",
                escape(&upstream.name),
                rtt.as_secs_f64()
            );
        }
    }
    // Keine Adressen: der Zähler sagt, *dass* gedrosselt wurde, nicht wen.
    // Ein Label mit der Client-IP wäre eine Anwesenheitsliste mit Zeitstempel,
    // die Prometheus für immer behält.
    gauge(
        &mut out,
        "alpendns_rate_limit_enabled",
        "1, wenn pro Client gedrosselt wird",
        f64::from(u8::from(snapshot.rate_limit.is_some())),
    );
    if let Some(limit) = &snapshot.rate_limit {
        counter(
            &mut out,
            "alpendns_rate_limited_total",
            "Anfragen, die wegen Überschreitung des Client-Limits verworfen wurden",
            limit.throttled,
        );
        gauge(
            &mut out,
            "alpendns_rate_limit_clients",
            "Clients, für die gerade ein Guthaben geführt wird",
            limit.tracked as f64,
        );
        gauge(
            &mut out,
            "alpendns_rate_limit_qps",
            "Konfigurierte Anfragen je Sekunde und Client",
            limit.per_client_qps,
        );
        gauge(
            &mut out,
            "alpendns_rate_limit_burst",
            "Konfiguriertes Guthaben, das ein Client ansammeln darf",
            limit.burst,
        );
    }

    // Die Obergrenzen des TCP-Listeners. Auch hier ohne Adresse, aus demselben
    // Grund wie bei der Drosselung. Ohne diese drei Zahlen wäre eine Grenze
    // unsichtbar: ein Gerät, dessen Verbindungen abgewiesen werden, bekommt
    // einfach keine Antworten mehr.
    counter(
        &mut out,
        "alpendns_tcp_connections_rejected_total",
        "TCP-Verbindungen, die eine Quell-IP über ihr Kontingent hinaus aufmachen wollte",
        snapshot.tcp.rejected_per_client,
    );
    counter(
        &mut out,
        "alpendns_tcp_connections_at_capacity_total",
        "Annahmen, die warten mussten, weil alle Verbindungsplätze belegt waren",
        snapshot.tcp.at_capacity,
    );
    counter(
        &mut out,
        "alpendns_tcp_body_timeouts_total",
        "TCP-Verbindungen, die ihr Längenpräfix geschickt und dann geschwiegen haben",
        snapshot.tcp.body_timeouts,
    );

    let _ = writeln!(
        out,
        "# HELP alpendns_upstream_down 1, wenn ein Upstream gerade übersprungen wird"
    );
    let _ = writeln!(out, "# TYPE alpendns_upstream_down gauge");
    for upstream in &snapshot.upstreams {
        let _ = writeln!(
            out,
            "alpendns_upstream_down{{resolver=\"{}\"}} {}",
            escape(&upstream.name),
            u8::from(upstream.down)
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::BlockReason;
    use std::time::Duration;

    fn snapshot() -> Snapshot {
        Snapshot {
            log: LogStats {
                queries: 100,
                blocked: 12,
                by_rcode: vec![("NoError".to_owned(), 88), ("NXDomain".to_owned(), 12)],
                // Wie im Betrieb: jeder Grund kommt vor, auch mit Wert null.
                // Sonst verschwände eine Kategorie aus der Ausgabe, sobald sie
                // gerade nicht auftritt, und ein Diagramm darüber bekäme Lücken.
                by_reason: BlockReason::ALL
                    .iter()
                    .map(|&reason| {
                        let count = match reason {
                            BlockReason::Blocklist => 9,
                            BlockReason::Regex => 2,
                            BlockReason::Schedule => 1,
                            _ => 0,
                        };
                        (reason, count)
                    })
                    .collect(),
                ring_entries: 0,
            },
            cache: CacheStats {
                hits: 60,
                stale_hits: 2,
                misses: 38,
                inserts: 38,
            },
            cache_entries: 38,
            policy: PolicyStats {
                blocked: 12,
                allowed: 3,
                passed: 85,
                entries: 79_747,
            },
            upstreams: vec![UpstreamStats {
                name: "quad9".to_owned(),
                scheme: "dot",
                successes: 38,
                failures: 1,
                rtt: Some(Duration::from_millis(34)),
                down: false,
            }],
            uptime: Duration::from_secs(3600),
            blocking_mode: BlockMode::Nxdomain,
            logging_mode: LogMode::Aggregate,
            list_formats: vec![("hosts".to_owned(), 2), ("wildcard".to_owned(), 1)],
            privacy: PrivacyCounters {
                ecs_stripped: 7,
                padded: 38,
                randomized: 38,
                cookies: 0,
            },
            aggregate_k: 5,
            below_threshold_queries: 21,
            zone_seed_rotation: Duration::from_secs(24 * 60 * 60),
            zone_seed_rotations: 3,
            dnssec_enabled: true,
            dnssec: DnssecCounters {
                secure: 11,
                insecure: 70,
                bogus: 1,
                indeterminate: 6,
            },
            detectors: vec![
                (DetectorKind::Dga, DetectorAction::Flag),
                (DetectorKind::Tunneling, DetectorAction::Block),
            ],
            detections: vec![(DetectorKind::Dga, 4), (DetectorKind::Nrd, 0)],
            rate_limit: Some(RateLimitStats {
                per_client_qps: 100.0,
                burst: 200.0,
                throttled: 5,
                tracked: 12,
            }),
            tcp: TcpCounters {
                rejected_per_client: 2,
                at_capacity: 1,
                body_timeouts: 3,
            },
        }
    }

    /// Der Zähler sagt, *dass* gedrosselt wurde. Stünde die Adresse als Label
    /// daneben, wäre die Metrik eine Anwesenheitsliste mit Zeitstempel — und
    /// Prometheus behält jede Zeitreihe für immer.
    #[test]
    fn the_rate_limit_metric_carries_no_address() {
        let out = render(&snapshot());
        assert!(out.contains("alpendns_rate_limited_total 5"));
        assert!(out.contains("alpendns_rate_limit_clients 12"));
        assert!(
            !out.contains("alpendns_rate_limited_total{"),
            "die Drosselungs-Metrik hat Labels bekommen"
        );
    }

    /// Abgeschaltet gibt es die Zähler nicht — statt einer Null, die aussieht
    /// wie "es wurde nie gedrosselt".
    #[test]
    fn a_disabled_rate_limit_is_visible_as_such() {
        let mut snapshot = snapshot();
        snapshot.rate_limit = None;
        let out = render(&snapshot);
        assert!(out.contains("alpendns_rate_limit_enabled 0"));
        assert!(!out.contains("alpendns_rate_limited_total"));
    }

    /// Die Grenzen des TCP-Listeners stehen in der Ausgabe, und sie sagen
    /// nicht, welches Gerät sie erreicht hat.
    #[test]
    fn the_tcp_limits_are_visible_without_naming_a_client() {
        let out = render(&snapshot());
        assert!(out.contains("alpendns_tcp_connections_rejected_total 2"));
        assert!(out.contains("alpendns_tcp_connections_at_capacity_total 1"));
        assert!(out.contains("alpendns_tcp_body_timeouts_total 3"));
        assert!(
            !out.contains("alpendns_tcp_connections_rejected_total{"),
            "die TCP-Metrik hat Labels bekommen"
        );
    }

    /// Jede Metrik braucht HELP und TYPE vor der ersten Zeile, sonst lehnt
    /// `promtool check metrics` sie ab.
    #[test]
    fn every_metric_is_declared_before_it_is_used() {
        let text = render(&snapshot());
        let mut declared = std::collections::HashSet::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                if let Some((name, _)) = rest.split_once(' ') {
                    declared.insert(name.to_owned());
                }
                continue;
            }
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let name = line
                .split(['{', ' '])
                .next()
                .expect("jede Zeile hat einen Namen");
            assert!(declared.contains(name), "'{name}' ohne TYPE-Zeile");
        }
    }

    #[test]
    fn every_sample_line_has_a_numeric_value() {
        let text = render(&snapshot());
        for line in text
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let value = line.rsplit(' ').next().expect("Wert");
            assert!(value.parse::<f64>().is_ok(), "kein Zahlenwert in '{line}'");
        }
    }

    #[test]
    fn counters_and_gauges_are_present() {
        let text = render(&snapshot());
        for expected in [
            "alpendns_queries_total 100",
            "alpendns_queries_blocked_total 12",
            "alpendns_cache_hits_total 60",
            "alpendns_cache_entries 38",
            "alpendns_list_entries 79747",
            "alpendns_responses_total{rcode=\"NXDomain\"} 12",
            "alpendns_upstream_queries_total{resolver=\"quad9\"} 38",
            "alpendns_upstream_down{resolver=\"quad9\"} 0",
            "alpendns_blocking_mode{mode=\"nxdomain\"} 1",
            "alpendns_blocking_mode{mode=\"zero_ip\"} 0",
            "alpendns_logging_mode{mode=\"aggregate\"} 1",
            "alpendns_logging_mode{mode=\"full\"} 0",
            "alpendns_lists{format=\"hosts\"} 2",
            "alpendns_lists{format=\"wildcard\"} 1",
            "alpendns_blocked_by_reason_total{reason=\"blocklist\"} 9",
            "alpendns_blocked_by_reason_total{reason=\"other\"} 0",
            "alpendns_privacy_ecs_stripped_total 7",
            "alpendns_queries_below_threshold_total 21",
            "alpendns_aggregate_k 5",
            "alpendns_zone_seed_rotations_total 3",
            "alpendns_dnssec_enabled 1",
            "alpendns_dnssec_total{result=\"secure\"} 11",
            "alpendns_dnssec_total{result=\"bogus\"} 1",
            "alpendns_detector_action{detector=\"dga\",action=\"flag\"} 1",
            "alpendns_detector_action{detector=\"dga\",action=\"block\"} 0",
            "alpendns_detector_action{detector=\"tunneling\",action=\"block\"} 1",
            // Nicht eingehängt heißt `off`.
            "alpendns_detector_action{detector=\"nrd\",action=\"off\"} 1",
            "alpendns_detections_total{detector=\"dga\"} 4",
            "alpendns_blocked_by_reason_total{reason=\"tunneling\"} 0",
            "alpendns_zone_seed_rotation_seconds 86400",
            "alpendns_upstream_transport{resolver=\"quad9\",transport=\"dot\"} 1",
        ] {
            assert!(text.contains(expected), "fehlt: {expected}\n{text}");
        }
    }

    #[test]
    fn exactly_one_mode_is_marked_active() {
        // Sonst wäre die Metrik zum Belegen unbrauchbar: über mehrere
        // Installationen summiert soll je Modus die Zahl der Installationen
        // herauskommen, die ihn benutzen.
        let text = render(&snapshot());
        for metric in ["alpendns_blocking_mode", "alpendns_logging_mode"] {
            let active = text
                .lines()
                .filter(|line| line.starts_with(metric) && line.ends_with(" 1"))
                .count();
            assert_eq!(active, 1, "{metric} hat {active} aktive Werte\n{text}");
        }
    }

    #[test]
    fn the_hit_ratio_is_a_fraction() {
        let text = render(&snapshot());
        let line = text
            .lines()
            .find(|l| l.starts_with("alpendns_cache_hit_ratio "))
            .expect("Zeile");
        let value: f64 = line
            .rsplit(' ')
            .next()
            .and_then(|v| v.parse().ok())
            .expect("Zahl");
        assert!((0.0..=1.0).contains(&value), "{value}");
    }

    #[test]
    fn label_values_are_escaped() {
        let mut snapshot = snapshot();
        snapshot.upstreams = vec![UpstreamStats {
            name: r#"boes"er\name"#.to_owned(),
            scheme: "doh",
            successes: 1,
            failures: 0,
            rtt: None,
            down: false,
        }];
        let text = render(&snapshot);
        assert!(
            text.contains(r#"resolver="boes\"er\\name""#),
            "Anführungszeichen im Label nicht maskiert:\n{text}"
        );
        // Jede Zeile muss weiterhin je Label genau zwei unmaskierte
        // Anführungszeichen haben — sonst hat ein Wert die Klammer gesprengt.
        for line in text.lines().filter(|l| l.contains("resolver=")) {
            let labels = line
                .split_once('{')
                .and_then(|(_, rest)| rest.rsplit_once('}'))
                .map_or(0, |(inside, _)| inside.split(',').count());
            let unescaped = line
                .replace(r#"\""#, "")
                .replace(r"\\", "")
                .matches('"')
                .count();
            assert_eq!(unescaped, labels * 2, "{line}");
        }
    }

    #[test]
    fn no_domain_name_can_appear_as_a_label() {
        // Prometheus behält jede Zeitreihe für immer; eine Domain als Label
        // wäre ein Query-Log mit anderem Dateinamen.
        let text = render(&snapshot());
        for line in text.lines() {
            assert!(
                !line.contains("domain=") && !line.contains("name=\"") && !line.contains("qname"),
                "verdächtiges Label in '{line}'"
            );
        }
    }

    /// Die Label-Schlüssel stehen abschließend fest.
    ///
    /// Der Test davor verbietet drei Schreibweisen und wäre mit einer vierten zu
    /// umgehen. Dieser dreht die Richtung um: erlaubt ist, was hier steht, und
    /// jedes neue Label muss durch diese Liste. Jeder Eintrag stammt aus einer
    /// geschlossenen Menge — Aufzählung oder Konfigurationswert —, keiner aus
    /// einer Anfrage. Ein Client oder eine Domain kann daher nicht als
    /// Label-Schlüssel auftauchen, egal wer was fragt.
    #[test]
    fn every_label_key_comes_from_a_closed_set() {
        const ALLOWED: [&str; 10] = [
            "version",   // Konstante aus dem Build
            "rcode",     // Aufzählung des Protokolls
            "mode",      // Aufzählung aus der Konfiguration
            "format",    // Aufzählung der Listenformate
            "reason",    // BlockReason::ALL
            "resolver",  // Name aus der Konfiguration
            "transport", // Aufzählung der Transporte
            "result",    // dnssec::Verdict::ALL
            "detector",  // detect::Detector::ALL
            "action",    // detect::Action::ALL
        ];
        let text = render(&snapshot());
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let Some((_, rest)) = line.split_once('{') else {
                continue;
            };
            let Some((inside, _)) = rest.rsplit_once('}') else {
                continue;
            };
            for pair in inside.split(',') {
                let key = pair.split('=').next().unwrap_or(pair);
                assert!(
                    ALLOWED.contains(&key),
                    "unbekannter Label-Schlüssel '{key}' in '{line}'"
                );
            }
        }
    }

    /// Kein Label-Wert sieht aus wie ein Domainname oder eine Adresse.
    ///
    /// Die Ergänzung zum Test darüber: der prüft die Schlüssel, dieser die
    /// Werte. Ein Resolver darf "quad9" heißen, aber nichts in dieser Ausgabe
    /// darf die Form eines abgefragten Namens oder einer Client-Adresse haben.
    #[test]
    fn no_label_value_looks_like_a_name_or_an_address() {
        let text = render(&snapshot());
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            for value in line.split('"').skip(1).step_by(2) {
                // Die Version ist die einzige Stelle, an der Punkte legitim
                // sind: "0.0.1" ist kein Name.
                if line.starts_with("alpendns_build_info") {
                    continue;
                }
                assert!(
                    value.parse::<std::net::IpAddr>().is_err(),
                    "Adresse als Label-Wert in '{line}'"
                );
                assert!(
                    !value.contains('.'),
                    "punktierter Name als Label-Wert in '{line}'"
                );
            }
        }
    }
}
