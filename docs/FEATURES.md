# Feature-Katalog

Ideen mit Bewertung. Nicht alles davon wird gebaut — das ist der Sinn einer Liste mit
Bewertung. Jeder Eintrag hat: was es ist, warum es interessant ist, was es kostet, und
wo die ehrlichen Grenzen liegen.

**Legende**

* **Aufwand:** S (ein Abend) · M (2–4 Abende) · L (eine Woche+)
* **Neu:** wie ungewöhnlich das in existierenden Resolvern ist —
  ○ Standard · ◐ selten · ● praktisch nirgends
* **Phase:** wo es in der [Roadmap](ROADMAP.md) liegt

---

## P — Privacy

### P1 · Zero-Log mit k-Anonymität `Aufwand M` `Neu ◐` `Phase 6/7`

Statt "Logging an/aus" vier Modi, Default aggregiert mit Schwellwert: eine Domain
erscheint erst in Statistiken, wenn sie mindestens `k`-mal (Default 5) abgefragt wurde.
Begründung und Details: [ADR-0004](adr/0004-logging-default-aggregiert.md).

**Warum das mehr ist als eine Einstellung:** die einmalig aufgerufenen Domains sind genau
die verräterischen. Übliche Query-Logs speichern sie mit Zeitstempel und Client-IP. Hier
existieren sie nach der Beantwortung nicht mehr.

**Grenze, erledigt:** Umgesetzt war das zunächst mit einem Count-Min-Sketch, und der
überschätzt seltene Elemente. Die k-Schwelle prüfte deshalb auf der *unteren*
Schätzgrenze — korrekt, aber die Schranke wächst mit dem Verkehr: bei einer Million
Anfragen lag sie bei 11 und damit über jedem üblichen `k`, sodass **keine** Domain mehr
in der Statistik erschien. Gemessen in [BENCHMARKS.md](BENCHMARKS.md); seither wird exakt
gezählt ([ADR-0015](adr/0015-exakte-zaehlung-statt-sketch.md)). Die Zusicherung ist
dieselbe geblieben, nur hält sie jetzt auch bei Verkehr.

### P2 · Upstream-Splitting nach Zone `Aufwand M` `Neu ●` `Phase 3/7`

**Das interessanteste Feature des Projekts.** Statt alle Anfragen an einen Resolver zu
schicken, wird der Upstream über `hash(seed, registrable_domain) % n` bestimmt.

Konsequenzen:

* Derselbe Name geht immer zum selben Resolver → der Cache bleibt vollständig wirksam,
  anders als bei Round-Robin.
* Jeder Resolver sieht nur etwa `1/n` deiner Domains. Bei drei Upstreams sieht keiner mehr
  als ein Drittel deines Profils.
* Der Seed wird beim Start zufällig gezogen: nach einem Neustart sieht jeder Anbieter ein
  *anderes* Drittel. Über die Zeit lernt keiner ein stabiles Bild.
* Weil die Zuordnung an der registrierbaren Domain hängt (nicht am vollständigen Namen),
  landen `mail.example.com` und `cdn.example.com` beim selben Resolver — die Struktur
  einer besuchten Seite verrät sich damit nicht an mehrere Anbieter.

**Grenzen, die dokumentiert gehören:** Jeder Upstream sieht weiterhin deine IP. Beliebte
Domains verteilen sich bei allen Nutzern gleich. Ein Angreifer mit Zugriff auf mehrere der
konfigurierten Upstreams hebt den Schutz auf — die Auswahl sollte verschiedene Betreiber
und Rechtsräume abdecken.

### P3 · Privacy-Budget `Aufwand S` `Neu ●` `Phase 7`

Der Server zählt, welcher Anteil der Anfragen zu welchem Upstream ging, und zeigt das in
der UI: *"Quad9 hat 34 % deiner Domains gesehen, Mullvad 33 %, dnsforge 33 %."*

Klein umzusetzen, aber es macht ein abstraktes Versprechen zu einer nachprüfbaren Zahl.
Es deckt auch Fehlkonfigurationen auf — wenn ein Upstream 90 % abbekommt, weil die
anderen ständig ausfallen, sieht man das sofort statt nie.

### P4 · Oblivious DoH als Client `Aufwand M` `Neu ◐` `Phase 7`

RFC 9230. Die Anfrage wird für den Ziel-Resolver verschlüsselt und über einen Proxy
geschickt: der Proxy kennt deine IP, aber nicht die Anfrage; der Resolver kennt die
Anfrage, aber nicht deine IP. Das ist die einzige Technik in dieser Liste, die das
Problem "der Upstream kennt deine IP" wirklich löst statt es zu verteilen.

**Grenze:** Braucht einen Proxy und einen ODoH-fähigen Zielresolver, die *nicht* demselben
Betreiber gehören dürfen — sonst ist der Schutz Theater. Die Auswahl ist überschaubar.
Zusätzliche Latenz durch den zusätzlichen Hop.

