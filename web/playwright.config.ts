import { defineConfig, devices } from "@playwright/test";

// The suite is parametrised by the page's origin and the bearer token ONLY
// (see e2e/support.ts), so it runs unchanged whichever binary serves the page.
// e2e/run.sh builds the hermetic fixture and exports both.
const baseURL = process.env.CC_WEB_BASE_URL;
if (!baseURL) {
  throw new Error("CC_WEB_BASE_URL is unset — run the suite through web/e2e/run.sh");
}

export default defineConfig({
  testDir: "./e2e",
  // One commander server backs every test, so run them serially: tests use
  // disjoint seeded sessions, but a create/scan in one would otherwise race a
  // tree assertion in another.
  workers: 1,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  // On CI, also an HTML report (web/playwright-report/), which the Web job
  // uploads on failure alongside test-results/.
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : [["list"]],
  use: {
    ...devices["Desktop Chrome"],
    baseURL,
    headless: true,
    viewport: { width: 1280, height: 800 },
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium" }],
});
