//! Integrationstests für Clients, Policies, Zeitpläne und Freigaben.

// Testcode darf panicken, siehe B.1 und clippy.toml.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use alpendns::clock::{FixedWallClock, TestClock};
use alpendns::config::BlockingConfig;
use alpendns::filter::LoadedLists;
use alpendns::filter::matcher::Builder as MatcherBuilder;
use alpendns::filter::parser::{Format, parse};
use alpendns::policy::clients::ClientRule;
use alpendns::policy::rules::RegexRules;
use alpendns::policy::schedule::Schedule;
use alpendns::policy::{Blueprint, Decision, Engine, PolicyBlueprint};
use alpendns::trace::{Ctx, MatchKind, ScheduleEffect, Step};
use hickory_proto::rr::Name;
use jiff::civil::{Time, Weekday};

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

fn policy(
    name: &str,
    blocklists: &[&str],
    allowlists: &[&str],
    regex: &[&str],
    schedules: Vec<Schedule>,
) -> PolicyBlueprint {
    let patterns: Vec<String> = regex.iter().map(|p| (*p).to_owned()).collect();
    PolicyBlueprint {
        name: Arc::from(name),
        blocklists: blocklists.iter().map(|n| Arc::from(*n)).collect(),
        allowlists: allowlists.iter().map(|n| Arc::from(*n)).collect(),
        regex: Arc::new(RegexRules::compile(name, &patterns).expect("gültige Muster")),
        schedules,
    }
}

fn client(name: &str, address: &str, policy: &str) -> ClientRule {
    ClientRule {
        name: Arc::from(name),
        nets: vec![alpendns::config::parse_net(address).expect("gültige Adresse")],
        policy: Arc::from(policy),
    }
}

fn engine(
    clients: Vec<ClientRule>,
    policies: Vec<PolicyBlueprint>,
    loaded: &LoadedLists,
) -> (Arc<TestClock>, Arc<FixedWallClock>, TestEngine) {
    let clock = Arc::new(TestClock::new());
    let wall = Arc::new(FixedWallClock::new(
        // Ein Montagmittag, an dem kein Zeitplan greift.
        "2026-08-31T12:00:00[Europe/Vienna]"
            .parse()
            .expect("gültiger Zeitpunkt"),
    ));
    let blueprint = Blueprint::new(clients, Arc::from("default"), policies);
    let entries = loaded.total_entries();
    let engine = Engine::new(
        blueprint.build(loaded).expect("Regelstand"),
        entries,
        &BlockingConfig::default(),
        Arc::clone(&clock),
        Arc::clone(&wall),
    );
    (clock, wall, engine)
}

fn ask(engine: &TestEngine, domain: &str, from: &str) -> (Decision, Ctx) {
    let peer: IpAddr = from.parse().expect("gültige Adresse");
    let mut ctx = Ctx::new(SocketAddr::new(peer, 4242));
    let name = Name::from_ascii(domain).expect("gültiger Name");
    let decision = engine.evaluate(&name, peer, &mut ctx);
    (decision, ctx)
}

fn bedtime(days: &[Weekday]) -> Schedule {
    Schedule {
        name: Arc::from("bedtime"),
        days: days.to_vec(),
        from: Time::new(21, 0, 0, 0).expect("gültig"),
        to: Time::new(7, 0, 0, 0).expect("gültig"),
        effect: ScheduleEffect::BlockAllExceptAllowlist,
    }
}

// ---------------------------------------------------------------------------

#[test]
fn two_source_addresses_get_different_verdicts() {
    // Roadmap Schritt 1.
    let loaded = lists(&[("spiele", "spiel.example\n")]);
    let (_, _, engine) = engine(
        vec![client("kids-tablet", "10.0.10.42", "kids")],
        vec![
            policy("default", &[], &[], &[], Vec::new()),
            policy("kids", &["spiele"], &[], &[], Vec::new()),
        ],
        &loaded,
    );

    assert_eq!(
        ask(&engine, "spiel.example.", "10.0.10.42").0,
        Decision::Block
    );
    assert_eq!(
        ask(&engine, "spiel.example.", "10.0.10.43").0,
        Decision::Allow
    );
}

#[test]
fn an_unknown_client_gets_the_default_policy() {
    let loaded = lists(&[("werbung", "ads.example\n")]);
    let (_, _, engine) = engine(
        vec![client("laptop", "10.0.0.5", "offen")],
        vec![
            policy("default", &["werbung"], &[], &[], Vec::new()),
            policy("offen", &[], &[], &[], Vec::new()),
        ],
        &loaded,
    );

    let (decision, ctx) = ask(&engine, "ads.example.", "192.168.1.1");
    assert_eq!(decision, Decision::Block);
    assert!(matches!(
        ctx.steps().first(),
        Some(Step::ClientMatched {
            by: MatchKind::Default,
            ..
        })
    ));
}

