/*
 * Device registration, account opening and session lifecycle for `CloudAccount`:
 * loading/registering device keys, re-registering unjoined devices whose code
 * window lapsed, minting sessions by signing hub challenges, retrying once on
 * 401, scheduling periodic renewals while a page is open, and handling hub
 * refusals (`CloudAccount` documents the methods it hands to these).
 */
import { type ChainState, type DeviceKeys, DeviceKeysUnreadable, hex, type KeyStore, loadDeviceKeys, signHubLogin } from "@/lib/e2e";
import { CODE_PAIRING_WINDOW_MS, type HubApi, type HubSession } from "../api";
import {
  type AccountStorage,
  afreshKey,
  defaultStorage,
  deviceKey,
  deviceSignKey,
  forgetDeviceIdIn,
  forgetIn,
  lostDeviceKey,
  lostWhyKey,
  readFrom,
  REGISTER_AFRESH_MARGIN_MS,
  writeTo,
} from "../account-storage";
import type { Keyring } from "../keyring";
import type { AccountBasics, AccountNote } from "./basics";
import { type Refusal, refusalOf, type Standing } from "./devices";
import { AccountError, KeysUnreadableError } from "./errors";
import { RENEW_MARGIN_MS, RENEW_RETRY_MS, SECOND_REFUSAL_MS } from "./limits";

/** What device session management reads of the account beyond its basics. */
export interface AccountSessionDeps {
  api: HubApi;
  session: HubSession;
  keys: DeviceKeys;
  origin: string;
  deviceSignNote: AccountNote;
  onSession?: (session: HubSession) => void;
  getRefusal(): Refusal | null;
  setRefusalValue(refusal: Refusal): void;
  onKeepFresh(): void;
  onStop(): void;
}

export class AccountSession {
  private renewing: Promise<void> | null = null;
  private renewTimer: ReturnType<typeof setTimeout> | null = null;
  private keepFreshFlag = false;
  private lastRenewal: { token: string; atMs: number } | null = null;

  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: AccountSessionDeps,
  ) {}

  get keepFresh(): boolean {
    return this.keepFreshFlag;
  }

  sessionExpiring(now = Date.now()): boolean {
    return !this.deps.session.token || this.deps.session.expiresMs - now < RENEW_MARGIN_MS;
  }

  /** Whether this device registered so long ago that it may no longer open a numeric-code pairing. */
  async codeWindowLapsed(): Promise<boolean> {
    try {
      const view = await this.deps.api.me(this.deps.session.token);
      const created = view.devices.find((d) => d.id === this.basics.deviceId)?.createdMs ?? null;
      return created !== null && Date.now() >= created + CODE_PAIRING_WINDOW_MS - REGISTER_AFRESH_MARGIN_MS;
    } catch (error) {
      console.warn("cloud account: could not read when this device registered:", error);
      return false;
    }
  }

  renewSession(): Promise<void> {
    this.renewing ??= (async () => {
      const nonce = await this.deps.api.sessionChallenge(this.basics.account, this.basics.deviceId);
      const timeMs = Date.now();
      const signature = await signHubLogin(
        { role: "deviceSession", account: this.basics.account, id: this.basics.deviceId, hub: this.deps.origin, nonce, timeMs },
        this.deps.keys.sign.privateKey,
      );
      const fresh = await this.deps.api.deviceSession({ account: this.basics.account, device: this.basics.deviceId, timeMs, nonce, signature });
      Object.assign(this.deps.session, fresh);
      // The hub took these keys' signature for this id: they are the ones it was registered with.
      if (this.deps.deviceSignNote.read() === null) this.deps.deviceSignNote.write(hex(this.deps.keys.signPublic));
      this.deps.onSession?.({ ...this.deps.session });
      this.schedule();
    })().finally(() => {
      this.renewing = null;
    });
    return this.renewing;
  }

  async renewAfter401(stale: string): Promise<string | null> {
    if (this.deps.getRefusal()) return null;
    if (this.renewing) {
      await this.renewing.catch(() => undefined);
      return this.deps.getRefusal() ? null : this.deps.session.token || null;
    }
    if (stale !== this.deps.session.token && this.deps.session.token && !this.sessionExpiring()) return this.deps.session.token;
    if (this.lastRenewal && this.lastRenewal.token === stale && Date.now() - this.lastRenewal.atMs < SECOND_REFUSAL_MS) {
      this.setRefusal("denied");
      return null;
    }
    try {
      await this.renewSession();
    } catch (error) {
      const refused = refusalOf(error);
      if (refused) this.setRefusal(refused);
      return null;
    }
    this.lastRenewal = { token: this.deps.session.token, atMs: Date.now() };
    return this.deps.session.token;
  }

  setRefusal(refusal: Refusal): void {
    if (this.deps.getRefusal()) return;
    this.deps.setRefusalValue(refusal);
    this.keepFreshFlag = false;
    if (this.renewTimer) clearTimeout(this.renewTimer);
    this.renewTimer = null;
    this.basics.emit();
  }

  keepSessionFresh(): void {
    this.keepFreshFlag = true;
    this.schedule();
    this.deps.onKeepFresh();
  }

  stopSession(): void {
    this.keepFreshFlag = false;
    if (this.renewTimer) clearTimeout(this.renewTimer);
    this.renewTimer = null;
    this.deps.onStop();
  }

  private schedule(delayMs?: number): void {
    if (this.renewTimer) clearTimeout(this.renewTimer);
    this.renewTimer = null;
    if (!this.keepFreshFlag || this.deps.getRefusal()) return;
    const wait = delayMs ?? Math.max(1000, this.deps.session.expiresMs - Date.now() - RENEW_MARGIN_MS);
    // Long sessions are looked at again every six hours (timers cap at ~24.8 days).
    this.renewTimer = setTimeout(
      () => {
        this.renewTimer = null;
        if (!this.sessionExpiring()) return this.schedule();
        this.renewSession().catch((error: unknown) => {
          const refused = refusalOf(error);
          if (refused) this.setRefusal(refused);
          else this.schedule(RENEW_RETRY_MS);
        });
      },
      Math.min(wait, 6 * 60 * 60 * 1000),
    );
    (this.renewTimer as { unref?: () => void }).unref?.();
  }
}

