"use strict";

// Claude Commander web UI — talks to claude-commander-server through the
// same-origin reverse proxy provided by claude-commander-web.
//
// Auth is chosen by the server binary and reported at /webui/config:
//   - "bff":    the browser is already behind Basic auth (the binary injects the
//               bearer token upstream). We send no Authorization and no WS auth
//               frame — the proxy handles both.
//   - "direct": the browser holds the commander token. We send it as a Bearer
//               header on /api and as the in-band `auth` frame on the WS.
//
// The terminal is a real PTY attach (/ws/attach): binary frames are raw PTY
// bytes; text frames are JSON control (auth/attach/resize + ready/detached/error).

const POLL_MS = 1500;
const TOKEN_KEY = "cc_token";

const els = {
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
  shellBtn: document.getElementById("shell-btn"),
  reviewBtn: document.getElementById("review-btn"),
  infoBtn: document.getElementById("info-btn"),
  restart: document.getElementById("restart-btn"),
  kill: document.getElementById("kill-btn"),
  delete: document.getElementById("delete-btn"),
  // main
  placeholder: document.getElementById("terminal-placeholder"),
  terminal: document.getElementById("terminal"),
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
  connectModal: document.getElementById("connect-modal"),
  connectForm: document.getElementById("connect-form"),
  connectToken: document.getElementById("connect-token"),
  connectError: document.getElementById("connect-error"),
};

const state = {
  mode: null, // "bff" | "direct"
  token: null, // direct mode only
  sessions: [],
  projects: [],
  createOptions: null,
  agentStates: {}, // { sessionId: "working" | "idle" | ... }
  selectedId: null,
  collapsed: new Set(),
  showInfo: false,
  ws: null,
  term: null,
  fit: null,
  ctrlPending: false,
  attachKind: "agent", // "agent" | "shell" — which pane of the session to attach
};

// Effort levels / permission modes aren't enumerated by the server's
// create-options, so offer the standard set (all optional).
const EFFORT_LEVELS = ["low", "medium", "high"];
const PERMISSION_MODES = ["default", "plan", "acceptEdits", "bypassPermissions"];

// ---- Auth-aware fetch ----

function authHeaders() {
  return state.mode === "direct" && state.token
    ? { Authorization: "Bearer " + state.token }
    : {};
}

function onAuthFailure() {
  if (state.mode === "direct") {
    state.token = null;
    try {
      localStorage.removeItem(TOKEN_KEY);
    } catch (_) {}
    openConnect("That token was rejected.");
  }
}

async function apiGet(path) {
  const res = await fetch(path, {
    headers: { Accept: "application/json", ...authHeaders() },
  });
  if (res.status === 401) {
    onAuthFailure();
    throw new Error("unauthorized");
  }
  if (!res.ok) throw new Error("HTTP " + res.status);
  return res.json();
}

async function action(method, path, body) {
  const opts = { method, headers: { ...authHeaders() } };
  if (body !== undefined) {
    opts.headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(body);
  }
  const res = await fetch(path, opts);
  if (res.status === 401) {
    onAuthFailure();
    return null;
  }
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

// ---- Connect (direct mode) ----

function openConnect(errMsg) {
  if (errMsg) {
    els.connectError.textContent = errMsg;
    els.connectError.classList.remove("hidden");
  } else {
    els.connectError.classList.add("hidden");
  }
  openModal(els.connectModal);
  els.connectToken.focus();
}

els.connectForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const token = els.connectToken.value.trim();
  if (!token) return;
  state.token = token;
  try {
    localStorage.setItem(TOKEN_KEY, token);
  } catch (_) {}
  // Validate by a real request.
  try {
    await apiGet("/api/workspace");
    closeModal(els.connectModal);
    els.connectToken.value = "";
    refreshAll();
  } catch (_) {
    // onAuthFailure already re-opened with an error.
  }
});

// ---- Workspace polling ----

async function refreshAll() {
  try {
    const [ws, agents] = await Promise.all([
      apiGet("/api/workspace"),
      apiGet("/api/agent-states").catch(() => ({ states: {} })),
    ]);
    state.sessions = ws.sessions || [];
    state.projects = ws.projects || [];
    state.agentStates = (agents && agents.states) || {};
    setConn("ok", "connected");
    renderTree();
  } catch (e) {
    if (e.message !== "unauthorized") setConn("error", "disconnected");
  }
}

