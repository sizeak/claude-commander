import assert from "node:assert/strict";
import { test } from "node:test";
import { sessionEdit } from "../src/edit_session.ts";
import type { SessionInfo } from "../src/generated/index.ts";

const info = { program: "claude", stack_parent_session_id: "parent" } as SessionInfo;
test("session editor trims settings and preserves the base unless it changed", () => {
  const edit = sessionEdit(info, " renamed ", " codex ", "Review", true, "parent");
  assert.deepEqual(edit, {
    title: "renamed",
    program: "codex",
    section: "Review",
    keep_alive: true,
    restart: false,
    base: null,
  });
});
test("automatic section and unstacking are explicit clears", () => {
  const edit = sessionEdit(info, "name", "claude", "", false, "");
  assert.equal(edit.section, null);
  assert.deepEqual(edit.base, { parent_session_id: null });
});
