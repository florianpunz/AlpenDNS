# ADR-0002: DNS-Wire-Format über `hickory-proto`, nicht selbst geschrieben

**Status:** angenommen · **Datum:** 2026-08-29

## Kontext

Ein DNS-Server muss DNS-Nachrichten lesen und schreiben können. Zwei Wege:

1. Selbst implementieren — RFC 1035 plus rund dreißig weitere RFCs für Record-Typen,
   EDNS(0), Namenskompression, DNSSEC-Record-Typen.
2. Eine bestehende Bibliothek benutzen. In Rust ist das praktisch
   [`hickory-proto`](https://docs.rs/hickory-proto) (Version 0.26, Stand Mai 2026), der
   Protokoll-Layer von Hickory DNS. Die Alternative wäre `domain` (NLnet Labs).

Der Reiz von Variante 1 ist real: man lernt DNS dabei richtig. Der Preis auch. Der
gefährlichste Teil eines DNS-Parsers ist die **Namenskompression** (RFC 1035 §4.1.4) —
Pointer im Nachrichtenkörper, die auf frühere Namen zeigen. Ein Pointer, der auf sich
selbst oder rückwärts in eine Schleife zeigt, ist die klassische DoS-Lücke; sie wurde in
mehr als einer produktiven Implementierung gefunden. Dazu kommen abgeschnittene Pakete,
Längenfelder, die über das Paketende hinausweisen, und Record-Typen mit variabler Struktur.

## Entscheidung

`hickory-proto` für Parsing und Serialisierung. Alles darüber — Cache, Filterung, Policy,
Upstream-Auswahl, Heuristik, API, UI — ist eigener Code.

`hickory-resolver` wird für die Upstream-Transporte (DoT/DoH/DoQ) verwendet, wo es passt.
`hickory-server` wird **nicht** als Rahmen übernommen: unsere Request-Pipeline mit
Decision-Trace ist die eigentliche Substanz des Projekts und soll nicht in fremde
Handler-Traits gepresst werden.

Der `recursor`-Crate von Hickory ist als experimentell markiert. Das ist kein Problem,
weil v1 keine Rekursion macht — aber es ist eine gute Nachricht für den Fall, dass die
Rekursion doch kommt: dann gibt es einen Startpunkt.

## Konsequenzen

* Die riskanteste Klasse von Speicher- und DoS-Bugs liegt in einer Bibliothek, die deutlich
  mehr Augen und deutlich mehr Fuzzing-Stunden gesehen hat als dieses Projekt je bekommen wird.
* Wir sind an deren Datentypen und an deren Release-Zyklus gebunden. Ein Major-Update kann
  Arbeit machen.
* Der Lerneffekt "wie sieht ein DNS-Paket auf dem Draht aus" fehlt. Gegenmittel, falls
  gewünscht: ein eigener Parser als separates, nicht produktives Übungsprojekt, geprüft
  gegen `hickory-proto` als Referenz (Differential Testing). Das ist ein schöner Weg, das
  Format zu lernen, ohne die Sicherheit des Servers daran zu hängen.
* Fuzzing bleibt trotzdem Pflicht — für *unsere* Verwendung der Bibliothek.

## Alternativen

* **`domain` (NLnet Labs):** sauberes Design, aber kleinere Community und weniger fertige
  Transporte für DoH/DoQ.
* **Eigener Parser:** siehe oben. Als Übung ja, als Produktionsbasis nein.
