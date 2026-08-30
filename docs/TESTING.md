# Teststrategie

Ein DNS-Server ist besonders gut testbar: Eingabe und Ausgabe sind Bytes über einen Socket,
und es gibt einen normativen Standard dafür, was richtig ist. Das nutzen wir aus.

## Die fünf Ebenen

### 1. Unit-Tests

Neben dem Code, `#[cfg(test)]`. Zuständig für: Blocklisten-Parser, Config-Deserialisierung,
Cache-TTL-Logik, Upstream-Auswahlstrategien, Heuristik-Scoring, Trace-Aufbau.

Regel: Jeder Parser wird nicht nur mit gültigen, sondern mit **kaputten** Eingaben getestet.
Für Blocklisten heißt das mindestens: leere Datei, Datei ohne Zeilenumbruch am Ende, Zeilen
mit CRLF, Kommentare, Inline-Kommentare, Unicode/IDN, Labels über 63 Zeichen, Namen über
255 Zeichen, führende/abschließende Punkte, doppelte Einträge, eine 300-MB-Zeile.

### 2. Property-Tests (`proptest`)

Für Invarianten, die für *alle* Eingaben gelten müssen:

* Blocklisten-Matcher: wenn `example.com` als Wildcard gelistet ist, matcht jede
  Subdomain und `notexample.com` matcht nie.
* Cache: eine gecachte Antwort hat nie eine höhere TTL als beim Einfügen.
* 0x20: `unrandomize(randomize(name)) == name`, und der Vergleich ist
  case-insensitive.
* Name-Normalisierung ist idempotent.

### 3. Fuzzing (`cargo fuzz`)

Alles, was Bytes vom Netzwerk oder aus fremden Dateien liest, bekommt ein Fuzz-Target:

* Parsen einer DNS-Nachricht (auch wenn `hickory-proto` das macht — wir fuzzen unsere
  Verwendung davon, inklusive der Stellen, an denen wir Felder herausziehen).
* Blocklisten-Zeilen, alle Formate.
* Config-TOML.
* DoH-Pfad-Parsing (Token-Extraktion).

Erfolgskriterium: kein Panic, keine Endlosschleife, kein unbegrenztes Wachstum. Ein
gefundener Crash wird als Corpus-Datei eingecheckt und wird zum Regressionstest.

In CI läuft jedes Target kurz (120 s) auf dem bestehenden Corpus. Lange Läufe macht man
lokal.

### 4. Integrations-Tests

Ein echter AlpenDNS-Prozess auf einem Loopback-Port, echte Anfragen, echte Antworten.
Der Upstream ist **immer** ein Fake — ein in-process DNS-Server mit fest verdrahteten
Antworten. Tests gehen nie ins Internet: nicht zu Resolvern, nicht zu Blocklisten-URLs.
Ein Test, der Netzwerk braucht, ist ein Test, der irgendwann rot ist, ohne dass sich
Code geändert hat.

Testfälle, die es geben muss:

* Query wird beantwortet (A, AAAA, CNAME-Kette, MX, TXT, NS, PTR).
* Query auf Blocklisten-Domain liefert die konfigurierte Block-Antwort.
* Allowlist schlägt Blocklist.
* Zweiter identischer Query kommt aus dem Cache (Upstream sieht genau eine Anfrage).
* 100 gleichzeitige identische Queries → genau eine Upstream-Anfrage (Dedup).
* Upstream tot → nächster Resolver; alle tot → `serve_stale`, dann SERVFAIL.
* Ein Client über dem Limit wird gedrosselt, ein anderer nicht
  (`tests/ratelimit.rs`). Die zweite Adresse ist `127.0.0.2` — Linux gibt das
  ganze `127.0.0.0/8` an Loopback, es muss nichts konfiguriert werden. Ein
  Unit-Test allein reichte hier nicht: geprüft werden muss, dass die
  Absenderadresse des Pakets ankommt und nicht die des Listeners.
* Antwort mit falscher Query-ID/falschem QNAME wird verworfen und nicht gecacht.
* Antwort über 1232 Byte über UDP setzt TC; derselbe Query über TCP liefert die volle
  Antwort.
* Rebinding: Upstream antwortet mit `192.168.1.1` auf einen öffentlichen Namen → blockiert.
* SIGHUP mit kaputter Config → alte Config bleibt aktiv, Server antwortet weiter.
* Policy-Zeitfenster: derselbe Query zu zwei simulierten Uhrzeiten, zwei Ergebnisse.
  (Zeit muss injizierbar sein — kein direkter `SystemTime::now()`-Aufruf in der Policy.)

