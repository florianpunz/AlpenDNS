# Roadmap

Der Plan ist in Phasen geschnitten. Jede Phase hat ein **Ziel**, eine **Schrittliste im
Verify-Format** (siehe CLAUDE.md, Teil A.4) und ein **Abnahmekriterium**. Eine Phase gilt
als fertig, wenn das Abnahmekriterium erfüllt ist — nicht, wenn der Code kompiliert.

**Aktuelle Phase: 8.**

Die Reihenfolge ist so gewählt, dass **nach Phase 4 ein Server steht, den du produktiv
im eigenen Netz benutzen kannst**. Alles danach macht ihn besser, nicht erst benutzbar.
Wenn die Motivation zwischendurch nachlässt: Phase 4 ist ein guter Ort zum Stehenbleiben.

Zeitangaben sind grobe Schätzungen für Abendarbeit mit Agentenunterstützung. Sie sind
Orientierung, keine Zusage.

---

## Phase 0 — Gerüst · ~1 Abend

**Ziel:** Ein Repository, in dem `cargo test` läuft und CI grün ist.

```
1. Workspace anlegen (Cargo.toml, crates/alpendns) → verify: cargo build läuft durch
2. rustfmt/clippy-Konfiguration aus dem Workspace ziehen → verify: cargo clippy --all-targets -- -D warnings ist grün
3. Git-Repo initialisieren, erster Commit → verify: git log zeigt einen Commit, git status ist sauber
4. CI-Workflow prüfen → verify: Push nach GitHub, Actions-Run ist grün
5. cargo-deny einrichten (deny.toml) → verify: cargo deny check läuft ohne Fehler
```

**Abnahme:** Alle vier Kommandos aus der Definition of Done laufen lokal und in CI durch.

---

## Phase 1 — Ein Server, der antwortet · ~2–3 Abende

**Ziel:** UDP und TCP auf Port 53, Anfrage wird an *einen* fest konfigurierten Upstream
weitergereicht, Antwort geht zurück. Kein Cache, kein Filter, keine Policy.

Das ist der "Hello World" eines DNS-Servers. Ab hier kannst du `dig @127.0.0.1 -p 5353
example.com` gegen deinen eigenen Code laufen lassen, und das ist der Punkt, ab dem das
Projekt real wird.

```
1. tokio-Runtime + UDP-Socket auf konfigurierbarem Port → verify: Integrationstest schickt Bytes, Server empfängt sie
2. Nachricht mit hickory-proto parsen, Query extrahieren → verify: Unit-Test mit aufgezeichnetem Query-Paket
3. Upstream über UDP fragen, Antwort zurückschicken → verify: dig @127.0.0.1 -p 5353 example.com liefert eine A-Record-Antwort
4. Antwort gegen die Frage validieren (ID, QNAME case-insensitive, QTYPE, QCLASS) → verify: Test mit manipulierter Antwort → verworfen
5. TCP-Listener inkl. 2-Byte-Längenpräfix → verify: dig +tcp liefert dasselbe Ergebnis
6. Truncation: Antwort > udp_payload_size setzt TC-Flag → verify: Test mit großer TXT-Antwort, dig ohne +tcp zeigt TC, mit +tcp die volle Antwort
7. Config aus TOML laden, deny_unknown_fields → verify: Test mit Tippfehler im Schlüssel → Start bricht mit klarer Meldung ab
8. Graceful Shutdown auf SIGTERM/SIGINT → verify: laufende Anfrage wird noch beantwortet, dann Exit 0
```

**Abnahme:** `dig` über UDP und TCP liefert korrekte Antworten. Ein Fuzz-Target auf dem
Anfragepfad läuft 5 Minuten ohne Crash.

**Erledigt am 2026-08-29.** Mit einer Abweichung: der Fuzz-Lauf ist crashfrei in der
Auslieferungs-Konfiguration (`cargo +nightly fuzz run -O`). Mit aktiven Overflow-Checks
findet er einen Panic in `hickory-proto 0.26.1` selbst, den wir nicht reparieren können.
Bewertung, Auflagen und der Weg zurück: [ADR-0005](adr/0005-tsig-panic-in-hickory-proto.md).
Weiterhin offen aus Phase 0: der CI-Lauf, dafür fehlt ein GitHub-Remote.

**Fallstricke:** Port 53 braucht Rechte — in der Entwicklung auf 5353 gehen. UDP hat keine
Verbindung: die Antwort muss an genau die Quelladresse zurück, von der die Anfrage kam,
und über denselben Socket.

---

## Phase 2 — Cache · ~2 Abende

**Ziel:** Wiederholte Anfragen kommen aus dem Speicher. Der Upstream sieht sie nicht mehr.

```
1. Cache-Key (Name lowercase, QType, QClass), Wert = Antwort + Ablaufzeitpunkt → verify: Unit-Test Insert/Lookup/Ablauf
2. TTL auf [min_ttl, max_ttl] klemmen, negative Antworten separat (RFC 2308) → verify: Property-Test, TTL nie größer als beim Einfügen
3. Cache in die Pipeline hängen → verify: Integrationstest, zweiter Query erzeugt keine Upstream-Anfrage
4. Größenbegrenzung mit LRU-Verdrängung → verify: Test füllt über max_entries, RSS bleibt stabil
5. Query-Deduplizierung für gleichzeitige identische Anfragen → verify: 100 parallele Queries → Fake-Upstream zählt genau 1
6. serve-stale (RFC 8767) → verify: Upstream abschalten, abgelaufener Eintrag wird trotzdem ausgeliefert
7. Prefetch bei 85 % der TTL → verify: Test mit Zeitraffer, Eintrag wird erneuert, ohne dass ein Client wartet
```

