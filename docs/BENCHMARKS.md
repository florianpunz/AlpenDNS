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
