//! Antwort-Cache mit TTL-Klemmung, serve-stale und Prefetch.
//!
//! Der Cache speichert die **ungefilterte** Antwort (ARCHITECTURE.md §4). Gefiltert
//! wird davor. Nur deshalb können sich alle Clients einen Cache teilen, ohne dass
//! die Policy des einen die Antwort des anderen beeinflusst.
//!
//! Gespeichert wird der Einfügezeitpunkt plus die geklemmte TTL, nicht die
//! Rest-TTL — sonst müsste bei jedem Treffer gerechnet statt verglichen werden.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use hickory_proto::op::{Message, Query, ResponseCode};
use hickory_proto::rr::{DNSClass, Name, RData, RecordType};
use lru::LruCache;
use std::sync::Arc;

use crate::clock::Clock;
use crate::config::CacheConfig;

/// Anzahl der Shards. Zweierpotenz, damit die Zuordnung eine Maskierung ist.
/// Ein einzelner Mutex über dem gesamten Cache wäre ein globaler Lock im
/// Anfragepfad (ARCHITECTURE.md §8).
const SHARDS: usize = 16;

/// TTL, mit der eine abgelaufene Antwort ausgeliefert wird (RFC 8767 §5
/// empfiehlt einen kleinen Wert, damit der Client bald wieder fragt).
const STALE_TTL: u32 = 30;

/// Cache-Schlüssel: Name, Typ, Klasse.
///
/// `Name` vergleicht und hasht in `hickory-proto` case-insensitiv — genau die
/// Normalisierung, die ein DNS-Cache braucht.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    name: Name,
    query_type: RecordType,
    query_class: DNSClass,
}

impl Key {
    pub fn from_query(query: &Query) -> Self {
        Self {
            name: query.name().clone(),
            query_type: query.query_type(),
            query_class: query.query_class(),
        }
    }
}

/// Ein Treffer im Cache.
#[derive(Debug)]
pub struct Hit {
    /// Die Antwort, TTLs bereits auf die Restlaufzeit gesetzt.
    pub response: Message,
    /// Der Eintrag war abgelaufen und wurde aus dem serve-stale-Fenster geliefert.
    pub stale: bool,
    /// Der Eintrag nähert sich dem Ablauf und sollte im Hintergrund erneuert werden.
    pub should_prefetch: bool,
}

#[derive(Debug)]
struct Entry {
    response: Arc<Message>,
    stored_at: Instant,
    /// Die geklemmte TTL, mit der eingefügt wurde.
    ttl: u32,
}

/// Zähler über die Lebensdauer des Prozesses.
///
/// Nur Summen, keine Namen — das ist unabhängig vom Log-Modus zulässig
/// (CLAUDE.md B.1, Regel 3). Der Prometheus-Endpunkt in Phase 6 liest hier ab.
#[derive(Debug, Default)]
struct Counters {
    hits: AtomicU64,
    stale_hits: AtomicU64,
    misses: AtomicU64,
    inserts: AtomicU64,
}

/// Momentaufnahme der Zähler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Treffer auf einen noch gültigen Eintrag.
    pub hits: u64,
    /// Treffer auf einen abgelaufenen Eintrag im serve-stale-Fenster.
    pub stale_hits: u64,
    pub misses: u64,
    /// Antworten, die tatsächlich abgelegt wurden.
    pub inserts: u64,
}

impl Stats {
    /// Anteil der Anfragen, die der Cache beantwortet hat — frisch oder stale.
    ///
    /// Ohne jede Anfrage ist die Quote 0.0 und nicht undefiniert.
    pub fn hit_rate(&self) -> f64 {
        let served = self.hits.saturating_add(self.stale_hits);
        let total = served.saturating_add(self.misses);
        if total == 0 {
            return 0.0;
        }
        served as f64 / total as f64
    }
}

/// Antwort-Cache.
#[derive(Debug)]
pub struct Cache<C> {
    shards: Box<[Mutex<LruCache<Key, Entry>>]>,
    counters: Counters,
    clock: C,
    min_ttl: u32,
    max_ttl: u32,
    max_negative_ttl: u32,
    serve_stale_max: u32,
    serve_stale: bool,
    prefetch_threshold: f32,
}

