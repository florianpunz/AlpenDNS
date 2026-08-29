# ADR-0004: Aggregiertes Logging als Default, Query-Log nur auf Ansage

**Status:** angenommen · **Datum:** 2026-08-29

## Kontext

Jeder DNS-Server im LAN sieht die vollständige Browsing-Historie jedes Geräts. Übliche
Lösungen loggen das per Default, weil die Statistik-Ansicht das Verkaufsargument ist:
"Top-Domains", "Queries pro Client", "letzte 24 Stunden".

Damit liegt auf einer Kiste im Keller eine Datei, die mehr über einen Haushalt aussagt
als die meisten anderen Daten im Netz — durchsuchbar, kopierbar, beschlagnahmbar,
und bei einer Kompromittierung des Geräts sofort abgreifbar.

Ohne jede Aufzeichnung ist der Server aber praktisch nicht bedienbar: "warum geht diese
Seite nicht mehr" braucht Kontext.

## Entscheidung

Vier Modi, Default `aggregate`:

| Modus | Was gespeichert wird | Wofür |
|---|---|---|
| `none` | nur globale Zähler | Maximum an Zurückhaltung |
| `aggregate` | Zähler + Domain-Häufigkeiten in einem Count-Min-Sketch; eine Domain erscheint erst ab `aggregate_k` Treffern (Default 5) in irgendeiner Ausgabe | Default |
| `ring` | zusätzlich die letzten `ring_seconds` (Default 300) im RAM-Ringpuffer, nie auf Platte | Debugging |
| `full` | zusätzlich strukturierte Zeilen auf Platte | bewusste Entscheidung des Betreibers |

Die k-Anonymitätsschwelle ist der entscheidende Teil: eine einmalig aufgerufene Domain
taucht nirgends auf. Genau die einmaligen Aufrufe sind die verräterischen.

Der aktive Modus wird in der UI dauerhaft angezeigt, nicht in einem Einstellungsdialog
versteckt.

## Konsequenzen

* "Zeig mir alle Anfragen von gestern" geht per Default nicht. Das ist der Punkt.
* Die Debugging-Erfahrung bleibt trotzdem gut, weil der Decision-Trace unabhängig vom
  Log-Modus existiert (siehe ARCHITECTURE.md §2) und im `ring`-Modus fünf Minuten lang
  im RAM abfragbar ist. Für "was ist gerade passiert" reicht das fast immer.
* Mehr Implementierungsaufwand als ein simples Logfile: Count-Min-Sketch, Ringpuffer,
  Schwellwertlogik.
* Für den Fall, dass jemand echtes Query-Logging braucht (Firmenumfeld, Forensik), ist
  `full` da — als Entscheidung, die in der Konfigurationsdatei sichtbar ist und in der UI
  angezeigt wird, nicht als stiller Default.

## Alternativen

* **Logging per Default an, Retention kurz:** üblich, aber die Datei existiert trotzdem.
* **Nur `none` und `full`:** einfacher, aber dann schaltet in der Praxis jeder `full` ein,
  weil er sonst nichts sieht — und lässt es an.
