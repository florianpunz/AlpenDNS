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

---

## Phase 7 — Live-Strom unter Last · gemessen am 2026-08-30

**Anlass:** Die Web-UI hing bei einem Lasttest und zeigte nur noch alle paar
Sekunden ein Update. Die Ursache lag nicht im Browser, sondern in der
Schnittstelle: der Server schickte **eine SSE-Nachricht je Anfrage**.

Gemessen gegen einen laufenden Server auf Port 15353 mit einer lokalen
Blockliste (alle Anfragen werden lokal beantwortet, kein Upstream im Spiel).
Der Lastgenerator ist ein UDP-Flooder ohne Antwort-Auswertung, `dnsperf` ist auf
dieser Maschine nach wie vor nicht installiert. Das Binary lief im
Profil `dev` — die absoluten Durchsatzzahlen sind deshalb *keine* Aussage über
die Leistung des Resolvers, nur der Rahmen für den Vergleich darunter.

| | vorher (rechnerisch) | nachher (gemessen) |
|---|---|---|
| Anfragen in 5 s | 484 294 | 484 294 |
| SSE-Nachrichten | 484 294 | **151** |
| JSON-Frames je Sekunde | ~97 000 | 25 |
| DOM-Zeilen je Sekunde im Browser | ~97 000 | 25 |

Von den 151 Nachrichten trugen 5 ein `skipped`-Feld, zusammen 428 471
ausgelassene Anfragen — je eine zu Beginn jeder Sekunde. Der Puls bleibt damit
vollständig, obwohl 99,97 % der Nachrichten entfallen.

**Kostet ein offenes GUI den Resolver etwas?** Zweimal 4 s Flut, einmal ohne und
einmal mit offener SSE-Verbindung:

| | beantwortete Anfragen in 4 s |
|---|---|
| ohne offenes GUI | 354 497 |
| mit offenem GUI | 387 734 |

Der Unterschied liegt innerhalb der Streuung zwischen zwei Läufen; ein
mitlesendes GUI ist im Rauschen nicht mehr zu finden. Vorher kostete es den
Server je Anfrage einen formatierten Zeitstempel, mehrere Allokationen und eine
JSON-Serialisierung.

**Im Normaltempo wird nichts ausgelassen:** 12 Anfragen im Abstand von 250 ms
ergaben 12 Nachrichten, keine davon mit `skipped`. Die Grenze greift erst
oberhalb von 25 Anfragen pro Sekunde — schneller kann ohnehin niemand mitlesen.

Die Browser-Seite ist nicht separat vermessen (dafür fehlt hier ein Browser).
Geändert sind dort drei Dinge, deren Wirkung sich aus der Zahl oben ergibt:
Ereignisse werden gesammelt und einmal je Bild gezeichnet statt einzeln, die
Tabelle hat einen Zuhörer statt zwei je Zeile, und eine unsichtbare Seite
zeichnet und pollt nicht mehr.

---

## Phase 8 — Heuristiken · gemessen am 2026-08-30

**Messaufbau.** Zwei Korpora aus der Majestic Million (CC-BY 3.0), beide unter
`corpus/` und beide nicht im Repo:

| Datei | Ränge | Zweck |
|---|---|---|
| `train-500k.txt` | 100 001 – 600 000 | Training des DGA-Modells |
| `top-100k.txt` | 1 – 100 000 | Messung |

Reproduzierbar mit `cargo test --release --test detect_corpus -- --ignored
measure --nocapture`; die Anleitung zum Beschaffen steht im Kopf derselben Datei
und in [TESTING.md](TESTING.md).

### Warum zwei Korpora, und was der erste Anlauf gekostet hat

Der erste Messlauf trainierte und maß auf **derselben** Top-100k und meldete
0,001 % Falsch-Positive. Auf ungesehenen Namen waren es **0,54 %** — Faktor 500.
Ein Modell erkennt die Namen wieder, aus denen es gebaut wurde. Seither wird auf
den Rängen dahinter trainiert und auf der Top-100k gemessen, die kein einziges
Mal ins Modell eingegangen ist.

