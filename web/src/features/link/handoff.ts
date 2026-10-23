/*
 * Signing a computer in to Droppen from its local settings
 * (docs/local-aggregator.md §3.3–3.4): the messages between the local page
 * (the opener, on the computer's own origin) and the droppen.app popup at
 * `/link`, and the rules both sides check them by.
 *
 * The popup says `ready` to its opener (`targetOrigin` "*": it carries
 * nothing). The opener answers only a window it opened itself, and only
 * when the message came from the hub's web origin, with the `request`: a
 * nonce, the machine's public keys and the aggregator's. The popup remembers
 * where the request came from — `event.origin`, which the browser sets and
 * nobody can forge — as `cb`, and from then on talks to that origin only.
 * After the person consents it hands the enrollment code back (`code`,
 * `RH1-…`; `DR1-…` is read too), then `done`. Every message carries `v: 1`;
 * a message of an unknown type, from an unknown window or origin, or with
 * another nonce, is ignored.
 *
 * Nothing secret ever goes in a URL: the popup opens at `/link?v=1`.
 */

import { fromHex, hex, sha256, utf8 } from "@/lib/e2e";

export const LINK_VERSION = 1;
/** The popup's path on the hub. */
export const LINK_PATH = "/link";
/** `window.open`'s name and features for it. */
export const LINK_WINDOW = "droppen-link";
export const LINK_FEATURES = "popup,width=480,height=760";

/** The machine's keys as the local server reported them (hex). */
export interface LinkMachine {
  hostname: string;
  signKey: string;
  dhKey: string;
  /** Set when the computer is in an account already: the "only add the aggregator" variant. */
  enrolled?: { accountFp: string; id: string } | null;
}

/** The aggregator's pending keys (hex). */
export interface LinkAggregator {
  signKey: string;
  dhKey: string;
}

export type LinkMessage =
  | { t: "rh-link/ready"; v: 1 }
  | { t: "rh-link/request"; v: 1; nonce: string; machine: LinkMachine; aggregator: LinkAggregator | null; aggregate: boolean }
  | { t: "rh-link/code"; v: 1; nonce: string; code: string; email?: string }
  | { t: "rh-link/done"; v: 1; nonce: string; account: string; email: string; machine: string; aggregator?: string | null }
  | { t: "rh-link/cancel"; v: 1; nonce: string }
  | { t: "rh-link/error"; v: 1; nonce: string; code: string; message: string };

const isHex = (s: unknown, len: number): s is string => typeof s === "string" && s.length === len && /^[0-9a-f]+$/.test(s);
const isText = (s: unknown, max = 512): s is string => typeof s === "string" && s.length <= max;

/** The longest host name the consent page shows. */
export const MAX_HOSTNAME = 64;

/**
 * A host name as the consent page shows it: what the computer says, for
 * reference only — so plain and short. Control and bidirectional-override
 * characters and `<>"'` are dropped; at most {@link MAX_HOSTNAME} characters.
 */
