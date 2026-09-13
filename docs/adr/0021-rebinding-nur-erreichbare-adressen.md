# ADR-0021: Rebinding flaggt nur Adressen, auf denen ein Gerät sitzen kann

**Status:** angenommen · **Datum:** 2026-09-13 · **Betrifft:**
[ROADMAP.md](../ROADMAP.md) Phase 8 Schritt 2 und die Abnahme der Phase,
`crates/alpendns/src/detect/rebinding.rs`

## Kontext

Der Rebinding-Detektor meldet jede Antwort, in der ein öffentlicher Name auf
eine private Adresse zeigt. Die Liste der privaten Bereiche war breit gefasst:
neben RFC 1918, Loopback und Link-Local enthielt sie `0.0.0.0`/`::` und den
ganzen Bereich `192.0.0.0/16`.

Die Beobachtungswoche hat das geprüft — 132 208 Anfragen vom 30.08. bis
08.09.2026: **1 763 Meldungen, kein einziger Angriff.** Sie zerfallen in genau
drei Gruppen:

| Adresse | Meldungen | Was es war |
|---|---|---|
| `0.0.0.0` | 1 467 | Sinkholes der Upstreams für Telemetrie |
| `::` | 281 | dasselbe für IPv6 |
| `192.0.0.0/16` | 15 | 192.0.0.170/171 (NAT64, RFC 7050), `github.blog` → 192.0.66.2, `secure.gravatar.com` → 192.0.73.2 |

Die ersten beiden Gruppen sind die übliche Art, eine Telemetrie-Domain
stillzulegen: Apple (`a.gslb.aaplimg.com`), Amazon (`unagi-eu.amazon.com`),
Microsoft (`settings-win.data.microsoft.com`), dazu Twitch, Sentry, NordVPN.
Die dritte Gruppe ist gerouteter Adressraum — Quad9 und Cloudflare antworten
unabhängig voneinander mit `192.0.66.2` auf `github.blog`.

Drei Dinge daran sind der Anlass, nicht die Zahl allein:

1. Der **eigene** Block-Modus `zero_ip` liefert genau die Antwort, die der
   Detektor als Angriff meldet. Das System widerspricht sich selbst.
2. Der Kommentar an der Zeile sagte `192.0.0/24`, der Code prüfte das ganze
   `/16`. Der Fehlalarm auf `github.blog` war ein Tippfehler in der Breite.
3. Ein Detektor, der 1 763-mal im Monat Fehlalarm gibt, wird nicht mehr
   gelesen. Das ist der eigentliche Schaden — nicht die Zahl im Log.

## Entscheidung

Die Liste enthält nur noch Adressen, **auf denen ein Gerät im lokalen Netz
sitzen kann**. Entfernt werden `0.0.0.0`, `::` und `192.0.0.0/16`. Es bleiben
Loopback, RFC 1918, Link-Local, Broadcast, CGNAT (100.64/10) und Benchmark
(198.18/15) — die Bereiche, die ein Angriff tatsächlich als Ziel braucht.

## Konsequenzen

* Nach der Änderung hätte der Detektor in achteinhalb Tagen echten Verkehrs
  **nichts** gemeldet. Das Panel "Auffällig" wird wieder lesbar; ein Treffer
  heißt etwas.
* Was nicht mehr gemeldet wird: eine Antwort mit `0.0.0.0`. Auf Linux führt
  eine Verbindung nach `0.0.0.0` auf den eigenen Rechner — das war 2024 ein
  bekannter Bypass der Private-Network-Access-Regeln, die Browser haben
  nachgezogen. Sollte das hierzulande wieder ein Thema werden, gehört es in
  einen eigenen Detektor, nicht in eine Liste, die jedes Sinkhole trifft.
* Ebenfalls nicht mehr gemeldet: ein Netz, das `192.0.0.0/16` intern benutzt.
  Der Bereich ist kein RFC-1918-Raum, sondern für IANA-Protokolle reserviert.
* Kein `allow_zones`-Eintrag nötig: `ipv4only.arpa` (192.0.0.170/171) fällt
  durch die Änderung von selbst heraus.

## Alternativen

* **`ipv4only.arpa` in `allow_zones` eintragen.** Hätte die 11 NAT64-Meldungen
  erledigt und die 1 752 Sinkhole-Meldungen stehen gelassen.
* **Die Sinkhole-Zonen einzeln eintragen.** Nicht möglich: Amazons Gerätenamen
  (`78cec6f6…minerva.devices.a2z.com`) sind zufällig erzeugt.
* **`0.0.0.0` auf `log` statt `flag` setzen.** Ändert die Sichtbarkeit, nicht
  die Zahl — das Panel bleibt voll.
* **Ein zweiter Schalter "Sinkholes melden".** Konfiguration für einen Fall,
  der keine Entscheidung ist.
