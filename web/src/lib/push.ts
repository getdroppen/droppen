import type { MachineKey } from "@/lib/machines/key";
import { apiFor } from "@/lib/machines/registry";

/**
 * Web Push subscription for this browser (server-rs `push` module is the peer).
 *
 * Subscribing is the opt-in: whatever `pushManager.subscribe` hands back is
 * posted verbatim to the server, and notifications are raised by the push
 * handler in public/sw.js. Only production bundles register a worker at all
 * (lib/sw-register.ts), so under `vite dev` this reports itself unsupported.
 *
 * A browser holds one subscription, made against one device's VAPID key: the
 * device and that key are kept here ({@link PushRecord}). When that device
 * renews its identity key its agent makes a new VAPID key and forgets every
 * subscription (docs/host-identity-rotation.md §2.2 推送): the page unsubscribes
 * the old one — the push service drops its endpoint — and subscribes again
 * against the new key ({@link followRenewedKey}), with no prompt when
 * notifications are allowed already; where the browser wants a tap for that,
 * the page asks for one ({@link pushRenewalNeeded}). What stays: until a page
 * of this browser opens, whoever held the old VAPID key may still send the
 * old endpoint notifications — made-up ones, with nothing of the account's in
 * them (they read no content).
 */
export type PushState = "unsupported" | "denied" | "off" | "on";

/** What this browser's subscription is for: the device, its VAPID key then, and its renewal (chain seq) that key is known current for. */
export interface PushRecord {
  machine: MachineKey;
  vapid: string;
  seq: number | null;
}

/** What subscribing needs of the page: its service worker, the notification permission, storage, and the device's API. */
export interface PushEnv {
  ready(): Promise<Pick<ServiceWorkerRegistration, "pushManager"> | null>;
  permission(): NotificationPermission | "unsupported";
  storage: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null;
  api(machine: MachineKey): PushApi;
}

/** The routes of the device's API a subscription uses. */
export interface PushApi {
  GET(path: "/api/push/vapid"): Promise<{ data?: { publicKey: string } }>;
  POST(path: "/api/push/subscriptions", init: { body: { endpoint: string; keys: { p256dh: string; auth: string } } }): Promise<{ error?: { error: string } }>;
  DELETE(path: "/api/push/subscriptions", init: { body: { endpoint: string } }): Promise<unknown>;
}

// This browser's, not a device's (one subscription per browser): kept with the hub account's keys.
export const PUSH_RECORD_KEY = "rh.hub.push.subscribed";
const RECORD_KEY = PUSH_RECORD_KEY;
/** A device whose renewal left this browser without notifications, until a tap turns them back on. */
export const PUSH_RENEW_KEY = "rh.hub.push.renew";
const RENEW_KEY = PUSH_RENEW_KEY;

/**
 * `serviceWorker.ready` never settles when no worker was ever registered — a
 * dev bundle, or an insecure context where registration silently failed. Every
 * read races it so the settings row resolves instead of spinning forever.
 */
const READY_TIMEOUT_MS = 2000;

function supported(): boolean {
  return (
    "serviceWorker" in navigator && "PushManager" in window && "Notification" in window
  );
}

async function readyOrNull(): Promise<ServiceWorkerRegistration | null> {
  if (!supported()) return null;
  return Promise.race([
    navigator.serviceWorker.ready,
    new Promise<null>((resolve) => window.setTimeout(() => resolve(null), READY_TIMEOUT_MS)),
  ]);
}

function pageStorage(): Pick<Storage, "getItem" | "setItem" | "removeItem"> | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

/** The page's own: its worker, `Notification.permission`, `localStorage` and `apiFor`. */
export function browserPushEnv(): PushEnv {
  return {
    ready: readyOrNull,
    permission: () => (typeof Notification === "undefined" ? "unsupported" : Notification.permission),
    storage: pageStorage(),
    api: (machine) => apiFor(machine) as unknown as PushApi,
  };
}

export function readPushRecord(env: Pick<PushEnv, "storage"> = browserPushEnv()): PushRecord | null {
  try {
    const r = JSON.parse(env.storage?.getItem(RECORD_KEY) ?? "null") as Partial<PushRecord> | null;
    if (!r || typeof r.machine !== "string" || typeof r.vapid !== "string") return null;
    return { machine: r.machine, vapid: r.vapid, seq: typeof r.seq === "number" ? r.seq : null };
  } catch {
    return null;
  }
}

