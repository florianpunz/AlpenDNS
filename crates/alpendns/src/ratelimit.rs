//! Rate-Limiting pro Client.
//!
//! Ein offener Resolver ist ein Amplification-Reflektor: eine kurze Anfrage mit
//! gefälschter Absenderadresse erzeugt eine lange Antwort an das Opfer.
//! Deshalb ist das hier Pflicht, bevor der Server irgendwo lauscht, wo er nicht
//! nur sein eigenes LAN sieht (CLAUDE.md B.5).
//!
//! Drei Entscheidungen, die von außen willkürlich aussehen:
//!
//! * **Über dem Limit wird verworfen, nicht abgelehnt.** Eine REFUSED-Antwort
//!   wäre selbst wieder ein Paket an eine Adresse, die die Anfrage womöglich
//!   nie gestellt hat — Drosseln, das den Reflektor betreibt.
//!   [ADR-0020](../../../docs/adr/0020-rate-limiting-verwirft.md).
//! * **IPv6 wird auf das /64 zusammengefasst.** Ein einzelner Host hat dort
//!   nicht eine Adresse, sondern beliebig viele aus demselben Präfix — mit
//!   Privacy Extensions wechselt er sie im Stundentakt. Pro Adresse gezählt
//!   bekäme derselbe Rechner ständig ein frisches Guthaben, und ein Angreifer
//!   erst recht.
//! * **Die Buckets liegen in mehreren Schubladen** (`SHARDS`), jede hinter
//!   ihrem eigenen Mutex. Ein einzelner Mutex im Anfragepfad wäre genau das,
//!   was CLAUDE.md B.3 Regel 5 verbietet; ein lockfreier Zähler pro Client
//!   wäre die Sorte Code, die man falsch macht.

use std::net::{IpAddr, Ipv6Addr};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use lru::LruCache;

use crate::clock::Clock;
use crate::config::RateLimitConfig;

/// Zahl der Schubladen. Zweierpotenz, damit die Zuordnung eine Maskierung ist.
///
/// 16 ist reichlich für einen Heimanschluss und kostet 16 leere LRU-Caches.
const SHARDS: usize = 16;

/// Ein Guthaben-Eimer nach dem Token-Bucket-Verfahren.
#[derive(Debug)]
struct Bucket {
    /// Verbleibendes Guthaben in Anfragen. Gebrochen, weil zwischen zwei
    /// Anfragen selten genau eine ganze Anfrage nachläuft.
    tokens: f64,
    /// Wann zuletzt nachgefüllt wurde.
    last: Instant,
}

/// Der Schlüssel, unter dem gezählt wird.
///
/// Für IPv4 die Adresse selbst, für IPv6 das /64 — siehe Modulkommentar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Bucketed(IpAddr);

impl Bucketed {
    fn from(addr: IpAddr) -> Self {
        match addr {
            IpAddr::V4(v4) => Self(IpAddr::V4(v4)),
            IpAddr::V6(v6) => {
                // Ein v4-mapped v6 (::ffff:10.0.0.1) ist derselbe Host wie die
                // v4-Adresse; ohne diese Zeile hätte er zwei Guthaben.
                if let Some(v4) = v6.to_ipv4_mapped() {
                    return Self(IpAddr::V4(v4));
                }
                let mut octets = v6.octets();
                for byte in octets.iter_mut().skip(8) {
                    *byte = 0;
                }
                Self(IpAddr::V6(Ipv6Addr::from(octets)))
            }
        }
    }
}

/// Drosselt Anfragen pro Client.
pub struct RateLimiter {
    shards: Box<[Mutex<LruCache<Bucketed, Bucket>>]>,
    /// Nachlaufende Anfragen je Sekunde.
    rate: f64,
    /// Höchstes ansammelbares Guthaben.
    burst: f64,
    /// Als `Arc<dyn …>` und nicht generisch: sonst müsste `Server` einen
    /// zweiten Typparameter tragen, den außer dem Limiter niemand braucht.
    clock: Arc<dyn Clock>,
    throttled: AtomicU64,
}

impl std::fmt::Debug for RateLimiter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimiter")
            .field("rate", &self.rate)
            .field("burst", &self.burst)
            .field("throttled", &self.throttled())
            .finish_non_exhaustive()
    }
}

