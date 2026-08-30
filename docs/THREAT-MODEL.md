# Bedrohungsmodell

Ein Privacy-Produkt ohne ehrliches Bedrohungsmodell ist Marketing. Dieses Dokument sagt,
wogegen AlpenDNS schützt, wogegen nicht, und wo die Grenzen unscharf sind.

## Wer sind die Gegner?

### A1 — Der Internetanbieter / das lokale Netz

**Kann:** allen unverschlüsselten Verkehr mitlesen, DNS-Antworten fälschen, DNS-Anfragen
auf den eigenen Resolver umbiegen (transparentes Redirect auf Port 53).

**AlpenDNS dagegen:** alle Upstream-Anfragen laufen über DoT/DoH/DoQ. Der Anbieter sieht
eine TLS-Verbindung zu einem bekannten Resolver, nicht die Namen. Port-53-Redirects
laufen ins Leere, weil AlpenDNS Port 53 nach außen nicht benutzt.

**Bleibt sichtbar:** die Ziel-IPs aller Verbindungen danach, und bei TLS ohne ECH der
Servername im ClientHello. **Das ist die wichtigste Einschränkung des ganzen Projekts:**
DNS-Verschlüsselung verbirgt, welchen Namen du *nachgeschlagen* hast, nicht, mit welcher
Adresse du dich *verbindest*. Wer davor Schutz braucht, braucht ein VPN oder Tor, nicht
diesen DNS-Server.

### A2 — Der Upstream-Resolver

**Kann:** jede Anfrage sehen, die er beantwortet, verknüpft mit deiner IP; daraus ein
Profil bauen; Antworten manipulieren.

**AlpenDNS dagegen:** `split_by_zone` verteilt die Namen deterministisch über mehrere
Anbieter, sodass jeder nur einen Teil sieht — die Einheit ist die registrierbare Domain
laut Public Suffix List, und der Seed wird per Default alle 24 Stunden neu gezogen, damit
kein Anbieter über die Zeit ein stabiles Bild behält ([ADR-0018](adr/0018-public-suffix-list-und-seed-rotation.md)).
ECS wird gestrippt, damit die Anfrage nicht zusätzlich dein Subnetz trägt. Padding
verhindert Rückschlüsse aus Nachrichtenlängen. Optional ODoH: der Proxy kennt deine IP
aber nicht die Anfrage, der Resolver umgekehrt ([ADR-0017](adr/0017-oblivious-doh.md)).

**Bleibt:** ohne ODoH sieht jeder Upstream deine IP und seinen Anteil der Namen. Der
Anteil ist nicht zufällig verteilt — populäre Domains landen bei jedem im selben Bucket,
das ist bei allen Nutzern gleich. Ein Angreifer mit Zugriff auf *mehrere* der
konfigurierten Upstreams hebt den Schutz auf. Bei der Auswahl der Upstreams gilt also:
verschiedene Betreiber, verschiedene Rechtsräume.

**Bleibt auch mit ODoH:** der Proxy weiß, mit welchem Anbieter du sprichst (`targethost`
steht in der URL, anders kann er nicht weiterreichen), und einmal je Prozessstart sieht
das Ziel deine Adresse beim Abruf seines öffentlichen Schlüssels — aber keine Frage
dabei. Gehören Proxy und Ziel demselben Betreiber, ist der Schutz aufgehoben; das kann
kein Code prüfen.

### A3 — Off-Path-Angreifer (Cache-Poisoning, Spoofing)

**Kann:** gefälschte Antworten schicken, wenn er Query-ID und Quellport errät.

**AlpenDNS dagegen:** verschlüsselte Upstream-Transporte machen das weitgehend gegenstandslos.
Zusätzlich: 0x20-Encoding, DNS Cookies (RFC 7873), strikte Validierung jeder Antwort gegen
die gestellte Frage vor dem Cachen, Quellport-Randomisierung.

**Seit Phase 7 geschlossen:** AlpenDNS rechnet die Signaturkette selbst nach, ab den
einkompilierten Root-Schlüsseln, und verwirft eine Antwort, deren Zone sich als signiert
ausweist und deren Kette nicht schließt (SERVFAIL). Das AD-Bit in unserer Antwort steht
danach für unser Urteil, nicht für die Behauptung des Upstreams
([ADR-0016](adr/0016-dnssec-validierung-im-forwarder.md)).

