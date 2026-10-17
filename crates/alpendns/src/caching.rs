//! Der Cache als Schicht vor dem eigentlichen Auflösen.
//!
//! [`CachingBackend`] implementiert selbst [`ResolveBackend`] und umschließt ein
//! zweites. Damit liegt der Cache genau dort, wo ARCHITECTURE.md §1 ihn
//! einzeichnet — zwischen Server und Auflösung — ohne dass Server oder Forwarder
//! davon wissen müssen. Für Phase 3 heißt das: der Upstream-Pool wird
//! eingesetzt, indem er das innere Backend ersetzt.
//!
//! Hier steckt außerdem die Query-Deduplizierung: 500 Clients, die gleichzeitig
//! denselben ungecachten Namen fragen, erzeugen genau eine Upstream-Anfrage.
//! Ohne das ist jeder Cache-Ablauf ein selbstgebauter Lastspitzen-Generator
//! (ARCHITECTURE.md §8).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use hickory_proto::op::Message;
use tokio::sync::broadcast;

use crate::cache::{Cache, Key};
use crate::clock::Clock;
use crate::config::CacheConfig;
use crate::resolve::{ResolveBackend, ResolveError};
use crate::trace::{Ctx, Step};

/// Die erste Sperrfrist, mit der ein Auflösungsfehler gemerkt wird.
///
/// RFC 9520 §3.2 verlangt mindestens 1 s und höchstens 5 min und schlägt als
/// Einstieg 5 s vor. Jeder weitere Fehler für denselben Namen verdoppelt die
/// Frist (siehe [`Failures::record`]).
const FAILURE_TTL: Duration = Duration::from_secs(5);

/// Obergrenze der Sperrfrist.
///
/// RFC 9520 §3.2 erlaubt bis 5 min. 60 s sind die bewusste Wahl: nach einem
/// Netzausfall wirkt eine vergessene Frist höchstens eine Minute je Name nach,
/// und ein Gerät, das im Minuten- oder Sekundentakt denselben toten Namen
/// fragt, ist trotzdem abgefangen (ADR-0027).
const FAILURE_TTL_MAX: Duration = Duration::from_secs(60);

/// Obergrenze für das Kurzzeitgedächtnis.
///
/// RFC 9520 §3.2 verlangt, Erschöpfung durch erfundene Namen zu verhindern;
/// beim Erreichen der Grenze fallen zuerst die abgelaufenen Einträge heraus.
const MAX_FAILURES: usize = 1024;

/// Cache und Deduplizierung vor einem anderen Backend.
#[derive(Debug)]
pub struct CachingBackend<B, C> {
    inner: Arc<B>,
    cache: Arc<Cache<C>>,
    inflight: Arc<Inflight>,
    failures: Arc<Failures>,
    prefetch: bool,
}

impl<B: ResolveBackend, C: Clock> CachingBackend<B, C> {
    pub fn new(inner: B, config: &CacheConfig, clock: C) -> Self {
        Self {
            inner: Arc::new(inner),
            cache: Arc::new(Cache::new(config, clock)),
            inflight: Arc::new(Inflight::default()),
            failures: Arc::new(Failures::default()),
            prefetch: config.prefetch,
        }
    }

    /// Zugriff auf den Cache, für Tests und später für Metriken.
    pub fn cache(&self) -> &Arc<Cache<C>> {
        &self.cache
    }
}