function agentStateFor(s) {
  return state.agentStates[s.id] || state.agentStates[s.session_id] || "unknown";
}

// ---- Tree render ----

function statusBadge(status) {
  const span = document.createElement("span");
  span.className = "badge " + status;
  span.textContent = status.replace(/_/g, " ");
  return span;
}

function agentBadge(st) {
  const span = document.createElement("span");
  span.className = "agent " + st;
  const dot = document.createElement("span");
  dot.className = "dot";
  span.appendChild(dot);
  span.appendChild(document.createTextNode(st.replace(/_/g, " ")));
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

  li.appendChild(agentBadge(agentStateFor(s)));

  li.addEventListener("click", () => selectSession(s.id));
  li.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    showContextMenu(e, sessionMenuItems(s));
  });
  return li;
}

function sessionMenuItems(s) {
  const items = [{ type: "label", text: s.title }];
  items.push({ text: "Open", onClick: () => selectSession(s.id) });
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
  items.push({ text: "Delete", danger: true, onClick: () => deleteSession(s) });
  return items;
}

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

function renderTree() {
  els.tree.innerHTML = "";

  if (state.projects.length === 0) {
    const div = document.createElement("div");
    div.className = "tree-empty";
    div.textContent = "No projects yet. Use ＋ add to register a repo.";
    els.tree.appendChild(div);
  }

  const byProject = new Map();
  for (const s of state.sessions) {
    const key = s.project_id;
    if (!byProject.has(key)) byProject.set(key, []);
    byProject.get(key).push(s);
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

    const sessions = byProject.get(p.id) || [];
    const count = document.createElement("span");
    count.className = "pcount";
    count.textContent = sessions.length || "";
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

  const sel = currentSession();
  const active = sel && sel.status !== "creating";
  els.restart.disabled = !sel;
  els.kill.disabled = !active;
  els.delete.disabled = !sel;
  els.infoBtn.disabled = !sel;
  els.reviewBtn.disabled = !sel;
  els.shellBtn.disabled = !sel;
  els.micBtn.disabled = !sel;
  els.kbdBtn.disabled = !sel;
  els.title.textContent = sel ? sel.title : "Select a session";
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
    ["Agent", agentStateFor(s).replace(/_/g, " ")],
    ["Program", s.program],
    ["PR", s.pr_number ? `#${s.pr_number} (${s.pr_state})` : "—"],
    ["Section", s.current_section || "—"],
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

// ---- Terminal (real PTY over /ws/attach) ----

function ensureTerm() {
  if (state.term) return;
  state.term = new Terminal({
    cursorBlink: true,
    disableStdin: false,
    fontFamily: "Menlo, Monaco, 'Courier New', monospace",
    fontSize: 13,
    scrollback: 5000,
    theme: { background: "#000000" },
  });
  state.fit = new FitAddon.FitAddon();
  state.term.loadAddon(state.fit);
  state.term.open(els.terminal);

  // Stop mobile keyboards mangling input (autocorrect/predictive re-sends).
  const ta = state.term.textarea;
  if (ta) {
    ta.setAttribute("autocorrect", "off");
    ta.setAttribute("autocapitalize", "off");
    ta.setAttribute("autocomplete", "off");
    ta.setAttribute("spellcheck", "false");
  }
  requestAnimationFrame(() => fitNow());

  // Keystrokes → raw PTY bytes (binary frame). A sticky Ctrl (from the key bar)
  // rewrites the next char to its control code. Paste arrives here too (multi-
  // char, so the sticky-Ctrl transform is skipped) and is forwarded verbatim.
  state.term.onData((data) => {
    let out = data;
    if (state.ctrlPending && data.length === 1) {
      out = String.fromCharCode(data.charCodeAt(0) & 0x1f);
      armCtrl(false);
    }
    sendData(out);
  });

  // Ctrl/Cmd+C copies the selection to the clipboard instead of sending SIGINT —
  // but only when there IS a selection, so a bare ^C still reaches the program.
  // (Paste is handled natively by xterm's paste event → onData above.)
  state.term.attachCustomKeyEventHandler((e) => {
    if (e.type !== "keydown") return true;
    const mod = e.ctrlKey || e.metaKey;
    // Ctrl+\ toggles between the agent pane and the paired shell, matching the
    // native client (that combo can't just be sent down the PTY — the toggle is
    // a client-side re-attach).
    if (e.ctrlKey && (e.key === "\\" || e.code === "Backslash")) {
      toggleShell();
      return false;
    }
    if (mod && (e.key === "c" || e.key === "C") && state.term.hasSelection()) {
      if (copySelection()) return false;
    }
    return true;
  });

  // Copy-on-select: xterm fires onSelectionChange synchronously while it handles
  // the drag, so the clipboard write runs inside that user gesture (Chrome
  // permits it) and sees the finalised selection — unlike a DOM mouseup on the
  // terminal div, which misses drags that release outside it and can run before
  // xterm updates the selection.
  state.term.onSelectionChange(() => copySelection());
}

// Copy the terminal's current selection to the clipboard, with a brief "copied"
// confirmation. Returns true if there was a selection to copy.
function copySelection() {
  if (!state.term || !state.term.hasSelection()) return false;
  const sel = state.term.getSelection();
  if (!sel || !navigator.clipboard) return false;
  navigator.clipboard
    .writeText(sel)
    .then(() => {
      setConn("ok", "copied");
      setTimeout(() => setConn("ok", "connected"), 1000);
    })
    .catch(() => {});
  return true;
}

// Send raw bytes to the PTY as a binary frame.
function sendData(data) {
  if (state.ws && state.ws.readyState === WebSocket.OPEN) {
    state.ws.send(new TextEncoder().encode(data));
  }
}

function fitNow() {
  if (state.fit) {
    try {
      state.fit.fit();
    } catch (_) {}
  }
}

let resizeTimer = null;
function sendResize() {
  if (resizeTimer) clearTimeout(resizeTimer);
  resizeTimer = setTimeout(sendResizeNow, 150);
}
function sendResizeNow() {
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

function selectSession(id) {
  if (state.selectedId === id) return;
  state.selectedId = id;
  state.attachKind = "agent"; // new session → start on the agent pane
  updateShellButton();
  renderTree();
  ensureTerm();
  els.placeholder.style.display = "none";
  if (isMobile()) closeDrawer();

  closeSocket();
  state.term.reset();
  openSocket(id);
}

let wsReconnectTimer = null;

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

// Open (or reopen) the PTY attach socket for `id`. Reconnects on unexpected drop
// (backgrounded mobile tab, network blip, server restart) while it stays
// selected; tmux replays the pane on re-attach, so it's seamless.
function openSocket(id) {
  closeSocket();
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const ws = new WebSocket(`${proto}//${location.host}/ws/attach`);
  ws.binaryType = "arraybuffer";
  state.ws = ws;

  ws.onopen = () => {
    setConn("ok", "connected");
    // direct mode: authenticate in-band. bff mode: the proxy injects auth.
    if (state.mode === "direct" && state.token) {
      ws.send(JSON.stringify({ type: "auth", token: state.token }));
    }
    const attach = { type: "attach", session_id: id };
    if (state.attachKind === "shell") attach.kind = "shell";
    ws.send(JSON.stringify(attach));
    fitNow();
    sendResizeNow();
  };

  ws.onmessage = (ev) => {
    if (typeof ev.data === "string") {
      handleControl(ev.data);
    } else {
      // Raw PTY bytes.
      state.term.write(new Uint8Array(ev.data));
    }
  };

  ws.onclose = () => {
    if (state.selectedId !== id || state.ws !== ws) return;
    state.ws = null;
    setConn("error", "reconnecting…");
    scheduleReconnect(id);
  };

  ws.onerror = () => setConn("error", "stream error");
}

function handleControl(text) {
  let msg;
  try {
    msg = JSON.parse(text);
  } catch (_) {
    return;
  }
  if (msg.type === "detached") {
    state.term.write(`\r\n\x1b[90m[detached: ${msg.reason || ""}]\x1b[0m\r\n`);
  } else if (msg.type === "error") {
    state.term.write(`\r\n\x1b[91m[error: ${msg.message || ""}]\x1b[0m\r\n`);
  }
  // "ready" needs no action; the pane streams in over binary frames.
}

function scheduleReconnect(id) {
  if (wsReconnectTimer) return;
  wsReconnectTimer = setTimeout(() => {
    wsReconnectTimer = null;
    if (state.selectedId === id && document.visibilityState === "visible") {
      openSocket(id);
    }
  }, 1500);
}

// Toggle between the agent pane and the paired shell by re-attaching with the
// other `kind`. The shell pane is created on demand server-side; tmux replays it
// on attach, so switching is just a fresh attach.
function toggleShell() {
  if (!state.selectedId) return;
  state.attachKind = state.attachKind === "shell" ? "agent" : "shell";
  updateShellButton();
  if (state.term) state.term.reset();
  closeSocket();
  openSocket(state.selectedId);
}

function updateShellButton() {
  const inShell = state.attachKind === "shell";
  els.shellBtn.textContent = inShell ? "Agent" : "Shell";
  els.shellBtn.title = inShell
    ? "Back to the agent pane (Ctrl+\\)"
    : "Toggle shell pane (Ctrl+\\)";
  els.shellBtn.classList.toggle("sticky-active", inShell);
}

els.shellBtn.addEventListener("click", toggleShell);

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
    sendResize();
  }
});

