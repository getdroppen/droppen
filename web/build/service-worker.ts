import { createHash } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

/**
 * Fill in what `public/sw.js` needs to know of the build it ships with
 * (vite.config.ts `precacheBootSet`, after the files are written):
 *
 * - `PRECACHE`: the assets the app boots from, fetched by the worker's install;
 * - `INTEGRITY`: every file under `assets/` → the sha256 (hex) of its bytes on
 *   disk, sorted by path;
 * - `SHELL_SHA256`: the sha256 of `index.html` as built, before a hub writes
 *   its `rh-hub` marker into it;
 * - `SHELL_SCRIPTS`: the CSP sources (`'sha256-…'`) of the shell's inline
 *   scripts, found as a hub finds them, for the policy the worker serves the
 *   offline shell with.
 *
 * The worker serves and caches an asset only when its bytes have the sha256
 * listed here, and keeps an offline shell only when it is this one, so
 * nothing written into the browser's Cache Storage — by a page, or by an
 * earlier delivery — outlives the next worker. Its own bytes are the only
 * thing it trusts; `scripts/web-manifest.mjs` refuses a build whose table
 * disagrees with its files, and `scripts/verify-web.mjs` checks it against
 * the attested manifest (docs/web-integrity.md).
 */
export function fillServiceWorker(dir: string, boot: readonly string[]): void {
  const file = join(dir, "sw.js");
  let text = readFileSync(file, "utf8");
  const integrity: Record<string, string> = {};
  for (const path of filesUnder(dir, "assets")) integrity[`/${path}`] = sha256(readFileSync(join(dir, path)));
  const shell = readFileSync(join(dir, "index.html"));
  const fills: [marker: string, value: string][] = [
    ["const PRECACHE = [];", `const PRECACHE = ${JSON.stringify(boot)};`],
    ["const INTEGRITY = {};", `const INTEGRITY = ${JSON.stringify(integrity)};`],
    ['const SHELL_SHA256 = "";', `const SHELL_SHA256 = ${JSON.stringify(sha256(shell))};`],
    ["const SHELL_SCRIPTS = [];", `const SHELL_SCRIPTS = ${JSON.stringify(inlineScriptSources(shell.toString("utf8")))};`],
  ];
  for (const [marker, value] of fills) {
    if (!text.includes(marker)) throw new Error(`${file} has no \`${marker}\` to fill in`);
    // A function, so nothing in the value is read as a replacement pattern.
    text = text.replace(marker, () => value);
  }
  writeFileSync(file, text);
}

/**
 * The CSP hash sources of `html`'s inline scripts — every `<script …>` whose
 * tag has no `src=`, its text up to `</script>` — in order, as a hub computes
 * them for its policy and scripts/verify-web.mjs reads them.
 */
export function inlineScriptSources(html: string): string[] {
  const out: string[] = [];
  let rest = html;
  for (;;) {
    const start = rest.indexOf("<script");
    if (start < 0) break;
    const openEnd = rest.indexOf(">", start);
    if (openEnd < 0) break;
    const close = rest.indexOf("</script>", openEnd + 1);
    if (close < 0) break;
    if (!rest.slice(start, openEnd).includes("src=")) out.push(`'sha256-${createHash("sha256").update(rest.slice(openEnd + 1, close)).digest("base64")}'`);
    rest = rest.slice(close + "</script>".length);
  }
  return out;
}

function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

/** Every file under `dir/sub`, as `sub/…` paths sorted bytewise (what the manifest sorts by). */
function filesUnder(dir: string, sub: string): string[] {
  const out: string[] = [];
  const walk = (rel: string) => {
    for (const entry of readdirSync(join(dir, rel), { withFileTypes: true })) {
      const path = `${rel}/${entry.name}`;
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile()) out.push(path);
      else throw new Error(`${join(dir, path)}: neither a file nor a directory`);
    }
  };
  walk(sub);
  return out.sort((a, b) => Buffer.compare(Buffer.from(a), Buffer.from(b)));
}
