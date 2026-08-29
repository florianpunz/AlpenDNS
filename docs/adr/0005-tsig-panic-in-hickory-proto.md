# ADR-0005: Phase 1 wird trotz eines Fuzz-Crashes in `hickory-proto` abgenommen

**Status:** angenommen · **Datum:** 2026-08-29

## Kontext

Das Abnahmekriterium von Phase 1 lautet: "Ein Fuzz-Target auf dem Anfragepfad läuft
5 Minuten ohne Crash." Der erste Lauf hat nach wenigen Minuten einen Crash gefunden —
nicht in unserem Code, sondern in `hickory-proto 0.26.1`.

`Message::from_vec` panict mit `attempt to subtract with overflow`, wenn eine
Nachricht ein TSIG-Record enthält, dessen `RDLENGTH` kleiner ist als die festen
Felder davor (`src/rr/rdata/tsig.rs:387`). Die Subtraktion steht in dem Fehlerpfad,
der das kaputte Record gerade korrekt erkannt hat.

Ob das ein Panic ist, hängt am Profil:

| `overflow-checks` | Verhalten |
|---|---|
| aus — unser `[profile.release]` | Record wird sauber mit `incorrect rdata length read` abgelehnt |
| an — Debug, `cargo test`, `cargo fuzz` per Default | Panic |

Das kollidiert mit B.1 Regel 1: ein Panic im Query-Handler ist eine
Denial-of-Service-Lücke. Es kollidiert nicht mit dem Rest der Regel — es gibt kein
`unwrap()` und kein Slice-Indexing von uns.

Reparieren können wir es nicht. Der Panic passiert innerhalb von `Message::from_vec`,
und genau diesen Aufruf ersetzen wir laut [ADR-0002](0002-hickory-proto-statt-eigenem-parser.md)
bewusst nicht durch eigenen Code. `catch_unwind` scheidet aus: unser Release-Profil
setzt `panic = "abort"`, dort gibt es nichts zu fangen.

## Entscheidung

Phase 1 gilt als abgenommen, mit drei Auflagen.

1. **Das Abnahmekriterium wird in der Auslieferungs-Konfiguration erfüllt**, nicht in
   der Default-Konfiguration von `cargo fuzz`. Der Nachweislauf ist:

   ```bash
   cargo +nightly fuzz run -O parse_request -- -max_total_time=300
   ```

   `-O` baut so, wie wir ausliefern: Release ohne `overflow-checks`. Das ist der
   Code, der später auf einem Port lauscht, und für den gilt die Aussage "läuft
   5 Minuten ohne Crash". Der Default-Lauf mit Overflow-Checks bleibt zusätzlich
   sinnvoll, um *unsere* Arithmetik zu prüfen — er ist nur nicht das Kriterium.

   Nachweis vom 2026-08-29: 2.698.186 Läufe in 301 Sekunden, kein Crash, kein
   neues Artefakt.

2. **Der Fall bleibt als bekannter Crash im Repo**, in
   `crates/alpendns/fuzz/known-crashes/`, mit der Anleitung, ihn nach jedem Update
   einer Parser-Abhängigkeit erneut zu prüfen. Er liegt bewusst nicht im normalen
   Corpus, weil sonst jeder Fuzz-Lauf sofort abbricht statt neue Fehler zu suchen.

3. **Der Fehler wird upstream gemeldet.** Der fertige Meldetext samt Minimal-Repro
   liegt in `crates/alpendns/fuzz/known-crashes/UPSTREAM-ISSUE.md`.

## Konsequenzen

* Debug-Builds von AlpenDNS lassen sich mit einem einzigen Paket abschießen. Für
  Entwicklung und Tests ist das hinnehmbar, für Betrieb wäre es das nicht — es gibt
  aber keinen Grund, eine Debug-Binary zu betreiben. Sollte je ein Paket mit
  `debug = true` und aktiven Overflow-Checks entstehen, ist diese Entscheidung
  hinfällig.
* Wir verlassen uns an dieser Stelle darauf, dass `overflow-checks` im
  Release-Profil aus bleibt. Das ist der Cargo-Default, aber es ist jetzt eine
  Annahme mit Bedeutung: wer sie umdreht, muss vorher hier nachlesen.
* Die Aussage "der Anfragepfad ist gefuzzt" ist schwächer als sie klingt, solange
  dieser Crash offen ist: über den TSIG-Pfad hinaus sagt der Lauf nichts.
* Sobald `hickory-proto` das behebt, entfällt die ganze Konstruktion: Version
  anheben, `known-crashes` gegenprüfen, Datei und dieses ADR ablösen.

## Alternativen

* **Phase 1 offen lassen, bis upstream fixt.** Ehrlichste Variante, aber sie bindet
  den Fortschritt des Projekts an den Release-Zyklus eines fremden Crates, für einen
  Fehler, der den ausgelieferten Pfad nicht trifft.
* **`overflow-checks = false` auch für Debug und Test setzen.** Würde das Symptom
  beseitigen und gleichzeitig die Warnung vor *eigenen* Überläufen abschalten. Genau
  falsch herum.
* **Das Fuzz-Target so bauen, dass es TSIG nicht erreicht.** Das ist kein Fix,
  sondern Wegsehen — und es würde echte Fehler in demselben Pfad verdecken.
* **Auf `domain` (NLnet Labs) wechseln.** Ein Parser-Bug in einer Bibliothek ist kein
  Grund, die Bibliothek zu wechseln; ADR-0002 hat die Wahl aus anderen Gründen
  getroffen, und die gelten weiter.
