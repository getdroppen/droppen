/*
 * A machine's encrypted streams, read in the browser: what the page shows when
 * the machine is not there to ask (`docs/cross-machine-architecture.md` §4–5).
 *
 * The agent writes three kinds of append-only stream to the hub, each record
 * sealed under a key derived from the machine key `MK` and released only under
 * a head the machine signed (`lib/e2e/record.ts`, `lib/e2e/stream.ts`):
 *
 * - the index (`rh.index`): `{reset?, upserts: [SessionSummary], removed:
 *   ["source/id"], asOf}`, folded into the session list — or just `{asOf}`,
 *   the agent's daily write that keeps a quiet machine's index on the hub
 *   (relay-and-plans §3.4), which only moves the time;
 * - the status (`rh.status`): each body the whole status — settings, accounts,
 *   quota, CLI versions, the system and the agent's version — replacing the
 *   last;
 * - one per session: `{fromIndex, items, endIndex, fp, prefixFp}`, the session
 *   feed's replace-from semantics, folded into the whole transcript. A copy
 *   the account's retention window trimmed starts with a window-start page,
 *   the same body with no items plus `{base, before}` (see {@link FeedCopy}).
 *
 * A {@link StreamSubscription} follows one stream over the page's hub link:
 * `SUB` from where it stands, then the hub's catch-up — maybe `GONE` and a
 * `CHECKPOINT` (a signed manifest whose pages, fetched over HTTP and checked
 * against it, are the state at its head), records, a head, `stream_synced` —
 * and from then on the live tail. Everything for one stream is processed in
 * the order it arrived, one frame at a time, although opening a record is
 * asynchronous: a record taken twice, or a head confirmed against a half-read
 * batch, is exactly the bug serialising prevents.
 *
 * What it hands out is only ever state the machine signed for: the fold of the
 * records a signed head released, or the pages of a manifest the machine
 * signed. A head that does not match, a record that does not open, a gap in a
 * session's items — the stream starts over from nothing (backing off if it
 * keeps happening), and until it is rebuilt the last good state stays shown.
 *
 * A machine that renewed its identity key (`docs/host-identity-rotation.md`
 * §5.4): every head and manifest from the hub is held to its current key, as
 * the account's chain gives it ({@link StreamClient.setMachineSignKey}); one
 * that does not verify has the chain synced once and is tried again under the
 * key the chain gives then — the machine may have renewed it before this
 * device heard — and is refused if it still does not. A position this device
 * verified and kept itself (a saved copy's head, the head a stream restarts
 * from) checks under any key the machine held.
 */
import type { components } from "@/api/schema";
import {
  type Bytes,
  copy,
  decodeManifest,
  decodePage,
  decodeSignedHead,
  equal,
  type Frame,
  fromUtf8,
  hex,
  LogReader,
  type MachineKey,
  type Manifest,
  manifestUpto,
  openEphemeral,
  openManifestPage,
  readerFromManifest,
  RecordError,
  Serial,
  type SignedHead,
  verifyStreamHeadUnder,
  FMT_FEED_V1,
  FMT_INDEX_V1,
  FMT_STATUS_V1,
} from "@/lib/e2e";
import type { CopyKind } from "./copy-source";
import type { LinkState } from "./link";

type SessionSummary = components["schemas"]["SessionSummary"];
type TranscriptItem = components["schemas"]["TranscriptItem"];

// ---------------------------------------------------------------------------
// Folds
// ---------------------------------------------------------------------------

/** A body that cannot be folded: the stream copy is broken and is rebuilt. */
export class FoldError extends Error {
  constructor(
    readonly kind: "gap" | "malformed",
    message: string,
  ) {
    super(message);
    this.name = "FoldError";
  }
}

