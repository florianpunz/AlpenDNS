//! Ein Pool gleichwertiger Upstreams mit Auswahlstrategie und Ausfallerkennung.
//!
//! Der Pool ist selbst ein [`ResolveBackend`] und generisch über das, was er
//! enthält. Im Betrieb ist das ein [`super::transport::Transport`]; in Tests ein
//! Fake, sodass Umschaltverhalten und Ausfallerkennung ohne TLS und ohne Netz
//! prüfbar sind.
//!
//! **Health-Tracking ist passiv** (ARCHITECTURE.md §5): gemessen wird, was die
//! echten Anfragen ohnehin verraten. Aktive Proben wären selbst wieder ein
//! Signal — ein Resolver, der alle 30 Sekunden angepingt wird, weiß, dass hier
//! jemand wohnt, auch wenn gerade niemand surft.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hickory_proto::op::Message;
use hickory_proto::rr::Name;

use crate::clock::Clock;
use crate::config::Strategy;
use crate::resolve::{ResolveBackend, ResolveError};
use crate::trace::{Ctx, Step};

use super::strategy;

/// So viele Fehlversuche hintereinander gelten als Ausfall.
///
/// Einer reicht nicht: ein verlorenes Paket ist normal. Drei hintereinander sind
/// ein Muster.
const FAILURE_THRESHOLD: u32 = 3;

/// So lange wird ein ausgefallener Upstream übersprungen, bevor er wieder an die
/// Reihe kommt.
const DOWN_DURATION: Duration = Duration::from_secs(30);

/// Gewicht der neuesten Messung im gleitenden Mittel der Antwortzeit.
const EWMA_ALPHA: f64 = 0.25;

/// Zustand eines Upstreams, wie ihn die bisherigen Anfragen zeigen.
#[derive(Debug, Default)]
struct Health {
    /// Gleitendes Mittel der Antwortzeit in Mikrosekunden. 0 = noch nie gemessen.
    ewma_micros: AtomicU64,
    consecutive_failures: AtomicU32,
    /// Bis wann dieser Upstream übersprungen wird.
    down_until: Mutex<Option<Instant>>,
    successes: AtomicU64,
    failures: AtomicU64,
}

/// Ein Upstream im Pool.
#[derive(Debug)]
pub struct Upstream<B> {
    name: String,
    /// `dot`, `doh`, `doq` — woher die Verschlüsselung kommt. Steht hier und
    /// nicht neben dem Pool, weil sonst zwei Listen parallel gepflegt werden
    /// müssten, die auseinanderlaufen können.
    scheme: &'static str,
    backend: B,
    health: Health,
}

impl<B> Upstream<B> {
    pub const fn new(name: String, scheme: &'static str, backend: B) -> Self {
        Self {
            name,
            scheme,
            backend,
            health: Health {
                ewma_micros: AtomicU64::new(0),
                consecutive_failures: AtomicU32::new(0),
                down_until: Mutex::new(None),
                successes: AtomicU64::new(0),
                failures: AtomicU64::new(0),
            },
        }
    }
}

/// Was ein Pool über einen Upstream zu berichten hat. Enthält keine Query-Namen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamStats {
    pub name: String,
    /// Der Transport, über den dieser Upstream gefragt wird.
    pub scheme: &'static str,
    pub successes: u64,
    pub failures: u64,
    /// Gleitendes Mittel der Antwortzeit, sofern schon einmal gemessen.
    pub rtt: Option<Duration>,
    /// Ob dieser Upstream gerade übersprungen wird.
    pub down: bool,
}

