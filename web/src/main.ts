// Claude Commander web UI — served by claude-commander-server itself, so every
// call is same-origin.
//
// The browser holds the server's bearer token: api.ts sends it as a Bearer
// header on /api and terminal.ts as the in-band `auth` frame on the WS. It
// comes from a `#token=…` link fragment (stored, then stripped from the
// address bar), else from localStorage, else the connect screen. A server
// running without auth needs none; the connect screen only opens on a 401.

import { Api, Auth, act, type TokenSource, Unauthorized } from "./api.ts";
import {
  closeDrawer,
  closeModal,
  copyToClipboard,
  els,
  flashConn,
  h,
  isMobile,
  type MenuItem,
  openDrawer,
  openModal,
  setConn,
  wireContextMenu,
  wireModals,
} from "./dom.ts";
import { initForms, openNewSession, removeProject } from "./forms.ts";
import type { ProjectInfo, SessionInfo } from "./generated/index.ts";
import { NotifyTracker, requestPermissionOnFirstClick, showNotifications } from "./notify.ts";
import { Poller } from "./poll.ts";
import { initReview, openReview } from "./review.ts";
import { initSettings } from "./settings.ts";
import { agentStateFor, currentSession, humanize, state } from "./state.ts";
import { attach, detach, fitNow, initTerminal, reattach, resume, sendResize } from "./terminal.ts";
import { TokenStore, takeHashToken } from "./token.ts";
import { renderTree, type TreeHandlers, TreeView } from "./tree.ts";

const POLL_MS = 1500;

const tokens = new TokenStore();
const auth = new Auth();
const api = new Api(auth, {
  onUnauthorized: (message) => {
    tokens.set(null);
    openConnect(message);
  },
});
const notifier = new NotifyTracker();

// ---- connect -----------------------------------------------------------------

function showConnectError(errMsg: string | undefined): void {
  els.connectError.textContent = errMsg ?? "";
  els.connectError.classList.toggle("hidden", !errMsg);
}

function openConnect(errMsg: string | undefined): void {
  showConnectError(errMsg);
  openModal(els.connectModal);
  els.connectToken.focus();
}

function setToken(token: string, source: TokenSource): void {
  auth.set(token, source);
  tokens.set(token);
}

els.connectForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const token = els.connectToken.value.trim();
  if (!token) return;
  setToken(token, "submitted");
  // Validate by a real request. A 401 re-opens the connect screen with its
  // own message; anything else (server down, a 5xx) is said here, since the
  // token was not judged either way.
  showConnectError(undefined);
  try {
    await api.workspace();
  } catch (err) {
    if (!(err instanceof Unauthorized)) {
      const why = err instanceof Error ? err.message : String(err);
      showConnectError(`Could not check the token: ${why}`);
    }
    return;
  }
  closeModal(els.connectModal);
  els.connectToken.value = "";
  refreshAll();
  // An attach the old token lost (WS_ERR_AUTH) picks up where it stopped.
  resume();
});

// ---- polling -----------------------------------------------------------------

// One poll at a time (poll.ts): the next is scheduled when this one finishes,
// and a refresh requested mid-poll runs once more straight after it.
const poller = new Poller(poll, POLL_MS);

/** Whether the latest poll reached the server and was accepted. */
let lastPollOk = false;

/** Refresh now (a mutation's follow-up, the refresh button). */
function refreshAll(): Promise<void> {
  return poller.trigger();
}

async function poll(): Promise<void> {
  // The token was refused: every poll would only 401 again. Wait on the
  // connect screen, which sets a new token (and resumes) on submit.
  if (auth.rejected) return;
  try {
    const [ws, agents] = await Promise.all([
      api.workspace(),
      api.agentStates().catch(() => ({ states: {}, commander_running: false })),
    ]);
    state.sessions = ws.sessions ?? [];
    state.projects = ws.projects ?? [];
    state.agentStates = agents?.states ?? {};
    showNotifications(
      notifier.observe({ sessions: state.sessions, agentStates: state.agentStates }),
      state.sessions,
      selectSession,
    );
    setConn("ok", "connected");
    lastPollOk = true;
    render();
  } catch (e) {
    lastPollOk = false;
    if (!(e instanceof Unauthorized)) setConn("error", "disconnected");
  }
}

// ---- tree + toolbar ------------------------------------------------------------

const treeHandlers: TreeHandlers = {
  onSelect: (id) => selectSession(id),
  onToggleProject: (id) => {
    if (state.collapsed.has(id)) state.collapsed.delete(id);
    else state.collapsed.add(id);
    render();
  },
  onNewSession: (p) => openNewSession(p),
  onRemoveProject: (p) => removeProject(p),
  sessionMenu: sessionMenuItems,
  projectMenu: projectMenuItems,
};

const tree = new TreeView((model) => renderTree(els.tree, model, treeHandlers));

function render(): void {
  tree.update({
    projects: state.projects,
    sessions: state.sessions,
    agentStates: state.agentStates,
    selectedId: state.selectedId,
    collapsed: state.collapsed,
  });
  renderToolbar();
  if (state.showInfo) renderInfo();
}

function renderToolbar(): void {
  const sel = currentSession();
  const active = !!sel && sel.status !== "creating";
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
}

