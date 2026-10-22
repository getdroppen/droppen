/*
 * Where a machine's offline copy comes from (`docs/local-aggregator.md` §6.2,
 * the `CopySource` seam).
 *
 * The copy (`mirror.ts`, and the machine list's `machine-status.ts`) keeps a
 * machine's index, status and sessions: what it shows, what it keeps on the
 * device, which sessions it follows. Where the records come from is not its
 * business. It asks a source by logical name — the index, the status, one
 * session — and the source does the rest:
 *
 * - {@link HubCopySource} (droppen.app): the hub's encrypted streams, read in
 *   the browser. It derives each name's stream ids from the machine's keys
 *   (`MK`, per key epoch: relay-and-plans §5), subscribes over the page's hub
 *   link (`streams.ts`), opens and checks every record, and says where a copy
 *   stands as the head the machine signed — so a copy read back from the
 *   device is trusted only while the machine's signature on it still checks
 *   (under any key it held: a renewal does not lose the copy).
 * - A gateway source (S1-7) will follow the same names through the local
 *   install, which has already opened and checked them.
 *
 * A copy is kept on the device under a *place* the source names (the hub's:
 * the stream id, hex) together with a *mark* the source can resume from (the
 * hub's: the signed head, encoded, hex) — the same record, in the same shape,
 * as before the source was its own thing, so every copy already on a device
 * still opens.
 */
import {
  type Bytes,
  type ChainState,
  decodeSignedHead,
  encodeSignedHead,
  epochsNewestFirst,
  fromHex,
  hex,
  indexStream,
  type MachineKey,
  namingKey,
  sessionNamingKey,
  type SignedHead,
  statusStream,
  streamId,
} from "@/lib/e2e";
import { feedFold, indexFold, type NamedStream, NewestStream, type SavedStream, statusFold, StreamClient, type StreamClientOptions, type StreamFold, type StreamSubscription } from "./streams";

/** What {@link HubCopySource.following} asks of the account (`CloudAccount` is one). */
export interface CopyAccount {
  /** A current machine of the account and its current sign key (`undefined`: not one now). */
  machine(id: string): { signKey: Uint8Array } | undefined;
  onChange(fn: () => void): () => void;
  /** The account's chain: a machine's key at the seq a copy kept on the device was verified at (`keysAt`), and its seq now. */
  readonly chain?: Pick<ChainState, "keysAt" | "deviceRecord" | "seq"> | null;
  /** Sync the account's chain: a head from the hub did not verify, and the machine may have renewed its key. */
  sync?(): Promise<unknown>;
}

/** A copy, by what it is rather than where it is. */
export type CopyName =
  | { readonly kind: "index" }
  | { readonly kind: "status" }
  /** `se`: the key epoch the session's index entry says its stream is named under (absent: the first). */
  | { readonly kind: "session"; readonly source: string; readonly id: string; readonly se?: unknown };

export type CopyKind = CopyName["kind"];

/** Where a copy stands, as its source can resume it. */
export interface CopyMark {
  /** Kept beside the state on the device; only its source reads it (the hub's: the signed head, encoded, hex). */
  readonly mark: string;
  /** The copy's version: the sequence number its next record will carry. */
  readonly seq: number;
  /** When the machine stood there, by its own clock (ms). */
  readonly timeMs: number;
}

/** A copy's state, and where it stands. */
export interface CopyAt<S> {
  readonly state: S;
  readonly at: CopyMark;
}

/** New state the machine stands behind. */
export interface CopyChange<S> extends CopyAt<S> {
  /** Rebuilt from nothing, rather than an update of the last state. */
  readonly reset: boolean;
  /** What was applied, in order. */
  readonly bodies: unknown[];
  /** How current the source knows it to be (ms). */
  readonly freshestMs: number;
}

export interface CopyEvents<S> {
  onState(change: CopyChange<S>): void;
  /** The catch-up is over; `exists: false` is a name the source has nothing under (yet). */
  onSynced?(exists: boolean): void;
  /** Followed under another place from here on (the index or status, moved to a newer key epoch's name). */
  onMoved?(place: string): void;
}

export interface CopyFollow {
  close(): void;
}

