import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import type { Plugin, ResolvedConfig } from "vite";
import { inlineScriptSources } from "./service-worker";

/**
 * `DROPPEN_WEB_MODE=cloud`: an optional, static-hosting build of the same web
 * app (a Cloudflare Pages-style `dist`), on top of the normal reproducible
 * build — never instead of it. The default build is untouched, byte for byte
 * (docs/web-integrity.md): no marker, no cloud files, and it is the one a
 * release attests.
 *
 * What the cloud build adds, after vite has written every file (and after
 * `fillServiceWorker` has hashed `index.html` as built):
 *
 * - the `rh-hub` marker in the shell's `<head>`, naming the canonical machine
 *   hub (`wss://api.droppen.app` by default) — the one source of truth for
 *   cloud mode, whose `https://` form is the HTTP API's base
 *   (`web/src/lib/hub/config.ts`);
 * - `dist/.well-known/droppen-hub`: the hub's hosts, the same answer a hub
 *   would give (`docs/hub-api/http.md`);
 * - `dist/_headers`: CSP, Permissions-Policy, Referrer-Policy and HSTS for a
 *   static host, with the shell's inline scripts allowed by the hash routine
 *   the build already uses for the service worker. `DROPPEN_WEB_ROLE` picks
 *   the host's role: `app` (the default, `my.`) allows WebAuthn on itself,
 *   frames Turnstile and calls the API cross-origin; `site` (`www.`/apex)
 *   allows no WebAuthn, frames nothing and calls only itself. That one file
 *   is the only difference between the two roles' dists.
 *
 * Every value is configuration: the defaults are droppen.app's, and an
 * operator may point a cloud build at another set of hosts. Nothing here is
 * compiled into a normal (`DROPPEN_WEB_MODE` unset) build, so self-hosted
 * servers and the open distribution are unaffected.
 */

/** The environment variable that opts a build into the cloud output. */
export const CLOUD_MODE_ENV = "DROPPEN_WEB_MODE";

export interface CloudConfig {
  /** The canonical machine hub, as the `rh-hub` marker names it (`wss://…`). */
  hubOrigin: string;
  /** The app's origin (`https://my.droppen.app`). */
  appOrigin: string;
  /** The public pages' origin (`https://www.droppen.app`). */
  siteOrigin: string;
  /** The Turnstile page the app frames, if any. */
  turnstileOrigin: string;
  /** The relays the page dials (`https://`/`wss://` origins; `*.` allowed once, leftmost). */
  relays: string[];
  /**
   * Which host this dist is deployed as: `app` (`my.`, WebAuthn and the
   * Turnstile frame, cross-origin API) or `site` (`www.`/apex: no WebAuthn, no
   * frame, no API calls — only this host's own files). Everything but
   * `_headers` is the same in both.
   */
  role: "app" | "site";
}

const DEFAULTS: CloudConfig = {
  hubOrigin: "wss://api.droppen.app",
  appOrigin: "https://my.droppen.app",
  siteOrigin: "https://www.droppen.app",
  turnstileOrigin: "https://verify.droppen.app",
  relays: ["https://*.relay.droppen.app", "wss://*.relay.droppen.app"],
  role: "app",
};