**Abnahme:** Cache-Trefferquote ist als Metrik sichtbar. `dnsperf` zeigt bei wiederholtem
Korpus eine deutlich höhere Rate als in Phase 1.

**Abgenommen am 2026-08-29.**

* **Trefferquote sichtbar:** `Cache::stats()` zählt Treffer, stale-Treffer und Misses.
  Der Server schreibt die Bilanz alle fünf Minuten ins Log — aber nur, wenn sich seit
  der letzten Zeile etwas getan hat, damit ein Server im Leerlauf still bleibt — und
  zusätzlich beim Herunterfahren. Nur Summen, keine Namen. Ein *abfragbarer Endpunkt*
  dafür ist Phase 6, Schritt 2, und wurde bewusst nicht vorgezogen (CLAUDE.md B.8).
* **Durchsatz:** `dnsperf` ist auf der Entwicklungsmaschine nicht installiert. An seine
  Stelle tritt ein Lastgenerator im Repo (`crates/alpendns/tests/load.rs`, läuft nur
  mit `--ignored`), damit die Zahlen reproduzierbar sind statt einmalig. Ergebnisse und
  Messaufbau stehen in [BENCHMARKS.md](BENCHMARKS.md): 32 000 Anfragen erreichen den
  Upstream bei lauter neuen Namen, genau **eine** bei wiederholtem Korpus; Durchsatz
  Faktor 3,3.
* **Nachtrag zu Schritt 4:** "RSS bleibt stabil" war zunächst nicht geprüft, nur die
  Zahl der Einträge. Jetzt gemessen: 160 000 neue Namen bei `max_entries = 10 000`
  lassen den Speicher um 764 KiB wachsen, nicht linear mit.

**Fallstricke:** Zeit muss injizierbar sein (`Clock`-Trait), sonst sind alle TTL-Tests
`sleep`-basiert und langsam. Nicht `SystemTime::now()` direkt im Cache aufrufen.

---

## Phase 3 — Verschlüsselte Upstreams · ~2–3 Abende

**Ziel:** Kein Klartext-DNS mehr nach außen. Mehrere Upstreams mit Auswahlstrategie.

```
1. DoT-Transport (hickory-resolver, rustls) → verify: tcpdump zeigt Port 853 TLS, kein Klartext-DNS
2. DoH-Transport (HTTP/2) → verify: Integrationstest gegen lokalen DoH-Fake
3. Upstream-Pool mit mehreren Resolvern, Strategie fastest → verify: Test mit zwei Fakes unterschiedlicher Latenz, der schnellere gewinnt
4. Passives Health-Tracking, toter Upstream wird übersprungen → verify: Fake antwortet nicht mehr, Anfragen gehen an den zweiten, Metrik zeigt den Ausfall
5. Strategie split_by_zone mit Seed beim Start → verify: Unit-Test, gleiche Domain immer derselbe Upstream; anderer Seed → andere Verteilung
6. forward_zone für interne Zonen (Klartext erlaubt) → verify: Query auf home.arpa geht an den LAN-Server, alles andere verschlüsselt
7. Privacy-Grundlagen: ECS strippen, EDNS-Padding, DNS Cookies, 0x20 → verify: je ein Unit-Test; 0x20-Test prüft Round-Trip und case-insensitiven Vergleich
8. DoQ-Transport → verify: Integrationstest gegen lokalen DoQ-Fake
```

**Abnahme:** `tcpdump port 53` auf dem Uplink zeigt keinen einzigen DNS-Klartext-Paket
mehr. Fällt ein Upstream aus, merkt es kein Client.

**Abgenommen am 2026-08-29.** Damit ist die letzte offene Abweichung von B.1 Regel 7
geschlossen: `udp://` in einem `upstream_pool` ist jetzt ein Startfehler, nicht mehr
der Normalfall.

* **Kein Klartext nach außen:** `tcpdump` braucht root und stand nicht zur Verfügung.
  Stattdessen über `ss` geprüft, welche Verbindungen der laufende Prozess hat:
  `9.9.9.9:853` (DoT) und `194.242.2.4:443` (DoH), keine einzige auf Port 53. Dazu
  ein Integrationstest, der einen Klartext-Resolver als Falle aufstellt und prüft,
  dass er nie kontaktiert wird.
* **Ausfall bleibt unbemerkt:** Unit-Tests im Pool decken das ab — toter Upstream
  wird nach drei Fehlversuchen übersprungen, erholt sich nach der Sperre wieder, und
  wenn alle als tot gelten, wird trotzdem gefragt (B.1 Regel 6).
* **Fallstrick, der teuer war:** `hickory-net` schreibt die Query-ID auf einer
  gemultiplexten Verbindung um und gibt sie in der Antwort nicht zurück. Die Fakes
  prüften nur Antwortinhalt und RCODE und waren deshalb grün, während `dig` gegen
  den echten Prozess "ID mismatch" meldete und in den Timeout lief. Der Test
  `every_transport_returns_the_clients_query_id_and_question` hält das jetzt fest.
  Lehre: ein Fake, der nur prüft, was man erwartet, prüft zu wenig.
