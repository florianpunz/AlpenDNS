# Performance- und Leichtgewichtigkeits-Analyse

Zahlen, keine Behauptungen — derselbe Grundsatz wie in [BENCHMARKS.md](BENCHMARKS.md).
Dieses Dokument sammelt, was beim Durchgehen des **heißen Pfads** aufgefallen ist:
Query-Pipeline, Cache, Matcher, Upstream-Pool, die Detektoren aus Phase 8.

## Methodik und Messstatus

Diese Analyse ist ein **Lese-Ergebnis**, kein Messlauf. Zwei Dinge gehören deshalb
vorweg, sonst liest sich die Spalte „Messergebnis" falsch:

1. **Was gemessen ist**, steht in [BENCHMARKS.md](BENCHMARKS.md) und wird hier
   nur zitiert. Die Messmaschine ist Linux (`load.rs` mit SO_REUSEPORT und
   Fake-Upstream im Prozess, `--test-threads=1`).
2. **Was hier neu vorgeschlagen wird, ist noch nicht gemessen.** Der Lastgenerator
   läuft nur unter Linux, das Korpus liegt nicht im Repo, und auf der Maschine,
   auf der diese Analyse entstand, gibt es keine Rust-Toolchain. Ein Kandidat
   steht deshalb nicht als „schneller" da, nur als „lohnt zu messen" — mit dem
   Befehl, der ihn bestätigt oder verwirft.

Genau deshalb unterscheidet die Tabelle unten zwei Sorten von Einträgen:

* **gemessen** — die Zahl kommt aus BENCHMARKS.md und ist nachgewiesen.
* **ausstehend** — der Lösungsansatz ist isoliert umzusetzen und gegen die
  Baseline zu messen, bevor er in den `main`-Zweig dürfte. Vorher wäre es
  Optimierung ohne Messung, und genau die verbietet das Projekt ausdrücklich.

Der Weg vom Kandidaten zur Entscheidung ist für alle ausstehenden derselbe:

```bash
# 1. Baseline (Auslieferungszustand, teurer Fall: lauter neue Namen)
cargo test --release --test load -- --ignored --nocapture --test-threads=1

# 2. Isoliert auf einem perf/<thema>-Branch umsetzen, dann Gegenlauf.
#    Bewertet wird der Vergleich beider Zeilen, nicht der Absolutwert.
```

## Ergebnis auf einen Blick

| # | Fundstelle | Problem | Lösung | Messergebnis | Aufwand |
|---|---|---|---|---|---|
| 1 | `server/mod.rs` `build_event` | baut `query_type`, `why` (Vec<String>), `findings` bei **jeder** Anfrage, obwohl `aggregate`/`none` sie verwerfen | Felder nur materialisieren, wenn der Log-Modus sie braucht | ausstehend | M |
| 2 | `upstream/pool.rs` `is_down` | `Mutex`-Lock je Upstream je Cache-Miss, obwohl der Wert nur bei Ausfall geschrieben wird | `Mutex<Option<Instant>>` → `AtomicU64` (monotone Mikrosekunden) | ausstehend | S |
| 3 | `policy/mod.rs` `resolve` | `asked`-String (to_ascii + lowercase) bei jeder Anfrage, gebraucht nur für den Rebinding-Check | nur bauen, wenn ein Antwort-Detektor eingehängt ist | ausstehend | S |
| 4 | `detect/dga/mod.rs` | `symbols`/`mean_surprise` allozieren Vec je Label; Labels sind ≤ 63 Zeichen | Stack-Puffer fester Größe statt Heap-Vec | ausstehend | S |
| 5 | `upstream/strategy.rs` `registrable_domain` | PSL-Lookup + bis zu drei String-Allokationen je Cache-Miss | Hashen der registrierbaren Domain ohne die letzte `to_owned()` | ausstehend | S |
| 6 | `detect/typosquat.rs` `embeds` | `format!(".{protected}")` drei Mal je Eintrag je Anfrage | einmal beim Aufbau des Detektors vorberechnen | ausstehend | S |
| — | Detektoren gesamt | laufen bei jeder Allow-Anfrage | **keiner** — akzeptierter Preis, siehe unten | **gemessen: ~10 %** | — |
| — | Tunneling-Mutex | globaler Mutex je Anfrage | **keiner jetzt** — Umkehrbedingung dokumentiert | **gemessen: Teil der 10 %** | — |
| — | Cache-Treffer `with_ttl` | Deep-Clone der Antwort je Treffer | **keiner** — Preis des Treffers | **gemessen: p50 18,8 µs** | — |

