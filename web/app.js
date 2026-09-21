// AlpenDNS — Web-UI.
//
// Kein Framework, kein Build-Schritt, keine externe Ressource: die Seite liegt
// im Binary und funktioniert ohne Internet (CLAUDE.md B.6). Der Token wird im
// Browser gehalten und bei jeder Anfrage mitgeschickt; der Server kennt keine
// Sitzungen.
//
// Die Leitidee ist die aus ADR-0004: alles zeigen, nichts merken. Jede Kurve
// auf dieser Seite kommt aus Zählern, nie aus Namen — die Zeitreihe unter
// /api/history kann gar keinen Namen enthalten, weil sie keine Felder dafür
// hat. Was an Namen sichtbar ist, hat entweder die k-Schwelle überschritten
// oder steht im Ringpuffer, den der Log-Modus erlaubt.
//
// Benutzt werden genau die Endpunkte, die es gibt: /api/status für den Rahmen,
// /api/events und /api/recent für das Protokoll, /api/top für die Nebenspalte,
// /api/history für die Kurven und /api/explain für den Klick auf eine Zeile.
//
// Unter Last gilt: der Strom diktiert nicht das Tempo der Seite. Ereignisse
// werden gesammelt und höchstens einmal je Bild gezeichnet — sonst erzwingt
// jede einzelne Anfrage ein Layout, und bei einem Lasttest steht der Browser.
// Der Server deckelt zusätzlich, wie viele Nachrichten er je Sekunde schickt.
"use strict";

const TOKEN_KEY = "alpendns.token";
/** Derselbe Schlüssel, den das kurze Skript im Kopf der Seite liest. */
const THEME_KEY = "alpendns.theme";
/** So viele Zeilen hält das Protokoll. Darüber fällt die älteste heraus. */
const MAX_ROWS = 300;
const POLL_MS = 5000;
/** Die Zeitreihe wächst in Fünf-Minuten-Schritten; öfter zu fragen bringt nichts. */
const HISTORY_MS = 60_000;
/** Breite der Sparkline in Sekunden. */
const SPARK_SECONDS = 60;
/** Ab hier gilt eine Antwort als schnell bzw. als langsam (Millisekunden). */
const MS_FAST = 20;
const MS_SLOW = 100;
/** So viele Upstreams bekommen ein eigenes Band; der Rest wird zusammengefasst. */
const SPLIT_BANDS = 4;

// Nur aus dem Speicher des Browsers. Den Token zusätzlich aus der URL zu lesen
// wäre bequem und würde ihn in jeden Verlauf schreiben; für den Live-Strom ist
// er dort unvermeidlich, überall sonst nicht.
let token = localStorage.getItem(TOKEN_KEY) || "";
let stream = null;
/** Gesetzt, solange der Server in einem Modus läuft, der keine Namen behält. */
let quietMode = "";
/** Ohne Namen (Modus none/aggregate) hat das Protokoll keine Namensspalte. */
let namesAvailable = true;

const $ = (id) => document.getElementById(id);

/** Baut eine Zelle über textContent. Fremde Daten werden nie als Auszeichnung
 *  eingefügt: ein Domainname aus dem Netz ist Text, kein Markup. */
function cell(text, className) {
  const td = document.createElement("td");
  td.textContent = text;
  if (className) td.className = className;
  return td;
}