### P5 · DDR/DNR — Clients automatisch auf Verschlüsselung heben `Aufwand M` `Neu ●` `Phase 10`

RFC 9462 (DDR) und RFC 9463 (DNR). Ein Client fragt `_dns.resolver.arpa` beim
konfigurierten Klartext-Resolver und bekommt zurück: *"denselben Dienst gibt es
verschlüsselt unter dieser Adresse."* Aktuelle Betriebssysteme werten das aus und wechseln
selbstständig von UDP/53 auf DoH oder DoT.

**Warum das ungewöhnlich ist:** Praktisch kein Heim-Resolver kann das. Damit wird
LAN-Verkehr zum Resolver verschlüsselt, ohne dass auf einem einzigen Gerät etwas
konfiguriert werden muss — das ist der Unterschied zwischen "ich habe DoH eingerichtet"
und "im ganzen Haushalt läuft DNS verschlüsselt".

**Voraussetzung:** ein Zertifikat, dem die Clients trauen, mit der IP oder dem Namen des
Resolvers als SAN. Im LAN heißt das entweder eine echte Domain mit ACME-DNS-01 oder eine
eigene CA, die auf den Geräten liegt.

### P6 · Hygiene-Grundlagen `Aufwand S je` `Neu ○` `Phase 3`

Kein Alleinstellungsmerkmal, aber Voraussetzung dafür, dass der Rest nicht Fassade ist:
ECS strippen (RFC 7871 nicht weiterreichen), EDNS-Padding (RFC 7830/8467), DNS Cookies
(RFC 7873), 0x20-Encoding, Quellport-Randomisierung, TTL-Deckel gegen langlebiges
Tracking über DNS-Cache.

### P7 · Cover-Traffic — bewusst verworfen

Zufällige Fake-Anfragen sollen das echte Muster verstecken. In der Praxis: hoher
Upstream-Verkehr, und ein Beobachter kann echte von generierten Anfragen meist trennen
(Timing, Wiederholungsmuster, Verteilung der Namen). Kosten sicher, Nutzen fraglich.

Die abgeschwächte Variante ist dagegen sinnvoll und steckt schon im Cache-Prefetch
(Phase 2): häufig genutzte Namen werden im Hintergrund erneuert, wodurch Upstream-Verkehr
entsteht, der nicht mit Nutzeraktivität korreliert. Das ist ein Nebeneffekt einer nützlichen
Funktion, kein eigenes Feature.

---

## D — Erkennung ohne Cloud

Alle Detektoren liefern einen Score und eine Begründung, laufen lokal, und stehen per
Default auf `flag`, nicht `block`. Ein Detektor, der das Internet kaputtmacht, wird
abgeschaltet — und mit ihm alle anderen.

### D1 · DNS-Rebinding-Schutz `Aufwand S` `Neu ○` `Phase 8`

Antworten mit privaten IPs (RFC 1918, Loopback, Link-Local) auf öffentliche Namen werden
verworfen. Klassischer Schutz gegen Angriffe, bei denen eine Webseite über den Browser auf
Geräte im LAN zugreift. `dnsmasq` und `unbound` können das; es fehlt in vielen
Blocklisten-Lösungen. Braucht eine Ausnahmeliste für interne Zonen und für Dienste, die
das legitim tun.

### D2 · DNS-Tunneling-Erkennung `Aufwand M` `Neu ◐` `Phase 8`

Datenexfiltration über DNS hat auffällige Merkmale: sehr lange Labels, hohe Entropie in
den Namen, viele einmalige Subdomains unter einer Zone, ungewöhnlich hoher Anteil an
TXT-/NULL-Anfragen, hohe Anfragerate an eine einzelne Zone.

Statt einer einzelnen Regel: mehrere Signale pro Zone über ein Zeitfenster, kombiniert zu
einem Score. Der entscheidende Trick ist, **pro Zone** statt pro Anfrage zu bewerten —
eine einzelne lange Subdomain ist normal, tausend davon unter derselben Zone nicht.

**Grenze:** Manche CDNs und Antivirus-Produkte sehen genauso aus. Deshalb eine Ausnahmeliste
und `flag` als Default.

### D3 · DGA-Erkennung `Aufwand M` `Neu ◐` `Phase 8`

Malware erzeugt Domainnamen algorithmisch (`kqxvbnzmrt.com`). Ein Zeichen-N-Gramm-Modell
über einem großen Korpus normaler Domains erkennt das gut, ist wenige hundert Kilobyte
groß, braucht keine GPU und läuft in Mikrosekunden.

Vorgehen: 3-Gramm-Wahrscheinlichkeiten aus einer Popularitätsliste lernen, Score = negative
Log-Likelihood, normalisiert auf die Namenslänge. Ergänzende Merkmale: Konsonantenhäufungen,
Ziffernanteil, Wörterbuch-Treffer.

