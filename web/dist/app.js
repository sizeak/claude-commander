"use strict";

// Claude Commander web UI — vanilla JS frontend.
//
// Talks to the embedded axum server: polls /api/sessions and /api/projects,
// opens a WebSocket per selected session to mirror its tmux pane and forward
// keystrokes, and drives projects/settings/new-session via the JSON API. HTTP
// Basic auth is handled by the browser (the page itself loaded behind it), so
// fetch()/WebSocket carry the stored credentials automatically.

const POLL_MS = 1500;

const els = {
  // sidebar
  tree: document.getElementById("tree"),
  conn: document.getElementById("conn-status"),
  newBtn: document.getElementById("new-btn"),
  settingsBtn: document.getElementById("settings-btn"),
  refresh: document.getElementById("refresh-btn"),
  addProjectBtn: document.getElementById("add-project-btn"),
  // toolbar
  menuBtn: document.getElementById("menu-btn"),
  title: document.getElementById("session-title"),
  micBtn: document.getElementById("mic-btn"),
  kbdBtn: document.getElementById("kbd-btn"),
  historyBtn: document.getElementById("history-btn"),
  infoBtn: document.getElementById("info-btn"),
  restart: document.getElementById("restart-btn"),
  kill: document.getElementById("kill-btn"),
  delete: document.getElementById("delete-btn"),
  // main
  placeholder: document.getElementById("terminal-placeholder"),
  terminal: document.getElementById("terminal"),
  resumeLive: document.getElementById("resume-live"),
  keyBar: document.getElementById("key-bar"),
  ctrlKey: document.getElementById("ctrl-key"),
  backdrop: document.getElementById("sidebar-backdrop"),
  infoPanel: document.getElementById("info-panel"),
  infoList: document.getElementById("info-list"),
  // modals
  newModal: document.getElementById("new-modal"),
  newForm: document.getElementById("new-form"),
  newProject: document.getElementById("new-project"),
  newProjectName: document.getElementById("new-project-name"),
  newTitle: document.getElementById("new-title"),
  newProgram: document.getElementById("new-program"),
  newEffort: document.getElementById("new-effort"),
  newMode: document.getElementById("new-mode"),
  newSection: document.getElementById("new-section"),
  newBase: document.getElementById("new-base"),
  newPrompt: document.getElementById("new-prompt"),
  projectModal: document.getElementById("project-modal"),
  projectPath: document.getElementById("project-path"),
  addPathBtn: document.getElementById("add-path-btn"),
  scanDirBtn: document.getElementById("scan-dir-btn"),
  settingsModal: document.getElementById("settings-modal"),
  settingsForm: document.getElementById("settings-form"),
  settingsRestart: document.getElementById("settings-restart"),
  webPwStatus: document.getElementById("web-pw-status"),
};

const state = {
  sessions: [],
  projects: [],
  meta: null,
  selectedId: null,
  showInfo: false,
  collapsed: new Set(), // project ids that are collapsed in the tree
  ws: null,
  term: null,
  fit: null,
  // "live" mirrors the pane on each tick; "history" pauses that and shows a
  // scrollable snapshot of the pane's scrollback so the user can scroll up.
  mode: "live",
  // Sticky Ctrl modifier for the on-screen key bar: when armed, the next typed
  // character is sent as its control code.
  ctrlPending: false,
};

// ---- Generic fetch helpers ----

async function apiGet(path) {
  const res = await fetch(path, { headers: { Accept: "application/json" } });
  if (!res.ok) throw new Error("HTTP " + res.status);
  return res.json();
}

async function action(method, path, body) {
  const opts = { method };
  if (body !== undefined) {
    opts.headers = { "Content-Type": "application/json" };
    opts.body = JSON.stringify(body);
  }
  const res = await fetch(path, opts);
  if (!res.ok) {
    let msg = "HTTP " + res.status;
    try {
      const j = await res.json();
      if (j.error) msg = j.error;
    } catch (_) {}
    alert("Action failed: " + msg);
    return null;
  }
  if (res.status === 204) return true;
  try {
    return await res.json();
  } catch (_) {
    return true;
  }
}

function setConn(cls, text) {
  els.conn.className = "conn " + cls;
  els.conn.textContent = text;
}

// ---- Sessions + projects polling ----

async function refreshAll() {
  try {
    const [sessions, projects] = await Promise.all([
      apiGet("/api/sessions"),
      apiGet("/api/projects"),
    ]);
    state.sessions = sessions;
    state.projects = projects;
    setConn("ok", "connected");
    renderTree();
  } catch (e) {
    setConn("error", "disconnected");
  }
}

function statusBadge(status) {
  const span = document.createElement("span");
  span.className = "badge " + status;
  span.textContent = status.replace(/_/g, " ");
  return span;
}