/// Eine Menge gleichwertiger Upstreams.
#[derive(Debug)]
pub struct Pool<B, C> {
    upstreams: Vec<Upstream<B>>,
    strategy: Strategy,
    /// Beim Start zufällig gezogen und danach im konfigurierten Abstand neu
    /// gewürfelt. Bestimmt bei `split_by_zone`, welcher Upstream welche
    /// Domains sieht.
    ///
    /// Atomar statt hinter einem Lock: gelesen wird er bei jeder Anfrage,
    /// geschrieben höchstens einmal am Tag. Ein `Mutex` im Anfragepfad wäre für
    /// dieses Verhältnis der falsche Preis (B.3 Regel 5). Dass eine Anfrage
    /// mitten in der Rotation noch den alten Wert sieht, ist folgenlos: der
    /// Upstream, den sie damit wählt, war eine Sekunde vorher der richtige.
    seed: AtomicU64,
    /// Wie oft der Seed seit dem Start neu gezogen wurde.
    rotations: AtomicU64,
    clock: C,
}

impl<B: ResolveBackend, C: Clock> Pool<B, C> {
    pub fn new(upstreams: Vec<Upstream<B>>, strategy: Strategy, clock: C) -> Self {
        Self::with_seed(upstreams, strategy, clock, rand::random())
    }

    /// Wie [`Self::new`], aber mit festem Seed — für Tests, die eine
    /// bestimmte Verteilung erwarten.
    pub const fn with_seed(
        upstreams: Vec<Upstream<B>>,
        strategy: Strategy,
        clock: C,
        seed: u64,
    ) -> Self {
        Self {
            upstreams,
            strategy,
            seed: AtomicU64::new(seed),
            rotations: AtomicU64::new(0),
            clock,
        }
    }

    /// Der aktuell gültige Seed.
    pub fn seed(&self) -> u64 {
        self.seed.load(Ordering::Relaxed)
    }

    /// Wie oft der Seed seit dem Start neu gezogen wurde.
    ///
    /// Steht in der Metrik, weil "die Zuordnung rotiert" sonst eine Behauptung
    /// in der Konfiguration bliebe statt einer Zahl, die im Betrieb steigt.
    pub fn rotations(&self) -> u64 {
        self.rotations.load(Ordering::Relaxed)
    }

    /// Würfelt die Zuordnung Domain → Upstream neu.
    ///
    /// Was das kostet und was es bringt: ab dem nächsten Cache-Miss sieht ein
    /// anderer Anbieter die Domain. Über einen Tag gerechnet sieht damit jeder
    /// Anbieter mehr *verschiedene* Domains als vorher — aber keiner von ihnen
    /// mehr ein Bild, das über die Rotation hinaus stabil bleibt. Genau das ist
    /// der Zweck: ein Profil entsteht aus Wiedererkennung über Zeit, nicht aus
    /// einzelnen Anfragen (FEATURES.md P2).
    ///
    /// Der Antwort-Cache bleibt unberührt — er liegt vor dem Pool und kennt
    /// keine Upstreams (ARCHITECTURE.md §1).
    pub fn rotate_seed(&self) {
        self.seed.store(rand::random(), Ordering::Relaxed);
        let count = self
            .rotations
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        tracing::info!(
            rotations = count,
            "Zonen-Seed neu gezogen; die Zuordnung Domain → Upstream ist ab jetzt eine andere"
        );
    }

    pub fn stats(&self) -> Vec<UpstreamStats> {
        let now = self.clock.now();
        self.upstreams
            .iter()
            .map(|upstream| {
                let micros = upstream.health.ewma_micros.load(Ordering::Relaxed);
                UpstreamStats {
                    name: upstream.name.clone(),
                    scheme: upstream.scheme,
                    successes: upstream.health.successes.load(Ordering::Relaxed),
                    failures: upstream.health.failures.load(Ordering::Relaxed),
                    rtt: (micros > 0).then(|| Duration::from_micros(micros)),
                    down: is_down(&upstream.health, now),
                }
            })
            .collect()
    }