**Grenze:** kurze Namen sind statistisch nicht unterscheidbar; `bit.ly`, `t.co` und
zufällig aussehende CDN-Hostnamen erzeugen Falsch-Positive. Wortlisten-basierte DGAs
(zwei echte Wörter aneinander) erkennt das Modell nicht. Deshalb: Falsch-Positiv-Rate ist
das Qualitätsmaß, nicht die Trefferquote.

### D4 · Typosquat-Wächter `Aufwand M` `Neu ●` `Phase 8`

Du hinterlegst die Domains, die dir wichtig sind — Bank, Behördenportal, Arbeitgeber.
Jede aufgelöste Domain wird gegen diese kleine Liste geprüft: Damerau-Levenshtein-Distanz
1–2, Tastatur-Nachbarschaft, Unicode-Confusables (kyrillisches `а` in `sparkasse.at`),
IDN-Homographen, verwechselbare TLDs.

**Warum das ungewöhnlich ist:** Bestehende Lösungen prüfen gegen globale Phishing-Listen —
also gegen das, was gestern schon gemeldet war. Hier läuft der Vergleich gegen *deine*
zwanzig Domains, wodurch auch eine Domain auffällt, die vor zehn Minuten registriert wurde
und auf keiner Liste steht. Der Rechenaufwand ist trivial, weil die Schutzliste klein ist.

Sinnvolle Ergänzung: bei einem Treffer nicht stumpf blocken, sondern eine
Sinkhole-Erklärseite ausliefern — *"dieser Name ähnelt sparkasse.at, unterscheidet sich
aber in einem Zeichen"*. Das ist der Moment, in dem der Schutz tatsächlich wirkt.

### D5 · Neu registrierte Domains `Aufwand S` `Neu ◐` `Phase 8`

Domains, die vor weniger als 30 Tagen registriert wurden, sind überproportional oft
bösartig. AlpenDNS liest eine lokale Datei mit Domain + Registrierungsdatum und flaggt
Treffer.

Die Datei kommt aus deinem AlpenShield-Projekt (CT-Logs, Zonendaten). Die Schnittstelle
ist bewusst eine Datei und kein API-Aufruf: der Resolver darf nicht davon abhängen, dass
ein zweiter Dienst läuft.

### D6 · Erklärbarkeit ist Pflicht, nicht Kür

Jeder Detektor liefert nicht nur einen Score, sondern die Merkmale, die dazu geführt haben
(*"Label-Entropie 4.7, 340 einmalige Subdomains in 5 Minuten"*). Ohne das ist ein
Falsch-Positiv nicht debugbar, und du wirst das Feature abschalten statt es zu verbessern.

---

## C — Clients und Policies

### C1 · Client-Identität jenseits der IP `Aufwand M` `Neu ●` `Phase 5`

IP-basierte Zuordnung bricht, sobald ein Gerät das Netz verlässt. AlpenDNS identifiziert
zusätzlich über DoH-Pfad-Token (`/dns-query/<32 zufällige Bytes>`) und mTLS-Client-Zertifikate.

**Folge:** Dein Handy behält seine Policy im Mobilfunknetz. Das Tablet der Kinder behält
seine Regeln auch im WLAN der Nachbarn. Das ist der Punkt, an dem ein Heim-Resolver zum
persönlichen Resolver wird — und es kostet nur die Token-Extraktion aus dem Pfad, weil
jeder Standard-DoH-Client das ohne Anpassung mitmacht.

### C2 · Zeitpläne `Aufwand S` `Neu ○` `Phase 5`

Regeln, die zu bestimmten Zeiten gelten. Braucht eine injizierbare Uhr, sonst ist es nicht
testbar. Zeitzonen und Sommerzeit sind die einzige echte Schwierigkeit.

### C3 · Temporäre Freigaben `Aufwand S` `Neu ◐` `Phase 5`

*"Erlaube `youtube.com` für 20 Minuten."* Über API, UI oder CLI. Läuft automatisch ab.
Das Feature, das den Unterschied macht, wenn außer dir noch jemand im Haushalt wohnt —
ohne das wird der Filter beim ersten Konflikt komplett abgeschaltet.

### C4 · Policy-Simulation `Aufwand S` `Neu ●` `Phase 5`

```
$ alpendns policy test ads.example.com --client kids-tablet
BLOCKED
  client   kids-tablet          ← matched by ip 10.0.10.42
  policy   kids
  allowlist local-allow         ← kein Treffer
  blocklist oisd-big:118432     ← ||ads.example.com^
  verdict  NXDOMAIN
```

Testen, ohne die Anfrage zu stellen. Damit wird eine Policy-Änderung überprüfbar, bevor
sie live ist — und es ist die Grundlage des Replay-Harness aus [TESTING.md](TESTING.md).