impl<B: ResolveBackend, C: Clock> ResolveBackend for CachingBackend<B, C> {
    fn resolve(
        &self,
        request: &Message,
        ctx: &mut Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let inner = Arc::clone(&self.inner);
        let cache = Arc::clone(&self.cache);
        let inflight = Arc::clone(&self.inflight);
        let failures = Arc::clone(&self.failures);
        let prefetch = self.prefetch;
        let request = request.clone();
        let key = request.queries.first().map(Key::from_query);

        async move {
            // Ohne Frage gibt es nichts zu cachen; das fängt die Pipeline zwar
            // schon ab, aber dieses Backend soll für sich genommen korrekt sein.
            let Some(key) = key else {
                return inner.resolve(&request, ctx).await;
            };

            if let Some(hit) = cache.get(&key) {
                ctx.record(Step::CacheHit {
                    ttl_left: hit.response.answers.first().map_or(0, |record| record.ttl),
                    stale: hit.stale,
                });
                if hit.stale || (prefetch && hit.should_prefetch) {
                    spawn_refresh(
                        Arc::clone(&inner),
                        Arc::clone(&cache),
                        Arc::clone(&inflight),
                        Arc::clone(&failures),
                        request.clone(),
                        key,
                    );
                }
                return Ok(adopt(hit.response, &request));
            }

            // Kurz zuvor gescheitert? Dann nicht noch einmal fragen — der
            // Upstream lief gerade erst in denselben Fehler (RFC 9520 §3.2).
            // Hinter dem Cache: ein abgelaufener Eintrag geht vor.
            if failures.is_fresh(&key, cache.now()) {
                ctx.record(Step::FailureCached);
                return Err(ResolveError::CachedFailure);
            }

            match inflight.claim(&key) {
                Claim::Follower(mut receiver) => match receiver.recv().await {
                    Ok(Some(response)) => Ok(adopt((*response).clone(), &request)),
                    Ok(None) | Err(_) => Err(ResolveError::Coalesced),
                },
                Claim::Leader(mut leader) => {
                    let result = inner.resolve(&request, ctx).await;
                    match &result {
                        Ok(response) => {
                            failures.forget(&key);
                            cache.insert(key.clone(), response);
                            leader.complete(Arc::new(response.clone()));
                        }
                        Err(_) => failures.record(key.clone(), cache.now()),
                    }
                    result
                }
            }
        }
    }
}

/// Frischt einen Eintrag im Hintergrund auf, ohne dass ein Client wartet.
///
/// Läuft über dieselbe Inflight-Verwaltung wie normale Anfragen: eine bereits
/// laufende Auflösung wird nicht verdoppelt.
fn spawn_refresh<B: ResolveBackend, C: Clock>(
    inner: Arc<B>,
    cache: Arc<Cache<C>>,
    inflight: Arc<Inflight>,
    failures: Arc<Failures>,
    request: Message,
    key: Key,
) {
    tokio::spawn(async move {
        let Claim::Leader(mut leader) = inflight.claim(&key) else {
            return;
        };
        // Eine Auffrischung im Hintergrund gehört zu keiner Client-Anfrage und
        // bekommt deshalb einen eigenen, verworfenen Kontext.
        let mut ctx = Ctx::internal();
        match inner.resolve(&request, &mut ctx).await {
            Ok(response) => {
                failures.forget(&key);
                cache.insert(key, &response);
                leader.complete(Arc::new(response));
            }
            // Scheitert die Auffrischung, bleibt der alte Eintrag stehen und
            // wird beim nächsten Mal erneut versucht — aber nicht sofort, denn
            // der Fehler ist jetzt für FAILURE_TTL gemerkt.
            Err(error) => {
                failures.record(key, cache.now());
                tracing::debug!(%error, "background refresh failed");
            }
        }
    });
}

/// Ein gemerkter Fehler: wann er auftrat und wie lange er sperrt.
#[derive(Debug, Clone, Copy)]
struct Failure {
    at: Instant,
    /// Die Frist dieses Eintrags — beim ersten Fehler [`FAILURE_TTL`], bei
    /// jedem weiteren verdoppelt bis [`FAILURE_TTL_MAX`].
    ttl: Duration,
}

/// Kurzzeitgedächtnis für gescheiterte Auflösungen (RFC 9520 §3.2, ADR-0027).
///
/// Getrennt vom Antwort-Cache: dort liegen Daten, hier liegt ein Zustand. Der
/// Schlüssel ist derselbe — (Name, Typ, Klasse) —, damit der Fehler genau die
/// Fragen abfängt, die ihn ausgelöst haben.
#[derive(Debug, Default)]
struct Failures {
    map: Mutex<HashMap<Key, Failure>>,
}

impl Failures {
    /// Ob für diesen Schlüssel ein noch gültiger Fehler vorliegt.
    fn is_fresh(&self, key: &Key, now: Instant) -> bool {
        let map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        map.get(key)
            .is_some_and(|failure| now.saturating_duration_since(failure.at) < failure.ttl)
    }

