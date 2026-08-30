// AlpenDNS — Web-UI.
//
// Kein Framework, kein Build-Schritt, keine externe Ressource: die Seite liegt
// im Binary und funktioniert ohne Internet (CLAUDE.md B.6). Der Token wird im
// Browser gehalten und bei jeder Anfrage mitgeschickt; der Server kennt keine
// Sitzungen.
//
// Benutzt werden genau die Endpunkte, die es gibt: /api/status für den Rahmen,
// /api/events und /api/recent für das Protokoll, /api/top für die Nebenspalte.
"use strict";

const TOKEN_KEY = "alpendns.token";
/** So viele Zeilen hält das Protokoll. Darüber fällt die älteste heraus. */
const MAX_ROWS = 300;
const POLL_MS = 5000;
/** Breite der Sparkline in Sekunden. */
const SPARK_SECONDS = 60;
/** Ab hier gilt eine Antwort als schnell bzw. als langsam (Millisekunden). */
const MS_FAST = 20;
const MS_SLOW = 100;

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

const thousands = (value) => value.toLocaleString("de-AT");

/** Die Uhrzeit aus einem Zeitstempel.
 *
 *  /api/recent liefert den vollen ISO-Stempel, der Live-Strom nur die Uhrzeit.
 *  Im Protokoll steht beides untereinander, also wird hier vereinheitlicht —
 *  ein Datum in jeder Zeile wäre in einem Live-Protokoll ohnehin Ballast. */
function clockTime(at) {
  return at.includes("T") ? at.slice(11, 19) : at;
}
const percent = (value) => `${(value * 100).toFixed(1)} %`;

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
  if (d > 0) return `${d} d ${h} h`;
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

// ── Die Sparkline ─────────────────────────────────────────────────────────
//
// Sie zählt die Anfragen des Live-Stroms je Sekunde — dieselbe Quelle wie das
// Protokoll, kein zusätzlicher Endpunkt. Der Puffer wächst von einer Sekunde
// auf sechzig und schiebt erst dann: so füllt sich die Linie von links, statt
// eine Minute Nullen zu behaupten, die niemand beobachtet hat.

const spark = [0];

function drawSpark() {
  const path = $("spark");
  if (spark.length < 2) {
    path.setAttribute("d", "");
    return;
  }
  // Der Maßstab folgt der Spitze; mindestens 1, damit eine ruhige Minute flach
  // am Boden liegt statt durch Rundungsrauschen zu zappeln.
  const peak = Math.max(1, ...spark);
  const step = 240 / (SPARK_SECONDS - 1);
  let d = "";
  spark.forEach((value, index) => {
    const x = (index * step).toFixed(1);
    const y = (39 - (value / peak) * 38).toFixed(1);
    d += `${index === 0 ? "M" : " L"}${x} ${y}`;
  });
  path.setAttribute("d", d);
}

function tickSpark() {
  spark.push(0);
  if (spark.length > SPARK_SECONDS) spark.shift();
  drawSpark();
}

// ── Frage 1: Läuft er? ────────────────────────────────────────────────────

function renderUpstreams(list) {
  const box = $("upstreams");
  box.replaceChildren();
  for (const upstream of list) {
    const item = document.createElement("li");

    const dot = document.createElement("span");
    dot.className = `up-dot ${upstream.down ? "is-down" : "is-up"}`;

    const name = document.createElement("span");
    name.className = "up-name";
    name.textContent = upstream.name;

    const rtt = document.createElement("span");
    rtt.className = "up-rtt";
    if (upstream.down) {
      rtt.textContent = "ausgefallen";
    } else if (upstream.rtt_ms === null) {
      rtt.textContent = "–";
    } else {
      rtt.textContent = `${upstream.rtt_ms.toFixed(0)} ms`;
      const slot = latencyClass(upstream.rtt_ms);
      if (slot) rtt.classList.add(slot);
    }

    item.append(dot, name, rtt);
    box.append(item);
  }
}

async function refreshStatus() {
  const status = await api("/api/status");
  setReachable(true);

  $("version").textContent = status.version;
  $("uptime").textContent = formatUptime(status.uptime_seconds);
  $("mode").textContent = status.logging_mode;

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

  // Der aktive Log-Modus steht dauerhaft auf der Seite, nicht in einem
  // Einstellungsdialog (ADR-0004). Er bestimmt, was das Protokoll zeigen kann.
  const quiet = status.logging_mode === "none" || status.logging_mode === "aggregate";
  namesAvailable = !quiet;
  const note = $("live-note");
  if (quiet) {
    note.textContent =
      `Modus '${status.logging_mode}': keine Namen. ` +
      `Sichtbar ist, dass etwas passiert — nicht was.`;
    note.hidden = false;
  } else {
    note.hidden = true;
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

  $("why-empty").hidden = true;
  box.hidden = false;
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

  const ms = cell(event.ms.toFixed(1), "c-ms");
  const slot = latencyClass(event.ms);
  if (slot) ms.classList.add(slot);
  tr.append(ms);
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
  const top = await api("/api/top?limit=12");
  const list = $("top");
  list.replaceChildren();
  for (const entry of top) {
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
  await Promise.allSettled([loadRecent(), refreshTop()]);
  connectStream();
  setInterval(tickSpark, 1000);
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
