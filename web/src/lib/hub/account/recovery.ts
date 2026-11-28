/*
 * Founding an account (chain v3 genesis + initial user key), sealing and
 * maintaining recovery packages, rotating recovery words, and recovering and
 * activating a device with the recovery words (`CloudAccount` documents the
 * methods it hands to these).
 */
import {
  type Action,
  type Bytes,
  CHAIN_VERSION_3,
  ChainClock,
  ChainState,
  decodeEntry,
  type DeviceInfo,
  type DeviceKeys,
  type Draft,
  encodeEntry,
  type Entry,
  entryHash,
  equal,
  genesis3Draft,
  hex,
  mnemonicFromEntropy,
  type NoiseKeyPair,
  RECOVERY_DELAY_MS,
  RECOVERY_RECIPIENT,
  recoveryKeyFor,
  recoveryKeys,
  sealForRecovery,
  signDraft,
  userKeyCommit,
} from "@/lib/e2e";
import { chainV3Writable, type HubApi, HubApiError } from "../api";
import { verifyBatch } from "../chain-policy";
import { keyName, type Keyring, openRecoveryPackage, recoveryTag, type UserKeyVault } from "../keyring";
import { recoverySecret } from "../recovery-words";
import { checkpoints, type Held } from "../seen-store";
import type { AccountBasics, AccountNote } from "./basics";
import type { ChainTrust } from "./chain-trust";
import type { Standing } from "./devices";
import { AccountError, noNewAccounts } from "./errors";

const random32 = () => crypto.getRandomValues(new Uint8Array(32));

/** What genesis and recovery flows read of the account beyond its basics. */
export interface AccountRecoveryDeps {
  api: HubApi;
  token(): string;
  keys: DeviceKeys;
  noiseKeys(): NoiseKeyPair;
  vault: UserKeyVault;
  keyring: Keyring;
  trust: Pick<ChainTrust, "pinned" | "readHeld" | "pinnedGenesis" | "pinMade" | "pin" | "latchFork" | "assertNotForked">;
  standing(): Standing;
  joinedNote: AccountNote;
  info(): DeviceInfo;
  setChain(state: ChainState, clock: ChainClock): void;
  sync(): Promise<number>;
  append(action: Action): Promise<void>;
  landed(entries: Entry[], now?: number): Promise<void>;
  withUserKeys<T>(use: (keys: { epoch: number; key: Bytes }[]) => Promise<T>): Promise<T>;
}

