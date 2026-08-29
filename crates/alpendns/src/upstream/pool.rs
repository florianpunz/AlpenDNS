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
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt as _;
use futures_util::stream::FuturesUnordered;
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
    backend: B,
    health: Health,
}

impl<B> Upstream<B> {
    pub const fn new(name: String, backend: B) -> Self {
        Self {
            name,
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
    fanout: usize,
    /// Beim Start zufällig gezogen. Bestimmt bei `split_by_zone`, welcher
    /// Upstream welche Domains sieht — nach jedem Neustart anders.
    seed: u64,
    next: AtomicUsize,
    clock: C,
}

impl<B: ResolveBackend, C: Clock> Pool<B, C> {
    pub fn new(upstreams: Vec<Upstream<B>>, strategy: Strategy, fanout: usize, clock: C) -> Self {
        Self {
            upstreams,
            strategy,
            fanout: fanout.max(1),
            seed: rand::random(),
            next: AtomicUsize::new(0),
            clock,
        }
    }

    /// Wie [`Self::new`], aber mit festem Seed — für Tests, die eine
    /// bestimmte Verteilung erwarten.
    pub fn with_seed(
        upstreams: Vec<Upstream<B>>,
        strategy: Strategy,
        fanout: usize,
        clock: C,
        seed: u64,
    ) -> Self {
        let mut pool = Self::new(upstreams, strategy, fanout, clock);
        pool.seed = seed;
        pool
    }

    pub fn stats(&self) -> Vec<UpstreamStats> {
        let now = self.clock.now();
        self.upstreams
            .iter()
            .map(|upstream| {
                let micros = upstream.health.ewma_micros.load(Ordering::Relaxed);
                UpstreamStats {
                    name: upstream.name.clone(),
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
            Strategy::Fastest => {
                let latencies: Vec<Option<Duration>> = self
                    .upstreams
                    .iter()
                    .map(|u| {
                        let micros = u.health.ewma_micros.load(Ordering::Relaxed);
                        (micros > 0).then(|| Duration::from_micros(micros))
                    })
                    .collect();
                strategy::by_latency(&latencies)
            }
            Strategy::RoundRobin => {
                let start = self.next.fetch_add(1, Ordering::Relaxed);
                strategy::round_robin(start, count)
            }
            Strategy::SplitByZone => match question {
                Some(name) => strategy::by_zone(self.seed, name, count),
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
        ctx: &Ctx,
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
            Err(error) => {
                self.record_failure(index);
                tracing::debug!(upstream = %upstream.name, %error, "Upstream-Anfrage fehlgeschlagen");
                Err(error)
            }
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
        ctx: &Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let question = request.queries.first().map(|q| q.name().clone());
        async move {
            let order = self.order(question.as_ref());
            if order.is_empty() {
                return Err(ResolveError::NoUpstreamLeft);
            }

            let mut last_error = None;
            let mut remaining = order.as_slice();
            while !remaining.is_empty() {
                let batch = remaining.len().min(self.fanout);
                let (now, rest) = remaining.split_at(batch);
                remaining = rest;

                // Bei fanout = 1 ist das genau ein Versuch; darüber laufen sie
                // parallel und der erste Erfolg gewinnt.
                let mut attempts: FuturesUnordered<_> = now
                    .iter()
                    .map(|&index| self.try_one(index, request, ctx))
                    .collect();
                while let Some(result) = attempts.next().await {
                    match result {
                        Ok(response) => return Ok(response),
                        Err(error) => last_error = Some(error),
                    }
                }
            }
            Err(last_error.unwrap_or(ResolveError::NoUpstreamLeft))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{SystemClock, TestClock};
    use crate::trace::Ctx;
    use hickory_proto::op::{MessageType, OpCode, Query};
    use hickory_proto::rr::RecordType;
    use std::sync::Arc;

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
            _ctx: &Ctx,
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
        strategy: Strategy,
        clock: Arc<TestClock>,
    ) -> Pool<Arc<Fake>, Arc<TestClock>> {
        let upstreams = fakes
            .iter()
            .enumerate()
            .map(|(i, fake)| Upstream::new(format!("fake{i}"), Arc::clone(fake)))
            .collect();
        Pool::with_seed(upstreams, strategy, 1, clock, 0x5eed)
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
            Strategy::RoundRobin,
            Arc::clone(&clock),
        );

        // Genug Anfragen, dass der tote Upstream die Schwelle reißt.
        for _ in 0..6 {
            pool.resolve(&question("example.com."), &ctx())
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
            pool.resolve(&question("example.com."), &ctx())
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
            Strategy::RoundRobin,
            Arc::clone(&clock),
        );

        for _ in 0..6 {
            let _ = pool.resolve(&question("example.com."), &ctx()).await;
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
            pool.resolve(&question("example.com."), &ctx())
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
        let pool = pool_of(&[Arc::clone(&dead)], Strategy::RoundRobin, clock);

        for _ in 0..5 {
            assert!(
                pool.resolve(&question("example.com."), &ctx())
                    .await
                    .is_err()
            );
        }
        assert!(pool.stats().first().expect("Statistik").down);

        let before = dead.calls();
        let _ = pool.resolve(&question("example.com."), &ctx()).await;
        assert!(dead.calls() > before, "es wurde niemand mehr gefragt");
    }

    #[tokio::test]
    async fn fastest_strategy_settles_on_the_quick_upstream() {
        // Echte Uhr, weil hier echte Latenzen gemessen werden.
        let slow = Fake::new(40);
        let quick = Fake::new(1);
        let upstreams = vec![
            Upstream::new("slow".to_owned(), Arc::clone(&slow)),
            Upstream::new("quick".to_owned(), Arc::clone(&quick)),
        ];
        let pool = Pool::new(upstreams, Strategy::Fastest, 1, SystemClock);

        // Zwei Anfragen zum Einmessen, danach sollte der schnelle gewinnen.
        for _ in 0..2 {
            pool.resolve(&question("example.com."), &ctx())
                .await
                .expect("beantwortet");
        }
        let slow_after_warmup = slow.calls();
        for _ in 0..10 {
            pool.resolve(&question("example.com."), &ctx())
                .await
                .expect("beantwortet");
        }

        assert_eq!(
            slow.calls(),
            slow_after_warmup,
            "der langsame Upstream wurde weiter gefragt"
        );
        assert!(quick.calls() >= 10);
    }

    #[tokio::test]
    async fn split_by_zone_sends_a_domain_to_the_same_upstream_every_time() {
        let fakes: Vec<Arc<Fake>> = (0..3).map(|_| Fake::new(0)).collect();
        let clock = Arc::new(TestClock::new());
        let pool = pool_of(&fakes, Strategy::SplitByZone, clock);

        for _ in 0..12 {
            pool.resolve(&question("www.example.com."), &ctx())
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
        let pool = pool_of(&fakes, Strategy::SplitByZone, clock);

        // Herausfinden, wer zuständig ist, und ihn kaputt machen.
        pool.resolve(&question("www.example.com."), &ctx())
            .await
            .expect("beantwortet");
        let responsible = fakes
            .iter()
            .position(|f| f.calls() > 0)
            .expect("jemand war zuständig");
        if let Some(fake) = fakes.get(responsible) {
            fake.broken.store(true, Ordering::SeqCst);
        }

        pool.resolve(&question("www.example.com."), &ctx())
            .await
            .expect("ein anderer Upstream muss einspringen");
        let answered_elsewhere = fakes
            .iter()
            .enumerate()
            .any(|(i, f)| i != responsible && f.calls() > 0);
        assert!(answered_elsewhere, "kein Ausweichweg genommen");
    }

    #[tokio::test]
    async fn round_robin_spreads_the_load() {
        let fakes: Vec<Arc<Fake>> = (0..3).map(|_| Fake::new(0)).collect();
        let clock = Arc::new(TestClock::new());
        let pool = pool_of(&fakes, Strategy::RoundRobin, clock);

        for _ in 0..9 {
            pool.resolve(&question("example.com."), &ctx())
                .await
                .expect("beantwortet");
        }
        for (i, fake) in fakes.iter().enumerate() {
            assert_eq!(
                fake.calls(),
                3,
                "Upstream {i} bekam {} Anfragen",
                fake.calls()
            );
        }
    }

    #[tokio::test]
    async fn fanout_two_asks_both_and_takes_the_first_answer() {
        let slow = Fake::new(40);
        let quick = Fake::new(1);
        let upstreams = vec![
            Upstream::new("slow".to_owned(), Arc::clone(&slow)),
            Upstream::new("quick".to_owned(), Arc::clone(&quick)),
        ];
        let pool = Pool::with_seed(upstreams, Strategy::RoundRobin, 2, SystemClock, 1);

        pool.resolve(&question("example.com."), &ctx())
            .await
            .expect("beantwortet");

        assert_eq!(slow.calls(), 1, "fanout = 2 muss beide fragen");
        assert_eq!(quick.calls(), 1);
    }

    #[tokio::test]
    async fn an_empty_pool_reports_that_nobody_is_left() {
        let clock = Arc::new(TestClock::new());
        let pool: Pool<Arc<Fake>, Arc<TestClock>> = pool_of(&[], Strategy::RoundRobin, clock);
        assert!(matches!(
            pool.resolve(&question("example.com."), &ctx()).await,
            Err(ResolveError::NoUpstreamLeft)
        ));
    }
}
