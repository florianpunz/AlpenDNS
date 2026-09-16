# Betrieb

Installation, Upgrade, Backup, Fehlersuche. Diese Datei ist so geschrieben, dass
jemand anderes danach installieren kann, ohne zu fragen — das ist das
Abnahmekriterium von Phase 9, Schritt 8.

Alles hier gilt für Debian 12/13 und Ubuntu 24.04 aufwärts. AlpenDNS ist ein
Linux-Programm; andere Systeme sind kein Ziel.

---

## 1. Installation

### Paket bauen

Auf einem Rechner mit Rust-Toolchain, einmalig:

```bash
cargo install cargo-deb --locked
```

Dann im Repository:

```bash
cargo deb -p alpendns
# → target/debian/alpendns_0.0.1-1_amd64.deb
```

`cargo deb` baut selbst mit `--release`. Das Paket enthält ein statisch gegen
`ring`/`rustls` gelinktes Binary; eine OpenSSL-Version auf dem Zielsystem spielt
keine Rolle.

### Paket installieren

```bash
sudo apt install ./alpendns_0.0.1-1_amd64.deb
```

Das Paket legt an:

| Pfad | Was | Wem gehört es |
|---|---|---|
| `/usr/bin/alpendns` | Das Programm | root |
| `/etc/alpendns/alpendns.toml` | Konfiguration (conffile) | root |
| `/lib/systemd/system/alpendns.service` | Die Unit | root |
| `/var/lib/alpendns/` | API-Token, NRD-Datei | `alpendns` |
| `/var/cache/alpendns/` | Heruntergeladene Blocklisten | `alpendns` |
| `/var/log/alpendns/` | Query-Log, nur im Modus `full` | `alpendns` |
| `/usr/share/doc/alpendns/` | Dieses Handbuch, Beispielkonfiguration | root |

Die drei Verzeichnisse unter `/var` legt **systemd** beim Start an
(`StateDirectory=`, `CacheDirectory=`, `LogsDirectory=`), nicht das Paket. Damit
gibt es genau eine Stelle, die Rechte setzt, und ein Upgrade kann sie nicht
kaputtmachen.

Der Dienstuser `alpendns` ist ein Systemuser ohne Login-Shell und ohne
Home-Verzeichnis.

### Nach der Installation

Frisch installiert lauscht der Server **nur auf Loopback** — von außen ist er
nicht erreichbar. Das ist Absicht: ein Resolver, der ungefragt am Internet
lauscht, ist ein Amplification-Reflektor.

Prüfen, dass er antwortet:

```bash
dig @127.0.0.1 example.com          # sollte eine Adresse liefern
dig @127.0.0.1 doubleclick.net      # sollte NXDOMAIN liefern
systemctl status alpendns
```

### Für das eigene Netz freigeben

Eine Änderung, im ersten Block von `/etc/alpendns/alpendns.toml`:

```toml
[server]
listen_udp = ["127.0.0.1:53", "192.168.1.10:53"]
listen_tcp = ["127.0.0.1:53", "192.168.1.10:53"]
```

Die konkrete LAN-Adresse dieses Rechners eintragen, **nicht** `0.0.0.0`: die
Wildcard bindet auch an eine Schnittstelle, die morgen am Internet hängt.

Dann:

```bash
sudo alpendns -c /etc/alpendns/alpendns.toml check
sudo systemctl restart alpendns
```

`check` prüft die Konfiguration und die Verzeichnisse, ohne den Server zu
starten, und sagt am Ende, ob ein Listener über das eigene Netz hinausreicht.
Dieselbe Prüfung läuft als `ExecStartPre` vor jedem Start: ist die Datei kaputt,
startet der neue Prozess gar nicht erst, und bei einem `restart` bleibt der
Dienst unten und sagt warum — statt in einer Neustartschleife zu landen.

Zuletzt die Clients umstellen: im Router den DHCP-DNS-Server auf die Adresse
dieses Rechners setzen.

