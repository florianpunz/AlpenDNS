"use strict";

const TOKEN_KEY = "alpendns.token";
const THEME_KEY = "alpendns.theme";
const POLL_MS = 5000;
const HISTORY_MS = 60000;
const MAX_ROWS = 200;
const SPARK_SECONDS = 60;
const MS_WARN = 250;
const ERROR_RCODES = new Set(["SERVFAIL", "REFUSED", "FORMERR", "NOTIMP"]);

let token = localStorage.getItem(TOKEN_KEY) || "";
let stream = null;
let status = null;
let history = null;
let shares = [];
let listInfos = [];
let filter = "all";
let searchTerm = "";
let quietMode = "";
let namesAvailable = true;
let started = false;
let selectedEntry = null;
let inspectSeq = 0;
let lastTrace = "";
let lastSubject = "";

const $ = (id) => document.getElementById(id);

const entries = [];
const pending = [];
let flushScheduled = false;

const series = { queries: [], blocked: [], cache: [], latency: [] };
let bucket = { queries: 0, blocked: 0, cache: 0, ms: 0 };

function td(className) {
  const cell = document.createElement("td");
  cell.className = className;
  return cell;
}

function span(className) {
  const node = document.createElement("span");
  node.className = className;
  return node;
}

async function api(path) {
  const response = await fetch(path, {
    headers: { Authorization: `Bearer ${token}` },
  });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  return response.json();
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

const LOCALE = "en-US";

const TRANSPORTS = { dot: "DoT", doh: "DoH", doq: "DoQ" };

const transportLabel = (value) => TRANSPORTS[value] ?? String(value).toUpperCase();

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

function msText(value) {
  if (value === null || value === undefined) return "–";
  return value >= 100 ? decimal(value, 0) : decimal(value, 1);
}

function clockTime(at) {
  return at.includes("T") ? at.slice(11, 19) : at;
}

const RELATIVE = new Intl.RelativeTimeFormat(LOCALE, { numeric: "auto" });

function relativeTime(iso, now = Date.now()) {
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return null;
  const seconds = (then - now) / 1000;
  const units = [
    ["year", 31_536_000],
    ["month", 2_592_000],
    ["week", 604_800],
    ["day", 86_400],
    ["hour", 3_600],
    ["minute", 60],
  ];
  for (const [unit, size] of units) {
    if (Math.abs(seconds) >= size) {
      return RELATIVE.format(Math.round(seconds / size), unit);
    }
  }
  return RELATIVE.format(Math.round(seconds), "second");
}

function formatUptime(seconds) {
  const d = Math.floor(seconds / 86400);
  const h = Math.floor((seconds % 86400) / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  if (d > 0) return `${thousands(d)} d ${h} h`;
  if (h > 0) return `${h} h ${m} min`;
  return `${m} min`;
}

function linePath(values, peak, step, base, span) {
  if (values.length === 0) return "";
  const top = base - span;
  const clamp = (y) => Math.min(base, Math.max(top, y));
  const points = values.map((value, index) => [
    index * step,
    base - (value / peak) * span,
  ]);
  if (points.length === 1) {
    return `M${points[0][0].toFixed(1)} ${points[0][1].toFixed(1)}`;
  }

  const segments = [`M${points[0][0].toFixed(1)} ${points[0][1].toFixed(1)}`];
  const last = points.length - 1;
  for (let i = 0; i < last; i += 1) {
    const before = points[i === 0 ? 0 : i - 1];
    const from = points[i];
    const to = points[i + 1];
    const after = points[i + 2 > last ? last : i + 2];
    const c1x = from[0] + (to[0] - before[0]) / 6;
    const c1y = clamp(from[1] + (to[1] - before[1]) / 6);
    const c2x = to[0] - (after[0] - from[0]) / 6;
    const c2y = clamp(to[1] - (after[1] - from[1]) / 6);
    segments.push(
      `C${c1x.toFixed(1)} ${c1y.toFixed(1)} ` +
        `${c2x.toFixed(1)} ${c2y.toFixed(1)} ` +
        `${to[0].toFixed(1)} ${to[1].toFixed(1)}`,
    );
  }
  return segments.join("");
}

function setPath(id, d) {
  $(id).setAttribute("d", d);
}

function pushSample(name, value) {
  const values = series[name];
  values.push(value);
  if (values.length > SPARK_SECONDS) values.shift();
}

function drawSpark(name, peak) {
  const values = series[name];
  if (values.length < 2) {
    setPath(`s-${name}`, "");
    return;
  }
  setPath(
    `s-${name}`,
    linePath(values, Math.max(1, peak), 240 / (SPARK_SECONDS - 1), 34, 28),
  );
}

function tickSparks() {
  if (document.hidden) return;
  const queries = bucket.queries;
  pushSample("queries", queries);
  pushSample("blocked", queries ? (bucket.blocked / queries) * 100 : 0);
  pushSample("cache", queries ? (bucket.cache / queries) * 100 : 0);
  pushSample("latency", queries ? bucket.ms / queries : 0);
  bucket = { queries: 0, blocked: 0, cache: 0, ms: 0 };

  drawSpark("queries", Math.max(...series.queries));
  drawSpark("blocked", 100);
  drawSpark("cache", 100);
  drawSpark("latency", Math.max(...series.latency));
}

function setReachable(reachable) {
  $("health-dot").classList.toggle("is-stale", !reachable);
  $("health-dot").classList.toggle("is-live", reachable);
  if (!reachable) {
    $("health").textContent = "Status unreachable";
    $("status-detail").textContent =
      "No response from the API — the numbers are the last known state.";
  }
}

function renderStatus(next) {
  status = next;
  quietMode = next.logging_mode === "ring" || next.logging_mode === "full"
    ? ""
    : next.logging_mode;
  namesAvailable = quietMode === "";

  const upstreams = next.upstreams ?? [];
  const reachable = upstreams.filter((upstream) => !upstream.down).length;
  const transports = new Set(upstreams.map((upstream) => upstream.transport));
  const protocol = transports.size === 1 ? ` over ${transportLabel([...transports][0])}` : "";
  const healthy = upstreams.length > 0 && reachable === upstreams.length;

  $("health").textContent = healthy ? "Resolving normally" : "Resolving with problems";
  $("health-dot").classList.toggle("is-live", healthy);
  $("health-dot").classList.toggle("is-stale", !healthy);

  $("status-detail").textContent = [
    upstreams.length
      ? `${reachable} of ${upstreams.length} upstreams reachable${protocol}`
      : "no upstream configured",
    `up ${formatUptime(next.uptime_seconds)}`,
    `version ${next.version}`,
    `logging mode ${next.logging_mode}`,
  ].join(" · ");

  $("m-queries").textContent = thousands(next.queries);
  $("m-blocked").textContent = percent(next.block_rate);
  $("m-blocked-sub").textContent = `${thousands(next.blocked)} queries`;
  $("m-cache").textContent = percent(next.cache_hit_rate);

  if (next.latency?.p50_ms !== null && next.latency?.p50_ms !== undefined) {
    $("m-latency").textContent = `${msText(next.latency.p50_ms)} ms`;
    $("m-latency-sub").textContent =
      `median · p95 ${msText(next.latency.p95_ms)} ms`;
  } else {
    $("m-latency").textContent = "–";
    $("m-latency-sub").textContent = "no measurement yet";
  }

  renderUpstreams(next);
  renderPrivacy(next);
  renderReasons(next.block_reasons ?? []);
  renderFooter(next);
  syncEmptyStates();
}

function renderUpstreams(next) {
  const box = $("upstreams");
  box.replaceChildren();

  const list = next.upstreams ?? [];
  list.forEach((upstream, index) => {
    const item = document.createElement("li");
    item.className = "upstream";

    const dot = span("up-dot");
    if (upstream.down) dot.classList.add("is-down");

    const name = span("up-name");
    name.textContent = upstream.name;
    const transport = span("up-transport");
    transport.textContent = transportLabel(upstream.transport);
    name.append(transport);

    const latency = span("up-latency");
    if (upstream.p50_ms === null || upstream.p50_ms === undefined) {
      latency.textContent = "–";
    } else {
      latency.textContent =
        `${msText(upstream.p50_ms)} / ${msText(upstream.p95_ms)} ms`;
      if (upstream.p95_ms !== null && upstream.p95_ms >= MS_WARN) {
        latency.classList.add("is-warn");
      }
    }

    const bar = span("up-bar");
    const fill = span("");
    const share = shares[index] ?? 0;
    fill.style.width = `${share.toFixed(1)}%`;
    bar.title = `${share.toFixed(1)} % of upstream answers in the last 24 hours`;
    bar.append(fill);

    item.append(dot, name, latency, bar);
    box.append(item);
  });

  const mix = new Map();
  for (const upstream of list) {
    mix.set(upstream.transport, (mix.get(upstream.transport) ?? 0) + 1);
  }
  $("up-note").textContent = [...mix]
    .map(([transport, count]) => `${count}× ${transportLabel(transport)}`)
    .join(" · ");
}

function setPrivacy(id, enabled, text) {
  const value = $(id);
  const row = value.closest(".privacy-row");
  row.classList.toggle("is-off", !enabled);
  value.textContent = enabled ? text : "Off";
}

function renderPrivacy(next) {
  const switches = next.privacy?.enabled ?? {};
  const counters = next.privacy ?? {};

  setPrivacy("pc-ecs", switches.ecs, `${thousands(counters.ecs_stripped)} seen`);
  setPrivacy("pc-pad", switches.padding, `${thousands(counters.padded)} padded`);
  setPrivacy(
    "pc-secure",
    next.dnssec.enabled,
    `${thousands(next.dnssec.secure)} valid`,
  );
  setPrivacy(
    "pc-bogus",
    next.dnssec.enabled,
    `${thousands(next.dnssec.bogus)} dropped`,
  );
  setPrivacy(
    "pc-case",
    switches.case_randomization,
    `${thousands(counters.case_randomized)} randomized`,
  );
  setPrivacy("pc-cookie", switches.cookies, `${thousands(counters.cookies)} sent`);

  $("bogus-review").hidden = !(next.dnssec.bogus > 0 && namesAvailable);
}

const REASON_LABELS = {
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

function renderReasons(reasons) {
  const total = reasons.reduce((sum, entry) => sum + entry.count, 0);
  const bar = $("reason-bar");
  const legend = $("reason-legend");
  bar.replaceChildren();
  legend.replaceChildren();

  bar.hidden = total === 0;
  $("reason-empty").hidden = total !== 0;
  if (total === 0) return;

  reasons
    .filter((entry) => entry.count > 0)
    .forEach((entry, index) => {
      const band = `band-${(index % 4) + 1}`;

      const segment = span(band);
      segment.style.width = `${(entry.count / total) * 100}%`;
      segment.title = `${REASON_LABELS[entry.reason] ?? entry.reason}: ${thousands(entry.count)}`;
      bar.append(segment);

      const item = document.createElement("li");
      const swatch = span(`swatch ${band}`);
      const name = span("");
      name.textContent = REASON_LABELS[entry.reason] ?? entry.reason;
      const count = span("count");
      count.textContent = thousands(entry.count);
      item.append(swatch, name, count);
      legend.append(item);
    });
}

function renderFooter(next) {
  const oldest = listInfos
    .map((info) => info.fetched_at)
    .filter(Boolean)
    .sort()[0];
  const entriesText = `${thousands(next.list_entries)} blocklist entries`;
  $("foot-lists").textContent = oldest
    ? `${entriesText} · oldest list updated ${relativeTime(oldest)}`
    : entriesText;

  $("foot-mode").textContent = next.persists_to_disk
    ? "Every query is also written to the log file (mode full)."
    : {
        ring: "Names are kept in memory only (mode ring).",
        aggregate: "Queries are counted without names (mode aggregate).",
        none: "Only counters are kept (mode none).",
      }[next.logging_mode] ?? `Logging mode ${next.logging_mode}.`;
  $("foot-version").textContent = `AlpenDNS ${next.version}`;
}

async function refreshStatus() {
  const next = await api("/api/status");
  setReachable(true);
  renderStatus(next);
  return next;
}

function computeShares(view) {
  const sums = (view.upstreams ?? []).map(() => 0);
  for (const item of view.buckets ?? []) {
    (item.upstreams ?? []).forEach((value, index) => {
      sums[index] = (sums[index] ?? 0) + value;
    });
  }
  const total = sums.reduce((sum, value) => sum + value, 0);
  shares = sums.map((value) => (total ? (value / total) * 100 : 0));
}

function drawDay(view) {
  const buckets = view.buckets ?? [];
  const ready = buckets.length >= 2;
  $("chart-traffic").hidden = !ready;
  $("day-empty").hidden = ready;
  if (!ready) {
    setPath("day-queries", "");
    setPath("day-blocked", "");
    return;
  }

  const step = 240 / (buckets.length - 1);
  const queries = buckets.map((item) => item.queries);
  const blocked = buckets.map((item) => item.blocked);
  const peak = Math.max(1, ...queries);

  setPath("day-queries", linePath(queries, peak, step, 54, 46));
  setPath("day-blocked", linePath(blocked, peak, step, 54, 46));
}

async function refreshHistory() {
  const view = await api("/api/history");
  history = view;
  computeShares(view);
  drawDay(view);
  if (status) renderUpstreams(status);
}

async function refreshLists() {
  listInfos = await api("/api/lists");
  if (status) renderFooter(status);
}

function isError(event) {
  return event.source === "failed" || ERROR_RCODES.has(String(event.rcode).toUpperCase());
}

function sameQuery(one, other) {
  return Boolean(
    one.name &&
      other.name &&
      one.name === other.name &&
      one.type === other.type &&
      (one.client ?? "") === (other.client ?? "") &&
      one.blocked === other.blocked &&
      one.rcode === other.rcode &&
      (one.source ?? "") === (other.source ?? ""),
  );
}

function matchesFilter(entry) {
  const event = entry.event;
  if (filter === "blocked" && !event.blocked) return false;
  if (filter === "errors" && !isError(event)) return false;
  if (searchTerm) {
    const haystack = `${event.name ?? ""} ${event.client ?? ""}`.toLowerCase();
    if (!haystack.includes(searchTerm)) return false;
  }
  return true;
}

function applyFilter(entry) {
  entry.tr.hidden = !matchesFilter(entry);
}

function paintResult(cell, event) {
  cell.replaceChildren();
  const result = span("result");
  if (event.blocked) {
    result.classList.add("is-blocked");
    result.textContent = "BLOCKED";
    result.title = event.rcode;
  } else if (isError(event)) {
    result.classList.add("is-error");
    result.textContent = String(event.rcode).toUpperCase();
  } else {
    result.classList.add("is-muted");
    result.textContent = String(event.rcode).toUpperCase();
  }
  cell.append(result);
}

function paintMs(cell, event) {
  cell.classList.remove("is-warn");
  if (event.blocked) {
    cell.textContent = "";
    return;
  }
  if (event.source === "cache") {
    cell.textContent = "cache";
    return;
  }
  if (event.source === "local") {
    cell.textContent = "local";
    return;
  }
  if (event.source === "failed") {
    cell.textContent = "–";
    return;
  }
  cell.textContent = msText(event.ms);
  if (event.ms >= MS_WARN) cell.classList.add("is-warn");
}

function paintName(entry) {
  const event = entry.event;
  const cell = entry.cells.name;

  if (event.name) {
    if (!entry.button) {
      entry.button = document.createElement("button");
      entry.button.type = "button";
      entry.button.className = "name-button";
      entry.button.title = "Why this answer";
      cell.append(entry.button);
      entry.times = span("times");
      cell.append(entry.times);
    }
    entry.button.textContent = event.name;
    entry.button.dataset.domain = event.name;
    if (event.client) entry.button.dataset.client = event.client;
    if (entry.times) {
      entry.times.textContent = entry.count > 1 ? `×${entry.count}` : "";
    }
  } else {
    cell.replaceChildren();
    const empty = span("name-static");
    empty.textContent = "–";
    cell.append(empty);
  }
}

function buildEntry(event) {
  const tr = document.createElement("tr");
  const cells = {
    time: td("c-time"),
    name: td("c-name"),
    type: td("c-type"),
    client: td("c-client"),
    result: td("c-result"),
    ms: td("c-ms"),
  };
  tr.append(...Object.values(cells));

  const entry = { event, tr, count: 1, cells, button: null, times: null };
  fillEntry(entry);
  tr.dataset.domain = event.name ?? "";
  if (event.client) tr.dataset.client = event.client;
  return entry;
}

function fillEntry(entry) {
  const event = entry.event;
  entry.tr.classList.toggle("is-blocked", event.blocked);
  entry.cells.time.textContent = clockTime(event.at);
  entry.cells.type.textContent = event.type ?? "–";
  entry.cells.client.textContent = event.client ?? "–";
  paintName(entry);
  paintResult(entry.cells.result, event);
  paintMs(entry.cells.ms, event);
}

function pushEvent(event) {
  const top = entries[0];
  if (top && sameQuery(top.event, event)) {
    top.event = event;
    top.count += 1;
    fillEntry(top);
    applyFilter(top);
    return;
  }

  if (
    !event.name &&
    top &&
    !top.event.name &&
    top.event.type === event.type &&
    top.event.blocked === event.blocked &&
    top.event.rcode === event.rcode &&
    (top.event.source ?? "") === (event.source ?? "")
  ) {
    top.event = event;
    top.count += 1;
    fillEntry(top);
    applyFilter(top);
    return;
  }

  const entry = buildEntry(event);
  applyFilter(entry);
  entries.unshift(entry);
  $("log").prepend(entry.tr);

  while (entries.length > MAX_ROWS) {
    const removed = entries.pop();
    removed.tr.remove();
  }
}

function flushRows() {
  flushScheduled = false;
  if (pending.length === 0) return;
  for (const event of pending) pushEvent(event);
  pending.length = 0;
  syncEmptyStates();
}

function addRow(event) {
  bucket.queries += 1 + (event.skipped ?? 0);
  if (event.blocked) bucket.blocked += 1;
  if (event.source === "cache") bucket.cache += 1;
  bucket.ms += Number(event.ms ?? 0);

  if (document.hidden) return;
  pending.push(event);
  if (pending.length > MAX_ROWS) pending.splice(0, pending.length - MAX_ROWS);
  if (!flushScheduled) {
    flushScheduled = true;
    requestAnimationFrame(flushRows);
  }
}

function syncEmptyStates() {
  const total = entries.length;
  const visible = entries.filter((entry) => !entry.tr.hidden).length;

  $("log-table").hidden = total === 0;
  $("log-empty").hidden = visible > 0;

  if (total === 0) {
    $("log-empty-note").textContent = quietMode
      ? `Mode '${quietMode}': the server keeps no names, so nothing can appear here.`
      : "No query answered yet. New ones appear on their own.";
  } else {
    $("log-empty-note").textContent = "No query matches this filter.";
  }

  $("log-foot").textContent = quietMode
    ? `Mode '${quietMode}': names are not retained. Set privacy.logging.mode to 'ring' or 'full' to see domains.`
    : "Select a domain to see why it was answered that way.";

  $("flagged-empty").hidden = $("flagged").childElementCount > 0;
}

function setFilter(next) {
  filter = next;
  for (const chip of document.querySelectorAll(".chip")) {
    const active = chip.dataset.filter === next;
    chip.classList.toggle("is-active", active);
    chip.setAttribute("aria-pressed", String(active));
  }
  for (const entry of entries) applyFilter(entry);
  syncEmptyStates();
}

const BLOCK_MODES = { Nxdomain: "NXDOMAIN", ZeroIp: "0.0.0.0", Sinkhole: "sinkhole" };

const STEP_FORMS = [
  [/^no client entry matches, '(.+)' applies$/, "Client",
    (m) => `${m[1]} · no rule matched`],
  [/^Client '(.+)' recognized by source address$/, "Client",
    (m) => `${m[1]} · source address`],
  [/^Policy '(.+)'$/, "Policy", (m) => m[1]],
  [/^temporary (allow|deny), (.+) left$/, "Temporary",
    (m) => `${m[1]} · ${m[2]} left`],
  [/^Allowlist '(.+)' line (\d+): '(.+)'$/, "Allowlist",
    (m) => `${m[1]} · line ${m[2]} · ${m[3]}`],
  [/^Blocklist '(.+)' line (\d+): '(.+)'$/, "Blocklist",
    (m) => `${m[1]} · line ${m[2]} · ${m[3]}`, true],
  [/^Regex rule from policy '(.+)': \/(.+)\/$/, "Regex",
    (m) => `${m[1]} · /${m[2]}/`],
  [/^Schedule '(.+)' active: .+$/, "Schedule",
    (m) => `${m[1]} · allowlist only`],
  [/^from cache \((valid|expired), (\d+) s left\)$/, "Answer",
    (m) => `cache · ${m[1]} · ${m[2]} s left`],
  [/^Upstream '(.+)' answered in (.+)$/, "Answer",
    (m) => `${m[1]} · ${m[2]}`],
  [/^DNSSEC verified locally: (.+)$/, "DNSSEC", (m) => m[1]],
  [/^(.+) (blocks|flags|logs) \(score (.+)\): .*$/, "Detection",
    (m) => `${m[1]} · ${m[2]} · ${m[3]}`],
  [/^Answer synthesized locally, mode (.+)$/, "Answer",
    (m) => `synthesized · ${BLOCK_MODES[m[1]] ?? m[1]}`, true],
  [/^from local zone '(.+)', (\d+) record\(s\)$/, "Answer",
    (m) => `local zone ${m[1]} · ${m[2]} record(s)`],
  [/^the same question failed moments ago; .+$/, "Answer",
    () => "not asked · failed moments ago"],
];

function describeStep(step) {
  for (const [pattern, label, detail, key] of STEP_FORMS) {
    const match = pattern.exec(step);
    if (match) return { label, detail: detail(match), key: Boolean(key) };
  }
  return { label: "Step", detail: step, key: false };
}

function renderInspector(answer, record) {
  const subject = $("inspect-name");
  subject.textContent = answer.domain;
  subject.classList.toggle("is-allowed", !answer.blocked);

  const meta = [];
  if (record) {
    meta.push(record.type ?? "–", clockTime(record.at), `client ${record.client ?? "unknown"}`);
  } else {
    meta.push("evaluated just now", `client ${answer.client}`);
  }
  $("inspect-meta").textContent = meta.join(" · ");

  const badge = $("inspect-verdict");
  badge.hidden = false;
  badge.textContent = answer.blocked ? "Blocked" : "Allowed";
  badge.classList.toggle("is-allowed", !answer.blocked);

  const list = $("inspect-steps");
  list.replaceChildren();
  for (const step of answer.steps ?? []) {
    const { label, detail, key } = describeStep(step);
    const item = document.createElement("li");
    if (key) item.classList.add("is-key");
    const station = span("step-label");
    station.textContent = label;
    const outcome = span("step-detail");
    outcome.textContent = detail;
    item.append(station, outcome);
    list.append(item);
  }
  if (list.childElementCount === 0) {
    const item = document.createElement("li");
    const outcome = span("step-detail");
    outcome.textContent = "No step was recorded.";
    item.append(outcome);
    list.append(item);
  }

  lastTrace = (answer.steps ?? []).join("\n");
  lastSubject = answer.domain;
  $("inspect-note").textContent =
    "Evaluated now against the current lists, not replayed from the log.";
  $("inspect-allow").hidden = !answer.blocked;
  $("inspect-allow").textContent = "Allow this domain";
  $("inspect-allow").disabled = false;
}

async function openInspector({ name, client, entry, record }) {
  if (!name) return;
  if (selectedEntry && selectedEntry !== entry) {
    selectedEntry.tr.classList.remove("is-selected");
  }
  selectedEntry = entry ?? null;
  if (entry) entry.tr.classList.add("is-selected");

  const sequence = ++inspectSeq;
  $("inspect-name").textContent = name;
  $("inspect-name").classList.remove("is-allowed");
  $("inspect-meta").textContent = record
    ? [record.type ?? "–", clockTime(record.at), `client ${record.client ?? "unknown"}`].join(" · ")
    : "evaluated just now";
  $("inspect-verdict").hidden = true;
  $("inspect-steps").replaceChildren();
  $("inspect-note").textContent = "Evaluating against the current lists…";
  $("inspect-allow").hidden = true;
  $("inspect-copy").textContent = "Copy trace";
  lastTrace = "";
  lastSubject = name;

  const dialog = $("inspector");
  if (!dialog.open) dialog.showModal();

  const query = new URLSearchParams({ domain: name });
  if (client && client !== "–" && client !== "unknown" && client !== "default") {
    query.set("client", client);
  }

  try {
    const answer = await api(`/api/explain?${query}`);
    if (sequence !== inspectSeq) return;
    renderInspector(answer, record);
  } catch {
    if (sequence !== inspectSeq) return;
    $("inspect-note").textContent = "Evaluation was not possible.";
  }
}

function loadRecent(recent) {
  for (let index = recent.length - 1; index >= 0; index -= 1) {
    pushEvent(recent[index]);
  }
  syncEmptyStates();
}

async function refreshFlagged() {
  const found = await api("/api/flagged?limit=25");
  const list = $("flagged");
  list.replaceChildren();

  for (const entry of found) {
    for (const finding of entry.findings ?? []) {
      list.append(flaggedRow(entry, finding));
    }
  }

  $("flagged-note").textContent = found.length
    ? `${thousands(found.length)} in the last few minutes`
    : "";
  $("flagged-note").hidden = found.length === 0;
  $("flagged-note-empty").textContent = quietMode
    ? `mode '${quietMode}': the server keeps no names, so nothing can appear here.`
    : "No heuristic has flagged anything.";
  syncEmptyStates();
}

function flaggedRow(entry, finding) {
  const item = document.createElement("li");

  const subject = span("subject");
  subject.textContent = entry.name;

  const meta = document.createElement("p");
  meta.className = "meta";
  const label = span("");
  label.textContent = finding.label;
  const score = span("score");
  score.textContent = finding.score;
  const action = span("");
  action.textContent = finding.action === "block" ? "blocked" : "flagged";
  meta.append(label, score, action);

  const why = document.createElement("p");
  why.className = "why";
  why.textContent = finding.reason;

  const actions = document.createElement("div");
  actions.className = "actions";
  actions.append(
    decideButton("Allow", "/api/allow", entry.name),
    decideButton("Deny", "/api/deny", entry.name),
  );

  item.append(subject, meta, why, actions);
  return item;
}

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

function connectStream() {
  if (stream) stream.abort();
  const controller = new AbortController();
  stream = controller;
  void pump(controller.signal);
}

function setStreamState(live) {
  $("stream-state").textContent = live ? "updates live" : "reconnecting";
  $("stream-flag").classList.toggle("is-off", !live);
  $("stream-flag-text").textContent = live ? "streaming" : "reconnecting";
}

async function pump(signal) {
  while (!signal.aborted) {
    try {
      const response = await fetch("/api/events", {
        headers: { Authorization: `Bearer ${token}` },
        signal,
      });
      if (!response.ok) throw new Error(String(response.status));
      setStreamState(true);
      await readEvents(response.body, signal);
    } catch {
      if (signal.aborted) return;
    }
    setStreamState(false);
    await new Promise((resolve) => setTimeout(resolve, 3000));
  }
}

async function readEvents(body, signal) {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    if (signal.aborted) {
      await reader.cancel().catch(() => {});
      return;
    }
    const { value, done } = await reader.read();
    if (done) return;
    buffer += decoder.decode(value, { stream: true });
    let end;
    while ((end = buffer.indexOf("\n\n")) >= 0) {
      const block = buffer.slice(0, end);
      buffer = buffer.slice(end + 2);
      const data = block
        .split("\n")
        .filter((line) => line.startsWith("data:"))
        .map((line) => line.slice(5).replace(/^ /, ""))
        .join("\n");
      if (!data) continue;
      try {
        addRow(JSON.parse(data));
      } catch {}
    }
  }
}

function openCommand() {
  const dialog = $("command");
  if (dialog.open) return;
  $("command-input").value = "";
  dialog.showModal();
  $("command-input").focus();
}

function isTyping(target) {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable ||
      target.tagName === "INPUT" ||
      target.tagName === "TEXTAREA" ||
      target.tagName === "SELECT")
  );
}