/// Roadmap Schritt 2: die Auswertungsreihenfolge über alle Kombinationen.
#[test]
fn the_evaluation_order_holds_for_every_combination() {
    let loaded = lists(&[
        ("block", "beides.example\nnurblock.example\n"),
        ("allow", "beides.example\nnurallow.example\n"),
    ]);

    // (Domain, auf Blockliste, auf Allowlist, trifft Regex, erwartetes Verdikt)
    let cases: &[(&str, bool, bool, bool, Decision)] = &[
        ("frei.example.", false, false, false, Decision::Allow),
        ("nurblock.example.", true, false, false, Decision::Block),
        ("nurallow.example.", false, true, false, Decision::Allow),
        // Der Kern: auf beiden Listen gewinnt die Allowlist.
        ("beides.example.", true, true, false, Decision::Allow),
        (
            "regex-treffer.example.",
            false,
            false,
            true,
            Decision::Block,
        ),
    ];

    let (_, _, engine) = engine(
        Vec::new(),
        vec![policy(
            "default",
            &["block"],
            &["allow"],
            &[r"^regex-treffer\."],
            Vec::new(),
        )],
        &loaded,
    );

    for (domain, on_block, on_allow, on_regex, expected) in cases {
        let (decision, ctx) = ask(&engine, domain, "10.0.0.1");
        assert_eq!(
            decision,
            *expected,
            "{domain} (block={on_block} allow={on_allow} regex={on_regex}): {}",
            ctx.explain()
        );
    }
}

/// Roadmap Schritt 3: die Schrittfolge bei einem Blocklisten-Treffer.
#[test]
fn the_trace_records_every_step_of_a_block() {
    let loaded = lists(&[("stevenblack", "# Kopf\nads.example.com\n")]);
    let (_, _, engine) = engine(
        vec![client("kids-tablet", "10.0.10.42", "kids")],
        vec![
            policy("default", &[], &[], &[], Vec::new()),
            policy("kids", &["stevenblack"], &[], &[], Vec::new()),
        ],
        &loaded,
    );

    let (decision, ctx) = ask(&engine, "sub.ads.example.com.", "10.0.10.42");
    assert_eq!(decision, Decision::Block);

    let steps = ctx.steps();
    assert_eq!(
        steps.len(),
        4,
        "unerwartete Schrittfolge: {}",
        ctx.explain()
    );
    assert!(matches!(
        steps.first(),
        Some(Step::ClientMatched {
            by: MatchKind::Address,
            ..
        })
    ));
    match steps.get(1) {
        Some(Step::PolicyApplied { policy }) => assert_eq!(&**policy, "kids"),
        other => panic!("erwartet war PolicyApplied, nicht {other:?}"),
    }
    match steps.get(2) {
        Some(Step::BlocklistHit {
            list,
            line,
            matched,
        }) => {
            assert_eq!(&**list, "stevenblack");
            assert_eq!(*line, 2);
            assert_eq!(matched, "ads.example.com", "der Eintrag, nicht die Frage");
        }
        other => panic!("erwartet war BlocklistHit, nicht {other:?}"),
    }
    assert!(matches!(steps.get(3), Some(Step::Synthesized { .. })));

    // Die Erklärung muss ohne Konfiguration daneben lesbar sein.
    let text = ctx.explain();
    assert!(text.contains("kids-tablet"), "{text}");
    assert!(text.contains("Zeile 2"), "{text}");
}

/// Roadmap Schritt 4: derselbe Query zu zwei simulierten Uhrzeiten.
#[test]
fn the_same_query_is_decided_differently_at_two_times() {
    let loaded = lists(&[("schule", "lernen.example\n")]);
    let (_, wall, engine) = engine(
        vec![client("kids-tablet", "10.0.10.42", "kids")],
        vec![
            policy("default", &[], &[], &[], Vec::new()),
            policy(
                "kids",
                &[],
                &["schule"],
                &[],
                vec![bedtime(&[Weekday::Monday])],
            ),
        ],
        &loaded,
    );

    // Montag 12:00 — kein Fenster aktiv.
    assert_eq!(
        ask(&engine, "spiel.example.", "10.0.10.42").0,
        Decision::Allow
    );

    // Montag 22:00 — Bettzeit.
    wall.set(
        "2026-08-31T22:00:00[Europe/Vienna]"
            .parse()
            .expect("gültig"),
    );
    let (decision, ctx) = ask(&engine, "spiel.example.", "10.0.10.42");
    assert_eq!(decision, Decision::Block, "{}", ctx.explain());
    assert!(
        ctx.steps()
            .iter()
            .any(|step| matches!(step, Step::ScheduleHit { .. })),
        "der Zeitplan taucht nicht in der Begründung auf: {}",
        ctx.explain()
    );
}

#[test]
fn the_allowlist_still_works_during_a_schedule() {
    // Das ist der Sinn von block_all_except_allowlist.
    let loaded = lists(&[("schule", "lernen.example\n")]);
    let (_, wall, engine) = engine(
        Vec::new(),
        vec![policy(
            "default",
            &[],
            &["schule"],
            &[],
            vec![bedtime(&[Weekday::Monday])],
        )],
        &loaded,
    );
    wall.set(
        "2026-08-31T22:00:00[Europe/Vienna]"
            .parse()
            .expect("gültig"),
    );

    assert_eq!(
        ask(&engine, "lernen.example.", "10.0.0.1").0,
        Decision::Allow
    );
    assert_eq!(
        ask(&engine, "spiel.example.", "10.0.0.1").0,
        Decision::Block
    );
}

