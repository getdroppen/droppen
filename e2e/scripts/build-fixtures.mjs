// Turn the built web app into the three static hosts the harness serves.
//
// `my.droppen.test` is the real web build (`web/dist`). When the tree has the
// cloud build (`web/build/cloud-mode.ts`, the `DROPPEN_WEB_MODE=cloud` output)
// the build already carries the `rh-hub` marker, `/.well-known/droppen-hub`
// and `_headers`; otherwise this script adds the same three things, plus a
// small fetch shim that sends the app's same-origin `/v1/*` calls to
// `https://api.droppen.test` (the real cloud build derives that origin from
// the marker itself; the shim is the stand-in until that build is on `main`).
//
// `www.` and the apex are static-fixture pages here: the site role's real
// build does not exist yet. They carry the target policies
// (`Permissions-Policy` denying WebAuthn, no `rh-hub` marker, no API in
// `connect-src`, a retiring service worker) and the app links the site build
// must have. When the site role lands, `siteFixture` is the function to
// replace with that build; the policies and the specs stay.
import { createHash } from "node:crypto";
import { copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { HOSTS } from "./certs.mjs";

const here = dirname(fileURLToPath(import.meta.url));
export const E2E_ROOT = resolve(here, "..");
export const REPO_ROOT = resolve(E2E_ROOT, "..");
export const OUT = join(E2E_ROOT, ".e2e");
export const DIST = join(OUT, "dist");

export const ORIGINS = {
  app: `https://${HOSTS.app}`,
  site: `https://${HOSTS.site}`,
  apex: `https://${HOSTS.apex}`,
  api: `https://${HOSTS.api}`,
  hub: `wss://${HOSTS.api}`,
};

/** Deterministic marker the app tests look for; step 3 moves it here. */
export const APP_MARKER = ORIGINS.hub;

function inlineScriptHashes(html) {
  const hashes = [];
  for (const match of String(html).matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script>/gi)) {
    if (/\bsrc\s*=/i.test(match[1])) continue;
    hashes.push(`'sha256-${createHash("sha256").update(match[2], "utf8").digest("base64")}'`);
  }
  return hashes;
}

function appHeaders(html) {
  const scriptSrc = ["script-src 'self'", ...inlineScriptHashes(html)].join(" ");
  return [
    "/*",
    `  Content-Security-Policy: default-src 'self'; ${scriptSrc}; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' ${ORIGINS.api} ${ORIGINS.hub}; worker-src 'self' blob:; manifest-src 'self'; frame-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'; object-src 'none'`,
    "  Permissions-Policy: publickey-credentials-get=(self), publickey-credentials-create=(self)",
    "  Referrer-Policy: no-referrer",
    "  Strict-Transport-Security: max-age=31536000; includeSubDomains",
    "",
    "# The hosts document is JSON: a static host has no extension to guess it from.",
    "/.well-known/droppen-hub",
    "  Content-Type: application/json; charset=utf-8",
    "  Cache-Control: no-cache",
    "",
  ].join("\n");
}

function siteHeaders({ webauthn = "none", allowApi = false, allowAlias = false } = {}) {
  const policy = webauthn === "self" ? "(self)" : "()";
  const connect = [
    "'self'",
    ...(allowApi ? [ORIGINS.api, ORIGINS.hub] : []),
    // The apex's own machine alias (`wss://droppen.test`), never the API.
    ...(allowAlias ? [`wss://${HOSTS.apex}`] : []),
  ].join(" ");
  return [
    "/*",
    `  Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src ${connect}; worker-src 'self'; manifest-src 'self'; frame-src 'none'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'; object-src 'none'`,
    `  Permissions-Policy: publickey-credentials-get=${policy}, publickey-credentials-create=${policy}`,
    "  Referrer-Policy: no-referrer",
    "  Strict-Transport-Security: max-age=31536000; includeSubDomains",
    "",
    "/.well-known/droppen-hub",
    "  Content-Type: application/json; charset=utf-8",
    "  Cache-Control: no-cache",
    "",
  ].join("\n");
}

const HUB_DOC = `${JSON.stringify({ origin: ORIGINS.hub, app: ORIGINS.app, site: ORIGINS.site }, null, 2)}\n`;

const RETIRING_SW = `// The site holds no keys and caches nothing: a worker from the old hub
// retires itself instead of serving the page.
self.addEventListener("install", (event) => {
  event.waitUntil(self.skipWaiting());
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      await self.registration.unregister();
      for (const key of await caches.keys()) await caches.delete(key);
    })(),
  );
});
`;

function writeCommon(dir) {
  mkdirSync(join(dir, ".well-known"), { recursive: true });
  writeFileSync(join(dir, ".well-known", "droppen-hub"), HUB_DOC);
}