/** How one kind of stream turns its bodies into state. Pure: never mutates `state`. */
export interface StreamFold<S> {
  /** The record and page format its bodies carry. */
  readonly fmt: number;
  empty(): S;
  /** `state` with one more body applied; throws {@link FoldError}. */
  apply(state: S, body: unknown): S;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The session list: rows by `source/id`, and the time the agent last listed them. */
export interface IndexState {
  rows: Record<string, SessionSummary>;
  asOf: number | null;
}

export const indexKey = (row: Pick<SessionSummary, "source" | "id">) => `${row.source}/${row.id}`;

export const indexFold: StreamFold<IndexState> = {
  fmt: FMT_INDEX_V1,
  empty: () => ({ rows: {}, asOf: null }),
  apply(state, body) {
    if (!isRecord(body)) throw new FoldError("malformed", "an index body is an object");
    const rows: Record<string, SessionSummary> = body.reset === true ? {} : { ...state.rows };
    if (Array.isArray(body.removed)) {
      for (const key of body.removed) if (typeof key === "string") delete rows[key];
    }
    if (Array.isArray(body.upserts)) {
      for (const row of body.upserts) {
        if (isRecord(row) && typeof row.source === "string" && typeof row.id === "string") {
          rows[indexKey(row as unknown as SessionSummary)] = row as unknown as SessionSummary;
        }
      }
    }
    return { rows, asOf: typeof body.asOf === "number" ? body.asOf : state.asOf };
  },
};

/** The machine's status as its last body said it; every field may be missing. */
export interface StatusState {
  asOf: number | null;
  /** `GET /api/settings` as the machine answered it. */
  settings: Record<string, unknown> | null;
  /** `GET /api/accounts`, cut down to what is safe to keep (no credentials). */
  accounts: Record<string, unknown> | null;
  /** `GET /api/stats/quota`. */
  quota: Record<string, unknown> | null;
  cli: { claude: string | null; codex: string | null } | null;
  /**
   * The newest machine-key epoch the agent says it seals under (`null`: it
   * did not say — an agent from before it did, or a copy saved before then).
   * A device holding a newer epoch than this sees a machine still running an
   * old key.
   */
  mkEpoch: number | null;
  /**
   * What the machine is called: the name a full device gave it, else its
   * hostname — only ever in the sealed status, never told to the hub. Absent
   * from a copy saved before the agent sent it (read it with
   * {@link machineDisplayName}).
   */
  displayName?: string | null;
  /**
   * The machine's device-chain alarms (M2 re-review): it was served a chain
   * that conflicts with its own — a fork, a rollback, another genesis — by
   * the hub or by a device. Oldest first; absent when there are none.
   */
  alarms?: ChainAlarm[];
  /**
   * Which devices ever opened a channel to the machine, as its agent
   * recorded them (review D3; {@link ControllersRecord}). Absent from an
   * agent that does not keep the record, and from a body that does not parse
   * as one: then nobody knows.
   */
  controllers?: ControllersRecord;
  /**
   * The machine's system as its agent read it at start (S1-5; read it with
   * `systemLabel`). Absent from an agent from before it said, and from a
   * field that does not parse as one: then `settings.platform` stands in.
   */
  os?: StatusOs;
  /**
   * The agent's own version (S1-5). Absent from an agent from before it said:
   * then `settings.serverVersion` stands in.
   */
  agentVersion?: string;
}

/**
 * A machine's system as its status says it: `name` is the agent's name for
 * it (`macos`, `linux`, `windows`, …), `version` the system's (`15.3.1`,
 * `24.04`), `distro` the `os-release` ID of a Linux (`ubuntu`), `arch` the
 * processor's (`aarch64`, `x86_64`). Each but the name may be unknown.
 */
export interface StatusOs {
  name: string;
  version: string | null;
  distro: string | null;
  arch: string | null;
}

/** The longest field of the status's `os` or `agentVersion` the page keeps, in characters. */
const SYSTEM_FIELD_MAX = 64;

/** A short text field, trimmed and cut; `null` for anything else, or nothing. */
function shortText(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const v = value.trim().slice(0, SYSTEM_FIELD_MAX).trim();
  return v || null;
}

/** A status's `os`, defensively: one without a name is no system at all. */
export function parseStatusOs(value: unknown): StatusOs | undefined {
  if (!isRecord(value)) return undefined;
  const name = shortText(value.name);
  if (!name) return undefined;
  return { name, version: shortText(value.version), distro: shortText(value.distro), arch: shortText(value.arch) };
}

/**
 * The devices whose handshake a machine's agent ever accepted (the status's
 * `controllers`), complete for every handshake while its chain was at seq
 * `from` or later — since `fromMs` by its clock (its enrollment, or the
 * upgrade that started the record). `seq` is the agent's chain when the
 * body was written. Evidence for the re-enroll warning (`reEnrollEvidence`),
 * never a reason to drop it: a device that did connect controlled that
 * computer and could have edited the record.
 */
export interface ControllersRecord {
  from: number;
  fromMs: number;
  seq: number;
  /** By id: each device and when the agent first accepted its handshake. */
  devices: { id: string; firstMs: number }[];
}

/** The most devices a `controllers` record holds (the agent keeps 256). */
export const CONTROLLERS_MAX = 1024;

/** A status's `controllers`, strictly: anything malformed is no record at all. */
export function parseControllers(value: unknown): ControllersRecord | undefined {
  if (!isRecord(value)) return undefined;
  const { from, fromMs, seq, devices } = value;
  const count = (n: unknown): n is number => Number.isSafeInteger(n) && (n as number) >= 0;
  if (!count(from) || !count(fromMs) || !count(seq) || from > seq) return undefined;
  if (!Array.isArray(devices) || devices.length > CONTROLLERS_MAX) return undefined;
  const out: { id: string; firstMs: number }[] = [];
  for (const d of devices) {
    if (!isRecord(d) || typeof d.id !== "string" || d.id.length === 0 || d.id.length > 256 || !count(d.firstMs)) return undefined;
    out.push({ id: d.id, firstMs: d.firstMs });
  }
  return { from, fromMs, seq, devices: out };
}

/** One of a machine's chain alarms: when it raised it (its clock) and what it saw. */
export interface ChainAlarm {
  atMs: number;
  what: string;
}

/** How long a machine's chain alarm is shown. */
export const ALARM_SHOWN_MS = 7 * 24 * 60 * 60 * 1000;

/** The alarms in `status` worth showing at `now`: the last week's, newest first. */
export function recentAlarms(status: Pick<StatusState, "alarms"> | null | undefined, now = Date.now()): ChainAlarm[] {
  return (status?.alarms ?? []).filter((a) => now - a.atMs < ALARM_SHOWN_MS).sort((x, y) => y.atMs - x.atMs);
}

function alarmsOf(value: unknown): ChainAlarm[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter((a): a is { atMs: number; what: string } => isRecord(a) && Number.isSafeInteger(a.atMs) && typeof a.what === "string")
    .slice(-16)
    .map((a) => ({ atMs: a.atMs, what: a.what.slice(0, 300) }));
}

/** The longest display name the agent takes, in characters. */
export const DISPLAY_NAME_MAX = 64;

/** A machine's name from its status, or `null` when it has none to give (the id stands in). */
export function machineDisplayName(status: Pick<StatusState, "displayName"> | null | undefined): string | null {
  const name = status?.displayName;
  return typeof name === "string" && name.trim() ? name.trim() : null;
}

export const statusFold: StreamFold<StatusState> = {
  fmt: FMT_STATUS_V1,
  empty: () => ({ asOf: null, settings: null, accounts: null, quota: null, cli: null, mkEpoch: null }),
  apply(_state, body) {
    if (!isRecord(body)) throw new FoldError("malformed", "a status body is an object");
    const cli = isRecord(body.cli)
      ? {
          claude: typeof body.cli.claude === "string" ? body.cli.claude : null,
          codex: typeof body.cli.codex === "string" ? body.cli.codex : null,
        }
      : null;
    return {
      asOf: typeof body.asOf === "number" ? body.asOf : null,
      settings: isRecord(body.settings) ? body.settings : null,
      accounts: isRecord(body.accounts) ? body.accounts : null,
      quota: isRecord(body.quota) ? body.quota : null,
      cli,
      mkEpoch: Number.isInteger(body.mkEpoch) && (body.mkEpoch as number) >= 0 && (body.mkEpoch as number) <= 0xffff_ffff ? (body.mkEpoch as number) : null,
      // Only when there is one: a status that names nothing reads as before.
      ...(typeof body.displayName === "string" && body.displayName.trim() ? { displayName: body.displayName.slice(0, 4 * DISPLAY_NAME_MAX) } : {}),
      ...(alarmsOf(body.alarms).length ? { alarms: alarmsOf(body.alarms) } : {}),
      ...(parseControllers(body.controllers) ? { controllers: parseControllers(body.controllers) } : {}),
      // An older agent says neither: its settings' platform and version stand in (`systemLabel`, `agentVersionOf`).
      ...(parseStatusOs(body.os) ? { os: parseStatusOs(body.os) } : {}),
      ...(shortText(body.agentVersion) ? { agentVersion: shortText(body.agentVersion)! } : {}),
    };
  },
};

/**
 * One session's transcript as its stream has it: items `[startIndex,
 * endIndex)` (the whole of it, normally from 0), and the agent's fingerprints
 * of `[0, endIndex)` and of `[0, endIndex - 3)` — the latter the resume marker
 * a live server would have sent with the same window.
 */
export interface FeedCopy {
  /** Whether any body has been applied: the first one says where it starts. */
  started: boolean;
  startIndex: number;
  endIndex: number;
  items: TranscriptItem[];
  fp: string | null;
  prefixFp: string | null;
  /**
   * Set when the agent trimmed the copy to the account's retention window
   * (relay-and-plans §3.4): the copy holds the items from `base` on, and the
   * `base` entries before them are only on the machine, readable while it is
   * online. Absent when the copy was not trimmed by time — one that starts
   * past 0 only because of the agent's item cap has `startIndex > 0` and no
   * `base`. Read it with {@link copyWindow}.
   */
  base?: number;
  /** With `base`: when the newest entry left out was written (RFC 3339), `null` if it was undated. */
  before?: string | null;
}

const nonNegativeInt = (value: unknown): value is number => Number.isInteger(value) && (value as number) >= 0;

/**
 * Where a session copy the retention window trimmed starts: `base` entries
 * before it are only on the machine, the newest of them from `before`
 * (RFC 3339, or `null`). `null` for a copy that holds its session from where
 * the agent copies it.
 */
export function copyWindow(state: Pick<FeedCopy, "base" | "before"> | null | undefined): { base: number; before: string | null } | null {
  return state && nonNegativeInt(state.base) && state.base > 0 ? { base: state.base, before: typeof state.before === "string" ? state.before : null } : null;
}

export const feedFold: StreamFold<FeedCopy> = {
  fmt: FMT_FEED_V1,
  empty: () => ({ started: false, startIndex: 0, endIndex: 0, items: [], fp: null, prefixFp: null }),
  apply(state, body) {
    if (!isRecord(body) || !Array.isArray(body.items) || !nonNegativeInt(body.fromIndex) || !nonNegativeInt(body.endIndex)) {
      throw new FoldError("malformed", "a session body has fromIndex, items and endIndex");
    }
    const fromIndex = body.fromIndex;
    const items = body.items as TranscriptItem[];
    if (body.endIndex !== fromIndex + items.length) throw new FoldError("malformed", "a session body's items do not run from fromIndex to endIndex");
    const marks = {
      fp: typeof body.fp === "string" && body.fp ? body.fp : null,
      prefixFp: typeof body.prefixFp === "string" && body.prefixFp ? body.prefixFp : null,
    };
    if (nonNegativeInt(body.base) && items.length === 0) {
      // A window-start page (first in a trimmed copy's manifest): the copy
      // starts here, and the `base` entries before it stay on the machine.
      const before = typeof body.before === "string" ? body.before.slice(0, 64) : null;
      return { started: true, startIndex: fromIndex, endIndex: fromIndex, items: [], ...marks, base: body.base, before };
    }
    // What a window page said holds while the copy still starts past it.
    const held = state.base !== undefined && fromIndex >= state.base ? { base: state.base, before: state.before ?? null } : {};
    if (!state.started || fromIndex <= state.startIndex) {
      // The first body says where the copy starts; one from before the start
      // of what is held replaces all of it.
      return { started: true, startIndex: fromIndex, endIndex: body.endIndex, items, ...marks, ...held };
    }
    if (fromIndex > state.endIndex) throw new FoldError("gap", `a session body starts at ${fromIndex}, past the ${state.endIndex} items held`);
    return {
      started: true,
      startIndex: state.startIndex,
      endIndex: body.endIndex,
      items: [...state.items.slice(0, fromIndex - state.startIndex), ...items],
      ...marks,
      ...held,
    };
  },
};

/** The fold each kind of copy is kept with (`CopyKind` in `copy-source.ts`). */
export const copyFolds: Record<CopyKind, StreamFold<unknown>> = {
  index: indexFold as StreamFold<unknown>,
  status: statusFold as StreamFold<unknown>,
  session: feedFold as StreamFold<unknown>,
};

/** Parse one released body: JSON in UTF-8, of the fold's own format. */
function parseBody(fmt: number, expected: number, body: Uint8Array): unknown {
  if (fmt !== expected) throw new FoldError("malformed", `a body of format ${fmt} on a stream of format ${expected}`);
  try {
    return JSON.parse(fromUtf8(body));
  } catch {
    throw new FoldError("malformed", "a body is not JSON");
  }
}

// ---------------------------------------------------------------------------
// The wire
// ---------------------------------------------------------------------------

/** What a subscription needs of the hub link (`HubLink` is one). */
export interface StreamLink {
  readonly state: LinkState;
  onState(fn: (state: LinkState) => void): () => void;
  onControl(fn: (message: Record<string, unknown>) => void): () => void;
  reserve(route: (frame: Frame) => void): number;
  release(ch: number): void;
  send(frame: Frame): boolean;
  sendControl(message: Record<string, unknown>): boolean;
}

/** Run at most `n` jobs at once, in the order they were asked for. */
export class Limiter {
  private running = 0;
  private readonly queue: (() => void)[] = [];

