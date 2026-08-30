# ADR-0015: Exakte Zählung statt Count-Min-Sketch

**Status:** angenommen · **Datum:** 2026-08-30 · **Nachtrag zu:** [ADR-0004](0004-logging-default-aggregiert.md)

## Kontext

Im Modus `aggregate` zählt AlpenDNS, wie oft eine Domain gefragt wurde, und gibt
sie erst ab `aggregate_k` Treffern aus. Umgesetzt war das mit einem
Count-Min-Sketch: 2^18 Zähler × 4 Zeilen × 4 Byte, fest 4 MiB.

Ein Sketch **überschätzt** — der abgelesene Wert ist nie kleiner als der wahre,
weil fremde Namen auf dieselben Zähler fallen. Die Schwelle prüfte deshalb
korrekt auf der unteren Schätzgrenze, `Schätzung − Fehlerschranke`, mit
Fehlerschranke `e · Anfragen / Breite`. FEATURES.md P1 hatte diese Grenze
ausdrücklich benannt.

Der Haken daran: **die Fehlerschranke wächst mit der Zahl der Anfragen, nicht mit
der Zahl der Namen.** Ein Haushalts-Resolver hat wenige Namen und viele Anfragen —
genau die Kombination, die dem Sketch nicht liegt. Gemessen
(BENCHMARKS.md, Phase 7, `cargo test --release --test load -- --ignored`):

| `k = 5`, jeder Name fünfmal gefragt | Sketch | exakt |
|---|---:|---:|
| 250 000 Anfragen, 50 000 Namen · Fehlerschranke | 2 | 0 |
| … davon über der Schwelle | **45** | **50 000** |
| 1 000 000 Anfragen, 200 000 Namen · Fehlerschranke | 11 | 0 |
| … davon über der Schwelle | **0** | **200 000** |

Bei einer Million Anfragen — ein Tag in einem gut ausgestatteten Haushalt — ist
die Top-Domain-Ausgabe **leer**, egal was gefragt wurde. Der Modus, der der
Default ist und den ADR-0004 als den Kompromiss zwischen Nutzen und
Zurückhaltung beschreibt, liefert dann nur noch die Zurückhaltung.

Das ist kein Umsetzungsfehler. Die untere Schranke ist genau richtig; der Sketch
versagt zur sicheren Seite, wie die Modul-Dokumentation es beschrieb. Es ist die
Wahl der Struktur: ein Count-Min-Sketch ist für Datenströme gebaut, deren
Kardinalität nicht in den Speicher passt. Bei einem Haushalts-Resolver passt sie —
dieselbe Begründung, aus der [ADR-0008](0008-hashmap-statt-bloom-und-trie.md) den
Bloom-Filter nicht gebaut hat.

## Entscheidung

Der Sketch wird durch eine `HashMap` mit **exakten** Zählern ersetzt
(`logging::counts::Counts`). Damit entfallen Fehlerschranke, untere Schätzgrenze
und die Erklärung dazu.

Zwei Eigenschaften des Sketch mussten dabei erhalten bleiben — sie waren der
eigentliche Grund für ihn, nicht der Speicher:

**Erstens: kein Name unterhalb der Schwelle im Speicher.** Genau die einmalig
gefragten Domains sind die verräterischen (FEATURES.md P1); eine
`HashMap<String, u32>` würde jede von ihnen bis zum Neustart aufbewahren und das
zentrale Versprechen des Projekts kassieren. Der Schlüssel ist deshalb ein
`u64`-Hash des Namens mit einem beim Start zufällig gezogenen Salz
(`RandomState`), nicht der Name. Aus der Tabelle lässt sich kein Name ablesen;
der Klartext landet erst in `reportable`, wenn die Schwelle erreicht ist — wie
vorher.

Eine Kollision im 64-Bit-Raum würde zwei Namen zusammenzählen und damit
überschätzen, also denselben Fehler machen wie der Sketch. Bei 200 000 Einträgen
liegt ihre Wahrscheinlichkeit nach dem Geburtstagsproblem bei rund 10⁻⁹, gegen
eine Fehlerschranke von 11 beim Sketch. Der Unterschied ist kein Grad, sondern
eine Größenordnung, in der man aufhört zu rechnen.

**Zweitens: eine Obergrenze für den Speicher.** Ein Sketch hat eine feste Größe;
eine `HashMap` wächst mit der Zahl verschiedener Namen, und die ist von außen
steuerbar — DNS-Tunneling erzeugt zufällige Subdomains, aber auch ein Browser mit
NXDOMAIN-Proben tut das. Die Tabelle ist deshalb bei `MAX_TRACKED = 200_000`
gedeckelt (rund 6,5 MB, dieselbe Größenordnung wie die 4 MiB des Sketch). Ist sie
voll, bekommen neue Namen **keinen** Zähler und erreichen die Schwelle damit nie:
fail closed, wie B.1 Regel 6 es für Policy-Entscheidungen verlangt.

`MAX_TRACKED` ist eine Konstante, kein Konfigurationsschlüssel. Wer sie erreicht,
hat kein Einstellungsproblem.

## Konsequenzen

* Die Statistik stimmt wieder. Ein fünfmal gefragter Name erscheint bei `k = 5`,
  unabhängig davon, wie viel Verkehr sonst läuft.
* Zählen kostet rund 45 ns statt 100 ns, Nachschlagen 35 ns statt 90 ns
  (BENCHMARKS.md). Beides ist neben 28 µs für eine Anfrage aus dem Cache
  bedeutungslos; erwähnenswert nur, weil die probabilistische Struktur hier auch
  nicht schneller war.
* Der Speicher hängt jetzt am Verkehr statt fest zu sein: 1,4 MB bei 50 000
  Namen, 6,5 MB im Vollausbau. Für einen Raspberry Pi mit 1 GB ist das tragbar.
  **Das ist die Zahl, die diese Entscheidung umdrehen würde** — nicht die Latenz.
* Der Test `a_rare_name_stays_hidden_among_many_others` bleibt bestehen, obwohl
  er exakt gezählt nicht mehr fehlschlagen *kann*. Er prüft die Zusicherung,
  nicht die Implementierung, und wäre bei einem Rückbau sofort wieder scharf.
* Neu ist ein Zähler `dropped` für Anfragen, die keinen Zähler mehr bekamen. Ohne
  ihn wäre der volle Deckel nicht von "es wird gerade nichts gefragt" zu
  unterscheiden.

## Alternativen

* **`HashMap<String, u32>` mit dem Namen als Schlüssel.** Naheliegend und
  offensichtlich exakt — und sie speichert jede einmal gefragte Domain. Das ist
  genau das, wogegen `aggregate` existiert. Nicht verhandelbar.
* **Den Sketch behalten und breiter machen.** Die Fehlerschranke ist
  `e · Anfragen / Breite`; um bei einer Million Anfragen unter 1 zu bleiben,
  bräuchte es 2^22 Zähler je Zeile — 64 MiB, das Zehnfache der exakten Tabelle,
  für eine Struktur, die weiterhin nur schätzt.
* **Die Zähler regelmäßig zurücksetzen**, um die Fehlerschranke klein zu halten.
  Verschiebt das Problem auf ein Fenster, dessen Länge wieder ein
  Konfigurationsschlüssel wäre — und eine Domain, die über zwei Fenster verteilt
  `k`-mal gefragt wird, verschwindet dann aus der Statistik.
* **Kein Deckel auf der Tabelle.** Bequemer und ein Speicherleck, das jedes Gerät
  im LAN auslösen kann.