    /// Merkt den Fehler zu einem Schlüssel und verdoppelt dabei dessen Frist.
    ///
    /// RFC 9520 §3.2: „the initial time for negatively caching a resolution
    /// failure might be set to 5 seconds and increased after each retry that
    /// results in another resolution failure". Der Eintrag bleibt nach Ablauf
    /// stehen, damit der nächste Fehler die Verdopplung fortsetzen kann;
    /// zurückgesetzt wird er nur durch einen Erfolg ([`Self::forget`]).
    fn record(&self, key: Key, now: Instant) {
        let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        // Ohne Obergrenze wäre das ein Gedächtnis für jeden erfundenen Namen.
        if map.len() >= MAX_FAILURES {
            map.retain(|_, failure| now.saturating_duration_since(failure.at) < failure.ttl);
        }
        if map.len() < MAX_FAILURES {
            let ttl = map.get(&key).map_or(FAILURE_TTL, |failure| {
                (failure.ttl * 2).min(FAILURE_TTL_MAX)
            });
            map.insert(key, Failure { at: now, ttl });
        }
    }

    /// Vergisst einen Fehler — nach einem Erfolg ist er überholt.
    fn forget(&self, key: &Key) {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
    }

    /// Anzahl der gemerkten Fehler. Nur für Tests.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// Übernimmt ID und Frage des Clients in eine Antwort aus dem Cache.
///
/// Die gecachte Antwort trägt die Frage dessen, der sie ausgelöst hat. Ein
/// Client, der `EXAMPLE.com` gefragt hat, will das auch zurückbekommen.
fn adopt(mut response: Message, request: &Message) -> Message {
    response.metadata.id = request.metadata.id;
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    response.queries = request.queries.clone();
    response
}

/// Laufende Auflösungen, nach Cache-Schlüssel.
#[derive(Debug, Default)]
struct Inflight {
    map: Mutex<HashMap<Key, broadcast::Sender<Option<Arc<Message>>>>>,
}

enum Claim {
    /// Wir lösen auf und benachrichtigen die anderen.
    Leader(Leader),
    /// Jemand anderes ist schon dran; wir warten auf dessen Ergebnis.
    Follower(broadcast::Receiver<Option<Arc<Message>>>),
}

impl Inflight {
    fn claim(self: &Arc<Self>, key: &Key) -> Claim {
        let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sender) = map.get(key) {
            return Claim::Follower(sender.subscribe());
        }
        let (sender, _) = broadcast::channel(1);
        map.insert(key.clone(), sender.clone());
        Claim::Leader(Leader {
            inflight: Arc::clone(self),
            key: key.clone(),
            sender,
            result: None,
        })
    }

    fn release(&self, key: &Key) {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(key);
    }
}

/// Das Recht, eine bestimmte Anfrage aufzulösen.
///
/// Räumt sich beim Verlassen des Gültigkeitsbereichs selbst auf — auch wenn die
/// Auflösung fehlschlägt oder der Task abgebrochen wird. Ohne das bliebe der
/// Schlüssel in der Inflight-Tabelle stehen und jeder spätere Frager würde auf
/// ein Ergebnis warten, das nie kommt.
struct Leader {
    inflight: Arc<Inflight>,
    key: Key,
    sender: broadcast::Sender<Option<Arc<Message>>>,
    result: Option<Arc<Message>>,
}

impl Leader {
    fn complete(&mut self, response: Arc<Message>) {
        self.result = Some(response);
    }
}

