// Shared harness for the web UI e2e.
//
// The suite depends on the *target* through exactly two values — the page's
// origin (CC_WEB_BASE_URL, also Playwright's baseURL) and the commander bearer
// token (CC_WEB_TOKEN) — so it runs unchanged against whichever binary serves
// the page and fronts /api. Every API call below goes to that same origin.
//
// The rest (seeded session titles, repo paths to add/scan) describes the
// *fixture* web/e2e/run.sh builds, not the target.

import {
  type APIRequestContext,
  test as base,
  expect,
  type Locator,
  type Page,
} from "@playwright/test";
import type { ProjectInfo, SessionInfo, Snapshot } from "../src/generated/index.ts";

function required(name: string): string {
  const v = process.env[name];
  if (!v) throw new Error(`${name} is unset — run the suite through web/e2e/run.sh`);
  return v;
}

export const TOKEN = required("CC_WEB_TOKEN");

// Seeded by run.sh (keep the two in step).
export const PROJECT = "alpha";
export const ECHO_SESSION = "Echo pane";
export const REVIEW_SESSION = "Review me";
export const LIFECYCLE_SESSION = "Lifecycle";
export const REVIEW_FILE = "src/pool.rs";

export const fixture = {
  /** A git repo that exists on disk but is not yet a registered project. */
  get addRepo() {
    return required("CC_WEB_FIXTURE_ADD_REPO");
  },
  /** A directory holding two unregistered repos (gamma, delta). */
  get scanDir() {
    return required("CC_WEB_FIXTURE_SCAN_DIR");
  },
};

// ---- the commander API, through the page's own origin ----------------------

// The wire shapes are protocol's generated types, as in the page: a
// hand-written copy here would keep passing after a rename it should catch.
export type { ProjectInfo, SessionInfo };

export async function api<T = unknown>(
  request: APIRequestContext,
  method: "GET" | "POST" | "PATCH" | "DELETE",
  path: string,
  body?: unknown,
): Promise<T> {
  const res = await request.fetch(`/api${path}`, {
    method,
    headers: { Authorization: `Bearer ${TOKEN}` },
    data: body,
  });
  expect(res.ok(), `${method} /api${path} → ${res.status()} ${await res.text()}`).toBeTruthy();
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

export async function workspace(request: APIRequestContext) {
  return api<Snapshot>(request, "GET", "/workspace");
}

export async function sessionByTitle(request: APIRequestContext, title: string) {
  const ws = await workspace(request);
  const s = ws.sessions.find((x) => x.title === title);
  if (!s) throw new Error(`no session titled ${title}`);
  return s;
}

/** The pane's current text, as the server captures it (not the browser). */
export async function paneText(request: APIRequestContext, id: string): Promise<string> {
  const res = await request.get(`/api/sessions/${id}/pane?lines=200`, {
    headers: { Authorization: `Bearer ${TOKEN}` },
  });
  expect(res.ok()).toBeTruthy();
  return res.text();
}

// ---- page helpers ------------------------------------------------------------

/**
 * Every test gets a `dialogs` log. alert/confirm/prompt are accepted (so a
 * Kill/Delete confirm goes through) and their messages recorded; a test that
 * did not expect one should assert the log is free of "Action failed".
 */
export const test = base.extend<{ dialogs: string[] }>({
  dialogs: async ({ page }, use) => {
    const log: string[] = [];
    page.on("dialog", async (d) => {
      log.push(d.message());
      await d.accept();
    });
    await use(log);
  },
});
export { expect };

/** Load the page and get past the connect screen with the good token. */
export async function connect(page: Page) {
  await page.goto("/");
  const modal = page.locator("#connect-modal");
  await expect(modal).toBeVisible();
  await page.locator("#connect-token").fill(TOKEN);
  await modal.getByRole("button", { name: "Connect" }).click();
  await expect(modal).toBeHidden();
  await expect(page.locator("#conn-status")).toHaveText("connected");
}

export function sessionRow(page: Page, title: string): Locator {
  return page.locator("#tree li.session").filter({
    has: page.locator(".session-name", { hasText: new RegExp(`^${escapeRe(title)}$`) }),
  });
}

export function projectHeader(page: Page, name: string): Locator {
  return page.locator("#tree .project-header").filter({
    has: page.locator(".pname", { hasText: new RegExp(`^${escapeRe(name)}$`) }),
  });
}

/** A project's header plus its session list. */
export function projectGroup(page: Page, name: string): Locator {
  return page.locator("#tree .project-group").filter({
    has: page.locator(".project-header .pname", { hasText: new RegExp(`^${escapeRe(name)}$`) }),
  });
}

/** The terminal's rendered text (xterm's DOM renderer rows). */
export function terminalText(page: Page): Locator {
  return page.locator("#terminal .xterm-rows");
}

/** Select a session in the tree and wait for the attached pane to paint. */
export async function openSession(page: Page, title: string) {
  await sessionRow(page, title).click();
  await expect(page.locator("#session-title")).toHaveText(title);
  await expect(page.locator("#terminal-placeholder")).toBeHidden();
}

function escapeRe(s: string) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
