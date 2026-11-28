/*
 * Chain v3 host keys (review D2, S1-4): sealing committed machine keys one by
 * one for devices that do not manage the account, answering their requests,
 * checking gaps, and requesting missing epochs (`CloudAccount` documents the
 * methods it hands to these).
 */
import { type Bytes, type ChainState, type DeviceRecord, encodeHostKeyMail, type MachineKey, type NoiseKeyPair, sealHostKey } from "@/lib/e2e";
import type { HostKeysSent } from "../account-storage";
import { type HubApi, HubApiError, MAIL_HOST_KEY, MAIL_KEY_REQUEST } from "../api";
import { admits, noDeviceRoleYet } from "../chain-policy";
import type { Keyring } from "../keyring";
import type { AccountBasics, AccountNote } from "./basics";
import type { Standing } from "./devices";
import { AccountError } from "./errors";
import { HOST_KEYS_WAIT, KEY_REQUEST_EVERY_MS, MAX_HOST_KEY_MAILS } from "./limits";

/** What host-key distribution reads of the account beyond its basics. */
export interface HostKeysDeps {
  api: HubApi;
  token(): string;
  noiseKeys(): NoiseKeyPair;
  keyring: Keyring;
  isAdmin(): boolean;
  standing(): Standing;
  pinned(): boolean;
  assertNotForked(): void;
  requireAdmin(): ChainState;
  keysWithheldFrom(id: string): boolean;
  machineKeyBytes(machine: string, mkEpoch: number, ukEpoch?: number): Promise<Bytes | null>;
  machineKeys(id: string): Promise<Map<number, MachineKey>>;
  receiveMail(): Promise<number>;
  resendUserKeys(id: string): Promise<number[]>;
  hostKeyRequestNote: AccountNote;
  hostKeysSentNote(to: string): AccountNote;
  getMissingHostKeys(): string[];
  setMissingHostKeys(missing: string[]): void;
}