impl Drop for Leader {
    fn drop(&mut self) {
        self.inflight.release(&self.key);
        // Schlägt fehl, wenn niemand wartet — das ist der Normalfall.
        let _ = self.sender.send(self.result.take());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    use hickory_proto::op::{Message, MessageType, OpCode, Query};
    use hickory_proto::rr::rdata::A;
    use hickory_proto::rr::{Name, RData, Record, RecordType};

    use super::*;
    use crate::cache::Key;
    use crate::clock::TestClock;
    use crate::config::CacheConfig;
    use crate::resolve::ResolveError;
    use crate::trace::Ctx;

    /// Ein Backend, das jede Anfrage zählt und auf Kommando scheitert.
    ///
    /// Der Zähler ist die eigentliche Aussage dieser Tests: er sagt, ob eine
    /// Frage den Upstream erreicht hat oder nicht.
    #[derive(Debug, Default)]
    struct Fake {
        calls: AtomicUsize,
        failing: AtomicBool,
        /// Antwortet mit NoError, aber ohne Records — nichts, was der Cache
        /// behalten dürfte (kein SOA, keine Antwort).
        empty: AtomicBool,
    }

    impl Fake {
        fn failing() -> Arc<Self> {
            let fake = Arc::new(Self::default());
            fake.failing.store(true, Ordering::SeqCst);
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
            self.calls.fetch_add(1, Ordering::SeqCst);
            let failing = self.failing.load(Ordering::SeqCst);
            let empty = self.empty.load(Ordering::SeqCst);
            let request = request.clone();
            async move {
                if failing {
                    return Err(ResolveError::Timeout);
                }
                let mut response = Message::response(request.metadata.id, OpCode::Query);
                response.add_queries(request.queries.iter().cloned());
                if let Some(query) = request.queries.first()
                    && !empty
                {
                    response.add_answer(Record::from_rdata(
                        query.name().clone(),
                        10,
                        RData::A(A::new(203, 0, 113, 7)),
                    ));
                }
                Ok(response)
            }
        }
    }

    fn request(name: &str) -> Message {
        let mut message = Message::new(1, MessageType::Query, OpCode::Query);
        message.add_query(Query::query(
            Name::from_ascii(name).expect("gültiger Name"),
            RecordType::A,
        ));
        message
    }

    fn backend(
        fake: &Arc<Fake>,
        clock: &Arc<TestClock>,
    ) -> CachingBackend<Arc<Fake>, Arc<TestClock>> {
        CachingBackend::new(Arc::clone(fake), &CacheConfig::default(), Arc::clone(clock))
    }

    #[tokio::test]
    async fn a_cached_failure_does_not_reach_the_upstream_again() {
        let clock = Arc::new(TestClock::new());
        let fake = Fake::failing();
        let backend = backend(&fake, &clock);

        assert!(
            backend
                .resolve(&request("example.com."), &mut Ctx::internal())
                .await
                .is_err()
        );
        assert_eq!(fake.calls(), 1);

        assert!(
            backend
                .resolve(&request("example.com."), &mut Ctx::internal())
                .await
                .is_err()
        );
        assert_eq!(
            fake.calls(),
            1,
            "die zweite gleiche Anfrage hat den Upstream erreicht"
        );
    }

    #[tokio::test]
    async fn after_the_window_the_upstream_is_asked_again() {
        let clock = Arc::new(TestClock::new());
        let fake = Fake::failing();
        let backend = backend(&fake, &clock);

        assert!(
            backend
                .resolve(&request("example.com."), &mut Ctx::internal())
                .await
                .is_err()
        );
        // Mehr als FAILURE_TTL (5 s), über die Uhr statt über sleep.
        clock.advance(Duration::from_secs(6));

        assert!(
            backend
                .resolve(&request("example.com."), &mut Ctx::internal())
                .await
                .is_err()
        );
        assert_eq!(fake.calls(), 2, "nach Ablauf wurde nicht wieder gefragt");
    }

    #[tokio::test]
    async fn a_stale_entry_wins_over_the_cached_failure() {
        let clock = Arc::new(TestClock::new());
        let fake = Fake::failing();
        let backend = backend(&fake, &clock);

        // Ein Fehler für diesen Schlüssel steht im Kurzzeitgedächtnis …
        assert!(
            backend
                .resolve(&request("example.com."), &mut Ctx::internal())
                .await
                .is_err()
        );
        assert_eq!(fake.calls(), 1);

        // … und ein abgelaufener, aber noch auslieferbarer Eintrag im Cache.
        let key = Key::from_query(request("example.com.").queries.first().expect("eine Frage"));
        let mut cached = request("example.com.");
        cached.add_answer(Record::from_rdata(
            Name::from_ascii("example.com.").expect("Name"),
            10,
            RData::A(A::new(192, 0, 2, 1)),
        ));
        backend.cache().insert(key, &cached);
        clock.advance(Duration::from_secs(11));

        let response = backend
            .resolve(&request("example.com."), &mut Ctx::internal())
            .await
            .expect("die abgelaufene Antwort wird ausgeliefert");
        assert!(
            !response.answers.is_empty(),
            "der gecachte Fehler hat den abgelaufenen Eintrag verdrängt"
        );
    }

    #[tokio::test]
    async fn the_lockout_doubles_with_each_further_failure() {
        // 5 s, 10 s, 20 s, 40 s, 60 s — und darüber hinaus nicht mehr
        // (RFC 9520 §3.2 erlaubt bis 5 min; 60 s begrenzen die Nachwirkung
        // nach einem Netzausfall, ADR-0027).
        let clock = Arc::new(TestClock::new());
        let fake = Fake::failing();
        let backend = backend(&fake, &clock);

        assert!(
            backend
                .resolve(&request("example.com."), &mut Ctx::internal())
                .await
                .is_err()
        );
        let mut calls = 1;

        for window in [5_u64, 10, 20, 40, 60, 60] {
            clock.advance(Duration::from_secs(window - 1));
            assert!(
                backend
                    .resolve(&request("example.com."), &mut Ctx::internal())
                    .await
                    .is_err()
            );
            assert_eq!(fake.calls(), calls, "{window}s: vor Ablauf gefragt");

            clock.advance(Duration::from_secs(2));
            assert!(
                backend
                    .resolve(&request("example.com."), &mut Ctx::internal())
                    .await
                    .is_err()
            );
            calls += 1;
            assert_eq!(
                fake.calls(),
                calls,
                "{window}s: nach Ablauf nicht wieder gefragt"
            );
        }
    }

    #[tokio::test]
    async fn a_success_resets_the_lockout() {
        let clock = Arc::new(TestClock::new());
        let fake = Fake::failing();
        let backend = backend(&fake, &clock);
        let req = request("example.com.");

        // Zweimal scheitern: die Sperre steht danach bei 10 s.
        assert!(backend.resolve(&req, &mut Ctx::internal()).await.is_err());
        clock.advance(Duration::from_secs(6));
        assert!(backend.resolve(&req, &mut Ctx::internal()).await.is_err());
        assert_eq!(fake.calls(), 2);

        // Nach 6 s von 10 s wird noch nicht gefragt.
        clock.advance(Duration::from_secs(6));
        assert!(backend.resolve(&req, &mut Ctx::internal()).await.is_err());
        assert_eq!(fake.calls(), 2, "die verdoppelte Sperre galt nicht");

        // Jetzt antwortet der Upstream (ohne Records, damit nichts im Cache
        // landet): der Erfolg setzt die Sperre zurück.
        fake.failing.store(false, Ordering::SeqCst);
        fake.empty.store(true, Ordering::SeqCst);
        clock.advance(Duration::from_secs(5));
        assert!(backend.resolve(&req, &mut Ctx::internal()).await.is_ok());
        assert_eq!(fake.calls(), 3);

        // Ein neuer Fehler sperrt wieder nur 5 s.
        fake.failing.store(true, Ordering::SeqCst);
        fake.empty.store(false, Ordering::SeqCst);
        assert!(backend.resolve(&req, &mut Ctx::internal()).await.is_err());
        assert_eq!(fake.calls(), 4);
        clock.advance(Duration::from_secs(4));
        assert!(backend.resolve(&req, &mut Ctx::internal()).await.is_err());
        assert_eq!(
            fake.calls(),
            4,
            "nach dem Erfolg gilt nicht wieder die kurze Sperre"
        );
        clock.advance(Duration::from_secs(2));
        assert!(backend.resolve(&req, &mut Ctx::internal()).await.is_err());
        assert_eq!(fake.calls(), 5);
    }

    #[tokio::test]
    async fn the_failure_table_stays_bounded() {
        // RFC 9520 §3.2: Erschöpfung durch erfundene Namen verhindern.
        let clock = Arc::new(TestClock::new());
        let fake = Fake::failing();
        let backend = backend(&fake, &clock);

        for i in 0..MAX_FAILURES + 500 {
            let name = format!("n{i}.example.");
            let _ = backend.resolve(&request(&name), &mut Ctx::internal()).await;
        }
        let len = backend.failures.len();
        assert!(len > 0, "kein Fehler gemerkt");
        assert!(
            len <= MAX_FAILURES,
            "{len} Einträge bei Grenze {MAX_FAILURES}"
        );
    }
}
