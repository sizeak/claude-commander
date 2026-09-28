import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { WS_ERR_AUTH, WS_ERR_NO_SESSION } from "../src/generated/constants.ts";
import {
  AttachLifecycle,
  attachFrame,
  parseControl,
  RECONNECT_MAX_MS,
  wsAttachUrl,
} from "../src/ws.ts";

describe("attach frames", () => {
  test("the URL follows the page's scheme", () => {
    assert.equal(wsAttachUrl({ protocol: "http:", host: "h:1" }), "ws://h:1/ws/attach");
    assert.equal(wsAttachUrl({ protocol: "https:", host: "h" }), "wss://h/ws/attach");
  });

  test("the agent pane is the wire default and is omitted", () => {
    assert.deepEqual(attachFrame("s1", "agent", { cols: 80, rows: 24 }), {
      type: "attach",
      session_id: "s1",
      cols: 80,
      rows: 24,
    });
  });

  test("a shell attach names its kind; an unmeasured terminal sends no size", () => {
    assert.deepEqual(attachFrame("s1", "shell", null), {
      type: "attach",
      session_id: "s1",
      kind: "shell",
    });
    assert.deepEqual(attachFrame("s1", "agent", { cols: 0, rows: 0 }), {
      type: "attach",
      session_id: "s1",
    });
  });

  test("parseControl rejects what is not a control frame", () => {
    assert.deepEqual(parseControl('{"type":"ready","session":"x"}'), {
      type: "ready",
      session: "x",
    });
    assert.equal(parseControl("not json"), null);
    assert.equal(parseControl("42"), null);
    assert.equal(parseControl("null"), null);
  });
});

describe("AttachLifecycle: what a close means", () => {
  const error = (message: string) => ({ type: "error", message }) as const;

  test("an auth error goes to the connect screen and never retries", () => {
    const l = new AttachLifecycle();
    l.onControl(error(WS_ERR_AUTH));
    assert.deepEqual(l.onClose(), { kind: "auth" });
  });

  test("an unknown session is final and says so", () => {
    for (const message of [WS_ERR_NO_SESSION, `${WS_ERR_NO_SESSION}: abc123`]) {
      const l = new AttachLifecycle();
      l.onControl(error(message));
      assert.deepEqual(l.onClose(), { kind: "gone", message });
    }
  });

  test("any other close reconnects, backing off to a cap", () => {
    const l = new AttachLifecycle();
    const delays = Array.from({ length: 10 }, () => {
      const d = l.onClose();
      assert.equal(d.kind, "reconnect");
      return d.kind === "reconnect" ? d.delayMs : -1;
    });
    for (let i = 1; i < delays.length; i++) {
      assert.ok((delays[i] ?? 0) >= (delays[i - 1] ?? 0), `${delays}`);
    }
    assert.ok((delays[1] ?? 0) > (delays[0] ?? 0), `${delays}`);
    assert.equal(delays.at(-1), RECONNECT_MAX_MS);
  });

  test("another error frame still reconnects", () => {
    const l = new AttachLifecycle();
    l.onControl(error("attach failed: tmux exited"));
    assert.equal(l.onClose().kind, "reconnect");
  });

  test("a successful attach resets the backoff", () => {
    const l = new AttachLifecycle();
    const first = l.onClose();
    l.onClose();
    l.onClose();
    l.onControl({ type: "ready", session: "s1" });
    assert.deepEqual(l.onClose(), first);
  });
});
