# ADR-0016: DNSSEC selbst validieren, auch als Forwarder

**Status:** angenommen · **Datum:** 2026-08-30 · **Betrifft:** [ADR-0002](0002-hickory-proto-statt-eigenem-parser.md), [ADR-0003](0003-forwarder-first.md), [THREAT-MODEL.md](../THREAT-MODEL.md)

## Kontext

AlpenDNS ist ein Forwarder. Die Antwort kommt von Quad9, Mullvad oder wem sonst,
und mit ihr ein AD-Bit — ein einzelnes Bit, das genau der Rechner gesetzt hat,
dem gegenüber der ganze Rest des Projekts Zurückhaltung übt. Verschlüsselte
Transporte, ECS strippen, `split_by_zone`: alles davon geht davon aus, dass ein
Upstream neugierig sein könnte. Beim AD-Bit hieß es bisher: wird schon stimmen.

THREAT-MODEL.md hat das seit Phase 1 als offenen Punkt geführt: *"ohne
DNSSEC-Validierung vertraut AlpenDNS dem Upstream. Ein kompromittierter Upstream
kann lügen."* ADR-0003 hat beim Verzicht auf Rekursion ausdrücklich vermerkt,
dass eigene Validierung auch im Forwarder-Modus möglich und sinnvoll ist.

## Entscheidung

**AlpenDNS rechnet die Signaturkette selbst nach, ab den einkompilierten
Root-Schlüsseln.** Per Default an (`privacy.dnssec = true`). Eine Antwort, deren
Zone sich als signiert ausweist und deren Kette nicht schließt, wird verworfen;
der Client bekommt SERVFAIL. Das ist das Standardverhalten nach RFC 4035 §5.5
und das, was `unbound` und `knot-resolver` tun.

Die Kryptografie kommt aus `hickory-net`/`hickory-proto` (`dnssec-ring`), aus
derselben Begründung wie ADR-0002 für das Wire-Format: eine
Signaturprüfungskette selbst zu schreiben ist die Sorte Code, bei der ein Fehler
nicht auffällt, weil das falsche Ergebnis genauso aussieht wie das richtige.
Unser Anteil steht in `crate::dnssec` und ist die *Auswertung*: aus vielen
Record-Stempeln ein Urteil je Antwort, und die Folgerung daraus.

Drei Folgeentscheidungen, die von außen willkürlich aussehen:

**1. Zusammengefasst wird pessimistisch.** Ein einziger `Bogus`-Record macht die
ganze Antwort faul. Sonst könnte ein Angreifer einen gefälschten Record neben
echte hängen und käme durch. Angesehen werden Answer- **und**
Authority-Abschnitt: eine negative Antwort trägt ihren Beweis in den
NSEC-Records der Authority, und würde man nur den Answer-Abschnitt prüfen, käme
jedes gefälschte NXDOMAIN als „keine Aussage“ durch.

**2. `Bogus` ist terminal, es wird kein zweiter Upstream gefragt.** Naheliegend
wäre das Gegenteil — vielleicht lügt ja nur einer. Dagegen stehen zwei Dinge.
Eine Zone mit kaputter Signatur ist bei *jedem* Anbieter kaputt, der Zweitversuch
brächte also fast immer dasselbe Ergebnis; und er zeigte den Namen einem weiteren
Anbieter, also genau das, was `split_by_zone` verhindern soll. Aus demselben Grund
zählt `Bogus` **nicht** als Fehlversuch für die Ausfallerkennung: sonst könnte
eine einzige kaputte Zone nach drei Anfragen den ganzen Pool als tot markieren
und einen selbstgemachten Ausfall auslösen.

**3. Die Signaturen gehen nur an einen Client mit DO-Bit,** und das
Wegräumen passiert an der **Außenkante, hinter dem Cache** (`dnssec::for_client`
in `server::handle_request`). Beides ist erst im Betrieb richtig geworden, siehe
unten. Das AD-Bit in unserer Antwort steht für *unser* Urteil und geht nur an
einen Client, der DO oder AD gesetzt hat (RFC 6840 §5.8).

## Preis