    /// Die Reihenfolge, in der Upstreams versucht werden.
    ///
    /// Erst die Strategie, dann werden ausgefallene ans Ende sortiert. Sie
    /// fliegen nicht raus: wenn alle als tot gelten, wird trotzdem gefragt.
    /// Auflösung geht vor Aktualität (B.1 Regel 6) — und lieber ein Versuch bei
    /// einem vermeintlich toten Upstream als gar keine Antwort.
    fn order(&self, question: Option<&Name>) -> Vec<usize> {
        let count = self.upstreams.len();
        let base = match self.strategy {
            Strategy::SplitByZone => match question {
                Some(name) => strategy::by_zone(self.seed(), name, count),
                // Ohne Frage gibt es nichts zu verteilen.
                None => strategy::round_robin(0, count),
            },
        };

        let now = self.clock.now();
        let (healthy, down): (Vec<usize>, Vec<usize>) = base.into_iter().partition(|&i| {
            self.upstreams
                .get(i)
                .is_none_or(|u| !is_down(&u.health, now))
        });
        healthy.into_iter().chain(down).collect()
    }

    fn record_success(&self, index: usize, rtt: Duration) {
        let Some(upstream) = self.upstreams.get(index) else {
            return;
        };
        upstream.health.successes.fetch_add(1, Ordering::Relaxed);
        let was_down = upstream
            .health
            .consecutive_failures
            .swap(0, Ordering::Relaxed)
            >= FAILURE_THRESHOLD;
        if was_down {
            *upstream
                .health
                .down_until
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            tracing::info!(upstream = %upstream.name, "Upstream antwortet wieder");
        }

        let micros = u64::try_from(rtt.as_micros()).unwrap_or(u64::MAX);
        let previous = upstream.health.ewma_micros.load(Ordering::Relaxed);
        let updated = if previous == 0 {
            micros
        } else {
            #[expect(
                clippy::cast_precision_loss,
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "Mikrosekunden-Latenzen liegen weit unter der Genauigkeitsgrenze von f64"
            )]
            {
                (EWMA_ALPHA * micros as f64 + (1.0 - EWMA_ALPHA) * previous as f64) as u64
            }
        };
        upstream
            .health
            .ewma_micros
            .store(updated, Ordering::Relaxed);
    }

    fn record_failure(&self, index: usize) {
        let Some(upstream) = self.upstreams.get(index) else {
            return;
        };
        upstream.health.failures.fetch_add(1, Ordering::Relaxed);
        let failures = upstream
            .health
            .consecutive_failures
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if failures == FAILURE_THRESHOLD {
            let until = self.clock.now().checked_add(DOWN_DURATION);
            *upstream
                .health
                .down_until
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = until;
            tracing::warn!(
                upstream = %upstream.name,
                failures,
                "Upstream gilt als ausgefallen und wird vorerst übersprungen"
            );
        }
    }

    /// Fragt einen einzelnen Upstream und schreibt das Ergebnis in die Statistik.
    async fn try_one(
        &self,
        index: usize,
        request: &Message,
        ctx: &mut Ctx,
    ) -> Result<Message, ResolveError> {
        let Some(upstream) = self.upstreams.get(index) else {
            return Err(ResolveError::NoUpstreamLeft);
        };
        let started = self.clock.now();
        match upstream.backend.resolve(request, ctx).await {
            Ok(response) => {
                let rtt = self.clock.now().saturating_duration_since(started);
                self.record_success(index, rtt);
                ctx.record(Step::UpstreamUsed {
                    resolver: std::sync::Arc::from(upstream.name.as_str()),
                    rtt,
                });
                Ok(response)
            }
            // Eine faule Signatur ist kein Ausfall des Upstreams: er hat
            // geantwortet, und die Antwort ist bei jedem anderen Anbieter
            // genauso faul. Würde sie als Fehlversuch zählen, könnte eine
            // einzige kaputte Zone nach drei Anfragen den ganzen Pool als tot
            // markieren.
            Err(ResolveError::Bogus) => {
                ctx.record(Step::UpstreamUsed {
                    resolver: std::sync::Arc::from(upstream.name.as_str()),
                    rtt: self.clock.now().saturating_duration_since(started),
                });
                Err(ResolveError::Bogus)
            }
            Err(error) => {
                self.record_failure(index);
                tracing::debug!(upstream = %upstream.name, %error, "Upstream-Anfrage fehlgeschlagen");
                Err(error)
            }
        }
    }
}