  constructor(private readonly n: number) {}

  async run<T>(job: () => Promise<T>): Promise<T> {
    if (this.running >= this.n) await new Promise<void>((resolve) => this.queue.push(resolve));
    this.running++;
    try {
      return await job();
    } finally {
      this.running--;
      this.queue.shift()?.();
    }
  }
}

export interface StreamClientOptions {
  link: StreamLink;
  /**
   * The machine's Ed25519 public key, from the device chain: its current one,
   * which every head, manifest and draft from the hub must carry (a renewal:
   * {@link StreamClient.setMachineSignKey}).
   */
  machineSignKey: Uint8Array;
  /**
   * The machine's sign key as the account's chain had it at `seq` (`keysAt`;
   * `null`: its first key — a position kept before positions carried a seq).
   * A position this device verified and kept itself — a saved copy's head,
   * the head a stream restarts from — checks under the key of the seq it was
   * verified at (`SavedStream.keySeq`), not under any key the machine ever
   * held (docs/host-identity-rotation.md §5.4). Never what a head or manifest
   * from the hub is held to.
   */
  signKeyAt?: (seq: number | null) => Uint8Array | null | undefined;
  /** The account chain's seq now: what a head this device verifies is kept with (`SavedStream.keySeq`). */
  chainSeq?: () => number | null;
  /**
   * A head or manifest from the hub did not verify under the machine's key:
   * sync the account's chain and give the machine's current key then
   * (`null`: none known). Asked once for each such refusal (refusals at the
   * same time share it) and at most once per `resyncEveryMs` (default 10 s);
   * without it the refusal stands.
   */
  resyncSignKey?: () => Promise<Uint8Array | null | undefined>;
  resyncEveryMs?: number;
  /** Every epoch of `MK` this device holds. */
  epochs: ReadonlyMap<number, MachineKey>;
  /** `GET /v1/streams/<stream>/pages/<hash>`: the sealed page, unchecked. */
  fetchPage(stream: Uint8Array, page: Uint8Array): Promise<Uint8Array>;
  /** Page fetches at once, across every stream (default 4). */
  pageConcurrency?: number;
  /** How long to wait before the `attempt`th restart in a row (0-based). */
  retryDelay?: (attempt: number) => number;
  /**
   * A record or page came under a key epoch this client does not hold (a
   * rotation it has not caught up with): look for keys again, and hand any
   * new ones to {@link StreamClient.addEpochs}. Asked at most once per
   * `missingKeyEveryMs` (default 10 s), however many streams stall.
   */
  onMissingKey?: () => void;
  missingKeyEveryMs?: number;
}

/** 0, then 1 s doubling to a minute: a stream that keeps breaking is not hammered. */
export function streamRetryDelay(attempt: number): number {
  return attempt <= 0 ? 0 : Math.min(1000 * 2 ** (attempt - 1), 60_000);
}

/** What a subscription says about itself. */
export interface StreamEvents<S> {
  /**
   * New state the machine signed for: records a signed head released, or a
   * manifest's pages. `reset` when it was rebuilt from nothing (not an
   * update of the last state), `bodies` what was applied, in order.
   */
  onState?(state: S, change: { reset: boolean; bodies: unknown[]; head: SignedHead; seq: number; freshestMs: number | null; keySeq: number | null }): void;
  /** The catch-up is over; `exists: false` is a stream the hub has not got yet. */
  onSynced?(exists: boolean): void;
  /** A live draft (`EPHEMERAL`), opened. Never stored. */
  onDraft?(body: unknown): void;
  /** Something went wrong and the stream is starting over (diagnostics). */
  onError?(error: unknown): void;
}

/** Where a saved copy stands: the signed head at its position, and its state there. */
export interface SavedStream<S> {
  head: SignedHead;
  state: S;
  /**
   * The account chain's seq when this device verified `head` (`StreamClientOptions.chainSeq`):
   * the machine's key then is the one it was checked under (`keysAt`). Absent: kept before
   * positions carried one — the machine's first key.
   */
  keySeq?: number | null;
}

/** Every stream subscription of one machine, over one hub link. */
export class StreamClient {
  private readonly subs = new Set<StreamSubscription<unknown>>();
  private readonly limiter: Limiter;
  private readonly offs: (() => void)[];
  /** Every epoch key held: those it started with, and any added since (`addEpochs`). */
  private readonly epochs: Map<number, MachineKey>;
  private missingAskedAt = Number.NEGATIVE_INFINITY;
  private readonly epochListeners = new Set<(epochs: ReadonlyMap<number, MachineKey>) => void>();
  /** The machine's current key: what heads, manifests and drafts from the hub must carry. */
  private signKey: Bytes;
  private resyncing: Promise<void> | null = null;
  private resyncedAt = Number.NEGATIVE_INFINITY;