export interface CopySource {
  /** Resolves once the source knows whether it can follow anything. */
  ready(): Promise<void>;
  /** Whether it can: it holds keys for the machine. */
  readonly keyed: boolean;
  /** Hear when what it can follow changes: its first keys arrived, or more of them (a rotation). */
  onKeys(fn: () => void): () => void;
  /**
   * Where `name`'s copy may be kept on this device, the one to look at first
   * first (the index and status: under the newest key epoch, then older
   * ones). Empty: nothing can be had under it.
   */
  places(name: CopyName): Promise<string[]>;
  /** A mark kept at `place`, if it still holds (the hub's: the machine's signature on it checks). */
  resume(place: string, mark: string): Promise<CopyMark | null>;
  /**
   * Follow `name` from `places[start.at]` and the copy kept there (`places`
   * as {@link places} gave them). The index and status move on to a newer
   * place on their own ({@link CopyEvents.onMoved}).
   */
  follow<S>(name: CopyName, places: readonly string[], start: { at: number; from: CopyAt<S> | null }, events: CopyEvents<S>): CopyFollow | null;
  /** Whether waiting for a copy that is not on the device yet is worth it (the hub's: its link is up). */
  readonly reachable: boolean;
  /** Hear when a session it refused for want of room may be asked for again (none: it never refuses). */
  onRoom?(fn: () => void): () => void;
  close(): void;
}

/**
 * How many streams the page may follow over its one hub link at once. The
 * hub refuses a connection's subscriptions past its cap (`subs_per_conn`,
 * 256), and which one loses is a race — it could be a machine's index. So the
 * page keeps well under it, and what gives way is a session's stream, never
 * an index or a status: those are taken whatever the count (a machine's list
 * and name are what every view of it needs), and a session is followed only
 * while there is room.
 */
export class StreamBudget {
  private used = 0;
  /** A session was refused since room was last made: whoever was refused is told when there is room again. */
  private refused = false;
  private readonly roomListeners = new Set<() => void>();

  constructor(readonly limit = STREAM_BUDGET) {}

  /** Streams counted now. */
  get count(): number {
    return this.used;
  }

  /**
   * An index or a status: always granted, counted as the names it actually
   * has subscribed (`resize` as that changes — a newest-name follow watches
   * the newer names too while the machine has not moved yet).
   */
  require(weight = 1): { resize(weight: number): void; release(): void } {
    let held = weight;
    let done = false;
    this.used += held;
    return {
      resize: (next) => {
        if (done || next === held) return;
        this.add(next - held);
        held = next;
      },
      release: () => {
        if (done) return;
        done = true;
        this.add(-held);
      },
    };
  }

  /** A session's stream, if there is room for it; `null` when there is not. */
  take(): (() => void) | null {
    if (this.used >= this.limit) {
      this.refused = true;
      return null;
    }
    this.used += 1;
    let done = false;
    return () => {
      if (done) return;
      done = true;
      this.add(-1);
    };
  }

  /** Hear when room is made after a session was refused (to ask for it again). */
  onRoom(fn: () => void): () => void {
    this.roomListeners.add(fn);
    return () => this.roomListeners.delete(fn);
  }

  private add(delta: number): void {
    this.used += delta;
    if (delta >= 0 || !this.refused || this.used >= this.limit) return;
    this.refused = false;
    for (const fn of [...this.roomListeners]) {
      try {
        fn();
      } catch {
        // one listener failing must not starve the others
      }
    }
  }
}

/** The page's stream budget: three quarters of the hub's per-connection cap. */
export const STREAM_BUDGET = 192;

/** A hub stream's mark: its signed head, encoded only when it is kept. */
/**
 * Machine `host`'s sign key as `chain` had it at `seq` (`keysAt`) — at `null`,
 * its first (a position kept before positions carried a seq: from before any
 * renewal). `null` when the chain does not say.
 */
export function signKeyAt(chain: Pick<ChainState, "keysAt" | "deviceRecord"> | null | undefined, host: string, seq: number | null): Uint8Array | null {
  if (!chain) return null;
  const at = seq ?? chain.deviceRecord(host)?.addedSeq;
  return at === undefined ? null : (chain.keysAt(host, at)?.signKey ?? null);
}

