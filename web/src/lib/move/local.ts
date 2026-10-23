/*
 * What this origin holds of a device, read without creating anything: the
 * device ids it noted (`rh.hub.device.<account>`), whether device keys are
 * stored, whether a session is kept. The legacy origin asks before it offers
 * to move; the app host asks before it takes a device in.
 */
import { DEVICE_KEYS_PREFIX, KEY_STORE_DB, KEY_STORE_STORE } from "@/lib/e2e/keystore";
import { loadSession } from "@/lib/hub/api";
import { DEVICE_NOTE_PREFIX } from "./allowlist";
import { localEntries, type MoveEnv } from "./env";
import type { LocalDevice } from "./types";

/** The device ids this browser noted, per account. */
export function notedDevices(env: Pick<MoveEnv, "storage">): { account: string; device: string }[] {
  return localEntries(env.storage, (name) => name.startsWith(DEVICE_NOTE_PREFIX) && name.length > DEVICE_NOTE_PREFIX.length)
    .filter((e) => e.value.length > 0)
    .map((e) => ({ account: e.key.slice(DEVICE_NOTE_PREFIX.length), device: e.value }));
}

/** Whether device keys are stored here (the key store is read as it is; never created). */
export async function deviceKeysStored(env: Pick<MoveEnv, "records">): Promise<boolean> {
  const records = await env.records(KEY_STORE_DB, KEY_STORE_STORE).readAll();
  return !!records?.some((r) => r.key.startsWith(DEVICE_KEYS_PREFIX));
}

/** What this origin holds of a device (`null`: nothing — no device id noted and no device keys). */
export async function readLocalDevice(env: Pick<MoveEnv, "storage" | "records">): Promise<LocalDevice | null> {
  const accounts = notedDevices(env);
  const keys = await deviceKeysStored(env);
  if (!accounts.length && !keys) return null;
  let session = null;
  try {
    session = loadSession(env.storage as Storage);
  } catch {
    // none kept
  }
  const bySession = session ? accounts.find((a) => a.account === session.account && a.device === session.device) : undefined;
  const primary = bySession ?? (accounts.length === 1 ? accounts[0]! : null);
  return { primary, accounts, keys, signedIn: session !== null };
}