function writePushRecord(env: Pick<PushEnv, "storage">, record: PushRecord | null): void {
  try {
    if (record) env.storage?.setItem(RECORD_KEY, JSON.stringify(record));
    else env.storage?.removeItem(RECORD_KEY);
  } catch {
    // memory only: the next renewal is noticed when a page subscribes again
  }
}

function setRenewal(env: Pick<PushEnv, "storage">, machine: MachineKey | null): void {
  try {
    if (machine) env.storage?.setItem(RENEW_KEY, machine);
    else env.storage?.removeItem(RENEW_KEY);
  } catch {
    // memory only
  }
}

/**
 * After this browser's device moved here from another origin (`lib/move`):
 * the device its notifications came from there. The subscription itself
 * never moves — it belongs to the old origin's worker, and Notification
 * permission is the origin's — so a tap here turns them back on
 * ({@link pushMovedFrom}, `features/move/push-moved-notice.tsx`).
 */
export const PUSH_MOVED_KEY = "rh.hub.push.moved";

/** The device whose notifications were on before this browser's device moved here, until they are on again (`null`: none). */
export function pushMovedFrom(env: Pick<PushEnv, "storage"> = browserPushEnv()): MachineKey | null {
  try {
    return env.storage?.getItem(PUSH_MOVED_KEY) || null;
  } catch {
    return null;
  }
}

/** Stop offering to turn notifications back on after a move. */
export function forgetPushMoved(env: Pick<PushEnv, "storage"> = browserPushEnv()): void {
  try {
    env.storage?.removeItem(PUSH_MOVED_KEY);
  } catch {
    // memory only
  }
}

/** The device whose renewal left this browser without notifications until a tap turns them back on (`null`: none). */
export function pushRenewalNeeded(env: Pick<PushEnv, "storage"> = browserPushEnv()): MachineKey | null {
  try {
    return env.storage?.getItem(RENEW_KEY) ?? null;
  } catch {
    return null;
  }
}

export async function pushState(): Promise<PushState> {
  if (!supported()) return "unsupported";
  if (Notification.permission === "denied") return "denied";
  const reg = await readyOrNull();
  if (reg === null) return "unsupported";
  return (await reg.pushManager.getSubscription()) === null ? "off" : "on";
}

/** base64url text to the raw bytes `applicationServerKey` insists on. The
 *  buffer is allocated explicitly so the result is a plain-ArrayBuffer view,
 *  which is what BufferSource narrows to. */
function urlBase64ToUint8Array(value: string): Uint8Array<ArrayBuffer> {
  const padded = value + "=".repeat((4 - (value.length % 4)) % 4);
  const raw = atob(padded.replace(/-/g, "+").replace(/_/g, "/"));
  const bytes = new Uint8Array(new ArrayBuffer(raw.length));
  for (let i = 0; i < raw.length; i++) bytes[i] = raw.charCodeAt(i);
  return bytes;
}

/** Subscribe `reg` against `vapid` and hand the subscription to `machine`. */
async function subscribeAgainst(machine: MachineKey, reg: Pick<ServiceWorkerRegistration, "pushManager">, vapid: string, env: PushEnv): Promise<void> {
  const sub = await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: urlBase64ToUint8Array(vapid) });
  const json = sub.toJSON();
  if (!json.endpoint || !json.keys?.p256dh || !json.keys.auth) {
    throw new Error("browser returned a malformed subscription");
  }
  const res = await env.api(machine).POST("/api/push/subscriptions", {
    body: {
      endpoint: json.endpoint,
      keys: { p256dh: json.keys.p256dh, auth: json.keys.auth },
    },
  });
  if (res.error) throw new Error(res.error.error);
}

