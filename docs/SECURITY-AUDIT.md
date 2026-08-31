# Security-Audit

Ein manueller Audit der gesamten Codebase (`crates/alpendns/src/`), Schwerpunkt auf
Rust-spezifischen Risiken in Netzwerk- und Parsing-Code: DNS-Paket-Parsing,
Upstream-Handling, Config-Parsing. Durchgeführt am 2026-08-31 auf `main`
(Stand ad3d776), read-only — es wurden **keine Code-Änderungen** vorgenommen.

## Ergebnis

**Keine Critical-Lücke.** Die harten Regeln aus CLAUDE.md B.1 werden durch
Workspace-Lints tatsächlich durchgesetzt, nicht nur behauptet. Gefunden wurden zwei
mittlere DoS-Vektoren (fehlender Timeout bzw. fehlende Grenze an zwei Stellen, an denen
externer Fluss **nicht** über das DNS-Wire-Format läuft) und drei Low-/Hardening-Punkte.

Das Versprechen „jedes Byte vom Netzwerk ist feindlich" ist strukturell eingelöst. Die
lohnendste Stelle für künftige Audits ist daher nicht das DNS-Parsing, sondern die beiden
HTTP/TCP-Bodies, die der Prozess selbst liest.

## Was geprüft und sauber befunden wurde

Diese Punkte wurden gezielt gesucht und **nicht** gefunden — festgehalten, damit ein
späterer Audit nicht von vorn anfängt:

