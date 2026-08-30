# ADR-0019: Heuristiken melden, sie blocken nicht

**Status:** angenommen · **Datum:** 2026-08-30 · **Betrifft:** [FEATURES.md](../FEATURES.md) D1–D6, [ROADMAP.md](../ROADMAP.md) Phase 8

## Kontext

Phase 8 bringt fünf Detektoren: DGA, Tunneling, Rebinding, Typosquatting und
neu registrierte Domains. Vier davon sind Heuristiken im eigentlichen Sinn — sie
raten, begründet, aber sie raten.

Die Beispielkonfiguration zeigte `tunneling` und `rebinding` von Anfang an auf
`block`. Die Roadmap sagt für dieselbe Phase "alles per Default nur `flag`", und
CLAUDE.md B.8 verlangt ausdrücklich eine Rückfrage, bevor ein Detektor auf
`block` steht. Es musste eine Seite gewinnen.

## Entscheidung

**Alle fünf stehen per Default auf `flag`.** Sie melden, sie blocken nicht. Die
Beispielkonfiguration ist angeglichen worden, nicht der Code.

Der Grund steht in FEATURES.md, Abschnitt D, und er ist keine Vorsicht um der
Vorsicht willen: *ein Detektor, der Internet kaputtmacht, wird abgeschaltet — und
mit ihm alle anderen.* Wer einmal erlebt hat, dass die Bank nicht lädt, schaltet
`[detection]` als Ganzes ab und kommt nicht wieder. Die vier Heuristiken sind
zusammen mehr wert als jede einzelne, und der Weg dorthin führt über eine
Beobachtungswoche.

### Vier Stufen und nicht zwei

`off` · `log` · `flag` · `block`. Der Unterschied zwischen `log` und `flag` ist
der einzige, der Erklärung braucht: beide zählen mit und stehen im Trace, aber
nur `flag` stellt die Anfrage in die Liste, die sich jemand ansieht. Wer eine
Heuristik erst kennenlernen will, nimmt `log`; wer sie beurteilen will, `flag`.

### Die Schwellen stehen im Code, nicht in der Konfiguration

Jeder Detektor bringt sein `DEFAULT_THRESHOLD` selbst mit. Eine Schwelle ergibt
nur zusammen mit der Rechnerei einen Sinn, die den Score erzeugt — sie in eine
Beispieldatei zu schreiben und dort zu pflegen hieße, zwei Dinge synchron zu
halten, die zusammengehören. Die Konfiguration kann sie überschreiben; der
Default kommt von dort, wo er begründbar ist.

Alle Schwellen sind **gemessen und nicht geraten**. Wie, steht in
[BENCHMARKS.md](../BENCHMARKS.md); womit, in
`crates/alpendns/tests/detect_corpus.rs`.

### Zwei Traits, weil es zwei Stellen in der Pipeline sind

`NameDetector` sieht die Frage, `AnswerDetector` die Antwort. Der
Rebinding-Schutz prüft, ob eine private Adresse für einen öffentlichen Namen
zurückkommt — dafür muss die Auflösung schon gelaufen sein (ARCHITECTURE.md §1,
Schicht 5). Ein gemeinsamer Trait mit einem `Option<&Message>` würde
verschweigen, dass die beiden an verschiedenen Stellen laufen, und die erste
Verwechslung wäre ein Detektor, der immer `None` bekommt und nie etwas findet.

### Die Heuristiken stehen zuletzt

Reihenfolge in `Engine::evaluate`: befristete Freigabe → Allowlist → befristete
Sperre → Blocklisten → Regex → Zeitplan → **Heuristiken**.

Sie sind das unschärfste Mittel im Haus, und alles, was eine klare Regel
entscheiden kann, soll vorher entschieden sein. Sonst stünde im Trace ein Score,
wo eine Zeile aus einer Liste hingehört. Eine Freigabe schlägt jeden Detektor:
wer eine Domain ausdrücklich erlaubt hat, will sie erreichen, egal was ein Score
dazu sagt.

### Ein Detektor ohne seine Daten wird gar nicht erst eingehängt

Der Typosquat-Wächter braucht eine Schutzliste, der NRD-Detektor eine Datei.
Fehlen sie, erscheint der Detektor im Status als `off` statt als eingeschaltet.
Ein Detektor, der eingeschaltet aussieht und nichts finden kann, ist eine
Zusicherung, die nicht eintritt (B.1 Regel 5).

### `forward_zone` ist automatisch vom Rebinding-Schutz ausgenommen

Wer eine Zone ausdrücklich ins eigene Netz leitet, hat damit schon gesagt, dass
private Adressen von dort in Ordnung sind. Ohne diese Ergänzung wäre der
Rebinding-Schutz beim ersten Start eine Falle: der LAN-Nameserver antwortet für
`home.arpa` naturgemäß mit `192.168.x.y`, und das ist genau der Treffer, auf den
der Detektor wartet.

## Was das kostet

**Zustand im Anfragepfad.** Die Tunneling-Erkennung ist der erste Teil des
Projekts, der sich etwas über Anfragen hinaus merkt: je Zone ein Zeitfenster mit
ein paar Zählern. Das ist unvermeidlich — der ganze Witz von D2 ist, *pro Zone*
statt pro Anfrage zu bewerten. Begrenzt ist es zweifach: höchstens 4096 Zonen,
höchstens 256 gemerkte Subdomains je Zone, und beides fällt nach dem Zeitfenster
weg. Gespeichert werden **gesalzene Hashes** der Subdomains, nicht die Namen.

**Ein Modell im Binary.** 107 KiB Tabelle für die DGA-Erkennung. Herkunft,
Lizenz und Erzeugung stehen in `src/detect/dga/model.bin.md`.

**Namen in den Begründungen.** Ein `Finding` trägt den Query-Namen — es soll ihn
tragen, sonst ist ein Fehlalarm nicht debugbar (FEATURES.md D6). Damit ist die
Begründung die neueste Stelle, an der ein Name entkommen könnte. Sie unterliegt
denselben Regeln wie alles im Trace (B.1 Regel 3), und der Leck-Test
`no_query_name_leaves_the_process_in_the_quiet_modes` durchsucht seit Phase 8
auch die Liste der auffälligen Anfragen.

## Umkehrbedingung

Nach der Beobachtungswoche aus dem Abnahmekriterium: wer die Fehlalarm-Liste
durchgesehen und die Ausnahmen gesetzt hat, darf einzelne Detektoren auf `block`
stellen. Der naheliegende erste ist `rebinding` — er ist als einziger keine
Heuristik, sondern eine Ja-Nein-Regel, und seine Fehlalarme sind benannt und über
`allow_zones` behebbar.

Dieses ADR steht dem nicht entgegen: es legt den **Auslieferungszustand** fest,
nicht das, was jemand für sein Netz einstellt.