/** Subscribe this browser to `machine`'s notifications. */
export async function enablePush(machine: MachineKey, env: PushEnv = browserPushEnv()): Promise<void> {
  const reg = await env.ready();
  if (reg === null) throw new Error("push unsupported here");

  // Must be called from the user gesture that reached this function; browsers
  // reject a permission prompt that is not tied to one.
  const permission = typeof Notification === "undefined" ? env.permission() : await Notification.requestPermission();
  if (permission !== "granted") throw new Error("notification permission not granted");

  // Minted server-side on the first call, then stable — until the device renews its identity key.
  const key = await env.api(machine).GET("/api/push/vapid");
  if (!key.data) throw new Error("the server did not return a VAPID key");
  // One subscription per browser: one made against another key (a renewed device's old one) goes first.
  const old = await reg.pushManager.getSubscription();
  if (old && pushRenewalNeeded(env) !== null) await old.unsubscribe().catch(() => false);
  await subscribeAgainst(machine, reg, key.data.publicKey, env);
  // Current for no renewal in particular: the device may have renewed its identity key and not yet its push key (an agent
  // from before it refuses the key meanwhile), so its latest renewal is looked at (`followPushRenewals`).
  writePushRecord(env, { machine, vapid: key.data.publicKey, seq: null });
  setRenewal(env, null);
  forgetPushMoved(env);
}

/**
 * Stop `machine`'s notifications to this browser (`null`: the device is not
 * known — the browser's own subscription goes all the same). `rowWithinMs`:
 * how long the device's row may take to go before the browser's subscription
 * goes regardless (a sign-out does not wait on a device that is offline).
 */
export async function disablePush(machine: MachineKey | null, env: PushEnv = browserPushEnv(), opts: { rowWithinMs?: number } = {}): Promise<void> {
  const reg = await env.ready();
  writePushRecord(env, null);
  setRenewal(env, null);
  forgetPushMoved(env);
  if (reg === null) return;
  const sub = await reg.pushManager.getSubscription();
  if (sub === null) return;
  // Drop the server's row first, while the endpoint is still in hand, but do
  // not let that failure strand the browser subscription: the send path prunes
  // rows the push service rejects, so an orphaned row heals itself.
  if (machine !== null) {
    const row = Promise.resolve()
      .then(() => env.api(machine).DELETE("/api/push/subscriptions", { body: { endpoint: sub.endpoint } }))
      .catch(() => undefined);
    const within = opts.rowWithinMs;
    let timer: ReturnType<typeof setTimeout> | undefined;
    await (within === undefined ? row : Promise.race([row, new Promise((resolve) => (timer = setTimeout(resolve, within)))]));
    clearTimeout(timer);
  }
  // The push service forgets the endpoint: nothing sent to it reaches this browser again.
  await sub.unsubscribe();
}

/** How long a sign-out waits for the device to drop this browser's subscription. */
export const SIGN_OUT_PUSH_MS = 3_000;

/**
 * Signing out (either way): this browser's notifications stop — the device
 * this browser subscribed with drops its row, and the browser's subscription
 * goes, which makes the push service forget the endpoint. A browser left
 * subscribed would go on showing whoever uses it next what the account's
 * sessions are doing (their titles). Never throws.
 */
export async function disablePushOnSignOut(env: PushEnv = browserPushEnv(), rowWithinMs = SIGN_OUT_PUSH_MS): Promise<void> {
  try {
    await disablePush(readPushRecord(env)?.machine ?? null, env, { rowWithinMs });
  } catch (error) {
    console.warn("push: could not unsubscribe on sign-out:", error);
  }
}

/**
 * When to look again at a renewed device's push key that was not new yet (or
 * the device out of reach): its agent makes the new one in the background,
 * once it runs with its new identity key (and answers 503 `push_renewing`
 * meanwhile). A key that was the same at every look is taken as current at
 * the last; otherwise the next change of the account starts again.
 */
export const PUSH_RENEW_RETRY_MS: readonly number[] = [5_000, 15_000, 30_000, 60_000, 2 * 60_000, 5 * 60_000, 10 * 60_000];

/**
 * `machine` renewed its identity key (its chain entry `rotatedSeq`): when this
 * browser's subscription is for it and its VAPID key changed (the agent makes a
 * new one, and forgets every subscription, as it renews), drop the old
 * subscription — so the push service stops taking the old endpoint — and
 * subscribe again against the new key. Without a prompt where notifications
 * are allowed already; where the browser refuses to without a tap (or they are
 * not allowed), the old one is dropped all the same and a tap is asked for
 * ({@link pushRenewalNeeded}).
 *
 * The same key as before is the agent not done yet (it makes the new one in
 * the background, after it took its new identity key): `pending`, nothing
 * recorded — taken for current only when `acceptUnchanged` (the last look,
 * {@link followPushRenewals}). Recorded current, a key the agent then
 * replaced would never be followed: notifications would stop for good, and
 * the old endpoint stay subscribed.
 */
