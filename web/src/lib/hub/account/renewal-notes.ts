/*
 * What this page knows of the hosts' identity key renewals beyond the chain
 * (docs/host-identity-rotation.md §3.8, §4): the renewal runner's note (which
 * hosts renew, wait at the hub, or are isolated or flagged), the disputes this
 * browser verified and keeps, what is exposed, and what an automatic renewal
 * may not go past. `CloudAccount` documents the methods it hands to these.
 */
import { type Accept, type Assurance, ASSURANCE_ON_DEVICE, type ChainState } from "@/lib/e2e";
import type { CrossTab } from "../account-storage";
import { acceptChange, capDisputes, disputeChecksOut, disputeId, disputeIsolates, disputesNotIn, type Exposure, exposedControllers, hostsToRotate, type KeptDispute, mergeDisputes, parseDisputes, type RenewalNote } from "../chain-policy";
import { checkpoints, type Held } from "../seen-store";
import type { AccountBasics, AccountNote } from "./basics";
import { AccountError } from "./errors";
import { RenewalError } from "./renewal";

/** What the renewal notes read of the account beyond its basics. */
export interface RenewalNotesDeps {
  /** What this browser holds of the account: its pin and seen record. */
  readHeld(): Promise<Held>;
  /** Catch up with the hub's chain. */
  sync(): Promise<number>;
  /** The disputes this browser keeps for the account (`keptDisputesKey`), and no other note. */
  keptDisputes: AccountNote;
  /** This device's own guard (`localAssuredKey`), and no other note. */
  localAssured: AccountNote;
}

