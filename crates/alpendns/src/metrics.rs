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
use crate::logging::LogStats;
use crate::policy::PolicyStats;
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
    use std::time::Duration;

    fn snapshot() -> Snapshot {
        Snapshot {
            log: LogStats {
                queries: 100,
                blocked: 12,
                by_rcode: vec![("NoError".to_owned(), 88), ("NXDomain".to_owned(), 12)],
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
                successes: 38,
                failures: 1,
                rtt: Some(Duration::from_millis(34)),
                down: false,
            }],
            uptime: Duration::from_secs(3600),
        }
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
        ] {
            assert!(text.contains(expected), "fehlt: {expected}\n{text}");
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
        // Jede Zeile muss weiterhin genau zwei unmaskierte Anführungszeichen haben.
        for line in text.lines().filter(|l| l.contains("resolver=")) {
            let unescaped = line
                .replace(r#"\""#, "")
                .replace(r"\\", "")
                .matches('"')
                .count();
            assert_eq!(unescaped, 2, "{line}");
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
}