* **Offen gelassen:** `split_by_zone` bestimmt die registrierbare Domain über die
  letzten beiden Labels. Für `example.co.uk` ist das zu grob — die Folge ist eine
  ungleiche Verteilung, keine Privacy-Lücke. Die saubere Lösung braucht die Public
  Suffix List und ist als Phase 7, Schritt 1 eingeplant, wo die Verteilung ohnehin
  gemessen wird.
* **Nachträglich zurückgebaut (Phase 7):** Schritt 3 baute `fastest`, dazu kamen
  `round_robin` und `fanout`. Alle drei sind wieder weg — `split_by_zone` ist die
  einzige Strategie, und es wird immer genau ein Upstream gefragt.
  [ADR-0011](adr/0011-eine-upstream-strategie.md),
  [ADR-0012](adr/0012-fanout-entfaellt.md).

**Fallstricke:** 0x20 vertragen nicht alle Upstreams — pro Pool abschaltbar machen und im
Fehlerfall automatisch deaktivieren, statt Anfragen scheitern zu lassen.

---

## Phase 4 — Blocklisten · ~3–4 Abende · **hier ist der Server benutzbar**

**Ziel:** Listen importieren, matchen, blocken, aktuell halten. Ab hier ersetzt AlpenDNS
ein Pi-hole im eigenen Netz.

```
1. Parser für Format hosts → verify: Unit-Tests inkl. Kommentaren, CRLF, IPv6-Zeilen, Müllzeilen
2. Parser für domains und wildcard → verify: Unit-Tests, führende Punkte und *. werden korrekt normalisiert
3. ~~Parser für Adblock-Syntax~~ → in Phase 7 wieder entfernt, [ADR-0014](adr/0014-adblock-und-rpz-parser-entfallen.md)
4. ~~Parser für RPZ-Zonendateien~~ → in Phase 7 wieder entfernt, [ADR-0014](adr/0014-adblock-und-rpz-parser-entfallen.md)
5. Matcher, v1 als HashSet mit Suffix-Lookup, liefert RuleRef → verify: Property-Test Wildcard-Semantik; notexample.com matcht nie wegen example.com
6. Allowlist mit Vorrang vor Blocklisten → verify: Integrationstest, Domain auf beiden Listen wird durchgelassen
7. Block-Antwort synthetisieren (nxdomain / zero_ip / sinkhole) → verify: je ein Test, RCODE und Antwortinhalt korrekt
8. Listen-Download mit ETag/If-Modified-Since, Cache auf Platte → verify: zweiter Abruf gegen lokalen HTTP-Fake liefert 304, keine Neuverarbeitung
9. Atomarer Tausch per ArcSwap, kein Ausfall beim Update → verify: Lasttest während eines Updates, keine Fehlerantwort, keine Latenzspitze
10. Erststart ohne erreichbare Liste bricht ab, späterer Ausfall nicht → verify: zwei Tests für beide Fälle
11. Benchmark Matcher, RSS bei 2 Mio. Einträgen messen → verify: Zahlen stehen in docs/BENCHMARKS.md
```

**Abnahme:** 2 Millionen Einträge geladen, p99-Latenz für einen Cache-Hit unter 1 ms,
RSS dokumentiert.

**Abgenommen am 2026-08-29.**

* **Zahlen erfüllt und deutlich:** zwei Millionen Einträge geladen, p99 für eine
  Anfrage aus dem Cache **28 µs** statt der geforderten 1 ms. Der Matcher braucht
  135 MB, rund 69 Byte je Eintrag; Nachschlagen p99 880 ns im teuren Fall (kein
  Treffer, alle Suffix-Ebenen). Alles in [BENCHMARKS.md](BENCHMARKS.md).
* **Fallstrick beantwortet:** die Messung rechtfertigt weder Bloom-Filter noch
  invertierten Trie. Entscheidung und Umkehrbedingung in
  [ADR-0008](adr/0008-hashmap-statt-bloom-und-trie.md) — der Speicherbedarf ist die
  Zahl, die sie kippen würde, nicht die Latenz.
* Gegen die echte StevenBlack-Liste geprüft: 79 747 Einträge, `doubleclick.net`
  liefert NXDOMAIN, ein zweiter Start meldet `origin=NotModified` — der ETag greift.

