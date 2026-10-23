/*
 * The sending side, at the legacy origin (the push's tab, the pull's popup).
 *
 * Holding the move's lock and the app's (`locks.ts`), it reads the two stores
 * and the moved `localStorage` entries as they are, sends them to the one
 * window it expects, and waits. Only an answer that the app host signed in
 * with them, naming the digest it sent, lets it forget its copy — and only if
 * its stores still read to that digest (no other tab wrote meanwhile).
 * Anything else — a refusal, a closed window, a timeout, a change — leaves
 * everything here as it was.
 */
import { DEVICE_KEYS_PREFIX, KEY_STORE_DB } from "@/lib/e2e/keystore";
import { clearSession, loadSession } from "@/lib/hub/api";
import { LOCAL_CACHE_DB, LOCAL_CACHE_STORE } from "@/lib/local-cache";
import { seenKey } from "@/lib/hub/seen-store";
import { PUSH_RECORD_KEY, PUSH_RENEW_KEY, readPushRecord } from "@/lib/push";
import { MOVED_DATABASES, movesLocal } from "./allowlist";
import { type DigestEntry, digestOf, Unmovable } from "./digest";
import { localEntries, type MoveEnv, type MoveStorage } from "./env";
import { readLocalDevice } from "./local";
import { BUSY, DEVICE_KEYS_LOCK, holdLocks, LOCAL_WRAP_LOCK, MOVE_LOCK, noRenewal } from "./locks";
import { type BundleStore, LinkEnded, type Message, MSG, type MoveBundle, type PeerLink } from "./protocol";
import type { MoveOrigins, MoveOutcome, MoveStep, MoveUnfinished } from "./types";

/** How long the locks are tried for, and a renewal in another tab waited out. */
export const LOCK_WAIT_MS = 5_000;
/** How long the app host may take to write and sign in. */
export const VERIFY_MS = 120_000;
/** How long the logout of the old session, and dropping the push subscription, may take after a move. */
const AFTER_MS = 5_000;
/** How long deleting a database waits on another page that keeps it open. */
const DESTROY_MS = 2_000;

/** What the digest covers: every store record and every moved entry. */
export function bundleEntries(stores: readonly BundleStore[], local: readonly { key: string; value: string }[]): DigestEntry[] {
  return [...stores.flatMap((s) => s.records.map((r) => ({ where: `idb:${s.db}/${s.store}`, key: r.key, value: r.value }))), ...local.map((e) => ({ where: "ls", key: e.key, value: e.value }))];
}

/** This origin's stores and moved entries as they are now, ready to send (`null`: no device keys here). Throws {@link Unmovable}. */
export async function takeSnapshot(env: MoveEnv, primary: { account: string; device: string }): Promise<MoveBundle | null> {
  const stores: BundleStore[] = [];
  for (const d of MOVED_DATABASES) {
    const records = await env.records(d.db, d.store).readAll();
    if (records) stores.push({ db: d.db, store: d.store, version: d.version, records });
  }
  if (!stores.find((s) => s.db === KEY_STORE_DB)?.records.some((r) => r.key.startsWith(DEVICE_KEYS_PREFIX))) return null;
  const local = localEntries(env.storage, movesLocal);
  const pushWasOn = readPushRecord({ storage: env.storage })?.machine ?? null;
  const digest = await digestOf(bundleEntries(stores, local));
  return { v: 1, from: env.origin, account: primary.account, device: primary.device, stores, local, pushWasOn, digest };
}

/** Where the legacy origin notes a move whose keys went but which did not finish ({@link MoveUnfinished}). */
export const MOVE_OUT_KEY = "rh.hub.move.out";
/** The tokens kept under the e-mail itself before they were hashed (`api.ts`): never moved, gone with the rest. */
const LEGACY_KNOWN_CLIENT_PREFIX = "rh.hub.knownClient:";

