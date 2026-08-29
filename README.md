# AlpenDNS

Ein privacy-fokussierter DNS-Server für Linux, in Rust.

> **Status: Phase 3 — verschlüsselte Upstreams.** UDP und TCP für Clients,
> Weiterleitung über **DoT, DoH oder DoQ** an einen Pool mehrerer Resolver mit
> Auswahlstrategie und Ausfallerkennung. Dazu ein Cache mit TTL-Klemmung,
> LRU-Verdrängung, serve-stale, Prefetch und Query-Deduplizierung, sowie
> ECS-Stripping, EDNS-Padding, DNS Cookies und 0x20. Klartext geht nur noch in
> ausdrücklich konfigurierte Zonen des eigenen Netzes.
> Noch kein Filter und keine Policy — das ist Phase 4 und 5.
> Siehe [docs/ROADMAP.md](docs/ROADMAP.md).

```bash
cargo run -- -c config/alpendns.minimal.toml
dig @127.0.0.1 -p 5353 example.com
```

## Warum noch ein DNS-Server?

Pi-hole und AdGuard Home lösen "Werbung blocken im LAN" gut. Was sie nicht lösen:

* **Dein Upstream sieht weiterhin alles.** Ein einzelner verschlüsselter Upstream ersetzt
  den neugierigen Provider durch einen neugierigen Anbieter. AlpenDNS verteilt Anfragen
  deterministisch über mehrere Upstreams, sodass keiner das vollständige Profil sieht.
* **Blocken ist eine Blackbox.** "Warum geht diese Seite nicht?" ist in den meisten Setups
  eine Suche im Log. AlpenDNS gibt zu jeder Antwort eine Entscheidungskette aus: welche
  Liste, welche Regel, welche Policy.
* **Filterung ist rein listenbasiert.** Listen kennen nur, was gestern schon bekannt war.
  AlpenDNS bewertet zusätzlich lokal und ohne Cloud: DGA-Muster, DNS-Tunneling,
  Rebinding, Typosquatting auf Domains, die dir wichtig sind.
* **Clients werden per IP unterschieden.** Das bricht, sobald das Gerät das Netz wechselt.
  AlpenDNS identifiziert Clients zusätzlich über DoH-Pfad-Token und mTLS — die Policy
  folgt dem Gerät auch unterwegs.
* **Logging ist an oder aus.** AlpenDNS hat einen aggregierenden Default mit
  k-Anonymitäts-Schwelle: du siehst Muster, aber die Kiste speichert nicht, wer wann was
  aufgerufen hat.

Das erklärte Ziel ist nicht, Pi-hole zu ersetzen, sondern die Fragen zu beantworten, die
Pi-hole offen lässt.

## Was v1 kann (geplant)

| | |
|---|---|
| Rolle | Forwarding Resolver (keine eigene Rekursion, siehe unten) |
| Client-Transporte | UDP/53, TCP/53, DoT, DoH, DoQ |
| Upstream-Transporte | DoT, DoH, DoQ; Klartext nur für explizite interne Zonen |
| Filterung | Blocklisten (hosts, domains, wildcard, Adblock-Syntax, RPZ), Allowlists, Regex-Regeln |
| Policies | pro Client/Gruppe, Zeitfenster, temporäre Freigaben |
| Privacy | ECS-Stripping, Padding, DNS Cookies, 0x20, Upstream-Splitting, aggregiertes Logging |
| Heuristik | DGA, Tunneling, Rebinding, Typosquatting, neu registrierte Domains |
| Betrieb | systemd-Unit, .deb-Paket, Prometheus-Metriken, Web-UI |

## Was v1 ausdrücklich nicht ist

* **Kein rekursiver Resolver.** AlpenDNS fragt Upstreams, nicht die Root-Server. Die
  Architektur hält die Stelle frei, an der eine Rekursion später eingehängt würde
  ([ADR-0003](docs/adr/0003-forwarder-first.md)), aber es wird nichts dafür vorgebaut.
* **Kein autoritativer Nameserver.** Wer eine Zone hosten will, nimmt Knot oder NSD.
* **Kein DHCP-Server.** Pi-hole macht das mit; das ist eine andere Aufgabe.
* **Kein Ersatz für ein VPN.** DNS-Privacy schützt die Namensauflösung. Die IP-Verbindung
  danach sieht dein Provider trotzdem. Siehe [docs/THREAT-MODEL.md](docs/THREAT-MODEL.md).

## Repository

```
CLAUDE.md                    Regeln für Coding-Agenten in diesem Repo
config/alpendns.example.toml Ziel-Konfiguration (dient als Spezifikation)
crates/                      Rust-Workspace
docs/ROADMAP.md              Phasenplan mit Abnahmekriterien
docs/ARCHITECTURE.md         Aufbau, Request-Pipeline, Datenmodell
docs/FEATURES.md             Feature-Katalog mit Aufwand/Nutzen-Bewertung
docs/THREAT-MODEL.md         Wogegen das hier schützt — und wogegen nicht
docs/TESTING.md              Teststrategie
docs/adr/                    Architekturentscheidungen mit Begründung
```

## Entwicklung

Voraussetzung: Rust stable (die `rust-toolchain.toml` zieht die passende Version).

```bash
cargo build

# Definition of Done
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo deny check
```

Ein Change gilt als fertig, wenn alle vier Prüfkommandos durchlaufen. Einzelne Tests,
Fuzzing und der `dig`-Smoke-Test stehen in [CLAUDE.md](CLAUDE.md) B.4.

## Lizenz

Noch nicht entschieden. Zwei sinnvolle Optionen:

* **AGPL-3.0** — jeder, der AlpenDNS als Dienst anbietet, muss Änderungen offenlegen.
  Passt zur Privacy-Motivation, schreckt kommerzielle Nutzung ab.
* **MIT/Apache-2.0** — maximale Verbreitung, auch in Produkten anderer.

Der Workspace steht vorläufig auf `AGPL-3.0-or-later`. Das ist eine Entscheidung, die vor
der ersten Veröffentlichung bewusst getroffen und in einem ADR festgehalten werden sollte.
