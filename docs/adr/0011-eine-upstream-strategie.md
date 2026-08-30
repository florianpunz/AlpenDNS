# ADR-0011: `split_by_zone` ist die einzige Upstream-Strategie

**Status:** angenommen · **Datum:** 2026-08-30 · **Ersetzt einen Teil von:** [ARCHITECTURE.md §5](../ARCHITECTURE.md)

## Kontext

`upstream_pool.strategy` kannte drei Werte. ARCHITECTURE.md §5 beschreibt sie und
disqualifiziert zwei davon im selben Atemzug:

* `fastest` — "schnell, aber ein Resolver sieht am Ende fast alles",
* `round_robin` — "jeder Upstream lernt trotzdem irgendwann alles",
* `split_by_zone` — "**der interessante Fall**".

Eine Einstellung, deren Dokumentation zwei ihrer drei Werte als untauglich für den
Zweck des Projekts bezeichnet, ist keine Einstellung, sondern eine Falle. Wer
`fastest` wählt, weil es "schnell" klingt, hebt das Feature auf, für das AlpenDNS
gebaut ist (FEATURES.md P2). Der Preis dafür ist eine niedrigere Latenz zu einem
Upstream, den ein Cache-Treffer ohnehin überspringt.

Dazu kommt: die drei Strategien waren nicht gleich teuer im Code. `fastest` brauchte
eine Sortierung nach EWMA (`strategy::by_latency`), `round_robin` einen
Reihum-Zeiger im Pool (`next: AtomicUsize`) — beides ausschließlich für Werte, die
niemand mit Verstand einstellt.

## Entscheidung

`fastest` und `round_robin` werden entfernt. `split_by_zone` bleibt als einziger
Wert von `upstream_pool.strategy`.

Beide Namen bleiben in der Konfiguration **erkannt**: `Strategy` bekommt ein
handgeschriebenes `Deserialize`, das bei `fastest` und `round_robin` sagt, was
stattdessen gilt. Ein abgeleitetes `Deserialize` hätte nur "unknown variant"
gemeldet, und eine Konfiguration von gestern hätte den Betreiber raten lassen.

`strategy::round_robin` bleibt als Funktion, weil `by_zone` die Ausweichwege
hinter den zuständigen Upstream reiht. `by_latency` ist weg.

Die EWMA der Antwortzeit bleibt ebenfalls: sie speist
`alpendns_upstream_rtt_seconds` und die Statusanzeige. Sie steuert nur nicht mehr
die Auswahl.

## Konsequenzen

* Der `strategy`-Schlüssel hat jetzt genau einen zulässigen Wert. Das ist ein
  offener Rest — siehe Alternativen.
* Die Tests zur Ausfallerkennung im Pool liefen bisher über `round_robin`, weil
  dort feststeht, wer zuerst gefragt wird. Mit `split_by_zone` hängt das am Seed;
  die Tests suchen sich deshalb über `seed_starting_at` einen Seed, bei dem der
  Testname beim gewünschten Upstream landet. Umständlicher, aber es prüft, was
  im Betrieb tatsächlich läuft.
* Wer bisher `fastest` fuhr, bekommt nach dem Update höhere Latenz zu einem
  Teil der Domains und dafür die Aufteilung, wegen der er den Resolver
  installiert hat.
* Ein Pool mit *einem* Resolver verhält sich unverändert: `zone_index` liefert
  bei `count <= 1` immer 0.

## Alternativen

* **Beide Strategien lassen und in der Dokumentation abraten.** Der Status quo.
  Er kostet Code, den niemand einstellen sollte, und lädt genau zu der
  Fehlkonfiguration ein, die das Feature aushebelt.
* **`strategy` ganz streichen.** Konsequenter: ein Schlüssel mit einem
  zulässigen Wert ist keine Wahl. Nicht gemacht, weil der Auftrag ausdrücklich
  nur die beiden Werte nannte und `deny_unknown_fields` sonst jede bestehende
  Konfiguration mit `strategy = "split_by_zone"` beim Start abweist — für einen
  Gewinn von einer Zeile. Der Kandidat ist notiert, nicht ausgeführt.
* **`fastest` als Testkonstrukt behalten**, um es gegen `split_by_zone` zu
  messen. Geprüft: `tests/load.rs` benutzt es nicht. Eine Vergleichsmessung, die
  niemand fährt, ist toter Code mit Zeremonie.
