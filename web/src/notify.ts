// Desktop notifications, while the page is open but backgrounded (e.g. another
// tab). `NotifyTracker` decides *what* changed between two polls — pure, so
// `node --test` covers it; `showNotifications` is the browser side.

import { COMMANDER_SENTINEL_ID } from "./generated/constants.ts";
import type { AgentState, SessionId, SessionInfo } from "./generated/index.ts";
import { agentStateFor } from "./state.ts";

export type NotifyKind = "needs_input" | "finished";

export interface NotifyEvent {
  /** The session's id. */
  key: string;
  kind: NotifyKind;
}

export interface Poll {
  sessions: readonly SessionInfo[];
  agentStates: Partial<Record<SessionId, AgentState>>;
}

interface Seen {
  unread: boolean;
  agent: AgentState;
}

/**
 * Edge-triggered: each transition is reported once, on the poll that first
 * shows it. The first poll only records a baseline, so loading the page never
 * notifies for states that already held — and likewise a session's first
 * appearance only baselines it.
 *
 * - **finished** is `SessionInfo.unread` going false → true: the server's own
 *   record that the agent finished and nobody has looked since. A polled
 *   `working → idle` is not used — a 1.5 s sample of the pane detector can
 *   both miss that edge and invent one from a flap.
 * - **needs input** is the agent state entering `waiting_for_input`.
 *
 * Only real sessions are tracked: the commander's sentinel entry in
 * `AgentStatesSnapshot.states` has no session and never notifies.
 */
export class NotifyTracker {
  private prev: Map<string, Seen> | null = null;

  observe(poll: Poll): NotifyEvent[] {
    const next = new Map<string, Seen>();
    for (const s of poll.sessions) {
      if (s.id === COMMANDER_SENTINEL_ID || s.session_id === COMMANDER_SENTINEL_ID) continue;
      next.set(s.id, { unread: s.unread, agent: agentStateFor(poll.agentStates, s) });
    }
    const prev = this.prev;
    this.prev = next;
    if (!prev) return [];

    const events: NotifyEvent[] = [];
    for (const [key, now] of next) {
      const before = prev.get(key);
      if (!before) continue;
      if (now.agent === "waiting_for_input" && before.agent !== "waiting_for_input") {
        events.push({ key, kind: "needs_input" });
      } else if (now.unread && !before.unread) {
        events.push({ key, kind: "finished" });
      }
    }
    return events;
  }
}

const MESSAGES: Record<NotifyKind, string> = {
  needs_input: "needs your input",
  finished: "finished",
};

/** Browsers only allow asking from a user gesture, so ask on the first click. */
export function requestPermissionOnFirstClick(): void {
  document.addEventListener(
    "click",
    () => {
      if ("Notification" in window && Notification.permission === "default") {
        Notification.requestPermission().catch(() => {});
      }
    },
    { once: true },
  );
}

/**
 * Raise a notification per event — only while this tab is hidden (if it is
 * the active tab the change is already on screen) and permission is granted.
 */
export function showNotifications(
  events: readonly NotifyEvent[],
  sessions: readonly SessionInfo[],
  onOpen: (id: string) => void,
): void {
  if (events.length === 0) return;
  if (!("Notification" in window) || Notification.permission !== "granted") return;
  if (document.visibilityState !== "hidden") return;
  for (const { key: id, kind } of events) {
    const s = sessions.find((x) => x.id === id);
    const n = new Notification(`${s ? s.title : "Session"} — ${MESSAGES[kind]}`, {
      body: s ? `${s.project_name} · ${s.branch}` : "",
      tag: id, // replaces an earlier notification for the same session
      // @ts-expect-error `renotify` is in the spec and Chromium, not yet in lib.dom
      renotify: true,
    });
    n.onclick = () => {
      window.focus();
      if (s) onOpen(id);
      n.close();
    };
  }
}