class HeadMark implements CopyMark {
  private encoded: string | null = null;
  /** `keySeq`: the account chain's seq when this device verified `head` (its key then: `keysAt`). */
  constructor(
    readonly head: SignedHead,
    readonly keySeq: number | null = null,
  ) {}
  /** The signed head, encoded, hex — and `@<keySeq>` when known. */
  get mark(): string {
    this.encoded ??= hex(encodeSignedHead(this.head)) + (this.keySeq === null ? "" : `@${this.keySeq}`);
    return this.encoded;
  }
  get seq(): number {
    return this.head.head.nextSeq;
  }
  get timeMs(): number {
    return this.head.head.timeMs;
  }
}

/** A kept mark: the signed head (hex) and, after `@`, the chain seq it was verified at (absent in marks from before). */
function parseMark(mark: string): { head: SignedHead; keySeq: number | null } {
  const [encoded, seq] = mark.split("@");
  const keySeq = seq !== undefined && /^\d{1,15}$/.test(seq) ? Number(seq) : null;
  if (seq !== undefined && keySeq === null) throw new Error("not a mark");
  return { head: decodeSignedHead(fromHex(encoded!)), keySeq };
}

/** The signed head behind a mark this source made (or kept), with the seq it was verified at. */
function headOf(at: CopyMark): { head: SignedHead; keySeq: number | null } {
  return at instanceof HeadMark ? { head: at.head, keySeq: at.keySeq } : parseMark(at.mark);
}

const FOLDS: Record<CopyKind, StreamFold<unknown>> = {
  index: indexFold as StreamFold<unknown>,
  status: statusFold as StreamFold<unknown>,
  session: feedFold as StreamFold<unknown>,
};

/** A machine's index and status names under every epoch held, newest first. */
export async function namesOf(epochs: ReadonlyMap<number, MachineKey>): Promise<{ index: NamedStream[]; status: NamedStream[] }> {
  const ordered = epochsNewestFirst(epochs);
  const index = await Promise.all(ordered.map(async ([epoch, mk]) => ({ epoch, stream: await indexStream(mk) })));
  const status = await Promise.all(ordered.map(async ([epoch, mk]) => ({ epoch, stream: await statusStream(mk) })));
  return { index, status };
}

/**
 * The hub's streams of one machine (see the module comment). Over a client
 * that holds the machine's keys ({@link HubCopySource.over}), or following the
 * account's keys for it ({@link HubCopySource.following}): inert until the
 * first arrive, and taking each new epoch as it comes.
 */
/** When a machine's first key is looked for again, after the account showed the machine with none (ms after each look). */
export const KEY_RETRY_MS = [1_500, 4_000, 10_000, 30_000];

export class HubCopySource implements CopySource {
  private keys: { mk: MachineKey; client: StreamClient } | null;
  private readonly keyListeners = new Set<() => void>();
  private readonly loaded: Promise<void>;
  private offAccount: (() => void) | null = null;
  /** The client is this source's own (made here), and goes with it. */
  private ownsClient = false;
  private closed = false;

  private constructor(
    keys: { mk: MachineKey; client: StreamClient } | null,
    private readonly budget: StreamBudget | null,
    loaded: (source: HubCopySource) => Promise<void> = () => Promise.resolve(),
  ) {
    this.keys = keys;
    this.loaded = loaded(this);
  }

  /**
   * Over `keys`: `MK` of the first epoch (it names a session whose index entry
   * has no `se`) and a client holding every epoch; `null`: nothing to follow.
   * `budget`: the page's, when the client shares its link with others.
   */
  static over(keys: { mk: MachineKey; client: StreamClient } | null, budget: StreamBudget | null = null): HubCopySource {
    return new HubCopySource(keys, budget);
  }