/** The narrow surface of `CloudAccount` that {@link openAccount} drives while opening it. */
export interface OpenableAccount {
  readonly account: string;
  readonly deviceId: string;
  readonly session: HubSession;
  readonly keys: DeviceKeys;
  readonly keyring: Keyring;
  readonly renewer: (stale: string) => Promise<string | null>;
  readonly chain: ChainState | null;
  readonly standing: Standing;
  readonly pinned: boolean;
  refusal: Refusal | null;
  lostDevice: string | null;
  lostWhy: string | null;
  renewSession(): Promise<void>;
  sync(): Promise<number>;
  setRefusal(refusal: Refusal): void;
  confirmRemoval(loginToken: string): Promise<boolean>;
  stopSession(): void;
  settle(): Promise<void>;
}

/** Options read by {@link openAccount}. */
export interface OpenAccountOptions {
  storage?: AccountStorage;
  replaceUnreadableKeys?: boolean;
  keyReadRetryMs?: readonly number[];
}

/**
 * Open this browser's device for `session`'s account (`CloudAccount.open`),
 * re-registering an unjoined device afresh at most once per sign-in when the
 * hub dropped it, its code window lapsed, or the user asked to sign in again.
 */
export async function openAccount<T extends OpenableAccount>(
  api: HubApi,
  session: HubSession,
  store: KeyStore,
  opts: OpenAccountOptions,
  create: (own: HubSession, id: string, keys: DeviceKeys, storage: AccountStorage) => { account: T; session: AccountSession },
  mayRegisterAfresh = true,
): Promise<T> {
  const storage = opts.storage ?? defaultStorage();
  const loginToken = session.device ? null : session.token;
  let id = readFrom(storage, deviceKey(session.account));
  if (!id && session.device) {
    id = session.device;
    writeTo(storage, deviceKey(session.account), id);
  }
  // Keys that are stored but would not open are left as they are, and so is
  // the device id: never a new device on a read that may yet work.
  let loaded: Awaited<ReturnType<typeof loadDeviceKeys>>;
  try {
    loaded = await loadDeviceKeys(store, { expected: id !== null, replaceUnreadable: opts.replaceUnreadableKeys, retryMs: opts.keyReadRetryMs });
  } catch (error) {
    if (!(error instanceof DeviceKeysUnreadable)) throw error;
    console.warn(`cloud account: ${error.message}; ${id ?? "this browser"} keeps them as they are`);
    throw new KeysUnreadableError(error.reason, error.message);
  }
  const { keys, created } = loaded;
  const signKey = hex(keys.signPublic);
  // Keys made just now (the store held none), or not the ones this id was
  // registered with: the hub refuses every signature they make for it, for
  // good. Sign in as a new device instead, and keep the old id to say what
  // was lost.
  const registeredWith = readFrom(storage, deviceSignKey(session.account));
  if (id && (created || (registeredWith !== null && registeredWith !== signKey))) {
    const why = created ? (loaded.instead ?? "new keys were made") : "the keys this browser holds are not the ones it was registered with";
    console.warn(`cloud account: this browser no longer holds the keys of ${id} (${why}); it signs in as a new device`);
    writeTo(storage, lostDeviceKey(session.account), id);
    writeTo(storage, lostWhyKey(session.account), why);
    forgetDeviceIdIn(storage, session.account, id);
    id = null;
  }
  if (!id) {
    if (!loginToken) throw new AccountError("refused", "sign in again: this browser is not a device of the account yet");
    // The device whose keys this browser lost, if it did: the one that adds this one offers to remove it (`replaces`).
    id = await api.registerDevice(loginToken, keys.signPublic, keys.dhPublic, { replaces: readFrom(storage, lostDeviceKey(session.account)) });
    writeTo(storage, deviceKey(session.account), id);
    writeTo(storage, deviceSignKey(session.account), signKey);
  }
  const own: HubSession = session.device === id ? { ...session } : { account: session.account, token: "", expiresMs: 0, device: id };
  const { account, session: sessionFlow } = create(own, id, keys, storage);
  account.lostDevice = readFrom(storage, lostDeviceKey(session.account));
  account.lostWhy = account.lostDevice ? readFrom(storage, lostWhyKey(session.account)) : null;
  api.renew = account.renewer;
  const unjoined = () => account.chain !== null && account.standing === "absent";
  const registerAfresh = async (orStay = false): Promise<T | null> => {
    if (!loginToken) return null;
    let freshId: string;
    try {
      freshId = await api.registerDevice(loginToken, account.keys.signPublic, account.keys.dhPublic, { replaces: account.lostDevice });
    } catch (error) {
      if (!orStay) throw error;
      console.warn(`cloud account: could not register this browser afresh, it stays ${account.deviceId}:`, error);
      return null;
    }
    console.info(`cloud account: ${account.deviceId} never joined the chain and can no longer pair by code: this browser is ${freshId} now`);
    const stale = account.session.token;
    account.stopSession();
    if (stale && stale !== loginToken) await api.logout(stale).catch(() => undefined);
    forgetDeviceIdIn(storage, account.account, account.deviceId);
    writeTo(storage, deviceKey(account.account), freshId);
    writeTo(storage, deviceSignKey(account.account), hex(account.keys.signPublic));
    // Its keys open (it signed the registration): nothing is replaced on this second open.
    return openAccount(api, session, store, { ...opts, replaceUnreadableKeys: false }, create, false);
  };
  try {
    if (sessionFlow.sessionExpiring()) await account.renewSession();
    await account.sync();
  } catch (error) {
    const refused = account.refusal ?? refusalOf(error);
    if (!refused) throw error;
    account.setRefusal(refused);
    if (loginToken) {
      try {
        await account.confirmRemoval(loginToken);
      } catch (e) {
        console.warn("cloud account: could not read the chain to check the removal:", e);
      }
      // Not a device of the chain, and the hub no longer has it: what the sign-in is for is a new one.
      if (mayRegisterAfresh && refused === "denied" && unjoined()) return (await registerAfresh(false)) ?? account;
    }
    return account;
  }
  // A lost device the account no longer lists (removed from another device) is no longer worth saying.
  if (account.lostDevice && account.chain && !account.chain.device(account.lostDevice)) {
    forgetIn(storage, lostDeviceKey(account.account));
    forgetIn(storage, lostWhyKey(account.account));
    account.lostDevice = null;
    account.lostWhy = null;
  }
  // "Sign in again" asked for a device the chain names by now (added
  // meanwhile, from the QR code it showed), or for another id: not wanted.
  const asked = readFrom(storage, afreshKey(account.account));
  if (asked !== null && (asked !== account.deviceId || !unjoined())) forgetIn(storage, afreshKey(account.account));
  // Asked for, or past its code window: a new device, whose window is open.
  // If the hub will not register one now, this one stays: its QR code still works.
  if (mayRegisterAfresh && loginToken && unjoined() && (asked === account.deviceId || (await sessionFlow.codeWindowLapsed()))) {
    const fresh = await registerAfresh(true);
    if (fresh) return fresh;
  }
  await account.keyring.load();
  if (account.standing === "active" && account.pinned) await account.settle();
  return account;
}