// ---- Viewport sizing (mobile keyboard / dictation) ----

function syncAppHeight() {
  const h = window.visualViewport ? window.visualViewport.height : window.innerHeight;
  document.documentElement.style.setProperty("--app-vh", `${Math.round(h)}px`);
}
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

// ---- Mobile: drawer ----

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

// ---- Mobile: on-screen keys ----

const KEY_SEQ = {
  esc: "\x1b",
  tab: "\t",
  enter: "\r",
  up: "\x1b[A",
  down: "\x1b[B",
  right: "\x1b[C",
  left: "\x1b[D",
};
function armCtrl(on) {
  state.ctrlPending = on;
  els.ctrlKey.classList.toggle("sticky-active", on);
}
els.kbdBtn.addEventListener("click", () => {
  if (state.term) state.term.focus();
});
els.keyBar.querySelectorAll("button[data-key]").forEach((btn) => {
  btn.addEventListener("mousedown", (e) => e.preventDefault());
  btn.addEventListener("click", () => {
    const key = btn.dataset.key;
    if (key === "ctrl") {
      armCtrl(!state.ctrlPending);
      return;
    }
    const seq = KEY_SEQ[key];
    if (seq) sendData(seq);
  });
});

// ---- Microphone dictation (Web Speech API) ----

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
      "Microphone dictation needs a secure (HTTPS) connection. Reach this UI over " +
        "HTTPS (e.g. via Tailscale) and the mic button will work."
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

