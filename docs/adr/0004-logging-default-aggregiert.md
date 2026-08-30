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
| `aggregate` | Zähler + Domain-Häufigkeiten in einer Tabelle ohne Namen; eine Domain erscheint erst ab `aggregate_k` Treffern (Default 5) in irgendeiner Ausgabe | Default |
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
* Mehr Implementierungsaufwand als ein simples Logfile: Zählertabelle, Ringpuffer,
  Schwellwertlogik.
* Für den Fall, dass jemand echtes Query-Logging braucht (Firmenumfeld, Forensik), ist
  `full` da — als Entscheidung, die in der Konfigurationsdatei sichtbar ist und in der UI
  angezeigt wird, nicht als stiller Default.

## Alternativen

* **Logging per Default an, Retention kurz:** üblich, aber die Datei existiert trotzdem.
* **Nur `none` und `full`:** einfacher, aber dann schaltet in der Praxis jeder `full` ein,
  weil er sonst nichts sieht — und lässt es an.

---

## Nachtrag, 2026-08-30: die Zählstruktur, nicht die Entscheidung

Die vier Modi und der Default bleiben. Ausgetauscht ist nur, *womit* `aggregate`
zählt: statt eines Count-Min-Sketch eine exakte Tabelle unter einem gesalzenen
Hash. Der Sketch war bei realistischem Verkehr so ungenau, dass die
Top-Domain-Ausgabe leer blieb. Zahlen und Begründung:
[ADR-0015](0015-exakte-zaehlung-statt-sketch.md).
