# Benchmarks

Zahlen, keine Behauptungen. Jede Phase erhebt sie neu (docs/TESTING.md §5); alte
Werte bleiben stehen, damit Veränderungen sichtbar sind.

Alle Messungen laufen gegen einen **Fake-Upstream im selben Prozess**. Tests und
Messungen kontaktieren nie echte Resolver. Das drückt die Zahlen in eine Richtung,
die man beim Lesen mitdenken muss: der Fake antwortet ohne Netzwerklatenz, deshalb
ist der Vorteil des Caches hier **unter**schätzt. Gegen einen echten Upstream über
DoT liegt zwischen Cache-Treffer und Upstream-Anfrage nicht ein Faktor 3, sondern
die Round-Trip-Zeit ins Internet.

## Messmaschine

| | |
|---|---|
| CPU | AMD Ryzen 5 5600X, 6 Kerne / 12 Threads |
| RAM | 31 GiB |
| Kernel | Linux 7.0.0-30-generic |
| Rust | 1.98.0, Profil `release` (`lto = "thin"`, `codegen-units = 1`) |

Die Zahlen sind maschinenabhängig. Aussagekräftig ist der Vergleich der Zeilen
untereinander, nicht der Absolutwert.

## Reproduzieren

```bash
cargo test --release --test load -- --ignored --nocapture --test-threads=1
```

`--test-threads=1` ist Pflicht — sonst laufen die Messungen gleichzeitig und
konkurrieren um dieselben Kerne, was den Durchsatz um rund ein Drittel drückt.

`dnsperf` (docs/TESTING.md §5) ist auf der Entwicklungsmaschine nicht installiert;
der Lastgenerator in `crates/alpendns/tests/load.rs` ersetzt es vorerst. Er hat
gegenüber `dnsperf` einen Vorteil und einen Nachteil: er läuft ohne
Systempaket-Installation und misst reproduzierbar denselben Aufbau — dafür ist er
kein etabliertes Werkzeug, dessen Zahlen mit anderen Projekten vergleichbar wären.

---

## Phase 2 — Cache · gemessen am 2026-08-29

16 Clients × 2000 Anfragen = 32 000 Anfragen je Durchlauf.

| Korpus | Anfragen/s | Upstream-Anfragen |
|---|---:|---:|
| jede Anfrage ein neuer Name | 103 317 | 32 000 |
| immer derselbe Name | 341 761 | **1** |

Faktor 3,3 beim Durchsatz. Die wichtigere Spalte ist die rechte: bei wiederholtem
Korpus erreicht genau **eine** Anfrage den Upstream. Das ist nicht nur eine
Leistungs-, sondern eine Privacy-Aussage — der Upstream sieht 32 000 Anfragen
weniger.

### Speicher bei Verdrängung

`max_entries = 10 000`, fünf Runden mit je 32 000 **neuen** Namen (160 000 insgesamt,
also das Sechzehnfache der Cache-Größe):

| | RSS |
|---|---:|
| nach 1 Runde | 13 420 KiB |
| nach 5 Runden | 14 184 KiB |
| Zuwachs | 764 KiB |

Ohne funktionierende LRU-Verdrängung müsste der Speicher hier linear mitwachsen.
Er tut es nicht.

---

## Phase 3 — nach 0x20, Cookies und ECS-Stripping · gemessen am 2026-08-29

Derselbe Aufbau. Interessant war, was die Privacy-Mechanismen auf dem Klartext-Weg
kosten: 0x20 würfelt für jede Anfrage die Schreibweise des Namens neu, Cookies
hängen eine EDNS-Option an.

| Korpus | Anfragen/s | vorher | Upstream-Anfragen |
|---|---:|---:|---:|
| jede Anfrage ein neuer Name | 110 106 | 103 317 | 32 000 |
| immer derselbe Name | 347 726 | 341 761 | **1** |

Faktor 3,2. Die Unterschiede liegen im Rauschen der Messung — die Privacy-Schicht
kostet nichts Messbares.

| | RSS |
|---|---:|
| nach 1 Runde | 13 860 KiB |
| nach 5 Runden | 14 680 KiB |
| Zuwachs | 820 KiB |

**Nicht gemessen:** der verschlüsselte Weg. Ein DoT- oder DoQ-Handshake gegen einen
Fake im selben Prozess misst vor allem die Krypto-Bibliothek, nicht AlpenDNS. Die
Zahl, die zählt, ist ohnehin die Latenz zum echten Upstream — im Smoke-Test lagen
Quad9 (DoT) bei 34 ms und Mullvad (DoH) bei 113 ms.

---

## Phase 4 — Blocklisten · gemessen am 2026-08-29

Zwei Millionen Einträge, wie es das Abnahmekriterium verlangt.

### Matcher

| | |
|---|---:|
| Liste parsen (hosts-Format) | 396 ms |
| Matcher bauen | 795 ms |
| Nachschlagen, Treffer | p50 230 ns · p99 620 ns |
| Nachschlagen, kein Treffer | p50 390 ns · p99 880 ns |

