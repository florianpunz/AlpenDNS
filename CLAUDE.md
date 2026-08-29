# CLAUDE.md — AlpenDNS

Diese Datei gilt für jeden Coding-Agenten, der in diesem Repository arbeitet.
Sie besteht aus zwei Teilen: den allgemeinen Verhaltensregeln (Teil A, unverändert)
und den projektspezifischen Regeln (Teil B).

---

## Teil A — Behavioral Guidelines

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.
Tradeoff: These guidelines bias toward caution over speed. For trivial tasks, use judgment.
1. Think Before Coding
Don't assume. Don't hide confusion. Surface tradeoffs.
Before implementing:

* State your assumptions explicitly. If uncertain, ask.
* If multiple interpretations exist, present them - don't pick silently.
* If a simpler approach exists, say so. Push back when warranted.
* If something is unclear, stop. Name what's confusing. Ask.

2. Simplicity First
Minimum code that solves the problem. Nothing speculative.

* No features beyond what was asked.
* No abstractions for single-use code.
* No "flexibility" or "configurability" that wasn't requested.
* No error handling for impossible scenarios.
* If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.
3. Surgical Changes
Touch only what you must. Clean up only your own mess.
When editing existing code:

* Don't "improve" adjacent code, comments, or formatting.
* Don't refactor things that aren't broken.
* Match existing style, even if you'd do it differently.
* If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:

* Remove imports/variables/functions that YOUR changes made unused.
* Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.
4. Goal-Driven Execution
Define success criteria. Loop until verified.
Transform tasks into verifiable goals:

* "Add validation" → "Write tests for invalid inputs, then make them pass"
* "Fix the bug" → "Write a test that reproduces it, then make it pass"
* "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:

```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]

```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

## Teil B — Projektspezifisch: AlpenDNS

### B.0 Was das hier ist

AlpenDNS ist ein privacy-fokussierter DNS-Server in Rust für Linux (Debian/Ubuntu).
v1 ist ein **forwarding resolver**: er nimmt Anfragen aus dem LAN entgegen, filtert sie
gegen Blocklisten und Policies, und leitet sie verschlüsselt (DoT/DoH/DoQ) an Upstreams
weiter. Eigene Rekursion ab den Root-Servern ist **kein** v1-Ziel und wird es
möglicherweise nie. Die Architektur hält die Tür dafür offen (ein Trait hinter dem
Cache), aber es wird nichts dafür vorgebaut.

Vor jeder Arbeit: `docs/ROADMAP.md` lesen, dort steht die aktuelle Phase.
Architektur und Begründungen: `docs/ARCHITECTURE.md`, `docs/adr/`.

Der Autor ist Systemadministrator, kein ausgebildeter Softwareentwickler, und lernt an
diesem Projekt. Das heißt konkret: erkläre nicht-offensichtliche Entscheidungen kurz im
Commit oder in der Antwort, statt sie kommentarlos einzubauen. Ein Einzeiler
"warum so und nicht anders" ist mehr wert als drei Absätze Doku.

**Stand:** Phasen 1 bis 4 sind umgesetzt und abgenommen. Der Praxistest im echten
Netz ist bewusst auf Phase 9 verschoben — vorher gibt es keine systemd-Unit und
damit keinen Betrieb auf Port 53. Der gesamte testbare Code
liegt in der Library (`src/lib.rs` und die Module daneben), `main.rs` macht nur
Start, Signale und Shutdown — Voraussetzung dafür, dass Module später ohne Umbau zu
eigenen Crates werden.

Die Pipeline ist eine Kette von `ResolveBackend`-Implementierungen, von außen nach
innen: `FilterBackend` → `CachingBackend` → `ZoneRouter` → `Pool` → `Transport`
(DoT/DoH/DoQ) bzw. `ForwardBackend` (Klartext, nur für `forward_zone`). Keine Schicht
kennt die anderen. Der Filter liegt **vor** dem Cache, damit dieser die ungefilterte
Antwort hält (ARCHITECTURE.md §4) — Phase 5 hängt die Policies zwischen Filter und
Cache, ohne eine der übrigen anzufassen.

