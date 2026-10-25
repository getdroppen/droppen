// The cloud build (`build/cloud-mode.ts`, `DROPPEN_WEB_MODE=cloud`): the
// marker, `/.well-known/droppen-hub` and `_headers` added to a finished
// `dist`, and nothing at all in a normal build.

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { RETIRING_WORKER, cloudConfig, contentSecurityPolicy, headersFile, hubDocument, markShell, writeCloudBuild } from "../build/cloud-mode.js";

const SHELL = '<!doctype html><html><head><meta charset="UTF-8" /><script>boot()</script></head><body><div id="root"></div></body></html>';
const HASH = `'sha256-${createHash("sha256").update("boot()").digest("base64")}'`;
/** The hub's documented hosts route (docs/hub-api/http.md). */
const HOSTS_PATH = "/.well-known/droppen-hub";
const config = cloudConfig({ DROPPEN_WEB_MODE: "cloud" });

/** A finished build, as vite leaves it: the shell, the real worker and the assets directory. */
const REAL_WORKER = "/* Droppen service worker */\nconst INTEGRITY = {};\n";
function writeDir(dir: string): void {
  writeFileSync(join(dir, "index.html"), SHELL);
  writeFileSync(join(dir, "sw.js"), REAL_WORKER);
  mkdirSync(join(dir, "assets"), { recursive: true });
}

test("only `DROPPEN_WEB_MODE=cloud` turns the cloud output on, with droppen.app's defaults", () => {
  assert.equal(cloudConfig({}), null, "the normal, attested build");
  assert.equal(cloudConfig({ DROPPEN_WEB_MODE: "self-hosted" }), null);
  assert.deepEqual(config, {
    hubOrigin: "wss://api.droppen.app",
    appOrigin: "https://my.droppen.app",
    siteOrigin: "https://www.droppen.app",
    turnstileOrigin: "https://verify.droppen.app",
    relays: ["https://*.relay.droppen.app", "wss://*.relay.droppen.app"],
    role: "app",
  });
  assert.equal(cloudConfig({ DROPPEN_WEB_MODE: "cloud", DROPPEN_WEB_ROLE: "site" })?.role, "site");
  assert.throws(() => cloudConfig({ DROPPEN_WEB_MODE: "cloud", DROPPEN_WEB_ROLE: "admin" }), /DROPPEN_WEB_ROLE/);
  // Every value is configuration, and a malformed one fails the build.
  const over = cloudConfig({
    DROPPEN_WEB_MODE: "cloud",
    DROPPEN_WEB_HUB_ORIGIN: "wss://hub.example",
    DROPPEN_WEB_APP_ORIGIN: "https://app.example",
    DROPPEN_WEB_SITE_ORIGIN: "https://www.example",
    DROPPEN_WEB_TURNSTILE_ORIGIN: "https://verify.example",
    DROPPEN_WEB_RELAYS: "https://relay.example wss://relay.example",
  });
  assert.equal(over?.hubOrigin, "wss://hub.example");
  assert.deepEqual(over?.relays, ["https://relay.example", "wss://relay.example"]);
  for (const bad of [
    { DROPPEN_WEB_HUB_ORIGIN: "https://api.droppen.app" },
    { DROPPEN_WEB_HUB_ORIGIN: "wss://api.droppen.app/path" },
    { DROPPEN_WEB_APP_ORIGIN: "my.droppen.app" },
    { DROPPEN_WEB_SITE_ORIGIN: "https://www.droppen.app/sessions" },
    { DROPPEN_WEB_RELAYS: "relay.example" },
  ]) {
    assert.throws(() => cloudConfig({ DROPPEN_WEB_MODE: "cloud", ...bad }), /is not/, JSON.stringify(bad));
  }
});

test("the marker goes right after `<head>`, exactly as a hub and the verifier write and remove it", () => {
  const marked = markShell(SHELL, "wss://api.droppen.app");
  assert.ok(marked.startsWith('<!doctype html><html><head><meta name="rh-hub" content="wss://api.droppen.app" /><meta charset="UTF-8" />'));
  assert.equal(marked.split("rh-hub").length - 1, 1);
  assert.throws(() => markShell(marked, "wss://api.droppen.app"), /already carries/);
  assert.equal(markShell("<p>x</p>", "wss://hub.example"), '<meta name="rh-hub" content="wss://hub.example" /><p>x</p>', "a shell with no head");
});

test("the hosts document is the hub's own answer, naming the API, app and site origins", () => {
  assert.deepEqual(JSON.parse(hubDocument(config!)), {
    origin: "wss://api.droppen.app",
    app: "https://my.droppen.app",
    site: "https://www.droppen.app",
  });
});

