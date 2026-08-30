//! Heuristiken: Muster erkennen, die keine Liste kennt.
//!
//! Alles lokal, alles erklärbar, alles per Default nur `flag`. Die drei Wörter
//! sind das Ziel von Phase 8 und stehen nicht zufällig in dieser Reihenfolge.
//!
//! **Lokal** heißt: kein Dienst wird gefragt, kein Modell nachgeladen. Das
//! N-Gramm-Modell der DGA-Erkennung liegt im Binary, die NRD-Liste ist eine
//! Datei auf der Platte (FEATURES.md D5: bewusst eine Datei und kein
//! API-Aufruf, damit der Resolver nicht von einem zweiten Dienst abhängt).
//!
//! **Erklärbar** heißt: ein Detektor liefert nie nur einen Score, sondern die
//! Merkmale, die dazu geführt haben (FEATURES.md D6). Ohne das ist ein
//! Falsch-Positiv nicht debugbar, und wer es nicht debuggen kann, schaltet das
//! Feature ab statt es zu verbessern. Deshalb ist [`Finding::reason`] keine
//! Option, sondern Pflichtfeld.
//!
//! **Per Default `flag`** heißt: keiner dieser Detektoren blockt, bevor jemand
//! eine Woche lang zugesehen hat, was er findet (Roadmap, Abnahme Phase 8, und
//! CLAUDE.md B.8). Ein Detektor, der Internet kaputtmacht, wird abgeschaltet —
//! und mit ihm alle anderen.
//!
//! # Zwei Sorten Detektor, und warum es zwei sind
//!
//! [`NameDetector`] sieht die **Frage**, [`AnswerDetector`] die **Antwort**.
//! Das ist keine Geschmacksfrage: der Rebinding-Schutz prüft, ob eine private
//! IP-Adresse für einen öffentlichen Namen zurückkommt, und dafür muss die
//! Auflösung schon gelaufen sein (ARCHITECTURE.md §1, Schicht 5). Ein
//! gemeinsamer Trait mit einem `Option<&Message>` würde verschweigen, dass die
//! beiden an verschiedenen Stellen der Pipeline laufen.

pub mod dga;
pub mod nrd;
pub mod rebinding;
pub mod tunneling;
pub mod typosquat;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use hickory_proto::op::Message;
use hickory_proto::rr::RecordType;

/// Welcher Detektor angeschlagen hat.
///
/// Eine geschlossene Aufzählung: die Werte landen als Metrik-Label und in der
/// API, und dort darf nichts stehen, was aus einer Anfrage stammt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Detector {
    Dga,
    Tunneling,
    Rebinding,
    Typosquat,
    Nrd,
}

impl Detector {
    pub const ALL: [Self; 5] = [
        Self::Dga,
        Self::Tunneling,
        Self::Rebinding,
        Self::Typosquat,
        Self::Nrd,
    ];

    /// Der Name aus der Konfiguration. Auch der Metrik-Label-Wert.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dga => "dga",
            Self::Tunneling => "tunneling",
            Self::Rebinding => "rebinding",
            Self::Typosquat => "typosquat",
            Self::Nrd => "nrd",
        }
    }

    /// Wie der Detektor in der Oberfläche heißt.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dga => "Algorithmisch erzeugter Name",
            Self::Tunneling => "Tunneling",
            Self::Rebinding => "DNS-Rebinding",
            Self::Typosquat => "Typosquatting",
            Self::Nrd => "Neu registriert",
        }
    }
}

impl std::fmt::Display for Detector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Was mit einem Treffer geschieht.
///
/// Die Stufen sind bewusst vier und nicht zwei. `log` und `flag` unterscheiden
/// sich darin, wem sie auffallen: `log` zählt nur mit, `flag` stellt die
/// Anfrage zusätzlich in die Liste der auffälligen Anfragen, die sich jemand
/// ansieht. Wer eine Heuristik erst kennenlernen will, nimmt `log`; wer sie
/// beurteilen will, `flag`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Action {
    /// Der Detektor läuft nicht. Kostet nichts.
    Off,
    /// Läuft und zählt mit, sonst nichts.
    Log,
    /// Läuft, zählt mit, und die Anfrage erscheint als auffällig. Default.
    #[default]
    Flag,
    /// Die Anfrage wird geblockt wie ein Blocklisten-Treffer.
    Block,
}