async function api(path) {
  const response = await fetch(path, {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
}

// ── Zahlen ────────────────────────────────────────────────────────────────
//
// Deutsches Format überall: Punkt als Tausendertrennung, Komma als
// Dezimalzeichen. Eine Oberfläche, die "1,234" und "1.234" mischt, lässt jede
// Zahl zweimal lesen.

const LOCALE = "en-US";

const thousands = (value) => Number(value ?? 0).toLocaleString(LOCALE);

const percent = (value) =>
  `${(Number(value ?? 0) * 100).toLocaleString(LOCALE, {
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  })} %`;

const decimal = (value, digits) =>
  Number(value ?? 0).toLocaleString(LOCALE, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });

/** Die Uhrzeit aus einem Zeitstempel.
 *
 *  /api/recent liefert den vollen ISO-Stempel, der Live-Strom nur die Uhrzeit.
 *  Im Protokoll steht beides untereinander, also wird hier vereinheitlicht —
 *  ein Datum in jeder Zeile wäre in einem Live-Protokoll ohnehin Ballast. */
function clockTime(at) {
  return at.includes("T") ? at.slice(11, 19) : at;
}

/** Die Farbklasse für eine Latenz — oder keine, wenn sie nichts zu sagen hat.
 *  Dieselben Schwellen für Upstreams und Protokoll, sonst hieße dieselbe Farbe
 *  an zwei Stellen zweierlei. */
function latencyClass(ms) {
  if (ms < MS_FAST) return "ms-fast";
  if (ms < MS_SLOW) return null;
  return "ms-slow";
}

function formatUptime(seconds) {
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (d > 0) return `${thousands(d)} d ${h} h`;
  if (h > 0) return `${h} h ${m} min`;
  return `${m} min`;
}

/** Der Punkt neben der Laufzeit: ist der Status gerade abrufbar? */
function setReachable(reachable) {
  const box = $("reach");
  box.classList.toggle("is-stale", !reachable);
  box.title = reachable
    ? "Status is reachable"
    : "No response — the numbers are the last known state";
}

/** Leere Flächen sagen, warum sie leer sind. Die Tabelle weicht dabei ganz:
 *  ein Spaltenkopf ohne Zeilen fällt in sich zusammen und sieht kaputt aus. */
function syncEmptyStates() {
  const hasRows = $("log").childElementCount > 0;
  $("log-empty").hidden = hasRows;
  $("log-table").hidden = !hasRows;
  $("top-empty").hidden = $("top").childElementCount > 0;
  $("flagged-empty").hidden = $("flagged").childElementCount > 0;
}

/** Setzt den Pfad eines eingebetteten Diagramms.
 *
 *  Alle Kurven dieser Seite laufen hier durch: das <path> steht im HTML, das
 *  Skript setzt nur sein d. Kein Element wird im SVG-Namensraum erzeugt, keine
 *  Bibliothek geladen (B.6). */
function setPath(id, d) {
  const path = $(id);
  path.setAttribute("d", d);
}

// ── Die Sparkline ─────────────────────────────────────────────────────────
//
// Sie zählt die Anfragen des Live-Stroms je Sekunde — dieselbe Quelle wie das
// Protokoll, kein zusätzlicher Endpunkt. Der Puffer wächst von einer Sekunde
// auf sechzig und schiebt erst dann: so füllt sich die Linie von links, statt
// eine Minute Nullen zu behaupten, die niemand beobachtet hat.

const spark = [0];

function drawSpark() {
  if (spark.length < 2) {
    setPath("spark", "");
    return;
  }
  // Der Maßstab folgt der Spitze; mindestens 1, damit eine ruhige Minute flach
  // am Boden liegt statt durch Rundungsrauschen zu zappeln.
  setPath("spark", linePath(spark, Math.max(1, ...spark), 240 / (SPARK_SECONDS - 1), 39, 38));
}

function tickSpark() {
  spark.push(0);
  if (spark.length > SPARK_SECONDS) spark.shift();
  // Im Hintergrund weiterzählen, aber nicht zeichnen: ein Diagramm, das
  // niemand sieht, ist reine Rechenzeit.
  if (!document.hidden) drawSpark();
}

/** Eine Linie über gleich breite Punkte, von links nach rechts. */
function linePath(values, peak, step, base, span) {
  let d = "";
  values.forEach((value, index) => {
    const x = (index * step).toFixed(1);
    const y = (base - (value / peak) * span).toFixed(1);
    d += `${index === 0 ? "M" : " L"}${x} ${y}`;
  });
  return d;
}

// ── Die Zeitreihe: 24 Stunden aus Zählern ─────────────────────────────────
//
// Der Server hält 288 Eimer à fünf Minuten im Arbeitsspeicher. Ein Neustart
// setzt sie zurück, und das ist kein Mangel: was der Prozess nicht überlebt,
// kann auch niemand später auslesen.

const DAY_WIDTH = 240;
const DAY_HEIGHT = 52;
const SPLIT_HEIGHT = 44;

function drawDay(history) {
  const buckets = history.buckets ?? [];
  // Ein einzelner Punkt ist noch keine Kurve.
  const ready = buckets.length >= 2;
  $("day-empty").hidden = ready;
  for (const id of ["chart-traffic", "chart-cache"]) $(id).hidden = !ready;
  document.querySelector(".axis").hidden = !ready;
  if (!ready) return;

  const step = DAY_WIDTH / (buckets.length - 1);
  const queries = buckets.map((bucket) => bucket.queries);
  const blocked = buckets.map((bucket) => bucket.blocked);
  // Beide Kurven teilen sich den Maßstab: sonst sähen zwei geblockte Anfragen
  // aus wie tausend, und der Vergleich, um den es geht, wäre gelogen.
  const peak = Math.max(1, ...queries);
  setPath("day-queries", linePath(queries, peak, step, DAY_HEIGHT - 1, DAY_HEIGHT - 2));
  setPath("day-blocked", linePath(blocked, peak, step, DAY_HEIGHT - 1, DAY_HEIGHT - 2));

  // Die Trefferquote ist ein Anteil und hat deshalb einen festen Maßstab von
  // null bis eins — eine automatische Skalierung würde ruhige Phasen
  // dramatisieren.
  const rate = buckets.map((bucket) => {
    const total = bucket.cache_hits + bucket.cache_misses;
    return total === 0 ? 0 : bucket.cache_hits / total;
  });
  setPath("day-cache", linePath(rate, 1, step, DAY_HEIGHT - 1, DAY_HEIGHT - 2));
}

/** Die Aufteilung der Anfragen auf die Upstreams, als gestapelte Flächen.
 *
 *  Die Frage dahinter ist keine Auslastungsfrage, sondern eine Privacy-Frage:
 *  wenn split_by_zone funktioniert, sieht kein Resolver mehr als seinen Teil
 *  der Namen. Eine schiefe Fläche heißt, dass einer zu viel sieht. */
function drawSplit(history) {
  const buckets = history.buckets ?? [];
  const names = history.upstreams ?? [];
  const bands = Math.min(SPLIT_BANDS, Math.max(names.length, 1));
  const step = buckets.length > 1 ? DAY_WIDTH / (buckets.length - 1) : 0;

  // Anteile je Eimer, auf höchstens vier Bänder gefaltet.
  const shares = buckets.map((bucket) => {
    const values = new Array(bands).fill(0);
    (bucket.upstreams ?? []).forEach((value, index) => {
      const slot = Math.min(index, bands - 1);
      values[slot] += value;
    });
    const total = values.reduce((sum, value) => sum + value, 0);
    return total === 0 ? values.map(() => 0) : values.map((value) => value / total);
  });

  for (let band = 0; band < SPLIT_BANDS; band += 1) {
    const path = `split-${band + 1}`;
    if (band >= bands || buckets.length < 2) {
      setPath(path, "");
      continue;
    }
    // Obere Kante hin, untere Kante zurück: eine geschlossene Fläche ohne
    // Lücken zwischen den Bändern.
    const upper = [];
    const lower = [];
    shares.forEach((values, index) => {
      const x = (index * step).toFixed(1);
      let below = 0;
      for (let i = 0; i < band; i += 1) below += values[i];
      const top = below + values[band];
      upper.push(`${x} ${(SPLIT_HEIGHT - top * SPLIT_HEIGHT).toFixed(1)}`);
      lower.push(`${x} ${(SPLIT_HEIGHT - below * SPLIT_HEIGHT).toFixed(1)}`);
    });
    lower.reverse();
    setPath(path, `M${upper.join(" L")} L${lower.join(" L")} Z`);
  }
}

async function refreshHistory() {
  const history = await api("/api/history");
  drawDay(history);
  drawSplit(history);
}

// ── Frage 1: Läuft er? ────────────────────────────────────────────────────

function renderUpstreams(list) {
  const box = $("upstreams");
  box.replaceChildren();
  list.forEach((upstream, index) => {
    const item = document.createElement("li");

    // Das Band ordnet die Zeile der Fläche darüber zu; der Punkt sagt, ob der
    // Upstream gerade antwortet. Zwei Zeichen, zwei Aussagen.
    const swatch = document.createElement("span");
    swatch.className = `swatch band-${Math.min(index + 1, SPLIT_BANDS)}`;

    const dot = document.createElement("span");
    dot.className = `up-dot ${upstream.down ? "is-down" : "is-up"}`;

    const name = document.createElement("span");
    name.className = "up-name";
    name.textContent = upstream.name;
    name.title = `${upstream.name} over ${upstream.transport.toUpperCase()}`;

    const rtt = document.createElement("span");
    rtt.className = "up-rtt";
    if (upstream.down) {
      rtt.textContent = "down";
    } else if (upstream.rtt_ms === null) {
      rtt.textContent = "–";
    } else {
      rtt.textContent = `${decimal(upstream.rtt_ms, 0)} ms`;
      const slot = latencyClass(upstream.rtt_ms);
      if (slot) rtt.classList.add(slot);
    }

    item.append(swatch, dot, name, rtt);
    box.append(item);
  });

  // Der Transport-Mix gehört in die Kopfzeile des Panels: er beantwortet
  // "verschlüsselt womit", und das ist eine Eigenschaft der ganzen Gruppe.
  const mix = new Map();
  for (const upstream of list) {
    mix.set(upstream.transport, (mix.get(upstream.transport) ?? 0) + 1);
  }
  $("up-note").textContent = [...mix]
    .map(([transport, count]) => `${count}× ${transport.toUpperCase()}`)
    .join(" · ");
}

/** Der aktive Log-Modus, als Kennzahl im Kopf neben Version und Laufzeit. */
function renderMode(status) {
  $("mode").textContent = status.logging_mode;
  quietMode =
    status.logging_mode === "none" || status.logging_mode === "aggregate"
      ? status.logging_mode
      : "";
}

/** Die Verteilung der Block-Gründe als ein Balken. */
function renderReasons(reasons) {
  const LABELS = {
    blocklist: "Blocklist",
    regex: "Regex rule",
    schedule: "Schedule",
    temporary_deny: "temporarily denied",
    dga: "Generated name",
    tunneling: "Tunneling",
    rebinding: "Rebinding",
    typosquat: "Typosquatting",
    nrd: "Newly registered",
    other: "unassigned",
  };
  const total = reasons.reduce((sum, entry) => sum + entry.count, 0);
  const bar = $("reason-bar");
  const legend = $("reason-legend");
  bar.replaceChildren();
  legend.replaceChildren();

  bar.hidden = total === 0;
  $("reason-empty").hidden = total !== 0;
  if (total === 0) return;

  // Nur die Gründe, die auch vorkommen. Seit Phase 8 sind es zehn Kategorien,
  // und neun leere Legendenzeilen wären keine Information, sondern Rauschen.
  const present = reasons.filter((entry) => entry.count > 0);
  present.forEach((entry, index) => {
    // Die Graustufen-Rampe hat vier Stufen (B.6). Bei mehr Kategorien wiederholt
    // sie sich; die Zuordnung trägt ohnehin die Legende, nicht der Ton.
    const band = `band-${(index % SPLIT_BANDS) + 1}`;

    const segment = document.createElement("span");
    segment.className = band;
    segment.style.width = `${(entry.count / total) * 100}%`;
    segment.title = `${LABELS[entry.reason] ?? entry.reason}: ${thousands(entry.count)}`;
    bar.append(segment);

    const item = document.createElement("li");
    const swatch = document.createElement("span");
    swatch.className = `swatch ${band}`;
    const name = document.createElement("span");
    name.className = "n";
    name.textContent = LABELS[entry.reason] ?? entry.reason;
    const count = document.createElement("span");
    count.className = "c";
    count.textContent = thousands(entry.count);
    item.append(swatch, name, count);
    legend.append(item);
  });
}

async function refreshStatus() {
  const status = await api("/api/status");
  setReachable(true);

  $("version").textContent = status.version;
  $("uptime").textContent = formatUptime(status.uptime_seconds);

  $("queries").textContent = thousands(status.queries);
  // Die Anzahl ist die Kennzahl, der Anteil ihr Kleingedrucktes.
  const rate = document.createElement("span");
  rate.className = "trail";
  rate.textContent = percent(status.block_rate);
  $("blocked").replaceChildren(
    document.createTextNode(thousands(status.blocked)),
    rate,
  );
  $("hitrate").textContent = percent(status.cache_hit_rate);
  $("entries").textContent = thousands(status.list_entries);

  renderUpstreams(status.upstreams);
  renderMode(status);
  renderReasons(status.block_reasons ?? []);

  $("pc-ecs").textContent = thousands(status.privacy.ecs_stripped);
  $("pc-pad").textContent = thousands(status.privacy.padded);
  $("pc-case").textContent = thousands(status.privacy.case_randomized);
  $("pc-cookie").textContent = thousands(status.privacy.cookies);
  $("pc-secure").textContent = thousands(status.dnssec.secure);
  const bogus = $("pc-bogus");
  bogus.textContent = thousands(status.dnssec.bogus);
  bogus.classList.toggle("is-bogus", status.dnssec.bogus > 0);

  // Der aktive Log-Modus bestimmt, was das Protokoll überhaupt zeigen kann.
  const quiet = status.logging_mode === "none" || status.logging_mode === "aggregate";
  namesAvailable = !quiet;
  const note = $("live-note");
  // Nur der Modus "full" schreibt jede Zeile auf die Platte — das ist hier die
  // einzige gelbe Warnung. Die übrigen Modi bleiben neutral und ohne Zusatz.
  note.classList.toggle("is-persisting", status.persists_to_disk);
  if (quiet) {
    note.textContent = `ephemeral · mode '${status.logging_mode}': you can see that something happens — not what`;
  } else if (status.persists_to_disk) {
    note.textContent = "mode 'full' · every row is additionally written to a file";
  } else {
    note.textContent = "ephemeral";
  }

  // Dasselbe für die Begründung: ohne Namen gibt es keine anklickbare Zeile,
  // und der Aufforderungssatz wäre eine Anleitung ins Leere.
  $("why-empty-note").textContent = quiet
    ? `mode '${status.logging_mode}': the server keeps no names, so nothing can appear here.`
    : "Nothing has been blocked since this page was opened. Click a row in the log to ask for its decision chain.";

  if (status.logging_mode === "none") {
    $("top-note").textContent = "In mode 'none', no names are counted.";
  } else if (quiet) {
    $("top-note").textContent = "Names only appear above the k-threshold.";
  } else {
    $("top-note").textContent = "No name seen often enough yet.";
  }
}

// ── Frage 3: Warum wurde das geblockt? ────────────────────────────────────

function showReason(event) {
  const box = $("why-body");
  box.replaceChildren();

  const subject = document.createElement("span");
  subject.className = "subject";
  // Die Farbe des Namens wiederholt nur das Verdikt aus der Kontextzeile: rot
  // für geblockt, grün für durchgelassen. Geblockt ist der Regelfall dieses
  // Panels, also bleibt der Name ohne Klasse rot.
  if (!event.blocked) subject.classList.add("is-allowed");
  subject.textContent = event.name;
  box.append(subject);

  const context = document.createElement("p");
  context.className = "context";
  context.textContent =
    `at ${clockTime(event.at)} · ${event.client ?? "unknown"} · answered with ${event.rcode}`;
  box.append(context);

  const chain = document.createElement("ol");
  chain.className = "chain";
  for (const step of event.why ?? []) {
    const item = document.createElement("li");
    item.textContent = step;
    chain.append(item);
  }
  box.append(chain);

  $("why-source").hidden = true;
  $("why-empty").hidden = true;
  box.hidden = false;
}

/** Die Entscheidungskette auf Anfrage — dieselbe Auswertung wie
 *  `alpendns policy test`, nur über /api/explain.
 *
 *  Der Unterschied zur Kette aus dem Protokoll: die dort ist ein Protokoll
 *  dessen, was passiert ist; diese hier ist die Antwort auf "und was würde
 *  jetzt passieren?". Deshalb steht dabei, woher sie kommt.
 *
 *  Die angeklickte Zeile ist zugleich die Anweisung, das Panel in Ruhe zu
 *  lassen: solange sie gesetzt ist, führt der Strom die Begründung nicht mehr
 *  nach. Eine Auswahl, die die nächste geblockte Anfrage überschreibt, ist
 *  keine — man liest die Kette ja nicht in der Sekunde, in der man klickt. */
let askedRow = null;

async function explainRow(name, client, row) {
  if (askedRow) askedRow.classList.remove("is-asked");
  askedRow = row;
  row.classList.add("is-asked");

  const query = new URLSearchParams({ domain: name });
  // "default" ist kein Client, sondern der Name für "keine Regel passt". Den
  // kann der Server nicht in eine Adresse übersetzen — also wird er nicht
  // mitgeschickt, und /api/explain wertet mit der Loopback-Adresse aus, was
  // dieselbe Default-Policy trifft.
  if (
    client &&
    client !== "–" &&
    client !== "unknown" &&
    client !== "default"
  ) {
    query.set("client", client);
  }

  const source = $("why-source");
  try {
    const answer = await api(`/api/explain?${query}`);
    showReason({
      name: answer.domain,
      at: new Date().toTimeString().slice(0, 8),
      client: answer.client,
      blocked: answer.blocked,
      rcode: answer.blocked ? "blocked" : "allowed",
      why: answer.steps,
    });
    source.textContent = "evaluated now, not from the log";
    source.hidden = false;
  } catch {
    source.textContent = "Evaluation was not possible.";
    source.hidden = false;
  }
}

// ── Frage 2: Was gerade passiert ──────────────────────────────────────────

/** Die Antwort als Badge. Geblockt sagt der Text, die Farbe wiederholt es nur —
 *  wer sie nicht sieht, verliert nichts. Der echte RCODE bleibt im Titel. */
function rcodeCell(event) {
  const td = document.createElement("td");
  td.className = "c-rcode";
  const badge = document.createElement("span");
  badge.className = "badge";
  if (event.blocked) {
    badge.classList.add("is-blocked");
    badge.textContent = "BLOCKED";
    badge.title = event.rcode;
  } else {
    badge.textContent = event.rcode;
    if (event.rcode === "NXDOMAIN") badge.classList.add("is-muted");
  }
  td.append(badge);
  return td;
}

function row(event) {
  const tr = document.createElement("tr");
  if (event.blocked) tr.className = "is-blocked";
  tr.append(cell(clockTime(event.at), "c-time"));

  const name = cell(event.name ?? "no name", "c-name");
  if (!event.name) name.classList.add("unnamed");
  tr.append(name);

  tr.append(cell(event.client ?? "–", "c-client"));
  tr.append(rcodeCell(event));

  const ms = cell(decimal(event.ms, 1), "c-ms");
  const slot = latencyClass(event.ms);
  if (slot) ms.classList.add(slot);
  tr.append(ms);

  // Anklickbar nur mit Namen: ohne ihn gibt es nichts zu erklären, und eine
  // Zeile, die auf einen Klick nicht reagiert, ist schlimmer als eine, die
  // gar nicht danach aussieht.
  //
  // Die Zeile bekommt keine eigenen Zuhörer: bei 300 Zeilen wären das 600, und
  // jede neue Zeile müsste zwei anlegen. Stattdessen hört die Tabelle einmal zu
  // und findet die Zeile über closest() — der Name hängt am Element.
  if (event.name) {
    tr.classList.add("askable");
    tr.tabIndex = 0;
    tr.title = "Ask for decision chain";
    tr.dataset.domain = event.name;
    if (event.client) tr.dataset.client = event.client;
  }
  return tr;
}

/** Ein Klick oder Enter irgendwo in der Tabelle. */
function onRowActivate(trigger) {
  if (trigger.type === "keydown") {
    if (trigger.key !== "Enter" && trigger.key !== " ") return;
    trigger.preventDefault();
  }
  const tr = trigger.target.closest("tr.askable");
  if (tr) explainRow(tr.dataset.domain, tr.dataset.client, tr);
}

// Was seit dem letzten Bild hereinkam. Mehr als MAX_ROWS zu behalten wäre
// sinnlos: alles Ältere fiele beim Zeichnen sofort wieder aus der Tabelle.
const pending = [];
let flushScheduled = false;

function addRow(event) {
  // Die Sparkline zählt jede Anfrage — auch die, die der Server ausgelassen
  // hat. Sonst zeigte die Linie unter Last einen ruhigen Server.
  spark[spark.length - 1] += 1 + (event.skipped ?? 0);

  // Im Hintergrund gibt es nichts zu zeichnen. Der Puls bleibt trotzdem
  // gezählt, damit die Linie beim Zurückkommen stimmt.
  if (document.hidden) return;

  pending.push(event);
  if (pending.length > MAX_ROWS) pending.splice(0, pending.length - MAX_ROWS);
  if (!flushScheduled) {
    flushScheduled = true;
    requestAnimationFrame(flushRows);
  }
}

/** Zeichnet alle gesammelten Ereignisse in einem Durchgang.
 *
 *  Der teure Teil am alten Weg war nicht das Erzeugen der Zeilen, sondern das
 *  Wechselspiel: anhängen, Höhe lesen, scrollen — pro Anfrage einmal, und jedes
 *  Lesen zwingt den Browser, das Layout sofort neu zu rechnen. Hier wird alles
 *  in einem Fragment gebaut, einmal eingehängt und einmal gemessen. */
function flushRows() {
  flushScheduled = false;
  if (pending.length === 0) return;

  const body = $("log");
  const scroller = $("log-scroll");
  const wasAtTop = scroller.scrollTop < 4;
  const heightBefore = scroller.scrollHeight;

  // Das Protokoll wächst nach oben, also rückwärts durch den Puffer: die
  // jüngste Zeile steht am Ende und muss zuoberst landen.
  const fragment = document.createDocumentFragment();
  let newestBlocked = null;
  for (let index = pending.length - 1; index >= 0; index -= 1) {
    const event = pending[index];
    fragment.append(row(event));
    if (!newestBlocked && event.blocked && event.name) newestBlocked = event;
  }
  body.prepend(fragment);

  while (body.childElementCount > MAX_ROWS) body.lastElementChild.remove();

  // Wer im Protokoll nach unten gescrollt hat, soll nicht mitgeschoben werden.
  if (!wasAtTop) {
    scroller.scrollTop += scroller.scrollHeight - heightBefore;
  }

  pending.length = 0;
  syncEmptyStates();
  // Nur die jüngste Begründung: unter Last wäre alles andere ein Flackern, das
  // niemand lesen kann. Und gar keine, sobald jemand selbst eine Zeile gewählt
  // hat — dann gehört das Panel dieser Zeile, bis die Seite neu geladen wird.
  if (newestBlocked && !askedRow) showReason(newestBlocked);
}

async function refreshTop() {
  const report = await api("/api/top?limit=12");
  const list = $("top");
  list.replaceChildren();
  for (const entry of report.domains ?? []) {
    const item = document.createElement("li");
    if (entry.blocked) item.className = "is-blocked";
    const name = document.createElement("span");
    name.className = "n";
    name.textContent = entry.name;
    const count = document.createElement("span");
    count.className = "c";
    count.textContent = thousands(entry.count);
    item.append(name, count);
    list.append(item);
  }

  // Was die Liste verschweigt, steht als Summe daneben. Ohne diese Zeile sähe
  // ein Server mit viel seltenem Verkehr aus wie einer ohne Verkehr.
  const hidden = $("top-hidden");
  if (report.below_threshold_queries > 0) {
    hidden.textContent =
      `${thousands(report.below_threshold_queries)} queries across ` +
      `${thousands(report.below_threshold_names)} names below the k-threshold`;
    hidden.hidden = false;
  } else {
    hidden.hidden = true;
  }
  syncEmptyStates();
}

/**
 * Die auffälligen Anfragen.
 *
 * Kommen aus demselben Ringpuffer wie das Protokoll — in den leisen Log-Modi
 * gibt der nichts heraus, und dann bleibt dieses Panel leer. Das ist kein
 * Fehler, sondern die Einstellung, und der Leertext sagt es auch.
 */
async function refreshFlagged() {
  const entries = await api("/api/flagged?limit=25");
  const list = $("flagged");
  list.replaceChildren();

  for (const entry of entries) {
    for (const finding of entry.findings ?? []) {
      list.append(flaggedRow(entry, finding));
    }
  }
  $("flagged-note").textContent = entries.length
    ? `${thousands(entries.length)} in the last few minutes`
    : "";
  $("flagged-note").hidden = entries.length === 0;
  // Leer heißt nicht immer "nichts gefunden": in den leisen Log-Modi behält der
  // Server keine Namen, und dann kann hier nichts stehen. Das gehört gesagt,
  // sonst sieht ein zurückhaltend eingestellter Server aus wie ein untätiger.
  $("flagged-note-empty").textContent = quietMode
    ? `mode '${quietMode}': the server keeps no names, so nothing can appear here.`
    : "No heuristic has flagged anything.";
  syncEmptyStates();
}

function flaggedRow(entry, finding) {
  const item = document.createElement("li");

  const subject = document.createElement("span");
  subject.className = "subject";
  subject.textContent = entry.name;

  const meta = document.createElement("p");
  meta.className = "meta";
  const label = document.createElement("span");
  label.textContent = finding.label;
  const score = document.createElement("span");
  score.className = "score";
  score.textContent = finding.score;
  const action = document.createElement("span");
  action.textContent = finding.action === "block" ? "blocked" : "flagged";
  meta.append(label, score, action);

  // Die Merkmale, die zum Score geführt haben. Ohne sie ist ein Fehlalarm
  // nicht nachvollziehbar, und dann wird die Heuristik abgeschaltet statt
  // verbessert.
  const why = document.createElement("p");
  why.className = "why";
  why.textContent = finding.reason;

  const actions = document.createElement("div");
  actions.className = "actions";
  actions.append(
    decideButton("Allow", `/api/allow`, entry.name),
    decideButton("Deny", `/api/deny`, entry.name),
  );

  item.append(subject, meta, why, actions);
  return item;
}

/** Ein Knopf, der eine befristete Freigabe oder Sperre setzt. */
function decideButton(caption, path, domain) {
  const button = document.createElement("button");
  button.type = "button";
  button.textContent = caption;
  button.addEventListener("click", async () => {
    button.disabled = true;
    try {
      await post(path, { domain, seconds: 3600 });
      button.textContent = `${caption} ✓`;
    } catch {
      button.textContent = "failed";
      button.disabled = false;
    }
  });
  return button;
}

async function post(path, body) {
  const response = await fetch(path, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Authorization: `Bearer ${token}`,
    },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(String(response.status));
  return response.json();
}

function connectStream() {
  if (stream) stream.close();
  // EventSource kann keine Header setzen, deshalb der Token in der URL.
  stream = new EventSource(`/api/events?token=${encodeURIComponent(token)}`);
  stream.onmessage = (message) => {
    try {
      addRow(JSON.parse(message.data));
    } catch {
      /* eine kaputte Zeile ist kein Grund, den Strom abzubrechen */
    }
  };
}

/** Füllt das Protokoll beim Laden, damit die Seite nicht leer beginnt. */
async function loadRecent() {
  const recent = await api("/api/recent?limit=80");
  // Ein Fragment, ein Einhängen: 80-mal einzeln voranzustellen heißt 80-mal
  // Layout.
  const fragment = document.createDocumentFragment();
  for (const event of recent) fragment.append(row(event));
  $("log").prepend(fragment);
  const newestBlocked = recent.find((event) => event.blocked && event.name);
  if (newestBlocked && !askedRow) showReason(newestBlocked);
  syncEmptyStates();
}

function showApp() {
  $("login").hidden = true;
  $("app").hidden = false;
}

let started = false;

async function start() {
  await refreshStatus();
  showApp();
  await Promise.allSettled([
    loadRecent(),
    refreshTop(),
    refreshHistory(),
    refreshFlagged(),
  ]);
  if (started) return;
  started = true;

  // Ein Zuhörer für die ganze Tabelle statt zwei je Zeile.
  const log = $("log");
  log.addEventListener("click", onRowActivate);
  log.addEventListener("keydown", onRowActivate);

  connectStream();
  setInterval(tickSpark, 1000);
  setInterval(async () => {
    if (document.hidden) return;
    try {
      await refreshHistory();
    } catch {
      /* die Kurve bleibt eben stehen; der Status meldet den Ausfall */
    }
  }, HISTORY_MS);
  setInterval(async () => {
    // Im Hintergrund fragt die Seite nichts ab: sie soll nebenher offen sein
    // dürfen, ohne dafür Rechenzeit zu verlangen.
    if (document.hidden) return;
    try {
      await refreshStatus();
      await refreshTop();
    } catch {
      setReachable(false);
    }
  }, POLL_MS);

  // Beim Zurückkommen einmal nachziehen, statt bis zum nächsten Takt einen
  // veralteten Stand zu zeigen.
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) return;
    drawSpark();
    refreshStatus().catch(() => setReachable(false));
  });
}

