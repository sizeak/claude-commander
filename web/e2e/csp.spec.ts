import { ECHO_SESSION, expect, openSession, TOKEN, terminalText, test } from "./support";

// The server sends a Content-Security-Policy with the page
// (crates/claude-commander-server/src/webui.rs, `CSP`). A directive that is too
// tight fails quietly in the browser — a blocked xterm stylesheet leaves an
// unstyled terminal, a blocked socket a pane that never paints — so boot and
// attach here while recording every violation and console error.
test("boot and attach run clean under the page's CSP", async ({ page }) => {
  const violations: string[] = [];
  const errors: string[] = [];
  await page.exposeFunction("__ccCspViolation", (v: string) => {
    violations.push(v);
  });
  await page.addInitScript(() => {
    document.addEventListener("securitypolicyviolation", (e) => {
      const report = (window as unknown as { __ccCspViolation: (v: string) => void })
        .__ccCspViolation;
      report(`${e.violatedDirective} blocked ${e.blockedURI || "inline"}`);
    });
  });
  page.on("console", (m) => {
    if (m.type() === "error") errors.push(m.text());
  });
  page.on("pageerror", (e) => errors.push(e.message));

  // A linked token, so booting makes no 401 (which Chrome logs as an error).
  const resp = await page.goto(`/#token=${TOKEN}`);
  expect(resp?.headers()["content-security-policy"]).toContain("frame-ancestors 'none'");
  await expect(page.locator("#connect-modal")).toBeHidden();
  await openSession(page, ECHO_SESSION);
  await expect(terminalText(page)).toContainText("echo agent ready");
  // xterm's runtime-injected <style> took effect: the DOM renderer sets the
  // rows' font size (terminal.ts's `fontSize: 13`) there, not in style.css.
  await expect(page.locator("#terminal .xterm-rows")).toHaveCSS("font-size", "13px");

  expect(violations).toEqual([]);
  expect(errors).toEqual([]);
});
