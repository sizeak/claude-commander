// The /ws/attach protocol, minus the socket: frame builders, control-frame
// parsing, and `AttachLifecycle`, which decides what a close means. Pure, so
// `node --test` covers it; terminal.ts owns the actual WebSocket.
//
// Binary frames are raw PTY bytes; text frames are JSON `ClientControl` (sent)
// and `ServerControl` (received).

import { WS_ERR_AUTH, WS_ERR_NO_SESSION } from "./generated/constants.ts";
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

/**
 * How to read a `ServerControl.error` message. The two final ones are
 * protocol constants (`ws::WS_ERR_AUTH`, `ws::WS_ERR_NO_SESSION` — the latter
 * may carry a `: detail` suffix, which the Rust client's `handshake_error`
 * accepts too); anything else is transient.
 */
export function classifyError(message: string): "auth" | "no_session" | "other" {
  if (message === WS_ERR_AUTH) return "auth";
  if (message === WS_ERR_NO_SESSION || message.startsWith(`${WS_ERR_NO_SESSION}: `)) {
    return "no_session";
  }
  return "other";
}

/** What a close of the attach socket should lead to. */
export type CloseDecision =
  /** Transient (network blip, server restart, backgrounded tab): try again. */
  | { kind: "reconnect"; delayMs: number }
  /** The token was rejected: back to the connect screen; retrying can't help. */
  | { kind: "auth" }
  /** The session does not exist: say so; retrying can't help. */
  | { kind: "gone"; message: string };

export const RECONNECT_BASE_MS = 1000;
export const RECONNECT_MAX_MS = 15_000;

/**
 * One attach — a session + pane — across however many sockets it takes. Fed
 * each control frame, asked on every close what to do next. Reconnects back
 * off exponentially (capped) until an attach succeeds (`ready`), so a server
 * that is down is not hammered at a fixed rate.
 */
export class AttachLifecycle {
  private failures = 0;
  private final: CloseDecision | null = null;

  /** A line to show in the terminal for this control frame, if any. */
  onControl(msg: ServerControl): string | null {
    switch (msg.type) {
      case "ready":
        // The pane streams in over binary frames; nothing to show.
        this.failures = 0;
        return null;
      case "detached":
        return `\r\n\x1b[90m[detached: ${msg.reason ?? ""}]\x1b[0m\r\n`;
      case "error": {
        const message = msg.message ?? "";
        const kind = classifyError(message);
        if (kind === "auth") this.final = { kind: "auth" };
        else if (kind === "no_session") this.final = { kind: "gone", message };
        return `\r\n\x1b[91m[error: ${message}]\x1b[0m\r\n`;
      }
      default:
        return null;
    }
  }

  onClose(): CloseDecision {
    if (this.final) return this.final;
    const delayMs = Math.min(RECONNECT_BASE_MS * 2 ** this.failures, RECONNECT_MAX_MS);
    this.failures++;
    return { kind: "reconnect", delayMs };
  }
}
