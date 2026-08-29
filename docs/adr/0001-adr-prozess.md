# ADR-0001: Architekturentscheidungen werden festgehalten

**Status:** angenommen · **Datum:** 2026-08-29

## Kontext

Bei einem Projekt, das über Monate nebenher entsteht und an dem ein Coding-Agent
mitarbeitet, geht das *Warum* einer Entscheidung schneller verloren als das *Was*. Der
Agent hat kein Gedächtnis über Sessions hinweg, und in sechs Monaten hat der Autor es
auch nicht mehr.

Das teuerste Muster dabei: eine bewusst getroffene Entscheidung wird später als
"komischer Code" wahrgenommen und wegrefaktoriert.

## Entscheidung

Jede Entscheidung, die schwer rückgängig zu machen ist oder von außen falsch aussieht,
bekommt ein kurzes Dokument in `docs/adr/`, durchnummeriert.

Format: Kontext, Entscheidung, Konsequenzen, Alternativen. Eine Seite, nicht mehr.
Ein ADR wird nicht geändert, sondern durch ein neues ersetzt (`Status: abgelöst durch ADR-00xx`).

Wann ein ADR fällig ist:

* Wahl oder Wechsel einer zentralen Abhängigkeit
* Datenstruktur im heißen Pfad
* alles, was das Konfigurationsformat oder die Wire-Kompatibilität betrifft
* jede bewusste Abweichung von einem RFC
* Lizenz

Kein ADR für: Formatierung, Benennung, alles, was in einem Nachmittag zurückgebaut ist.

## Konsequenzen

Etwas Schreibarbeit bei jeder größeren Entscheidung. Dafür kann jede neue Agent-Session
in `docs/adr/` nachlesen, warum etwas so ist, statt es "aufzuräumen".
