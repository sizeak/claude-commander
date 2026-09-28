import assert from "node:assert/strict";
import { describe, test } from "node:test";
import type { ProjectInfo, SessionInfo } from "../src/generated/index.ts";
import { groupByProject, type TreeModel, TreeView } from "../src/tree.ts";

const project = { id: "p1", name: "alpha", repo_path: "/r/alpha" } as ProjectInfo;
const session = (id: string, extra: Partial<SessionInfo> = {}) =>
  ({ id, session_id: id, project_id: "p1", title: id, status: "running", ...extra }) as SessionInfo;

// What each poll hands the view: freshly parsed JSON, so equal data arrives
// as new objects every time.
function model(over: Partial<TreeModel> = {}): TreeModel {
  return {
    projects: [structuredClone(project)],
    sessions: [session("a"), session("b")],
    agentStates: { a: "idle" },
    selectedId: null,
    collapsed: new Set(),
    ...over,
  };
}

function counting() {
  const renders: TreeModel[] = [];
  return { view: new TreeView((m) => renders.push(m)), renders };
}

describe("TreeView", () => {
  test("renders the first model", () => {
    const { view, renders } = counting();
    assert.equal(view.update(model()), true);
    assert.equal(renders.length, 1);
  });

  test("an unchanged poll does not re-render", () => {
    const { view, renders } = counting();
    view.update(model());
    assert.equal(view.update(model()), false);
    assert.equal(view.update(model()), false);
    assert.equal(renders.length, 1);
  });

  test("any rendered change re-renders", () => {
    const changes: Partial<TreeModel>[] = [
      { sessions: [session("a", { status: "stopped" }), session("b")] },
      { sessions: [session("a")] },
      { agentStates: { a: "working" } },
      { selectedId: "b" },
      { collapsed: new Set(["p1"]) },
      { projects: [] },
    ];
    for (const change of changes) {
      const { view, renders } = counting();
      view.update(model());
      assert.equal(view.update(model(change)), true, JSON.stringify(change));
      assert.equal(renders.length, 2);
    }
  });
});

describe("groupByProject", () => {
  test("groups in snapshot order", () => {
    const by = groupByProject([session("a"), session("x", { project_id: "p2" }), session("b")]);
    assert.deepEqual(
      [...by].map(([p, ss]) => [p, ss.map((s) => s.id)]),
      [
        ["p1", ["a", "b"]],
        ["p2", ["x"]],
      ],
    );
  });
});
