/*
 * User key epochs, legacy machine-key migration, opening committed machine
 * keys from the keyring and host-key vault, and storing keys delivered by a
 * pairing (`CloudAccount` documents the methods it hands to these).
 */
import { type Action, type Bytes, type ChainState, type Entry, hex, type KeyStore, type MachineKey, userKeyCommit } from "@/lib/e2e";
import { type AccountLocks, LEGACY_MIGRATION_LOCK } from "../account-storage";
import { type HubApi, MAIL_KEY_REQUEST } from "../api";
import type { PendingReencrypt } from "../deliver";
import {
  type HostKeyVault,
  type Keyring,
  machineKeyBytes,
  migrateLegacyMachineKeys,
  moveLegacyMachineKeysToVault,
  openMachineKeys,
  type UserKeyVault,
} from "../keyring";
import type { AccountBasics, AccountNote } from "./basics";
import type { ChainTrust } from "./chain-trust";
import type { Standing } from "./devices";
import { AccountError } from "./errors";
import { KEY_REQUEST_EVERY_MS } from "./limits";

const random32 = () => crypto.getRandomValues(new Uint8Array(32));

/** What user-key and keyring operations read of the account beyond its basics. */
export interface UserKeysDeps {
  api: HubApi;
  token(): string;
  store: KeyStore;
  vault: UserKeyVault;
  hostKeys: HostKeyVault;
  keyring: Keyring;
  locks(): AccountLocks | null;
  trust: Pick<ChainTrust, "pinned" | "readHeld" | "pinnedGenesis" | "pin" | "assertNotForked">;
  isAdmin(): boolean;
  standing(): Standing;
  keyRequestNote: AccountNote;
  getMissingUserKey(): number | null;
  setMissingUserKey(gap: number | null): void;
  appendBatch(build: (chain: ChainState) => Promise<Action[]>): Promise<Entry[]>;
  sealForRecovery(epoch: number, raw: Uint8Array): Promise<boolean>;
  receiveMail(): Promise<number>;
  mailUserKeys(id: string, dhKey: Uint8Array, keys: { epoch: number; key: Uint8Array }[]): Promise<void>;
  checkHostKeys(): Promise<string[]>;
}