Klartext-DNS nach außen ist erledigt (B.1 Regel 7): `udp://` in einem
`upstream_pool` ist ein Startfehler. Eine Abweichung bleibt offen — B.1 Regel 1:
`hickory-proto 0.26.1` panict beim Parsen eines kaputten TSIG-Records, wenn
Overflow-Checks an sind. Bewertung und Auflagen in ADR-0006, bekannter Fall in
`crates/alpendns/fuzz/known-crashes/`. Dazu eine dokumentierte Advisory-Ausnahme in
`deny.toml` (RUSTSEC-2026-0009 in einer dev-dependency). Der CI-Lauf fehlt
weiterhin, dafür gibt es kein GitHub-Remote.

Zahlen in `docs/BENCHMARKS.md`. Aktuelle Arbeit: Phase 5 (Clients und Policies).

**Doku-Karte:** `docs/ROADMAP.md` = aktuelle Phase und Abnahmekriterien ·
`docs/ARCHITECTURE.md` = Zielbild · `docs/TESTING.md` = Teststrategie ·
`docs/THREAT-MODEL.md` = wogegen geschützt wird und wogegen nicht · `docs/FEATURES.md` =
Katalog mit Aufwand/Nutzen · `docs/adr/` = warum etwas so ist ·
`config/alpendns.example.toml` = **Spezifikation des Zielformats**; noch nicht
implementierte Abschnitte sind dort mit `[PHASE n]` markiert.

### B.1 Harte Regeln (nicht verhandelbar)

Diese Regeln überschreiben "Simplicity First" nicht — sie definieren, was in diesem
Projekt "korrekt" heißt.

1. **Jedes Byte vom Netzwerk ist feindlich.** Kein `unwrap()`, kein `expect()`, kein
   `panic!()`, kein Slice-Indexing (`buf[i]`, `&buf[a..b]`) auf irgendeinem Pfad, der
   Netzwerkdaten verarbeitet. Ein Panic im Query-Handler ist eine Denial-of-Service-Lücke.
   Clippy erzwingt das (`unwrap_used = "deny"`, `panic = "deny"`,
   `indexing_slicing = "deny"`). `expect_used` steht in `Cargo.toml` nur auf `warn`, wird
   aber in CI durch `RUSTFLAGS: "-D warnings"` fatal — lokal rutscht ein `expect()` also
   durch, in CI nicht. Erlaubt sind `unwrap()`/`expect()` ausschließlich in
   `#[cfg(test)]`-Code; `clippy.toml` nimmt solchen Code aus. Integrationstests
   unter `tests/` sind ein eigenes Crate und fallen *nicht* darunter — sie
   brauchen `#![allow(clippy::expect_used, clippy::indexing_slicing)]` am
   Dateikopf.
2. **`unsafe` ist verboten** (`unsafe_code = "forbid"` im Workspace). Wenn du glaubst, du
   brauchst es: das ist ein Fall für "stop und nachfragen".
3. **Keine Query-Namen im Log ohne ausdrückliche Konfiguration.** Default ist
   `privacy.logging.mode = "aggregate"`. Ein `tracing::info!("query {name}")` an falscher
   Stelle bricht das zentrale Versprechen des Projekts. Query-Namen dürfen nur über die
   dafür vorgesehene Log-Schicht laufen, die den konfigurierten Modus durchsetzt.
4. **Keine Telemetrie nach außen.** Der Prozess kontaktiert genau drei Sorten Ziele:
   konfigurierte Upstream-Resolver, konfigurierte Blocklisten-URLs, und sonst nichts.
   Kein Update-Check, kein Crash-Reporting, kein "anonymous usage stats".
5. **Der Server startet nicht mit kaputter Konfiguration.** Unbekannte Config-Schlüssel
   sind ein Fehler (`serde(deny_unknown_fields)`), kein Warning. Ein Tippfehler in
   `blocklist` darf nicht dazu führen, dass jemand ungefiltert im Internet hängt und es
   nicht merkt.
6. **Fail closed bei Policy, fail open bei Verfügbarkeit.** Wenn eine Blockliste nicht
   geladen werden kann: Server startet mit der zuletzt gecachten Version und schreit im
   Log; er startet nicht ungefiltert. Wenn ein Upstream tot ist: nächster Upstream,
   notfalls `serve_stale` — Auflösung geht vor Aktualität.
7. **Kein Klartext-DNS nach außen.** Upstreams sind DoT/DoH/DoQ. Ausnahme: explizit
   konfigurierte `forward_zone`-Einträge ins eigene LAN.

