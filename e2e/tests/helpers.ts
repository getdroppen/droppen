import { readFileSync } from "node:fs";
import { resolve } from "node:path";

export const APP = process.env.E2E_APP_URL ?? "https://my.droppen.test";
export const SITE = process.env.E2E_SITE_URL ?? "https://www.droppen.test";
export const APEX = process.env.E2E_APEX_URL ?? "https://droppen.test";
export const API = process.env.E2E_API_URL ?? "https://api.droppen.test";
export const EVIL = process.env.E2E_EVIL_URL ?? "https://evil.test";
export const HUB_WS = API.replace(/^https:/, "wss:");
export const HOSTS = { app: new URL(APP).host, site: new URL(SITE).host, apex: new URL(APEX).host, api: new URL(API).host };

export const HUB_LOG = process.env.E2E_HUB_LOG ?? resolve(process.cwd(), ".e2e", "hub-log.jsonl");

export interface HubEntry {
  ts: number;
  kind: "http" | "done" | "ws-open" | "ws-close" | "ws-message" | "ws-handshake" | "ws-rejected";
  method?: string;
  path?: string;
  origin?: string | null;
  allowed?: boolean;
  status?: number;
  host?: string | null;
  hub?: string;
  accepted?: boolean;
  text?: string;
}

/** Every line the fake hub appended so far (empty before the server starts). */
export function hubLog(): HubEntry[] {
  try {
    return readFileSync(HUB_LOG, "utf8")
      .split("\n")
      .filter(Boolean)
      .map((line) => JSON.parse(line) as HubEntry);
  } catch {
    return [];
  }
}

/** Wait for a log line; the specs observe requests, they do not assume they happened. */
export async function waitForHub(predicate: (entry: HubEntry) => boolean, timeout = 15_000): Promise<HubEntry> {
  const end = Date.now() + timeout;
  for (;;) {
    const hit = hubLog().find(predicate);
    if (hit) return hit;
    if (Date.now() > end) throw new Error(`no hub log entry matched; last entries: ${JSON.stringify(hubLog().slice(-6))}`);
    await new Promise((done) => setTimeout(done, 100));
  }
}

/** The `publickey-credentials-*` values from a Permissions-Policy header. */
export function webauthnPolicy(header: string | undefined): { get: string | null; create: string | null } {
  const value = (name: string) => {
    for (const part of (header ?? "").split(",")) {
      const at = part.indexOf("=");
      if (at < 0) continue;
      if (part.slice(0, at).trim().toLowerCase() === name) return part.slice(at + 1).trim();
    }
    return null;
  };
  return { get: value("publickey-credentials-get"), create: value("publickey-credentials-create") };
}

/** The sources of one CSP directive. */
export function cspSources(header: string | undefined, directive: string): string[] {
  for (const part of (header ?? "").split(";")) {
    const [name, ...sources] = part.trim().split(/\s+/);
    if (name?.toLowerCase() === directive) return sources;
  }
  return [];
}

/** Names of the IndexedDB databases this origin has (empty where the API is absent). */
export async function indexedDbNames(page: import("@playwright/test").Page): Promise<string[]> {
  return page.evaluate(async () => {
    const factory = indexedDB as IDBFactory & { databases?: () => Promise<{ name?: string }[]> };
    if (typeof factory.databases !== "function") return [];
    // Called on the factory: Firefox and WebKit throw on a detached method.
    return (await factory.databases()).map((db) => db.name ?? "");
  });
}