function sessionRow(s) {
  const li = document.createElement("li");
  li.className = "session" + (s.id === state.selectedId ? " active" : "");
  li.dataset.id = s.id;

  const row = document.createElement("div");
  row.className = "session-row";
  const name = document.createElement("span");
  name.className = "session-name";
  name.textContent = s.title;
  row.appendChild(name);
  row.appendChild(statusBadge(s.status));
  li.appendChild(row);

  const meta = document.createElement("div");
  meta.className = "session-meta";
  const pr = s.pr_number ? ` · PR #${s.pr_number}` : "";
  meta.textContent = `${s.branch}${pr}`;
  li.appendChild(meta);

  li.addEventListener("click", () => selectSession(s.id));
  li.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    showContextMenu(e, sessionMenuItems(s));
  });
  return li;
}

// Build the right-click menu for a session.
function sessionMenuItems(s) {
  const items = [{ type: "label", text: s.title }];
  items.push({
    text: "Open",
    onClick: () => selectSession(s.id),
  });
  if (s.status !== "creating") {
    items.push({
      text: "Restart",
      onClick: async () => {
        if (await action("POST", `/api/sessions/${s.id}/restart`)) refreshAll();
      },
    });
    items.push({
      text: "Kill",
      onClick: async () => {
        if (!confirm(`Kill session "${s.title}"?`)) return;
        if (await action("POST", `/api/sessions/${s.id}/kill`)) refreshAll();
      },
    });
  }
  items.push({ type: "sep" });
  items.push({
    text: "Delete",
    danger: true,
    onClick: () => deleteSession(s),
  });
  return items;
}

// Build the right-click menu for a project header.
function projectMenuItems(p) {
  return [
    { type: "label", text: p.name },
    { text: "New session here", onClick: () => openNewSession(p) },
    {
      text: "Copy repo path",
      onClick: () => copyToClipboard(p.repo_path, "Path copied"),
    },
    { type: "sep" },
    { text: "Remove project", danger: true, onClick: () => removeProject(p) },
  ];
}

// Render the sidebar as a project-grouped tree: each project is a header with a
// ＋ (new session in this project) and ✕ (remove project), and its sessions are
// nested underneath. Collapsing a project hides its sessions.
function renderTree() {
  els.tree.innerHTML = "";

  if (state.projects.length === 0) {
    const div = document.createElement("div");
    div.className = "tree-empty";
    div.textContent = "No projects yet. Use ＋ add to register a repo.";
    els.tree.appendChild(div);
  }

  // Group sessions by project name (SessionInfo carries project_name).
  const byProject = new Map();
  for (const s of state.sessions) {
    if (!byProject.has(s.project_name)) byProject.set(s.project_name, []);
    byProject.get(s.project_name).push(s);
  }

  for (const p of state.projects) {
    const group = document.createElement("div");
    group.className = "project-group";

    const header = document.createElement("div");
    header.className = "project-header";
    const collapsed = state.collapsed.has(p.id);

    const twisty = document.createElement("span");
    twisty.className = "twisty";
    twisty.textContent = collapsed ? "▶" : "▼";
    header.appendChild(twisty);

    const name = document.createElement("span");
    name.className = "pname";
    name.textContent = p.name;
    name.title = p.repo_path;
    header.appendChild(name);

    const count = document.createElement("span");
    count.className = "pcount";
    count.textContent = p.session_count || "";
    header.appendChild(count);

    const hover = document.createElement("span");
    hover.className = "phover";
    const add = document.createElement("button");
    add.className = "icon-btn add";
    add.textContent = "＋";
    add.title = `New session in ${p.name}`;
    add.addEventListener("click", (e) => {
      e.stopPropagation();
      openNewSession(p);
    });
    const del = document.createElement("button");
    del.className = "icon-btn del";
    del.textContent = "✕";
    del.title = `Remove ${p.name}`;
    del.addEventListener("click", (e) => {
      e.stopPropagation();
      removeProject(p);
    });
    hover.appendChild(add);
    hover.appendChild(del);
    header.appendChild(hover);

    header.addEventListener("click", () => {
      if (state.collapsed.has(p.id)) state.collapsed.delete(p.id);
      else state.collapsed.add(p.id);
      renderTree();
    });
    header.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      showContextMenu(e, projectMenuItems(p));
    });
    group.appendChild(header);

    if (!collapsed) {
      const ul = document.createElement("ul");
      ul.className = "project-sessions";
      const sessions = byProject.get(p.name) || [];
      if (sessions.length === 0) {
        const empty = document.createElement("li");
        empty.className = "empty";
        empty.textContent = "no sessions";
        ul.appendChild(empty);
      } else {
        for (const s of sessions) ul.appendChild(sessionRow(s));
      }
      group.appendChild(ul);
    }

    els.tree.appendChild(group);
  }

  // Sessions whose project isn't in the projects list (shouldn't normally
  // happen, but don't silently hide them).
  const known = new Set(state.projects.map((p) => p.name));
  const orphans = state.sessions.filter((s) => !known.has(s.project_name));
  if (orphans.length) {
    const group = document.createElement("div");
    group.className = "project-group";
    const header = document.createElement("div");
    header.className = "project-header";
    header.innerHTML = '<span class="twisty"></span><span class="pname">(other)</span>';
    group.appendChild(header);
    const ul = document.createElement("ul");
    ul.className = "project-sessions";
    for (const s of orphans) ul.appendChild(sessionRow(s));
    group.appendChild(ul);
    els.tree.appendChild(group);
  }

  // Keep toolbar in sync with the selected session's current status.
  const sel = currentSession();
  const active = sel && sel.status !== "creating";
  els.restart.disabled = !sel;
  els.kill.disabled = !active;
  els.delete.disabled = !sel;
  els.infoBtn.disabled = !sel;
  els.micBtn.disabled = !sel;
  els.kbdBtn.disabled = !sel;
  els.historyBtn.disabled = !sel;
  els.title.textContent = sel ? sel.title : "Select a session";
  // Drives the mobile-only special-key bar (shown only with a session open).
  document.body.classList.toggle("has-session", !!sel);
  if (state.showInfo) renderInfo();
}