### B.2 Rust-Konventionen

* Edition 2024, stable toolchain (`rust-toolchain.toml`). Kein nightly außer für `cargo fuzz`.
* Async-Runtime: **tokio**, multithreaded. Kein zweites Runtime-Framework dazu.
* DNS-Wire-Format: **`hickory-proto`**. Wir parsen und serialisieren DNS-Nachrichten nicht
  selbst (ADR-0002). Die Server-Logik oben drauf ist unsere.
* Fehler: `thiserror` für Bibliotheks-Crates, `anyhow` nur im Binary/`main`.
* Logging: `tracing` mit strukturierten Feldern, nie `println!` außerhalb von CLI-Ausgaben.
* Serialisierung Config: `serde` + `toml`.
* Ein neues Dependency braucht eine Zeile Begründung im PR/Commit. `cargo deny` läuft in CI:
  keine GPL-inkompatiblen Lizenzen, keine Crates mit offenen RUSTSEC-Advisories.
* Öffentliche Items in Library-Crates haben Doc-Kommentare. Private Funktionen nur dann,
  wenn das *Warum* nicht aus dem Code hervorgeht.
* Formatierung: `cargo fmt` mit Default-Settings. Keine Diskussion darüber.

### B.3 Struktur

```
crates/
  alpendns/          Binary: Startup, Config laden, Signale, Shutdown
  alpendns-server/   Listener (UDP/TCP/DoT/DoH/DoQ), Request-Pipeline
  alpendns-cache/    Antwort-Cache, serve-stale, Prefetch
  alpendns-upstream/ Upstream-Pools, Transporte, Auswahlstrategien
  alpendns-filter/   Blocklisten: Parser, Matcher, Update-Scheduler
  alpendns-policy/   Client-Identität, Policy-Auswertung, Entscheidungs-Trace
  alpendns-detect/   Heuristiken (DGA, Tunneling, Rebinding, Typosquat)
  alpendns-api/      HTTP-API + Auslieferung der Web-UI
web/                 Web-UI (siehe B.6)
```

Crates entstehen **erst, wenn die zugehörige Phase drankommt**. Lege keine leeren Crates
"schon mal" an. Wenn Code in `alpendns` noch klein genug ist, bleibt er dort.

**Pipeline:** Listener → Policy → Cache → `ResolveBackend` → Post-Processing (ausführlich
in `docs/ARCHITECTURE.md` §1). Fünf Regeln folgen daraus, die von außen willkürlich
aussehen und trotzdem keine sind:

1. **Gefiltert wird vor dem Cache.** Der Cache hält die *ungefilterte* Antwort. Nur so
   teilen sich alle Clients einen Cache, ohne dass die Policy des einen die Antwort des
   anderen beeinflusst.
2. **Auflösen liegt hinter dem Trait `ResolveBackend`**, mit genau einer Implementierung
   (`ForwardBackend`). ADR-0003 zählt auf, was dieser Trait ausdrücklich *nicht*
   rechtfertigt.
3. **Der `Trace` entsteht immer**, unabhängig vom Log-Modus; der Modus entscheidet nur,
   was mit ihm passiert. Ein Matcher, der `bool` liefert statt einer `RuleRef`, macht den
   Trace unbrauchbar und ist deshalb falsch.
4. **Zeit ist injizierbar** (`Clock`-Trait). Kein `SystemTime::now()` in Cache oder
   Policy — sonst sind alle TTL- und Zeitplan-Tests `sleep`-basiert und langsam.
5. **Selten wechselnder Zustand per `ArcSwap`** (Listen, Config, Policies). Kein globaler
   Mutex im Anfragepfad.

### B.4 Testing

Vollständig in `docs/TESTING.md`. Das Minimum, das für jeden Change gilt:

* Jeder Bugfix beginnt mit einem Test, der den Bug reproduziert.
* Jeder Parser (Blocklisten-Formate, Config, DNS-Nachrichten-Handling) bekommt Tests mit
  kaputten, abgeschnittenen und bösartigen Eingaben — nicht nur mit dem Happy Path.
* Integration-Tests fragen einen echten AlpenDNS-Prozess auf einem Loopback-Port ab.
  Tests kontaktieren **nie** echte Upstream-Resolver oder laden echte Blocklisten-URLs.

