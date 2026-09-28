import {
  api,
  connect,
  ECHO_SESSION,
  expect,
  LIFECYCLE_SESSION,
  openSession,
  PROJECT,
  paneText,
  projectHeader,
  REVIEW_SESSION,
  sessionByTitle,
  sessionRow,
  terminalText,
  test,
} from "./support";

test.describe("session tree and terminal", () => {
  test.beforeEach(async ({ page }) => {
    await connect(page);
  });

  test("the tree lists the seeded sessions under their project", async ({ page }) => {
    await expect(projectHeader(page, PROJECT)).toBeVisible();
    for (const title of [ECHO_SESSION, REVIEW_SESSION, LIFECYCLE_SESSION]) {
      await expect(sessionRow(page, title)).toBeVisible();
      await expect(sessionRow(page, title).locator(".badge")).toHaveText("running");
    }
    // Toolbar actions stay disabled until a session is picked.
    await expect(page.locator("#session-title")).toHaveText("Select a session");
    await expect(page.locator("#restart-btn")).toBeDisabled();
    await expect(page.locator("#review-btn")).toBeDisabled();
  });

  test("selecting a session attaches its pane", async ({ page }) => {
    await openSession(page, ECHO_SESSION);
    await expect(sessionRow(page, ECHO_SESSION)).toHaveClass(/active/);
    await expect(terminalText(page)).toContainText("echo agent ready");
    await expect(page.locator("#restart-btn")).toBeEnabled();
    await expect(page.locator("#kill-btn")).toBeEnabled();
  });

  test("typed input reaches the pane", async ({ page, request }) => {
    const s = await sessionByTitle(request, ECHO_SESSION);
    await openSession(page, ECHO_SESSION);
    await expect(terminalText(page)).toContainText("echo agent ready");

    const marker = `e2e-typed-${Date.now()}`;
    await page.locator("#terminal").click();
    await page.keyboard.type(marker);
    await page.keyboard.press("Enter");

    // The stand-in answers each line it reads, so `got:` proves the bytes went
    // through the PTY into the process — not just into xterm's local echo.
    await expect(terminalText(page)).toContainText(`got:${marker}`);
    await expect.poll(() => paneText(request, s.id)).toContain(`got:${marker}`);
  });

  test("the attach and a browser resize both size the pane", async ({ page, request }) => {
    const s = await sessionByTitle(request, ECHO_SESSION);
    // The stand-in prints `size:<rows> <cols>` whenever its PTY's size
    // changes, so the pane's last such line is the size tmux gave the pane.
    const paneSize = async () => {
      const m = [...(await paneText(request, s.id)).matchAll(/size:(\d+) (\d+)/g)].at(-1);
      return m ? { rows: Number(m[1]), cols: Number(m[2]) } : null;
    };
    // The rows xterm renders; tmux keeps one of them for its status line.
    const expectedPaneRows = async () =>
      (await page.locator("#terminal .xterm-rows > div").count()) - 1;

    await openSession(page, ECHO_SESSION);
    await expect(terminalText(page)).toContainText("echo agent ready");

    // The attach handshake sizes the pane to the browser (the session starts
    // at tmux's 200x50, wider than this viewport's terminal).
    // Polled as a difference: xterm re-fits asynchronously too.
    const rowsMismatch = async () => ((await paneSize())?.rows ?? -1) - (await expectedPaneRows());
    await expect.poll(rowsMismatch).toBe(0);
    const before = await paneSize();
    expect(before?.cols).toBeLessThan(200);

    // A smaller window sends a resize frame and the pane follows it.
    await page.setViewportSize({ width: 900, height: 560 });
    await expect.poll(async () => (await paneSize())?.cols).toBeLessThan(before?.cols ?? 0);
    await expect.poll(rowsMismatch).toBe(0);
    expect((await paneSize())?.rows).toBeLessThan(before?.rows ?? 0);
  });

  // Last in the file: it restarts the echo session the tests above rely on.
  test("an attach comes back after the session is restarted elsewhere", async ({
    page,
    request,
  }) => {
    const s = await sessionByTitle(request, ECHO_SESSION);
    await openSession(page, ECHO_SESSION);
    await expect(terminalText(page)).toContainText("echo agent ready");

    // Through the API, not the page: the TUI, the CLI or another tab.
    await api(request, "POST", `/sessions/${s.id}/restart`);
    // The old pane's session ended, which the attach reports and stops on...
    await expect(terminalText(page)).toContainText("session_ended");
    // ...then, with the session running again, it re-attaches on its own
    // (the terminal is reset for the new pane) without a click.
    await expect(terminalText(page)).not.toContainText("session_ended", { timeout: 30_000 });
    await expect(terminalText(page)).toContainText("echo agent ready");
    await expect(page.locator("#conn-status")).toHaveText("connected");

    // And it is live: typed input reaches the new pane.
    const marker = `e2e-restarted-${Date.now()}`;
    await page.locator("#terminal").click();
    await page.keyboard.type(marker);
    await page.keyboard.press("Enter");
    await expect(terminalText(page)).toContainText(`got:${marker}`);
  });
});