impl Action {
    pub const ALL: [Self; 4] = [Self::Off, Self::Log, Self::Flag, Self::Block];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Log => "log",
            Self::Flag => "flag",
            Self::Block => "block",
        }
    }

    pub const fn is_off(self) -> bool {
        matches!(self, Self::Off)
    }

    pub const fn blocks(self) -> bool {
        matches!(self, Self::Block)
    }

    /// Ob die Anfrage in der Liste der auffälligen Anfragen erscheint.
    ///
    /// Auch `block` gehört dazu: gerade eine geblockte Anfrage will man sehen.
    pub const fn is_visible(self) -> bool {
        matches!(self, Self::Flag | Self::Block)
    }
}

impl<'de> serde::Deserialize<'de> for Action {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        match text.as_str() {
            "off" => Ok(Self::Off),
            "log" => Ok(Self::Log),
            "flag" => Ok(Self::Flag),
            "block" => Ok(Self::Block),
            other => Err(serde::de::Error::custom(format!(
                "unbekannte Aktion '{other}' — erlaubt sind off, log, flag, block"
            ))),
        }
    }
}

/// Ein Score als Promille, 0 bis 1000.
///
/// Nicht als `f32`, obwohl die Konfiguration `0.85` schreibt und FEATURES.md
/// von 0.0–1.0 spricht. Der Wert landet in [`crate::trace::Step`], und der ist
/// `Eq` — ein `f32` darin ginge nicht, ohne die Ableitung im ganzen Trace
/// aufzugeben. Promille sind außerdem in der Ausgabe eindeutig: `0.873` ist
/// dasselbe auf jeder Maschine, `0.8730000257` nicht.
pub type Permille = u16;

/// Rechnet einen Score aus der Konfiguration in Promille um, geklemmt auf 0–1000.
pub fn permille(score: f32) -> Permille {
    // NaN zuerst: es ist weder größer noch kleiner als irgendetwas, und ohne
    // diesen Zweig fiele es durch beide Vergleiche.
    if score.is_nan() || score <= 0.0 {
        return 0;
    }
    if score >= 1.0 {
        return 1000;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "der Wert liegt nach den beiden Zweigen darüber echt zwischen 0 und 1000"
    )]
    {
        (score * 1000.0).round() as Permille
    }
}

/// Score als Text, so wie ihn die Konfiguration schreibt: `0.873`.
#[expect(
    clippy::integer_division,
    reason = "Ganzzahldivision ist hier der Zweck: Promille in Vor- und Nachkommateil zerlegen"
)]
pub fn format_score(score: Permille) -> String {
    format!("{}.{:03}", score / 1000, score % 1000)
}

/// Was ein Detektor gefunden hat.
///
/// `reason` ist Pflicht und keine Option — siehe FEATURES.md D6. Er darf den
/// Query-Namen enthalten: ein `Finding` wandert in den [`crate::trace::Ctx`],
/// und der unterliegt derselben Regel wie alles andere darin (B.1 Regel 3) —
/// was davon den Prozess verlässt, entscheidet allein die Logging-Schicht.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub detector: Detector,
    pub score: Permille,
    /// Die Merkmale, die zum Score geführt haben, in einem Satz.
    pub reason: Arc<str>,
}

impl Finding {
    pub fn new(detector: Detector, score: Permille, reason: impl Into<Arc<str>>) -> Self {
        Self {
            detector,
            score,
            reason: reason.into(),
        }
    }
}

