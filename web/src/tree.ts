// The sidebar's project → session tree.
//
// `TreeView` decides whether a poll needs a re-render (pure, tested);
// `renderTree` builds the DOM from a `TreeModel`.

import { clear, h, type MenuItem, showContextMenu } from "./dom.ts";
import type {
  AgentState,
  ProjectInfo,
  SessionId,
  SessionInfo,
  SessionStatus,
} from "./generated/index.ts";
import { agentStateFor, humanize } from "./state.ts";

/** Everything the tree renders — and nothing else. */
export interface TreeModel {
  projects: readonly ProjectInfo[];
  sessions: readonly SessionInfo[];
  agentStates: Partial<Record<SessionId, AgentState>>;
  selectedId: string | null;
  collapsed: ReadonlySet<string>;
}

export interface TreeHandlers {
  onSelect(id: string): void;
  onToggleProject(id: string): void;
  onNewSession(p: ProjectInfo): void;
  onRemoveProject(p: ProjectInfo): void;
  sessionMenu(s: SessionInfo): MenuItem[];
  projectMenu(p: ProjectInfo): MenuItem[];
}

/** Sessions grouped by project id, in snapshot order. */
export function groupByProject(sessions: readonly SessionInfo[]): Map<string, SessionInfo[]> {
  const by = new Map<string, SessionInfo[]>();
  for (const s of sessions) {
    const list = by.get(s.project_id);
    if (list) list.push(s);
    else by.set(s.project_id, [s]);
  }
  return by;
}

/** Renders the tree when its model changes. */
export class TreeView {
  private readonly render: (model: TreeModel) => void;

  constructor(render: (model: TreeModel) => void) {
    this.render = render;
  }

  /** Returns whether it re-rendered. */
  update(model: TreeModel): boolean {
    this.render(model);
    return true;
  }
}

// ---- DOM ---------------------------------------------------------------------

function statusBadge(status: SessionStatus): HTMLElement {
  return h("span", { className: `badge ${status}`, text: humanize(status) });
}

function agentBadge(st: AgentState): HTMLElement {
  return h("span", { className: `agent ${st}` }, h("span", { className: "dot" }), humanize(st));
}

function sessionRow(s: SessionInfo, model: TreeModel, on: TreeHandlers): HTMLElement {
  const li = h(
    "li",
    { className: `session${s.id === model.selectedId ? " active" : ""}` },
    h(
      "div",
      { className: "session-row" },
      h("span", { className: "session-name", text: s.title }),
      statusBadge(s.status),
    ),
    h("div", {
      className: "session-meta",
      text: `${s.branch}${s.pr_number ? ` · PR #${s.pr_number}` : ""}`,
    }),
    agentBadge(agentStateFor(model.agentStates, s)),
  );
  li.dataset.id = s.id;
  li.addEventListener("click", () => on.onSelect(s.id));
  li.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    showContextMenu(e, on.sessionMenu(s));
  });
  return li;
}

function iconButton(className: string, text: string, title: string, onClick: () => void) {
  const btn = h("button", { className: `icon-btn ${className}`, text, title });
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    onClick();
  });
  return btn;
}

function projectGroup(
  p: ProjectInfo,
  sessions: readonly SessionInfo[],
  model: TreeModel,
  on: TreeHandlers,
): HTMLElement {
  const collapsed = model.collapsed.has(p.id);
  const name = h("span", { className: "pname", text: p.name, title: p.repo_path });
  const header = h(
    "div",
    { className: "project-header" },
    h("span", { className: "twisty", text: collapsed ? "▶" : "▼" }),
    name,
    h("span", { className: "pcount", text: sessions.length ? String(sessions.length) : "" }),
    h(
      "span",
      { className: "phover" },
      iconButton("add", "＋", `New session in ${p.name}`, () => on.onNewSession(p)),
      iconButton("del", "✕", `Remove ${p.name}`, () => on.onRemoveProject(p)),
    ),
  );
  header.addEventListener("click", () => on.onToggleProject(p.id));
  header.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    showContextMenu(e, on.projectMenu(p));
  });

  const group = h("div", { className: "project-group" }, header);
  if (!collapsed) {
    const ul = h("ul", { className: "project-sessions" });
    if (sessions.length === 0) ul.append(h("li", { className: "empty", text: "no sessions" }));
    else for (const s of sessions) ul.append(sessionRow(s, model, on));
    group.append(ul);
  }
  return group;
}

export function renderTree(container: HTMLElement, model: TreeModel, on: TreeHandlers): void {
  clear(container);
  if (model.projects.length === 0) {
    container.append(
      h("div", {
        className: "tree-empty",
        text: "No projects yet. Use ＋ add to register a repo.",
      }),
    );
  }
  const byProject = groupByProject(model.sessions);
  for (const p of model.projects) {
    container.append(projectGroup(p, byProject.get(p.id) ?? [], model, on));
  }
}