  /**
   * `machine` of `account`, its keys read from the account now and again
   * whenever the account changes (`keys` reads them: `account.machineKeys`
   * with the page's key store). A client over `client` is made once there are
   * any; it is given every epoch that arrives after (`addEpochs`).
   *
   * A machine the account has but no key of yet — just added from another
   * page, whose first key went into the keyring after its chain entry — is
   * looked for again a few times (`refresh`: the keyring, then the account
   * tells its listeners), at {@link KEY_RETRY_MS}, until its key is there.
   *
   * The machine's sign key follows the account too (a renewal,
   * `docs/host-identity-rotation.md` §5.4): heads and manifests from the hub
   * are held to the key the account's chain gives now, a refused one has the
   * chain synced once (`account.sync`) before it stands, and a copy kept on
   * the device resumes under any key the machine held (`chain.retiredKeys`).
   */
  static following(opts: {
    account: CopyAccount;
    machine: string;
    keys: () => Promise<ReadonlyMap<number, MachineKey>>;
    client: Omit<StreamClientOptions, "machineSignKey" | "epochs">;
    /** The page's stream budget, shared by every machine's source. */
    budget?: StreamBudget | null;
    /** Look for the account's keys again (the keyring); the account's change then reaches this source. */
    refresh?: () => Promise<void>;
  }): HubCopySource {
    let running: Promise<void> = Promise.resolve();
    let retries = 0;
    let retry: ReturnType<typeof setTimeout> | undefined;
    return new HubCopySource(null, opts.budget ?? null, (source) => {
      // The machine renewed its identity key: what the hub sends from now on must carry the new one — at once, as the
      // account changes, never queued behind an earlier change's key loading (which waits on every stream's queue, and
      // a hub can stall those with page fetches while another stream would still take heads under the old key).
      const followKey = () => {
        try {
          const info = opts.account.machine(opts.machine);
          if (info && source.keys && !source.closed) source.keys.client.setMachineSignKey(info.signKey);
        } catch (error) {
          console.warn("cloud: machine key:", error);
        }
      };
      const sync = () => {
        followKey();
        running = running.then(async () => {
          try {
            const info = opts.account.machine(opts.machine);
            // Again here: a client made by an earlier turn of this queue, after the change was heard.
            followKey();
            const epochs = info ? await opts.keys() : new Map<number, MachineKey>();
            if (source.closed) return;
            if (source.keys) {
              await source.keys.client.addEpochs(epochs);
              return;
            }
            const mk = namingKey(epochs);
            if (info && !mk && opts.refresh && retry === undefined && retries < KEY_RETRY_MS.length) {
              // In the account, no key here yet: its key may be on its way into the keyring.
              retry = setTimeout(() => {
                retry = undefined;
                if (source.closed || source.keys) return;
                void opts.refresh!().catch((error: unknown) => console.warn("cloud: machine keys:", error));
              }, KEY_RETRY_MS[retries++]);
            }
            if (!info || !mk) return;
            const { account, machine } = opts;
            const client = new StreamClient({
              ...opts.client,
              machineSignKey: info.signKey,
              epochs,
              signKeyAt: opts.client.signKeyAt ?? ((seq) => signKeyAt(account.chain, machine, seq)),
              chainSeq: opts.client.chainSeq ?? (() => account.chain?.seq ?? null),
              resyncSignKey:
                opts.client.resyncSignKey ??
                (async () => {
                  await account.sync?.();
                  return account.machine(machine)?.signKey ?? null;
                }),
            });
            source.keys = { mk, client };
            source.ownsClient = true;
            source.emitKeys();
          } catch (error) {
            console.warn("cloud: machine keys:", error);
          }
        });
        return running;
      };
      source.offAccount = opts.account.onChange(() => void sync());
      return sync();
    });
  }

  ready(): Promise<void> {
    return this.loaded;
  }

  get keyed(): boolean {
    return this.keys !== null;
  }

  /** The client the streams are followed over (`null`: no keys). */
  get client(): StreamClient | null {
    return this.keys?.client ?? null;
  }

  onKeys(fn: () => void): () => void {
    let offEpochs: (() => void) | null = null;
    const attach = () => {
      if (!offEpochs && this.keys) offEpochs = this.keys.client.onEpochs(() => fn());
    };
    const first = () => {
      attach();
      fn();
    };
    attach();
    this.keyListeners.add(first);
    return () => {
      this.keyListeners.delete(first);
      offEpochs?.();
    };
  }