function currentSession() {
  return state.sessions.find((s) => s.id === state.selectedId) || null;
}

// ---- Info panel ----

function toggleInfo() {
  state.showInfo = !state.showInfo;
  els.infoPanel.classList.toggle("hidden", !state.showInfo);
  if (state.showInfo) renderInfo();
  fitNow();
  sendResize();
}

function renderInfo() {
  const s = currentSession();
  els.infoList.innerHTML = "";
  if (!s) return;
  const rows = [
    ["Title", s.title],
    ["Project", s.project_name],
    ["Branch", s.branch],
    ["Status", s.status],
    ["Program", s.program],
    ["PR", s.pr_number ? `#${s.pr_number} (${s.pr_state})` : "—"],
    ["PR labels", s.pr_labels && s.pr_labels.length ? s.pr_labels.join(", ") : "—"],
    ["Review", s.review_decision || "—"],
    ["Created", s.created_at ? new Date(s.created_at).toLocaleString() : "—"],
    ["ID", s.id],
  ];
  for (const [k, v] of rows) {
    const dt = document.createElement("dt");
    dt.textContent = k;
    const dd = document.createElement("dd");
    dd.textContent = v;
    els.infoList.appendChild(dt);
    els.infoList.appendChild(dd);
  }
}

// ---- Terminal ----

function ensureTerm() {
  if (state.term) return;
  state.term = new Terminal({
    // We paint each captured row at an explicit cursor position (see
    // paintSnapshot), so convertEol must stay OFF — letting xterm translate the
    // snapshot's "\n" into line feeds would interact with auto-wrap on
    // full-width rows and insert spurious blank lines (vertical drift).
    convertEol: false,
    cursorBlink: false,
    disableStdin: false,
    fontFamily: "Menlo, Monaco, 'Courier New', monospace",
    fontSize: 13,
    scrollback: 0,
    theme: { background: "#000000" },
  });
  state.fit = new FitAddon.FitAddon();
  state.term.loadAddon(state.fit);
  state.term.open(els.terminal);
  // Mobile keyboards apply autocorrect / predictive text / auto-capitalisation
  // to xterm's hidden input textarea, which rewrites its contents and makes
  // typed text re-send and duplicate/garble. Turn all of that off so each
  // keystroke is forwarded verbatim, once.
  if (state.term.textarea) {
    const ta = state.term.textarea;
    ta.setAttribute("autocorrect", "off");
    ta.setAttribute("autocapitalize", "off");
    ta.setAttribute("autocomplete", "off");
    ta.setAttribute("spellcheck", "false");
  }
  // Fit after layout settles — calling fit() synchronously right after open()
  // can measure a zero/short container and pick the wrong size.
  requestAnimationFrame(() => fitNow());

  // Forward every keystroke to the session as raw bytes. Typing while the
  // history view is up snaps back to the live mirror first. A sticky Ctrl
  // (armed from the on-screen key bar) rewrites the next character to its
  // control code (e.g. Ctrl + C → 0x03).
  state.term.onData((data) => {
    if (state.mode === "history") exitHistory();
    let out = data;
    if (state.ctrlPending && data.length === 1) {
      out = String.fromCharCode(data.charCodeAt(0) & 0x1f);
      armCtrl(false);
    }
    sendData(out);
  });

}

// Keep the layout sized to the *visible* viewport. The mobile soft keyboard and
// the dictation panel shrink window.visualViewport rather than the layout
// viewport; without this the app keeps its full height and the terminal gets
// pushed off-screen (needing a refresh). Driving --app-vh from the visual
// viewport keeps everything within the visible area.
function syncAppHeight() {
  const h = window.visualViewport ? window.visualViewport.height : window.innerHeight;
  document.documentElement.style.setProperty("--app-vh", `${Math.round(h)}px`);
}