  constructor(readonly opts: StreamClientOptions) {
    this.signKey = copy(opts.machineSignKey);
    this.epochs = new Map(opts.epochs);
    this.limiter = new Limiter(opts.pageConcurrency ?? 4);
    this.offs = [
      opts.link.onState((state) => {
        if (state === "open") for (const sub of this.subs) sub.linkOpened();
      }),
      opts.link.onControl((message) => {
        const ch = message.ch;
        if (typeof ch !== "number" || (message.t !== "stream_synced" && message.t !== "stream_refused")) return;
        for (const sub of this.subs) if (sub.holds(ch)) sub.control(ch, message);
      }),
    ];
  }

  /** Follow `stream` from `saved` (or from nothing). Starts at once. */
  subscribe<S>(stream: Bytes, fold: StreamFold<S>, events: StreamEvents<S> = {}, saved: SavedStream<S> | null = null): StreamSubscription<S> {
    const sub = new StreamSubscription<S>(this, stream, fold, events);
    this.subs.add(sub as StreamSubscription<unknown>);
    sub.start(saved);
    return sub;
  }

  /** @internal */
  forget(sub: StreamSubscription<unknown>): void {
    this.subs.delete(sub);
  }

  /** Stop every subscription and stop listening to the link. */
  close(): void {
    for (const sub of [...this.subs]) sub.close();
    for (const off of this.offs) off();
  }

  /**
   * Key epochs this device came to hold since (a rotation's new `MK`):
   * every live reader takes them at once, and a stream that stopped for want
   * of one starts again now rather than after its backoff. A machine that
   * names its streams per epoch moves them to the new epoch's names, so
   * whoever follows a machine's index or status hears of the new epochs
   * ({@link StreamClient.onEpochs}). Returns how many were new.
   */
  async addEpochs(epochs: ReadonlyMap<number, MachineKey>): Promise<number> {
    const fresh = [...epochs].filter(([epoch]) => !this.epochs.has(epoch));
    if (!fresh.length) return 0;
    for (const [epoch, mk] of fresh) this.epochs.set(epoch, mk);
    await Promise.all([...this.subs].map((sub) => sub.addEpochs(fresh)));
    for (const fn of [...this.epochListeners]) {
      try {
        fn(this.epochKeys);
      } catch {
        // one listener failing must not starve the others
      }
    }
    return fresh.length;
  }

