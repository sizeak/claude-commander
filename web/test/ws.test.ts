import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { attachFrame, parseControl, wsAttachUrl } from "../src/ws.ts";

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