async function restartSession(s: SessionInfo): Promise<void> {
  if (!(await act(api.restartSession(s.id)))) return;
  // The old pane's session ended (a final close); attach to the new one.
  if (state.selectedId === s.id) reattach();
  refreshAll();
}

async function killSession(s: SessionInfo): Promise<void> {
  if (!confirm(`Kill session "${s.title}"?`)) return;
  if (await act(api.killSession(s.id))) refreshAll();
}

async function deleteSession(s: SessionInfo): Promise<void> {
  if (!confirm(`Delete session "${s.title}"? This removes its worktree.`)) return;
  if (!(await act(api.deleteSession(s.id)))) return;
  if (state.selectedId === s.id) {
    state.selectedId = null;
    detach();
    els.placeholder.style.display = "flex";
  }
  refreshAll();
}

function sessionMenuItems(s: SessionInfo): MenuItem[] {
  const items: MenuItem[] = [
    { type: "label", text: s.title },
    { text: "Open", onClick: () => selectSession(s.id) },
  ];
  if (s.status !== "creating") {
    items.push({ text: "Restart", onClick: () => restartSession(s) });
    items.push({ text: "Kill", onClick: () => killSession(s) });
  }
  items.push({ type: "sep" });
  items.push({ text: "Delete", danger: true, onClick: () => deleteSession(s) });
  return items;
}

function projectMenuItems(p: ProjectInfo): MenuItem[] {
  return [
    { type: "label", text: p.name },
    { text: "New session here", onClick: () => openNewSession(p) },
    { text: "Copy repo path", onClick: () => copyToClipboard(p.repo_path, "Path copied") },
    { type: "sep" },
    { text: "Remove project", danger: true, onClick: () => removeProject(p) },
  ];
}

function selectSession(id: string): void {
  if (state.selectedId === id) return;
  state.selectedId = id;
  render();
  els.placeholder.style.display = "none";
  if (isMobile()) closeDrawer();
  attach(id);
}

// ---- info panel ----------------------------------------------------------------

function toggleInfo(): void {
  state.showInfo = !state.showInfo;
  els.infoPanel.classList.toggle("hidden", !state.showInfo);
  if (state.showInfo) renderInfo();
  fitNow();
  sendResize();
}

function renderInfo(): void {
  const s = currentSession();
  els.infoList.replaceChildren();
  if (!s) return;
  const rows: [string, string][] = [
    ["Title", s.title],
    ["Project", s.project_name],
    ["Branch", s.branch],
    ["Status", s.status],
    ["Agent", humanize(agentStateFor(state.agentStates, s))],
    ["Program", s.program],
    ["PR", s.pr_number ? `#${s.pr_number} (${s.pr_state})` : "—"],
    ["Section", s.current_section || "—"],
    ["Created", s.created_at ? new Date(s.created_at).toLocaleString() : "—"],
    ["ID", s.id],
  ];
  for (const [k, v] of rows) els.infoList.append(h("dt", { text: k }), h("dd", { text: v }));
}

// ---- viewport sizing (mobile keyboard / dictation) -----------------------------

function syncAppHeight(): void {
  const vh = window.visualViewport ? window.visualViewport.height : window.innerHeight;
  document.documentElement.style.setProperty("--app-vh", `${Math.round(vh)}px`);
}

function handleViewportChange(): void {
  syncAppHeight();
  fitNow();
  sendResize();
}

// ---- wiring --------------------------------------------------------------------

function wire(): void {
  wireModals();
  wireContextMenu();
  requestPermissionOnFirstClick();
  initTerminal(auth, { onAuthRejected: (token) => api.unauthorized(token) });
  initReview(api);
  initSettings(api);
  initForms(api, { refresh: refreshAll, select: selectSession });

  window.addEventListener("resize", handleViewportChange);
  window.visualViewport?.addEventListener("resize", handleViewportChange);
  window.visualViewport?.addEventListener("scroll", handleViewportChange);

  els.menuBtn.addEventListener("click", () => {
    if (document.body.classList.contains("drawer-open")) closeDrawer();
    else openDrawer();
  });
  els.backdrop.addEventListener("click", closeDrawer);

  els.refresh.addEventListener("click", () => refreshAll());
  els.infoBtn.addEventListener("click", toggleInfo);
  els.restart.addEventListener("click", () => {
    const s = currentSession();
    if (s) restartSession(s);
  });
  els.kill.addEventListener("click", () => {
    const s = currentSession();
    if (s) killSession(s);
  });
  els.delete.addEventListener("click", () => {
    const s = currentSession();
    if (s) deleteSession(s);
  });
  els.reviewBtn.addEventListener("click", () => {
    const s = currentSession();
    if (s) openReview(s);
  });
}

// ---- boot ----------------------------------------------------------------------

async function boot(): Promise<void> {
  syncAppHeight();
  wire();
  // A `#token=` link wins over a stored token (token.ts says why), and says
  // so once the server has accepted it.
  const linked = takeHashToken(location, history, tokens);
  if (linked) auth.set(linked.token, "hash");
  else auth.set(tokens.get(), "stored");
  // A 401 here opens the connect screen (Api's onUnauthorized).
  await poller.start();
  if (linked && lastPollOk) {
    const text = linked.replacedOther
      ? "using the link's token (replaced the saved one)"
      : "connected with the link's token";
    flashConn(text, 4000);
  }
  if (isMobile() && !state.selectedId) openDrawer();
}

boot();