**Bleibt:** DNSSEC deckt nur signierte Zonen ab, und das ist der kleinere Teil des Netzes.
Für eine unsignierte Zone kann ein kompromittierter Upstream weiterhin lügen, und dagegen
gibt es im Forwarder-Modus kein Mittel — auch ein Rekursor hätte keines. Wer die
Validierung abschaltet (`privacy.dnssec = false`), landet wieder beim Zustand davor.

### A4 — Der Angreifer im eigenen LAN

**Kann:** den Resolver mit Anfragen fluten, DNS-Tunneling zur Exfiltration benutzen,
DNS-Rebinding gegen andere Geräte im LAN fahren.

**AlpenDNS dagegen:** Rate-Limiting pro Client, Tunneling-Heuristik, Rebinding-Schutz
(private IPs in Antworten für öffentliche Namen werden verworfen).

**Bleibt:** ein Gerät im LAN kann AlpenDNS umgehen, indem es direkt 1.1.1.1 fragt oder
DoH zu einem eigenen Anbieter aufbaut. Dagegen hilft nur die Firewall (Port 53 nach außen
sperren, bekannte DoH-Endpunkte blocken) — das ist eine Netzwerk-Aufgabe, kein
Resolver-Feature.

### A5 — Wer physischen Zugriff auf die Kiste hat

**Kann:** alles.

**AlpenDNS dagegen:** wenig, außer möglichst wenig zu speichern, was es zu holen gibt.
Default-Logging ist aggregiert mit k-Anonymitätsschwelle; es gibt kein Query-Log, das
jemand beschlagnahmen könnte. Das ist der eigentliche Wert des Zero-Log-Defaults —
nicht Verschlüsselung, sondern Nicht-Existenz der Daten.

### A6 — Der Betreiber gegen seine eigenen Nutzer

Ein DNS-Server im Familiennetz ist ein Überwachungswerkzeug. Wer eine Kinder-Policy
konfiguriert, kann auch Anfragen mitlesen.

AlpenDNS macht das nicht unmöglich, aber es macht den Default-Zustand zu "nicht
mitschreiben" und den Wechsel zu `full` zu einer sichtbaren, dokumentierten Handlung
in der Konfiguration. Die UI zeigt den aktiven Log-Modus prominent.

## Was ausdrücklich nicht abgedeckt ist

* **Verkehrsanalyse.** Zeitpunkte, Größen und Häufigkeiten von Anfragen bleiben ein Signal,
  auch bei Verschlüsselung. Padding hilft gegen Längen, nicht gegen Timing.
* **Kompromittierter Client.** Malware auf dem Laptop umgeht jeden Resolver.
* **Malware-Schutz.** Die Heuristiken erkennen Muster, keine Bedrohungen. Ein
  DGA-Score von 0.9 heißt "sieht aus wie generiert", nicht "ist bösartig".
* **Anonymität.** AlpenDNS ist ein Privacy-Werkzeug, kein Anonymitätswerkzeug. Der
  Unterschied ist nicht kosmetisch.

## Angriffsfläche des Projekts selbst

Der größte realistische Schaden geht nicht von einem Angreifer aus, sondern vom Code:

1. **Ein Panic im Anfragepfad** ist ein Remote-DoS für das ganze Netz. Deshalb die harten
   Clippy-Regeln in CLAUDE.md B.1.
2. **Ein offener Resolver** ist ein Amplification-Reflektor. Deshalb Rate-Limiting und
   Default-Listener nur auf privaten Adressen.
3. **Eine Abhängigkeit mit Netzwerkzugriff** bricht das Telemetrie-Versprechen ohne
   Zutun des Autors. Deshalb `cargo deny` und die Begründungspflicht für neue Dependencies.
4. **Falsch-Positive in den Heuristiken** machen das Internet kaputt und führen dazu, dass
   der Nutzer alles abschaltet. Deshalb ist `flag` der Default, nicht `block`.
