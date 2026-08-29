// AlpenDNS — Web-UI.
//
// Kein Framework, kein Build-Schritt, keine externe Ressource: die Seite liegt
// im Binary und funktioniert ohne Internet (CLAUDE.md B.6). Der Token wird im
// Browser gehalten und bei jeder Anfrage mitgeschickt; der Server kennt keine
// Sitzungen.
"use strict";

const TOKEN_KEY = "alpendns.token";
const MAX_ROWS = 60;

let token = localStorage.getItem(TOKEN_KEY) || "";
let stream = null;

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
  if (response.status === 401) throw new Error("unauthorized");
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
}

function formatUptime(seconds) {
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (d > 0) return `seit ${d} d ${h} h`;
  if (h > 0) return `seit ${h} h ${m} min`;
  return `seit ${m} min`;
}

const percent = (value) => `${(value * 100).toFixed(1)} %`;
const thousands = (value) => value.toLocaleString("de-AT");

/** Zeigt am Punkt neben der Laufzeit, ob der Server gerade antwortet. */
function setReachable(reachable) {
  const dot = document.querySelector(".live-dot");
  if (dot) dot.classList.toggle("is-stale", !reachable);
  if (dot) {
    dot.title = reachable
      ? "Der Server antwortet"
      : "Keine Antwort — die Zahlen sind der letzte bekannte Stand";
  }
}

/** Frage 1: Läuft er? */
async function refreshStatus() {
  const status = await api("/api/status");
  setReachable(true);

  $("version").textContent = `Version ${status.version}`;
  $("uptime").textContent = formatUptime(status.uptime_seconds);
  $("mode").textContent = `Logging: ${status.logging_mode}`;

  $("queries").textContent = thousands(status.queries);
  $("blocked").textContent =
    `${thousands(status.blocked)} · ${percent(status.block_rate)}`;
  $("hitrate").textContent = percent(status.cache_hit_rate);
  $("entries").textContent = thousands(status.list_entries);

  const body = $("upstreams");
  body.replaceChildren();
  for (const upstream of status.upstreams) {
    const row = document.createElement("tr");
    row.append(
      cell(upstream.name),
      cell(upstream.rtt_ms === null ? "–" : `${upstream.rtt_ms.toFixed(0)} ms`, "num"),
      cell(thousands(upstream.ok), "num"),
      cell(thousands(upstream.failed), "num"),
      cell(upstream.down ? "ausgefallen" : "erreichbar"),
    );
    if (upstream.down) row.className = "is-blocked";
    body.append(row);
  }

  // Der Log-Modus bestimmt, was der Live-Strom überhaupt zeigen kann. Das
  // gehört sichtbar auf die Seite und nicht in einen Einstellungsdialog
  // (ADR-0004).
  const note = $("live-note");
  if (status.logging_mode === "none" || status.logging_mode === "aggregate") {
    note.textContent =
      `Im Modus '${status.logging_mode}' werden keine Namen gespeichert. ` +
      `Der Strom zeigt, dass etwas passiert, aber nicht was. ` +
      `Für Namen braucht es 'ring' (nur im Arbeitsspeicher) oder 'full' (auf Platte).`;
    note.hidden = false;
  } else {
    note.hidden = true;
  }
}

/** Frage 3: Warum wurde das geblockt? */
function showReason(event) {
  const box = $("why");
  box.replaceChildren();

  const heading = document.createElement("p");
  heading.append(document.createTextNode("Geblockt: "));
  const subject = document.createElement("span");
  subject.className = "subject";
  subject.textContent = event.name;
  heading.append(subject);
  if (event.client) {
    heading.append(document.createTextNode(` — angefragt von ${event.client}`));
  }
  box.append(heading);

  const when = document.createElement("p");
  when.className = "when";
  when.textContent = `um ${event.at}, beantwortet mit ${event.rcode}`;
  box.append(when);

  const list = document.createElement("ol");
  for (const step of event.why || []) {
    const item = document.createElement("li");
    item.textContent = step;
    list.append(item);
  }
  box.append(list);
}

/** Frage 2: Was gerade passiert. */
function addRow(event) {
  const body = $("live");
  const row = document.createElement("tr");
  if (event.blocked) row.className = "is-blocked";
  row.append(
    cell(event.at),
    cell(event.name || "(nicht gespeichert)", "name"),
    cell(event.client || "–"),
    cell(event.rcode, "rcode"),
    cell(event.ms.toFixed(1), "num"),
  );
  body.prepend(row);
  while (body.childElementCount > MAX_ROWS) body.lastElementChild.remove();

  if (event.blocked && event.name) showReason(event);
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

/** Füllt die Liste beim Laden, damit die Seite nicht leer beginnt. */
async function loadRecent() {
  const recent = await api("/api/recent?limit=40");
  for (const entry of recent.slice().reverse()) addRow(entry);
}

/** Blendet das Anmeldefenster aus und die Zahlen ein. */
function showApp() {
  $("login").hidden = true;
  $("app").hidden = false;
}

async function start() {
  await refreshStatus();
  showApp();
  await loadRecent().catch(() => {});
  connectStream();
  setInterval(
    () => refreshStatus().catch(() => setReachable(false)),
    5000,
  );
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
