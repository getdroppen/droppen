/*
 * Bringing the account's chain up to date from the hub or a handshake, holding
 * every view against what this browser verified, signing and appending batches
 * on it, and vouching for its head (`CloudAccount` documents the methods it
 * hands to these).
 */
import {
  type Action,
  type Attestation,
  CHAIN_VERSION,
  CHAIN_VERSION_3,
  ChainClock,
  ChainState,
  copy,
  createAttestation,
  decodeEntry,
  type DeviceKeys,
  type Draft,
  encodeEntry,
  type Entry,
  entryHash,
  equal,
  fixedVersion,
  HandshakeError,
  hex,
  signDraft,
} from "@/lib/e2e";
import { type HubApi, HubApiError } from "../api";
import { verifyBatch } from "../chain-policy";
import { checkpoints, SeenStoreError } from "../seen-store";
import type { AccountBasics } from "./basics";
import type { ChainTrust } from "./chain-trust";
import { AccountError, heldWho, MovingCheckpoint } from "./errors";
import type { ActivationHold } from "./held-back";
import { CHAIN_V3_WAIT, FIRST_LOAD_TRIES, MAX_APPEND_ENTRIES } from "./limits";
import type { RenewalNotes } from "./renewal-notes";
import type { HeldBack } from "./views";

/** What chain sync and batch appends read of the account beyond its basics. */
export interface ChainSyncDeps {
  api: HubApi;
  token(): string;
  keys: DeviceKeys;
  trust: ChainTrust;
  hold: ActivationHold;
  renewalNotes: Pick<RenewalNotes, "checkDisputesOn" | "disputesDueOn" | "placeDisputesPast">;
  setChain(state: ChainState, clock: ChainClock): void;
  signChained(chain: ChainState, actions: Action[], asV3?: boolean): Promise<Entry[]>;
}

