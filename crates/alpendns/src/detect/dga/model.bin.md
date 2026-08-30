# `model.bin` — das 3-Gramm-Modell der DGA-Erkennung

Diese Datei beschreibt die Binärdatei daneben: woher sie kommt, wie sie erzeugt
wurde und was in ihr steht. Ohne sie wäre `model.bin` ein Klumpen von 107 KiB,
den niemand prüfen oder neu erzeugen kann.

## Herkunft der Daten

**Majestic Million**, abgerufen am 2026-08-30 von
`https://downloads.majestic.com/majestic_million.csv`.

Lizenz: **CC-BY 3.0** (Majestic-12 Ltd.). Die Lizenz verlangt Namensnennung und
erlaubt abgeleitete Werke — `model.bin` ist ein solches: es enthält keinen
einzigen Domainnamen, sondern nur aggregierte Häufigkeiten von Zeichentripeln.

Warum diese Liste und nicht Tranco oder Cisco Umbrella: sie kommt als reine CSV
ohne Zip-Archiv, und ihre Lizenz benennt den Fall abgeleiteter Werke
ausdrücklich. Die Liste selbst liegt **nicht** im Repository (`corpus/` steht in
`.gitignore`) — sie ist 80 MB groß, für den Bau nicht nötig und fremde Daten.

## Was trainiert wurde und was gemessen wird

| | Ränge | Zweck |
|---|---|---|
| `corpus/train-500k.txt` | 100 001 – 600 000 | Training |
| `corpus/top-100k.txt` | 1 – 100 000 | Messung der Falsch-Positiv-Rate |

**Die Trennung ist der Punkt.** Beim ersten Anlauf war es andersherum — trainiert
und gemessen auf derselben Top-100k —, und die Zahl war um den Faktor 500 zu
gut: 0,001 % gegen 0,54 % auf ungesehenen Namen. Ein Modell erkennt die Namen
wieder, aus denen es gebaut wurde.

Gemessen wird auf der Top-100k, weil die Roadmap es so verlangt und weil es der
aussagekräftigste Ausschnitt ist: das sind die Namen, die im Betrieb tatsächlich
gefragt werden.

## Aufbau der Datei

38³ = 54 872 Einträge zu je zwei Byte, little-endian, insgesamt 109 744 Byte.
Der Index eines Tripels `(a, b, c)` ist `(a · 38 + b) · 38 + c`.

Das Alphabet hat 38 Symbole: `0` ist die Randmarke und zugleich der Auffangwert
für alles Unbekannte, `1..=26` sind `a`–`z`, `27..=36` sind `0`–`9`, `37` ist der
Bindestrich.

Jeder Eintrag ist `-log2(P(c | a, b)) · 512`, auf `u16` geklemmt — also die
Überraschung in Bit, mit einer Auflösung von 1/512 Bit. Geschätzt mit additiver
Glättung, α = 0,01: ein nie gesehenes Tripel bekommt dadurch einen hohen, aber
endlichen Wert statt unendlich.

## Neu erzeugen

```sh
mkdir -p corpus
curl -sSL https://downloads.majestic.com/majestic_million.csv | tail -n +2 \
  | cut -d, -f3 > /tmp/majestic.txt
head -100000            /tmp/majestic.txt > corpus/top-100k.txt
sed -n '100001,600000p' /tmp/majestic.txt > corpus/train-500k.txt

cargo test --release --test detect_corpus -- --ignored train_the_dga_model --nocapture
cargo build                                  # das neue Modell einbacken
cargo test --release --test detect_corpus -- --ignored measure --nocapture
```

Der zweite Schritt ist nicht optional: `include_bytes!` bäckt die Datei zur
Bauzeit ein, und ein Messlauf direkt nach dem Training misst noch das alte
Modell. Beim ersten Anlauf hat genau das eine Runde gekostet — die Statistik
zeigte lauter Nullen.

## Verteilung, aus der die Schwellen kommen

Überraschung in Bit je Tripel, über die Trainingsdaten:

| Quantil | Bit |
|---|---:|
| Median | 3,69 |
| 90 % | 4,50 |
| 99 % | 6,24 |
| 99,9 % | 7,85 |
| Maximum | 10,73 |

Zum Vergleich die nachgebauten DGA-Familien (Mittelwert):

| Familie | Bit | Trefferquote bei Schwelle 0,75 |
|---|---:|---:|
| alphanumerisch | 7,92 | 92,8 % |
| necurs-artig | 6,17 | 40,6 % |
| conficker-artig | 6,01 | 28,6 % |
| kraken-artig (aussprechbar) | 4,79 | 0,5 % |
| suppobox-artig (Wörterbuch) | 3,45 | 0,0 % |

Die beiden Verteilungen überlappen. Das ist keine Schwäche der Umsetzung,
sondern die Eigenschaft eines Zeichenmodells: ein aussprechbar erzeugter Name
*ist* statistisch ein gewachsener Name. Deshalb ist die Falsch-Positiv-Rate das
Qualitätsmaß und nicht die Trefferquote (FEATURES.md D3).
