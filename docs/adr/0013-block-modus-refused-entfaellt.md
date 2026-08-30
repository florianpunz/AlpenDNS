# ADR-0013: Der Block-Modus `refused` entfällt

**Status:** angenommen · **Datum:** 2026-08-30

## Kontext

`blocking.mode` kannte vier Werte. Die Dokumentation von `refused` im Code lautete:

> "Ich beantworte das nicht." Ehrlich, aber manche Clients fragen dann den
> nächsten Resolver in ihrer Liste — und der antwortet.

Das ist keine Randnotiz, sondern die Beschreibung eines Block-Modus, der nicht
blockt. RCODE 5 (REFUSED) bedeutet für einen Client: *dieser* Server will nicht,
frag jemand anderen. Genau so verhalten sich Auflöser auch — systemd-resolved,
Android und die meisten Betriebssystem-Auflöser gehen bei REFUSED zum nächsten
konfigurierten Server weiter, während sie NXDOMAIN als endgültige Antwort
akzeptieren. Ein Gerät mit einem zweiten DNS-Eintrag umgeht die Filterung damit
vollständig, und das Log von AlpenDNS zeigt trotzdem einen sauberen Block.

Der teuerste Teil daran ist nicht die fehlende Sperre, sondern dass sie unsichtbar
fehlschlägt. Wer `refused` einstellt, sieht Blockzähler steigen und glaubt, es
funktioniert. Das widerspricht B.1 Regel 6 ("fail closed bei Policy") an genau der
Stelle, an der die Regel gilt.

## Entscheidung

`refused` wird entfernt. Es bleiben `nxdomain` (Default), `zero_ip` und
`sinkhole` — drei Antworten, die der Client als endgültig behandelt.

`BlockMode` bekommt wie `Strategy` (ADR-0011) ein handgeschriebenes
`Deserialize`, das bei `refused` erklärt, warum es weg ist und was stattdessen
gilt.

Der Test `no_mode_answers_with_refused` hält fest, dass **kein** verbleibender
Modus REFUSED erzeugt. Das ist die eigentliche Zusicherung: nicht "der Wert ist
aus dem Enum verschwunden", sondern "der RCODE verlässt den Prozess nicht mehr
als Block-Antwort".

## Konsequenzen

* Wer `refused` konfiguriert hatte, muss beim Update auf `nxdomain` wechseln —
  und hat dann eine Filterung, die auch greift.
* REFUSED bleibt als RCODE anderswo möglich (etwa was ein Upstream schickt); die
  Zusicherung betrifft nur die selbst erzeugten Block-Antworten.
* Es bleiben drei Modi mit sichtbar unterschiedlichem Preis: NXDOMAIN lügt
  freundlich, `zero_ip` kann einen Timeout auslösen, `sinkhole` bricht bei HTTPS
  am Zertifikat ab. Diese Unterschiede sind echte Abwägungen und rechtfertigen
  die Einstellung — im Gegensatz zu einem vierten Wert, der die Funktion abschaltet.

## Alternativen

* **Behalten und in der Dokumentation warnen.** Der Status quo, und die Warnung
  stand bereits da. Sie hat nicht verhindert, dass der Wert wählbar war.
* **Behalten, aber nur zusammen mit einer Prüfung**, dass der Client keinen
  zweiten Resolver hat. Das kann ein DNS-Server nicht wissen.
