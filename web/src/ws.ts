// The /ws/attach protocol, minus the socket: frame builders, control-frame
// parsing, and `AttachLifecycle`, which decides what a close means. Pure, so
// `node --test` covers it; terminal.ts owns the actual WebSocket.
//
// Binary frames are raw PTY bytes; text frames are JSON `ClientControl` (sent)
// and `ServerControl` (received).

import type { AttachKind, ClientControl, ServerControl } from "./generated/index.ts";

export function wsAttachUrl(loc: Pick<Location, "protocol" | "host">): string {
  const proto = loc.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${loc.host}/ws/attach`;
}

export function authFrame(token: string): ClientControl {
  return { type: "auth", token };
}

/**
 * The attach handshake. It carries the terminal's size so the server spawns
 * `tmux attach` at the right geometry: sizing with a resize *after* it lets
 * tmux paint an 80x24 screen first, which xterm reflows and tmux's incremental
 * repaint never clears (#281).
 */
export function attachFrame(
  sessionId: string,
  kind: AttachKind,
  size: { cols: number; rows: number } | null,
): ClientControl {
  const frame: Extract<ClientControl, { type: "attach" }> = {
    type: "attach",
    session_id: sessionId,
  };
  // `agent` is the default and omitted on the wire (protocol's AttachKind).
  if (kind === "shell") frame.kind = "shell";
  if (size?.cols && size.rows) {
    frame.cols = size.cols;
    frame.rows = size.rows;
  }
  return frame;
}

export function resizeFrame(cols: number, rows: number): ClientControl {
  return { type: "resize", cols, rows };
}

export function parseControl(text: string): ServerControl | null {
  try {
    const msg = JSON.parse(text) as ServerControl;
    return msg && typeof msg === "object" && typeof msg.type === "string" ? msg : null;
  } catch {
    return null;
  }
}

/** What a close of the attach socket should lead to. */
export type CloseDecision = { kind: "reconnect"; delayMs: number };

export const RECONNECT_MS = 1500;

/** Per-socket attach state: fed each control frame, asked what a close means. */
export class AttachLifecycle {
  /** A line to show in the terminal for this control frame, if any. */
  onControl(msg: ServerControl): string | null {
    switch (msg.type) {
      case "detached":
        return `\r\n\x1b[90m[detached: ${msg.reason ?? ""}]\x1b[0m\r\n`;
      case "error":
        return `\r\n\x1b[91m[error: ${msg.message ?? ""}]\x1b[0m\r\n`;
      default:
        // "ready" needs no action; the pane streams in over binary frames.
        return null;
    }
  }

  onClose(): CloseDecision {
    return { kind: "reconnect", delayMs: RECONNECT_MS };
  }
}
