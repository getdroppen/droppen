/*
 * What a move copies, and so what the legacy origin forgets once the app
 * host signed in with it: every record of the two IndexedDB stores, as they
 * are, and the `localStorage` entries below. Nothing else crosses — the
 * receiving side refuses a bundle naming anything outside this list.
 *
 * The `localStorage` list is "sign out and remove this browser"'s (droppen#73,
 * `forgetBrowser` in `lib/hub/account.ts`): every note it forgets is moved
 * (`test/move-allowlist.test.ts` holds the two together), plus the account's
 * other notes, the seen record's fallback and a few display settings. Not
 * moved: the hub session (`rh.hub.session`: the app host signs in afresh, and
 * that sign-in is the proof the move worked), the push subscription record
 * (`rh.hub.push.*`: a subscription belongs to an origin's worker; only "they
 * were on" crosses, `MoveBundle.pushWasOn`), the offline copies
 * (`rh-local-cache`: they rebuild), Cache Storage and `sessionStorage`.
 */
import { KEY_STORE_DB, KEY_STORE_STORE } from "@/lib/e2e/keystore";
import { SEEN_DB, SEEN_IN_IDB, SEEN_STORE } from "@/lib/hub/seen-store";
import { HUB_ONLY_KEY } from "@/lib/transport/p2p";

/** An IndexedDB object store a move copies whole: out-of-line string keys, no key path, no indexes. */
export interface MovedDatabase {
  db: string;
  store: string;
  /** The version the store is created at where it is missing (the version its own code opens it at). */
  version: number;
}

/** The key store (device keys, the local wrap key and everything sealed under it) and the seen record (the pinned genesis). */
export const MOVED_DATABASES: readonly MovedDatabase[] = [
  { db: KEY_STORE_DB, store: KEY_STORE_STORE, version: 1 },
  { db: SEEN_DB, store: SEEN_STORE, version: 1 },
];

/** The moved database named so, if it is one. */
export function movedDatabase(db: string, store: string): MovedDatabase | undefined {
  return MOVED_DATABASES.find((d) => d.db === db && d.store === store);
}

/** `localStorage` entries moved by their name's start (each ends in the account, the device, or both). */
const MOVED_PREFIXES = [
  // The device id and the key it was registered with, and what the browser noted about its device (forgetBrowser's notes).
  "rh.hub.device.",
  "rh.hub.devicekey.",
  "rh.hub.lost.",
  "rh.hub.lost-why.",
  "rh.hub.afresh.",
  "rh.hub.recovered.",
  "rh.hub.keyreq.",
  "rh.hub.hostkeyreq.",
  "rh.hub.hostkeysent.",
  "rh.hub.ack.",
  "rh.hub.reenroll.",
  // Renewals this browser verified a dispute of, and its own "confirm on the device" setting.
  "rh.hub.renewal.",
  // Machine keys still to be re-encrypted.
  "rh.hub.reencrypt.v2.",
  // The seen record and the pinned genesis where IndexedDB was not to be had.
  "rh.hub.seen.",
  "rh.hub.genesis.",
  // The device picked, the account's main device, sign-in changes already said, registrations dismissed.
  "rh.hub.machine.",
  "rh.hub.home.",
  "rh.hub.sign-in-changes.",
  "rh.hub.registered.dismissed.",
  // Known-client tokens (by the e-mail's hash).
  "rh.hub.knownClientHash:",
];

/**
 * `localStorage` entries moved by their whole name: the relay picked by hand,
 * "no direct connections" (`lib/transport/p2p.ts`: a privacy setting a move
 * must not turn off), the soft-crypto consent, display settings.
 */
const MOVED_NAMES = [SEEN_IN_IDB, "rh.relays.manual", HUB_ONLY_KEY, "droppen.e2e-compat", "rh.theme", "rh.fontSize", "rh.lang"];

/** Never moved, whatever the lists above say: the session, the push subscription, the move's own notes. */
const NEVER_MOVED = ["rh.hub.session", "rh.hub.push.", "rh.hub.move."];

/** Whether the `localStorage` entry `name` is moved. */
export function movesLocal(name: string): boolean {
  if (NEVER_MOVED.some((p) => name === p || (p.endsWith(".") && name.startsWith(p)))) return false;
  return MOVED_NAMES.includes(name) || MOVED_PREFIXES.some((p) => name.startsWith(p) && name.length > p.length);
}

/** Where this browser notes its device id for `account` (`rh.hub.device.<account>`, `lib/hub/account.ts`). */
export const DEVICE_NOTE_PREFIX = "rh.hub.device.";
/** Where it notes the signing key that device id was registered with (hex). */
export const DEVICE_KEY_NOTE_PREFIX = "rh.hub.devicekey.";
