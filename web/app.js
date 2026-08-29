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

// ── Frage 1: Läuft er? ────────────────────────────────────────────────────

async function refreshStatus() {
  const status = await api("/api/status");
  setReachable(true);

  $("version").textContent = status.version;
  $("uptime").textContent = formatUptime(status.uptime_seconds);
  $("mode").textContent = status.logging_mode;

  $("queries").textContent = thousands(status.queries);
  $("blocked").textContent = `${thousands(status.blocked)} · ${percent(status.block_rate)}`;
  $("hitrate").textContent = percent(status.cache_hit_rate);
  $("entries").textContent = thousands(status.list_entries);

  const upstreams = $("upstreams");
  upstreams.replaceChildren();
  status.upstreams.forEach((upstream, index) => {
    if (index > 0) {
      const sep = document.createElement("span");
      sep.className = "sep";
      sep.textContent = "·";
      upstreams.append(sep);
    }
    const span = document.createElement("span");
    if (upstream.down) span.className = "down";
    const rtt = upstream.rtt_ms === null ? "–" : `${upstream.rtt_ms.toFixed(0)} ms`;
    span.textContent = upstream.down
      ? `${upstream.name} ausgefallen`
      : `${upstream.name} ${rtt}`;
    upstreams.append(span);
  });

  // Der aktive Log-Modus steht dauerhaft auf der Seite, nicht in einem
  // Einstellungsdialog (ADR-0004). Er bestimmt, was das Protokoll zeigen kann.
  const quiet = status.logging_mode === "none" || status.logging_mode === "aggregate";
  namesAvailable = !quiet;
  const note = $("live-note");
  if (quiet) {
    note.textContent =
      `Modus '${status.logging_mode}': der Server speichert keine Namen. ` +
      `Sichtbar ist, dass etwas passiert — nicht was.`;
    note.hidden = false;
  } else {
    note.hidden = true;
  }
  $("top-note").hidden = !quiet;
  $("top-note").textContent =
    `Nur Namen ab ${status.logging_mode === "none" ? "—" : "der k-Schwelle"}.`;
  if (status.logging_mode === "none") {
    $("top-note").textContent = "Im Modus 'none' werden keine Namen gezählt.";
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
}

// ── Frage 2: Was gerade passiert ──────────────────────────────────────────

function row(event) {
  const tr = document.createElement("tr");
  if (event.blocked) tr.className = "is-blocked";
  tr.append(cell(clockTime(event.at), "c-time"));

  const name = cell(event.name ?? "ohne Namen", "c-name");
  if (!event.name) name.classList.add("unnamed");
  tr.append(name);

  tr.append(cell(event.client ?? "–", "c-client"));
  tr.append(cell(event.rcode, "c-rcode"));
  tr.append(cell(event.ms.toFixed(1), "c-ms"));
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