export class HostKeys {
  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: HostKeysDeps,
  ) {}

  hostKeyRecipients(host: string, chain: ChainState | null = this.basics.state): DeviceRecord[] {
    if (!chain) return [];
    return chain
      .allDevices()
      .filter((d) => d.status.type === "active" && !d.admin && d.info.id !== this.basics.deviceId && admits(chain, host, d.info.id) && !this.deps.keysWithheldFrom(d.info.id));
  }

  /** Seal host `host`'s key epoch `epoch` from this (admin) device for device `to` and mail it (purpose 5). */
  private async mailHostKeyTo(to: DeviceRecord, host: string, epoch: number, key: Uint8Array): Promise<void> {
    this.deps.assertNotForked();
    const ctx = { account: this.basics.account, sender: this.basics.deviceId, recipient: to.info.id, host, epoch };
    const sealed = await sealHostKey(this.deps.noiseKeys(), to.info.dhKey, ctx, key);
    this.deps.assertNotForked();
    await this.deps.api.mail(this.deps.token(), { recipient: to.info.id, sender: this.basics.deviceId, purpose: MAIL_HOST_KEY, epoch, sealed: encodeHostKeyMail(host, sealed) });
  }

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
    if (!this.deps.isAdmin()) return { sent, failed, waiting };
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
    const chain = this.deps.requireAdmin();
    const to = chain.device(id);
    if (!to || id === this.basics.deviceId) throw new AccountError("notInChain", `${id} is not a current device of the account`);
    if (to.admin) throw new AccountError("notAdmin", `${id} manages the account: it reads the hosts' keys with the user key`);
    // A host waiting to renew its identity key, or isolated, is sealed nothing (§4.3): whoever holds its old key would read them.
    if (this.deps.keysWithheldFrom(id)) return [];
    const wanted = chain
      .hosts()
      .filter((h) => h.status.type === "active" && (opts.only === undefined || h.info.id === opts.only) && admits(chain, h.info.id, id))
      .flatMap((h) => Array.from({ length: chain.machineKeyEpochs(h.info.id) }, (_, epoch) => ({ host: h.info.id, epoch })))
      .filter(({ host, epoch }) => opts.after?.[host] === undefined || epoch > opts.after[host]!)
      .sort((a, b) => b.epoch - a.epoch || (a.host < b.host ? -1 : a.host > b.host ? 1 : 0));
    const sealed: { host: string; epoch: number }[] = [];
    if (!wanted.length) return sealed;
    try {
      await this.deps.keyring.refresh();
    } catch {
      // the cached copy
    }
    for (const { host, epoch } of wanted) {
      if (sealed.length >= MAX_HOST_KEY_MAILS) break;
      const key = await this.deps.machineKeyBytes(host, epoch);
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

  /** What this admin sealed device `to` lately (`hostKeysSentKey`). */
  private hostKeysSent(to: string): HostKeysSent | null {
    try {
      const v = JSON.parse(this.deps.hostKeysSentNote(to).read() ?? "null") as HostKeysSent | null;
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
    this.deps.hostKeysSentNote(to).write(JSON.stringify({ atMs, through } satisfies HostKeysSent));
  }

  async answerHostKeyRequest(id: string): Promise<number> {
    const last = this.hostKeysSent(id);
    // A window that starts in the future (this browser's clock went back, or the record is spoiled) is no window (re-review 1).
    const since = last === null ? -1 : Date.now() - last.atMs;
    const within = last !== null && since >= 0 && since < KEY_REQUEST_EVERY_MS;
    const sealed = await this.sealHostKeys(id, within ? { after: last.through } : {});
    this.noteHostKeysSent(id, sealed, !within);
    return sealed.length;
  }

  async handHostKeys(id: string, wait = HOST_KEYS_WAIT, signal?: AbortSignal): Promise<number | null> {
    const deadline = Date.now() + wait.forMs;
    for (;;) {
      const chain = this.basics.state;
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

  async hostKeyGaps(): Promise<{ host: string; epoch: number }[]> {
    const chain = this.basics.state;
    if (!chain || !this.deps.pinned()) return [];
    const gaps: { host: string; epoch: number }[] = [];
    for (const h of chain.hosts()) {
      if (h.status.type !== "active" || h.info.id === this.basics.deviceId || !admits(chain, h.info.id, this.basics.deviceId)) continue;
      const latest = chain.latestMachineEpoch(h.info.id);
      if (latest === null) continue;
      if ((await this.deps.machineKeys(h.info.id)).has(latest)) continue;
      gaps.push({ host: h.info.id, epoch: latest });
    }
    return gaps;
  }

  async checkHostKeys(): Promise<string[]> {
    let gaps = this.deps.isAdmin() ? [] : await this.hostKeyGaps();
    if (gaps.length && this.deps.standing() === "active" && this.deps.pinned()) {
      try {
        await this.deps.receiveMail();
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
    if (missing.join(",") !== this.deps.getMissingHostKeys().join(",")) {
      this.deps.setMissingHostKeys(missing);
      this.basics.emit();
    }
    return missing;
  }

  async requestHostKeys(force = false): Promise<string[]> {
    const state = this.basics.state;
    if (!state || this.deps.standing() !== "active" || !this.deps.pinned() || this.deps.isAdmin()) return [];
    try {
      const last = JSON.parse(this.deps.hostKeyRequestNote.read() ?? "null") as { atMs?: unknown } | null;
      if (!force && typeof last?.atMs === "number" && Date.now() - last.atMs < KEY_REQUEST_EVERY_MS) return [];
    } catch {
      // asked never, or the record is spoiled: ask
    }
    const asked: string[] = [];
    for (const d of state.admins()) {
      if (d.info.id === this.basics.deviceId) continue;
      try {
        await this.deps.api.mail(this.deps.token(), { recipient: d.info.id, sender: this.basics.deviceId, purpose: MAIL_KEY_REQUEST, epoch: 0, sealed: new Uint8Array(0) });
        asked.push(d.info.id);
      } catch (error) {
        console.warn(`cloud account: could not ask ${d.info.id} for the hosts' keys:`, error);
      }
    }
    this.deps.hostKeyRequestNote.write(JSON.stringify({ atMs: Date.now() }));
    return asked;
  }

  async resendKeys(id: string): Promise<number> {
    const record = this.basics.state?.device(id);
    if (!record) throw new AccountError("notInChain", `${id} is not a current device of the account`);
    if (record.admin) return (await this.deps.resendUserKeys(id)).length;
    return this.sendHostKeys(id);
  }
}
