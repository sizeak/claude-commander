import {
  connect,
  expect,
  LIFECYCLE_SESSION,
  openSession,
  PROJECT,
  sessionByTitle,
  sessionRow,
  test,
  workspace,
} from "./support";

test.describe("session lifecycle", () => {
  test.beforeEach(async ({ page }) => {
    await connect(page);
  });

  test("kill then restart a session", async ({ page, request, dialogs }) => {
    await openSession(page, LIFECYCLE_SESSION);
    const badge = sessionRow(page, LIFECYCLE_SESSION).locator(".badge");
    await expect(badge).toHaveText("running");

    await page.locator("#kill-btn").click();
    expect(dialogs).toContainEqual(expect.stringContaining(`Kill session "${LIFECYCLE_SESSION}"`));
    await expect(badge).toHaveText("stopped");
    expect((await sessionByTitle(request, LIFECYCLE_SESSION)).status).toBe("stopped");

    await page.locator("#restart-btn").click();
    await expect(badge).toHaveText("running");
    expect((await sessionByTitle(request, LIFECYCLE_SESSION)).status).toBe("running");
    expect(dialogs.filter((d) => d.startsWith("Action failed"))).toEqual([]);
  });

  test("create a session from the project, then delete it", async ({ page, request, dialogs }) => {
    const title = `e2e-created-${Date.now()}`;

    await page.getByTitle(`New session in ${PROJECT}`).click();
    const modal = page.locator("#new-modal");
    await expect(modal).toBeVisible();
    await expect(page.locator("#new-project-name")).toHaveValue(PROJECT);
    await page.locator("#new-title").fill(title);
    await modal.getByRole("button", { name: "Create" }).click();

    await expect(modal).toBeHidden();
    await expect(sessionRow(page, title)).toBeVisible();
    // A successful create selects (and attaches to) the new session.
    await expect(page.locator("#session-title")).toHaveText(title);
    const created = await sessionByTitle(request, title);
    expect(created.status).toBe("running");

    await page.locator("#delete-btn").click();
    expect(dialogs).toContainEqual(expect.stringContaining(`Delete session "${title}"`));
    await expect(sessionRow(page, title)).toHaveCount(0);
    await expect(page.locator("#session-title")).toHaveText("Select a session");
    await expect(page.locator("#terminal-placeholder")).toBeVisible();
    const ws = await workspace(request);
    expect(ws.sessions.map((s) => s.title)).not.toContain(title);
    expect(dialogs.filter((d) => d.startsWith("Action failed"))).toEqual([]);
  });
});
