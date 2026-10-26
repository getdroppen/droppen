// A browser with three sites for the key move's tests (`lib/move`): the
// legacy origin, the app host and another site. Windows deliver `postMessage`
// as browsers do — only when the target origin is the window's own (or `*`),
// a task later, structured-cloned, with `origin` the sender's and `source`
// the receiver's own handle on the sender — and know their opener. Each
// origin has its own `localStorage`, IndexedDB stores (in memory), Web Locks
// and tabs channel; one hub serves both, and signs a device in only for a
// `HubLogin` its key signed naming the host the request came to.
import { hex, fromHex } from "../src/lib/e2e/bytes.js";
import { type KeyStore, KEY_STORE_DB, KEY_STORE_STORE, loadDeviceKeys } from "../src/lib/e2e/keystore.js";
import { verifyHubLogin } from "../src/lib/e2e/sig.js";
import { SEEN_DB, SEEN_STORE } from "../src/lib/hub/seen-store.js";
import type { MoveEnv, MoveLocks, MoveStorage, PeerMessage, PeerWindow } from "../src/lib/move/env.js";
import { memoryRecordStore } from "../src/lib/move/stores.js";
import type { MoveEvent } from "../src/lib/move/types.js";

export const LEGACY = "https://droppen.test";
export const APP = "https://my.droppen.test";
export const EVIL = "https://evil.test";
export const ORIGINS = { legacy: LEGACY, app: APP };

export class MapStorage implements MoveStorage {
  readonly values = new Map<string, string>();
  get length() {
    return this.values.size;
  }
  key(i: number) {
    return [...this.values.keys()][i] ?? null;
  }
  getItem(k: string) {
    return this.values.get(k) ?? null;
  }
  setItem(k: string, v: string) {
    this.values.set(k, String(v));
  }
  removeItem(k: string) {
    this.values.delete(k);
  }
}

/** Web Locks for one origin: exclusive, `ifAvailable` honoured, the rest queued. */
export class FakeLocks implements MoveLocks {
  readonly held = new Set<string>();
  private waiting: (() => void)[] = [];
  async request<T>(name: string, options: { mode: "exclusive"; ifAvailable?: boolean }, callback: (lock: unknown) => Promise<T> | T): Promise<T> {
    while (this.held.has(name)) {
      if (options.ifAvailable) return callback(null);
      await new Promise<void>((resolve) => this.waiting.push(resolve));
    }
    this.held.add(name);
    try {
      return await callback({ name });
    } finally {
      this.held.delete(name);
      const waiting = this.waiting;
      this.waiting = [];
      for (const w of waiting) w();
    }
  }
  async query() {
    return { held: [...this.held].map((name) => ({ name })) };
  }
}

/** One origin's storage. */
export class Site {
  readonly storage = new MapStorage();
  readonly locks = new FakeLocks();
  readonly stores = new Map<string, ReturnType<typeof memoryRecordStore>>();
  readonly events: MoveEvent[] = [];
  pushDropped = 0;
  constructor(readonly origin: string) {}
  records(db: string, store: string) {
    const key = `${db}/${store}`;
    let s = this.stores.get(key);
    if (!s) {
      s = memoryRecordStore(db, store);
      this.stores.set(key, s);
    }
    return s;
  }
  /** The key store over this site's `rh-e2e` records. */
  keyStore(): KeyStore {
    const rs = this.records(KEY_STORE_DB, KEY_STORE_STORE);
    return {
      get: async (name) => rs.data?.get(name),
      put: async (name, value) => void (rs.data ??= new Map()).set(name, value),
      delete: async (name) => void rs.data?.delete(name),
      list: async (prefix) => [...(rs.data?.keys() ?? [])].filter((k) => k.startsWith(prefix)),
    };
  }
  /** Every record of `rh-e2e` (names), `[]` when the database is gone. */
  keyNames(): string[] {
    return [...(this.records(KEY_STORE_DB, KEY_STORE_STORE).data?.keys() ?? [])].sort();
  }
}

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