function showApp() {
  $("login").hidden = true;
  $("app").hidden = false;
}

async function start() {
  await refreshStatus();
  showApp();

  await Promise.allSettled([
    api("/api/recent?limit=80").then(loadRecent),
    refreshHistory(),
    refreshFlagged(),
    refreshLists(),
  ]);

  if (started) return;
  started = true;

  connectStream();
  setInterval(tickSparks, 1000);
  setInterval(() => {
    if (document.hidden) return;
    refreshHistory().catch(() => {});
  }, HISTORY_MS);
  setInterval(() => {
    if (document.hidden) return;
    refreshLists().catch(() => {});
  }, HISTORY_MS);
  setInterval(async () => {
    if (document.hidden) return;
    try {
      await refreshStatus();
      await refreshFlagged();
    } catch {
      setReachable(false);
    }
  }, POLL_MS);

  document.addEventListener("visibilitychange", () => {
    if (document.hidden) return;
    bucket = { queries: 0, blocked: 0, cache: 0, ms: 0 };
    refreshStatus().catch(() => setReachable(false));
    refreshHistory().catch(() => {});
  });
}

function setTheme(mode) {
  document.documentElement.dataset.theme = mode;
  $("theme-toggle").setAttribute(
    "aria-label",
    mode === "dark" ? "Switch to light mode" : "Switch to dark mode",
  );
}

