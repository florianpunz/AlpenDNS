# Architektur

Dieses Dokument beschreibt das Zielbild. Was davon schon existiert, steht in
[ROADMAP.md](ROADMAP.md).

## 1. Grundform

AlpenDNS ist ein einzelner Prozess, ein einzelnes Binary, eine Konfigurationsdatei.
Kein Datenbankserver, kein Redis, kein Sidecar. Das ist Absicht: die Zielgruppe ist
jemand, der eine Kiste im Keller hat und will, dass sie läuft.

Innerhalb des Prozesses gibt es fünf Schichten, die eine Anfrage nacheinander durchläuft:

```
                 ┌──────────────────────────────────────────────┐
  UDP/53 ──┐     │ 1  Listener                                  │
  TCP/53 ──┼────▶│    Transport terminieren, Nachricht parsen,   │
  DoT/853  │     │    Rate-Limit, Client-Adresse feststellen     │
  DoH/443 ─┤     └───────────────────┬──────────────────────────┘
  DoQ/853 ─┘                         │  Request { msg, client_id }
                                     ▼
                 ┌──────────────────────────────────────────────┐
                 │ 2  Policy                                     │
                 │    Client → Policy auflösen                   │
                 │    Allowlist → Blocklisten → Regex → Zeitplan │
                 │    Heuristiken (DGA/Tunnel/Typosquat)         │
                 └───────────┬──────────────────────┬───────────┘
                             │ Verdict::Block       │ Verdict::Allow
                             ▼                      ▼
                 ┌────────────────────┐  ┌──────────────────────┐
                 │ synthetische        │  │ 3  Cache             │
                 │ Antwort             │  │    Hit → zurück      │
                 │ (NXDOMAIN/0.0.0.0/  │  │    Miss → weiter     │
                 │  REFUSED/Sinkhole)  │  │    Stale → parallel  │
                 └─────────┬──────────┘  └──────────┬───────────┘
                           │                        │ Miss
                           │                        ▼
                           │             ┌──────────────────────┐
                           │             │ 4  Resolve-Backend   │
                           │             │    (Trait)           │
                           │             │  ├ Forwarder (v1)    │
                           │             │  └ Recursor (offen)  │
                           │             └──────────┬───────────┘
                           │                        │ Antwort
                           │                        ▼
                           │             ┌──────────────────────┐
                           │             │ 5  Post-Processing   │
                           │             │  Rebinding-Check,    │
                           │             │  Antwort validieren, │
                           │             │  TTL klemmen, cachen │
                           │             └──────────┬───────────┘
                           └────────────┬───────────┘
                                        ▼
                             Decision-Trace → Logging/Metrik/API
```

### Die Stelle, an der Rekursion später einhängen würde

Schicht 4 ist ein Trait:

```rust
trait ResolveBackend: Send + Sync {
    async fn resolve(&self, q: &Query, ctx: &Ctx) -> Result<Response, ResolveError>;
}
```

v1 implementiert genau eine Variante: `ForwardBackend`. Wenn eines Tages ein
`RecursiveBackend` dazukommt, ändert sich an den Schichten 1, 2, 3 und 5 nichts.
Das ist der komplette Vorbau für Rekursion — mehr wird bewusst nicht gemacht
([ADR-0003](adr/0003-forwarder-first.md)).

## 2. Der Decision-Trace

Das ist das architektonisch wichtigste Detail und der Grund, warum "warum wurde das
geblockt?" in AlpenDNS beantwortbar ist.

Jede Anfrage erzeugt einen `Trace` — eine kleine, allokationsarme Liste von Schritten:

```rust
struct Trace {
    id: u64,                 // monoton, Prozess-lokal
    steps: SmallVec<[Step; 8]>,
    verdict: Verdict,
    elapsed: Duration,
}

enum Step {
    ClientMatched { client: ClientId, by: MatchKind },
    PolicyApplied { policy: PolicyId },
    AllowlistHit  { list: ListId, rule: RuleRef },
    BlocklistHit  { list: ListId, rule: RuleRef },
    ScheduleHit   { schedule: ScheduleId },
    Heuristic     { name: &'static str, score: f32, action: Action },
    CacheHit      { ttl_left: u32, stale: bool },
    UpstreamUsed  { pool: PoolId, resolver: ResolverId, rtt: Duration },
    Synthesized   { mode: BlockMode },
}
```

Der Trace ist **immer** da, unabhängig vom Log-Modus. Was mit ihm passiert, entscheidet
die Logging-Schicht:

| `privacy.logging.mode` | Was mit dem Trace passiert |
|---|---|
| `none` | Zähler hochzählen, Trace verwerfen |
| `aggregate` | Zähler + Häufigkeit unter einem gesalzenen Hash, exakt gezählt; erst ab `aggregate_k` Treffern taucht der Name in Statistiken auf ([ADR-0015](adr/0015-exakte-zaehlung-statt-sketch.md)) |
| `ring` | zusätzlich für `ring_seconds` in einem RAM-Ringpuffer, nie auf Platte |
| `full` | zusätzlich als strukturierte Zeile auf Platte |

