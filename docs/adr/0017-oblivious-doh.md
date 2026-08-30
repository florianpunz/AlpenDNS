# ADR-0017: Oblivious DoH über `odoh-rs`, Schlüsselabruf direkt

**Status:** angenommen · **Datum:** 2026-08-30 · **Betrifft:** [FEATURES.md](../FEATURES.md) P4, [ADR-0002](0002-hickory-proto-statt-eigenem-parser.md)

## Kontext

Jede andere Privacy-Maßnahme des Projekts *verteilt* das Problem „der Upstream
kennt deine IP“. `split_by_zone` sorgt dafür, dass kein Anbieter mehr als einen
Bruchteil der Domains sieht — aber jeder sieht seinen Bruchteil samt Absender.
Über Wochen ergibt das trotzdem ein Profil.

ODoH (RFC 9230) löst es statt es zu verteilen: die Anfrage wird für den
Zielresolver verschlüsselt und über einen Proxy geschickt. Der Proxy sieht die
Adresse und einen undurchsichtigen Block; das Ziel sieht die Frage und als
Absender den Proxy.

## Entscheidung

### Die Krypto kommt aus `odoh-rs`

`odoh-rs` ist Cloudflares Referenzimplementierung, BSD-2-Clause, zwei Dateien
Quelltext, und liefert das ODoH-Wire-Format, das Parsen der veröffentlichten
Konfiguration und das HPKE darunter (RFC 9180, `hpke`).

Die Alternative wäre gewesen, nur `hpke` zu nehmen und das Framing selbst zu
schreiben. Dagegen spricht dasselbe wie in ADR-0002: die Info-Strings, die
Schlüsselableitung und die AAD-Konstruktion sind die Stelle, an der ein Fehler
den ganzen Schutz still aufhebt — die Nachricht wäre weiterhin verschlüsselt,
nur eben falsch, und niemand merkt es, weil die Antwort trotzdem ankommt.

`odoh-rs` baut keine eigenen Verbindungen auf; das HTTP darüber machen wir mit
`reqwest`, das für die Blocklisten ohnehin schon im Baum liegt (B.1 Regel 4
bleibt damit unberührt: kontaktiert werden nur konfigurierte Ziele).

Als vierte Abhängigkeit kommt `hpke` selbst dazu — allein, um
`hpke::rand_core::RngCore` benennen zu können, das `odoh-rs` in seiner Signatur
führt. Zehn Zeilen Adapter über unser `rand` sind billiger als ein zweites
Zufallssystem im Baum.

### Der Schlüsselabruf geht direkt zum Ziel, nicht über den Proxy

Das ist die unangenehme Stelle, und sie gehört benannt statt weggelassen.

Einmal je Prozessstart holt AlpenDNS den öffentlichen Schlüssel des Ziels von
dessen `/.well-known/odohconfigs`. Diese eine Verbindung geht **nicht** über den
Proxy — das Ziel sieht dabei also die Adresse. Es sieht dabei keine einzige
Frage, und es erfährt nur, dass hier jemand ist, der später ODoH benutzen wird;
nicht wann und nicht wofür. Alle folgenden Anfragen laufen über den Proxy.

Über den Proxy zu gehen wäre der saubere Weg. Er steht nicht offen: ein
ODoH-Proxy nimmt ausschließlich `application/oblivious-dns-message` entgegen und
ist kein allgemeiner HTTP-Proxy. RFC 9230 §6.1 lässt offen, woher die
Konfiguration kommt, und nennt Wege außerhalb des Protokolls; deshalb nimmt
`OdohTransport::new` die Adresse des Schlüssels als Parameter entgegen, statt sie
zu bauen. Wer sie anderswoher hat, muss sie nicht über einen Umweg
unterschieben. Den Normalfall liefert `well_known_config_url`.

### Der Startfehler statt der halben Wirkung

Mit eingeschaltetem ODoH muss **jeder** Resolver im Pool `doh://` sprechen —
ODoH gibt es nur über HTTP. Ein `dot://` daneben ist ein Startfehler mit
Begründung, nicht ein stilles Durchreichen im Klartextpfad. Eine Einstellung,
die für die Hälfte der Anfragen nichts tut, ist schlimmer als eine, die es nicht
gibt (B.1 Regel 5). Ebenso ist ein `http://`-Proxy ein Startfehler: über
Klartext sähe ein Mitleser Zieladresse und Zeitpunkt jeder Anfrage.

### DNSSEC gilt auch über ODoH

Der ODoH-Transport wird als `DnsHandle` verpackt, damit `DnssecDnsHandle`
(ADR-0016) sich davorhängen kann. Ohne diesen Umweg täte `privacy.dnssec = true`
mit eingeschaltetem ODoH still nichts — wieder eine Einstellung ohne Wirkung.
Der Preis: die Kettenabfragen laufen ebenfalls über den Proxy, jede ein eigener
HTTP-Umlauf. Der Validierungs-Cache begrenzt das auf einmal je Zone.

## Grenzen, die keine Umsetzung beheben kann

**Proxy und Ziel dürfen nicht demselben Betreiber gehören.** Sonst kennt einer
beides und ODoH ist ein aufwendiger DoH-Umweg. Das kann kein Code prüfen; es
steht in der Beispielkonfiguration über dem Schlüssel.

**Der Proxy weiß, mit wem du sprichst.** `targethost` steht als Parameter in der
URL — anders geht es nicht, er muss ja weiterreichen. Er weiß also: diese
Adresse fragt bei diesem Anbieter. Nicht: was.

**Zusätzliche Latenz.** Ein Umlauf mehr, plus die Kryptografie. In einem
Haushaltsnetz ist das der Cache-Miss-Fall und damit selten.

**Die Auswahl ist überschaubar.** Es gibt wenige öffentliche ODoH-Proxys und
-Ziele. Deshalb ist die Einstellung Default-aus: eingeschaltet, ohne die beiden
Adressen bewusst gewählt zu haben, wäre sie eine Beruhigung ohne Deckung.

## Was geprüft ist

`crates/alpendns/tests/odoh.rs` baut die vollständige Kette auf Loopback: ein
Proxy, der weiterreicht ohne entschlüsseln zu können, und ein Ziel mit echtem
Schlüsselpaar. Der wichtigste Test durchsucht die Bytes, die durch den Proxy
gingen, nach den Labels des Query-Namens — sie stehen nicht drin. Dazu: der
Schlüssel wird genau einmal geholt und danach gehalten; eine vom Proxy
veränderte Antwort wird verworfen statt ausgeliefert; ein toter Proxy ergibt
einen Fehler statt eines Hängers.