$("theme-toggle").addEventListener("click", () => {
  const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
  try {
    localStorage.setItem(THEME_KEY, next);
  } catch {}
  setTheme(next);
});

setTheme(document.documentElement.dataset.theme || "light");

$("login").addEventListener("submit", async (submitEvent) => {
  submitEvent.preventDefault();
  token = $("token").value.trim();
  try {
    await start();
    localStorage.setItem(TOKEN_KEY, token);
  } catch {
    const error = $("login-error");
    error.textContent = "The token was not accepted.";
    error.hidden = false;
  }
});

$("log").addEventListener("click", (event) => {
  const button = event.target.closest(".name-button");
  if (!button || !button.dataset.domain) return;
  const entry = entries.find((item) => item.button === button) ?? null;
  void openInspector({
    name: button.dataset.domain,
    client: button.dataset.client ?? null,
    entry,
    record: entry?.event ?? null,
  });
});

for (const chip of document.querySelectorAll(".chip")) {
  chip.addEventListener("click", () => setFilter(chip.dataset.filter));
}

$("log-search").addEventListener("input", (event) => {
  searchTerm = event.target.value.trim().toLowerCase();
  for (const entry of entries) applyFilter(entry);
  syncEmptyStates();
});

$("bogus-review").addEventListener("click", () => {
  setFilter("errors");
  $("log-scroll").scrollIntoView({ block: "nearest" });
});