// Viewport changed (window resize, keyboard/dictation open-close, orientation):
// re-sync height, refit the xterm grid, and tell the server the new size.
function handleViewportChange() {
  syncAppHeight();
  fitNow();
  sendResize();
}

window.addEventListener("resize", handleViewportChange);
if (window.visualViewport) {
  window.visualViewport.addEventListener("resize", handleViewportChange);
  window.visualViewport.addEventListener("scroll", handleViewportChange);
}

// Send a string to the session as raw UTF-8 bytes (no-op if the socket is down).
function sendData(data) {
  if (state.ws && state.ws.readyState === WebSocket.OPEN) {
    state.ws.send(new TextEncoder().encode(data));
  }
}

// Fit the xterm grid to its container (no-op if not ready). Safe to call often.
function fitNow() {
  if (state.fit) {
    try {
      state.fit.fit();
    } catch (_) {}
  }
}

// Tell the server the terminal's current size so it can resize the tmux window
// to match — otherwise capture-pane returns content laid out for tmux's default
// width and the UI looks scrambled. Debounced so a window drag doesn't spam
// resize-window calls.
let resizeTimer = null;
function sendResize() {
  if (resizeTimer) clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    if (
      state.ws &&
      state.ws.readyState === WebSocket.OPEN &&
      state.term &&
      state.term.cols &&
      state.term.rows
    ) {
      state.ws.send(
        JSON.stringify({ type: "resize", cols: state.term.cols, rows: state.term.rows })
      );
    }
  }, 150);
}

function selectSession(id) {
  if (state.selectedId === id) return;
  state.selectedId = id;
  renderTree();

  ensureTerm();
  els.placeholder.style.display = "none";
  // Switching sessions always drops back to the live view.
  resetHistoryUi();
  // On mobile the sidebar is a drawer; picking a session gets it out of the way.
  if (isMobile()) closeDrawer();

  // Tear down any previous socket, then open a fresh (reconnecting) one.
  closeSocket();

  state.term.options.scrollback = 0;
  state.term.reset();
  state.term.write("\x1b[2J\x1b[H");

  openSocket(id);
}

// Reconnect timer handle, so we never stack multiple pending reconnects.
let wsReconnectTimer = null;

// Intentionally close the terminal socket (switching/closing a session). Nulls
// the handlers first so the teardown doesn't trigger the auto-reconnect path.
function closeSocket() {
  if (wsReconnectTimer) {
    clearTimeout(wsReconnectTimer);
    wsReconnectTimer = null;
  }
  if (state.ws) {
    state.ws.onclose = null;
    state.ws.onmessage = null;
    state.ws.onerror = null;
    try {
      state.ws.close();
    } catch (_) {}
    state.ws = null;
  }
}

// Open (or reopen) the terminal socket for `id`. On an *unexpected* drop — a
// mobile browser suspending a backgrounded tab, a flaky network, or a server
// restart — we auto-reconnect while this session stays selected, so switching
// tabs (or a deploy) doesn't leave a dead terminal. The current screen is kept;
// the first snapshot from the new socket repaints it.
function openSocket(id) {
  closeSocket();

  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const ws = new WebSocket(`${proto}//${location.host}/ws/sessions/${id}`);
  ws.binaryType = "arraybuffer";
  state.ws = ws;

  ws.onopen = () => {
    setConn("ok", "connected");
    // Fit to the container, then push our size up front (bypassing the debounce)
    // so the server resizes the tmux window before the first capture tick and
    // the very first snapshot is laid out for the right dimensions.
    fitNow();
    if (state.term && state.term.cols && state.term.rows) {
      ws.send(
        JSON.stringify({ type: "resize", cols: state.term.cols, rows: state.term.rows })
      );
    }
  };

  ws.onmessage = (ev) => {
    // History view is frozen: ignore live snapshots so the scroll position holds.
    if (state.mode === "history") return;
    const text = typeof ev.data === "string" ? ev.data : new TextDecoder().decode(ev.data);
    paintSnapshot(text);
  };

  ws.onclose = () => {
    // Ignore if this isn't the active session's socket any more (a newer socket
    // or an intentional teardown superseded it).
    if (state.selectedId !== id || state.ws !== ws) return;
    state.ws = null;
    setConn("error", "reconnecting…");
    scheduleReconnect(id);
  };

  ws.onerror = () => setConn("error", "stream error");
}

// Retry the socket after a short delay, but only while the page is visible — a
// backgrounded mobile tab can't do useful work, so we wait for the return (see
// the visibilitychange handler) rather than hammering reconnects in the dark.
function scheduleReconnect(id) {
  if (wsReconnectTimer) return;
  wsReconnectTimer = setTimeout(() => {
    wsReconnectTimer = null;
    if (state.selectedId === id && document.visibilityState === "visible") {
      openSocket(id);
    }
  }, 1500);
}

