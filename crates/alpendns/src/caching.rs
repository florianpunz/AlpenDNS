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

use hickory_proto::op::Message;
use tokio::sync::broadcast;

use crate::cache::{Cache, Key};
use crate::clock::Clock;
use crate::config::CacheConfig;
use crate::resolve::{ResolveBackend, ResolveError};
use crate::trace::{Ctx, Step};

/// Cache und Deduplizierung vor einem anderen Backend.
#[derive(Debug)]
pub struct CachingBackend<B, C> {
    inner: Arc<B>,
    cache: Arc<Cache<C>>,
    inflight: Arc<Inflight>,
    prefetch: bool,
}

impl<B: ResolveBackend, C: Clock> CachingBackend<B, C> {
    pub fn new(inner: B, config: &CacheConfig, clock: C) -> Self {
        Self {
            inner: Arc::new(inner),
            cache: Arc::new(Cache::new(config, clock)),
            inflight: Arc::new(Inflight::default()),
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
        ctx: &Ctx,
    ) -> impl std::future::Future<Output = Result<Message, ResolveError>> + Send {
        let inner = Arc::clone(&self.inner);
        let cache = Arc::clone(&self.cache);
        let inflight = Arc::clone(&self.inflight);
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
                        request.clone(),
                        key,
                    );
                }
                return Ok(adopt(hit.response, &request));
            }

            match inflight.claim(&key) {
                Claim::Follower(mut receiver) => match receiver.recv().await {
                    Ok(Some(response)) => Ok(adopt((*response).clone(), &request)),
                    Ok(None) | Err(_) => Err(ResolveError::Coalesced),
                },
                Claim::Leader(mut leader) => {
                    let result = inner.resolve(&request, ctx).await;
                    if let Ok(ref response) = result {
                        cache.insert(key, response);
                        leader.complete(Arc::new(response.clone()));
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
    request: Message,
    key: Key,
) {
    tokio::spawn(async move {
        let Claim::Leader(mut leader) = inflight.claim(&key) else {
            return;
        };
        // Eine Auffrischung im Hintergrund gehört zu keiner Client-Anfrage und
        // bekommt deshalb einen eigenen, verworfenen Kontext.
        match inner.resolve(&request, &Ctx::internal()).await {
            Ok(response) => {
                cache.insert(key, &response);
                leader.complete(Arc::new(response));
            }
            // Scheitert die Auffrischung, bleibt der alte Eintrag stehen und
            // wird beim nächsten Mal erneut versucht.
            Err(error) => tracing::debug!(%error, "Auffrischung im Hintergrund fehlgeschlagen"),
        }
    });
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
