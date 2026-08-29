# Fuzz-Seeds

Von Hand gepflegte Eingaben. Gefundene Crashes werden hier eingecheckt und damit
zu Regressionstests (docs/TESTING.md §3). Der von libFuzzer selbst erzeugte Corpus
unter `fuzz/corpus/` ist dagegen Wegwerfmaterial und nicht im Repo.

Mitlaufen lassen:

```bash
cargo +nightly fuzz run parse_request fuzz/corpus/parse_request fuzz/seeds/parse_request
```

## `parse_request/tsig-rdata-length-underflow`

Gefunden am 2026-08-29, erster Fuzz-Lauf der Phase 1.

**Was passiert:** `Message::from_vec` panict beim Parsen eines TSIG-Records mit
einer RDATA-Längenangabe, die kleiner ist als das, was bis dahin schon gelesen
wurde. In `hickory-proto 0.26.1`, `src/rr/rdata/tsig.rs:387`, wird
`end_idx - decoder.index()` gerechnet — im Fehlerpfad, der das kaputte Record
gerade korrekt erkannt hat. Die Subtraktion läuft unter Null.

**Wirkung:**

* Release-Build (`overflow-checks` aus, so wird ausgeliefert): kein Panic, das
  Record wird sauber mit `incorrect rdata length read` abgelehnt. Der überlaufene
  Wert landet nur in der Fehlermeldung.
* Debug-Build und `cargo test` (`overflow-checks` an): Panic, also ein
  Denial-of-Service für jeden, der eine Debug-Binary laufen lässt.

**Der Fehler liegt in der Bibliothek, nicht in AlpenDNS.** Auf unserer Seite gibt
es nichts zu reparieren: der Panic passiert innerhalb von `Message::from_vec`,
und das ist der Aufruf, den wir laut ADR-0002 bewusst nicht selbst ersetzen.

**Deshalb ist dieses Target derzeit nicht crashfrei**, und das
Abnahmekriterium von Phase 1 ("5 Minuten ohne Crash") ist offen. Offene Punkte:
Upstream melden, und danach entweder auf eine gefixte Version warten oder
begründen, warum der Release-Pfad ausreicht.