export class ChainSync {
  /**
   * The newest entry of a view this page verified and could not take, as its
   * record could not be written (`install`: the store failed): until this
   * view reaches it, the page vouches for nothing (`attestation`).
   */
  private unrecordedThrough = -1;

  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: ChainSyncDeps,
  ) {}

  /**
   * Make `next` this view (with `clock`), if it extends it — and first hold
   * it against what this browser verified, in the same step that writes it,
   * so a view that conflicts with it is never installed: the fork is latched
   * and thrown, and this view stays as it was. Nor is a view whose record
   * could not be written (the store failed: a {@link SeenStoreError}, to be
   * tried again). `false`: `next` held nothing this view lacks (another call
   * took it meanwhile).
   */
  async install(next: ChainState, clock: ChainClock): Promise<boolean> {
    return this.deps.trust.withSeenLock(async () => {
      this.deps.trust.assertNotForked();
      const current = this.basics.state;
      if (current) {
        if (next.seq <= current.seq) {
          if (!equal(current.hashAt(next.seq)!, next.head)) {
            throw this.deps.trust.latchFork(`two views of the account's chain in this page differ at entry ${next.seq}`);
          }
          // Nothing new — but held against the record all the same: another
          // page may have verified, since this view was taken, a chain this
          // one contradicts, and a handshake on it must not go on.
          const held = await this.deps.trust.readHeld();
          if (held.genesis === hex(current.hashAt(0)!)) this.deps.trust.heldAgainst(current, held);
          return false;
        }
        if (!equal(next.hashAt(current.seq)!, current.head)) {
          throw this.deps.trust.latchFork(`two views of the account's chain in this page differ at entry ${current.seq}`);
        }
        // The sightings this view took meanwhile, kept (the earliest of each).
        for (const [seq, ms] of this.basics.clock.firstSeen) {
          const at = clock.firstSeen.get(seq);
          if (at === undefined || ms < at) clock.firstSeen.set(seq, ms);
        }
      }
      // This device's id, listed with keys that are not this browser's: the
      // hub gave this browser someone else's id (a device waiting after a
      // passkey join, say). Never "pending", never this device: an alarm.
      const listed = next.deviceRecord(this.basics.deviceId);
      if (listed && (!equal(listed.info.signKey, this.deps.keys.signPublic) || !equal(listed.info.dhKey, this.deps.keys.dhPublic))) {
        throw this.deps.trust.latchFork(`the account's chain lists this device's id (${this.basics.deviceId}) with keys that are not this browser's`);
      }
      // Kept only under the pin it is of (a chain this browser never pinned is not recorded).
      const genesis = hex(next.hashAt(0)!);
      // How far this browser has verified, with this view: what a dispute past the view is placed by, below.
      let recorded = -1;
      try {
        await this.deps.trust.updateHeld((held) => {
          if (held.genesis !== genesis) return undefined;
          const record = this.deps.trust.nextRecord(next, held, clock);
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
        await this.deps.renewalNotes.checkDisputesOn(next);
      } while (this.deps.renewalNotes.disputesDueOn(next).length);
      // And those past it placed by the record just written, in the same step:
      // a reload holds a host isolated before any channel opens.
      this.deps.renewalNotes.placeDisputesPast(next, recorded);
      // A fork latched meanwhile (another call of this page) installs nothing either.
      this.deps.trust.assertNotForked();
      this.deps.setChain(next, clock);
      return true;
    });
  }

  /** A copy of this view's clock, for a merge that is installed only once it is held against the seen record. */
  private clockCopy(): ChainClock {
    return new ChainClock(this.basics.clock.nowMs, this.basics.clock.liveFromSeq, new Map(this.basics.clock.firstSeen));
  }

  async completeHandshake<T>(finish: (merged: { chain: ChainState; clock: ChainClock }) => Promise<T>): Promise<T> {
    this.deps.trust.assertNotForked();
    const state = this.basics.state;
    if (!state) throw new AccountError("notInChain", "the account has no chain yet");
    this.basics.clock.nowMs = Date.now();
    const merged = { chain: state.clone(), clock: this.clockCopy() };
    let result: T;
    try {
      result = await finish(merged);
    } catch (error) {
      if (error instanceof HandshakeError && error.kind === "fork") throw this.deps.trust.latchFork(error.message);
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
    if (await this.install(merged.chain, merged.clock)) this.basics.emit();
  }

  sync(): Promise<number> {
    return this.catchUp(this.deps.token());
  }

  /** `sync` with `token` (a login token, read-only, for `confirmRemoval`). */
  async catchUp(token: string, opts: { noRenew?: boolean } = {}): Promise<number> {
    this.deps.trust.assertNotForked();
    const now = Date.now();
    this.basics.clock.nowMs = now;
    const state = this.basics.state;
    if (!state) {
      const served = async () => (await this.deps.api.chain(token, 0, opts)).entries.map((e) => decodeEntry(e));
      let decoded = await served();
      if (decoded.length === 0) {
        if ((await this.deps.trust.pinnedGenesis()) !== null) throw this.deps.trust.latchFork("the hub serves no chain for an account whose genesis this browser pinned");
        return 0;
      }
      let verified: Awaited<ReturnType<ChainSync["verifyServed"]>>;
      // Other pages may record more while this one loads, more than once: a
      // served chain that is only behind the record (and agrees with it) is
      // looked at again a few times before it is called a fork; one that
      // contradicts it is a fork at once (`verifyServed` latches it).
      for (let tries = 1; ; tries++) {
        try {
          verified = await this.verifyServed(decoded, now);
          break;
        } catch (error) {
          if (!(error instanceof MovingCheckpoint) || this.deps.trust.forked !== null) throw error;
          if (tries >= FIRST_LOAD_TRIES) throw this.deps.trust.latchFork(error.message);
          decoded = await served();
        }
      }
      await this.install(verified.state, verified.clock);
      this.deps.hold.setHeld(verified.held);
      this.basics.emit();
      return verified.state.seq + 1;
    }
    const { entries } = await this.deps.api.chain(token, state.seq + 1, opts);
    const decoded = entries.map((e) => decodeEntry(e));
    const next = state.clone();
    const clock = this.clockCopy();
    const { applied, error } = await next.mergePrefix(decoded, clock);
    // What took effect is kept and told before anything else is decided — once held against the seen record.
    if (applied && (await this.install(next, clock))) this.basics.emit();
    this.deps.hold.setHeld(this.deps.hold.heldFrom(error, decoded));
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
    const trust = await this.deps.trust.readHeld();
    const pinned = trust.genesis;
    const seen = pinned ? trust.record : null;
    const clock = seen ? ChainClock.resume(now, seen.seenThrough, seen.firstSeen) : ChainClock.live(now, decoded.length);
    const { state, error } = await ChainState.verifyPrefix(decoded, undefined, clock);
    if (pinned && pinned !== hex(state.hashAt(0)!)) throw this.deps.trust.latchFork("the hub served a different genesis than this device pinned");
    const held = this.deps.hold.heldFrom(error, decoded);
    // Against every checkpoint this browser holds (an older page's kept apart too).
    let behind = false;
    for (const mark of pinned ? checkpoints(trust) : []) {
      if (state.seq < mark.seenThrough) {
        // Shorter than the record: a view another page of this browser went
        // past a moment ago — a checkpoint still moving, to be looked at again
        // — unless it contradicts the record where they overlap.
        const theirs = mark.heads?.find(([seq]) => seq === state.seq)?.[1];
        if (theirs !== undefined && theirs !== hex(state.head)) {
          throw this.deps.trust.latchFork(`the hub served a chain that forks at entry ${state.seq} from the one this device saw`);
        }
        behind = true;
      } else if (hex(state.hashAt(mark.seenThrough)!) !== mark.head) {
        throw this.deps.trust.latchFork("the hub served a chain without the head this device saw");
      }
    }
    if (behind) throw new MovingCheckpoint("the hub served less of the chain than this device has seen");
    return { state, clock, held };
  }

  async absorb(encoded: Uint8Array[]): Promise<number> {
    this.deps.trust.assertNotForked();
    // Pushes follow the hub's head, past what this view holds back.
    const state = this.basics.state;
    if (!state || this.deps.hold.heldBack) return this.sync();
    let entries: Entry[];
    try {
      entries = encoded.map((e) => decodeEntry(e));
    } catch {
      return 0;
    }
    this.basics.clock.nowMs = Date.now();
    const next = state.clone();
    const clock = this.clockCopy();
    const { applied, error } = await next.mergePrefix(entries, clock);
    if (applied && (await this.install(next, clock))) this.basics.emit();
    if (error?.kind === "outOfOrder") return applied + (await this.sync());
    this.deps.hold.setHeld(this.deps.hold.heldFrom(error, entries));
    return applied;
  }

  /** Entries the hub took, into this view (merged: the hub's push of them may have landed first). */
  async landed(entries: Entry[], now = Date.now()): Promise<void> {
    this.basics.clock.nowMs = now;
    const next = this.basics.state!.clone();
    const clock = this.clockCopy();
    const { error } = await next.mergePrefix(entries, clock);
    await this.install(next, clock);
    this.basics.emit();
    if (error) throw error;
  }

  /**
   * Take `chain` — the hub's, past a held-back self-activation — with this
   * device's own `mine` on it: a veto (or the user's "approve") settled what
   * the day's delay was there for. Everything new is first seen now, and the
   * clock goes on live from its head.
   */
  async adopt(chain: ChainState, mine: Entry[], now = Date.now()): Promise<void> {
    if (mine.length) await chain.merge(mine, ChainClock.history());
    const clock = ChainClock.resume(now, chain.seq, this.basics.clock.firstSeen.entries());
    for (let seq = this.basics.state!.seq + 1; seq <= chain.seq; seq++) clock.observe(seq);
    await this.install(chain, clock);
    this.deps.hold.settled();
    this.basics.emit();
  }

  /**
   * `actions` as this device's entries, each on the one before (from `chain`'s
   * head). `asV3`: the shared actions as v3 from the first one on (the
   * account's upgrade: its first v3 entry).
   */
  async signChained(chain: ChainState, actions: Action[], asV3 = false): Promise<Entry[]> {
    let seq = chain.seq;
    let prev = copy(chain.head);
    // As `ChainState.draft` would on the chain with the entries before applied:
    // a shared action is v3 once the chain (or this batch) is.
    let v3 = chain.isV3 || asV3;
    const out: Entry[] = [];
    for (const action of actions) {
      const version = fixedVersion(action) ?? (v3 ? CHAIN_VERSION_3 : CHAIN_VERSION);
      if (version === CHAIN_VERSION_3) v3 = true;
      const draft: Draft = { version, account: this.basics.account, seq: ++seq, prev, createdMs: Date.now(), signer: { type: "device", id: this.basics.deviceId }, action };
      this.deps.trust.assertNotForked();
      const entry = await signDraft(draft, this.deps.keys.sign.privateKey);
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
  async appendBatch(
    build: (chain: ChainState) => Promise<Action[]>,
    opts: {
      overHeld?: boolean;
      asV3?: boolean;
      /** Checked right before the signed batch is published (after the signing's awaits), on the chain it was built on: a throw publishes nothing. */
      beforePublish?: (base: ChainState) => void;
    } = {},
  ): Promise<Entry[]> {
    this.deps.trust.assertNotForked();
    for (let attempt = 0; ; attempt++) {
      const state = this.basics.state;
      if (!state) throw new AccountError("notInChain", "the account has no chain yet");
      const held = this.deps.hold.heldBack;
      if (held && !opts.overHeld) {
        throw new AccountError(
          "heldBack",
          `${heldWho(held)}: approve or remove it first`,
          held.readyMs,
        );
      }
      const base = held ? await this.deps.hold.provisional(held) : state;
      const actions = await build(base);
      // The hub takes 1–64 entries an append: a bigger batch is said here, before anything is signed.
      if (actions.length > MAX_APPEND_ENTRIES) {
        throw new AccountError("tooManyEntries", "that would write {{n}} entries to the account's chain at once, and the hub takes at most {{max}}: do it in smaller steps", undefined, undefined, {
          n: String(actions.length),
          max: String(MAX_APPEND_ENTRIES),
        });
      }
      const entries = await this.deps.signChained(base, actions, opts.asV3);
      // Refused here, by the chain's own rule, before anything leaves this device (review 13).
      await verifyBatch(base, entries);
      try {
        this.deps.trust.assertNotForked();
        opts.beforePublish?.(base);
        await this.deps.api.appendChain(
          this.deps.token(),
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
  async appendWaitingFor(host: string, build: (chain: ChainState) => Promise<Action[]>, wait = CHAIN_V3_WAIT): Promise<Entry[]> {
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

  async attestation(): Promise<Attestation> {
    this.deps.trust.assertNotForked();
    if (!this.basics.state) throw new AccountError("notInChain", "the account has no chain yet");
    // A page behind what this browser verified vouches for nothing: its word,
    // freshly dated, for a head before (say) a removal is what a hub keeping
    // the newer entries from it would need. It catches up first; a hub that
    // will not let it is a fork. Whether the account is pinned is read from
    // the store here, never taken from `pinned`: that is what this page saw
    // last, and another page may have pinned the account since. Under
    // another pin, or contradicting what this browser verified where they
    // overlap, this view is a fork. Nor does a page vouch for a head it has
    // since verified past, only not taken (its record would not write).
    const held = await this.deps.trust.readHeld();
    const genesis = hex(this.basics.state.hashAt(0)!);
    if (held.genesis !== null && held.genesis !== genesis) {
      throw this.deps.trust.latchFork("this browser pinned another genesis for the account than this page's chain starts with");
    }
    if (held.genesis !== null) this.deps.trust.heldAgainst(this.basics.state, held);
    const recorded = held.genesis !== null ? Math.max(-1, ...checkpoints(held).map((mark) => mark.seenThrough)) : -1;
    if (recorded > this.basics.state.seq || this.unrecordedThrough > this.basics.state.seq) {
      await this.sync();
      const after = this.basics.state!;
      if (after.seq < recorded) {
        if (this.deps.hold.heldBack) {
          throw new AccountError("heldBack", "this page holds back entries another page of this browser took: it vouches for nothing until it takes them");
        }
        throw this.deps.trust.latchFork(`the hub serves this page the account's chain through entry ${after.seq}, and another page of this browser verified it through ${recorded}`);
      }
      if (after.seq < this.unrecordedThrough) {
        throw new SeenStoreError(`this page verified the account's chain through entry ${this.unrecordedThrough} and could not record it: it vouches for nothing until it takes that — try again`);
      }
    }
    return createAttestation(this.basics.state!, this.basics.deviceId, this.deps.keys.sign.privateKey, Date.now());
  }
}
