/*
 * Account membership & chain v3 changes: pairing devices, promoting/demoting
 * admins, toggling host status, updating accept policies, removing devices
 * (including self-removal and vetoes), rotating keys, upgrading to chain v3,
 * and approving pending devices (`CloudAccount` documents the methods it hands
 * to these).
 */
import {
  type Accept,
  type Action,
  type Bytes,
  type ChainState,
  type DeviceKeys,
  type Entry,
  type MachineKeyCommit,
  machineKeyCommit,
  MAX_KEY_COMMITS,
  type NoiseKeyPair,
  RULES,
  userKeyCommit,
  utf8,
} from "@/lib/e2e";
import { type HubApi, HubApiError } from "../api";
import { type AcceptChange, acceptChange, admits, compareBytes, NO_COURIER, noDeviceRoleYet, userKeyStale } from "../chain-policy";
import type { DeliveryResult, MachineKeyCourier } from "../deliver";
import { keyName, type Keyring, rewrapMachineKeys, type UserKeyVault, wrapMachineKey } from "../keyring";
import { type Candidate, completePairing } from "../pairing";
import type { AccountBasics } from "./basics";
import { hasRunAi, type SelfRemoval, type Standing } from "./devices";
import { AccountError, heldWho, hostAdminRefused, ranAiAdminRefused } from "./errors";
import type { ActivationHold } from "./held-back";
import type { RenewalNotes } from "./renewal-notes";
import type { FreshUserKey, HeldBack, RotationReport, VetoPlan } from "./views";

const random32 = () => crypto.getRandomValues(new Uint8Array(32));

/** What membership changes and key rotations read of the account beyond its basics. */
export interface AccountMembershipDeps {
  api: HubApi;
  token(): string;
  keys: DeviceKeys;
  noiseKeys(): NoiseKeyPair;
  vault: UserKeyVault;
  keyring: Keyring;
  hold: ActivationHold;
  renewalNotes: Pick<RenewalNotes, "refuseExposedAdmission">;
  isAdmin(): boolean;
  standing(): Standing;
  pinned(): boolean;
  forked(): string | null;
  assertNotForked(): void;
  requireAdmin(): ChainState;
  requireUpgraded(chain: ChainState): void;
  requireWritable(writable: boolean | undefined): void;
  sync(): Promise<number>;
  append(action: Action): Promise<void>;
  appendBatch(
    build: (chain: ChainState) => Promise<Action[]>,
    opts?: { overHeld?: boolean; asV3?: boolean; beforePublish?: (base: ChainState) => void },
  ): Promise<Entry[]>;
  appendWaitingFor(host: string, build: (chain: ChainState) => Promise<Action[]>, wait?: { forMs: number; everyMs: number }): Promise<Entry[]>;
  heldUserKey(id: string): boolean;
  ensureUserKey(): Promise<number | null>;
  noUserKey(): AccountError;
  withUserKeys<T>(use: (keys: { epoch: number; key: Bytes }[]) => Promise<T>): Promise<T>;
  keepUserKey(epoch: number, uk: Bytes): Promise<boolean>;
  mailUserKey(epoch: number, uk: Bytes): Promise<{ sealedTo: string[]; failedTo: string[]; failedWhy: Record<string, string> }>;
  mailUserKeys(id: string, dhKey: Uint8Array, keys: { epoch: number; key: Uint8Array }[]): Promise<void>;
  mailHostKey(host: string, epoch: number, key: Uint8Array, noRole?: Set<string>): Promise<{ sent: string[]; failed: Record<string, string>; waiting: string[] }>;
  sendHostKeys(id: string, only?: string): Promise<number>;
  commitHostKey(
    id: string,
    mk: Bytes,
    prefix: (chain: ChainState) => Action[],
    append: (build: (chain: ChainState) => Promise<Action[]>) => Promise<Entry[]>,
  ): Promise<{ mkEpoch: number; ukEpoch: number; userKey: FreshUserKey | null }>;
  issueMachineKey(
    id: string,
    courier: MachineKeyCourier,
    reencrypt?: boolean,
    prefix?: Action[] | ((chain: ChainState) => Action[]),
  ): Promise<{ mkEpoch: number; ukEpoch: number; result: DeliveryResult; hostKeysTo: string[]; userKey?: FreshUserKey }>;
  handOver(id: string, mkEpoch: number, ukEpoch: number, mk: Uint8Array, courier: MachineKeyCourier, reencrypt: boolean): Promise<DeliveryResult>;
  machineCheck(machine: string): (mkEpoch: number, key: Bytes) => Promise<void>;
  approvePasskeyJoin(c: Candidate): Promise<void>;
}

