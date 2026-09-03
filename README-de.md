# AlpenDNS

Ein privacy-fokussierter DNS-Server für Linux, in Rust. Ein **forwarding
Resolver**: er nimmt Anfragen aus dem LAN entgegen, filtert sie gegen
Blocklisten und Policies und leitet sie verschlüsselt (DoT/DoH/DoQ) an Upstreams
weiter. Eigene Rekursion ab den Root-Servern ist bewusst kein Ziel.

> **Status: Phase 9.** Phasen 1–9 sind umgesetzt — 1 Forwarder, 2 Cache,
> 3 verschlüsselte Upstreams, 4 Blocklisten, 5 Clients & Policies,
> 6 Sichtbarkeit, 7 Privacy, 8 Heuristik, 9 Betrieb. Abgenommen sind 1–7;
> Phase 8 und 9 laufen gerade im echten Netz. Plan und Abnahmekriterien:
> [docs/ROADMAP.md](docs/ROADMAP.md).

## Schnellstart

```bash
cargo run -- -c config/alpendns.minimal.toml
dig @127.0.0.1 -p 5353 example.com

# Warum wurde etwas geblockt? Ohne laufenden Server:
cargo run -- -c config/alpendns.minimal.toml policy test doubleclick.net

# Web-UI: http://127.0.0.1:8053 — der Token steht in api.token_file
```

### Als Dienst auf einem Server

```bash
cargo install cargo-deb --locked
cargo deb -p alpendns
sudo apt install ./target/debian/alpendns_0.0.1-1_amd64.deb

dig @127.0.0.1 example.com
```

Danach läuft der Resolver als unprivilegierter Dienst — vorerst nur auf
Loopback. Für das eigene Netz die LAN-Adresse in
`/etc/alpendns/alpendns.toml` eintragen und
`sudo alpendns -c /etc/alpendns/alpendns.toml check` laufen lassen. Alles
Weitere in [docs/OPERATIONS.md](docs/OPERATIONS.md).

## Warum noch ein DNS-Server?

Pi-hole und AdGuard Home lösen "Werbung blocken im LAN" gut. Was sie nicht lösen:

* **Dein Upstream sieht weiterhin alles.** Ein einzelner verschlüsselter Upstream ersetzt
  den neugierigen Provider durch einen neugierigen Anbieter. AlpenDNS verteilt Anfragen
  deterministisch über mehrere Upstreams (`split_by_zone`), sodass keiner das vollständige
  Profil sieht.
* **Blocken ist eine Blackbox.** "Warum geht diese Seite nicht?" ist in den meisten Setups
  eine Suche im Log. AlpenDNS gibt zu jeder Antwort eine Entscheidungskette aus: welche
  Liste, welche Regel, welche Policy.
* **Filterung ist rein listenbasiert.** Listen kennen nur, was gestern schon bekannt war.
  AlpenDNS bewertet zusätzlich lokal und ohne Cloud: DGA-Muster, DNS-Tunneling, Rebinding,
  Typosquatting auf Domains, die dir wichtig sind.
* **Logging ist an oder aus.** AlpenDNS hat einen aggregierenden Default mit
  k-Anonymitäts-Schwelle: du siehst Muster, aber die Kiste speichert nicht, wer wann was
  aufgerufen hat.

Das erklärte Ziel ist nicht, Pi-hole zu ersetzen, sondern die Fragen zu beantworten, die
Pi-hole offen lässt.

## Was v1 kann

| | |
|---|---|
| Rolle | Forwarding Resolver (keine eigene Rekursion) |
| Client-Transporte | UDP/53, TCP/53 |
| Upstream-Transporte | DoT, DoH, DoQ; Klartext nur für explizite interne Zonen |
| Filterung | Blocklisten (hosts, domains, wildcard), Allowlists, Regex-Regeln |
| Policies | pro Client (IP/Subnetz), Zeitfenster, temporäre Freigaben |
| Privacy | ECS-Stripping, Padding, DNS Cookies, 0x20, Upstream-Splitting, DNSSEC-Validierung, ODoH, aggregiertes Logging |
| Heuristik | DGA, Tunneling, Rebinding, Typosquatting, neu registrierte Domains — alle auf `flag`, blocken nichts |
| Betrieb | systemd-Unit, .deb-Paket, Prometheus-Metriken, Web-UI, Drosselung pro Client, SIGHUP-Reload |

Verschlüsselte Listener für Clients (DoT/DoH/DoQ) und Client-Identifikation über
DoH-Pfad-Token oder mTLS sind geplant, aber noch nicht gebaut — die Policy unterscheidet
Clients heute über die IP.

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
docs/OPERATIONS.md           Installation, Upgrade, Backup, Fehlersuche
packaging/                   systemd-Unit, Debian-Skripte, Auslieferungskonfiguration
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

[AGPL-3.0-or-later](LICENSE). Die Lizenz ist bewusst gewählt: Auch wer AlpenDNS nur als
Netzwerk-Dienst anbietet, muss den Quelltext seiner Änderungen offenlegen (AGPL §13) —
das passt zur Privacy-Motivation des Projekts.