$("command-open").addEventListener("click", openCommand);

document.addEventListener("keydown", (event) => {
  if (event.key !== "/" || isTyping(event.target) || $("inspector").open) return;
  event.preventDefault();
  openCommand();
});

$("command").addEventListener("click", (event) => {
  const dialog = $("command");
  if (event.target === dialog) dialog.close();
});

$("command-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const name = $("command-input").value.trim();
  if (!name) return;
  $("command").close();
  const known = entries.find(
    (entry) => entry.event.name?.toLowerCase() === name.toLowerCase(),
  );
  void openInspector({
    name,
    client: known?.event.client ?? null,
    entry: known ?? null,
    record: known?.event ?? null,
  });
});

$("inspect-close").addEventListener("click", () => $("inspector").close());

$("inspector").addEventListener("close", () => {
  if (selectedEntry) selectedEntry.tr.classList.remove("is-selected");
  selectedEntry = null;
});

$("inspect-copy").addEventListener("click", async () => {
  const button = $("inspect-copy");
  try {
    await navigator.clipboard.writeText(lastTrace);
    button.textContent = "Copied";
  } catch {
    button.textContent = "Copy failed";
  }
  setTimeout(() => {
    button.textContent = "Copy trace";
  }, 1500);
});

$("inspect-allow").addEventListener("click", async () => {
  const button = $("inspect-allow");
  if (!lastSubject) return;
  button.disabled = true;
  try {
    await post("/api/allow", { domain: lastSubject, seconds: 300 });
    button.textContent = "Allowed for 5 minutes";
    const query = new URLSearchParams({ domain: lastSubject });
    const answer = await api(`/api/explain?${query}`);
    renderInspector(answer, selectedEntry?.event ?? null);
    $("inspect-allow").textContent = "Allowed for 5 minutes";
    $("inspect-allow").disabled = true;
  } catch {
    button.textContent = "Failed";
    button.disabled = false;
  }
});

start().catch(() => {
  $("login").hidden = false;
  $("token").focus();
});
