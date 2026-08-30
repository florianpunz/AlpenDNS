# ADR-0012: `fanout` entfällt — es wird immer genau ein Upstream gefragt

**Status:** angenommen · **Datum:** 2026-08-30 · **Zieht nach sich:** Nachtrag zu [ADR-0009](0009-decision-trace-mit-mutex.md)

## Kontext

`upstream_pool.fanout` bestimmte, wie viele Resolver *gleichzeitig* gefragt
werden. Der Kommentar in der Beispielkonfiguration sagte, was das kostet:
"`>1` kostet Privacy, spart Latenz."

Das ist zu freundlich formuliert. Bei `fanout = 2` sieht jede Anfrage *zwei*
Anbieter statt einem. Damit ist `split_by_zone` aufgehoben — das Feature, dessen
ganzer Zweck ist, dass jeder Anbieter nur einen Bruchteil der Domains sieht
(FEATURES.md P2). Ein Pool mit drei Resolvern und `fanout = 3` schickt jede
Anfrage an alle drei: das ist das Gegenteil dessen, wofür der Pool da ist.

Der Gewinn dagegen ist klein. Gespart wird die Latenz *eines* Upstreams, und auch
das nur bei einem Cache-Miss; der Cache beantwortet den überwiegenden Teil der
Anfragen ohnehin ohne jeden Upstream (BENCHMARKS.md, Phase 2). Für die Ausfälle,
gegen die `fanout` sonst noch helfen könnte, gibt es bereits das passive
Health-Tracking: nach drei Fehlversuchen wird ein Upstream übersprungen, und
schon vorher übernimmt beim ersten Fehlschlag der nächste in der Reihe.

`fanout` hatte außerdem eine Nebenwirkung tief in der Architektur. Es war der
**einzige** Grund, warum der Decision-Trace hinter einem `Mutex` lag: mehrere
gleichzeitige Upstream-Aufgaben wollten in denselben Trace schreiben, und dafür
gibt es keine zwei exklusiven Referenzen (ADR-0009).

## Entscheidung

`fanout` wird entfernt. Der Pool fragt die Upstreams **der Reihe nach**, in der
Reihenfolge, die `split_by_zone` vorgibt, und hört beim ersten Erfolg auf.
`FuturesUnordered` und die Stapelbildung im Pool entfallen; übrig bleibt eine
`for`-Schleife.

**Daraus folgt der Rückbau des Traces.** `ResolveBackend::resolve` nimmt wieder
`&mut Ctx`, `Ctx::record` nimmt `&mut self`, `Ctx::steps` liefert `&[Step]`. Die
Skizze in ARCHITECTURE.md §2 war von Anfang an richtig; nur `fanout` stand ihr im
Weg.

Der Schlüssel bleibt **erkannt**: `UpstreamPool` behält ein privates Feld
`fanout`, das beim Validieren einen Fehler mit Begründung wirft. Ohne das meldete
`deny_unknown_fields` nur "unknown field `fanout`", und wer ihn gesetzt hatte,
wüsste nicht, ob der Server jetzt mehr oder weniger fragt.

## Konsequenzen

* Ein Cache-Miss kostet im schlechtesten Fall die Latenz eines toten Upstreams
  plus die des nächsten, statt das Maximum aus beiden parallel. Das ist der Preis
  und er ist die Latenz eines Timeouts — nicht die einer Anfrage.
* Der `Mutex` im Anfragepfad ist weg. Er kostete kaum etwas (ADR-0009 rechnete
  das vor), aber `Ctx::steps()` klonte bei **jeder** Anfrage den ganzen Vektor,
  weil ein Mutex keine Referenz herausgeben kann. Diese Kopie ist ebenfalls weg.
* Ein Mutex im Anfragepfad wäre irgendwann als "so machen wir das hier" gelesen
  worden. Die Signatur sagt jetzt, was gilt: eine Anfrage, ein Besitzer.
* `Pool::new` und `Pool::with_seed` haben ein Argument weniger.
* Wer `fanout = 1` in der Konfiguration stehen hat — der Default —, muss die
  Zeile beim Update löschen. Unschön, aber B.1 Regel 5 lässt keine still
  ignorierten Schlüssel zu, und ein Schlüssel, der nichts mehr tut, ist genau
  das.

## Alternativen

* **`fanout` behalten und auf 1 festnageln.** Ein Schlüssel mit einem
  zulässigen Wert, der weiterhin `FuturesUnordered` und den `Mutex` im Trace
  rechtfertigt. Das Schlechteste aus beidem.
* **`fanout` nur bei einem Upstream-Ausfall erlauben** ("hedged requests" nach
  einem Zeitfenster). Verteidigbar — die zweite Anfrage geht dann nur raus, wenn
  die erste ohnehin hängt. Aber es ist ein neues Feature mit eigenem Zeitparameter,
  und der Auftrag hieß Fläche verkleinern. Notiert, nicht gebaut.
* **Den Trace vorsichtshalber hinter dem `Mutex` lassen**, falls später etwas
  Nebenläufiges kommt. Das ist Spekulation, und sie kostet bei jeder Anfrage eine
  Kopie des Schritt-Vektors. Kommt der Fall, ist der Weg zurück ein Commit.