  /** Every `MK` epoch held now, and its key. */
  get epochKeys(): ReadonlyMap<number, MachineKey> {
    return new Map(this.epochs);
  }

  /** Hear of epochs added later ({@link StreamClient.addEpochs}). */
  onEpochs(fn: (epochs: ReadonlyMap<number, MachineKey>) => void): () => void {
    this.epochListeners.add(fn);
    return () => this.epochListeners.delete(fn);
  }

  /** The machine's current key, as this client holds it: what every head and manifest from the hub must carry. */
  get machineSignKey(): Bytes {
    return copy(this.signKey);
  }

  /**
   * The machine renewed its identity key (the account's chain says so):
   * every head, manifest and draft from the hub must carry `key` from now on,
   * and none signed by the old one counts any more. Live readers take it at
   * once, and a stream that stopped (on a head under the new key, say)
   * starts again now rather than after its backoff. Returns whether it changed.
   */
  setMachineSignKey(key: Uint8Array): boolean {
    if (equal(key, this.signKey)) return false;
    this.signKey = copy(key);
    for (const sub of [...this.subs]) sub.signKeyChanged(this.signKey);
    return true;
  }

  /**
   * @internal The key a position this device kept at chain seq `keySeq` was
   * checked under (`signKeyAt`; `null`: kept before positions carried one),
   * beside the current one: a copy stays readable across a renewal, under
   * exactly the key it was verified with — not any key the machine held.
   */
  keptKeys(keySeq: number | null): Uint8Array[] {
    let then: Uint8Array | null | undefined;
    try {
      then = this.opts.signKeyAt?.(keySeq);
    } catch {
      then = null;
    }
    return then && !equal(then, this.signKey) ? [then] : [];
  }

  /** @internal The account chain's seq now, kept with a head this device verifies (`null`: not known). */
  stamp(): number | null {
    try {
      return this.opts.chainSeq?.() ?? null;
    } catch {
      return null;
    }
  }

  /**
   * Whether `head` is a position to resume from: one this device verified and
   * kept itself at chain seq `keySeq` — signed by the machine's key of that
   * seq (`keysAt`), or its current one. Never for a head from the hub.
   */
  verifyKept(head: SignedHead, keySeq: number | null = null): Promise<boolean> {
    return verifyStreamHeadUnder(head, [this.signKey, ...this.keptKeys(keySeq)]);
  }

  /**
   * @internal A head or manifest from the hub did not verify under `tried`:
   * sync the account's chain (once for every refusal meanwhile, at most once
   * per `resyncEveryMs`) and give the machine's current key if it is another
   * one now. `null`: there is no other key to try, and the refusal stands.
   */
  async renewedSignKey(tried: Uint8Array): Promise<Bytes | null> {
    const resync = this.opts.resyncSignKey;
    if (resync && !this.resyncing && Date.now() - this.resyncedAt >= (this.opts.resyncEveryMs ?? 10_000)) {
      this.resyncedAt = Date.now();
      const run: Promise<void> = (async () => {
        try {
          const key = await resync();
          if (key) this.setMachineSignKey(key);
        } catch {
          // no chain to be had now: the refusal stands
        }
      })();
      this.resyncing = run;
      void run.then(() => {
        if (this.resyncing === run) this.resyncing = null;
      });
    }
    await this.resyncing;
    return equal(this.signKey, tried) ? null : copy(this.signKey);
  }

  /** @internal A stream stalled on an epoch it has no key for: ask for keys (rate-limited). */
  missingKey(): void {
    const now = Date.now();
    if (now - this.missingAskedAt < (this.opts.missingKeyEveryMs ?? 10_000)) return;
    this.missingAskedAt = now;
    this.opts.onMissingKey?.();
  }

  /** @internal A reader with every epoch key this device holds. */
  async keyed(reader: LogReader): Promise<LogReader> {
    for (const [epoch, mk] of this.epochs) await reader.addEpoch(epoch, mk);
    return reader;
  }

  /** The newest `MK` epoch held (`null`: none): the only one a draft may come under. */
  get newestEpoch(): number | null {
    let newest: number | null = null;
    for (const epoch of this.epochs.keys()) if (newest === null || epoch > newest) newest = epoch;
    return newest;
  }

  /**
   * @internal The bodies of `m`'s pages, in order: fetched (the last first,
   * since the newest state is the one a reader looks at), each checked
   * against the manifest's hash, opened under its own epoch's key, and held
   * to `fmt` — the manifest carries no format, each page says its own.
   */
  async pages(m: Manifest, reader: LogReader, fmt: number): Promise<Bytes[]> {
    const stream = m.head.head.stream;
    const out: Bytes[] = new Array(m.pages.length);
    const order = m.pages.map((_, i) => i).reverse();
    await Promise.all(
      order.map((i) =>
        this.limiter.run(async () => {
          const encoded = await this.opts.fetchPage(stream, m.pages[i]!);
          const key = reader.keyFor(decodePage(encoded).epoch);
          if (!key) throw new RecordError("noKey", `no key for page ${i}'s epoch`);
          const page = await openManifestPage(key, m, i, encoded);
          if (page.fmt !== fmt) throw new RecordError("malformed", `page ${i} is format ${page.fmt} on a stream of format ${fmt}`);
          out[i] = page.body;
        }),
      ),
    );
    return out;
  }
}

/**
 * One stream, followed. Owns its `ch` on the link (a new one every time it
 * subscribes, never reused), its {@link LogReader}, and the folded state at
 * the reader's confirmed position — the only state it ever hands out.
 */
export class StreamSubscription<S> {
  private reader: LogReader | null = null;
  /** The fold at `reader.confirmed`; `null` once dropped (`gone`) until rebuilt. */
  private state: S | null = null;
  /** The signed head `state` stands at; `null` for a reader that has confirmed nothing yet. */
  private head: SignedHead | null = null;
  /** The chain seq {@link head} was verified at (its key: `keysAt`), `null` when not known. */
  private headSeq: number | null = null;
  /** The next state handed out replaces rather than updates (a fresh start, a `gone`). */
  private rebuilding = true;
  private ch: number | null = null;
  private synced = false;
  private closed = false;
  private failures = 0;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private retryFrom: "confirmed" | "fresh" = "fresh";
  private readonly serial = new Serial();
  private readonly hexId: string;