Die UI zeigt "warum" aus dem Ringpuffer. Deshalb funktioniert die Erklärung auch bei
Log-Modus `ring`, ohne dass irgendwo ein Query-Log liegt.

**Eine Abweichung in der Umsetzung** (Phase 5): die Schritte tragen Namen als
`Arc<str>` statt IDs, damit ein Trace ohne die Konfiguration daneben lesbar ist.
Begründung: [ADR-0009](adr/0009-decision-trace-mit-mutex.md).

Die zweite Abweichung — der Trace hinter einem `Mutex` statt exklusiv durchgereicht
— ist wieder weg. Sie hatte genau einen Grund, `fanout > 1`, und der ist mit
`fanout` entfallen ([ADR-0012](adr/0012-fanout-entfaellt.md)). `resolve` bekommt
den Kontext als `&mut Ctx`.

## 3. Blocklisten: Datenstruktur

Das Naive wäre ein `HashSet<String>` mit den Domains. Bei 1–2 Millionen Einträgen sind das
je nach Länge 100–200 MB und pro Anfrage mehrere Lookups (für jede Suffix-Ebene einen).

Das Zielmodell:

1. **Labels umdrehen und internieren.** `ads.example.com` → `com.example.ads`. Damit wird
   Wildcard-Matching ein Präfix-Problem statt eines Suffix-Problems.
2. **Ein Bloom-Filter davor.** Die überwältigende Mehrheit der Anfragen ist nicht auf einer
   Liste. Ein Bloom-Filter mit 1 % Fehlerrate beantwortet die in ~50 ns ohne Cache-Miss.
3. **Dahinter die exakte Struktur**, nur bei Bloom-Treffer befragt.
4. **Ein Match liefert eine `RuleRef`** (Listen-ID + Zeilennummer), nicht nur `true` —
   sonst gibt es keinen Trace.

**Gemessen, und vorerst nicht gebaut:** Phase 4 hat die einfache Variante umgesetzt und
vermessen — zwei Millionen Einträge, Nachschlagen p99 unter einer Mikrosekunde, 135 MB.
Damit rechtfertigt keine Zahl den Bloom-Filter. Das Zielmodell oben bleibt als Plan
stehen; die Entscheidung und die Zahlen stehen in
[ADR-0008](adr/0008-hashmap-statt-bloom-und-trie.md).

**Updates ohne Ausfall:** Listen werden in eine neue Struktur geladen und per
`arc_swap::ArcSwap` atomar getauscht. Laufende Anfragen sehen die alte, neue die neue.
Kein Lock im heißen Pfad.

## 4. Cache

* Key: `(Name (lowercase), QType, QClass)`. Der Name wird für den Key normalisiert, für
  0x20 zum Upstream aber in Originalschreibweise gehalten.
* Wert: die vollständige Antwort plus Ablaufzeitpunkt, nicht die Rest-TTL — sonst muss man
  bei jedem Hit rechnen, statt zu vergleichen.
* TTL wird auf `[min_ttl, max_ttl]` geklemmt, negative Antworten separat.
* **Serve-stale (RFC 8767):** Bei Ablauf wird die alte Antwort ausgeliefert *und* parallel
  eine Auffrischung angestoßen. Der Client wartet nicht.
* **Prefetch:** Einträge, die in ihrem Leben mehr als N-mal getroffen wurden, werden bei
  85 % der TTL im Hintergrund erneuert. Praktischer Nebeneffekt: gleichmäßigerer
  Upstream-Verkehr, weniger Korrelierbarkeit von "Nutzer war gerade aktiv".
* **Cache-Isolation zwischen Policies:** Der Cache speichert die *unfilterte* Antwort.
  Filterung passiert vor dem Cache. Damit teilen sich alle Clients einen Cache, ohne dass
  die Policy des einen die Antwort des anderen beeinflusst.

## 5. Upstream-Auswahl

Ein Pool ist eine Liste von Resolvern plus eine Strategie — und es gibt nur noch eine:

* `split_by_zone` — Der Upstream wird über `hash(registrable_domain) % n` bestimmt,
  mit einem beim Start zufällig gezogenen Seed. Folgen: derselbe Name geht immer zum
  selben Resolver (Cache bleibt wirksam), aber jeder Resolver sieht nur ~1/n deiner
  Domains, und welches Drittel er sieht, ist bei jedem Neustart anders. Details und
  Grenzen: [FEATURES.md](FEATURES.md), P2.

**Entfernt:** `fastest` (EWMA der RTT) und `round_robin`. Beide sind klassisch und
beide laufen darauf hinaus, dass am Ende jeder Upstream alles gesehen hat — genau das,
wogegen dieses Projekt antritt. Sie standen zwei Absätze über ihrer eigenen Widerlegung.
Begründung: [ADR-0011](adr/0011-eine-upstream-strategie.md).

