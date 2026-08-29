# ADR-0009: Der Decision-Trace sammelt über einen Mutex und trägt Namen statt IDs

**Status:** angenommen · **Datum:** 2026-08-29 · **Verfeinert:** [ARCHITECTURE.md §2](../ARCHITECTURE.md)

## Kontext

ARCHITECTURE.md §2 skizziert den Trace als `SmallVec<[Step; 8]>` mit Schritten,
die auf IDs verweisen (`AllowlistHit { list: ListId, rule: RuleRef }`). Beim
Umsetzen in Phase 5 stellten sich zwei Fragen, die die Skizze offen lässt.

**Erstens: wie kommt der Trace durch die Pipeline?** Die naheliegende Antwort ist
eine exklusive Referenz — `resolve(&self, request, ctx: &mut Ctx)`. Das
funktioniert bis zum Upstream-Pool: bei `fanout > 1` fragt er mehrere Resolver
**gleichzeitig**, und jede dieser Aufgaben will ihren Schritt eintragen. Zwei
exklusive Referenzen auf denselben Trace gibt es nicht.

**Zweitens: wer löst die IDs auf?** Ein Schritt mit `ListId(3)` ist ohne die
Konfiguration daneben nicht lesbar. Genau das braucht aber `alpendns policy test`
— und ab Phase 6 die UI, die "warum wurde das geblockt?" beantworten soll, ohne
dass jemand eine Nachschlagetabelle mitliefert.

## Entscheidung

**Der Trace liegt hinter einem `Mutex` und wird als `&Ctx` geteilt**, nicht als
`&mut Ctx` durchgereicht.

**Die Schritte tragen `Arc<str>` mit dem Namen**, nicht die ID: `BlocklistHit {
list: Arc<str>, line: u32, matched: String }`. Ein Trace ist damit für sich allein
lesbar.

Statt `SmallVec` wird ein `Vec::with_capacity(8)` benutzt. Das ist eine Allokation
pro Anfrage — dieselbe Größenordnung wie das Klonen der Nachricht, das ohnehin
passiert, und es spart ein Dependency.

## Konsequenzen

* Ein unumkämpfter Mutex kostet einige Nanosekunden; eine Anfrage aus dem Cache
  dauert 28 µs (BENCHMARKS.md). Bei fünf Schritten liegt der Anteil unter einem
  Promille. Die Alternative wäre gewesen, `fanout > 1` vom Trace auszunehmen —
  also ausgerechnet den Fall nicht zu erklären, in dem mehrere Upstreams
  beteiligt sind.
* Der Trace ist nach der Anfrage sofort verwendbar, ohne Registry. `explain()`
  liefert die Begründungskette als Text, und die CLI gibt sie unverändert aus.
* `Arc<str>`-Klone je Schritt statt `Copy`-IDs. Ein `Arc`-Klon ist ein
  atomarer Zähler; gegenüber `matched: String`, das ohnehin allokiert, fällt das
  nicht ins Gewicht.
* **Der Trace enthält Query-Namen.** Das ist beabsichtigt und der Grund, warum er
  laut ARCHITECTURE.md §2 unabhängig vom Log-Modus entsteht: erst die
  Logging-Schicht entscheidet, was mit ihm passiert. Bis diese Schicht in Phase 6
  existiert, darf ihn niemand ins Log schreiben — die Stellen, die heute loggen,
  schreiben nur Listennamen und Zeilennummern (B.1 Regel 3).

## Alternativen

* **`&mut Ctx` durchreichen.** Null Kosten, scheitert aber am nebenläufigen
  Fanout. Ein Sonderweg für diesen einen Fall wäre schlechter als ein Mutex
  überall.
* **Jede Schicht gibt ihre Schritte zurück und der Aufrufer fügt zusammen.**
  Ändert jede Signatur der Pipeline und verteilt die Reihenfolge auf alle
  Schichten, statt sie an einer Stelle zu haben.
* **IDs mit Registry.** Spart Speicher pro Schritt und kostet jedem Leser des
  Traces eine Abhängigkeit auf die Konfiguration, aus der er stammt. Für ein
  Projekt, dessen Zweck Erklärbarkeit ist, der falsche Tausch.