// Coming back to a backgrounded tab: mobile browsers often suspended or closed
// the socket, so reconnect immediately; if it survived, just refit and force a
// fresh snapshot in case the viewport changed while we were away.
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState !== "visible" || !state.selectedId) return;
  if (!state.ws || state.ws.readyState > WebSocket.OPEN) {
    if (wsReconnectTimer) {
      clearTimeout(wsReconnectTimer);
      wsReconnectTimer = null;
    }
    openSocket(state.selectedId);
  } else if (state.ws.readyState === WebSocket.OPEN) {
    fitNow();
    forceResync();
  }
});

// Paint a full visible-screen snapshot (a flat grid of rows, ANSI included)
// onto the terminal. Each captured line is one terminal row, so we position the
// cursor at the start of each row explicitly (\x1b[<row>;1H), clear that row
// (\x1b[2K), and write the line. This avoids relying on \n/auto-wrap, which on
// full-width rows inserts phantom blank lines and drifts the layout. Rows the
// snapshot doesn't cover (shorter capture than the viewport) are cleared so no
// stale content lingers. The whole repaint is wrapped in cursor hide/show to
// avoid a flickering caret.
function paintSnapshot(text) {
  if (!state.term) return;
  const rows = state.term.rows;
  const lines = text.split("\n");
  let out = "\x1b[?25l\x1b[H"; // hide cursor, home
  for (let r = 0; r < rows; r++) {
    // Move to row r+1, col 1; clear the line; write content (or nothing).
    out += `\x1b[${r + 1};1H\x1b[2K`;
    if (r < lines.length) out += lines[r];
  }
  out += "\x1b[?25h"; // show cursor
  state.term.write(out);
}

// ---- Mobile: sidebar drawer ----

// Match the CSS breakpoint that switches the sidebar to an off-canvas drawer.
function isMobile() {
  return window.matchMedia("(max-width: 768px)").matches;
}

function openDrawer() {
  document.body.classList.add("drawer-open");
  els.backdrop.classList.remove("hidden");
}
function closeDrawer() {
  document.body.classList.remove("drawer-open");
  els.backdrop.classList.add("hidden");
}

els.menuBtn.addEventListener("click", () => {
  if (document.body.classList.contains("drawer-open")) closeDrawer();
  else openDrawer();
});
els.backdrop.addEventListener("click", closeDrawer);

// ---- Mobile: on-screen keyboard + special keys ----

// Escape sequences the key bar emits (arrow keys, Esc, Tab, Enter).
const KEY_SEQ = {
  esc: "\x1b",
  tab: "\t",
  enter: "\r",
  up: "\x1b[A",
  down: "\x1b[B",
  right: "\x1b[C",
  left: "\x1b[D",
};

// Arm/disarm the sticky Ctrl modifier and reflect it on the key.
function armCtrl(on) {
  state.ctrlPending = on;
  els.ctrlKey.classList.toggle("sticky-active", on);
}

// The ⌨ button just focuses the terminal, which raises the soft keyboard.
els.kbdBtn.addEventListener("click", () => {
  if (state.term) state.term.focus();
});

els.keyBar.querySelectorAll("button[data-key]").forEach((btn) => {
  // Keep focus on the terminal's textarea so the soft keyboard doesn't drop.
  btn.addEventListener("mousedown", (e) => e.preventDefault());
  btn.addEventListener("click", () => {
    const key = btn.dataset.key;
    if (key === "ctrl") {
      armCtrl(!state.ctrlPending);
      return;
    }
    if (state.mode === "history") exitHistory();
    const seq = KEY_SEQ[key];
    if (seq) sendData(seq);
  });
});

// ---- Microphone dictation (Web Speech API) ----

// Browser speech recognition requires a secure context (HTTPS); over plain HTTP
// the API is absent, so the button explains what's needed rather than silently
// failing. When it works, final transcripts are sent to the pane as if typed.
let recognition = null;
let dictating = false;

function updateMicButton() {
  els.micBtn.classList.toggle("mic-active", dictating);
  els.micBtn.title = dictating ? "Stop dictation" : "Dictate (microphone)";
}