export async function followRenewedKey(
  machine: MachineKey,
  rotatedSeq: number | null,
  env: PushEnv = browserPushEnv(),
  opts: { acceptUnchanged?: boolean } = {},
): Promise<"none" | "pending" | "current" | "renewed" | "needsTap"> {
  const record = readPushRecord(env);
  if (!record || record.machine !== machine || rotatedSeq === null || (record.seq !== null && record.seq >= rotatedSeq)) return "none";
  const key = await env.api(machine).GET("/api/push/vapid");
  if (!key.data) throw new Error("the device did not return a VAPID key");
  if (key.data.publicKey === record.vapid) {
    if (!opts.acceptUnchanged) return "pending";
    writePushRecord(env, { ...record, seq: rotatedSeq });
    return "current";
  }
  const reg = await env.ready();
  const old = await reg?.pushManager.getSubscription();
  await old?.unsubscribe().catch(() => false);
  if (reg && env.permission() === "granted") {
    try {
      await subscribeAgainst(machine, reg, key.data.publicKey, env);
      writePushRecord(env, { machine, vapid: key.data.publicKey, seq: rotatedSeq });
      setRenewal(env, null);
      return "renewed";
    } catch (error) {
      console.warn("push: subscribing again after a renewal:", error);
    }
  }
  writePushRecord(env, null);
  setRenewal(env, machine);
  return "needsTap";
}

/**
 * Follow `account`'s chain for a renewal of the device this browser's
 * subscription is for ({@link followRenewedKey}): now, and on every change.
 * One at a time. A key not new yet, or a device out of reach, is looked at
 * again after each of `retryMs` (not on every change meanwhile). A key the
 * same at every one of those looks is taken as current at the last; a round
 * with a device out of reach in it ends without, and the next change of the
 * account starts another. A later renewal starts over.
 */
export function followPushRenewals(
  account: { chain: { machine(id: string): { rotatedSeq: number | null } | undefined } | null; onChange(fn: () => void): () => void },
  env: PushEnv = browserPushEnv(),
  retryMs: readonly number[] = PUSH_RENEW_RETRY_MS,
): () => void {
  const renewalOf = (machine: MachineKey) => account.chain?.machine(machine)?.rotatedSeq ?? null;
  let running = false;
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | null = null;
  /** The renewal being followed (`machine@seq`): how many looks again were scheduled for it, and how many found the same key. */
  let following: { key: string; tries: number; same: number } | null = null;
  const again = () => {
    const f = following;
    if (stopped || !f) return;
    // The round is over: the next change of the account looks again.
    if (f.tries >= retryMs.length) {
      following = null;
      return;
    }
    const delay = retryMs[f.tries++]!;
    timer = setTimeout(() => {
      timer = null;
      check(true);
    }, delay);
    (timer as { unref?: () => void }).unref?.();
  };
  const check = (scheduled = false) => {
    const record = readPushRecord(env);
    if (!record || running || stopped) return;
    const seq = renewalOf(record.machine);
    if (seq === null || (record.seq !== null && record.seq >= seq)) return;
    const key = `${record.machine}@${seq}`;
    let f = following;
    if (!f || f.key !== key) {
      following = f = { key, tries: 0, same: 0 };
      if (timer) clearTimeout(timer);
      timer = null;
    } else if (!scheduled) return;
    running = true;
    const round = f;
    // Only a key that was the same at every look of the round (none out of reach) is taken, at its last.
    void followRenewedKey(record.machine, seq, env, { acceptUnchanged: round.same >= retryMs.length })
      .then(
        (result) => {
          if (result !== "pending") return;
          round.same++;
          again();
        },
        (error: unknown) => {
          console.warn("push: following a renewal:", error);
          again();
        },
      )
      .finally(() => {
        running = false;
      });
  };
  check();
  const off = account.onChange(() => check());
  return () => {
    stopped = true;
    if (timer) clearTimeout(timer);
    off();
  };
}
