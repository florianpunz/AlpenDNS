# Bekannte Crashes

Eingaben, die dieses Projekt gefunden hat und die noch nicht behoben sind. Sie
liegen **nicht** im normalen Corpus: ein Fuzz-Lauf würde sonst sofort abbrechen,
statt neue Fehler zu suchen.

Nach jedem Update einer Parser-Abhängigkeit hiergegen laufen lassen:

```bash
cargo +nightly fuzz run parse_request fuzz/known-crashes/parse_request
```

Läuft das ohne Abbruch durch, ist der Fehler weg und die Datei kann samt dieses
Abschnitts verschwinden. Der von libFuzzer selbst erzeugte Corpus unter
`fuzz/corpus/` ist Wegwerfmaterial und nicht im Repo.

---

## `parse_request/tsig-rdata-length-underflow`

Gefunden am 2026-08-29 im ersten Fuzz-Lauf der Phase 1.
Bewertung und Entscheidung: [ADR-0005](../../../../docs/adr/0005-tsig-panic-in-hickory-proto.md).

**Was passiert:** `Message::from_vec` panict beim Parsen eines TSIG-Records,
dessen RDLENGTH kleiner ist als die festen Felder davor. In
`hickory-proto 0.26.1`, `src/rr/rdata/tsig.rs:387`, wird
`end_idx - decoder.index()` gerechnet — ausgerechnet in dem Fehlerpfad, der das
kaputte Record gerade korrekt erkannt hat. Die Subtraktion läuft unter Null.

**Wirkung hängt am Profil:**

| Profil | Verhalten |
|---|---|
| `overflow-checks = false` (unser Release-Profil) | Record wird sauber mit `incorrect rdata length read` abgelehnt |
| `overflow-checks = true` (Debug, `cargo test`, Fuzzing mit `-a`) | Panic |

**Der Fehler liegt in der Bibliothek, nicht in AlpenDNS.** Der Panic passiert
innerhalb von `Message::from_vec` — genau dem Aufruf, den wir laut
[ADR-0002](../../../../docs/adr/0002-hickory-proto-statt-eigenem-parser.md)
bewusst nicht selbst ersetzen.

Meldetext für upstream: [`UPSTREAM-ISSUE.md`](UPSTREAM-ISSUE.md).