function toggleDictation() {
  if (dictating) {
    if (recognition) recognition.stop();
    return;
  }
  const SR = window.SpeechRecognition || window.webkitSpeechRecognition;
  if (!window.isSecureContext || !SR) {
    alert(
      "Microphone dictation needs a secure (HTTPS) connection — browsers block " +
        "mic access over plain HTTP. Reach this UI over HTTPS (e.g. a Tailscale/" +
        "WireGuard address, a TLS reverse proxy, or the mutual-TLS mode) and the " +
        "mic button will work."
    );
    return;
  }
  recognition = new SR();
  recognition.lang = navigator.language || "en-US";
  recognition.continuous = true;
  recognition.interimResults = false;
  recognition.onresult = (e) => {
    for (let i = e.resultIndex; i < e.results.length; i++) {
      if (e.results[i].isFinal) sendData(e.results[i][0].transcript);
    }
  };
  recognition.onend = () => {
    dictating = false;
    recognition = null;
    updateMicButton();
  };
  recognition.onerror = (e) => {
    // "no-speech"/"aborted" are routine (silence, or the user stopping); surface
    // the rest briefly.
    if (e.error !== "aborted" && e.error !== "no-speech") {
      setConn("error", `mic: ${e.error}`);
      setTimeout(() => setConn("ok", "connected"), 2000);
    }
  };
  try {
    recognition.start();
    dictating = true;
    updateMicButton();
  } catch (_) {
    dictating = false;
    recognition = null;
    updateMicButton();
  }
}

els.micBtn.addEventListener("click", toggleDictation);

// ---- Terminal history / scroll-back view ----

// Reset the history UI to the live state without touching the socket. Used both
// when leaving history and when switching sessions (which rebuilds the socket).
function resetHistoryUi() {
  state.mode = "live";
  els.resumeLive.classList.add("hidden");
  els.historyBtn.classList.remove("sticky-active");
}

// Ask the server for a fresh snapshot immediately. The live stream only pushes
// on change, so after we clear the screen (leaving history) we'd otherwise stare
// at a blank pane until its content next changed. Reusing the resize control
// message forces the server to drop its dedupe hash and repaint next tick.
function forceResync() {
  if (
    state.ws &&
    state.ws.readyState === WebSocket.OPEN &&
    state.term &&
    state.term.cols &&
    state.term.rows
  ) {
    state.ws.send(
      JSON.stringify({ type: "resize", cols: state.term.cols, rows: state.term.rows })
    );
  }
}

// Enter the frozen, scrollable history view: fetch the pane's scrollback and
// render it with real line breaks so xterm builds a buffer the user can scroll.
async function enterHistory() {
  const s = currentSession();
  if (!s || !state.term) return;
  try {
    const res = await fetch(`/api/sessions/${s.id}/scrollback?lines=2000`, {
      headers: { Accept: "text/plain" },
    });
    if (!res.ok) throw new Error("HTTP " + res.status);
    const text = await res.text();
    state.mode = "history";
    els.resumeLive.classList.remove("hidden");
    els.historyBtn.classList.add("sticky-active");
    state.term.options.scrollback = 5000;
    state.term.reset();
    // convertEol is off (see ensureTerm), so translate LF → CRLF ourselves.
    state.term.write(text.replace(/\r?\n/g, "\r\n"));
  } catch (e) {
    alert("Failed to load history: " + e.message);
  }
}

// Leave the history view and resume the live mirror.
function exitHistory() {
  if (state.mode !== "history") return;
  resetHistoryUi();
  if (state.term) {
    state.term.options.scrollback = 0;
    state.term.reset();
    state.term.write("\x1b[2J\x1b[H");
  }
  forceResync();
}

els.historyBtn.addEventListener("click", () => {
  if (state.mode === "history") exitHistory();
  else enterHistory();
});
els.resumeLive.addEventListener("click", exitHistory);

// ---- Session lifecycle actions ----

els.refresh.addEventListener("click", refreshAll);
els.infoBtn.addEventListener("click", toggleInfo);

els.restart.addEventListener("click", async () => {
  const s = currentSession();
  if (!s) return;
  if (await action("POST", `/api/sessions/${s.id}/restart`)) refreshAll();
});

els.kill.addEventListener("click", async () => {
  const s = currentSession();
  if (!s) return;
  if (!confirm(`Kill session "${s.title}"?`)) return;
  if (await action("POST", `/api/sessions/${s.id}/kill`)) refreshAll();
});

els.delete.addEventListener("click", () => {
  const s = currentSession();
  if (s) deleteSession(s);
});

// Delete a session (shared by the toolbar button and the right-click menu).
async function deleteSession(s) {
  if (!confirm(`Delete session "${s.title}"? This removes its worktree.`)) return;
  if (await action("DELETE", `/api/sessions/${s.id}`)) {
    if (state.selectedId === s.id) {
      state.selectedId = null;
      closeSocket();
      els.placeholder.style.display = "flex";
    }
    refreshAll();
  }
}

// ---- Right-click context menu ----

let ctxMenuEl = null;

function hideContextMenu() {
  if (ctxMenuEl) {
    ctxMenuEl.remove();
    ctxMenuEl = null;
  }
}