- **`unsafe`:** kommt nirgends vor (`unsafe_code = "forbid"`).
- **`panic!` / `unreachable!` / `todo!`:** nirgends im Produktionscode.
- **`unwrap()` / `expect()` auf externem Input:** der einzige Nicht-Test-`unwrap()` steht
  in [ratelimit.rs:210](../crates/alpendns/src/ratelimit.rs#L210), ist mit
  `#[expect(clippy::unwrap_used)]` markiert und beweisbar sicher (`SHARDS` ist eine
  Konstante > 0). Alle übrigen Treffer liegen in `#[cfg(test)]`-Blöcken. Die
  Workspace-Lints (`unwrap_used = "deny"`, `panic = "deny"`, `indexing_slicing = "deny"`)
  und `-D warnings` in CI machen diesen Zustand erzwungen, nicht zufällig.
- **Handgeschriebenes DNS-Parsing:** keines. Wire-Format läuft über `hickory-proto`
  ([ADR-0002](adr/0002-hickory-proto-statt-eigenem-parser.md)). Die eigene Schicht validiert
  ID, Message-Type, QNAME (case-insensitive), QTYPE und QCLASS gegen die gestellte Frage
  ([dns.rs](../crates/alpendns/src/dns.rs)); `format_error` und `encode_for_udp` kommen ohne
  Slice-Indexing aus.
- **Integer-Overflow in Längen-/Offset-Berechnung:** durchgängig `saturating_*`,
  `checked_*` oder `try_from(...).unwrap_or(MAX)`. Kein stiller Wrap gefunden.
- **Config-Parsing fail-closed:** `serde(deny_unknown_fields)`, Plaintext-Verbot im Pool,
  `tls_name`-Pflicht, IP-Literal-Pflicht für Upstreams (kein DNS-abhängiges Auflösen).
  Ein Tippfehler führt zum Startfehler, nicht zum ungefilterten Betrieb (B.1 Regel 5).
- **XSS in der Web-UI:** ausgeschlossen. Domainnamen fließen in die UI, aber das Skript
  benutzt ausschließlich `textContent`/`setAttribute`, kein `innerHTML`/`outerHTML`/
  `insertAdjacentHTML` — als Test fixiert in
  [ui.rs:551-557](../crates/alpendns/src/api/ui.rs#L551-L557). Zusätzlich Token-Pflicht mit
  Vergleich in konstanter Zeit ([api/mod.rs:180](../crates/alpendns/src/api/mod.rs#L180)).
- **Metrik-Label-Injection:** ausgeschlossen. Alle Label-Werte stammen aus Config oder
  geschlossenen Enums; Domain-/Client-Werte als Label sind per Test verboten
  ([metrics.rs:711-788](../crates/alpendns/src/metrics.rs#L711-L788)).
- **Cache-/Policy-Races:** der Cache ist gesharded (`Mutex<LruCache>` je Shard), selten
  wechselnder Zustand liegt hinter `ArcSwap`. Der `Inflight`-Dedup gibt den Key und das
  `broadcast`-Signal in korrekter Reihenfolge frei.
- **Rate-Limit läuft vor dem Parsen:** [server/mod.rs](../crates/alpendns/src/server/mod.rs)
  drosselt auf Basis der Peer-IP, bevor ein Byte der Anfrage geparst wird.

## Befunde

### Medium — TCP-Slowloris: fehlender Timeout beim Lesen des Bodys

- **Datei:Zeile:** [tcp.rs:86](../crates/alpendns/src/server/tcp.rs#L86)
- **Beschreibung:** Der Lesevorgang des 2-Byte-Längenpräfix hat `IDLE_TIMEOUT` (10 s,
  [tcp.rs:74](../crates/alpendns/src/server/tcp.rs#L74)). Das anschließende
  `stream.read_exact(&mut packet)` — bis zu 65 535 Byte — hat **keinen** Timeout. Ein
  Client schickt zwei Bytes (`0xFF 0xFF`) und hält die Verbindung dann still: Der Task
  hängt unbegrenzt und hält eine `vec![0; 65535]`-Allokation.
- **Risiko:** klassischer Slowloris. Es gibt weder ein Limit pro Verbindung noch ein
  Gesamtlimit für gleichzeitige TCP-Verbindungen (`tracker.spawn` pro `accept`, ohne
  Semaphore). Eine Handvoll stiller Verbindungen bindet Tasks und Speicher — bei einem
  öffentlich lauschenden Resolver genügt ein einzelner Client. Für den reinen
  LAN-Resolver braucht es einen kompromittierten Host im eigenen Netz, daher Medium statt
  High.
- **Muster:** Der Präfix-Timeout erweckt den Eindruck, die Verbindung sei komplett
  abgesichert — gerade darum fällt der fehlende Timeout am Body beim ersten Blick nicht auf.

### Medium — Blocklisten-Download puffert unbegrenzt trotz 64-MB-Limit

- **Datei:Zeile:** [source.rs:183](../crates/alpendns/src/filter/source.rs#L183) (Prüfung
  erst [source.rs:187](../crates/alpendns/src/filter/source.rs#L187))
- **Beschreibung:** Das Größenlimit `MAX_LIST_BYTES` (64 MB) wird **vor** dem Lesen nur
  über `content_length()` geprüft ([source.rs:171](../crates/alpendns/src/filter/source.rs#L171)).
  Eine Antwort ohne `Content-Length` (Chunked-Encoding) umgeht diese Prüfung.
  `response.text().await` puffert dann den **kompletten** Body, und die echte Grenze
  greift erst danach.
- **Risiko:** Das Limit ist als Unbounded-Allocation-Schutz gedacht und versagt genau bei
  der Antwortform, die es nicht ankündigt. Ausnutzbar nur, wenn die Blocklisten-Quelle
  kompromittiert ist oder bösartig antwortet (die URLs sind Admin-konfiguriert, also
  vertraut) — daher Medium. Die Korrektur wäre ein streaming-basiertes `read_limited`
  für den Body statt `text()`.

### Low — TOCTOU in `read_limited`

- **Datei:Zeile:** [source.rs:294-302](../crates/alpendns/src/filter/source.rs#L294-L302)
- **Beschreibung:** `metadata.len()`-Prüfung und `read_to_string` sind zwei getrennte
  Schritte. Zwischen beiden kann die Datei wachsen oder ausgetauscht werden (Symlink-Tausch
  oder Schreiben durch einen lokalen Nutzer mit Rechten am Cache-Verzeichnis); der
  zwischengespeicherte Fallback liest dann mehr als 64 MB.
- **Risiko:** setzt lokalen Schreibzugriff auf das Cache-Verzeichnis des Dienst-Users
  voraus; nicht über das Netz erreichbar. Defense-in-Depth, daher Low.

### Low — API-Token-Datei kurzzeitig mit Default-Rechten

- **Datei:Zeile:** [main.rs:800-807](../crates/alpendns/src/main.rs#L800-L807)
- **Beschreibung:** `std::fs::write(path, …)` legt die Datei mit Umask-Rechten an (typisch
  `0644`), erst danach wird `0o600` gesetzt. In diesem Fenster ist das frisch erzeugte
  Passwort welterlesbar. Außerdem: Existiert die Datei bereits mit lockeren Rechten, liest
  `read_or_create_token` sie nur und strafft die Rechte nicht nach
  ([main.rs:782-787](../crates/alpendns/src/main.rs#L782-L787)).
- **Risiko:** lokaler Angreifer, winziges Zeitfenster, aber es ist das API-Credential.
  Sauber wäre `OpenOptions` mit `mode(0o600)` (Unix) direkt beim Anlegen, sodass es nie
  ein offenes Fenster gibt.

### Kontext (kein Codeproblem) — Amplifikation über gespoofte Quell-IP

- **Beobachtung:** Der Rate-Limiter schlüsselt über die Quell-IP. Bei UDP ist die
  Quell-IP fälschbar; die Drosselung verhindert daher **nicht** eine
  Amplifikations-Reflexion gegen ein gespooftes Opfer, sie deckelt nur das
  pro-Client-Volumen. Die Antwortgröße ist durch `udp_payload_size` begrenzt.
- **Einordnung:** inhärente Eigenschaft eines Resolvers, kein Bug. Das Projekt weiß es:
  `alpendns check` warnt bei öffentlichen Listenern explizit, B.5 macht Drosselung zur
  Pflicht. Nur zur Vollständigkeit festgehalten, kein priorisierter Befund.

## Einordnung

Die zwei nennenswerten Lücken liegen nicht in der gefürchteten Klasse „Panic/unsafe auf
Input", sondern in **fehlenden Timeouts/Bounds an den zwei Stellen, an denen der Prozess
selbst einen Body liest, der nicht das DNS-Wire-Format ist**: der TCP-Body in
[tcp.rs](../crates/alpendns/src/server/tcp.rs) und der HTTP-Body der Blocklisten in
[source.rs](../crates/alpendns/src/filter/source.rs). Beides ist mit begrenztem Aufwand zu
schließen, beides ist kein Critical.
