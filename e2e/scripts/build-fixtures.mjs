// Turn the built web app into the three static hosts the harness serves.
//
// On a tree with the cloud build (`web/build/cloud-mode.ts` and its
// `DROPPEN_WEB_ROLE`), `prepare.mjs` builds the real `app` and `site` outputs
// and this script serves them as they are:
//
// - `my.droppen.test`  the app role (WebAuthn, API and socket in `connect-src`)
// - `www.droppen.test` the site role (public pages, no WebAuthn, `'self'`)
// - `droppen.test`     the same site role, with the hub's `apex_mode: move`
//                      answer, so the real move notice runs
//
// The only edits are the fixture's: a deliberate break for the mutation run,
// and `apex_mode` in `/.well-known/droppen-hub` (the build writes the hosts
// file; a hub adds `apex_mode`, and the legacy page needs it to run the move
// notice instead of the app).
//
// On a tree without the cloud build (or without roles), the fallback is a
// normal build plus a marker and a small fetch shim on `my.`, and hand-written
// site/apex pages carrying the target policies. It exists only so the harness
// can run against older trees.
import { createHash } from "node:crypto";
import { copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { HOSTS } from "./certs.mjs";

const here = dirname(fileURLToPath(import.meta.url));
export const E2E_ROOT = resolve(here, "..");
export const REPO_ROOT = resolve(E2E_ROOT, "..");
export const OUT = join(E2E_ROOT, ".e2e");
export const DIST = join(OUT, "dist");
/** The two role builds, copied out of `web/dist` before the next one overwrites it. */
export const BUILDS = join(OUT, "builds");

export const ORIGINS = {
  app: `https://${HOSTS.app}`,
  site: `https://${HOSTS.site}`,
  apex: `https://${HOSTS.apex}`,
  api: `https://${HOSTS.api}`,
  hub: `wss://${HOSTS.api}`,
};

/** Deterministic marker both cloud builds write (`DROPPEN_WEB_HUB_ORIGIN`). */
export const APP_MARKER = ORIGINS.hub;

const HUB_DOC = `${JSON.stringify({ origin: ORIGINS.hub, app: ORIGINS.app, site: ORIGINS.site }, null, 2)}\n`;

/** The hub's answer a real hub serves (`apex_mode`): the site build's file has no field for it. */
const HUB_DOC_WITH_APEX = `${JSON.stringify({ origin: ORIGINS.hub, app: ORIGINS.app, site: ORIGINS.site, apex_mode: "move" }, null, 2)}\n`;

function replaceOrThrow(file, from, to) {
  const text = readFileSync(file, "utf8");
  if (!text.includes(from)) throw new Error(`${file} has no ${JSON.stringify(from)} to rewrite`);
  writeFileSync(file, text.replaceAll(from, to));
}

/** Point every static host's hosts document at the hub's full answer. */
function writeHostsDocument(dir) {
  const wellKnown = join(dir, ".well-known");
  mkdirSync(wellKnown, { recursive: true });
  writeFileSync(join(wellKnown, "droppen-hub"), HUB_DOC_WITH_APEX);
}

function* jsFiles(dir) {
  if (!existsSync(dir)) return;
  for (const name of readdirSync(dir)) {
    const file = join(dir, name);
    if (statSync(file).isDirectory()) yield* jsFiles(file);
    else if (name.endsWith(".js")) yield file;
  }
}

// ------------------------------------------------------- real cloud builds

/** `builds/app` → `dist/app`, with only the mutation of the acceptance run. */
function appFromCloud(dest, mutation) {
  cpSync(join(BUILDS, "app"), dest, { recursive: true });
  const index = join(dest, "index.html");
  const html = readFileSync(index, "utf8");
  if (!/<meta\b[^>]*\bname\s*=\s*["']rh-hub["']/i.test(html)) {
    throw new Error(`${index} carries no rh-hub marker: is DROPPEN_WEB_MODE=cloud set for the app build?`);
  }
  if (mutation === "marker") {
    writeFileSync(index, html.replace(/(<meta\b[^>]*\bname\s*=\s*["']rh-hub["'][^>]*\bcontent\s*=\s*["'])[^"']*(["'])/i, "$1wss://api.droppen.invalid$2"));
  } else if (mutation === "api-base") {
    // The marker is the API base's one source (`hubHttpOrigin()`); break the
    // ws→http rewrite in the bundle, so the marker stays right and the calls
    // go nowhere. (The string survives minification; see the self-check.)
    let broken = false;
    for (const file of jsFiles(join(dest, "assets"))) {
      const text = readFileSync(file, "utf8");
      if (!text.includes("http$1://")) continue;
      writeFileSync(file, text.replaceAll("http$1://", "http$1://e2e-broken."));
      broken = true;
    }
    if (!broken) throw new Error("api-base mutation: the built app has no ws→http rewrite to break");
  }
  writeHostsDocument(dest);
}

/** `builds/site` → `dist/site` and `dist/apex`; the site policy is the only mutation. */
function siteFromCloud(dest, mutation) {
  cpSync(join(BUILDS, "site"), dest, { recursive: true });
  if (mutation === "site-policy") {
    const headers = join(dest, "_headers");
    replaceOrThrow(headers, "publickey-credentials-get=()", "publickey-credentials-get=(self)");
    replaceOrThrow(headers, "publickey-credentials-create=()", "publickey-credentials-create=(self)");
  }
  writeHostsDocument(dest);
}

// ------------------------------------------------------------- fallback

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

function siteHeaders({ webauthn = "none", allowAlias = false } = {}) {
  const policy = webauthn === "self" ? "(self)" : "()";
  const connect = ["'self'", ...(allowAlias ? [`wss://${HOSTS.apex}`] : [])].join(" ");
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

function copyAsset(from, to) {
  if (existsSync(from)) copyFileSync(from, to);
}

function appFallback(dest, mutation) {
  const dist = join(REPO_ROOT, "web", "dist");
  if (!existsSync(join(dist, "index.html"))) {
    throw new Error(`no built web app at ${dist}: run \`pnpm --filter @remote-hand/web build\` first (scripts/prepare.mjs does)`);
  }
  cpSync(dist, dest, { recursive: true });
  const index = join(dest, "index.html");
  let html = readFileSync(index, "utf8");
  const hasMarker = /<meta\b[^>]*\bname\s*=\s*["']rh-hub["']/i.test(html);
  const breaksMarker = mutation === "marker" || (mutation === "api-base" && hasMarker);
  const marker = breaksMarker ? "wss://api.droppen.invalid" : APP_MARKER;

  if (!hasMarker) {
    // A normal build: add what the cloud build would (marker + the implied
    // cross-origin API base) so the page runs in cloud mode.
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
  mkdirSync(join(dest, ".well-known"), { recursive: true });
  writeFileSync(join(dest, ".well-known", "droppen-hub"), HUB_DOC);
}

function siteFallback(dest, mutation) {
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
  mkdirSync(join(dest, ".well-known"), { recursive: true });
  writeFileSync(join(dest, ".well-known", "droppen-hub"), HUB_DOC);
}

function apexFallback(dest) {
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
  mkdirSync(join(dest, ".well-known"), { recursive: true });
  writeFileSync(join(dest, ".well-known", "droppen-hub"), HUB_DOC);
}

/**
 * Write `app`, `site` and `apex` under `.e2e/dist`. `cloud` picks the real
 * role builds from `.e2e/builds` (see `prepare.mjs`); `E2E_MUTATION` names a
 * deliberate break the acceptance run catches: `marker`, `cors`, `site-policy`
 * or `api-base`.
 */
export function buildFixtures({ mutation = process.env.E2E_MUTATION ?? "", cloud = false } = {}) {
  rmSync(DIST, { recursive: true, force: true });
  for (const name of ["app", "site", "apex"]) mkdirSync(join(DIST, name), { recursive: true });
  if (cloud) {
    appFromCloud(join(DIST, "app"), mutation);
    siteFromCloud(join(DIST, "site"), mutation);
    siteFromCloud(join(DIST, "apex"), mutation);
  } else {
    appFallback(join(DIST, "app"), mutation);
    siteFallback(join(DIST, "site"), mutation);
    apexFallback(join(DIST, "apex"));
  }
  writeFileSync(join(OUT, "mutation.json"), `${JSON.stringify({ mutation }, null, 2)}\n`);
  return DIST;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  console.log(buildFixtures());
}
