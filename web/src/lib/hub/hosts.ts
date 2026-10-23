/*
 * Which of its hub's host names this page is on (docs/hub-api/README.md,
 * "Hosts").
 *
 * A hub may serve the web app on a host of its own (the app host, e.g.
 * `https://my.droppen.app`) and its public pages on another (the site host,
 * `https://www.droppen.app`), and keep its origin (`droppen.app`) for what
 * devices that run AI and relays dial. It says so at
 * `GET /.well-known/droppen-hub` ({@link HubHosts}), the same answer on every
 * host it serves. The page reads it once, before anything else of cloud mode
 * starts, and runs in one of four modes ({@link HostMode}):
 *
 * - `single`: the hub serves everything on one host, as it always did — the
 *   hub does not answer (an older hub, a self-hosted one), names no app host,
 *   or this is its origin while it still serves the app there. Nothing
 *   changes from before.
 * - `app`: the app host. `/` is the app (the sessions, or the sign-in), and
 *   the public pages are on the site host.
 * - `site`: the site host. Only the public pages, with no key store, no
 *   service worker and no sign-in here: every way into the app is a link to
 *   the app host.
 * - `legacy`: the hub's origin, once the app has moved off it: the move
 *   notice (`features/hosts/legacy.tsx`), which offers a browser that holds a
 *   device here to move it, and sends any other on to the app host.
 *
 * The answer only picks which of the build's own pages run where: every
 * request still goes to the page's own host, and the hub it signs in to is
 * still the shell's marker (`config.ts`).
 */
import { hubOrigin } from "./config";

/** Where the hub says which of its names serves what. */
export const HUB_HOSTS_PATH = "/.well-known/droppen-hub";

/** What the hub's origin does with the app's pages (`apex_mode`). */
export type ApexMode = "app" | "move" | "redirect";

/** `GET /.well-known/droppen-hub`. */
export interface HubHosts {
  /** The hub's origin as devices that run AI dial it (`wss://droppen.app`). */
  origin: string;
  /** The web app's origin (`https://my.droppen.app`); `null`: the hub's own. */
  app: string | null;
  /** The public pages' origin (`https://www.droppen.app`); `null`: wherever the app is. */
  site: string | null;
  /**
   * The hub's origin still serves the app (`app`, also when the hub does not
   * say), shows the move notice in its place (`move`), or redirects its
   * pages to the app host (`redirect`).
   */
  apexMode: ApexMode;
}

export type HostMode = "single" | "app" | "site" | "legacy";

