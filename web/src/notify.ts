// Desktop notifications, while the page is open but backgrounded (e.g. another
// tab). `NotifyTracker` decides *what* changed between two polls — pure, so
// `node --test` covers it; `showNotifications` is the browser side.

import type { AgentState, SessionId, SessionInfo } from "./generated/index.ts";

export type NotifyKind = "needs_input" | "finished";

export interface NotifyEvent {
  /** The key the transition was observed under (a session id). */
  key: string;
  kind: NotifyKind;
}

export interface Poll {
  sessions: readonly SessionInfo[];
  agentStates: Partial<Record<SessionId, AgentState>>;
}

/**
 * Edge-triggered: each transition is reported once, on the poll that first
 * shows it. The first poll only records a baseline, so loading the page never
 * notifies for states that already held.
 */
export class NotifyTracker {
  private prev: Poll | null = null;

  observe(poll: Poll): NotifyEvent[] {
    const prev = this.prev;
    this.prev = { sessions: poll.sessions, agentStates: { ...poll.agentStates } };
    if (!prev) return [];
    const events: NotifyEvent[] = [];
    for (const [key, st] of Object.entries(poll.agentStates)) {
      const before = prev.agentStates[key];
      if (st === before) continue;
      if (st === "waiting_for_input") events.push({ key, kind: "needs_input" });
      else if (st === "idle" && before === "working") events.push({ key, kind: "finished" });
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
  for (const { key, kind } of events) {
    const s = sessions.find((x) => x.id === key || x.session_id === key);
    const id = s ? s.id : key;
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
