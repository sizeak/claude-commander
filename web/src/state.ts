// The page's shared, in-memory state: the last polled workspace plus UI
// selections. Pure data — no DOM — so the logic modules can take it in tests.

import type {
  AgentState,
  CreateOptions,
  ProjectInfo,
  SessionId,
  SessionInfo,
} from "./generated/index.ts";

export interface AppState {
  sessions: SessionInfo[];
  projects: ProjectInfo[];
  /** `AgentStatesSnapshot.states` from the last poll. */
  agentStates: Partial<Record<SessionId, AgentState>>;
  createOptions: CreateOptions | null;
  selectedId: string | null;
  collapsed: Set<string>;
  showInfo: boolean;
}

export const state: AppState = {
  sessions: [],
  projects: [],
  agentStates: {},
  createOptions: null,
  selectedId: null,
  collapsed: new Set(),
  showInfo: false,
};

export function agentStateFor(
  agentStates: AppState["agentStates"],
  s: Pick<SessionInfo, "id" | "session_id">,
): AgentState {
  return agentStates[s.id] ?? agentStates[s.session_id] ?? "unknown";
}

export function currentSession(): SessionInfo | null {
  return state.sessions.find((s) => s.id === state.selectedId) ?? null;
}

/** "waiting_for_input" → "waiting for input". */
export function humanize(s: string): string {
  return s.replace(/_/g, " ");
}