export class AccountMembership {
  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: AccountMembershipDeps,
  ) {}

  async pairDevice(candidate: Candidate, opts: { admin: boolean }): Promise<"delivered" | "redelivered"> {
    // The hub's chain as it stands first: a self-activation this page has not
    // seen yet is held back here (and so refuses the pairing) before any key goes.
    await this.deps.sync();
    this.deps.hold.refuseWhileHeld();
    const chain = this.deps.requireAdmin();
    const listed = chain.deviceRecord(candidate.device);
    if (!opts.admin && !listed) this.deps.requireUpgraded(chain);
    // A device that joined with a passkey (waiting, or replaced meanwhile):
    // approved here, by this very pairing — then its keys go in the delivery.
    const joining = listed?.viaPasskey != null && listed.status.type !== "active";
    if (joining) await this.deps.approvePasskeyJoin(candidate);
    const done = await completePairing(
      {
        // The delivery (every user key epoch, sealed) leaves only while this view has not forked, nor holds an activation back.
        api: {
          mail: (token, mail) => {
            this.deps.assertNotForked();
            this.deps.hold.refuseWhileHeld();
            return this.deps.api.mail(token, mail);
          },
        },
        token: this.deps.token(),
        device: this.basics.deviceId,
        signPublic: this.deps.keys.signPublic,
        noise: this.deps.noiseKeys(),
        chain: () => this.basics.state!,
        append: (action) => this.deps.append(action),
        withUserKeys: (use) => this.deps.withUserKeys(use),
      },
      candidate,
      opts,
    );
    const record = this.basics.state?.device(candidate.device);
    if (record && !record.admin && !record.host) {
      try {
        await this.deps.sendHostKeys(record.info.id);
      } catch (error) {
        console.warn(`cloud account: could not send ${record.info.id} the hosts' keys (it asks again):`, error);
      }
    }
    return joining ? "delivered" : done;
  }

  async setAdmin(id: string, admin: boolean, opts: { courier?: MachineKeyCourier; reencrypt?: boolean } = {}): Promise<RotationReport | null> {
    const { report, keysFailed } = await this.writeAdmin(id, admin, opts);
    if (keysFailed !== null) console.warn(`cloud account: could not send ${id} its keys (re-send them from the devices page):`, keysFailed);
    return report;
  }

  async promote(id: string): Promise<{ keysFailed: unknown }> {
    const { keysFailed } = await this.writeAdmin(id, true, {});
    return { keysFailed };
  }

  /** {@link setAdmin}'s work, with what mailing the device its keys came to (`null`: sent, or nothing to send). */
  private async writeAdmin(id: string, admin: boolean, opts: { courier?: MachineKeyCourier; reencrypt?: boolean }): Promise<{ report: RotationReport | null; keysFailed: unknown }> {
    const chain = this.deps.requireAdmin();
    this.deps.requireUpgraded(chain);
    const record = chain.device(id);
    if (!record) throw new AccountError("notInChain", `${id} is not a current device of the account`);
    if (record.admin === admin) return { report: null, keysFailed: null };
    if (admin && id === this.basics.deviceId) throw new AccountError("self", "a device cannot make itself a full device: do it from another full device");
    if (admin && record.host) throw hostAdminRefused();
    // D1 extended: nor one that ran AI and stopped — refused here, before anything is signed.
    if (admin && hasRunAi(chain, record)) throw ranAiAdminRefused(id);
    if (!admin && chain.admins().length <= 1) throw new AccountError("lastAdmin", "the account's last full device stays one");
    const entry: Action = { type: "setAdmin", id, admin };
    let report: RotationReport | null = null;
    if (!admin && opts.courier && id !== this.basics.deviceId) report = await this.rotate(opts.courier, opts.reencrypt ?? false, [entry]);
    else await this.deps.append(entry);
    const now = this.basics.state?.device(id);
    if (!now || now.host || id === this.basics.deviceId) return { report, keysFailed: null };
    try {
      if (admin) await this.deps.withUserKeys((keys) => this.deps.mailUserKeys(id, now.info.dhKey, keys));
      else await this.deps.sendHostKeys(id);
    } catch (error) {
      return { report, keysFailed: error };
    }
    return { report, keysFailed: null };
  }

  async setHost(id: string, host: boolean, courier: MachineKeyCourier, opts: { writable?: boolean } = {}): Promise<{ mkEpoch: number; result: DeliveryResult; userKey?: FreshUserKey } | null> {
    const chain = this.deps.requireAdmin();
    this.deps.requireUpgraded(chain);
    const record = chain.device(id);
    if (!record) throw new AccountError("notInChain", `${id} is not a current device of the account`);
    if (record.host === host) return null;
    if (!host) {
      await this.deps.append({ type: "setHost", id, host: false });
      return null;
    }
    if (id === this.basics.deviceId) throw new AccountError("self", "a device cannot make itself run AI");
    if (record.admin) throw hostAdminRefused();
    this.deps.requireWritable(opts.writable);
    // Only a computer that ran AI before (an enrolled machine) runs it again.
    if (chain.machineKeyEpochs(id) === 0) throw new AccountError("notInChain", "{{id}} never ran AI: add it as a device that runs AI instead", undefined, undefined, { id });
    const mk = random32();
    try {
      // [setHost, keyEpochs] in one call — a new user key in it too while the newest is stale (`commitHostKey`).
      const { mkEpoch, ukEpoch, userKey } = await this.deps.commitHostKey(
        id,
        mk,
        () => [{ type: "setHost", id, host: true }],
        (build) => this.deps.appendWaitingFor(id, build),
      );
      await this.deps.mailHostKey(id, mkEpoch, mk);
      this.basics.emit();
      return { mkEpoch, result: await this.deps.handOver(id, mkEpoch, ukEpoch, mk, courier, false), ...(userKey ? { userKey } : {}) };
    } finally {
      mk.fill(0);
    }
  }

  async setAccept(host: string, from: Accept, courier: MachineKeyCourier = NO_COURIER): Promise<AcceptChange> {
    const chain = this.deps.requireAdmin();
    this.deps.requireUpgraded(chain);
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
    this.deps.renewalNotes.refuseExposedAdmission(chain, host, accept);
    const setAccept: Action = { type: "setAccept", host, from: accept };
    const checked = (c: ChainState): Action[] => {
      this.deps.renewalNotes.refuseExposedAdmission(c, host, accept);
      return [setAccept];
    };
    if (shutOut.length) {
      if (courier === NO_COURIER) throw new AccountError("invalidEntry", "shutting a device out hands the device that runs AI a new key: say how", undefined, RULES.hostKeyNext);
      // Those shut out may have read its key: a new epoch, in the same call (review D2).
      const issued = await this.deps.issueMachineKey(host, courier, false, checked);
      return { narrowed: true, admitted, shutOut, mkEpoch: issued.mkEpoch, result: issued.result, hostKeysTo: issued.hostKeysTo, ...(issued.userKey ? { userKey: issued.userKey } : {}) };
    }
    await this.deps.appendBatch(async (c) => checked(c));
    // Let in: the host's committed keys, so it reads what the host wrote.
    for (const id of admitted) {
      try {
        await this.deps.sendHostKeys(id, host);
      } catch (error) {
        // A host whose device role has not signed in yet asks itself once it has.
        const to = this.basics.state?.device(id);
        if (!to || !noDeviceRoleYet(to, error)) console.warn(`cloud account: could not send ${id} ${host}'s keys (it asks again):`, error);
      }
    }
    return { narrowed: false, admitted, shutOut };
  }

  async removeDevice(
    id: string,
    courier: MachineKeyCourier,
    reencrypt = false,
    opts: {
      rotateMachineKeys?: boolean;
      veto?: Pick<VetoPlan, "device" | "seq" | "view">;
      alsoTrusted?: readonly string[];
    } = {},
  ): Promise<RotationReport> {
    if (id === this.basics.deviceId) throw new AccountError("self", "remove this device from another device");
    const veto = opts.veto?.device === id ? opts.veto : undefined;
    const { held, sweepTo } = this.deps.hold.vetoedHold(id, veto);
    if (!held) {
      const record = this.basics.state?.deviceRecord(id);
      const v3 = this.basics.state?.isV3 === true;
      const none: RotationReport = { ukEpoch: null, sealedTo: [], failedTo: [], failedWhy: {}, recovery: false, machines: [] };
      if (v3) {
        const removal: Action = { type: "removeDevice3", id };
        // It ever held the user key (review 1: by the chain's history, not its flags now): everything rotates.
        if (this.deps.heldUserKey(id)) return this.rotate(courier, reencrypt, [removal]);
        // One that could control a host may have its key: the machine keys rotate whatever was asked (review D2).
        if (opts.rotateMachineKeys === false && !this.controlsAHost(id)) {
          this.deps.requireAdmin();
          await this.deps.append(removal);
          return none;
        }
        return this.rotate(courier, reencrypt, [removal], { userKey: false });
      }
      if (record?.host && !record.admin) {
        this.deps.requireAdmin();
        await this.deps.append({ type: "removeMachine", id });
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
    const view = sweepTo === null || !veto ? this.deps.hold.trustedView() : new Set([...veto.view, this.basics.deviceId]);
    try {
      const report = await this.rotate(
        courier,
        reencrypt,
        (chain) => {
          const live = this.deps.hold.heldBack;
          // Another device's activation held back meanwhile: the veto is not signed on top of it (that would take it in).
          if (live && live.device !== held.device) throw new AccountError("heldBack", `${heldWho(live)}: approve or remove it first`, live.readyMs);
          // The hold as it stands now (its entries may have grown); taken in on
          // the way, everything the chain the veto is signed on has from the
          // activation on — what the held device added since the plan included;
          // taken in before the call, up to where this view took it in.
          const now: HeldBack = live ?? { ...held, entries: chain.allEntries().filter((e) => e.draft.seq >= held.seq && (sweepTo === null || e.draft.seq <= sweepTo)) };
          const scope = this.deps.hold.vetoScope(now, chain, { view, ...(alsoTrusted ? { trusted: alsoTrusted } : {}) });
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
      this.deps.hold.planDone(held.seq);
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

  selfRemoval(): SelfRemoval {
    const chain = this.basics.state;
    if (!chain || this.deps.standing() !== "active" || this.deps.forked() !== null) return "notActive";
    if (!this.deps.isAdmin()) return "notAdmin";
    if (!this.deps.pinned()) return "notActive";
    if (!chain.admins().some((d) => d.info.id !== this.basics.deviceId)) return "lastAdmin";
    const me = this.basics.deviceId;
    const slots = chain.allSlots().some((s) => !s.removed && (s.setBy === me || chain.deviceRecord(me)?.viaPasskey === s.slot.slot));
    if (chain.voidsRecoveryWords(me, "removal") || slots) return "keepsSignIn";
    if (this.deps.hold.heldBack) return "heldBack";
    return "removable";
  }

  async removeSelf(): Promise<void> {
    const why = this.selfRemoval();
    if (why !== "removable") {
      const kind = ({ notActive: "notInChain", notAdmin: "notAdmin", lastAdmin: "lastAdmin", keepsSignIn: "self", heldBack: "heldBack" } as const)[why];
      throw new AccountError(kind, `this device cannot remove itself (${why}): remove it from another full device`);
    }
    const me = this.basics.deviceId;
    await this.deps.appendBatch(async (chain) => {
      if (!chain.admins().some((d) => d.info.id !== me)) throw new AccountError("lastAdmin", "the account's last full device stays one");
      return [chain.isV3 ? { type: "removeDevice3", id: me } : { type: "removeDevice", id: me }];
    });
  }

  controlsAHost(id: string): boolean {
    const chain = this.basics.state;
    if (!chain) return false;
    return chain.hosts().some((h) => h.status.type === "active" && h.info.id !== id && admits(chain, h.info.id, id));
  }

  async rotate(
    courier: MachineKeyCourier,
    reencrypt = false,
    prefix: Action[] | ((chain: ChainState) => Action[]) = [],
    opts: { overHeld?: boolean; userKey?: boolean; machineKeys?: boolean; asV3?: boolean } = {},
  ): Promise<RotationReport> {
    if (!this.basics.state || this.deps.standing() !== "active" || !this.deps.pinned()) throw new AccountError("notInChain", "only an active device rotates the keys");
    if (!this.deps.isAdmin()) throw new AccountError("notAdmin", "only a full device of the account rotates the keys");
    const withMachineKeys = opts.machineKeys !== false;
    try {
      await this.deps.sync();
    } catch (error) {
      console.warn("cloud account: sync before rotating:", error);
    }
    try {
      await this.deps.keyring.refresh();
    } catch {
      // the cached copy
    }
    // Machine keys never go under a stale user key: a new one comes with them then.
    const withUserKey = opts.userKey !== false || userKeyStale(this.basics.state!);
    // Machine keys alone are wrapped under the newest user key, which this device must hold — known before anything is committed.
    const wrapEpoch = withUserKey ? null : await this.deps.ensureUserKey();
    if (!withUserKey && wrapEpoch === null) throw this.deps.noUserKey();
    let uk: Bytes | null = null;
    let mks: { machine: string; mk: Bytes }[] = [];
    const zero = () => {
      uk?.fill(0);
      for (const m of mks) m.mk.fill(0);
    };
    let vetoed: RotationReport["vetoed"];
    try {
      const entries = await this.deps.appendBatch(async (chain) => {
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
          commits.push({ machine, epoch, hash: await machineKeyCommit(this.basics.account, machine, epoch, mk) });
        }
        const keyEpochs: Action[] = [];
        const user = fresh ? { epoch: ukEpoch, hash: await userKeyCommit(this.basics.account, ukEpoch, fresh) } : null;
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
      const recovery = ukEpoch !== null && newUk ? await this.deps.keepUserKey(ukEpoch, newUk) : false;
      const prepared: { machine: string; mkEpoch: number; mk: Bytes; keyring?: string }[] = [];
      const userKey = await this.deps.vault.key(wrapUnder);
      for (const { machine, mk } of mks) {
        const mkEpoch = mkEpochs.get(machine)!;
        try {
          await this.deps.keyring.put(keyName.machine(machine, mkEpoch, wrapUnder), await wrapMachineKey(userKey, this.basics.account, machine, mkEpoch, wrapUnder, mk));
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
          const state = this.basics.state!;
          const machines = state
            .hosts()
            .filter((m) => m.status.type === "active")
            .map((m) => ({ id: m.info.id, epochs: state.machineKeyEpochs(m.info.id) }));
          const { rewrapped: done, failed } = await rewrapMachineKeys(this.deps.vault, this.deps.keyring, this.basics.account, machines, ukEpoch, (m) => this.deps.machineCheck(m));
          if (failed.length) console.warn("cloud account: older machine keys not rewrapped under the new user key:", failed);
          rewrapped = { done, failed };
        } catch (error) {
          console.warn("cloud account: rewrapping older machine keys:", error);
        }
      }
      this.basics.emit();
      const { sealedTo, failedTo, failedWhy } = ukEpoch !== null && newUk ? await this.deps.mailUserKey(ukEpoch, newUk) : { sealedTo: [], failedTo: [], failedWhy: {} };
      // The devices that hold no user key read the hosts with these (chain v3; none on a v2 chain).
      const hostKeysTo = new Set<string>();
      const hostKeysFailed: Record<string, string> = {};
      const noRole = new Set<string>();
      for (const p of prepared) {
        const { sent, failed } = await this.deps.mailHostKey(p.machine, p.mkEpoch, p.mk, noRole);
        for (const id of sent) hostKeysTo.add(id);
        Object.assign(hostKeysFailed, failed);
      }
      for (const id of Object.keys(hostKeysFailed)) hostKeysTo.delete(id);
      const machines = await Promise.all(
        prepared.map(async (p) => {
          const keyring = p.keyring !== undefined ? { keyring: p.keyring } : {};
          try {
            return { machine: p.machine, mkEpoch: p.mkEpoch, result: await this.deps.handOver(p.machine, p.mkEpoch, wrapUnder, p.mk, courier, reencrypt), ...keyring };
          } catch (error) {
            return { machine: p.machine, mkEpoch: p.mkEpoch, error: error instanceof Error ? error.message : String(error), ...keyring };
          }
        }),
      );
      this.basics.emit();
      const hostKeys = hostKeysTo.size || Object.keys(hostKeysFailed).length ? { hostKeysTo: [...hostKeysTo], hostKeysFailed } : {};
      return { ukEpoch, sealedTo, failedTo, failedWhy, recovery, machines, ...hostKeys, ...(vetoed ? { vetoed } : {}), ...(rewrapped ? { rewrapped } : {}) };
    } finally {
      zero();
    }
  }

  upgradePlan(): { version: 3; ukEpoch: number; machines: 0 } | null {
    const chain = this.basics.state;
    if (!chain || chain.isV3) return null;
    return { version: 3, ukEpoch: chain.userKeyEpochs(), machines: 0 };
  }

  async upgradeToV3(opts: { writable?: boolean }): Promise<RotationReport> {
    const chain = this.deps.requireAdmin();
    if (chain.isV3) throw new AccountError("invalidEntry", "the account is on chain v3 already", undefined, "upgraded");
    this.deps.requireWritable(opts.writable);
    // No machine key changes: nothing is handed to an agent.
    return this.rotate(NO_COURIER, false, [], { machineKeys: false, asV3: true });
  }

  async approveDevice(id: string): Promise<void> {
    const held = this.deps.hold.heldBack;
    if (held && held.device === id) return this.deps.hold.vouchForHeld(held);
    const record = this.basics.state?.deviceRecord(id);
    if (record?.status.type !== "pending") throw new AccountError("notPending", `${id} is not waiting for approval`);
    // One that joined with a passkey is approved only by pairing with it (`pairDevice`): its id alone says nothing of whose screen it is.
    if (record.viaPasskey !== null) {
      throw new AccountError("notPending", "{{id}} joined with a passkey: approve it by scanning its QR code or typing its code in my devices → add a browser or phone", undefined, undefined, { id });
    }
    await this.deps.append({ type: "approveDevice", id });
    // A user key only to a device that manages the account (a passkey's joins do not: chain v3 §3.2).
    if (record.admin) await this.deps.withUserKeys((keys) => this.deps.mailUserKeys(id, record.info.dhKey, keys));
    else if (!record.host) await this.deps.sendHostKeys(id);
  }
}