### Oberfläche

Die Web-UI zeigt Namen und lauscht deshalb nur auf Loopback. Von einem anderen
Rechner aus über einen SSH-Tunnel:

```bash
ssh -L 8053:127.0.0.1:8053 <server>
# dann http://127.0.0.1:8053 im Browser
```

Den Token fragt die Seite beim ersten Aufruf ab:

```bash
sudo cat /var/lib/alpendns/api.token
```

Wer die UI ohne Tunnel erreichbar macht, veröffentlicht sein Query-Log, sobald
der Token bekannt wird. Der vorgesehene Weg ist der Tunnel oder ein Reverse
Proxy mit eigener Authentifizierung.

Die Oberfläche folgt dem Farbschema des Systems. Der Knopf rechts oben in der
Kopfzeile (Sonne im hellen, Mond im dunklen Modus) überstimmt es; die Wahl bleibt
im Browser gespeichert. Ohne JavaScript bleibt es beim hellen Modus. Ein
`prefers-reduced-transparency` im System schaltet den Blur ab — die Seite bleibt
lesbar, nur ohne das Material.

---

## 2. Upgrade

```bash
cargo deb -p alpendns                            # neues Paket bauen
sudo apt install ./alpendns_<version>_amd64.deb  # einspielen
```

Was dabei passiert:

* `/etc/alpendns/alpendns.toml` ist ein **conffile**. Eine bearbeitete Datei
  wird nicht überschrieben; `dpkg` fragt, wenn sich beide Seiten geändert haben.
* Der Dienst wird nach dem Upgrade neu gestartet (`restart-after-upgrade`).
  Vorher läuft `alpendns check` — eine Konfiguration, die die neue Version nicht
  versteht, verhindert den Start, und die alte Instanz läuft weiter, bis sie
  planmäßig beendet wird.
* Blocklisten-Cache und API-Token bleiben liegen.

**Vor einem Upgrade lohnt ein Blick in `docs/ROADMAP.md`:** wenn eine Phase
Konfigurationsschlüssel entfernt hat, sagt der Start das mit Begründung — aber
er sagt es eben erst beim Start. `alpendns -c /etc/alpendns/alpendns.toml check`
mit dem **neuen** Binary vor dem Neustart ist die schnellere Antwort.

### Zurückrollen

```bash
sudo apt install ./alpendns_<alte-version>_amd64.deb --allow-downgrades
```

Der Zustand unter `/var` ist zwischen Versionen kompatibel: Blocklisten-Cache
und Token sind Dateien ohne Schema.

---

## 3. Backup

Zu sichern ist genau eine Datei:

```
/etc/alpendns/alpendns.toml
```

Alles andere ist wiederherstellbar: Blocklisten werden neu geladen, der
API-Token wird neu erzeugt, der Cache ist ohnehin flüchtig. Wer den Token
behalten will (damit die UI im Browser nicht neu fragt), nimmt
`/var/lib/alpendns/api.token` dazu.

```bash
sudo tar czf alpendns-backup-$(date +%F).tar.gz \
    /etc/alpendns/alpendns.toml \
    /var/lib/alpendns/api.token
```

Wiederherstellen: Datei zurückkopieren, `alpendns check`, `systemctl restart`.

Es gibt bewusst **keine Datenbank**: das Query-Log lebt im Modus `aggregate` nur
als Zähler, im Modus `ring` nur im Arbeitsspeicher. Ein Backup davon zu machen
wäre der Widerspruch zum Zweck des Projekts.

---

## 4. Fehlersuche

Zuerst immer:

```bash
systemctl status alpendns
journalctl -u alpendns -n 100 --no-pager
```

### Der Dienst startet nicht

`alpendns check` sagt in fast allen Fällen warum:

```bash
sudo -u alpendns alpendns -c /etc/alpendns/alpendns.toml check
```