// ---- Toolbar lifecycle actions ----

els.refresh.addEventListener("click", refreshAll);
els.infoBtn.addEventListener("click", toggleInfo);
els.restart.addEventListener("click", async () => {
  const s = currentSession();
  if (s && (await action("POST", `/api/sessions/${s.id}/restart`))) refreshAll();
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
  document.body.appendChild(menu);
  const { innerWidth: w, innerHeight: h } = window;
  const rect = menu.getBoundingClientRect();
  const x = Math.min(e.clientX, w - rect.width - 8);
  const y = Math.min(e.clientY, h - rect.height - 8);
  menu.style.left = `${Math.max(8, x)}px`;
  menu.style.top = `${Math.max(8, y)}px`;
  ctxMenuEl = menu;
}
document.addEventListener("click", (e) => {
  if (ctxMenuEl && !ctxMenuEl.contains(e.target)) hideContextMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") hideContextMenu();
});
window.addEventListener("blur", hideContextMenu);
els.tree.addEventListener("scroll", hideContextMenu);

async function copyToClipboard(text, okMsg) {
  try {
    await navigator.clipboard.writeText(text);
    setConn("ok", okMsg || "copied");
    setTimeout(() => setConn("ok", "connected"), 1200);
  } catch (_) {
    window.prompt("Copy:", text);
  }
}

// ---- Modals ----

function openModal(el) {
  el.classList.remove("hidden");
}
function closeModal(el) {
  el.classList.add("hidden");
}
document.querySelectorAll("[data-close]").forEach((btn) => {
  btn.addEventListener("click", () => closeModal(document.getElementById(btn.dataset.close)));
});
document.querySelectorAll(".modal-overlay").forEach((overlay) => {
  overlay.addEventListener("click", (e) => {
    // The connect modal is mandatory (no dismiss by backdrop).
    if (e.target === overlay && overlay.id !== "connect-modal") closeModal(overlay);
  });
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    document.querySelectorAll(".modal-overlay:not(.hidden)").forEach((m) => {
      if (m.id !== "connect-modal") closeModal(m);
    });
  }
});

