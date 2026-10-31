# Bekannte Crashes

Eingaben, die dieses Projekt gefunden hat und die noch nicht behoben sind. Sie
liegen **nicht** im normalen Corpus: ein Fuzz-Lauf würde sonst sofort abbrechen,
statt neue Fehler zu suchen. Derzeit ist keiner offen.

Sobald wieder ein Fall hier liegt: nach jedem Update einer Parser-Abhängigkeit
hiergegen laufen lassen:

```bash
cargo +nightly fuzz run parse_request fuzz/known-crashes/parse_request
```

Läuft das ohne Abbruch durch, ist der Fehler weg und die Datei kann samt dieses
Abschnitts verschwinden. Der von libFuzzer selbst erzeugte Corpus unter
`fuzz/corpus/` ist Wegwerfmaterial und nicht im Repo.

Der letzte Fall, der TSIG-RDLENGTH-Underflow, wurde am 2026-10-31 mit
hickory-proto 0.26.3 geschlossen; die Bewertung steht in
[ADR-0006](../../../../docs/adr/0006-tsig-panic-in-hickory-behoben.md).
