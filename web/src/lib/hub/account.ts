/*
 * This browser as a device of a hub account: its keys, its id and session at
 * the hub, its verified view of the account's device chain, and what a device
 * does with them — create the account's genesis, enroll machines (checking
 * each agent's possession proof before signing "add machine"), hand machines
 * their keys, add and remove devices (rotating the keys when one goes),
 * recover with the recovery words, and vouch for the chain head it sees.
 *
 * - Device keys are WebCrypto keys whose private halves are non-extractable
 *   `CryptoKey`s while in use, kept in IndexedDB sealed under a device-local
 *   key (`lib/e2e/keystore.ts`) so the page can use them after a reload. That
 *   is no wall against code in this origin: it can open them as the loader
 *   does. What keeps them is a trusted client (docs/web-integrity.md).
 * - The hub session is the DEVICE's (`HubApi`): minted only by the device key
 *   signing a hub challenge (`renewSession`), renewed before it expires and
 *   once on a 401. A hub that refuses it twice is treated like a removal —
 *   but keys are wiped only when the chain itself shows the removal.
 * - `AK` exists only while genesis is signed and is then dropped.
 * - The recovery key `RK` is generated here and shown once, as twelve words;
 *   only `RSK`'s public half (derived from it), with its signature over the
 *   account's root key, goes into the chain, and each `UK` epoch is sealed to
 *   it into the keyring (`rk/<epoch>/<device>/<rsk tag>`). Nothing stores `RK`.
 * - `UK` epochs and machine keys: `keyring.ts` (never at rest as bytes).
 * - **Commit before use.** Every new `UK` and `MK` epoch is committed in the
 *   chain (a `keyEpochs` entry, appended in the same call as the entry that
 *   needs it) before it is stored, wrapped, mailed or handed to an agent, and
 *   every key this device takes — from a pairing, the mailbox, the recovery
 *   packages or the keyring — must be the one the chain committed. Epoch
 *   numbers come only from the chain. So a removed device, the hub, or both
 *   can neither plant a key nor push an epoch out of reach.
 * - **Pinning.** The chain is fetched and verified on every load. A browser
 *   pins the account's genesis only once it has reason to trust it: it made
 *   it, a pairing delivery MAC'd with this screen's secret vouched for it, or
 *   the recovery words' `RSK` is the chain's current recovery key (whose
 *   proof covers the account's root). Until then it takes no keys at all.
 * - **First sightings.** What this device has verified (`seenThrough`) and
 *   when it first saw each entry are kept per account, so a reload resumes the
 *   live clock rather than taking a join and its activation delivered
 *   together as history. Every tab shares the record, and it only moves
 *   forward: a tab left behind never lowers it, and two tabs whose chains
 *   differ where they overlap are a fork. It is kept in IndexedDB with the
 *   pinned genesis, and every write of either is a read-compare-write in one
 *   transaction, so no tab's stale view can lower it or pin over it
 *   (`seen-store.ts`).
 * - **Alerts.** Every device-changing entry this device did not sign itself
 *   (another device added or removed, a recovery join, new recovery words) is
 *   an alert until it is acknowledged here.
 * - **Chain v3** (docs/local-aggregator.md §3): every endpoint is a device;
 *   「可运行 AI」(host) and 「可管理账号」(admin) are its two flags. A new
 *   account is v3 from its genesis (`createGenesis`, only while the hub
 *   takes v3 entries: `/v1/me`'s `chain_v3.writable`). What is written
 *   follows the chain itself (review D4): on an account made before chain
 *   v3, v2 entries until it holds a v3 entry, v3 entries after. Its first
 *   v3 entry is a step of its own that the person confirms (`upgradeToV3`,
 *   only while the hub takes v3 entries, passed in as `{writable}`); until
 *   then exactly what v2 can say is written, and never a v2 device that is
 *   not full (an old agent would let it reach admin routes). On a v3 chain
 *   the hub gates only what adds a host or makes one host again. A device that
 *   runs AI never manages the account (review D1). Only admins hold `UK`; a
 *   device that is not an admin is handed the committed `MK` epochs of the
 *   hosts that accept its control, and only those, one by one (mailbox
 *   purpose 5, `HostKeyVault`; review D2), and asks an admin (purpose 4) when
 *   it lacks one — never for a user key. A host that is not an admin is such
 *   a device too (S1-4: the local install controls with its one identity): it
 *   is sealed the other hosts' keys once its device role signed in at the
 *   hub, and its requests are answered like any other's.
 * - **Every batch is verified here first** (`appendBatch`): applied to a copy
 *   of this view with the chain's own verifier before it is signed away, so
 *   a batch the chain would refuse is refused here, by its rule.
 */
import {
  type Accept,
  type Action,
  ASSURANCE_ON_DEVICE,
  type OpenPause,
  PAUSE_CANCELLED,
  PAUSE_COMPLETED,
  PAUSE_START,
  type PauseRemoval,
  type Assurance,
  type Attestation,
  type Bytes,
  CHAIN_VERSION,
  CHAIN_VERSION_3,
  ChainClock,
  ChainError,
  ChainState,
  copy,
  createAttestation,
  DecodeError,
  decodeEntry,
  decodeHostKeyMail,
  encodeDraft,
  type DeviceInfo,
  type DeviceKeys,
  DeviceKeysUnreadable,
  type DeviceRecord,
  type Draft,
  encodeEntry,
  encodeHostKeyMail,
  type EnrollCode,
  enrollToken,
  type Entry,
  entryHash,
  equal,
  exportPublic,
  fingerprint,
  fixedVersion,
  fromHex,
  generateEnrollCode,
  genesis3Draft,
  HandshakeError,
  hex,
  type HostRotation,
  hostRotationBody,
  isValidEd25519Key,
  isValidX25519,
  type KeyStore,
  loadDeviceKeys,
  credentialHash,
  MAX_KEY_COMMITS,
  MAX_PASSKEY_SLOTS,
  newSlotId,
  type MachineInfo,
  type MachineKey,
  type MachineKeyCommit,
  machineKeyCommit,
  mnemonicFromEntropy,
  type NoiseKeyPair,
  openHostKey,
  openUserKey,
  passkeyAuthority,
  passkeySlotProof,
  prfInput,
  RECOVERY_DELAY_MS,
  RECOVERY_RECIPIENT,
  recoveryKeyFor,
  recoveryKeys,
  rotateSas,
  RULES,
  sealForRecovery,
  sealHostKey,
  sealTo,
  sha256,
  signDraft,
  signHubLogin,
  type SlotRecord,
  userKeyCommit,
  utf8,
  verifyEnrollProof,
  verifyPasskeySlot,
  verifyHostRotation,
  verifyRotateConfirm,
  WrapError,
} from "@/lib/e2e";
import { chainV3Writable, CODE_PAIRING_WINDOW_MS, type HostRotationView, type HubApi, HubApiError, type HubSession, MAIL_HOST_KEY, MAIL_KEY_REQUEST, MAIL_PAIRING, MAIL_USER_KEY } from "./api";
import { hubOrigin } from "./config";
import {
  ChannelUnavailable,
  deliverMachineKey,
  type DeliveryResult,
  type MachineKeyCourier,
  notePendingReencrypt,
  type PendingReencrypt,
  type RenewalCourier,
  type RotateRequest,
  STALE_GRACE_MS,
} from "./deliver";
import {
  HostKeyVault,
  type KeptRenewal,
  keyName,
  Keyring,
  machineKeyBytes,
  migrateLegacyMachineKeys,
  moveLegacyMachineKeysToVault,
  openMachineKeys,
  openRecoveryPackage,
  rewrapMachineKeys,
  recoveryTag,
  RenewalStash,
  UserKeyVault,
  wrapMachineKey,
} from "./keyring";
import { type Candidate, completePairing } from "./pairing";
import {
  type AcceptChange,
  type ChainReplay,
  type ChainReplayHooks,
  type Exposure,
  type KeptDispute,
  type PauseEnding,
  type ReEnrollEvidence,
  type ReEnrollLine,
  type RenewalNote,
  type ReplayDevice,
  type RotateAnswer,
  MAX_KEPT_DISPUTES,
  NO_COURIER,
  NO_DEVICE_ROLE_HINT,
  NothingToWrite,
  acceptChange,
  acceptNames,
  admits,
  assuredRenewalAfter,
  checkRotateAnswer,
  compareBytes,
  compareUtf8Ids,
  disputeIsolates,
  edgeHeld,
  exposedControllers,
  exposedCycles,
  groupNarrowing,
  heldUserKey,
  hostsExposedBy,
  hostsToRotate,
  lastUserKeyLoss,
  noDeviceRoleYet,
  parseRotateAnswer,
  pauseReturns,
  reEnrollCulprits,
  reEnrollEvidence,
  renewalHelps,
  renewalRefusal,
  replayChain,
  rotateAnswerJson,
  rotationDueIn,
  splitPauseRemovals,
  userKeyStale,
  verifyBatch,
} from "./chain-policy";
export {
  NO_DEVICE_ROLE_HINT,
  acceptChange,
  admits,
  assuredRenewalAfter,
  capDisputes,
  checkRotateAnswer,
  compareUtf8Ids,
  disputeIsolates,
  edgeHeld,
  exposedControllers,
  exposedCycles,
  groupNarrowing,
  heldUserKey,
  hostsExposedBy,
  hostsToRotate,
  noDeviceRoleYet,
  parseRotateAnswer,
  pauseReturns,
  reEnrollCulprits,
  reEnrollEvidence,
  renewalHelps,
  renewalRefusal,
  rotationDueIn,
  splitPauseRemovals,
  userKeyStale,
  verifyBatch,
} from "./chain-policy";
export type {
  AcceptChange,
  Exposure,
  KeptDispute,
  PauseEnding,
  ReEnrollEvidence,
  ReEnrollLine,
  RenewalNote,
  RotateAnswer,
} from "./chain-policy";

import { recoverySecret } from "./recovery-words";
import {
  type AckRecord,
  checkpoints,
  compareSeen,
  describeError,
  genesisKey,
  type Held,
  higherSeen,
  pageSeenStore,
  parseSeenJson,
  parseStored,
  sameStored,
  seenKey,
  type SeenRecord,
  type SeenStore,
  SeenStoreError,
  seenNext,
  type StoredTrust,
} from "./seen-store";
import {
  type AccountLocks,
  type AccountStorage,
  ackKey,
  afreshKey,
  type CrossTab,
  defaultCrossTab,
  defaultLocks,
  defaultSeenWatch,
  defaultStorage,
  deviceKey,
  deviceSignKey,
  forgetDeviceIdIn,
  forgetIn,
  type HostKeysSent,
  hostKeyRequestKey,
  hostKeysSentKey,
  joinedKey,
  keptDisputesKey,
  keyRequestKey,
  LEGACY_MIGRATION_LOCK,
  localAssuredKey,
  lostDeviceKey,
  lostWhyKey,
  readFrom,
  REGISTER_AFRESH_MARGIN_MS,
  renewalLockName,
  type SeenWatch,
  writeTo,
} from "./account-storage";
export type { AccountLocks, AccountStorage, CrossTab, SeenWatch } from "./account-storage";
import type { ControllersRecord } from "./streams";
import { deviceFingerprint, hasRunAi, joinAction, type Refusal, refusalOf, type SelfRemoval, type Standing } from "./account/devices";
import { AccountError, heldWho, hostAdminRefused, KeysUnreadableError, MovingCheckpoint, noNewAccounts, notUpgraded, ranAiAdminRefused } from "./account/errors";
import { CHAIN_V3_WAIT, FIRST_LOAD_TRIES, HOST_KEYS_WAIT, KEY_REQUEST_EVERY_MS, MAX_APPEND_ENTRIES, MAX_HOST_KEY_MAILS, RENEW_MARGIN_MS, RENEW_RETRY_MS, SECOND_REFUSAL_MS } from "./account/limits";
import { answeredWithin, BUSY, CODE_WAIT_MS, RENEW_RESTARTS, RenewalError, type RenewalReport, type RenewOptions } from "./account/renewal";
import type { ChainAlert, FreshUserKey, HeldBack, HeldChange, RotationReport, VetoPlan, VetoTarget } from "./account/views";
import { forgetBrowser } from "./account/wipe";
import { ReEnrollWarnings } from "./account/re-enroll";
import { ChainAlerts } from "./account/alerts";
import { type AccountNote, accountBasics } from "./account/basics";
import { RenewalNotes } from "./account/renewal-notes";
export { deviceFingerprint, hasRunAi, joinAction, limitedDevice } from "./account/devices";
export type { JoinKeys, PromoteBlock, Refusal, SelfRemoval, Standing } from "./account/devices";
export { AccountError, hostAdminRefused, KeysUnreadableError, v3NotWritableError } from "./account/errors";
export type { AccountErrorKind } from "./account/errors";
export { CHAIN_V3_WAIT, HOST_KEYS_WAIT, KEY_REQUEST_EVERY_MS, MAX_APPEND_ENTRIES, MAX_HOST_KEY_MAILS, RENEW_MARGIN_MS } from "./account/limits";
export { CODE_WAIT_MS, RenewalError } from "./account/renewal";
export type { RenewalFailure, RenewalReport, RenewOptions } from "./account/renewal";
export type { ChainAlert, FreshUserKey, HeldBack, HeldChange, RotationReport, VetoPlan, VetoTarget } from "./account/views";
export { WipeError, wipeFailures } from "./account/wipe";

export interface AccountOptions {
  /** The hub origin sign-ins name (`wss://hub`, the page's meta tag); defaults to `hubOrigin()`. */
  origin?: string;
  /** Told whenever this device's session changes (to keep it). */
  onSession?: (session: HubSession) => void;
  /** Where the device id and acknowledgements are kept (`localStorage`), and older pages' pin and seen record (read, never deleted). */
  storage?: AccountStorage;
  /** Where the pinned genesis and the seen record are kept: what this device trusts and verified of the chain, and when it first saw each entry (`pageSeenStore`: IndexedDB). */
  seen?: SeenStore;
  /** What serialises a pin and the acknowledgements it starts across pages, and everything where the store is `localStorage` (`navigator.locks`; `null`: none). */
  locks?: AccountLocks | null;
  /**
   * Device keys that are stored but will not open are replaced, and this
   * browser signs in as a new device: what a person chooses on the "could not
   * open its keys" screen. Never on its own.
   */
  replaceUnreadableKeys?: boolean;
  /** The waits between reads of a key store that did not give the device keys (`DEVICE_KEYS_RETRY_MS`; tests). */
  keyReadRetryMs?: readonly number[];
  /** How this browser's other tabs hear of disputes this one verified ({@link CrossTab}; `null`: none). Defaults to a `BroadcastChannel` with `localStorage`. */
  crossTab?: CrossTab | null;
}

type Listener = () => void;

const random32 = () => crypto.getRandomValues(new Uint8Array(32));

/** `keys` with the `wanted` epochs first, then the rest newest first. */
export function requestedFirst<T extends { epoch: number }>(keys: T[], wanted: ReadonlySet<number>): T[] {
  return [...keys].sort((x, y) => Number(wanted.has(y.epoch)) - Number(wanted.has(x.epoch)) || y.epoch - x.epoch);
}

export class CloudAccount {
  private clock = ChainClock.live(Date.now(), 0);
  readonly vault: UserKeyVault;
  readonly keyring: Keyring;
  /** The hosts' machine keys handed to this device one by one (it is not an admin: chain v3). */
  readonly hostKeys: HostKeyVault;
  /** The renewals this browser asked for and has not finished (`renewIdentity`, `resumeRenewal`). */
  private readonly renewalStash: RenewalStash;
  private readonly listeners = new Set<Listener>();
  /** Set once the hub refused this device a session (see {@link Refusal}). */
  refusal: Refusal | null = null;
  /**
   * A device id this browser held for the account but lost the keys of (the
   * browser dropped them, or another account's reset took them): it signed in
   * as a new device instead, and the old one is still listed until a device of
   * the account removes it. Cleared once this device is in the account.
   */
  lostDevice: string | null = null;
  /** With {@link lostDevice}: what this browser's key store held instead of its keys, in a few words. */
  lostWhy: string | null = null;
  /** While a pairing screen waits for its delivery: pairing mail is left for it. */
  pairing = false;
  private renewing: Promise<void> | null = null;
  private renewTimer: ReturnType<typeof setTimeout> | null = null;
  private keepFresh = false;
  /** The chain's newest user key epoch when this device does not hold it ("missing key"); kept by `checkUserKey`. */
  missingUserKey: number | null = null;
  /** (Not an admin.) The current hosts whose newest committed key epoch this device cannot open; kept by `checkHostKeys`. */
  missingHostKeys: string[] = [];
  /** Stop hearing the other tabs ({@link stopSession}). */
  private unhearCrossTab: (() => void) | null = null;
  private readonly crossTab: CrossTab | null;
  private held: HeldBack | null = null;
  /**
   * The last hold a sync took in on this device's own clock (its day passed
   * here): its activation, and how far this view went with it. A veto the
   * person confirmed on that hold's plan is still a veto, but only while
   * nothing newer has been taken in since (`removeDevice`'s `veto`).
   */
  private taken: { seq: number; device: string; takenThroughSeq: number } | null = null;
  /** Set (for good, in this page) once this view conflicted with the chain this browser verified: why. */
  private forkedWhy: string | null = null;
  private unwatchSeen: (() => void) | null = null;
  /** This page's turn at the seen record: one read-compare-write of it at a time here, whether or not there are Web Locks. */
  private seenTurn: Promise<unknown> = Promise.resolve();
  /** The genesis this browser pinned (hex), as the seen store said it when this page last read or wrote it (`pinned`). */
  private pinnedAt: string | null = null;
  /**
   * The newest entry of a view this page verified and could not take, as its
   * record could not be written (`install`: the store failed): until this
   * view reaches it, the page vouches for nothing (`attestation`).
   */
  private unrecordedThrough = -1;
  private heldTimer: ReturnType<typeof setTimeout> | null = null;
  private lastRenewal: { token: string; atMs: number } | null = null;
  /** What `HubApi.renew` is set to for this account (`open` sets it). */
  readonly renewer = (stale: string): Promise<string | null> => this.renewAfter401(stale);
  private readonly origin: string;
  private readonly storage: AccountStorage;
  private readonly seen: SeenStore;
  private readonly locks: AccountLocks | null;
  /** The re-enroll warnings (review D3), kept for the account in this browser. */
  private readonly reEnroll = new ReEnrollWarnings({
    account: () => this.account,
    state: () => this.state,
    renews: (host) => this.renews(host),
    read: (key) => this.read(key),
    write: (key, value) => this.write(key, value),
    emit: () => this.emit(),
  });
  /** What the account's collaborators read of it (`AccountBasics`). */
  private readonly basics = accountBasics({
    account: () => this.account,
    deviceId: () => this.deviceId,
    state: () => this.state,
    clock: () => this.clock,
    emit: () => this.emit(),
  });
  /** The device-change alerts and their acknowledgement. */
  private readonly chainAlerts = new ChainAlerts(this.basics, {
    pinned: () => this.pinned,
    standing: () => this.standing,
    readAck: () => this.readAck(),
    writeAck: (ack) => this.writeAck(ack),
  });
  /** What the renewal runner noted, the disputes kept here, and what an automatic renewal may not go past (`RenewalNotes`). */
  private readonly renewalNotes: RenewalNotes;

  private constructor(
    readonly api: HubApi,
    readonly session: HubSession,
    readonly deviceId: string,
    readonly keys: DeviceKeys,
    private state: ChainState | null,
    readonly store: KeyStore,
    private readonly opts: AccountOptions,
  ) {
    this.origin = opts.origin ?? hubOrigin() ?? "";
    this.storage = opts.storage ?? defaultStorage();
    this.seen = opts.seen ?? pageSeenStore(this.storage);
    this.locks = opts.locks === undefined ? defaultLocks() : opts.locks;
    this.crossTab = opts.crossTab === undefined ? defaultCrossTab(this.storage) : opts.crossTab;
    this.renewalNotes = new RenewalNotes(
      this.basics,
      {
        readHeld: () => this.readHeld(),
        sync: () => this.sync(),
        keptDisputes: this.note(keptDisputesKey(this.account)),
        localAssured: this.note(localAssuredKey(this.account)),
      },
      this.crossTab,
    );
    // Another tab kept disputes: merged here, and every listener (each channel among them) looks again once they are checked.
    this.unhearCrossTab = this.crossTab?.subscribe((message) => this.renewalNotes.heardDisputes(message)) ?? null;
    this.vault = new UserKeyVault(store, session.account);
    this.hostKeys = new HostKeyVault(store, session.account);
    this.renewalStash = new RenewalStash(store, session.account);
    this.keyring = new Keyring(
      {
        list: () => api.keys(this.session.token),
        put: (name, blob) => {
          // Nothing sealed or wrapped leaves a page whose view forked (`forked`), however long ago its work began.
          this.assertNotForked();
          return api.putKey(this.session.token, name, blob);
        },
      },
      store,
      session.account,
      deviceId,
    );
  }

  get account(): string {
    return this.session.account;
  }

  get chain(): ChainState | null {
    return this.state;
  }

  /** Whether this device manages the account (chain v3's admin; v2's full device): it holds `UK`, signs entries. */
  get isAdmin(): boolean {
    return this.state?.admin(this.deviceId) !== undefined;
  }

  /** This device's own clock for the chain (first sightings: recovery delays). */
  get chainClock(): ChainClock {
    return this.clock;
  }

  /** The device's X25519 pair, as Noise and key sealing take it. */
  get noiseKeys(): NoiseKeyPair {
    return { privateKey: this.keys.dh.privateKey, publicKey: this.keys.dhPublic };
  }

  /**
   * Open this browser's device for `session`'s account — a device session
   * kept from before (maybe expired: it is renewed with a new signature), or a
   * password sign-in's login token (which registers this browser as a device
   * if it is not one yet): keys, id, a device session, the chain — and, as an
   * active device that pinned the account, its keys settled. If the hub
   * refuses this device a session, the account comes back with `refusal`
   * set (and, with a login token, the chain read to see whether it really
   * removed this device).
   *
   * A sign-in (a login token) on a browser whose device never joined the
   * chain registers it afresh when the hub has forgotten it (a device that
   * never joined is deleted a day after it registered) or when it registered
   * too long ago to open a numeric-code pairing any more
   * ({@link CODE_PAIRING_WINDOW_MS}), or when its pairing screen asked to
   * sign in again ({@link registerAfreshOnSignIn}): that opens the window
   * again. Never a device the chain names, in any state — by the chain read
   * now. A new device the hub will not register now leaves the old one as it
   * was (its QR code still works).
   */
  static async open(api: HubApi, session: HubSession, store: KeyStore, opts: AccountOptions = {}): Promise<CloudAccount> {
    return CloudAccount.openAs(api, session, store, opts, true);
  }

