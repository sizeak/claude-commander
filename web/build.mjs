// Bundles the web UI into crates/claude-commander-server/webui/, which the
// server embeds (rust-embed; a debug build reads it from disk).
//
//   node build.mjs           one build
//   node build.mjs --watch   rebuild on change (pair with a debug server)
//
// The output is committed and CI checks it is fresh (build, then
// `git diff --exit-code` on webui/), so it must be byte-for-byte reproducible:
// fixed file names (no content hashes), no sourcemaps (they embed paths), no
// timestamps, and a licence banner built only from the installed packages.
//
// Output: index.html + favicon.svg (copied), style.css (src/style.css with
// xterm's stylesheet inlined by its @import), app.js (src/main.ts + xterm +
// addon-fit). Anything else in webui/ is deleted, so a renamed asset cannot
// linger in the embedded page.

import { readdir, readFile, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const WEB = dirname(fileURLToPath(import.meta.url));
const OUT = join(WEB, "..", "crates", "claude-commander-server", "webui");
const OUTPUTS = new Set(["app.js", "style.css", "index.html", "favicon.svg"]);

// xterm's dist bundles carry no licence comment for esbuild's legalComments to
// keep, so state the notice for each bundled package here, from its own
// package.json + LICENSE (which the lockfile pins).
async function licenceBanner() {
  const parts = ["/*! Bundled third-party code:"];
  for (const pkg of ["@xterm/xterm", "@xterm/addon-fit"]) {
    const dir = join(WEB, "node_modules", pkg);
    const meta = JSON.parse(await readFile(join(dir, "package.json"), "utf8"));
    const licence = (await readFile(join(dir, "LICENSE"), "utf8")).trim();
    parts.push("", `${meta.name}@${meta.version} (${meta.license})`, "", licence);
  }
  // A `*/` inside the licence text would end the comment early.
  return `${parts.join("\n").replaceAll("*/", "* /")}\n*/`;
}

async function pruneStale() {
  let names = [];
  try {
    names = await readdir(OUT);
  } catch {
    return;
  }
  await Promise.all(
    names
      .filter((n) => !OUTPUTS.has(n))
      .map((n) => rm(join(OUT, n), { recursive: true, force: true })),
  );
}

const options = {
  absWorkingDir: WEB,
  entryPoints: [
    { in: "src/main.ts", out: "app" },
    { in: "src/style.css", out: "style" },
    { in: "src/index.html", out: "index" },
    { in: "src/favicon.svg", out: "favicon" },
  ],
  outdir: OUT,
  bundle: true,
  format: "iife",
  platform: "browser",
  target: ["es2022"],
  minify: true,
  legalComments: "eof",
  charset: "utf8",
  loader: { ".html": "copy", ".svg": "copy" },
  banner: { js: await licenceBanner() },
  logLevel: "info",
};

await pruneStale();
if (process.argv.includes("--watch")) {
  const ctx = await esbuild.context(options);
  await ctx.watch();
} else {
  await esbuild.build(options);
}