export class AccountUserKeys {
  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: UserKeysDeps,
  ) {}

  /** Whether another active admin ("full") device shares this account (and so may hold keys this one lacks). */
  private hasPeers(): boolean {
    return (this.basics.state?.admins() ?? []).some((d) => d.info.id !== this.basics.deviceId);
  }

  async ensureUserKey(): Promise<number | null> {
    const chain = this.basics.state;
    // Only an admin holds (or makes) a user key.
    if (!this.deps.isAdmin()) return null;
    const latest = chain?.latestUserEpoch() ?? null;
    if (latest !== null) return (await this.checkUserKey()) === null ? latest : null;
    if (!chain || this.deps.standing() !== "active" || !this.deps.trust.pinned || this.hasPeers()) return null;
    const uk = random32();
    try {
      await this.deps.appendBatch(async (c) => {
        if (c.userKeyEpochs() !== 0) throw new AccountError("noUserKey", "another device made the account's first user key meanwhile");
        return [{ type: "keyEpochs", user: { epoch: 0, hash: await userKeyCommit(this.basics.account, 0, uk) }, machines: [] }];
      });
      await this.deps.vault.put(0, uk);
      try {
        await this.deps.sealForRecovery(0, uk);
      } catch (error) {
        console.warn("cloud account: recovery package:", error);
      }
    } finally {
      uk.fill(0);
    }
    this.basics.emit();
    return 0;
  }

  /** Why there is no user key to wrap under: "missing key" when the chain commits one this device lacks. */
  noUserKey(): AccountError {
    const missing = this.deps.getMissingUserKey();
    if (missing !== null) {
      return new AccountError(
        "missingKey",
        `missing key: this device does not hold the account's newest user key (epoch ${missing}). It asked the account's other devices for it; if none has it, rotate the keys again`,
      );
    }
    return new AccountError("noUserKey", "this device holds no user key yet");
  }

  async userKeyGap(): Promise<number | null> {
    const latest = this.basics.state?.latestUserEpoch() ?? null;
    if (latest === null || (await this.deps.vault.has(latest))) return null;
    return latest;
  }

  async checkUserKey(): Promise<number | null> {
    // A device that does not manage the account never asks for a user key (chain v3 §3.3).
    let gap = this.deps.isAdmin() ? await this.userKeyGap() : null;
    if (gap !== null && this.deps.standing() === "active" && this.deps.trust.pinned) {
      try {
        await this.deps.receiveMail();
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
    if (this.deps.getMissingUserKey() !== gap) {
      this.deps.setMissingUserKey(gap);
      this.basics.emit();
    }
    return gap;
  }

  async requestUserKeys(epoch: number, force = false): Promise<string[]> {
    const state = this.basics.state;
    if (!state || this.deps.standing() !== "active" || !this.deps.trust.pinned) return [];
    try {
      const last = JSON.parse(this.deps.keyRequestNote.read() ?? "null") as { epoch?: unknown; atMs?: unknown } | null;
      if (!force && last?.epoch === epoch && typeof last.atMs === "number" && Date.now() - last.atMs < KEY_REQUEST_EVERY_MS) return [];
    } catch {
      // asked never, or the record is spoiled: ask
    }
    const asked: string[] = [];
    for (const d of state.admins()) {
      if (d.info.id === this.basics.deviceId) continue;
      try {
        await this.deps.api.mail(this.deps.token(), { recipient: d.info.id, sender: this.basics.deviceId, purpose: MAIL_KEY_REQUEST, epoch, sealed: new Uint8Array(0) });
        asked.push(d.info.id);
      } catch (error) {
        console.warn(`cloud account: could not ask ${d.info.id} for the user key:`, error);
      }
    }
    this.deps.keyRequestNote.write(JSON.stringify({ epoch, atMs: Date.now() }));
    return asked;
  }

  async lacksKeys(): Promise<boolean> {
    if (this.deps.standing() !== "active") return false;
    // Asked of the store where this page saw no pin: another page may have pinned the account since.
    if (!this.deps.trust.pinned) await this.deps.trust.readHeld();
    if (!this.deps.trust.pinned) return true;
    // Not an admin: it holds no user key by design, and asks for the hosts' keys it lacks.
    if (!this.deps.isAdmin()) return false;
    if ((await this.deps.vault.epochs()).length) return false;
    return (await this.ensureUserKey()) === null;
  }

  async resendUserKeys(id: string): Promise<number[]> {
    if (id === this.basics.deviceId) throw new AccountError("self", "send keys to this device from another one");
    const record = this.basics.state?.admin(id);
    if (!record) throw new AccountError("notInChain", `${id} is not a current full device`);
    return this.withUserKeys(async (keys) => {
      if (!keys.length) throw new AccountError("noUserKey", "this device holds no user key to send");
      await this.deps.mailUserKeys(id, record.info.dhKey, keys);
      return keys.map((k) => k.epoch);
    });
  }

  async migrate(): Promise<number> {
    const chain = this.basics.state;
    if (!chain) return 0;
    const machines = chain.hosts().map((m) => m.info.id);
    const run = () =>
      this.deps.isAdmin()
        ? migrateLegacyMachineKeys(this.deps.store, this.deps.vault, this.deps.keyring, this.basics.account, machines, () => this.ensureUserKey())
        : moveLegacyMachineKeysToVault(this.deps.store, this.deps.hostKeys, machines, (machine, epoch, key) => chain.checkMachineKey(machine, epoch, key));
    const locks = this.deps.locks();
    const moved = locks ? await locks.request(LEGACY_MIGRATION_LOCK, { mode: "exclusive", ifAvailable: true }, (lock) => (lock === null ? 0 : run())) : await run();
    if (moved) this.basics.emit();
    return moved;
  }

  /** The chain's check of machine `machine`'s key epochs. */
  machineCheck(machine: string): (mkEpoch: number, key: Bytes) => Promise<void> {
    const chain = this.basics.state;
    return async (mkEpoch, key) => {
      if (!chain) throw new AccountError("notInChain", "the account has no chain yet");
      await chain.checkMachineKey(machine, mkEpoch, key);
    };
  }

  async machineKeyBytes(machine: string, mkEpoch: number, ukEpoch?: number): Promise<Bytes | null> {
    if (!this.basics.state || !this.deps.trust.pinned) return null;
    await this.deps.keyring.load();
    return machineKeyBytes(this.deps.vault, this.deps.keyring, this.basics.account, machine, mkEpoch, this.machineCheck(machine), ukEpoch);
  }

  pinnedMachineKey(item: PendingReencrypt): Promise<Bytes | null> {
    return this.machineKeyBytes(item.machine, item.mkEpoch, item.ukEpoch);
  }

  async machineKeys(id: string): Promise<Map<number, MachineKey>> {
    const chain = this.basics.state;
    if (!chain || !this.deps.trust.pinned) return new Map();
    await this.deps.keyring.load();
    const committed = chain.machineKeyEpochs(id);
    const keys = (await this.deps.vault.epochs()).length
      ? await openMachineKeys(this.deps.vault, this.deps.keyring, this.basics.account, id, committed, this.machineCheck(id))
      : new Map<number, MachineKey>();
    // Chain v3: the epochs an admin handed this device one by one (each checked against the chain as it was taken).
    for (const epoch of await this.deps.hostKeys.epochs(id)) {
      if (epoch >= committed || keys.has(epoch)) continue;
      try {
        keys.set(epoch, await this.deps.hostKeys.key(id, epoch));
      } catch (error) {
        console.warn(`cloud account: ${id}'s key epoch ${epoch} does not open here:`, error);
      }
    }
    return keys;
  }

  async withUserKeys<T>(use: (keys: { epoch: number; key: Bytes }[]) => Promise<T>): Promise<T> {
    this.deps.trust.assertNotForked();
    try {
      await this.deps.receiveMail();
    } catch (error) {
      console.warn("cloud account: mail:", error);
    }
    this.deps.trust.assertNotForked();
    // And again once the keys are read, right before they are handed over.
    return this.deps.vault.withAllRaw((keys) => {
      this.deps.trust.assertNotForked();
      return use(keys);
    });
  }

  async refreshKeys(): Promise<void> {
    if (this.deps.standing() !== "active" || !this.deps.trust.pinned) return;
    try {
      await this.deps.receiveMail();
    } catch (error) {
      console.warn("cloud account: mail:", error);
    }
    await this.deps.keyring.refresh();
    // A device that holds no user key asks an admin for the hosts' keys it lacks (rate-limited).
    if (!this.deps.isAdmin()) {
      try {
        await this.deps.checkHostKeys();
      } catch (error) {
        console.warn("cloud account: host keys:", error);
      }
    }
    this.basics.emit();
  }

  async storeUserKeys(keys: { epoch: number; key: Uint8Array }[]): Promise<void> {
    const chain = this.basics.state;
    if (!chain) throw new AccountError("notInChain", "the account has no chain yet");
    for (const { epoch, key } of keys) await chain.checkUserKey(epoch, key);
    const pinned = await this.deps.trust.pinnedGenesis();
    if (pinned && pinned !== hex(chain.hashAt(0)!)) throw new AccountError("fork", "the hub served a different genesis than this device pinned");
    await this.deps.trust.pin();
    for (const { epoch, key } of keys) await this.deps.vault.put(epoch, key);
    this.basics.emit();
  }
}
