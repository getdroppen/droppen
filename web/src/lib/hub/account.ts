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
 *   packages or the keyring — must be the one the chain committed.
 * - **Pinning & first sightings.** Handled by `account/chain-trust.ts` and
 *   `account/chain-sync.ts`; alerts by `account/alerts.ts`; held-back
 *   self-activations by `account/held-back.ts`; genesis & recovery by
 *   `account/recovery.ts`; enrollment, delivery & mail by
 *   `account/enrollment.ts`; chain v3 host keys by `account/host-keys.ts`;
 *   user keys & keyring reads by `account/user-keys.ts`; membership &
 *   rotations by `account/membership.ts`; identity key renewals & pauses by
 *   `account/renewal-flow.ts` and `account/renewal-notes.ts`.
 */
import {
  type Accept,
  type Action,
  type Assurance,
  type Attestation,
  type Bytes,
  ChainClock,
  type ChainState,
  copy,
  credentialHash,
  type DeviceInfo,
  type DeviceKeys,
  type DeviceRecord,
  encodeEntry,
  type EnrollCode,
  type Entry,
  equal,
  fingerprint,
  hex,
  type HostRotation,
  type KeyStore,
  type MachineInfo,
  type MachineKey,
  MAX_PASSKEY_SLOTS,
  newSlotId,
  type NoiseKeyPair,
  type OpenPause,
  passkeyAuthority,
  passkeySlotProof,
  PAUSE_CANCELLED,
  PAUSE_COMPLETED,
  type PauseRemoval,
  prfInput,
  signDraft,
  type SlotRecord,
  verifyPasskeySlot,
} from "@/lib/e2e";
import { type HostRotationView, type HubApi, type HubSession } from "./api";
import { hubOrigin } from "./config";
import type { DeliveryResult, MachineKeyCourier, PendingReencrypt, RenewalCourier } from "./deliver";
import { HostKeyVault, Keyring, RenewalStash, UserKeyVault } from "./keyring";
import type { Candidate } from "./pairing";
import {
  type AcceptChange,
  type Exposure,
  heldUserKey,
  type KeptDispute,
  NO_COURIER,
  type PauseEnding,
  type ReEnrollLine,
  type RenewalNote,
  rotationDueIn,
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
import { type AckRecord, genesisKey, pageSeenStore, parseStored, seenKey, type SeenStore } from "./seen-store";
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
  forgetIn,
  hostKeyRequestKey,
  hostKeysSentKey,
  joinedKey,
  keptDisputesKey,
  keyRequestKey,
  localAssuredKey,
  readFrom,
  reEnrollConnectedKey,
  reEnrollDismissedKey,
  type SeenWatch,
  writeTo,
} from "./account-storage";
export type { AccountLocks, AccountStorage, CrossTab, SeenWatch } from "./account-storage";
import type { ControllersRecord } from "./streams";
import { ChainAlerts } from "./account/alerts";
import { type AccountNote, accountBasics } from "./account/basics";
import { ChainSync } from "./account/chain-sync";
import { ChainTrust } from "./account/chain-trust";
import { type Refusal, type SelfRemoval, type Standing } from "./account/devices";
import { HostEnrollment } from "./account/enrollment";
import { AccountError, notUpgraded } from "./account/errors";
import { ActivationHold } from "./account/held-back";
import { HostKeys } from "./account/host-keys";
import { HOST_KEYS_WAIT } from "./account/limits";
import { AccountMembership } from "./account/membership";
import { ReEnrollWarnings } from "./account/re-enroll";
import { AccountRecovery } from "./account/recovery";
import type { RenewalReport, RenewOptions } from "./account/renewal";
import { RenewalFlow } from "./account/renewal-flow";
import { RenewalNotes } from "./account/renewal-notes";
import { AccountSession, openAccount } from "./account/session";
import { AccountUserKeys } from "./account/user-keys";
import type { ChainAlert, FreshUserKey, HeldBack, HeldChange, RotationReport, VetoPlan } from "./account/views";
import { forgetBrowser } from "./account/wipe";