Die Zahl darunter ist die zweite, nicht die erste.

### DGA-Erkennung

Schwelle 0,75. **77 von 100 000 Namen gemeldet — 0,077 %**, gegen eine Zusage von
0,1 %.

| Schwelle | Falsch-Positive | alphanumerisch | necurs-artig | conficker-artig |
|---:|---:|---:|---:|---:|
| 0,70 | 0,098 % | 93,8 % | 47,0 % | 33,2 % |
| **0,75** | **0,077 %** | **92,8 %** | **40,6 %** | **28,6 %** |
| 0,80 | 0,039 % | 38,8 % | 31,9 % | 18,7 % |

0,75 steht unmittelbar vor der Kante: bei 0,80 bricht die alphanumerische
Familie von 92,8 % auf 38,8 % ein, während die Fehlalarme nur von 77 auf 39
zurückgehen. Bei 0,70 bliebe keine Reserve unter der Zusage.

**Trefferquote je Familie** (je 5000 nachgebaute Namen, Schwelle 0,75):

| Familie | Ø Überraschung | Trefferquote |
|---|---:|---:|
| alphanumerisch | 7,92 Bit | 92,8 % |
| necurs-artig | 6,17 Bit | 40,6 % |
| conficker-artig | 6,01 Bit | 28,6 % |
| kraken-artig (aussprechbar) | 4,79 Bit | 0,5 % |
| suppobox-artig (Wörterbuch) | 3,45 Bit | 0,0 % |

Zum Vergleich gewachsene Namen: Median 3,69 Bit, 99 % bei 6,24, Maximum 10,73.
**Die Verteilungen überlappen** — ein aussprechbar erzeugter Name *ist*
statistisch ein gewachsener Name. Die letzten beiden Zeilen sind deshalb keine
Lücke in der Umsetzung, sondern die Grenze des Verfahrens (FEATURES.md D3), und
sie stehen als Test in `dga::tests::a_word_list_dga_is_honestly_not_detected`.

**Zwei Klassen systematischer Fehlalarme sind dabei verschwunden:**

* *Punycode.* Vier der zwanzig auffälligsten Namen im ersten Lauf waren IDNs —
  `xn--vhqrb498dfmcffp24qfocl09dqkh.cn` sieht für ein lateinisches Zeichenmodell
  aus wie base32. Sie werden nicht mehr bewertet; ein benannter blinder Fleck ist
  besser als ein Fehlalarm für ganze Sprachräume.
* *Private Suffixe.* `d1a2b3c4e5.cloudfront.net` ist ein erzeugter Name — nur
  vergibt ihn der Anbieter so, und `cloudfront.net` steht selbst in der Public
  Suffix List. Unterhalb eines privaten Suffixes wird nicht mehr bewertet.

Was bleibt, sind zum größten Teil **Pinyin-Kürzel**: `hnqxdzkj.com`,
`lzdsxxb.com`, `pzhsdqfybjfwzx.cn`. Gewachsene Namen aus Anfangsbuchstaben
chinesischer Silben, für ein Modell über lateinischem Text nicht von Zufall zu
unterscheiden.

### Tunneling-Erkennung

Schwelle 0,75. **0 von 100 000 Namen gemeldet — 0,0000 %.**

Der Korpus wird dabei so eingespielt, wie er im schlimmsten Fall aussähe: alle
100 000 Namen innerhalb eines Fensters von fünf Minuten. Das ist weit mehr
Verkehr, als ein Haushalt erzeugt.

| Verkehr | Score |
|---|---:|
| 20 Hosts unter einer Zone, je fünfmal gefragt | 0,17 |
| `dnscat2`-artig (hex, 36 Zeichen, TXT) | 0,81 |
| `iodine`-artig (base32, 58 Zeichen, TXT) | 1,00 |

Die Schwelle steht am **unteren** Rand der Treffer und nicht in der Mitte der
Lücke: `dnscat2` kodiert hexadezimal und kommt über 4 Bit Entropie nicht hinaus,
liegt also knapp über 0,8. Bei 0,8 als Schwelle entschiede die zweite
Nachkommastelle darüber, ob der verbreitetste Tunnel auffällt.

