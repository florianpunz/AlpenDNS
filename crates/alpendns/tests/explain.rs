//! Diese Datei pinnt, wo "erklären" nicht wie "auswerten" aussehen darf.
//!
//! `policy::explain` ist eine Frage, keine Anfrage: die drei Policy-Zähler
//! dürfen sich nicht bewegen, und doch muss ein von einer Heuristik geblockter
//! Name weiterhin als geblockt erscheinen. Beides gleichzeitig ist der Punkt.

// Testcode darf panicken, siehe B.1 und clippy.toml.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use alpendns::clock::{FixedWallClock, TestClock};
use alpendns::config::BlockingConfig;
use alpendns::detect::{Action, Detector, Detectors, Finding, NameDetector, Observation};
use alpendns::filter::LoadedLists;
use alpendns::filter::matcher::Builder as MatcherBuilder;
use alpendns::filter::parser::{Format, parse};
use alpendns::policy::rules::RegexRules;
use alpendns::policy::{Blueprint, Decision, Engine, PolicyBlueprint};
use alpendns::trace::Ctx;
use hickory_proto::rr::Name;

type TestEngine = Engine<Arc<TestClock>, Arc<FixedWallClock>>;

fn lists(entries: &[(&str, &str)]) -> LoadedLists {
    LoadedLists::from_matchers(
        entries
            .iter()
            .map(|(name, text)| {
                let parsed = parse(text, Format::Wildcard);
                let mut builder = MatcherBuilder::new();
                builder.add(name, &parsed);
                (Arc::from(*name), Arc::new(builder.build()))
            })
            .collect(),
    )
}

fn policy(name: &str, blocklists: &[&str], allowlists: &[&str], regex: &[&str]) -> PolicyBlueprint {
    let patterns: Vec<String> = regex.iter().map(|p| (*p).to_owned()).collect();
    PolicyBlueprint {
        name: Arc::from(name),
        blocklists: blocklists.iter().map(|n| Arc::from(*n)).collect(),
        allowlists: allowlists.iter().map(|n| Arc::from(*n)).collect(),
        regex: Arc::new(RegexRules::compile(name, &patterns).expect("valid patterns")),
        schedules: Vec::new(),
    }
}

fn engine(
    policies: Vec<PolicyBlueprint>,
    loaded: &LoadedLists,
    detectors: Detectors,
) -> TestEngine {
    let clock = Arc::new(TestClock::new());
    let wall = Arc::new(FixedWallClock::new(
        "2026-08-31T12:00:00[Europe/Vienna]"
            .parse()
            .expect("valid instant"),
    ));
    let blueprint = Blueprint::new(Vec::new(), Arc::from("default"), policies);
    let entries = loaded.total_entries();
    Engine::new(
        blueprint.build(loaded).expect("rule set"),
        entries,
        &BlockingConfig::default(),
        clock,
        wall,
    )
    .with_detectors(detectors)
}

/// Die Policy, auf der alle drei Tests laufen: eine Blockliste, eine Allowlist
/// und ein Detektor, der einen bestimmten Namensteil blockt.
fn shape() -> (LoadedLists, Vec<PolicyBlueprint>) {
    let loaded = lists(&[
        ("spiele", "spiel.example\n"),
        ("ausnahmen", "ausnahme.example\n"),
    ]);
    let policies = vec![policy("default", &["spiele"], &["ausnahmen"], &[])];
    (loaded, policies)
}

#[derive(Debug)]
struct Dummy;

impl NameDetector for Dummy {
    fn detector(&self) -> Detector {
        Detector::Dga
    }

    fn inspect(&self, observation: &Observation<'_>) -> Option<Finding> {
        observation
            .name
            .contains("verdaechtig")
            .then(|| Finding::new(Detector::Dga, 930, "test detector DGA: suspicious"))
    }
}

fn peer() -> IpAddr {
    "127.0.0.1".parse().expect("valid address")
}