Die drei letzten Zeilen sind **keine Kandidaten** — sie stehen in der Tabelle, weil
sie im heißen Pfad am lautesten sind und die Entscheidung, sie so zu lassen, Teil
dieser Analyse ist (siehe „Verworfene Ansätze").

---

## Die Kandidaten im Detail

### 1. `build_event` materialisiert Namen und Begründung in jedem Log-Modus

**Ist-Zustand.** [server/mod.rs:206–239](crates/alpendns/src/server/mod.rs#L206-L239)
baut bei jeder beantworteten Anfrage ein `QueryEvent`, und darin stehen — immer —
`name`, `query_type`, `why` (ein `Vec<String>`, für jeden `Step` ein
`format!`-Aufruf, siehe [trace.rs:108–170](crates/alpendns/src/trace.rs#L108-L170))
und `findings`. Die Logging-Schicht wirft davon in den leisen Modi das meiste weg:

| Feld | gebraucht in `none` | in `aggregate` (Default) | in `ring`/`full` |
|---|---|---|---|
| `name` | nein | ja (gesalzener Zähler) | ja |
| `query_type` | nein | **nein** | ja |
| `why` | nein | **nein** | ja |
| `findings` | nein | **nein** | ja |

Im Default `aggregate` werden also `query_type`, `why` und `findings` bei *jeder*
Anfrage formatiert und dann verworfen. `why` ist dabei der teure Teil: je Schritt
ein `write!` mit Namen, Dauer oder Score — bei einem normalen Allow-Treffer zwei
bis vier Schritte, alle als eigene String-Allokation.

**Lösungsansatz.** Die Felder erst bauen, wenn der Modus sie braucht. Konkret:
`build_event` (oder `QueryLog::record`) prüft `mode.keeps_names()` und lässt die
teuren Felder sonst leer bzw. baut sie gar nicht. Der Trace selbst (`ctx.steps()`)
bleibt unangetastet — die Regel „der Trace entsteht immer, der Modus entscheidet
nur, was mit ihm passiert" (ARCHITECTURE.md §2, B.1 Regel 3) gilt weiterhin für
*Schritte*, nicht für die *Formatierung* daraus.

**Trade-off-Check.** Es entfällt keine Funktionalität: in `ring`/`full` wird
weiter alles gebaut, in `aggregate`/`none` wird nichts gebaut, was dort ohnehin
niemals gelesen wird. Der einzige Risikopunkt ist, dass `QueryEvent` heute ein
einziges Struct ist, das `record` komplett konsumiert — der Umbau macht einzelne
Felder optional oder verschiebt ihre Erzeugung. Deshalb M, nicht S. Die
Privacy-Zusicherung (`no_query_name_leaves_the_process_in_the_quiet_modes`) muss
nach dem Umbau grün bleiben; sie ändert sich nicht, weil am *Inhalt* nichts ändert.

### 2. `pool::order` nimmt einen Mutex je Upstream je Anfrage

**Ist-Zustand.** [pool.rs:361–367](crates/alpendns/src/upstream/pool.rs#L361-L367)
liest `is_down` für **jeden** Upstream bei **jedem** Cache-Miss aus
`down_until: Mutex<Option<Instant>>` ([pool.rs:47](crates/alpendns/src/upstream/pool.rs#L47)).
Der Wert wird nur geschrieben, wenn ein Upstream zum dritten Mal in Folge ausfällt
oder sich erholt ([pool.rs:216–279](crates/alpendns/src/upstream/pool.rs#L216-L279))
— also praktisch nie, gelesen wird er dauernd. Das ist genau das Muster, für das
B.3 Regel 5 (`ArcSwap`/Atomic statt Mutex im Anfragepfad) gemacht ist.

**Lösungsansatz.** `down_until` als `AtomicU64`, kodiert als „monotone Mikrosekunden
seit einem Prozess-Startzeitpunkt", `0` heißt „nicht down". Schreiben und Lesen
werden `Ordering::Relaxed`-Zugriffe. Der Vergleich `now < until` bleibt derselbe,
nur eben ohne Lock.

**Trade-off-Check.** Kein Verhalten ändert sich: dieselbe Schwelle, dieselbe
Cooldown-Dauer. Der einzige Preis ist eine kleine Kodierfunktion für die
Uhrzeit. Ein `Instant` hat keine portable Epoche, deshalb der Umweg über
`Instant::now().elapsed()` — der Startzeitpunkt wird einmal in der `Health`
festgehalten. Geringes Risiko, klarer Gewinn: ein Relaxed-Load ersetzt einen
Mutex-Lock (der bei Konkurrenz bis zum Syscall reichen kann).

### 3. `asked` wird bei jeder Anfrage gebaut, gebraucht nur der Rebinding-Check

**Ist-Zustand.** [policy/mod.rs:620–621](crates/alpendns/src/policy/mod.rs#L620-L621)
baut `asked` — `query.name().to_ascii().trim_end_matches('.').to_lowercase()` — bei
jeder Anfrage. Gebraucht wird es nur unten im Rebinding-Post-Check
([policy/mod.rs:636](crates/alpendns/src/policy/mod.rs#L636)). Auf dem typischen
Cache-Treffer-Pfad wird dieser String erzeugt und nie benutzt.

**Lösungsansatz.** `asked` nur bauen, wenn `self.engine` überhaupt einen
Antwort-Detektor hält (also der Rebinding-Schutz nicht `off` ist). Das ist beim
Aufbau bekannt und als `bool`/Methodenaufruf abfragbar, ohne den Anfragepfad zu
ändern.

**Trade-off-Check.** Trivial und ohne Verhaltensänderung; die eine zusätzliche
String-Allokation je Anfrage entfällt auf dem Trefferpfad. Aufwand S.

### 4. DGA-Erkennung alloziiert je Label auf dem Heap

**Ist-Zustand.** [dga/mod.rs:144–175](crates/alpendns/src/detect/dga/mod.rs#L144-L175):
`symbols()` legt einen `Vec<usize>` an, `mean_surprise()` ruft das auf und
iteriert. Ein DNS-Label ist laut RFC höchstens 63 Zeichen, mit den drei
Randmarken also höchstens 66 Symbole — eine Obergrenze, die in den Prozess
eingebaut ist, nicht erst zur Laufzeit entsteht.

**Lösungsansatz.** Feste Stack-Puffer (`[usize; 66]` o. Ä.) statt `Vec`, analog
zur `SmallVec`-Idee aus dem Trace. Zwei Heap-Allokationen je Label entfallen; die
DGA-Erkennung läuft bei jeder Allow-Anfrage, also auf dem heißen Pfad.

**Trade-off-Check.** Kein Verhaltensunterschied, nur Allokationsersatz. Die
Grenze `MIN_LENGTH` und die Schwellen bleiben unberührt — es ändert sich nur, wo
die Symbolliste liegt. Aufwand S. Einzige Sorgfalt: die Puffergröße als Konstante
neben `ALPHABET` dokumentieren, damit niemand die 66 „optimiert", ohne die
Randmarken mitzuzählen.

### 5. `registrable_domain` baut Strings, die nur gehasht werden

**Ist-Zustand.** [strategy.rs:23–30](crates/alpendns/src/upstream/strategy.rs#L23-L30):
`to_ascii()` (Allokation), `trim_end_matches('.').to_ascii_lowercase()` (zweite
Allokation), `psl::domain_str(...)`, und am Ende `.map_or(text.clone(), ToOwned::to_owned)` —
eine dritte Allokation, die bei `zone_index` nur dazu da ist, den `&str` zu
besitzen, damit er gehasht werden kann. Es reicht, den `&str` direkt zu hashen,
solange `text` lebt.

**Lösungsansatz.** Die letzte `to_owned()` weglassen und den von `psl::domain_str`
geborgten `&str` hashen. `text` bleibt bis zum Ende der Funktion am Leben; der
Hash-Besuch braucht keinen eigenen Besitzer.

**Trade-off-Check.** Reine Allokationsvermeidung auf dem Miss-Pfad, kein
Verhaltensunterschied — der Hash desselben `&str` ist identisch. Aufwand S.
Hinweis zur Ehrlichkeit: der Gewinn trifft nur Cache-Misses; im Haushaltsbetrieb
mit hoher Trefferquote ist er klein.

### 6. Typosquat baut den Embed-String je Eintrag je Anfrage neu

**Ist-Zustand.** [typosquat.rs:179–184](crates/alpendns/src/detect/typosquat.rs#L179-L184)
setzt in `embeds` drei Mal `format!(".{protected}")` zusammen — je geschützter
Domain, je Anfrage. Die geschützten Domains sind beim Aufbau des Detektors
bekannt und ändern sich nie.

**Lösungsansatz.** Den vorangestellten Punkt-String einmal beim Aufbau
([typosquat.rs:67–83](crates/alpendns/src/detect/typosquat.rs#L67-L83)) an das
`Protected` hängen und in `embeds` nur noch vergleichen.

**Trade-off-Check.** Nur relevant, wenn `detection.typosquat.protect` nicht leer
ist; dann entfallen drei kleine Allokationen je geschützter Domain je Anfrage.
Aufwand S, kein Verhalten geändert.

---

## Verworfene Ansätze

Diese wurden beim Durchgehen notiert und **bewusst nicht** als Kandidat
aufgenommen. Jeder kurz mit dem Grund, damit die Entscheidung nachvollziehbar
bleibt und nicht „wir haben es übersehen" heißt.

### Detektoren sind ~10 % Durchsatz — wird so gelassen

**Gemessen** (BENCHMARKS.md „Phase 8"): vier Detektoren kosten rund 10 %,
schlechtester Lauf 15 %. Das ist ein Preis, aber bezahlbar: 87 000 Anfragen/s
sind für einen Haushalts-Resolver drei Größenordnungen über dem Bedarf. Die
Entscheidung, den Preis zu tragen, steht schon in
[ADR-0019](adr/0019-heuristiken-melden-statt-blocken.md) und in der
BENCHMARKS-Begründung — hier wird sie nur noch einmal sichtbar gemacht, nicht
neu verhandelt.

### Tunneling-Mutex: kein Umbau vor der Umkehrbedingung

Der globale `Mutex<HashMap>` in [tunneling.rs](crates/alpendns/src/detect/tunneling.rs)
ist der lauteste einzelne Detektor-Posten. Die Umkehrbedingung ist bereits
dokumentiert (BENCHMARKS.md „Phase 8"): wenn der Durchsatz einmal unter zwei
Drittel fällt — mehr Detektoren, schwächere Hardware —, wandert die Zonentabelle
hinter ein `ArcSwap` oder eine Hash-Aufteilung. **Vorher wäre es Optimierung ohne
Messung.** Der Kandidat 2 (Pool-Mutex) ist deshalb der ehrlichere erste Schritt:
dort ist der Mutex ein *reiner Lese*-Mutex ohne Inhalt, hier schützt er echten
Zustand.

### Cache-Treffer klont die Antwort komplett — das ist der Preis des Treffers

`with_ttl` ([cache.rs:291–305](crates/alpendns/src/cache.rs#L291-L305)) klont die
gesamte Nachricht, um die Rest-TTL zu setzen. **Gemessen:** p50 18,8 µs, p99
28,1 µs — bei geladenem Zwei-Millionen-Matcher. Die Antwort muss je Treffer mit
der passenden Rest-TTL heraus, und die gecachte Kopie darf nicht mutiert werden;
ein Clone ist der direkte Weg dahin. Die Alternative (Antworten in einer
kompakteren Form halten und billig neu bauen) wäre ein Architektur-Umbau für eine
Zahl, die gegen die DoT-Round-Trip-Zeit ins Internet ohnehin verschwindet.
**Verworfen:** kein messbarer Nutzen erwartet, hoher Aufwand.

### `cache::shard` hascht mit SipHash — unter der Nachweisgrenze

[shard()](crates/alpendns/src/cache.rs#L150-L155) berechnet je Lookup einen
SipHash über den ganzen Schlüssel, nur um 4 Bit abzuleiten. SipHash ist teurer
als ein billiger Hash — aber gemessen an einem Cache-Treffer von 18,8 µs sind das
Nanosekunden. Ein schnellerer Hash bräuchte außerdem eine neue Abhängigkeit
(`ahash`/`fxhash`/…), die B.2 eine Begründung verlangt. **Verworfen:** unter der
Nachweisgrenze, und eine Abhängigkeit für unter 1 % eines Treffers ist der falsche
Handel. Erst wieder erwägen, wenn ein Profiler die Shard-Auswahl sichtbar macht.

### UDP-Puffer-Pooling — Komplexität ohne Nutzen

Der `to_vec()` je Paket in [udp.rs](crates/alpendns/src/server/udp.rs) alloziiert
je eingehendem Paket einen frischen Puffer. Das ist, wie tokio UDP funktioniert:
jeder `recv` braucht einen Puffer, und die Antwort braucht ihre Bytes. Ein Pool
darüber spart eine Allokation und erkauft sie mit Lebensdauer-Buchführung und dem
Risiko, dass ein gepoolter Puffer irgendwo weiterlebt. **Verworfen:** Aufwand
steht in keinem Verhältnis zum Gewinn, solange kein Profil die UDP-Allokation als
heiß zeigt.

### Doppelte `Message`-Klone in Cache- und Transportschicht

`request.clone()` in [caching.rs:61](crates/alpendns/src/caching.rs#L61) und die
Klone in [transport.rs](crates/alpendns/src/upstream/transport.rs) sind billig:
eine Anfrage trägt nur ihre Frage, keine Antworten. **Verworfen:** nicht der Ort,
an dem Zeit verloren geht; ein Umbau hier riskierte Lebensdauer-Fehler für nichts.

---

## Was als Nächstes passieren muss

Die ausstehenden Kandidaten sind isoliert — je ein `perf/<thema>`-Branch, nie auf
`main` — umzusetzen und gegen die Baseline zu messen. Erst ein Ansatz mit
nachgewiesenem Nutzen kommt zurück. Die Reihenfolge der Messung sollte der Spalte
„Aufwand" folgen, nicht dem vermuteten Gewinn: die drei S-Kandidaten (2, 3, 4)
sind je ein Nachmittag und liefern schnell eine belastbare Entscheidung.

Diese Analyse ändert nichts am `main`-Zweig und nichts an der laufenden
Praxistest-Woche — sie ist die Grundlage dafür, *nach* der Abnahme mit Messungen
zu entscheiden, was davon sich lohnt.
