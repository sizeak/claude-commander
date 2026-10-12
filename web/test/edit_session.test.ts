import assert from "node:assert/strict";
import { test } from "node:test";
import { programChoices, sessionEdit } from "../src/edit_session.ts";
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

test("program choices show aliases, deduplicate commands and retain an unconfigured current command", () => {
  const configured = [
    { label: "Claude alias", command: "claude --model opus" },
    { label: "Another alias", command: "claude --model opus" },
    { label: "Codex alias", command: "codex --sandbox workspace-write" },
  ];
  const known = programChoices(configured, "claude --model opus");
  assert.equal(known.length, 2);
  assert.deepEqual(known[0], configured[0]);
  const custom = programChoices(configured, "custom-agent --flag");
  assert.deepEqual(custom.at(-1), { label: "custom-agent --flag", command: "custom-agent --flag" });
  assert.deepEqual(programChoices([], "claude"), [{ label: "claude", command: "claude" }]);
});

test("pending changes retain an unconfigured live program so they can be cancelled", () => {
  const choices = programChoices(
    [{ label: "Codex alias", command: "codex" }],
    "codex",
    "custom-agent --flag",
  );
  assert.deepEqual(
    choices.map((p) => p.command),
    ["codex", "custom-agent --flag"],
  );
  assert.deepEqual(
    programChoices([], "codex", "custom-agent --flag").map((p) => p.command),
    ["codex", "custom-agent --flag"],
  );
});
