import assert from "node:assert/strict";
import { describe, test } from "node:test";
import { ConnStatus } from "../src/status.ts";

describe("ConnStatus: what the header shows", () => {
  test("the poll's state shows when nothing else is set", () => {
    const s = new ConnStatus();
    s.setBase("ok", "connected");
    assert.deepEqual(s.view(), { cls: "ok", text: "connected" });
    s.setBase("error", "disconnected");
    assert.deepEqual(s.view(), { cls: "error", text: "disconnected" });
  });

  test("a terminal state survives the next poll until cleared", () => {
    const s = new ConnStatus();
    s.setSticky("error", "session ended");
    // The next poll succeeding must not paper over it.
    s.setBase("ok", "connected");
    assert.deepEqual(s.view(), { cls: "error", text: "session ended" });
    s.clearSticky();
    assert.deepEqual(s.view(), { cls: "ok", text: "connected" });
  });

  test("losing the server outranks a sticky terminal state", () => {
    // "session ended" is about one pane; "disconnected" is about everything,
    // and hiding it behind the pane's state would claim a server we can't reach.
    const s = new ConnStatus();
    s.setSticky("error", "session ended");
    s.setBase("error", "disconnected");
    assert.deepEqual(s.view(), { cls: "error", text: "disconnected" });
    s.setBase("ok", "connected");
    assert.deepEqual(s.view(), { cls: "error", text: "session ended" });
  });

  test("a flash wins while it lasts, then gives way to what is underneath", () => {
    const s = new ConnStatus();
    s.setBase("ok", "connected");
    const f = s.flash("ok", "copied");
    s.setBase("ok", "connected"); // a poll landing mid-flash
    assert.deepEqual(s.view(), { cls: "ok", text: "copied" });
    s.endFlash(f);
    assert.deepEqual(s.view(), { cls: "ok", text: "connected" });

    // Underneath can be a sticky state, not only "connected".
    s.setSticky("error", "gone");
    const g = s.flash("ok", "copied");
    s.endFlash(g);
    assert.deepEqual(s.view(), { cls: "error", text: "gone" });
  });

  test("ending an older flash leaves a newer one showing", () => {
    const s = new ConnStatus();
    const a = s.flash("ok", "first");
    s.flash("error", "second");
    s.endFlash(a);
    assert.deepEqual(s.view(), { cls: "error", text: "second" });
  });
});