  constructor(
    private readonly client: StreamClient,
    readonly stream: Bytes,
    private readonly fold: StreamFold<S>,
    private readonly events: StreamEvents<S>,
  ) {
    this.hexId = hex(stream);
  }

  /** The stream id as the hub spells it. */
  get id(): string {
    return this.hexId;
  }

  /** The seq the state stands at. */
  get confirmed(): number {
    return this.head?.head.nextSeq ?? 0;
  }

  /** Whether the hub has said the catch-up on the current `ch` is over. */
  get isSynced(): boolean {
    return this.synced;
  }

  /** Whether it holds a state the machine signed for (one saved and still the machine's, or one read since). */
  get hasState(): boolean {
    return this.head !== null;
  }

  /** @internal */
  holds(ch: number): boolean {
    return this.ch === ch;
  }

  /** @internal */
  start(saved: SavedStream<S> | null): void {
    void this.serial.run(async () => {
      if (saved && equal(saved.head.head.stream, this.stream)) {
        try {
          // The head is checked again: nothing read back from the device is
          // trusted for where it stands unless the machine signed it — under
          // its key of the seq this device checked it at before keeping it.
          const keySeq = saved.keySeq ?? null;
          this.install(await this.client.keyed(await LogReader.fromHead(this.client.machineSignKey, saved.head, this.client.keptKeys(keySeq))));
          this.state = saved.state;
          this.head = saved.head;
          this.headSeq = keySeq;
          this.rebuilding = false;
        } catch {
          this.reader = null;
        }
      }
      if (!this.reader) await this.fresh();
      this.subscribeNow();
    });
  }

  /** Stop following; the link is told, and nothing more is handed out. */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.dropCh();
    this.client.forget(this as StreamSubscription<unknown>);
  }

  /** @internal The link is (back) up: subscribe again from where the state stands. */
  linkOpened(): void {
    // A restart already waiting out its backoff subscribes when it is due.
    if (this.closed || this.ch !== null || this.timer !== undefined) return;
    void this.serial.run(async () => {
      // Subscribed meanwhile (a start that was still queued): nothing to do.
      if (this.ch === null) await this.restart("confirmed");
    });
  }

  /** @internal `stream_synced` or `stream_refused` for `ch`. */
  control(ch: number, message: Record<string, unknown>): void {
    void this.serial.run(async () => {
      if (this.closed || this.ch !== ch) return;
      if (message.t === "stream_synced") {
        this.synced = true;
        this.events.onSynced?.(message.exists !== false);
        return;
      }
      // Refused: not a broken copy, so it is kept; asked again later.
      this.events.onError?.(new Error(`the hub refused the stream: ${String(message.code ?? "refused")}${typeof message.message === "string" ? ` (${message.message})` : ""}`));
      this.dropCh();
      this.retryLater("confirmed", typeof message.retry_ms === "number" ? message.retry_ms : 0);
    });
  }

  /**
   * Install `reader` as the one this stream reads with (M2): first held to
   * the client's key as it is NOW — the machine may have renewed it while
   * the reader was being built (its epochs installed), and the renewal
   * reached only the reader there was then. Heads from here on are held to
   * the current key, never the one the reader started with.
   */
  private install(reader: LogReader): void {
    reader.setMachineKey(this.client.machineSignKey);
    this.reader = reader;
  }

  private async fresh(): Promise<void> {
    this.install(await this.client.keyed(LogReader.fromStart(this.stream, this.client.machineSignKey)));
    this.state = this.fold.empty();
    this.head = null;
    this.rebuilding = true;
  }

  private dropCh(): void {
    const ch = this.ch;
    if (ch === null) return;
    this.ch = null;
    this.synced = false;
    this.client.opts.link.sendControl({ t: "stream_unsub", ch });
    this.client.opts.link.release(ch);
  }

  /** `SUB` on a fresh `ch` from where the reader stands, if the link is up (else on its next open). */
  private subscribeNow(): void {
    if (this.closed || !this.reader) return;
    const link = this.client.opts.link;
    if (link.state !== "open") return;
    const ch = link.reserve((frame) => this.route(ch, frame));
    this.ch = ch;
    this.synced = false;
    link.send({ type: "sub", ch, stream: this.stream, fromSeq: this.reader.confirmed });
  }

  /** Start over: from the confirmed state (a new reader at its head), or from nothing. */
  private async restart(from: "confirmed" | "fresh"): Promise<void> {
    if (this.closed) return;
    this.dropCh();
    if (from === "fresh" || this.state === null) await this.fresh();
    else {
      // A new reader, because the old one may hold records it took but no
      // head released: the hub sends them again from the confirmed seq. The
      // head it stands on is one this device checked (it may carry a key the
      // machine renewed away from since); what comes next is held to the current.
      const key = this.client.machineSignKey;
      this.install(await this.client.keyed(this.head ? await LogReader.fromHead(key, this.head, this.client.keptKeys(this.headSeq)) : LogReader.fromStart(this.stream, key)));
    }
    this.subscribeNow();
  }

  /** @internal The machine's key changed (`StreamClient.setMachineSignKey`): the reader holds heads to it from now on, and a stalled restart is brought forward. */
  signKeyChanged(key: Uint8Array): void {
    if (this.closed) return;
    this.reader?.setMachineKey(key);
    if (this.timer === undefined) return;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.failures = 0;
    const from = this.retryFrom;
    void this.serial.run(() => this.restart(from));
  }

  /**
   * `check` a head or manifest from the hub under `tried`, the machine's key
   * as this stream holds it. One that does not verify (`badHead`) may be
   * under a key the machine renewed to before this device heard: the chain
   * is synced once and, if it gives another key now, `check` runs again under
   * that one. Otherwise the refusal stands.
   */
  private async underCurrentKey<T>(tried: Uint8Array, check: (key: Uint8Array) => Promise<T>): Promise<T> {
    try {
      return await check(tried);
    } catch (error) {
      if (!(error instanceof RecordError && error.kind === "badHead")) throw error;
      const renewed = await this.client.renewedSignKey(tried);
      if (!renewed || this.closed) throw error;
      return check(renewed);
    }
  }

