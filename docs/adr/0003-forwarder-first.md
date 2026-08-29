# ADR-0003: Forwarder in v1, Rekursion offen gelassen — aber nicht vorgebaut

**Status:** angenommen · **Datum:** 2026-08-29

## Kontext

Ein DNS-Server kann Namen auf zwei Arten auflösen:

* **Forwarding:** die Anfrage an einen anderen Resolver weiterreichen (Quad9, Mullvad,
  dnsforge, …) und dessen Antwort ausliefern.
* **Rekursion:** selbst bei den Root-Servern anfangen, sich über TLD-Server zu den
  autoritativen Servern der Zone durchhangeln, DNSSEC validieren.

Rekursion ist aus Privacy-Sicht attraktiv: kein Anbieter sieht die vollständige Anfrage,
weil jeder Server auf dem Weg nur seinen Teil des Namens erfährt (verstärkt durch
QNAME-Minimisation, RFC 9156). Sie ist aber auch deutlich mehr Arbeit: Delegation-Handling,
Glue-Records, CNAME-Ketten über Zonengrenzen, kaputte autoritative Server, Loop-Erkennung,
DNSSEC-Validierung mit Trust-Anchor-Rollover, Root-Hints-Pflege, negatives Caching nach
RFC 2308. Und sie hat eigene Privacy-Nachteile: die eigene IP kontaktiert jeden
autoritativen Server direkt, was für ein Heimanschluss-IP mit einem einzelnen Nutzer eine
sehr eindeutige Signatur ist.

Der Autor hat ausdrücklich gesagt, dass Rekursion möglicherweise nie kommt.

## Entscheidung

v1 ist ein Forwarder. Es gibt **genau einen** Vorbau für eine spätere Rekursion: das
Auflösen liegt hinter einem Trait `ResolveBackend` mit einer Implementierung
(`ForwardBackend`). Mehr nicht.

Was das ausdrücklich **nicht** heißt:

* keine Root-Hints-Verwaltung "für später"
* keine Delegation-Datenstrukturen, die niemand benutzt
* keine Konfigurationsschlüssel für Rekursion
* keine Traits mit einer Implementierung an anderen Stellen "für Symmetrie"

Falls die Rekursion je kommt, ist der wahrscheinliche Weg nicht "selbst schreiben",
sondern `hickory-recursor` hinter demselben Trait — dann ist es eine Wochenendaufgabe
statt eines Quartals.

## Konsequenzen

* v1 ist in Wochen erreichbar statt in Monaten. Ein laufender Server, der etwas Nützliches
  tut, ist mehr wert als ein halbfertiger perfekter.
* Das Vertrauen verschiebt sich vom Provider zum Upstream-Resolver. Das ist ein realer
  Nachteil, und die Antwort darauf ist nicht Rekursion, sondern das Splitting über mehrere
  Upstreams (siehe FEATURES.md P2) — das nimmt jedem einzelnen Upstream das vollständige
  Bild, ohne die eigene IP bei jedem autoritativen Server der Welt zu hinterlassen.
* QNAME-Minimisation bringt im Forwarder-Modus nichts; sie greift nur bei eigener
  Rekursion. Das gehört ehrlich in die Dokumentation, statt als Feature aufgeführt zu werden.
* Eigene DNSSEC-Validierung ist auch im Forwarder-Modus möglich und sinnvoll (dem AD-Bit
  des Upstreams zu glauben ist wertlos). Sie ist als eigener Punkt in Phase 7 notiert und
  hängt nicht an dieser Entscheidung.

## Alternativen

* **Rekursion in v1:** höheres Privacy-Ideal, erheblich höheres Risiko, das Projekt nie
  fertig zu bekommen.
* **Beides parallel von Anfang an:** doppelte Testmatrix ab Tag eins, für einen Modus,
  den vielleicht niemand einschaltet.
