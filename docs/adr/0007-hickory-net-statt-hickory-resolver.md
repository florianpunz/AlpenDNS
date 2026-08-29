# ADR-0007: Transporte aus `hickory-net`, Pool und Auswahl bleiben unsere

**Status:** angenommen · **Datum:** 2026-08-29 · **Ergänzt:** [ADR-0002](0002-hickory-proto-statt-eigenem-parser.md)

## Kontext

[ADR-0002](0002-hickory-proto-statt-eigenem-parser.md) hält fest:
"`hickory-resolver` wird für die Upstream-Transporte (DoT/DoH/DoQ) verwendet, wo es
passt." Beim Umsetzen von Phase 3 stellten sich zwei Dinge heraus.

**Erstens haben sich die Crates umsortiert.** In `hickory 0.26` ist `hickory-proto`
auf das reine Wire-Format eingedampft; die Transporte liegen in einem neuen Crate
`hickory-net`. `hickory-resolver` baut darauf auf und fügt hinzu: einen eigenen
Cache, eine eigene Wiederholungslogik, einen eigenen Name-Server-Pool mit eigener
Auswahl.

**Zweitens ist genau das der Teil, den wir selbst haben.** Der Cache ist Phase 2 und
laut ARCHITECTURE.md §4 bewusst unserer — er speichert die *ungefilterte* Antwort,
weil die Filterung davor liegt. Pool, Auswahlstrategien und Ausfallerkennung sind
Phase 3, Schritte 3 bis 5, und `split_by_zone` gibt es anderswo nicht
(FEATURES.md P2). `hickory-resolver` einzusetzen hieße, diese Teile doppelt zu haben
und die eigenen gegen fremde durchzureichen.

## Entscheidung

Wir benutzen **`hickory-net` für die Transporte** — DoT, DoH und DoQ, also
TLS-Handshake, HTTP/2-Rahmen, QUIC-Streams und die Zuordnung von Antworten zu
Anfragen auf einer gemultiplexten Verbindung. Das ist dieselbe Klasse riskanten
Codes, um die es in ADR-0002 ging, und dieselbe Begründung gilt.

**Nicht** benutzt wird `hickory-resolver`. Unser eigen bleiben:

* wann eine Verbindung aufgebaut, wiederverwendet und verworfen wird
  (`upstream::transport`),
* welcher Upstream eine Anfrage bekommt (`upstream::strategy`),
* wann ein Upstream als ausgefallen gilt (`upstream::pool`),
* der Cache (`cache`, Phase 2),
* was vor dem Senden mit der Nachricht passiert (`privacy`).

Als Krypto-Backend werden durchgehend die `-ring`-Varianten gewählt, nicht
`aws-lc-rs`: letzteres führt die OpenSSL-Lizenz mit, die nicht in der Allowlist von
`deny.toml` steht.

## Konsequenzen

* Der Verbindungs-Lebenszyklus ist unser Code, rund 190 Zeilen. Er muss richtig
  sein: eine Verbindung, die nach einem Fehler nicht verworfen wird, beantwortet
  keine Anfrage mehr.
* Ein API-Bruch in `hickory-net` trifft uns direkt. Dafür sind wir nicht an die
  Release-Politik von `hickory-resolver` gebunden, das deutlich mehr Oberfläche hat.
* Die Aussage aus ADR-0002 bleibt sinngemäß gültig, der Crate-Name darin ist
  überholt. Dieses ADR ersetzt ADR-0002 nicht — die Entscheidung "kein eigener
  Parser" steht unverändert.
* `hickory-net` schreibt die Query-ID auf einer gemultiplexten Verbindung selbst um.
  Sie zurückzusetzen ist unsere Aufgabe, und sie zu vergessen ist ein Fehler, den
  keiner unserer Fake-Tests gesehen hat — siehe die Notiz zu Phase 3 in ROADMAP.md.

## Alternativen

* **`hickory-resolver` mit seinem Pool.** Spart unseren Pool, bringt aber einen
  zweiten Cache mit — genau das, was ARCHITECTURE.md §4 ausschließt — und macht
  `split_by_zone` zu einem Fremdkörper in fremder Auswahl-Logik.
* **Transporte selbst schreiben.** TLS-Handshake, HTTP/2 und QUIC von Hand: dieselbe
  Antwort wie in ADR-0002, nur mit größerem Risiko.