function copyAsset(from, to) {
  if (existsSync(from)) copyFileSync(from, to);
}

function appFixture(dest, mutation) {
  const dist = join(REPO_ROOT, "web", "dist");
  if (!existsSync(join(dist, "index.html"))) {
    throw new Error(`no built web app at ${dist}: run \`pnpm --filter @remote-hand/web build\` first (scripts/prepare.mjs does)`);
  }
  cpSync(dist, dest, { recursive: true });
  const index = join(dest, "index.html");
  let html = readFileSync(index, "utf8");
  const hasMarker = /<meta\b[^>]*\bname\s*=\s*["']rh-hub["']/i.test(html);
  // `api-base` breaks the HTTP base: the shim's target below when the build
  // has no cloud mode, and the marker (its only source of the base) when it has.
  const breaksMarker = mutation === "marker" || (mutation === "api-base" && hasMarker);
  const marker = breaksMarker ? "wss://api.droppen.invalid" : APP_MARKER;

  if (!hasMarker) {
    // The stand-in for web/build/cloud-mode.ts (until that build is on main):
    // the marker makes the page cloud mode, and the shim gives the page the
    // cross-origin API base the marker implies.
    const apiBase = mutation === "api-base" ? "https://api.droppen.invalid" : ORIGINS.api;
    const shim = `(() => { const f = window.fetch.bind(window); const api = ${JSON.stringify(apiBase)}; window.fetch = (input, init) => f(typeof input === "string" && input.startsWith("/v1/") ? api + input : input, init); })();`;
    const head = html.indexOf("<head>");
    const tag = `<script>${shim}</script>`;
    const markerTag = `<meta name="rh-hub" content="${marker}" />`;
    html = head < 0 ? markerTag + tag + html : html.slice(0, head + 6) + markerTag + tag + html.slice(head + 6);
  } else if (breaksMarker) {
    html = html.replace(/(<meta\b[^>]*\bname\s*=\s*["']rh-hub["'][^>]*\bcontent\s*=\s*["'])[^"']*(["'])/i, `$1${marker}$2`);
  }
  writeFileSync(index, html);
  writeFileSync(join(dest, "_headers"), appHeaders(html));
  writeCommon(dest);
}

function siteFixture(dest, mutation) {
  mkdirSync(dest, { recursive: true });
  writeFileSync(
    join(dest, "index.html"),
    `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Droppen</title>
<link rel="icon" href="/favicon.svg" />
</head>
<body>
<main>
<h1>Droppen</h1>
<p>Private sessions from any browser to the machines you own.</p>
<a data-app-link="signin" href="${ORIGINS.app}/signin">Sign in</a>
<a data-app-link="open" href="${ORIGINS.app}/sessions">Open the app</a>
<a href="/privacy">Privacy</a>
</main>
</body>
</html>
`,
  );
  copyAsset(join(REPO_ROOT, "web", "dist", "favicon.svg"), join(dest, "favicon.svg"));
  writeFileSync(join(dest, "sw.js"), RETIRING_SW);
  writeFileSync(join(dest, "_headers"), siteHeaders({ webauthn: mutation === "site-policy" ? "self" : "none" }));
  writeCommon(dest);
}

function apexFixture(dest) {
  mkdirSync(dest, { recursive: true });
  writeFileSync(
    join(dest, "index.html"),
    `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="robots" content="noindex" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<title>Droppen has moved</title>
<link rel="icon" href="/favicon.svg" />
</head>
<body>
<main>
<h1>Droppen has moved</h1>
<p>The app is now at <a id="app-link" href="${ORIGINS.app}/">${HOSTS.app}</a>.</p>
</main>
</body>
</html>
`,
  );
  copyAsset(join(REPO_ROOT, "web", "dist", "favicon.svg"), join(dest, "favicon.svg"));
  writeFileSync(join(dest, "sw.js"), RETIRING_SW);
  writeFileSync(join(dest, "_headers"), siteHeaders({ allowAlias: true }));
  writeCommon(dest);
}

/**
 * Write `app`, `site` and `apex` under `.e2e/dist`. `E2E_MUTATION` names a
 * deliberate break the acceptance run needs to catch: `marker`, `cors`,
 * `site-policy` or `api-base`.
 */
export function buildFixtures({ mutation = process.env.E2E_MUTATION ?? "" } = {}) {
  rmSync(DIST, { recursive: true, force: true });
  for (const name of ["app", "site", "apex"]) mkdirSync(join(DIST, name), { recursive: true });
  appFixture(join(DIST, "app"), mutation);
  siteFixture(join(DIST, "site"), mutation);
  apexFixture(join(DIST, "apex"));
  writeFileSync(join(OUT, "mutation.json"), `${JSON.stringify({ mutation }, null, 2)}\n`);
  return DIST;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log(buildFixtures());
}
