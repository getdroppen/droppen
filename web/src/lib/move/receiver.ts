/*
 * The receiving side, at the app host (the push's popup, the pull's tab).
 *
 * A bundle is taken only into an origin that holds no device — or that holds
 * this very device from a move that did not finish:
 * - what an earlier move of the same account and device wrote and never
 *   signed in with (`rh.hub.move.in` says `writing`, and nothing else is
 *   here) is written over;
 * - the same device, signed in with before (`verified`), only gains what it
 *   lacks — nothing it holds is replaced — and is signed in with again (the
 *   sender then forgets its copy: the move that was cut off finishes).
 * Any other device here: refused (`targetHasDevice`).
 *
 * It writes each store in one transaction, then proves the move: the device
 * keys read back and open, the hub gives a fresh device session for the
 * moved key (`POST /v1/session`, naming this page's hub), the account lists
 * the device with that key (`GET /v1/me`), and everything written reads back
 * to the digest the sender sent. Only then is the session kept and the
 * sender told; on any failure what was written is taken out again, what it
 * replaced put back.
 */
import { hex } from "@/lib/e2e/bytes";
import { DEVICE_KEYS_PREFIX, KEY_STORE_DB, KEY_STORE_STORE, readDeviceKeys } from "@/lib/e2e/keystore";
import { signHubLogin } from "@/lib/e2e/sig";
import { HubApi, type HubSession, saveSession } from "@/lib/hub/api";
import { PUSH_MOVED_KEY } from "@/lib/push";
import { DEVICE_KEY_NOTE_PREFIX, DEVICE_NOTE_PREFIX, movedDatabase, movesLocal } from "./allowlist";
import { digestOf, Unmovable } from "./digest";
import type { MoveEnv, MoveStorage } from "./env";
import { readLocalDevice } from "./local";
import { BUSY, DEVICE_KEYS_LOCK, holdLocks, MOVE_LOCK } from "./locks";
import type { MoveBundle, Refusal } from "./protocol";
import { bundleEntries, LOCK_WAIT_MS } from "./sender";
import type { PriorRecord } from "./stores";
import type { LocalDevice, MoveOrigins } from "./types";

/** What this origin noted of a move into it: `writing` until it signed in with what it got, then `verified`. */
export const MOVE_MARKER = "rh.hub.move.in";

export interface MoveMarker {
  v: 1;
  state: "writing" | "verified";
  account: string;
  device: string;
  from: string;
  atMs: number;
}

export function readMarker(storage: Pick<MoveStorage, "getItem">): MoveMarker | null {
  try {
    const m = JSON.parse(storage.getItem(MOVE_MARKER) ?? "null") as Partial<MoveMarker> | null;
    if (!m || m.v !== 1 || (m.state !== "writing" && m.state !== "verified") || typeof m.account !== "string" || typeof m.device !== "string") return null;
    return m as MoveMarker;
  } catch {
    return null;
  }
}

function writeMarker(storage: MoveStorage, marker: MoveMarker | null): void {
  if (marker) storage.setItem(MOVE_MARKER, JSON.stringify(marker));
  else storage.removeItem(MOVE_MARKER);
}

export type Received =
  | { ok: true; account: string; device: string; digest: string; pushWasOn: string | null; resumed: boolean }
  /** `holds`: whether this origin holds the bundle's device after the refusal (`null`: it did not look). */
  | { ok: false; reason: Refusal; detail: string; holds: boolean | null };

/**
 * Forget a note that no longer says anything: nothing of a device here at
 * all, or another device's id noted here and not the one it names (removed
 * and replaced since). Keys with no device id noted yet are what a move cut
 * off between its two writes leaves: the note stands. Returns the note that
 * stands.
 */
export function settleMarker(storage: MoveStorage, here: LocalDevice | null): MoveMarker | null {
  const marker = readMarker(storage);
  if (!marker) return null;
  const stands = !!here && (here.accounts.length === 0 || here.accounts.some((a) => a.account === marker.account && a.device === marker.device));
  if (stands) return marker;
  try {
    storage.removeItem(MOVE_MARKER);
  } catch {
    // read again next time
  }
  return null;
}

/** Whether the device here is the one an unfinished move into this origin left (and so may be written over by its next bundle). */
export function onlyUnfinishedHere(storage: MoveStorage, here: LocalDevice | null): boolean {
  const marker = settleMarker(storage, here);
  return !!here && marker?.state === "writing" && here.accounts.every((a) => a.account === marker.account && a.device === marker.device);
}

const ID_TEXT = /^[\x21-\x7e]{1,64}$/;