/// Zieht den Zonen-Seed regelmäßig neu, bis der Shutdown kommt.
///
/// Läuft als eigene Aufgabe statt in der Anfrage: eine Rotation, die an
/// Anfragen hängt, käme auf einem stillen Server nie und auf einem lauten
/// dauernd. `every == 0` schaltet sie ab — dann gilt der Seed vom Start bis zum
/// Neustart, das Verhalten aus Phase 3.
pub async fn run_seed_rotation<B: ResolveBackend, C: Clock>(
    pool: std::sync::Arc<Pool<B, C>>,
    every: Duration,
    shutdown: tokio_util::sync::CancellationToken,
) {
    if every.is_zero() {
        tracing::info!("Zonen-Seed-Rotation ist abgeschaltet; die Zuordnung gilt bis zum Neustart");
        return;
    }
    let mut ticker = tokio::time::interval(every);
    // Der erste Tick kommt sofort; der Seed vom Start ist noch frisch.
    ticker.tick().await;
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            _ = ticker.tick() => pool.rotate_seed(),
        }
    }
}

/// Ob ein Upstream gerade übersprungen wird.
fn is_down(health: &Health, now: Instant) -> bool {
    let guard = health
        .down_until
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard.is_some_and(|until| now < until)
}

impl<B: ResolveBackend, C: Clock> ResolveBackend for Pool<B, C> {
    fn resolve(
        &self,
        request: &Message,
        ctx: &mut Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let question = request.queries.first().map(|q| q.name().clone());
        async move {
            let order = self.order(question.as_ref());

            // Der Reihe nach, bis einer antwortet. Nie zwei gleichzeitig: das
            // zeigte dieselbe Frage zwei Anbietern und hob split_by_zone auf
            // (ADR-0012).
            let mut last_error = None;
            for index in order {
                match self.try_one(index, request, ctx).await {
                    Ok(response) => return Ok(response),
                    // Terminal. Eine Zone mit kaputter Signatur ist überall
                    // kaputt; den nächsten Upstream zu fragen brächte dieselbe
                    // Antwort und zeigte den Namen einem zweiten Anbieter —
                    // genau das, was split_by_zone verhindern soll.
                    Err(ResolveError::Bogus) => return Err(ResolveError::Bogus),
                    Err(error) => last_error = Some(error),
                }
            }
            Err(last_error.unwrap_or(ResolveError::NoUpstreamLeft))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TestClock;
    use crate::trace::Ctx;
    use hickory_proto::op::{MessageType, OpCode, Query};
    use hickory_proto::rr::RecordType;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    /// Ein Upstream, der auf Kommando antwortet, schweigt oder trödelt.
    #[derive(Debug)]
    struct Fake {
        delay: Duration,
        broken: std::sync::atomic::AtomicBool,
        calls: AtomicUsize,
    }

    impl Fake {
        fn new(delay_ms: u64) -> Arc<Self> {
            Arc::new(Self {
                delay: Duration::from_millis(delay_ms),
                broken: std::sync::atomic::AtomicBool::new(false),
                calls: AtomicUsize::new(0),
            })
        }

        fn broken() -> Arc<Self> {
            let fake = Self::new(0);
            fake.broken.store(true, Ordering::SeqCst);
            fake
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl ResolveBackend for Fake {
        fn resolve(
            &self,
            request: &Message,
            _ctx: &mut Ctx,
        ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
            let request = request.clone();
            async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if !self.delay.is_zero() {
                    tokio::time::sleep(self.delay).await;
                }
                if self.broken.load(Ordering::SeqCst) {
                    return Err(ResolveError::Timeout);
                }
                let mut response = Message::response(request.metadata.id, OpCode::Query);
                response.add_queries(request.queries.iter().cloned());
                Ok(response)
            }
        }
    }

    /// Ein Kontext für Tests, die sich nicht für den Trace interessieren.
    fn ctx() -> Ctx {
        Ctx::new(std::net::SocketAddr::from(([127, 0, 0, 1], 5555)))
    }

    fn question(name: &str) -> Message {
        let mut message = Message::new(1, MessageType::Query, OpCode::Query);
        message.add_query(Query::query(
            Name::from_ascii(name).expect("gültiger Name"),
            RecordType::A,
        ));
        message
    }

    fn pool_of(
        fakes: &[Arc<Fake>],
        clock: Arc<TestClock>,
        seed: u64,
    ) -> Pool<Arc<Fake>, Arc<TestClock>> {
        let upstreams = fakes
            .iter()
            .enumerate()
            .map(|(i, fake)| Upstream::new(format!("fake{i}"), "dot", Arc::clone(fake)))
            .collect();
        Pool::with_seed(upstreams, Strategy::SplitByZone, clock, seed)
    }

    /// Ein Seed, bei dem `name` beim Upstream `target` landet.
    ///
    /// Seit `split_by_zone` die einzige Strategie ist, steht die Reihenfolge
    /// nicht mehr frei: wer zuerst gefragt wird, hängt am Seed. Tests zur
    /// Ausfallerkennung brauchen aber einen bekannten Ersten, sonst prüfen sie
    /// je nach Hash mal das eine und mal das andere.
    fn seed_starting_at(name: &str, count: usize, target: usize) -> u64 {
        let name = Name::from_ascii(name).expect("gültiger Name");
        (0..1000)
            .find(|&seed| strategy::zone_index(seed, &name, count) == target)
            .expect("unter tausend Seeds ist einer dabei")
    }

    #[tokio::test]
    async fn a_dead_upstream_is_skipped_after_repeated_failures() {
        // Roadmap Schritt 4: der Fake antwortet nicht mehr, die Anfragen gehen
        // an den zweiten, und die Statistik zeigt den Ausfall.
        let dead = Fake::broken();
        let alive = Fake::new(0);
        let clock = Arc::new(TestClock::new());
        let pool = pool_of(
            &[Arc::clone(&dead), Arc::clone(&alive)],
            Arc::clone(&clock),
            seed_starting_at("example.com.", 2, 0),
        );

        // Genug Anfragen, dass der tote Upstream die Schwelle reißt.
        for _ in 0..6 {
            pool.resolve(&question("example.com."), &mut ctx())
                .await
                .expect("der lebende Upstream antwortet");
        }
        let calls_while_failing = dead.calls();

        let stats = pool.stats();
        let dead_stats = stats.first().expect("Statistik für den ersten");
        assert!(dead_stats.down, "Ausfall nicht in der Statistik: {stats:?}");
        assert!(dead_stats.failures >= FAILURE_THRESHOLD.into());

        // Ab jetzt wird er übersprungen.
        for _ in 0..5 {
            pool.resolve(&question("example.com."), &mut ctx())
                .await
                .expect("weiter beantwortet");
        }
        assert_eq!(
            dead.calls(),
            calls_while_failing,
            "der ausgefallene Upstream wurde weiter gefragt"
        );
        assert!(alive.calls() >= 5);
    }

    #[tokio::test]
    async fn a_recovered_upstream_is_used_again_after_the_cooldown() {
        let flaky = Fake::broken();
        let alive = Fake::new(0);
        let clock = Arc::new(TestClock::new());
        let pool = pool_of(
            &[Arc::clone(&flaky), Arc::clone(&alive)],
            Arc::clone(&clock),
            seed_starting_at("example.com.", 2, 0),
        );

        for _ in 0..6 {
            let _ = pool.resolve(&question("example.com."), &mut ctx()).await;
        }
        assert!(pool.stats().first().expect("Statistik").down);

        flaky.broken.store(false, Ordering::SeqCst);
        clock.advance(DOWN_DURATION + Duration::from_secs(1));
        assert!(
            !pool.stats().first().expect("Statistik").down,
            "die Sperre läuft nicht ab"
        );

        let before = flaky.calls();
        for _ in 0..4 {
            pool.resolve(&question("example.com."), &mut ctx())
                .await
                .expect("beantwortet");
        }
        assert!(
            flaky.calls() > before,
            "der erholte Upstream bleibt außen vor"
        );
    }

    #[tokio::test]
    async fn all_upstreams_down_still_gets_asked() {
        // B.1 Regel 6: fail open bei Verfügbarkeit. Wenn alle als tot gelten,
        // wird trotzdem gefragt statt sofort aufzugeben.
        let dead = Fake::broken();
        let clock = Arc::new(TestClock::new());
        let pool = pool_of(&[Arc::clone(&dead)], clock, 0x5eed);

        for _ in 0..5 {
            assert!(
                pool.resolve(&question("example.com."), &mut ctx())
                    .await
                    .is_err()
            );
        }
        assert!(pool.stats().first().expect("Statistik").down);

        let before = dead.calls();
        let _ = pool.resolve(&question("example.com."), &mut ctx()).await;
        assert!(dead.calls() > before, "es wurde niemand mehr gefragt");
    }

    #[tokio::test]
    async fn split_by_zone_sends_a_domain_to_the_same_upstream_every_time() {
        let fakes: Vec<Arc<Fake>> = (0..3).map(|_| Fake::new(0)).collect();
        let clock = Arc::new(TestClock::new());
        let pool = pool_of(&fakes, clock, 0x5eed);

        for _ in 0..12 {
            pool.resolve(&question("www.example.com."), &mut ctx())
                .await
                .expect("beantwortet");
        }

        let used: Vec<usize> = fakes.iter().map(|f| f.calls()).collect();
        assert_eq!(
            used.iter().filter(|&&calls| calls > 0).count(),
            1,
            "die Domain verteilte sich auf mehrere Upstreams: {used:?}"
        );
        assert!(used.contains(&12));
    }

    #[tokio::test]
    async fn split_by_zone_falls_back_when_the_responsible_upstream_fails() {
        let fakes: Vec<Arc<Fake>> = (0..3).map(|_| Fake::new(0)).collect();
        let clock = Arc::new(TestClock::new());
        let pool = pool_of(&fakes, clock, 0x5eed);

        // Herausfinden, wer zuständig ist, und ihn kaputt machen.
        pool.resolve(&question("www.example.com."), &mut ctx())
            .await
            .expect("beantwortet");
        let responsible = fakes
            .iter()
            .position(|f| f.calls() > 0)
            .expect("jemand war zuständig");
        if let Some(fake) = fakes.get(responsible) {
            fake.broken.store(true, Ordering::SeqCst);
        }

        pool.resolve(&question("www.example.com."), &mut ctx())
            .await
            .expect("ein anderer Upstream muss einspringen");
        let answered_elsewhere = fakes
            .iter()
            .enumerate()
            .any(|(i, f)| i != responsible && f.calls() > 0);
        assert!(answered_elsewhere, "kein Ausweichweg genommen");
    }

    #[tokio::test]
    async fn only_one_upstream_is_asked_when_it_answers() {
        // Das Gegenstück zum entfernten fanout: solange der zuständige Upstream
        // antwortet, sieht kein zweiter die Frage (ADR-0012).
        let fakes: Vec<Arc<Fake>> = (0..3).map(|_| Fake::new(0)).collect();
        let clock = Arc::new(TestClock::new());
        let pool = pool_of(&fakes, clock, 0x5eed);

        pool.resolve(&question("example.com."), &mut ctx())
            .await
            .expect("beantwortet");

        let asked: usize = fakes.iter().map(|f| f.calls()).sum();
        assert_eq!(asked, 1, "mehr als ein Upstream sah die Frage");
    }

    #[tokio::test]
    async fn rotating_the_seed_moves_domains_to_other_upstreams() {
        // Der Zweck der Rotation: nach ihr sieht ein anderer Anbieter die
        // Domain. Über viele Namen geprüft, weil ein einzelner auch nach dem
        // Neuwürfeln zufällig beim selben Upstream landen kann.
        let fakes: Vec<Arc<Fake>> = (0..4).map(|_| Fake::new(0)).collect();
        let clock = Arc::new(TestClock::new());
        let pool = pool_of(&fakes, clock, 0x5eed);

        // Verschiedene registrierbare Domains — Subdomains derselben Domain
        // gehören zusammen und würden hier nichts zeigen.
        let domains: Vec<Name> = (0..200)
            .map(|i| Name::from_ascii(format!("www.site{i}.com.")).expect("gültiger Name"))
            .collect();
        let before = strategy::distribution(pool.seed(), domains.iter(), 4);

        pool.rotate_seed();
        assert_eq!(pool.rotations(), 1);
        let after = strategy::distribution(pool.seed(), domains.iter(), 4);

        assert_ne!(
            before, after,
            "die Zuordnung ist nach der Rotation dieselbe geblieben"
        );
    }

    #[tokio::test]
    async fn rotation_keeps_the_distribution_even() {
        // Eine Rotation, die die Verteilung schief macht, wäre eine
        // Verschlechterung. Nach jedem Wurf muss das Abnahmekriterium aus
        // Schritt 1 weiter gelten.
        let fakes: Vec<Arc<Fake>> = (0..3).map(|_| Fake::new(0)).collect();
        let clock = Arc::new(TestClock::new());
        let pool = pool_of(&fakes, clock, 0x5eed);
        let domains: Vec<Name> = (0..10_000)
            .map(|i| Name::from_ascii(format!("www.site{i}.com.")).expect("gültiger Name"))
            .collect();

        for _ in 0..5 {
            pool.rotate_seed();
            let buckets = strategy::distribution(pool.seed(), domains.iter(), 3);
            let deviation = strategy::max_deviation(&buckets);
            assert!(deviation < 0.05, "{deviation} — {buckets:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_rotation_task_keeps_rotating_until_shutdown() {
        let fakes: Vec<Arc<Fake>> = (0..2).map(|_| Fake::new(0)).collect();
        let clock = Arc::new(TestClock::new());
        let pool = Arc::new(pool_of(&fakes, clock, 0x5eed));
        let shutdown = tokio_util::sync::CancellationToken::new();

        let task = tokio::spawn(run_seed_rotation(
            Arc::clone(&pool),
            Duration::from_secs(3600),
            shutdown.clone(),
        ));
        tokio::time::sleep(Duration::from_secs(3 * 3600 + 60)).await;
        assert_eq!(pool.rotations(), 3, "die Aufgabe rotiert nicht im Takt");

        shutdown.cancel();
        task.await.expect("die Aufgabe endet beim Shutdown");
        let stopped_at = pool.rotations();
        tokio::time::sleep(Duration::from_secs(4 * 3600)).await;
        assert_eq!(pool.rotations(), stopped_at, "sie läuft weiter");
    }

    #[tokio::test(start_paused = true)]
    async fn a_rotation_interval_of_zero_switches_the_rotation_off() {
        // Das Verhalten aus Phase 3: der Seed gilt bis zum Neustart.
        let fakes: Vec<Arc<Fake>> = (0..2).map(|_| Fake::new(0)).collect();
        let clock = Arc::new(TestClock::new());
        let pool = Arc::new(pool_of(&fakes, clock, 0x5eed));
        let seed = pool.seed();

        run_seed_rotation(
            Arc::clone(&pool),
            Duration::ZERO,
            tokio_util::sync::CancellationToken::new(),
        )
        .await;

        tokio::time::sleep(Duration::from_secs(48 * 3600)).await;
        assert_eq!(pool.rotations(), 0);
        assert_eq!(pool.seed(), seed);
    }

    #[tokio::test]
    async fn an_empty_pool_reports_that_nobody_is_left() {
        let clock = Arc::new(TestClock::new());
        let pool: Pool<Arc<Fake>, Arc<TestClock>> = pool_of(&[], clock, 0x5eed);
        assert!(matches!(
            pool.resolve(&question("example.com."), &mut ctx()).await,
            Err(ResolveError::NoUpstreamLeft)
        ));
    }
}
