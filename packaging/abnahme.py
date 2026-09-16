#!/usr/bin/env python3
"""Wertet das Query-Log einer Beobachtungswoche aus.

Liest die JSON-Zeilen aus `[privacy.logging]` und zählt je Detektor, wie oft er
angeschlagen hat — mit den Namen, weil sich ein Fehlalarm nur am Namen beurteilen
lässt und nicht an seiner Zahl.

    abnahme.py [AB] [LOG]

`AB` ist ein Zeitstempel im Format des Logs (`2026-09-16T16:24`); gezählt wird ab
dort. Ohne Argument wird die ganze Datei gelesen. `LOG` überschreibt den Pfad,
damit sich auch eine gesicherte Kopie auswerten lässt.

Warum es das gibt: `/api/flagged` liest den Ringpuffer, und der ist nur
`ring_seconds` tief. Für die Auswertung einer Woche braucht es `mode = "full"`
und dieses Skript. Siehe docs/OPERATIONS.md §6.
"""

import collections
import json
import sys

CUT = sys.argv[1] if len(sys.argv) > 1 else "1970-01-01T00:00"
LOG = sys.argv[2] if len(sys.argv) > 2 else "/var/log/alpendns/queries.jsonl"

total = 0
first = last = None
names = set()
per_detector = collections.Counter()
by_detector = collections.defaultdict(collections.Counter)

with open(LOG, encoding="utf-8", errors="replace") as handle:
    for line in handle:
        line = line.strip()
        if not line:
            continue
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        at = entry.get("at", "")
        if at < CUT:
            continue
        total += 1
        if first is None:
            first = at
        last = at
        if entry.get("name"):
            names.add(entry["name"])
        for finding in entry.get("findings", []):
            detector = finding.get("detector")
            per_detector[detector] += 1
            by_detector[detector][(entry.get("name"), finding.get("score"))] += 1

print(f"Quelle:  {LOG}")
print(f"Periode: ab {CUT}")
print(f"  {total} Anfragen, {len(names)} verschiedene Namen")
print(f"  von {first} bis {last}\n")

print("Funde je Detektor:")
if not per_detector:
    print("  keine")
for detector, count in per_detector.most_common():
    print(f"  {str(detector):<12} {count:>7}")

print("\nHäufigste Namen je Detektor:")
for detector, counter in sorted(by_detector.items()):
    print(f"\n--- {detector} — {sum(counter.values())} Funde, {len(counter)} Namen ---")
    for (name, score), count in counter.most_common(25):
        print(f"  {count:>5}x  {str(score):<6} {name}")