### Typosquat-Wächter

Schutzliste mit fünf Domains gegen dieselben 100 000 Namen: **18 Meldungen,
0,018 %**. Keine davon ist eine der geschützten Domains selbst — das ist der Teil
des Kriteriums, der zählt.

### Was die Detektoren den Anfragepfad kosten

Die Frage ist nicht rhetorisch: die Tunneling-Erkennung nimmt bei **jeder**
Anfrage einen `Mutex` über einer Tabelle, und CLAUDE.md B.3 Regel 5 sagt "kein
globaler Mutex im Anfragepfad".

Gemessen mit dem Lastgenerator aus Phase 2, lauter neue Namen — nur dann laufen
die Detektoren wirklich bei jeder Anfrage (`cargo test --release --test load --
--ignored throughput_with_and_without_detectors`). Drei Läufe:

| Lauf | ohne Detektoren | mit vier Detektoren | Anteil |
|---|---:|---:|---:|
| 1 | 96 191 /s | 87 065 /s | 90,5 % |
| 2 | 100 707 /s | 85 689 /s | 85,1 % |
| 3 | 97 127 /s | 88 064 /s | 90,7 % |

**Rund 10 % Durchsatz**, im schlechtesten Lauf 15 %. Das ist kein Rauschen,
sondern ein Preis — und er ist bezahlbar: 87 000 Anfragen pro Sekunde sind für
einen Haushalts-Resolver drei Größenordnungen über dem, was je gebraucht wird.

Der Mutex bleibt damit vorerst. Die Umkehrbedingung ist dieselbe wie bei
[ADR-0008](adr/0008-hashmap-statt-bloom-und-trie.md): wenn diese Messung eines
Tages unter zwei Drittel fällt — etwa weil mehr Detektoren dazukommen oder der
Resolver auf schwächerer Hardware läuft —, gehört die Zonentabelle hinter ein
`ArcSwap` oder eine Aufteilung nach Hash. Vorher wäre es Optimierung ohne
Messung, und die verbietet Phase 4 ausdrücklich.

### Speicher

| | |
|---|---|
| DGA-Modell im Binary | 109 744 Byte |
| Tunneling-Zustand, Obergrenze | 4096 Zonen × höchstens 256 Hashes je Zone |

Beide Grenzen sind hart und stehen als Test da
(`the_table_does_not_grow_with_traffic`, `the_unique_set_per_zone_is_bounded`):
die Tabelle liegt im Anfragepfad und darf nicht mit dem Verkehr wachsen.

---

## Phase 9 — Drosselung pro Client · gemessen am 2026-08-30

```bash
cargo test --release --test load -- --ignored --nocapture --test-threads=1 \
    throughput_with_and_without_rate_limiting a_flooding_client
```

Die Last kommt hier aus **16 verschiedenen Absenderadressen** (`127.0.0.1` bis
`127.0.0.16`) und nicht wie in den früheren Läufen aus einer. Anders wäre die
Messung sinnlos: aus einer Quelle wäre die einzige Frage, wie schnell ein Eimer
leerläuft.

### Was die Drosselung den Anfragepfad kostet

Das Limit steht dabei so hoch (1 000 000/s), dass nichts verworfen wird —
gemessen wird der Weg durch den Token-Bucket, nicht seine Wirkung.

| Anfragepfad | Anfragen/s | p50 | p99 |
|---|---:|---:|---:|
| ohne Drosselung | 85 788 /s | 145 µs | 663 µs |
| mit Drosselung | 84 078 /s | 147 µs | 674 µs |

**2 % Durchsatz**, p99 um 11 µs schlechter. Das ist der Preis für ein
Pflichtstück (CLAUDE.md B.5), und er ist keiner: die Drosselung sitzt *vor* dem
Parsen, ein verworfenes Paket kostet einen Hash und einen Vergleich.

