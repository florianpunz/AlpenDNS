# ADR-0008: Die `HashMap` bleibt — Bloom-Filter und invertierter Trie kommen nicht

**Status:** angenommen · **Datum:** 2026-08-29

## Kontext

[ARCHITECTURE.md §3](../ARCHITECTURE.md) beschreibt als Zielmodell für die
Blocklisten: Labels umdrehen und internieren, einen Bloom-Filter davor, dahinter
eine exakte Struktur, die nur bei einem Bloom-Treffer befragt wird. Die Roadmap
schreibt für Phase 4 dagegen ausdrücklich vor: "v1 als HashSet mit Suffix-Lookup",
und der Fallstrick dazu lautet "Erst messen, dann optimieren. Bloom-Filter und
invertierter Trie kommen nur, wenn Schritt 11 zeigt, dass es nötig ist."

Schritt 11 ist gemessen. Mit zwei Millionen Einträgen
(`cargo test --release --test load -- --ignored`, Zahlen in
[BENCHMARKS.md](../BENCHMARKS.md)):

| | |
|---|---|
| Nachschlagen, Treffer | p50 230 ns, p99 620 ns |
| Nachschlagen, **kein** Treffer | p50 390 ns, p99 880 ns |
| Speicher des Matchers | 135 MB, rund 69 Byte je Eintrag |
| Aufbau aus geparsten Einträgen | 0,8 s |
| p99 einer Anfrage aus dem Cache, bei geladenen zwei Millionen Einträgen | 28 µs |

Der teure Fall — ein Name, der auf keiner Liste steht und deshalb alle
Suffix-Ebenen durchläuft — kostet unter einer Mikrosekunde. Das Abnahmekriterium
der Phase verlangt für eine Anfrage aus dem Cache eine p99 unter einer
Millisekunde; gemessen sind 28 Mikrosekunden, also das Fünfunddreißigfache
Luft.

## Entscheidung

Die `HashMap` mit Suffix-Nachschlag bleibt. Bloom-Filter, Label-Umkehrung und
invertierter Trie werden **nicht** gebaut.

ARCHITECTURE.md §3 wird nicht gelöscht, sondern verweist auf dieses ADR: das
Zielmodell bleibt als überlegter Plan dokumentiert, samt der Messung, die ihn
vorerst überflüssig macht.

## Konsequenzen

* Rund 135 MB für zwei Millionen Einträge. Auf einem Raspberry Pi mit 1 GB ist
  das viel, aber tragbar; bei vier Millionen Einträgen wäre es das nicht mehr.
  **Das ist die Zahl, die diese Entscheidung umdreht** — nicht die Latenz.
* Der Bloom-Filter hätte vor allem Speicher gekostet, nicht gespart: er kommt
  *zusätzlich* zur exakten Struktur und lohnt erst, wenn diese so groß wird,
  dass sie nicht mehr in den Cache passt und jeder Zugriff ein Speicher-Miss ist.
  Bei p99 von 880 ns ist dieser Punkt sichtbar nicht erreicht.
* Der Code ist rund 150 Zeilen statt einiger hundert, und das Nachschlagen ist in
  einem Absatz erklärbar. Das ist bei einer Datenstruktur im heißen Pfad kein
  Nebenaspekt.
* Die Messung ist reproduzierbar im Repo (`tests/load.rs`). Wer die Entscheidung
  umdrehen will, hat die Ausgangszahlen.

## Alternativen

* **Jetzt schon Bloom-Filter und Trie bauen.** Mehr Code, mehr Speicher, und
  keine Zahl, die den Aufwand rechtfertigt. Genau der Fall, vor dem der
  Fallstrick in der Roadmap warnt.
* **Domains internieren, um Speicher zu sparen.** Naheliegender als der
  Bloom-Filter, wenn der Speicher zum Problem wird: gemeinsame Suffixe wie
  `.example.com` liegen derzeit hundertfach im Speicher. Das wäre der erste
  Schritt, wenn 135 MB zu viel werden — nicht der Bloom-Filter.