**`time` ist jetzt eine Produktionsabhängigkeit, und damit ändert sich die
Begründung einer Advisory-Ausnahme.** `hickory-proto/dnssec-ring` zieht über
sein internes Feature `__dnssec` das Crate `time` herein. Bis Phase 7 kam `time`
nur über `rcgen` und damit als dev-dependency; die Ausnahme für RUSTSEC-2026-0009
in `deny.toml` lautete deshalb „steckt gar nicht im Binary“. Das stimmt nicht
mehr.

Die Ausnahme bleibt trotzdem, mit engerer Begründung: das Advisory betrifft das
*Parsen* von RFC-2822-Datumszeichenketten aus fremder Eingabe. hickory benutzt
aus `time` ausschließlich `OffsetDateTime`, in den Konstruktoren von RRSIG und
SIG; die Zeitstempel darin kommen als 32-Bit-Zahlen vom Draht, nicht als
Zeichenkette. Der verwundbare Pfad wird nicht betreten. Der vollständige Text
samt Umkehrbedingung steht in `deny.toml`.

Die Alternative wäre gewesen, die MSRV von 1.85 auf 1.88 zu heben — dann löst
der Resolver `time 0.3.55` auf und die Ausnahme fällt ersatzlos weg. Dagegen
stand die Debian-Paketierung aus Phase 9: Debian 13 liefert `rustc 1.85`, mit
1.88 ließe sich das `.deb` nicht mehr mit dem Compiler der Distribution bauen.
Sobald die MSRV aus anderem Grund steigt, verschwindet die Ausnahme.

**Zusätzliche Anfragen.** Für jede neue Zone holt der validierende Griff
DNSKEY- und DS-Sätze nach, über dieselbe Verbindung zu demselben Upstream. Ein
Cache darüber liegt in `DnssecDnsHandle`. Es sind trotzdem mehr Anfragen als
vorher, und die erste Auflösung in einer Zone dauert länger.

**Der Cache hält mehr Bytes als vorher.** Er speichert die Antwort samt
Signaturen, damit ein Client mit DO sie noch bekommen kann; gestutzt wird erst
beim Ausliefern. Der Preis ist ein paar hundert Byte je signiertem Eintrag.

## Was geprüft ist

`crates/alpendns/tests/dnssec.rs` fährt drei Vektoren mit **echten** Signaturen
durch dieselbe Prüfung, die im Betrieb läuft: gültige Signatur → `Secure` und
die Antwort geht durch; verdrehtes Bit in der Signatur → `Bogus` und die Antwort
wird verworfen; fehlende Signatur in einer nachweislich signierten Zone →
ebenfalls `Bogus`.

Der Aufbau pinnt den Schlüssel der Testzone als Trust Anchor, statt eine Kette
bis zur echten Root zu bauen — `DnssecDnsHandle` hört bei einem Schlüssel aus
dem Anchor-Store auf, nach DS-Records zu suchen. An der Rechnerei ist dabei
nichts abgekürzt.

Dass der validierende Griff im Transport wirklich vorgeschaltet ist, hält
`encrypted.rs::dnssec_sets_the_do_bit_and_asks_for_the_chain` fest: mit
`dnssec = true` steht das DO-Bit auf dem Draht und es gehen Kettenabfragen
raus, ohne geht genau eine Frage raus und das DO-Bit fehlt.

## Was der erste Lauf gegen echte Upstreams gezeigt hat

Drei Dinge, die kein Unit-Test gefunden hätte, weil sie alle drei an der
Wirklichkeit hängen. Sie stehen hier, weil sie erklären, warum der Code an
diesen Stellen so aussieht.

**`dnssec-failed.org` ergab SERVFAIL, aber der Zähler blieb auf null.** Das
Ergebnis stimmte, die Buchführung nicht: wenn der Upstream selbst validiert,
kommt gar keine Antwort mit Records zurück, sondern ein leeres SERVFAIL. Der
NSEC-Beweis geht dann nicht auf, und `hickory` liefert das als **Fehler**
(`DnsError::Nsec { proof, response }`) statt als gestempelte Nachricht. Damit
lief es an der Auswertung vorbei: der Zähler blieb stehen, dem Upstream wurde
ein Fehlversuch angerechnet, die Verbindung wurde verworfen und der nächste
Anbieter bekam dieselbe Frage vorgelegt — genau die drei Dinge, die oben
ausgeschlossen sind. `dnssec::from_error` fängt den Fall jetzt ab und holt das
Urteil samt Antwort aus dem Fehler.