  private static async openAs(api: HubApi, session: HubSession, store: KeyStore, opts: AccountOptions, mayRegisterAfresh: boolean): Promise<CloudAccount> {
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
    const account = new CloudAccount(api, own, id, keys, null, store, { ...opts, storage });
    account.lostDevice = readFrom(storage, lostDeviceKey(session.account));
    account.lostWhy = account.lostDevice ? readFrom(storage, lostWhyKey(session.account)) : null;
    api.renew = account.renewer;
    try {
      if (account.sessionExpiring()) await account.renewSession();
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
        if (mayRegisterAfresh && refused === "denied" && account.unjoined) return (await account.registerAfresh(api, loginToken, session, store, opts)) ?? account;
      }
      return account;
    }
    // A lost device the account no longer lists (removed from another device) is no longer worth saying.
    if (account.lostDevice && account.state && !account.state.device(account.lostDevice)) {
      account.forget(lostDeviceKey(account.account));
      account.forget(lostWhyKey(account.account));
      account.lostDevice = null;
      account.lostWhy = null;
    }
    // "Sign in again" asked for a device the chain names by now (added
    // meanwhile, from the QR code it showed), or for another id: not wanted.
    const asked = account.read(afreshKey(account.account));
    if (asked !== null && (asked !== account.deviceId || !account.unjoined)) account.forget(afreshKey(account.account));
    // Asked for, or past its code window: a new device, whose window is open.
    // If the hub will not register one now, this one stays: its QR code still works.
    if (mayRegisterAfresh && loginToken && account.unjoined && (asked === account.deviceId || (await account.codeWindowLapsed()))) {
      const fresh = await account.registerAfresh(api, loginToken, session, store, opts, { orStay: true });
      if (fresh) return fresh;
    }
    await account.keyring.load();
    if (account.standing === "active" && account.pinned) await account.settle();
    return account;
  }

  /** A chain exists, and names this device in no state at all: it never joined (the pairing screen's device). */
  private get unjoined(): boolean {
    return this.state !== null && this.standing === "absent";
  }

  /**
   * Whether this device registered so long ago that it may no longer open a
   * numeric-code pairing (or will not within a minute): its registration as
   * `/v1/me` lists it. `false` when that cannot be told.
   */
  private async codeWindowLapsed(): Promise<boolean> {
    try {
      const view = await this.api.me(this.session.token);
      const created = view.devices.find((d) => d.id === this.deviceId)?.createdMs ?? null;
      return created !== null && Date.now() >= created + CODE_PAIRING_WINDOW_MS - REGISTER_AFRESH_MARGIN_MS;
    } catch (error) {
      console.warn("cloud account: could not read when this device registered:", error);
      return false;
    }
  }

  /**
   * Leave this (unjoined) device behind and open the account as a newly
   * registered one, once per sign-in. The new device is registered first:
   * if the hub will not have one now (its limits on new devices: a few a
   * day, a few waiting at once; or no answer) the refusal is thrown — or,
   * with `orStay`, nothing has changed and the answer is `null`: this
   * device goes on as it was (its QR code works for as long as the hub
   * keeps it).
   */
  private async registerAfresh(
    api: HubApi,
    loginToken: string,
    session: HubSession,
    store: KeyStore,
    opts: AccountOptions,
    { orStay = false }: { orStay?: boolean } = {},
  ): Promise<CloudAccount | null> {
    let id: string;
    try {
      id = await api.registerDevice(loginToken, this.keys.signPublic, this.keys.dhPublic, { replaces: this.lostDevice });
    } catch (error) {
      if (!orStay) throw error;
      console.warn(`cloud account: could not register this browser afresh, it stays ${this.deviceId}:`, error);
      return null;
    }
    console.info(`cloud account: ${this.deviceId} never joined the chain and can no longer pair by code: this browser is ${id} now`);
    const stale = this.session.token;
    this.stopSession();
    if (stale && stale !== loginToken) await api.logout(stale).catch(() => undefined);
    forgetDeviceIdIn(this.storage, this.account, this.deviceId);
    this.write(deviceKey(this.account), id);
    this.write(deviceSignKey(this.account), hex(this.keys.signPublic));
    // Its keys open (it signed the registration): nothing is replaced on this second open.
    return CloudAccount.openAs(api, session, store, { ...opts, replaceUnreadableKeys: false }, false);
  }

  /**
   * The pairing screen's "sign in again": the next sign-in registers this
   * browser afresh — a new device with the same keys, a new window to pair
   * by code — if the chain does not name this device then. It may by then:
   * the QR code it showed can still be read meanwhile, and that device
   * keeps its id. Nothing is forgotten until the new device is registered,
   * so a sign-in the hub will not register one for goes on as this one.
   * Whether it was asked: never for a device the chain names now.
   */
  registerAfreshOnSignIn(): boolean {
    if (this.standing !== "absent") return false;
    this.write(afreshKey(this.account), this.deviceId);
    return true;
  }

  /** Told after the chain, this device's keys or its session change. */
  onChange(fn: Listener): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit(): void {
    for (const fn of [...this.listeners]) {
      try {
        fn();
      } catch {
        // a listener's problem
      }
    }
  }

  private read(key: string): string | null {
    return readFrom(this.storage, key);
  }

  private write(key: string, value: string): void {
    writeTo(this.storage, key, value);
  }

  private forget(key: string): void {
    forgetIn(this.storage, key);
  }

  /** This browser's note `key` for the account, bound to that key: what a collaborator that keeps a note is handed. */
  private note(key: string): AccountNote {
    return { read: () => this.read(key), write: (value) => this.write(key, value), forget: () => this.forget(key) };
  }

  private info(): DeviceInfo {
    return { id: this.deviceId, signKey: this.keys.signPublic, dhKey: this.keys.dhPublic, full: true };
  }

  // ------------------------------------------------------------ sessions

  private sessionExpiring(now = Date.now()): boolean {
    return !this.session.token || this.session.expiresMs - now < RENEW_MARGIN_MS;
  }

  /**
   * A new device session: the hub's challenge signed with this device's key
   * (`HubLogin`, role `deviceSession`, naming the hub this page dialled).
   * Rejects with the hub's refusal (403 `device_removed`, 401 `denied`).
   */
  renewSession(): Promise<void> {
    this.renewing ??= (async () => {
      const nonce = await this.api.sessionChallenge(this.account, this.deviceId);
      const timeMs = Date.now();
      const signature = await signHubLogin({ role: "deviceSession", account: this.account, id: this.deviceId, hub: this.origin, nonce, timeMs }, this.keys.sign.privateKey);
      const fresh = await this.api.deviceSession({ account: this.account, device: this.deviceId, timeMs, nonce, signature });
      Object.assign(this.session, fresh);
      // The hub took these keys' signature for this id: they are the ones it was registered with.
      if (this.read(deviceSignKey(this.account)) === null) this.write(deviceSignKey(this.account), hex(this.keys.signPublic));
      this.opts.onSession?.({ ...this.session });
      this.schedule();
    })().finally(() => {
      this.renewing = null;
    });
    return this.renewing;
  }

  /** `HubApi.renew`: once per refusal — a session minted for a 401 and refused at once is a second refusal. */
  async renewAfter401(stale: string): Promise<string | null> {
    if (this.refusal) return null;
    if (this.renewing) {
      await this.renewing.catch(() => undefined);
      return this.refusal ? null : this.session.token || null;
    }
    if (stale !== this.session.token && this.session.token && !this.sessionExpiring()) return this.session.token;
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
    this.lastRenewal = { token: this.session.token, atMs: Date.now() };
    return this.session.token;
  }

  /** The hub will not have this device: stop renewing, tell the page. */
  setRefusal(refusal: Refusal): void {
    if (this.refusal) return;
    this.refusal = refusal;
    this.keepFresh = false;
    if (this.renewTimer) clearTimeout(this.renewTimer);
    this.renewTimer = null;
    this.emit();
  }

  /** Keep the device session fresh while the page is open (renewed shortly before it expires). */
  keepSessionFresh(): void {
    this.keepFresh = true;
    this.schedule();
    this.scheduleHeld();
    if (!this.unwatchSeen) this.watchSeenRecord();
  }

  /** Stop renewing (a sign-out, a wipe). */
  stopSession(): void {
    this.keepFresh = false;
    if (this.renewTimer) clearTimeout(this.renewTimer);
    this.renewTimer = null;
    if (this.heldTimer) clearTimeout(this.heldTimer);
    this.heldTimer = null;
    this.unwatchSeen?.();
    // Nor does it hear the other tabs any more (what they keep would have it catch up with the hub).
    this.unhearCrossTab?.();
    this.unhearCrossTab = null;
    if (this.api.renew === this.renewer) this.api.renew = null;
  }

  private schedule(delayMs?: number): void {
    if (this.renewTimer) clearTimeout(this.renewTimer);
    this.renewTimer = null;
    if (!this.keepFresh || this.refusal) return;
    const wait = delayMs ?? Math.max(1000, this.session.expiresMs - Date.now() - RENEW_MARGIN_MS);
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

  /**
   * The hub said this device was removed (or refused it): read the chain
   * with a password sign-in's login token (read-only) and see. `true` — and
   * this view shows the removal — only if the chain, verified exactly as
   * `sync` verifies it (the genesis this device pinned, the head it saw last,
   * its own live clock and first sightings), removes this device. M2
   * re-review: this used to verify with every entry taken as history, so a
   * hub holding the recovery words could join, self-activate at once and
   * remove the owner — and the owner's page wiped its keys. Now a removal
   * behind a self-activation that is too early here is held back with it
   * (`heldBack`): not confirmed, nothing wiped, and the join is vetoable.
   */
  async confirmRemoval(loginToken: string): Promise<boolean> {
    await this.catchUp(loginToken, { noRenew: true });
    return this.standing === "removed";
  }

  // ------------------------------------------------------------ the chain

  /**
   * Whether this browser pinned this account's genesis (and so trusts the
   * chain enough to take keys) — as the seen store said it last time this
   * page read or wrote it (a pin, once set, never changes). So it can lag a
   * pin another page made: what must not lag reads the store itself
   * (`attestation`, `lacksKeys`), and the watch takes such a pin in
   * ({@link recheckSeen}).
   */
  get pinned(): boolean {
    return this.pinnedAt !== null && !!this.state && this.pinnedAt === hex(this.state.hashAt(0)!);
  }

  /** The genesis this browser pinned for the account (hex), as the seen store holds it now. */
  private async pinnedGenesis(): Promise<string | null> {
    return (await this.readHeld()).genesis;
  }

  /** Trust this chain's genesis from now on (see the header: only for a reason). */
  private async pin(): Promise<void> {
    if (!this.state) return;
    const state = this.state;
    const genesis = hex(state.hashAt(0)!);
    // The pin and the record in one step: another page may have pinned
    // another genesis since this one last looked (two first set-ups at once,
    // a hub taking both). Then that pin stands, with its record.
    await this.withSeenLock(async () => {
      await this.updateHeld((held) => {
        if (held.genesis !== null && held.genesis !== genesis) throw this.latchFork("another page of this browser pinned a different genesis for this account");
        return { genesis, record: this.nextRecord(state, held, this.clock) };
      });
      // Alerts start from here: what happened before this device trusted the account is history.
      if (!this.readAck()) this.writeAck({ through: state.seq, extra: [] });
    });
  }

  /**
   * Pin the genesis this device just made, which the hub holds by now: the
   * account's first user key is kept only once it is pinned, and there is no
   * making it again. A store that fails twice leaves the pin where older
   * pages keep it (`localStorage`), which every read takes where the store
   * has none — and a pin another page made meanwhile is a fork there — so the
   * key is not lost; the record follows with the next view.
   */
  private async pinMade(): Promise<void> {
    for (let tries = 1; ; tries++) {
      try {
        return await this.pin();
      } catch (error) {
        if (!(error instanceof SeenStoreError)) throw error;
        if (tries < 2) continue;
        console.warn("cloud account: the seen store would not take the pin of the genesis this device made; it is kept in localStorage:", error);
        const genesis = hex(this.state!.hashAt(0)!);
        this.write(genesisKey(this.account), genesis);
        this.pinnedAt = genesis;
        if (!this.readAck()) this.writeAck({ through: this.state!.seq, extra: [] });
        return;
      }
    }
  }

  /**
   * What this browser holds for the account, from what the store handed back
   * (`raw`) — and, where the store is not `localStorage` itself, what older
   * pages keep there, read every time (an older page may still be writing
   * it) and never deleted: their pin counts where the store has none (and
   * another one is a fork); their record, beside that same pin, is folded in
   * only as far as it is proven one chain with the store's, and is otherwise
   * kept apart, every view held against it as well ({@link higherSeen}). It
   * never lowers the record; one that disagrees with it is a fork (latched
   * and thrown). A record with no pin beside it says nothing of this chain.
   */
  private trustFrom(raw: unknown): { held: Held; stored: StoredTrust } {
    const stored = parseStored(raw);
    const none: Held = { genesis: null, record: null, apart: null };
    if (!this.seen.atomic) return { held: stored.genesis === null ? none : { genesis: stored.genesis, record: stored.seen, apart: null }, stored };
    const theirGenesis = this.read(genesisKey(this.account));
    if (stored.genesis !== null && theirGenesis !== null && theirGenesis !== stored.genesis) {
      throw this.latchFork("an older page of this browser pinned a different genesis for this account");
    }
    const genesis = stored.genesis ?? theirGenesis;
    if (genesis === null) return { held: none, stored };
    const theirRaw = theirGenesis === genesis ? this.read(seenKey(this.account)) : null;
    const both = higherSeen(stored.seen, theirRaw === null ? null : parseSeenJson(theirRaw));
    if ("forkAt" in both) throw this.latchFork(`the record an older page of this browser left forks at entry ${both.forkAt} from the one this browser keeps`);
    return { held: { genesis, record: both.record, apart: both.apart }, stored };
  }

  /** `op` on the seen store; whatever fails there (not a fork) is a {@link SeenStoreError}: tried again, never taken for a fork. */
  private async atStore<T>(op: () => Promise<T>): Promise<T> {
    try {
      return await op();
    } catch (error) {
      if (error instanceof AccountError || error instanceof SeenStoreError) throw error;
      throw new SeenStoreError(`what this browser verified of the account could not be read or written (${describeError(error)}): try again`, { cause: error });
    }
  }

  /** What this browser holds for the account now ({@link trustFrom}). */
  private async readHeld(): Promise<Held> {
    const { held } = this.trustFrom(await this.atStore(() => this.seen.get(this.account)));
    this.pinnedAt = held.genesis;
    return held;
  }

  /**
   * Read what this browser holds for the account, make the next of it
   * (`make`: a pin and a record; `undefined`: leave them), and write that —
   * as one step across the browser's pages: the store's transaction, which no
   * other page's comes between (where it is `localStorage`, only the Web Lock
   * the caller holds). What older pages keep in `localStorage` is folded in
   * as {@link trustFrom} says, and left there. `make` runs inside the
   * transaction, synchronously; a fork it throws, or the fold finds, leaves
   * everything as it was. Nothing is written when nothing changed. Resolves
   * once it is committed, and rejects (a {@link SeenStoreError}) when it was
   * not: then nothing that hangs on it may go on. Once committed, the pin and
   * the record are put where older pages read them too ({@link mirrorHeld}:
   * `view`, the view `make` held against them, may prove them one chain).
   */
  private async updateHeld(make: (held: Held) => { genesis?: string; record?: SeenRecord } | undefined, view?: ChainState): Promise<void> {
    const step: { genesis: string | null; record: SeenRecord | null } = { genesis: null, record: null };
    await this.atStore(() =>
      this.seen.update(this.account, (raw) => {
        const { held, stored } = this.trustFrom(raw);
        const change = make(held);
        const next: StoredTrust = { v: 1, genesis: change?.genesis ?? held.genesis, seen: change?.record ?? held.record };
        step.genesis = next.genesis;
        step.record = next.seen;
        return sameStored(next, stored) ? undefined : next;
      }),
    );
    this.pinnedAt = step.genesis;
    if (step.genesis !== null) this.mirrorHeld(step.genesis, step.record, view);
  }

  /**
   * The pin and the record this browser holds (`genesis`, `record`), where
   * older pages of it read them (`localStorage`; and a page that cannot open
   * IndexedDB): else a page of an older version knows nothing of what newer
   * pages verified, and a hub could keep it behind that — its word for an
   * old head, a load of an old chain. The pin where there is none; the
   * record only where it raises theirs and is proven one chain with it — by
   * the two records, or by `view` (a verified view holding the head of
   * both: a view that went further than the record keeps hashes of, say) —
   * never over a higher one, nor one kept apart (which is a checkpoint of
   * its own), nor under another pin. New pages only ever fold that record
   * in ({@link trustFrom}). Best effort, under the caller's seen-record lock
   * (as older pages write it); as `localStorage` goes, a page's copy of it
   * can be stale, as it always could.
   */
  private mirrorHeld(genesis: string, record: SeenRecord | null, view?: ChainState): void {
    const theirGenesis = this.read(genesisKey(this.account));
    if (theirGenesis === null) this.write(genesisKey(this.account), genesis);
    else if (theirGenesis !== genesis) return;
    // Where the store is `localStorage` itself, it wrote the record there.
    if (!record || !this.seen.atomic) return;
    const theirRaw = this.read(seenKey(this.account));
    const theirs = theirRaw === null ? null : parseSeenJson(theirRaw);
    if (theirs) {
      if (theirs.seenThrough >= record.seenThrough) return;
      const both = higherSeen(record, theirs);
      if ("forkAt" in both) return;
      const holds = (mark: SeenRecord) => !!view && view.seq >= mark.seenThrough && hex(view.hashAt(mark.seenThrough)!) === mark.head;
      if (both.apart && !(holds(record) && holds(theirs))) return;
    }
    this.write(seenKey(this.account), JSON.stringify(record));
  }

  /**
   * The record `state` makes of what this browser holds (`held`), once held
   * against both its checkpoints (a fork latched and thrown): the record
   * moved forward with this view's first sightings ({@link seenNext}), and an
   * older page's kept apart folded in once that is proven one chain with it.
   */
  private nextRecord(state: ChainState, held: Held, clock: ChainClock): SeenRecord {
    const next = seenNext(this.heldAgainst(state, held), held.record, clock);
    if (!held.apart) return next;
    const both = higherSeen(next, held.apart);
    if ("forkAt" in both) throw this.latchFork(`this page's chain forks at entry ${both.forkAt} from the one an older page of this browser verified`);
    return both.record!;
  }

  /** `state` held against what this browser holds ({@link seenAgainst}), the checkpoint kept apart as well. */
  private heldAgainst(state: ChainState, held: Held): { record: Omit<SeenRecord, "firstSeen">; checked: boolean } {
    const against = this.seenAgainst(state, held.record);
    if (held.apart) {
      const apart = compareSeen(state, held.apart);
      if ("forkAt" in apart) throw this.latchFork(`this page's chain forks at entry ${apart.forkAt} from the one an older page of this browser verified`);
    }
    return against;
  }

  /** Forget the seen record (a wipe), in the store and where older pages keep it; the pin stays. */
  private async forgetSeen(): Promise<void> {
    try {
      await this.withSeenLock(() =>
        this.atStore(() =>
          this.seen.update(this.account, (raw) => {
            const stored = parseStored(raw);
            return stored.seen === null ? undefined : { ...stored, seen: null };
          }),
        ),
      );
    } catch (error) {
      console.warn("cloud account: could not forget the seen record:", error);
    }
    this.forget(seenKey(this.account));
  }

  /**
   * Set when this view conflicted with the chain this browser verified (a
   * fork): why. From then on this page attests nothing, opens no channel,
   * signs no entry and seals no key, and the app shows the alarm; a reload
   * verifies afresh against the record, which still holds what was verified.
   */
  get forked(): string | null {
    return this.forkedWhy;
  }

  /** Latch this page's fork alarm (see {@link forked}), and the error to throw for it. */
  private latchFork(message: string): AccountError {
    if (this.forkedWhy === null) {
      this.forkedWhy = message;
      console.error(`cloud account: ALARM: ${message}`);
      this.emit();
    }
    return new AccountError("fork", message);
  }

  /** Throws once this page's view forked ({@link forked}): nothing is attested, opened, signed or sealed on it. */
  assertNotForked(): void {
    if (this.forkedWhy !== null) throw new AccountError("fork", this.forkedWhy);
  }

  /**
   * `work` in this page's turn at the seen record (one at a time here),
   * holding the account's seen-record lock across the browser's pages (Web
   * Locks) where there are any.
   */
  private withSeenLock<T>(work: () => T | Promise<T>): Promise<T> {
    const run = () => (this.locks ? this.locks.request(seenKey(this.account), { mode: "exclusive" }, work) : work());
    const turn = this.seenTurn.then(run, run);
    this.seenTurn = turn.catch(() => undefined);
    return turn;
  }

  /**
   * What `state` makes of the seen record `stored`, which every page of this
   * browser shares and which only moves forward ({@link compareSeen}); a
   * fork is latched and thrown, before anything is written or installed.
   */
  private seenAgainst(state: ChainState, stored: SeenRecord | null): { record: Omit<SeenRecord, "firstSeen">; checked: boolean } {
    const against = compareSeen(state, stored);
    if ("forkAt" in against) throw this.latchFork(`this page's chain forks at entry ${against.forkAt} from the one another page of this browser verified`);
    return against;
  }

  /**
   * Hold what this browser holds against this view again, and put back what
   * this page verified where the record is lower (or gone, or the pin with
   * it) — said loudly: no page's write lowers it, so something else did (code
   * in this origin, a wipe in another page). One that conflicts with this
   * view, or another pin, is a fork.
   */
  private async recheckSeen(): Promise<void> {
    const done = () => !this.state || this.forkedWhy !== null || this.standing === "removed";
    if (done()) return;
    if (!this.pinned) {
      // `pinned` is the store as this page last read or wrote it: another
      // page may have pinned the account since (two pairing screens open).
      // Taken in here, and this view held against the record from then on.
      await this.readHeld();
      if (done() || !this.pinned) return;
      this.emit();
    }
    await this.withSeenLock(() =>
      this.updateHeld((held) => {
        const state = this.state!;
        const genesis = hex(state.hashAt(0)!);
        if (held.genesis !== null && held.genesis !== genesis) throw this.latchFork("another genesis was pinned for this account under this page");
        if (held.genesis !== null && held.record && held.record.seenThrough >= state.seq) {
          // Not lowered below this view; a conflict still shows.
          this.heldAgainst(state, held);
          return undefined;
        }
        const what = held.genesis === null ? "the pin and the seen record were gone" : held.record ? `the seen record was lowered to entry ${held.record.seenThrough}` : "the seen record was gone";
        console.error(`cloud account: ALARM: ${what} under this page's ${state.seq}; putting it back`);
        return { genesis, record: this.nextRecord(state, held.genesis === null ? { genesis: null, record: null, apart: null } : held, this.clock) };
      }),
    );
  }

  /**
   * While this page is open, keep the seen record from staying lowered (or
   * gone): hold it against this view now, whenever another page writes it
   * (the store says so: a `BroadcastChannel`; `storage` events where the
   * record is in `localStorage`), and whenever this page comes back into
   * view ({@link recheckSeen}). What it cannot help: a record lowered while
   * no page holding the higher view is open — the next load then trusts the
   * lower one.
   */
  watchSeenRecord(on: SeenWatch = defaultSeenWatch()): () => void {
    this.unwatchSeen?.();
    const look = () => void this.recheckSeen().catch((error: unknown) => console.warn("cloud account: reading the seen record again:", error));
    const unhear = this.seen.watch((account) => {
      if (account === null || account === this.account) look();
    });
    const onVisibility = () => {
      if (on.visibility?.visibilityState !== "hidden") look();
    };
    on.visibility?.addEventListener("visibilitychange", onVisibility);
    look();
    const unwatch = () => {
      unhear();
      on.visibility?.removeEventListener("visibilitychange", onVisibility);
      if (this.unwatchSeen === unwatch) this.unwatchSeen = null;
    };
    this.unwatchSeen = unwatch;
    return unwatch;
  }

  /**
   * Make `next` this view (with `clock`), if it extends it — and first hold
   * it against what this browser verified, in the same step that writes it,
   * so a view that conflicts with it is never installed: the fork is latched
   * and thrown, and this view stays as it was. Nor is a view whose record
   * could not be written (the store failed: a {@link SeenStoreError}, to be
   * tried again). `false`: `next` held nothing this view lacks (another call
   * took it meanwhile).
   */
  private async install(next: ChainState, clock: ChainClock): Promise<boolean> {
    return this.withSeenLock(async () => {
      this.assertNotForked();
      const current = this.state;
      if (current) {
        if (next.seq <= current.seq) {
          if (!equal(current.hashAt(next.seq)!, next.head)) throw this.latchFork(`two views of the account's chain in this page differ at entry ${next.seq}`);
          // Nothing new — but held against the record all the same: another
          // page may have verified, since this view was taken, a chain this
          // one contradicts, and a handshake on it must not go on.
          const held = await this.readHeld();
          if (held.genesis === hex(current.hashAt(0)!)) this.heldAgainst(current, held);
          return false;
        }
        if (!equal(next.hashAt(current.seq)!, current.head)) throw this.latchFork(`two views of the account's chain in this page differ at entry ${current.seq}`);
        // The sightings this view took meanwhile, kept (the earliest of each).
        for (const [seq, ms] of this.clock.firstSeen) {
          const at = clock.firstSeen.get(seq);
          if (at === undefined || ms < at) clock.firstSeen.set(seq, ms);
        }
      }
      // This device's id, listed with keys that are not this browser's: the
      // hub gave this browser someone else's id (a device waiting after a
      // passkey join, say). Never "pending", never this device: an alarm.
      const listed = next.deviceRecord(this.deviceId);
      if (listed && (!equal(listed.info.signKey, this.keys.signPublic) || !equal(listed.info.dhKey, this.keys.dhPublic))) {
        throw this.latchFork(`the account's chain lists this device's id (${this.deviceId}) with keys that are not this browser's`);
      }
      // Kept only under the pin it is of (a chain this browser never pinned is not recorded).
      const genesis = hex(next.hashAt(0)!);
      // How far this browser has verified, with this view: what a dispute past the view is placed by, below.
      let recorded = -1;
      try {
        await this.updateHeld((held) => {
          if (held.genesis !== genesis) return undefined;
          const record = this.nextRecord(next, held, clock);
          recorded = Math.max(record.seenThrough, ...checkpoints(held).map((mark) => mark.seenThrough));
          return { record };
        }, next);
      } catch (error) {
        // Verified, not taken: this page's word for the older head it still shows is no good meanwhile.
        if (error instanceof SeenStoreError) this.unrecordedThrough = Math.max(this.unrecordedThrough, next.seq);
        throw error;
      }
      // What this page kept or heard of that the new view can judge, checked on
      // it before it is taken: one that counted while this view was behind goes
      // on counting, or stops, with no moment in between — one heard while
      // those checks ran too (the last look, finding none, is right before
      // the view is taken).
      do {
        await this.renewalNotes.checkDisputesOn(next);
      } while (this.renewalNotes.disputesDueOn(next).length);
      // And those past it placed by the record just written, in the same step:
      // a reload holds a host isolated before any channel opens.
      this.renewalNotes.placeDisputesPast(next, recorded);
      // A fork latched meanwhile (another call of this page) installs nothing either.
      this.assertNotForked();
      this.state = next;
      this.clock = clock;
      return true;
    });
  }

  /** A copy of this view's clock, for a merge that is installed only once it is held against the seen record. */
  private clockCopy(): ChainClock {
    return new ChainClock(this.clock.nowMs, this.clock.liveFromSeq, new Map(this.clock.firstSeen));
  }

  /**
   * Finish a channel handshake (`finishHandshake`, as `finish` does) on a
   * copy of this view, which is then held against this view and the seen
   * record — whether the agent accepted the channel or refused it — and
   * taken where it adds entries. A refusal can be exactly how this device
   * learns of a removal or a recovery join: what the reply taught (it was
   * verified) is kept, not left for a hub that may go on withholding it. A
   * fork, whether the handshake or the copy shows it, is latched (`forked`)
   * and thrown; nothing else of the channel goes on.
   */
  async completeHandshake<T>(finish: (merged: { chain: ChainState; clock: ChainClock }) => Promise<T>): Promise<T> {
    this.assertNotForked();
    if (!this.state) throw new AccountError("notInChain", "the account has no chain yet");
    this.clock.nowMs = Date.now();
    const merged = { chain: this.state.clone(), clock: this.clockCopy() };
    let result: T;
    try {
      result = await finish(merged);
    } catch (error) {
      if (error instanceof HandshakeError && error.kind === "fork") throw this.latchFork(error.message);
      await this.takeMerged(merged);
      throw error;
    }
    await this.takeMerged(merged);
    return result;
  }

  /**
   * A handshake's copy of this view, held against it — newer, equal or
   * older, each must be the same chain where they overlap (else a fork) —
   * and taken (and told) where it adds entries.
   */
  private async takeMerged(merged: { chain: ChainState; clock: ChainClock }): Promise<void> {
    if (await this.install(merged.chain, merged.clock)) this.emit();
  }

  /**
   * Bring the chain up to date from the hub, verifying every entry. The first
   * load of a page resumes this device's live clock where it left off (or, on
   * a browser that never pinned the account, starts it at the head: every
   * entry is history, each first seen now). A chain shorter than what this
   * device verified before, or one that does not contain the head it saw, is
   * a fork — looked at once more first, as another page of this browser may
   * have recorded more just after this one fetched — and so is no chain at
   * all on a browser that pinned the account's genesis.
   *
   * It takes the chain as far as this device's clock lets it, and keeps that
   * (M2 re-review): a recovered device's self-activation that is too early by
   * this device's own first sighting of its join stops it there — the join
   * is in, first seen now and saved, so it shows as pending and can be
   * vetoed, and the activation is taken once the day has passed here
   * (`heldBack`). It used to throw before anything was saved, so every load
   * saw the join "for the first time" and the page never opened again.
   */
  sync(): Promise<number> {
    return this.catchUp(this.session.token);
  }

  /** `sync` with `token` (a login token, read-only, for `confirmRemoval`). */
  private async catchUp(token: string, opts: { noRenew?: boolean } = {}): Promise<number> {
    this.assertNotForked();
    const now = Date.now();
    this.clock.nowMs = now;
    if (!this.state) {
      const served = async () => (await this.api.chain(token, 0, opts)).entries.map((e) => decodeEntry(e));
      let decoded = await served();
      if (decoded.length === 0) {
        if ((await this.pinnedGenesis()) !== null) throw this.latchFork("the hub serves no chain for an account whose genesis this browser pinned");
        return 0;
      }
      let verified: Awaited<ReturnType<CloudAccount["verifyServed"]>>;
      // Other pages may record more while this one loads, more than once: a
      // served chain that is only behind the record (and agrees with it) is
      // looked at again a few times before it is called a fork; one that
      // contradicts it is a fork at once (`verifyServed` latches it).
      for (let tries = 1; ; tries++) {
        try {
          verified = await this.verifyServed(decoded, now);
          break;
        } catch (error) {
          if (!(error instanceof MovingCheckpoint) || this.forkedWhy !== null) throw error;
          if (tries >= FIRST_LOAD_TRIES) throw this.latchFork(error.message);
          decoded = await served();
        }
      }
      await this.install(verified.state, verified.clock);
      this.setHeld(verified.held);
      this.emit();
      return verified.state.seq + 1;
    }
    const { entries } = await this.api.chain(token, this.state.seq + 1, opts);
    const decoded = entries.map((e) => decodeEntry(e));
    const next = this.state.clone();
    const clock = this.clockCopy();
    const { applied, error } = await next.mergePrefix(decoded, clock);
    // What took effect is kept and told before anything else is decided — once held against the seen record.
    if (applied && (await this.install(next, clock))) this.emit();
    this.setHeld(this.heldFrom(error, decoded));
    return applied;
  }

  /**
   * The hub's whole chain verified as this device verifies every first load
   * — the one check `sync` and `confirmRemoval` share: against the genesis it
   * pinned and the head it saw last (else a fork, not latched: the caller
   * looks once more), on its own live clock resumed from its saved first
   * sightings, and as far as that clock lets it.
   */
  private async verifyServed(decoded: Entry[], now: number): Promise<{ state: ChainState; clock: ChainClock; held: HeldBack | null }> {
    const trust = await this.readHeld();
    const pinned = trust.genesis;
    const seen = pinned ? trust.record : null;
    const clock = seen ? ChainClock.resume(now, seen.seenThrough, seen.firstSeen) : ChainClock.live(now, decoded.length);
    const { state, error } = await ChainState.verifyPrefix(decoded, undefined, clock);
    if (pinned && pinned !== hex(state.hashAt(0)!)) throw this.latchFork("the hub served a different genesis than this device pinned");
    const held = this.heldFrom(error, decoded);
    // Against every checkpoint this browser holds (an older page's kept apart too).
    let behind = false;
    for (const mark of pinned ? checkpoints(trust) : []) {
      if (state.seq < mark.seenThrough) {
        // Shorter than the record: a view another page of this browser went
        // past a moment ago — a checkpoint still moving, to be looked at again
        // — unless it contradicts the record where they overlap.
        const theirs = mark.heads?.find(([seq]) => seq === state.seq)?.[1];
        if (theirs !== undefined && theirs !== hex(state.head)) throw this.latchFork(`the hub served a chain that forks at entry ${state.seq} from the one this device saw`);
        behind = true;
      } else if (hex(state.hashAt(mark.seenThrough)!) !== mark.head) {
        throw this.latchFork("the hub served a chain without the head this device saw");
      }
    }
    if (behind) throw new MovingCheckpoint("the hub served less of the chain than this device has seen");
    return { state, clock, held };
  }

  /**
   * Why a merge of `served` stopped, as what this device holds back: `null`
   * when it did not stop; a self-activation too early here (and everything
   * the hub served from it) — with the recovery words, or through a passkey
   * slot (`passkeyActivate`: any admin may veto it, docs/passkeys.md §4.4,
   * so it names its device as a recovered one does); anything else is
   * thrown — a fork as the alarm it is (latched), an entry that does not
   * verify as itself.
   */
  private heldFrom(error: ChainError | null, served: Entry[]): HeldBack | null {
    if (!error) return null;
    if (error.kind === "fork") throw this.latchFork(error.message);
    if (error.kind !== "tooEarly" || error.detail.seq === undefined) throw error;
    const seq = error.detail.seq;
    const entries = served.filter((e) => e.draft.seq >= seq);
    const action = entries[0]?.draft.action;
    const device = action?.type === "activateRecovered" || action?.type === "passkeyActivate" ? action.id : null;
    return { seq, device, readyMs: error.detail.readyMs ?? 0, entries };
  }

  /**
   * Entries the hub has past this view that this device does not take yet: a
   * recovered (or passkey-joined) device's self-activation that comes before
   * a day has passed since this device first saw its join (and whatever
   * follows it). The join is in this view, pending: "approve" takes the rest
   * on the user's word, "remove" vetoes it on top of the hub's chain. Taken
   * by itself once `readyMs` has passed (the next sync).
   */
  get heldBack(): HeldBack | null {
    return this.held;
  }

  private setHeld(held: HeldBack | null): void {
    const was = this.held;
    // A hold taken in by the clock (not by the person's approve or veto, `adopt`): kept, so a veto confirmed on it a moment ago still is one.
    if (was?.device && was.seq !== held?.seq && this.state && this.state.seq >= was.seq) {
      const at = this.state.entryAt(was.seq)?.draft.action;
      if ((at?.type === "activateRecovered" || at?.type === "passkeyActivate") && at.id === was.device) this.taken = { seq: was.seq, device: was.device, takenThroughSeq: this.state.seq };
    }
    this.held = held;
    this.scheduleHeld();
    if ((was?.seq ?? -1) !== (held?.seq ?? -1) || (was?.entries.length ?? 0) !== (held?.entries.length ?? 0)) this.emit();
  }

  /** While the page is open, look again once a held-back activation may be taken. */
  private scheduleHeld(): void {
    if (this.heldTimer) clearTimeout(this.heldTimer);
    this.heldTimer = null;
    if (!this.held || !this.keepFresh) return;
    const wait = Math.max(1000, this.held.readyMs - Date.now() + 1000);
    this.heldTimer = setTimeout(
      () => {
        this.heldTimer = null;
        this.sync().catch((error: unknown) => console.warn("cloud account: sync after a held-back activation:", error));
      },
      Math.min(wait, 6 * 60 * 60 * 1000),
    );
    (this.heldTimer as { unref?: () => void }).unref?.();
  }

  /**
   * Entries the hub pushed (a `chain` frame): merged if they follow on, else
   * a sync over HTTP fills the gap. A fork is an alarm, never merged.
   */
  async absorb(encoded: Uint8Array[]): Promise<number> {
    this.assertNotForked();
    // Pushes follow the hub's head, past what this view holds back.
    if (!this.state || this.held) return this.sync();
    let entries: Entry[];
    try {
      entries = encoded.map((e) => decodeEntry(e));
    } catch {
      return 0;
    }
    this.clock.nowMs = Date.now();
    const next = this.state.clone();
    const clock = this.clockCopy();
    const { applied, error } = await next.mergePrefix(entries, clock);
    if (applied && (await this.install(next, clock))) this.emit();
    if (error?.kind === "outOfOrder") return applied + (await this.sync());
    this.setHeld(this.heldFrom(error, entries));
    return applied;
  }

  /**
   * Where this device stands: an active device, a recovered one awaiting
   * approval, removed, or not in the chain. (A record for this id with other
   * keys never gets into this view: `install` calls it a fork.)
   */
  get standing(): Standing {
    const record = this.state?.deviceRecord(this.deviceId);
    if (!record) return "absent";
    return record.status.type;
  }

  /** Whether this device is a current, active device of the account's chain. */
  get inChain(): boolean {
    return this.state?.device(this.deviceId) !== undefined;
  }

  /** Entries the hub took, into this view (merged: the hub's push of them may have landed first). */
  private async landed(entries: Entry[], now = Date.now()): Promise<void> {
    this.clock.nowMs = now;
    const next = this.state!.clone();
    const clock = this.clockCopy();
    const { error } = await next.mergePrefix(entries, clock);
    await this.install(next, clock);
    this.emit();
    if (error) throw error;
  }

  /**
   * The hub's chain past a held-back activation, verified with no clock:
   * only ever to sign this device's veto on (a removal must go on the hub's
   * head) or to take on the user's word ("approve") — never trusted before.
   */
  private async provisional(held: HeldBack): Promise<ChainState> {
    return ChainState.verify([...this.state!.allEntries(), ...held.entries], undefined, ChainClock.history());
  }

  /**
   * Take `chain` — the hub's, past a held-back self-activation — with this
   * device's own `mine` on it: a veto (or the user's "approve") settled what
   * the day's delay was there for. Everything new is first seen now, and the
   * clock goes on live from its head.
   */
  private async adopt(chain: ChainState, mine: Entry[], now = Date.now()): Promise<void> {
    if (mine.length) await chain.merge(mine, ChainClock.history());
    const clock = ChainClock.resume(now, chain.seq, this.clock.firstSeen.entries());
    for (let seq = this.state!.seq + 1; seq <= chain.seq; seq++) clock.observe(seq);
    await this.install(chain, clock);
    this.held = null;
    // Settled by the person: no earlier plan is a veto any more.
    this.taken = null;
    this.scheduleHeld();
    this.emit();
  }

  /**
   * `actions` as this device's entries, each on the one before (from `chain`'s
   * head). `asV3`: the shared actions as v3 from the first one on (the
   * account's upgrade: its first v3 entry).
   */
  private async signChained(chain: ChainState, actions: Action[], asV3 = false): Promise<Entry[]> {
    let seq = chain.seq;
    let prev = copy(chain.head);
    // As `ChainState.draft` would on the chain with the entries before applied:
    // a shared action is v3 once the chain (or this batch) is.
    let v3 = chain.isV3 || asV3;
    const out: Entry[] = [];
    for (const action of actions) {
      const version = fixedVersion(action) ?? (v3 ? CHAIN_VERSION_3 : CHAIN_VERSION);
      if (version === CHAIN_VERSION_3) v3 = true;
      const draft: Draft = { version, account: this.account, seq: ++seq, prev, createdMs: Date.now(), signer: { type: "device", id: this.deviceId }, action };
      this.assertNotForked();
      const entry = await signDraft(draft, this.keys.sign.privateKey);
      out.push(entry);
      prev = await entryHash(entry);
    }
    return out;
  }

  /**
   * Sign the entries `build` makes for the chain as it stands, and append
   * them in one call (they land together or not at all) — built again on a
   * newer head once, if the hub had one this view lacked.
   *
   * While an activation is held back (`heldBack`) the hub's head is past this
   * view: only a removal (`overHeld`, the veto) is signed there, on the hub's
   * chain, which this device then takes; anything else waits for the user to
   * approve or remove the device first.
   */
  private async appendBatch(
    build: (chain: ChainState) => Promise<Action[]>,
    opts: {
      overHeld?: boolean;
      asV3?: boolean;
      /** Checked right before the signed batch is published (after the signing's awaits), on the chain it was built on: a throw publishes nothing. */
      beforePublish?: (base: ChainState) => void;
    } = {},
  ): Promise<Entry[]> {
    this.assertNotForked();
    for (let attempt = 0; ; attempt++) {
      if (!this.state) throw new AccountError("notInChain", "the account has no chain yet");
      const held = this.held;
      if (held && !opts.overHeld) {
        throw new AccountError(
          "heldBack",
          `${heldWho(held)}: approve or remove it first`,
          held.readyMs,
        );
      }
      const base = held ? await this.provisional(held) : this.state;
      const actions = await build(base);
      // The hub takes 1–64 entries an append: a bigger batch is said here, before anything is signed.
      if (actions.length > MAX_APPEND_ENTRIES) {
        throw new AccountError("tooManyEntries", "that would write {{n}} entries to the account's chain at once, and the hub takes at most {{max}}: do it in smaller steps", undefined, undefined, {
          n: String(actions.length),
          max: String(MAX_APPEND_ENTRIES),
        });
      }
      const entries = await this.signChained(base, actions, opts.asV3);
      // Refused here, by the chain's own rule, before anything leaves this device (review 13).
      await verifyBatch(base, entries);
      try {
        this.assertNotForked();
        opts.beforePublish?.(base);
        await this.api.appendChain(
          this.session.token,
          entries.map((e) => encodeEntry(e)),
        );
      } catch (error) {
        // A newer head is worth one more try; a used-up limit (409 `limit`)
        // is not, nor a hub that takes no v3 entry yet (`chain_v3_not_ready`),
        // nor a renewal it does not take yet (`rotation_not_ready`: its caller waits).
        if (
          attempt === 0 &&
          error instanceof HubApiError &&
          error.status === 409 &&
          error.code !== "limit" &&
          error.code !== "chain_v3_not_ready" &&
          error.code !== "rotation_not_ready"
        ) {
          await this.sync();
          continue;
        }
        throw error;
      }
      if (held) await this.adopt(base, entries);
      else await this.landed(entries);
      return entries;
    }
  }

  /** Sign and append one entry as this device (once more on a newer head, if the hub had one this view lacked). */
  async append(action: Action): Promise<void> {
    await this.appendBatch(async () => [action]);
  }

  /**
   * `appendBatch` for a batch that adds host `host` (or makes it one again):
   * the host's agent declares `chain/3` as it connects, which can come a
   * moment after its enrollment proof — so a 409 `chain_v3_not_ready` that
   * waits for that host alone is tried again for a few seconds
   * ({@link CHAIN_V3_WAIT}). One that names any other host is not.
   */
  private async appendWaitingFor(host: string, build: (chain: ChainState) => Promise<Action[]>, wait = CHAIN_V3_WAIT): Promise<Entry[]> {
    const deadline = Date.now() + wait.forMs;
    for (;;) {
      try {
        return await this.appendBatch(build);
      } catch (error) {
        const waiting = error instanceof HubApiError && error.status === 409 && error.code === "chain_v3_not_ready" ? (error.waitingHosts ?? null) : null;
        if (!waiting || waiting.length !== 1 || waiting[0] !== host || Date.now() + wait.everyMs > deadline) throw error;
        await new Promise((resolve) => setTimeout(resolve, wait.everyMs));
      }
    }
  }

  /** This device must manage the account (else `notAdmin`). */
  private requireAdmin(): ChainState {
    const chain = this.state;
    if (!chain || this.standing !== "active" || !this.pinned) throw new AccountError("notInChain", "only an active device of the account does this");
    if (!this.isAdmin) throw new AccountError("notAdmin", "only a full device of the account can do this");
    return chain;
  }

  /** A v3-only change: the chain must hold a v3 entry already (review D4: the upgrade is a step of its own). */
  private requireUpgraded(chain: ChainState): void {
    if (!chain.isV3) throw notUpgraded();
  }

  /** What the hub gates on a v3 chain (a host added, or made one again): only while it takes v3 entries (`chain_v3.writable`). */
  private requireWritable(writable: boolean | undefined): void {
    if (!writable) throw new AccountError("notWritable", "this needs Droppen updated on every device of the account that runs AI first");
  }

  // ------------------------------------------------------------ alerts

  private readAck(): AckRecord | null {
    try {
      const raw = this.read(ackKey(this.account, this.deviceId));
      if (!raw) return null;
      const r = JSON.parse(raw) as Partial<AckRecord>;
      if (!Number.isSafeInteger(r.through) || !Array.isArray(r.extra)) return null;
      return { through: r.through!, extra: r.extra.filter((n): n is number => Number.isSafeInteger(n)) };
    } catch {
      return null;
    }
  }

  private writeAck(ack: AckRecord): void {
    this.write(ackKey(this.account, this.deviceId), JSON.stringify(ack));
  }

  /**
   * What this device has not acknowledged: every device added or removed,
   * recovery join and new recovery key since it trusted the account, that it
   * did not sign or act on itself — oldest first. (A recovery join still
   * waiting for approval is the pending-recovery banner's.)
   */
  alerts(): ChainAlert[] {
    return this.chainAlerts.alerts();
  }

  /** "That was me": the alert at `seq` is acknowledged on this device. */
  acknowledge(seq: number): void {
    this.chainAlerts.acknowledge(seq);
  }

  /**
   * Whether a rotation ("rotate keys again") is due: a device that held the
   * user key no longer manages the account, and the keys it could read are
   * not all retired yet — no new user key since, or a current host still on
   * a machine key from before it (see {@link rotationDueIn}).
   */
  get rotationDue(): boolean {
    return this.state ? rotationDueIn(this.state) : false;
  }

  /**
   * Whether the newest user key is one a device that no longer manages the
   * account held ({@link userKeyStale}): a batch that issues machine keys
   * carries a new user key epoch of its own then, and wraps them under it.
   */
  get userKeyStale(): boolean {
    return this.state ? userKeyStale(this.state) : false;
  }

  /** Whether device `id` ever held the user key, by the chain's history (review 1). */
  heldUserKey(id: string): boolean {
    return this.state ? heldUserKey(this.state, id) : false;
  }

  /**
   * The settle steps an active device runs after loading (and after the chain
   * moves): take the keys mailed to it, refresh the keyring, move M1's raw
   * machine keys out of the key store (whatever the device's role:
   * {@link migrate}), and keep a recovery package for every epoch it holds.
   * Each step on its own: one failing leaves the others; listeners are told
   * once they have run. Nothing at all on a device that has not pinned the
   * account.
   */
  async settle(): Promise<void> {
    if (this.standing !== "active" || !this.pinned) return;
    // A device that does not manage the account holds no user key (chain v3):
    // it keeps the hosts' machine keys it was handed (and the raw ones it
    // still had from before the keyring), and asks for those it lacks.
    const steps: [string, () => Promise<unknown>][] = this.isAdmin
      ? [
          ["keyring", () => this.keyring.refresh()],
          ["mail", () => this.receiveMail()],
          ["newest user key", () => this.checkUserKey()],
          ["migrate", () => this.migrate()],
          ["recovery packages", () => this.ensureRecoveryPackages("any")],
        ]
      : [
          ["keyring", () => this.keyring.refresh()],
          ["mail", () => this.receiveMail()],
          ["migrate", () => this.migrate()],
          ["host keys", () => this.checkHostKeys()],
        ];
    for (const [what, step] of steps) {
      try {
        await step();
      } catch (error) {
        console.warn(`cloud account: ${what}:`, error);
      }
    }
    // What was read off the chain as it moved may have come before these:
    // a machine added from another page has its first key put in the keyring
    // just after its chain entry, so whatever read its keys then (its offline
    // copy, `HubCopySource.following`) reads again now.
    this.emit();
  }

  // ------------------------------------------------------------ keys

  /**
   * Create the account's chain, on chain v3 from its first entry
   * (local-aggregator §3.5): `genesis3` naming this device as the first
   * admin — a full device that does not run AI (review D1) — and `UK` epoch
   * 0 committed in the same call, as a v3 `keyEpochs`. That is where
   * `upgradeToV3` takes a v2 account (its first v3 entry is a user key
   * epoch), so a new account has nothing to upgrade, and nothing that reads
   * only chain v2 ever follows it.
   *
   * Only while the hub takes v3 entries (`opts.writable`; when not given,
   * `/v1/me`'s `chain_v3.writable` is asked): a hub that does not is refused
   * here, before anything is signed — and a 409 `chain_v3_not_ready` to the
   * append likewise — in words that say so. No v2 account is made in its
   * place. With `withRecovery`, returns the twelve recovery words to show
   * (once).
   */
  async createGenesis(withRecovery: boolean, opts: { writable?: boolean } = {}): Promise<string[] | null> {
    // A browser that pinned this account's genesis makes no other: another page
    // of this browser founded the account a moment ago (the pin is shared), or
    // the hub serves a chain it should not — a fork. Said before anything else.
    await this.refuseIfPinned();
    const writable = opts.writable ?? chainV3Writable(await this.api.me(this.session.token));
    if (!writable) throw noNewAccounts();
    try {
      return await this.foundAccount(withRecovery);
    } catch (error) {
      // The hub stopped taking v3 entries since it said it did.
      if (error instanceof HubApiError && error.status === 409 && error.code === "chain_v3_not_ready") throw noNewAccounts();
      throw error;
    }
  }

  /**
   * Refuse a new genesis on a browser that pinned one for this account. When
   * the hub's chain starts with that very genesis, another page of this
   * browser set the account up a moment ago (two pages on the set-up screen:
   * the pin is shared) — said as that (`setUpElsewhere`), not as a fork; any
   * other chain, or none, is a fork, latched.
   */
  private async refuseIfPinned(): Promise<void> {
    // What this browser holds that cannot be read tells neither, nor a chain
    // that cannot be read: those errors go up as they are, to be tried again.
    const trust = await this.readHeld();
    if (trust.genesis === null) return;
    const { entries } = await this.api.chain(this.session.token, 0);
    if (await this.holdsRecord(entries, trust)) {
      throw new AccountError("setUpElsewhere", "this account was just set up in another tab of this browser: reload this page");
    }
    throw this.latchFork("this browser pinned a genesis for this account already: it does not make a new one");
  }

  /**
   * Whether `served` is the chain this browser verified (`trust`): it
   * verifies as a whole, starts with the genesis pinned, and holds the head of
   * every checkpoint at its height — not merely the right first entry (a hub
   * could serve that alone, or with another history after it). Only the
   * served chain's own failings make it `false`.
   */
  private async holdsRecord(served: Uint8Array[], trust: Held): Promise<boolean> {
    let state: ChainState;
    try {
      if (served.length === 0) return false;
      const verified = await ChainState.verifyPrefix(
        served.map((e) => decodeEntry(e)),
        undefined,
        ChainClock.history(),
      );
      if (verified.error) return false;
      state = verified.state;
    } catch {
      return false;
    }
    if (hex(state.hashAt(0)!) !== trust.genesis) return false;
    return checkpoints(trust).every((mark) => state.seq >= mark.seenThrough && hex(state.hashAt(mark.seenThrough)!) === mark.head);
  }

  /** The v3 genesis and `UK` epoch 0 in one call ({@link createGenesis}). */
  private async foundAccount(withRecovery: boolean): Promise<string[] | null> {
    // AK: generated for this one signature and never stored.
    const ak = (await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
    const akPublic = new Uint8Array(await crypto.subtle.exportKey("raw", ak.publicKey));
    let words: string[] | null = null;
    let recoveryKey = null;
    if (withRecovery) {
      const rk = crypto.getRandomValues(new Uint8Array(16));
      try {
        words = await mnemonicFromEntropy(rk);
        recoveryKey = await recoveryKeyFor(await recoveryKeys(rk), this.account, akPublic);
      } finally {
        rk.fill(0);
      }
    }
    const now = Date.now();
    const draft = genesis3Draft(
      this.account,
      akPublic,
      { id: this.deviceId, signKey: this.keys.signPublic, dhKey: this.keys.dhPublic, flags: { host: false, admin: true } },
      recoveryKey,
      now,
    );
    const genesis = await signDraft(draft, ak.privateKey);
    const uk = random32();
    try {
      const commit: Draft = {
        version: CHAIN_VERSION_3,
        account: this.account,
        seq: 1,
        prev: await entryHash(genesis),
        createdMs: now,
        signer: { type: "device", id: this.deviceId },
        action: { type: "keyEpochs", user: { epoch: 0, hash: await userKeyCommit(this.account, 0, uk) }, machines: [] },
      };
      const keyEpochs = await signDraft(commit, this.keys.sign.privateKey);
      // Refused here, by the chain's own rule, before anything leaves this device (review 13).
      await verifyBatch(null, [genesis, keyEpochs]);
      this.assertNotForked();
      await this.api.appendChain(this.session.token, [encodeEntry(genesis), encodeEntry(keyEpochs)]);
      const clock = ChainClock.live(now, 0);
      this.state = await ChainState.verify([genesis, keyEpochs], undefined, clock);
      this.clock = clock;
      await this.pinMade();
      // Committed: now it may be kept and sealed.
      await this.vault.put(0, uk);
      try {
        await this.sealForRecovery(0, uk);
      } catch (error) {
        console.warn("cloud account: recovery package:", error);
      }
    } finally {
      uk.fill(0);
    }
    this.emit();
    return words;
  }

  private async sealForRecovery(epoch: number, raw: Uint8Array): Promise<boolean> {
    this.assertNotForked();
    const rsk = this.state?.currentRecoveryKey;
    if (!rsk) return false;
    this.assertNotForked();
    const sealed = await sealForRecovery(this.noiseKeys, this.deviceId, this.account, rsk, epoch, raw);
    await this.keyring.put(keyName.recovery(epoch, this.deviceId, rsk), sealed);
    return true;
  }

  /**
   * A recovery package from this device, for the current recovery key, for
   * every `UK` epoch it holds that has none: a package a device no longer
   * current sealed still opens, but none is counted on — after a
   * removal this device's own must cover every epoch.
   *
   * `scope`: `own` (a rotation, new words) writes this device's own package
   * for every epoch it has not written one for; `any` (the settle steps)
   * only for an epoch no current full device has a package for. The hub
   * keeps only the newest package per epoch (relay-and-plans §6), so with
   * `own` on every load two devices would take turns rewriting theirs.
   */
  async ensureRecoveryPackages(scope: "own" | "any" = "own"): Promise<number> {
    const chain = this.state;
    const rsk = chain?.currentRecoveryKey;
    if (!chain || !rsk || this.standing !== "active" || !this.pinned) return 0;
    const covered = new Set<number>();
    if (scope === "any") {
      const tag = recoveryTag(rsk);
      for (const p of this.keyring.recoveryBlobs()) if (p.tag === tag && chain.admin(p.sender)) covered.add(p.ukEpoch);
    }
    let made = 0;
    for (const epoch of await this.vault.epochs()) {
      if (covered.has(epoch) || this.keyring.hasBase(keyName.recovery(epoch, this.deviceId, rsk), this.deviceId)) continue;
      try {
        if (await this.vault.withRaw(epoch, (raw) => this.sealForRecovery(epoch, raw))) made++;
      } catch (error) {
        console.warn(`cloud account: recovery package for epoch ${epoch}:`, error);
      }
    }
    return made;
  }

  /** Whether another active admin ("full") device shares this account (and so may hold keys this one lacks). */
  private hasPeers(): boolean {
    return (this.state?.admins() ?? []).some((d) => d.info.id !== this.deviceId);
  }

  /**
   * The `UK` epoch new machine keys are wrapped under: the newest the chain
   * commits, and only if this device holds it — making (and committing) epoch
   * 0 only when this is the account's sole active device and the chain has
   * committed none. With another device about, the keys exist there and are
   * to be sent here, never made up.
   *
   * M2 re-review: this used to be the newest epoch *held*, so a device that
   * missed a rotation's mail (the hub dropped it, or the rotation died after
   * its commit) wrapped a new machine's key under a user key the removed
   * device still holds. Now a gap is "missing key" (`missingUserKey`): the
   * device looks again, asks the account's other devices for it, and wraps
   * nothing until it has it.
   */
  async ensureUserKey(): Promise<number | null> {
    const chain = this.state;
    // Only an admin holds (or makes) a user key.
    if (!this.isAdmin) return null;
    const latest = chain?.latestUserEpoch() ?? null;
    if (latest !== null) return (await this.checkUserKey()) === null ? latest : null;
    if (!chain || this.standing !== "active" || !this.pinned || this.hasPeers()) return null;
    const uk = random32();
    try {
      await this.appendBatch(async (c) => {
        if (c.userKeyEpochs() !== 0) throw new AccountError("noUserKey", "another device made the account's first user key meanwhile");
        return [{ type: "keyEpochs", user: { epoch: 0, hash: await userKeyCommit(this.account, 0, uk) }, machines: [] }];
      });
      await this.vault.put(0, uk);
      try {
        await this.sealForRecovery(0, uk);
      } catch (error) {
        console.warn("cloud account: recovery package:", error);
      }
    } finally {
      uk.fill(0);
    }
    this.emit();
    return 0;
  }

  /** Why there is no user key to wrap under: "missing key" when the chain commits one this device lacks. */
  private noUserKey(): AccountError {
    if (this.missingUserKey !== null) {
      return new AccountError(
        "missingKey",
        `missing key: this device does not hold the account's newest user key (epoch ${this.missingUserKey}). It asked the account's other devices for it; if none has it, rotate the keys again`,
      );
    }
    return new AccountError("noUserKey", "this device holds no user key yet");
  }

  /** The chain's newest committed `UK` epoch, when this device does not hold it. */
  async userKeyGap(): Promise<number | null> {
    const latest = this.state?.latestUserEpoch() ?? null;
    if (latest === null || (await this.vault.has(latest))) return null;
    return latest;
  }

  /**
   * Whether this device holds the chain's newest user key; if not, look for
   * it again (the mailbox) and ask the account's other full devices to send
   * it (`requestUserKeys`). Returns the epoch still missing (`null`: none),
   * and keeps `missingUserKey` up to date for the page.
   */
  async checkUserKey(): Promise<number | null> {
    // A device that does not manage the account never asks for a user key (chain v3 §3.3).
    let gap = this.isAdmin ? await this.userKeyGap() : null;
    if (gap !== null && this.standing === "active" && this.pinned) {
      try {
        await this.receiveMail();
      } catch (error) {
        console.warn("cloud account: mail:", error);
      }
      gap = await this.userKeyGap();
      if (gap !== null) {
        try {
          await this.requestUserKeys(gap);
        } catch (error) {
          console.warn("cloud account: asking for the user key:", error);
        }
      }
    }
    if (this.missingUserKey !== gap) {
      this.missingUserKey = gap;
      this.emit();
    }
    return gap;
  }

  /**
   * Ask every other current full device to mail this one its user keys
   * (purpose 4: no secret in it, only the epoch wanted). At most once per
   * epoch every {@link KEY_REQUEST_EVERY_MS} unless `force`. Returns who was asked.
   */
  async requestUserKeys(epoch: number, force = false): Promise<string[]> {
    if (!this.state || this.standing !== "active" || !this.pinned) return [];
    const key = keyRequestKey(this.account, this.deviceId);
    try {
      const last = JSON.parse(this.read(key) ?? "null") as { epoch?: unknown; atMs?: unknown } | null;
      if (!force && last?.epoch === epoch && typeof last.atMs === "number" && Date.now() - last.atMs < KEY_REQUEST_EVERY_MS) return [];
    } catch {
      // asked never, or the record is spoiled: ask
    }
    const asked: string[] = [];
    for (const d of this.state.admins()) {
      if (d.info.id === this.deviceId) continue;
      try {
        await this.api.mail(this.session.token, { recipient: d.info.id, sender: this.deviceId, purpose: MAIL_KEY_REQUEST, epoch, sealed: new Uint8Array(0) });
        asked.push(d.info.id);
      } catch (error) {
        console.warn(`cloud account: could not ask ${d.info.id} for the user key:`, error);
      }
    }
    this.write(key, JSON.stringify({ epoch, atMs: Date.now() }));
    return asked;
  }

  /**
   * An active device of the account that cannot read it: it never pinned
   * the account (it must pair again), or holds no user key (its keys never
   * arrived).
   */
  async lacksKeys(): Promise<boolean> {
    if (this.standing !== "active") return false;
    // Asked of the store where this page saw no pin: another page may have pinned the account since.
    if (!this.pinned) await this.readHeld();
    if (!this.pinned) return true;
    // Not an admin: it holds no user key by design, and asks for the hosts' keys it lacks.
    if (!this.isAdmin) return false;
    if ((await this.vault.epochs()).length) return false;
    return (await this.ensureUserKey()) === null;
  }

  /**
   * Mail every `UK` epoch held to device `id` — already an active full
   * device of the chain — sealed from this device to its chain key (purpose
   * 1, as a rotation does): how a device that missed a rotation's mail gets
   * its keys. It takes them only if it pinned the account before (a device
   * that never did pairs again instead).
   */
  async resendUserKeys(id: string): Promise<number[]> {
    if (id === this.deviceId) throw new AccountError("self", "send keys to this device from another one");
    const record = this.state?.admin(id);
    if (!record) throw new AccountError("notInChain", `${id} is not a current full device`);
    return this.withUserKeys(async (keys) => {
      if (!keys.length) throw new AccountError("noUserKey", "this device holds no user key to send");
      await this.mailUserKeys(id, record.info.dhKey, keys);
      return keys.map((k) => k.epoch);
    });
  }

  /**
   * M1/M2a kept machine keys as raw hex in the key store: move them out, on
   * every device, one page of the browser at a time (Web Locks) — a page that
   * finds another at it skips it: it is done again on the next settle, and
   * nothing waits on a page that may be stalled at the hub. A full device
   * wraps them into the keyring under the newest user key and deletes them
   * once the hub holds that ({@link migrateLegacyMachineKeys}); one that does
   * not manage the account — no user key, and no keyring writes — seals each
   * that is the key the chain committed into its host-key vault, and deletes
   * the raw copy once the sealed one opens to it
   * ({@link moveLegacyMachineKeysToVault}).
   */
  async migrate(): Promise<number> {
    const chain = this.state;
    if (!chain) return 0;
    const machines = chain.hosts().map((m) => m.info.id);
    const run = () =>
      this.isAdmin
        ? migrateLegacyMachineKeys(this.store, this.vault, this.keyring, this.account, machines, () => this.ensureUserKey())
        : moveLegacyMachineKeysToVault(this.store, this.hostKeys, machines, (machine, epoch, key) => chain.checkMachineKey(machine, epoch, key));
    const moved = this.locks ? await this.locks.request(LEGACY_MIGRATION_LOCK, { mode: "exclusive", ifAvailable: true }, (lock) => (lock === null ? 0 : run())) : await run();
    if (moved) this.emit();
    return moved;
  }

  /** The account key's fingerprint: what an enrolling machine pins. */
  async fingerprint(): Promise<Uint8Array> {
    if (!this.state) throw new AccountError("notInChain", "the account has no chain yet");
    return fingerprint(this.state.accountKey);
  }

  /** A fresh enrollment code, its token announced to the hub (the secret never is). */
  async startEnrollment(): Promise<EnrollCode> {
    return (await this.startTimedEnrollment()).code;
  }

  /**
   * {@link startEnrollment}, with when the hub says the code runs out
   * (`expires_ms`, by the hub's clock; `null` when its answer has none).
   */
  async startTimedEnrollment(): Promise<{ code: EnrollCode; expiresMs: number | null }> {
    const code = generateEnrollCode(await this.fingerprint());
    const expiresMs = await this.api.createEnrollment(this.session.token, this.deviceId, await enrollToken(code));
    return { code, expiresMs: typeof expiresMs === "number" && Number.isFinite(expiresMs) ? expiresMs : null };
  }

  /**
   * Wait for the agent's proof, check it against the code's secret, and add
   * the machine with exactly the keys the proof covers — with its first key
   * epoch committed in the same call, and that key's blob put into the
   * keyring. `handOverMachineKey` then gives it to the agent.
   *
   * On a chain that holds a v3 entry the computer joins as a v3 device that
   * runs AI (`[addDevice3{host}, keyEpochs]`, local-aggregator §4.3 step 7)
   * — only while the hub takes v3 entries (`opts.writable`: it gates a new
   * host) — else as v2's machine (`[addMachine, keyEpochs]`). It never
   * manages the account (review D1). Its first machine key then goes to the
   * devices that are not admins and that it accepts control from (purpose 5:
   * none, until an admin lets one), so they can read what it writes. While
   * the newest user key is stale, the batch carries a new one, and the first
   * machine key goes under that (`commitHostKey`).
   */
  async completeEnrollment(
    code: EnrollCode,
    signal?: AbortSignal,
    pollMs = 1000,
    /**
     * The keys the machine that asked for this code showed beforehand (the
     * local sign-in's request): a proof under any other keys is refused, so
     * a code that leaked to another machine adds nothing.
     */
    expect?: { signKey: Uint8Array; dhKey: Uint8Array },
    opts: {
      writable?: boolean;
      wait?: { forMs: number; everyMs: number };
      /**
       * Whether it is added as a full device: never, for a device that runs
       * AI (review D1) — `true` is refused here, before any code is used, as
       * the chain's verifier would refuse it.
       */
      admin?: boolean;
    } = {},
  ): Promise<MachineInfo> {
    if (opts.admin) throw hostAdminRefused();
    // What the hub would refuse is refused before anything is asked of it: a new host on a v3 chain only while it takes v3 entries.
    if (this.state?.isV3) this.requireWritable(opts.writable);
    const token = await enrollToken(code);
    for (;;) {
      if (signal?.aborted) throw signal.reason ?? new DOMException("aborted", "AbortError");
      const status = await this.api.enrollment(this.session.token, token);
      if (status.state === "expired") throw new AccountError("expired", "the enrollment code expired");
      if (status.state === "proved" && status.machine && status.signKey && status.dhKey && status.proof) {
        const ok = await verifyEnrollProof(code, this.account, status.machine, status.signKey, status.dhKey, status.proof);
        if (!ok) throw new AccountError("proof", "the device's enrollment proof does not verify: it was not added");
        if (expect && !(equal(status.signKey, expect.signKey) && equal(status.dhKey, expect.dhKey))) {
          throw new AccountError("proof", "the device that proved this code is not the one that asked for it: it was not added");
        }
        const machine: MachineInfo = { id: status.machine, signKey: status.signKey, dhKey: status.dhKey };
        const mk = random32();
        try {
          // One batch: the hub refuses a host that joins without its first machine key epoch.
          const { mkEpoch } = await this.commitHostKey(
            machine.id,
            mk,
            (chain) => [joinAction(machine, { host: true, admin: false }, chain)],
            (build) => this.appendWaitingFor(machine.id, build, opts.wait),
          );
          // The devices that hold no user key read what it writes with this.
          await this.mailHostKey(machine.id, mkEpoch, mk);
        } finally {
          mk.fill(0);
        }
        this.emit();
        return machine;
      }
      await new Promise((resolve) => setTimeout(resolve, pollMs));
    }
  }

  /**
   * Give machine `id` a new key epoch: generate `MK`, commit it in the chain,
   * put its keyring blob (wrapped under the newest `UK` epoch — a new one
   * carried in the same call while the newest is stale: `commitHostKey`),
   * then hand it to the agent — in the channel, or by mailbox
   * (`deliver.ts`). The bytes are zeroed after.
   */
  async issueMachineKey(
    id: string,
    courier: MachineKeyCourier,
    reencrypt = false,
    /** Entries the new epoch follows in the same call (a narrowed accept list: review D2), made for the chain each batch is built on. */
    prefix: Action[] | ((chain: ChainState) => Action[]) = [],
  ): Promise<{ mkEpoch: number; ukEpoch: number; result: DeliveryResult; hostKeysTo: string[]; userKey?: FreshUserKey }> {
    if (!this.state?.machine(id)) throw new AccountError("notInChain", "no device {{id}} that runs AI in the chain", undefined, undefined, { id });
    const mk = random32();
    try {
      const { mkEpoch, ukEpoch, userKey } = await this.commitHostKey(id, mk, (chain) => (typeof prefix === "function" ? prefix(chain) : prefix), (build) => this.appendBatch(build));
      // The devices that hold no user key and that it accepts read it with this (only those: review D2).
      const { sent } = await this.mailHostKey(id, mkEpoch, mk);
      this.emit();
      return { mkEpoch, ukEpoch, result: await this.handOver(id, mkEpoch, ukEpoch, mk, courier, reencrypt), hostKeysTo: sent, ...(userKey ? { userKey } : {}) };
    } finally {
      mk.fill(0);
    }
  }

  /**
   * Commit `mk` as host `id`'s next key epoch — after the entries `prefix`
   * makes (a join, hosting again, a narrowed accept list), in one call
   * (`append`) — and put its keyring blob, wrapped under the newest user key
   * this device holds.
   *
   * Unless that user key is stale ({@link userKeyStale}: a device that held
   * it no longer manages the account — a demotion of itself leaves it so
   * until another admin rotates): then the same `keyEpochs` entry carries a
   * new user key epoch, the new machine key is wrapped under that one, and
   * it is shared as a rotation shares it — kept, sealed for the recovery
   * words, mailed to the other admins (never to the device that left). The
   * re-review: these batches used to wrap a new host's key under the user
   * key the demoted or removed device still holds. (The other hosts' keys
   * are left to the rotation that is still due: `rotationDue`.)
   */
  private async commitHostKey(
    id: string,
    mk: Bytes,
    prefix: (chain: ChainState) => Action[],
    append: (build: (chain: ChainState) => Promise<Action[]>) => Promise<Entry[]>,
  ): Promise<{ mkEpoch: number; ukEpoch: number; userKey: FreshUserKey | null }> {
    const stale = this.state !== null && userKeyStale(this.state);
    // The newest user key, when the new key may go under it: known (or asked for) before anything is committed.
    const wrapEpoch = stale ? null : await this.ensureUserKey();
    if (!stale && wrapEpoch === null) throw this.noUserKey();
    const fresh: { uk: Bytes | null } = { uk: null };
    try {
      const entries = await append(async (chain) => {
        fresh.uk?.fill(0);
        fresh.uk = null;
        const epoch = chain.machineKeyEpochs(id);
        const machines = [{ machine: id, epoch, hash: await machineKeyCommit(this.account, id, epoch, mk) }];
        // Built on the chain as it stands (a newer head too): a new user key when its newest is stale, or is not the one checked above.
        let user: { epoch: number; hash: Bytes } | null = null;
        if (userKeyStale(chain) || wrapEpoch === null || chain.latestUserEpoch() !== wrapEpoch) {
          fresh.uk = random32();
          const ukEpoch = chain.userKeyEpochs();
          user = { epoch: ukEpoch, hash: await userKeyCommit(this.account, ukEpoch, fresh.uk) };
        }
        return [...prefix(chain), { type: "keyEpochs", user, machines }];
      });
      const committed = entries.flatMap((e) => (e.draft.action.type === "keyEpochs" ? [e.draft.action] : []));
      const freshEpoch = committed.find((a) => a.user)?.user?.epoch ?? null;
      const mkEpoch = committed.flatMap((a) => a.machines).find((m) => m.machine === id)?.epoch ?? this.state!.latestMachineEpoch(id)!;
      const newUk = freshEpoch !== null ? fresh.uk : null;
      // Committed: now the new user key may be kept and sealed for the recovery words.
      const recovery = newUk ? await this.keepUserKey(freshEpoch!, newUk) : false;
      const ukEpoch = freshEpoch ?? wrapEpoch!;
      let userKey: FreshUserKey | null = null;
      try {
        await this.keyring.put(keyName.machine(id, mkEpoch, ukEpoch), await wrapMachineKey(await this.vault.key(ukEpoch), this.account, id, mkEpoch, ukEpoch, mk));
      } finally {
        // After the blob (a device that reads the keyring as the mail lands finds it), and whether or not it went in.
        if (newUk) userKey = { epoch: freshEpoch!, recovery, ...(await this.mailUserKey(freshEpoch!, newUk)) };
      }
      return { mkEpoch, ukEpoch, userKey };
    } finally {
      fresh.uk?.fill(0);
    }
  }

  /** A new user key epoch, committed: kept here, with this device's recovery package for every epoch it holds. Whether the new one went into the recovery package. */
  private async keepUserKey(epoch: number, uk: Bytes): Promise<boolean> {
    await this.vault.put(epoch, uk);
    await this.ensureRecoveryPackages();
    const rsk = this.state?.currentRecoveryKey ?? null;
    return rsk !== null && this.keyring.hasBase(keyName.recovery(epoch, this.deviceId, rsk), this.deviceId);
  }

  /** A new user key epoch to every other current admin (purpose 1; never to a device that does not manage the account). */
  private async mailUserKey(epoch: number, uk: Bytes): Promise<{ sealedTo: string[]; failedTo: string[]; failedWhy: Record<string, string> }> {
    const sealedTo: string[] = [];
    const failedTo: string[] = [];
    const failedWhy: Record<string, string> = {};
    for (const d of this.state?.admins() ?? []) {
      if (d.info.id === this.deviceId) continue;
      try {
        await this.mailUserKeys(d.info.id, d.info.dhKey, [{ epoch, key: uk }]);
        sealedTo.push(d.info.id);
      } catch (error) {
        console.warn(`cloud account: could not mail the new user key to ${d.info.id}:`, error);
        failedTo.push(d.info.id);
        failedWhy[d.info.id] = error instanceof HubApiError ? error.code : error instanceof Error ? error.message : String(error);
      }
    }
    return { sealedTo, failedTo, failedWhy };
  }

  /**
   * Hand machine `id` its newest committed key epoch, read back from the
   * keyring (checked against the chain) — or, when no blob this device can
   * open holds it, a new epoch (`issueMachineKey`).
   */
  async handOverMachineKey(id: string, courier: MachineKeyCourier): Promise<{ mkEpoch: number; result: DeliveryResult }> {
    this.assertNotForked();
    const latest = this.state?.latestMachineEpoch(id) ?? null;
    const found = latest === null ? null : await this.newestMachineKeyBlob(id, latest);
    if (latest === null || !found) {
      const issued = await this.issueMachineKey(id, courier);
      return { mkEpoch: issued.mkEpoch, result: issued.result };
    }
    try {
      this.assertNotForked();
      return { mkEpoch: latest, result: await deliverMachineKey(this.guarded(courier), id, latest, found, false) };
    } finally {
      found.fill(0);
    }
  }

  /**
   * `courier`, sending nothing once this page's view forked: checked as each
   * send begins (in the channel, or sealed into the mailbox), however long
   * it waited for the machine to come online first.
   */
  private guarded(courier: MachineKeyCourier): MachineKeyCourier {
    return {
      waitOnline: (machine, timeoutMs) => courier.waitOnline(machine, timeoutMs),
      postInChannel: (machine, body) => {
        this.assertNotForked();
        return courier.postInChannel(machine, body);
      },
      mail: (machine, epoch, mk) => {
        this.assertNotForked();
        return courier.mail(machine, epoch, mk);
      },
    };
  }

  /**
   * {@link guarded} for a renewal (host identity rotation §3.2): its
   * `/rh/rotate` and its word on the head go out only from a page whose view
   * has not forked — the fork latch holds them as it holds every key.
   */
  private guardedRenewal(courier: RenewalCourier): RenewalCourier {
    return {
      ...this.guarded(courier),
      rotate: (machine, body) => {
        this.assertNotForked();
        return courier.rotate(machine, body);
      },
      attest: () => {
        this.assertNotForked();
        return courier.attest();
      },
    };
  }

  private async newestMachineKeyBlob(id: string, mkEpoch: number): Promise<Bytes | null> {
    try {
      await this.keyring.refresh();
    } catch {
      // the cached copy
    }
    return this.machineKeyBytes(id, mkEpoch);
  }

  /** Hand `mk` to the agent (`deliver.ts`); a re-encryption that could only go by mailbox is noted, pinned to this very key, to ask again. */
  private async handOver(id: string, mkEpoch: number, ukEpoch: number, mk: Uint8Array, courier: MachineKeyCourier, reencrypt: boolean): Promise<DeliveryResult> {
    this.assertNotForked();
    // An isolated host is handed no key at all (§3.8): its identity may be someone else's now. It gets one with its next renewal.
    if (this.isolated(id)) return { via: "refused", status: 0, message: "isolated: renew its identity key on the device first", code: "isolated" };
    const result = await deliverMachineKey(this.guarded(courier), id, mkEpoch, mk, reencrypt);
    if (result.via === "mailbox" && reencrypt) {
      // The mailbox carries no re-encryption request: ask again in the next channel.
      notePendingReencrypt(this.account, { machine: id, mkEpoch, ukEpoch, mkHash: hex(await machineKeyCommit(this.account, id, mkEpoch, mk)) });
      result.pending = true;
    }
    return result;
  }

  /** The chain's check of machine `machine`'s key epochs. */
  private machineCheck(machine: string): (mkEpoch: number, key: Bytes) => Promise<void> {
    const chain = this.state;
    return async (mkEpoch, key) => {
      if (!chain) throw new AccountError("notInChain", "the account has no chain yet");
      await chain.checkMachineKey(machine, mkEpoch, key);
    };
  }

  /**
   * `MK` epoch `mkEpoch` of `machine` as bytes, from a keyring blob under a
   * `UK` epoch held (only `ukEpoch`, if given) that unwraps to the key the
   * chain committed — only to hand it to the agent again in the channel; the
   * caller zeroes it. `null` when none does.
   */
  async machineKeyBytes(machine: string, mkEpoch: number, ukEpoch?: number): Promise<Bytes | null> {
    if (!this.state || !this.pinned) return null;
    await this.keyring.load();
    return machineKeyBytes(this.vault, this.keyring, this.account, machine, mkEpoch, this.machineCheck(machine), ukEpoch);
  }

  /** The key a noted re-encryption is pinned to (its `MK` epoch, under its `UK` epoch, checked against the chain). */
  pinnedMachineKey(item: PendingReencrypt): Promise<Bytes | null> {
    return this.machineKeyBytes(item.machine, item.mkEpoch, item.ukEpoch);
  }

  /**
   * Every epoch of machine `id`'s key this browser can open — only those the
   * chain committed, each from a keyring blob (under a `UK` epoch it holds)
   * that unwraps to the committed key: what its streams are read with.
   * (`store` and `maxEpochs` are kept for callers.)
   */
  async machineKeys(id: string, _store?: KeyStore, _maxEpochs = 64): Promise<Map<number, MachineKey>> {
    const chain = this.state;
    if (!chain || !this.pinned) return new Map();
    await this.keyring.load();
    const committed = chain.machineKeyEpochs(id);
    const keys = (await this.vault.epochs()).length ? await openMachineKeys(this.vault, this.keyring, this.account, id, committed, this.machineCheck(id)) : new Map<number, MachineKey>();
    // Chain v3: the epochs an admin handed this device one by one (each checked against the chain as it was taken).
    for (const epoch of await this.hostKeys.epochs(id)) {
      if (epoch >= committed || keys.has(epoch)) continue;
      try {
        keys.set(epoch, await this.hostKeys.key(id, epoch));
      } catch (error) {
        console.warn(`cloud account: ${id}'s key epoch ${epoch} does not open here:`, error);
      }
    }
    return keys;
  }

  /**
   * Take the `UK` epochs mailed to this device (purpose 1): each opened only
   * if addressed to it, for this account, from a current full device of the
   * verified chain, and exactly the key the chain committed for its epoch —
   * then kept encrypted at rest. Mail that fails those checks is dropped. Only
   * on a device that pinned the account. Returns how many epochs were new.
   */
  async receiveMail(): Promise<number> {
    if (this.standing !== "active" || !this.state || !this.pinned) return 0;
    const mail = await this.api.mailbox(this.session.token);
    if (!mail.length) return 0;
    const hostKeyAhead = (m: { sender: string; epoch: number; sealed: Uint8Array }) => {
      try {
        return !this.state!.admin(m.sender) || this.state!.machineKeyEpochs(decodeHostKeyMail(m.sealed).host) <= m.epoch;
      } catch {
        return false;
      }
    };
    if (
      mail.some(
        (m) =>
          (m.purpose === MAIL_USER_KEY && (!this.state!.admin(m.sender) || this.state!.userKeyEpochs() <= m.epoch)) ||
          (m.purpose === MAIL_HOST_KEY && hostKeyAhead(m)) ||
          (m.purpose === MAIL_KEY_REQUEST && !this.state!.device(m.sender)),
      )
    ) {
      // A sender, or an epoch, this view does not know yet may be one it is behind on.
      await this.sync();
    }
    let fresh = 0;
    const asking = new Map<string, Set<number>>();
    /** Devices that are not admins asking for the hosts' machine keys (chain v3): answered with those, never a user key. */
    const askingHostKeys = new Set<string>();
    for (const m of mail) {
      if (m.purpose === MAIL_KEY_REQUEST) {
        const peer = this.state.device(m.sender);
        if (m.sender !== this.deviceId && peer?.admin) {
          // Another current full device lacks a user key epoch this one holds:
          // it gets every epoch held (to its chain key), once per batch.
          if (await this.vault.has(m.epoch)) asking.set(m.sender, (asking.get(m.sender) ?? new Set<number>()).add(m.epoch));
        } else if (m.sender !== this.deviceId && peer && this.isAdmin) {
          // A device that does not manage the account (§3.3) — a host's
          // device role too (S1-4: the local install): only the hosts'
          // machine keys, once per batch.
          askingHostKeys.add(m.sender);
        }
        await this.api.ackMail(this.session.token, m.id);
      } else if (m.purpose === MAIL_HOST_KEY) {
        // A host's machine key epoch an admin sealed for this device: from a
        // current admin, for this device, exactly the key the chain committed
        // for that host and epoch (`openHostKey`) — else dropped.
        try {
          const { host, sealed } = decodeHostKeyMail(m.sealed);
          const raw = await openHostKey(this.noiseKeys, this.deviceId, this.state, { account: this.account, sender: m.sender, recipient: this.deviceId, host, epoch: m.epoch }, sealed);
          try {
            if (await this.hostKeys.put(host, m.epoch, raw)) fresh++;
          } finally {
            raw.fill(0);
          }
        } catch (error) {
          if (!(error instanceof WrapError) && !(error instanceof DecodeError)) throw error;
          console.warn(`cloud account: dropped a host key mailed by ${m.sender}: ${error.message}`);
        }
        await this.api.ackMail(this.session.token, m.id);
      } else if (m.purpose === MAIL_USER_KEY) {
        const ctx = { purpose: "userKey" as const, account: this.account, sender: m.sender, recipient: this.deviceId, epoch: m.epoch };
        try {
          const raw = await openUserKey(this.noiseKeys, this.deviceId, this.state, ctx, m.sealed);
          try {
            if (await this.vault.put(m.epoch, raw)) fresh++;
          } finally {
            raw.fill(0);
          }
        } catch (error) {
          if (!(error instanceof WrapError)) throw error;
          console.warn(`cloud account: dropped a key mailed by ${m.sender}: ${error.message}`);
        }
        await this.api.ackMail(this.session.token, m.id);
      } else if (m.purpose === MAIL_PAIRING) {
        // A pairing delivery is for a device pairing (again): stale here.
        if (!this.pairing) await this.api.ackMail(this.session.token, m.id);
      } else {
        // A purpose this device does not handle (a machine key, or a newer
        // kind — `knownMailPurpose` is false): it would sit in the mailbox
        // for good, filling it, so it is acknowledged unread (M2 re-review;
        // relay-and-plans §6).
        await this.api.ackMail(this.session.token, m.id);
      }
    }
    for (const [id, wanted] of asking) {
      // A user key goes to a current admin only (chain v3 §3.3: never to a host or a plain device).
      const peer = this.state.admin(id);
      if (!peer) continue;
      try {
        // The epochs it asked for first, then the rest newest first: past the
        // hub's mail rate, what it needs still arrives (the review of the M2
        // re-review fixes, N5: oldest first, the asked-for epoch could be the
        // one the rate cut off).
        await this.vault.withAllRaw((keys) => this.mailUserKeys(id, peer.info.dhKey, requestedFirst(keys, wanted)));
      } catch (error) {
        console.warn(`cloud account: could not send ${id} the user keys it asked for:`, error);
      }
    }
    for (const id of askingHostKeys) {
      try {
        await this.answerHostKeyRequest(id);
      } catch (error) {
        console.warn(`cloud account: could not send ${id} the machine keys it asked for:`, error);
      }
    }
    if (fresh) {
      try {
        await this.keyring.refresh();
      } catch {
        // later
      }
      this.emit();
    }
    return fresh;
  }

  /** Seal `UK` epochs from this device to device `id` and mail them (purpose 1). */
  private async mailUserKeys(id: string, dhKey: Uint8Array, keys: { epoch: number; key: Uint8Array }[]): Promise<void> {
    this.assertNotForked();
    for (const { epoch, key } of keys) {
      this.assertNotForked();
      const sealed = await sealTo(this.noiseKeys, dhKey, { purpose: "userKey", account: this.account, sender: this.deviceId, recipient: id, epoch }, key);
      this.assertNotForked();
      await this.api.mail(this.session.token, { recipient: id, sender: this.deviceId, purpose: MAIL_USER_KEY, epoch, sealed });
    }
  }

  // ------------------------------------------------------------ host keys (chain v3)

  /**
   * The devices that are handed host `host`'s machine keys one by one (review
   * D2): every current device that does not manage the account (it holds no
   * `UK`) and that `host` accepts control from (`mayControl`) — another host
   * among them (S1-4: a local install controls with its one identity; its
   * device role takes them) — and not this device. None by default: a host
   * accepts admins only until an admin lets others in. Empty on a v2 chain
   * (no accept policy there). Never a host waiting to renew its identity
   * key, nor an isolated one ({@link keysWithheldFrom}).
   */
  hostKeyRecipients(host: string, chain: ChainState | null = this.state): DeviceRecord[] {
    if (!chain) return [];
    return chain
      .allDevices()
      .filter((d) => d.status.type === "active" && !d.admin && d.info.id !== this.deviceId && admits(chain, host, d.info.id) && !this.keysWithheldFrom(d.info.id));
  }

  /** Seal host `host`'s key epoch `epoch` from this (admin) device for device `to` and mail it (purpose 5). */
  private async mailHostKeyTo(to: DeviceRecord, host: string, epoch: number, key: Uint8Array): Promise<void> {
    this.assertNotForked();
    const ctx = { account: this.account, sender: this.deviceId, recipient: to.info.id, host, epoch };
    const sealed = await sealHostKey(this.noiseKeys, to.info.dhKey, ctx, key);
    this.assertNotForked();
    await this.api.mail(this.session.token, { recipient: to.info.id, sender: this.deviceId, purpose: MAIL_HOST_KEY, epoch, sealed: encodeHostKeyMail(host, sealed) });
  }

  /**
   * A new key epoch of host `host` (just committed: an enrollment, hosting
   * again, a rotation, a narrowed accept list) to every device that holds no
   * user key and that the host accepts control from ({@link
   * hostKeyRecipients}). Each failure is said and left: the device asks for
   * it again (purpose 4). Returns who got it, who did not, and the hosts
   * whose device role has not signed in at the hub yet (`waiting`: an agent
   * from before S1-4 never does; one that does asks for what it lacks) —
   * not failures.
   */
  async mailHostKey(
    host: string,
    epoch: number,
    key: Uint8Array,
    /** Hosts already found with no device role in this call's batch (a rotation): not tried again. */
    noRole: Set<string> = new Set(),
  ): Promise<{ sent: string[]; failed: Record<string, string>; waiting: string[] }> {
    const sent: string[] = [];
    const failed: Record<string, string> = {};
    const waiting: string[] = [];
    if (!this.isAdmin) return { sent, failed, waiting };
    for (const d of this.hostKeyRecipients(host)) {
      if (noRole.has(d.info.id)) {
        waiting.push(d.info.id);
        continue;
      }
      try {
        await this.mailHostKeyTo(d, host, epoch, key);
        sent.push(d.info.id);
        this.noteHostKeysSent(d.info.id, [{ host, epoch }], false);
      } catch (error) {
        if (noDeviceRoleYet(d, error)) {
          noRole.add(d.info.id);
          waiting.push(d.info.id);
          continue;
        }
        failed[d.info.id] = error instanceof HubApiError ? error.code : error instanceof Error ? error.message : String(error);
        console.warn(`cloud account: could not mail ${host}'s new key to ${d.info.id}:`, error);
      }
    }
    return { sent, failed, waiting };
  }

  /**
   * Every committed key epoch of every current host that accepts control
   * from device `id` (review D2: no other host's) this device can open,
   * sealed for `id` — one that does not manage the account (it never gets a
   * user key: §3.3), a host's device role included (S1-4) — newest epochs
   * first, at most {@link MAX_HOST_KEY_MAILS}: when it joins, when an admin
   * re-sends its keys, when a host starts accepting it (`only`: that host),
   * and when it asks (purpose 4). Returns how many went. A host whose device
   * role has not signed in at the hub yet is refused there (400: see
   * {@link noDeviceRoleYet}); that error is thrown.
   */
  async sendHostKeys(id: string, only?: string): Promise<number> {
    const sealed = await this.sealHostKeys(id, { only });
    this.noteHostKeysSent(id, sealed, only === undefined);
    return sealed.length;
  }

  /**
   * Seal device `id` the committed epochs of the hosts that accept it
   * ({@link sendHostKeys}) — of `only` that host, or newer than `after`
   * says per host. The epochs sealed.
   */
  private async sealHostKeys(id: string, opts: { only?: string; after?: Record<string, number> } = {}): Promise<{ host: string; epoch: number }[]> {
    const chain = this.requireAdmin();
    const to = chain.device(id);
    if (!to || id === this.deviceId) throw new AccountError("notInChain", `${id} is not a current device of the account`);
    if (to.admin) throw new AccountError("notAdmin", `${id} manages the account: it reads the hosts' keys with the user key`);
    // A host waiting to renew its identity key, or isolated, is sealed nothing (§4.3): whoever holds its old key would read them.
    if (this.keysWithheldFrom(id)) return [];
    const wanted = chain
      .hosts()
      .filter((h) => h.status.type === "active" && (opts.only === undefined || h.info.id === opts.only) && admits(chain, h.info.id, id))
      .flatMap((h) => Array.from({ length: chain.machineKeyEpochs(h.info.id) }, (_, epoch) => ({ host: h.info.id, epoch })))
      .filter(({ host, epoch }) => opts.after?.[host] === undefined || epoch > opts.after[host]!)
      .sort((a, b) => b.epoch - a.epoch || (a.host < b.host ? -1 : a.host > b.host ? 1 : 0));
    const sealed: { host: string; epoch: number }[] = [];
    if (!wanted.length) return sealed;
    try {
      await this.keyring.refresh();
    } catch {
      // the cached copy
    }
    for (const { host, epoch } of wanted) {
      if (sealed.length >= MAX_HOST_KEY_MAILS) break;
      const key = await this.machineKeyBytes(host, epoch);
      if (!key) continue;
      try {
        await this.mailHostKeyTo(to, host, epoch, key);
        sealed.push({ host, epoch });
      } finally {
        key.fill(0);
      }
    }
    return sealed;
  }

  /** What this admin sealed device `to` lately ({@link hostKeysSentKey}). */
  private hostKeysSent(to: string): HostKeysSent | null {
    try {
      const v = JSON.parse(this.read(hostKeysSentKey(this.account, this.deviceId, to)) ?? "null") as HostKeysSent | null;
      return v && typeof v.atMs === "number" && v.through && typeof v.through === "object" ? v : null;
    } catch {
      return null;
    }
  }

  /** Note host keys sealed to `to` (`full`: every key it may hold — its throttle's window starts again). */
  private noteHostKeysSent(to: string, sealed: { host: string; epoch: number }[], full: boolean): void {
    const last = this.hostKeysSent(to);
    const through: Record<string, number> = { ...(last?.through ?? {}) };
    for (const { host, epoch } of sealed) through[host] = Math.max(through[host] ?? -1, epoch);
    const atMs = full ? Date.now() : (last?.atMs ?? 0);
    this.write(hostKeysSentKey(this.account, this.deviceId, to), JSON.stringify({ atMs, through } satisfies HostKeysSent));
  }

  /**
   * Answer device `id`'s request for the hosts' keys (purpose 4; security
   * review of S1-4, S4/N7): every key it may hold at most once per {@link
   * KEY_REQUEST_EVERY_MS} — counted from the last time it was sealed them all,
   * by this answer, its registration (`handHostKeys`) or a re-send — and in
   * between only the epochs newer than what was sealed to it since (a key
   * that turned over and reached it already is not sealed twice). Returns
   * how many went.
   */
  async answerHostKeyRequest(id: string): Promise<number> {
    const last = this.hostKeysSent(id);
    // A window that starts in the future (this browser's clock went back, or the record is spoiled) is no window (re-review 1).
    const since = last === null ? -1 : Date.now() - last.atMs;
    const within = last !== null && since >= 0 && since < KEY_REQUEST_EVERY_MS;
    const sealed = await this.sealHostKeys(id, within ? { after: last.through } : {});
    this.noteHostKeysSent(id, sealed, !within);
    return sealed.length;
  }

  /**
   * A new host (just registered: the local sign-in's popup, an enrollment
   * code) is sealed the keys of the other hosts that accept it (S1-4,
   * local-aggregator §4.3 step 7) — once its device role signed in at the
   * hub, which it does within a second of seeing itself in the chain. Tried
   * every `wait.everyMs` for `wait.forMs`; then left to the host, which asks
   * for what it lacks (purpose 4). Returns how many keys went, or `null`
   * when its device role never showed up in time (an agent from before
   * S1-4, or one with the aggregator off). Nothing is sent to a host no
   * other host accepts.
   */
  async handHostKeys(id: string, wait = HOST_KEYS_WAIT, signal?: AbortSignal): Promise<number | null> {
    const deadline = Date.now() + wait.forMs;
    for (;;) {
      const chain = this.state;
      const to = chain?.device(id);
      if (!chain || !to || to.admin || !chain.hosts().some((h) => h.status.type === "active" && admits(chain, h.info.id, id))) return 0;
      try {
        return await this.sendHostKeys(id);
      } catch (error) {
        if (!noDeviceRoleYet(to, error)) throw error;
      }
      if (signal?.aborted || Date.now() + wait.everyMs > deadline) return null;
      await new Promise((resolve) => setTimeout(resolve, wait.everyMs));
    }
  }

  /**
   * The newest committed key epoch of every current host that accepts this
   * device's control and that it cannot open: what it lacks to read them now
   * (a host that does not accept it is none of its business: review D2).
   */
  async hostKeyGaps(): Promise<{ host: string; epoch: number }[]> {
    const chain = this.state;
    if (!chain || !this.pinned) return [];
    const gaps: { host: string; epoch: number }[] = [];
    for (const h of chain.hosts()) {
      if (h.status.type !== "active" || h.info.id === this.deviceId || !admits(chain, h.info.id, this.deviceId)) continue;
      const latest = chain.latestMachineEpoch(h.info.id);
      if (latest === null) continue;
      if ((await this.machineKeys(h.info.id)).has(latest)) continue;
      gaps.push({ host: h.info.id, epoch: latest });
    }
    return gaps;
  }

  /**
   * (A device that does not manage the account.) Whether it can open every
   * current host's newest key epoch; if not, look in the mailbox again and
   * ask the admins (`requestHostKeys`: at most once per
   * {@link KEY_REQUEST_EVERY_MS}). Keeps `missingHostKeys` for the page and
   * returns it.
   */
  async checkHostKeys(): Promise<string[]> {
    let gaps = this.isAdmin ? [] : await this.hostKeyGaps();
    if (gaps.length && this.standing === "active" && this.pinned) {
      try {
        await this.receiveMail();
      } catch (error) {
        console.warn("cloud account: mail:", error);
      }
      gaps = await this.hostKeyGaps();
      if (gaps.length) {
        try {
          await this.requestHostKeys();
        } catch (error) {
          console.warn("cloud account: asking for the hosts' keys:", error);
        }
      }
    }
    const missing = gaps.map((g) => g.host);
    if (missing.join(",") !== this.missingHostKeys.join(",")) {
      this.missingHostKeys = missing;
      this.emit();
    }
    return missing;
  }

  /**
   * (A device that does not manage the account.) Ask the account's admins for
   * the hosts' machine keys (purpose 4 — the hub takes it from such a device
   * only to an admin; no secret, epoch 0): they answer with host keys, never
   * a user key. At most once per {@link KEY_REQUEST_EVERY_MS} unless `force`.
   * Returns who was asked.
   */
  async requestHostKeys(force = false): Promise<string[]> {
    if (!this.state || this.standing !== "active" || !this.pinned || this.isAdmin) return [];
    const key = hostKeyRequestKey(this.account, this.deviceId);
    try {
      const last = JSON.parse(this.read(key) ?? "null") as { atMs?: unknown } | null;
      if (!force && typeof last?.atMs === "number" && Date.now() - last.atMs < KEY_REQUEST_EVERY_MS) return [];
    } catch {
      // asked never, or the record is spoiled: ask
    }
    const asked: string[] = [];
    for (const d of this.state.admins()) {
      if (d.info.id === this.deviceId) continue;
      try {
        await this.api.mail(this.session.token, { recipient: d.info.id, sender: this.deviceId, purpose: MAIL_KEY_REQUEST, epoch: 0, sealed: new Uint8Array(0) });
        asked.push(d.info.id);
      } catch (error) {
        console.warn(`cloud account: could not ask ${d.info.id} for the hosts' keys:`, error);
      }
    }
    this.write(key, JSON.stringify({ atMs: Date.now() }));
    return asked;
  }

  /**
   * Send device `id` its keys again: every user key epoch to an admin
   * (`resendUserKeys`), the hosts' machine keys to a device that is not one
   * (`sendHostKeys`) — a host's device role included (S1-4).
   */
  async resendKeys(id: string): Promise<number> {
    const record = this.state?.device(id);
    if (!record) throw new AccountError("notInChain", `${id} is not a current device of the account`);
    if (record.admin) return (await this.resendUserKeys(id)).length;
    return this.sendHostKeys(id);
  }

  // ------------------------------------------------------------ chain v3 changes

  /**
   * Add a paired device (`pairing.ts`'s candidate, its fingerprint compared):
   * as an admin (it gets every user key epoch in the pairing delivery) or not
   * (the delivery carries no user key — it pins the account all the same —
   * and the machine keys of the hosts that accept it follow as purpose 5). A
   * device that does not manage the account exists only in chain v3: the
   * account must be upgraded first (review D4). A device waiting after it
   * joined with a passkey is approved this way, and only this way: the keys
   * the pairing checked must be the ones it joined with
   * ({@link approvePasskeyJoin}).
   */
  async pairDevice(candidate: Candidate, opts: { admin: boolean }): Promise<"delivered" | "redelivered"> {
    // The hub's chain as it stands first: a self-activation this page has not
    // seen yet is held back here (and so refuses the pairing) before any key goes.
    await this.sync();
    this.refuseWhileHeld();
    const chain = this.requireAdmin();
    const listed = chain.deviceRecord(candidate.device);
    if (!opts.admin && !listed) this.requireUpgraded(chain);
    // A device that joined with a passkey (waiting, or replaced meanwhile):
    // approved here, by this very pairing — then its keys go in the delivery.
    const joining = listed?.viaPasskey != null && listed.status.type !== "active";
    if (joining) await this.approvePasskeyJoin(candidate);
    const done = await completePairing(
      {
        // The delivery (every user key epoch, sealed) leaves only while this view has not forked, nor holds an activation back.
        api: {
          mail: (token, mail) => {
            this.assertNotForked();
            this.refuseWhileHeld();
            return this.api.mail(token, mail);
          },
        },
        token: this.session.token,
        device: this.deviceId,
        signPublic: this.keys.signPublic,
        noise: this.noiseKeys,
        chain: () => this.state!,
        append: (action) => this.append(action),
        withUserKeys: (use) => this.withUserKeys(use),
      },
      candidate,
      opts,
    );
    const record = this.state?.device(candidate.device);
    if (record && !record.admin && !record.host) {
      try {
        await this.sendHostKeys(record.info.id);
      } catch (error) {
        console.warn(`cloud account: could not send ${record.info.id} the hosts' keys (it asks again):`, error);
      }
    }
    return joining ? "delivered" : done;
  }

  /**
   * While this device holds back a self-activation ({@link heldBack}), it
   * hands no device its keys by pairing — not even one already listed, which
   * needs no new entry, and so would not meet the refusal every entry but
   * the veto meets (`appendBatch`): that device, opening the account afresh,
   * may take the held activation as history. It waits, as they do, until the
   * person approves or removes the held device.
   */
  private refuseWhileHeld(): void {
    const held = this.held;
    if (!held) return;
    throw new AccountError(
      "heldBack",
      "{{id}} activated itself sooner than this device allows: approve it or remove it first. Until then this device adds no device and pairs none again.",
      held.readyMs,
      undefined,
      { id: held.device ?? "?" },
    );
  }

  /**
   * Make device `id` a full device, or a limited one (`setAdmin`, chain v3
   * only; 「我的设备」 offers only the first: {@link limitedDevice}).
   * Never on for this device itself, nor for a device that runs AI (review
   * D1) or ever did ({@link hasRunAi}: D1 extended); never off for the last
   * admin. A device given it is mailed every
   * user key epoch held; one that loses it is handed the machine keys of the
   * hosts that accept it, which it reads them with from now on.
   *
   * It held the user key: taking the account from it retires that key
   * (review 1) — with `opts.courier`, in the same call as a removal's
   * rotation (`[setAdmin false, keyEpochs…]`: a new user key, a new machine
   * key per host, sealed only to who may have them), returned as its report.
   * Without one (or for this device itself, which may sign nothing after
   * it), the rotation is left due (`rotationDue`) for an admin to run.
   */
  async setAdmin(id: string, admin: boolean, opts: { courier?: MachineKeyCourier; reencrypt?: boolean } = {}): Promise<RotationReport | null> {
    const { report, keysFailed } = await this.writeAdmin(id, admin, opts);
    if (keysFailed !== null) console.warn(`cloud account: could not send ${id} its keys (re-send them from the devices page):`, keysFailed);
    return report;
  }

  /**
   * 「我的设备」's "make it a full device": `setAdmin(id, true)`, and
   * whether its user keys reached the hub. The entry is written either way;
   * the device need not be online, its keys wait in its mailbox — unless
   * mailing them failed (its mailbox at the hub full, the hub holding back
   * this device's mail…): then `keysFailed` is why, and "re-send keys" on
   * its row sends them again.
   */
  async promote(id: string): Promise<{ keysFailed: unknown }> {
    const { keysFailed } = await this.writeAdmin(id, true, {});
    return { keysFailed };
  }

  /** {@link setAdmin}'s work, with what mailing the device its keys came to (`null`: sent, or nothing to send). */
  private async writeAdmin(id: string, admin: boolean, opts: { courier?: MachineKeyCourier; reencrypt?: boolean }): Promise<{ report: RotationReport | null; keysFailed: unknown }> {
    const chain = this.requireAdmin();
    this.requireUpgraded(chain);
    const record = chain.device(id);
    if (!record) throw new AccountError("notInChain", `${id} is not a current device of the account`);
    if (record.admin === admin) return { report: null, keysFailed: null };
    if (admin && id === this.deviceId) throw new AccountError("self", "a device cannot make itself a full device: do it from another full device");
    if (admin && record.host) throw hostAdminRefused();
    // D1 extended: nor one that ran AI and stopped — refused here, before anything is signed.
    if (admin && hasRunAi(chain, record)) throw ranAiAdminRefused(id);
    if (!admin && chain.admins().length <= 1) throw new AccountError("lastAdmin", "the account's last full device stays one");
    const entry: Action = { type: "setAdmin", id, admin };
    let report: RotationReport | null = null;
    if (!admin && opts.courier && id !== this.deviceId) report = await this.rotate(opts.courier, opts.reencrypt ?? false, [entry]);
    else await this.append(entry);
    const now = this.state?.device(id);
    if (!now || now.host || id === this.deviceId) return { report, keysFailed: null };
    try {
      if (admin) await this.withUserKeys((keys) => this.mailUserKeys(id, now.info.dhKey, keys));
      else await this.sendHostKeys(id);
    } catch (error) {
      return { report, keysFailed: error };
    }
    return { report, keysFailed: null };
  }

  /**
   * Stop or restart running AI on device `id` (`setHost`, chain v3 only).
   * Stopping takes it out of the rotations and the hosts' list; its stream
   * is written no more. Restarting must commit a new machine key epoch in
   * the same batch (§3.2 rule 6): made here, put into the keyring, mailed to
   * the devices it accepts control from that hold no user key, and handed to
   * its agent — and only while the hub takes v3 entries (`opts.writable`: it
   * gates a host made one again). A device that manages the account never
   * runs AI (review D1).
   */
  async setHost(id: string, host: boolean, courier: MachineKeyCourier, opts: { writable?: boolean } = {}): Promise<{ mkEpoch: number; result: DeliveryResult; userKey?: FreshUserKey } | null> {
    const chain = this.requireAdmin();
    this.requireUpgraded(chain);
    const record = chain.device(id);
    if (!record) throw new AccountError("notInChain", `${id} is not a current device of the account`);
    if (record.host === host) return null;
    if (!host) {
      await this.append({ type: "setHost", id, host: false });
      return null;
    }
    if (id === this.deviceId) throw new AccountError("self", "a device cannot make itself run AI");
    if (record.admin) throw hostAdminRefused();
    this.requireWritable(opts.writable);
    // Only a computer that ran AI before (an enrolled machine) runs it again.
    if (chain.machineKeyEpochs(id) === 0) throw new AccountError("notInChain", "{{id}} never ran AI: add it as a device that runs AI instead", undefined, undefined, { id });
    const mk = random32();
    try {
      // [setHost, keyEpochs] in one call — a new user key in it too while the newest is stale (`commitHostKey`).
      const { mkEpoch, ukEpoch, userKey } = await this.commitHostKey(
        id,
        mk,
        () => [{ type: "setHost", id, host: true }],
        (build) => this.appendWaitingFor(id, build),
      );
      await this.mailHostKey(id, mkEpoch, mk);
      this.emit();
      return { mkEpoch, result: await this.handOver(id, mkEpoch, ukEpoch, mk, courier, false), ...(userKey ? { userKey } : {}) };
    } finally {
      mk.fill(0);
    }
  }

  /**
   * Whom host `host` accepts control from (`setAccept`, chain v3 only; "谁可以
   * 控制这台电脑"): admin devices only (the default), every device, or admin
   * devices and the ones named. Control is full access to that computer,
   * its identity key included (review D3).
   *
   * A list that lets in a device that was not let in before hands it the
   * host's committed machine keys (purpose 5). One that shuts out a device
   * that was let in (a narrowing, review D2) is written with the host's next
   * machine key epoch in the same call (`[setAccept, keyEpochs]`, the
   * chain's `hostKeyNext` rule): the new key is put into the keyring, sealed
   * to the devices still let in and handed to the agent (`courier`) — and the
   * host renews its identity key (`hostsToRotate`; an older agent: it should
   * be re-enrolled, `reEnrollCulprits`). A host that must renew its own first
   * is never let in ({@link mustRenewFirst}).
   */
  async setAccept(host: string, from: Accept, courier: MachineKeyCourier = NO_COURIER): Promise<AcceptChange> {
    const chain = this.requireAdmin();
    this.requireUpgraded(chain);
    if (!chain.machine(host)) throw new AccountError("notInChain", "{{host}} is not a device of the account that runs AI", undefined, undefined, { host });
    let accept: Accept = from;
    if (from.type === "devices") {
      // Ascending by bytes, no repeats, never the host itself; none named is the default.
      const ids = [...new Set(from.ids)].filter((id) => id !== host).sort((a, b) => compareBytes(utf8(a), utf8(b)));
      accept = ids.length ? { type: "devices", ids } : { type: "admins" };
    }
    const now = chain.acceptPolicy(host);
    if (now.type === accept.type && (accept.type !== "devices" || (now.type === "devices" && now.ids.join("\n") === accept.ids.join("\n")))) {
      return { narrowed: false, admitted: [], shutOut: [] };
    }
    const { admitted, shutOut } = acceptChange(chain, host, accept);
    // A host that may have had its identity key copied, or is isolated, is let control no other one until it renewed it (§4.1, §7
    // decision 8). One that cannot renew (an older agent, the hub not taking renewals yet) stays under the re-enroll warning instead.
    // Checked here, and again on the chain every (re)built batch goes on.
    this.renewalNotes.refuseExposedAdmission(chain, host, accept);
    const setAccept: Action = { type: "setAccept", host, from: accept };
    const checked = (c: ChainState): Action[] => {
      this.renewalNotes.refuseExposedAdmission(c, host, accept);
      return [setAccept];
    };
    if (shutOut.length) {
      if (courier === NO_COURIER) throw new AccountError("invalidEntry", "shutting a device out hands the device that runs AI a new key: say how", undefined, RULES.hostKeyNext);
      // Those shut out may have read its key: a new epoch, in the same call (review D2).
      const issued = await this.issueMachineKey(host, courier, false, checked);
      return { narrowed: true, admitted, shutOut, mkEpoch: issued.mkEpoch, result: issued.result, hostKeysTo: issued.hostKeysTo, ...(issued.userKey ? { userKey: issued.userKey } : {}) };
    }
    await this.appendBatch(async (c) => checked(c));
    // Let in: the host's committed keys, so it reads what the host wrote.
    for (const id of admitted) {
      try {
        await this.sendHostKeys(id, host);
      } catch (error) {
        // A host whose device role has not signed in yet asks itself once it has.
        const to = this.state?.device(id);
        if (!to || !noDeviceRoleYet(to, error)) console.warn(`cloud account: could not send ${id} ${host}'s keys (it asks again):`, error);
      }
    }
    return { narrowed: false, admitted, shutOut };
  }

  /**
   * Remove device `id` — and, because it held them, retire the keys: in the
   * same call as the removal, a new `UK` epoch and a new `MK` epoch for every
   * active machine are committed; then the keys are put, mailed and handed
   * over (`rotate`; `reencrypt`: the agent rebuilds its streams under the new
   * key and the old ciphertext is deleted).
   *
   * Chain v3 (the chain holds a v3 entry — never because the hub would take
   * one: review D4): `removeDevice3`, for any device. A device that managed
   * the account held `UK`: everything is rotated, as above. One that did not
   * held only machine keys: those alone are rotated (local-aggregator §3.3),
   * and only if `opts.rotateMachineKeys` (the default). Chain v2 removes a
   * machine (`removeMachine`) with no rotation: it held its own key only.
   */
  async removeDevice(
    id: string,
    courier: MachineKeyCourier,
    reencrypt = false,
    opts: {
      rotateMachineKeys?: boolean;
      /**
       * The veto the person confirmed (`vetoPlan`): it stays a veto — what the
       * device brought in goes with it — even if this device's day has passed
       * and the activation was taken in before the call.
       */
      veto?: Pick<VetoPlan, "device" | "seq" | "view">;
      /** With `veto`: the trusted additions the person was shown and chose to take out too (issue #17). */
      alsoTrusted?: readonly string[];
    } = {},
  ): Promise<RotationReport> {
    if (id === this.deviceId) throw new AccountError("self", "remove this device from another device");
    const veto = opts.veto?.device === id ? opts.veto : undefined;
    // The hold the veto is over: this view's, or — taken in by this device's
    // clock since the person read the plan — that one, but only while it is
    // the last one taken in and the chain here has not moved since: a stale
    // plan could take out a device trusted after it (the person looks again).
    let held: HeldBack | null = this.held;
    let sweepTo: number | null = null;
    if (!held && veto) {
      const taken = this.taken;
      if (!taken || taken.seq !== veto.seq || taken.device !== id || this.state?.seq !== taken.takenThroughSeq) {
        throw new AccountError("vetoStale", "{{id}}'s activation was taken in here, and the account's chain has moved on since: look at your devices again, and remove what is not yours there", undefined, undefined, { id });
      }
      held = { seq: veto.seq, device: id, readyMs: 0, entries: [] };
      // What the plan could stand for: up to where this view took the hold in.
      sweepTo = taken.takenThroughSeq;
    }
    if (!held) {
      const record = this.state?.deviceRecord(id);
      const v3 = this.state?.isV3 === true;
      const none: RotationReport = { ukEpoch: null, sealedTo: [], failedTo: [], failedWhy: {}, recovery: false, machines: [] };
      if (v3) {
        const removal: Action = { type: "removeDevice3", id };
        // It ever held the user key (review 1: by the chain's history, not its flags now): everything rotates.
        if (this.heldUserKey(id)) return this.rotate(courier, reencrypt, [removal]);
        // One that could control a host may have its key: the machine keys rotate whatever was asked (review D2).
        if (opts.rotateMachineKeys === false && !this.controlsAHost(id)) {
          this.requireAdmin();
          await this.append(removal);
          return none;
        }
        return this.rotate(courier, reencrypt, [removal], { userKey: false });
      }
      if (record?.host && !record.admin) {
        this.requireAdmin();
        await this.append({ type: "removeMachine", id });
        return none;
      }
      return this.rotate(courier, reencrypt, [{ type: "removeDevice", id }]);
    }
    // While an activation is held back the hub's head is past this view, and
    // a removal goes on it — so only the veto of that very device does, and
    // it takes out everything that device brought in with it (M2 re-review
    // N1: any removal used to take in the hub's whole chain past the hold,
    // the thief's activation and its second device included, and mailed them
    // the new user key).
    if (id !== held.device) {
      throw new AccountError(
        "heldBack",
        `${heldWho(held)}: remove or approve it before removing anything else`,
        held.readyMs,
      );
    }
    // Asked for (issue #17): also what the devices this view trusted brought
    // in meanwhile — only the ones the person was shown (`vetoPlan`), and what
    // those brought in, as the chain the veto is signed on has it.
    const alsoTrusted = opts.alsoTrusted?.length ? new Set(opts.alsoTrusted) : undefined;
    // The devices this view trusted, as it stands now: the syncs ahead (rotate's,
    // and appendBatch's on a 409) may take the activation in once its day has
    // passed here, and the veto must still take out what it is for — not find
    // the held device trusted by then and remove nothing. Taken in already
    // before the call: as the plan the person confirmed read them.
    const view = sweepTo === null || !veto ? this.trustedView() : new Set([...veto.view, this.deviceId]);
    try {
      const report = await this.rotate(
        courier,
        reencrypt,
        (chain) => {
          const live = this.held;
          // Another device's activation held back meanwhile: the veto is not signed on top of it (that would take it in).
          if (live && live.device !== held.device) throw new AccountError("heldBack", `${heldWho(live)}: approve or remove it first`, live.readyMs);
          // The hold as it stands now (its entries may have grown); taken in on
          // the way, everything the chain the veto is signed on has from the
          // activation on — what the held device added since the plan included;
          // taken in before the call, up to where this view took it in.
          const now: HeldBack = live ?? { ...held, entries: chain.allEntries().filter((e) => e.draft.seq >= held.seq && (sweepTo === null || e.draft.seq <= sweepTo)) };
          const scope = this.vetoScope(now, chain, { view, ...(alsoTrusted ? { trusted: alsoTrusted } : {}) });
          // A veto that removes nothing is never published: it would be a bare
          // rotation, its new keys mailed to everyone still in.
          if (!scope.devices.length && !scope.machines.length) {
            throw new AccountError(
              "nothingToRemove",
              "{{id}} and what it brought in are out of the account already, so nothing was signed. Approving it is safe now: it takes in the account's chain as it stands.",
              undefined,
              undefined,
              { id: held.device ?? id },
            );
          }
          // A v3 chain takes v3 removals only (no v2 entry after the first v3 one).
          if (chain.isV3) return [...scope.devices, ...scope.machines].map((d): Action => ({ type: "removeDevice3", id: d }));
          return [...scope.devices.map((d): Action => ({ type: "removeDevice", id: d })), ...scope.machines.map((m): Action => ({ type: "removeMachine", id: m }))];
        },
        { overHeld: true },
      );
      // Done: the plan it was read on is no veto any more.
      if (this.taken?.seq === held.seq) this.taken = null;
      return report;
    } catch (error) {
      // A veto is one append (it cannot be split): with the wider choice on, the way out is to leave it off.
      if (alsoTrusted && error instanceof AccountError && error.kind === "tooManyEntries") {
        throw new AccountError(
          "tooManyEntries",
          "this veto would write {{n}} entries to the account's chain at once, and the hub takes at most {{max}}: untick “Also remove what your other devices added”, then remove those devices one by one afterwards",
          undefined,
          undefined,
          error.vars,
        );
      }
      throw error;
    }
  }

  /**
   * Whether this device may take itself out of the account's chain as it
   * signs out for good ({@link removeSelf}), and if not, why:
   * - `notActive`: it is no active device of the account (no chain, pending,
   *   removed, or another view latched): there is nothing it may sign;
   * - `notAdmin`: only a device that manages the account signs entries;
   * - `lastAdmin`: the chain keeps one admin device at least;
   * - `keepsSignIn`: removing it would void the recovery words or the passkey
   *   slots it set (or came back with) — a sign-out must not take those from
   *   the account; another admin device removes it, knowing that;
   * - `heldBack`: an activation is held back here (only its veto is signed).
   */
  selfRemoval(): SelfRemoval {
    const chain = this.state;
    if (!chain || this.standing !== "active" || this.forked !== null) return "notActive";
    if (!this.isAdmin) return "notAdmin";
    if (!this.pinned) return "notActive";
    if (!chain.admins().some((d) => d.info.id !== this.deviceId)) return "lastAdmin";
    const me = this.deviceId;
    const slots = chain.allSlots().some((s) => !s.removed && (s.setBy === me || chain.deviceRecord(me)?.viaPasskey === s.slot.slot));
    if (chain.voidsRecoveryWords(me, "removal") || slots) return "keepsSignIn";
    if (this.held) return "heldBack";
    return "removable";
  }

  /**
   * Take this device out of the account's chain — signed by itself, as its
   * last entry: `removeDevice3` (chain v3), v2's `removeDevice`. Only what
   * {@link selfRemoval} calls `removable` (checked again on the chain the
   * entry is built on). Nothing is rotated: a device that removes itself is
   * the one that would make the new keys, so it is no reason to retire them
   * — it forgets its own right after ({@link wipe}); the user key it held
   * goes stale for the other admin devices all the same ({@link userKeyStale})
   * and is replaced with the next batch that issues keys. The hub ends its
   * sessions as it takes the removal.
   */
  async removeSelf(): Promise<void> {
    const why = this.selfRemoval();
    if (why !== "removable") {
      const kind = ({ notActive: "notInChain", notAdmin: "notAdmin", lastAdmin: "lastAdmin", keepsSignIn: "self", heldBack: "heldBack" } as const)[why];
      throw new AccountError(kind, `this device cannot remove itself (${why}): remove it from another full device`);
    }
    const me = this.deviceId;
    await this.appendBatch(async (chain) => {
      if (!chain.admins().some((d) => d.info.id !== me)) throw new AccountError("lastAdmin", "the account's last full device stays one");
      return [chain.isV3 ? { type: "removeDevice3", id: me } : { type: "removeDevice", id: me }];
    });
  }

  /** Whether device `id` (not an admin) may control some current host now: removing it rotates the machine keys whatever was asked (review D2). */
  controlsAHost(id: string): boolean {
    const chain = this.state;
    if (!chain) return false;
    return chain.hosts().some((h) => h.status.type === "active" && h.info.id !== id && admits(chain, h.info.id, id));
  }

  // ------------------------------------------------------------ identity key renewal (docs/host-identity-rotation.md)

  /**
   * What the renewal runner (`renewal.ts`) knows that the chain does not say:
   * which hosts renew their identity key here at all (`renews`: the account
   * is on chain v3, the hub takes renewals, and their agents declared
   * `rotate/1`), which the hub has `pending` (their device role suspended,
   * §4.3), and which are isolated (§3.8, checked evidence). Told whenever it
   * changes; the page re-renders.
   */
  noteRenewals(note: RenewalNote): void {
    this.renewalNotes.noteRenewals(note);
  }

  /**
   * The current hosts that should renew their identity key now, each with
   * why ({@link hostsToRotate}: the chain's losses, transitively), for the
   * chain this device holds.
   */
  hostsToRotate(): Map<string, Exposure> {
    return this.renewalNotes.hostsToRotate();
  }

  /** Whether host `id` renews its identity key (rather than being re-enrolled): see {@link noteRenewals}. Never on a v2 chain. */
  renews(id: string): boolean {
    return this.renewalNotes.renews(id);
  }

  /**
   * Whether host `id` is isolated (§3.8): a key it renewed away from by an
   * automatic renewal signed in again, and nothing since confirmed it on the
   * device. No channel is opened to it, no key sealed to it, and it renews
   * only with the code its screen shows.
   */
  isolated(id: string): boolean {
    return this.renewalNotes.isolated(id);
  }

  /**
   * Keep disputes this page verified (§3.8), so
   * that what isolated a host here stays isolating it — whatever the hub says
   * or omits later, across reloads and tabs: read, merged, written, never
   * shrunk here. Isolation follows from them on this page's own chain
   * ({@link disputeIsolates}), before any channel is opened or key sealed.
   */
  keepDisputes(found: readonly KeptDispute[]): void {
    this.renewalNotes.keepDisputes(found);
  }

  /**
   * The disputes this browser verified for the account, as kept
   * ({@link keepDisputes}): what storage holds, and what this page kept or
   * heard of in memory — one each, by host and renewal. Each is verified
   * again on this page's chain (R6 review) and counts only once it checks
   * out: storage, and another tab, are this origin's to write, not evidence.
   * One about a renewal this page's chain does not hold yet counts until
   * the chain brings it, if this browser verified that far or cannot tell
   * (`ahead`, see {@link keptChecks}); one past that does not, but is kept.
   */
  keptDisputes(): KeptDispute[] {
    return this.renewalNotes.keptDisputes();
  }

  /**
   * Whether the hub flags host `id` isolated with no evidence this page checked
   * (R6 review): an unverified note — keys are withheld from it and it renews only
   * confirmed on the device, but nothing here treats it as isolated.
   */
  flagged(id: string): boolean {
    return this.renewalNotes.flagged(id);
  }

  /**
   * Whether this device seals host `id`'s device role none of the hosts' keys
   * (purpose 5, and no answer to its purpose-4 request: §4.3, §8 item 11):
   * it is isolated, or waiting to renew — `pending` at the hub, or exposed
   * on the chain and able to renew. One that cannot renew (an older agent)
   * keeps getting them: it would never get them back.
   */
  keysWithheldFrom(id: string): boolean {
    return this.renewalNotes.keysWithheldFrom(id);
  }

  /**
   * Renew host `host`'s identity key (docs/host-identity-rotation.md §3.1–§3.7):
   *
   * 1. register an attempt at the hub with the hash of a fresh nonce
   *    (`PUT /v1/rotations/{host}`; only one at a time per host, across pages);
   * 2. ask the host over the end-to-end channel (`POST /rh/rotate`, to the
   *    DH key the chain has now) with the nonce, an ephemeral X25519 key and
   *    this device's chain view: it stages a new seed and answers with its new
   *    keys, both keys' signatures over the proof and a MAC under the new DH
   *    key and the ephemeral one;
   * 3. check all of it here: the view is the one asked about, the new keys are
   *    valid and ones the chain never named, the old key and the new one
   *    signed the proof, the MAC proves the new DH key; and, confirmed on the
   *    device (`ASSURANCE_ON_DEVICE`), the code the person typed is the proof's
   *    (`rotateSas`) — the page never shows it;
   * 4. wait until the hub staged it and its window passed (the host's every
   *    sign-in with the old key had to confirm with the new one);
   * 5. write `[rotateHostKey, keyEpochs]` — its next machine key epoch in the
   *    same call, sealed to the new key only — and hand that key over with a
   *    re-encryption (channel to the new key, else its mailbox), seal its
   *    device role the hosts' keys again, and vouch for the new head.
   *
   * Anything that does not check out withdraws the attempt and throws a
   * {@link RenewalError}; the host stays exposed and is tried again by the
   * runner as `renewal.ts` decides. Only a current admin, on chain v3.
   */
  async renewIdentity(host: string, courier: RenewalCourier, opts: RenewOptions): Promise<RenewalReport> {
    for (let tries = 1; ; tries++) {
      try {
        return await this.renewOnce(host, courier, opts);
      } catch (error) {
        // A loss after it staged (§4.1 「清除」): withdrawn already — again, with a fresh nonce and seed.
        if (error instanceof RenewalError && error.kind === "superseded" && tries < RENEW_RESTARTS) continue;
        throw error;
      }
    }
  }

  /**
   * Whether `host`'s renewal may be written on `chain` — the very chain the
   * batch is signed on, on each build and every try on a newer head:
   * still a host; never ahead of an exposed host it lets control
   * it; no device that could control it lost after the view its new key was
   * staged on (`superseded`: started again); and, unless it is confirmed on
   * the device, not isolated and not refused by the account's policy or this
   * device's own guard — whatever was learned while it waited.
   */
  private checkRenewalOn(chain: ChainState, host: string, rotation: HostRotation, assurance: Assurance): void {
    if (!chain.machine(host)) throw new AccountError("notInChain", "{{host}} is not a device of the account that runs AI", undefined, undefined, { host });
    const exposed = hostsToRotate(chain, (h) => this.renews(h));
    const x = exposed.get(host);
    if (x) {
      const first = exposedControllers(chain, exposed, host);
      if (first.length) {
        throw new RenewalError("renewFirst", `renew the identity key of ${first.join(", ")} first: ${host} lets ${first.length > 1 ? "them" : "it"} control it`, undefined, first);
      }
      if ([...x.lost.values(), ...x.via.values()].some((at) => at > rotation.stagedSeq)) {
        throw new RenewalError("superseded", `a device that could control ${host} was lost after its new key was staged: starting again`);
      }
    }
    this.renewalNotes.checkAssurance(chain, host, assurance);
  }

  /**
   * The checks every renewal of `host` signed here goes through first,
   * whoever asks (the runner or a person) and whether it starts or is
   * finished after a reload ({@link resumeRenewal}): an admin, chain v3, a
   * current host, never ahead of an exposed host it lets control it, the
   * account's policy chosen; and an automatic one never for an isolated host,
   * nor against the account's policy or this device's own guard.
   */
  private requireRenewable(host: string, assurance: Assurance): void {
    const chain0 = this.requireAdmin();
    this.requireUpgraded(chain0);
    if (!chain0.machine(host)) throw new AccountError("notInChain", "{{host}} is not a device of the account that runs AI", undefined, undefined, { host });
    // Controllers first (§4.1): never ahead of an exposed host it lets control it — whoever asks, the runner or a person.
    this.renewalNotes.requireRenewFirst(host);
    // The account chose how renewals are confirmed first (§5.6 「打开的顺序」).
    if (chain0.renewalPolicySeq === null) throw new RenewalError("policyUnset", "choose how identity key renewals are confirmed first");
    if (assurance !== ASSURANCE_ON_DEVICE && this.isolated(host)) {
      throw new RenewalError("needsAssurance", `${host} is isolated: renew its identity key with the code its screen shows`);
    }
    // The account's policy, or this device's own guard (§4.4): no automatic renewal is signed here.
    if (assurance !== ASSURANCE_ON_DEVICE && this.renewalsAssured) {
      throw new RenewalError("needsAssurance", "renewals are confirmed on the device here", this.localAssured() ? "local" : "policy");
    }
  }

  /**
   * `work` holding this browser's lock on renewing `host` (`navigator.locks`),
   * from before the attempt is registered until it is written or withdrawn —
   * or {@link BUSY}, without running it, while another tab of this browser
   * holds it: that tab is alive, and on it. Without Web Locks it just runs.
   */
  private async holdRenewal<T>(host: string, work: () => Promise<T>): Promise<T | typeof BUSY> {
    if (!this.locks) return work();
    return this.locks.request<T | typeof BUSY>(renewalLockName(this.account, host), { mode: "exclusive", ifAvailable: true }, async (lock) => (lock === null ? BUSY : await work()));
  }

  /** Keep `kept` for the next page of this browser ({@link resumeRenewal}); a store that fails leaves it as before: lost with this page. */
  private async keepRenewal(kept: KeptRenewal): Promise<void> {
    try {
      await this.renewalStash.put(kept);
    } catch (error) {
      console.warn(`cloud account: could not keep ${kept.host}'s renewal for a reload in its window:`, error);
    }
  }

  /** Forget what was kept of the attempt `nonceHash` (hex) names, if it still is that one. */
  private async forgetRenewal(host: string, nonceHash: string): Promise<void> {
    try {
      await this.renewalStash.drop(host, nonceHash);
    } catch (error) {
      console.warn(`cloud account: could not forget ${host}'s renewal:`, error);
    }
  }

  /**
   * The rest of the attempt `nonceHash` names (`work`, told how to say it was
   * written): unless it was, withdrawn at the hub — and, either way, nothing
   * of it kept for another page after.
   */
  private async settleRenewal<T>(host: string, nonceHash: Uint8Array, work: (written: () => Promise<void>) => Promise<T>): Promise<T> {
    let committed = false;
    try {
      return await work(async () => {
        committed = true;
        await this.forgetRenewal(host, hex(nonceHash));
      });
    } finally {
      if (!committed) {
        // Withdrawn: a staging nobody will write is void (an answer that did not check out may not have been the computer's).
        // This attempt only: by now the host may have another page's newer one (this one ran out meanwhile), which stays.
        try {
          await this.api.deleteRotation(this.session.token, host, { nonceHash });
        } catch {
          // the attempt lapses in 10 minutes anyway
        }
        // Whatever the withdrawal did: this attempt is over, and no other page writes it.
        await this.forgetRenewal(host, hex(nonceHash));
      }
    }
  }

  /** One try of {@link renewIdentity}: anything that does not check out withdraws it. */
  private async renewOnce(host: string, unguarded: RenewalCourier, opts: RenewOptions): Promise<RenewalReport> {
    // Nothing of a renewal leaves a page whose view forked: every call to the host and every key goes through the latch.
    this.assertNotForked();
    const courier = this.guardedRenewal(unguarded);
    const assurance = opts.assurance;
    const recheck = this.renewalNotes.assuranceRecheck(host, assurance);
    this.requireRenewable(host, assurance);
    const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
    const now = opts.now ?? Date.now;
    const nonce = crypto.getRandomValues(new Uint8Array(16));
    const eph = (await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
    const ephPublic = await exportPublic(eph.publicKey);
    opts.onStep?.("requesting");
    const nonceHash = await sha256(nonce);
    const done = await this.holdRenewal(host, async () => {
      try {
        await this.api.putRotation(this.session.token, host, { nonceHash, assurance });
      } catch (error) {
        throw renewalRefusal(error);
      }
      // Kept before anything is asked of the host: should this page go away now, the next page of this browser withdraws
      // it at once (`resumeRenewal`) instead of the host waiting for it to lapse.
      const kept: KeptRenewal = { host, device: this.deviceId, nonceHash: hex(nonceHash), assurance, askedMs: now() };
      await this.keepRenewal(kept);
      return this.settleRenewal(host, nonceHash, async (written) => {
        try {
          await this.sync();
        } catch (error) {
          console.warn("cloud account: sync before renewing:", error);
        }
        // The head the request is about, vouched for before it goes: a page that just changed the account (its renewal
        // policy, which starts the renewals) would otherwise ask on a head it never vouched for.
        const vouched = await this.vouch(courier);
        const { view, answer } = await this.askRotate(host, courier, { nonce, eph: ephPublic, assurance }, sleep, now, vouched);
        recheck();
        const chain = this.state!;
        const old = chain.machine(host)?.info;
        if (!old) throw new AccountError("notInChain", "{{host}} is not a device of the account that runs AI", undefined, undefined, { host });
        const rotation = await checkRotateAnswer(chain, host, old, answer, view, nonce, assurance);
        const body = hostRotationBody(this.account, rotation, old.signKey, old.dhKey);
        if (!(await verifyRotateConfirm(eph.privateKey, rotation.dhKey, body, answer.mac))) {
          throw new RenewalError("badAnswer", `${host}'s answer does not prove it holds its new key`, "mac");
        }
        if (assurance === ASSURANCE_ON_DEVICE) {
          opts.onStep?.("confirming");
          const typed = opts.askCode ? await answeredWithin(opts.askCode(host), opts.codeWaitMs ?? CODE_WAIT_MS) : null;
          if (typed === null) throw new RenewalError("cancelled", "no code was typed");
          const want = (await rotateSas(body)).replace("-", "");
          if (typed.toUpperCase().replace(/[^A-Z2-7]/g, "") !== want) {
            throw new RenewalError("wrongCode", `the code typed is not the one ${host} shows: something else may be answering as it`);
          }
        }
        // All of it checked here: what writes it is kept (sealed) for the window, which this page may not live through —
        // a reload, a crash, a closed tab — so that the next page of this browser writes it (`resumeRenewal`).
        await this.keepRenewal({ ...kept, checked: { nonce: hex(nonce), view: { seq: view.seq, head: hex(view.head) }, answer: rotateAnswerJson(answer) } });
        return this.writeRenewal(host, courier, opts, { rotation, readyMs: answer.readyMs, sleep, now, recheck, written });
      });
    });
    if (done === BUSY) throw new RenewalError("inProgress", `another tab of this browser is renewing ${host} now`, "tab");
    return done;
  }

  /**
   * Forget what this browser kept of attempts the hub no longer has (`rows`,
   * its renewals as last read): written, withdrawn, voided or run out, by
   * whichever page. Each is read at the hub again first, and kept while the
   * hub still has it open — a tab of this browser that is on it keeps it.
   */
  async forgetLeftRenewals(rows: readonly HostRotationView[]): Promise<void> {
    const open = (r: HostRotationView | undefined, nonceHash: string) => !!r && (r.state === "attempt" || r.state === "staged") && r.nonceHash === nonceHash;
    for (const host of await this.renewalStash.hosts().catch(() => [] as string[])) {
      // One that does not open now (a store that hands back nothing for a while, say) is left as it is.
      const kept = await this.renewalStash.get(host).catch(() => null);
      if (!kept || open(rows.find((r) => r.host === host), kept.nonceHash)) continue;
      let now: HostRotationView | undefined;
      try {
        now = (await this.api.rotations(this.session.token)).hosts.find((r) => r.host === host);
      } catch {
        return;
      }
      if (!open(now, kept.nonceHash)) await this.forgetRenewal(host, kept.nonceHash);
    }
  }

  /** Whether this browser kept the attempt `nonceHash` (hex) names of `host`: one a page of it asked for, maybe gone since ({@link resumeRenewal}). */
  async keptRenewal(host: string, nonceHash: string): Promise<boolean> {
    const kept = await this.renewalStash.get(host).catch(() => null);
    return kept !== null && kept.nonceHash === nonceHash;
  }

  /**
   * Finish what this browser asked of `host` in a page that went away before
   * writing it — reloaded, crashed or closed in the window (§4.2: only the
   * page that asked can check the host's answer, so nobody else writes it,
   * and the host would wait for the staging to lapse). The hub's row must
   * still be the attempt this browser kept ({@link KeptRenewal}); its answer
   * is checked again on this page's chain — the view, both signatures, keys
   * the chain never named — and then written as the page that asked would
   * have, after the same checks of order, policy and isolation. An attempt
   * left before its answer checked out here is withdrawn at once instead:
   * never written.
   *
   * `null`: nothing of this browser's to finish (what it kept of it is
   * forgotten), or withdrawn — the runner starts again with a fresh nonce;
   * `"busy"`: another tab of this browser holds it, and is alive — or, in a
   * browser without Web Locks, may be (nothing tells a live tab from one that
   * went away there: its attempt is left for the runner's
   * abandonment, as another page's). Anything that does not check out
   * withdraws it and throws, as {@link renewIdentity}.
   */
  async resumeRenewal(host: string, unguarded: RenewalCourier, opts: Partial<Omit<RenewOptions, "assurance">> = {}): Promise<RenewalReport | "busy" | null> {
    if (!(await this.renewalStash.get(host).catch(() => null))) return null;
    // No Web Locks: the tab that asked may still be on it (typing the code, waiting for the window) — never taken from it.
    if (!this.locks) return "busy";
    const done = await this.holdRenewal(host, async (): Promise<RenewalReport | null> => {
      // Read again under the lock: the tab that held it may have finished it meanwhile.
      const kept = await this.renewalStash.get(host).catch(() => null);
      if (!kept) return null;
      let row: HostRotationView | undefined;
      try {
        row = (await this.api.rotations(this.session.token)).hosts.find((r) => r.host === host);
      } catch (error) {
        throw renewalRefusal(error);
      }
      if (!row || (row.state !== "attempt" && row.state !== "staged") || row.nonceHash !== kept.nonceHash || kept.device !== this.deviceId) {
        // Written, withdrawn, voided or run out since (or another device's now): nothing of it to finish.
        await this.forgetRenewal(host, kept.nonceHash);
        return null;
      }
      const nonceHash = fromHex(kept.nonceHash);
      const assurance = kept.assurance;
      const full: RenewOptions = { ...opts, assurance };
      const sleep = full.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
      const now = full.now ?? Date.now;
      try {
        return await this.settleRenewal(host, nonceHash, async (written) => {
          const checked = kept.checked;
          // Left before its answer checked out here (the MAC needs the ephemeral key, which went with that page): withdrawn.
          if (!checked) return null;
          this.assertNotForked();
          const courier = this.guardedRenewal(unguarded);
          const recheck = this.renewalNotes.assuranceRecheck(host, assurance);
          this.requireRenewable(host, assurance);
          const nonce = fromHex(checked.nonce);
          if (!equal(await sha256(nonce), nonceHash)) throw new RenewalError("badAnswer", `what this browser kept of ${host}'s renewal is not its attempt`, "nonce");
          const chain = this.state!;
          const old = chain.machine(host)!.info;
          const answer = parseRotateAnswer(checked.answer);
          const rotation = await checkRotateAnswer(chain, host, old, answer, { seq: checked.view.seq, head: fromHex(checked.view.head) }, nonce, assurance);
          return this.writeRenewal(host, courier, full, { rotation, readyMs: answer.readyMs, sleep, now, recheck, written });
        });
      } catch (error) {
        // A loss after it staged (§4.1 「清除」): withdrawn already — the runner starts again with a fresh nonce and seed.
        if (error instanceof RenewalError && error.kind === "superseded") return null;
        throw error;
      }
    });
    return done === BUSY ? "busy" : done;
  }

  /**
   * The end of a renewal whose answer checked out (`renewOnce`, or
   * `resumeRenewal` after a reload): wait until the hub staged `rotation` and
   * its window passed, write `[rotateHostKey, keyEpochs]` — checked on the
   * very chain each build signs on — and hand the new machine key over.
   * `written` is told as soon as the chain took it.
   */
  private async writeRenewal(
    host: string,
    courier: RenewalCourier,
    opts: RenewOptions,
    r: { rotation: HostRotation; readyMs: number | null; sleep: (ms: number) => Promise<void>; now: () => number; recheck: () => void; written: () => Promise<void> },
  ): Promise<RenewalReport> {
    const { rotation, sleep, now } = r;
    const assurance = rotation.assurance;
    opts.onStep?.("waiting");
    let readyMs = await this.waitStaged(host, rotation, r.readyMs, sleep, now, opts, r.recheck);
    opts.onStep?.("writing");
    const mk = random32();
    try {
      let issued: { mkEpoch: number; ukEpoch: number; userKey: FreshUserKey | null } | null = null;
      for (let tries = 0; issued === null; tries++) {
        const wait = readyMs - now();
        if (wait > 0) await sleep(Math.min(wait + 250, opts.maxWaitMs ?? 10 * 60_000));
        try {
          // Checked on the chain it is built on — after any wait, and again on a newer head after a 409.
          issued = await this.commitHostKey(
            host,
            mk,
            (chain) => {
              this.checkRenewalOn(chain, host, rotation, assurance);
              return [{ type: "rotateHostKey", rotation }];
            },
            // …and once more right before publication, after the signing's awaits: the guard or a dispute
            // that arrived meanwhile stops it there.
            (build) => this.appendBatch(build, { beforePublish: (base) => this.checkRenewalOn(base, host, rotation, assurance) }),
          );
        } catch (error) {
          // Early by the hub's clock: its `ready_ms` says when (a few times; the window is short).
          if (error instanceof HubApiError && error.code === "rotation_not_ready" && error.reason === "window" && tries < 5) {
            readyMs = Math.max(error.readyMs ?? 0, now() + 1000);
            continue;
          }
          // Not staged any more: the hub voided it in the window — and says why (graded evidence, §3.3).
          if (error instanceof HubApiError && error.code === "rotation_not_ready" && (error.reason === "not_staged" || error.reason === "mismatch")) {
            const row = await this.api.rotations(this.session.token).then((v) => v.hosts.find((x) => x.host === host), () => undefined);
            if (row?.failure) throw new RenewalError("voided", `the hub voided ${host}'s renewal: ${row.failure.reason}`, row.failure.reason);
          }
          throw renewalRefusal(error);
        }
      }
      await r.written();
      const { mkEpoch, ukEpoch, userKey } = issued;
      const seq = this.state!.deviceRecord(host)?.rotatedSeq ?? this.state!.seq;
      // No longer waiting: the hosts' keys go to its new key from here on. Confirmed on the device, it is no longer isolated either (§3.8).
      const pending = new Set(this.renewalNotes.renewalNote.pending);
      pending.delete(host);
      const isolated = new Set(this.renewalNotes.renewalNote.isolated);
      if (assurance === ASSURANCE_ON_DEVICE) isolated.delete(host);
      this.noteRenewals({ ...this.renewalNotes.renewalNote, pending, isolated });
      opts.onStep?.("handing");
      // Its keys to the devices it lets control it (purpose 5), then the key to it — sealed to the new DH key only — with a re-encryption.
      await this.mailHostKey(host, mkEpoch, mk);
      let result: DeliveryResult;
      try {
        result = await this.handOver(host, mkEpoch, ukEpoch, mk, courier, true);
      } catch (error) {
        result = { via: "refused", status: 0, message: error instanceof Error ? error.message : String(error) };
      }
      let hostKeys: number | null = null;
      try {
        hostKeys = await this.handHostKeys(host, opts.hostKeysWait ?? HOST_KEYS_WAIT);
      } catch (error) {
        console.warn(`cloud account: could not seal ${host}'s device role the hosts' keys again (it asks):`, error);
      }
      try {
        await courier.attest();
      } catch (error) {
        console.warn("cloud account: attesting after a renewal:", error);
      }
      this.emit();
      return {
        host,
        seq,
        assurance,
        signKey: rotation.signKey,
        dhKey: rotation.dhKey,
        fingerprint: await deviceFingerprint(rotation.signKey, rotation.dhKey),
        mkEpoch,
        result,
        hostKeys,
        ...(userKey ? { userKey } : {}),
      };
    } finally {
      mk.fill(0);
    }
  }

  /**
   * `POST /rh/rotate` to `host` with this device's chain view: once more
   * after a sync when the view is not on its chain (409 `not_on_chain`); and
   * again for {@link STALE_GRACE_MS} while the host finds no admin's recent
   * word on the account as it sees it (412 `stale`), by its `reason`:
   * `stale` — the newest entries taken first (this page may lag the host: an
   * entry another device wrote, or another tab), then that head vouched for;
   * `behind` — the host catches up on its own, and this page takes what the
   * hub has too. A `fork` or a `lapsed` is not waited out. Only another admin
   * device's word counts for this device's request unless it is the sole
   * admin (`ChainState.freshness`), so what outlasts the grace says who it
   * waits for ({@link staleRefusal}). `problem`: why the word before the first
   * ask did not go out, if it did not.
   */
  private async askRotate(
    host: string,
    courier: RenewalCourier,
    req: { nonce: Uint8Array; eph: Uint8Array; assurance: Assurance },
    sleep: (ms: number) => Promise<void>,
    now: () => number,
    problem: unknown = null,
  ): Promise<{ view: { seq: number; head: Bytes }; answer: RotateAnswer }> {
    const deadline = now() + STALE_GRACE_MS;
    let resynced = false;
    for (;;) {
      const chain = this.state!;
      const view = { seq: chain.seq, head: copy(chain.head) };
      const body: RotateRequest = { nonce: hex(req.nonce), eph: hex(req.eph), chainSeq: view.seq, chainHead: hex(view.head), assurance: req.assurance };
      let res: { status: number; body: unknown };
      try {
        res = await courier.rotate(host, body);
      } catch (error) {
        // The fork alarm stands as it is: not a failure to reach the host.
        if (error instanceof AccountError && error.kind === "fork") throw error;
        if (error instanceof ChannelUnavailable && error.reason === "offline") throw new RenewalError("offline", `${host} is offline`);
        throw new RenewalError("failed", error instanceof Error ? error.message : String(error), error instanceof ChannelUnavailable ? error.reason : undefined);
      }
      const answer = res.body as { error?: unknown; message?: unknown; reason?: unknown } | null;
      const code = typeof answer?.error === "string" ? answer.error : "";
      const said = typeof answer?.message === "string" ? answer.message : `${res.status}`;
      if (res.status === 200) return { view, answer: parseRotateAnswer(res.body) };
      if (res.status === 404) throw new RenewalError("tooOld", `${host}'s Droppen does not renew its identity key`);
      if (res.status === 412) {
        const reason = typeof answer?.reason === "string" ? answer.reason : "stale";
        if (reason === "fork") {
          // Not waited out. Whether this page's own view forked from the hub's, its sync says (and latches the alarm);
          // the host raised its own.
          await this.syncForRenewal();
          throw new RenewalError("fork", `${host}'s view of the account's device list conflicts with what an admin device vouched for`, reason);
        }
        if (reason === "lapsed") throw new RenewalError("lapsed", `${host} takes no control from this device now: ${said}`, reason);
        if (now() < deadline) {
          // The newest entries first, then a word on that head — never on one this page has not caught up to. What
          // went wrong in this round (if anything) is what the refusal says, should it stay.
          const synced = await this.syncForRenewal();
          problem = (reason === "behind" ? null : await this.vouch(courier)) ?? synced;
          await sleep(2000);
          continue;
        }
        throw this.staleRefusal(host, reason, problem);
      }
      if (res.status === 409 && code === "not_on_chain" && !resynced) {
        resynced = true;
        await this.sync().catch(() => undefined);
        continue;
      }
      if (res.status === 409 && code === "hub_not_ready") throw new RenewalError("hubNotReady", `${host} cannot stage a renewal at the hub yet`, code);
      if (res.status === 409 && code === "hub_refused") {
        const reason = typeof answer?.reason === "string" ? answer.reason : "refused";
        if (reason === "conflict") throw new RenewalError("voided", `the hub saw another renewal of ${host}`, reason);
        if (reason === "disabled") throw new RenewalError("disabled", "the hub takes no identity key renewal now", reason);
        throw new RenewalError("refused", `the hub would not stage ${host}'s renewal: ${reason}`, reason);
      }
      throw new RenewalError("refused", `${host} refused: ${said}`, code || String(res.status));
    }
  }

  /** Take the hub's newest entries for a renewal: `null`, or why not (the fork alarm is thrown, as ever). */
  private async syncForRenewal(): Promise<unknown> {
    try {
      await this.sync();
      return null;
    } catch (error) {
      if (error instanceof AccountError && error.kind === "fork") throw error;
      console.warn("cloud account: sync for a renewal:", error);
      return error;
    }
  }

  /** Vouch for the head this page holds now (`courier.attest`): `null` once it went out, or why not (the fork alarm is thrown). */
  private async vouch(courier: RenewalCourier): Promise<unknown> {
    try {
      await courier.attest();
      return null;
    } catch (error) {
      if (error instanceof AccountError && error.kind === "fork") throw error;
      console.warn("cloud account: vouching for the chain head for a renewal:", error);
      return error;
    }
  }

  /**
   * A 412 `stale` (`reason`) that outlasted the grace, as a
   * {@link RenewalError}: `behind`, or `unconfirmed` — naming the account's
   * other admin devices, one of which has to vouch for the head (this
   * device's own word does not count while there are others), or, as the
   * sole admin, why its own word did not go (`problem`, the last one seen).
   */
  private staleRefusal(host: string, reason: string, problem: unknown): RenewalError {
    if (reason === "behind") return new RenewalError("behind", `${host} has not caught up with the account's latest changes yet`, reason);
    const others = (this.state?.admins() ?? []).map((d) => d.info.id).filter((id) => id !== this.deviceId);
    if (others.length) {
      return new RenewalError("unconfirmed", `${host} waits for another admin device (${others.join(", ")}) to vouch for the account's latest change: this device's own word does not count for its own request`, reason, others);
    }
    const why = problem === null || problem === undefined ? null : problem instanceof Error ? problem.message : String(problem);
    return new RenewalError("unconfirmed", `${host} found no recent word from this device on the account's latest change${why ? `: ${why}` : ""}`, why ?? reason, []);
  }

  /**
   * When the hub staged `rotation` and its window closes (its `ready_ms`):
   * from the host's answer, else from `GET /v1/rotations`, looked at until it
   * is staged with exactly these keys. A staging voided meanwhile (another
   * holder of the old key: `not_acked`, `conflict`) is a {@link RenewalError}
   * `voided`; so is one staged with other keys.
   */
  private async waitStaged(
    host: string,
    rotation: HostRotation,
    answered: number | null,
    sleep: (ms: number) => Promise<void>,
    now: () => number,
    opts: RenewOptions,
    recheck: () => void = () => undefined,
  ): Promise<number> {
    const deadline = now() + (opts.maxWaitMs ?? 10 * 60_000);
    const readyMs = answered;
    for (;;) {
      recheck();
      let row;
      try {
        row = (await this.api.rotations(this.session.token)).hosts.find((r) => r.host === host);
      } catch (error) {
        throw renewalRefusal(error);
      }
      if (row?.failure && row.state !== "staged") throw new RenewalError("voided", `the hub voided ${host}'s renewal: ${row.failure.reason}`, row.failure.reason);
      if (row?.state === "staged") {
        if (!row.signKey || !equal(row.signKey, rotation.signKey) || (row.dhKey && !equal(row.dhKey, rotation.dhKey))) {
          throw new RenewalError("voided", `the hub staged other keys for ${host}`, "mismatch");
        }
        return row.readyMs ?? readyMs ?? now();
      }
      if (row?.state === "committed") throw new RenewalError("voided", `${host} renewed elsewhere meanwhile`, "committed");
      // No row (or a bare mark) after this page opened its attempt: it was withdrawn — by another page, or the hub. Not
      // waited for: tried again with the usual backoff.
      if (!row || row.state === "pending") throw new RenewalError("voided", `${host}'s renewal was withdrawn`, "withdrawn");
      if (now() >= deadline) throw new RenewalError("failed", `the hub did not stage ${host}'s renewal in time`, "timeout");
      await sleep(opts.pollMs ?? 2000);
    }
  }

  /**
   * The account's 「更换身份密钥时总在设备上确认」 (§3.4 rule 9, §4.4), as this
   * page's verified chain has it (`setRenewalPolicy`; off until an admin turns
   * it on): every renewal then confirmed on the device — the chain refuses any
   * other (`RULES.rotateNeedsAssurance`).
   */
  get alwaysAssured(): boolean {
    return this.renewalNotes.alwaysAssured;
  }

  /** Whether an admin chose the account's renewal policy (a `SetRenewalPolicy` on the chain): nothing renews on its own before (§5.6 「打开的顺序」). */
  get renewalPolicyChosen(): boolean {
    return this.state?.renewalPolicySeq !== null && this.state?.renewalPolicySeq !== undefined;
  }

  /** Set the account's renewal policy: a `setRenewalPolicy` entry, through the chain (an admin, chain v3). */
  async setRenewalPolicy(alwaysAssured: boolean): Promise<void> {
    const chain = this.requireAdmin();
    this.requireUpgraded(chain);
    // Chosen already, and the same: nothing to write. Never chosen: either choice is written (§5.6 「打开的顺序」).
    if (chain.renewalPolicySeq !== null && chain.alwaysAssured === alwaysAssured) return;
    await this.append({ type: "setRenewalPolicy", alwaysAssured });
  }

  /**
   * This device's own 「这台设备只签在设备上确认的换钥」 (§4.4): while set, it signs
   * no renewal that is not confirmed on the device — whatever the hub or the
   * chain says. Kept in this browser only.
   */
  localAssured(): boolean {
    return this.renewalNotes.localAssured();
  }

  setLocalAssured(on: boolean): void {
    this.renewalNotes.setLocalAssured(on);
  }

  /** Whether renewals signed here must be confirmed on the device: the account's policy, or this device's own guard. */
  get renewalsAssured(): boolean {
    return this.renewalNotes.renewalsAssured;
  }

  /**
   * Whether device `id` is a host no other host may be made to accept until
   * it renewed its identity key (§4.1, §7 decision 8): isolated, or exposed
   * ({@link hostsToRotate}) and able to renew. `HostControl` says so before
   * Save; `setAccept` refuses it.
   */
  mustRenewFirst(id: string): boolean {
    return this.renewalNotes.mustRenewFirst(id);
  }

  /**
   * {@link mustRenewFirst} on `chain` — the chain a batch is being built on
   * (a 409's rebuild goes on a newer one). The hub's isolated
   * flag holds a host back too (R6 review): unverified, it is no proof — but no
   * reason to let it control another host either.
   */
  mustRenewFirstOn(chain: ChainState, id: string): boolean {
    return this.renewalNotes.mustRenewFirstOn(chain, id);
  }

  /**
   * Write the renewal-pause markers `build` makes for the chain as it stands
   * (§3.4 rule 10; built again on a newer head): a start, an end, or an end
   * with the start that keeps what must stay held. Whether anything was
   * written (an empty build writes nothing).
   */
  async writePause(build: (chain: ChainState) => Action[], beforePublish?: (base: ChainState) => void): Promise<boolean> {
    const chain0 = this.requireAdmin();
    this.requireUpgraded(chain0);
    try {
      await this.appendBatch(
        async (chain) => {
          const actions = build(chain);
          if (!actions.length) throw new NothingToWrite();
          return actions;
        },
        { beforePublish },
      );
      return true;
    } catch (error) {
      if (error instanceof NothingToWrite) return false;
      throw error;
    }
  }

  /** How long the encoded entry of a pause start with `removals` would be, signed here on `chain` (M6: `MAX_PAUSE_ENTRY_LEN`). */
  pauseEntryLen(chain: ChainState, removals: PauseRemoval[]): number {
    const draft = chain.draft({ type: "device", id: this.deviceId }, { type: "renewalPause", op: new Uint8Array(16), phase: PAUSE_START, removals }, Date.now());
    return encodeDraft(draft).length + 64;
  }

  /**
   * What ending open pause `op` now would give back, and what stops it
   * (§4.1 「卡住的暂停」, R6 review): the edges that come back, the hosts among
   * them whose base lets every device in, and each edge that must stay held,
   * with why — judged on this page's chain alone (`edgeHeld`). `null`: no
   * such open pause.
   */
  pauseEnding(op: Uint8Array): PauseEnding | null {
    const chain = this.state;
    const pause = chain?.openPauses().find((p) => equal(p.op, op));
    if (!chain || !pause) return null;
    const { edges, all } = pauseReturns(chain, pause);
    const held = this.heldEdges(chain, edges);
    // Started by a device that is no longer an admin: in force all the same, and ended only by a person (M7).
    return { edges, all, held, orphan: !chain.admin(pause.signer) };
  }

  /**
   * Of `edges`, those that must stay held on `chain` (§4.1 「结束之前」), and
   * why: judged on the chain alone, with the isolation and the hub's flags
   * this page knows now. An exposed end that can never renew here (an older
   * agent, `renews`) says so (`cannotRenew`), not merely `exposed`.
   */
  private heldEdges(chain: ChainState, edges: { host: string; id: string }[]): PauseEnding["held"] {
    const exposed = hostsToRotate(chain);
    return edges.flatMap((e) => {
      const why = edgeHeld(chain, e, { exposed, isolated: (id) => this.isolated(id), flagged: (id) => this.flagged(id) });
      if (!why) return [];
      return [{ ...e, who: why.who, why: why.why === "exposed" && chain.machine(why.who) && !this.renews(why.who) ? ("cannotRenew" as const) : why.why }];
    });
  }

  /**
   * End open pause `op` (§3.4 rule 10, §4.1): `phase` completed (the runner)
   * or cancelled (a person's 「结束暂停」, after a confirm). `plan` says, on the
   * chain the batch is built on, which of its edges must stay held — they go
   * in a new start in the same batch (`next`) — or `null` to write nothing.
   *
   * - What it gives back is checked in the build and once more right before
   *   publication (H1), with the isolation and the hub's flags known then:
   *   one that must stay held aborts it (`exposed`; the runner tries again
   *   on its next look, a person is told). That last check is of the edges
   *   the SIGNED batch gives back — never of a plan made again, which would
   *   keep held what the signed batch returns all the same.
   * - `expect`: the exact edges a person was shown (M7). Anything else
   *   coming back — a base widened, an overlapping pause ended — refuses it
   *   (`pauseChanged`): ask again.
   * - Once written, every device let in again is sealed the host's committed
   *   machine key epochs (D2, purpose 5): what it missed while paused.
   *
   * The edges given back; `null`: nothing written.
   */
  async endPauseWith(
    op: Uint8Array,
    phase: typeof PAUSE_COMPLETED | typeof PAUSE_CANCELLED,
    plan: (chain: ChainState, pause: OpenPause) => { keep: PauseRemoval[] } | null,
    opts: { expect?: { host: string; id: string }[]; next?: Uint8Array } = {},
  ): Promise<{ host: string; id: string }[] | null> {
    const opHex = hex(op);
    const next: Bytes = opts.next ? copy(opts.next) : crypto.getRandomValues(new Uint8Array(16));
    type Edge = { host: string; id: string };
    const edgeKey = (e: Edge) => `${e.host}<${e.id}`;
    const refuseHeld = (chain: ChainState, back: Edge[]) => {
      const held = this.heldEdges(chain, back);
      if (held.length) {
        throw new AccountError("exposed", "renew the identity key of {{ids}} first: ending the pause would let it control a device again", undefined, undefined, {
          ids: [...new Set(held.map((h) => h.who))].sort().join(", "),
        });
      }
    };
    // What ending it on `chain` gives back, held to everything it must be: the actions (none: nothing to write), and the edges they return.
    const settle = (chain: ChainState): { actions: Action[]; back: Edge[] } => {
      const pause = chain.openPauses().find((p) => hex(p.op) === opHex);
      if (!pause) return { actions: [], back: [] };
      const kept = plan(chain, pause);
      if (!kept) return { actions: [], back: [] };
      const keep = new Set(kept.keep.flatMap((r) => r.ids.map((id) => edgeKey({ host: r.host, id }))));
      const back = pauseReturns(chain, pause).edges.filter((e) => !keep.has(edgeKey(e)));
      refuseHeld(chain, back);
      if (opts.expect) {
        const want = new Set(opts.expect.map(edgeKey));
        if (want.size !== back.length || back.some((e) => !want.has(edgeKey(e)))) {
          throw new AccountError("pauseChanged", "what ending the pause gives back has changed: look at it again");
        }
      }
      const start: Action[] = kept.keep.length ? [{ type: "renewalPause", op: next, phase: PAUSE_START, removals: kept.keep }] : [];
      return { actions: [...start, { type: "renewalPause", op: pause.op, phase, removals: [] }], back };
    };
    // The edges the batch being signed gives back: fixed when it is built (built again on a newer head, they are that build's).
    let signed: Edge[] = [];
    const wrote = await this.writePause(
      (chain) => {
        const built = settle(chain);
        signed = built.back;
        return built.actions;
      },
      (base) => {
        // Right before it goes out (H1): exactly what the SIGNED batch gives back, judged with the isolation and the hub's
        // flags known by then — not a plan made again, which would keep held an edge the signed batch returns anyway.
        refuseHeld(base, signed);
        // …and the plan still ends it at all.
        if (!settle(base).actions.length) throw new AccountError("pauseChanged", "what ending the pause gives back has changed: look at it again");
      },
    );
    if (!wrote) return null;
    const returned = signed;
    // D2: what a device only paused missed — the host's committed epochs — sealed to it now that it is let in again.
    for (const e of returned) {
      const to = this.state?.device(e.id);
      if (!to || to.admin || e.id === this.deviceId || !this.state?.machine(e.host)) continue;
      try {
        await this.sendHostKeys(e.id, e.host);
      } catch (error) {
        if (!noDeviceRoleYet(to, error)) console.warn(`cloud account: could not send ${e.id} ${e.host}'s keys after a pause (it asks again):`, error);
      }
    }
    return returned;
  }

  /**
   * 「结束暂停」 (§4.1): cancel open pause `op` — asked first, by a person; never
   * on its own. Refused while an edge it gives back must stay held (the same
   * check as a completion's), on the chain the batch is built on.
   */
  async endPause(op: Uint8Array, expect?: { host: string; id: string }[]): Promise<void> {
    const returned = await this.endPauseWith(op, PAUSE_CANCELLED, () => ({ keep: [] }), { expect });
    if (!returned) throw new AccountError("notInChain", "that pause has ended already");
  }

  /**
   * The exposed hosts that must renew their identity key before `host` does
   * (§4.1 「次序先于收窄」): those it lets control it that have not renewed
   * since. Empty when `host` is not exposed — a renewal nobody asked for is
   * never held back.
   */
  renewFirst(host: string): string[] {
    return this.renewalNotes.renewFirst(host);
  }

  /**
   * The current hosts to re-enroll for a new identity (review D3; see
   * {@link reEnrollCulprits}): those with a warning this browser was not told
   * to drop ({@link reEnrollWarnings}, {@link dismissReEnroll}), what the
   * hosts' statuses say now (`controllersOf`) taken in. A host that renews
   * its identity key instead ({@link renews}) is none of them.
   */
  reEnrollDue(controllersOf?: (host: string) => ControllersRecord | undefined): Set<string> {
    return this.reEnroll.reEnrollDue(controllersOf);
  }

  /**
   * Per current host, each device that could control it and no longer can
   * (review D3; {@link reEnrollCulprits}), with what the host's own record
   * says of it ({@link reEnrollEvidence}; `controllersOf`: its status's
   * `controllers`) and whether a person dismissed it here. The record is
   * evidence shown with the warning, never a reason to drop it: a device that
   * did connect controlled that computer and could have edited its record.
   * Only a person's "I'm sure it never connected" drops one, and not when a
   * record ever named the device (kept here, so a later record that leaves
   * it out cannot unsay it).
   *
   * Only for the hosts that cannot renew their identity key (an agent without
   * `rotate/1`, a v2 account: docs/host-identity-rotation.md §4.1, §7
   * decision 3): one that can ({@link renews}) renews whatever a record says.
   */
  reEnrollWarnings(controllersOf?: (host: string) => ControllersRecord | undefined): Map<string, ReEnrollLine[]> {
    return this.reEnroll.reEnrollWarnings(controllersOf);
  }

  /**
   * "I'm sure `device` never connected to `host` — don't warn about it" (review
   * D3), kept in this browser for the account: the warning for that loss —
   * `lostSeq`, the one the person was shown ({@link ReEnrollLine.lostSeq}) —
   * is dropped here until {@link warnAgainReEnroll}, a later loss of the
   * same device, or a record of `host`'s that names it. Nothing if `host`
   * is not due for `device`.
   */
  dismissReEnroll(host: string, device: string, lostSeq: number): void {
    this.reEnroll.dismissReEnroll(host, device, lostSeq);
  }

  /** Warn about `device` for `host` again ({@link dismissReEnroll} undone). */
  warnAgainReEnroll(host: string, device: string): void {
    this.reEnroll.warnAgainReEnroll(host, device);
  }

  /**
   * What a veto of `held.device` takes out with it, as `chain` (the hub's,
   * past the hold) stands: the device itself, and every device and machine
   * the held-back entries brought in through it — added, approved or
   * activated by it or by a device it brought in, or joined or activated with
   * the recovery words or through a passkey slot (a secret held off every
   * device, as the words are) — that is still there. What a device this view
   * already trusted signed in that stretch stands.
   *
   * Unless `opts.trusted` (issue #17, the person's choice): then also what
   * those devices — all of them but this one — brought in in that stretch
   * (`true`), or only the ones in the set (what the person was shown), and
   * in either case what those brought in in turn. Never this device, and
   * never a device this view already trusted: what the veto removes was
   * brought in only while the activation was held back here. (So the last
   * admin is never among them: this device, which signs the veto, is one.)
   *
   * `opts.view`: the devices this view trusted, taken before any sync could
   * take the activation in ({@link trustedView}); without it, this view's
   * active devices now.
   */
  vetoScope(held: HeldBack, chain: ChainState, opts: { trusted?: true | ReadonlySet<string>; view?: ReadonlySet<string> } = {}): { devices: string[]; machines: string[] } {
    const tainted = new Set<string>(held.device ? [held.device] : []);
    const machines = new Set<string>();
    const trust = opts.trusted;
    for (const e of held.entries) {
      const { signer, action } = e.draft;
      const brought = signer.type === "recovery" || signer.type === "passkey" || (signer.type === "device" && tainted.has(signer.id));
      // Signed by another device this view trusted: what it brought in goes only if asked.
      const byTrusted = !brought && trust !== undefined && signer.type === "device" && signer.id !== this.deviceId;
      if (!brought && !byTrusted) continue;
      const take = (id: string, into: Set<string>) => {
        if (brought || trust === true || (typeof trust === "object" && trust.has(id))) into.add(id);
      };
      if (action.type === "addDevice" || action.type === "recoveryJoin" || action.type === "recoveryJoin3") take(action.device.id, tainted);
      else if (action.type === "addDevice3") take(action.device.id, action.device.flags.host && !action.device.flags.admin ? machines : tainted);
      else if (action.type === "approveDevice" || action.type === "activateRecovered" || action.type === "passkeyActivate") take(action.id, tainted);
      else if (action.type === "addMachine") take(action.machine.id, machines);
    }
    // Neither this device nor one this view already trusted: only what came in while the activation was held back.
    const view = opts.view ?? this.trustedView();
    const trustedHere = (id: string) => id === this.deviceId || view.has(id);
    const devices = [...tainted].filter((id) => !trustedHere(id) && chain.deviceRecord(id) && chain.deviceRecord(id)!.status.type !== "removed");
    devices.sort((x, y) => (x === held.device ? -1 : y === held.device ? 1 : 0));
    return { devices, machines: [...machines].filter((m) => !trustedHere(m) && chain.machine(m)) };
  }

  /**
   * What vetoing the held-back activation would take out, on the hub's chain
   * as it stands now (`null` with nothing held back): for the page to list
   * before the person confirms, with the choice to take out what the devices
   * this view trusted brought in meanwhile too (`trusted`; issue #17), and
   * what it does to the recovery words.
   */
  async vetoPlan(): Promise<VetoPlan | null> {
    const held = this.held;
    if (!held?.device || !this.state) return null;
    const view = this.trustedView();
    const chain = await this.provisional(held);
    const scope = this.vetoScope(held, chain, { view });
    const wide = this.vetoScope(held, chain, { view, trusted: true });
    const inScope = new Set([...scope.devices, ...scope.machines]);
    // Who brought each one in: the first held-back entry that did.
    const by = new Map<string, string | null>();
    for (const e of held.entries) {
      const { signer, action } = e.draft;
      const subject =
        action.type === "addDevice" || action.type === "recoveryJoin" || action.type === "recoveryJoin3" || action.type === "addDevice3"
          ? action.device.id
          : action.type === "addMachine"
            ? action.machine.id
            : action.type === "approveDevice" || action.type === "activateRecovered" || action.type === "passkeyActivate"
              ? action.id
              : null;
      if (subject !== null && subject !== held.device && !by.has(subject)) by.set(subject, signer.type === "device" ? signer.id : signer.type === "passkey" ? signer.slot : null);
    }
    const target = (id: string): VetoTarget => ({ id, host: chain.deviceRecord(id)?.host === true, by: by.get(id) ?? null });
    const trusted = [...wide.devices, ...wide.machines].filter((id) => !inScope.has(id));
    const voids = (ids: Iterable<string>) => [...ids].some((id) => chain.voidsRecoveryWords(id, "removal"));
    const voidsWords = voids(inScope);
    // The passkey slots a removal turns off, as the chain rules: those its device registered, and the one it came in through.
    const slotsOf = (ids: Iterable<string>) => {
      const gone = new Set(ids);
      const through = new Set([...gone].flatMap((id) => chain.deviceRecord(id)?.viaPasskey ?? []));
      return new Set(chain.allSlots().flatMap((s) => (!s.removed && (gone.has(s.setBy) || through.has(s.slot.slot)) ? [s.slot.slot] : [])));
    };
    const slots = slotsOf(inScope);
    return {
      device: held.device,
      seq: held.seq,
      view: [...view],
      scope: [...scope.devices, ...scope.machines].map(target),
      trusted: trusted.map(target),
      voidsWords,
      trustedVoidsWords: !voidsWords && voids(trusted),
      voidsPasskeys: slots.size > 0,
      trustedVoidsPasskeys: [...slotsOf(trusted)].some((slot) => !slots.has(slot)),
    };
  }

  /** The devices this view trusts now (active in it), and this device: what a veto never removes. */
  private trustedView(): Set<string> {
    const ids = new Set((this.state?.allDevices() ?? []).filter((d) => d.status.type === "active").map((d) => d.info.id));
    ids.add(this.deviceId);
    return ids;
  }

  /** The entries held back past the activation, as the page lists them ("what approving takes in"). */
  heldChanges(): HeldChange[] {
    const held = this.held;
    if (!held) return [];
    return held.entries.map((e) => {
      const { signer, action } = e.draft;
      const subject =
        action.type === "addDevice" || action.type === "recoveryJoin" || action.type === "addDevice3" || action.type === "recoveryJoin3"
          ? action.device.id
          : action.type === "addMachine"
            ? action.machine.id
            : action.type === "removeDevice" ||
                action.type === "removeMachine" ||
                action.type === "approveDevice" ||
                action.type === "activateRecovered" ||
                action.type === "removeDevice3" ||
                action.type === "setAdmin" ||
                action.type === "setHost" ||
                action.type === "passkeyActivate"
              ? action.id
              : action.type === "setAccept"
                ? action.host
                : null;
      // A passkey slot's entries are named by the slot (as the alerts name them).
      return { seq: e.draft.seq, kind: action.type, by: signer.type === "device" ? signer.id : signer.type === "passkey" ? signer.slot : null, subject };
    });
  }

  /**
   * A new `UK` epoch and a new `MK` epoch per active machine, in this order:
   * committed in the chain (with `prefix`, e.g. a removal, in the same call);
   * the `UK` kept here; a recovery package from this device for every `UK`
   * epoch it holds; each machine's new key into the keyring; the new `UK`
   * mailed to every other active full device; the new machine keys sealed to
   * every device that holds no user key (chain v3, purpose 5); and the
   * machine keys handed to their agents. The epoch numbers are the chain's
   * next. Also "rotate keys again": run on its own, it completes a rotation
   * that died half-way.
   *
   * `opts.userKey: false` (a device that held no user key left: chain v3
   * §3.3): new machine keys only, wrapped under the newest user key this
   * device holds — unless that one is stale ({@link userKeyStale}: a device
   * that held it no longer manages the account, its rotation still due):
   * then a new user key comes with them all the same (the re-review: they
   * used to be wrapped under the key the demoted device holds).
   * `opts.machineKeys: false`: a new user key alone (the account's upgrade).
   * `opts.asV3`: written as chain v3 (the upgrade).
   */
  async rotate(
    courier: MachineKeyCourier,
    reencrypt = false,
    prefix: Action[] | ((chain: ChainState) => Action[]) = [],
    opts: { overHeld?: boolean; userKey?: boolean; machineKeys?: boolean; asV3?: boolean } = {},
  ): Promise<RotationReport> {
    if (!this.state || this.standing !== "active" || !this.pinned) throw new AccountError("notInChain", "only an active device rotates the keys");
    if (!this.isAdmin) throw new AccountError("notAdmin", "only a full device of the account rotates the keys");
    const withMachineKeys = opts.machineKeys !== false;
    try {
      await this.sync();
    } catch (error) {
      console.warn("cloud account: sync before rotating:", error);
    }
    try {
      await this.keyring.refresh();
    } catch {
      // the cached copy
    }
    // Machine keys never go under a stale user key: a new one comes with them then.
    const withUserKey = opts.userKey !== false || userKeyStale(this.state);
    // Machine keys alone are wrapped under the newest user key, which this device must hold — known before anything is committed.
    const wrapEpoch = withUserKey ? null : await this.ensureUserKey();
    if (!withUserKey && wrapEpoch === null) throw this.noUserKey();
    let uk: Bytes | null = null;
    let mks: { machine: string; mk: Bytes }[] = [];
    const zero = () => {
      uk?.fill(0);
      for (const m of mks) m.mk.fill(0);
    };
    let vetoed: RotationReport["vetoed"];
    try {
      const entries = await this.appendBatch(async (chain) => {
        zero();
        const before = typeof prefix === "function" ? prefix(chain) : prefix;
        const removed = before.flatMap((a) => (a.type === "removeDevice" || a.type === "removeMachine" || a.type === "removeDevice3" ? [a.id] : []));
        // The hosts the batch takes out get no new key.
        const gone = new Set(removed.filter((id) => chain.deviceRecord(id)?.host));
        vetoed = opts.overHeld ? { devices: removed.filter((id) => !gone.has(id)).slice(1), machines: [...gone] } : undefined;
        // On a newer head too: a new user key if the newest went stale meanwhile, or is not the one checked above.
        const fresh = withUserKey || userKeyStale(chain) || chain.latestUserEpoch() !== wrapEpoch ? random32() : null;
        uk = fresh;
        const ukEpoch = chain.userKeyEpochs();
        mks = withMachineKeys
          ? chain
              .hosts()
              .filter((m) => m.status.type === "active" && !gone.has(m.info.id))
              .map((m) => ({ machine: m.info.id, mk: random32() }))
          : [];
        const commits: MachineKeyCommit[] = [];
        for (const { machine, mk } of mks) {
          const epoch = chain.machineKeyEpochs(machine);
          commits.push({ machine, epoch, hash: await machineKeyCommit(this.account, machine, epoch, mk) });
        }
        const keyEpochs: Action[] = [];
        const user = fresh ? { epoch: ukEpoch, hash: await userKeyCommit(this.account, ukEpoch, fresh) } : null;
        if (user || commits.length) keyEpochs.push({ type: "keyEpochs", user, machines: commits.slice(0, MAX_KEY_COMMITS) });
        for (let i = MAX_KEY_COMMITS; i < commits.length; i += MAX_KEY_COMMITS) keyEpochs.push({ type: "keyEpochs", user: null, machines: commits.slice(i, i + MAX_KEY_COMMITS) });
        return [...before, ...keyEpochs];
      }, { overHeld: opts.overHeld, asV3: opts.asV3 });
      const committed = entries.flatMap((e) => (e.draft.action.type === "keyEpochs" ? [e.draft.action] : []));
      const ukEpoch = committed.find((a) => a.user)?.user?.epoch ?? null;
      const mkEpochs = new Map(committed.flatMap((a) => a.machines.map((m) => [m.machine, m.epoch] as const)));
      const wrapUnder = ukEpoch ?? wrapEpoch!;
      const newUk = uk as Bytes | null;
      // Committed: now the keys may be used.
      const recovery = ukEpoch !== null && newUk ? await this.keepUserKey(ukEpoch, newUk) : false;
      const prepared: { machine: string; mkEpoch: number; mk: Bytes; keyring?: string }[] = [];
      const userKey = await this.vault.key(wrapUnder);
      for (const { machine, mk } of mks) {
        const mkEpoch = mkEpochs.get(machine)!;
        try {
          await this.keyring.put(keyName.machine(machine, mkEpoch, wrapUnder), await wrapMachineKey(userKey, this.account, machine, mkEpoch, wrapUnder, mk));
          prepared.push({ machine, mkEpoch, mk });
        } catch (error) {
          // Said loudly, and handed over anyway (M2 re-review N2: skipping the
          // hand-over left the agent sealing under the key the removed device
          // holds — exactly what a full keyring was for).
          const why = error instanceof HubApiError ? error.code : error instanceof Error ? error.message : String(error);
          console.error(`cloud account: the keyring did not take ${machine}'s new key (${why}); handing it over all the same`);
          prepared.push({ machine, mkEpoch, mk, keyring: why || "the keyring did not take its key" });
        }
      }
      // Every older machine-key wrap this device can read, again under the
      // new user key and its own name (relay-and-plans §6) — before the new
      // epoch is mailed, so a device that reads the keyring when the mail
      // lands finds them all. The hub then reclaims the wraps they supersede.
      let rewrapped: RotationReport["rewrapped"];
      if (ukEpoch !== null) {
        try {
          const machines = this.state
            .hosts()
            .filter((m) => m.status.type === "active")
            .map((m) => ({ id: m.info.id, epochs: this.state!.machineKeyEpochs(m.info.id) }));
          const { rewrapped: done, failed } = await rewrapMachineKeys(this.vault, this.keyring, this.account, machines, ukEpoch, (m) => this.machineCheck(m));
          if (failed.length) console.warn("cloud account: older machine keys not rewrapped under the new user key:", failed);
          rewrapped = { done, failed };
        } catch (error) {
          console.warn("cloud account: rewrapping older machine keys:", error);
        }
      }
      this.emit();
      const { sealedTo, failedTo, failedWhy } = ukEpoch !== null && newUk ? await this.mailUserKey(ukEpoch, newUk) : { sealedTo: [], failedTo: [], failedWhy: {} };
      // The devices that hold no user key read the hosts with these (chain v3; none on a v2 chain).
      const hostKeysTo = new Set<string>();
      const hostKeysFailed: Record<string, string> = {};
      const noRole = new Set<string>();
      for (const p of prepared) {
        const { sent, failed } = await this.mailHostKey(p.machine, p.mkEpoch, p.mk, noRole);
        for (const id of sent) hostKeysTo.add(id);
        Object.assign(hostKeysFailed, failed);
      }
      for (const id of Object.keys(hostKeysFailed)) hostKeysTo.delete(id);
      const machines = await Promise.all(
        prepared.map(async (p) => {
          const keyring = p.keyring !== undefined ? { keyring: p.keyring } : {};
          try {
            return { machine: p.machine, mkEpoch: p.mkEpoch, result: await this.handOver(p.machine, p.mkEpoch, wrapUnder, p.mk, courier, reencrypt), ...keyring };
          } catch (error) {
            return { machine: p.machine, mkEpoch: p.mkEpoch, error: error instanceof Error ? error.message : String(error), ...keyring };
          }
        }),
      );
      this.emit();
      const hostKeys = hostKeysTo.size || Object.keys(hostKeysFailed).length ? { hostKeysTo: [...hostKeysTo], hostKeysFailed } : {};
      return { ukEpoch, sealedTo, failedTo, failedWhy, recovery, machines, ...hostKeys, ...(vetoed ? { vetoed } : {}), ...(rewrapped ? { rewrapped } : {}) };
    } finally {
      zero();
    }
  }

  /**
   * What upgrading the account to chain v3 writes (review D4), for the page
   * to show before the person confirms: one entry, the account's first v3
   * one — a new user key epoch (`keyEpochs`, version 3). `null` when the
   * chain holds a v3 entry already (or there is no chain).
   */
  upgradePlan(): { version: 3; ukEpoch: number; machines: 0 } | null {
    const chain = this.state;
    if (!chain || chain.isV3) return null;
    return { version: 3, ukEpoch: chain.userKeyEpochs(), machines: 0 };
  }

  /**
   * Upgrade the account to chain v3 (review D4, local-aggregator §3.5): a
   * step of its own that the person confirms, only from a device that
   * manages the account and only while the hub takes v3 entries
   * (`opts.writable`). Its first v3 entry is a rotation of the user key
   * written as v3 — harmless, and useful: a new user key for every device
   * that manages the account, sealed for the recovery words, the older
   * machine keys wrapped under it. After it, nothing but v3 is written, and
   * apps and agents that read only chain v2 cannot follow the account.
   */
  async upgradeToV3(opts: { writable?: boolean }): Promise<RotationReport> {
    const chain = this.requireAdmin();
    if (chain.isV3) throw new AccountError("invalidEntry", "the account is on chain v3 already", undefined, "upgraded");
    this.requireWritable(opts.writable);
    // No machine key changes: nothing is handed to an agent.
    return this.rotate(NO_COURIER, false, [], { machineKeys: false, asV3: true });
  }

  /**
   * Every `UK` epoch this device holds — after taking any waiting in its
   * mailbox, so what it hands on is complete — as bytes for the length of
   * `use` (a pairing delivery, an approved device's keys).
   */
  async withUserKeys<T>(use: (keys: { epoch: number; key: Bytes }[]) => Promise<T>): Promise<T> {
    this.assertNotForked();
    try {
      await this.receiveMail();
    } catch (error) {
      console.warn("cloud account: mail:", error);
    }
    this.assertNotForked();
    // And again once the keys are read, right before they are handed over.
    return this.vault.withAllRaw((keys) => {
      this.assertNotForked();
      return use(keys);
    });
  }

  /**
   * Look for keys again now (a stream met a record under an epoch this
   * device cannot open): the mail, then the keyring; listeners are told.
   */
  async refreshKeys(): Promise<void> {
    if (this.standing !== "active" || !this.pinned) return;
    try {
      await this.receiveMail();
    } catch (error) {
      console.warn("cloud account: mail:", error);
    }
    await this.keyring.refresh();
    // A device that holds no user key asks an admin for the hosts' keys it lacks (rate-limited).
    if (!this.isAdmin) {
      try {
        await this.checkHostKeys();
      } catch (error) {
        console.warn("cloud account: host keys:", error);
      }
    }
    this.emit();
  }

  /** Approve a device that joined with the recovery key, and mail it every `UK` epoch held (it may have joined before the newest). */
  async approveDevice(id: string): Promise<void> {
    const held = this.held;
    if (held && held.device === id) return this.vouchForHeld(held);
    const record = this.state?.deviceRecord(id);
    if (record?.status.type !== "pending") throw new AccountError("notPending", `${id} is not waiting for approval`);
    // One that joined with a passkey is approved only by pairing with it (`pairDevice`): its id alone says nothing of whose screen it is.
    if (record.viaPasskey !== null) throw new AccountError("notPending", "{{id}} joined with a passkey: approve it by scanning its QR code or typing its code in my devices → add a browser or phone", undefined, undefined, { id });
    await this.append({ type: "approveDevice", id });
    // A user key only to a device that manages the account (a passkey's joins do not: chain v3 §3.2).
    if (record.admin) await this.withUserKeys((keys) => this.mailUserKeys(id, record.info.dhKey, keys));
    else if (!record.host) await this.sendHostKeys(id);
  }

  /**
   * "That was me" for a held-back self-activation: take the hub's chain past
   * it now, on the user's word — unless that chain no longer has this device
   * (then approving would only be a way to wipe it).
   */
  private async vouchForHeld(held: HeldBack): Promise<void> {
    const chain = await this.provisional(held);
    if (chain.device(this.deviceId) === undefined) {
      throw new AccountError("heldBack", "the hub's chain goes on to remove this device: remove the recovered device from another device instead");
    }
    await this.adopt(chain, []);
  }

  /**
   * Make new recovery words: the chain's recovery key replaced
   * (`setRecoveryKey`, with `RSK`'s proof over the account's root), and every
   * `UK` epoch held sealed to it. Returns the words to show once.
   */
  async setRecoveryWords(): Promise<string[]> {
    if (this.standing !== "active" || !this.state || !this.pinned) throw new AccountError("notInChain", "only an active device sets the recovery key");
    const rk = crypto.getRandomValues(new Uint8Array(16));
    let words: string[];
    let key;
    try {
      words = await mnemonicFromEntropy(rk);
      key = await recoveryKeyFor(await recoveryKeys(rk), this.account, this.state.accountKey);
    } finally {
      rk.fill(0);
    }
    await this.append({ type: "setRecoveryKey", key });
    await this.withUserKeys(async (keys) => {
      for (const { epoch, key: raw } of keys) await this.sealForRecovery(epoch, raw);
    });
    return words;
  }

  /** `RSK` and the recovery seal key from what the user typed, checked against the chain's recovery key. */
  private async recoveryFrom(phrase: string) {
    await this.sync();
    const chain = this.state;
    if (!chain) throw new AccountError("notInChain", "the account has no chain yet");
    const rsk = chain.currentRecoveryKey;
    if (!rsk) throw new AccountError("noRecoveryKey", "this account has no recovery key (any more)");
    const rk = await recoverySecret(phrase);
    try {
      const keys = await recoveryKeys(rk);
      if (!equal(keys.signingPublic, rsk)) throw new AccountError("wrongWords", "these are not this account's current recovery words");
      return { chain, keys };
    } finally {
      rk.fill(0);
    }
  }

  /**
   * Open every recovery package for an epoch not held yet, whoever sealed it
   * — a current admin, or one demoted or removed since (that one may be
   * the only package for its epoch) — each only if it is the key the chain
   * committed (`openRecoveryPackage`); returns how many were.
   */
  private async openRecoveryPackages(chain: ChainState, seal: NoiseKeyPair): Promise<number> {
    await this.keyring.refresh();
    let opened = 0;
    for (const p of this.keyring.recoveryBlobs().sort((a, b) => b.updatedMs - a.updatedMs)) {
      if (await this.vault.has(p.ukEpoch)) continue;
      try {
        const raw = await openRecoveryPackage(seal, chain, { purpose: "userKey", account: this.account, sender: p.sender, recipient: RECOVERY_RECIPIENT, epoch: p.ukEpoch }, p.blob);
        try {
          if (await this.vault.put(p.ukEpoch, raw)) opened++;
        } finally {
          raw.fill(0);
        }
      } catch {
        // under an older recovery key, from a device the chain never named, not the committed key, or spoiled
      }
    }
    return opened;
  }

  /**
   * Come back with the recovery words, on a browser not in the chain: check
   * the words' `RSK` is the chain's current recovery key (the chain proves
   * `RSK` signed this account's root, so a hub cannot make up a chain around
   * it) — and only then pin the account — open the recovery packages (the
   * account's `UK` epochs), then join with `RSK`. The device is then
   * *pending*: another full device approves it, or it activates itself once
   * {@link RECOVERY_DELAY_MS} have passed.
   */
  async recover(phrase: string): Promise<{ epochs: number[] }> {
    if (this.standing !== "absent") throw new AccountError("notInChain", "this device is already in the chain");
    const { chain, keys } = await this.recoveryFrom(phrase);
    const pinned = await this.pinnedGenesis();
    if (pinned && pinned !== hex(chain.hashAt(0)!)) throw new AccountError("fork", "the hub served a different genesis than this device pinned");
    await this.pin();
    await this.openRecoveryPackages(chain, keys.seal);
    const epochs = await this.vault.epochs();
    if (!epochs.length) throw new AccountError("noPackages", "no recovery package opens with these words");
    // A v3 chain takes v3 joins only: an admin, never a host (review D1, D4).
    const join: Action = chain.isV3
      ? { type: "recoveryJoin3", device: { id: this.deviceId, signKey: this.keys.signPublic, dhKey: this.keys.dhPublic, flags: { host: false, admin: true } } }
      : { type: "recoveryJoin", device: this.info() };
    const entry = await signDraft(chain.draft({ type: "recovery" }, join, Date.now()), keys.signing);
    // Refused here, by the chain's own rule, before it is posted (review 13).
    await verifyBatch(chain, [entry]);
    this.assertNotForked();
    await this.api.appendChain(this.session.token, [encodeEntry(entry)]);
    await this.landed([entry]);
    this.write(joinedKey(this.account, this.deviceId), String(Date.now()));
    return { epochs };
  }

  /** When this pending device may activate itself (by its own first sighting of its join), or `null` if not pending. */
  recoveryReadyAt(): number | null {
    const record = this.state?.deviceRecord(this.deviceId);
    if (record?.status.type !== "pending") return null;
    const seen = this.clock.firstSeen.get(record.status.joinSeq);
    if (seen !== undefined) return seen + RECOVERY_DELAY_MS;
    const joined = Number(this.read(joinedKey(this.account, this.deviceId)));
    if (Number.isFinite(joined) && joined > 0) return joined + RECOVERY_DELAY_MS;
    return (this.state!.entryAt(record.status.joinSeq)?.draft.createdMs ?? Date.now()) + RECOVERY_DELAY_MS;
  }

  /** Activate this pending device with `RSK` (the words again), once the delay has passed. */
  async activateRecovered(phrase: string, now = Date.now()): Promise<void> {
    if (this.standing !== "pending") throw new AccountError("notPending", "this device is not waiting for activation");
    const readyAt = this.recoveryReadyAt()!;
    if (now < readyAt) throw new AccountError("tooEarly", "too early to activate this device", readyAt);
    const { chain, keys } = await this.recoveryFrom(phrase);
    if (this.standing !== "pending") return; // approved meanwhile
    const entry = await signDraft(chain.draft({ type: "recovery" }, { type: "activateRecovered", id: this.deviceId }, now), keys.signing);
    // Refused here, by the chain's own rule, before it is posted (review 13). (The delay is this device's
    // own clock's, checked above, and the hub's: the check here takes the activation as history.)
    await verifyBatch(chain, [entry]);
    try {
      this.assertNotForked();
      await this.api.appendChain(this.session.token, [encodeEntry(entry)]);
    } catch (error) {
      // The hub keeps its own first sighting of the join (`ChainError::TooEarly`'s text).
      const ready = error instanceof HubApiError ? /activate itself only from (\d+)/.exec(error.message) : null;
      if (ready) throw new AccountError("tooEarly", error instanceof Error ? error.message : "too early", Number(ready[1]));
      throw error;
    }
    await this.landed([entry], now);
    // Epochs made while this device waited were sealed to the recovery key too.
    await this.openRecoveryPackages(chain, keys.seal);
    this.forget(joinedKey(this.account, this.deviceId));
    this.emit();
  }

  // ------------------------------------------------------------ joining with a passkey (docs/passkeys.md §4)

  /** The chain's current passkey slots: the passkeys a new device can join with. */
  passkeySlots(): SlotRecord[] {
    return this.state?.allSlots().filter((s) => !s.removed) ?? [];
  }

  /** The current slot naming the passkey whose credential id hashes to `credHash` (hex), if any. */
  passkeySlotFor(credHash: string): SlotRecord | null {
    return this.passkeySlots().find((s) => hex(s.slot.credHash) === credHash) ?? null;
  }

  /**
   * What letting new devices join with a passkey will use (§4.2): a fresh
   * slot id and salt, and the PRF input they make — asked of the passkey at
   * its creation (one gesture, §4.5) or right after ({@link enablePasskeyJoin}).
   */
  passkeyJoinPlan(): { slot: string; saltNonce: Bytes; prfInput: Bytes } {
    const named = new Set(this.state?.allSlots().map((s) => s.slot.slot) ?? []);
    let slot = newSlotId();
    while (named.has(slot)) slot = newSlotId();
    const saltNonce = crypto.getRandomValues(new Uint8Array(32));
    return { slot, saltNonce, prfInput: prfInput(saltNonce) };
  }

  /**
   * Let new devices join the account with the passkey `credentialId` (§4.5):
   * from its PRF output for `plan`'s input, `PAK` and its proof over the
   * account's root — and a `passkeySlotSet` naming it. The slot grants
   * nothing (`grants: 0`: the chain lets a passkey add only a plain, pending
   * device) and lets none activate itself (`selfActivate: false`): a device
   * that joins with it waits until a full device approves it by pairing
   * with it (its QR code or 8-digit code), or removes it. `prf` is zeroed
   * here. A full device, on a v3 chain.
   */
  async enablePasskeyJoin(credentialId: Uint8Array, plan: { slot: string; saltNonce: Uint8Array }, prf: Uint8Array | null): Promise<void> {
    try {
      const chain = this.requireAdmin();
      this.requireUpgraded(chain);
      if (!prf) throw new AccountError("noPrf", "new devices cannot join with this passkey in this browser (some browsers and password managers do not support it): try another browser, or another passkey");
      const credHash = await credentialHash(credentialId);
      const current = chain.allSlots().filter((s) => !s.removed);
      if (current.some((s) => equal(s.slot.credHash, credHash))) return;
      if (current.length >= MAX_PASSKEY_SLOTS) throw new AccountError("tooManySlots", "new devices can join with at most {{n}} passkeys: turn one off first", undefined, undefined, { n: String(MAX_PASSKEY_SLOTS) });
      const { signing, pak } = await passkeyAuthority(prf, this.account, plan.slot);
      const proof = await passkeySlotProof(signing, pak, this.account, chain.accountKey, plan.slot, credHash, plan.saltNonce);
      await this.append({ type: "passkeySlotSet", slot: { slot: plan.slot, key: pak, proof, credHash, saltNonce: copy(plan.saltNonce), grants: 0, selfActivate: false } });
    } finally {
      prf?.fill(0);
    }
  }

  /** Stop new devices joining with slot `slot` (`passkeySlotRemove`). A device waiting after joining with it still waits, to be approved or removed. */
  async disablePasskeyJoin(slot: string): Promise<void> {
    const chain = this.requireAdmin();
    if (!chain.slot(slot)) return;
    await this.append({ type: "passkeySlotRemove", slot });
  }

  /**
   * Join the account with a passkey, on a browser not in its chain (§4.6):
   * `prf` is the passkey's PRF output for slot `slot`'s input (the passkey
   * `credentialId` — the slot must name it). Before anything is trusted:
   * the chain (verified from its genesis as it was read) has that slot, the
   * `PAK` the PRF gives is the slot's, and the slot's proof holds over this
   * chain's root — so a hub that made up a chain around the real slot is
   * caught (it cannot evaluate the PRF). Only then is the account pinned,
   * and this device added, *pending*, by `PAK` (`addDevice3` with no flags).
   * It then shows the pairing offer (QR code, 8-digit code) a full device
   * approves it by ({@link pairDevice}), or a device removes it. `prf` is
   * zeroed here.
   */
  async joinWithPasskey(slot: string, credentialId: Uint8Array, prf: Uint8Array): Promise<void> {
    try {
      if (this.standing !== "absent") throw new AccountError("notInChain", "this device is already in the chain");
      await this.sync();
      const chain = this.state;
      if (!chain) throw new AccountError("notInChain", "the account has no chain yet");
      const record = chain.slot(slot);
      const credHash = await credentialHash(credentialId);
      if (!record || !equal(record.slot.credHash, credHash)) {
        throw new AccountError("noPasskeySlot", "new devices cannot join with this passkey: on a device already in the account, let them in account settings, or pair this device instead");
      }
      const { signing, pak } = await passkeyAuthority(prf, this.account, slot);
      if (!equal(pak, record.slot.key) || !(await verifyPasskeySlot(record.slot, this.account, chain.accountKey))) {
        throw new AccountError("wrongPasskey", "this passkey does not open this account here: pair this device from another one instead");
      }
      const pinned = await this.pinnedGenesis();
      if (pinned && pinned !== hex(chain.hashAt(0)!)) throw new AccountError("fork", "the hub served a different genesis than this device pinned");
      await this.pin();
      const join: Action = { type: "addDevice3", device: { id: this.deviceId, signKey: this.keys.signPublic, dhKey: this.keys.dhPublic, flags: { host: false, admin: false } } };
      const entry = await signDraft(chain.draft({ type: "passkey", slot }, join, Date.now()), signing);
      // Refused here, by the chain's own rule, before it is posted.
      await verifyBatch(chain, [entry]);
      this.assertNotForked();
      await this.api.appendChain(this.session.token, [encodeEntry(entry)]);
      await this.landed([entry]);
    } finally {
      prf.fill(0);
    }
  }

  /**
   * Approve device `c.device`, waiting after joining with a passkey, by the
   * pairing the person ran with it (its QR code or its 8-digit code:
   * `pairing.ts`) — never by its id alone (§4.6 5a as built: the PAKE run or
   * the offer's secret bound the keys `c` names to the screen the person
   * looked at). Only if the chain, as it stands when this device signs,
   * still lists that device as waiting with exactly those keys: else nothing
   * is signed (`joinReplaced`: a newer join with the same passkey replaced
   * it, or a device removed it; `joinMismatch`: the pairing's keys are not
   * the join's). It becomes a full device in the same batch
   * (`approveDevice`, `setAdmin`): every browser or phone of the account is
   * one (local-aggregator §3.1, 2026-10-20) — the passkey alone never made
   * it one, the person approving does. The keys follow in the pairing's
   * delivery ({@link pairDevice}).
   */
  private async approvePasskeyJoin(c: Candidate): Promise<void> {
    this.requireUpgraded(this.requireAdmin());
    await this.appendBatch(async (chain) => {
      const record = chain.deviceRecord(c.device);
      if (!record || record.viaPasskey === null) throw new AccountError("notPending", `${c.device} is not waiting to join with a passkey`);
      if (record.status.type !== "pending") {
        throw new AccountError("joinReplaced", "{{id}} no longer waits to join: a newer join with the same passkey replaced it, or a device removed it. Nothing was approved.", undefined, undefined, { id: c.device });
      }
      if (!equal(record.info.signKey, c.signKey) || !equal(record.info.dhKey, c.dhKey)) {
        throw new AccountError("joinMismatch", "the device that showed this code is not the one that asked to join as {{id}}: nothing was approved", undefined, undefined, { id: c.device });
      }
      return [
        { type: "approveDevice", id: c.device },
        { type: "setAdmin", id: c.device, admin: true },
      ];
    });
  }

  /**
   * Keep the `UK` epochs a pairing delivered — the delivery verified with
   * this screen's secret against the chain as it stands — each checked
   * against the chain's commitment; and pin the account (the delivery vouched
   * for it). The caller zeroes the keys.
   */
  async storeUserKeys(keys: { epoch: number; key: Uint8Array }[]): Promise<void> {
    const chain = this.state;
    if (!chain) throw new AccountError("notInChain", "the account has no chain yet");
    for (const { epoch, key } of keys) await chain.checkUserKey(epoch, key);
    const pinned = await this.pinnedGenesis();
    if (pinned && pinned !== hex(chain.hashAt(0)!)) throw new AccountError("fork", "the hub served a different genesis than this device pinned");
    await this.pin();
    for (const { epoch, key } of keys) await this.vault.put(epoch, key);
    this.emit();
  }

  /**
   * This device was removed from the account (or signs out for good): forget
   * everything it holds for it — `UK` epochs, the cached keyring, M1's raw
   * machine keys, its device keys and the local wrap key, its id at the hub,
   * and what it noted about the chain. (The page clears its offline copies
   * too.) Every step is tried, whatever an earlier one did; what failed is
   * thrown at the end, each step on its own ({@link WipeError}).
   */
  async wipe(): Promise<void> {
    this.stopSession();
    const hosts = (this.state?.hosts() ?? []).map((m) => m.info.id);
    await forgetBrowser({
      store: this.store,
      storage: this.storage,
      account: this.account,
      deviceId: this.deviceId,
      vaults: [this.vault, this.hostKeys, this.renewalStash, this.keyring],
      legacyHosts: hosts,
      forgetSeen: () => this.forgetSeen(),
    });
  }

  /**
   * {@link wipe} for a page that could not open the account (its chain
   * forked, its keys would not open, an error): what this browser keeps for
   * `account`, under the device id it noted, forgotten from `store` and the
   * browser's storage, step by step. Its device keys go with it: the next
   * sign-in here is a new device. Raw machine keys from before the keyring
   * go too, chain or none: every one the key store lists.
   */
  static async wipeLocal(store: KeyStore, account: string, opts: { storage?: AccountStorage; seen?: SeenStore } = {}): Promise<void> {
    const storage = opts.storage ?? defaultStorage();
    const deviceId = readFrom(storage, deviceKey(account));
    const seen = opts.seen ?? pageSeenStore(storage);
    const keyring = new Keyring({ list: async () => [], put: async () => undefined }, store, account, deviceId ?? "");
    await forgetBrowser({
      store,
      storage,
      account,
      deviceId,
      vaults: [new UserKeyVault(store, account), new HostKeyVault(store, account), new RenewalStash(store, account), keyring],
      legacyHosts: [],
      forgetSeen: async () => {
        await seen.update(account, (raw) => {
          const stored = parseStored(raw);
          return stored.seen === null ? undefined : { ...stored, seen: null };
        });
        forgetIn(storage, seenKey(account));
      },
    });
  }

  /** This device's signed statement of the chain head it sees now. */
  async attestation(): Promise<Attestation> {
    this.assertNotForked();
    if (!this.state) throw new AccountError("notInChain", "the account has no chain yet");
    // A page behind what this browser verified vouches for nothing: its word,
    // freshly dated, for a head before (say) a removal is what a hub keeping
    // the newer entries from it would need. It catches up first; a hub that
    // will not let it is a fork. Whether the account is pinned is read from
    // the store here, never taken from `pinned`: that is what this page saw
    // last, and another page may have pinned the account since. Under
    // another pin, or contradicting what this browser verified where they
    // overlap, this view is a fork. Nor does a page vouch for a head it has
    // since verified past, only not taken (its record would not write).
    const held = await this.readHeld();
    const genesis = hex(this.state.hashAt(0)!);
    if (held.genesis !== null && held.genesis !== genesis) throw this.latchFork("this browser pinned another genesis for the account than this page's chain starts with");
    if (held.genesis !== null) this.heldAgainst(this.state, held);
    const recorded = held.genesis !== null ? Math.max(-1, ...checkpoints(held).map((mark) => mark.seenThrough)) : -1;
    if (recorded > this.state.seq || this.unrecordedThrough > this.state.seq) {
      await this.sync();
      if (this.state.seq < recorded) {
        if (this.held) throw new AccountError("heldBack", "this page holds back entries another page of this browser took: it vouches for nothing until it takes them");
        throw this.latchFork(`the hub serves this page the account's chain through entry ${this.state.seq}, and another page of this browser verified it through ${recorded}`);
      }
      if (this.state.seq < this.unrecordedThrough) {
        throw new SeenStoreError(`this page verified the account's chain through entry ${this.unrecordedThrough} and could not record it: it vouches for nothing until it takes that — try again`);
      }
    }
    return createAttestation(this.state, this.deviceId, this.keys.sign.privateKey, Date.now());
  }

  /** The hello's view and whether a machine is current, for the channel. */
  machine(id: string): MachineInfo | undefined {
    return this.state?.machine(id)?.info;
  }
}