/** The hub both origins are served by. */
export class FakeHub {
  readonly devices = new Map<string, Uint8Array>();
  readonly tokens = new Map<string, { account: string; device: string }>();
  readonly nonces = new Set<string>();
  readonly loggedOut: string[] = [];
  readonly signIns: { host: string; account: string; device: string }[] = [];
  deny = false;
  /** `/v1/session` waits for this while it is set. */
  gate: Promise<void> | null = null;

  register(account: string, device: string, signKey: Uint8Array) {
    this.devices.set(`${account}/${device}`, signKey);
  }

  async fetch(pageOrigin: string, input: string, init: RequestInit = {}): Promise<Response> {
    const url = new URL(input, pageOrigin);
    const body = typeof init.body === "string" ? (JSON.parse(init.body) as Record<string, unknown>) : {};
    const bearer = (init.headers as Record<string, string> | undefined)?.authorization?.replace(/^Bearer /, "") ?? "";
    switch (url.pathname) {
      case "/v1/session/challenge": {
        const nonce = hex(crypto.getRandomValues(new Uint8Array(32)));
        this.nonces.add(nonce);
        return json({ nonce });
      }
      case "/v1/session": {
        if (this.gate) await this.gate;
        const { account, device, time_ms, nonce, signature } = body as { account: string; device: string; time_ms: number; nonce: string; signature: string };
        const key = this.devices.get(`${account}/${device}`);
        if (this.deny || !key || !this.nonces.delete(nonce)) return json({ error: "denied" }, 401);
        const ok = await verifyHubLogin({ role: "deviceSession", account, id: device, hub: `wss://${url.host}`, nonce: fromHex(nonce), timeMs: time_ms }, key, fromHex(signature));
        if (!ok) return json({ error: "denied" }, 401);
        const token = hex(crypto.getRandomValues(new Uint8Array(16)));
        this.tokens.set(token, { account, device });
        this.signIns.push({ host: url.host, account, device });
        return json({ account, device, token, expires_ms: Date.now() + 3_600_000 });
      }
      case "/v1/me": {
        const who = this.tokens.get(bearer);
        if (!who) return json({ error: "unauthorized" }, 401);
        const devices = [...this.devices.entries()].filter(([k]) => k.startsWith(`${who.account}/`)).map(([k, signKey]) => ({ id: k.slice(who.account.length + 1), sign_key: hex(signKey) }));
        return json({ account: who.account, devices });
      }
      case "/v1/logout":
        this.tokens.delete(bearer);
        this.loggedOut.push(bearer);
        return new Response(null, { status: 204 });
      default:
        return json({ error: "not_found" }, 404);
    }
  }
}

export class FakeWindow {
  closed = false;
  opener: FakeWindow | null = null;
  readonly listeners = new Set<(e: PeerMessage) => void>();
  /** Every message this window was handed (what it listens to or not). */
  readonly received: PeerMessage[] = [];
  constructor(
    readonly id: number,
    readonly origin: string,
    readonly path: string,
  ) {}
}

type Page = (env: MoveEnv, win: FakeWindow) => void;

export class World {
  readonly sites = new Map<string, Site>();
  readonly hub = new FakeHub();
  readonly windows: FakeWindow[] = [];
  readonly pages = new Map<string, Page>();
  blockPopups = false;
  /** The origins the hub pinned (`/.well-known/droppen-hub`). */
  pinned: { legacy: string; app: string } | null = ORIGINS;
  private readonly handles = new Map<string, PeerWindow>();

  site(origin: string): Site {
    let s = this.sites.get(origin);
    if (!s) {
      s = new Site(origin);
      this.sites.set(origin, s);
    }
    return s;
  }