### 5. Konformität und Last

* **Konformität:** eine Sammlung realer Anfragen als pcap, gegen AlpenDNS und gegen
  `unbound` abgespielt; die Antworten müssen in den relevanten Feldern übereinstimmen
  (RCODE, Answer-Section, Flags). Unterschiede sind entweder Bugs oder bewusste
  Abweichungen, die dokumentiert werden.
* **Last:** `dnsperf` oder `flamethrower` gegen einen Fake-Upstream. Gemessen werden
  Anfragen/s, p50/p99/p999-Latenz, RSS. Die Zahlen kommen in `docs/BENCHMARKS.md` und
  werden pro Phase neu erhoben. Ohne Baseline ist "das ist jetzt schneller" eine Behauptung.
  Solange keines der beiden Werkzeuge installiert ist, übernimmt der Lastgenerator in
  `crates/alpendns/tests/load.rs` diese Rolle:

  ```bash
  cargo test --release --test load -- --ignored --nocapture --test-threads=1
  ```

  Er läuft wegen `#[ignore]` nicht in CI und nicht bei `cargo test` — eine Lastmessung
  in der Definition of Done würde jeden Durchlauf verlangsamen und wäre auf fremder
  Hardware ohnehin nicht vergleichbar.

### 6. Messläufe gegen Korpora (seit Phase 8)

Die Heuristiken haben Abnahmekriterien mit Zahlen: "unter 0,1 % Falsch-Positive
auf einem Top-100k-Korpus". Solche Zahlen brauchen echte Daten, und die liegen
**nicht im Repo** — sie sind fremd, ein bis zwei Megabyte groß und für den Bau
nicht nötig. `corpus/` steht in `.gitignore`.

```bash
mkdir -p corpus
curl -sSL https://downloads.majestic.com/majestic_million.csv | tail -n +2 \
  | cut -d, -f3 > /tmp/majestic.txt
head -100000            /tmp/majestic.txt > corpus/top-100k.txt    # Messung
sed -n '100001,600000p' /tmp/majestic.txt > corpus/train-500k.txt  # Training

cargo test --release --test detect_corpus -- --ignored measure --nocapture
```

Majestic Million, CC-BY 3.0. Über `ALPENDNS_CORPUS_DIR` lässt sich ein anderes
Verzeichnis angeben.

**Getrennt wird nicht aus Ordnungsliebe.** Beim ersten Anlauf lief Training und
Messung auf derselben Liste, und die Falsch-Positiv-Rate war um den Faktor 500
zu gut — 0,001 % gegen 0,54 % auf ungesehenen Namen. Ein Modell erkennt die
Namen wieder, aus denen es gebaut wurde. Wer eine dieser Zahlen neu erhebt, muss
die Trennung mit erheben.

Das Modell selbst wird mit demselben Werkzeug erzeugt; wie, steht in
`crates/alpendns/src/detect/dga/model.bin.md`.

## Der Replay-Harness

Das Werkzeug, das sich am meisten auszahlt und das man früh baut:

```
alpendns-replay --corpus queries.jsonl --config test.toml --expect expected.jsonl
```

Eine Datei mit Anfragen (Name, Typ, Client), eine mit erwarteten Verdikten. Damit wird
jede Änderung an Listen, Policies oder Heuristiken zu einem messbaren Diff statt zu einem
Bauchgefühl. Der Harness ist auch die Grundlage für den Blocklist-Diff-Review aus
[FEATURES.md](FEATURES.md) (O3).

Für die Heuristiken braucht es zwei Korpora:

* **Benign:** die Top-100k-Domains einer öffentlichen Popularitätsliste. Erwartung:
  Falsch-Positiv-Rate unter 0.1 %. Das ist das eigentliche Qualitätsmaß für DGA- und
  Typosquat-Erkennung, nicht die Trefferquote.
* **Malign:** bekannte DGA-Familien und Tunneling-Beispiele. Erwartung: Trefferquote pro
  Familie dokumentiert, nicht als eine Zahl gemittelt.

## Definition of Done

Ein Change ist fertig, wenn:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo deny check
```

durchlaufen, **und** der Change entweder einen neuen Test mitbringt oder eine Zeile
Begründung, warum er keinen braucht. "Kompiliert" ist nicht fertig.

Die Lastmessung gehört ausdrücklich **nicht** dazu (siehe §5).