  /** @internal New epoch keys (`StreamClient.addEpochs`): into the reader, and a stalled restart brought forward. */
  addEpochs(keys: [number, MachineKey][]): Promise<void> {
    return this.serial.run(async () => {
      if (this.closed) return;
      if (this.reader) for (const [epoch, mk] of keys) await this.reader.addEpoch(epoch, mk);
      if (this.timer !== undefined) {
        clearTimeout(this.timer);
        this.timer = undefined;
        this.failures = 0;
        await this.restart(this.retryFrom);
      }
    });
  }

  private retryLater(from: "confirmed" | "fresh", atLeastMs = 0): void {
    this.retryFrom = from;
    clearTimeout(this.timer);
    const wait = Math.max(atLeastMs, (this.client.opts.retryDelay ?? streamRetryDelay)(this.failures++));
    this.timer = setTimeout(() => {
      this.timer = undefined;
      void this.serial.run(() => this.restart(from));
    }, wait);
  }

  private route(ch: number, frame: Frame): void {
    if (frame.type === "close") {
      // The link went away under it; it is back on the next open.
      if (this.ch === ch) {
        this.ch = null;
        this.synced = false;
      }
      return;
    }
    void this.serial.run(() => this.process(ch, frame));
  }

  private async process(ch: number, frame: Frame): Promise<void> {
    if (this.closed || this.ch !== ch) return;
    if ("stream" in frame && !equal(frame.stream, this.stream)) return;
    try {
      switch (frame.type) {
        case "gone":
          // Compacted past where this stood: a checkpoint follows. What was
          // shown stays shown until it is rebuilt.
          this.reader = null;
          this.state = null;
          this.head = null;
          this.rebuilding = true;
          return;
        case "checkpoint":
          return await this.checkpoint(ch, frame.manifest);
        case "recs":
          if (!this.reader) throw new RecordError("outOfOrder", "records arrived with no starting point");
          for (const record of frame.records) await this.reader.push(record);
          return;
        case "head":
          return await this.confirm(decodeSignedHead(frame.head));
        case "ephemeral":
          if (this.reader && this.events.onDraft) {
            const reader = this.reader;
            // Signed by the machine (under its current key), and under the
            // newest key epoch held: a draft under an older one is somebody
            // who still holds that key.
            const current = this.client.newestEpoch;
            if (current === null) return;
            try {
              const { body } = await openEphemeral((epoch) => reader.keyFor(epoch), this.stream, this.client.machineSignKey, current, frame.payload);
              this.events.onDraft(JSON.parse(fromUtf8(body)));
            } catch {
              // a draft that does not open is only a draft not shown
            }
          }
          return;
        default:
          return;
      }
    } catch (error) {
      this.failed(error);
    }
  }

  private failed(error: unknown): void {
    this.events.onError?.(error);
    if (error instanceof RecordError && error.kind === "noKey") this.client.missingKey();
    this.dropCh();
    // A head that arrived before the records it covers rolled the reader
    // back to what it had confirmed; anything else means this copy cannot be
    // trusted to continue, and it is rebuilt from the start.
    const incomplete = error instanceof RecordError && error.kind === "incomplete";
    this.retryLater(incomplete ? "confirmed" : "fresh");
  }

  private async confirm(signed: SignedHead): Promise<void> {
    const reader = this.reader;
    const before = this.state;
    if (!reader || before === null) throw new RecordError("outOfOrder", "a head arrived with no starting point");
    // A refused head leaves the reader as it was, so it can be tried again. The chain's seq is noted as the key is
    // set for the check: the position is kept with the seq whose key verified it, not a later one.
    const at: { seq: number | null } = { seq: null };
    // Held to the client's current key, not whatever the reader was built with (M2).
    const released = await this.underCurrentKey(this.client.machineSignKey, (key) => {
      reader.setMachineKey(key);
      at.seq = this.client.stamp();
      return reader.confirm(signed);
    });
    const head = reader.confirmedHead;
    if (!head) return;
    if (released.length === 0 && !this.rebuilding && this.head === head) return;
    let state: S = before;
    const bodies: unknown[] = [];
    for (const record of released) {
      const body = parseBody(record.fmt, this.fold.fmt, record.body);
      state = this.fold.apply(state, body);
      bodies.push(body);
    }
    const reset = this.rebuilding;
    this.state = state;
    this.head = head;
    // A head from the hub: kept with the chain's seq it was verified at — that seq's key is the one it carries.
    this.headSeq = at.seq;
    this.rebuilding = false;
    if (released.length > 0) this.failures = 0;
    this.events.onState?.(state, { reset, bodies, head, seq: head.head.nextSeq, freshestMs: reader.freshestMs, keySeq: this.headSeq });
  }

  private async checkpoint(ch: number, encoded: Uint8Array): Promise<void> {
    const m = decodeManifest(encoded);
    if (!equal(m.head.head.stream, this.stream)) throw new RecordError("forged", "a checkpoint for another stream");
    const upto = manifestUpto(m);
    const reader = this.reader;
    // A compaction this copy is already past, or standing exactly on: nothing
    // to rebuild. (Checked before the signature: it changes nothing here.)
    // Only a copy some signed head (or manifest) has already confirmed counts:
    // the empty state of a reader that has confirmed nothing is not the state
    // a manifest at 0 describes.
    if (
      reader &&
      this.state !== null &&
      this.head !== null &&
      (reader.confirmed > upto || (reader.confirmed === upto && equal(reader.confirmedChainLink, m.head.head.link)))
    ) {
      return;
    }
    const next = await this.client.keyed(await this.underCurrentKey(this.client.machineSignKey, (key) => readerFromManifest(key, m)));
    const pages = await this.client.pages(m, next, this.fold.fmt);
    // The machine renewed its key while the pages came: the manifest is held to the new one (refused, and asked for again).
    if (!equal(next.machineKey, this.client.machineSignKey)) throw new RecordError("badHead", "the machine renewed its key while its manifest was read");
    let state = this.fold.empty();
    const bodies: unknown[] = [];
    for (const page of pages) {
      const body = parseBody(this.fold.fmt, this.fold.fmt, page);
      state = this.fold.apply(state, body);
      bodies.push(body);
    }
    if (this.closed) return;
    this.install(next);
    this.state = state;
    this.head = m.head;
    this.headSeq = this.client.stamp();
    this.rebuilding = false;
    this.failures = 0;
    this.events.onState?.(state, { reset: true, bodies, head: m.head, seq: upto, freshestMs: next.freshestMs, keySeq: this.headSeq });
    // Past the catch-up, records after the manifest may already have gone by
    // on this `ch`: ask for them again from where the state now stands.
    if (this.synced && this.ch === ch) {
      this.dropCh();
      this.subscribeNow();
    }
  }
}

