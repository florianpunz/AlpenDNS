// AlpenDNS — die Demo-Schicht.
//
// Diese Datei gehört nicht zum Produkt. `web/demo/build.sh` setzt sie beim
// Bauen der öffentlichen Demo zwischen `index.html` und `app.js` und ersetzt
// dort genau zwei Dinge: `fetch` und `EventSource`. Markup, Stylesheet und die
// Oberfläche selbst bleiben unverändert das, was der Server ausliefert — die
// Demo kann deshalb nicht anders aussehen als das Original, weil sie das
// Original *ist*, nur mit einer anderen Datenquelle.
//
// ── Die erfundene Anlage hat keinen Zustand ────────────────────────────────
//
// Jede Zahl ist eine reine Funktion der Uhrzeit, ermittelt über einen
// zustandslosen Hash statt über einen Zufallsgenerator. Das hat drei Folgen,
// und alle drei sind gewollt:
//
//   * Zwei Besucher zur selben Sekunde sehen dieselbe Seite.
//   * Ein Neuladen bringt dieselben Zeilen zurück — es waren Ereignisse dieser
//     erfundenen Welt, keine frisch gewürfelten.
//   * Das Ende von /api/recent schließt fugenlos an das erste Ereignis des
//     Live-Stroms an. Genau das kann ein Besucher nachsehen.
//
// Ein Zufallsgenerator mit Startwert könnte das nicht: sein Ergebnis hinge
// davon ab, wie viele Ziehungen vorher passiert sind, also davon, wann der
// Besucher gekommen ist.
//
// ── Was erfunden ist und was nicht ─────────────────────────────────────────
//
// Erfunden sind die Zahlen und der Haushalt, der sie erzeugt. *Nicht* erfunden
// sind die Wortlaute: die Entscheidungsketten stehen genauso in
// `src/trace.rs`, die Begründungen der Heuristiken genauso in `src/detect/`,
// die Liste mit den Resolvern, den Blocklisten, den Policies und dem Zeitplan
// steht genauso in `config/alpendns.example.toml`. Die Demo erfindet keine
// Vokabeln, sie erfindet einen Haushalt.
"use strict";

