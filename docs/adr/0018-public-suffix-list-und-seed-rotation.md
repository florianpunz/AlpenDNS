# ADR-0018: Public Suffix List und Seed-Rotation für `split_by_zone`

**Status:** angenommen · **Datum:** 2026-08-30 · **Nachtrag zu:** [ADR-0011](0011-eine-upstream-strategie.md) · **Betrifft:** [FEATURES.md](../FEATURES.md) P2

## Kontext

`split_by_zone` bestimmt den Upstream über `hash(seed, registrierbare Domain)`.
Zwei Dinge daran waren seit Phase 3 als Abweichung notiert.

**Die registrierbare Domain war geraten.** Genommen wurden die letzten beiden
Labels. Für `www.example.com` ist das richtig; für `shop.example.co.uk` liefert
es `co.uk`. Damit fielen sämtliche `.co.uk`-Namen auf denselben Hashwert und
landeten bei einem einzigen Upstream — keine Privacy-Lücke, aber genau die
schiefe Verteilung, die das Abnahmekriterium von Phase 7 (unter 5 % Abweichung)
messen wollte. Dasselbe gilt für `.com.au`, `.ac.at`, `.co.jp` und einige
hundert weitere.

**Der Seed galt bis zum Neustart.** FEATURES.md P2 verkauft als Eigenschaft:
*„nach einem Neustart sieht jeder Anbieter ein anderes Drittel. Über die Zeit
lernt keiner ein stabiles Bild."* Der zweite Satz stimmt nur, wenn jemand neu
startet. Ein Dienst, der ein halbes Jahr durchläuft — also das erklärte Ziel von
Phase 9 —, gibt jedem Anbieter ein halbes Jahr lang denselben Ausschnitt.

## Entscheidung

### Die registrierbare Domain kommt aus der Public Suffix List

Dependency `psl` (MIT/Apache-2.0). Die Liste ist einkompiliert: kein Netzabruf,
keine Laufzeitdatei, keine zweite Sorte Zustand, die aktuell gehalten werden
muss. Aktualisiert wird sie durch ein Versions-Update des Crates, wie jede
andere Abhängigkeit auch.

Fällt die Liste nicht zu — ein einzelnes Label wie `localhost`, oder ein Name,
der nur aus einem Suffix besteht — gilt der Name selbst als Einheit. Das ist die
sichere Richtung: im Zweifel *weniger* aufteilen, damit nicht plötzlich zwei
Anbieter denselben Namensraum sehen.

Eine eigene Tabelle der häufigsten mehrteiligen Suffixe wäre die
Dependency-freie Alternative gewesen. Sie wäre eine Liste, die jemand pflegen
müsste und die niemand pflegen würde — und ihr Veralten fiele nicht auf, weil
das Symptom eine leicht schiefe Verteilung ist und kein Fehler.

### Der Seed wird regelmäßig neu gezogen

Neuer Schlüssel `[[upstream_pool]] seed_rotation`, Default `"24h"`, `"0s"`
stellt das Verhalten aus Phase 3 wieder her. Die Rotation läuft als eigene
Aufgabe: eine, die an Anfragen hinge, käme auf einem stillen Server nie und auf
einem lauten dauernd.

**Die Abwägung dahinter ist nicht offensichtlich.** Rotation macht die Sache in
einer Hinsicht *schlechter*: über einen Monat gerechnet sieht jeder Anbieter
mehr verschiedene Domains als ohne. Sie macht sie in der entscheidenden Hinsicht
besser: keiner behält ein Bild, das über die Rotation hinaus stabil bleibt. Ein
Profil entsteht aus Wiedererkennung über Zeit — „diese Adresse fragt seit
Monaten dieselben zwanzig Domains" — nicht aus einzelnen Anfragen.

24 Stunden ist der Kompromiss: oft genug, dass kein Anbieter über Wochen
dasselbe Bild sieht; selten genug, dass die Zuordnung innerhalb eines Surftags
stabil bleibt und nicht mitten in einer Sitzung ein zweiter Anbieter dieselbe
Domain zu sehen bekommt.

Der Antwort-Cache bleibt unberührt. Er liegt vor dem Pool und kennt keine
Upstreams (ARCHITECTURE.md §1); eine Rotation kostet nichts an Trefferquote,
sie ändert nur, wen der nächste Cache-Miss fragt.

Der Seed liegt als `AtomicU64` im Pool, nicht hinter einem Lock: gelesen wird er
bei jeder Anfrage, geschrieben höchstens einmal am Tag. Dass eine Anfrage mitten
in der Rotation noch den alten Wert sieht, ist folgenlos — der Upstream, den sie
damit wählt, war eine Sekunde vorher der richtige.

## Was geprüft ist

`ten_thousand_domains_stay_within_five_percent_per_upstream` ist das
Abnahmekriterium selbst: 10 000 Domains, ein Fünftel davon unter mehrteiligen
Suffixen, über acht Seeds und vier Poolgrößen, maximale Abweichung je Upstream
unter 5 %. `a_multi_label_suffix_no_longer_lands_on_one_upstream` ist die
Gegenprobe mit ausschließlich `.co.uk`-Namen — mit der alten Näherung wäre die
Abweichung 100 % gewesen.

`rotation_keeps_the_distribution_even` prüft, dass jede Rotation eine
gleichmäßige Verteilung ergibt und nicht bloß eine andere.
`the_rotation_task_keeps_rotating_until_shutdown` nagelt Takt und Ende fest.

Die Kennzahlen `distribution` und `max_deviation` stehen in
`upstream::strategy` und nicht im Testcode: eine Zahl, die nur im Test
existiert, verschwindet beim nächsten Umbau still. In der Metrik steht
`alpendns_zone_seed_rotations_total` — damit ist „die Zuordnung rotiert" keine
Behauptung in der Konfiguration, sondern eine Zahl, die im Betrieb steigt.