// ---- New session ----

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

async function ensureCreateOptions() {
  if (!state.createOptions) state.createOptions = await apiGet("/api/create-options");
  return state.createOptions;
}

async function openNewSession(project) {
  try {
    const opts = await ensureCreateOptions();
    els.newProject.value = project.repo_path;
    els.newProjectName.value = project.name;
    fillSelect(els.newEffort, EFFORT_LEVELS, { placeholder: "(default)" });
    fillSelect(els.newMode, PERMISSION_MODES, { placeholder: "(default)" });
    fillSelect(els.newSection, opts.sections || [], { placeholder: "(auto)" });
    els.newProgram.placeholder = opts.default_program || "(default)";
    openModal(els.newModal);
    els.newTitle.focus();
  } catch (e) {
    if (e.message !== "unauthorized") alert("Failed to load form options: " + e.message);
  }
}

els.newBtn.addEventListener("click", () => {
  if (state.projects.length === 0) {
    alert("Add a project first.");
    openModal(els.projectModal);
    return;
  }
  const sel = currentSession();
  const target =
    (sel && state.projects.find((p) => p.id === sel.project_id)) || state.projects[0];
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
    base_branch: els.newBase.value.trim() || null,
    section: els.newSection.value || null,
    initial_prompt: els.newPrompt.value.trim() || null,
  };
  const res = await action("POST", "/api/sessions", body);
  if (res) {
    closeModal(els.newModal);
    els.newForm.reset();
    await refreshAll();
    const id = res.id || (res.info && res.info.id);
    if (id) selectSession(id);
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
    const added = res.added != null ? res.added : "?";
    const skipped = res.skipped != null ? res.skipped : "?";
    alert(`Scan complete: ${added} added, ${skipped} skipped.`);
    refreshAll();
  }
});
async function removeProject(p) {
  if (!confirm(`Remove project "${p.name}"? This deletes all its sessions and worktrees.`))
    return;
  if (await action("DELETE", `/api/projects/${p.id}`)) refreshAll();
}

// ---- Settings ----

els.settingsBtn.addEventListener("click", async () => {
  try {
    const c = await apiGet("/api/config");
    document.getElementById("set-branch-prefix").value = c.branch_prefix || "";
    document.getElementById("set-fetch-before-create").checked = !!c.fetch_before_create;
    document.getElementById("set-resume-session").checked = !!c.resume_session;
    document.getElementById("set-project-pull-enabled").checked = !!c.project_pull_enabled;
    openModal(els.settingsModal);
  } catch (e) {
    if (e.message !== "unauthorized") alert("Failed to load settings: " + e.message);
  }
});
els.settingsForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const patch = {
    branch_prefix: document.getElementById("set-branch-prefix").value,
    fetch_before_create: document.getElementById("set-fetch-before-create").checked,
    resume_session: document.getElementById("set-resume-session").checked,
    project_pull_enabled: document.getElementById("set-project-pull-enabled").checked,
  };
  if (await action("PATCH", "/api/config", patch)) closeModal(els.settingsModal);
});

// ---- Review (diff + comments) ----

const rv = {
  sessionId: null,
  snapshot: null,
  view: document.getElementById("review-view"),
  body: document.getElementById("review-body"),
  title: document.getElementById("review-title"),
  status: document.getElementById("review-status"),
};

function reviewDisplayPath(f) {
  return f.status === "deleted" ? f.old_path : f.new_path;
}

async function openReview() {
  const s = currentSession();
  if (!s) return;
  rv.sessionId = s.id;
  rv.title.textContent = `Review — ${s.title}`;
  rv.status.textContent = "loading…";
  rv.body.innerHTML = "";
  rv.view.classList.remove("hidden");
  try {
    rv.snapshot = await apiGet(`/api/sessions/${s.id}/review`);
    renderReview();
  } catch (e) {
    if (e.message !== "unauthorized") {
      rv.body.innerHTML = `<div class="rv-empty">Failed to load review: ${e.message}</div>`;
      rv.status.textContent = "";
    }
  }
}