/** What this origin noted of a move that did not finish (`null`: none). */
export function readMoveOut(storage: Pick<MoveStorage, "getItem">): MoveUnfinished | null {
  try {
    const m = JSON.parse(storage.getItem(MOVE_OUT_KEY) ?? "null") as Partial<MoveUnfinished> | null;
    if (!m || (m.state !== "sent" && m.state !== "verified") || typeof m.account !== "string" || typeof m.device !== "string" || typeof m.app !== "string") return null;
    return { state: m.state, account: m.account, device: m.device, app: m.app, atMs: typeof m.atMs === "number" ? m.atMs : 0 };
  } catch {
    return null;
  }
}

function noteMoveOut(storage: MoveStorage, note: MoveUnfinished | null): void {
  try {
    if (note) storage.setItem(MOVE_OUT_KEY, JSON.stringify(note));
    else storage.removeItem(MOVE_OUT_KEY);
  } catch {
    // memory only: the next move finds it out again
  }
}

function capped<T>(work: Promise<T>, ms: number): Promise<T | undefined> {
  return Promise.race([work.catch(() => undefined), new Promise<undefined>((resolve) => setTimeout(resolve, ms))]);
}

/**
 * Forget everything that moved, here: both stores emptied (then their
 * databases deleted), the moved entries, the session (ended at the hub too),
 * the push subscription, and what nothing here can read any more — the
 * offline copies (sealed under a key that went), the known-client tokens
 * kept under e-mails, the note of an unfinished move. Returns what could not
 * be forgotten.
 */