// Der Modus steht schon am Wurzelelement — das Skript im Kopf der Seite hat ihn
// vor dem ersten Zeichnen gesetzt. Hier wird nur noch umgeschaltet.
//
// Die Beschriftung nennt die Handlung und nicht den Zustand: "Switch to dark"
// heißt, dass ein Klick dorthin führt. Wer die Sonne sieht, weiß so auch ohne
// die Farben, in welchem Modus er ist.
function setTheme(mode) {
  document.documentElement.dataset.theme = mode;
  $("theme-toggle").setAttribute(
    "aria-label",
    mode === "dark" ? "Switch to light mode" : "Switch to dark mode",
  );
}

$("theme-toggle").addEventListener("click", () => {
  const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
  // Ohne Speicher (privates Fenster) gilt die Wahl nur für diese Seite.
  try {
    localStorage.setItem(THEME_KEY, next);
  } catch {
    /* dann eben nicht */
  }
  setTheme(next);
});

// Im Markup steht die helle Beschriftung, weil das Markup den Modus nicht
// kennt. Hier wird sie einmal an den tatsächlichen angeglichen.
setTheme(document.documentElement.dataset.theme || "light");

$("login").addEventListener("submit", async (submitEvent) => {
  submitEvent.preventDefault();
  token = $("token").value.trim();
  try {
    // start() prüft den Token, indem es den Status holt; erst danach wird
    // gespeichert und umgeschaltet.
    await start();
    localStorage.setItem(TOKEN_KEY, token);
  } catch {
    const error = $("login-error");
    error.textContent = "The token was not accepted.";
    error.hidden = false;
  }
});

start().catch(() => {
  $("login").hidden = false;
  $("token").focus();
});
