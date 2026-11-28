/*
 * Enrolling machines (proof check, initial key epoch), committing and handing
 * over machine keys (with guarded couriers), mailing user keys, and receiving
 * the account mailbox (`CloudAccount` documents the methods it hands to these).
 */
import {
  type Action,
  type Bytes,
  type ChainState,
  DecodeError,
  decodeHostKeyMail,
  type EnrollCode,
  enrollToken,
  type Entry,
  equal,
  generateEnrollCode,
  hex,
  type MachineInfo,
  machineKeyCommit,
  type NoiseKeyPair,
  openHostKey,
  openUserKey,
  sealTo,
  userKeyCommit,
  verifyEnrollProof,
  WrapError,
} from "@/lib/e2e";
import { type HubApi, HubApiError, MAIL_HOST_KEY, MAIL_KEY_REQUEST, MAIL_PAIRING, MAIL_USER_KEY } from "../api";
import { userKeyStale } from "../chain-policy";
import {
  deliverMachineKey,
  type DeliveryResult,
  type MachineKeyCourier,
  notePendingReencrypt,
  type RenewalCourier,
} from "../deliver";
import { type HostKeyVault, keyName, type Keyring, type UserKeyVault, wrapMachineKey } from "../keyring";
import type { AccountBasics } from "./basics";
import { joinAction, type Standing } from "./devices";
import { AccountError, hostAdminRefused } from "./errors";
import type { FreshUserKey } from "./views";

const random32 = () => crypto.getRandomValues(new Uint8Array(32));

/** `keys` with the `wanted` epochs first, then the rest newest first. */
export function requestedFirst<T extends { epoch: number }>(keys: T[], wanted: ReadonlySet<number>): T[] {
  return [...keys].sort((x, y) => Number(wanted.has(y.epoch)) - Number(wanted.has(x.epoch)) || y.epoch - x.epoch);
}

/** What enrollment, machine-key delivery and mailbox handling read of the account beyond its basics. */
export interface HostEnrollmentDeps {
  api: HubApi;
  token(): string;
  noiseKeys(): NoiseKeyPair;
  vault: UserKeyVault;
  hostKeys: HostKeyVault;
  keyring: Keyring;
  isAdmin(): boolean;
  standing(): Standing;
  pinned(): boolean;
  pairing(): boolean;
  assertNotForked(): void;
  requireWritable(writable: boolean | undefined): void;
  fingerprint(): Promise<Uint8Array>;
  sync(): Promise<number>;
  appendBatch(build: (chain: ChainState) => Promise<Action[]>): Promise<Entry[]>;
  appendWaitingFor(host: string, build: (chain: ChainState) => Promise<Action[]>, wait?: { forMs: number; everyMs: number }): Promise<Entry[]>;
  ensureUserKey(): Promise<number | null>;
  noUserKey(): AccountError;
  ensureRecoveryPackages(scope?: "own" | "any"): Promise<number>;
  mailHostKey(host: string, epoch: number, key: Uint8Array, noRole?: Set<string>): Promise<{ sent: string[]; failed: Record<string, string>; waiting: string[] }>;
  answerHostKeyRequest(id: string): Promise<number>;
  machineKeyBytes(machine: string, mkEpoch: number, ukEpoch?: number): Promise<Bytes | null>;
  isolated(id: string): boolean;
}

