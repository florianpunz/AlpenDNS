# TODOS — offene Punkte mit Umsetzungsplan

Die Liste stand hier vorher als reine Aufzählung. Sie steht weiter unten unverändert im
Kern, jetzt aber mit je einem Plan: **Befund** (was der Code heute wirklich tut, mit
Belegstelle), **Entwurf**, **Schritte im Verify-Format** wie in der Roadmap, was bewusst
*nicht* gebaut wird, und Aufwand.

Keiner dieser Punkte gehört zu einer Phase mit Abnahmekriterium. Alles hier ist
Phase-10-Material im Sinne von [ROADMAP.md](ROADMAP.md): jederzeit verwerfbar, keine
Reihenfolge außer der am Ende empfohlenen.

Alle Datei- und Zeilenangaben stammen vom 2026-09-01. Zeilennummern altern schneller als
der Rest; die Modul- und Funktionsnamen sind der belastbare Teil.

---

## Drei Befunde, die quer zu mehreren Punkten liegen

Beim Durchsehen für diesen Plan sind drei Dinge aufgefallen, die in keinem TODO stehen,
aber mehrere davon beeinflussen. Sie gehören zuerst geklärt, sonst setzen mehrere Pläne
auf einer falschen Annahme auf.

**B1 — Es gibt keinen `SIGHUP`-Reload — erledigt.** [ARCHITECTURE.md](ARCHITECTURE.md) §7
beschreibt einen Reload, bei dem eine kaputte Konfiguration verworfen wird und die alte
aktiv bleibt. Der ist jetzt gebaut: `reload_on_hangup` in `main.rs` lädt auf `SIGHUP` die
Policy-Schicht neu — Clients, Policies, Regex, Zeitpläne und Listenquellen, atomar
eingetauscht — und lässt bei einer kaputten Konfiguration den alten Stand stehen. Was nicht
hot-reloadbar ist (Listener, Upstreams/TLS, Cache, Drosselung, Block-Modus, Detektoren),
nennt das Log bei jedem Reload. §7 ist entsprechend angepasst; die Querverweise auf B1 in
den Punkten 7, 9 und 10 sind damit überholt.

**B2 — Blocklisten werden nur beim Start geladen.** In
[filter/](../crates/alpendns/src/filter/) und [main.rs](../crates/alpendns/src/main.rs)
gibt es keinen Update-Scheduler; der in CLAUDE.md B.3 genannte "Update-Scheduler" ist
Zielbild, nicht Code. Die Listen kommen einmal über `filter::source`, das `<name>.list`
und `<name>.meta` (mit `etag:` und `last-modified:`) ins `CacheDirectory` schreibt. Für
Punkt 3 heißt das: "zuletzt aktualisiert vor 2 Tagen" misst heute im Wesentlichen, wie
lange der Prozess läuft. Der Punkt ist damit nur halb so groß wie er aussieht — und die
andere Hälfte (regelmäßig nachladen) ist das eigentlich Fehlende.