// Show a context menu at the event's cursor position. `items` is a list of
// { text, onClick, danger } entries, plus { type: "sep" } / { type: "label" }.
function showContextMenu(e, items) {
  hideContextMenu();
  const menu = document.createElement("div");
  menu.id = "context-menu";
  for (const item of items) {
    if (item.type === "sep") {
      const sep = document.createElement("div");
      sep.className = "ctx-sep";
      menu.appendChild(sep);
    } else if (item.type === "label") {
      const lbl = document.createElement("div");
      lbl.className = "ctx-label";
      lbl.textContent = item.text;
      menu.appendChild(lbl);
    } else {
      const el = document.createElement("div");
      el.className = "ctx-item" + (item.danger ? " danger" : "");
      el.textContent = item.text;
      el.addEventListener("click", () => {
        hideContextMenu();
        item.onClick();
      });
      menu.appendChild(el);
    }
  }
  // Position at the cursor, then nudge back on-screen if it would overflow.
  document.body.appendChild(menu);
  const { innerWidth: w, innerHeight: h } = window;
  const rect = menu.getBoundingClientRect();
  const x = Math.min(e.clientX, w - rect.width - 8);
  const y = Math.min(e.clientY, h - rect.height - 8);
  menu.style.left = `${Math.max(8, x)}px`;
  menu.style.top = `${Math.max(8, y)}px`;
  ctxMenuEl = menu;
}

// Dismiss the menu on any click elsewhere, Esc, scroll, or window blur.
document.addEventListener("click", (e) => {
  if (ctxMenuEl && !ctxMenuEl.contains(e.target)) hideContextMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") hideContextMenu();
});
window.addEventListener("blur", hideContextMenu);
els.tree.addEventListener("scroll", hideContextMenu);

// Copy text to the clipboard, with a transient status hint.
async function copyToClipboard(text, okMsg) {
  try {
    await navigator.clipboard.writeText(text);
    setConn("ok", okMsg || "copied");
    setTimeout(() => setConn("ok", "connected"), 1200);
  } catch (_) {
    // Clipboard API needs a secure context (https/localhost); fall back.
    window.prompt("Copy:", text);
  }
}

// ---- Modals (open/close) ----

function openModal(el) {
  el.classList.remove("hidden");
}
function closeModal(el) {
  el.classList.add("hidden");
}
// Any element with data-close="<id>" closes that modal.
document.querySelectorAll("[data-close]").forEach((btn) => {
  btn.addEventListener("click", () => closeModal(document.getElementById(btn.dataset.close)));
});
// Click on the dim backdrop closes the modal.
document.querySelectorAll(".modal-overlay").forEach((overlay) => {
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) closeModal(overlay);
  });
});
// Esc closes any open modal.
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    document.querySelectorAll(".modal-overlay:not(.hidden)").forEach(closeModal);
  }
});

// ---- New session ----

async function ensureMeta() {
  if (!state.meta) state.meta = await apiGet("/api/meta");
  return state.meta;
}

function fillSelect(sel, values, { placeholder } = {}) {
  sel.innerHTML = "";
  if (placeholder !== undefined) {
    const o = document.createElement("option");
    o.value = "";
    o.textContent = placeholder;
    sel.appendChild(o);
  }
  for (const v of values) {
    const o = document.createElement("option");
    o.value = typeof v === "object" ? v.value : v;
    o.textContent = typeof v === "object" ? v.label : v;
    sel.appendChild(o);
  }
}

// Open the new-session form pre-targeted at a specific project.
async function openNewSession(project) {
  try {
    const meta = await ensureMeta();
    els.newProject.value = project.repo_path; // hidden field used on submit
    els.newProjectName.value = project.name;
    fillSelect(els.newEffort, meta.effort_levels, { placeholder: "(default)" });
    fillSelect(els.newMode, meta.permission_modes, { placeholder: "(default)" });
    fillSelect(els.newSection, meta.sections || [], { placeholder: "(auto)" });
    els.newProgram.placeholder = meta.default_program || "(default)";
    openModal(els.newModal);
    els.newTitle.focus();
  } catch (e) {
    alert("Failed to load form options: " + e.message);
  }
}

// Top-bar ＋: create in the only project, the selected session's project, or
// prompt to add one if there are none.
els.newBtn.addEventListener("click", () => {
  if (state.projects.length === 0) {
    alert("Add a project first.");
    openModal(els.projectModal);
    return;
  }
  const sel = currentSession();
  const target =
    (sel && state.projects.find((p) => p.name === sel.project_name)) ||
    state.projects[0];
  openNewSession(target);
});

els.newForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const body = {
    project_path: els.newProject.value,
    title: els.newTitle.value.trim(),
    program: els.newProgram.value.trim() || null,
    effort: els.newEffort.value || null,
    mode: els.newMode.value || null,
    section: els.newSection.value || null,
    base_branch: els.newBase.value.trim() || null,
    initial_prompt: els.newPrompt.value.trim() || null,
  };
  const res = await action("POST", "/api/sessions", body);
  if (res) {
    closeModal(els.newModal);
    els.newForm.reset();
    await refreshAll();
    // Jump straight into the new session's terminal.
    if (res.id) selectSession(res.id);
  }
});

