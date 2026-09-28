import { api, connect, expect, test } from "./support";

interface Config {
  branch_prefix: string;
  fetch_before_create: boolean;
  resume_session: boolean;
  project_pull_enabled: boolean;
}

test.describe("settings", () => {
  test("read reflects the server; save patches it", async ({ page, request, dialogs }) => {
    const original = await api<Config>(request, "GET", "/config");
    try {
      await connect(page);
      await page.locator("#settings-btn").click();
      const modal = page.locator("#settings-modal");
      await expect(modal).toBeVisible();

      // Read: the form shows the server's values.
      await expect(page.locator("#set-branch-prefix")).toHaveValue(original.branch_prefix ?? "");
      const resume = page.locator("#set-resume-session");
      if (original.resume_session) await expect(resume).toBeChecked();
      else await expect(resume).not.toBeChecked();
      if (original.fetch_before_create)
        await expect(page.locator("#set-fetch-before-create")).toBeChecked();
      else await expect(page.locator("#set-fetch-before-create")).not.toBeChecked();

      // Patch: change two fields and save.
      await page.locator("#set-branch-prefix").fill("e2e-prefix/");
      await resume.setChecked(!original.resume_session);
      await modal.getByRole("button", { name: "Save" }).click();
      await expect(modal).toBeHidden();

      const after = await api<Config>(request, "GET", "/config");
      expect(after.branch_prefix).toBe("e2e-prefix/");
      expect(after.resume_session).toBe(!original.resume_session);
      // Untouched fields round-trip unchanged.
      expect(after.fetch_before_create).toBe(original.fetch_before_create);
      expect(after.project_pull_enabled).toBe(original.project_pull_enabled);

      // Re-opening reads the saved values back.
      await page.locator("#settings-btn").click();
      await expect(page.locator("#set-branch-prefix")).toHaveValue("e2e-prefix/");
      if (after.resume_session) await expect(resume).toBeChecked();
      else await expect(resume).not.toBeChecked();
      expect(dialogs).toEqual([]);
    } finally {
      await api(request, "PATCH", "/config", {
        branch_prefix: original.branch_prefix,
        fetch_before_create: original.fetch_before_create,
        resume_session: original.resume_session,
        project_pull_enabled: original.project_pull_enabled,
      });
    }
  });
});