function closeReview() {
  rv.view.classList.add("hidden");
  rv.sessionId = null;
  rv.snapshot = null;
  rv.body.innerHTML = "";
}

async function reloadReview() {
  if (!rv.sessionId) return;
  try {
    rv.snapshot = await apiGet(`/api/sessions/${rv.sessionId}/review`);
    renderReview();
  } catch (_) {}
}

function commentsFor(file, side, lineno) {
  return (rv.snapshot.comments || []).filter(
    (c) => c.file === file && c.side === side && c.line_range[0] === lineno
  );
}

function renderReview() {
  const snap = rv.snapshot;
  rv.body.innerHTML = "";
  const files = (snap.diff && snap.diff.files) || [];
  rv.status.textContent = `${files.length} file(s) · ${snap.comments.length} comment(s)`;
  if (files.length === 0) {
    rv.body.innerHTML = `<div class="rv-empty">No changes against ${snap.base}.</div>`;
    return;
  }
  const reviewed = new Set(snap.reviewed || []);
  for (const f of files) rv.body.appendChild(renderReviewFile(f, reviewed));
}

function renderReviewFile(f, reviewed) {
  const dp = reviewDisplayPath(f);
  const wrap = document.createElement("div");
  wrap.className = "rv-file";

  const header = document.createElement("div");
  header.className = "rv-file-header";
  const path = document.createElement("span");
  path.className = "rv-file-path";
  path.textContent = f.status === "renamed" ? `${f.old_path} → ${f.new_path}` : dp;
  header.appendChild(path);
  const st = document.createElement("span");
  st.className = "rv-file-status";
  st.textContent = f.status;
  header.appendChild(st);
  const stat = document.createElement("span");
  stat.innerHTML = `<span class="rv-stat-add">+${f.added}</span> <span class="rv-stat-del">-${f.removed}</span>`;
  header.appendChild(stat);
  const rev = document.createElement("label");
  rev.className = "rv-reviewed";
  const cb = document.createElement("input");
  cb.type = "checkbox";
  cb.checked = reviewed.has(dp);
  cb.addEventListener("change", () => toggleReviewed(dp, cb));
  rev.appendChild(cb);
  rev.appendChild(document.createTextNode("reviewed"));
  header.appendChild(rev);
  wrap.appendChild(header);

  if (f.binary) {
    const b = document.createElement("div");
    b.className = "rv-binary";
    b.textContent = "Binary file not shown.";
    wrap.appendChild(b);
    return wrap;
  }

  for (const h of f.hunks || []) {
    const hh = document.createElement("div");
    hh.className = "rv-hunk-header";
    hh.textContent = `@@ -${h.old_start},${h.old_lines} +${h.new_start},${h.new_lines} @@${h.header ? " " + h.header : ""}`;
    wrap.appendChild(hh);
    for (const line of h.lines) {
      wrap.appendChild(renderReviewLine(dp, line));
      const side = line.origin === "deletion" ? "old" : "new";
      const lineno = side === "old" ? line.old_lineno : line.new_lineno;
      if (lineno != null) {
        for (const c of commentsFor(dp, side, lineno)) wrap.appendChild(renderComment(c));
      }
    }
  }
  return wrap;
}

function renderReviewLine(file, line) {
  const row = document.createElement("div");
  row.className = "rv-line " + line.origin;
  const gutter = document.createElement("span");
  gutter.className = "rv-gutter";
  const o = line.old_lineno != null ? line.old_lineno : "";
  const n = line.new_lineno != null ? line.new_lineno : "";
  gutter.textContent = `${String(o).padStart(4)} ${String(n).padStart(4)}`;
  row.appendChild(gutter);
  const content = document.createElement("span");
  content.className = "rv-content";
  content.textContent = line.content;
  row.appendChild(content);
  row.addEventListener("click", () => openComposer(row, file, line));
  return row;
}