export { deviceFingerprint, hasRunAi, joinAction, limitedDevice } from "./account/devices";
export type { JoinKeys, PromoteBlock, Refusal, SelfRemoval, Standing } from "./account/devices";
export { requestedFirst } from "./account/enrollment";
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
  /** The chain's newest user key epoch when this device does not hold it ("missing key"); kept by `checkUserKey`. */
  missingUserKey: number | null = null;
  /** (Not an admin.) The current hosts whose newest committed key epoch this device cannot open; kept by `checkHostKeys`. */
  missingHostKeys: string[] = [];
  /** Stop hearing the other tabs ({@link stopSession}). */
  private unhearCrossTab: (() => void) | null = null;
  private readonly crossTab: CrossTab | null;
  /** What `HubApi.renew` is set to for this account (`open` sets it). */
  readonly renewer = (stale: string): Promise<string | null> => this.renewAfter401(stale);
  private readonly storage: AccountStorage;
  private readonly locks: AccountLocks | null;
  /** The re-enroll warnings (review D3), kept for the account in this browser. */
  private readonly reEnroll: ReEnrollWarnings;
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
  /** A self-activation this device holds back, and what settles it (`ActivationHold`). */
  private readonly hold = new ActivationHold(this.basics, {
    sync: () => this.sync(),
    adopt: (chain, mine) => this.chainSync.adopt(chain, mine),
    latchFork: (message) => this.trust.latchFork(message),
    keepFresh: () => this.sessionFlow.keepFresh,
  });
  /** What this browser trusts of the account's chain: the pinned genesis, the seen record and the fork latch (`ChainTrust`). */
  private readonly trust: ChainTrust;
  private readonly sessionFlow: AccountSession;
  private readonly chainSync: ChainSync;
  private readonly userKeysFlow: AccountUserKeys;
  private readonly recoveryFlow: AccountRecovery;
  private readonly hostKeysFlow: HostKeys;
  private readonly enrollmentFlow: HostEnrollment;
  private readonly membershipFlow: AccountMembership;
  private readonly renewalFlow: RenewalFlow;

  private constructor(
    readonly api: HubApi,
    readonly session: HubSession,
    readonly deviceId: string,
    readonly keys: DeviceKeys,
    private state: ChainState | null,
    readonly store: KeyStore,
    opts: AccountOptions,
  ) {
    const origin = opts.origin ?? hubOrigin() ?? "";
    this.storage = opts.storage ?? defaultStorage();
    const seen = opts.seen ?? pageSeenStore(this.storage);
    this.locks = opts.locks === undefined ? defaultLocks() : opts.locks;
    this.trust = new ChainTrust(
      this.basics,
      {
        genesisNote: this.note(genesisKey(this.account)),
        seenNote: this.note(seenKey(this.account)),
        readAck: () => this.readAck(),
        writeAck: (ack) => this.writeAck(ack),
        standing: () => this.standing,
      },
      seen,
      this.locks,
    );
    this.sessionFlow = new AccountSession(this.basics, {
      api,
      session,
      keys,
      origin,
      deviceSignNote: this.note(deviceSignKey(this.account)),
      onSession: opts.onSession,
      getRefusal: () => this.refusal,
      setRefusalValue: (r) => {
        this.refusal = r;
      },
      onKeepFresh: () => {
        this.hold.scheduleHeld();
        this.trust.keepWatching();
      },
      onStop: () => {
        this.hold.stopTimer();
        this.trust.stopWatching();
        this.unhearCrossTab?.();
        this.unhearCrossTab = null;
        if (this.api.renew === this.renewer) this.api.renew = null;
      },
    });
    this.crossTab = opts.crossTab === undefined ? defaultCrossTab(this.storage) : opts.crossTab;
    this.renewalNotes = new RenewalNotes(
      this.basics,
      {
        readHeld: () => this.trust.readHeld(),
        sync: () => this.sync(),
        keptDisputes: this.note(keptDisputesKey(this.account)),
        localAssured: this.note(localAssuredKey(this.account)),
      },
      this.crossTab,
    );
    this.reEnroll = new ReEnrollWarnings(this.basics, {
      renews: (host) => this.renews(host),
      dismissed: this.note(reEnrollDismissedKey(this.account)),
      connected: this.note(reEnrollConnectedKey(this.account)),
    });
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
    this.chainSync = new ChainSync(this.basics, {
      api,
      token: () => this.session.token,
      keys,
      trust: this.trust,
      hold: this.hold,
      renewalNotes: this.renewalNotes,
      setChain: (state, clock) => {
        this.state = state;
        this.clock = clock;
      },
      signChained: (chain, actions, asV3) => this.signChained(chain, actions, asV3),
    });
    this.userKeysFlow = new AccountUserKeys(this.basics, {
      api,
      token: () => this.session.token,
      store,
      vault: this.vault,
      hostKeys: this.hostKeys,
      keyring: this.keyring,
      locks: () => this.locks,
      trust: this.trust,
      isAdmin: () => this.isAdmin,
      standing: () => this.standing,
      keyRequestNote: this.note(keyRequestKey(this.account, this.deviceId)),
      getMissingUserKey: () => this.missingUserKey,
      setMissingUserKey: (gap) => {
        this.missingUserKey = gap;
      },
      appendBatch: (build) => this.chainSync.appendBatch(build),
      sealForRecovery: (epoch, raw) => this.recoveryFlow.sealForRecovery(epoch, raw),
      receiveMail: () => this.receiveMail(),
      mailUserKeys: (id, dhKey, list) => this.enrollmentFlow.mailUserKeys(id, dhKey, list),
      checkHostKeys: () => this.checkHostKeys(),
    });
    this.recoveryFlow = new AccountRecovery(this.basics, {
      api,
      token: () => this.session.token,
      keys,
      noiseKeys: () => this.noiseKeys,
      vault: this.vault,
      keyring: this.keyring,
      trust: this.trust,
      standing: () => this.standing,
      joinedNote: this.note(joinedKey(this.account, this.deviceId)),
      info: () => this.info(),
      setChain: (state, clock) => {
        this.state = state;
        this.clock = clock;
      },
      sync: () => this.sync(),
      append: (action) => this.append(action),
      landed: (entries, now) => this.chainSync.landed(entries, now),
      withUserKeys: (use) => this.withUserKeys(use),
    });
    this.hostKeysFlow = new HostKeys(this.basics, {
      api,
      token: () => this.session.token,
      noiseKeys: () => this.noiseKeys,
      keyring: this.keyring,
      isAdmin: () => this.isAdmin,
      standing: () => this.standing,
      pinned: () => this.pinned,
      assertNotForked: () => this.assertNotForked(),
      requireAdmin: () => this.requireAdmin(),
      keysWithheldFrom: (id) => this.keysWithheldFrom(id),
      machineKeyBytes: (machine, mkEpoch, ukEpoch) => this.machineKeyBytes(machine, mkEpoch, ukEpoch),
      machineKeys: (id) => this.machineKeys(id),
      receiveMail: () => this.receiveMail(),
      resendUserKeys: (id) => this.resendUserKeys(id),
      hostKeyRequestNote: this.note(hostKeyRequestKey(this.account, this.deviceId)),
      hostKeysSentNote: (to) => this.note(hostKeysSentKey(this.account, this.deviceId, to)),
      getMissingHostKeys: () => this.missingHostKeys,
      setMissingHostKeys: (missing) => {
        this.missingHostKeys = missing;
      },
    });
    this.enrollmentFlow = new HostEnrollment(this.basics, {
      api,
      token: () => this.session.token,
      noiseKeys: () => this.noiseKeys,
      vault: this.vault,
      hostKeys: this.hostKeys,
      keyring: this.keyring,
      isAdmin: () => this.isAdmin,
      standing: () => this.standing,
      pinned: () => this.pinned,
      pairing: () => this.pairing,
      assertNotForked: () => this.assertNotForked(),
      requireWritable: (writable) => this.requireWritable(writable),
      fingerprint: () => this.fingerprint(),
      sync: () => this.sync(),
      appendBatch: (build) => this.chainSync.appendBatch(build),
      appendWaitingFor: (host, build, wait) => this.chainSync.appendWaitingFor(host, build, wait),
      ensureUserKey: () => this.ensureUserKey(),
      noUserKey: () => this.userKeysFlow.noUserKey(),
      ensureRecoveryPackages: (scope) => this.ensureRecoveryPackages(scope),
      mailHostKey: (host, epoch, key, noRole) => this.mailHostKey(host, epoch, key, noRole),
      answerHostKeyRequest: (id) => this.answerHostKeyRequest(id),
      machineKeyBytes: (machine, mkEpoch, ukEpoch) => this.machineKeyBytes(machine, mkEpoch, ukEpoch),
      isolated: (id) => this.isolated(id),
    });
    this.membershipFlow = new AccountMembership(this.basics, {
      api,
      token: () => this.session.token,
      keys,
      noiseKeys: () => this.noiseKeys,
      vault: this.vault,
      keyring: this.keyring,
      hold: this.hold,
      renewalNotes: this.renewalNotes,
      isAdmin: () => this.isAdmin,
      standing: () => this.standing,
      pinned: () => this.pinned,
      forked: () => this.forked,
      assertNotForked: () => this.assertNotForked(),
      requireAdmin: () => this.requireAdmin(),
      requireUpgraded: (chain) => this.requireUpgraded(chain),
      requireWritable: (writable) => this.requireWritable(writable),
      sync: () => this.sync(),
      append: (action) => this.append(action),
      appendBatch: (build, batchOpts) => this.chainSync.appendBatch(build, batchOpts),
      appendWaitingFor: (host, build, wait) => this.chainSync.appendWaitingFor(host, build, wait),
      heldUserKey: (id) => this.heldUserKey(id),
      ensureUserKey: () => this.ensureUserKey(),
      noUserKey: () => this.userKeysFlow.noUserKey(),
      withUserKeys: (use) => this.withUserKeys(use),
      keepUserKey: (epoch, uk) => this.enrollmentFlow.keepUserKey(epoch, uk),
      mailUserKey: (epoch, uk) => this.enrollmentFlow.mailUserKey(epoch, uk),
      mailUserKeys: (id, dhKey, list) => this.enrollmentFlow.mailUserKeys(id, dhKey, list),
      mailHostKey: (host, epoch, key, noRole) => this.mailHostKey(host, epoch, key, noRole),
      sendHostKeys: (id, only) => this.sendHostKeys(id, only),
      commitHostKey: (id, mk, prefix, append) => this.enrollmentFlow.commitHostKey(id, mk, prefix, append),
      issueMachineKey: (id, courier, reencrypt, prefix) => this.issueMachineKey(id, courier, reencrypt, prefix),
      handOver: (id, mkEpoch, ukEpoch, mk, courier, reencrypt) => this.enrollmentFlow.handOver(id, mkEpoch, ukEpoch, mk, courier, reencrypt),
      machineCheck: (machine) => this.userKeysFlow.machineCheck(machine),
      approvePasskeyJoin: (c) => this.approvePasskeyJoin(c),
    });
    this.renewalFlow = new RenewalFlow(this.basics, {
      api,
      token: () => this.session.token,
      locks: () => this.locks,
      renewalStash: this.renewalStash,
      renewalNotes: this.renewalNotes,
      assertNotForked: () => this.assertNotForked(),
      requireAdmin: () => this.requireAdmin(),
      requireUpgraded: (chain) => this.requireUpgraded(chain),
      checkRenewalOn: (chain, host, rotation, assurance) => this.checkRenewalOn(chain, host, rotation, assurance),
      sync: () => this.sync(),
      append: (action) => this.append(action),
      appendBatch: (build, batchOpts) => this.chainSync.appendBatch(build, batchOpts),
      guardedRenewal: (courier) => this.guardedRenewal(courier),
      commitHostKey: (id, mk, prefix, append) => this.enrollmentFlow.commitHostKey(id, mk, prefix, append),
      mailHostKey: (host, epoch, key, noRole) => this.mailHostKey(host, epoch, key, noRole),
      handOver: (id, mkEpoch, ukEpoch, mk, courier, reencrypt) => this.enrollmentFlow.handOver(id, mkEpoch, ukEpoch, mk, courier, reencrypt),
      handHostKeys: (id, wait, signal) => this.handHostKeys(id, wait, signal),
      sendHostKeys: (id, only) => this.sendHostKeys(id, only),
    });
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
   * active device that pinned the account, its keys settled.
   */
  static open(api: HubApi, session: HubSession, store: KeyStore, opts: AccountOptions = {}): Promise<CloudAccount> {
    return openAccount(api, session, store, opts, (own, id, keys, storage) => {
      const account = new CloudAccount(api, own, id, keys, null, store, { ...opts, storage });
      return { account, session: account.sessionFlow };
    });
  }

  /** The pairing screen's "sign in again": the next sign-in registers this browser afresh if still unjoined. */
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

  /** A new device session: the hub's challenge signed with this device's key. */
  renewSession(): Promise<void> {
    return this.sessionFlow.renewSession();
  }

  /** `HubApi.renew`: once per refusal — a session minted for a 401 and refused at once is a second refusal. */
  renewAfter401(stale: string): Promise<string | null> {
    return this.sessionFlow.renewAfter401(stale);
  }

  /** The hub will not have this device: stop renewing, tell the page. */
  setRefusal(refusal: Refusal): void {
    this.sessionFlow.setRefusal(refusal);
  }

  /** Keep the device session fresh while the page is open (renewed shortly before it expires). */
  keepSessionFresh(): void {
    this.sessionFlow.keepSessionFresh();
  }

  /** Stop renewing (a sign-out, a wipe). */
  stopSession(): void {
    this.sessionFlow.stopSession();
  }

  /** Read the chain with a password sign-in's login token to verify whether it removes this device. */
  async confirmRemoval(loginToken: string): Promise<boolean> {
    await this.chainSync.catchUp(loginToken, { noRenew: true });
    return this.standing === "removed";
  }

  // ------------------------------------------------------------ the chain

  /** Whether this browser pinned this account's genesis (and so trusts the chain enough to take keys). */
  get pinned(): boolean {
    return this.trust.pinned;
  }

  /** Set when this view conflicted with the chain this browser verified (a fork): why. */
  get forked(): string | null {
    return this.trust.forked;
  }

  /** Throws once this page's view forked ({@link forked}): nothing is attested, opened, signed or sealed on it. */
  assertNotForked(): void {
    this.trust.assertNotForked();
  }

  /** While this page is open, keep the seen record from staying lowered or gone. */
  watchSeenRecord(on: SeenWatch = defaultSeenWatch()): () => void {
    return this.trust.watchSeenRecord(on);
  }

  /** Finish a channel handshake on a copy of this view, holding it against this view and the seen record. */
  completeHandshake<T>(finish: (merged: { chain: ChainState; clock: ChainClock }) => Promise<T>): Promise<T> {
    return this.chainSync.completeHandshake(finish);
  }

  /** Bring the chain up to date from the hub, verifying every entry. */
  sync(): Promise<number> {
    return this.chainSync.sync();
  }

  /** Entries the hub has past this view that this device does not take yet (a held-back self-activation). */
  get heldBack(): HeldBack | null {
    return this.hold.heldBack;
  }

  /** Entries the hub pushed (a `chain` frame): merged if they follow on, else a sync fills the gap. */
  absorb(encoded: Uint8Array[]): Promise<number> {
    return this.chainSync.absorb(encoded);
  }

  /** Where this device stands: active, pending, removed, or absent. */
  get standing(): Standing {
    const record = this.state?.deviceRecord(this.deviceId);
    if (!record) return "absent";
    return record.status.type;
  }

  /** Whether this device is a current, active device of the account's chain. */
  get inChain(): boolean {
    return this.state?.device(this.deviceId) !== undefined;
  }

  /** Sign and append one entry as this device (once more on a newer head, if the hub had one this view lacked). */
  append(action: Action): Promise<void> {
    return this.chainSync.append(action);
  }

  private signChained(chain: ChainState, actions: Action[], asV3 = false): Promise<Entry[]> {
    return this.chainSync.signChained(chain, actions, asV3);
  }

  private guardedRenewal(courier: RenewalCourier): RenewalCourier {
    return this.enrollmentFlow.guardedRenewal(courier);
  }

  private checkRenewalOn(chain: ChainState, host: string, rotation: HostRotation, assurance: Assurance): void {
    this.renewalFlow.checkRenewalOn(chain, host, rotation, assurance);
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

  private unkeptAck: AckRecord | null = null;

  private readAck(): AckRecord | null {
    if (this.unkeptAck) return this.unkeptAck;
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
    const key = ackKey(this.account, this.deviceId);
    const raw = JSON.stringify(ack);
    this.write(key, raw);
    this.unkeptAck = this.read(key) === raw ? null : ack;
  }

  /** Device-changing entries this device did not sign itself and has not acknowledged yet, oldest first. */
  alerts(): ChainAlert[] {
    return this.chainAlerts.alerts();
  }

  /** "That was me": the alert at `seq` is acknowledged on this device. */
  acknowledge(seq: number): void {
    this.chainAlerts.acknowledge(seq);
  }

  /** Whether a rotation ("rotate keys again") is due (see {@link rotationDueIn}). */
  get rotationDue(): boolean {
    return this.state ? rotationDueIn(this.state) : false;
  }

  /** Whether the newest user key is one a device that no longer manages the account held ({@link userKeyStale}). */
  get userKeyStale(): boolean {
    return this.state ? userKeyStale(this.state) : false;
  }

  /** Whether device `id` ever held the user key, by the chain's history (review 1). */
  heldUserKey(id: string): boolean {
    return this.state ? heldUserKey(this.state, id) : false;
  }

  /** Settle steps an active pinned device runs after loading or after the chain moves. */
  async settle(): Promise<void> {
    if (this.standing !== "active" || !this.pinned) return;
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
    this.emit();
  }

  // ------------------------------------------------------------ genesis, keys & enrollment

  /** Create the account's chain on chain v3 with `UK` epoch 0 committed in the same call. */
  createGenesis(withRecovery: boolean, opts: { writable?: boolean } = {}): Promise<string[] | null> {
    return this.recoveryFlow.createGenesis(withRecovery, opts);
  }

  /** Seal a `UK` epoch to the chain's current recovery key into the keyring. */
  private sealForRecovery(epoch: number, raw: Uint8Array): Promise<boolean> {
    return this.recoveryFlow.sealForRecovery(epoch, raw);
  }

  /** Write recovery packages for `UK` epochs held under the current recovery key. */
  ensureRecoveryPackages(scope: "own" | "any" = "own"): Promise<number> {
    return this.recoveryFlow.ensureRecoveryPackages(scope);
  }

  /** The `UK` epoch new machine keys are wrapped under, making epoch 0 only as sole admin when none exists. */
  ensureUserKey(): Promise<number | null> {
    return this.userKeysFlow.ensureUserKey();
  }

  /** The chain's newest committed `UK` epoch, when this device does not hold it. */
  userKeyGap(): Promise<number | null> {
    return this.userKeysFlow.userKeyGap();
  }

  /** Check whether this device holds the newest user key, checking mail and requesting if missing. */
  checkUserKey(): Promise<number | null> {
    return this.userKeysFlow.checkUserKey();
  }

  /** Ask every other current full device to mail this one its user keys (purpose 4). */
  requestUserKeys(epoch: number, force = false): Promise<string[]> {
    return this.userKeysFlow.requestUserKeys(epoch, force);
  }

  /** Whether this active device cannot read the account (unpinned, or admin holding no user key). */
  lacksKeys(): Promise<boolean> {
    return this.userKeysFlow.lacksKeys();
  }

  /** Mail every `UK` epoch held to active admin `id` sealed to its chain key (purpose 1). */
  resendUserKeys(id: string): Promise<number[]> {
    return this.userKeysFlow.resendUserKeys(id);
  }

  /** Move legacy raw machine keys out of the key store into the keyring (admin) or host-key vault. */
  migrate(): Promise<number> {
    return this.userKeysFlow.migrate();
  }

  /** The account key's fingerprint: what an enrolling machine pins. */
  async fingerprint(): Promise<Uint8Array> {
    if (!this.state) throw new AccountError("notInChain", "the account has no chain yet");
    return fingerprint(this.state.accountKey);
  }

  /** A fresh enrollment code, its token announced to the hub (the secret never is). */
  startEnrollment(): Promise<EnrollCode> {
    return this.enrollmentFlow.startEnrollment();
  }

  /** {@link startEnrollment}, with when the hub says the code runs out. */
  startTimedEnrollment(): Promise<{ code: EnrollCode; expiresMs: number | null }> {
    return this.enrollmentFlow.startTimedEnrollment();
  }

  /** Wait for the agent's proof, verify it, commit the machine and its first key epoch, and mail host keys. */
  completeEnrollment(
    code: EnrollCode,
    signal?: AbortSignal,
    pollMs = 1000,
    expect?: { signKey: Uint8Array; dhKey: Uint8Array },
    opts: { writable?: boolean; wait?: { forMs: number; everyMs: number }; admin?: boolean } = {},
  ): Promise<MachineInfo> {
    return this.enrollmentFlow.completeEnrollment(code, signal, pollMs, expect, opts);
  }

  /** Generate and commit a new `MK` epoch for machine `id`, put its keyring blob, mail host keys, and hand it over. */
  issueMachineKey(
    id: string,
    courier: MachineKeyCourier,
    reencrypt = false,
    prefix: Action[] | ((chain: ChainState) => Action[]) = [],
  ): Promise<{ mkEpoch: number; ukEpoch: number; result: DeliveryResult; hostKeysTo: string[]; userKey?: FreshUserKey }> {
    return this.enrollmentFlow.issueMachineKey(id, courier, reencrypt, prefix);
  }

  /** Hand machine `id` its newest committed key epoch from the keyring, or issue a new epoch if absent. */
  handOverMachineKey(id: string, courier: MachineKeyCourier): Promise<{ mkEpoch: number; result: DeliveryResult }> {
    return this.enrollmentFlow.handOverMachineKey(id, courier);
  }

  /** `MK` epoch `mkEpoch` of `machine` as bytes, verified against the chain; the caller zeroes it. */
  machineKeyBytes(machine: string, mkEpoch: number, ukEpoch?: number): Promise<Bytes | null> {
    return this.userKeysFlow.machineKeyBytes(machine, mkEpoch, ukEpoch);
  }

  /** The key a noted re-encryption is pinned to (its `MK` epoch, under its `UK` epoch, checked against the chain). */
  pinnedMachineKey(item: PendingReencrypt): Promise<Bytes | null> {
    return this.userKeysFlow.pinnedMachineKey(item);
  }

  /** Every committed epoch of machine `id`'s key this browser can open (`store` and `maxEpochs` kept for callers). */
  machineKeys(id: string, _store?: KeyStore, _maxEpochs = 64): Promise<Map<number, MachineKey>> {
    return this.userKeysFlow.machineKeys(id);
  }

  /** Take keys mailed to this device (user keys, host keys, key requests) and answer peer requests. */
  receiveMail(): Promise<number> {
    return this.enrollmentFlow.receiveMail();
  }

  // ------------------------------------------------------------ host keys (chain v3)

  /** Current non-admin devices that `host` accepts control from and that are not withheld keys. */
  hostKeyRecipients(host: string, chain: ChainState | null = this.state): DeviceRecord[] {
    return this.hostKeysFlow.hostKeyRecipients(host, chain);
  }

  /** Seal and mail a newly committed key epoch of `host` to every eligible non-admin device (purpose 5). */
  mailHostKey(
    host: string,
    epoch: number,
    key: Uint8Array,
    noRole: Set<string> = new Set(),
  ): Promise<{ sent: string[]; failed: Record<string, string>; waiting: string[] }> {
    return this.hostKeysFlow.mailHostKey(host, epoch, key, noRole);
  }

  /** Seal and mail committed machine key epochs of admitting hosts to non-admin device `id`. */
  sendHostKeys(id: string, only?: string): Promise<number> {
    return this.hostKeysFlow.sendHostKeys(id, only);
  }

  /** Answer non-admin device `id`'s throttled request for host keys (purpose 4). */
  answerHostKeyRequest(id: string): Promise<number> {
    return this.hostKeysFlow.answerHostKeyRequest(id);
  }

  /** Wait briefly for a newly registered host's device role and seal it the other admitting hosts' keys. */
  handHostKeys(id: string, wait = HOST_KEYS_WAIT, signal?: AbortSignal): Promise<number | null> {
    return this.hostKeysFlow.handHostKeys(id, wait, signal);
  }

  /** The newest committed key epoch of every admitting current host that this device cannot open. */
  hostKeyGaps(): Promise<{ host: string; epoch: number }[]> {
    return this.hostKeysFlow.hostKeyGaps();
  }

  /** Check missing host keys (non-admin), checking mail and requesting from admins if any are missing. */
  checkHostKeys(): Promise<string[]> {
    return this.hostKeysFlow.checkHostKeys();
  }

  /** Ask the account's admins for the hosts' machine keys (purpose 4, rate-limited unless `force`). */
  requestHostKeys(force = false): Promise<string[]> {
    return this.hostKeysFlow.requestHostKeys(force);
  }

  /** Send device `id` its keys again: user keys to an admin, host keys to a non-admin. */
  resendKeys(id: string): Promise<number> {
    return this.hostKeysFlow.resendKeys(id);
  }

  // ------------------------------------------------------------ chain v3 changes & membership

  /** Add a paired device (or approve a passkey join via pairing) and deliver its keys. */
  pairDevice(candidate: Candidate, opts: { admin: boolean }): Promise<"delivered" | "redelivered"> {
    return this.membershipFlow.pairDevice(candidate, opts);
  }

  /** Make device `id` a full device or a limited one (`setAdmin`, chain v3 only). */
  setAdmin(id: string, admin: boolean, opts: { courier?: MachineKeyCourier; reencrypt?: boolean } = {}): Promise<RotationReport | null> {
    return this.membershipFlow.setAdmin(id, admin, opts);
  }

  /** Promote device `id` to a full device, returning whether mailing its user keys failed. */
  promote(id: string): Promise<{ keysFailed: unknown }> {
    return this.membershipFlow.promote(id);
  }

  /** Stop or restart running AI on device `id` (`setHost`, chain v3 only). */
  setHost(id: string, host: boolean, courier: MachineKeyCourier, opts: { writable?: boolean } = {}): Promise<{ mkEpoch: number; result: DeliveryResult; userKey?: FreshUserKey } | null> {
    return this.membershipFlow.setHost(id, host, courier, opts);
  }

  /** Update whom `host` accepts control from (`setAccept`, chain v3 only), rotating its key on narrowing. */
  setAccept(host: string, from: Accept, courier: MachineKeyCourier = NO_COURIER): Promise<AcceptChange> {
    return this.membershipFlow.setAccept(host, from, courier);
  }

  /** Remove device `id` (or veto a held-back activation) and rotate the keys it could read. */
  removeDevice(
    id: string,
    courier: MachineKeyCourier,
    reencrypt = false,
    opts: {
      rotateMachineKeys?: boolean;
      veto?: Pick<VetoPlan, "device" | "seq" | "view">;
      alsoTrusted?: readonly string[];
    } = {},
  ): Promise<RotationReport> {
    return this.membershipFlow.removeDevice(id, courier, reencrypt, opts);
  }

  /** Whether this device may take itself out of the account's chain on sign-out, and if not, why. */
  selfRemoval(): SelfRemoval {
    return this.membershipFlow.selfRemoval();
  }

  /** Take this device out of the account's chain, signed by itself as its last entry. */
  removeSelf(): Promise<void> {
    return this.membershipFlow.removeSelf();
  }

  /** Whether non-admin device `id` may control some current host now. */
  controlsAHost(id: string): boolean {
    return this.membershipFlow.controlsAHost(id);
  }

  // ------------------------------------------------------------ identity key renewal (docs/host-identity-rotation.md)

  /** Update the renewal runner's note (which hosts renew, are pending, isolated, or flagged). */
  noteRenewals(note: RenewalNote): void {
    this.renewalNotes.noteRenewals(note);
  }

  /** The current hosts that should renew their identity key now, each with why. */
  hostsToRotate(): Map<string, Exposure> {
    return this.renewalNotes.hostsToRotate();
  }

  /** Whether host `id` renews its identity key (rather than being re-enrolled). Never on a v2 chain. */
  renews(id: string): boolean {
    return this.renewalNotes.renews(id);
  }

  /** Whether host `id` is isolated (§3.8). */
  isolated(id: string): boolean {
    return this.renewalNotes.isolated(id);
  }

  /** Keep disputes this page verified (§3.8) across reloads and tabs. */
  keepDisputes(found: readonly KeptDispute[]): void {
    this.renewalNotes.keepDisputes(found);
  }

  /** The verified disputes kept for the account in this browser. */
  keptDisputes(): KeptDispute[] {
    return this.renewalNotes.keptDisputes();
  }

  /** Whether the hub flags host `id` isolated with no evidence this page checked. */
  flagged(id: string): boolean {
    return this.renewalNotes.flagged(id);
  }

  /** Whether this device withholds host keys from host `id`'s device role (isolated or waiting to renew). */
  keysWithheldFrom(id: string): boolean {
    return this.renewalNotes.keysWithheldFrom(id);
  }

  /** Renew host `host`'s identity key (docs/host-identity-rotation.md §3.1–§3.7). */
  renewIdentity(host: string, courier: RenewalCourier, opts: RenewOptions): Promise<RenewalReport> {
    return this.renewalFlow.renewIdentity(host, courier, opts);
  }

  /** Forget stashed renewal attempts that the hub no longer has open. */
  forgetLeftRenewals(rows: readonly HostRotationView[]): Promise<void> {
    return this.renewalFlow.forgetLeftRenewals(rows);
  }

  /** Whether this browser kept the attempt `nonceHash` (hex) names of `host`. */
  keptRenewal(host: string, nonceHash: string): Promise<boolean> {
    return this.renewalFlow.keptRenewal(host, nonceHash);
  }

  /** Finish a renewal attempt this browser started in a page that went away before writing it. */
  resumeRenewal(host: string, unguarded: RenewalCourier, opts: Partial<Omit<RenewOptions, "assurance">> = {}): Promise<RenewalReport | "busy" | null> {
    return this.renewalFlow.resumeRenewal(host, unguarded, opts);
  }

  /** Whether the account requires every identity key renewal to be confirmed on the device. */
  get alwaysAssured(): boolean {
    return this.renewalNotes.alwaysAssured;
  }

  /** Whether an admin chose the account's renewal policy (`SetRenewalPolicy` on the chain). */
  get renewalPolicyChosen(): boolean {
    return this.state?.renewalPolicySeq !== null && this.state?.renewalPolicySeq !== undefined;
  }

  /** Set the account's renewal policy via a `setRenewalPolicy` entry (admin, chain v3). */
  setRenewalPolicy(alwaysAssured: boolean): Promise<void> {
    return this.renewalFlow.setRenewalPolicy(alwaysAssured);
  }

  /** This device's own local guard requiring on-device confirmation for renewals. */
  localAssured(): boolean {
    return this.renewalNotes.localAssured();
  }

  setLocalAssured(on: boolean): void {
    this.renewalNotes.setLocalAssured(on);
  }

  /** Whether renewals signed here must be confirmed on the device (account policy or local guard). */
  get renewalsAssured(): boolean {
    return this.renewalNotes.renewalsAssured;
  }

  /** Whether device `id` is a host that must renew its identity key before another host may accept it. */
  mustRenewFirst(id: string): boolean {
    return this.renewalNotes.mustRenewFirst(id);
  }

  /** {@link mustRenewFirst} evaluated on `chain`. */
  mustRenewFirstOn(chain: ChainState, id: string): boolean {
    return this.renewalNotes.mustRenewFirstOn(chain, id);
  }

  /** Write renewal-pause markers `build` makes for the chain as it stands. */
  writePause(build: (chain: ChainState) => Action[], beforePublish?: (base: ChainState) => void): Promise<boolean> {
    return this.renewalFlow.writePause(build, beforePublish);
  }

  /** Encoded entry length of a pause start with `removals` signed here on `chain`. */
  pauseEntryLen(chain: ChainState, removals: PauseRemoval[]): number {
    return this.renewalFlow.pauseEntryLen(chain, removals);
  }

  /** What ending open pause `op` now would give back, and what stops it. */
  pauseEnding(op: Uint8Array): PauseEnding | null {
    return this.renewalFlow.pauseEnding(op);
  }

  /** End open pause `op`, optionally keeping edges in a fresh pause start in the same batch. */
  endPauseWith(
    op: Uint8Array,
    phase: typeof PAUSE_COMPLETED | typeof PAUSE_CANCELLED,
    plan: (chain: ChainState, pause: OpenPause) => { keep: PauseRemoval[] } | null,
    opts: { expect?: { host: string; id: string }[]; next?: Uint8Array } = {},
  ): Promise<{ host: string; id: string }[] | null> {
    return this.renewalFlow.endPauseWith(op, phase, plan, opts);
  }

  /** Cancel open pause `op` (user action), refused while any returned edge must stay held. */
  endPause(op: Uint8Array, expect?: { host: string; id: string }[]): Promise<void> {
    return this.renewalFlow.endPause(op, expect);
  }

  /** The exposed hosts that must renew their identity key before `host` does. */
  renewFirst(host: string): string[] {
    return this.renewalNotes.renewFirst(host);
  }

  /** The current hosts to re-enroll for a new identity (review D3). */
  reEnrollDue(controllersOf?: (host: string) => ControllersRecord | undefined): Set<string> {
    return this.reEnroll.reEnrollDue(controllersOf);
  }

  /** Per current host, each device that could control it and no longer can, with evidence. */
  reEnrollWarnings(controllersOf?: (host: string) => ControllersRecord | undefined): Map<string, ReEnrollLine[]> {
    return this.reEnroll.reEnrollWarnings(controllersOf);
  }

  /** Dismiss the re-enroll warning for `(host, device)` at `lostSeq` in this browser. */
  dismissReEnroll(host: string, device: string, lostSeq: number): void {
    this.reEnroll.dismissReEnroll(host, device, lostSeq);
  }

  /** Warn about `device` for `host` again ({@link dismissReEnroll} undone). */
  warnAgainReEnroll(host: string, device: string): void {
    this.reEnroll.warnAgainReEnroll(host, device);
  }

  /** What a veto of `held.device` takes out with it on `chain`. */
  vetoScope(held: HeldBack, chain: ChainState, opts: { trusted?: true | ReadonlySet<string>; view?: ReadonlySet<string> } = {}): { devices: string[]; machines: string[] } {
    return this.hold.vetoScope(held, chain, opts);
  }

  /** What vetoing the held-back activation would take out on the hub's chain as it stands now. */
  async vetoPlan(): Promise<VetoPlan | null> {
    return this.hold.vetoPlan();
  }

  /** The entries held back past the activation, as the page lists them ("what approving takes in"). */
  heldChanges(): HeldChange[] {
    return this.hold.heldChanges();
  }

  /** Rotate `UK` and active machine keys (optionally prefixed with removals or policy actions). */
  rotate(
    courier: MachineKeyCourier,
    reencrypt = false,
    prefix: Action[] | ((chain: ChainState) => Action[]) = [],
    opts: { overHeld?: boolean; userKey?: boolean; machineKeys?: boolean; asV3?: boolean } = {},
  ): Promise<RotationReport> {
    return this.membershipFlow.rotate(courier, reencrypt, prefix, opts);
  }

  /** What upgrading the account to chain v3 writes (`null` if already v3 or no chain). */
  upgradePlan(): { version: 3; ukEpoch: number; machines: 0 } | null {
    return this.membershipFlow.upgradePlan();
  }

  /** Upgrade the account to chain v3 by writing its first v3 entry (a user key rotation). */
  upgradeToV3(opts: { writable?: boolean }): Promise<RotationReport> {
    return this.membershipFlow.upgradeToV3(opts);
  }

  /** Every `UK` epoch this device holds (after checking mail) as bytes for the length of `use`. */
  withUserKeys<T>(use: (keys: { epoch: number; key: Bytes }[]) => Promise<T>): Promise<T> {
    return this.userKeysFlow.withUserKeys(use);
  }

  /** Look for keys again now: mail, keyring refresh, and host-key check for non-admins. */
  refreshKeys(): Promise<void> {
    return this.userKeysFlow.refreshKeys();
  }

  /** Approve a device that joined with the recovery key (or vouch for a held-back activation) and mail it its keys. */
  approveDevice(id: string): Promise<void> {
    return this.membershipFlow.approveDevice(id);
  }

  /** Make new recovery words (`setRecoveryKey`) and seal every `UK` epoch held to them. */
  setRecoveryWords(): Promise<string[]> {
    return this.recoveryFlow.setRecoveryWords();
  }

  /** Come back with the recovery words on a browser not in the chain, pinning the account and joining pending. */
  recover(phrase: string): Promise<{ epochs: number[] }> {
    return this.recoveryFlow.recover(phrase);
  }

  /** When this pending device may activate itself (by its own first sighting of its join), or `null` if not pending. */
  recoveryReadyAt(): number | null {
    return this.recoveryFlow.recoveryReadyAt();
  }

  /** Activate this pending device with `RSK` (the words again), once the delay has passed. */
  activateRecovered(phrase: string, now = Date.now()): Promise<void> {
    return this.recoveryFlow.activateRecovered(phrase, now);
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
      const pinned = await this.trust.pinnedGenesis();
      if (pinned && pinned !== hex(chain.hashAt(0)!)) throw new AccountError("fork", "the hub served a different genesis than this device pinned");
      await this.trust.pin();
      const join: Action = { type: "addDevice3", device: { id: this.deviceId, signKey: this.keys.signPublic, dhKey: this.keys.dhPublic, flags: { host: false, admin: false } } };
      const entry = await signDraft(chain.draft({ type: "passkey", slot }, join, Date.now()), signing);
      // Refused here, by the chain's own rule, before it is posted.
      await verifyBatch(chain, [entry]);
      this.assertNotForked();
      await this.api.appendChain(this.session.token, [encodeEntry(entry)]);
      await this.chainSync.landed([entry]);
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
    await this.chainSync.appendBatch(async (chain) => {
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
  storeUserKeys(keys: { epoch: number; key: Uint8Array }[]): Promise<void> {
    return this.userKeysFlow.storeUserKeys(keys);
  }

  /**
   * This device was removed from the account (or signs out for good): forget
   * everything it holds for it. Every step is tried, whatever an earlier one
   * did; what failed is thrown at the end ({@link WipeError}).
   */
  async wipe(): Promise<void> {
    this.stopSession();
    this.unkeptAck = null;
    const hosts = (this.state?.hosts() ?? []).map((m) => m.info.id);
    await forgetBrowser({
      store: this.store,
      storage: this.storage,
      account: this.account,
      deviceId: this.deviceId,
      vaults: [this.vault, this.hostKeys, this.renewalStash, this.keyring],
      legacyHosts: hosts,
      forgetSeen: () => this.trust.forgetSeen(),
    });
  }

  /** {@link wipe} for a page that could not open the account. */
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
  attestation(): Promise<Attestation> {
    return this.chainSync.attestation();
  }

  /** The hello's view and whether a machine is current, for the channel. */
  machine(id: string): MachineInfo | undefined {
    return this.state?.machine(id)?.info;
  }
}