/// Roadmap Schritt 5: Freigabe für 60 s, danach wieder geblockt.
#[test]
fn a_temporary_grant_expires() {
    let loaded = lists(&[("block", "gesperrt.example\n")]);
    let (clock, _, engine) = engine(
        Vec::new(),
        vec![policy("default", &["block"], &[], &[], Vec::new())],
        &loaded,
    );

    assert_eq!(
        ask(&engine, "gesperrt.example.", "10.0.0.1").0,
        Decision::Block
    );

    engine
        .temporary()
        .grant("gesperrt.example", Duration::from_secs(60));
    let (decision, ctx) = ask(&engine, "gesperrt.example.", "10.0.0.1");
    assert_eq!(decision, Decision::Allow);
    assert!(
        ctx.steps()
            .iter()
            .any(|step| matches!(step, Step::TemporaryAllow { .. })),
        "die Freigabe fehlt in der Begründung: {}",
        ctx.explain()
    );

    clock.advance(Duration::from_secs(61));
    assert_eq!(
        ask(&engine, "gesperrt.example.", "10.0.0.1").0,
        Decision::Block,
        "die Freigabe lief nicht ab"
    );
}

#[test]
fn a_temporary_grant_covers_subdomains() {
    let loaded = lists(&[("block", "gesperrt.example\n")]);
    let (_, _, engine) = engine(
        Vec::new(),
        vec![policy("default", &["block"], &[], &[], Vec::new())],
        &loaded,
    );
    engine
        .temporary()
        .grant("gesperrt.example", Duration::from_secs(60));
    assert_eq!(
        ask(&engine, "cdn.gesperrt.example.", "10.0.0.1").0,
        Decision::Allow
    );
}

/// Roadmap Schritt 6: Regex-Regeln pro Policy.
#[test]
fn regex_rules_apply_only_to_their_own_policy() {
    let loaded = LoadedLists::default();
    let (_, _, engine) = engine(
        vec![client("kids-tablet", "10.0.10.42", "kids")],
        vec![
            policy("default", &[], &[], &[], Vec::new()),
            policy("kids", &[], &[], &[r"(?:^|\.)spiele\."], Vec::new()),
        ],
        &loaded,
    );

    assert_eq!(
        ask(&engine, "spiele.example.", "10.0.10.42").0,
        Decision::Block
    );
    assert_eq!(
        ask(&engine, "www.spiele.example.", "10.0.10.42").0,
        Decision::Block
    );
    assert_eq!(
        ask(&engine, "spiele.example.", "10.0.0.1").0,
        Decision::Allow,
        "die Regel einer Policy wirkte auf eine andere"
    );
}

#[test]
fn the_trace_names_the_regex_that_matched() {
    let loaded = LoadedLists::default();
    let (_, _, engine) = engine(
        Vec::new(),
        vec![policy("default", &[], &[], &[r"^ads\."], Vec::new())],
        &loaded,
    );
    let (_, ctx) = ask(&engine, "ads.example.com.", "10.0.0.1");
    assert!(ctx.explain().contains(r"/^ads\./"), "{}", ctx.explain());
}

#[test]
fn a_policy_that_does_not_exist_blocks_nothing_instead_of_everything() {
    // Ein Tippfehler im Policy-Namen darf nicht dazu führen, dass ein Gerät
    // plötzlich gar nichts mehr erreicht. Die Konfigurationsprüfung fängt das
    // ab; hier zählt, dass auch der Notnagel harmlos ist.
    let loaded = lists(&[("block", "ads.example\n")]);
    let (_, _, engine) = engine(
        vec![client("laptop", "10.0.0.5", "gibtsnicht")],
        vec![policy("default", &["block"], &[], &[], Vec::new())],
        &loaded,
    );
    assert_eq!(ask(&engine, "ads.example.", "10.0.0.5").0, Decision::Allow);
    assert_eq!(ask(&engine, "ads.example.", "10.0.0.6").0, Decision::Block);
}

#[test]
fn shared_lists_are_not_duplicated_between_policies() {
    // Zwei Policies mit derselben Liste sollen sich den Matcher teilen.
    let loaded = lists(&[("gross", "ads.example\n")]);
    let matcher = loaded.get("gross").expect("Liste");
    let (_, _, engine) = engine(
        vec![client("a", "10.0.0.5", "eins")],
        vec![
            policy("default", &["gross"], &[], &[], Vec::new()),
            policy("eins", &["gross"], &[], &[], Vec::new()),
        ],
        &loaded,
    );
    assert_eq!(ask(&engine, "ads.example.", "10.0.0.5").0, Decision::Block);
    assert_eq!(ask(&engine, "ads.example.", "10.0.0.6").0, Decision::Block);
    assert!(
        Arc::strong_count(&matcher) >= 3,
        "die Policies halten offenbar eigene Kopien statt sich den Matcher zu teilen"
    );
}