function renderComment(c) {
  const el = document.createElement("div");
  el.className = "rv-comment " + c.status;
  const meta = document.createElement("div");
  meta.className = "rv-comment-meta";
  const who = document.createElement("span");
  const range =
    c.line_range[1] !== c.line_range[0]
      ? `${c.line_range[0]}-${c.line_range[1]}`
      : `${c.line_range[0]}`;
  who.textContent = `${c.side}:${range} · ${c.status}`;
  meta.appendChild(who);
  const del = document.createElement("button");
  del.className = "rv-comment-del";
  del.textContent = "✕";
  del.title = "Delete comment";
  del.addEventListener("click", () => deleteComment(c.id));
  meta.appendChild(del);
  el.appendChild(meta);
  const body = document.createElement("div");
  body.textContent = c.comment;
  el.appendChild(body);
  return el;
}

let composerEl = null;
function openComposer(afterRow, file, line) {
  if (composerEl) composerEl.remove();
  const side = line.origin === "deletion" ? "old" : "new";
  const lineno = side === "old" ? line.old_lineno : line.new_lineno;
  if (lineno == null) return;

  const box = document.createElement("div");
  box.className = "rv-composer";
  const ta = document.createElement("textarea");
  ta.rows = 2;
  ta.placeholder = `Comment on ${side} line ${lineno}…`;
  box.appendChild(ta);
  const actions = document.createElement("div");
  actions.className = "rv-composer-actions";
  const cancel = document.createElement("button");
  cancel.className = "ghost";
  cancel.textContent = "Cancel";
  cancel.addEventListener("click", () => {
    box.remove();
    composerEl = null;
  });
  const save = document.createElement("button");
  save.className = "primary";
  save.textContent = "Comment";
  save.addEventListener("click", async () => {
    const text = ta.value.trim();
    if (!text) return;
    const ok = await action("POST", `/api/sessions/${rv.sessionId}/comments`, {
      file,
      side,
      line_range: [lineno, lineno],
      snippet: line.content,
      comment: text,
    });
    if (ok) {
      box.remove();
      composerEl = null;
      reloadReview();
    }
  });
  actions.appendChild(cancel);
  actions.appendChild(save);
  box.appendChild(actions);
  afterRow.insertAdjacentElement("afterend", box);
  composerEl = box;
  ta.focus();
}

async function deleteComment(cid) {
  if (await action("DELETE", `/api/sessions/${rv.sessionId}/comments/${cid}`)) reloadReview();
}

async function toggleReviewed(dp, cb) {
  const res = await action("POST", `/api/sessions/${rv.sessionId}/files/reviewed`, {
    display_path: dp,
  });
  if (res && typeof res.reviewed === "boolean") cb.checked = res.reviewed;
  else cb.checked = !cb.checked; // revert on failure
}

async function applyComments() {
  const res = await action("POST", `/api/sessions/${rv.sessionId}/comments/apply`);
  if (!res) return;
  switch (res.outcome) {
    case "applied":
      alert(`Applied ${res.count} comment(s) — brief sent to the agent.`);
      break;
    case "deferred":
      alert(`Wrote ${res.count} comment(s), but the agent wasn't ready — re-apply when it's at a prompt.`);
      break;
    case "blocked":
      alert(`Blocked: ${(res.drifted || []).length} comment(s) drifted. Refresh and re-anchor.`);
      break;
    default:
      alert("No staged comments to apply.");
  }
  reloadReview();
}

els.reviewBtn.addEventListener("click", openReview);
document.getElementById("review-close").addEventListener("click", closeReview);
document.getElementById("review-refresh").addEventListener("click", reloadReview);
document.getElementById("review-apply").addEventListener("click", applyComments);
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !rv.view.classList.contains("hidden")) closeReview();
});

// ---- Boot ----

async function boot() {
  syncAppHeight();
  try {
    const cfg = await fetch("/webui/config").then((r) => r.json());
    state.mode = cfg.mode || "bff";
  } catch (_) {
    state.mode = "bff";
  }
  if (state.mode === "direct") {
    try {
      state.token = localStorage.getItem(TOKEN_KEY);
    } catch (_) {}
  }
  try {
    await refreshAll();
    if (state.mode === "direct" && !state.token) openConnect();
  } catch (_) {}
  if (isMobile() && !state.selectedId) openDrawer();
  setInterval(refreshAll, POLL_MS);
}

boot();
