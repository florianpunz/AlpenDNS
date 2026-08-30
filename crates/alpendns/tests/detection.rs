//! Die Heuristiken durch die echte Pipeline — Phase 8, Schritt 1, 2 und 7.
//!
//! Die Detektoren selbst haben ihre Tests neben sich; hier geht es um das, was
//! nur im Zusammenspiel sichtbar wird:
//!
//! * ein Detektor läuft *durch die Pipeline* und landet im Trace (Schritt 1),
//! * die vier Stufen `off`/`log`/`flag`/`block` tun je etwas anderes,
//! * der Rebinding-Schutz sieht die **Antwort** und nicht die Frage (Schritt 2),
//! * die Reihenfolge der Auswertung stimmt: eine Freigabe schlägt alles,
//! * eine befristete Sperre wirkt und läuft ab (Schritt 7).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use alpendns::clock::{FixedWallClock, TestClock};
use alpendns::config::BlockingConfig;
use alpendns::detect::{
    Action, Detector, Detectors, Finding, NameDetector, Observation, rebinding,
};
use alpendns::filter::block::BlockMode;
use alpendns::policy::{Blueprint, Decision, Engine, PolicyBackend};
use alpendns::resolve::{ResolveBackend, ResolveError};
use alpendns::trace::{Ctx, Step};
use hickory_proto::op::{Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::A;
use hickory_proto::rr::{Name, RData, Record, RecordType};

type TestEngine = Engine<Arc<TestClock>, FixedWallClock>;

fn name(text: &str) -> Name {
    Name::from_ascii(text).expect("gültiger Name")
}

fn wall() -> FixedWallClock {
    FixedWallClock::new(
        "2026-08-30T12:00:00+02:00[Europe/Vienna]"
            .parse()
            .expect("gültiger Zeitpunkt"),
    )
}

fn blocking() -> BlockingConfig {
    BlockingConfig {
        mode: BlockMode::Nxdomain,
        ..BlockingConfig::default()
    }
}

/// Eine Engine ohne Listen und Policies — nur mit den übergebenen Detektoren.
fn engine(detectors: Detectors, clock: &Arc<TestClock>) -> TestEngine {
    let config: alpendns::config::Config = toml::from_str(
        r#"
[server]
listen_udp = ["127.0.0.1:5353"]

[[upstream_pool]]
name = "default"

[[upstream_pool.resolver]]
name = "quad9"
addr = "dot://9.9.9.9:853"
tls_name = "dns.quad9.net"
"#,
    )
    .expect("Konfiguration parst");
    let blueprint = Blueprint::from_config(&config).expect("Blueprint");
    let lists = alpendns::filter::LoadedLists::default();
    Engine::new(
        blueprint.build(&lists).expect("Regelstand"),
        0,
        &blocking(),
        Arc::clone(clock),
        wall(),
    )
    .with_detectors(detectors)
}

fn ctx() -> Ctx {
    Ctx::new(SocketAddr::from(([127, 0, 0, 1], 5555)))
}

fn peer() -> IpAddr {
    IpAddr::from([127, 0, 0, 1])
}

// ---------------------------------------------------------------------------
// Schritt 1: ein Dummy-Detektor durch die Pipeline
// ---------------------------------------------------------------------------

/// Der Detektor aus dem Abnahmekriterium: er schlägt auf einen bekannten Namen
/// an und liefert Score und Begründung.
#[derive(Debug)]
struct Dummy;

impl NameDetector for Dummy {
    fn detector(&self) -> Detector {
        Detector::Dga
    }

    fn inspect(&self, observation: &Observation<'_>) -> Option<Finding> {
        observation.name.contains("verdaechtig").then(|| {
            Finding::new(
                Detector::Dga,
                930,
                format!("Testdetektor: '{}' enthält 'verdaechtig'", observation.name),
            )
        })
    }
}

fn with_dummy(action: Action) -> Detectors {
    Detectors::new().with_name(Dummy, action, 0)
}

/// Sucht den Detektor-Schritt im Trace.
fn detected(ctx: &Ctx) -> Option<(Detector, u16, Action, String)> {
    ctx.steps().iter().find_map(|step| match step {
        Step::Detected {
            detector,
            score,
            reason,
            action,
        } => Some((*detector, *score, *action, reason.to_string())),
        _ => None,
    })
}

#[test]
fn a_detector_runs_through_the_pipeline_and_lands_in_the_trace() {
    // Das Abnahmekriterium aus Schritt 1, wörtlich.
    let clock = Arc::new(TestClock::new());
    let engine = engine(with_dummy(Action::Flag), &clock);
    let mut ctx = ctx();

    let decision = engine.evaluate(&name("verdaechtig.example.com."), peer(), &mut ctx);

    assert_eq!(decision, Decision::Allow, "flag blockt nicht");
    let (detector, score, action, reason) = detected(&ctx).expect("kein Schritt im Trace");
    assert_eq!(detector, Detector::Dga);
    assert_eq!(score, 930);
    assert_eq!(action, Action::Flag);
    assert!(reason.contains("verdaechtig"), "{reason}");

    // Und die Begründungskette ist für Menschen lesbar — dieselbe, die
    // `alpendns policy test` ausgibt.
    let text = ctx.explain();
    assert!(text.contains("Score 0.930"), "{text}");
}

#[test]
fn the_four_actions_do_four_different_things() {
    for (action, expected, in_trace) in [
        (Action::Off, Decision::Allow, false),
        (Action::Log, Decision::Allow, true),
        (Action::Flag, Decision::Allow, true),
        (Action::Block, Decision::Block, true),
    ] {
        let clock = Arc::new(TestClock::new());
        let engine = engine(with_dummy(action), &clock);
        let mut ctx = ctx();
        let decision = engine.evaluate(&name("verdaechtig.example.com."), peer(), &mut ctx);
        assert_eq!(decision, expected, "{action:?}");
        assert_eq!(detected(&ctx).is_some(), in_trace, "{action:?} im Trace");
    }
}

#[test]
fn log_counts_but_stays_out_of_the_flagged_list() {
    // Der Unterschied zwischen `log` und `flag`: beide stehen im Trace, aber
    // nur `flag` erscheint in der Liste, die sich jemand ansieht.
    for (action, visible) in [
        (Action::Log, false),
        (Action::Flag, true),
        (Action::Block, true),
    ] {
        let clock = Arc::new(TestClock::new());
        let engine = engine(with_dummy(action), &clock);
        let mut ctx = ctx();
        engine.evaluate(&name("verdaechtig.example.com."), peer(), &mut ctx);
        let flagged = alpendns::logging::Flagged::from_steps(ctx.steps());
        assert_eq!(!flagged.is_empty(), visible, "{action:?}");
    }
}

#[test]
fn a_detector_that_blocks_names_itself_as_the_reason() {
    let clock = Arc::new(TestClock::new());
    let engine = engine(with_dummy(Action::Block), &clock);
    let mut ctx = ctx();
    engine.evaluate(&name("verdaechtig.example.com."), peer(), &mut ctx);

    assert_eq!(
        alpendns::logging::BlockReason::from_steps(ctx.steps()),
        alpendns::logging::BlockReason::Detector(Detector::Dga)
    );
}

#[test]
fn a_detector_that_only_flags_is_not_the_block_reason() {
    // Sonst stünde in der Statistik ein Grund für eine Anfrage, die gar nicht
    // geblockt wurde.
    let clock = Arc::new(TestClock::new());
    let engine = engine(with_dummy(Action::Flag), &clock);
    let mut ctx = ctx();
    engine.evaluate(&name("verdaechtig.example.com."), peer(), &mut ctx);
    assert_eq!(
        alpendns::logging::BlockReason::from_steps(ctx.steps()),
        alpendns::logging::BlockReason::Other,
        "ein flag wurde als Block-Grund gezählt"
    );
}

#[test]
fn an_ordinary_name_leaves_no_trace_entry() {
    let clock = Arc::new(TestClock::new());
    let engine = engine(with_dummy(Action::Block), &clock);
    let mut ctx = ctx();
    assert_eq!(
        engine.evaluate(&name("example.com."), peer(), &mut ctx),
        Decision::Allow
    );
    assert!(detected(&ctx).is_none());
}

// ---------------------------------------------------------------------------
// Reihenfolge der Auswertung
// ---------------------------------------------------------------------------

#[test]
fn a_temporary_grant_beats_a_blocking_detector() {
    // Die Allowlist steht vorn, weil sie sonst keinen Zweck hätte — das gilt
    // erst recht gegenüber einer Heuristik. Wer eine Domain freigibt, will sie
    // erreichen, egal was ein Score dazu sagt.
    let clock = Arc::new(TestClock::new());
    let engine = engine(with_dummy(Action::Block), &clock);
    engine
        .temporary()
        .grant("verdaechtig.example.com", Duration::from_secs(300));

    let mut ctx = ctx();
    assert_eq!(
        engine.evaluate(&name("verdaechtig.example.com."), peer(), &mut ctx),
        Decision::Allow
    );
    assert!(detected(&ctx).is_none(), "der Detektor lief trotzdem");
}

#[test]
fn a_temporary_denial_blocks_and_expires() {
    // Schritt 7: der Knopf neben einer auffälligen Anfrage.
    let clock = Arc::new(TestClock::new());
    let engine = engine(Detectors::new(), &clock);
    engine
        .denied()
        .grant("boese.example.com", Duration::from_secs(300));

    let mut first = ctx();
    assert_eq!(
        engine.evaluate(&name("boese.example.com."), peer(), &mut first),
        Decision::Block
    );
    assert!(
        first
            .steps()
            .iter()
            .any(|step| matches!(step, Step::TemporaryDeny { .. })),
        "{:?}",
        first.steps()
    );

    // Subdomains gelten mit — sonst wäre eine Sperre in der Praxis nutzlos.
    let mut below = ctx();
    assert_eq!(
        engine.evaluate(&name("www.boese.example.com."), peer(), &mut below),
        Decision::Block
    );

    clock.advance(Duration::from_secs(301));
    let mut later = ctx();
    assert_eq!(
        engine.evaluate(&name("boese.example.com."), peer(), &mut later),
        Decision::Allow,
        "die Sperre läuft nicht ab"
    );
}

#[test]
fn a_grant_beats_a_denial() {
    // Beides kommt von einem Menschen; die Freigabe ist die ausdrücklichere
    // Anordnung — sie ist das Mittel, einen Fehlgriff zu korrigieren.
    let clock = Arc::new(TestClock::new());
    let engine = engine(Detectors::new(), &clock);
    engine
        .denied()
        .grant("strittig.example.com", Duration::from_secs(300));
    engine
        .temporary()
        .grant("strittig.example.com", Duration::from_secs(300));

    let mut ctx = ctx();
    assert_eq!(
        engine.evaluate(&name("strittig.example.com."), peer(), &mut ctx),
        Decision::Allow
    );
}

// ---------------------------------------------------------------------------
// Schritt 2: Rebinding durch die Pipeline
// ---------------------------------------------------------------------------

/// Ein Upstream, der eine feste Adresse zurückgibt.
#[derive(Debug)]
struct FakeUpstream(IpAddr);

impl ResolveBackend for FakeUpstream {
    fn resolve(
        &self,
        request: &Message,
        _ctx: &mut Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let request = request.clone();
        let address = self.0;
        async move {
            let mut response = Message::response(request.metadata.id, OpCode::Query);
            response.add_queries(request.queries.iter().cloned());
            if let Some(query) = request.queries.first() {
                let data = match address {
                    IpAddr::V4(v4) => RData::A(A(v4)),
                    IpAddr::V6(v6) => RData::AAAA(hickory_proto::rr::rdata::AAAA(v6)),
                };
                response
                    .answers
                    .push(Record::from_rdata(query.name().clone(), 60, data));
            }
            Ok(response)
        }
    }
}

fn question(text: &str) -> Message {
    let mut message = Message::new(0x1234, MessageType::Query, OpCode::Query);
    message.add_query(Query::query(name(text), RecordType::A));
    message
}

async fn ask(detectors: Detectors, upstream: IpAddr, asked: &str) -> (Message, Vec<String>) {
    let clock = Arc::new(TestClock::new());
    let engine = Arc::new(engine(detectors, &clock));
    let backend = PolicyBackend::new(engine, FakeUpstream(upstream));
    let mut ctx = ctx();
    let response = backend
        .resolve(&question(asked), &mut ctx)
        .await
        .expect("beantwortet");
    let steps = ctx.steps().iter().map(ToString::to_string).collect();
    (response, steps)
}

fn rebinding_detectors(action: Action, allow: &[&str]) -> Detectors {
    Detectors::new().with_answer(
        rebinding::Rebinding::new(allow.iter().map(|zone| name(zone)).collect()),
        action,
        0,
    )
}

#[tokio::test]
async fn rebinding_sees_the_answer_not_the_question() {
    // Der Punkt von Schritt 2: die Frage ist unauffällig, erst die Antwort
    // verrät den Angriff. Ein Detektor, der nur den Namen sieht, fände hier
    // nichts.
    let (response, steps) = ask(
        rebinding_detectors(Action::Block, &[]),
        "192.168.1.1".parse().expect("Adresse"),
        "boese.example.com.",
    )
    .await;

    assert_eq!(response.metadata.response_code, ResponseCode::NXDomain);
    assert!(
        response.answers.is_empty(),
        "die Adresse ging trotzdem raus"
    );
    assert!(
        steps.iter().any(|step| step.contains("192.168.1.1")),
        "{steps:?}"
    );
}

#[tokio::test]
async fn a_public_answer_passes_untouched() {
    let (response, steps) = ask(
        rebinding_detectors(Action::Block, &[]),
        "93.184.216.34".parse().expect("Adresse"),
        "example.com.",
    )
    .await;

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1);
    assert!(
        !steps.iter().any(|step| step.contains("Rebinding")),
        "{steps:?}"
    );
}