  /** The first keys arrived. */
  private emitKeys(): void {
    for (const fn of [...this.keyListeners]) {
      try {
        fn();
      } catch {
        // one listener failing must not starve the others
      }
    }
  }

  get reachable(): boolean {
    return this.keys?.client.opts.link.state === "open";
  }

  onRoom(fn: () => void): () => void {
    return this.budget?.onRoom(fn) ?? (() => undefined);
  }

  /** The named streams `places` last gave for the index and the status, by place. */
  private readonly named = new Map<string, NamedStream>();

  async places(name: CopyName): Promise<string[]> {
    const keys = this.keys;
    if (!keys) return [];
    if (name.kind === "session") {
      const mk = name.se === undefined ? keys.mk : sessionNamingKey(keys.client.epochKeys, name.se);
      return mk ? [hex(await streamId(mk, name.source, name.id))] : [];
    }
    const named = (await namesOf(keys.client.epochKeys))[name.kind];
    for (const n of named) this.named.set(hex(n.stream), n);
    return named.map((n) => hex(n.stream));
  }

  async resume(_place: string, mark: string): Promise<CopyMark | null> {
    const keys = this.keys;
    if (!keys) return null;
    try {
      const { head, keySeq } = parseMark(mark);
      // A mark this device kept: under the machine's key of the seq it was verified at (`keysAt`), or its current one.
      return (await keys.client.verifyKept(head, keySeq)) ? new HeadMark(head, keySeq) : null;
    } catch {
      return null;
    }
  }

  follow<S>(name: CopyName, places: readonly string[], start: { at: number; from: CopyAt<S> | null }, events: CopyEvents<S>): CopyFollow | null {
    const keys = this.keys;
    if (!keys || this.closed || !places.length) return null;
    const fold = FOLDS[name.kind] as StreamFold<S>;
    const kept = start.from ? headOf(start.from.at) : null;
    const saved: SavedStream<S> | null = start.from && kept ? { head: kept.head, state: start.from.state, keySeq: kept.keySeq } : null;
    const onState = (state: S, change: { reset: boolean; bodies: unknown[]; head: SignedHead; freshestMs: number | null; keySeq: number | null }) =>
      events.onState({ state, at: new HeadMark(change.head, change.keySeq), reset: change.reset, bodies: change.bodies, freshestMs: change.freshestMs ?? change.head.head.timeMs });
    if (name.kind === "session") {
      // Only while the page has room for another stream (`StreamBudget`).
      const release = this.budget ? this.budget.take() : () => undefined;
      if (!release) return null;
      const sub: StreamSubscription<S> = keys.client.subscribe(fromHex(places[start.at] ?? places[0]!) as Bytes, fold, { onState, onSynced: events.onSynced }, saved);
      return {
        close: () => {
          release();
          sub.close();
        },
      };
    }
    // The index and the status are counted, never refused, as the names
    // actually subscribed (one, or more while newer names are watched).
    const held = this.budget?.require(1) ?? null;
    const named = places.map((p) => this.named.get(p) ?? { epoch: 0, stream: fromHex(p) as Bytes });
    const newest = new NewestStream<S>(
      keys.client,
      fold,
      named,
      {
        onSwitch: (to) => {
          held?.resize(newest.followed);
          events.onMoved?.(hex(to.stream));
        },
        onState: (_at, state, change) => {
          held?.resize(newest.followed);
          onState(state, change);
        },
        onSynced: (exists) => {
          held?.resize(newest.followed);
          events.onSynced?.(exists);
        },
      },
      { at: start.at, saved },
    );
    held?.resize(newest.followed);
    // A rotation's key arriving: the machine moves its index and status to the new epoch's names.
    const offEpochs = keys.client.onEpochs((epochs) => {
      void namesOf(epochs).then((n) => {
        for (const x of n[name.kind]) this.named.set(hex(x.stream), x);
        newest.setCandidates(n[name.kind]);
        held?.resize(newest.followed);
      });
    });
    return {
      close: () => {
        held?.release();
        offEpochs();
        newest.close();
      },
    };
  }

  close(): void {
    this.closed = true;
    this.offAccount?.();
    this.offAccount = null;
    if (this.ownsClient) this.keys?.client.close();
    this.keyListeners.clear();
  }
}