Das `sudo -u alpendns` ist wichtig: als root sieht man Verzeichnisse
schreibbar, die es für den Dienst nicht sind.

| Meldung | Bedeutung |
|---|---|
| `unknown field ...` | Tippfehler in einem Schlüssel. Unbekannte Schlüssel sind ein Startfehler, kein Warning — sonst hinge jemand ungefiltert im Internet und merkte es nicht. |
| `Blocklisten konnten beim Start nicht geladen werden` | Kein Netz beim allerersten Start, und noch nichts im Cache. Später ist ein Ausfall unkritisch: dann gilt die zwischengespeicherte Fassung weiter. |
| `Listener konnten nicht geöffnet werden` | Port 53 ist belegt — siehe unten. |
| `... kann nicht geschrieben werden` | Rechte unter `/var` verstellt. `systemctl restart alpendns` setzt sie neu, weil systemd die Verzeichnisse verwaltet. |

### Port 53 ist belegt

Meist `systemd-resolved` (Ubuntu, manche Debian-Installationen). Wer hört:

```bash
sudo ss -lunp sport = :53
```

`systemd-resolved` belegt normalerweise `127.0.0.53:53` und stört nicht. Belegt
es `0.0.0.0:53`, muss sein Stub-Listener weichen:

```bash
sudo mkdir -p /etc/systemd/resolved.conf.d
printf '[Resolve]\nDNSStubListener=no\n' | \
    sudo tee /etc/systemd/resolved.conf.d/alpendns.conf
sudo systemctl restart systemd-resolved
sudo systemctl restart alpendns
```

Soll der Rechner selbst über AlpenDNS auflösen, danach noch
`/etc/resolv.conf` bzw. den `DNS=`-Eintrag in `resolved.conf` auf `127.0.0.1`
zeigen lassen.

### Namen lösen nicht auf, obwohl der Dienst läuft

Die Reihenfolge, in der man sucht:

```bash
# 1. Antwortet der Server überhaupt?
dig @127.0.0.1 example.com

# 2. Wird der Name geblockt — und von welcher Regel?
sudo alpendns -c /etc/alpendns/alpendns.toml policy test example.com

# 3. Kommen die Upstreams durch?
journalctl -u alpendns | grep -i upstream
```

`policy test` beantwortet die Frage "warum wurde das geblockt?" ohne Blick ins
Log und ohne dass der Server laufen muss. Es zeigt dieselbe Begründung, die die
UI unter "Warum?" anzeigt.

Ein Name, der zu Unrecht geblockt wird, gehört auf eine Allowlist oder als
befristete Freigabe in die UI (Knopf "Freigeben").

### Ein Gerät bekommt keine Antworten mehr

Möglicherweise die Drosselung. Der Zähler steht im Log und in der Metrik:

```bash
journalctl -u alpendns | grep -i drossel
curl -s localhost:9153/metrics | grep rate_limit   # nur wenn [metrics] an ist
```

Wenn `alpendns_rate_limited_total` steigt, während ein Gerät klagt: das Limit
liegt bei 100 Anfragen je Sekunde und Client mit einer Spitze von 200. Das ist
für ein einzelnes Gerät sehr viel — wer es trotzdem erreicht, hat entweder eine
Schleife im Netz oder ein Gerät, das mehrere Hosts vertritt (ein weiterer
Resolver, ein NAT davor). Im zweiten Fall gehören die Werte hoch:

```toml
[server.rate_limit]
per_client_qps = 500
burst = 1000
```

Abschalten (`enabled = false`) ist nur richtig, solange der Server ausschließlich
auf Loopback lauscht.

### Alles auf einmal sehen

```bash
sudo systemctl show alpendns -p MainPID -p User   # läuft er unprivilegiert?
systemd-analyze security alpendns                 # sind die Schranken aktiv?
curl -s -H "Authorization: Bearer $(sudo cat /var/lib/alpendns/api.token)" \
     localhost:8053/api/status | head -40
```