impl RateLimiter {
    /// Baut den Limiter aus der Konfiguration.
    ///
    /// Gibt `None` zurück, wenn er abgeschaltet ist — dann gibt es ihn nicht,
    /// statt dass er jede Anfrage durchwinkt.
    pub fn from_config(config: &RateLimitConfig, clock: Arc<dyn Clock>) -> Option<Arc<Self>> {
        config.enabled.then(|| {
            Arc::new(Self::new(
                config.per_client_qps,
                config.burst,
                config.max_clients,
                clock,
            ))
        })
    }

    /// Für Tests und den Aufbau von Hand.
    pub fn new(qps: u32, burst: u32, max_clients: usize, clock: Arc<dyn Clock>) -> Self {
        // Pro Schublade ein Anteil der Obergrenze; mindestens einer, sonst
        // hätte ein `max_clients` unter SHARDS gar keinen Platz.
        // `div_euclid` statt `/`: Clippy verbietet den Operator im Baum
        // (`integer_division`), weil er meistens ein übersehener Rundungsfehler
        // ist. Hier ist das Abrunden genau die Absicht.
        let per_shard =
            NonZeroUsize::new(max_clients.div_euclid(SHARDS)).unwrap_or(NonZeroUsize::MIN);
        let shards = (0..SHARDS)
            .map(|_| Mutex::new(LruCache::new(per_shard)))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            shards,
            rate: f64::from(qps),
            burst: f64::from(burst.max(1)),
            clock,
            throttled: AtomicU64::new(0),
        }
    }

    /// Darf diese Anfrage bearbeitet werden?
    ///
    /// Zieht bei `true` genau eine Anfrage vom Guthaben ab.
    pub fn allow(&self, peer: IpAddr) -> bool {
        let key = Bucketed::from(peer);
        let now = self.clock.now();
        let shard = self.shard_for(&key);
        let mut buckets = shard.lock().unwrap_or_else(PoisonError::into_inner);

        let bucket = match buckets.get_mut(&key) {
            Some(bucket) => bucket,
            None => {
                // Neuer Client: volles Guthaben, davon gleich eine Anfrage.
                buckets.put(
                    key,
                    Bucket {
                        tokens: self.burst - 1.0,
                        last: now,
                    },
                );
                return true;
            }
        };

        // `saturating_duration_since`, weil `Instant` bei einer Testuhr auch
        // stehen bleiben darf und ein `duration_since` dann panickt.
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.last = now;
        bucket.tokens = (bucket.tokens + elapsed * self.rate).min(self.burst);
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            self.throttled.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Wie viele Anfragen seit dem Start verworfen wurden.
    pub fn throttled(&self) -> u64 {
        self.throttled.load(Ordering::Relaxed)
    }

    /// Wie viele Clients gerade beobachtet werden.
    pub fn tracked(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.lock().unwrap_or_else(PoisonError::into_inner).len())
            .sum()
    }

    /// Das konfigurierte Limit, für Metrik und Statusanzeige.
    pub fn limits(&self) -> (f64, f64) {
        (self.rate, self.burst)
    }

    fn shard_for(&self, key: &Bucketed) -> &Mutex<LruCache<Bucketed, Bucket>> {
        use std::hash::{BuildHasher as _, RandomState};
        // Ein fester Hasher pro Prozess reicht: die Zuordnung zur Schublade ist
        // kein Geheimnis, sie soll nur gleichmäßig streuen.
        static HASHER: std::sync::OnceLock<RandomState> = std::sync::OnceLock::new();
        let hash = HASHER.get_or_init(RandomState::new).hash_one(key);
        // SHARDS ist eine Zweierpotenz, der Index kann nicht danebenliegen.
        let index = (hash as usize) & (SHARDS - 1);
        self.shards.get(index).unwrap_or_else(|| {
            // Unerreichbar, aber `indexing_slicing` ist verboten und ein
            // `expect` wäre in CI fatal (CLAUDE.md B.1). Die erste Schublade
            // ist eine korrekte, nur unfaire Antwort.
            #[expect(
                clippy::unwrap_used,
                reason = "shards ist nie leer: SHARDS ist eine Konstante > 0"
            )]
            self.shards.first().unwrap()
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::clock::TestClock;

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("Testadresse")
    }

    #[test]
    fn a_client_may_spend_its_burst_and_is_then_throttled() {
        let limiter = RateLimiter::new(10, 5, 1024, Arc::new(TestClock::new()));
        let client = ip("10.0.0.1");
        for i in 0..5 {
            assert!(limiter.allow(client), "Anfrage {i} im Burst abgelehnt");
        }
        assert!(!limiter.allow(client), "sechste Anfrage kam durch");
        assert_eq!(limiter.throttled(), 1);
    }

    #[test]
    fn other_clients_are_unaffected() {
        let limiter = RateLimiter::new(10, 5, 1024, Arc::new(TestClock::new()));
        let loud = ip("10.0.0.1");
        let quiet = ip("10.0.0.2");
        for _ in 0..20 {
            let _ = limiter.allow(loud);
        }
        assert!(!limiter.allow(loud), "der laute Client kam durch");
        for i in 0..5 {
            assert!(
                limiter.allow(quiet),
                "der leise Client wurde bei {i} gedrosselt"
            );
        }
    }

    #[test]
    fn credit_refills_over_time() {
        let clock = Arc::new(TestClock::new());
        let limiter = RateLimiter::new(10, 5, 1024, Arc::clone(&clock) as Arc<dyn Clock>);
        let client = ip("10.0.0.1");
        for _ in 0..5 {
            assert!(limiter.allow(client));
        }
        assert!(!limiter.allow(client));

        // 10 Anfragen/s heißt: nach einer halben Sekunde sind fünf zurück.
        clock.advance(Duration::from_millis(500));
        for i in 0..5 {
            assert!(
                limiter.allow(client),
                "nach dem Nachlaufen bei {i} abgelehnt"
            );
        }
        assert!(
            !limiter.allow(client),
            "mehr als das Nachgelaufene kam durch"
        );
    }

    #[test]
    fn credit_never_grows_beyond_the_burst() {
        let clock = Arc::new(TestClock::new());
        let limiter = RateLimiter::new(10, 5, 1024, Arc::clone(&clock) as Arc<dyn Clock>);
        let client = ip("10.0.0.1");
        assert!(limiter.allow(client));
        // Eine Stunde Ruhe ergibt nicht 36 000 Anfragen auf einen Schlag.
        clock.advance(Duration::from_secs(3600));
        for _ in 0..5 {
            assert!(limiter.allow(client));
        }
        assert!(!limiter.allow(client), "der Eimer lief über");
    }

    /// Mit Privacy Extensions wechselt ein Host seine v6-Adresse laufend. Pro
    /// Adresse gezählt hätte er jedes Mal wieder volles Guthaben.
    #[test]
    fn ipv6_addresses_in_the_same_prefix_share_one_bucket() {
        let limiter = RateLimiter::new(10, 5, 1024, Arc::new(TestClock::new()));
        for _ in 0..5 {
            assert!(limiter.allow(ip("2001:db8:1:2::1")));
        }
        assert!(
            !limiter.allow(ip("2001:db8:1:2:aaaa:bbbb:cccc:dddd")),
            "eine zweite Adresse aus demselben /64 bekam ein eigenes Guthaben"
        );
        // Ein anderes /64 ist ein anderer Client.
        assert!(limiter.allow(ip("2001:db8:1:3::1")));
    }

    #[test]
    fn a_v4_mapped_address_is_the_same_client_as_the_v4_address() {
        let limiter = RateLimiter::new(10, 2, 1024, Arc::new(TestClock::new()));
        assert!(limiter.allow(ip("10.0.0.1")));
        assert!(limiter.allow(ip("::ffff:10.0.0.1")));
        assert!(
            !limiter.allow(ip("10.0.0.1")),
            "die gemappte Adresse hatte ein eigenes Guthaben"
        );
    }

    /// Eine Flut gefälschter Absender darf den Speicher nicht wachsen lassen.
    #[test]
    fn the_number_of_tracked_clients_stays_bounded() {
        let limiter = RateLimiter::new(10, 5, 160, Arc::new(TestClock::new()));
        for i in 0..10_000_u32 {
            let _ = limiter.allow(IpAddr::from(i.to_be_bytes()));
        }
        assert!(
            limiter.tracked() <= 160,
            "{} Clients im Speicher, erlaubt sind 160",
            limiter.tracked()
        );
    }

    #[test]
    fn disabled_means_there_is_no_limiter_at_all() {
        let config = RateLimitConfig {
            enabled: false,
            ..RateLimitConfig::default()
        };
        assert!(RateLimiter::from_config(&config, Arc::new(TestClock::new())).is_none());
    }
}