// ---- Add / scan project ----

els.addProjectBtn.addEventListener("click", () => {
  els.projectPath.value = "";
  openModal(els.projectModal);
  els.projectPath.focus();
});

els.addPathBtn.addEventListener("click", async () => {
  const path = els.projectPath.value.trim();
  if (!path) return;
  if (await action("POST", "/api/projects", { path })) {
    closeModal(els.projectModal);
    refreshAll();
  }
});

els.scanDirBtn.addEventListener("click", async () => {
  const path = els.projectPath.value.trim();
  if (!path) return;
  const res = await action("POST", "/api/projects/scan", { path });
  if (res) {
    closeModal(els.projectModal);
    alert(`Scan complete: ${res.added} added, ${res.skipped} skipped.`);
    refreshAll();
  }
});

async function removeProject(p) {
  if (!confirm(`Remove project "${p.name}"? This deletes all its sessions and worktrees.`))
    return;
  if (await action("DELETE", `/api/projects/${p.id}`)) refreshAll();
}

// ---- Settings ----

// Show the Basic-auth field or the mTLS fields depending on the selected mode.
function syncAuthFields() {
  const mode = document.getElementById("set-web-ui-auth").value;
  const mtls = mode === "mutual_tls";
  document.getElementById("auth-mtls-fields").classList.toggle("hidden", !mtls);
  document.getElementById("auth-basic-fields").classList.toggle("hidden", mtls);
}
document.getElementById("set-web-ui-auth").addEventListener("change", syncAuthFields);

els.settingsBtn.addEventListener("click", async () => {
  try {
    const data = await apiGet("/api/config");
    const c = data.config;
    document.getElementById("set-default-program").value = c.default_program || "";
    document.getElementById("set-branch-prefix").value = c.branch_prefix || "";
    document.getElementById("set-worktrees-dir").value = c.worktrees_dir || "";
    document.getElementById("set-editor").value = c.editor || "";
    document.getElementById("set-fetch-before-create").checked = !!c.fetch_before_create;
    document.getElementById("set-resume-session").checked = !!c.resume_session;
    document.getElementById("set-web-ui-enabled").checked = !!c.web_ui_enabled;
    document.getElementById("set-web-ui-port").value = c.web_ui_port || 8420;
    document.getElementById("set-web-ui-auth").value = c.web_ui_auth || "basic";
    document.getElementById("set-web-ui-password").value = "";
    els.webPwStatus.textContent = c.web_ui_password
      ? "A password is set."
      : "No password set — Basic auth won't start until you set one.";
    document.getElementById("set-web-ui-tls-cert").value = c.web_ui_tls_cert || "";
    document.getElementById("set-web-ui-tls-key").value = c.web_ui_tls_key || "";
    document.getElementById("set-web-ui-tls-client-ca").value = c.web_ui_tls_client_ca || "";
    syncAuthFields();
    els.settingsRestart.classList.toggle("hidden", !data.restart_required);
    openModal(els.settingsModal);
  } catch (e) {
    alert("Failed to load settings: " + e.message);
  }
});

els.settingsForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const patch = {
    default_program: document.getElementById("set-default-program").value,
    branch_prefix: document.getElementById("set-branch-prefix").value,
    worktrees_dir: document.getElementById("set-worktrees-dir").value,
    editor: document.getElementById("set-editor").value,
    fetch_before_create: document.getElementById("set-fetch-before-create").checked,
    resume_session: document.getElementById("set-resume-session").checked,
    web_ui_enabled: document.getElementById("set-web-ui-enabled").checked,
    web_ui_port: parseInt(document.getElementById("set-web-ui-port").value, 10) || 8420,
    web_ui_auth: document.getElementById("set-web-ui-auth").value,
    web_ui_tls_cert: document.getElementById("set-web-ui-tls-cert").value,
    web_ui_tls_key: document.getElementById("set-web-ui-tls-key").value,
    web_ui_tls_client_ca: document.getElementById("set-web-ui-tls-client-ca").value,
  };
  // Only send the password if the user typed a new one (blank = leave unchanged).
  const pw = document.getElementById("set-web-ui-password").value;
  if (pw) patch.web_ui_password = pw;

  const res = await action("PUT", "/api/config", patch);
  if (res) {
    if (res.restart_required) {
      alert("Saved. A restart is required for some changes (e.g. the web UI port or auth type) to take effect.");
    }
    closeModal(els.settingsModal);
  }
});

// ---- Boot ----

syncAppHeight();
refreshAll().then(() => {
  // On a phone nothing is selected yet, so start with the session list open.
  if (isMobile() && !state.selectedId) openDrawer();
});
setInterval(refreshAll, POLL_MS);