export async function wipeHere(env: MoveEnv): Promise<string[]> {
  const failures: string[] = [];
  for (const d of [...MOVED_DATABASES, { db: LOCAL_CACHE_DB, store: LOCAL_CACHE_STORE }]) {
    const store = env.records(d.db, d.store);
    try {
      await store.clear();
    } catch (error) {
      failures.push(`${d.db}: ${error instanceof Error ? error.message : String(error)}`);
      continue;
    }
    await store.destroy(DESTROY_MS);
  }
  for (const { key } of localEntries(env.storage, (name) => movesLocal(name) || name.startsWith(LEGACY_KNOWN_CLIENT_PREFIX) || name === MOVE_OUT_KEY)) {
    try {
      env.storage.removeItem(key);
    } catch (error) {
      failures.push(`${key}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  const session = loadSession(env.storage as Storage);
  clearSession(env.storage as Storage);
  for (const key of [PUSH_RECORD_KEY, PUSH_RENEW_KEY]) {
    try {
      env.storage.removeItem(key);
    } catch {
      // gone with the worker
    }
  }
  await Promise.all([
    session?.token ? capped(env.fetch("/v1/logout", { method: "POST", headers: { authorization: `Bearer ${session.token}` } }), AFTER_MS) : undefined,
    capped(env.dropPushSubscription(), AFTER_MS),
  ]);
  return failures;
}

/** How the sending side reaches the window that receives. */
export interface SendIo {
  link: PeerLink;
  /** The receiving window's id, once it said it is ready. */
  peerReady(): Promise<string>;
}

function refusedOutcome(reply: Message): MoveOutcome {
  const detail = typeof reply.detail === "string" ? reply.detail.slice(0, 300) : "";
  switch (reply.reason) {
    case "targetHasDevice":
      return { kind: "targetHasDevice" };
    case "verifyFailed":
      return { kind: "verifyFailed", reason: detail || "the app host could not sign in with the moved keys" };
    case "busy":
      return { kind: "busy" };
    default:
      return { kind: "failed", reason: detail || "the app host refused the keys" };
  }
}

/** An ended link, a store that would not read, anything thrown: as the move's outcome. */
export function failureOutcome(error: unknown): MoveOutcome {
  if (error instanceof LinkEnded) return error.why === "timeout" ? { kind: "timeout" } : { kind: "closed" };
  return { kind: "failed", reason: error instanceof Error ? `${error instanceof Unmovable ? "" : `${error.name}: `}${error.message}`.slice(0, 300) : String(error) };
}

/**
 * Send this origin's device through `io`, and forget it here once the app
 * host signed in with it (see the file's comment). Before the keys go, the
 * move is noted here ({@link MOVE_OUT_KEY}): a move cut off after that is
 * `unfinished`, never `closed` — the app host may hold the device.
 */
export async function sendMove(env: MoveEnv, origins: MoveOrigins, io: SendIo, step: (s: MoveStep) => void): Promise<MoveOutcome> {
  let here;
  try {
    here = await readLocalDevice(env);
  } catch (error) {
    return failureOutcome(error);
  }
  const primary = here?.primary;
  if (!here || !primary || !here.keys) return { kind: "nothingToMove" };
  let moved = false;
  let sent = false;
  env.tabs?.post({ t: "moving" });
  const note = (state: MoveUnfinished["state"]): MoveUnfinished => ({ state, account: primary.account, device: primary.device, app: origins.app, atMs: Date.now() });
  try {
    if (!(await noRenewal(env.locks, LOCK_WAIT_MS))) return { kind: "busy" };
    const locks = [MOVE_LOCK, DEVICE_KEYS_LOCK, LOCAL_WRAP_LOCK, ...here.accounts.map((a) => seenKey(a.account))];
    const out = await holdLocks(
      env.locks,
      locks,
      async (): Promise<MoveOutcome> => {
        const bundle = await takeSnapshot(env, primary);
        if (!bundle) return { kind: "nothingToMove" };
        const id = await io.peerReady();
        step("sending");
        // Noted before the keys go: from here on the app host may hold the device, whatever happens to this page.
        const before = readMoveOut(env.storage);
        if (before?.state !== "verified" || before.account !== primary.account || before.device !== primary.device) noteMoveOut(env.storage, note("sent"));
        sent = true;
        io.link.send({ t: MSG.bundle, id, bundle });
        step("verifying");
        const reply = await io.link.next([MSG.verified, MSG.refused], VERIFY_MS, id);
        if (reply.t === MSG.refused) {
          // Whether the app host holds this device after its refusal: no (it took nothing in, or put back what
          // it wrote), yes (it had it from a move before), or it does not say (as noted before this attempt).
          noteMoveOut(env.storage, reply.holds === false ? null : reply.holds === true ? note("verified") : before);
          return refusedOutcome(reply);
        }
        if (reply.digest !== bundle.digest || reply.account !== bundle.account || reply.device !== bundle.device) {
          io.link.send({ t: MSG.kept, id, reason: "mismatch" });
          return { kind: "unfinished", why: "mismatch" };
        }
        noteMoveOut(env.storage, note("verified"));
        // Still what was sent? Another tab (one from before this version) may have written meanwhile.
        const now = await takeSnapshot(env, primary);
        if (!now || now.digest !== bundle.digest) {
          io.link.send({ t: MSG.kept, id, reason: "changed" });
          return { kind: "unfinished", why: "changed" };
        }
        step("wiping");
        const failures = await wipeHere(env);
        if (failures.length) console.warn("move: the old copy could not all be removed:", failures);
        io.link.send({ t: MSG.wiped, id });
        moved = true;
        return { kind: "moved", account: bundle.account, device: bundle.device, pushWasOn: bundle.pushWasOn, resumed: reply.resumed === true, senderWiped: failures.length === 0 };
      },
      LOCK_WAIT_MS,
    );
    return out === BUSY ? { kind: "busy" } : out;
  } catch (error) {
    const outcome = failureOutcome(error);
    // Cut off after the keys went: the app host may hold the device, and this origin still does.
    if (sent && (outcome.kind === "closed" || outcome.kind === "timeout")) return { kind: "unfinished", why: outcome.kind };
    return outcome;
  } finally {
    env.tabs?.post(moved ? { t: "moved", to: origins.app } : { t: "stopped" });
  }
}