/// Was ein [`NameDetector`] zu sehen bekommt.
///
/// Der Name ist schon normalisiert (klein, ohne abschließenden Punkt), damit
/// nicht jeder Detektor dasselbe noch einmal macht. Der Typ steht dabei, weil
/// die Tunneling-Erkennung den Anteil an TXT- und NULL-Anfragen je Zone
/// braucht (FEATURES.md D2).
#[derive(Debug, Clone, Copy)]
pub struct Observation<'a> {
    pub name: &'a str,
    pub query_type: RecordType,
}

/// Ein Detektor, der die Frage ansieht.
pub trait NameDetector: Send + Sync + std::fmt::Debug {
    fn detector(&self) -> Detector;

    /// `None` heißt: nichts Auffälliges. Ein `Some` unterhalb der Schwelle
    /// filtert [`Detectors`] heraus — ein Detektor muss die Schwelle nicht
    /// selbst kennen.
    fn inspect(&self, observation: &Observation<'_>) -> Option<Finding>;
}

/// Ein Detektor, der die Antwort ansieht.
///
/// Es gibt genau einen: den Rebinding-Schutz. Der Trait steht trotzdem da,
/// weil die Stelle in der Pipeline eine andere ist als bei [`NameDetector`] und
/// das im Typ sichtbar sein soll.
pub trait AnswerDetector: Send + Sync + std::fmt::Debug {
    fn detector(&self) -> Detector;

    fn inspect(&self, name: &str, response: &Message) -> Option<Finding>;
}

/// Ein Detektor plus das, was die Konfiguration über ihn sagt.
#[derive(Debug)]
pub struct Configured<D> {
    pub detector: D,
    pub action: Action,
    /// Unterhalb dieser Schwelle gilt ein Treffer als nicht auffällig genug.
    pub threshold: Permille,
}

impl<D> Configured<D> {
    /// Ob ein Fund zählt: Detektor an und Score über der Schwelle.
    const fn accepts(&self, finding: &Finding) -> bool {
        !self.action.is_off() && finding.score >= self.threshold
    }
}

/// Alle konfigurierten Detektoren.
///
/// Hält sie als Trait-Objekte statt als benanntes Feld je Detektor: die
/// Auswertung ist für alle dieselbe Schleife, und ein sechster Detektor soll
/// hier nichts ändern müssen außer einer Zeile beim Aufbau.
#[derive(Debug, Default)]
pub struct Detectors {
    names: Vec<Configured<Box<dyn NameDetector>>>,
    answers: Vec<Configured<Box<dyn AnswerDetector>>>,
}

impl Detectors {
    pub const fn new() -> Self {
        Self {
            names: Vec::new(),
            answers: Vec::new(),
        }
    }

    /// Fügt einen Detektor hinzu. `Off` wird gar nicht erst aufgenommen — ein
    /// abgeschalteter Detektor soll im Anfragepfad nicht einmal als
    /// übersprungener Eintrag auftauchen.
    pub fn with_name(
        mut self,
        detector: impl NameDetector + 'static,
        action: Action,
        threshold: Permille,
    ) -> Self {
        if !action.is_off() {
            self.names.push(Configured {
                detector: Box::new(detector),
                action,
                threshold,
            });
        }
        self
    }