export class HostEnrollment {
  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: HostEnrollmentDeps,
  ) {}

  async startEnrollment(): Promise<EnrollCode> {
    return (await this.startTimedEnrollment()).code;
  }

  async startTimedEnrollment(): Promise<{ code: EnrollCode; expiresMs: number | null }> {
    const code = generateEnrollCode(await this.deps.fingerprint());
    const expiresMs = await this.deps.api.createEnrollment(this.deps.token(), this.basics.deviceId, await enrollToken(code));
    return { code, expiresMs: typeof expiresMs === "number" && Number.isFinite(expiresMs) ? expiresMs : null };
  }

  async completeEnrollment(
    code: EnrollCode,
    signal?: AbortSignal,
    pollMs = 1000,
    expect?: { signKey: Uint8Array; dhKey: Uint8Array },
    opts: {
      writable?: boolean;
      wait?: { forMs: number; everyMs: number };
      admin?: boolean;
    } = {},
  ): Promise<MachineInfo> {
    if (opts.admin) throw hostAdminRefused();
    // What the hub would refuse is refused before anything is asked of it: a new host on a v3 chain only while it takes v3 entries.
    if (this.basics.state?.isV3) this.deps.requireWritable(opts.writable);
    const token = await enrollToken(code);
    for (;;) {
      if (signal?.aborted) throw signal.reason ?? new DOMException("aborted", "AbortError");
      const status = await this.deps.api.enrollment(this.deps.token(), token);
      if (status.state === "expired") throw new AccountError("expired", "the enrollment code expired");
      if (status.state === "proved" && status.machine && status.signKey && status.dhKey && status.proof) {
        const ok = await verifyEnrollProof(code, this.basics.account, status.machine, status.signKey, status.dhKey, status.proof);
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
            (build) => this.deps.appendWaitingFor(machine.id, build, opts.wait),
          );
          // The devices that hold no user key read what it writes with this.
          await this.deps.mailHostKey(machine.id, mkEpoch, mk);
        } finally {
          mk.fill(0);
        }
        this.basics.emit();
        return machine;
      }
      await new Promise((resolve) => setTimeout(resolve, pollMs));
    }
  }

  async issueMachineKey(
    id: string,
    courier: MachineKeyCourier,
    reencrypt = false,
    prefix: Action[] | ((chain: ChainState) => Action[]) = [],
  ): Promise<{ mkEpoch: number; ukEpoch: number; result: DeliveryResult; hostKeysTo: string[]; userKey?: FreshUserKey }> {
    if (!this.basics.state?.machine(id)) throw new AccountError("notInChain", "no device {{id}} that runs AI in the chain", undefined, undefined, { id });
    const mk = random32();
    try {
      const { mkEpoch, ukEpoch, userKey } = await this.commitHostKey(id, mk, (chain) => (typeof prefix === "function" ? prefix(chain) : prefix), (build) => this.deps.appendBatch(build));
      // The devices that hold no user key and that it accepts read it with this (only those: review D2).
      const { sent } = await this.deps.mailHostKey(id, mkEpoch, mk);
      this.basics.emit();
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
   * words, mailed to the other admins (never to the device that left).
   */
  async commitHostKey(
    id: string,
    mk: Bytes,
    prefix: (chain: ChainState) => Action[],
    append: (build: (chain: ChainState) => Promise<Action[]>) => Promise<Entry[]>,
  ): Promise<{ mkEpoch: number; ukEpoch: number; userKey: FreshUserKey | null }> {
    const stale = this.basics.state !== null && userKeyStale(this.basics.state);
    // The newest user key, when the new key may go under it: known (or asked for) before anything is committed.
    const wrapEpoch = stale ? null : await this.deps.ensureUserKey();
    if (!stale && wrapEpoch === null) throw this.deps.noUserKey();
    const fresh: { uk: Bytes | null } = { uk: null };
    try {
      const entries = await append(async (chain) => {
        fresh.uk?.fill(0);
        fresh.uk = null;
        const epoch = chain.machineKeyEpochs(id);
        const machines = [{ machine: id, epoch, hash: await machineKeyCommit(this.basics.account, id, epoch, mk) }];
        // Built on the chain as it stands (a newer head too): a new user key when its newest is stale, or is not the one checked above.
        let user: { epoch: number; hash: Bytes } | null = null;
        if (userKeyStale(chain) || wrapEpoch === null || chain.latestUserEpoch() !== wrapEpoch) {
          fresh.uk = random32();
          const ukEpoch = chain.userKeyEpochs();
          user = { epoch: ukEpoch, hash: await userKeyCommit(this.basics.account, ukEpoch, fresh.uk) };
        }
        return [...prefix(chain), { type: "keyEpochs", user, machines }];
      });
      const committed = entries.flatMap((e) => (e.draft.action.type === "keyEpochs" ? [e.draft.action] : []));
      const freshEpoch = committed.find((a) => a.user)?.user?.epoch ?? null;
      const mkEpoch = committed.flatMap((a) => a.machines).find((m) => m.machine === id)?.epoch ?? this.basics.state!.latestMachineEpoch(id)!;
      const newUk = freshEpoch !== null ? fresh.uk : null;
      // Committed: now the new user key may be kept and sealed for the recovery words.
      const recovery = newUk ? await this.keepUserKey(freshEpoch!, newUk) : false;
      const ukEpoch = freshEpoch ?? wrapEpoch!;
      let userKey: FreshUserKey | null = null;
      try {
        await this.deps.keyring.put(keyName.machine(id, mkEpoch, ukEpoch), await wrapMachineKey(await this.deps.vault.key(ukEpoch), this.basics.account, id, mkEpoch, ukEpoch, mk));
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
  async keepUserKey(epoch: number, uk: Bytes): Promise<boolean> {
    await this.deps.vault.put(epoch, uk);
    await this.deps.ensureRecoveryPackages();
    const rsk = this.basics.state?.currentRecoveryKey ?? null;
    return rsk !== null && this.deps.keyring.hasBase(keyName.recovery(epoch, this.basics.deviceId, rsk), this.basics.deviceId);
  }

  /** A new user key epoch to every other current admin (purpose 1; never to a device that does not manage the account). */
  async mailUserKey(epoch: number, uk: Bytes): Promise<{ sealedTo: string[]; failedTo: string[]; failedWhy: Record<string, string> }> {
    const sealedTo: string[] = [];
    const failedTo: string[] = [];
    const failedWhy: Record<string, string> = {};
    for (const d of this.basics.state?.admins() ?? []) {
      if (d.info.id === this.basics.deviceId) continue;
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

  /** Seal `UK` epochs from this device to device `id` and mail them (purpose 1). */
  async mailUserKeys(id: string, dhKey: Uint8Array, keys: { epoch: number; key: Uint8Array }[]): Promise<void> {
    this.deps.assertNotForked();
    for (const { epoch, key } of keys) {
      this.deps.assertNotForked();
      const sealed = await sealTo(this.deps.noiseKeys(), dhKey, { purpose: "userKey", account: this.basics.account, sender: this.basics.deviceId, recipient: id, epoch }, key);
      this.deps.assertNotForked();
      await this.deps.api.mail(this.deps.token(), { recipient: id, sender: this.basics.deviceId, purpose: MAIL_USER_KEY, epoch, sealed });
    }
  }

  async handOverMachineKey(id: string, courier: MachineKeyCourier): Promise<{ mkEpoch: number; result: DeliveryResult }> {
    this.deps.assertNotForked();
    const latest = this.basics.state?.latestMachineEpoch(id) ?? null;
    const found = latest === null ? null : await this.newestMachineKeyBlob(id, latest);
    if (latest === null || !found) {
      const issued = await this.issueMachineKey(id, courier);
      return { mkEpoch: issued.mkEpoch, result: issued.result };
    }
    try {
      this.deps.assertNotForked();
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
  guarded(courier: MachineKeyCourier): MachineKeyCourier {
    return {
      waitOnline: (machine, timeoutMs) => courier.waitOnline(machine, timeoutMs),
      postInChannel: (machine, body) => {
        this.deps.assertNotForked();
        return courier.postInChannel(machine, body);
      },
      mail: (machine, epoch, mk) => {
        this.deps.assertNotForked();
        return courier.mail(machine, epoch, mk);
      },
    };
  }

  /**
   * {@link guarded} for a renewal (host identity rotation §3.2): its
   * `/rh/rotate` and its word on the head go out only from a page whose view
   * has not forked — the fork latch holds them as it holds every key.
   */
  guardedRenewal(courier: RenewalCourier): RenewalCourier {
    return {
      ...this.guarded(courier),
      rotate: (machine, body) => {
        this.deps.assertNotForked();
        return courier.rotate(machine, body);
      },
      attest: () => {
        this.deps.assertNotForked();
        return courier.attest();
      },
    };
  }

  private async newestMachineKeyBlob(id: string, mkEpoch: number): Promise<Bytes | null> {
    try {
      await this.deps.keyring.refresh();
    } catch {
      // the cached copy
    }
    return this.deps.machineKeyBytes(id, mkEpoch);
  }

  /** Hand `mk` to the agent (`deliver.ts`); a re-encryption that could only go by mailbox is noted, pinned to this very key, to ask again. */
  async handOver(id: string, mkEpoch: number, ukEpoch: number, mk: Uint8Array, courier: MachineKeyCourier, reencrypt: boolean): Promise<DeliveryResult> {
    this.deps.assertNotForked();
    // An isolated host is handed no key at all (§3.8): its identity may be someone else's now. It gets one with its next renewal.
    if (this.deps.isolated(id)) return { via: "refused", status: 0, message: "isolated: renew its identity key on the device first", code: "isolated" };
    const result = await deliverMachineKey(this.guarded(courier), id, mkEpoch, mk, reencrypt);
    if (result.via === "mailbox" && reencrypt) {
      // The mailbox carries no re-encryption request: ask again in the next channel.
      notePendingReencrypt(this.basics.account, { machine: id, mkEpoch, ukEpoch, mkHash: hex(await machineKeyCommit(this.basics.account, id, mkEpoch, mk)) });
      result.pending = true;
    }
    return result;
  }

  async receiveMail(): Promise<number> {
    if (this.deps.standing() !== "active" || !this.basics.state || !this.deps.pinned()) return 0;
    const mail = await this.deps.api.mailbox(this.deps.token());
    if (!mail.length) return 0;
    const hostKeyAhead = (m: { sender: string; epoch: number; sealed: Uint8Array }) => {
      try {
        return !this.basics.state!.admin(m.sender) || this.basics.state!.machineKeyEpochs(decodeHostKeyMail(m.sealed).host) <= m.epoch;
      } catch {
        return false;
      }
    };
    if (
      mail.some(
        (m) =>
          (m.purpose === MAIL_USER_KEY && (!this.basics.state!.admin(m.sender) || this.basics.state!.userKeyEpochs() <= m.epoch)) ||
          (m.purpose === MAIL_HOST_KEY && hostKeyAhead(m)) ||
          (m.purpose === MAIL_KEY_REQUEST && !this.basics.state!.device(m.sender)),
      )
    ) {
      // A sender, or an epoch, this view does not know yet may be one it is behind on.
      await this.deps.sync();
    }
    const state = this.basics.state!;
    let fresh = 0;
    const asking = new Map<string, Set<number>>();
    /** Devices that are not admins asking for the hosts' machine keys (chain v3): answered with those, never a user key. */
    const askingHostKeys = new Set<string>();
    for (const m of mail) {
      if (m.purpose === MAIL_KEY_REQUEST) {
        const peer = state.device(m.sender);
        if (m.sender !== this.basics.deviceId && peer?.admin) {
          // Another current full device lacks a user key epoch this one holds:
          // it gets every epoch held (to its chain key), once per batch.
          if (await this.deps.vault.has(m.epoch)) asking.set(m.sender, (asking.get(m.sender) ?? new Set<number>()).add(m.epoch));
        } else if (m.sender !== this.basics.deviceId && peer && this.deps.isAdmin()) {
          // A device that does not manage the account (§3.3) — a host's
          // device role too (S1-4: the local install): only the hosts'
          // machine keys, once per batch.
          askingHostKeys.add(m.sender);
        }
        await this.deps.api.ackMail(this.deps.token(), m.id);
      } else if (m.purpose === MAIL_HOST_KEY) {
        // A host's machine key epoch an admin sealed for this device: from a
        // current admin, for this device, exactly the key the chain committed
        // for that host and epoch (`openHostKey`) — else dropped.
        try {
          const { host, sealed } = decodeHostKeyMail(m.sealed);
          const raw = await openHostKey(this.deps.noiseKeys(), this.basics.deviceId, state, { account: this.basics.account, sender: m.sender, recipient: this.basics.deviceId, host, epoch: m.epoch }, sealed);
          try {
            if (await this.deps.hostKeys.put(host, m.epoch, raw)) fresh++;
          } finally {
            raw.fill(0);
          }
        } catch (error) {
          if (!(error instanceof WrapError) && !(error instanceof DecodeError)) throw error;
          console.warn(`cloud account: dropped a host key mailed by ${m.sender}: ${error.message}`);
        }
        await this.deps.api.ackMail(this.deps.token(), m.id);
      } else if (m.purpose === MAIL_USER_KEY) {
        const ctx = { purpose: "userKey" as const, account: this.basics.account, sender: m.sender, recipient: this.basics.deviceId, epoch: m.epoch };
        try {
          const raw = await openUserKey(this.deps.noiseKeys(), this.basics.deviceId, state, ctx, m.sealed);
          try {
            if (await this.deps.vault.put(m.epoch, raw)) fresh++;
          } finally {
            raw.fill(0);
          }
        } catch (error) {
          if (!(error instanceof WrapError)) throw error;
          console.warn(`cloud account: dropped a key mailed by ${m.sender}: ${error.message}`);
        }
        await this.deps.api.ackMail(this.deps.token(), m.id);
      } else if (m.purpose === MAIL_PAIRING) {
        // A pairing delivery is for a device pairing (again): stale here.
        if (!this.deps.pairing()) await this.deps.api.ackMail(this.deps.token(), m.id);
      } else {
        // A purpose this device does not handle (a machine key, or a newer
        // kind — `knownMailPurpose` is false): it would sit in the mailbox
        // for good, filling it, so it is acknowledged unread (M2 re-review;
        // relay-and-plans §6).
        await this.deps.api.ackMail(this.deps.token(), m.id);
      }
    }
    for (const [id, wanted] of asking) {
      // A user key goes to a current admin only (chain v3 §3.3: never to a host or a plain device).
      const peer = this.basics.state!.admin(id);
      if (!peer) continue;
      try {
        // The epochs it asked for first, then the rest newest first: past the
        // hub's mail rate, what it needs still arrives (the review of the M2
        // re-review fixes, N5: oldest first, the asked-for epoch could be the
        // one the rate cut off).
        await this.deps.vault.withAllRaw((keys) => this.mailUserKeys(id, peer.info.dhKey, requestedFirst(keys, wanted)));
      } catch (error) {
        console.warn(`cloud account: could not send ${id} the user keys it asked for:`, error);
      }
    }
    for (const id of askingHostKeys) {
      try {
        await this.deps.answerHostKeyRequest(id);
      } catch (error) {
        console.warn(`cloud account: could not send ${id} the machine keys it asked for:`, error);
      }
    }
    if (fresh) {
      try {
        await this.deps.keyring.refresh();
      } catch {
        // later
      }
      this.basics.emit();
    }
    return fresh;
  }
}