RSS des Testprozesses über den ganzen Lauf: 31 272 KiB → 39 732 KiB, mit 16
beobachteten Clients. Die Buchführung selbst ist gedeckelt (LRU, Default 8192
Clients); der Zuwachs hier ist der Cache mit 32 000 neuen Namen, nicht der
Limiter.

### Ein Störer neben normalen Clients

Ein Client feuert zehn Sekunden lang, so schnell er kann, gegen ein Limit von
200/s (Spitze 400). Daneben laufen die 16 Messclients ihre übliche Last.

| | |
|---|---:|
| Der Störer schickte | 2 746 620 Anfragen |
| davon verworfen | 2 742 924 (99,87 %) |
| Die übrigen Clients | 3 804 Anfragen/s, p50 157 µs, **p99 765 µs** |

Der Befund, auf den es ankommt, ist die **p99 der übrigen Clients: 765 µs gegen
674 µs im ungestörten Lauf**. Unter einer Flut von 274 000 Paketen je Sekunde
bleibt die Antwortzeit für alle anderen praktisch unverändert — das ist es, was
„andere IPs unbeeinflusst" heißen soll.

Der Durchsatz der übrigen Clients fällt dabei von 84 000 auf 3 800 Anfragen/s,
und das ist **kein** Ergebnis über die Drosselung: der Störer läuft als Task im
selben Prozess auf denselben Kernen und verbrennt sie mit seiner eigenen
Sendeschleife. Über echtes Netz wäre die Konkurrenz eine andere. Die Zahl steht
hier, weil sie im Testausgang steht — nicht als Aussage über den Betrieb.

### Anfragen/s, p99, RSS auf einen Blick

Die drei Zahlen, die ROADMAP Phase 9 Schritt 7 verlangt, im Auslieferungszustand
(Cache an, Drosselung an, keine Detektoren, lauter neue Namen — also der teure
Fall, in dem der Cache nie hilft):

| | |
|---|---:|
| Durchsatz | **84 078 Anfragen/s** |
| p99 | **674 µs** |
| RSS am Ende des Laufs | **39 732 KiB** |

---

## TCP-Verbindungsaufbau unter den neuen Grenzen · gemessen am 2026-09-16

```bash
cargo test --release --test load -- --ignored --nocapture --test-threads=1 \
    tcp_connection_setup_with_the_limits
```

Der UDP-Durchsatz oben sagt über diese Änderung nichts: dort wird kein einziges
Mal eine Verbindung aufgebaut. Die Grenzen aus TODOS Nr. 1 sitzen im
Accept-Pfad, also misst dieser Lauf **Verbindungen je Sekunde mit je einer
Anfrage**, von 16 Absenderadressen mit je 500 Verbindungen. Jeder Client kommt
von einer eigenen Adresse — sonst greift vorher das Kontingent je Adresse und
die Messung misst das Falsche.

Verglichen wird gegen den Stand vor der Änderung. Weil der alte Stand die
Zähler nicht kennt, lief die Messung dort gegen eine Attrappe derselben
Signatur (`with_tcp_stats` ohne Wirkung); der Accept-Pfad selbst blieb
unangetastet.

| | Verbindungen/s |
|---|---:|
| vorher (Median aus 7 Läufen) | 31 068 |
| nachher (Median aus 7 Läufen) | 29 030 |
| Streuung je Seite | 27 300 – 33 900 |

**Der Unterschied liegt in der Streuung.** Ein Lauf allein sagt hier nichts:
die Werte derselben Variante schwanken um bis zu 20 %, gegeneinander gemessen
in wechselnder Reihenfolge. Was bleibt, ist die Aussage, die der Aufbau hergibt:
ein Semaphor-Zugriff, ein Hash-Eintrag und ein `Arc` je Verbindung sind
gegenüber einem TCP-Handshake nicht messbar. Die Grenzen kosten im
Verbindungsaufbau nichts, was diese Maschine auflösen könnte.

Kein Wunder: sie greifen nur, wenn es schon zu spät ist. Im Normalbetrieb ist
der Preis ein Zweig, der nie genommen wird.