/// Erklären ist kein Auswerten: keine der sechs Verzweigungen der Auswertung
/// darf einen Zähler bewegen.
#[test]
fn explain_leaves_the_policy_counters_alone() {
    let (loaded, policies) = shape();
    let engine = engine(
        policies,
        &loaded,
        Detectors::new().with_name(Dummy, Action::Block, 0),
    );
    engine
        .temporary()
        .grant("befreit.example", Duration::from_secs(60));

    // Sechs Eingaben, eine je Zählerstelle: der gewöhnliche Durchlauf, der
    // Wurzelname (normalize schlägt fehl), die Allowlist, die befristete
    // Freigabe, die Blockliste und der Detektor.
    let cases: &[(&str, bool, &str)] = &[
        ("frei.example", false, "final passed"),
        (".", false, "normalize-miss passed"),
        ("ausnahme.example", false, "allowlist allowed"),
        ("befreit.example", false, "temporary-allow allowed"),
        ("spiel.example", true, "blocklist block"),
        ("verdaechtig.example", true, "detector block"),
    ];

    let before = engine.stats();
    for (domain, blocked, branch) in cases {
        let explanation = alpendns::policy::explain(&engine, domain, peer())
            .unwrap_or_else(|error| panic!("{domain} ({branch}): {error}"));
        assert_eq!(
            explanation.blocked, *blocked,
            "{domain} ({branch}) was decided wrongly: {explanation:?}"
        );
        assert_eq!(
            engine.stats(),
            before,
            "explain moved a counter on the {branch} branch ({domain})"
        );
    }
}

/// Der DNS-Pfad zählt weiter — sonst wäre die Reparatur nur ein Ausschalten.
#[test]
fn the_dns_path_still_counts() {
    let (loaded, policies) = shape();
    let engine = engine(
        policies,
        &loaded,
        Detectors::new().with_name(Dummy, Action::Block, 0),
    );
    engine
        .temporary()
        .grant("befreit.example", Duration::from_secs(60));
    let peer = peer();

    let before = engine.stats();
    let mut expected = before;

    let ask = |domain: &str| {
        let mut ctx = Ctx::new(SocketAddr::new(peer, 4242));
        let name = Name::from_ascii(domain).expect("valid name");
        engine.evaluate(&name, peer, &mut ctx)
    };

    assert_eq!(ask("spiel.example."), Decision::Block);
    expected.blocked += 1;
    assert_eq!(engine.stats(), expected, "a DNS block was not counted");

    assert_eq!(ask("frei.example."), Decision::Allow);
    expected.passed += 1;
    assert_eq!(engine.stats(), expected, "a DNS pass was not counted");

    assert_eq!(ask("ausnahme.example."), Decision::Allow);
    expected.allowed += 1;
    assert_eq!(engine.stats(), expected, "an allowlist hit was not counted");

    assert_eq!(ask("befreit.example."), Decision::Allow);
    expected.allowed += 1;
    assert_eq!(
        engine.stats(),
        expected,
        "a temporary grant was not counted"
    );
}

/// Die Erklärung befragt die Heuristiken weiter: ein Detektor-Block muss
/// sichtbar bleiben, nicht nur die synthetisierte Antwort.
#[test]
fn explain_still_reports_a_heuristic_block() {
    let (loaded, policies) = shape();
    let engine = engine(
        policies,
        &loaded,
        Detectors::new().with_name(Dummy, Action::Block, 0),
    );

    let explanation =
        alpendns::policy::explain(&engine, "verdaechtig.example", peer()).expect("valid name");

    assert!(
        explanation.blocked,
        "the detector block vanished: {explanation:?}"
    );
    assert!(
        explanation.steps.iter().any(|step| step.contains("DGA")),
        "the detector step is missing from the reasoning: {:?}",
        explanation.steps
    );
    assert!(
        explanation
            .steps
            .iter()
            .any(|step| step.contains("synthesized") || step.contains("Synthesized")),
        "the synthesized step is missing: {:?}",
        explanation.steps
    );
}