export function plainHostname(text: string): string {
  return [...text.replace(/[\u0000-\u001f\u007f-\u009f\u200b-\u200f\u202a-\u202e\u2066-\u2069\ufeff<>"']/g, "")].slice(0, MAX_HOSTNAME).join("");
}

function machineOf(value: unknown): LinkMachine | null {
  if (!value || typeof value !== "object") return null;
  const m = value as Record<string, unknown>;
  if (!isText(m.hostname, 255) || !isHex(m.signKey, 64) || !isHex(m.dhKey, 64)) return null;
  const hostname = plainHostname(m.hostname);
  let enrolled: LinkMachine["enrolled"] = null;
  if (m.enrolled !== undefined && m.enrolled !== null) {
    const e = m.enrolled as Record<string, unknown>;
    if (!isHex(e.accountFp, 32) || !isText(e.id, 64)) return null;
    enrolled = { accountFp: e.accountFp, id: e.id };
  }
  return { hostname, signKey: m.signKey, dhKey: m.dhKey, enrolled };
}

function aggregatorOf(value: unknown): LinkAggregator | null | undefined {
  if (value === null || value === undefined) return null;
  const a = value as Record<string, unknown>;
  return isHex(a.signKey, 64) && isHex(a.dhKey, 64) ? { signKey: a.signKey, dhKey: a.dhKey } : undefined;
}

/** A message from the other side, if it is one this version understands; anything else is `null` (ignored). */
export function parseLinkMessage(data: unknown): LinkMessage | null {
  if (!data || typeof data !== "object") return null;
  const m = data as Record<string, unknown>;
  if (m.v !== LINK_VERSION || typeof m.t !== "string") return null;
  switch (m.t) {
    case "rh-link/ready":
      return { t: m.t, v: 1 };
    case "rh-link/request": {
      const machine = machineOf(m.machine);
      const aggregator = aggregatorOf(m.aggregator);
      if (!isHex(m.nonce, 32) || !machine || aggregator === undefined) return null;
      return { t: m.t, v: 1, nonce: m.nonce, machine, aggregator, aggregate: m.aggregate === true && aggregator !== null };
    }
    case "rh-link/code":
      if (!isHex(m.nonce, 32) || !isText(m.code, 128) || !/^(RH1|DR1)-/i.test(m.code)) return null;
      return { t: m.t, v: 1, nonce: m.nonce, code: m.code, ...(isText(m.email, 254) ? { email: m.email } : {}) };
    case "rh-link/done":
      if (!isHex(m.nonce, 32) || !isText(m.account, 64) || !isText(m.email, 254) || !isText(m.machine, 64)) return null;
      return { t: m.t, v: 1, nonce: m.nonce, account: m.account, email: m.email, machine: m.machine, aggregator: isText(m.aggregator, 64) ? m.aggregator : null };
    case "rh-link/cancel":
      return isHex(m.nonce, 32) ? { t: m.t, v: 1, nonce: m.nonce } : null;
    case "rh-link/error":
      if (!isHex(m.nonce, 32) || !isText(m.code, 64) || !isText(m.message, 1000)) return null;
      return { t: m.t, v: 1, nonce: m.nonce, code: m.code, message: m.message };
    default:
      return null;
  }
}

/**
 * The fingerprint both sides show for the machine: the first 16 bytes of
 * SHA-256("rh/v1 machine fingerprint\0" ‖ signing key ‖ channel key), as 8
 * groups of 4 hex digits (`machine_fingerprint` in `server-rs/src/agent/control.rs`).
 */
export async function machineFingerprint(signKey: Uint8Array, dhKey: Uint8Array): Promise<string> {
  const digest = (await sha256(utf8("rh/v1 machine fingerprint\0"), signKey, dhKey)).slice(0, 16);
  const text = hex(digest);
  return text.match(/.{4}/g)!.join(" ");
}

/** The same from the hex keys a request carries. */
export function machineFingerprintHex(signKey: string, dhKey: string): Promise<string> {
  return machineFingerprint(fromHex(signKey), fromHex(dhKey));
}

/**
 * How the popup treats the origin that asked (`cb`):
 * - `local`: this computer itself — `localhost`, `*.localhost`, 127.0.0.0/8,
 *   `[::1]`. Nobody but this computer answers there.
 * - `warn`: an address a computer of one's own usually has — a private,
 *   link-local or carrier-grade (100.64/10, Tailscale) address, a name with
 *   no dot, `*.local`, `*.lan`, `*.home.arpa`, `*.internal`, `*.ts.net` — a red
 *   warning, an explicit "this is my own device", and the fingerprint the
 *   computer's settings show. Whoever answers on the network the browser is
 *   on can answer there, and `*.ts.net` is public (Tailscale Funnel gives
 *   anyone one): a fake settings page there must not get a code for one
 *   unwary click.
 * - `public`: any other host — a public name or address. Droppen on a
 *   computer of one's own is rarely there (a reverse proxy under one's own
 *   domain), and a phishing page always is: the warning, and the host name
 *   typed out to confirm it, not a tick ({@link confirmsHost}).
 * - `refuse`: not an http(s) origin at all, the hub's own origin, or the
 *   hub's host on another scheme or port (a page there asking for a code for
 *   itself) — unless that host is loopback, where a test hub and the
 *   computer share it. Where the hub names its hosts (`hosts`, from
 *   `/.well-known/droppen-hub`: `lib/hub/hosts.ts`), any of them too — its
 *   origin, the app host, the site host — and any name under its origin's
 *   (`*.droppen.app`: relays, the Turnstile page): none of them is a
 *   device's own page.
 *
 * Known and accepted for now (docs/local-aggregator.md §3.4): another program
 * on this computer's loopback, on another port, is "local" too.
 */
export type CallbackTier = "local" | "warn" | "public" | "refuse";

/**
 * Names that are not on the public internet: mDNS, the home network's, private
 * use, a tailnet's. The local server answers to the same ones
 * (`LOCAL_SUFFIXES` in `server-rs/src/origin.rs`).
 */
const PRIVATE_SUFFIXES = [".local", ".lan", ".home.arpa", ".internal", ".ts.net"];

/** Whether `host` (a URL's hostname, IPv6 in brackets) is one of the networks a computer of one's own sits on. */
export function privateHost(host: string): boolean {
  const name = host.toLowerCase().replace(/\.$/, "");
  const v4 = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(name);
  if (v4) {
    const [a, b] = [Number(v4[1]), Number(v4[2])];
    return a === 10 || a === 127 || a === 0 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 169 && b === 254) || (a === 100 && b >= 64 && b <= 127);
  }
  if (name.startsWith("[")) {
    const v6 = name.slice(1, -1);
    // Unique local (fc00::/7), link-local (fe80::/10), loopback, unspecified, and IPv4-mapped (as the URL parser writes it).
    if (/^f[cd][0-9a-f]{0,2}:/.test(v6) || /^fe[89ab][0-9a-f]?:/.test(v6) || v6 === "::1" || v6 === "::") return true;
    const mapped = /^::ffff:([0-9a-f]{1,4}):([0-9a-f]{1,4})$/.exec(v6);
    if (mapped) {
      const hi = parseInt(mapped[1]!, 16);
      const lo = parseInt(mapped[2]!, 16);
      return privateHost(`${hi >> 8}.${hi & 255}.${lo >> 8}.${lo & 255}`);
    }
    return false;
  }
  return !name.includes(".") || PRIVATE_SUFFIXES.some((suffix) => name.endsWith(suffix));
}

/** Whether `host` (lower-case) is one of the hub's names, or a name under its origin's ({@link callbackTier}). */
function hubName(host: string, hosts: HubHostNames): boolean {
  const name = (origin: string) => {
    try {
      return new URL(origin.replace(/^ws(s?):/, "http$1:")).hostname.toLowerCase();
    } catch {
      return null;
    }
  };
  const apex = name(hosts.origin);
  const names = [hosts.origin, hosts.app, hosts.site].map((o) => (o ? name(o) : null));
  return names.includes(host) || (apex !== null && host.endsWith(`.${apex}`));
}

/** The hub's names `callbackTier` refuses besides the page's own (`HubHosts` of `lib/hub/hosts.ts`). */
export interface HubHostNames {
  /** `wss://droppen.app` */
  origin: string;
  app: string | null;
  site: string | null;
}

export function callbackTier(cb: string, hubWeb: string, hosts: HubHostNames | null = null): CallbackTier {
  let url: URL;
  try {
    url = new URL(cb);
  } catch {
    return "refuse";
  }
  if (url.origin !== cb || (url.protocol !== "http:" && url.protocol !== "https:")) return "refuse";
  let hub: URL | null = null;
  try {
    hub = new URL(hubWeb);
  } catch {
    hub = null;
  }
  if (hub === null || url.origin === hub.origin) return "refuse";
  const host = url.hostname.toLowerCase();
  const loopback = host === "localhost" || host.endsWith(".localhost") || host === "[::1]" || /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host);
  // The hub's host on another scheme or port is the hub's too — but a hub
  // run on this computer for testing shares its loopback with the computer.
  if (!loopback && host === hub.hostname.toLowerCase()) return "refuse";
  if (!loopback && hosts && hubName(host, hosts)) return "refuse";
  if (loopback) return "local";
  return privateHost(host) ? "warn" : "public";
}

/** A host name as typed or shown, for comparing: no case, blanks, brackets or trailing dot. */
const bareHost = (host: string) => host.trim().toLowerCase().replace(/^\[(.*)\]$/, "$1").replace(/\.$/, "");

/** The host name a `public` origin is confirmed by typing (no port: the name is what the person must read). */
export function hostToType(cb: string): string {
  try {
    return bareHost(new URL(cb).hostname);
  } catch {
    return "";
  }
}

/** Whether what was typed confirms a `public` origin: its host name ({@link hostToType}), in any case. */
export function confirmsHost(typed: string, cb: string): boolean {
  const host = hostToType(cb);
  return host !== "" && bareHost(typed) === host;
}

/** The hub's web origin for a hub address someone typed (`https://…`, `wss://…`). `null` when it is none. */
export function hubWebOrigin(input: string): string | null {
  const text = input.trim();
  if (!text) return null;
  try {
    const url = new URL(text.replace(/^wss:/i, "https:").replace(/^ws:/i, "http:"));
    if (url.protocol !== "https:" && url.protocol !== "http:") return null;
    return url.origin;
  } catch {
    return null;
  }
}

/*
 * The browser hand-off (docs/local-aggregator.md §4.9;
 * `server-rs/src/api/link_handoff.rs`): when there is no opener to post to —
 * the Droppen desktop app's window, or a browser that blocks the popup — the
 * local page has the browser open `<hub>/link#droppen-link=<payload>`, and
 * droppen.app sends the browser back to `<origin>/link/return#droppen-return=…`
 * with the enrollment code sealed to the attempt's one-off key
 * (`lib/e2e/link-return.ts`). Both live in fragments only, which never reach
 * a server; each page takes its fragment out of the address before anything
 * else reads it (`fragment-boot.ts`).
 */

/** The fragment key of the droppen.app address. */
export const HANDOFF_FRAGMENT = "droppen-link";
/** The local page that takes droppen.app's answer back. */
export const RETURN_PATH = "/link/return";
/** The fragment key of the address droppen.app sends the browser back to. */
export const RETURN_FRAGMENT = "droppen-return";

/** A hand-off as droppen.app reads it from the address. */
export interface HandoffRequest {
  /** The sign-in attempt (32 hex digits). */
  attempt: string;
  /** The attempt's one-off X25519 key (64 hex digits). */
  key: string;
  /** The attempt's state (32 hex digits): only the page that opened the attempt was given it. */
  state: string;
  /** The device's page: where the answer goes back to (`http://127.0.0.1:51234`). */
  origin: string;
  /** When the attempt expires (ms, by the device's clock). */
  expiresMs: number;
  machine: LinkMachine;
}

/** Bytes as unpadded base64url. */
export function base64Url(data: Uint8Array): string {
  let bin = "";
  for (const b of data) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** Unpadded base64url as bytes, or `null`. */
export function fromBase64Url(text: string): Uint8Array<ArrayBuffer> | null {
  if (!/^[A-Za-z0-9_-]*$/.test(text)) return null;
  try {
    const b64 = text.replace(/-/g, "+").replace(/_/g, "/");
    const bin = atob(b64 + "=".repeat((4 - (b64.length % 4)) % 4));
    return Uint8Array.from(bin, (c) => c.charCodeAt(0));
  } catch {
    return null;
  }
}

/** A plain `http(s)://host[:port]` origin, or `null`. */
export function plainOrigin(text: unknown): string | null {
  if (typeof text !== "string" || text.length > 255) return null;
  try {
    const url = new URL(text);
    if ((url.protocol !== "http:" && url.protocol !== "https:") || url.origin !== text) return null;
    return url.origin;
  } catch {
    return null;
  }
}

/** The payload (`server-rs/src/api/link_handoff.rs`, `Payload`) as a hand-off, if it is a well-formed one. */
export function handoffOf(value: unknown): HandoffRequest | null {
  if (!value || typeof value !== "object") return null;
  const p = value as Record<string, unknown>;
  if (p.v !== 1 || !isHex(p.a, 32) || !isHex(p.k, 64) || !isHex(p.s, 32) || typeof p.x !== "number" || !Number.isFinite(p.x)) return null;
  const origin = plainOrigin(p.o);
  const m = p.m as Record<string, unknown> | undefined;
  if (!origin || !m) return null;
  const e = m.e as Record<string, unknown> | undefined;
  const machine = machineOf({ hostname: m.h, signKey: m.s, dhKey: m.d, enrolled: e ? { accountFp: e.f, id: e.i } : null });
  if (!machine) return null;
  return { attempt: p.a, key: p.k, state: p.s, origin, expiresMs: p.x, machine };
}

/** The same hand-off as its payload (what {@link handoffOf} reads): for keeping it in this tab. */
export function handoffPayload(h: HandoffRequest): unknown {
  const e = h.machine.enrolled;
  return { v: 1, a: h.attempt, k: h.key, s: h.state, o: h.origin, x: h.expiresMs, m: { h: h.machine.hostname, s: h.machine.signKey, d: h.machine.dhKey, ...(e ? { e: { f: e.accountFp, i: e.id } } : {}) } };
}

/** The `#droppen-link=…` fragment of droppen.app's address as a hand-off, or `null`. */
export function parseHandoff(hash: string): HandoffRequest | null {
  const text = hash.replace(/^#/, "");
  const prefix = `${HANDOFF_FRAGMENT}=`;
  if (!text.startsWith(prefix)) return null;
  const raw = fromBase64Url(text.slice(prefix.length));
  if (!raw || raw.length > 4096) return null;
  try {
    return handoffOf(JSON.parse(new TextDecoder().decode(raw)));
  } catch {
    return null;
  }
}

/** Where droppen.app sends the browser back with its sealed answer. */
export function returnAddress(origin: string, attempt: string, sealed: Uint8Array): string {
  return `${origin}${RETURN_PATH}#${RETURN_FRAGMENT}=${attempt}.${base64Url(sealed)}`;
}

/** The `#droppen-return=<attempt>.<sealed>` fragment of the return address, or `null`. */
export function parseReturn(hash: string): { attempt: string; sealed: string } | null {
  const text = hash.replace(/^#/, "");
  const prefix = `${RETURN_FRAGMENT}=`;
  if (!text.startsWith(prefix)) return null;
  const [attempt, sealed, ...rest] = text.slice(prefix.length).split(".");
  if (rest.length || !isHex(attempt, 32) || !sealed || sealed.length > 2048 || !fromBase64Url(sealed)) return null;
  return { attempt, sealed };
}

/** Whether the local page is the one that takes droppen.app's answer back. */
export function isReturnPath(pathname: string): boolean {
  return (pathname.replace(/\/+$/, "") || "/") === RETURN_PATH;
}