(function () {
  // ── Zufall, der keiner ist ───────────────────────────────────────────────

  /** Ein 32-Bit-Hash. Derselbe Einsatz, dasselbe Ergebnis — immer. */
  function hash32(x) {
    x = Math.imul(x ^ (x >>> 16), 0x45d9f3b);
    x = Math.imul(x ^ (x >>> 16), 0x45d9f3b);
    return (x ^ (x >>> 16)) >>> 0;
  }

  /** Eine Zahl in [0,1) — bestimmt durch (Sekunde, Nummer, Zweck). */
  function rnd(sec, k, salt) {
    return (
      hash32((sec | 0) + Math.imul(k + 1, 0x9e3779b1) + Math.imul(salt, 0x85ebca6b)) /
      4294967296
    );
  }

  /** FNV-1a über einen Namen. Namen sind stabil, also auch ihr Weg. */
  function stringHash(text) {
    let hash = 2166136261;
    for (let index = 0; index < text.length; index += 1) {
      hash ^= text.charCodeAt(index);
      hash = Math.imul(hash, 16777619);
    }
    return hash;
  }

  const gauss = (x, mu, sigma) => Math.exp(-0.5 * ((x - mu) / sigma) ** 2);

  // ── Die Uhr ──────────────────────────────────────────────────────────────

  const BUCKET_SECONDS = 300;
  const BUCKETS = 288; // 24 Stunden
  const AGGREGATE_K = 5; // wie [privacy.logging] aggregate_k
  const VERSION = "0.0.1"; // wie env!("CARGO_PKG_VERSION")
  const UPTIME_SECONDS = 6 * 86400 + 7 * 3600 + 42 * 60;

  const nowSec = () => Math.floor(Date.now() / 1000);

  function hourOf(sec) {
    const at = new Date(sec * 1000);
    return at.getHours() + at.getMinutes() / 60 + at.getSeconds() / 3600;
  }

  function isWeekend(sec) {
    const day = new Date(sec * 1000).getDay();
    return day === 0 || day === 6;
  }

  /** Wann der erfundene Prozess gestartet ist. Absolut verankert, nicht an
   *  Mitternacht: sonst sprängen Laufzeit und Zähler für jeden um, der die
   *  Seite über Nacht offen lässt. */
  const START = nowSec() - UPTIME_SECONDS;

  // ── Der Tagesgang ────────────────────────────────────────────────────────

  /** Anfragen je Sekunde. Der Sockel ist bewusst hoch: eine Demo, die nachts
   *  stillsteht, sieht aus wie eine kaputte. */
  function qps(sec) {
    const hour = hourOf(sec);
    const weekend = isWeekend(sec);
    let value = 0.9;
    value += (weekend ? 1.6 : 2.2) * gauss(hour, weekend ? 9.5 : 7.3, weekend ? 1.6 : 0.9);
    value += (weekend ? 1.9 : 1.6) * gauss(hour, 13.0, 3.4);
    value -= 0.7 * gauss(hour, 17.2, 0.9); // die Pendlerdelle
    value += 3.4 * gauss(hour, 20.4, weekend ? 1.9 : 1.6);
    return Math.max(0.25, value);
  }

  // ── Der Haushalt ─────────────────────────────────────────────────────────

  /** Ein Gerät. `profile` ist seine Tagesform — ein Tablet ist nachmittags und
   *  abends da, ein Arbeitsrechner vormittags, der Drucker immer. */
  const CLIENTS = [
    { name: "kids-tablet", ip: "10.0.10.42", policy: "kids", weight: 0.62, profile: "kids" },
    { name: "kids-phone", ip: "10.0.10.51", policy: "kids", weight: 0.38, profile: "kids" },
    { name: "living-room", ip: "10.0.10.60", policy: "default", weight: 1.0, profile: "evening" },
    { name: "work-laptop", ip: "10.0.10.20", policy: "default", weight: 1.0, profile: "work" },
    // Kein Eintrag in der Konfiguration: Drucker, Thermostat, Gäste. Die
    // Policy heißt dann "default", und genau so steht es im Protokoll.
    { name: "default", ip: "–", policy: "default", weight: 0.55, profile: "flat" },
  ];

  const PROFILES = {
    kids: (h) => 0.02 + 1.0 * gauss(h, 19.5, 3.0) + 0.55 * gauss(h, 8.0, 1.1),
    evening: (h) => 0.45 + 1.6 * gauss(h, 20.0, 3.0),
    work: (h) => 0.25 + 1.3 * gauss(h, 11.5, 3.2) + 0.5 * gauss(h, 19.0, 2.0),
    flat: () => 0.55,
  };

  /** Der Zeitplan "bedtime" aus der Beispielkonfiguration: montags bis
   *  donnerstags und sonntags von 21:00 bis 07:00 wird für die Policy "kids"
   *  alles außer der Allowlist geblockt. */
  function inBedtime(sec) {
    const at = new Date(sec * 1000);
    const day = at.getDay();
    if (day === 5 || day === 6) return false; // Freitag und Samstag sind frei
    const hour = hourOf(sec);
    return hour >= 21 || hour < 7;
  }

  // ── Namen ────────────────────────────────────────────────────────────────

  /** Die Töpfe. `share` ist der Anteil an allen Anfragen, `boost` die
   *  Tagesform, `blocked` die Zugehörigkeit zu einer der Listen. Nur Werbung
   *  und Telemetrie stehen darauf — deshalb ist die Grundblockrate dieses
   *  Haushalts genau die Summe ihrer beiden Anteile, und deshalb steht sie
   *  nirgends als Zahl, sondern wird überall ausgerechnet. */
  const POOLS = [
    { key: "ads", share: 0.06, blocked: true, boost: (h) => 0.7 + 0.5 * gauss(h, 20.0, 3.5) },
    { key: "telemetry", share: 0.035, blocked: true, boost: () => 1 },
    { key: "streaming", share: 0.16, boost: (h) => 1 + 2.0 * gauss(h, 20.4, 2.0) },
    { key: "cdn", share: 0.2, boost: () => 1 },
    { key: "platform", share: 0.14, boost: () => 1 },
    { key: "work", share: 0.16, boost: (h) => 0.6 + 0.9 * gauss(h, 11.5, 3.2) },
    { key: "lan", share: 0.045, boost: () => 1 },
    { key: "tail", share: 0.2, boost: () => 1 },
  ];

  /** Die Namen, die oft genug vorkommen, um gezählt zu werden.
   *
   *  `verdict` ist das, was die Policy mit ihnen macht: `blocked` steht auf
   *  einer der beiden Listen, `allowlisted` steht ebenfalls darauf, aber
   *  `local-allow` hat Vorrang, `flagged` fällt einer Heuristik auf — was
   *  nichts blockt, weil alle fünf auf `flag` stehen. `list` ist die Liste,
   *  die im Trefferfall im Trace steht. */
  const NAMES = [
    // Der Fernseher und die Musikanlage, abends.
    { pool: "streaming", name: "youtube.com", w: 9 },
    { pool: "streaming", name: "googlevideo.com", w: 7 },
    { pool: "streaming", name: "nflxvideo.net", w: 5 },
    { pool: "streaming", name: "audio-ak-spotify-com.akamaized.net", w: 4 },
    { pool: "streaming", name: "netflix.com", w: 3 },
    { pool: "streaming", name: "ttvnw.net", w: 2, list: "twitch" },

    // Was immer im Haus passiert, ohne dass jemand etwas anklickt.
    { pool: "cdn", name: "fonts.gstatic.com", w: 6 },
    { pool: "cdn", name: "cdn.jsdelivr.net", w: 4 },
    // Steht auf `oisd-big` und wird trotzdem durchgelassen: eine App braucht
    // sie, und `local-allow` hat Vorrang. Genau dafür gibt es die Allowlist.
    // Sie gehört in diesen Topf und nicht zu Werbung — nicht weil sie keine
    // wäre, sondern weil ein CDN eben ständig gefragt wird. Und weil sie ständig
    // gefragt wird, sieht ein Besucher sie auch: eine Freigabe, die im
    // Protokoll nie auftaucht, erklärt nichts.
    { pool: "cdn", name: "cdn.branch.io", w: 3, verdict: "allowlisted", list: "oisd-big" },
    { pool: "cdn", name: "d1a2b3c4.cloudfront.net", w: 3 },
    { pool: "cdn", name: "static.crates.io", w: 2 },
    { pool: "cdn", name: "objects.githubusercontent.com", w: 2 },

    { pool: "platform", name: "gs-loc.apple.com", w: 5 },
    { pool: "platform", name: "connectivitycheck.gstatic.com", w: 4 },
    { pool: "platform", name: "mesu.apple.com", w: 3 },
    { pool: "platform", name: "time.cloudflare.com", w: 3 },

    { pool: "work", name: "github.com", w: 4 },
    { pool: "work", name: "slack.com", w: 3 },
    { pool: "work", name: "api.github.com", w: 3 },
    { pool: "work", name: "registry.npmjs.org", w: 3 },
    { pool: "work", name: "crates.io", w: 2 },
    { pool: "work", name: "docs.rs", w: 2 },

    // Werbung und Messdienste. Das ist der Teil, der geblockt wird.
    { pool: "ads", name: "graph.facebook.com", w: 6, verdict: "blocked", list: "oisd-big" },
    { pool: "ads", name: "ads.doubleclick.net", w: 5, verdict: "blocked", list: "oisd-big" },
    { pool: "ads", name: "analytics.tiktok.com", w: 4, verdict: "blocked", list: "oisd-big" },
    { pool: "ads", name: "px.ads.linkedin.com", w: 3, verdict: "blocked", list: "oisd-big" },
    { pool: "ads", name: "app-measurement.com", w: 3, verdict: "blocked", list: "oisd-big" },
    { pool: "ads", name: "googlesyndication.com", w: 3, verdict: "blocked", list: "oisd-big" },
    { pool: "ads", name: "criteo.com", w: 2, verdict: "blocked", list: "oisd-big" },
    { pool: "ads", name: "scorecardresearch.com", w: 2, verdict: "blocked", list: "oisd-big" },
    { pool: "ads", name: "adsrvr.org", w: 2, verdict: "blocked", list: "oisd-big" },

    { pool: "telemetry", name: "vortex.data.microsoft.com", w: 3, verdict: "blocked", list: "stevenblack-unified" },
    { pool: "telemetry", name: "settings-win.data.microsoft.com", w: 2, verdict: "blocked", list: "stevenblack-unified" },
    { pool: "telemetry", name: "telemetry.mozilla.org", w: 2, verdict: "blocked", list: "stevenblack-unified" },
    { pool: "telemetry", name: "device-metrics-us.amazon.com", w: 2, verdict: "blocked", list: "stevenblack-unified" },
    { pool: "telemetry", name: "watson.telemetry.microsoft.com", w: 1, verdict: "blocked", list: "stevenblack-unified" },

    // Ging nie ins Internet. Der einzige Ort, an dem Klartext-DNS erlaubt ist.
    { pool: "lan", name: "nas.home.arpa", w: 3, forward: true },
    { pool: "lan", name: "ha.home.arpa", w: 2, forward: true },
    { pool: "lan", name: "drucker.home.arpa", w: 1, forward: true },
  ];

  /** Die drei Anfragen, die auffallen, ohne geblockt zu werden. Ein Gerät, das
   *  alle paar Minuten einen neuen Namen erfindet, eins, das Daten über DNS
   *  schaufelt, und eins, das auf eine private Adresse zeigt. */
  const FLAGGED = [
    {
      name: "kqxvbnzmrtwp3.com",
      detector: "dga",
      label: "Algorithmically generated name",
      score: "0.873",
      reason:
        "'kqxvbnzmrtwp3' does not fit naturally grown names: 4.1 bits of surprise per " +
        "character triple, longest consonant run 12, digit share 8 %",
    },
    {
      name: "y7hq2vxn8km4p.net",
      detector: "dga",
      label: "Algorithmically generated name",
      score: "0.941",
      reason:
        "'y7hq2vxn8km4p' does not fit naturally grown names: 4.6 bits of surprise per " +
        "character triple, longest consonant run 7, digit share 23 %",
    },
    {
      name: "9f2a1c8e4b7d3a6f1e5c.sync.telemetry-update.net",
      detector: "tunneling",
      label: "Tunneling",
      score: "0.910",
      reason:
        "Zone 'sync.telemetry-update.net': 412 unique subdomains in 1238 queries, " +
        "label entropy 3.9 bits/character, mean label length 28, 86 of them TXT/NULL",
    },
    {
      name: "b4e1d9a70c3f82.sync.telemetry-update.net",
      detector: "tunneling",
      label: "Tunneling",
      score: "0.884",
      reason:
        "Zone 'sync.telemetry-update.net': 388 unique subdomains in 1104 queries, " +
        "label entropy 3.8 bits/character, mean label length 26, 71 of them TXT/NULL",
    },
    {
      name: "cam.homeip.net",
      detector: "rebinding",
      label: "DNS rebinding",
      score: "1.000",
      reason: "Answer to public name 'cam.homeip.net' contains the private address 10.0.10.7",
    },
    {
      name: "nas.freeddns.org",
      detector: "rebinding",
      label: "DNS rebinding",
      score: "1.000",
      reason: "Answer to public name 'nas.freeddns.org' contains the private address 192.168.1.10",
    },
  ];

  const UPSTREAMS = [
    { name: "quad9", transport: "dot", rtt: 13 },
    { name: "mullvad", transport: "doh", rtt: 24 },
    { name: "dnsforge", transport: "doh", rtt: 33 },
  ];

  /** Die Registrable Domain — grob, aber ausreichend: die letzten zwei
   *  Marken. `split_by_zone` hasht sie, also geht alles unter derselben Domain
   *  immer denselben Weg. */
  function registrable(name) {
    const parts = name.split(".");
    return parts.length <= 2 ? name : parts.slice(-2).join(".");
  }

  function upstreamFor(name) {
    return hash32(stringHash(registrable(name))) % UPSTREAMS.length;
  }

  // ── Ziehen nach Gewicht ──────────────────────────────────────────────────

  function pick(list, value) {
    let total = 0;
    for (const entry of list) total += entry.weight;
    let rest = value * total;
    for (const entry of list) {
      rest -= entry.weight;
      if (rest <= 0) return entry.item;
    }
    return list.length ? list[list.length - 1].item : null;
  }

  function clientWeights(hour) {
    return CLIENTS.map((client) => ({
      weight: client.weight * PROFILES[client.profile](hour),
      item: client,
    }));
  }

  function poolWeights(hour) {
    return POOLS.map((pool) => ({
      weight: pool.share * pool.boost(hour),
      item: pool,
    }));
  }

  /** Eine feste Abweichung je Name, zwischen 0,9 und 1,1.
   *
   *  Ohne sie bekämen zwei Namen mit demselben Gewicht genau dieselbe Zahl —
   *  und zwei gleiche Zahlen nebeneinander in der Liste der häufigsten Namen
   *  verraten sofort, dass hier gerechnet und nicht gezählt wurde. Echter
   *  Verkehr bringt solche Gleichstände nicht hervor.
   *
   *  Die Abweichung hängt am Namen und nicht an der Uhr: derselbe Name hat
   *  morgen dasselbe Gewicht, sonst sprängen die Zahlen beim Neuladen. */
  const nameNoise = (name) => 0.9 + 0.2 * (hash32(stringHash(name)) / 4294967296);

  function nameWeights(pool) {
    return NAMES.filter((entry) => entry.pool === pool).map((entry) => ({
      weight: entry.w * nameNoise(entry.name),
      item: entry,
    }));
  }

  /** Der lange Schwanz: Namen, die einmal oder zweimal vorkommen und deshalb
   *  unter der k-Schwelle bleiben. Er darf keinen Namen erzeugen, der auch
   *  oben steht, sonst ginge die Rechnung im Top-Panel nicht auf. */
  function tailName(sec, k) {
    const roll = rnd(sec, k, 41);
    const hex = (n) => hash32(stringHash(String(sec) + ":" + k + ":" + n)).toString(16);
    if (roll < 0.4) return `${hex(1).slice(0, 12)}.cloudfront.net`;
    if (roll < 0.6) return `${hex(2)}${hex(3).slice(0, 8)}.s3.eu-central-1.amazonaws.com`;
    if (roll < 0.75) {
      const probes = ["wpad", "_dmarc", "_ldap._tcp", "autodiscover", "_sip._tcp"];
      const host = NAMES[Math.floor(rnd(sec, k, 42) * NAMES.length) % NAMES.length].name;
      return `${probes[Math.floor(rnd(sec, k, 43) * probes.length) % probes.length]}.${registrable(host)}`;
    }
    return `d${hex(4).slice(0, 6)}.${hex(5).slice(0, 6)}.example-cdn.net`;
  }

  // ── Ein einzelnes Ereignis ───────────────────────────────────────────────

  /** Wie viele Anfragen in diese Sekunde fallen. Der Nachkommateil wird
   *  ausgewürfelt, nicht abgeschnitten — sonst verschwände er. */
  function eventsInSecond(sec) {
    const rate = qps(sec);
    const whole = Math.floor(rate);
    const count = whole + (rnd(sec, 0, 2) < rate - whole ? 1 : 0);
    const events = [];
    for (let k = 0; k < count; k += 1) events.push(makeEvent(sec, k));
    return events;
  }

  function makeEvent(sec, k) {
    const hour = hourOf(sec);

    // 1. Aus welchem Topf kommt die Frage?
    const poolEntry = pick(poolWeights(hour), rnd(sec, k, 1));
    let entry;
    if (poolEntry.key === "tail") {
      // Der Schwanz ist fast immer langweilig; alle paar Minuten steht einer
      // der auffälligen Namen darin.
      const flagged = rnd(sec, k, 2) < 0.0125;
      if (flagged) {
        const found = FLAGGED[Math.floor(rnd(sec, k, 3) * FLAGGED.length) % FLAGGED.length];
        entry = { name: found.name, pool: "tail", flagged: found };
      } else {
        entry = { name: tailName(sec, k), pool: "tail" };
      }
    } else {
      entry = pick(nameWeights(poolEntry.key), rnd(sec, k, 4)) ?? { name: "example.com", pool: poolEntry.key };
    }

    // 2. Von welchem Gerät?
    const client = pick(clientWeights(hour), rnd(sec, k, 5));
    const inWindow = inBedtime(sec) && client.policy === "kids";

    // 3. Wie entscheidet die Policy? Die Reihenfolge ist die der echten
    //    Pipeline: Allowlist vor Blockliste, und ein Treffer auf der
    //    Allowlist beendet die Sache, bevor der Zeitplan greifen kann.
    const verdict = decide(entry, client, inWindow);

    // 4. Wie schnell, und mit welcher Antwort?
    //
    // Die Zahl entsteht genau einmal und wandert danach in die Zeile *und* in
    // die Kette. Zweimal gewürfelt wäre sie zweimal verschieden, und wer eine
    // langsame Zeile anklickt, läse in der Begründung eine schnelle.
    const cached = !verdict.blocked && rnd(sec, k, 6) < hitRate(hour) * (entry.pool === "tail" ? 0.55 : 1);
    const upstream = UPSTREAMS[upstreamFor(entry.name)];
    let ms;
    if (verdict.blocked) {
      ms = 0.08 + rnd(sec, k, 7) * 0.35; // synthetisiert, kein Weg nach draußen
    } else if (entry.forward) {
      ms = 0.9 + rnd(sec, k, 7) * 1.4; // ins eigene Netz, nicht ins Internet
    } else if (cached) {
      ms = 0.15 + rnd(sec, k, 7) * 1.0; // ein Speicherzugriff, kein Netz
    } else {
      ms = upstream.rtt + 0.8 + rnd(sec, k, 7) * 2.0;
      // Die eine Antwort in fünfzig, die hängt — und nur hier. Ein Cache-Treffer,
      // der 180 ms braucht, wäre keine Latenz, sondern Unsinn: die Kette daneben
      // sagt "from cache", und die Zahl widerspräche ihr.
      if (rnd(sec, k, 8) < 0.02) ms = 110 + rnd(sec, k, 9) * 130;
    }

    const roll = rnd(sec, k, 10);
    const rcode = verdict.blocked
      ? "NXDOMAIN"
      : roll < 0.965
        ? "NoError"
        : "NXDOMAIN"; // ein Tippfehler, ein umgezogener Name

    const types = ["A", "A", "A", "A", "A", "A", "A", "AAAA", "AAAA", "HTTPS", "TXT"];
    const type = types[Math.floor(rnd(sec, k, 11) * types.length) % types.length];

    const event = {
      sec,
      name: entry.name,
      type,
      client: client.name,
      blocked: verdict.blocked,
      rcode,
      ms,
      why: [],
      findings: [],
    };

    // 5. Die Kette.
    const steps = [];
    if (client.name === "default") {
      steps.push("no client entry matches, 'default' applies");
    } else {
      steps.push(`Client '${client.name}' recognized by source address`);
    }
    steps.push(`Policy '${client.policy}'`);

    const temporary = decisions.get(entry.name);
    if (temporary && temporary.until > sec) {
      const left = temporary.until - sec;
      if (temporary.kind === "deny") {
        steps.push(`temporary deny, ${left}s left`);
        steps.push("Answer synthesized locally, mode Nxdomain");
      } else {
        steps.push(`temporary allow, ${left}s left`);
        steps.push(...resolution(entry, cached, upstream, sec, k, ms));
      }
    } else if (verdict.allowlisted) {
      steps.push(`Allowlist 'local-allow' line 118: 'branch.io'`);
      steps.push(...resolution(entry, cached, upstream, sec, k, ms));
    } else if (verdict.bySchedule) {
      steps.push("Schedule 'bedtime' active: everything except the allowlist is blocked");
      steps.push("Answer synthesized locally, mode Nxdomain");
    } else if (verdict.byBlocklist) {
      steps.push(blocklistStep(entry));
      steps.push("Answer synthesized locally, mode Nxdomain");
    } else {
      if (entry.flagged) {
        steps.push(
          `${entry.flagged.label} flags (score ${entry.flagged.score}): ${entry.flagged.reason}`,
        );
        event.findings.push({
          detector: entry.flagged.detector,
          label: entry.flagged.label,
          score: entry.flagged.score,
          action: "flag",
          reason: entry.flagged.reason,
        });
      }
      steps.push(...resolution(entry, cached, upstream, sec, k, ms));
    }

    event.why = steps;
    return event;
  }

  /** Alles, was nach der Policy passiert: Cache oder Weg nach draußen. Ein
   *  Cache-Treffer hat keinen DNSSEC-Schritt — es gab nichts nachzurechnen.
   *
   *  `ms` kommt von außen herein und wird hier nicht noch einmal bestimmt: die
   *  Zeile im Protokoll und der Satz in der Kette sind zwei Ansichten derselben
   *  Antwort, und zwei Ansichten dürfen sich nicht widersprechen. */
  function resolution(entry, cached, upstream, sec, k, ms) {
    if (entry.forward) {
      return [`Upstream '10.0.0.1' answered in ${ms.toFixed(1)}ms`];
    }
    if (cached) {
      const left = 30 + Math.floor(rnd(sec, k, 12) * 280);
      return [`from cache (valid, ${left} s left)`];
    }
    // Die Prüfung fällt im Transport, der Upstream-Eintrag danach im Pool —
    // deshalb steht DNSSEC vor dem Resolver und nicht dahinter.
    const verdict = rnd(sec, k, 13) < 0.94 ? "Secure" : "Insecure";
    return [
      `DNSSEC verified locally: ${verdict}`,
      `Upstream '${upstream.name}' answered in ${ms.toFixed(1)}ms`,
    ];
  }

  function blocklistStep(entry) {
    // Eine Wildcard-Liste trifft die Domain, nicht den gefragten Namen.
    // Deshalb steht im Trace bei oisd-big etwas anderes als das Gefragte.
    const matched = entry.list === "oisd-big" ? registrable(entry.name) : entry.name;
    const line = 1 + (hash32(stringHash(entry.name)) % 204000);
    return `Blocklist '${entry.list}' line ${line}: '${matched}'`;
  }

  /** Was die Policy mit einem Namen macht. Die Reihenfolge ist die der echten
   *  Pipeline und keine Erfindung dieser Datei. */
  function decide(entry, client, inWindow) {
    if (entry.verdict === "allowlisted") return { blocked: false, allowlisted: true };
    if (inWindow) return { blocked: true, bySchedule: true };
    if (entry.verdict === "blocked") return { blocked: true, byBlocklist: true };
    return { blocked: false };
  }

  /** Wie oft der Cache greift. Nachts fast immer, zur Abendspitze seltener:
   *  dann kommen viele Namen zum ersten Mal. */
  const hitRate = (hour) => 0.93 - 0.15 * gauss(hour, 20.4, 3.0);

  // ── Was ein Besucher anklicken kann ──────────────────────────────────────

  /** Befristete Freigaben und Sperren. Der einzige Zustand dieser Datei — und
   *  er gehört dem Besucher, nicht der erfundenen Anlage. */
  const decisions = new Map();

  // ── Die Zeitreihe ────────────────────────────────────────────────────────

  const bucketIndex = (sec) => Math.floor(sec / BUCKET_SECONDS);

  /** Der Teil des laufenden Eimers, der bereits vergangen ist — 1 für jeden
   *  abgeschlossenen Eimer.
   *
   *  Der Boden von vier Sekunden ist Absicht: eine Kurve, deren rechte Kante in
   *  den ersten Sekunden eines Eimers platt am Boden klebt und dann springt,
   *  sieht aus, als hätte der Server ausgesetzt.
   *
   *  Die Funktion steht hier und nicht zweimal, weil `history()` und `totals()`
   *  dieselben Eimer verschieden gerechnet haben — eins gegen vier Sekunden.
   *  Der Unterschied ist winzig und trotzdem falsch: die Kurve und die Kennzahl
   *  daneben zeigten für dieselbe Viertelstunde zwei Zahlen. */
  const bucketFraction = (index, now) =>
    index === bucketIndex(now) ? Math.max(now - index * BUCKET_SECONDS, 4) / BUCKET_SECONDS : 1;

  /** Ein Eimer à fünf Minuten. `fraction` ist der Teil, der bereits vergangen
   *  ist: der neueste Eimer wächst noch, genau wie beim echten Ring. */
  function bucketAt(index, fraction) {
    const start = index * BUCKET_SECONDS;
    const span = BUCKET_SECONDS * fraction;
    const jitter = 0.92 + 0.16 * rnd(index, 3, 21);
    const queries = qps(start) * span * jitter;

    const hour = hourOf(start);
    const { byList, bySchedule } = blockedShares(start, hour);
    const blockedByList = queries * byList;
    const blockedBySchedule = queries * bySchedule;
    const blocked = blockedByList + blockedBySchedule;

    // Was nicht geblockt wurde, lief durch den Cache. Treffer und Fehler
    // ergeben zusammen genau diese Menge — sonst widerspräche die Trefferquote
    // der Anfragenzahl.
    const answered = queries - blocked;
    const misses = answered * (1 - hitRate(hour));
    const hits = answered - misses;

    // split_by_zone teilt nahezu gleichmäßig auf; die Abweichung ist
    // Stichprobenrauschen, kein Muster.
    const parts = UPSTREAMS.map((upstream, slot) => {
      const share = 1 / UPSTREAMS.length + (rnd(index, slot + 1, 22) - 0.5) * 0.06;
      return { weight: Math.max(0.01, share), item: upstream };
    });
    const spread = parts.reduce((sum, part) => sum + part.weight, 0);

    return {
      queries,
      blocked,
      cache_hits: hits,
      cache_misses: misses,
      upstreams: parts.map((part) => (misses * part.weight) / spread),
      // Die Aufteilung nach Grund. Sie steht nicht in der Antwort an die Seite
      // — dort steht sie in `/api/status` als Summe —, sondern nur hier, damit
      // `totals()` sie aufaddieren kann. `publicBucket()` wirft sie wieder weg.
      by_list: blockedByList,
      by_schedule: blockedBySchedule,
    };
  }

  /** Ein Eimer in der Form, die `/api/history` ausliefert — und in genau dieser.
   *  Die Zerlegung nach Grund gehört nicht dazu; sie käme in der echten Antwort
   *  auch nicht vor, und ein Feld, das nur die Demo hat, wäre eine Einladung,
   *  sich darauf zu verlassen. */
  function publicBucket(bucket) {
    return {
      queries: bucket.queries,
      blocked: bucket.blocked,
      cache_hits: bucket.cache_hits,
      cache_misses: bucket.cache_misses,
      upstreams: bucket.upstreams,
    };
  }

  /** Wie sich die Anfragen dieser Stunde aufteilen: der Anteil, den die
   *  Blocklisten nehmen, und der, den der Zeitplan nimmt.
   *
   *  Beides wird aus denselben Töpfen und Profilen abgeleitet, aus denen auch
   *  die Ereignisse entstehen. Eine Konstante hier wäre bequemer und falsch:
   *  die Kurve über 24 Stunden und das Protokoll daneben zeigten dann zwei
   *  verschiedene Anlagen.
   *
   *  Die Reihenfolge aus `decide()` steckt in der zweiten Zeile: der Zeitplan
   *  greift vor der Liste, also nimmt er der Liste die Fälle weg, in denen er
   *  selbst zuschlägt. Ohne das Faktorenspiel zählte eine Abfrage zweimal. */
  function blockedShares(sec, hour) {
    const pools = poolWeights(hour);
    const poolTotal = pools.reduce((sum, entry) => sum + entry.weight, 0);
    const listed =
      pools.filter((entry) => entry.item.blocked).reduce((sum, entry) => sum + entry.weight, 0) /
      poolTotal;

    const clients = clientWeights(hour);
    const clientTotal = clients.reduce((sum, entry) => sum + entry.weight, 0);
    const kids =
      clients.filter((entry) => entry.item.policy === "kids").reduce((sum, entry) => sum + entry.weight, 0) /
      clientTotal;
    const bySchedule = inBedtime(sec) ? kids : 0;

    return { byList: listed * (1 - bySchedule), bySchedule };
  }

  /** Die 24 Stunden bis jetzt, ältester Eimer zuerst — wie die echte Ansicht. */
  function history() {
    const now = nowSec();
    const current = bucketIndex(now);
    const buckets = [];
    for (let offset = BUCKETS - 1; offset >= 0; offset -= 1) {
      const index = current - offset;
      buckets.push(publicBucket(bucketAt(index, bucketFraction(index, now))));
    }
    return {
      bucket_seconds: BUCKET_SECONDS,
      upstreams: UPSTREAMS.map((upstream) => upstream.name),
      buckets,
    };
  }

  /** Alles seit dem Start des erfundenen Prozesses. Dieselbe Eimerfunktion wie
   *  die 24-Stunden-Ansicht — die Kennzahlen und die Kurve können deshalb
   *  nicht auseinanderlaufen. */
  let totalsCache = { at: 0, value: null };

  function totals() {
    const now = nowSec();
    if (totalsCache.value && totalsCache.at === now) return totalsCache.value;

    const first = bucketIndex(START);
    const current = bucketIndex(now);
    const sum = {
      queries: 0,
      blocked: 0,
      byList: 0,
      bySchedule: 0,
      hits: 0,
      misses: 0,
      upstreams: UPSTREAMS.map(() => 0),
    };
    for (let index = first; index <= current; index += 1) {
      const bucket = bucketAt(index, bucketFraction(index, now));
      sum.queries += bucket.queries;
      sum.blocked += bucket.blocked;
      sum.byList += bucket.by_list;
      sum.bySchedule += bucket.by_schedule;
      sum.hits += bucket.cache_hits;
      sum.misses += bucket.cache_misses;
      bucket.upstreams.forEach((value, slot) => {
        sum.upstreams[slot] += value;
      });
    }
    totalsCache = { at: now, value: sum };
    return sum;
  }

  // ── Die obersten Namen ───────────────────────────────────────────────────

  /** Wie oft jeder Name über einen Tag gerechnet vorkommt, als Anteil an allen
   *  Anfragen.
   *
   *  Dieselbe Verteilung, aus der auch die Ereignisse gezogen werden — nur über
   *  24 Stunden aufsummiert statt für eine Sekunde ausgewertet. Der Tag wird
   *  stündlich abgetastet; die Töpfe ändern sich nur mit der Stunde, und die
   *  Gewichte innerhalb eines Topfes gar nicht.
   *
   *  Der Umweg lohnt sich: eine von Hand gewählte Verteilung für diese Liste
   *  wäre eine zweite Wahrheit neben der ersten. Und sie flöge auf — zwei Namen
   *  mit demselben Gewicht bekämen exakt dieselbe Zahl, und ein Gleichstand
   *  zwischen zwei unabhängigen Namen ist in echtem Verkehr keiner. */
  function nameShares() {
    const shares = new Map();
    const now = nowSec();
    const buckets = 24;
    for (let step = 0; step < buckets; step += 1) {
      const hour = hourOf(now - step * 3600);
      const pools = poolWeights(hour);
      const poolSum = pools.reduce((sum, pool) => sum + pool.weight, 0);
      for (const pool of pools) {
        const names = nameWeights(pool.item.key);
        const nameSum = names.reduce((sum, entry) => sum + entry.weight, 0);
        if (nameSum === 0) continue; // der lange Schwanz hat hier keine Namen
        for (const entry of names) {
          const share = (pool.weight / poolSum) * (entry.weight / nameSum);
          shares.set(entry.item.name, (shares.get(entry.item.name) ?? 0) + share / buckets);
        }
      }
    }
    return shares;
  }

  /** Die Namen über der k-Schwelle. Ihre Summe ist der Teil der Anfragen, den
   *  die Ansicht zeigen darf; alles darunter steht als Summe daneben.
   *
   *  Die Rechnung muss aufgehen, sonst kann ein Besucher sie widerlegen: bei
   *  k = 5 hat jeder verborgene Name höchstens vier Treffer, und jede Anfrage
   *  ist entweder sichtbar oder verborgen — nie beides und nie keine. Deshalb
   *  ist die Zahl unter der Schwelle keine Schätzung, sondern der Rest. */
  function top(limit) {
    const total = totals().queries;
    const shares = nameShares();

    const domains = NAMES.map((entry) => ({
      name: entry.name,
      count: Math.round(total * (shares.get(entry.name) ?? 0)),
      blocked: entry.verdict === "blocked",
    }))
      .filter((entry) => entry.count >= AGGREGATE_K)
      .sort((one, other) => other.count - one.count);

    const shown = domains.reduce((acc, entry) => acc + entry.count, 0);
    const belowQueries = Math.max(0, Math.round(total - shown));
    // Im Schnitt 3,2 Treffer je verborgenem Namen — unter der Schwelle, und
    // die Summe bleibt dadurch kleiner als das Vierfache der Anzahl.
    const belowNames = Math.ceil(belowQueries / 3.2);

    return {
      threshold: AGGREGATE_K,
      domains: domains.slice(0, limit),
      below_threshold_queries: belowQueries,
      below_threshold_names: belowNames,
    };
  }

  // ── Die Endpunkte ────────────────────────────────────────────────────────

  function status() {
    const sum = totals();
    const answered = sum.hits + sum.misses;
    const reasons = {
      blocklist: Math.round(sum.byList),
      regex: 0,
      schedule: Math.round(sum.bySchedule),
      temporary_deny: decisions.size,
      dga: 0,
      tunneling: 0,
      rebinding: 0,
      typosquat: 0,
      nrd: 0,
      other: 0,
    };

    return {
      version: VERSION,
      uptime_seconds: nowSec() - START,
      // "ring": Namen im Arbeitsspeicher, nichts auf der Platte. In "aggregate"
      // — dem Auslieferungszustand — gäbe es hier gar nichts zu sehen.
      logging_mode: "ring",
      aggregate_k: AGGREGATE_K,
      persists_to_disk: false,
      block_reasons: Object.keys(reasons).map((reason) => ({ reason, count: reasons[reason] })),
      privacy: {
        // Nur dieser eine zählt, wenn ein Client tatsächlich ECS geschickt
        // hat; die anderen drei zählen je ausgehender Anfrage. Vier gleiche
        // Zahlen wären ein Verräter.
        ecs_stripped: Math.round(sum.misses * 0.06),
        padded: Math.round(sum.misses * 0.997),
        case_randomized: Math.round(sum.misses * 0.999),
        cookies: Math.round(sum.misses * 0.998),
      },
      dnssec: {
        enabled: true,
        secure: Math.round(sum.misses * 0.94),
        insecure: Math.round(sum.misses * 0.055),
        // Ein paar verworfene Antworten. Eine dauerhafte Null sähe aus, als
        // wäre die Prüfung abgeschaltet.
        bogus: Math.round(sum.misses * 0.00008),
        indeterminate: Math.round(sum.misses * 0.0049),
      },
      // Zwei der fünf stehen auf "off": Typosquat ohne protect-Liste findet
      // nichts, NRD ohne Quelle läuft leer. Das ist der ehrliche Stand aus der
      // ersten Beobachtungsperiode, nicht der Wunschzustand.
      detectors: [
        { name: "dga", label: "Algorithmically generated name", action: "flag", found: 214 },
        { name: "tunneling", label: "Tunneling", action: "flag", found: 38 },
        { name: "rebinding", label: "DNS rebinding", action: "flag", found: 11 },
        { name: "typosquat", label: "Typosquatting", action: "off", found: 0 },
        { name: "nrd", label: "Newly registered", action: "off", found: 0 },
      ],
      queries: Math.round(sum.queries),
      blocked: Math.round(sum.blocked),
      block_rate: sum.queries === 0 ? 0 : sum.blocked / sum.queries,
      cache_hit_rate: answered === 0 ? 0 : sum.hits / answered,
      cache_entries: 41286,
      list_entries: 482119,
      upstreams: UPSTREAMS.map((upstream, slot) => ({
        name: upstream.name,
        transport: upstream.transport,
        ok: Math.round(sum.upstreams[slot]),
        failed: slot === 1 ? 3 : 0,
        rtt_ms: upstream.rtt + 2.5 * Math.sin(nowSec() / 900 + slot),
        down: false,
      })),
    };
  }

  /** Die jüngsten Anfragen, neueste zuerst. Die letzte volle Sekunde ist die
   *  Grenze: was danach kommt, liefert der Live-Strom — sonst stünde jede
   *  Zeile zweimal im Protokoll. */
  function recent(limit) {
    const now = nowSec();
    const out = [];
    for (let sec = now - 1; sec > now - 900 && out.length < limit; sec -= 1) {
      const batch = eventsInSecond(sec);
      for (let index = batch.length - 1; index >= 0 && out.length < limit; index -= 1) {
        out.push(fullEvent(batch[index]));
      }
    }
    return out;
  }

  /** Die auffälligen Anfragen. Sie kommen aus demselben Strom wie das
   *  Protokoll — nicht aus einer zweiten Quelle, die etwas anderes behaupten
   *  könnte. */
  function flagged(limit) {
    const now = nowSec();
    const out = [];
    for (let sec = now - 1; sec > now - 1200 && out.length < limit; sec -= 1) {
      const batch = eventsInSecond(sec);
      for (let index = batch.length - 1; index >= 0 && out.length < limit; index -= 1) {
        if (batch[index].findings.length) out.push(fullEvent(batch[index]));
      }
    }
    return out;
  }

  function fullEvent(event) {
    return {
      at: isoLocal(event.sec),
      name: event.name,
      type: event.type,
      client: event.client,
      blocked: event.blocked,
      rcode: event.rcode,
      why: event.why,
      ms: event.ms,
      ...(event.findings.length ? { findings: event.findings } : {}),
    };
  }

  function streamEvent(event) {
    return {
      at: clockTime(event.sec),
      blocked: event.blocked,
      rcode: event.rcode,
      ms: event.ms,
      name: event.name,
      client: event.client,
      why: event.why,
    };
  }

  /** Dieselbe Auswertung wie `alpendns policy test`: was würde jetzt
   *  passieren? */
  function explain(domain, client) {
    const name = (domain || "").trim().toLowerCase();
    const who = client || "default";
    const policy = CLIENTS.find((entry) => entry.name === who)?.policy ?? "default";
    const known =
      NAMES.find((entry) => entry.name === name) ??
      FLAGGED.find((entry) => entry.name === name) ?? { name, pool: "tail" };

    const first =
      who === "default"
        ? "no client entry matches, 'default' applies"
        : `Client '${who}' recognized by source address`;
    const steps = [first, `Policy '${policy}'`];

    const temporary = decisions.get(name);
    if (temporary && temporary.until > nowSec()) {
      const left = temporary.until - nowSec();
      if (temporary.kind === "deny") {
        steps.push(`temporary deny, ${left}s left`, "Answer synthesized locally, mode Nxdomain");
        return { domain: name, client: who, blocked: true, steps };
      }
      steps.push(`temporary allow, ${left}s left`);
      steps.push("from cache (valid, 240 s left)");
      return { domain: name, client: who, blocked: false, steps };
    }

    if (known.verdict === "allowlisted") {
      steps.push("Allowlist 'local-allow' line 118: 'branch.io'");
      steps.push("from cache (valid, 240 s left)");
      return { domain: name, client: who, blocked: false, steps };
    }
    if (inBedtime(nowSec()) && policy === "kids") {
      steps.push("Schedule 'bedtime' active: everything except the allowlist is blocked");
      steps.push("Answer synthesized locally, mode Nxdomain");
      return { domain: name, client: who, blocked: true, steps };
    }
    if (known.verdict === "blocked") {
      steps.push(blocklistStep(known));
      steps.push("Answer synthesized locally, mode Nxdomain");
      return { domain: name, client: who, blocked: true, steps };
    }
    const found = FLAGGED.find((entry) => entry.name === name);
    if (found) {
      steps.push(`${found.label} flags (score ${found.score}): ${found.reason}`);
    }
    steps.push(
      found ? "DNSSEC verified locally: Secure" : "from cache (valid, 180 s left)",
    );
    if (found) steps.push(`Upstream '${UPSTREAMS[upstreamFor(name)].name}' answered in 22.4ms`);
    return { domain: name, client: who, blocked: false, steps };
  }

  // ── Zeitformate ──────────────────────────────────────────────────────────

  const pad = (value) => String(value).padStart(2, "0");

  /** `%Y-%m-%dT%H:%M:%S%:z` — was /api/recent liefert. */
  function isoLocal(sec) {
    const at = new Date(sec * 1000);
    const offset = -at.getTimezoneOffset();
    const sign = offset < 0 ? "-" : "+";
    const absolute = Math.abs(offset);
    return (
      `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}` +
      `T${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}` +
      `${sign}${pad(Math.floor(absolute / 60))}:${pad(absolute % 60)}`
    );
  }

  /** `%H:%M:%S` — was der Live-Strom liefert. */
  function clockTime(sec) {
    const at = new Date(sec * 1000);
    return `${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}`;
  }

  // ── Die Endpunkte, von außen gesehen ─────────────────────────────────────

  function answer(path, params, method, body) {
    switch (method + " " + path) {
      case "GET /api/status":
        return status();
      case "GET /api/history":
        return history();
      case "GET /api/top":
        return top(clamp(params.get("limit"), 50));
      case "GET /api/recent":
        return recent(clamp(params.get("limit"), 80));
      case "GET /api/flagged":
        return flagged(clamp(params.get("limit"), 25));
      case "GET /api/explain":
        return explain(params.get("domain"), params.get("client"));
      case "GET /api/allow":
      case "GET /api/deny":
        return [...decisions]
          .filter(([, value]) => value.until > nowSec())
          .map(([domain, value]) => ({
            domain,
            remaining_seconds: value.until - nowSec(),
          }));
      case "POST /api/allow":
      case "POST /api/deny": {
        const domain = JSON.parse(body || "{}").domain ?? "";
        decisions.set(domain, {
          kind: path.endsWith("allow") ? "allow" : "deny",
          until: nowSec() + 3600,
        });
        return { domain, remaining_seconds: 3600 };
      }
      default:
        return null;
    }
  }

  const clamp = (raw, fallback) => Math.min(500, Math.max(1, Number(raw) || fallback));

  /** Ersetzt `fetch`. Der Token-Header wird nicht geprüft: es gibt keinen
   *  Server, der ihn prüfen könnte. */
  window.fetch = function (input, init) {
    const url = typeof input === "string" ? input : (input && input.url) || String(input);
    const [path, query] = url.split("?");
    const method = ((init && init.method) || "GET").toUpperCase();
    const body = (init && init.body) || null;

    let data = null;
    try {
      data = answer(path, new URLSearchParams(query || ""), method, body);
    } catch (error) {
      console.warn("demo: " + path + " konnte nicht beantwortet werden", error);
    }

    if (data === null) {
      // Ein 200 mit {} wäre schlimmer als ein 404: die nächste Fassung von
      // app.js stürbe an `.map` von undefined statt an einer leeren Fläche.
      console.warn("demo: " + method + " " + path + " kennt die Demo nicht");
      return Promise.resolve(
        new Response(JSON.stringify({ error: "not found" }), {
          status: 404,
          headers: { "Content-Type": "application/json" },
        }),
      );
    }
    return Promise.resolve(
      new Response(JSON.stringify(data), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      }),
    );
  };

  /** Der Pfad des Live-Stroms. Steht hier als Zeichenkette und nicht nur in
   *  `app.js`, damit `build.sh` ihn in beiden Dateien findet: der Strom ist der
   *  einzige Endpunkt, der nicht über `fetch` läuft, und ohne diese Zeile müsste
   *  die Drift-Prüfung ihn in der Shell ausnehmen. */
  const EVENTS_PATH = "/api/events";

  /** Ersetzt `EventSource`. Ein echter Server schickt, was gerade passiert;
   *  hier wird es aus der Uhr abgeleitet — und zwar höchstens ein Ereignis je
   *  Takt, damit die Zeilen über die Sekunde verteilt eintreffen statt in
   *  einem Schub. */
  class DemoEventSource {
    constructor(url) {
      this.url = url;
      // Anders als bei `fetch` ist die Adresse hier folgenlos: der Strom kommt
      // ohnehin aus der Uhr. Wenn `app.js` den Endpunkt je umbenennt, ist diese
      // Zeile die einzige Stelle, an der es auffällt.
      if (!String(url).startsWith(EVENTS_PATH)) {
        console.warn("demo: unerwarteter Strom unter " + url);
      }
      this.readyState = 1;
      this.onmessage = null;
      this.closed = false;
      this.second = 0;
      this.sent = 0;
      this.timer = setInterval(() => this.pump(), 250);
    }

    pump() {
      if (this.closed || typeof this.onmessage !== "function") return;
      const sec = nowSec();
      if (sec !== this.second) {
        this.second = sec;
        this.sent = 0;
      }
      const queue = eventsInSecond(sec);
      if (this.sent < queue.length) {
        this.onmessage({ data: JSON.stringify(streamEvent(queue[this.sent])) });
        this.sent += 1;
      }
    }

    /** Muss wirklich aufräumen: app.js schließt vor jedem Neuverbinden, und
     *  ein liegengebliebener Takt schriebe weiter in eine abgehängte Tabelle. */
    close() {
      this.closed = true;
      this.readyState = 2;
      clearInterval(this.timer);
      this.timer = null;
      this.onmessage = null;
    }
  }
  window.EventSource = DemoEventSource;

  // ── Der Hinweis ──────────────────────────────────────────────────────────
  //
  // Ohne ihn könnte ein Besucher glauben, er sähe sein eigenes DNS. Er sitzt
  // als Pille neben dem Schriftzug und bedient sich derselben Variablen wie
  // die Faktenzeile daneben — keine neuen Farben, keine neuen Abstände.

  const NOTICE = "Demo · sample data";
  const NOTICE_TITLE =
    "This page is a self-contained demo. Every number in it is generated in " +
    "your browser: there is no server behind it and no DNS query was made.";

  function addNotice() {
    const mark = document.querySelector(".masthead-inner .mark");
    if (!mark) return;
    const style = document.createElement("style");
    style.textContent =
      ".demo-pill{font-size:var(--t-micro);text-transform:uppercase;" +
      "letter-spacing:0.08em;color:var(--faint);border:1px solid var(--hairline);" +
      "border-radius:var(--radius);padding:var(--s-4) var(--s-8);" +
      "white-space:nowrap;cursor:help}";
    document.head.append(style);
    const pill = document.createElement("span");
    pill.className = "demo-pill";
    pill.textContent = NOTICE;
    pill.title = NOTICE_TITLE;
    mark.after(pill);
    document.title = "AlpenDNS — Demo";
  }

  // ── Ohne Speicher geht es auch ───────────────────────────────────────────
  //
  // app.js liest den Token ganz oben ohne try/catch. Wirft der Zugriff auf
  // localStorage — privates Fenster, strenge Einstellungen —, stirbt das ganze
  // Skript und die Seite bliebe leer; nicht einmal die Anmeldung erschiene.

  try {
    window.localStorage.getItem("alpendns.probe");
  } catch {
    const memory = new Map();
    try {
      Object.defineProperty(window, "localStorage", {
        configurable: true,
        value: {
          getItem: (key) => (memory.has(key) ? memory.get(key) : null),
          setItem: (key, value) => memory.set(key, String(value)),
          removeItem: (key) => memory.delete(key),
          clear: () => memory.clear(),
        },
      });
    } catch {
      /* dann eben ohne */
    }
  }

  addNotice();

  // Für die Nachprüfung ohne Browser: die Welt ist eine reine Funktion, also
  // lässt sie sich von außen befragen, ohne dass eine Seite gezeichnet wird.
  window.__demoWorld = {
    status,
    history,
    top,
    recent,
    flagged,
    explain,
    eventsInSecond,
    qps,
    totals,
    isoLocal,
  };
})();
