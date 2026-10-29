/*
 * What this browser trusts of the account's chain, shared by its pages: the
 * genesis it pinned, the seen record (what it verified, and when it first saw
 * each entry), and this page's fork latch. Every view is held against them
 * before it is taken; the pin is set once and the record only moves forward.
 * `CloudAccount` documents the methods it hands to these.
 */
import { type ChainClock, type ChainState, hex } from "@/lib/e2e";
import { type AccountLocks, defaultSeenWatch, type SeenWatch } from "../account-storage";
import {
  type AckRecord,
  compareSeen,
  describeError,
  type Held,
  higherSeen,
  parseSeenJson,
  parseStored,
  sameStored,
  seenKey,
  type SeenRecord,
  type SeenStore,
  SeenStoreError,
  seenNext,
  type StoredTrust,
} from "../seen-store";
import type { AccountBasics, AccountNote } from "./basics";
import type { Standing } from "./devices";
import { AccountError } from "./errors";

/** What the trust reads of the account beyond its basics. */
export interface ChainTrustDeps {
  /** The genesis older pages read as pinned (`genesisKey`, `localStorage`), and no other note. */
  genesisNote: AccountNote;
  /** The seen record older pages read (`seenKey`, `localStorage`), and no other note. */
  seenNote: AccountNote;
  /** What this device acknowledged of the alerts: pinning the account writes where they start. */
  readAck(): AckRecord | null;
  writeAck(ack: AckRecord): void;
  /** Where this device stands in the chain. */
  standing(): Standing;
}