export class AccountRecovery {
  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: AccountRecoveryDeps,
  ) {}

  async createGenesis(withRecovery: boolean, opts: { writable?: boolean } = {}): Promise<string[] | null> {
    // A browser that pinned this account's genesis makes no other: another page
    // of this browser founded the account a moment ago (the pin is shared), or
    // the hub serves a chain it should not — a fork. Said before anything else.
    await this.refuseIfPinned();
    const writable = opts.writable ?? chainV3Writable(await this.deps.api.me(this.deps.token()));
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
    const trust = await this.deps.trust.readHeld();
    if (trust.genesis === null) return;
    const { entries } = await this.deps.api.chain(this.deps.token(), 0);
    if (await this.holdsRecord(entries, trust)) {
      throw new AccountError("setUpElsewhere", "this account was just set up in another tab of this browser: reload this page");
    }
    throw this.deps.trust.latchFork("this browser pinned a genesis for this account already: it does not make a new one");
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
        recoveryKey = await recoveryKeyFor(await recoveryKeys(rk), this.basics.account, akPublic);
      } finally {
        rk.fill(0);
      }
    }
    const now = Date.now();
    const draft = genesis3Draft(
      this.basics.account,
      akPublic,
      { id: this.basics.deviceId, signKey: this.deps.keys.signPublic, dhKey: this.deps.keys.dhPublic, flags: { host: false, admin: true } },
      recoveryKey,
      now,
    );
    const genesis = await signDraft(draft, ak.privateKey);
    const uk = random32();
    try {
      const commit: Draft = {
        version: CHAIN_VERSION_3,
        account: this.basics.account,
        seq: 1,
        prev: await entryHash(genesis),
        createdMs: now,
        signer: { type: "device", id: this.basics.deviceId },
        action: { type: "keyEpochs", user: { epoch: 0, hash: await userKeyCommit(this.basics.account, 0, uk) }, machines: [] },
      };
      const keyEpochs = await signDraft(commit, this.deps.keys.sign.privateKey);
      // Refused here, by the chain's own rule, before anything leaves this device (review 13).
      await verifyBatch(null, [genesis, keyEpochs]);
      this.deps.trust.assertNotForked();
      await this.deps.api.appendChain(this.deps.token(), [encodeEntry(genesis), encodeEntry(keyEpochs)]);
      const clock = ChainClock.live(now, 0);
      this.deps.setChain(await ChainState.verify([genesis, keyEpochs], undefined, clock), clock);
      await this.deps.trust.pinMade();
      // Committed: now it may be kept and sealed.
      await this.deps.vault.put(0, uk);
      try {
        await this.sealForRecovery(0, uk);
      } catch (error) {
        console.warn("cloud account: recovery package:", error);
      }
    } finally {
      uk.fill(0);
    }
    this.basics.emit();
    return words;
  }

  async sealForRecovery(epoch: number, raw: Uint8Array): Promise<boolean> {
    this.deps.trust.assertNotForked();
    const rsk = this.basics.state?.currentRecoveryKey;
    if (!rsk) return false;
    this.deps.trust.assertNotForked();
    const sealed = await sealForRecovery(this.deps.noiseKeys(), this.basics.deviceId, this.basics.account, rsk, epoch, raw);
    await this.deps.keyring.put(keyName.recovery(epoch, this.basics.deviceId, rsk), sealed);
    return true;
  }

  async ensureRecoveryPackages(scope: "own" | "any" = "own"): Promise<number> {
    const chain = this.basics.state;
    const rsk = chain?.currentRecoveryKey;
    if (!chain || !rsk || this.deps.standing() !== "active" || !this.deps.trust.pinned) return 0;
    const covered = new Set<number>();
    if (scope === "any") {
      const tag = recoveryTag(rsk);
      for (const p of this.deps.keyring.recoveryBlobs()) if (p.tag === tag && chain.admin(p.sender)) covered.add(p.ukEpoch);
    }
    let made = 0;
    for (const epoch of await this.deps.vault.epochs()) {
      if (covered.has(epoch) || this.deps.keyring.hasBase(keyName.recovery(epoch, this.basics.deviceId, rsk), this.basics.deviceId)) continue;
      try {
        if (await this.deps.vault.withRaw(epoch, (raw) => this.sealForRecovery(epoch, raw))) made++;
      } catch (error) {
        console.warn(`cloud account: recovery package for epoch ${epoch}:`, error);
      }
    }
    return made;
  }

  async setRecoveryWords(): Promise<string[]> {
    const state = this.basics.state;
    if (this.deps.standing() !== "active" || !state || !this.deps.trust.pinned) {
      throw new AccountError("notInChain", "only an active device sets the recovery key");
    }
    const rk = crypto.getRandomValues(new Uint8Array(16));
    let words: string[];
    let key;
    try {
      words = await mnemonicFromEntropy(rk);
      key = await recoveryKeyFor(await recoveryKeys(rk), this.basics.account, state.accountKey);
    } finally {
      rk.fill(0);
    }
    await this.deps.append({ type: "setRecoveryKey", key });
    await this.deps.withUserKeys(async (keys) => {
      for (const { epoch, key: raw } of keys) await this.sealForRecovery(epoch, raw);
    });
    return words;
  }

  /** `RSK` and the recovery seal key from what the user typed, checked against the chain's recovery key. */
  private async recoveryFrom(phrase: string) {
    await this.deps.sync();
    const chain = this.basics.state;
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
    await this.deps.keyring.refresh();
    let opened = 0;
    for (const p of this.deps.keyring.recoveryBlobs().sort((a, b) => b.updatedMs - a.updatedMs)) {
      if (await this.deps.vault.has(p.ukEpoch)) continue;
      try {
        const raw = await openRecoveryPackage(seal, chain, { purpose: "userKey", account: this.basics.account, sender: p.sender, recipient: RECOVERY_RECIPIENT, epoch: p.ukEpoch }, p.blob);
        try {
          if (await this.deps.vault.put(p.ukEpoch, raw)) opened++;
        } finally {
          raw.fill(0);
        }
      } catch {
        // under an older recovery key, from a device the chain never named, not the committed key, or spoiled
      }
    }
    return opened;
  }

  async recover(phrase: string): Promise<{ epochs: number[] }> {
    if (this.deps.standing() !== "absent") throw new AccountError("notInChain", "this device is already in the chain");
    const { chain, keys } = await this.recoveryFrom(phrase);
    const pinned = await this.deps.trust.pinnedGenesis();
    if (pinned && pinned !== hex(chain.hashAt(0)!)) throw new AccountError("fork", "the hub served a different genesis than this device pinned");
    await this.deps.trust.pin();
    await this.openRecoveryPackages(chain, keys.seal);
    const epochs = await this.deps.vault.epochs();
    if (!epochs.length) throw new AccountError("noPackages", "no recovery package opens with these words");
    // A v3 chain takes v3 joins only: an admin, never a host (review D1, D4).
    const join: Action = chain.isV3
      ? { type: "recoveryJoin3", device: { id: this.basics.deviceId, signKey: this.deps.keys.signPublic, dhKey: this.deps.keys.dhPublic, flags: { host: false, admin: true } } }
      : { type: "recoveryJoin", device: this.deps.info() };
    const entry = await signDraft(chain.draft({ type: "recovery" }, join, Date.now()), keys.signing);
    // Refused here, by the chain's own rule, before it is posted (review 13).
    await verifyBatch(chain, [entry]);
    this.deps.trust.assertNotForked();
    await this.deps.api.appendChain(this.deps.token(), [encodeEntry(entry)]);
    await this.deps.landed([entry]);
    this.deps.joinedNote.write(String(Date.now()));
    return { epochs };
  }

  recoveryReadyAt(): number | null {
    const state = this.basics.state;
    const record = state?.deviceRecord(this.basics.deviceId);
    if (record?.status.type !== "pending") return null;
    const seen = this.basics.clock.firstSeen.get(record.status.joinSeq);
    if (seen !== undefined) return seen + RECOVERY_DELAY_MS;
    const joined = Number(this.deps.joinedNote.read());
    if (Number.isFinite(joined) && joined > 0) return joined + RECOVERY_DELAY_MS;
    return (state!.entryAt(record.status.joinSeq)?.draft.createdMs ?? Date.now()) + RECOVERY_DELAY_MS;
  }

  async activateRecovered(phrase: string, now = Date.now()): Promise<void> {
    if (this.deps.standing() !== "pending") throw new AccountError("notPending", "this device is not waiting for activation");
    const readyAt = this.recoveryReadyAt()!;
    if (now < readyAt) throw new AccountError("tooEarly", "too early to activate this device", readyAt);
    const { chain, keys } = await this.recoveryFrom(phrase);
    if (this.deps.standing() !== "pending") return; // approved meanwhile
    const entry = await signDraft(chain.draft({ type: "recovery" }, { type: "activateRecovered", id: this.basics.deviceId }, now), keys.signing);
    // Refused here, by the chain's own rule, before it is posted (review 13). (The delay is this device's
    // own clock's, checked above, and the hub's: the check here takes the activation as history.)
    await verifyBatch(chain, [entry]);
    try {
      this.deps.trust.assertNotForked();
      await this.deps.api.appendChain(this.deps.token(), [encodeEntry(entry)]);
    } catch (error) {
      // The hub keeps its own first sighting of the join (`ChainError::TooEarly`'s text).
      const ready = error instanceof HubApiError ? /activate itself only from (\d+)/.exec(error.message) : null;
      if (ready) throw new AccountError("tooEarly", error instanceof Error ? error.message : "too early", Number(ready[1]));
      throw error;
    }
    await this.deps.landed([entry], now);
    // Epochs made while this device waited were sealed to the recovery key too.
    await this.openRecoveryPackages(chain, keys.seal);
    this.deps.joinedNote.forget();
    this.basics.emit();
  }
}