test("the CSP is the hub's policy with the cross-origin API and relays, and the shell's inline scripts by hash", () => {
  const csp = contentSecurityPolicy(SHELL, config!);
  assert.ok(csp.startsWith(`default-src 'self'; script-src 'self' ${HASH}; `));
  assert.ok(csp.includes("connect-src 'self' https://api.droppen.app wss://api.droppen.app https://*.relay.droppen.app wss://*.relay.droppen.app;"));
  assert.ok(csp.includes("frame-src https://verify.droppen.app;"));
  assert.ok(csp.includes("frame-ancestors 'none';"));
  assert.ok(csp.includes("object-src 'none'"));
  assert.ok(!csp.includes("rh-hub"), "the marker is not script");
});

test("`_headers` carries CSP, Permissions-Policy, Referrer-Policy and HSTS for every file", () => {
  const headers = headersFile(SHELL, config!);
  assert.ok(headers.startsWith("/*\n  Content-Security-Policy: default-src 'self'; script-src 'self' "));
  assert.ok(headers.includes("  Permissions-Policy: publickey-credentials-get=(self), publickey-credentials-create=(self)\n"));
  assert.ok(headers.includes("  Referrer-Policy: no-referrer\n"));
  assert.ok(headers.includes("  Strict-Transport-Security: max-age=31536000; includeSubDomains\n"));
  assert.ok(headers.includes("/.well-known/droppen-hub\n  Content-Type: application/json"));
});

test("the site role allows no WebAuthn, frames nothing and calls only itself", () => {
  const site = cloudConfig({ DROPPEN_WEB_MODE: "cloud", DROPPEN_WEB_ROLE: "site" })!;
  const csp = contentSecurityPolicy(SHELL, site);
  assert.ok(csp.includes("connect-src 'self';"));
  assert.ok(csp.includes("frame-src 'none';"));
  assert.ok(csp.includes("frame-ancestors 'none';"));
  assert.ok(csp.includes("object-src 'none'"));
  assert.ok(!csp.includes("api.droppen.app"), "the site calls no API");
  assert.ok(!csp.includes("relay.droppen.app"), "nor a relay");
  assert.ok(!csp.includes("verify.droppen.app"), "and frames no Turnstile page");
  const headers = headersFile(SHELL, site);
  assert.ok(headers.includes("  Permissions-Policy: publickey-credentials-get=(), publickey-credentials-create=()\n"));
  assert.ok(headers.includes("  Referrer-Policy: no-referrer\n"));
  assert.ok(headers.includes("  Strict-Transport-Security: max-age=31536000; includeSubDomains\n"));
});

test("a cloud build writes the marker, the hosts document and `_headers` — and never a build attestation", () => {
  const dir = mkdtempSync(join(tmpdir(), "cloud-build-"));
  try {
    writeDir(dir);
    writeCloudBuild(dir, config!);
    assert.ok(readFileSync(join(dir, "index.html"), "utf8").includes('<meta name="rh-hub" content="wss://api.droppen.app" />'));
    assert.equal(JSON.parse(readFileSync(join(dir, HOSTS_PATH), "utf8")).origin, "wss://api.droppen.app");
    assert.ok(readFileSync(join(dir, "_headers"), "utf8").includes("Content-Security-Policy:"));
    // The hub owns `/.well-known/droppen-build`; a static host must not answer it,
    // so the verifier falls back to the release's attested manifest.
    assert.equal(readdirSync(join(dir, ".well-known")).sort().join(","), "droppen-hub");
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("the app and site roles differ only in `_headers` and the site's retiring worker", () => {
  const app = mkdtempSync(join(tmpdir(), "cloud-app-"));
  const site = mkdtempSync(join(tmpdir(), "cloud-site-"));
  try {
    for (const dir of [app, site]) writeDir(dir);
    writeCloudBuild(app, config!);
    writeCloudBuild(site, cloudConfig({ DROPPEN_WEB_MODE: "cloud", DROPPEN_WEB_ROLE: "site" })!);
    assert.equal(readFileSync(join(app, "index.html"), "utf8"), readFileSync(join(site, "index.html"), "utf8"), "the marked shell");
    assert.equal(readFileSync(join(app, HOSTS_PATH), "utf8"), readFileSync(join(site, HOSTS_PATH), "utf8"), "the hosts document");
    assert.notEqual(readFileSync(join(app, "_headers"), "utf8"), readFileSync(join(site, "_headers"), "utf8"), "the headers differ");
    assert.equal(readFileSync(join(app, "sw.js"), "utf8"), REAL_WORKER, "the app keeps its own worker");
    assert.equal(readFileSync(join(site, "sw.js"), "utf8"), RETIRING_WORKER, "the site retires any worker left there");
    assert.deepEqual(readdirSync(app).sort(), [".well-known", "_headers", "assets", "index.html", "sw.js"]);
    assert.deepEqual(readdirSync(site).sort(), [".well-known", "_headers", "assets", "index.html", "sw.js"]);
  } finally {
    rmSync(app, { recursive: true, force: true });
    rmSync(site, { recursive: true, force: true });
  }
});