**`dig` bekam die ganze Signaturkette, ohne danach gefragt zu haben.** Die
Unterscheidung war falsch: das AD-Bit in einer *Anfrage* heißt nach RFC 6840
§5.7 "sag mir dein Urteil", nicht "schick mir die Kette" — und `dig` setzt es
per Default. Nur das DO-Bit fordert die Records an. Seither trennt
`client_wants_records` (DO) von `client_wants_verdict` (DO oder AD).

**Ein `dig +dnssec` bekam null Signaturen, weil ein `dig` ohne davor da war.**
Das Wegräumen lief im Transport, also *unter* dem Cache — und der hält eine
Antwort für alle Clients (ARCHITECTURE.md §4). Wer als Erster ohne DO fragte,
legte die gestutzte Fassung in den Cache. Der Cache hält jetzt die vollständige,
validierte Antwort; `dnssec::for_client` schneidet an der Außenkante zu, je
Client. Das ist dieselbe Trennung, aus der die Filterung vor dem Cache liegt:
was für alle gilt, gehört in den Cache, was für einen gilt, davor oder danach.

## Nachtrag vom 2026-08-30: ein Ausfall ist kein Befund

Beim Lauf gegen echte Upstreams in Phase 8 kam `wikipedia.org` einmal als
SERVFAIL zurück und beim nächsten Versuch als NOERROR. Die Ursache war eine
Verwechslung, die oben angelegt war.

Antwortet der Upstream **selbst** mit SERVFAIL und schickt dabei keine Records,
meldet `hickory` `Bogus` — es fehlen ja die NSEC-Records, mit denen sich etwas
beweisen ließe. Auf dem Draht sieht das genauso aus wie eine Zone mit kaputter
Signatur. Es ist aber etwas ganz anderes: *wir haben nichts gesehen, worüber
sich urteilen ließe.*

Der Unterschied ist teuer, weil `Bogus` nach Punkt 2 oben **terminal** ist. Ein
einzelner Wackler beim Upstream wurde damit zu einem harten SERVFAIL für den
Client, ohne dass der zweite, gesunde Upstream je gefragt worden wäre — ein
selbstgemachter Ausfall, und noch dazu einer, der sich beim nächsten Versuch von
selbst erledigt und deshalb schwer zu finden ist.

Unterschieden wird jetzt an dem, was die Antwort enthält: **leer und mit
Fehler-RCODE** heißt Ausfall (`ResolveError::Unproven`), alles andere heißt
Urteil. Beides zusammen gibt es nicht — eine Zone mit kaputter Signatur liefert
Records, sonst hätte niemand etwas zu prüfen. `Unproven` fragt den nächsten
Upstream, rechnet aber **niemandem einen Fehlversuch an**: sonst könnte eine
kaputte Zone weiterhin den Pool leerräumen, was Punkt 2 ja gerade verhindern
soll.

**Damit ist eine Zahl aus der Roadmap überholt.** Dort steht zu Phase 7:
`dnssec-failed.org → SERVFAIL, bogus=1`. Der Client bekommt weiterhin SERVFAIL,
aber der Zähler steht jetzt auf 0 — Quad9 validiert selbst und liefert eine leere
Fehlerantwort, wir sehen also nie eine faule Signatur. Die alte 1 war der
Mislabel, nicht die neue 0.

## Nachtrag vom 2026-09-19: das CD-Bit des Clients entscheidet nicht mehr

Ein Scan über die Codebase hat dieselbe Ursache dreimal gefunden (F1, F2, F3):
beide Transporte hingen die Validierung am CD-Bit der Anfrage. Die Begründung
stand als Kommentar an `dnssec::checking_disabled` — *wer die Prüfung
abbestellt, schadet nur sich* — und sie war falsch, an derselben Stelle, an der
Punkt 3 oben schon einmal falsch war.

