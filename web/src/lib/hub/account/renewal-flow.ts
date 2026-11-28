/*
 * Host identity key renewal execution and renewal pauses
 * (docs/host-identity-rotation.md §3–§5): registering attempts, asking hosts
 * to stage, verifying answers and SAS codes, resuming after reloads, writing
 * `[rotateHostKey, keyEpochs]`, setting renewal policy, and writing/ending
 * renewal pauses (`CloudAccount` documents the methods it hands to these).
 */
import {
  type Action,
  ASSURANCE_ON_DEVICE,
  type Assurance,
  type Bytes,
  type ChainState,
  copy,
  encodeDraft,
  type Entry,
  equal,
  exportPublic,
  fromHex,
  hex,
  type HostRotation,
  hostRotationBody,
  type OpenPause,
  PAUSE_CANCELLED,
  PAUSE_COMPLETED,
  PAUSE_START,
  type PauseRemoval,
  rotateSas,
  sha256,
  verifyRotateConfirm,
} from "@/lib/e2e";
import { type AccountLocks, renewalLockName } from "../account-storage";
import { type HostRotationView, type HubApi, HubApiError } from "../api";
import {
  checkRotateAnswer,
  edgeHeld,
  exposedControllers,
  hostsToRotate,
  noDeviceRoleYet,
  NothingToWrite,
  parseRotateAnswer,
  type PauseEnding,
  pauseReturns,
  renewalRefusal,
  type RotateAnswer,
  rotateAnswerJson,
} from "../chain-policy";
import {
  ChannelUnavailable,
  type DeliveryResult,
  type MachineKeyCourier,
  type RenewalCourier,
  type RotateRequest,
  STALE_GRACE_MS,
} from "../deliver";
import type { KeptRenewal, RenewalStash } from "../keyring";
import type { AccountBasics } from "./basics";
import { deviceFingerprint } from "./devices";
import { AccountError } from "./errors";
import { HOST_KEYS_WAIT } from "./limits";
import { answeredWithin, BUSY, CODE_WAIT_MS, RENEW_RESTARTS, RenewalError, type RenewalReport, type RenewOptions } from "./renewal";
import type { RenewalNotes } from "./renewal-notes";
import type { FreshUserKey } from "./views";

const random32 = () => crypto.getRandomValues(new Uint8Array(32));

/** What host identity key renewal and pause management read of the account beyond its basics. */
export interface RenewalFlowDeps {
  api: HubApi;
  token(): string;
  locks(): AccountLocks | null;
  renewalStash: RenewalStash;
  renewalNotes: RenewalNotes;
  assertNotForked(): void;
  requireAdmin(): ChainState;
  requireUpgraded(chain: ChainState): void;
  checkRenewalOn(chain: ChainState, host: string, rotation: HostRotation, assurance: Assurance): void;
  sync(): Promise<number>;
  append(action: Action): Promise<void>;
  appendBatch(
    build: (chain: ChainState) => Promise<Action[]>,
    opts?: { overHeld?: boolean; asV3?: boolean; beforePublish?: (base: ChainState) => void },
  ): Promise<Entry[]>;
  guardedRenewal(courier: RenewalCourier): RenewalCourier;
  commitHostKey(
    id: string,
    mk: Bytes,
    prefix: (chain: ChainState) => Action[],
    append: (build: (chain: ChainState) => Promise<Action[]>) => Promise<Entry[]>,
  ): Promise<{ mkEpoch: number; ukEpoch: number; userKey: FreshUserKey | null }>;
  mailHostKey(host: string, epoch: number, key: Uint8Array, noRole?: Set<string>): Promise<{ sent: string[]; failed: Record<string, string>; waiting: string[] }>;
  handOver(id: string, mkEpoch: number, ukEpoch: number, mk: Uint8Array, courier: MachineKeyCourier, reencrypt: boolean): Promise<DeliveryResult>;
  handHostKeys(id: string, wait?: { forMs: number; everyMs: number }, signal?: AbortSignal): Promise<number | null>;
  sendHostKeys(id: string, only?: string): Promise<number>;
}