    pub fn with_answer(
        mut self,
        detector: impl AnswerDetector + 'static,
        action: Action,
        threshold: Permille,
    ) -> Self {
        if !action.is_off() {
            self.answers.push(Configured {
                detector: Box::new(detector),
                action,
                threshold,
            });
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.answers.is_empty()
    }

    /// Alle eingeschalteten Detektoren mit ihrer Aktion, für Status und Metrik.
    pub fn configured(&self) -> Vec<(Detector, Action)> {
        let mut all: Vec<(Detector, Action)> = self
            .names
            .iter()
            .map(|entry| (entry.detector.detector(), entry.action))
            .chain(
                self.answers
                    .iter()
                    .map(|entry| (entry.detector.detector(), entry.action)),
            )
            .collect();
        all.sort_unstable();
        all
    }

    /// Lässt alle Namens-Detektoren über eine Frage laufen.
    ///
    /// **Alle**, nicht bis zum ersten Treffer: wer wissen will, ob eine Domain
    /// sowohl algorithmisch erzeugt *als auch* frisch registriert ist, braucht
    /// beide Funde. Die Reihenfolge des Ergebnisses ist die der Konfiguration.
    pub fn inspect_name(&self, observation: &Observation<'_>) -> Vec<(Finding, Action)> {
        self.names
            .iter()
            .filter_map(|entry| {
                let finding = entry.detector.inspect(observation)?;
                entry.accepts(&finding).then_some((finding, entry.action))
            })
            .collect()
    }

    /// Lässt alle Antwort-Detektoren über eine Antwort laufen.
    pub fn inspect_answer(&self, name: &str, response: &Message) -> Vec<(Finding, Action)> {
        self.answers
            .iter()
            .filter_map(|entry| {
                let finding = entry.detector.inspect(name, response)?;
                entry.accepts(&finding).then_some((finding, entry.action))
            })
            .collect()
    }
}

/// Wie oft welcher Detektor angeschlagen hat.
///
/// Prozessweit und atomar, aus demselben Grund wie bei [`crate::privacy`] und
/// [`crate::dnssec`]: die Detektoren liegen hinter Trait-Objekten, und einen
/// Zähler durch jeden davon zu fädeln wäre Aufwand für eine Addition.
///
/// Gezählt wird **oberhalb der Schwelle**, also das, was tatsächlich zu einem
/// Eintrag im Trace geführt hat. Ein Detektor, der rechnet und nichts findet,
/// erscheint hier nicht — sonst wäre die Zahl die der Anfragen und nicht die
/// der Funde.
static DETECTIONS: [AtomicU64; Detector::ALL.len()] =
    [const { AtomicU64::new(0) }; Detector::ALL.len()];

/// Trägt einen Fund in die Statistik ein.
pub fn record(detector: Detector) {
    if let Some(counter) = Detector::ALL
        .iter()
        .position(|&known| known == detector)
        .and_then(|index| DETECTIONS.get(index))
    {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// Der Stand aller Detektor-Zähler, in der Reihenfolge von [`Detector::ALL`].
pub fn counters() -> Vec<(Detector, u64)> {
    Detector::ALL
        .iter()
        .enumerate()
        .map(|(index, &detector)| {
            (
                detector,
                DETECTIONS
                    .get(index)
                    .map_or(0, |counter| counter.load(Ordering::Relaxed)),
            )
        })
        .collect()
}

/// Der schärfste Fund einer Liste: erst `block` vor `flag`, dann der höhere Score.
///
/// Was in den Trace als *entscheidender* Schritt gehört, wenn mehrere Detektoren
/// gleichzeitig angeschlagen haben.
pub fn strongest(findings: &[(Finding, Action)]) -> Option<&(Finding, Action)> {
    findings.iter().max_by(|left, right| {
        left.1
            .blocks()
            .cmp(&right.1.blocks())
            .then(left.0.score.cmp(&right.0.score))
    })
}

/// Baut die Detektoren aus der Konfiguration.
///
/// Steht hier und nicht in `main.rs`, damit ein Integrationstest dieselbe
/// Zusammenstellung bekommt wie der Betrieb — ein Detektor, der nur im
/// Binary eingehängt wird, ist im Test nicht derselbe.
///
/// Zwei Detektoren hängen an Daten, die fehlen dürfen: Typosquat braucht eine
/// Schutzliste, NRD eine Datei. Ohne sie werden sie **gar nicht erst
/// eingehängt** — sonst stünde in Status und Metrik ein eingeschalteter
/// Detektor, der nie etwas finden kann.
pub fn from_config<C, W>(
    config: &crate::config::DetectionConfig,
    rebinding_allow_zones: Vec<hickory_proto::rr::Name>,
    clock: C,
    wall: W,
) -> Detectors
where
    C: crate::clock::Clock,
    W: crate::clock::WallClock,
{
    let mut detectors = Detectors::new()
        .with_name(
            dga::Dga::new(),
            config.dga.action,
            permille(config.dga.threshold),
        )
        .with_name(
            tunneling::Tunneling::new(
                config.tunneling.window,
                &config.tunneling.allow_zones,
                clock,
            ),
            config.tunneling.action,
            permille(config.tunneling.threshold),
        )
        .with_answer(
            rebinding::Rebinding::new(rebinding_allow_zones),
            config.rebinding.action,
            0,
        );

    let typosquat = typosquat::Typosquat::new(&config.typosquat.protect);
    if typosquat.is_empty() {
        if !config.typosquat.action.is_off() {
            tracing::info!(
                "Typosquat-Wächter bleibt aus: detection.typosquat.protect ist leer, es gibt \
                 nichts zu schützen"
            );
        }
    } else {
        detectors = detectors.with_name(
            typosquat,
            config.typosquat.action,
            permille(config.typosquat.threshold),
        );
    }

    match config.nrd.source.as_deref() {
        Some(path) => match nrd::Nrd::load(path, config.nrd.max_age, wall) {
            Ok(nrd) if !nrd.is_empty() => {
                detectors = detectors.with_name(nrd, config.nrd.action, 0);
            }
            Ok(_) => {
                tracing::info!("NRD-Detektor bleibt aus: die Liste ist leer");
            }
            Err(error) => {
                // Kein Startfehler: der Resolver hängt nicht davon ab, dass ein
                // zweiter Dienst gelaufen ist (B.1 Regel 6).
                tracing::warn!(%error, "NRD-Liste nicht lesbar; der Detektor bleibt aus");
            }
        },
        None => {
            if !config.nrd.action.is_off() {
                tracing::info!("NRD-Detektor bleibt aus: detection.nrd.source ist nicht gesetzt");
            }
        }
    }

    detectors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Dummy {
        detector: Detector,
        score: Permille,
    }

    impl NameDetector for Dummy {
        fn detector(&self) -> Detector {
            self.detector
        }

        fn inspect(&self, observation: &Observation<'_>) -> Option<Finding> {
            observation.name.contains("boese").then(|| {
                Finding::new(
                    self.detector,
                    self.score,
                    format!("Testdetektor, Name enthält 'boese': {}", observation.name),
                )
            })
        }
    }

    fn observe(name: &str) -> Observation<'_> {
        Observation {
            name,
            query_type: RecordType::A,
        }
    }

    #[test]
    fn a_detector_below_the_threshold_does_not_count() {
        let detectors = Detectors::new().with_name(
            Dummy {
                detector: Detector::Dga,
                score: 700,
            },
            Action::Flag,
            850,
        );
        assert!(detectors.inspect_name(&observe("boese.example")).is_empty());
    }

    #[test]
    fn a_detector_at_the_threshold_counts() {
        // Die Schwelle ist einschließend. Wer 0.85 konfiguriert, meint "ab
        // 0.85", nicht "über 0.85".
        let detectors = Detectors::new().with_name(
            Dummy {
                detector: Detector::Dga,
                score: 850,
            },
            Action::Flag,
            850,
        );
        assert_eq!(detectors.inspect_name(&observe("boese.example")).len(), 1);
    }

    #[test]
    fn a_detector_that_is_off_is_not_even_registered() {
        let detectors = Detectors::new().with_name(
            Dummy {
                detector: Detector::Dga,
                score: 1000,
            },
            Action::Off,
            0,
        );
        assert!(detectors.is_empty());
        assert!(detectors.configured().is_empty());
        assert!(detectors.inspect_name(&observe("boese.example")).is_empty());
    }

    #[test]
    fn every_detector_runs_not_just_the_first() {
        // Zwei Funde für denselben Namen sind der interessante Fall: eine
        // frisch registrierte Domain mit algorithmisch erzeugtem Namen.
        let detectors = Detectors::new()
            .with_name(
                Dummy {
                    detector: Detector::Dga,
                    score: 900,
                },
                Action::Flag,
                0,
            )
            .with_name(
                Dummy {
                    detector: Detector::Nrd,
                    score: 950,
                },
                Action::Log,
                0,
            );
        let found = detectors.inspect_name(&observe("boese.example"));
        assert_eq!(found.len(), 2);
        let detectors_found: Vec<Detector> =
            found.iter().map(|(finding, _)| finding.detector).collect();
        assert_eq!(detectors_found, vec![Detector::Dga, Detector::Nrd]);
    }

    #[test]
    fn blocking_wins_over_a_higher_score() {
        // Sonst stünde im Trace als entscheidender Schritt ein Detektor, der
        // die Anfrage gar nicht geblockt hat.
        let findings = vec![
            (Finding::new(Detector::Dga, 990, "hoch"), Action::Flag),
            (Finding::new(Detector::Nrd, 500, "niedrig"), Action::Block),
        ];
        let winner = strongest(&findings).expect("einer davon");
        assert_eq!(winner.0.detector, Detector::Nrd);

        // Unter gleicher Aktion gewinnt der höhere Score.
        let findings = vec![
            (Finding::new(Detector::Dga, 990, "hoch"), Action::Flag),
            (Finding::new(Detector::Nrd, 500, "niedrig"), Action::Flag),
        ];
        assert_eq!(
            strongest(&findings).expect("einer davon").0.detector,
            Detector::Dga
        );
    }

    #[test]
    fn nothing_is_found_in_an_unremarkable_name() {
        let detectors = Detectors::new().with_name(
            Dummy {
                detector: Detector::Dga,
                score: 1000,
            },
            Action::Flag,
            0,
        );
        assert!(detectors.inspect_name(&observe("example.com")).is_empty());
    }

    #[test]
    fn scores_convert_and_format_without_surprises() {
        assert_eq!(permille(0.85), 850);
        assert_eq!(permille(0.0), 0);
        assert_eq!(permille(1.0), 1000);
        // Werte außerhalb von 0..1 werden geklemmt statt zu überlaufen.
        assert_eq!(permille(-3.0), 0);
        assert_eq!(permille(42.0), 1000);
        assert_eq!(permille(f32::NAN), 0);
        assert_eq!(permille(f32::INFINITY), 1000);

        assert_eq!(format_score(850), "0.850");
        assert_eq!(format_score(1000), "1.000");
        assert_eq!(format_score(7), "0.007");
        assert_eq!(format_score(0), "0.000");
    }

    #[test]
    fn every_action_and_detector_has_a_stable_name() {
        // Die Werte landen als Metrik-Label. Keine Punkte, keine Namen, keine
        // Dopplungen.
        let mut seen = std::collections::HashSet::new();
        for detector in Detector::ALL {
            assert!(seen.insert(detector.as_str()), "{detector:?} doppelt");
            assert!(!detector.as_str().contains('.'));
            assert!(!detector.label().is_empty());
        }
        let mut seen = std::collections::HashSet::new();
        for action in Action::ALL {
            assert!(seen.insert(action.as_str()), "{action:?} doppelt");
        }
        assert_eq!(Action::default(), Action::Flag, "Default ist nicht flag");
    }

    #[test]
    fn only_block_blocks_and_only_flag_or_block_are_visible() {
        assert!(Action::Block.blocks());
        for action in [Action::Off, Action::Log, Action::Flag] {
            assert!(!action.blocks(), "{action:?} blockt");
        }
        assert!(Action::Flag.is_visible() && Action::Block.is_visible());
        assert!(!Action::Log.is_visible() && !Action::Off.is_visible());
    }
}