impl<C: Clock> Cache<C> {
    pub fn new(config: &CacheConfig, clock: C) -> Self {
        // Mindestens ein Eintrag pro Shard, sonst wäre der Cache bei kleinen
        // max_entries wirkungslos.
        let per_shard = config.max_entries.div_ceil(SHARDS).max(1);
        let capacity = NonZeroUsize::new(per_shard).unwrap_or(NonZeroUsize::MIN);
        let shards = (0..SHARDS)
            .map(|_| Mutex::new(LruCache::new(capacity)))
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Self {
            shards,
            counters: Counters::default(),
            clock,
            min_ttl: secs(config.min_ttl),
            max_ttl: secs(config.max_ttl),
            max_negative_ttl: secs(config.max_negative_ttl),
            serve_stale_max: secs(config.serve_stale_max),
            serve_stale: config.serve_stale,
            prefetch_threshold: config.prefetch_threshold,
        }
    }

    fn shard(&self, key: &Key) -> Option<&Mutex<LruCache<Key, Entry>>> {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        // SHARDS ist eine Zweierpotenz, deshalb Maskierung statt Modulo.
        self.shards.get(hasher.finish() as usize & (SHARDS - 1))
    }

    /// Sucht einen Eintrag und setzt die TTLs auf die Restlaufzeit.
    pub fn get(&self, key: &Key) -> Option<Hit> {
        let now = self.clock.now();
        let Some(shard) = self.shard(key) else {
            self.counters.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        let mut guard = shard.lock().unwrap_or_else(PoisonError::into_inner);

        // Der Borrow auf den Eintrag endet hier, damit unten `pop` möglich ist.
        let Some((response, stored_at, ttl)) = guard
            .get(key)
            .map(|entry| (Arc::clone(&entry.response), entry.stored_at, entry.ttl))
        else {
            self.counters.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };

        let elapsed = secs(now.saturating_duration_since(stored_at));
        if elapsed < ttl {
            let remaining = ttl.saturating_sub(elapsed);
            let used = f64::from(elapsed) / f64::from(ttl.max(1));
            self.counters.hits.fetch_add(1, Ordering::Relaxed);
            return Some(Hit {
                response: with_ttl(&response, remaining),
                stale: false,
                should_prefetch: used >= f64::from(self.prefetch_threshold),
            });
        }

        let stale_deadline = ttl.saturating_add(self.serve_stale_max);
        if self.serve_stale && elapsed < stale_deadline {
            self.counters.stale_hits.fetch_add(1, Ordering::Relaxed);
            return Some(Hit {
                response: with_ttl(&response, STALE_TTL),
                stale: true,
                should_prefetch: false,
            });
        }

        // Endgültig abgelaufen: Platz freigeben, statt ihn bis zur Verdrängung
        // zu halten.
        guard.pop(key);
        self.counters.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Momentaufnahme der Zähler.
    pub fn stats(&self) -> Stats {
        Stats {
            hits: self.counters.hits.load(Ordering::Relaxed),
            stale_hits: self.counters.stale_hits.load(Ordering::Relaxed),
            misses: self.counters.misses.load(Ordering::Relaxed),
            inserts: self.counters.inserts.load(Ordering::Relaxed),
        }
    }

    /// Legt eine Antwort ab, sofern sie cachefähig ist.
    ///
    /// Gibt die verwendete TTL zurück, oder `None`, wenn nicht gecacht wurde.
    pub fn insert(&self, key: Key, response: &Message) -> Option<u32> {
        let ttl = self.cacheable_ttl(response)?;
        let shard = self.shard(&key)?;
        let entry = Entry {
            response: Arc::new(response.clone()),
            stored_at: self.clock.now(),
            ttl,
        };
        shard
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .put(key, entry);
        self.counters.inserts.fetch_add(1, Ordering::Relaxed);
        Some(ttl)
    }

    /// Entscheidet, ob und wie lange eine Antwort gecacht werden darf.
    ///
    /// Positive Antworten bekommen die kleinste TTL ihrer Answer-Records,
    /// geklemmt auf `[min_ttl, max_ttl]`. Negative Antworten (NXDOMAIN und
    /// NODATA) richten sich nach RFC 2308 §5 nach dem SOA der Authority-Section;
    /// ohne SOA gibt es keinen belastbaren Anhaltspunkt und wir cachen nicht.
    fn cacheable_ttl(&self, response: &Message) -> Option<u32> {
        // Eine gekürzte Antwort ist unvollständig; sie zu cachen würde den
        // Fehler festschreiben.
        if response.metadata.truncation {
            return None;
        }

        match response.metadata.response_code {
            ResponseCode::NoError if !response.answers.is_empty() => {
                let smallest = response
                    .answers
                    .iter()
                    .filter(|record| record.record_type() != RecordType::OPT)
                    .map(|record| record.ttl)
                    .min()?;
                Some(smallest.clamp(self.min_ttl, self.max_ttl))
            }
            ResponseCode::NoError | ResponseCode::NXDomain => {
                let soa_ttl =
                    response
                        .authorities
                        .iter()
                        .find_map(|record| match &record.data {
                            RData::SOA(soa) => Some(record.ttl.min(soa.minimum)),
                            _ => None,
                        })?;
                Some(soa_ttl.clamp(self.min_ttl, self.max_negative_ttl))
            }
            // SERVFAIL, REFUSED und alles andere sind Zustandsmeldungen, keine
            // Daten. Sie zu cachen würde einen kurzen Ausfall verlängern.
            _ => None,
        }
    }

    /// Anzahl der abgelegten Einträge. Für Tests und später für Metriken.
    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.lock().unwrap_or_else(PoisonError::into_inner).len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Kopiert eine Antwort und setzt alle Record-TTLs auf `ttl`.
///
/// Alle Records bekommen denselben Wert, nämlich die Restlaufzeit des Eintrags.
/// Das ist die kleinste TTL des Satzes und damit nie zu großzügig — ein Record
/// bleibt so nie länger gültig, als er beim Einfügen war.
fn with_ttl(response: &Message, ttl: u32) -> Message {
    let mut response = response.clone();
    for record in response
        .answers
        .iter_mut()
        .chain(response.authorities.iter_mut())
        .chain(response.additionals.iter_mut())
    {
        // Bei OPT ist das TTL-Feld kein TTL, sondern trägt Flags und Rcode-Bits.
        if record.record_type() != RecordType::OPT {
            record.ttl = ttl;
        }
    }
    response
}

/// Sekunden einer Dauer als `u32`, gesättigt.
fn secs(duration: Duration) -> u32 {
    u32::try_from(duration.as_secs()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TestClock;
    use hickory_proto::op::{MessageType, OpCode};
    use hickory_proto::rr::Record;
    use hickory_proto::rr::rdata::{A, SOA};
    use std::net::Ipv4Addr;

    fn name(text: &str) -> Name {
        Name::from_ascii(text).expect("gültiger Name")
    }

    fn config() -> CacheConfig {
        CacheConfig {
            max_entries: 1000,
            min_ttl: Duration::from_secs(10),
            max_ttl: Duration::from_secs(86400),
            max_negative_ttl: Duration::from_secs(900),
            serve_stale: true,
            serve_stale_max: Duration::from_secs(3600),
            prefetch: true,
            prefetch_threshold: 0.85,
        }
    }

    fn cache() -> Cache<Arc<TestClock>> {
        Cache::new(&config(), Arc::new(TestClock::new()))
    }

    fn key(text: &str) -> Key {
        Key::from_query(&Query::query(name(text), RecordType::A))
    }

    fn answer(text: &str, ttl: u32) -> Message {
        let mut msg = Message::new(1, MessageType::Response, OpCode::Query);
        msg.add_query(Query::query(name(text), RecordType::A));
        msg.add_answer(Record::from_rdata(
            name(text),
            ttl,
            RData::A(A(Ipv4Addr::new(10, 0, 0, 1))),
        ));
        msg
    }

    fn nxdomain(text: &str, soa_ttl: u32, minimum: u32) -> Message {
        let mut msg = Message::new(1, MessageType::Response, OpCode::Query);
        msg.metadata.response_code = ResponseCode::NXDomain;
        msg.add_query(Query::query(name(text), RecordType::A));
        msg.add_authority(Record::from_rdata(
            name("example.com."),
            soa_ttl,
            RData::SOA(SOA::new(
                name("ns.example.com."),
                name("hostmaster.example.com."),
                1,
                7200,
                3600,
                1209600,
                minimum,
            )),
        ));
        msg
    }

    #[test]
    fn insert_then_lookup_returns_the_entry() {
        let cache = cache();
        assert!(cache.is_empty());
        cache.insert(key("example.com."), &answer("example.com.", 300));

        let hit = cache.get(&key("example.com.")).expect("Treffer erwartet");
        assert!(!hit.stale);
        assert_eq!(hit.response.answers.len(), 1);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn lookup_is_case_insensitive() {
        let cache = cache();
        cache.insert(key("Example.COM."), &answer("Example.COM.", 300));
        assert!(cache.get(&key("eXaMpLe.com.")).is_some());
    }

    #[test]
    fn miss_on_unknown_key() {
        assert!(cache().get(&key("nichts.example.")).is_none());
    }

    #[test]
    fn remaining_ttl_counts_down() {
        let clock = Arc::new(TestClock::new());
        let cache = Cache::new(&config(), Arc::clone(&clock));
        cache.insert(key("example.com."), &answer("example.com.", 300));

        clock.advance(Duration::from_secs(100));
        let hit = cache.get(&key("example.com.")).expect("noch frisch");
        assert_eq!(hit.response.answers.first().expect("ein Record").ttl, 200);
    }

    #[test]
    fn entry_expires_and_is_served_stale_then_dropped() {
        let clock = Arc::new(TestClock::new());
        let cache = Cache::new(&config(), Arc::clone(&clock));
        cache.insert(key("example.com."), &answer("example.com.", 300));

        clock.advance(Duration::from_secs(301));
        let hit = cache.get(&key("example.com.")).expect("stale erwartet");
        assert!(hit.stale);
        assert_eq!(
            hit.response.answers.first().expect("ein Record").ttl,
            STALE_TTL
        );

        // Jenseits von serve_stale_max ist der Eintrag weg.
        clock.advance(Duration::from_secs(3601));
        assert!(cache.get(&key("example.com.")).is_none());
        assert_eq!(cache.len(), 0, "abgelaufener Eintrag wurde nicht entfernt");
    }

    #[test]
    fn stale_is_not_served_when_disabled() {
        let clock = Arc::new(TestClock::new());
        let mut cfg = config();
        cfg.serve_stale = false;
        let cache = Cache::new(&cfg, Arc::clone(&clock));
        cache.insert(key("example.com."), &answer("example.com.", 300));

        clock.advance(Duration::from_secs(301));
        assert!(cache.get(&key("example.com.")).is_none());
    }

    #[test]
    fn ttl_is_clamped_to_the_configured_range() {
        let cache = cache();
        assert_eq!(
            cache.insert(key("kurz.example."), &answer("kurz.example.", 1)),
            Some(10),
            "unter min_ttl"
        );
        assert_eq!(
            cache.insert(key("lang.example."), &answer("lang.example.", 999_999)),
            Some(86400),
            "über max_ttl"
        );
    }

    #[test]
    fn negative_answer_uses_the_soa_minimum() {
        let cache = cache();
        // Kleineres von SOA-TTL (3600) und minimum (60).
        assert_eq!(
            cache.insert(key("weg.example."), &nxdomain("weg.example.", 3600, 60)),
            Some(60)
        );
    }

    #[test]
    fn negative_answer_is_clamped_to_max_negative_ttl() {
        let cache = cache();
        assert_eq!(
            cache.insert(key("weg.example."), &nxdomain("weg.example.", 86400, 86400)),
            Some(900)
        );
    }

    #[test]
    fn negative_answer_without_soa_is_not_cached() {
        let cache = cache();
        let mut msg = Message::new(1, MessageType::Response, OpCode::Query);
        msg.metadata.response_code = ResponseCode::NXDomain;
        msg.add_query(Query::query(name("weg.example."), RecordType::A));
        assert_eq!(cache.insert(key("weg.example."), &msg), None);
        assert!(cache.is_empty());
    }

    #[test]
    fn servfail_is_not_cached() {
        let cache = cache();
        let mut msg = answer("example.com.", 300);
        msg.metadata.response_code = ResponseCode::ServFail;
        msg.answers.clear();
        assert_eq!(cache.insert(key("example.com."), &msg), None);
    }

    #[test]
    fn truncated_answer_is_not_cached() {
        let cache = cache();
        let mut msg = answer("example.com.", 300);
        msg.metadata.truncation = true;
        assert_eq!(cache.insert(key("example.com."), &msg), None);
    }

    #[test]
    fn prefetch_is_flagged_past_the_threshold() {
        let clock = Arc::new(TestClock::new());
        let cache = Cache::new(&config(), Arc::clone(&clock));
        cache.insert(key("example.com."), &answer("example.com.", 100));

        clock.advance(Duration::from_secs(84));
        assert!(
            !cache
                .get(&key("example.com."))
                .expect("frisch")
                .should_prefetch,
            "84 % sollten noch nicht auslösen"
        );
        clock.advance(Duration::from_secs(2));
        assert!(
            cache
                .get(&key("example.com."))
                .expect("frisch")
                .should_prefetch,
            "86 % sollten auslösen"
        );
    }

    #[test]
    fn lru_evicts_beyond_max_entries() {
        let mut cfg = config();
        // 16 Shards, ein Eintrag pro Shard.
        cfg.max_entries = SHARDS;
        let cache = Cache::new(&cfg, Arc::new(TestClock::new()));

        for i in 0..200 {
            let domain = format!("host{i}.example.");
            cache.insert(key(&domain), &answer(&domain, 300));
        }
        assert!(
            cache.len() <= SHARDS,
            "Cache hält {} Einträge, erlaubt sind {SHARDS}",
            cache.len()
        );
        assert!(!cache.is_empty(), "es wurde gar nichts behalten");
    }

    proptest::proptest! {
        /// Die Invariante aus docs/TESTING.md §2, in der Form, die RFC 8767
        /// zulässt: eine frische Antwort ist nie länger gültig als beim
        /// Einfügen, eine abgelaufene bekommt genau die stale-TTL.
        #[test]
        fn served_ttl_never_exceeds_the_inserted_one(
            record_ttl in 0_u32..1_000_000,
            elapsed in 0_u64..200_000,
        ) {
            let clock = Arc::new(TestClock::new());
            let cache = Cache::new(&config(), Arc::clone(&clock));
            let inserted = cache
                .insert(key("example.com."), &answer("example.com.", record_ttl))
                .expect("positive Antwort ist cachefähig");

            clock.advance(Duration::from_secs(elapsed));

            if let Some(hit) = cache.get(&key("example.com.")) {
                let served = hit.response.answers.first().expect("ein Record").ttl;
                if hit.stale {
                    proptest::prop_assert_eq!(served, STALE_TTL);
                } else {
                    proptest::prop_assert!(
                        served <= inserted,
                        "ausgeliefert {} > eingefügt {}", served, inserted
                    );
                }
                proptest::prop_assert!(served <= 86400, "über max_ttl: {}", served);
            }
        }
    }

    #[test]
    fn counters_track_hits_misses_and_rate() {
        let clock = Arc::new(TestClock::new());
        let cache = Cache::new(&config(), Arc::clone(&clock));
        assert_eq!(cache.stats().hit_rate(), 0.0, "ohne Anfragen keine Quote");

        assert!(cache.get(&key("example.com.")).is_none());
        cache.insert(key("example.com."), &answer("example.com.", 300));
        assert!(cache.get(&key("example.com.")).is_some());
        assert!(cache.get(&key("example.com.")).is_some());

        clock.advance(Duration::from_secs(301));
        assert!(cache.get(&key("example.com.")).expect("stale").stale);

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.stale_hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.inserts, 1);
        assert!((stats.hit_rate() - 0.75).abs() < f64::EPSILON, "{stats:?}");
    }

    #[test]
    fn opt_record_ttl_is_left_alone() {
        // Das TTL-Feld eines OPT-Records trägt Flags, keine Lebensdauer.
        let mut msg = answer("example.com.", 300);
        msg.add_additional(Record::from_rdata(
            Name::root(),
            0x0000_8000,
            RData::Update0(RecordType::OPT),
        ));
        let adjusted = with_ttl(&msg, 42);
        assert_eq!(adjusted.answers.first().expect("Record").ttl, 42);
    }
}