Der teurere Fall ist der Nicht-Treffer: er läuft alle Suffix-Ebenen durch, während
ein Treffer meist auf der ersten hängen bleibt. Genau deshalb wird er getrennt
gemessen — im Betrieb ist er der Normalfall.

### Speicher

| | RSS |
|---|---:|
| vorher | 3 588 KiB |
| mit Matcher | 326 460 KiB |
| nach dem Freigeben des Matchers | 191 292 KiB |
| **Matcher selbst** | **135 168 KiB — rund 69 Byte je Eintrag** |

Die naheliegende Zahl (322 MB Zuwachs) wäre falsch: darin stecken die Liste im
Rohtext und die geparsten Einträge, die es beim Aufbau zusätzlich gab, plus das,
was der Allokator nach dem Freigeben nicht ans System zurückgibt. Die dritte Zeile
trennt beides.

### Anfrage aus dem Cache bei geladenen zwei Millionen Einträgen

| | |
|---|---:|
| p50 | 18,8 µs |
| **p99** | **28,1 µs** |
| p999 | 37,6 µs |

Das Abnahmekriterium verlangt unter 1 ms. Konsequenz für die Datenstruktur:
[ADR-0008](adr/0008-hashmap-statt-bloom-und-trie.md).

### Durchsatz unverändert

| Korpus | Anfragen/s | Phase 3 | Upstream-Anfragen |
|---|---:|---:|---:|
| jede Anfrage ein neuer Name | 104 838 | 110 106 | 32 000 |
| immer derselbe Name | 323 740 | 347 726 | **1** |

Der Filter liegt vor dem Cache und wird damit bei *jeder* Anfrage befragt. Dass der
Durchsatz trotzdem im Rauschen der Vormessung bleibt, passt zu den 880 ns pro
Nachschlag.

---

## Phase 7 — Zählstruktur hinter der k-Schwelle · gemessen am 2026-08-30

Der Count-Min-Sketch gegen eine exakte Tabelle. Beide im selben Durchlauf
gemessen, damit die Speicherzahlen vergleichbar sind. `k = 5`, jeder Name wird
fünfmal gefragt — ein Name, der die Schwelle also **genau** erreicht.

### 50 000 verschiedene Namen · 250 000 Anfragen

| | Sketch | exakt |
|---|---:|---:|
| Speicher | 4 168 KiB | 2 180 KiB |
| je Eintrag | 109 ns | 42 ns |
| je Abfrage | 99 ns | 34 ns |
| Fehlerschranke | 2 | 0 |
| **Namen über der Schwelle** | **45** | **50 000** |

### 200 000 verschiedene Namen · 1 000 000 Anfragen

| | Sketch | exakt |
|---|---:|---:|
| Speicher | 4 096 KiB | 4 356 KiB |
| je Eintrag | 78 ns | 54 ns |
| je Abfrage | 74 ns | 40 ns |
| Fehlerschranke | 11 | 0 |
| **Namen über der Schwelle** | **0** | **200 000** |

Die letzte Zeile ist die Zahl, um die es geht. Der Sketch überschätzt, deshalb
prüft die Schwelle auf der unteren Schätzgrenze — und die ist
`Schätzung − Fehlerschranke`. Bei 250 000 Anfragen liegt die Schranke bei 2, ein
fünfmal gefragter Name kommt also mit 3 an und bleibt unter `k = 5`: von 50 000
Namen schaffen es 45. Bei einer Million Anfragen liegt die Schranke bei 11, und
die Statistik ist **leer**.

Das ist kein Fehler in der Umsetzung — die untere Schranke ist genau richtig, und
FEATURES.md P1 hatte diese Grenze vorhergesagt. Es ist die Struktur, die für
diese Größenordnung nicht taugt: sie ist für Datenströme gebaut, deren
Kardinalität nicht in den Speicher passt. Bei einem Haushalts-Resolver passt sie.

Speicher und Zeit sind das Nebenergebnis: exakt gezählt ist es bei 50 000 Namen
halb so viel Speicher und rund doppelt so schnell; bei 200 000 Namen — der
Obergrenze der Tabelle — kostet es etwa gleich viel.

Allein gemessen, ohne den Sketch davor im selben Prozess, liegt die exakte
Tabelle bei **1 384 KiB** (50 000 Namen) und **6 532 KiB** (200 000 Namen). Die
Differenz zur Tabelle oben ist Allokator-Verhalten, nicht Struktur: dort wurden
gerade 4 MiB Sketch freigegeben. Die 6,5 MB im Vollausbau sind die ehrliche
Obergrenze, und sie ist gedeckelt — mehr als `MAX_TRACKED` Namen nimmt die
Tabelle nicht auf.

Konsequenz: [ADR-0015](adr/0015-exakte-zaehlung-statt-sketch.md).
