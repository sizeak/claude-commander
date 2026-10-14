import {
  api,
  connect,
  expect,
  openSession,
  REVIEW_FILE,
  REVIEW_SESSION,
  sessionByTitle,
  test,
} from "./support";

interface Snapshot {
  reviewed: string[];
  comments: { file: string; side: string; line_range: [number, number]; comment: string }[];
}

test.describe("review view", () => {
  test.beforeEach(async ({ page }) => {
    await connect(page);
    await openSession(page, REVIEW_SESSION);
    await page.locator("#review-btn").click();
    await expect(page.locator("#review-view")).toBeVisible();
    await expect(page.locator("#review-title")).toHaveText(`Review — ${REVIEW_SESSION}`);
  });

  test("renders the session's diff against its base", async ({ page }) => {
    const file = page.locator(".rv-file").filter({ hasText: REVIEW_FILE });
    await expect(file.locator(".rv-file-path")).toHaveText(REVIEW_FILE);
    await expect(file.locator(".rv-file-status")).toHaveText("modified");
    await expect(file.locator(".rv-hunk-header").first()).toContainText("@@");
    // cc_dirty_worktree's edit: one line inserted into acquire(), plus reapers.
    await expect(
      file.locator(".rv-line.addition", { hasText: "self.reap_expired();" }),
    ).toHaveCount(1);
    await expect(page.locator("#review-status")).toContainText("1 file(s)");

    await page.locator("#review-close").click();
    await expect(page.locator("#review-view")).toBeHidden();
  });

  test("add a comment on a diff line", async ({ page, request, dialogs }) => {
    const s = await sessionByTitle(request, REVIEW_SESSION);
    const text = `e2e comment ${Date.now()}`;

    await page.locator(".rv-line.addition", { hasText: "self.reap_expired();" }).click();
    const composer = page.locator(".rv-composer");
    await expect(composer).toBeVisible();
    await composer.locator("textarea").fill(text);
    await composer.getByRole("button", { name: "Comment" }).click();

    await expect(composer).toBeHidden();
    await expect(page.locator(".rv-comment", { hasText: text })).toBeVisible();
    const snap = await api<Snapshot>(request, "GET", `/sessions/${s.id}/review`);
    const c = snap.comments.find((x) => x.comment === text);
    expect(c).toMatchObject({ file: REVIEW_FILE, side: "new" });
    expect(dialogs).toEqual([]);
  });

  test("toggle a file reviewed and back", async ({ page, request }) => {
    const s = await sessionByTitle(request, REVIEW_SESSION);
    const reviewed = async () =>
      (await api<Snapshot>(request, "GET", `/sessions/${s.id}/review`)).reviewed;
    const box = page
      .locator(".rv-file")
      .filter({ hasText: REVIEW_FILE })
      .locator(".rv-reviewed input[type=checkbox]");

    await expect(box).not.toBeChecked();
    await box.check();
    await expect.poll(reviewed).toContain(REVIEW_FILE);

    // Survives a reload of the review from the server.
    await page.locator("#review-refresh").click();
    await expect(box).toBeChecked();

    await box.uncheck();
    await expect.poll(reviewed).not.toContain(REVIEW_FILE);
  });
});