**Der Cache ist der Grund.** Er ist nach (Name, Typ, Klasse) geschlüsselt
(`crate::caching`), CD steht nicht darin. Die Antwort, die ein CD-Client
auslöst, ist dieselbe, die der nächste Client ohne CD bekommt. Ein einzelner
Client im LAN hätte damit die Signaturprüfung für das ganze Netz abbestellt —
und nicht bloß theoretisch: bei gesetztem CD liefert der Upstream genau die
ungeprüfte Antwort, die DNSSEC abfangen soll (THREAT-MODEL.md, Punkt A3).

**Was jetzt gilt.** `Transport::send_checked` rechnet nur noch
`let validate = self.privacy.dnssec;`, `OdohBackend::resolve` nimmt den
validierenden Griff ohne Bedingung; `checking_disabled` hat keinen Aufrufer mehr
und ist weg. Es entscheidet die Konfiguration, nie die Anfrage. Nach außen war
CD ohnehin wirkungslos: `DnssecDnsHandle::send` in `hickory-net` setzt auf der
Strecke zum Upstream selbst `checking_disabled = false` und `authentic_data =
true`. Wirkung hatte CD nur nach innen, gegenüber dem eigenen Cache — und genau
dort gehörte es nicht hin.

Es ist dieselbe Trennung wie im dritten Punkt unter „Was der erste Lauf gegen
echte Upstreams gezeigt hat“, nur andersherum: damals wanderte das Zuschneiden
an die Außenkante, weil der Cache für alle gilt; hier verschwindet eine
Bedingung, die nie in den Cache hineingehört hätte. Was für alle gilt, muss für
alle entschieden werden.

**Die zweite Hälfte der Empfehlung ist bewusst nicht gebaut.** Vorgeschlagen war
auch, CD an der Außenkante zu ehren — AD-Bit löschen und dem CD-Client die
Rohdaten durchreichen. Das gibt es nicht: wer CD setzt und nach einem
Bogus-Namen fragt, bekommt SERVFAIL wie jeder andere. Ihm Daten zu geben, von
denen wir gerade nachgerechnet haben, dass sie falsch sind, wäre eine eigene
Entscheidung über `dnssec::for_client`. RFC 4035 §3.2.2 beschreibt CD als „nicht
prüfen“; dass wir es trotzdem tun, ist eine Abweichung — sie steht hier, damit
sie eine ist und kein Versehen.

**Was das kostet.** Ein Client, der CD setzt und selbst prüft, bekommt für eine
kaputte Zone SERVFAIL statt der Daten, mit denen er sein eigenes Urteil hätte
fällen können. Strenger als nötig, aber in der Richtung, in der ein Fehler
auffällt statt still zu bleiben. `encrypted.rs::a_client_setting_cd_does_not_disable_validation`
hält fest, dass mit CD das DO-Bit rausgeht und die Kette nachverfolgt wird. Der
ODoH-Zweig hat keinen eigenen Test; er ist gelesen, nicht gefahren — die
entfernte Bedingung ist eine reine Verschärfung, aber verlassen sollte man sich
darauf nicht.

**Umkehrbedingung dieses Nachtrags:** Wenn im Praxistest ein Client mit CD
aufläuft, der auf SERVFAIL stößt, ist die Antwort nicht, die Prüfung wieder
abzuschalten, sondern CD in `dnssec::for_client` zu behandeln — pro Client, an
der Außenkante, hinter dem Cache.

## Umkehrbedingung

Wenn im Praxistest aus Phase 9 kaputte Zonen zu Ausfällen führen, die niemand
erklären kann, ist die Antwort **nicht**, die Validierung abzuschalten, sondern
die verworfenen Antworten in der UI sichtbar zu machen — der Zähler
„DNSSEC verworfen“ steht dafür schon in der Privacy-Kachel. Erst wenn sich
zeigt, dass es regelmäßig legitime Zonen trifft, wäre ein Modus „prüfen, aber
durchlassen“ zu erwägen. Er ist bewusst nicht vorgebaut.