// ---------------------------------------------------------------------------
// The newest of a machine's index (or status) streams
// ---------------------------------------------------------------------------

/** One of a machine's index (or status) streams: its name under one key epoch. */
export interface NamedStream {
  epoch: number;
  stream: Bytes;
}

/** What a {@link NewestStream} says. */
export interface NewestStreamEvents<S> {
  /** The stream shown is another now: a newer epoch's that has something, or — looking — an older one. */
  onSwitch?(to: NamedStream): void;
  /** New state of the stream shown (as {@link StreamEvents.onState}). */
  onState?(at: NamedStream, state: S, change: { reset: boolean; bodies: unknown[]; head: SignedHead; seq: number; freshestMs: number | null; keySeq: number | null }): void;
  /** Every stream looked at has caught up: whether any of them is on the hub. */
  onSynced?(exists: boolean): void;
}

/**
 * Follow the newest of a machine's index (or status) streams
 * (relay-and-plans §5). An agent that names its streams per key epoch keeps
 * these under its newest epoch and moves them on a rotation; one that has not
 * yet (off since, or from before `streams/2`) has them under an older one.
 * So: the newest epoch held is tried first, then older ones while the newer
 * say they have nothing — and every newer name stays followed, so the moment
 * the machine moves (its new name gets something) this switches to it and lets
 * go of the older ones. Starts from the stream `start.at` (newest first), with
 * the copy saved for it.
 */
export class NewestStream<S> {
  private candidates: NamedStream[];
  /** Which candidate is shown. */
  private shown: number;
  private readonly subs = new Map<string, { at: NamedStream; sub: StreamSubscription<S>; synced: boolean; exists: boolean }>();
  private closed = false;

  constructor(
    private readonly client: StreamClient,
    private readonly fold: StreamFold<S>,
    candidates: readonly NamedStream[],
    private readonly events: NewestStreamEvents<S>,
    start: { at: number; saved: SavedStream<S> | null } = { at: 0, saved: null },
  ) {
    this.candidates = [...candidates].sort((a, b) => b.epoch - a.epoch);
    this.shown = Math.min(Math.max(start.at, 0), Math.max(this.candidates.length - 1, 0));
    for (let i = 0; i <= this.shown && i < this.candidates.length; i++) this.follow(this.candidates[i]!, i === this.shown ? start.saved : null);
  }

  /** The stream shown now (`null`: no candidate at all). */
  get current(): NamedStream | null {
    return this.candidates[this.shown] ?? null;
  }

  /** How many of the names are subscribed right now: the shown one, and every newer one still watched. */
  get followed(): number {
    return this.subs.size;
  }

  /** The machine's names under every epoch held now (more epochs came): the newer ones are followed too. */
  setCandidates(candidates: readonly NamedStream[]): void {
    if (this.closed) return;
    const shownHex = this.current ? hex(this.current.stream) : null;
    const known = new Set(this.candidates.map((c) => hex(c.stream)));
    this.candidates = [...candidates].sort((a, b) => b.epoch - a.epoch);
    const at = this.candidates.findIndex((c) => hex(c.stream) === shownHex);
    this.shown = at < 0 ? 0 : at;
    for (let i = 0; i < this.shown; i++) {
      const c = this.candidates[i]!;
      if (!known.has(hex(c.stream)) || !this.subs.has(hex(c.stream))) this.follow(c, null);
    }
    if (at < 0 && this.current) this.follow(this.current, null);
  }

  private follow(at: NamedStream, saved: SavedStream<S> | null): void {
    const key = hex(at.stream);
    if (this.subs.has(key) || this.closed) return;
    const entry = { at, synced: false, exists: false, sub: null as unknown as StreamSubscription<S> };
    this.subs.set(key, entry);
    entry.sub = this.client.subscribe(
      at.stream,
      this.fold,
      {
        onState: (state, change) => {
          entry.exists = true;
          const i = this.candidates.findIndex((c) => hex(c.stream) === key);
          if (i < 0) return;
          if (i < this.shown) this.promote(i);
          if (i === this.shown) this.events.onState?.(at, state, change);
        },
        onSynced: (exists) => {
          entry.synced = true;
          entry.exists ||= exists;
          this.settle();
        },
      },
      saved,
    );
  }

  /** A newer name has something: show it, and let go of the older ones. */
  private promote(i: number): void {
    this.shown = i;
    for (const c of this.candidates.slice(i + 1)) {
      const key = hex(c.stream);
      this.subs.get(key)?.sub.close();
      this.subs.delete(key);
    }
    this.events.onSwitch?.(this.candidates[i]!);
  }

  /** The shown name has nothing and nothing was saved for it: look one epoch further back. Say when everything looked at has answered. */
  private settle(): void {
    if (this.closed) return;
    const shown = this.current;
    const entry = shown ? this.subs.get(hex(shown.stream)) : undefined;
    if (entry?.synced && !entry.exists && !entry.sub.hasState && this.shown + 1 < this.candidates.length) {
      this.shown += 1;
      this.events.onSwitch?.(this.candidates[this.shown]!);
      this.follow(this.candidates[this.shown]!, null);
      return;
    }
    const all = [...this.subs.values()];
    if (all.length && all.every((e) => e.synced)) this.events.onSynced?.(all.some((e) => e.exists));
  }

  close(): void {
    this.closed = true;
    for (const { sub } of this.subs.values()) sub.close();
    this.subs.clear();
  }
}