export class RenewalFlow {
  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: RenewalFlowDeps,
  ) {}

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
  checkRenewalOn(chain: ChainState, host: string, rotation: HostRotation, assurance: Assurance): void {
    if (!chain.machine(host)) throw new AccountError("notInChain", "{{host}} is not a device of the account that runs AI", undefined, undefined, { host });
    const exposed = hostsToRotate(chain, (h) => this.deps.renewalNotes.renews(h));
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
    this.deps.renewalNotes.checkAssurance(chain, host, assurance);
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
    const chain0 = this.deps.requireAdmin();
    this.deps.requireUpgraded(chain0);
    if (!chain0.machine(host)) throw new AccountError("notInChain", "{{host}} is not a device of the account that runs AI", undefined, undefined, { host });
    // Controllers first (§4.1): never ahead of an exposed host it lets control it — whoever asks, the runner or a person.
    this.deps.renewalNotes.requireRenewFirst(host);
    // The account chose how renewals are confirmed first (§5.6 「打开的顺序」).
    if (chain0.renewalPolicySeq === null) throw new RenewalError("policyUnset", "choose how identity key renewals are confirmed first");
    if (assurance !== ASSURANCE_ON_DEVICE && this.deps.renewalNotes.isolated(host)) {
      throw new RenewalError("needsAssurance", `${host} is isolated: renew its identity key with the code its screen shows`);
    }
    // The account's policy, or this device's own guard (§4.4): no automatic renewal is signed here.
    if (assurance !== ASSURANCE_ON_DEVICE && this.deps.renewalNotes.renewalsAssured) {
      throw new RenewalError("needsAssurance", "renewals are confirmed on the device here", this.deps.renewalNotes.localAssured() ? "local" : "policy");
    }
  }

  /**
   * `work` holding this browser's lock on renewing `host` (`navigator.locks`),
   * from before the attempt is registered until it is written or withdrawn —
   * or {@link BUSY}, without running it, while another tab of this browser
   * holds it: that tab is alive, and on it. Without Web Locks it just runs.
   */
  private async holdRenewal<T>(host: string, work: () => Promise<T>): Promise<T | typeof BUSY> {
    const locks = this.deps.locks();
    if (!locks) return work();
    return locks.request<T | typeof BUSY>(renewalLockName(this.basics.account, host), { mode: "exclusive", ifAvailable: true }, async (lock) => (lock === null ? BUSY : await work()));
  }

  /** Keep `kept` for the next page of this browser ({@link resumeRenewal}); a store that fails leaves it as before: lost with this page. */
  private async keepRenewal(kept: KeptRenewal): Promise<void> {
    try {
      await this.deps.renewalStash.put(kept);
    } catch (error) {
      console.warn(`cloud account: could not keep ${kept.host}'s renewal for a reload in its window:`, error);
    }
  }

  /** Forget what was kept of the attempt `nonceHash` (hex) names, if it still is that one. */
  private async forgetRenewal(host: string, nonceHash: string): Promise<void> {
    try {
      await this.deps.renewalStash.drop(host, nonceHash);
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
          await this.deps.api.deleteRotation(this.deps.token(), host, { nonceHash });
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
    this.deps.assertNotForked();
    const courier = this.deps.guardedRenewal(unguarded);
    const assurance = opts.assurance;
    const recheck = this.deps.renewalNotes.assuranceRecheck(host, assurance);
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
        await this.deps.api.putRotation(this.deps.token(), host, { nonceHash, assurance });
      } catch (error) {
        throw renewalRefusal(error);
      }
      // Kept before anything is asked of the host: should this page go away now, the next page of this browser withdraws
      // it at once (`resumeRenewal`) instead of the host waiting for it to lapse.
      const kept: KeptRenewal = { host, device: this.basics.deviceId, nonceHash: hex(nonceHash), assurance, askedMs: now() };
      await this.keepRenewal(kept);
      return this.settleRenewal(host, nonceHash, async (written) => {
        try {
          await this.deps.sync();
        } catch (error) {
          console.warn("cloud account: sync before renewing:", error);
        }
        // The head the request is about, vouched for before it goes: a page that just changed the account (its renewal
        // policy, which starts the renewals) would otherwise ask on a head it never vouched for.
        const vouched = await this.vouch(courier);
        const { view, answer } = await this.askRotate(host, courier, { nonce, eph: ephPublic, assurance }, sleep, now, vouched);
        recheck();
        const chain = this.basics.state!;
        const old = chain.machine(host)?.info;
        if (!old) throw new AccountError("notInChain", "{{host}} is not a device of the account that runs AI", undefined, undefined, { host });
        const rotation = await checkRotateAnswer(chain, host, old, answer, view, nonce, assurance);
        const body = hostRotationBody(this.basics.account, rotation, old.signKey, old.dhKey);
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

  async forgetLeftRenewals(rows: readonly HostRotationView[]): Promise<void> {
    const open = (r: HostRotationView | undefined, nonceHash: string) => !!r && (r.state === "attempt" || r.state === "staged") && r.nonceHash === nonceHash;
    for (const host of await this.deps.renewalStash.hosts().catch(() => [] as string[])) {
      // One that does not open now (a store that hands back nothing for a while, say) is left as it is.
      const kept = await this.deps.renewalStash.get(host).catch(() => null);
      if (!kept || open(rows.find((r) => r.host === host), kept.nonceHash)) continue;
      let now: HostRotationView | undefined;
      try {
        now = (await this.deps.api.rotations(this.deps.token())).hosts.find((r) => r.host === host);
      } catch {
        return;
      }
      if (!open(now, kept.nonceHash)) await this.forgetRenewal(host, kept.nonceHash);
    }
  }

  async keptRenewal(host: string, nonceHash: string): Promise<boolean> {
    const kept = await this.deps.renewalStash.get(host).catch(() => null);
    return kept !== null && kept.nonceHash === nonceHash;
  }

  async resumeRenewal(host: string, unguarded: RenewalCourier, opts: Partial<Omit<RenewOptions, "assurance">> = {}): Promise<RenewalReport | "busy" | null> {
    if (!(await this.deps.renewalStash.get(host).catch(() => null))) return null;
    // No Web Locks: the tab that asked may still be on it (typing the code, waiting for the window) — never taken from it.
    if (!this.deps.locks()) return "busy";
    const done = await this.holdRenewal(host, async (): Promise<RenewalReport | null> => {
      // Read again under the lock: the tab that held it may have finished it meanwhile.
      const kept = await this.deps.renewalStash.get(host).catch(() => null);
      if (!kept) return null;
      let row: HostRotationView | undefined;
      try {
        row = (await this.deps.api.rotations(this.deps.token())).hosts.find((r) => r.host === host);
      } catch (error) {
        throw renewalRefusal(error);
      }
      if (!row || (row.state !== "attempt" && row.state !== "staged") || row.nonceHash !== kept.nonceHash || kept.device !== this.basics.deviceId) {
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
          this.deps.assertNotForked();
          const courier = this.deps.guardedRenewal(unguarded);
          const recheck = this.deps.renewalNotes.assuranceRecheck(host, assurance);
          this.requireRenewable(host, assurance);
          const nonce = fromHex(checked.nonce);
          if (!equal(await sha256(nonce), nonceHash)) throw new RenewalError("badAnswer", `what this browser kept of ${host}'s renewal is not its attempt`, "nonce");
          const chain = this.basics.state!;
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
          issued = await this.deps.commitHostKey(
            host,
            mk,
            (chain) => {
              this.deps.checkRenewalOn(chain, host, rotation, assurance);
              return [{ type: "rotateHostKey", rotation }];
            },
            // …and once more right before publication, after the signing's awaits: the guard or a dispute
            // that arrived meanwhile stops it there.
            (build) => this.deps.appendBatch(build, { beforePublish: (base) => this.deps.checkRenewalOn(base, host, rotation, assurance) }),
          );
        } catch (error) {
          // Early by the hub's clock: its `ready_ms` says when (a few times; the window is short).
          if (error instanceof HubApiError && error.code === "rotation_not_ready" && error.reason === "window" && tries < 5) {
            readyMs = Math.max(error.readyMs ?? 0, now() + 1000);
            continue;
          }
          // Not staged any more: the hub voided it in the window — and says why (graded evidence, §3.3).
          if (error instanceof HubApiError && error.code === "rotation_not_ready" && (error.reason === "not_staged" || error.reason === "mismatch")) {
            const row = await this.deps.api.rotations(this.deps.token()).then((v) => v.hosts.find((x) => x.host === host), () => undefined);
            if (row?.failure) throw new RenewalError("voided", `the hub voided ${host}'s renewal: ${row.failure.reason}`, row.failure.reason);
          }
          throw renewalRefusal(error);
        }
      }
      await r.written();
      const { mkEpoch, ukEpoch, userKey } = issued;
      const state = this.basics.state!;
      const seq = state.deviceRecord(host)?.rotatedSeq ?? state.seq;
      // No longer waiting: the hosts' keys go to its new key from here on. Confirmed on the device, it is no longer isolated either (§3.8).
      const pending = new Set(this.deps.renewalNotes.renewalNote.pending);
      pending.delete(host);
      const isolated = new Set(this.deps.renewalNotes.renewalNote.isolated);
      if (assurance === ASSURANCE_ON_DEVICE) isolated.delete(host);
      this.deps.renewalNotes.noteRenewals({ ...this.deps.renewalNotes.renewalNote, pending, isolated });
      opts.onStep?.("handing");
      // Its keys to the devices it lets control it (purpose 5), then the key to it — sealed to the new DH key only — with a re-encryption.
      await this.deps.mailHostKey(host, mkEpoch, mk);
      let result: DeliveryResult;
      try {
        result = await this.deps.handOver(host, mkEpoch, ukEpoch, mk, courier, true);
      } catch (error) {
        result = { via: "refused", status: 0, message: error instanceof Error ? error.message : String(error) };
      }
      let hostKeys: number | null = null;
      try {
        hostKeys = await this.deps.handHostKeys(host, opts.hostKeysWait ?? HOST_KEYS_WAIT);
      } catch (error) {
        console.warn(`cloud account: could not seal ${host}'s device role the hosts' keys again (it asks):`, error);
      }
      try {
        await courier.attest();
      } catch (error) {
        console.warn("cloud account: attesting after a renewal:", error);
      }
      this.basics.emit();
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
      const chain = this.basics.state!;
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
        await this.deps.sync().catch(() => undefined);
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
      await this.deps.sync();
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
    const others = (this.basics.state?.admins() ?? []).map((d) => d.info.id).filter((id) => id !== this.basics.deviceId);
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
        row = (await this.deps.api.rotations(this.deps.token())).hosts.find((r) => r.host === host);
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

  async setRenewalPolicy(alwaysAssured: boolean): Promise<void> {
    const chain = this.deps.requireAdmin();
    this.deps.requireUpgraded(chain);
    // Chosen already, and the same: nothing to write. Never chosen: either choice is written (§5.6 「打开的顺序」).
    if (chain.renewalPolicySeq !== null && chain.alwaysAssured === alwaysAssured) return;
    await this.deps.append({ type: "setRenewalPolicy", alwaysAssured });
  }

  async writePause(build: (chain: ChainState) => Action[], beforePublish?: (base: ChainState) => void): Promise<boolean> {
    const chain0 = this.deps.requireAdmin();
    this.deps.requireUpgraded(chain0);
    try {
      await this.deps.appendBatch(
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

  pauseEntryLen(chain: ChainState, removals: PauseRemoval[]): number {
    const draft = chain.draft({ type: "device", id: this.basics.deviceId }, { type: "renewalPause", op: new Uint8Array(16), phase: PAUSE_START, removals }, Date.now());
    return encodeDraft(draft).length + 64;
  }

  pauseEnding(op: Uint8Array): PauseEnding | null {
    const chain = this.basics.state;
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
      const why = edgeHeld(chain, e, { exposed, isolated: (id) => this.deps.renewalNotes.isolated(id), flagged: (id) => this.deps.renewalNotes.flagged(id) });
      if (!why) return [];
      return [{ ...e, who: why.who, why: why.why === "exposed" && chain.machine(why.who) && !this.deps.renewalNotes.renews(why.who) ? ("cannotRenew" as const) : why.why }];
    });
  }

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
      const to = this.basics.state?.device(e.id);
      if (!to || to.admin || e.id === this.basics.deviceId || !this.basics.state?.machine(e.host)) continue;
      try {
        await this.deps.sendHostKeys(e.id, e.host);
      } catch (error) {
        if (!noDeviceRoleYet(to, error)) console.warn(`cloud account: could not send ${e.id} ${e.host}'s keys after a pause (it asks again):`, error);
      }
    }
    return returned;
  }

  async endPause(op: Uint8Array, expect?: { host: string; id: string }[]): Promise<void> {
    const returned = await this.endPauseWith(op, PAUSE_CANCELLED, () => ({ keep: [] }), { expect });
    if (!returned) throw new AccountError("notInChain", "that pause has ended already");
  }
}