/** `raw` as a bundle from `origins.legacy` this side takes, or why not. */
export async function checkBundle(raw: unknown, origins: MoveOrigins): Promise<MoveBundle | string> {
  if (!raw || typeof raw !== "object") return "not a bundle";
  const b = raw as Partial<MoveBundle>;
  if (b.v !== 1) return "a bundle of another version";
  if (b.from !== origins.legacy) return "a bundle from another origin";
  if (typeof b.account !== "string" || !ID_TEXT.test(b.account) || typeof b.device !== "string" || !ID_TEXT.test(b.device)) return "a bundle naming no device";
  if (b.pushWasOn !== null && (typeof b.pushWasOn !== "string" || !ID_TEXT.test(b.pushWasOn))) return "a bundle with a malformed push note";
  if (!Array.isArray(b.stores) || !Array.isArray(b.local) || typeof b.digest !== "string") return "a malformed bundle";
  const seen = new Set<string>();
  for (const s of b.stores) {
    const known = s && typeof s === "object" ? movedDatabase(s.db, s.store) : undefined;
    if (!known || s.version !== known.version || seen.has(s.db) || !Array.isArray(s.records)) return "a bundle with a store the move does not copy";
    seen.add(s.db);
    const names = new Set<string>();
    for (const r of s.records) {
      if (!r || typeof r !== "object" || typeof r.key !== "string" || names.has(r.key)) return "a bundle with a malformed record";
      names.add(r.key);
    }
  }
  const names = new Set<string>();
  for (const e of b.local) {
    if (!e || typeof e !== "object" || typeof e.key !== "string" || typeof e.value !== "string" || names.has(e.key)) return "a bundle with a malformed entry";
    if (!movesLocal(e.key)) return `a bundle with an entry the move does not copy (${e.key.slice(0, 40)})`;
    names.add(e.key);
  }
  if (b.local.find((e) => e.key === `${DEVICE_NOTE_PREFIX}${b.account}`)?.value !== b.device) return "a bundle whose device is not the one it notes";
  if (!b.stores.find((s) => s.db === KEY_STORE_DB)?.records.some((r) => r.key.startsWith(DEVICE_KEYS_PREFIX))) return "a bundle without device keys";
  try {
    if ((await digestOf(bundleEntries(b.stores, b.local))) !== b.digest) return "a bundle that does not match its digest";
  } catch (error) {
    return error instanceof Unmovable ? `a bundle with ${error.message}` : "a bundle that could not be read";
  }
  return b as MoveBundle;
}

/** Whether the device keys stored here are, record for record, the bundle's. */
async function sameDeviceKeys(env: MoveEnv, bundle: MoveBundle): Promise<boolean> {
  const pick = (records: readonly { key: string; value: unknown }[]) => records.filter((r) => r.key.startsWith(DEVICE_KEYS_PREFIX)).map((r) => ({ where: "keys", key: r.key, value: r.value }));
  const here = (await env.records(KEY_STORE_DB, KEY_STORE_STORE).readAll()) ?? [];
  const theirs = bundle.stores.find((s) => s.db === KEY_STORE_DB)?.records ?? [];
  try {
    return (await digestOf(pick(here))) === (await digestOf(pick(theirs)));
  } catch {
    return false;
  }
}

class VerifyError extends Error {}

/** How the hub lists `device` in the account (`GET /v1/me`): its key (hex), `true` (listed, no key said), `false` (not listed), `null` (no list). */
async function listedKey(env: MoveEnv, token: string, device: string): Promise<string | boolean | null> {
  const res = await env.fetch("/v1/me", { headers: { authorization: `Bearer ${token}` } });
  if (!res.ok) throw new VerifyError(`the hub would not list the account's devices (${res.status})`);
  const me = (await res.json()) as { devices?: unknown };
  if (!Array.isArray(me.devices)) return null;
  const row = me.devices.find((d): d is { id: string; sign_key?: unknown } => !!d && typeof d === "object" && (d as { id?: unknown }).id === device);
  if (!row) return false;
  return typeof row.sign_key === "string" ? row.sign_key.toLowerCase() : true;
}

/** Prove the move (see the file's comment): the fresh session, or a {@link VerifyError}. */
async function verify(env: MoveEnv, bundle: MoveBundle, written: { db: string; store: string; keys: string[] }[], full: boolean): Promise<HubSession> {
  const read = await readDeviceKeys(env.keyStore());
  if (read.kind !== "found") throw new VerifyError(read.kind === "unreadable" ? `the moved device keys do not open here: ${read.why}` : "the moved device keys are not where they were written");
  const signKey = hex(read.keys.signPublic);
  const noted = env.storage.getItem(`${DEVICE_KEY_NOTE_PREFIX}${bundle.account}`);
  if (noted !== null && noted.toLowerCase() !== signKey) throw new VerifyError("the moved device keys are not the ones the device was registered with");
  if (!env.hub) throw new VerifyError("this page names no hub to sign in at");
  const api = new HubApi("", env.fetch);
  let session: HubSession;
  try {
    const nonce = await api.sessionChallenge(bundle.account, bundle.device);
    const timeMs = Date.now();
    const signature = await signHubLogin({ role: "deviceSession", account: bundle.account, id: bundle.device, hub: env.hub, nonce, timeMs }, read.keys.sign.privateKey);
    session = await api.deviceSession({ account: bundle.account, device: bundle.device, timeMs, nonce, signature });
  } catch (error) {
    throw new VerifyError(`the hub did not sign this device in with the moved key: ${error instanceof Error ? error.message : String(error)}`);
  }
  try {
    const listed = await listedKey(env, session.token, bundle.device);
    if (listed === false) throw new VerifyError("the account does not list this device");
    if (typeof listed === "string" && listed !== signKey) throw new VerifyError("the account lists this device with another key");
    if (full) {
      // Everything written reads back as it was sent.
      const entries = [];
      for (const w of written) {
        for (const r of await env.records(w.db, w.store).read(w.keys)) {
          if (!r.existed) throw new VerifyError(`${w.db} lost a record as it was written`);
          entries.push({ where: `idb:${w.db}/${w.store}`, key: r.key, value: r.value });
        }
      }
      for (const e of bundle.local) entries.push({ where: "ls", key: e.key, value: env.storage.getItem(e.key) });
      if ((await digestOf(entries)) !== bundle.digest) throw new VerifyError("what was written here does not read back as it was sent");
    }
    return session;
  } catch (error) {
    // The session goes with the keys it was for.
    await env.fetch("/v1/logout", { method: "POST", headers: { authorization: `Bearer ${session.token}` } }).catch(() => undefined);
    throw error instanceof VerifyError ? error : new VerifyError(error instanceof Error ? error.message : String(error));
  }
}

