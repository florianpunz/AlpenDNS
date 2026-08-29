# Roadmap

Der Plan ist in Phasen geschnitten. Jede Phase hat ein **Ziel**, eine **Schrittliste im
Verify-Format** (siehe CLAUDE.md, Teil A.4) und ein **Abnahmekriterium**. Eine Phase gilt
als fertig, wenn das Abnahmekriterium erfüllt ist — nicht, wenn der Code kompiliert.

**Aktuelle Phase: 3.**

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

**Fallstricke:** 0x20 vertragen nicht alle Upstreams — pro Pool abschaltbar machen und im
Fehlerfall automatisch deaktivieren, statt Anfragen scheitern zu lassen.

---

## Phase 4 — Blocklisten · ~3–4 Abende · **hier ist der Server benutzbar**

**Ziel:** Listen importieren, matchen, blocken, aktuell halten. Ab hier ersetzt AlpenDNS
ein Pi-hole im eigenen Netz.

```
1. Parser für Format hosts → verify: Unit-Tests inkl. Kommentaren, CRLF, IPv6-Zeilen, Müllzeilen
2. Parser für domains und wildcard → verify: Unit-Tests, führende Punkte und *. werden korrekt normalisiert
3. Parser für Adblock-Syntax (Teilmenge: ||domain^) → verify: Test dokumentiert explizit, welche Syntax unterstützt wird und welche ignoriert
4. Parser für RPZ-Zonendateien → verify: Unit-Test mit RPZ-Beispiel
5. Matcher, v1 als HashSet mit Suffix-Lookup, liefert RuleRef → verify: Property-Test Wildcard-Semantik; notexample.com matcht nie wegen example.com
6. Allowlist mit Vorrang vor Blocklisten → verify: Integrationstest, Domain auf beiden Listen wird durchgelassen
7. Block-Antwort synthetisieren (nxdomain / zero_ip / refused) → verify: je ein Test, RCODE und Antwortinhalt korrekt
8. Listen-Download mit ETag/If-Modified-Since, Cache auf Platte → verify: zweiter Abruf gegen lokalen HTTP-Fake liefert 304, keine Neuverarbeitung
9. Atomarer Tausch per ArcSwap, kein Ausfall beim Update → verify: Lasttest während eines Updates, keine Fehlerantwort, keine Latenzspitze
10. Erststart ohne erreichbare Liste bricht ab, späterer Ausfall nicht → verify: zwei Tests für beide Fälle
11. Benchmark Matcher, RSS bei 2 Mio. Einträgen messen → verify: Zahlen stehen in docs/BENCHMARKS.md
```

**Abnahme:** 2 Millionen Einträge geladen, p99-Latenz für einen Cache-Hit unter 1 ms,
RSS dokumentiert. Werbung ist im eigenen Netz weg. Der Server läuft eine Woche als
einziger Resolver im LAN, ohne dass jemand meckert.

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

---

## Phase 8 — Heuristik ohne Cloud · ~4–5 Abende

**Ziel:** Erkennung von Mustern, die keine Liste kennt. Alles lokal, alles erklärbar,
alles per Default nur `flag`.

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