**B3 — Das Query-Log wird synchron im Anfragepfad geschrieben.** `QueryLog::new`
([logging/mod.rs:367-375](../crates/alpendns/src/logging/mod.rs#L367-L375)) öffnet die
Datei einmal mit `create` + `append` und legt sie in einen `Mutex`; geschrieben wird mit
`writeln!` direkt in der Aufzeichnung
([logging/mod.rs:483-490](../crates/alpendns/src/logging/mod.rs#L483-L490)) — ohne Puffer,
unter demselben Schloss für alle Anfragen. Betrifft nur `mode = "full"`, ist also nicht der
Default, aber es ist genau die Sorte Schloss im Anfragepfad, die B.3 Regel 5 vermeiden
will. Gehört zu Punkt 2 und wird dort mitgemacht.

---

## 1. TCP-Slowloris — erledigt.

> Timeout auf den Body-Read + ein Cap auf gleichzeitige Verbindungen. Im LAN reicht ein
> kompromittiertes Gerät, um den Resolver lahmzulegen.

**Gebaut** in [server/tcp.rs](../crates/alpendns/src/server/tcp.rs): `BODY_TIMEOUT = 5 s` um
den Body-Read, im `select!` mit `shutdown`; ein `Semaphore` mit `MAX_CONNECTIONS`, dessen
Permit **vor** dem `accept` geholt wird und am Task hängt; `MAX_PER_CLIENT = 8` je Quell-IP
über eine `HashMap` unter einem Mutex im Verbindungsaufbau. Drei Zähler
(`alpendns_tcp_connections_rejected_total`, `..._at_capacity_total`, `..._body_timeouts_total`),
Zahlen in [BENCHMARKS.md](BENCHMARKS.md), Betriebsseite in
[OPERATIONS.md](OPERATIONS.md) §4.

Drei Abweichungen vom Plan, jede mit Grund:

* **`MAX_CONNECTIONS = 64` statt 256.** Die Zahl ist so klein, dass der Test die *echte*
  Konstante prüfen kann, ohne 256 Deskriptoren zu brauchen — und 64 gleichzeitige
  TCP-Verbindungen für DNS ist in einem Haushalts-LAN viel. Nebenbei heißt "Permit vor
  `accept`", dass dauerhaft eines reserviert ist: gleichzeitig bedient werden 63.
* **Ein dritter Zähler.** Die beiden geplanten hätten die Obergrenze unsichtbar gelassen:
  sie weist nichts ab, sie lässt warten. Ohne `at_capacity` sähe "die Grenze trägt" genauso
  aus wie "die Grenze ist nie erreicht worden". Er wird **vor** dem Warten gezählt, nicht
  danach — hinterher gezählt käme die Zahl erst, wenn der Platz frei wird.
* **Getestet wird zweigeteilt.** Der Body-Timeout läuft über `tokio::io::duplex` mit
  angehaltener Uhr (`handle_connection` ist dafür über den Stream generisch geworden) — an
  echten Sockets würde die automatisch vorlaufende Testuhr mit epoll um die Wette laufen.
  Die beiden Obergrenzen laufen an echten Sockets ohne Uhr, weil dort keine Zeit im Spiel
  ist; auf den Zähler wird kurz gewartet statt ihn zu unterstellen, weil der Annahme-Pfad
  dem Kernel-Backlog nachläuft.

**Befund.** Zwei Lücken, beide in [server/tcp.rs](../crates/alpendns/src/server/tcp.rs):

1. Das `IDLE_TIMEOUT` von 10 s liegt nur auf dem Lesen des **Längenpräfixes**
   ([tcp.rs:70-79](../crates/alpendns/src/server/tcp.rs#L70-L79)). Der Body wird eine Zeile
   weiter mit `stream.read_exact(&mut packet).await?`
   ([tcp.rs:85](../crates/alpendns/src/server/tcp.rs#L85)) **ohne jedes Zeitlimit** gelesen.
   Wer zwei Bytes `0xFF 0xFF` schickt und dann schweigt, hält einen Task, einen
   64-KB-Vektor und einen Deskriptor unbegrenzt. Der `select!` auf `shutdown` fehlt hier
   ebenfalls — eine solche Verbindung verzögert zusätzlich das Herunterfahren, bis
   `TimeoutStopSec=10s` greift.
2. `tracker.spawn` in der Accept-Schleife
   ([tcp.rs:43](../crates/alpendns/src/server/tcp.rs#L43)) hat **keine Obergrenze**. Die
   Grenze ist heute das Deskriptor-Limit des Prozesses.

Die bestehende Drosselung hilft nicht: `RateLimiter` wird in `handle_request` befragt, also
**pro Anfrage** — und eine Slowloris-Verbindung stellt nie eine Anfrage. Die zweite Lücke
ist die gefährlichere; ohne sie kostet die erste nur einen Task.

Serverseitig gibt es nur UDP und TCP (`server/` enthält `udp.rs`, `tcp.rs`, `mod.rs`);
DoT/DoH/DoQ sind ausschließlich Upstream-Transporte. Der Fix betrifft genau eine Datei.

**Entwurf.** Drei Konstanten im Modul, keine Konfiguration — im Sinne von B.2 gemessene
Werte im Code statt eines weiteren Schalters, den niemand dreht:

* `BODY_TIMEOUT = 5 s` um das `read_exact` des Bodys, im selben `tokio::select!` mit
  `shutdown` wie das Präfix-Lesen. Fünf Sekunden für höchstens 64 KB aus dem LAN sind
  großzügig.
* `MAX_CONNECTIONS = 256` als `Arc<Semaphore>`. Der Permit wird **vor** dem `accept` geholt
  (`acquire_owned().await`), nicht danach: dann läuft die Verbindung gar nicht erst auf,
  statt akzeptiert und sofort geschlossen zu werden, und das Backlog des Kernels drosselt
  von selbst. Der Permit hängt am Task und fällt mit ihm.
* `MAX_PER_CLIENT = 8` gleichzeitige Verbindungen je Quell-IP. Ohne das belegt ein Gerät
  alle 256 Plätze, und die Obergrenze wird selbst zur Waffe. Eine kleine
  `HashMap<IpAddr, u32>` unter einem Mutex in der Accept-Schleife genügt — dieser Mutex
  liegt nicht im Anfragepfad, sondern im Verbindungsaufbau.

Alle drei greifen nur, wenn es schon zu spät ist; im Normalbetrieb kosten sie einen
Semaphor-Zugriff je Verbindung.

**Schritte.**

```
1. BODY_TIMEOUT um den Body-Read, im select! mit shutdown → verify: Test schickt Präfix,
   dann nichts; Verbindung ist nach 5 s zu, andere Verbindungen werden weiter bedient
2. Semaphore vor accept, Permit am Task → verify: Test öffnet MAX_CONNECTIONS+1 stille
   Verbindungen; die letzte kommt erst durch, wenn eine frühere fällt
3. Grenze pro Quell-IP → verify: eine IP mit MAX_PER_CLIENT+1 Verbindungen blockiert keine
   zweite IP; deren Anfrage wird normal beantwortet
4. Zähler in metrics.rs: abgewiesene Verbindungen, Body-Zeitüberschreitungen
   → verify: /metrics zeigt beide nach dem Lasttest
5. Zahlen in BENCHMARKS.md → verify: Durchsatz vor/nach der Änderung, Abweichung benannt
```

Getestet wird ohne `sleep`: `tokio::time::pause()` plus `advance()` macht die Timeouts
deterministisch; die Semaphore braucht ohnehin keine Zeit.

**Bewusst nicht:** kein Schließen bestehender Verbindungen bei Erreichen der Grenze (wer
schon spricht, spricht zu Ende), keine Konfigurierbarkeit, kein `slip`-Verhalten.

**Aufwand:** S, ein Abend. **Risiko:** gering, die Änderung ist lokal. Kein ADR nötig.
**Priorität: hoch** — der einzige Punkt der Liste, der eine ausnutzbare Lücke schließt, und
der billigste dazu.

---

## 2. Log-Rotation im Modus `full`

> logging/mod.rs schreibt append-only in eine Datei, die der Prozess selbst öffnet —
> systemd/journald greift da nicht. Über Monate wächst das Query-Log unbegrenzt bis zur
> vollen Platte.

**Befund.** Stimmt, siehe B3. Eine Aufbewahrungsdauer gibt es nirgends; der Abschnitt
`[privacy.logging]` in [config.rs](../crates/alpendns/src/config.rs) hat keinen
entsprechenden Schlüssel. Ein Eintrag ist eine JSON-Zeile mit Name, Typ, Client, RCode,
Begründung, Dauer und Funden — grob 200 Byte. Bei 50 Anfragen/s sind das rund **850 MB pro
Tag**. Die Platte ist nicht in Monaten voll, sondern in Tagen.

**Der entscheidende Punkt ist kein Platzproblem.** Diese Datei ist die einzige Stelle im
System, an der Query-Namen den Prozess überleben (B.1 Regel 3, ADR-0004). "Rotation" heißt
hier vor allem: **wie lange werden Namen aufbewahrt.** Das ist eine Datenschutzentscheidung
und gehört deshalb in die Konfiguration, nicht in eine Konstante.

**Entwurf — `logrotate`, kein Eigenbau.** Weil die Datei mit `.append(true)` geöffnet ist,
schreibt jedes `write` ans aktuelle Dateiende, unabhängig vom Offset des Deskriptors. Damit
funktioniert `logrotate` mit `copytruncate` **ohne jede Codeänderung und ohne
Reopen-Signal** — kein Loch in der Datei, keine verlorene Zeile außer denen im Fenster
zwischen Kopie und Truncate. Genau deshalb gewinnt der Eigenbau hier nichts.

Ins Paket kommt `packaging/logrotate/alpendns`:

```
/var/log/alpendns/queries.jsonl {
    daily
    rotate 7
    maxsize 100M
    compress
    delaycompress
    copytruncate
    missingok
    notifempty
    create 0640 alpendns alpendns
}
```

`rotate 7` ist eine Vorgabe, kein Naturgesetz — sie gehört in OPERATIONS.md mit dem Satz,
dass sie die **Aufbewahrungsdauer der Namen** ist. `create 0640` deckt sich mit
`LogsDirectoryMode=0750` und `UMask=0077`; ohne die Zeile erbt die neue Datei die Rechte der
alten, was hier zufällig richtig, aber nicht garantiert wäre.

Dazu zwei Ergänzungen, die derselbe Handgriff sind:

* **Ein `BufWriter` um die Datei** (B3). Der Anfragepfad schreibt dann in den Puffer statt
  in einen Syscall; ein Task leert ihn im Sekundentakt und beim Herunterfahren. Ein Absturz
  kostet höchstens eine Sekunde Protokoll — beim Query-Log ist das kein Verlust, über den
  jemand traurig ist.
* **Ein Hinweis in `alpendns check`**, wenn `mode = "full"` konfiguriert ist und
  `/etc/logrotate.d/alpendns` fehlt. Das ist der Fall, in dem heute jemand die Platte
  vollschreibt, ohne es zu merken.

**Verworfene Alternativen.**

* *Rotation im Prozess* (Größe, Alter, Anzahl): rund 150 Zeilen, die Debian schon hat,
  inklusive der Fehlerfälle rund um Umbenennen unter laufendem Schreiben.
* *An journald übergeben:* verstößt gegen B.1 Regel 3. Die Aufbewahrung folgte dann der
  journald-Konfiguration statt der Projektkonfiguration, die Namen lägen an einem Ort, den
  jedes `journalctl` liest, und `SystemMaxUse` ist eine Größenangabe, keine Frist.
* *`maxsize` allein ohne `rotate`-Grenze:* begrenzt den Platz, nicht die Aufbewahrung.

**Schritte.**

```
1. logrotate-Datei ins Paket, in Cargo.toml (assets) eintragen → verify: dpkg -c zeigt sie
   unter /etc/logrotate.d/; logrotate -d meldet keinen Fehler
2. Test, dass Schreiben nach copytruncate weiterläuft → verify: Datei per truncate leeren,
   weiterschreiben, Größe wächst ab 0 statt ab dem alten Offset
3. BufWriter plus Leer-Task im Sekundentakt → verify: bestehende Logging-Tests grün; ein
   Test belegt, dass ein Eintrag nach dem Leeren in der Datei steht
4. Hinweis in alpendns check bei mode=full ohne logrotate-Datei → verify: check meldet es,
   scheitert aber nicht daran
5. OPERATIONS.md: rotate 7 = sieben Tage Namen, und wie man es ändert → verify: der Satz
   steht im Abschnitt zum Protokoll
```

**Aufwand:** S–M, ein Abend. **Priorität: hoch, sobald jemand `full` einschaltet** — bis
dahin null, weil im Default-Modus gar keine Datei geöffnet wird.

---

## 3. Alter der Blocklisten in der UI

> Bei Blocklisten "Alter" festhalten im UI, zuletzt aktualisiert vor (2 Tagen…)

**Befund.** `ListInfo` ([api/mod.rs:70-76](../crates/alpendns/src/api/mod.rs#L70-L76)) hat
`name`, `entries`, `format` — **keinen Zeitstempel**; gebaut wird es in
[main.rs:329](../crates/alpendns/src/main.rs#L329). Der Loader kennt `Origin`
(`File` / `Network` / `NotModified` / `StaleCache`,
[filter/source.rs:41-52](../crates/alpendns/src/filter/source.rs#L41-L52)), aber nur als
Herkunft dieses einen Ladevorgangs, nicht als Zeitpunkt. Auf Platte liegen `<name>.list` und
`<name>.meta` mit `etag:` und `last-modified:` — Letzteres ist die Angabe des
**Herausgebers**, nicht der Zeitpunkt des Abrufs.

Dazu Befund B2: **es gibt keinen Refresh zur Laufzeit.** Ohne ihn beantwortet "zuletzt
aktualisiert" nur, wann der Dienst zuletzt neu gestartet wurde — eine Anzeige, die genau
dann nichts nützt, wenn man sie braucht.

**Entwurf.** Der Punkt zerfällt in zwei, und der zweite ist der wichtigere.

**(a) Alter anzeigen.** Zwei Zeitangaben je Liste, weil sie zwei verschiedene Fragen
beantworten:

* *geholt am* — Zeitpunkt des letzten erfolgreichen Abrufs. Quelle ist die mtime von
  `<name>.list`; die überlebt den Neustart und muss deshalb nicht im Prozess gehalten
  werden. Bei `Origin::NotModified` wird die mtime angefasst, denn "der Server sagt, sie ist
  unverändert" heißt: sie ist aktuell.
* *veröffentlicht am* — der `last-modified:`-Wert aus der `.meta`-Datei, sofern der
  Herausgeber ihn liefert. Eine Liste, die seit acht Monaten unverändert ist, ist ein
  anderes Problem als eine, die seit acht Monaten nicht abgerufen wurde.

`ListInfo` bekommt zwei `Option`-Felder als absolute Zeitpunkte (RFC 3339). **Die
Formatierung zu "vor 2 Tagen" macht der Browser**, nicht der Server: eine relative Angabe
veraltet, während die Seite offen steht, und die Seite lädt ohnehin periodisch nach.
`Intl.RelativeTimeFormat` ist Teil der Plattform und damit keine externe Abhängigkeit (B.6).

Zur Farbe: `--warn` ist laut B.6 der hohen Latenz vorbehalten, und die Regel steht als Test
in [api/ui.rs](../crates/alpendns/src/api/ui.rs). Eine veraltete Liste ist damit **keine
Farbe**, sondern Text — "geholt vor 34 Tagen" sagt alles, was Rot auch sagen würde. Wer eine
Hervorhebung will, nimmt Schriftgewicht.

Dazu eine Metrik `alpendns_blocklist_age_seconds{list="…"}`, damit der Zustand auch ohne
offene UI sichtbar ist.

**(b) Der eigentliche Punkt: regelmäßig nachladen.** Ein Task, der je Liste in einem
konfigurierten Intervall (Default 24 h) neu lädt, den `Matcher` baut und ihn per `ArcSwap`
austauscht (B.3 Regel 5). Ein Fehlschlag ist kein Fehler: die alte Liste bleibt aktiv, der
Zähler steigt, das Alter wächst sichtbar — genau der fail-open-Fall aus B.1 Regel 6.

Das ist die größere Änderung und gehört **entschieden, nicht nebenbei gebaut**: es ist der
in CLAUDE.md B.3 als eigenes Crate vorgesehene Update-Scheduler.

**Schritte.**

```
1. mtime bei NotModified anfassen → verify: Test mit lokalem 304-Server, mtime ist danach neu
2. Zwei Zeitfelder in ListInfo, Quelle mtime + .meta → verify: /api/lists liefert beide;
   Test mit fehlender .meta liefert null statt Fehler
3. UI: Zeile "geholt vor …" je Liste, Formatierung im Browser → verify: ui.rs-Regeltests
   grün, insbesondere die Farbprüfung; danach ein Blick eines Menschen
4. Metrik alpendns_blocklist_age_seconds → verify: /metrics zeigt je Liste einen Wert
5. [getrennt entscheiden] Refresh-Task mit ArcSwap-Tausch → verify: Test mit zwei
   Listenständen hinter einem lokalen Server; nach dem Intervall greift der neue Stand, ohne
   dass eine Anfrage dazwischen scheitert
6. [getrennt] Fehlschlag hält die alte Liste → verify: Server antwortet 500, Matcher
   unverändert, Zähler steigt, Alter wächst
```

**Aufwand:** (a) S, ein halber Abend. (b) M, ein bis zwei Abende, plus ADR.
**Priorität:** (a) niedrig, (b) mittel — eine Blockliste, die nie aktualisiert wird, ist der
leise Ausfall des Kernversprechens.

---

## 4. Abhängigkeit von der Systemuhr dokumentieren

> Die eigene Validierung (ADR-0016) setzt eine korrekte Host-Uhr voraus — eine falsche Uhr
> macht jede signierte Zone "bogus".

**Befund.** [dnssec.rs](../crates/alpendns/src/dnssec.rs) prüft die Signaturzeiten nicht
selbst; das tut `hickory-net` beim Validieren, gegen die Systemuhr. Es gibt hier also
**keinen Skew-Puffer, den man einstellen könnte** — und das ist richtig so: ein Puffer wäre
die Aufweichung genau der Eigenschaft, für die ADR-0016 geschrieben wurde.

Drei Dinge fehlen konkret:

1. **Die Unit ordnet sich nicht hinter die Zeitsynchronisation.**
   [alpendns.service](../packaging/systemd/alpendns.service) hat
   `After=network-online.target`, aber **kein `After=time-sync.target`**. Beim Booten kann
   der Resolver also starten, bevor die Uhr steht. Das ist der praktisch häufigste Fall
   einer falschen Uhr — nicht der Angriff, sondern eine Kiste ohne RTC (Raspberry Pi), die
   mit dem Epoch-Datum hochkommt.
2. **Der Ausfallmodus ist hart.** Bogus heißt SERVFAIL. Eine falsche Uhr legt damit *jede
   signierte Zone* lahm, also den größeren Teil des Netzes, und sieht in den Metriken genau
   aus wie ein Angriff. Das ist die Sorte Ausfall, bei der man eine Stunde in die falsche
   Richtung sucht.
3. **Es steht nirgends.** Weder OPERATIONS.md §4 (Fehlersuche) noch THREAT-MODEL.md
   erwähnen die Uhr.

Nebenbefund: `ProtectClock=yes` steht in der Unit. Der Dienst kann die Uhr nicht stellen —
richtig so, und genau deshalb ist das Stellen Aufgabe des Systems und gehört ins Runbook.

**Entwurf — drei kleine Dinge, kein Feature.**

* **`After=time-sync.target` in die Unit.** Eine Zeile. `Wants=` ausdrücklich **nicht**: wer
  `systemd-timesyncd` bewusst abgeschaltet hat und die Zeit anders stellt, soll den Resolver
  nicht damit starten müssen. `After=` ohne `Wants=` ordnet nur, wenn das Ziel ohnehin
  läuft — das ist die richtige Stärke der Aussage.
* **Ein Abschnitt in OPERATIONS.md §4**, Titel etwa *"Fast alles ist SERVFAIL"*. Inhalt: Die
  DNSSEC-Validierung rechnet Signaturzeiten gegen die Systemuhr; geht die Uhr um mehr als
  das Signaturfenster falsch, ist jede signierte Zone bogus und damit SERVFAIL. Prüfen mit
  `timedatectl` (`System clock synchronized: yes`). NTP ist Voraussetzung, nicht Komfort. Auf
  Geräten ohne RTC steht die Uhr nach jedem Stromausfall falsch, bis das Netz da ist. Wer die
  Ursache nachweisen will, sieht sie am Zähler der bogus-Antworten: er steigt ab einem
  Zeitpunkt schlagartig und fällt nicht mehr.
* **Ein Hinweis in `alpendns check`:** wenn DNSSEC aktiv ist und die Uhr laut
  `/run/systemd/timesync/synchronized` bzw. `adjtimex` nicht synchron ist, sagt `check` das —
  als **Hinweis, nicht als Fehler**. Es darf den Start nicht verhindern: ein Resolver, der
  bei ungestellter Uhr gar nicht startet, ist beim Booten ohne Netz genau das
  Henne-Ei-Problem, das `check` laut
  [main.rs:812-822](../crates/alpendns/src/main.rs#L812-L822) bewusst meidet.

**Verworfen:** ein Skew-Puffer (weicht ADR-0016 auf); automatisches Abschalten der
Validierung bei unsynchroner Uhr (ein Angreifer, der die Uhr verstellen kann, schaltet damit
DNSSEC ab — die Umkehrung des Schutzziels); eine eigene NTP-Abfrage (verstößt gegen B.1
Regel 4, der Prozess kontaktiert genau drei Sorten Ziele).

**Schritte.**

```
1. After=time-sync.target in die Unit → verify: systemd-analyze verify alpendns.service ohne
   Fehler; systemctl list-dependencies --after zeigt das Ziel
2. Abschnitt in OPERATIONS.md §4 → verify: jemand mit dem Symptom "alles SERVFAIL" findet ihn
   über die Überschrift
3. Uhr-Hinweis in alpendns check → verify: Test mit gefälschtem Synchronstatus; check gibt
   den Hinweis und liefert trotzdem Exit 0
4. Satz in THREAT-MODEL.md: falsche Uhr als Verfügbarkeitsrisiko, gegen das nicht geschützt
   wird → verify: der Punkt steht in der Liste der bewusst offenen Punkte
```

**Aufwand:** S, ein halber Abend. **Priorität: mittel** — Schritt 1 ist eine Zeile mit echtem
Nutzen auf jedem Gerät ohne RTC.

---

## 5. Befristete Freigaben und Sperren überleben keinen Neustart

> Grants/Denials sind nur im RAM (temporary.rs). Ein Neustart verliert sie. Das ist
> vertretbar, aber es gehört als bewusste Entscheidung dokumentiert.

**Befund.** Bestätigt: `Temporary` hält `Mutex<HashMap<String, Instant>>`
([policy/temporary.rs:26-30](../crates/alpendns/src/policy/temporary.rs#L26-L30)), zweimal
instanziiert — einmal für Freigaben, einmal für Sperren.

**Warum Persistenz hier teurer ist, als sie aussieht**, steht schon im Modulkopf: die Frist
läuft über `Instant`, also über **monotone** Zeit, ausdrücklich damit eine verstellte
Systemuhr keine Freigabe verlängert. Ein `Instant` überlebt keinen Prozess und lässt sich
nicht serialisieren. Persistenz hieße: beim Schreiben in Wanduhrzeit umrechnen, beim Laden
zurück — und damit exakt die Uhrabhängigkeit einführen, die das Modul vermeidet. Kein
K.-o.-Argument (die Frist ist kurz, der Schaden begrenzt), aber der Kern der Entscheidung,
und so gehört er aufgeschrieben.

**Entwurf — dokumentieren, nicht bauen.** Zwei Stellen, keine Änderung an der Logik:

1. **Ein Absatz im Modulkopf von `temporary.rs`**, direkt nach dem bestehenden Absatz zur
   monotonen Zeit, etwa:

   > **Nichts davon überlebt einen Neustart, und das ist Absicht.** Die Einträge sind
   > befristet; sie zu persistieren hieße, ihre Frist in Wanduhrzeit umzurechnen und beim
   > Laden zurück — und damit genau die Uhrabhängigkeit einzuführen, die der Absatz darüber
   > vermeidet. Eine Freigabe, die einen Reboot überdauert, ist außerdem keine befristete
   > Freigabe mehr, sondern eine Allowlist mit Verfallsdatum; wer das will, trägt den Namen
   > in eine Allowlist ein.

2. **Ein Satz in der Oberfläche.** Das ist der eigentliche Fix des TODO: der Nutzer soll es
   nicht nach dem Reboot merken, sondern beim Klicken lesen. Im Panel der Freigaben, klein
   und in `--muted`: *"Freigaben gelten bis zum Ablauf oder bis zum nächsten Neustart des
   Dienstes."* Kostet eine Zeile HTML und beseitigt die Überraschung vollständig.

Dazu eine Fußnote in OPERATIONS.md §3 (Backup): Freigaben und Sperren sind nicht im Backup,
weil sie nicht auf Platte liegen.

**Falls doch Persistenz gewünscht ist** — dann so und nicht anders: geschrieben wird bei
jeder Änderung (nicht beim Shutdown; ein Absturz ist der Fall, für den man es baut) nach
`StateDirectory`, atomar über temporäre Datei plus `rename`. Gespeichert wird der
**Ablaufzeitpunkt als Wanduhrzeit**; beim Laden wird alles Abgelaufene verworfen und alles
Übrige auf `now() + Restlaufzeit` umgerechnet, gedeckelt auf `MAX_GRANT`. Springt die Uhr
zurück, ist dieser Deckel die einzige Absicherung — und deshalb nicht verhandelbar. Rund 80
Zeilen plus Tests. Der ehrliche Rat: nicht bauen, bevor sich jemand darüber beschwert hat.

**Aufwand:** S, eine Stunde für Variante 1+2. **Priorität: hoch** — der billigste Punkt der
Liste, und er beseitigt eine echte Überraschung.

---

## 6. Kontinuierliche Dependency-Triage

> cargo deny läuft, aber wer reagiert auf neue RUSTSEC-Advisories?

**Befund.** [.github/workflows/ci.yml](../.github/workflows/ci.yml) hat genau zwei Auslöser:
`push` auf `main` und `pull_request`. **Kein `schedule`.** Die Advisory-Datenbank ändert sich
aber, ohne dass jemand committet — in einem Repo, an dem abends unregelmäßig gearbeitet wird,
fällt ein neues Advisory folglich erst beim nächsten Commit auf, also womöglich Wochen
später. Das ist die eigentliche Lücke, und sie kostet fünf Zeilen.

Die zweite Hälfte ist der Triage-Weg, und dessen Muster existiert bereits und ist gut: die
Ausnahme für `RUSTSEC-2026-0009` in [deny.toml](../deny.toml) trägt Begründung,
Erreichbarkeitsanalyse des verwundbaren Pfads und eine **Umkehrbedingung** ("fällt weg,
sobald die MSRV auf 1.88 steigt"). Genau so gehört eine Ausnahme geschrieben. Was fehlt, ist
nur der Anlass, sie regelmäßig anzusehen.

**Entwurf — ein `schedule`-Job, kein Bot.**

```yaml
on:
  push:
    branches: [main]
  pull_request:
  schedule:
    - cron: '0 6 * * 1'     # montags 06:00 UTC
```

plus im `deny`-Job `issues: write` und ein Schritt, der bei Fehlschlag **ein** Issue anlegt —
oder ein bestehendes aktualisiert, statt jede Woche ein neues zu erzeugen.

**Warum kein Dependabot und kein Renovate.** Beide erzeugen Pull Requests. In einem
Ein-Personen-Projekt ist eine PR-Flut, die niemand merged, schlimmer als kein Bot: nach drei
Monaten stehen vierzig offene PRs, und die eine sicherheitsrelevante darunter geht unter —
die Meldung wird zu Rauschen, und Rauschen wird ignoriert. Dazu die Lieferkettenseite: ein
Bot mit Schreibrecht im Repo ist ein weiterer Weg, auf dem Code hereinkommt, ohne dass ein
Mensch ihn geschrieben hat. Für dieses Projekt gilt die Reihenfolge: **erst die Meldung, dann
vielleicht die Automatisierung.**

Falls es später doch Dependabot sein soll, dann so: `open-pull-requests-limit: 0` (nur
Security-Updates, keine Versionspflege), `groups` für die übrigen, `interval: monthly`. Das
ist der Kompromiss, der nicht zumüllt. Aber erst danach.

**Der Triage-Ablauf**, drei Sätze für OPERATIONS.md:

1. Issue kommt herein. Erste Frage: **Ist der verwundbare Pfad von hier aus erreichbar?**
   Beantwortet wird das wie bei der `time`-Ausnahme — nicht "steckt das Crate im Baum",
   sondern "wird die betroffene Funktion aufgerufen".
2. Erreichbar → beheben, notfalls durch Ersetzen der Abhängigkeit. Nicht erreichbar oder
   nicht behebbar → Eintrag in `deny.toml` mit Begründung **und Umkehrbedingung**.
3. Jede Ausnahme wird beim nächsten wöchentlichen Fehlschlag mitgelesen. Eine Ausnahme ohne
   Umkehrbedingung ist keine Ausnahme, sondern eine Kapitulation.

**Schritte.**

```
1. schedule-Trigger in ci.yml → verify: Actions zeigt nach dem ersten Montag einen Lauf ohne
   zugehörigen Commit
2. Issue-Schritt bei Fehlschlag des deny-Jobs → verify: Testlauf mit künstlich eingefügter
   verwundbarer Version legt genau ein Issue an; ein zweiter Lauf legt kein zweites an
3. Triage-Ablauf aufschreiben → verify: die drei Schritte stehen neben der bestehenden
   time-Ausnahme verlinkt
```

**Aufwand:** S, eine Stunde. **Priorität: hoch** — beste Wirkung pro Zeile in der ganzen
Liste.

---

## 7. Config-Versionierung

> Beim ersten breaking change (Key umbenennen) bricht eine alte Installation ohne
> Migrationspfad.

**Befund.** [config.rs](../crates/alpendns/src/config.rs) hat **kein** `version`-Feld und
benutzt an **keiner** Stelle `serde(alias)`. Beim Umbenennen eines Schlüssels bekommt der
Betreiber heute genau das, was serde sagt: `unknown field 'x'` — und dank
`deny_unknown_fields` scheitert der Start, was richtig ist (B.1 Regel 5), aber nicht verrät,
was zu tun ist.

**Der Punkt vermischt drei Dinge.** Sie sind zu trennen, sonst baut man das Falsche:

**(a) Bessere Fehlermeldung.** Bei einem unbekannten Schlüssel den ähnlichsten bekannten
vorschlagen ("unbekannter Schlüssel `blocklists` — meintest du `blocklist`?"). Ein
Levenshtein-Vergleich gegen die Feldliste, die serde im Fehler ohnehin mitliefert. Löst den
mit Abstand häufigsten Fall: den Tippfehler. **Aufwand: S.**

**(b) Umbenennungen ohne Bruch: `serde(alias)`.** Der alte Name bleibt eine Version lang als
Alias stehen und erzeugt beim Laden eine `tracing::warn!`-Zeile mit dem neuen Namen. `alias`
und `deny_unknown_fields` schließen einander **nicht** aus — ein Alias ist ein bekannter
Name, kein unbekannter. Das ist die 90-%-Lösung des eigentlichen TODO, und sie kostet je
Umbenennung eine Zeile. **Aufwand: S je Fall, null im Voraus.**

**(c) Ein `version`-Feld.** Klingt nach der Lösung, ist aber die schwächste der drei: es sagt
dem Server, was er ohnehin merkt, und verlangt vom Betreiber, eine Zahl zu pflegen, die er
nicht versteht. Nutzen hätte es nur mit einer echten Migration dahinter — und die ist hier
verbaut: `/etc/alpendns/alpendns.toml` ist ein **Debian-conffile**. Ein Programm, das eine
conffile-Datei selbsttätig umschreibt, bricht den conffile-Vertrag; `dpkg` fragt beim
nächsten Upgrade nach lokalen Änderungen, die der Betreiber nie gemacht hat. Automatische
Migration beim Start ist damit **ausgeschlossen**, nicht nur unschön.

**Empfehlung: (a) und (b) bauen, (c) nicht.** Kommt später doch eine große Umstellung, ist
der richtige Weg ein ausdrücklicher Befehl `alpendns migrate --in <alt> --out <neu>`, der
nach `stdout` oder in eine benannte Datei schreibt und die conffile-Datei nicht anfasst — der
Betreiber kopiert sie selbst. Dann ist der Vertrag gewahrt und die Migration sichtbar.

**Schritte.**

```
1. Feldvorschlag bei unbekanntem Schlüssel → verify: Test mit "blocklists" statt "blocklist"
   nennt den richtigen Namen im Fehlertext
2. Muster für Umbenennungen festhalten (serde(alias) + Warnung), mit einem Beispiel
   → verify: Test lädt Config mit altem Namen, Ergebnis identisch, Warnung wird geloggt
3. Entscheidung gegen version-Feld und automatische Migration als ADR → verify: ADR liegt in
   docs/adr/ und nennt den conffile-Vertrag als Grund
```

**Aufwand:** S, ein Abend für (a)+(b)+ADR. **Priorität: mittel** — (a) hilft ab sofort, (b)
ist Vorsorge, die nichts kostet, bis sie gebraucht wird.

---

## 8. Blockseite

> Wenn eine Website auf Blockliste steht → schöne Blockpage vom DNS-Server ("Diese Seite
> wurde blockiert weil…")

**Befund.** Der Mechanismus ist zum Teil schon da:
[filter/block.rs:26-44](../crates/alpendns/src/filter/block.rs#L26-L44) kennt drei Modi —
`Nxdomain` (Default), `ZeroIp` und **`Sinkhole`**, der bereits eine konfigurierte IPv4- und
IPv6-Adresse zurückgibt. Für eine Blockseite fehlt "nur" der HTTP-Server auf dieser Adresse.

**Und jetzt die unangenehme Seite, die vor dem Bau zu klären ist.** Eine Blockseite wirkt nur
bei **HTTP**. Bei HTTPS — also bei praktisch jedem Aufruf, den ein Mensch tätigt — sieht der
Nutzer keine Seite, sondern eine **Zertifikatswarnung**: der Sinkhole kann für
`www.beispiel.de` kein gültiges Zertifikat vorweisen. Bei einer Domain mit HSTS (alle großen)
bietet der Browser nicht einmal ein "trotzdem fortfahren" an, sondern bricht hart ab. Das
Ergebnis ist also nicht "schöne Seite statt Fehler", sondern **"Zertifikatsfehler statt
Namensfehler"** — für den Laien die schlechtere der beiden Meldungen, weil sie nach Angriff
aussieht.

Drei weitere Punkte, die real sind:

* **Nicht-Browser-Clients** — Apps, Update-Dienste, Telemetrie — sind die Mehrheit der
  geblockten Anfragen. Sie sehen keine Seite, sondern hängen im TCP-Verbindungsaufbau zum
  Sinkhole und laufen in Timeouts, statt sofort zu scheitern. `Nxdomain` ist für sie die
  freundlichere Antwort.
* **Wechselwirkung mit dem Rebinding-Detektor**
  ([detect/rebinding.rs](../crates/alpendns/src/detect/rebinding.rs)): der Sinkhole ist per
  Definition eine private Adresse als Antwort auf einen öffentlichen Namen — genau das
  Muster, das der Detektor meldet. Beide gleichzeitig aktiv heißt: jede geblockte Domain
  erzeugt einen Fund, und die Fundliste wird unlesbar. Das muss ausgenommen werden.
* **Die Begründung an die Seite zu bekommen** rührt an B.1 Regel 3. Der HTTP-Server bekommt
  vom Browser den `Host:`-Header, also den Namen — er müsste die Policy erneut befragen
  ("warum wäre dieser Name geblockt?"). Das geht ohne jede Speicherung über den bestehenden
  `explain`-Pfad ([api/mod.rs:386](../crates/alpendns/src/api/mod.rs#L386)), der genau diese
  Frage schon beantwortet: kein neuer Speicher, keine Verknüpfung von Name und Client.
  Wichtig: die Seite braucht `no-store`, sonst hält der Browser sie über das Ende der
  Blockierung hinaus.

**Empfehlung: die billigere Hälfte bauen, die teure nicht.** Was der Nutzer wirklich will,
ist nicht die Seite, sondern die **Antwort auf "warum geht das nicht?"** — und die gibt es
bereits über `explain`. Nur der Weg dorthin ist umständlich. Vorschlag:

* Ein Suchfeld in der Oberfläche, in das man einen Namen tippt und die Begründung bekommt
  ("geblockt durch Liste X, Regel Y"), daneben der bestehende Freigabeknopf. Der
  `explain`-Endpunkt kann das bereits; es fehlt der Weg in der UI.
* Wenn die Blockseite trotzdem gewünscht ist, dann **ausdrücklich als Option für HTTP-only**,
  mit einem Satz in der Doku, der die Zertifikatswarnung benennt, statt sie zu verschweigen.
  Ein Zertifikat aus einer eigenen CA, die auf allen Geräten ausgerollt wird, löst das
  technisch — und ist in einem Haushalt eine Zumutung.

**Schritte (für die empfohlene Hälfte).**

```
1. Suchfeld in der UI, das /api/explain befragt → verify: Eingabe eines geblockten Namens
   zeigt Liste und Regel; unbekannter Name zeigt "wird nicht geblockt"
2. Leerzustand des Felds nach B.6 (kein leerer Kasten, sondern ein Satz) → verify:
   ui.rs-Regeltests grün, danach ein Blick eines Menschen
3. Rebinding-Ausnahme für die konfigurierte Sinkhole-Adresse → verify: Test mit aktivem
   Sinkhole und aktivem Rebinding-Detektor erzeugt keinen Fund
```

**Aufwand:** Suchfeld S. Vollständige Blockseite M–L plus ADR, mit zweifelhaftem Ertrag.
**Priorität: niedrig** — und das ist eine bewusste Empfehlung gegen den ursprünglichen
Wunsch, keine Vergesslichkeit. **B.8-Entscheidung beim Autor.**

---

## 9. Configuration Wizard in der Web-UI

**Befund.** Die API ist heute lesend plus vier schreibende Endpunkte für befristete Einträge
(`/api/allow` und `/api/deny`, je POST und DELETE,
[api/mod.rs:110-131](../crates/alpendns/src/api/mod.rs#L110-L131)), abgesichert durch ein
Bearer-Token, in konstanter Zeit verglichen. **Konfiguration kann sie nicht schreiben.**

Drei Hindernisse, alle echt:

1. **`ProtectSystem=strict`** macht das ganze Dateisystem außer den `*Directory=`-Pfaden
   schreibgeschützt. `/etc/alpendns/` ist **nicht** darunter. Ein schreibender Wizard
   verlangt also `ReadWritePaths=/etc/alpendns` — eine Aufweichung der Härtung, die nach B.5
   begründet und abgenommen gehört.
2. **Kein Reload** (Befund B1). Selbst eine geschriebene Config wirkt erst nach `systemctl
   restart` — und den kann der Dienst nicht selbst auslösen, ohne einen Weg zu systemd zu
   bekommen, den `SystemCallFilter` und `CapabilityBoundingSet` gerade verhindern. Ein
   Wizard, der schreibt und dann sagt "bitte jetzt neu starten", ist ein halber Wizard.
3. **conffile.** Dieselbe Falle wie in Punkt 7.

**Entwurf — ein Wizard, der nicht schreibt.** Das klingt nach Rückzug und ist die bessere
Lösung: der Wizard führt durch die Fragen (Listener-Adressen, Upstreams, Blocklisten, erste
Policy) und erzeugt daraus **eine TOML-Vorschau** mit Kommentaren, zum Kopieren. Daneben
stehen die drei Befehle:

```
sudo tee /etc/alpendns/alpendns.toml     # Inhalt einfügen
sudo alpendns -c /etc/alpendns/alpendns.toml check
sudo systemctl restart alpendns
```

Damit sind alle drei Hindernisse umgangen, und der Betreiber sieht, was er einbaut — bei
einer Datei, deren Inhalt darüber entscheidet, ob sein Netz gefiltert ist, ist das keine
Unbequemlichkeit, sondern der Punkt. Die **Validierung** läuft trotzdem serverseitig: ein
Endpunkt `POST /api/config/validate` nimmt den TOML-Text, parst ihn durch dieselbe
`Config`-Struktur und dieselben Blueprint-Prüfungen wie `alpendns check` und antwortet mit
Fehlern samt Zeilennummer — ohne irgendetwas zu speichern. So ist die Vorschau nachweislich
gültig, bevor sie jemand einfügt.

Gestaltung nach B.6: die Seite scrollt nicht, ein mehrstufiger Wizard also als **eine Ansicht
mit Schrittanzeige**, nicht als lange Formularstrecke — links die Frage, rechts die wachsende
Vorschau. Ein Container, kein neuer.

**Wenn doch geschrieben werden soll**, dann in dieser Reihenfolge und nicht anders:
`ReadWritePaths=/etc/alpendns` in die Unit (mit Begründung in OPERATIONS.md §5), Schreiben
atomar über temporäre Datei plus `rename`, vorher Kopie nach `alpendns.toml.bak-<zeitstempel>`
im StateDirectory, Validierung **vor** dem Schreiben, und die conffile-Frage ausdrücklich
entschieden (etwa: die Datei aus dem conffile-Satz nehmen und stattdessen im `postinst`
anlegen, wenn sie fehlt).

**Schritte.**

```
1. POST /api/config/validate, parst und verwirft → verify: gültiges TOML → 200 mit
   Zusammenfassung; unbekannter Schlüssel → 400 mit Schlüsselname und Zeile; nichts auf Platte
2. Obergrenze auf die Bodygröße des Endpunkts → verify: 10-MB-Body wird abgelehnt, kein Panic
3. Wizard-Ansicht mit Schrittanzeige und wachsender Vorschau → verify: ui.rs-Regeltests grün;
   Seite scrollt bei 1400 px Breite nicht
4. Kopierknopf und die drei Befehle darunter → verify: Blick eines Menschen
5. [nur falls Schreiben gewünscht] B.8-Entscheidung zu ReadWritePaths und conffile → verify:
   der Autor hat entschieden; ohne Entscheidung wird nicht gebaut
```

**Aufwand:** M, zwei bis drei Abende für die schreibfreie Fassung. **Priorität: niedrig** —
sie ist eine Komfortfunktion für die Erstinstallation, also für ein Ereignis, das pro
Betreiber einmal stattfindet.

---

## 10. Ausfallsicherheit mit zwei Instanzen

> Ausfallsicherheit steht als Phase-10-Option (zwei Instanzen).
> [ROADMAP.md](ROADMAP.md): "Zwei Instanzen mit abgeglichenem Policy-Stand."

Der größte Punkt der Liste, deshalb ausführlicher — und von hinten aufgerollt: erst die
Frage, welche Ausfälle es überhaupt gibt, dann was davon eine zweite Instanz löst, dann erst
das Wie.

### 10.1 Welche Ausfälle gibt es, und was fängt sie heute schon ab?

| Ausfall | Heute abgefangen durch | Zweite Instanz hilft? |
|---|---|---|
| Prozessabsturz | `Restart=on-failure`, `RestartSec=2s` | **Nein.** Der Dienst ist in ~2 s zurück. Kein Stub-Resolver schaltet in 2 s um. |
| Absturzschleife (5×/300 s) | `StartLimitBurst=5` → Dienst bleibt unten | **Ja**, aber nur auf einem zweiten Host. |
| Kaputte Config nach Änderung | `ExecStartPre=alpendns check` — der alte Prozess läuft weiter | Teilweise: nur wenn **nacheinander** geändert wird. |
| Kaputtes Blocklisten-Update | Platten-Cache, `Origin::StaleCache` | **Nein.** Beide Instanzen laden dieselbe URL. Dagegen hilft Diff-Review (Roadmap O3), nicht Redundanz. |
| Upstream tot | Pool mit mehreren Upstreams, `serve_stale` ([cache.rs](../crates/alpendns/src/cache.rs)) | **Nein**, außer man konfiguriert die Pools bewusst verschieden. |
| Paket-Upgrade | — | **Ja.** Der Fall mit dem besten Verhältnis: nacheinander upgraden, nie sind beide unten. |
| Reboot des Hosts | — | **Ja**, zweiter Host. |
| Hardware / Strom | — | **Ja**, zweiter Host — und nur bei getrennter Stromversorgung. Zwei Kisten an derselben Steckdosenleiste sind eine Kiste. |
| Netzpartition | — | Kommt darauf an, wo. Nur mit zweitem Segment. |

**Die ehrliche Bilanz:** von neun Ausfallarten fängt eine zweite Instanz drei ab, und alle
drei setzen einen **zweiten Host** voraus. Eine zweite Instanz auf demselben Rechner ist fast
wertlos — der häufigste Ausfall (Absturz) ist von systemd schneller behoben, als ein Client
umschaltet. **Wer das baut, baut einen zweiten Rechner, nicht einen zweiten Prozess.** Das ist
der wichtigste Satz dieses Abschnitts.

### 10.2 Die Stelle, an der es meistens scheitert: die Client-Seite

Zwei Server nützen nichts, wenn die Clients nicht umschalten. Das ist der Teil, der nicht im
Code steht und den man **vor** dem Bau messen muss:

* **glibc** (`resolv.conf`): probiert die Server der Reihe nach, Default `timeout:5`,
  `attempts:2`, und **merkt sich den Ausfall ohne `options rotate` nicht**. Der Ausfall von
  Server 1 heißt dann: jede Anfrage wartet fünf Sekunden, bevor Server 2 drankommt. Das ist
  nicht "unbemerkt", das ist "es geht, aber alles hängt".
* **systemd-resolved** bleibt nach wenigen Sekunden am funktionierenden Server kleben —
  deutlich besser.
* **Windows** kennt bevorzugten und alternativen Server mit einem Ausfallgedächtnis in
  Minuten. **Android** fällt mit kurzen Timeouts am unauffälligsten um.
* **Der häufigste Fall im Homelab ist keiner davon:** alle Geräte fragen den Router, der
  Router forwardet an AlpenDNS. Dann zählt allein das Failover **des Routers** — und viele
  Consumer-Router führen entweder nur einen Forwarder, oder sie fallen beim Ausfall auf den
  Provider-DNS zurück. Im zweiten Fall ist die Folge des Ausfalls **ungefiltertes
  Klartext-DNS**, und der zweite Server wird nie gefragt. Das ist schlimmer als ein sichtbarer
  Ausfall.
* Ein Gerät mit bestehender DHCP-Lease erfährt von der zweiten Adresse erst beim nächsten
  Renew — unter Umständen stundenlang gar nicht.

**Deshalb ist Schritt eins keine Zeile Code, sondern eine Messung.** Ohne sie weiß man nicht,
ob man ein Problem löst oder eines dazubaut.

### 10.3 Was abgeglichen werden müsste — und was ausdrücklich nicht

| Zustand | Wo | Abgleichen? |
|---|---|---|
| Konfiguration, Policies, Clients | Datei, `Config::load` | **Nein — von Hand gleich halten.** Ein Abgleichkanal, der Config trägt, verteilt eine kaputte Config auf beide Instanzen und macht aus zwei Ausfallzonen eine. Das widerspricht dem Zweck (B.1 Regel 5 und 6). |
| Blocklisten | eigener Download je Instanz | Nein. Gleicher Inhalt, unabhängig geladen. |
| Antwort-Cache | RAM | Nein. Divergenz ist folgenlos. |
| Protokoll, Ringpuffer, Zähler, 24-h-Verlauf | RAM, [history.rs:10-14](../crates/alpendns/src/history.rs#L10-L14) | Nein — und zwar **prinzipiell nicht**: der Modulkopf nennt die Flüchtigkeit ausdrücklich als Zusicherung, nicht als Mangel. Folge: jede UI zeigt die Hälfte des Verkehrs. Das gehört in die UI geschrieben. |
| Rate-Limit-Buckets | RAM, je Instanz | Nein. Aber: zwei Instanzen verdoppeln effektiv das Budget je Client. `per_client_qps` gehört dann auf beiden halbiert — und nichts im Code erzwingt das. |
| Tunneling-Fenster | RAM, je Instanz | **Kann nicht.** Es ist Verkehrsstatistik über Namen, also genau das, was nach B.1 Regel 3 nicht über die Leitung soll. Folge: jeder Detektor sieht die Hälfte des Verkehrs und wird stumpfer. **Redundanz kostet hier Erkennungsqualität.** Dafür gibt es keine gute Antwort, nur die ehrliche Erwähnung. |
| NRD-Datei | Datei aus AlpenShield, [detect/nrd.rs](../crates/alpendns/src/detect/nrd.rs) | Kein Sync-Problem, sondern Betriebsarbeit: die Datei muss auf beiden Hosts liegen. Der Detektor lernt nichts, er liest nur — eine frisch gestartete Instanz entscheidet identisch. Fehlt die Datei, flaggt er dort schlicht nichts. |
| **Befristete Freigaben und Sperren** | RAM, `Instant`, [temporary.rs](../crates/alpendns/src/policy/temporary.rs) | **Der einzige Kandidat.** Es ist der einzige Zustand, den ein Mensch in der UI erzeugt und der sonst nirgends steht. |

Damit ist die Aufgabe scharf: "abgeglichener Policy-Stand" aus der Roadmap heißt in der Praxis
**eine einzige Datenart**, nicht "Zustandsreplikation".

### 10.4 Stufenplan

Jede Stufe bringt für sich einen Gewinn, und nach jeder kann man aufhören.

**Stufe 0 — messen und dokumentieren. Kein Code.**

Der zweite Server wird eingerichtet, bevor irgendetwas gebaut wird, und dann wird gemessen:
Instanz A abschalten, mit der Stoppuhr feststellen, wie lange Laptop, Handy, Fernseher und
Router brauchen — oder ob sie es überhaupt tun. Die Zahl gehört in
[BENCHMARKS.md](BENCHMARKS.md); sie ist die Rechtfertigung für alles Weitere. Ergibt die
Messung "der Router fällt auf den Provider-DNS zurück", dann ist **das** das Problem, das
gelöst gehört, und nicht der Zustandsabgleich.

```
1. Zweiten Host aufsetzen, dieselbe Config von Hand → verify: dig gegen beide liefert dieselbe
   Antwort für einen geblockten und einen erlaubten Namen
2. Beide Adressen per DHCP verteilen → verify: resolv.conf/ipconfig auf drei Geräten zeigt beide
3. Instanz A abschalten, Umschaltzeit je Gerätetyp messen → verify: Zahlen in BENCHMARKS.md,
   inklusive des Falls "Router fällt auf Provider-DNS zurück"
4. Abschnitt "Zwei Instanzen" in OPERATIONS.md: Reihenfolge beim Upgrade, per_client_qps
   halbieren, NRD-Datei auf beide Hosts → verify: jemand richtet danach ein zweites System ein,
   ohne nachzufragen
```

**Gewinn:** Reboot, Upgrade, Hardwareausfall — die drei Fälle aus 10.1. **Aufwand:** ein
Abend, keine Zeile Code. Für die meisten Homelabs endet der Punkt hier, und das ist ein gutes
Ergebnis.

**Stufe 1 — merken, dass die Instanzen auseinanderlaufen.**

Der stille Ausfall von Stufe 0 ist eine zweite Instanz, die seit vier Monaten mit einer
veralteten Konfiguration läuft, ohne dass es jemand merkt. Dagegen genügt eine **Prüfsumme**:
`/api/status` und die UI nennen einen Hash über die geladene Konfiguration (Config-Datei plus
Listen-Zusammenfassung), und der Betreiber vergleicht ihn zwischen beiden Instanzen — im
einfachsten Fall mit dem Auge.

```
1. Hash über die geladene Config in /api/status → verify: gleiche Datei → gleicher Hash; ein
   geänderter Schlüssel → anderer Hash
2. Hash klein im Kopf der Oberfläche → verify: ui.rs-Regeltests grün
```

**Gewinn:** der leise Konfigurationsdrift wird sichtbar. **Aufwand:** S, eine Stunde. Bester
Ertrag pro Zeile im ganzen Punkt 10.

**Stufe 2 — Freigaben auf beiden Instanzen, ohne Serveränderung.**

Der konkrete Ärger aus 10.3 ist: der Nutzer gibt eine Seite frei, sie geht auf dem Laptop und
nicht auf dem Handy — weil das Handy gerade Instanz B fragt. Die billigste ehrliche Lösung
verlagert die Konsistenz in die Oberfläche statt in den Server: **die Web-UI kennt die Adresse
der zweiten Instanz und schickt jeden Schreibvorgang an beide.** Ein Feld in der
Konfiguration, zwei `fetch` statt einem, und eine Rückmeldung, wenn nur einer geklappt hat.

Das kostet keine neue Server-Schnittstelle, kein Protokoll, keine Konfliktauflösung, und es
hat einen ehrlichen Fehlermodus: schlägt einer der beiden fehl, **sagt es die Oberfläche**,
statt still zu divergieren. Nachteil: es wirkt nur, wenn jemand die UI benutzt; eine Instanz,
die zwischendurch neu startet, holt nichts nach.

```
1. Optionale zweite API-Adresse in der UI-Konfiguration → verify: ohne den Wert verhält sich
   alles wie bisher
2. Schreibvorgänge (allow/deny, setzen und widerrufen) an beide → verify: Test mit zwei
   Loopback-Instanzen; nach dem Klick steht der Eintrag auf beiden
3. Teilfehler wird angezeigt → verify: zweite Instanz nicht erreichbar → die Oberfläche sagt,
   auf welcher der Eintrag fehlt, statt Erfolg zu melden
```

**Gewinn:** der konkrete Nutzerärger ist weg. **Aufwand:** S–M, ein Abend, fast alles in
`web/app.js`.

**Stufe 3 — echter Peer-Abgleich der befristeten Einträge.**

Erst wenn Stufe 2 nachweislich nicht reicht (eine Instanz startet neu und verliert den Stand;
Einträge werden auch außerhalb der UI gesetzt), lohnt der gebaute Abgleich. Der Entwurf in
Kürze:

* **Kein Leader.** Bei zwei Knoten gibt es kein Quorum, und eine feste Leader-Rolle verlöre
  die Schreibfähigkeit genau dann, wenn der Leader ausfällt — also im einzigen Fall, für den
  man das Ganze baut.
* **Ein Endpunkt**, `POST /api/peer/state`, hinter der **bestehenden** Bearer-Auth
  ([api/mod.rs:145-186](../crates/alpendns/src/api/mod.rs#L145-L186)) und demselben Token
  (beide Instanzen bekommen dieselbe Token-Datei; sie steht in OPERATIONS.md §3 ohnehin auf
  der Backup-Liste). Kein zweites Geheimnis — ein zweites Geheimnis ist das, was man beim
  Neuaufsetzen vergisst.
* **Push und Pull sind dieselbe Runde:** der Anrufer schickt seinen vollständigen Zustand, der
  Angerufene merged und antwortet mit seinem Zustand nach dem Merge. Ein reiner Pull ist ein
  Push mit leerer Liste. Übertragen wird der volle Zustand, kein Delta — er ist klein (Frist
  höchstens `MAX_GRANT`), und ein volles Bild kennt keinen verlorenen Delta-Eintrag.
* **Das `Instant`-Problem wird umgangen, nicht gelöst:** übertragen werden
  **Restlaufzeiten**, nie Zeitpunkte. Der Empfänger rechnet `clock.now() + rest` — exakt das,
  was `grant()` heute schon tut. Ein `Instant` verlässt den Prozess nie, es wird keine Wanduhr
  verglichen, und eine verstellte Uhr auf dem Peer ist folgenlos. Der Fehler ist die
  Übertragungslaufzeit auf einer Frist von Stunden.
* **Widerruf ist ein Grabstein**, kein Löschen, mit derselben Restlaufzeit wie der widerrufene
  Eintrag. Ohne das bringt der nächste Merge die widerrufene Freigabe zurück. Der Merge ist
  damit kommutativ, idempotent und assoziativ und braucht weder Journal noch
  Nachlaufprotokoll.
* **Der Kanal trägt Domainnamen** — also genau den Datentyp, den B.1 Regel 3 schützt. Deshalb:
  die Peer-Adresse muss Loopback sein, der Weg zum anderen Host führt durch einen Tunnel, den
  der Betreiber legt (WireGuard, SSH), und `alpendns check` weist eine Nicht-Loopback-Adresse
  ab. Eine im LAN im Klartext lauschende API wäre die falsche Antwort — sie liefert über
  `/api/recent` ohnehin Namen aus.
* **B.1 Regel 4 gerät unter Druck.** "Der Prozess kontaktiert genau drei Sorten Ziele:
  konfigurierte Upstream-Resolver, konfigurierte Blocklisten-URLs, und sonst nichts." Ein
  konfigurierter Peer ist eine vierte Sorte. Er ist keine Telemetrie — er geht an ein Ziel, das
  der Betreiber selbst benannt hat, und trägt nichts nach außen. Aber die Regel sagt "sonst
  nichts", und sie zu ergänzen ist eine **B.8-Entscheidung des Autors, kein Agentenbeschluss.**
  Ohne diese Abnahme wird Stufe 3 nicht gebaut.
* **Der Endpunkt verarbeitet Netzwerkdaten**, also gilt B.1 Regel 1 voll: Obergrenzen auf
  Bodygröße und Einträgezahl, dieselbe `parse_grant`-Prüfung und dieselbe
  `MAX_GRANT`-Kappung wie beim UI-Endpunkt, sättigende Arithmetik, kein Slice-Indexing.
  Maximaler Schaden eines übernommenen Peers: ein für höchstens `MAX_GRANT` freigegebener oder
  gesperrter Name. Er kann keine Config, keine Liste und keinen Detektor anfassen — das ist die
  Begründung dafür, dass das Protokoll bewusst **keine** Felder für Konfiguration hat.

Berührte Dateien: `policy/temporary.rs` (Eintrag wird von `Instant` zu einem kleinen Record mit
Frist, Grabstein-Flag und Zähler; dazu `export()` und `merge()`), `policy/mod.rs`
(Durchreichen), `api/mod.rs` (Route, DTOs, Obergrenzen), ein neues `peer.rs` (Task: `Notify`
bei lokaler Änderung mit Entprellung, dazu ein Ticker), `config.rs` (`[peer] url`), `main.rs`
(Verdrahtung, `check`-Auflagen), `api/ui.rs` (Erreichbarkeitspunkt), `metrics.rs` (zwei Zähler,
keine Namen). Nicht angefasst: `cache.rs`, `filter/*`, `ratelimit.rs`, `history.rs`,
`logging/*`, `detect/*` — sie halten Zustand, der bewusst divergiert.

**Aufwand:** L, drei bis vier Abende, plus ADR-0021. **Priorität: niedrig** — der abgeglichene
Zustand umfasst in einem Haushalt vielleicht ein Dutzend Einträge pro Woche.

### 10.5 Was an diesem Vorhaben schwach bleibt

Der Vollständigkeit halber, weil es zur Entscheidung gehört:

* Der Nutzen ist schmal, und der häufigste Ausfall wird davon nicht berührt.
* Der Peer-Kanal hängt an einem Tunnel, den AlpenDNS weder baut noch überwacht. Bricht er,
  divergiert der Zustand still; der Punkt in der UI mildert das, beseitigt es nicht.
* Der geteilte Token macht jede Instanz zum vollwertigen Leser der anderen. Wer eine Kiste
  übernimmt, hat beide Oberflächen inklusive der Namen im Ringpuffer.
* Der Merge schreibt in denselben Mutex, den der Anfragepfad über `check()` liest. Bei ein paar
  Dutzend Einträgen ist das messbar nichts, aber es ist genau die Sorte Schreiblast im
  Anfragepfad, die B.3 Regel 5 im Blick hat.
* Die Heuristiken werden stumpfer (10.3). Dafür gibt es keine Lösung, nur die Erwähnung.

### 10.6 Offene Entscheidungen (B.8)

1. Wird B.1 Regel 4 um den konfigurierten Peer ergänzt? Ohne diese Abnahme entfällt Stufe 3.
2. Wird der Reload aus ARCHITECTURE.md §7 gebaut oder der Absatz zurückgezogen (Befund B1)?
3. Ist eine Nicht-Loopback-Peer-Adresse ein Startfehler oder eine Warnung? Empfehlung: Fehler.

---

## Reihenfolge und Bündel

**Zuerst, weil billig und mit echter Wirkung:**

1. **Punkt 5** (ein Satz in der UI, ein Absatz im Modulkopf) — eine Stunde.
2. **Punkt 6** (`schedule`-Trigger) — eine Stunde, beste Wirkung pro Zeile.
3. **Punkt 1** (Slowloris) — **erledigt.** Ein Abend, der einzige Punkt, der eine Lücke
   schließt.
4. **Punkt 4** (`After=time-sync.target` plus Runbook-Absatz) — ein halber Abend.

**Danach, nach Bedarf:**

5. **Punkt 10, Stufe 0 und 1** — messen, dokumentieren, Config-Hash. Die Messung entscheidet,
   ob überhaupt weitergebaut wird.
6. **Punkt 2** (Log-Rotation) — sobald jemand `mode = "full"` einschaltet, vorher nicht.
7. **Punkt 7** (a und b: Feldvorschlag und `serde(alias)`-Muster).
8. **Punkt 3** — und zwar (b) vor (a): der Refresh-Task ist der eigentliche Punkt, die
   Altersanzeige ist erst danach ehrlich.

**Zuletzt oder gar nicht:** Punkt 9 (Wizard), Punkt 8 (Blockseite), Punkt 10 Stufe 3.

**Was zusammen erledigt wird:**

* **Punkt 2 + Befund B3:** der `BufWriter` gehört in denselben Griff wie die Rotation.
* **Punkt 3(b) + Punkt 10 Stufe 1:** beides beantwortet "läuft diese Instanz noch mit dem
  richtigen Stand?" — Listenalter und Config-Hash gehören in denselben Kopf der UI.
* **Punkt 7 + Punkt 9:** beide betreffen den Weg, auf dem Konfiguration ins System kommt. Der
  Validierungs-Endpunkt aus Punkt 9 ist derselbe Pfad wie der Feldvorschlag aus Punkt 7 —
  einmal bauen, zweimal benutzen.
* **Punkt 8 + Rebinding-Detektor:** die Sinkhole-Ausnahme ist auch ohne Blockseite fällig,
  sobald jemand `block_mode = "sinkhole"` benutzt. Das ist unabhängig vom Rest von Punkt 8 zu
  prüfen.

**Was sich widerspricht:** Punkt 9 (Wizard schreibt Config) und Punkt 7 (`alpendns migrate`
schreibt nichts) treffen beide auf den Debian-conffile-Vertrag. Die Entscheidung "schreibt
AlpenDNS jemals selbst in `/etc/alpendns/`?" ist **einmal** zu treffen und gilt dann für beide.

---

## Die ursprüngliche Liste

Zum Nachschlagen, unverändert:

- TCP-Slowloris — tcp.rs:86: Timeout auf den Body-Read + ein Cap auf gleichzeitige Verbindungen. Im LAN reicht ein kompromittiertes Gerät, um den Resolver lahmzulegen.
- Configuration Wizard in WebUI
- Log Rotation im full Mode: logging/mod.rs schreibt append-only in eine Datei, die der Prozess selbst öffnet — systemd/journald greift da nicht. Über Monate wächst das Query-Log unbegrenzt bis zur vollen Platte. Braucht Rotation (Größe/Alter)
- Wenn eine Website auf Blockliste steht -> Schöne Blockpage vom DNS server (Diese Seite wurde blockiert weil...)
- Uhrzeit Abhängigkeit dokumentieren: Die eigene Validierung (ADR-0016) setzt eine korrekte Host-Uhr voraus — eine falsche Uhr macht jede signierte Zone „bogus". Für den Betrieb gehört ein Satz ins Runbook: NTP/systemd-timesyncd ist Voraussetzung, nicht optional.
- Bei Blocklisten "Alter" festhalten im UI, zuletzt aktualisiert vor (2 Tagen...)
- Config Versionierung umsetzen: Heute ist die Config die Spezifikation (deny_unknown_fields). Beim ersten breaking change (Key umbenennen) bricht eine alte Installation ohne Migrationspfad. Für v1 kein Blocker, aber ein version-Feld + explizite Fehlermeldung wäre die Investition, die später teuer wird, wenn man sie nicht früh gemacht hat.
- Grants/Denials sind nur im RAM (temporary.rs). Ein Neustart verliert sie. Das ist vertretbar (sie sind befristet), aber es gehört als bewusste Entscheidung dokumentiert — sonst wundert sich jemand nach dem Reboot, warum die Freigabe weg ist.
- Dependency-Updates: cargo deny läuft, aber wer reagiert auf neue RUSTSEC-Advisories? Dependabot/Renovate einrichten, damit der Triage-Prozess für Ausnahmen (wie die dokumentierte time-Ausnahme) kontinuierlich ist statt einmalig.
- Ausfallsicherheit steht als Phase-10-Option (zwei Instanzen)