### Mehr im Log

Der Log-Level kommt aus `RUST_LOG`:

```bash
sudo systemctl edit alpendns
# [Service]
# Environment=RUST_LOG=debug
sudo systemctl restart alpendns
```

`debug` zeigt unter anderem, welche Client-Adresse gedrosselt wurde. **Query-Namen
stehen auch dort nicht** — die gehen ausschließlich über die Log-Schicht, und
die richtet sich nach `privacy.logging.mode`. Nach der Fehlersuche wieder
entfernen: `sudo systemctl revert alpendns`.

---

## 5. Was die Hardening-Direktiven bedeuten

`systemd-analyze security alpendns` rechnet die Unit nach; der Wert liegt bei
**1,5** (kleiner ist besser, gefordert waren unter 3,0). Was übrig bleibt, ist
das, was ein Resolver naturgemäß braucht: Netzzugang und Port 53.

| Direktive | Wogegen |
|---|---|
| `User=alpendns` + `AmbientCapabilities=CAP_NET_BIND_SERVICE` | Der Prozess war nie root. Port 53 kommt über eine einzelne Fähigkeit, nicht über Allmacht. |
| `NoNewPrivileges=yes` | Kein Weg zurück nach oben, auch nicht über ein SUID-Programm. |
| `ProtectSystem=strict` + `ProtectHome=yes` | Das ganze Dateisystem ist schreibgeschützt, außer den drei Verzeichnissen unter `/var`, die systemd selbst freigibt. |
| `PrivateTmp` / `PrivateDevices` | Kein gemeinsames `/tmp`, keine Geräte außer den harmlosen. |
| `MemoryDenyWriteExecute=yes` | Kein Speicher, der beschreibbar *und* ausführbar ist — die übliche Landebahn für Shellcode. |
| `SystemCallFilter=@system-service` + `~@privileged @resources` | Systemaufrufe, die ein Dienst nicht braucht, gibt es für ihn nicht. |
| `RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX AF_NETLINK` | Keine Raw-Sockets, kein Paket-Sniffing. `AF_NETLINK` steht dabei, weil glibc es beim Auflösen eines Hostnamens braucht — ohne die Zeile scheitert der Blocklisten-Download, und zwar still. |
| `ProtectKernelTunables/Modules/Logs`, `ProtectClock`, `LockPersonality` | Der Dienst kann am System nichts verstellen. |

Die Unit steht im Repository unter `packaging/systemd/alpendns.service` und ist
kommentiert.

---

## 6. Beobachtungswoche

Die Abnahme von Phase 8 und Phase 9 ist ein mehrtägiger Lauf im echten Netz.
Sie ist nicht durch einen Test ersetzbar, und sie hat eine Reihenfolge.

**Vorbereitung.** Für die Beobachtung braucht es Namen, und die überleben die
Woche nur, wenn sie auf Platte gehen. Der Modus `ring` reicht dafür **nicht**:
Er hält die letzten `ring_seconds` im Arbeitsspeicher, mehr nicht. Das Panel
"Auffällig" in der UI liest genau diesen Puffer — ein Fehlalarm, den niemand am
selben Tag notiert, ist danach weg. Für einen Lauf, der eine Woche dauert, ist
das die falsche Grundlage.

```toml
[privacy.logging]
# Nötig, damit die Woche am Ende auswertbar ist. Der Preis steht in ADR-0004:
# jeder aufgelöste Name liegt für die Dauer der Beobachtung auf Platte.
mode = "full"
```

Nach der Auswertung wieder zurückstellen.

Dazu die eigenen Domains in den Typosquat-Wächter eintragen (Bank, Behörde,
Arbeitgeber), sonst tut er nichts:

```toml
[detection]
typosquat = { action = "flag", threshold = 0.85, protect = ["meine-bank.at"] }
```

Alle fünf Detektoren stehen auf `flag`. **Sie bleiben die ganze Woche auf
`flag`.** Sie melden, sie blocken nicht.