const HUB_ORIGIN = /^wss?:\/\/[^/?#\s]+$/;
const WEB_ORIGIN = /^https?:\/\/[^/?#\s]+$/;
/** A host source a CSP `connect-src` takes: an origin, or one `*.` label. */
const RELAY_ORIGIN = /^(https|wss):\/\/(\*\.)?[a-z0-9-]+(\.[a-z0-9-]+)+$/i;

/** `value` as a bare origin (`scheme://host[:port]`, no path, no user info), or `null`. */
function originOf(value: string, shape: RegExp): string | null {
  if (!shape.test(value)) return null;
  try {
    const url = new URL(value);
    if (url.username || url.password || url.pathname !== "/" || url.search || url.hash) return null;
    return `${url.protocol}//${url.host}` === value.toLowerCase() || `${url.protocol}//${url.host}` === value ? `${url.protocol}//${url.host}` : null;
  } catch {
    return null;
  }
}

function fail(name: string, value: string, want: string): never {
  throw new Error(`${name}=${JSON.stringify(value)} is not ${want}`);
}

/**
 * The cloud configuration, or `null` for the normal (self-hosted, attested)
 * build. `env` is `process.env` by default; a value that is set but not an
 * origin fails the build rather than writing something a browser cannot use.
 */
export function cloudConfig(env: Record<string, string | undefined> = process.env): CloudConfig | null {
  if (env[CLOUD_MODE_ENV] !== "cloud") return null;
  const hub = originOf(env.DROPPEN_WEB_HUB_ORIGIN ?? DEFAULTS.hubOrigin, HUB_ORIGIN);
  if (!hub) fail("DROPPEN_WEB_HUB_ORIGIN", env.DROPPEN_WEB_HUB_ORIGIN ?? DEFAULTS.hubOrigin, "a wss:// origin");
  const app = originOf(env.DROPPEN_WEB_APP_ORIGIN ?? DEFAULTS.appOrigin, WEB_ORIGIN);
  if (!app) fail("DROPPEN_WEB_APP_ORIGIN", env.DROPPEN_WEB_APP_ORIGIN ?? DEFAULTS.appOrigin, "an https:// origin");
  const site = originOf(env.DROPPEN_WEB_SITE_ORIGIN ?? DEFAULTS.siteOrigin, WEB_ORIGIN);
  if (!site) fail("DROPPEN_WEB_SITE_ORIGIN", env.DROPPEN_WEB_SITE_ORIGIN ?? DEFAULTS.siteOrigin, "an https:// origin");
  const turnstile = originOf(env.DROPPEN_WEB_TURNSTILE_ORIGIN ?? DEFAULTS.turnstileOrigin, WEB_ORIGIN);
  if (!turnstile) fail("DROPPEN_WEB_TURNSTILE_ORIGIN", env.DROPPEN_WEB_TURNSTILE_ORIGIN ?? DEFAULTS.turnstileOrigin, "an https:// origin");
  const relays = (env.DROPPEN_WEB_RELAYS ?? DEFAULTS.relays.join(" ")).split(/\s+/).filter(Boolean);
  for (const relay of relays) if (!RELAY_ORIGIN.test(relay)) fail("DROPPEN_WEB_RELAYS", relay, "https:// or wss:// origins (one `*.` label allowed)");
  const role = env.DROPPEN_WEB_ROLE ?? DEFAULTS.role;
  if (role !== "app" && role !== "site") fail("DROPPEN_WEB_ROLE", role, "app or site");
  return { hubOrigin: hub, appOrigin: app, siteOrigin: site, turnstileOrigin: turnstile, relays, role };
}

/** The `https://` form of a `wss://` (or `ws://`) origin: where the HTTP API is. */
export function httpOf(origin: string): string {
  return origin.replace(/^ws(s?):\/\//, "http$1://");
}

/**
 * `html` with the hub's marker as the first thing after its `<head>`, exactly
 * as a hub writes it (and as `scripts/verify-web.mjs` removes it again).
 */
export function markShell(html: string, origin: string): string {
  if (html.includes('name="rh-hub"')) throw new Error("index.html already carries an rh-hub marker");
  const tag = `<meta name="rh-hub" content="${origin}" />`;
  const head = html.indexOf("<head>");
  return head < 0 ? tag + html : html.slice(0, head + "<head>".length) + tag + html.slice(head + "<head>".length);
}

/** `GET /.well-known/droppen-hub`, as a hub answers it (docs/hub-api/http.md). */
export function hubDocument(config: CloudConfig): string {
  return `${JSON.stringify({ origin: config.hubOrigin, app: config.appOrigin, site: config.siteOrigin }, null, 2)}\n`;
}

/**
 * The CSP a static host serves the shell with: the hub's own policy
 * (docs/hub-api/http.md, "Hosts") with the shell's inline scripts by hash.
 * The app role may dial the HTTP and socket API origins and the relays, and
 * frame the Turnstile page; the site role dials nothing but itself and frames
 * nothing (docs/hub-api/README.md, "Hosts").
 */
export function contentSecurityPolicy(html: string, config: CloudConfig): string {
  const scripts = inlineScriptSources(html);
  const app = config.role === "app";
  return [
    "default-src 'self'",
    ["script-src 'self'", ...scripts].join(" "),
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' data: blob:",
    "font-src 'self' data:",
    app ? ["connect-src 'self'", httpOf(config.hubOrigin), config.hubOrigin, ...config.relays].join(" ") : "connect-src 'self'",
    "worker-src 'self' blob:",
    "manifest-src 'self'",
    `frame-src ${app ? config.turnstileOrigin : "'none'"}`,
    "frame-ancestors 'none'",
    "base-uri 'self'",
    "form-action 'self'",
    "object-src 'none'",
  ].join("; ");
}

/**
 * `dist/_headers`, in the format a static host (Cloudflare Pages) reads: the
 * policy for every file, the shell's inline scripts allowed by hash.
 *
 * `Permissions-Policy` allows WebAuthn only on the app host (`my.droppen.app`):
 * the site role sends `=()` instead (docs/hub-api/README.md, "Hosts").
 */
export function headersFile(html: string, config: CloudConfig): string {
  const permissions =
    config.role === "app"
      ? "publickey-credentials-get=(self), publickey-credentials-create=(self)"
      : "publickey-credentials-get=(), publickey-credentials-create=()";
  return [
    "/*",
    `  Content-Security-Policy: ${contentSecurityPolicy(html, config)}`,
    `  Permissions-Policy: ${permissions}`,
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

/** Write the marker and the two cloud files into a finished `dist`. */
export function writeCloudBuild(dir: string, config: CloudConfig): void {
  const shellPath = join(dir, "index.html");
  const shell = markShell(readFileSync(shellPath, "utf8"), config.hubOrigin);
  writeFileSync(shellPath, shell);
  const wellKnown = join(dir, ".well-known");
  mkdirSync(wellKnown, { recursive: true });
  writeFileSync(join(wellKnown, "droppen-hub"), hubDocument(config));
  writeFileSync(join(dir, "_headers"), headersFile(shell, config));
}

/**
 * The vite plugin: `configResolved` records where the build goes, and
 * `closeBundle` runs after every `writeBundle` (the service worker's table is
 * filled by then), so the marker never enters the shell the worker hashed.
 */
export function cloudBuild(config: CloudConfig | null): Plugin {
  let outDir = "dist";
  return {
    name: "droppen-cloud-build",
    apply: "build",
    configResolved(resolved: ResolvedConfig) {
      outDir = resolve(resolved.root, resolved.build.outDir);
    },
    closeBundle() {
      if (config) writeCloudBuild(outDir, config);
    },
  };
}