/** Take `raw` (see the file's comment): what was taken in, or why not. Nothing it wrote stays unless it says `ok`. */
export async function receiveBundle(env: MoveEnv, origins: MoveOrigins, raw: unknown): Promise<Received> {
  const checked = await checkBundle(raw, origins);
  if (typeof checked === "string") return { ok: false, reason: "badBundle", detail: checked, holds: null };
  const bundle = checked;
  try {
    const out = await holdLocks(env.locks, [MOVE_LOCK, DEVICE_KEYS_LOCK], () => receiveLocked(env, bundle), LOCK_WAIT_MS);
    return out === BUSY ? { ok: false, reason: "busy", detail: "another tab of this browser is moving or opening its device keys", holds: null } : out;
  } catch (error) {
    return { ok: false, reason: "verifyFailed", detail: error instanceof Error ? error.message : String(error), holds: null };
  }
}

async function receiveLocked(env: MoveEnv, bundle: MoveBundle): Promise<Received> {
  const here = await readLocalDevice(env);
  const marker = settleMarker(env.storage, here);
  const ofBundle = (m: MoveMarker | null) => !!m && m.account === bundle.account && m.device === bundle.device;
  let mode: "fresh" | "resume";
  if (!here) mode = "fresh";
  // What an earlier move of this very device wrote and never signed in with, and nothing else here.
  else if (marker?.state === "writing" && ofBundle(marker) && onlyUnfinishedHere(env.storage, here)) mode = "fresh";
  else if (marker?.state === "verified" && ofBundle(marker) && (await sameDeviceKeys(env, bundle))) mode = "resume";
  else return { ok: false, reason: "targetHasDevice", detail: "this browser already holds another device here", holds: null };
  const note = (state: MoveMarker["state"]): MoveMarker => ({ v: 1, state, account: bundle.account, device: bundle.device, from: bundle.from, atMs: Date.now() });
  if (mode === "fresh") writeMarker(env.storage, note("writing"));
  const written: { db: string; store: string; keys: string[]; prior: PriorRecord[] }[] = [];
  const localPrior: { key: string; value: string | null }[] = [];
  try {
    for (const s of bundle.stores) {
      const prior = await env.records(s.db, s.store).write(s.records, mode === "fresh" ? "overwrite" : "ifAbsent", s.version);
      written.push({ db: s.db, store: s.store, keys: s.records.map((r) => r.key), prior });
    }
    for (const e of bundle.local) {
      const was = env.storage.getItem(e.key);
      if (mode === "resume" && was !== null) continue;
      localPrior.push({ key: e.key, value: was });
      env.storage.setItem(e.key, e.value);
    }
    const session = await verify(env, bundle, written, mode === "fresh");
    saveSession(session, env.storage as Storage);
    writeMarker(env.storage, note("verified"));
    if (bundle.pushWasOn) env.storage.setItem(PUSH_MOVED_KEY, bundle.pushWasOn);
    env.tabs?.post({ t: "arrived", account: bundle.account });
    return { ok: true, account: bundle.account, device: bundle.device, digest: bundle.digest, pushWasOn: bundle.pushWasOn, resumed: mode === "resume" };
  } catch (error) {
    // Back as it was: the stores, the entries, and (all of it put back) the note.
    let restored = true;
    for (const w of written.reverse()) {
      await env.records(w.db, w.store).restore(w.prior).catch(() => (restored = false));
    }
    for (const p of localPrior.reverse()) {
      try {
        if (p.value === null) env.storage.removeItem(p.key);
        else env.storage.setItem(p.key, p.value);
      } catch {
        restored = false;
      }
    }
    if (mode === "fresh" && restored) writeMarker(env.storage, marker);
    const detail = error instanceof Error ? error.message : String(error);
    // Resumed: the device it had before stays (signed in with already). Fresh: nothing of it is live here.
    return { ok: false, reason: "verifyFailed", detail: detail.slice(0, 300), holds: mode === "resume" };
  }
}
