import assert from "node:assert/strict";
import { describe, test } from "node:test";
import type { DiffLine } from "../src/generated/index.ts";
import { displayPath, lineAnchor } from "../src/review.ts";

const line = (over: Partial<DiffLine>): DiffLine =>
  ({ origin: "context", old_lineno: null, new_lineno: null, content: "", ...over }) as DiffLine;

describe("displayPath", () => {
  test("a deleted file is known by its old path, everything else by its new one", () => {
    assert.equal(displayPath({ status: "deleted", old_path: "a.rs", new_path: "" }), "a.rs");
    assert.equal(displayPath({ status: "added", old_path: "", new_path: "b.rs" }), "b.rs");
    assert.equal(displayPath({ status: "modified", old_path: "c.rs", new_path: "c.rs" }), "c.rs");
    assert.equal(
      displayPath({ status: "renamed", old_path: "old.rs", new_path: "new.rs" }),
      "new.rs",
    );
  });
});

describe("lineAnchor", () => {
  test("a deletion is anchored on the old side", () => {
    assert.deepEqual(lineAnchor(line({ origin: "deletion", old_lineno: 7 })), {
      side: "old",
      lineno: 7,
    });
  });

  test("an addition or context line is anchored on the new side", () => {
    assert.deepEqual(lineAnchor(line({ origin: "addition", new_lineno: 3 })), {
      side: "new",
      lineno: 3,
    });
    assert.deepEqual(lineAnchor(line({ origin: "context", old_lineno: 4, new_lineno: 5 })), {
      side: "new",
      lineno: 5,
    });
  });

  test("a line with no number on its side has no anchor", () => {
    assert.equal(lineAnchor(line({ origin: "deletion", new_lineno: 2 })), null);
    assert.equal(lineAnchor(line({ origin: "addition" })), null);
  });
});
