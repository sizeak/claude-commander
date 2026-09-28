import { connect, expect, PROJECT, projectHeader, test } from "./support";

test.describe("connect screen", () => {
  test("a good token connects and loads the workspace", async ({ page }) => {
    await connect(page);
    await expect(projectHeader(page, PROJECT)).toBeVisible();
  });

  test("a bad token is rejected and leaves the connect screen up", async ({ page }) => {
    await page.goto("/");
    const modal = page.locator("#connect-modal");
    await expect(modal).toBeVisible();

    await page.locator("#connect-token").fill("definitely-not-the-token");
    await modal.getByRole("button", { name: "Connect" }).click();

    await expect(page.locator("#connect-error")).toBeVisible();
    await expect(page.locator("#connect-error")).toContainText("rejected");
    // Outlive a poll cycle (1.5s) to prove it stays up rather than flashing.
    await page.waitForTimeout(2_000);
    await expect(modal).toBeVisible();
    await expect(page.locator("#conn-status")).not.toHaveText("connected");
    await expect(page.locator("#tree .project-header")).toHaveCount(0);
  });

  test("a rejected token after a reload returns to the connect screen", async ({ page }) => {
    await connect(page);
    // The page keeps the token across reloads; revoking it server-side isn't
    // possible here, so simulate the same thing from the browser: swap every
    // stored value for a bad one and reload.
    await page.evaluate(() => {
      for (let i = 0; i < localStorage.length; i++) {
        const k = localStorage.key(i);
        if (k) localStorage.setItem(k, "revoked-token");
      }
    });
    await page.reload();
    await expect(page.locator("#connect-modal")).toBeVisible();
    await expect(page.locator("#conn-status")).not.toHaveText("connected");
  });
});