const WS_ORIGIN = /^wss?:\/\/[^/?#\s]+$/;
const WEB_ORIGIN = /^https?:\/\/[^/?#\s]+$/;

/** `value` as an origin (`scheme://host[:port]`), or `null` when it is not exactly one (a path, user info, a default port written out, …). */
function originOf(value: unknown, shape: RegExp): URL | null {
  if (typeof value !== "string" || !shape.test(value)) return null;
  try {
    const url = new URL(value);
    if (url.username || url.password) return null;
    return `${url.protocol}//${url.host}` === value.toLowerCase() ? url : null;
  } catch {
    return null;
  }
}

/** A host name for this computer itself, where a test hub runs (`localhost`, 127.0.0.0/8, `[::1]`). */
function loopbackHost(host: string): boolean {
  return host === "localhost" || host.endsWith(".localhost") || host === "[::1]" || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host);
}

/** An IP address rather than a name (v4, or v6 in brackets). */
function ipHost(host: string): boolean {
  return host.startsWith("[") || /^\d{1,3}(\.\d{1,3}){3}$/.test(host);
}

/** Whether `host` is `apex` or a name under it, at a label boundary (`my.droppen.app` under `droppen.app`; never `evildroppen.app`). */
function underApex(host: string, apex: string): boolean {
  return host === apex || (host.length > apex.length + 1 && host.endsWith(`.${apex}`));
}

/**
 * The answer of `GET /.well-known/droppen-hub`, for a page on `pageOrigin`,
 * or `null` when it is not one to act on — then the page runs on one host,
 * exactly as before. Fields it does not know are ignored.
 *
 * The answer is not part of the attested build, and it decides where the
 * page sends this browser's keys (the move) and its deep links (with their
 * fragments: a verification token, a browser hand-off), so it is held to the
 * names of the hub this page came from. The apex is the hub's origin
 * (`wss://droppen.app`): a name, not an address, of two labels or more, no
 * port. The page itself must be on the apex or under it. `app` and `site` are
 * `https://` names under the apex at a label boundary (`my.droppen.app`; or
 * the apex itself, the hub's own), with no port. Only a hub on this
 * computer's loopback, for tests, may answer `ws://` and `http://` loopback
 * origins with ports. Anything else — a parent domain, a lookalike
 * (`droppen.app.evil.com`, `evildroppen.app`), plain http, a port, an
 * address, user info — and the whole answer is ignored.
 */
export function parseHubHosts(raw: unknown, pageOrigin: string): HubHosts | null {
  if (typeof raw !== "object" || raw === null) return null;
  const r = raw as Record<string, unknown>;
  const originUrl = originOf(r.origin, WS_ORIGIN);
  let page: URL;
  try {
    page = new URL(pageOrigin);
  } catch {
    return null;
  }
  if (!originUrl) return null;
  const apex = originUrl.hostname;
  const loopback = loopbackHost(apex);
  if (loopback) {
    if (!loopbackHost(page.hostname)) return null;
  } else {
    if (originUrl.protocol !== "wss:" || originUrl.port || ipHost(apex) || !apex.includes(".") || page.protocol !== "https:" || !underApex(page.hostname, apex)) return null;
  }
  const name = (value: unknown): string | null | undefined => {
    if (value === null || value === undefined) return null;
    const url = originOf(value, WEB_ORIGIN);
    if (!url) return undefined;
    if (loopback) return loopbackHost(url.hostname) ? url.origin : undefined;
    return url.protocol === "https:" && !url.port && !ipHost(url.hostname) && underApex(url.hostname, apex) ? url.origin : undefined;
  };
  const app = name(r.app);
  const site = name(r.site);
  // A name that is there but not one of the hub's: the answer is not one to act on.
  if (app === undefined || site === undefined) return null;
  const apexMode: ApexMode = r.apex_mode === "move" || r.apex_mode === "redirect" ? r.apex_mode : "app";
  return { origin: `${originUrl.protocol}//${originUrl.host}`, app, site, apexMode };
}

/** A hub origin's web form: `wss://droppen.app` → `https://droppen.app`. */
export function webOf(origin: string): string {
  return origin.replace(/^ws(s?):/, "http$1:");
}

/** Which mode a page on `pageOrigin` runs in ({@link HostMode}). */
export function hostModeFor(hosts: HubHosts | null, pageOrigin: string): HostMode {
  if (!hosts) return "single";
  const page = pageOrigin.toLowerCase();
  const apex = webOf(hosts.origin);
  const app = hosts.app ?? apex;
  if (hosts.site && hosts.site !== app && page === hosts.site) return "site";
  if (app === apex) return "single";
  if (page === app) return "app";
  if (page === apex) return hosts.apexMode === "app" ? "single" : "legacy";
  // A name the hub did not list (a proxy in front of it, a development
  // server): everything on this one host, as before.
  return "single";
}

let state: { hosts: HubHosts | null; mode: HostMode } = { hosts: null, mode: "single" };

/** The hub's answer this page runs by (`null`: none, or not read yet). */
export function hubHosts(): HubHosts | null {
  return state.hosts;
}

/** This page's mode (`single` until {@link resolveHostMode} has read the hub's answer). */
export function hostMode(): HostMode {
  return state.mode;
}

/** Set the hub's answer and this page's mode (the page does it once; tests in turn). */
export function setHostMode(hosts: HubHosts | null, mode: HostMode = hosts ? hostModeFor(hosts, globalThis.location?.origin ?? "") : "single"): void {
  state = { hosts, mode };
}

/** How long the page waits for the hub's answer before it goes on as before ({@link HostMode} `single`). */
export const HUB_HOSTS_WAIT_MS = 8_000;

/**
 * `GET /.well-known/droppen-hub` on the page's own host: the answer, or
 * `null` on anything else — no such route (an older hub: a 404, or the app's
 * shell for an unknown path), a body that is not one, names that are not
 * the hub's ({@link parseHubHosts}), no network, or no answer within `waitMs`.
 */
export async function fetchHubHosts(
  fetchFn: typeof fetch = globalThis.fetch,
  waitMs = HUB_HOSTS_WAIT_MS,
  pageOrigin: string = globalThis.location?.origin ?? "",
): Promise<HubHosts | null> {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), waitMs);
  try {
    const res = await fetchFn(HUB_HOSTS_PATH, { credentials: "omit", cache: "no-store", headers: { accept: "application/json" }, signal: abort.signal });
    if (!res.ok || !/^application\/json\b/i.test(res.headers.get("content-type") ?? "")) return null;
    return parseHubHosts(await res.json(), pageOrigin);
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Read the hub's answer and settle this page's mode: `single` for a page no
 * hub serves (local mode: there is nothing to ask) and whenever the hub does
 * not answer.
 */
export async function resolveHostMode(fetchFn?: typeof fetch): Promise<HostMode> {
  if (!hubOrigin()) {
    setHostMode(null);
    return "single";
  }
  setHostMode(await fetchHubHosts(fetchFn));
  return state.mode;
}

/**
 * The app's origin when it is not this page's: on the site host and on the
 * hub's origin after the move. `null`: the app is here.
 */
export function appOrigin(): string | null {
  if ((state.mode !== "site" && state.mode !== "legacy") || !state.hosts) return null;
  return state.hosts.app ?? webOf(state.hosts.origin);
}

/** The public pages' origin when it is not this page's (the app host, with a site host). `null`: they are here. */
export function siteOrigin(): string | null {
  return state.mode === "app" && state.hosts?.site ? state.hosts.site : null;
}

/** `path` (with any query and fragment) in the app: on the app host when that is another, else as it is. */
export function appUrl(path: string): string {
  return `${appOrigin() ?? ""}${path}`;
}

/**
 * The host name the app is at, as a person types it (`my.droppen.app`): the
 * hub's app host, else this page's host in cloud mode, else droppen.app (a
 * page no hub serves, and tests).
 */
export function appHost(): string {
  const app = state.hosts?.app ?? (state.hosts ? webOf(state.hosts.origin) : null);
  if (app) return new URL(app).host;
  try {
    if (hubOrigin() && globalThis.location?.host) return globalThis.location.host;
  } catch {
    // no document
  }
  return "droppen.app";
}

/**
 * The hub address a device that runs AI is given (`DROPPEN_HUB`, `--hub`):
 * the hub's origin when it names one, else the marker the page has
 * (`wss://…`), each in its web form.
 */
export function agentHub(marker: string): string {
  return webOf(state.hosts?.origin ?? marker);
}

/**
 * Take away what lets a browser install this page as an app (the manifest,
 * the home-screen web app tags): on a host that does not run the app (the
 * site host, the hub's origin after the move), an app installed here would
 * open on the wrong host. The app is installed from the app host.
 */
export function notInstallable(doc: Pick<Document, "querySelectorAll"> | undefined = globalThis.document): void {
  for (const el of doc?.querySelectorAll('link[rel="manifest"], meta[name="apple-mobile-web-app-capable"], meta[name="mobile-web-app-capable"]') ?? []) el.remove();
}

/**
 * The two ends of the browser key move — the hub's origin, where a browser's
 * device was set up before the app moved, and the app host — whenever the hub
 * names an app host of its own and this page is on one of the two, whatever
 * the origin does with the app's pages (`apex_mode`): the origin's `/move`
 * page (the sending side of a move the app host asks for) works while the
 * origin still serves the app, so the move can be tried before the cutover.
 * `null` otherwise.
 */
export function moveOrigins(pageOrigin: string = globalThis.location?.origin ?? ""): { legacy: string; app: string } | null {
  const hosts = state.hosts;
  if (!hosts?.app) return null;
  const legacy = webOf(hosts.origin);
  if (hosts.app === legacy) return null;
  const page = pageOrigin.toLowerCase();
  return page === legacy || page === hosts.app ? { legacy, app: hosts.app } : null;
}