#[tokio::test]
async fn the_rebinding_allow_list_works_through_the_pipeline() {
    // Das Abnahmekriterium aus Schritt 2.
    let (response, _) = ask(
        rebinding_detectors(Action::Block, &["home.arpa."]),
        "10.0.0.7".parse().expect("Adresse"),
        "drucker.home.arpa.",
    )
    .await;

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1, "die LAN-Antwort wurde geblockt");
}

#[tokio::test]
async fn rebinding_on_flag_reports_but_lets_the_answer_through() {
    // Der Auslieferungszustand: melden, nicht blocken.
    let (response, steps) = ask(
        rebinding_detectors(Action::Flag, &[]),
        "192.168.1.1".parse().expect("Adresse"),
        "boese.example.com.",
    )
    .await;

    assert_eq!(response.metadata.response_code, ResponseCode::NoError);
    assert_eq!(response.answers.len(), 1, "flag hat geblockt");
    assert!(
        steps.iter().any(|step| step.contains("192.168.1.1")),
        "der Fund fehlt im Trace: {steps:?}"
    );
}

// ---------------------------------------------------------------------------
// Aufbau aus der Konfiguration
// ---------------------------------------------------------------------------

#[test]
fn the_shipped_defaults_flag_and_never_block() {
    // Die Zusage aus der Roadmap und aus CLAUDE.md B.8. Wenn dieser Test
    // fehlschlägt, blockt eine Heuristik ab dem ersten Start.
    let config = alpendns::config::DetectionConfig::default();
    let detectors =
        alpendns::detect::from_config(&config, Vec::new(), Arc::new(TestClock::new()), wall());
    for (detector, action) in detectors.configured() {
        assert_ne!(
            action,
            Action::Block,
            "{detector:?} blockt im Auslieferungszustand"
        );
        assert_eq!(action, Action::Flag, "{detector:?}");
    }
}

