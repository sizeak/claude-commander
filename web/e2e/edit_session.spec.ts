import type { CreatedId, SessionId } from "../src/generated/index.ts";
import {
  api,
  connect,
  expect,
  openSession,
  PROJECT,
  paneText,
  sessionByTitle,
  test,
  workspace,
} from "./support";

for (const restart of [false, true]) {
  test(`edit session: program change restart=${restart}`, async ({ page, request }) => {
    const ws = await workspace(request);
    const project = ws.projects.find((p) => p.name === PROJECT);
    if (!project) throw new Error("Fixture project missing");
    const title = `edit-${restart}-${Date.now()}`;
    const created = await api<CreatedId<SessionId>>(request, "POST", "/sessions", {
      project_path: project.repo_path,
      title,
      program: "cat",
    });
    try {
      await connect(page);
      await openSession(page, title);
      await page.locator("#edit-session-btn").click();
      const modal = page.locator("#edit-modal");
      await expect(page.locator("#edit-title")).toHaveValue(title);
      await expect(page.locator("#edit-program")).toHaveValue("cat");
      await page.locator("#edit-title").fill(`${title}-renamed`);
      await page.locator("#edit-program").fill("sleep 60");
      await modal.getByRole("button", { name: "Save", exact: true }).click();
      await expect(page.locator("#edit-heading")).toHaveText("Restart session?");
      expect((await sessionByTitle(request, title)).program).toBe("cat");
      await page.locator(restart ? "#edit-now" : "#edit-later").click();
      await expect(modal).toBeHidden();
      const edited = await sessionByTitle(request, `${title}-renamed`);
      expect(edited.program).toBe(restart ? "sleep 60" : "cat");
      expect(edited.pending_program).toBe(restart ? null : "sleep 60");
      expect(edited.status).toBe("running");
      if (!restart) {
        // cat echoes text only while the original pane remains running.
        await page.locator("#terminal").click();
        await page.keyboard.type("still-running");
        await page.keyboard.press("Enter");
        await expect.poll(() => paneText(request, created.id)).toContain("still-running");
        await page.locator("#restart-btn").click();
      }
    } finally {
      await api(request, "DELETE", `/sessions/${created.id}`);
    }
  });
}