**Definition of Done** — diese vier müssen grün sein, "kompiliert" ist nicht fertig:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo deny check                        # braucht deny.toml, siehe B.0
```

Beim Entwickeln nicht jedes Mal die ganze Suite:

```bash
cargo test -p alpendns name_des_tests   # ein einzelner Test
cargo test -p alpendns --lib parser::   # alles unter einem Modulpfad
cargo test -p alpendns -- --nocapture   # Ausgabe des Tests sehen
```

Fuzzing braucht nightly (die einzige Ausnahme von B.2). In CI läuft jedes Target 120 s auf
dem eingecheckten Corpus, lange Läufe macht man lokal:

```bash
cargo +nightly fuzz run parse_message -- -max_total_time=120
```

Manueller Smoke-Test ab Phase 1 — Port 5353 statt 53, damit keine Rechte nötig sind:

```bash
dig @127.0.0.1 -p 5353 example.com
dig @127.0.0.1 -p 5353 +tcp example.com
```

Lokale Konfiguration zum Ausprobieren gehört nach `config/local.dev.toml` oder
`alpendns.toml` im Wurzelverzeichnis. Beides steht bereits in `.gitignore`, damit echte
Upstreams und Token nicht versehentlich im Repo landen.

### B.5 Sicherheit

* Der Dienst läuft als unprivilegierter User. Port 53 kommt über
  `AmbientCapabilities=CAP_NET_BIND_SERVICE` in der systemd-Unit, nicht über root.
* systemd-Hardening ist Teil der Definition of Done für Phase 9:
  `NoNewPrivileges`, `ProtectSystem=strict`, `PrivateTmp`, `MemoryDenyWriteExecute`,
  `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX`, `SystemCallFilter=@system-service`.
* Rate-Limiting pro Client-IP ist Pflicht, bevor der Server irgendwo öffentlich lauscht —
  ein offener Resolver ist ein Amplification-Reflektor.
* Antworten von Upstreams werden gegen die gestellte Frage validiert (Query-ID, QNAME
  case-insensitive, QTYPE, QCLASS), bevor sie in den Cache gehen.

### B.6 Web-UI

Zielbild: ruhig, dicht, ohne Deko. Orientierung an macOS-Systemeinstellungen und
Linear/Vercel-Dashboards, nicht an bunten Admin-Templates.

* Keine Gradient-Hero-Sections, keine Emoji als Icons, keine animierten Zahlen-Counter,
  keine Glassmorphism-Karten, keine bunten Badges für alles.
* Ein Akzentfarbton. Alles andere neutrale Graustufen. Hell und dunkel gleichwertig.
* System-Schriftart. Zahlen tabular (`font-variant-numeric: tabular-nums`), damit Werte
  in Tabellen nicht springen.
* Großzügige Weißräume, klare Hierarchie über Größe und Gewicht, nicht über Farbe.
* Die Startseite beantwortet drei Fragen ohne Klick: Läuft er? Was wurde gerade geblockt?
  Warum? Alles andere ist eine Ebene tiefer.
* Kein Client-seitiges Analytics, keine externen Fonts, keine CDN-Ressourcen. Die UI wird
  vom Server selbst ausgeliefert und funktioniert offline.

### B.7 Git

* Branch pro Änderung, `main` bleibt grün.
* Conventional Commits (`feat:`, `fix:`, `docs:`, `test:`, `refactor:`, `chore:`).
* Ein Commit = eine logische Änderung. Formatierungs-Rauschen kommt nicht in einen
  Feature-Commit.
* Commit-Message erklärt das *Warum*. Das *Was* steht im Diff.

### B.8 Wann du stoppen und fragen musst

Zusätzlich zu Teil A.1 — halte an, bevor du:

* eine der harten Regeln aus B.1 aufweichst;
* ein Dependency hinzufügst, das eigene Netzwerkverbindungen aufbaut;
* das Konfigurationsformat inkompatibel änderst;
* eine Heuristik aus `alpendns-detect` von `flag` auf `block` als Default stellst
  (Fehlalarme, die Internet kaputtmachen, sind das größte Risiko für die Akzeptanz);
* etwas implementierst, das in `docs/ROADMAP.md` einer späteren Phase zugeordnet ist.
