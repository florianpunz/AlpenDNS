#!/usr/bin/env python3
"""Evaluates the query log of an observation week.

Reads the JSON lines from `[privacy.logging]` and counts per detector how often
it fired — with the names, because a false positive can only be judged by its
name and not by its number.

    abnahme.py [AB] [LOG]

`AB` is a timestamp in the log's format (`2026-09-16T16:24`); counting starts
there. Without an argument the whole file is read. `LOG` overrides the path, so
that a saved copy can be evaluated as well.

Why this exists: `/api/flagged` reads the ring buffer, and that is only
`ring_seconds` deep. Evaluating a week needs `mode = "full"` and this script.
See docs/OPERATIONS.md §6.
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

print(f"Source:  {LOG}")
print(f"Period:  from {CUT}")
print(f"  {total} queries, {len(names)} distinct names")
print(f"  {first} to {last}\n")

print("Findings per detector:")
if not per_detector:
    print("  none")
for detector, count in per_detector.most_common():
    print(f"  {str(detector):<12} {count:>7}")

print("\nMost frequent names per detector:")
for detector, counter in sorted(by_detector.items()):
    print(f"\n--- {detector} — {sum(counter.values())} findings, {len(counter)} names ---")
    for (name, score), count in counter.most_common(25):
        print(f"  {count:>5}x  {str(score):<6} {name}")