**Während der Woche.** Einmal am Tag in die UI schauen, Panel "Auffällig":

* Was steht drin, das offensichtlich harmlos ist? Das ist ein Fehlalarm.
  Notieren — Domain, Detektor, Score, was das Gerät gerade tat.
* Reputationsdienste und Antivirus-Produkte sehen per Konstruktion wie ein
  DNS-Tunnel aus. Ihre Zonen gehören nach `detection.tunneling.allow_zones`.
* Split-Horizon-DNS und Geräte-Weboberflächen unter einem echten Namen lösen
  den Rebinding-Schutz aus. Ihre Zonen gehören nach
  `detection.rebinding.allow_zones`.
* Meckert jemand im Haushalt, dass etwas nicht geht: `alpendns policy test
  <domain>` sagt, ob es an AlpenDNS lag. In aller Regel war es die Blockliste
  und nicht ein Detektor — die Detektoren blocken ja nicht.

Nebenbei mitlaufen lassen, was ohne Aufwand zu haben ist:

```bash
# Läuft er durchgehend? Ein Neustart taucht als neue Startzeit auf.
systemctl show alpendns -p ActiveEnterTimestamp -p NRestarts

# Zähler über die Woche, wenn [metrics] an ist
curl -s localhost:9153/metrics | grep -E 'queries_total|cache_hit_ratio|rate_limited|detections'
```

**Am Ende der Woche.** Erst die Zahlen, dann die Entscheidung.
`packaging/abnahme.py` liest das Query-Log und zählt je Detektor:

```bash
# Zweites Argument optional — damit lässt sich auch eine gesicherte Kopie auswerten
python3 abnahme.py 2026-09-16T16:24 | tee abnahme-periode.txt
```

Es gibt Anfragen und **verschiedene Namen** aus, dann je Detektor die Funde und
die häufigsten Namen mit Score. Beides gehört zur Beurteilung: 7 Funde auf 4 025
Namen sind etwas anderes als 7 auf 40 000.

Dann die Fehlalarm-Liste durchsehen und entscheiden, je Detektor einzeln:

* Keine Fehlalarme über eine Woche echten Verkehrs → dieser Detektor darf auf
  `action = "block"`. Einer nach dem anderen, nicht alle zusammen.
* **Ein Detektor, der nie ausgelöst hat, ist damit nicht bewertet.** „Keine
  Fehlalarme" heißt bei ihm nur, dass nichts passiert ist — für `block` fehlt der
  Beleg, dass er überhaupt richtig auslöst. Entweder einen kontrollierten Test
  fahren oder ihn auf `flag` lassen.
* **Ein Detektor, der leer lief, ist ebenfalls nicht bewertet.** Eine leere
  `protect`-Liste oder eine fehlende `nrd.txt` erzeugt dieselbe Null wie ein
  sauberer Lauf — nur ohne Grundlage.
* Fehlalarme, die sich über `allow_zones` erledigen lassen → eintragen, weitere
  Woche beobachten.
* Fehlalarme, die sich nicht erledigen lassen → der Detektor bleibt auf `flag`.
  Das ist ein gültiges Ergebnis und kein Scheitern; die bekannten Grenzen der
  Verfahren stehen in `docs/FEATURES.md` und `docs/BENCHMARKS.md`.

Das Ergebnis gehört als Notiz in `docs/ROADMAP.md` unter die Abnahme der Phase —
mit Zahlen. „Lief bei mir" ist kein Abnahmekriterium.

---

## 7. Deinstallation

```bash
sudo apt remove alpendns    # Dienst weg, /etc und /var bleiben
sudo apt purge alpendns     # zusätzlich: Konfiguration, User, /var/lib, /var/cache
```

Nach `purge` bleibt nichts zurück. Vorher nicht vergessen, den DHCP-Server im
Router wieder auf einen anderen Resolver zu zeigen — sonst steht das Netz.