export class RenewalNotes {
  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: RenewalNotesDeps,
    /** How the tabs of this browser tell each other of disputes one of them verified (`null`: no other tab). */
    private readonly crossTab: CrossTab | null,
  ) {}

  /** The note as it stands ({@link noteRenewals}), for a renewal that writes the next one on it. */
  get renewalNote(): RenewalNote {
    return this.renewal;
  }

  noteRenewals(note: RenewalNote): void {
    const same = (a: ReadonlySet<string>, b: ReadonlySet<string>) => a.size === b.size && [...a].every((x) => b.has(x));
    const was = this.renewal;
    const flagged = note.flagged ?? new Set<string>();
    if (same(was.renews, note.renews) && same(was.pending, note.pending) && same(was.isolated, note.isolated) && same(was.flagged ?? new Set(), flagged)) return;
    if ([...note.isolated].some((h) => !was.isolated.has(h)) || [...flagged].some((h) => !(was.flagged ?? new Set()).has(h))) this.assuranceGen++;
    this.renewal = { renews: new Set(note.renews), pending: new Set(note.pending), isolated: new Set(note.isolated), flagged: new Set(flagged) };
    // What can renew changes what is exposed (`hostsToRotate`).
    this.exposureCache = null;
    this.basics.emit();
  }

  hostsToRotate(): Map<string, Exposure> {
    const chain = this.basics.state;
    if (!chain) return new Map();
    const cached = this.exposureCache;
    if (cached && cached.chain === chain && cached.seq === chain.seq) return cached.exposed;
    // Which hosts can renew decides what a renewal fixes (§4.1): an exposed host that cannot is a pass no renewal takes away.
    const exposed = hostsToRotate(chain, (h) => this.renews(h));
    this.exposureCache = { chain, seq: chain.seq, exposed };
    return exposed;
  }

  renews(id: string): boolean {
    return this.basics.state?.isV3 === true && this.renewal.renews.has(id);
  }

  isolated(id: string): boolean {
    return this.renewal.isolated.has(id) || this.isolatedByKept(id);
  }

  keepDisputes(found: readonly KeptDispute[]): void {
    // Every one kept, those still being checked too: a write never drops another tab's.
    const kept = this.keptAll();
    const fresh = disputesNotIn(kept, found);
    // Verified just now by the caller (`checkStatement`), on this page's chain —
    // one another tab told of, and still being checked here, as well.
    const confirmed = found.filter((d) => this.keptChecks.get(disputeId(d)) !== "ok");
    for (const d of found) this.keptChecks.set(disputeId(d), "ok");
    if (!fresh.length) {
      if (confirmed.length) {
        this.assuranceGen++;
        this.exposureCache = null;
        this.basics.emit();
      }
      return;
    }
    this.assuranceGen++;
    // Memory holds what storage is told: if the write fails, nothing is lost here.
    this.keptMemory = capDisputes(this.basics.state, mergeDisputes(kept, fresh));
    this.deps.keptDisputes.write(JSON.stringify(this.keptMemory));
    this.crossTab?.post({ account: this.basics.account, disputes: fresh });
    this.exposureCache = null;
    this.basics.emit();
  }

  keptDisputes(): KeptDispute[] {
    const all = this.keptAll();
    for (const d of all) this.checkKept(d);
    return all.filter((d) => {
      const check = this.keptChecks.get(disputeId(d));
      return check === "ok" || check === "ahead";
    });
  }

  flagged(id: string): boolean {
    return this.renewal.flagged?.has(id) === true;
  }

  keysWithheldFrom(id: string): boolean {
    const record = this.basics.state?.device(id);
    if (!record?.host) return false;
    if (this.isolated(id) || this.flagged(id) || this.renewal.pending.has(id)) return true;
    return this.renews(id) && this.hostsToRotate().has(id);
  }

  get alwaysAssured(): boolean {
    return this.basics.state?.alwaysAssured === true;
  }

  localAssured(): boolean {
    return this.deps.localAssured.read() === "1";
  }

  setLocalAssured(on: boolean): void {
    if (on) this.deps.localAssured.write("1");
    else this.deps.localAssured.forget();
    if (on) this.assuranceGen++;
    this.basics.emit();
  }

  get renewalsAssured(): boolean {
    return this.alwaysAssured || this.localAssured();
  }

  mustRenewFirst(id: string): boolean {
    return this.basics.state ? this.mustRenewFirstOn(this.basics.state, id) : false;
  }

  mustRenewFirstOn(chain: ChainState, id: string): boolean {
    if (!chain.device(id)?.host) return false;
    if (this.flagged(id) || this.renewal.isolated.has(id) || this.keptDisputes().some((d) => d.host === id && disputeIsolates(chain, d))) return true;
    const exposed = chain === this.basics.state ? this.hostsToRotate() : hostsToRotate(chain, (h) => this.renews(h));
    return this.renews(id) && exposed.has(id);
  }

  renewFirst(host: string): string[] {
    const chain = this.basics.state;
    if (!chain?.machine(host)) return [];
    const exposed = this.hostsToRotate();
    return exposed.has(host) ? exposedControllers(chain, exposed, host) : [];
  }

  /** What the renewal runner last said of the hosts (`noteRenewals`; docs/host-identity-rotation.md §4.2–§4.3, §3.8). */
  private renewal: RenewalNote = { renews: new Set(), pending: new Set(), isolated: new Set(), flagged: new Set() };

  /** {@link keptDisputes} as last read, by its stored text. */
  private keptRaw: string | null | undefined = undefined;

  private keptParsed: KeptDispute[] = [];

  /**
   * Every dispute this page kept or heard of from another tab, in memory too:
   * a storage write that fails — full, or not allowed — never
   * loses one, nor does another tab writing over it.
   */
  private keptMemory: KeptDispute[] = [];

  /**
   * What re-verifying a kept dispute on this page's chain found (R6 review), by
   * {@link disputeId}: `ok`; `bad` (its signature does not check out:
   * dropped, it isolates nothing); `checking`; or, for one about an entry
   * past this page's chain, which it cannot judge yet, `ahead` — within what
   * this browser has verified (its seen record), or the record would not
   * read: another page of it may have checked it — or `beyond`, past that:
   * none can have. Only `ok` and `ahead` count — `ahead` meanwhile (a page
   * behind keeps the host isolated: isolation fails safe). `beyond` is kept,
   * not counted, and placed again when the record may have moved; either is
   * checked on the view that brings its entry before that view is taken
   * ({@link install}).
   */
  private keptChecks = new Map<string, "ok" | "bad" | "checking" | "ahead" | "beyond">();

  /**
   * Bumped whenever what an automatic renewal may not go past changes:
   * this device's guard turned on, a dispute kept or heard of, the
   * hub's isolated hosts. An attempt in flight that sees it move checks again
   * at once — and every attempt checks once more right before publication.
   */
  private assuranceGen = 0;

  /** {@link hostsToRotate} for the chain as it was last asked, by its head (and the narrowed groups it read). */
  private exposureCache: { chain: ChainState; seq: number; exposed: Map<string, Exposure> } | null = null;

  /**
   * Another tab's disputes (or its storage write): kept in memory here too —
   * so a storage write that failed there loses none — but counted only once
   * this page has verified them on its own chain ({@link keptDisputes});
   * then everyone here is told at once ({@link checkKept}). One about a
   * renewal this page's chain does not hold yet counts meanwhile if this
   * browser has verified that far, and this page catches up to check it;
   * one placed past that before is placed again (the record may have moved).
   */
  heardDisputes(message: unknown): void {
    const m = (message && typeof message === "object" ? message : {}) as { account?: unknown; disputes?: unknown; storage?: unknown };
    if (m.account === this.basics.account) {
      const heard = parseDisputes(m.disputes);
      const fresh = disputesNotIn(this.keptAll(), heard);
      if (fresh.length) this.keptMemory = capDisputes(this.basics.state, mergeDisputes(this.keptAll(), fresh));
    } else if (!("storage" in m)) return;
    // What an automatic renewal may not go past may have moved (here, or once checked).
    this.assuranceGen++;
    this.exposureCache = null;
    for (const [id, check] of this.keptChecks) if (check === "beyond") this.keptChecks.delete(id);
    // The checks start now, not when someone next asks.
    this.keptDisputes();
    this.basics.emit();
  }

  /**
   * Place every kept dispute about an entry past `chain` that is not judged
   * yet: `ahead` (counted meanwhile) within `through`, what this browser has
   * verified — `beyond` (kept, not counted) past it. At once, no await.
   */
  placeDisputesPast(chain: ChainState, through: number): void {
    for (const d of this.keptAll()) {
      const id = disputeId(d);
      const check = this.keptChecks.get(id);
      if (d.seq <= chain.seq || check === "ok" || check === "bad") continue;
      const ahead = d.seq <= through;
      if (ahead && check !== "ahead") this.assuranceGen++;
      this.keptChecks.set(id, ahead ? "ahead" : "beyond");
      this.exposureCache = null;
    }
  }

  /** The kept disputes `chain` can judge that are not judged yet. */
  disputesDueOn(chain: ChainState): KeptDispute[] {
    return this.keptAll().filter((d) => {
      const check = this.keptChecks.get(disputeId(d));
      return d.seq <= chain.seq && check !== "ok" && check !== "bad";
    });
  }

  /** Check, on `chain`, every kept dispute it can judge that was not judged yet (a view about to be installed). */
  async checkDisputesOn(chain: ChainState): Promise<void> {
    for (const d of this.disputesDueOn(chain)) {
      const id = disputeId(d);
      const ok = await disputeChecksOut(chain, d);
      const now = this.keptChecks.get(id);
      if (now === "ok" || now === "bad") continue;
      this.settleKept(d, ok);
      this.exposureCache = null;
    }
  }

  /** {@link renewFirst}, as a refusal (`RenewalError` `renewFirst`). */
  requireRenewFirst(host: string): void {
    const first = this.renewFirst(host);
    if (first.length) {
      throw new RenewalError("renewFirst", `renew the identity key of ${first.join(", ")} first: ${host} lets ${first.length > 1 ? "them" : "it"} control it`, undefined, first);
    }
  }

  /** Refuse an accept list for `host` that lets in a host that must renew first, on `chain` (§4.1, §7 decision 8). */
  refuseExposedAdmission(chain: ChainState, host: string, accept: Accept): void {
    const { admitted } = acceptChange(chain, host, accept);
    const exposed = admitted.filter((id) => this.mustRenewFirstOn(chain, id));
    if (exposed.length) {
      throw new AccountError("exposed", "renew the identity key of {{ids}} first: a device that could control it no longer can, and may have copied it", undefined, undefined, {
        ids: exposed.join(", "),
      });
    }
  }

  /** What only a renewal confirmed on the device may go past (§3.8, §4.4): isolation, the hub's flag, the account's policy, this device's guard. */
  checkAssurance(chain: ChainState, host: string, assurance: Assurance): void {
    if (assurance === ASSURANCE_ON_DEVICE) return;
    if (this.renewal.isolated.has(host) || this.keptDisputes().some((d) => d.host === host && disputeIsolates(chain, d))) {
      throw new RenewalError("needsAssurance", `${host} is isolated: renew its identity key with the code its screen shows`);
    }
    if (this.flagged(host)) throw new RenewalError("needsAssurance", `the hub holds ${host} isolated: renew its identity key with the code its screen shows`, "flagged");
    if (chain.alwaysAssured || this.localAssured()) {
      throw new RenewalError("needsAssurance", "renewals are confirmed on the device here", this.localAssured() ? "local" : "policy");
    }
  }

  /** What an automatic renewal of `host` may not go past, as it stands now: when it moves, the attempt checks again at once. */
  assuranceRecheck(host: string, assurance: Assurance): () => void {
    let gen = this.assuranceGen;
    return () => {
      if (gen === this.assuranceGen || !this.basics.state) return;
      gen = this.assuranceGen;
      if (this.basics.state.machine(host)) this.checkAssurance(this.basics.state, host, assurance);
    };
  }

  /**
   * Every dispute kept for the account here that no check found bad — what
   * storage holds, and what this page kept or heard of in memory, one each by
   * host, renewal and signature — verified or not yet.
   */
  private keptAll(): KeptDispute[] {
    const raw = this.deps.keptDisputes.read();
    if (raw !== this.keptRaw) {
      let parsed: KeptDispute[] = [];
      try {
        parsed = parseDisputes(JSON.parse(raw ?? "[]") as unknown);
      } catch {
        parsed = [];
      }
      this.keptRaw = raw;
      this.keptParsed = parsed;
    }
    const all = this.keptMemory.length ? mergeDisputes(this.keptParsed, this.keptMemory) : this.keptParsed;
    return all.filter((d) => this.keptChecks.get(disputeId(d)) !== "bad");
  }

  /**
   * Verify kept dispute `d` on this page's chain, once (R6 review): the statement
   * must be signed by the key the renewal at `d.seq` retired. One that checks
   * out counts from then on, and listeners (each channel among them) are
   * told at once; one that does not is dropped. A chain that has no such
   * entry yet cannot judge it: if this browser has verified a chain that long
   * (its seen record), or the record will not read, another page of it may
   * have checked it, so it counts meanwhile (`ahead`); if not, no page of it
   * can have, and it does not count (`beyond`) — but it is kept: the record
   * may lag. Either way this page catches up, and it is checked on the view
   * that brings the entry ({@link checkDisputesOn}).
   */
  private checkKept(d: KeptDispute): void {
    const id = disputeId(d);
    const chain = this.basics.state;
    if (this.keptChecks.has(id) || !chain) return;
    this.keptChecks.set(id, "checking");
    void (async () => {
      if (chain.seq >= d.seq) {
        const ok = await disputeChecksOut(chain, d);
        if (this.keptChecks.get(id) !== "checking") return;
        this.settleKept(d, ok);
        if (ok) this.assuranceGen++;
      } else {
        let through: number | null = null;
        try {
          through = await this.verifiedThrough();
        } catch (error) {
          // Unread: it fails safe — counted, as one within the record.
          console.warn("cloud account: reading the seen record for a dispute:", error);
        }
        if (this.keptChecks.get(id) !== "checking") return;
        const ahead = through === null || d.seq <= through;
        this.keptChecks.set(id, ahead ? "ahead" : "beyond");
        if (ahead) this.assuranceGen++;
        void this.deps.sync().catch((error: unknown) => console.warn("cloud account: catching up with a dispute another tab kept:", error));
      }
      this.exposureCache = null;
      this.basics.emit();
    })();
  }

  /** The furthest entry this browser has verified of the account's chain (its seen record, any checkpoint of it); -1 when it pinned none. */
  private async verifiedThrough(): Promise<number> {
    const held = await this.deps.readHeld();
    return held.genesis === null ? -1 : Math.max(-1, ...checkpoints(held).map((mark) => mark.seenThrough));
  }

  /** What checking kept dispute `d`'s signature found: kept as `ok`, or as `bad` and dropped from memory. */
  private settleKept(d: KeptDispute, ok: boolean): void {
    const id = disputeId(d);
    this.keptChecks.set(id, ok ? "ok" : "bad");
    if (!ok) {
      console.warn(`cloud account: a kept dispute of ${d.host}'s renewal at ${d.seq} does not check out: dropped`);
      this.keptMemory = this.keptMemory.filter((k) => disputeId(k) !== id);
    }
  }

  /** Whether a kept dispute isolates host `id` on this page's chain. */
  private isolatedByKept(id: string): boolean {
    const chain = this.basics.state;
    if (!chain) return false;
    return this.keptDisputes().some((d) => d.host === id && disputeIsolates(chain, d));
  }
}
