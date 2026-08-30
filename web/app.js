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
"use strict";

const TOKEN_KEY = "alpendns.token";
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

const LOCALE = "de-AT";

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
    ? "Der Status ist abrufbar"
    : "Keine Antwort — die Zahlen sind der letzte bekannte Stand";
}

/** Leere Flächen sagen, warum sie leer sind. Die Tabelle weicht dabei ganz:
 *  ein Spaltenkopf ohne Zeilen fällt in sich zusammen und sieht kaputt aus. */
function syncEmptyStates() {
  const hasRows = $("log").childElementCount > 0;
  $("log-empty").hidden = hasRows;
  $("log-table").hidden = !hasRows;
  $("top-empty").hidden = $("top").childElementCount > 0;
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
  drawSpark();
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
    name.title = `${upstream.name} über ${upstream.transport.toUpperCase()}`;

    const rtt = document.createElement("span");
    rtt.className = "up-rtt";
    if (upstream.down) {
      rtt.textContent = "ausgefallen";
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

/** Der Privacy-Streifen: Modus, Schwelle, und wohin die Daten gehen. */
function renderPrivacy(status) {
  $("p-mode").textContent = `Modus ${status.logging_mode}`;

  if (status.logging_mode === "none") {
    $("p-k").textContent = "keine Namen";
  } else {
    $("p-k").textContent = `Namen ab ${thousands(status.aggregate_k)} Treffern`;
  }

  // Nicht behaupten, sondern nachsehen: im Modus full schreibt der Server
  // sehr wohl auf die Platte, und dann muss das hier stehen.
  const store = $("p-store");
  store.textContent = status.persists_to_disk
    ? "schreibt auf Platte"
    : "Daten nur im RAM";
  store.classList.toggle("is-persisting", status.persists_to_disk);
  $("privacy").title = status.persists_to_disk
    ? "Der Log-Modus 'full' schreibt jede Anfrage mit Namen in eine Datei."
    : "Nichts von dem, was hier zu sehen ist, überlebt einen Neustart.";
}

/** Die Verteilung der Block-Gründe als ein Balken. */
function renderReasons(reasons) {
  const LABELS = {
    blocklist: "Blockliste",
    regex: "Regex-Regel",
    schedule: "Zeitplan",
    other: "ohne Zuordnung",
  };
  const total = reasons.reduce((sum, entry) => sum + entry.count, 0);
  const bar = $("reason-bar");
  const legend = $("reason-legend");
  bar.replaceChildren();
  legend.replaceChildren();

  bar.hidden = total === 0;
  $("reason-empty").hidden = total !== 0;
  if (total === 0) return;

  reasons.forEach((entry, index) => {
    if (entry.count === 0) return;
    const band = `band-${Math.min(index + 1, SPLIT_BANDS)}`;

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
  renderPrivacy(status);
  renderReasons(status.block_reasons ?? []);

  $("pc-ecs").textContent = thousands(status.privacy.ecs_stripped);
  $("pc-pad").textContent = thousands(status.privacy.padded);
  $("pc-case").textContent = thousands(status.privacy.case_randomized);
  $("pc-cookie").textContent = thousands(status.privacy.cookies);

  // Der aktive Log-Modus bestimmt, was das Protokoll überhaupt zeigen kann.
  const quiet = status.logging_mode === "none" || status.logging_mode === "aggregate";
  namesAvailable = !quiet;
  const note = $("live-note");
  if (quiet) {
    note.textContent = `flüchtig · Modus '${status.logging_mode}': sichtbar ist, dass etwas passiert — nicht was`;
  } else if (status.persists_to_disk) {
    note.textContent = "Modus 'full' · jede Zeile geht zusätzlich in eine Datei";
  } else {
    note.textContent = "flüchtig · nichts davon wird gespeichert";
  }

  if (status.logging_mode === "none") {
    $("top-note").textContent = "Im Modus 'none' werden keine Namen gezählt.";
  } else if (quiet) {
    $("top-note").textContent = "Namen erscheinen erst ab der k-Schwelle.";
  } else {
    $("top-note").textContent = "Noch kein Name oft genug gesehen.";
  }
}

// ── Frage 3: Warum wurde das geblockt? ────────────────────────────────────

function showReason(event) {
  const box = $("why-body");
  box.replaceChildren();

  const subject = document.createElement("span");
  subject.className = "subject";
  subject.textContent = event.name;
  box.append(subject);

  const context = document.createElement("p");
  context.className = "context";
  context.textContent =
    `um ${clockTime(event.at)} · ${event.client ?? "unbekannt"} · beantwortet mit ${event.rcode}`;
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
 *  jetzt passieren?". Deshalb steht dabei, woher sie kommt. */
async function explainRow(name, client, row) {
  for (const marked of document.querySelectorAll("tr.is-asked")) {
    marked.classList.remove("is-asked");
  }
  row.classList.add("is-asked");

  const query = new URLSearchParams({ domain: name });
  if (client && client !== "–" && client !== "unbekannt") query.set("client", client);

  const source = $("why-source");
  try {
    const answer = await api(`/api/explain?${query}`);
    showReason({
      name: answer.domain,
      at: new Date().toTimeString().slice(0, 8),
      client: answer.client,
      rcode: answer.blocked ? "geblockt" : "durchgelassen",
      why: answer.steps,
    });
    source.textContent = "jetzt ausgewertet, nicht aus dem Protokoll";
    source.hidden = false;
  } catch {
    source.textContent = "Die Auswertung war nicht möglich.";
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

  const name = cell(event.name ?? "ohne Namen", "c-name");
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
  if (event.name) {
    tr.classList.add("askable");
    tr.tabIndex = 0;
    tr.title = "Entscheidungskette abfragen";
    const ask = () => explainRow(event.name, event.client, tr);
    tr.addEventListener("click", ask);
    tr.addEventListener("keydown", (key) => {
      if (key.key === "Enter" || key.key === " ") {
        key.preventDefault();
        ask();
      }
    });
  }
  return tr;
}

function addRow(event) {
  const body = $("log");
  const scroller = $("log-scroll");
  const wasAtTop = scroller.scrollTop < 4;

  const tr = row(event);
  body.prepend(tr);
  while (body.childElementCount > MAX_ROWS) body.lastElementChild.remove();

  // Wer im Protokoll nach unten gescrollt hat, soll nicht mitgeschoben werden.
  if (!wasAtTop) scroller.scrollTop += tr.offsetHeight;

  spark[spark.length - 1] += 1;
  syncEmptyStates();
  if (event.blocked && event.name) showReason(event);
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
      `${thousands(report.below_threshold_queries)} Anfragen auf ` +
      `${thousands(report.below_threshold_names)} Namen unter der k-Schwelle`;
    hidden.hidden = false;
  } else {
    hidden.hidden = true;
  }
  syncEmptyStates();
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
  const body = $("log");
  // Rückwärts anhängen ist billiger als 80-mal voranzustellen.
  for (const event of recent.slice().reverse()) body.prepend(row(event));
  const newestBlocked = recent.find((event) => event.blocked && event.name);
  if (newestBlocked) showReason(newestBlocked);
  syncEmptyStates();
}

function showApp() {
  $("login").hidden = true;
  $("app").hidden = false;
}

async function start() {
  await refreshStatus();
  showApp();
  await Promise.allSettled([loadRecent(), refreshTop(), refreshHistory()]);
  connectStream();
  setInterval(tickSpark, 1000);
  setInterval(async () => {
    try {
      await refreshHistory();
    } catch {
      /* die Kurve bleibt eben stehen; der Status meldet den Ausfall */
    }
  }, HISTORY_MS);
  setInterval(async () => {
    try {
      await refreshStatus();
      await refreshTop();
    } catch {
      setReachable(false);
    }
  }, POLL_MS);
}

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
    error.textContent = "Der Token wurde nicht akzeptiert.";
    error.hidden = false;
  }
});

start().catch(() => {
  $("login").hidden = false;
  $("token").focus();
});