  /** `holder`'s handle on `target` (one per pair, as a browser's `WindowProxy`). */
  handle(holder: FakeWindow, target: FakeWindow): PeerWindow {
    const key = `${holder.id}>${target.id}`;
    let h = this.handles.get(key);
    if (!h) {
      h = {
        postMessage: (data: unknown, targetOrigin: string) => {
          if (target.closed || holder.closed) return;
          if (targetOrigin !== "*" && targetOrigin !== target.origin) return;
          const cloned = structuredClone(data);
          setTimeout(() => {
            if (target.closed) return;
            const event = { origin: holder.origin, source: this.handle(target, holder), data: cloned };
            target.received.push(event);
            for (const l of [...target.listeners]) l(event);
          }, 0);
        },
        get closed() {
          return target.closed;
        },
        close: () => this.close(target),
      };
      this.handles.set(key, h);
    }
    return h;
  }

  /** A top-level window at `origin + path`, its page started (a task later). */
  window(url: string, opener: FakeWindow | null = null): FakeWindow {
    const u = new URL(url);
    const win = new FakeWindow(this.windows.length + 1, u.origin, u.pathname);
    win.opener = opener;
    this.windows.push(win);
    const page = this.pages.get(`${u.origin}${u.pathname}`);
    if (page) setTimeout(() => page(this.env(win), win), 0);
    return win;
  }

  close(win: FakeWindow): void {
    win.closed = true;
    win.listeners.clear();
  }

  closeAll(): void {
    for (const w of this.windows) this.close(w);
  }

  env(win: FakeWindow): MoveEnv {
    const site = this.site(win.origin);
    return {
      origin: win.origin,
      storage: site.storage,
      records: (db, store) => site.records(db, store),
      keyStore: () => site.keyStore(),
      locks: site.locks,
      tabs: { post: (e) => void site.events.push(e), subscribe: () => () => undefined },
      fetch: (input, init) => this.hub.fetch(win.origin, input, init),
      hub: `wss://${new URL(win.origin).host}`,
      pinned: this.pinned,
      open: (url) => (this.blockPopups ? null : this.handle(win, this.window(url, win))),
      opener: () => (win.opener ? this.handle(win, win.opener) : null),
      onMessage: (fn) => {
        win.listeners.add(fn);
        return () => win.listeners.delete(fn);
      },
      closeSelf: () => this.close(win),
      userActive: () => null,
      dropPushSubscription: async () => void site.pushDropped++,
    };
  }
}

export const ACCOUNT = "acct-1";
export const DEVICE = "dev-1";

/** A browser holding a device at `site`, registered at the hub: what each part of the move should carry, or not. */
export async function seedDevice(world: World, origin = LEGACY): Promise<{ signPublic: string }> {
  const site = world.site(origin);
  const ks = site.keyStore();
  const keys = (await loadDeviceKeys(ks)).keys;
  await ks.put("local-wrap/v1", await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]));
  await ks.put(`uk/${ACCOUNT}/0`, { iv: new Uint8Array(12), sealed: new Uint8Array([1, 2, 3]) });
  await site.records(SEEN_DB, SEEN_STORE).write([{ key: ACCOUNT, value: { v: 1, genesis: "ab".repeat(32), seen: null } }], "overwrite", 1);
  const s = site.storage;
  s.setItem(`rh.hub.device.${ACCOUNT}`, DEVICE);
  s.setItem(`rh.hub.devicekey.${ACCOUNT}`, hex(keys.signPublic));
  s.setItem(`rh.hub.ack.${ACCOUNT}.${DEVICE}`, "[]");
  s.setItem("rh.hub.session", JSON.stringify({ account: ACCOUNT, device: DEVICE, token: "old-token", expiresMs: Date.now() + 60_000 }));
  s.setItem("rh.hub.push.subscribed", JSON.stringify({ machine: "host-1", vapid: "vapid", seq: null }));
  s.setItem("rh.theme", "dark");
  s.setItem("rh.draft.note", "not moved");
  world.hub.register(ACCOUNT, DEVICE, keys.signPublic);
  world.hub.tokens.set("old-token", { account: ACCOUNT, device: DEVICE });
  return { signPublic: hex(keys.signPublic) };
}

export const tick = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

/** Wait until `ok()` holds (or fail after `ms`). */
export async function until(ok: () => boolean, ms = 5_000): Promise<void> {
  const end = Date.now() + ms;
  while (!ok()) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await tick(10);
  }
}