Health-Checking: passiv über Fehlerraten und Timeouts, nicht über aktive Probes — aktive
Probes sind selbst wieder ein Signal.

## 6. Client-Identität

In dieser Reihenfolge ausgewertet, erste Übereinstimmung gewinnt:

1. **mTLS-Client-Zertifikat** (DoT/DoQ) — stärkste Bindung, überlebt Netzwechsel.
2. **DoH-Pfad-Token** — `https://dns.miloo.at/dns-query/<32 zufällige Bytes>`.
   Funktioniert mit jedem Standard-DoH-Client, ohne Zertifikate zu verteilen.
3. **Quell-IP / Subnetz** — der LAN-Normalfall.
4. **Default-Policy** — alles, was nicht zugeordnet werden konnte.

Ein Token gehört in die Konfiguration, nicht in ein Log, und wird in der API nur mit den
letzten vier Zeichen angezeigt.

## 7. Konfiguration und Reload

Eine TOML-Datei, `serde` mit `deny_unknown_fields`. Zwei Wege, sie neu zu laden:

* `SIGHUP` → Konfiguration neu parsen. **Schlägt das Parsen fehl, bleibt die alte
  Konfiguration aktiv** und der Fehler geht ins Log. Ein Reload darf nie zu einem Zustand
  führen, in dem gefiltert werden sollte, aber nicht gefiltert wird.
* `alpendns check -c /etc/alpendns/alpendns.toml` → validiert ohne Neustart, nutzbar als
  `ExecStartPre` in der systemd-Unit.

Hot-reloadbar ist die **Policy-Schicht**: Clients, Policies, Regex, Zeitpläne und die
Listenquellen (URLs/Formate) werden beim Reload neu gebaut und atomar eingetauscht. Das ist
genau der Zustand, den `run_updater` ohnehin periodisch aus `Blueprint` + `Lists` aufbaut —
der Reload weckt ihn nur, statt auf den nächsten Refresh-Tick zu warten.

Alles andere erfordert einen Neustart: Listener-Adressen, Upstreams/TLS, `forward_zone`,
Cache-Konfiguration, Drosselung, `blocking.mode`/Sinkholes, Detektoren und
`privacy.logging.mode`. Der Reload nennt diese Grenze im Log ausdrücklich, statt eine
Änderung stillschweigend zu ignorieren.

## 8. Nebenläufigkeit

* Tokio multithreaded, ein Task pro Anfrage.
* Der UDP-Socket wird mit `SO_REUSEPORT` mehrfach geöffnet (ein Socket pro Worker), damit
  der Kernel die Verteilung übernimmt — das ist der Unterschied zwischen 30k und 200k
  Anfragen/s auf derselben Hardware.
* Geteilter Zustand: `ArcSwap` für alles, was selten wechselt (Listen, Config, Policies),
  eine sharded Map für den Cache. Kein globaler `Mutex` im Anfragepfad.
* **Query-Deduplizierung:** 500 Clients, die gleichzeitig denselben ungecachten Namen
  fragen, erzeugen genau eine Upstream-Anfrage. Ohne das ist ein Cache-Ablauf ein
  selbstgebauter Lastspitzen-Generator.

## 9. Persistenz

Default: **keine**. Der Prozess hält alles im RAM.

Optional auf Platte, jeweils abschaltbar:

* Blocklisten-Cache (`/var/cache/alpendns/lists/`) — damit ein Neustart ohne Internet
  gefiltert startet.
* Aggregierte Statistiken (`/var/lib/alpendns/stats.redb`) — Zähler pro Tag, keine
  Query-Namen unterhalb der k-Schwelle.
* Cache-Snapshot beim Shutdown — optional, spart den kalten Start.

Kein Query-Log, außer der Betreiber schaltet `full` ausdrücklich ein.

## 10. Fehlerbehandlung

| Situation | Verhalten |
|---|---|
| Upstream-Timeout | nächster Resolver im Pool; alle tot → `serve_stale`, sonst SERVFAIL |
| Blockliste nicht ladbar | zuletzt gecachte Version, Log-Fehler, Metrik hoch, Start nicht verhindern |
| Blockliste beim Erststart nicht ladbar | Start abbrechen — lieber kein DNS als ungefiltertes DNS |
| Kaputte Anfrage vom Client | FORMERR, Zähler hoch, kein Log-Spam pro Paket |
| Antwort passt nicht zur Frage | verwerfen, nicht cachen, als möglichen Spoofing-Versuch zählen |
| Config-Reload schlägt fehl | alte Config bleibt, Fehler ins Log |
| Panic in einem Task | darf nicht vorkommen (siehe CLAUDE.md B.1); wenn doch, `panic = "abort"` — ein halb kaputter Resolver ist schlimmer als ein neustartender |