#[test]
fn detectors_without_their_data_are_not_switched_on() {
    // Ein Typosquat-Wächter ohne Schutzliste und ein NRD-Detektor ohne Datei
    // können nichts finden. Sie als eingeschaltet zu melden wäre eine
    // Zusicherung, die nicht eintritt.
    let detectors = alpendns::detect::from_config(
        &alpendns::config::DetectionConfig::default(),
        Vec::new(),
        Arc::new(TestClock::new()),
        wall(),
    );
    let configured: Vec<Detector> = detectors
        .configured()
        .into_iter()
        .map(|(detector, _)| detector)
        .collect();
    assert!(!configured.contains(&Detector::Typosquat));
    assert!(!configured.contains(&Detector::Nrd));
    // Die drei ohne Datenbedarf laufen dagegen.
    for expected in [Detector::Dga, Detector::Tunneling, Detector::Rebinding] {
        assert!(configured.contains(&expected), "{expected:?} fehlt");
    }
}

#[test]
fn forward_zones_are_exempt_from_rebinding_without_being_configured() {
    // Die Falle, die es nicht geben soll: wer eine Zone ins eigene Netz
    // leitet, hat schon gesagt, dass private Adressen von dort in Ordnung sind.
    let config: alpendns::config::Config = toml::from_str(
        r#"
[server]
listen_udp = ["127.0.0.1:5353"]

[[upstream_pool]]
name = "default"

[[upstream_pool.resolver]]
name = "quad9"
addr = "dot://9.9.9.9:853"
tls_name = "dns.quad9.net"

[[forward_zone]]
zone = "home.arpa"
upstream = "udp://10.0.0.1:53"
"#,
    )
    .expect("Konfiguration parst");

    let zones = config.rebinding_allow_zones();
    assert!(
        zones.iter().any(|zone| zone == &name("home.arpa.")),
        "{zones:?}"
    );
}