### C5 · Conditional Forwarding `Aufwand S` `Neu ○` `Phase 3`

Interne Zonen (`home.arpa`, Reverse-Zonen des eigenen Netzes) gehen an den internen
Server und nie ins Internet. Standard-Funktionalität, aber im Homelab unverzichtbar —
ohne sie leakt jeder interne Hostname an den Upstream.

---

## O — Beobachtbarkeit und Bedienung

### O1 · Decision-Trace und "Warum wurde das geblockt?" `Aufwand M` `Neu ●` `Phase 5/6`

Jede Antwort trägt intern eine vollständige Begründungskette (ARCHITECTURE.md §2). Die UI
zeigt sie: welche Liste, welche Zeile, welche Policy, welcher Upstream, wie lange.

**Warum das architektonisch früh entschieden werden muss:** Man kann es nicht nachträglich
einbauen. Entweder die Pipeline sammelt die Schritte von Anfang an, oder man hat später
nur ein `bool` und rät.

### O2 · Breakage-Erkennung `Aufwand M` `Neu ●` `Phase 8+`

Wenn ein Client dieselbe geblockte Domain innerhalb weniger Sekunden mehrfach anfragt und
danach auffällig still ist, ist mit hoher Wahrscheinlichkeit gerade eine App kaputtgegangen.
AlpenDNS erkennt dieses Muster und schlägt in der UI vor: *"`api.example.com` wurde 12-mal
in 4 Sekunden von `florian-laptop` geblockt — vermutlich funktioniert etwas nicht.
Freigeben?"*

Das dreht die übliche Reihenfolge um: normalerweise merkt der Nutzer die Störung und sucht
im Log. Hier meldet sich der Server, bevor die Suche anfängt. Braucht kein Query-Log —
das Muster ist im Ringpuffer sichtbar.

### O3 · Blocklist-Diff-Review `Aufwand M` `Neu ●` `Phase 10`

Vor dem Anwenden eines Listen-Updates: *"Diese Aktualisierung blockiert 47 Domains neu,
davon 3, die du in den letzten 30 Tagen tatsächlich aufgerufen hast: …"*

Der Vergleich läuft gegen die aggregierten Zähler, nicht gegen ein Query-Log — also auch
im Datenschutz-Default möglich. Löst das Problem, dass ein Listen-Update irgendwann still
etwas kaputt macht und niemand die Verbindung zum Update herstellt.

### O4 · Live-Query-Stream `Aufwand S` `Neu ○` `Phase 6`

Server-Sent Events, in der UI als laufende Liste. Nützlich beim Einrichten eines neuen
Geräts. Respektiert den Log-Modus: bei `none` fließen nur Zähler.

### O5 · Prometheus + saubere Metriken `Aufwand S` `Neu ○` `Phase 6`

Queries/s, Blocks, Cache-Trefferquote, Upstream-RTT pro Resolver, Fehlerraten,
Listengrößen, RSS. Keine Metrik enthält je einen Query-Namen — ein Label mit hoher
Kardinalität ist hier nicht nur ein Performance-Problem, sondern ein Datenleck.

### O6 · Sinkhole mit Erklärung statt NXDOMAIN `Aufwand M` `Neu ◐` `Phase 6+`

Statt NXDOMAIN eine lokale IP mit einer Seite, die erklärt, was und warum geblockt wurde.
Funktioniert nur für HTTP; bei HTTPS bricht die Verbindung mit einem Zertifikatsfehler ab,
was verwirrender ist als ein sauberes NXDOMAIN. Deshalb: konfigurierbar, nicht Default,
und in der UI mit dieser Einschränkung beschrieben. Für D4 (Typosquat) ist es trotzdem die
richtige Wahl — dort ist die Erklärung der eigentliche Nutzen.

---

## Bewertung: was zuerst?

Wenn du nur drei Dinge baust, die AlpenDNS von allem anderen unterscheiden:

1. **P2 Upstream-Splitting** — löst ein Problem, das sonst niemand löst, und ist mit
   moderatem Aufwand machbar.
2. **O1 Decision-Trace** — muss früh in die Architektur, macht alles danach debugbar,
   und ist der Grund, warum die UI etwas kann, das andere UIs nicht können.
3. **D4 Typosquat-Wächter** — kleiner Aufwand, konkreter Nutzen, und die Idee
   "prüfe gegen *meine* wichtigen Domains statt gegen globale Listen" ist der Kern
   dessen, was ein persönlicher Resolver besser kann als ein zentraler Dienst.

**C1 (Client-Identität über DoH-Token)** ist der Nachzügler mit dem besten
Aufwand-Nutzen-Verhältnis — technisch fast geschenkt, praktisch der Unterschied zwischen
einem Heim-Resolver und einem persönlichen.