**Verschoben:** der Dauerbetrieb im echten Netz ("eine Woche als einziger Resolver
im LAN") stand ursprünglich hier. Er gehört zu Phase 9: vorher gibt es keine
systemd-Unit, und ohne sie läuft der Server nicht auf Port 53 und nicht über einen
Neustart hinweg. Einen Resolver im LAN aus einer Shell heraus zu betreiben wäre
kein Praxistest, sondern eine andere Baustelle.

**Fallstricke:** Erst messen, dann optimieren. Bloom-Filter und invertierter Trie
(ARCHITECTURE.md §3) kommen nur, wenn Schritt 11 zeigt, dass es nötig ist.

---

## Phase 5 — Clients und Policies · ~3 Abende

**Ziel:** Nicht mehr eine Regel für alle. Unterschiedliche Geräte, unterschiedliche Regeln,
Zeitfenster.

```
1. Client-Identifikation über IP/Subnetz → verify: Integrationstest, zwei Quell-IPs bekommen unterschiedliche Verdikte
2. Policy-Modell (Listen, Allowlists, Aktionen) + Auswertungsreihenfolge → verify: Tabellen-getriebener Test über alle Kombinationen
3. Decision-Trace vollständig befüllen → verify: Test prüft die Schrittfolge für einen Blocklisten-Treffer
4. Zeitpläne mit injizierbarer Uhr → verify: derselbe Query zu zwei simulierten Uhrzeiten, zwei Ergebnisse
5. Temporäre Freigaben mit TTL über die API → verify: Freigabe für 60 s, Query erlaubt; nach Ablauf wieder geblockt
6. Regex-Regeln pro Policy, mit Längen- und Laufzeitbegrenzung → verify: Fuzz-Test, keine katastrophale Backtracking-Laufzeit
7. Policy-Simulation als CLI: alpendns policy test <domain> --client <name> → verify: Ausgabe zeigt Verdikt plus vollständige Begründungskette
```

**Abnahme:** Ein Gerät im Netz hat eine strengere Policy als der Rest, inklusive
Zeitfenster, und `alpendns policy test` erklärt jede Entscheidung ohne Blick ins Log.

**Abgenommen am 2026-08-29.** Gegen die echte StevenBlack-Liste:

```
$ alpendns -c … policy test doubleclick.net
Verdikt:  GEBLOCKT
  1. kein Client-Eintrag passt, es gilt 'default'
  2. Policy 'default'
  3. Blockliste 'stevenblack-unified' Zeile 7092: 'doubleclick.net'
  4. Antwort selbst erzeugt, Modus Nxdomain

$ alpendns -c … policy test www.spiele.example --client kids-tablet
Verdikt:  GEBLOCKT
  3. Regex-Regel von Policy 'kids': /(?:^|\.)spiele\./
```

Dieselbe Domain ohne `--client` läuft durch — die Regel gehört nur der einen Policy.

* **Strukturell:** `resolve` bekommt jetzt einen `Ctx` mit Client-Adresse und Trace.
  Er lag zunächst hinter einem Mutex, weil bei `fanout > 1` mehrere
  Upstream-Aufgaben gleichzeitig eintrugen; seit `fanout` entfallen ist, wird er
  wieder exklusiv durchgereicht ([ADR-0009](adr/0009-decision-trace-mit-mutex.md)
  samt Nachtrag, [ADR-0012](adr/0012-fanout-entfaellt.md)).
* **Schritt 5 nachgeholt** mit der API aus Phase 6. Gegen den laufenden Server:
  `doubleclick.net` liefert NXDOMAIN, nach `POST /api/allow` NOERROR, nach Ablauf
  wieder NXDOMAIN.
* **Zum Backtracking (Schritt 6):** die Regex-Engine arbeitet mit endlichen
  Automaten. `(a+)+$` gegen 10 000 Zeichen läuft in unter einer Millisekunde statt
  exponentiell. Das ist eine Eigenschaft der Engine, keine Vorsichtsmaßnahme.

---

## Phase 6 — Sichtbarkeit: API, Metriken, Web-UI · ~4–5 Abende

**Ziel:** Man sieht, was der Server tut, ohne sich einzuloggen.

```
1. HTTP-API (axum) mit Token-Auth: Status, Statistik, Listen, Policies, temp. Freigaben → verify: Integrationstests pro Endpunkt, 401 ohne Token
2. Prometheus-Endpunkt → verify: promtool check metrics ist zufrieden; Zähler für Queries, Blocks, Cache-Trefferquote, Upstream-RTT, Fehler
3. Logging-Schicht mit den vier Modi aus ADR-0004 → verify: Test pro Modus; in none und aggregate taucht kein Query-Name in der Ausgabe auf
4. Ringpuffer für den ring-Modus → verify: Test, Einträge älter als ring_seconds sind weg, RAM wächst nicht
5. Live-Query-Stream über Server-Sent Events → verify: Testclient empfängt Ereignisse; bei Log-Modus none kommen nur Zähler
6. Web-UI Grundgerüst, vom Server selbst ausgeliefert, keine externen Ressourcen → verify: Seite lädt mit getrennter Netzwerkverbindung; Browser-Netzwerktab zeigt keine Fremd-Requests
7. UI-Ansicht "Warum wurde das geblockt?" auf Basis des Decision-Trace → verify: manuell — eine geblockte Domain zeigt Liste, Regel, Policy, Zeitpunkt
8. UI-Startseite beantwortet Läuft er? / Was wurde geblockt? / Warum? ohne Klick → verify: Screenshot-Review gegen die Vorgaben in CLAUDE.md B.6
```

**Abnahme:** Ein Außenstehender öffnet die UI und versteht in 30 Sekunden, was der Server
gerade tut. Die Seite funktioniert ohne Internetzugang.

**Umgesetzt am 2026-08-29; die Abnahme der UI steht aus.**

Gegen den laufenden Server geprüft:

```
/api/status ohne Token          → HTTP 401
/api/status mit Token           → {"logging_mode":"ring","queries":2,"blocked":1, …}
/api/recent                     → Namen samt vollständiger Begründungskette
POST /api/allow                 → geblockte Domain wird durchgelassen
GET  /                          → HTTP 200, die UI
/metrics ohne Token             → alpendns_queries_total 3, keine Namen
```

Am 2026-08-30 dazugekommen und ebenfalls gegen den laufenden Server geprüft:

```
/api/top                        → {"threshold":5,"domains":[{"name":"ads.example.com","count":6,…}],
                                   "below_threshold_queries":1,"below_threshold_names":1}
/api/history                    → {"bucket_seconds":300,"upstreams":["quad9","mullvad"],"buckets":[…]}
                                   — nur Zähler, kein Feld für einen Namen
/api/explain?domain=…           → dieselbe Kette wie `alpendns policy test`
```

* **Der wichtigste Test** ist `no_query_name_leaves_the_process_in_the_quiet_modes`:
  er fährt eine Anfrage durch und greppt alles, was der Prozess ausgeben kann —
  Zähler, Top-Domains, Ringpuffer, Datei — nach dem Query-Namen. In `none` und
  `aggregate` darf er nirgends stehen. Das ist der automatisierte Nachweis für
  das zentrale Versprechen des Projekts und läuft ab jetzt bei jedem `cargo test`.
* **k-Anonymität:** zunächst über einen Count-Min-Sketch, dessen Schwelle auf der
  *unteren* Schätzgrenze prüfte. In Phase 7 gemessen und ersetzt: die Fehlerschranke
  wuchs so schnell mit dem Verkehr, dass bei einer Million Anfragen gar keine Domain
  mehr in der Statistik erschien. Jetzt exakt gezählt, unter einem gesalzenen Hash
  statt unter dem Namen ([ADR-0015](adr/0015-exakte-zaehlung-statt-sketch.md)).
* **Entscheidungen zur Oberfläche** (zwei Listener, Token in der SSE-URL, UI ohne
  Build-Schritt, Prometheus von Hand):
  [ADR-0010](adr/0010-api-ui-und-metriken.md).

**Offen: Schritt 8.** "Screenshot-Review gegen die Vorgaben in CLAUDE.md B.6" ist
ein Blick eines Menschen auf eine gerenderte Seite. Automatisiert geprüft ist, was
sich prüfen lässt: keine Verweise nach außen, die drei Fragen als Überschriften
vorhanden, die semantischen Farben in beiden Schemata und nur in Selektoren
mit Bedeutung, zentrierter Container, 8er-Abstände, vier Kennzahlenkarten,
Sparkline als Inline-SVG ohne Bibliothek, erklärte Leerflächen, tabellarische
Ziffern, kein `innerHTML`. Ob die Seite *ruhig* aussieht, kann kein Test sagen.

Die Gestaltung wurde am 2026-08-30 überarbeitet: zentrierter Container (max.
1400 px), Kennzahlen als Karten mit Sparkline der letzten 60 Sekunden, Upstreams
als Zeilen mit Statuspunkt und Latenz, Badges und Latenzschwellen im Protokoll,
echte Leerzustände. Der eine Akzentton ist dabei durch die vier semantischen
Farben aus B.6 ersetzt worden, dazu kommt `--brand` allein für den Schriftzug;
Datenquellen und Endpunkte blieben unverändert.

**Fallstricke:** Das ist die Phase, in der ein Agent am ehesten in generisches
Dashboard-Design abrutscht. CLAUDE.md B.6 ist dafür da; bei jeder UI-Aufgabe explizit
darauf verweisen.

---

## Phase 7 — Privacy-Ausbau · ~3 Abende

**Ziel:** Die Mechanismen, die AlpenDNS von "Pi-hole mit DoH" unterscheiden.
Details zu jedem Punkt in [FEATURES.md](FEATURES.md).

```
1. split_by_zone härten: Seed-Rotation, Verteilung messen → verify: Test über 10k Domains, Abweichung pro Upstream unter 5 %
2. Privacy-Budget: pro Upstream zählen, welcher Anteil der Anfragen dorthin ging → verify: API liefert die Verteilung, UI zeigt sie
3. Eigene DNSSEC-Validierung statt dem AD-Bit des Upstreams zu glauben → verify: Testvektoren mit gültiger, ungültiger und fehlender Signatur
4. Oblivious DoH als Client (RFC 9230) → verify: Integrationstest gegen lokalen ODoH-Proxy-Fake
5. Aggregierte Statistik mit k-Anonymitätsschwelle → verify: Domain mit weniger als k Treffern erscheint in keiner API-Antwort
6. Sicherstellen, dass kein Codepfad Query-Namen unter Modus none/aggregate ausgibt → verify: Test fährt eine Session und greppt die gesamte Ausgabe nach dem Testnamen
```

**Abnahme:** Punkt 6 ist der wichtigste — er ist der automatisierte Beweis für das
zentrale Versprechen des Projekts und muss dauerhaft in CI laufen.

**Stand 2026-08-30 — Punkte 2 und 5 sind in der Oberfläche angekommen.** Die UI ist
entlang ADR-0004 ausgebaut worden, Leitsatz "alles zeigen, nichts merken":

* Der Privacy-Streifen im Kopf nennt dauerhaft Modus, k-Schwelle und ob etwas auf
  der Platte landet. Die letzte Angabe kommt aus dem laufenden Prozess
  (`QueryLog::writes_to_disk`), nicht aus einer Annahme — im Modus `full` steht
  dort "schreibt auf Platte".
* Zwei Kurven über 24 Stunden (Anfragen gegen geblockt, Cache-Trefferquote) aus
  `crate::history`: 288 Eimer à fünf Minuten im RAM, gespeist aus dem
  Zähler-Snapshot alle 30 Sekunden. Keine neue Persistenz; ein Neustart setzt die
  Reihe zurück. Die Struktur nimmt nur `Sample` entgegen und hat damit kein Feld,
  in das je ein Name passen würde.
* Punkt 2 sichtbar: die Aufteilung über die Zeit als gestapelte Fläche je Upstream,
  dazu der Transport-Mix und die Privacy-Zähler (ECS entfernt, Padding, 0x20,
  Cookies). Gezählt wird die *Wirkung* — `strip_ecs` zählt nur, wenn wirklich eine
  Option entfernt wurde, sonst zeigte die Zahl bloß, dass ein Schalter an ist.
* Punkt 5 vollständig: `/api/top` liefert Namen ausschließlich über der Schwelle
  und daneben die Summe dessen, was darunter bleibt (`below_threshold_queries`,
  `below_threshold_names`). Ohne diese Summe sähe ein Server mit viel seltenem
  Verkehr aus wie einer ohne Verkehr.
* Neues Panel "Block-Gründe" über `logging::BlockReason`, abgeleitet aus dem Trace.
  Kategorien heute: Blockliste, Regex-Regel, Zeitplan, ohne Zuordnung. **Die
  Heuristiken DGA, Tunneling und Rebinding fehlen darin, weil es sie noch nicht
  gibt** — sie sind Phase 8, Punkte 2 bis 4. Die Aufzählung nimmt sie dann ohne
  Umbau von Zählern, API oder UI auf.
* Ein Klick auf eine Protokollzeile fragt `/api/explain` und bekommt dieselbe
  Auswertung wie `alpendns policy test`: beide rufen `policy::explain` auf, damit
  Kommandozeile und UI nicht verschiedene Antworten auf dieselbe Frage geben.
* Alle Zähler im deutschen Zahlenformat (`de-AT`).

Nachgezogen am selben Tag: der Live-Strom war so gebaut, dass er je Anfrage eine
Nachricht schickte. Bei einem Lasttest legte das die Oberfläche lahm. Jetzt
deckelt der Server auf 25 Nachrichten je Sekunde und trägt die ausgelassenen als
Zahl nach (`skipped`), damit die Sparkline nicht lügt; der Browser sammelt
Ereignisse und zeichnet einmal je Bild. Zahlen in
[BENCHMARKS.md](BENCHMARKS.md#phase-7--live-strom-unter-last--gemessen-am-2026-08-30).

Dazu zwei neue Tests, die die Grenze festhalten: `every_label_key_comes_from_a_closed_set`
nagelt die Prometheus-Label-Schlüssel auf eine Positivliste fest (bisher waren nur
drei Schreibweisen verboten, eine vierte wäre durchgerutscht), und
`no_metric_label_carries_a_domain_or_client_name` fährt in allen vier Log-Modi
echten Verkehr durch und greppt die gerenderte Metrik nach Query- und Client-Namen.

**Umgesetzt am 2026-08-30 — alle sechs Punkte.** Die vier Kommandos der Definition
of Done laufen durch. Was dazugekommen ist:

**Schritt 1 — `split_by_zone` gehärtet** ([ADR-0018](adr/0018-public-suffix-list-und-seed-rotation.md)).
Die registrierbare Domain kommt jetzt aus der Public Suffix List (`psl`,
einkompiliert, kein Netzabruf, keine Laufzeitdatei) statt aus der Näherung
"letzte zwei Labels" — die lieferte für `shop.example.co.uk` das wirkungslose
`co.uk`. Der Seed wird per Default alle 24 Stunden neu gezogen
(`[[upstream_pool]] seed_rotation`, `"0s"` schaltet ab); vorher galt er bis zum
Neustart, und der Satz aus FEATURES.md P2 "über die Zeit lernt keiner ein
stabiles Bild" stimmte nur für den, der auch neu startet. Das Abnahmekriterium
ist ein Test und keine einmalige Messung: `ten_thousand_domains_stay_within_five_percent_per_upstream`
fährt 10 000 Domains — ein Fünftel unter mehrteiligen Suffixen — über vier
Poolgrößen und acht Seeds, größte Abweichung je Upstream **unter 5 %**. In der
Metrik steht `alpendns_zone_seed_rotations_total`, damit "die Zuordnung rotiert"
im Betrieb eine Zahl ist und keine Behauptung.

**Schritt 3 — eigene DNSSEC-Validierung** ([ADR-0016](adr/0016-dnssec-validierung-im-forwarder.md)).
Per Default an; eine Antwort, deren Zone sich als signiert ausweist und deren
Kette nicht schließt, wird verworfen (SERVFAIL, RFC 4035). Die drei Testvektoren
in `tests/dnssec.rs` laufen mit **echten** Signaturen durch dieselbe Prüfung wie
der Betrieb: gültig → `Secure` und durchgelassen, verdrehtes Bit → `Bogus` und
verworfen, fehlende Signatur in signierter Zone → `Bogus` und verworfen. Damit
ist der offene Punkt aus THREAT-MODEL.md A3 geschlossen.

Drei Entscheidungen, die im ADR begründet sind und von außen willkürlich
aussehen: zusammengefasst wird pessimistisch (ein fauler Record macht die
Antwort faul, Authority-Abschnitt eingeschlossen); `Bogus` ist terminal und
zählt *nicht* als Ausfall des Upstreams (sonst markiert eine kaputte Zone nach
drei Anfragen den ganzen Pool als tot); die Signaturen gehen nur an Clients
weiter, die mit DO danach gefragt haben.

**Schritt 4 — Oblivious DoH** ([ADR-0017](adr/0017-oblivious-doh.md)), Default
aus. `tests/odoh.rs` baut die vollständige Kette auf Loopback — ein Proxy, der
weiterreicht ohne entschlüsseln zu können, und ein Ziel mit echtem
Schlüsselpaar. Der wichtigste Test durchsucht die Bytes, die durch den Proxy
gingen, nach den Labels des Query-Namens; sie stehen nicht drin. Dazu geprüft:
der Schlüssel des Ziels wird genau einmal geholt, eine vom Proxy veränderte
Antwort wird verworfen, ein toter Proxy ergibt einen Fehler statt eines Hängers.
DNSSEC gilt auch über ODoH — der Transport ist als `DnsHandle` verpackt, damit
sich der validierende Griff davorhängen kann; sonst täte `dnssec = true` mit
eingeschaltetem ODoH still nichts.

**Abweichungen und Preise, die notiert gehören:**

* **`time` ist jetzt Produktionsabhängigkeit.** `hickory-proto/dnssec-ring`
  zieht es herein, und damit stimmt die alte Begründung der Advisory-Ausnahme
  RUSTSEC-2026-0009 ("steckt gar nicht im Binary") nicht mehr. Die Ausnahme
  bleibt mit engerer Begründung — der verwundbare Pfad (RFC-2822-Datumsparsen)
  wird nicht betreten; hickory benutzt aus `time` nur `OffsetDateTime` für
  RRSIG-Zeitstempel, die als Zahlen vom Draht kommen. Vollständig in `deny.toml`.
  Sie fällt weg, sobald die MSRV auf 1.88 steigt; dagegen steht die
  Debian-Paketierung aus Phase 9 (Debian 13 liefert `rustc 1.85`).
* **Der ODoH-Schlüsselabruf geht direkt zum Ziel**, nicht über den Proxy — das
  Ziel sieht dabei einmal je Prozessstart die Adresse, aber keine Frage. Über
  den Proxy ginge es nicht: der nimmt nur ODoH-Nachrichten entgegen.
* **Drei Fehler kamen erst beim Lauf gegen echte Upstreams heraus** und sind
  behoben: ein verworfenes `dnssec-failed.org` wurde nicht gezählt und dem
  Upstream als Fehlversuch angerechnet (hickory liefert diesen Fall als Fehler,
  nicht als gestempelte Nachricht); `dig` bekam die Signaturkette, ohne danach
  gefragt zu haben (AD in der Anfrage ist nach RFC 6840 §5.7 kein Wunsch nach
  Records, nur DO ist es); und ein `dig +dnssec` bekam null Signaturen, weil ein
  `dig` ohne davor da war — gestrippt wurde unter dem Cache, der eine Antwort
  für alle hält. Ausführlich in [ADR-0016](adr/0016-dnssec-validierung-im-forwarder.md).
  Gegen den laufenden Server nachgeprüft:

  ```
  dnssec-failed.org            → SERVFAIL, bogus=1, quad9 ohne Fehlversuch
  cloudflare.com               → NOERROR, ad-Flag, keine RRSIG in der Antwort
  cloudflare.com +dnssec       → dieselbe Cache-Zeile, RRSIG dabei
  gnu.org                      → NOERROR, kein ad-Flag (unsignierte Zone)
  ```
* **Die DNSSEC-Vektoren pinnen den Schlüssel der Testzone als Trust Anchor,**
  statt eine Kette bis zur echten Root zu bauen. An der Rechnerei ist dabei
  nichts abgekürzt; dass der validierende Griff im Transport auch wirklich
  vorgeschaltet ist, hält ein eigener Test in `encrypted.rs` fest.
* **Die Transport-Fakes in `encrypted.rs` laufen jetzt ohne Validierung.** Sie
  beantworten jede Frage mit demselben A-Record, auch eine nach DNSKEY — für
  einen validierenden Griff ist das keine unsignierte Zone, sondern eine kaputte
  Kette. Was dort geprüft wird, sind die Transporte.
* **Weiterhin offen aus Phase 0:** der CI-Lauf, dafür fehlt ein GitHub-Remote.
  Das Abnahmekriterium dieser Phase verlangt, dass Punkt 6 *dauerhaft in CI*
  läuft; lokal läuft er bei jedem `cargo test`.
* **Weiterhin offen aus Phase 6, Schritt 8:** der Blick eines Menschen auf die
  gerenderte UI. Dazugekommen sind dort ein Eintrag im Privacy-Streifen
  ("DNSSEC selbst geprüft" bzw. "dem Upstream geglaubt") und zwei Zähler in der
  Privacy-Kachel, darunter die verworfenen Antworten — die einzige Zahl der
  Reihe, die im Betrieb eine Frage aufwirft.

---

## Phase 8 — Heuristik ohne Cloud · ~4–5 Abende

**Ziel:** Erkennung von Mustern, die keine Liste kennt. Alles lokal, alles erklärbar,
alles per Default nur `flag`.

**Vorarbeit ist da:** `logging::BlockReason` und das UI-Panel "Block-Gründe"
existieren seit Phase 7. Ein neuer Detektor braucht dort je eine Variante plus
ihren Schritt im Trace — Zähler, Metrik-Label und Diagramm ziehen automatisch mit.

```
1. Framework: Detektor-Trait, Score 0.0–1.0 + Begründung, Aktion aus der Config → verify: Dummy-Detektor läuft durch die Pipeline und landet im Trace
2. Rebinding-Schutz (private IPs für öffentliche Namen) → verify: Test, Ausnahmeliste funktioniert
3. Tunneling-Erkennung (Entropie, Labellänge, Rate, TXT/NULL-Anteil pro Zone) → verify: iodine-/dnscat-Beispielkorpus wird erkannt, Top-100k-Korpus erzeugt unter 0.1 % Falsch-Positive
4. DGA-Erkennung über Zeichen-N-Gramme, Modell im Binary → verify: Trefferquote pro DGA-Familie dokumentiert, Falsch-Positiv-Rate auf Top-100k unter 0.1 %
5. Typosquat-Erkennung gegen protect-Liste (Damerau-Levenshtein + Unicode-Confusables) → verify: konstruierte Varianten von zwei Schutz-Domains werden erkannt, die Originale nie
6. NRD-Awareness aus lokaler Datei → verify: Testdatei mit Datumsangaben, Domain unter max_age wird geflaggt
7. UI: geflaggte Anfragen mit Score und Begründung, ein Klick zum Blocken oder Freigeben → verify: manuell
```

**Abnahme:** Eine Woche Betrieb im echten Netz mit allen Detektoren auf `flag`. Die
Falsch-Positiv-Liste wird durchgesehen; erst danach darf ein Detektor auf `block`.

**Hinweis:** Das ist die inhaltliche Brücke zu deinem AlpenShield-Projekt — die dort
gebaute Pipeline (CT-Logs, Zonendaten, Klassifikator) kann die NRD- und
Reputationsdateien liefern, die AlpenDNS hier lokal einliest. Die Schnittstelle dazwischen
ist bewusst eine simple Datei, kein API-Aufruf: der Resolver darf nicht davon abhängen,
dass ein zweiter Dienst läuft.

---

## Phase 9 — Betrieb und Paketierung · ~3 Abende

**Ziel:** Auf einer frischen Debian-VM in fünf Minuten installiert und gehärtet.

```
1. systemd-Unit mit CAP_NET_BIND_SERVICE, ohne root → verify: ps zeigt unprivilegierten User, Port 53 lauscht
2. Hardening-Direktiven → verify: systemd-analyze security alpendns zeigt einen Score unter 3.0
3. alpendns check als ExecStartPre → verify: kaputte Config verhindert den Start, alte Instanz läuft weiter
4. .deb-Paket mit cargo-deb, Config unter /etc/alpendns → verify: Installation auf frischer Debian-VM, Dienst startet
5. Rate-Limiting pro Client-IP → verify: Lasttest mit einer IP über dem Limit wird gedrosselt, andere IPs unbeeinflusst
6. Erst-Installation setzt sichere Defaults (Listener nur auf privaten Adressen) → verify: nach der Installation ist der Server von außen nicht erreichbar
7. Lasttest und Zahlen dokumentieren → verify: docs/BENCHMARKS.md enthält Queries/s, p99, RSS
8. Betriebsdoku: Installation, Upgrade, Backup, Fehlersuche → verify: jemand anderes installiert danach ohne Rückfragen
```

**Abnahme:** Frische VM, `apt install ./alpendns.deb`, funktionierender gehärteter
Resolver ohne manuelles Nacharbeiten.

**Dazu der Praxistest, der aus Phase 4 hierher verschoben wurde:** der Server läuft
eine Woche als einziger Resolver im LAN, ohne dass jemand meckert. Erst hier ist er
dafür überhaupt eingerichtet — auf Port 53, als Dienst, über Neustarts hinweg. Was
dabei auffällt, gehört als Fehlalarm-Liste oder Konfigurationsänderung
dokumentiert; "lief bei mir" ist kein Abnahmekriterium.

---

## Phase 10 — Optional, jederzeit verwerfbar

Keine Reihenfolge, keine Verpflichtung. Nach Lust und Bedarf:

* **Rekursion** über `hickory-recursor` hinter dem bestehenden `ResolveBackend`-Trait
  ([ADR-0003](adr/0003-forwarder-first.md)), inkl. QNAME-Minimisation.
* **DDR/DNR** (RFC 9462/9463): LAN-Clients finden deinen verschlüsselten Endpunkt
  automatisch und wechseln von Klartext auf DoH/DoT ([FEATURES.md](FEATURES.md), P5).
* **Blocklist-Diff-Review** vor dem Anwenden eines Listen-Updates (O3).
* **Zwei Instanzen** mit abgeglichenem Policy-Stand für Ausfallsicherheit.
* **Eigener DNS-Parser** als reines Lernprojekt, per Differential Testing gegen
  `hickory-proto` geprüft — bewusst außerhalb des Produktionspfads.

---

## Wie du mit dem Agenten arbeitest

* **Eine Phase = eine Arbeitssitzung**, nicht mehr. "Bau mir Phase 4 bis 8" führt
  zuverlässig zu 3000 Zeilen, die niemand mehr prüft.
* **Jeder Schritt zuerst als Test.** Die Verify-Spalten oben sind bereits die
  Testbeschreibungen; gib sie wörtlich weiter.
* **Nach jedem Schritt die vier Kommandos** aus der Definition of Done.
* **Bei Unklarheit stoppen lassen.** CLAUDE.md B.8 listet auf, wann das gilt — verweise
  im Zweifel ausdrücklich darauf.
* **Nach jeder Phase:** Roadmap aktualisieren (aktuelle Phase), offene Punkte und
  Abweichungen notieren, ADR schreiben, wenn eine Entscheidung gefallen ist.
