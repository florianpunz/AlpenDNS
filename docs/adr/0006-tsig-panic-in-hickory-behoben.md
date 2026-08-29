# ADR-0006: Der TSIG-Panic ist upstream behoben — wir warten auf 0.27 statt zu melden

**Status:** angenommen · **Datum:** 2026-08-29 · **Löst ab:** [ADR-0005](0005-tsig-panic-in-hickory-proto.md)

## Kontext

[ADR-0005](0005-tsig-panic-in-hickory-proto.md) hat Phase 1 abgenommen, obwohl der
Fuzzer einen Panic in `hickory-proto 0.26.1` gefunden hatte
(`src/rr/rdata/tsig.rs:387`, `end_idx - decoder.index()` läuft unter Null bei einem
TSIG-Record mit zu kleiner `RDLENGTH`). Die dritte Auflage dort lautete: upstream melden.

Vor dem Melden wurde geprüft, ob der Fehler dort schon bekannt ist. Ergebnis:

* **In `main` tritt er nicht mehr auf.** Das Minimal-Repro läuft dort mit aktiven
  Overflow-Checks durch. Nachgewiesen am 2026-08-29 gegen
  `hickory-proto 0.27.0-alpha.1` (`6c18ce30`) als direkte Git-Abhängigkeit.
* **Der Grund ist ein Umbau, keine gezielte Korrektur.** `end_idx` wird in `main`
  aus `decoder.len() + decoder.index()` gebildet statt aus `RDLENGTH`. Damit ist
  `decoder.index() <= end_idx` strukturell erfüllt. Die beiden Subtraktionen stehen
  unverändert im Code, können aber nicht mehr unterlaufen.
* **Es gibt keinen Release mit dem Fix.** `0.26.1` vom 2026-05-01 ist der neueste
  stabile Stand; `main` ist eine Alpha auf dem Weg zu 0.27.

## Entscheidung

Die dritte Auflage aus ADR-0005 entfällt: **es wird kein Issue eröffnet.** Ein Bericht
über einen Fehler, der im Entwicklungszweig bereits weg ist, kostet die Maintainer
Zeit und uns auch.

Die ersten beiden Auflagen aus ADR-0005 gelten unverändert weiter:

1. Das Abnahmekriterium von Phase 1 wird in der Auslieferungs-Konfiguration erfüllt
   (`cargo +nightly fuzz run -O parse_request`). Nachweis vom 2026-08-29: 2.698.186
   Läufe in 301 Sekunden, kein Crash, kein neues Artefakt.
2. Der Fall bleibt als bekannter Crash in `crates/alpendns/fuzz/known-crashes/`.

Dazu kommt die Bedingung, die diesen Zustand beendet:

3. **Beim Erscheinen von `hickory-proto 0.27` wird aktualisiert und gegengeprüft:**

   ```bash
   cargo +nightly fuzz run parse_request fuzz/known-crashes/parse_request
   ```

   Läuft das ohne Abbruch durch, verschwinden `known-crashes/`, dieses ADR und
   ADR-0005 gemeinsam. Auf eine Alpha wird dafür **nicht** vorgezogen: ein
   Resolver, der Pakete aus dem Netz parst, hängt nicht an einem
   Vorab-Release, um einen Fehler zu vermeiden, der den ausgelieferten Pfad nicht
   trifft.

## Konsequenzen

* Der Zustand ist jetzt zeitlich begrenzt und an ein konkretes Ereignis geknüpft,
  statt unbefristet zu gelten. Das war die eigentliche Schwäche von ADR-0005.
* Bis dahin bleibt es dabei: Debug-Builds lassen sich mit einem Paket abschießen,
  Release-Builds nicht. Wer eine Debug-Binary betreibt, hat ein anderes Problem.
* Wir tragen das Risiko, dass 0.27 lange auf sich warten lässt. `0.26.0` kam im
  April 2026, ein Jahr Abstand wäre nicht ungewöhnlich. Sollte die Alpha vorher aus
  anderen Gründen attraktiv werden, ist das eine eigene Entscheidung mit eigenem ADR.
* Der vorbereitete Meldetext entfällt. Er steht in der Historie, falls die
  Einschätzung sich ändert:
  `git show 8f878c1:crates/alpendns/fuzz/known-crashes/UPSTREAM-ISSUE.md`

## Alternativen

* **Trotzdem melden.** Denkbar, damit `0.26.x` einen Backport bekommt. Für einen
  Fehler, der nur mit aktiven Overflow-Checks zuschlägt, ist ein Backport aber
  unwahrscheinlich, und der Aufwand läge bei Leuten, die ihn schon behoben haben.
* **Jetzt auf `0.27.0-alpha.1` wechseln.** Löst den Fehler sofort und handelt sich
  eine instabile API im Parser eines Netzwerkdienstes ein. Falscher Tausch.
* **Eigener Fork mit Einzeiler-Fix per `[patch.crates-io]`.** Technisch sauber, aber
  wir würden einen Fork pflegen, `cargo deny` müsste eine Git-Quelle erlauben
  (`unknown-git = "deny"`), und das alles für einen Fehler, der uns im Betrieb nicht
  trifft.