export class ChainTrust {
  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: ChainTrustDeps,
    seen: SeenStore,
    /** What serialises the pin and the record across this browser's pages (`navigator.locks`; `null`: none). */
    private readonly locks: AccountLocks | null,
  ) {
    this.seen = seen;
  }

  /** Watch the seen record while the page keeps its session, unless it watches already (`CloudAccount.keepSessionFresh`). */
  keepWatching(): void {
    if (!this.unwatchSeen) this.watchSeenRecord();
  }

  /** Stop watching the seen record (`CloudAccount.stopSession`: a sign-out, a wipe). */
  stopWatching(): void {
    this.unwatchSeen?.();
  }

  get pinned(): boolean {
    return this.pinnedAt !== null && !!this.basics.state && this.pinnedAt === hex(this.basics.state.hashAt(0)!);
  }

  get forked(): string | null {
    return this.forkedWhy;
  }

  assertNotForked(): void {
    if (this.forkedWhy !== null) throw new AccountError("fork", this.forkedWhy);
  }

  watchSeenRecord(on: SeenWatch = defaultSeenWatch()): () => void {
    this.unwatchSeen?.();
    const look = () => void this.recheckSeen().catch((error: unknown) => console.warn("cloud account: reading the seen record again:", error));
    const unhear = this.seen.watch((account) => {
      if (account === null || account === this.basics.account) look();
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

  /** Set (for good, in this page) once this view conflicted with the chain this browser verified: why. */
  private forkedWhy: string | null = null;

  private unwatchSeen: (() => void) | null = null;

  /** This page's turn at the seen record: one read-compare-write of it at a time here, whether or not there are Web Locks. */
  private seenTurn: Promise<unknown> = Promise.resolve();

  /** The genesis this browser pinned (hex), as the seen store said it when this page last read or wrote it (`pinned`). */
  private pinnedAt: string | null = null;

  private readonly seen: SeenStore;

  /** Latch this page's fork alarm (see {@link forked}), and the error to throw for it. */
  latchFork(message: string): AccountError {
    if (this.forkedWhy === null) {
      this.forkedWhy = message;
      console.error(`cloud account: ALARM: ${message}`);
      this.basics.emit();
    }
    return new AccountError("fork", message);
  }

  /** What this browser holds for the account now ({@link trustFrom}). */
  async readHeld(): Promise<Held> {
    const { held } = this.trustFrom(await this.atStore(() => this.seen.get(this.basics.account)));
    this.pinnedAt = held.genesis;
    return held;
  }

  /** The genesis this browser pinned for the account (hex), as the seen store holds it now. */
  async pinnedGenesis(): Promise<string | null> {
    return (await this.readHeld()).genesis;
  }

  /** Trust this chain's genesis from now on (only for a reason: the **Pinning** note in `account.ts`'s header). */
  async pin(): Promise<void> {
    if (!this.basics.state) return;
    const state = this.basics.state;
    const genesis = hex(state.hashAt(0)!);
    // The pin and the record in one step: another page may have pinned
    // another genesis since this one last looked (two first set-ups at once,
    // a hub taking both). Then that pin stands, with its record.
    await this.withSeenLock(async () => {
      await this.updateHeld((held) => {
        if (held.genesis !== null && held.genesis !== genesis) throw this.latchFork("another page of this browser pinned a different genesis for this account");
        return { genesis, record: this.nextRecord(state, held, this.basics.clock) };
      });
      // Alerts start from here: what happened before this device trusted the account is history.
      if (!this.deps.readAck()) this.deps.writeAck({ through: state.seq, extra: [] });
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
  async pinMade(): Promise<void> {
    for (let tries = 1; ; tries++) {
      try {
        return await this.pin();
      } catch (error) {
        if (!(error instanceof SeenStoreError)) throw error;
        if (tries < 2) continue;
        console.warn("cloud account: the seen store would not take the pin of the genesis this device made; it is kept in localStorage:", error);
        const genesis = hex(this.basics.state!.hashAt(0)!);
        this.deps.genesisNote.write(genesis);
        this.pinnedAt = genesis;
        if (!this.deps.readAck()) this.deps.writeAck({ through: this.basics.state!.seq, extra: [] });
        return;
      }
    }
  }

  /** `state` held against what this browser holds ({@link seenAgainst}), the checkpoint kept apart as well. */
  heldAgainst(state: ChainState, held: Held): { record: Omit<SeenRecord, "firstSeen">; checked: boolean } {
    const against = this.seenAgainst(state, held.record);
    if (held.apart) {
      const apart = compareSeen(state, held.apart);
      if ("forkAt" in apart) throw this.latchFork(`this page's chain forks at entry ${apart.forkAt} from the one an older page of this browser verified`);
    }
    return against;
  }

  /**
   * The record `state` makes of what this browser holds (`held`), once held
   * against both its checkpoints (a fork latched and thrown): the record
   * moved forward with this view's first sightings ({@link seenNext}), and an
   * older page's kept apart folded in once that is proven one chain with it.
   */
  nextRecord(state: ChainState, held: Held, clock: ChainClock): SeenRecord {
    const next = seenNext(this.heldAgainst(state, held), held.record, clock);
    if (!held.apart) return next;
    const both = higherSeen(next, held.apart);
    if ("forkAt" in both) throw this.latchFork(`this page's chain forks at entry ${both.forkAt} from the one an older page of this browser verified`);
    return both.record!;
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
  async updateHeld(make: (held: Held) => { genesis?: string; record?: SeenRecord } | undefined, view?: ChainState): Promise<void> {
    const step: { genesis: string | null; record: SeenRecord | null } = { genesis: null, record: null };
    await this.atStore(() =>
      this.seen.update(this.basics.account, (raw) => {
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
   * `work` in this page's turn at the seen record (one at a time here),
   * holding the account's seen-record lock across the browser's pages (Web
   * Locks) where there are any.
   */
  withSeenLock<T>(work: () => T | Promise<T>): Promise<T> {
    const run = () => (this.locks ? this.locks.request(seenKey(this.basics.account), { mode: "exclusive" }, work) : work());
    const turn = this.seenTurn.then(run, run);
    this.seenTurn = turn.catch(() => undefined);
    return turn;
  }

  /** Forget the seen record (a wipe), in the store and where older pages keep it; the pin stays. */
  async forgetSeen(): Promise<void> {
    try {
      await this.withSeenLock(() =>
        this.atStore(() =>
          this.seen.update(this.basics.account, (raw) => {
            const stored = parseStored(raw);
            return stored.seen === null ? undefined : { ...stored, seen: null };
          }),
        ),
      );
    } catch (error) {
      console.warn("cloud account: could not forget the seen record:", error);
    }
    this.deps.seenNote.forget();
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
    const theirGenesis = this.deps.genesisNote.read();
    if (stored.genesis !== null && theirGenesis !== null && theirGenesis !== stored.genesis) {
      throw this.latchFork("an older page of this browser pinned a different genesis for this account");
    }
    const genesis = stored.genesis ?? theirGenesis;
    if (genesis === null) return { held: none, stored };
    const theirRaw = theirGenesis === genesis ? this.deps.seenNote.read() : null;
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
    const theirGenesis = this.deps.genesisNote.read();
    if (theirGenesis === null) this.deps.genesisNote.write(genesis);
    else if (theirGenesis !== genesis) return;
    // Where the store is `localStorage` itself, it wrote the record there.
    if (!record || !this.seen.atomic) return;
    const theirRaw = this.deps.seenNote.read();
    const theirs = theirRaw === null ? null : parseSeenJson(theirRaw);
    if (theirs) {
      if (theirs.seenThrough >= record.seenThrough) return;
      const both = higherSeen(record, theirs);
      if ("forkAt" in both) return;
      const holds = (mark: SeenRecord) => !!view && view.seq >= mark.seenThrough && hex(view.hashAt(mark.seenThrough)!) === mark.head;
      if (both.apart && !(holds(record) && holds(theirs))) return;
    }
    this.deps.seenNote.write(JSON.stringify(record));
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
    const done = () => !this.basics.state || this.forkedWhy !== null || this.deps.standing() === "removed";
    if (done()) return;
    if (!this.pinned) {
      // `pinned` is the store as this page last read or wrote it: another
      // page may have pinned the account since (two pairing screens open).
      // Taken in here, and this view held against the record from then on.
      await this.readHeld();
      if (done() || !this.pinned) return;
      this.basics.emit();
    }
    await this.withSeenLock(() =>
      this.updateHeld((held) => {
        const state = this.basics.state!;
        const genesis = hex(state.hashAt(0)!);
        if (held.genesis !== null && held.genesis !== genesis) throw this.latchFork("another genesis was pinned for this account under this page");
        if (held.genesis !== null && held.record && held.record.seenThrough >= state.seq) {
          // Not lowered below this view; a conflict still shows.
          this.heldAgainst(state, held);
          return undefined;
        }
        const what = held.genesis === null ? "the pin and the seen record were gone" : held.record ? `the seen record was lowered to entry ${held.record.seenThrough}` : "the seen record was gone";
        console.error(`cloud account: ALARM: ${what} under this page's ${state.seq}; putting it back`);
        return { genesis, record: this.nextRecord(state, held.genesis === null ? { genesis: null, record: null, apart: null } : held, this.basics.clock) };
      }),
    );
  }
}
