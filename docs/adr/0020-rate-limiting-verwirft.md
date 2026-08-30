# ADR-0020: Über dem Limit wird verworfen, nicht abgelehnt

**Status:** angenommen · **Datum:** 2026-08-30 · **Betrifft:** [ROADMAP.md](../ROADMAP.md) Phase 9 Schritt 5, CLAUDE.md B.5

## Kontext

Ein offener Resolver ist ein Amplification-Reflektor. Der Angriff ist alt und
billig: kurze Anfrage mit gefälschter Absenderadresse hinein, lange Antwort an
das Opfer hinaus. CLAUDE.md B.5 macht die Drosselung pro Client deshalb zur
Pflicht, bevor der Server irgendwo lauscht, wo er nicht nur sein eigenes LAN
sieht.

Damit stellen sich drei Fragen, die aussehen wie Details und keine sind.

## Entscheidung 1: verwerfen statt ablehnen

Über dem Limit wird das Paket kommentarlos fallengelassen. Kein REFUSED, kein
SERVFAIL, kein gekürztes „frag über TCP nach".

Der Grund ist der Angriff selbst: Die Absenderadresse ist bei UDP frei wählbar,
und der ganze Sinn der Drosselung ist, dass an eine Adresse, die vielleicht nie
gefragt hat, nichts geschickt wird. Eine REFUSED-Antwort ist kleiner als eine
echte Antwort, aber sie ist immer noch ein Paket an das Opfer — eine Drosselung,
die den Reflektor weiterbetreibt, nur leiser.

Der Preis ist echt und wird hier bewusst bezahlt: ein *legitimer* Client über
dem Limit sieht keine Ablehnung, sondern einen Timeout, und Timeouts sind für
den, der davorsitzt, schwerer zu deuten als eine Fehlermeldung. Dagegen steht,
dass die Grenzen so hoch liegen, dass ein einzelnes Gerät sie im Normalbetrieb
nicht erreicht (100 Anfragen/s dauerhaft, Spitze 200), und dass der Zähler
`alpendns_rate_limited_total` genau die Frage beantwortet, die dann gestellt
wird.

Das ist auch, was BIND und Knot mit Response Rate Limiting tun. Beide bieten
zusätzlich eine „slip"-Rate an: jede n-te überzählige Anfrage wird mit gesetztem
TC-Flag beantwortet, damit ein echter Client auf TCP ausweichen kann. Das ist
richtig für einen autoritativen Server im Internet, der wildfremde Clients
bedient. Für einen Resolver im eigenen LAN wäre es ein zweiter Schalter mit
einer dritten Bedeutung für einen Fall, der hier nicht vorkommt — und jeder
geslippte Antwort geht wieder an eine möglicherweise gefälschte Adresse.
Umkehrbedingung: sobald AlpenDNS Clients bedient, die nicht im eigenen Netz
stehen (DoH-Listener aus Phase 10), gehört slip auf die Tagesordnung.

## Entscheidung 2: IPv6 wird auf /64 zusammengefasst

Gezählt wird je IPv4-Adresse und je IPv6-**/64**, nicht je IPv6-Adresse.

Der Anlass ist nicht der Angreifer, sondern der normale Betrieb: mit Privacy
Extensions (RFC 8981) wechselt ein gewöhnlicher Laptop seine IPv6-Adresse im
Stundentakt und benutzt mehrere gleichzeitig. Pro Adresse gezählt bekäme
derselbe Rechner ständig ein frisches Guthaben — die Drosselung wäre für IPv6
Zierde. Dass ein Angreifer aus einem /64 dasselbe kann, kommt hinzu.

`::ffff:10.0.0.1` und `10.0.0.1` sind derselbe Host und teilen sich einen
Eimer. Ohne diese Zeile hätte ein Client, dessen Betriebssystem den Socket auf
v6 öffnet, zwei Guthaben.

Ein /64 ist die Zuteilung an ein einzelnes Netzsegment; wer feiner zählen will,
verliert mehr (den echten Host) als er gewinnt.

## Entscheidung 3: Token Bucket in Schubladen, keine gleitenden Fenster

Ein Eimer je Client mit Nachlaufrate und Obergrenze — die einfachste Struktur,
die „dauerhaft x, kurzfristig y" ausdrücken kann. Ein gleitendes Zeitfenster
wäre genauer und bräuchte je Client eine Liste von Zeitstempeln.

Die Eimer liegen in 16 Schubladen, jede hinter ihrem eigenen Mutex. CLAUDE.md
B.3 Regel 5 verbietet einen globalen Mutex im Anfragepfad, und das zu Recht:
bei 85 000 Anfragen/s wäre ein einzelnes Schloss die Serialisierung des ganzen
Servers. Etwas Lockfreies je Client wäre die Sorte Nebenläufigkeitscode, die man
falsch macht und bei der der Fehler erst unter Last auftritt.

Die Zahl der beobachteten Clients ist gedeckelt (LRU, Default 8192). Ohne diese
Grenze wäre die Drosselung bei einer Flut gefälschter Absenderadressen selbst
der Speicherfresser, den sie verhindern soll.

## Konsequenzen

* Ein gedrosselter Client sieht einen Timeout. Der Betrieb erkennt das an
  `alpendns_rate_limited_total`; die Anleitung dazu steht in
  [OPERATIONS.md](../OPERATIONS.md) §4.
* Die Adresse des gedrosselten Clients steht nur auf Log-Level `debug` und
  damit per Default nirgends. In der Metrik steht sie gar nicht: ein Label mit
  Client-IP wäre eine Anwesenheitsliste mit Zeitstempel, und Prometheus behält
  jede Zeitreihe für immer.
* Gemessener Preis auf dem Anfragepfad: **2 %** Durchsatz
  ([BENCHMARKS.md](../BENCHMARKS.md), Phase 9).
* Die Drosselung sitzt **vor** dem Parsen der Nachricht. Ein verworfenes Paket
  kostet damit einen Hash und einen Vergleich, nicht den Weg durch
  hickory-proto.

## Alternativen, die verworfen wurden

**Nur auf UDP drosseln.** TCP-Absender sind durch den Handshake bestätigt und
taugen nicht zur Reflexion. Trotzdem gilt das Limit auch dort: die zweite Gefahr
eines offenen Resolvers ist schlichte Erschöpfung — offene Verbindungen,
Upstream-Anfragen, Cache-Verdrängung —, und die kennt kein Transportprotokoll.

**Nach Antwortgröße gewichten** (teure Antworten kosten mehr Guthaben). Das ist
die genauere Bremse gegen Amplification, weil sie am Verstärkungsfaktor ansetzt.
Sie setzt aber voraus, dass die Antwort schon da ist — dann ist die Arbeit
getan, und der Schutz gegen Erschöpfung entfällt. Für einen Resolver ist die
Anfrage der richtige Zeitpunkt.

**Auf nftables verweisen.** Ein Paketfilter kann das auch, und in einem großen
Netz gehört es dorthin. Für ein Programm, dessen Versprechen „in fünf Minuten
installiert und gehärtet" lautet, ist eine Firewallregel, die jemand von Hand
schreiben muss, kein Schutz — sondern eine Fußnote.
