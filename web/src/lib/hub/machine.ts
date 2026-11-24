/*
 * One machine, reached over an end-to-end channel, as the `PipeOpener` the
 * tunnel transport runs on (`lib/transport/tunnel.ts`).
 *
 * The channel goes through a relay when one can carry it (relay-and-plans
 * §2.6): one is listed, the machine is (or, ticketed, comes) on it, and its
 * agent speaks relays at all. Otherwise it goes on the hub's own connection —
 * while the hub carries channels (`RH_HUB_CHANNEL_RELAY`, on by default).
 * Switched off, the hub answers the `OPEN` with 4426; the link remembers that
 * until it reconnects, so the channel is blocked (`blocked`: no relay
 * available, or the agent is too old for relays) without asking the hub
 * again, and opens through a relay as soon as one can carry it. Where the
 * machine is comes from what the hub says on the connection signed in now:
 * after a reconnect or a pause the channel waits for that, and "blocked" is
 * only ever said on it (see `carrier`), never on what the page heard before —
 * otherwise the try fails as {@link ChannelPending}, and the bar says the
 * page cannot reach the device yet and when it tries again. An open
 * channel is never moved between the hub and a relay (only a draining relay
 * or "reconnect now" moves it): the next one is placed afresh.
 *
 * The channel is opened on demand — when the first pipe is — and again after
 * it is lost (the hub or the relay restarted, the network changed, the agent
 * went away):
 *
 * 1. vouch for the chain head this device sees (`ATTEST`, on the hub; agents
 *    open no channel without a recent attestation of their head);
 * 2. `OPEN` a fresh `ch` to the machine and send the Noise IK first message,
 *    whose payload is this device's chain view — never a request;
 * 3. read the agent's reply: `accepted` opens the channel; `stale` means the
 *    attestation had not reached it yet (attest again and retry); a reply
 *    asking for chain entries means it is behind (sync and retry);
 * 4. run it on a `ChannelDriver`, each pipe one sub-stream framed with
 *    `lib/e2e/bridge.ts`.
 *
 * The hello says `web/1 paths/1 streams/2` (`streams/2`: this page reads
 * streams named per key epoch, so the agent may name them so — it remembers
 * that a device said it). An agent whose reply lists `paths/1` gets a
 * channel whose frames are sequenced (`lib/e2e/path.ts`), the hub path probed
 * for its round trip (`paths`, for the indicator); one that also lists
 * `direct/1` is asked for a direct path next to the hub
 * (`lib/transport/p2p.ts`) when this channel was made with `direct` and the
 * device is not set to hub only. An older agent gets exactly what it got
 * before.
 *
 * A machine that renews its identity key (`docs/host-identity-rotation.md`
 * §3.5, §5.1): a channel is only ever opened to the DH key the chain gives the
 * machine now ({@link MachineChannel.responderKey} says which), and one the
 * chain has since replaced is closed — direct paths with it — the moment the
 * account hears; the next pipe opens a channel to the new key. A handshake
 * that fails other than by the agent's refusal (the agent could not open the
 * hello, or its reply does not open: it may hold a key this device has not
 * heard of yet) syncs the chain and is tried once more.
 */
import {
  type Binding,
  type Bytes,
  bridgeMessage,
  CLIENT_FEATURE,
  ChannelDriver,
  type ChannelStream,
  copy,
  decodeResponseHead,
  DEFAULT_REKEY,
  DIRECT_FEATURE,
  encodeRequestHead,
  equal,
  finishHandshake,
  type Frame,
  fromUtf8,
  HandshakeError,
  MessageReader,
  type PathStats,
  serverFeatures,
  startDirectHandshake,
  startHandshake,
  STREAMS_FEATURE,
  utf8,
} from "@/lib/e2e";
import { DIRECT_TIMEOUT_MS, RELAY_SILENT_MS, relayPathGone, UNACKED_TIMEOUT_MS } from "@/lib/e2e/path";
import type { PathEvent } from "@/lib/e2e/channel";
import { type ConnFields, connLog, deltaOf, errorFields, maskId } from "@/lib/connection-log";
import { type DirectEnv, DirectHistory, DirectPaths } from "@/lib/transport/p2p";
import type { Pipe, PipeKind, PipeMessage, PipeMeta, PipeOpener } from "@/lib/transport/tunnel";
import { HUB_NO_CHANNELS, LINK_LOST } from "./link";
import type { CloudAccount } from "./account";
import type { HubLink } from "./link";
import { type RelayChoice, type RelayEntry, type RelayPool, RelayRefused } from "./relays";
import type { RelayLink } from "./relay-link";

export type MachineStatus = "idle" | "connecting" | "open" | "offline" | "error";

/**
 * Where a channel being opened is ({@link MachineChannel.step}), one per
 * thing it waits on: `presence` — for the hub to say where the machine is,
 * after the link (re)connected or the page came back ({@link PRESENCE_WAIT_MS});
 * `relay` — for a relay: the list, a ticket, the machine signing in there, the
 * connection to it (`city` once one is chosen); `handshake` — for the agent's
 * answer to the hello, on whichever carrier it went.
 */
export type ConnectStep = { kind: "presence" } | { kind: "relay"; city: string | null } | { kind: "handshake" };

/**
 * Why the device would not take a channel, when that is how the last try
 * ended: `notAccepted` — the hub or a relay refused the `OPEN` because the
 * device does not accept control from this one (its "who can control it", or
 * this is a limited device); `unconfirmed` — its agent kept asking for another
 * device's word on its chain head (`stale`) until the tries ran out;
 * `unsaved` — its agent could not save that this device connects
 * ({@link AGENT_UNSAVED}).
 */
export type ChannelRefusal = { kind: "notAccepted"; by: "hub" | "relay"; why: string | null } | { kind: "unconfirmed" } | { kind: "unsaved" };

/**
 * The agent's close for a handshake it took but could not save (`AGENT_UNSAVED`
 * in `crates/droppen-agent/src/link/mod.rs`): a device not on its record of who
 * connects is let in only once it is, and the computer's disk would not
 * take it. Retrying does not help until that is fixed there.
 */
export const AGENT_UNSAVED = 4507;
/**
 * A pipe the channel could not open a stream for: its request head does not
 * fit (over 64 KiB to write, over the channel's `MAX_META` to send) or every
 * stream of the channel is in use. Refused here, never sent, and said in the
 * connection log (`pipe.refused`). Not `CHANNEL_GONE` (4502): the channel
 * itself is fine, and nothing reconnects for it.
 */
export const PIPE_REFUSED = 4499;

/**
 * The agent's close for a hello it could not take (`refuse(.., 4401, "handshake
 * refused")` in `crates/droppen-agent/src/link/mod.rs`) — among other reasons, one
 * sealed to a key it does not hold: it renewed its identity key and this
 * device has not heard yet.
 */
export const AGENT_HANDSHAKE_REFUSED = 4401;

/** Why a channel whose hub path is gone was closed ({@link watchRelayPath}). */
export const RELAY_PATH_GONE = 4000;

/**
 * The page's watch on a channel's carrier path, as its driver's `onpaths`:
 * once the path is gone (`relayPathGone`) — not merely slow — the channel is
 * closed ({@link RELAY_PATH_GONE}) and the carrier (`carrier`, a relay link)
 * woken with its short deadline, once: a dead relay link must be replaced
 * before the next channel tries to use it. Rather than stall until the relay
 * link's own 60 s silence, a dead path is replaced within 15 s of the last
 * thing it brought. A probe answered late while data keeps coming is a slow
 * link, not a dead one — its answer waits behind what the channel's window
 * let the agent send first — and replacing that channel only had the page
 * ask for the same data again, over and over. Nor is the channel closed while
 * a direct path carries it ({@link relayGone}).
 */
export function watchRelayPath(driver: ChannelDriver, carrier: { wake(): void } | null, onGone?: (stats: PathStats) => void): (stats: PathStats) => void {
  let fired = false;
  return (stats) => {
    if (fired || driver.isClosed || !relayGone(driver, stats)) return;
    fired = true;
    // Told first (the connection log), and never in the way of what follows.
    try {
      onGone?.(stats);
    } catch {
      // the close goes on all the same
    }
    carrier?.wake();
    driver.close(RELAY_PATH_GONE, "the relay path stopped answering");
  };
}

/**
 * The relay rule (`relayPathGone`) for `driver`'s paths as `stats` gives them:
 * while a direct path is attached, with how long since anything came in on
 * the channel either way (`lastRxAgoMs`) — a relay path whose probes go
 * unanswered behind a direct path's traffic is not gone while that path
 * carries, and the channel is not closed under it.
 */
export function relayGone(driver: ChannelDriver, stats: PathStats): boolean {
  return relayPathGone(stats.relayProbeAgeMs, stats.relayQuietMs, stats.direct ? channelQuietMs(driver) : null);
}

/** How long since anything came in on `driver`'s channel, over either path (ms); never anything: as long as it can be. */
function channelQuietMs(driver: ChannelDriver): number {
  return driver.diagnostics().lastRxAgoMs ?? Number.POSITIVE_INFINITY;
}

/** A relay's close for a refused `OPEN`: not a machine this ticket names (`droppen-relay-core/src/route.rs`). */
const RELAY_FORBIDDEN = 4403;
/** The hub's closes for a refused `OPEN` (`droppen-relay-core/src/route.rs`, as the hub runs it). */
const HUB_OFFLINE = 4404;
const HUB_FORBIDDEN = 4403;
const ATTEMPTS = 4;
const REPLY_TIMEOUT_MS = 15_000;
/** What this page's hello says it speaks (space-separated tokens). */
export const CLIENT_INFO = `web/1 ${CLIENT_FEATURE} ${STREAMS_FEATURE}`;

export interface MachineChannelOptions {
  /** Try a direct path next to the hub (the page's own channel; not a short-lived one). */
  direct?: boolean;
  /** WebRTC and the page, for tests. */
  directEnv?: Partial<DirectEnv>;
  /** The relays to open the channel through (none: the hub, as before relays). */
  relays?: RelayPool;
  /**
   * The one channel an isolated host may be reached on (docs/host-identity-rotation.md
   * §3.8): opened for a renewal confirmed on the device, for its `/rh/rotate`
   * call alone — no key goes on it, nothing else is asked, and it is closed
   * after. The hub routes it only while that host has an open
   * `assurance = 1` attempt. Every other channel to an isolated host is
   * refused ({@link ChannelIsolated}).
   */
  isolatedRenewal?: boolean;
}

/**
 * The relay or the hub would not carry the channel, with the reason it gave,
 * if any (its own words, shown as they are: `errorText` puts them in the
 * reader's sentence).
 */
export class ChannelRefused extends Error {
  constructor(
    readonly by: "relay" | "hub",
    readonly why: string | null,
  ) {
    super(`the ${by} refused the channel${why ? `: ${why}` : ""}`);
    this.name = "ChannelRefused";
  }
}

/**
 * Why no channel can open now: no relay can carry it, and the hub carries
 * none (the bar says so). Said only on what holds now — what the hub said of
 * the machine on the connection signed in now, the relay list fetched again
 * if it is old, relays given up on after failures tried after all — never on
 * what the page heard before a pause ({@link ChannelPending}).
 */
export class ChannelBlocked extends Error {
  constructor(readonly kind: "noRelay" | "agentTooOld") {
    super(kind === "noRelay" ? "no relay available" : "the device's agent is too old to be reached");
    this.name = "ChannelBlocked";
  }
}

/**
 * No channel yet, and nothing to say it cannot open: the hub has not said
 * where the machine is since the link (re)connected or the page came back
 * ({@link PRESENCE_WAIT_MS}). Not {@link ChannelBlocked}: the bar says when
 * the page tries again, and the next try (the hub's word on the machine starts one)
 * places the channel on what it says.
 */
export class ChannelPending extends Error {
  constructor() {
    super("waiting for the hub to say where the device is");
    this.name = "ChannelPending";
  }
}

/**
 * The machine is isolated (docs/host-identity-rotation.md §3.8): a key it
 * renewed away from signed in again, and nothing since confirmed its
 * identity on the device itself — its identity may be someone else's. No
 * channel opens to it until it is renewed with the code its screen shows.
 */
export class ChannelIsolated extends Error {
  constructor(readonly machine: string) {
    super(`${machine} is isolated: renew its identity key on the device first`);
    this.name = "ChannelIsolated";
  }
}

/** What carries a channel's frames: a relay connection, or the hub's. */
interface Carrier {
  reserve(route: (frame: Frame) => void): number;
  release(ch: number): void;
  send(frame: Frame): boolean;
}

/** Which way the open channel goes, for the path indicator. */
export type ChannelRoute =
  | {
      kind: "relay";
      relay: RelayEntry;
      mode: "auto" | "manual";
      /** A relay outside the account's set, while none of the set works. */
      emergency?: boolean;
    }
  /** The hub's own connection: no relay could carry it. */
  | { kind: "hub" };

/** A route as the connection log says it: `relay:nrt-1`, or `hub`. */
export function routeName(route: ChannelRoute): string {
  return route.kind === "relay" ? `relay:${route.relay.id}` : "hub";
}

/** A carrier for a new channel, and the relay connection when it is one. */
interface Carried {
  carrier: Carrier;
  route: ChannelRoute;
  link: RelayLink | null;
}

/** Where a try to open a channel is: `connect` says it as {@link MachineChannel.step}; a move says nothing. */
type StepReport = (step: ConnectStep) => void;
const quiet: StepReport = () => undefined;

/** How long a new relay gets to have the machine signed in on it. */
const ATTACH_WAIT_MS = 1500;
/**
 * …and when there is nowhere else to go — the machine is on no other relay
 * this channel may use and the hub carries none: its set just moved (the
 * relay it was on drains or failed, relay-and-plans §2.7), or it was just
 * added. The Hub's new relay_set, the agent's ticket and its sign-in there
 * take a few round trips.
 */
const ATTACH_WAIT_ALONE_MS = 8000;
/** A move off a draining relay that found no way yet tries again this often, while the relay lasts. */
const MOVE_RETRY_MS = 2000;
/** …and for at most this long (a relay drains for 30 s by default). */
const MOVE_RETRY_MAX_MS = 60_000;
/** How long the old channel lives on after a move, for what is in flight on it. */
const MOVE_GRACE_MS = 1500;
/** The relay's close for a machine not signed in there. */
const RELAY_OFFLINE = 4404;
/**
 * How long a channel waits for the hub to say where the machine is, after the
 * link (re)connected or the page came back (the hub says it right after its
 * `welcome`): what the page heard before is not where the machine is now.
 */
export const PRESENCE_WAIT_MS = 5000;
/** A relay list older than this is fetched again before saying no relay can carry a channel. */
const DIRECTORY_FRESH_MS = 30_000;

/** A channel opened, not yet the page's. */
interface OpenedChannel {
  driver: ChannelDriver;
  direct: DirectPaths | null;
  route: ChannelRoute;
  off: () => void;
  /** The machine's DH key the handshake went to. */
  responder: Bytes;
  /** The channel as the connection log names it (`ch:<relay or hub>/<ch>`), and the try of its opening that made it. */
  logOn: string;
  attempt: number;
}

/**
 * Why this page closed a channel itself, for the connection log's
 * `channel.close` (`rule`): set just before the close. A channel the peer or
 * the carrier ended has none (its close code says).
 */
const closeRules = new WeakMap<ChannelDriver, "relay_silent" | "moved" | "key" | "done">();

/** The numbers the relay rule closed a channel on (`relayPathGone`), for its `channel.close`. */
const relayGoneStats = new WeakMap<ChannelDriver, { relayProbeAgeMs: number | null; relayQuietMs: number | null }>();

/** What the connection log samples of a running channel once a second (`pipes`: those open on it). */
function channelSampler(driver: ChannelDriver, pipes: () => number) {
  const deltas = { relayTx: deltaOf(), relayRx: deltaOf(), directTx: deltaOf(), directRx: deltaOf() };
  return () => {
    if (driver.isClosed) return null;
    const d = driver.diagnostics();
    const moved = {
      tx: deltas.relayTx(d.relayTx),
      rx: deltas.relayRx(d.relayRx),
      dtx: deltas.directTx(d.directTx),
      drx: deltas.directRx(d.directRx),
    };
    const paths = driver.paths;
    const open = pipes();
    const waiting = d.heldTx + d.queued + (d.dcBuffered ?? 0) + (d.unackedBytes ?? 0) + (d.heldRxBytes ?? 0);
    return {
      fields: {
        ...moved,
        // Bytes sent and not yet credited back by the carrier (`INITIAL_CREDIT` less what is left), and a sealed frame waiting for credit.
        inflight: d.inflight,
        heldTx: d.heldTx,
        queued: d.queued,
        dcBuf: d.dcBuffered,
        unacked: d.unackedBytes,
        unackedN: d.unackedFrames,
        heldRx: d.heldRxBytes,
        probes: d.probesOut,
        oldestProbeMs: d.oldestProbeMs,
        relayRttMs: paths?.relayRttMs ?? null,
        // What the relay rule judges by: the oldest hub probe's age (sent or waiting for credit), and how long nothing has come over the hub.
        relayProbeAgeMs: paths?.relayProbeAgeMs ?? null,
        relayQuietMs: paths?.relayQuietMs ?? null,
        directRttMs: paths?.direct ? paths.directRttMs : null,
        path: paths?.direct ? "direct" : "carrier",
        streams: d.streams,
        pipes: open,
      },
      // Traffic decides it: bytes that moved, or bytes waiting to. An open socket that says nothing (a feed) is not busy.
      busy: moved.tx + moved.rx + moved.dtx + moved.drx > 0 || waiting > 0,
    };
  };
}

/** Frames for one `ch`, awaited one at a time until a driver takes over. */
class FrameQueue {
  private readonly frames: Frame[] = [];
  private waiter: ((f: Frame | null) => void) | null = null;

  push(frame: Frame): void {
    if (this.waiter) {
      const w = this.waiter;
      this.waiter = null;
      w(frame);
    } else this.frames.push(frame);
  }

  next(timeoutMs: number): Promise<Frame | null> {
    const ready = this.frames.shift();
    if (ready) return Promise.resolve(ready);
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        this.waiter = null;
        resolve(null);
      }, timeoutMs);
      this.waiter = (f) => {
        clearTimeout(timer);
        resolve(f);
      };
    });
  }

  drain(): Frame[] {
    return this.frames.splice(0);
  }
}

/** A pipe that may be opened before its channel is up, and attaches when it is. */
class BridgePipe implements Pipe {
  onmessage: ((message: PipeMessage) => void) | null = null;
  onclose: ((code: number) => void) | null = null;
  private stream: ChannelStream | null = null;
  private readonly pending: PipeMessage[] = [];
  private closed = false;
  private readonly reader = new MessageReader();

  constructor(
    readonly kind: PipeKind,
    readonly meta: PipeMeta,
    /** Told once when the pipe is over, either way (the channel counts its open pipes for the connection log). */
    private readonly done: () => void = () => undefined,
  ) {}

  attach(stream: ChannelStream): void {
    if (this.closed) {
      stream.close(0);
      return;
    }
    this.stream = stream;
    stream.ondata = (data) => {
      this.reader.push(data);
      try {
        for (let m = this.reader.next(); m; m = this.reader.next()) {
          if (m.kind === "head") {
            const head = decodeResponseHead(m.payload);
            this.onmessage?.({ kind: "head", status: head.status, headers: head.headers });
          } else if (m.kind === "text") this.onmessage?.({ kind: "text", data: fromUtf8(m.payload) });
          else if (m.kind === "binary") this.onmessage?.({ kind: "binary", data: m.payload });
          else this.onmessage?.({ kind: "end" });
        }
      } catch {
        stream.close(4400);
      }
    };
    stream.onclose = (code) => this.fail(code);
    for (const m of this.pending.splice(0)) this.send(m);
  }

  fail(code: number): void {
    if (this.closed) return;
    this.closed = true;
    this.done();
    this.onclose?.(code);
  }

  send(message: PipeMessage): void {
    if (this.closed) return;
    if (!this.stream) {
      this.pending.push(message);
      return;
    }
    switch (message.kind) {
      case "text":
        this.stream.send(bridgeMessage("text", utf8(message.data)));
        break;
      case "binary":
        this.stream.send(bridgeMessage("binary", message.data));
        break;
      case "end":
        this.stream.send(bridgeMessage("end"));
        break;
      case "head":
        break;
    }
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.done();
    this.stream?.close(0);
  }
}

export class MachineChannel implements PipeOpener {
  status: MachineStatus = "idle";
  error: string | null = null;
  /** How long the last handshake took, for display. */
  handshakeMs: number | null = null;
  /**
   * Why the agent last refused a handshake, if it did: `stale` (no recent
   * attestation of its head from another full device yet — after a chain
   * change, until one vouches for it) or `needEntries` (it is behind).
   */
  lastRefusal: "stale" | "needEntries" | null = null;
  /**
   * The refusal the last try ended in, if it ended in one (see
   * {@link ChannelRefusal}): unlike `error` it outlives the next try's
   * "connecting", so the page can say why it is not getting in. Cleared once a
   * channel opens.
   */
  refusal: ChannelRefusal | null = null;
  /** Whether the try now running ended in a refusal ({@link refuse}): a try that failed some other way does not keep an older one. */
  private refusedThisTry = false;
  /**
   * How the open channel reaches the machine — over a direct path or the hub,
   * and each path's latest round trip — when the agent sequences frames
   * (`paths/1`); `null` otherwise.
   */
  paths: PathStats | null = null;
  /** Which relay (or the hub) the open channel goes through. */
  route: ChannelRoute | null = null;
  /** Why the channel cannot open now, when that is the reason. */
  blocked: ChannelBlocked["kind"] | null = null;
  /**
   * What the channel being opened waits on now, while it is `connecting`
   * (see {@link ConnectStep}); `null` between those waits and otherwise. A
   * move of an open channel ({@link move}) does not say: the page is live.
   */
  step: ConnectStep | null = null;
  private driver: ChannelDriver | null = null;
  /**
   * Drivers whose `close` has reached them (the carrier went away, or the
   * agent ended the channel) but which are still working through what came
   * before it: nothing new goes on them.
   */
  private readonly ending = new WeakSet<ChannelDriver>();
  private carrierOff: (() => void) | null = null;
  private moving: Promise<boolean> | null = null;
  private direct: DirectPaths | null = null;
  /**
   * How direct paths to this machine have fared, across its channels: a
   * channel opened after the hub connection came back carries on with the
   * backoff (and the run of paths that died young) its predecessor left.
   */
  private readonly directHistory = new DirectHistory();
  private connecting: Promise<ChannelDriver> | null = null;
  /** The machine's DH key the installed channel's handshake went to. */
  private responder: Bytes | null = null;
  /** Listening to the account while a channel is installed (only then): a renewal of the machine's key closes it. */
  private offAccount: (() => void) | null = null;
  private readonly listeners = new Set<(status: MachineStatus) => void>();
  private readonly pathListeners = new Set<(paths: PathStats | null) => void>();
  private readonly openListeners = new Set<() => void>();
  private readonly stepListeners = new Set<(step: ConnectStep | null) => void>();
  /** The machine as the connection log names it: a hash of its id (`maskId`), never the id. */
  private readonly logDevice: string | null;
  /** Pipes open on this channel (requests, sockets), for the connection log's samples. */
  private pipesOpen = 0;
  /** When a channel to the machine last ended (`performance.now`), for the connection log's `sinceLastCloseMs`. */
  private lastCloseAt: number | null = null;

  constructor(
    readonly link: HubLink,
    readonly account: CloudAccount,
    readonly machine: string,
    private readonly options: MachineChannelOptions = {},
  ) {
    this.logDevice = maskId(machine);
    // Whatever names the machine in a reason or an error (`ChannelIsolated`, say) names its hash in the log.
    connLog.hide(machine);
  }

  /** Log `name` about this machine's channel (its masked id added). Never throws: logging has no say in the channel. */
  private log(name: Parameters<typeof connLog.emit>[0], fields: ConnFields = {}, level?: Parameters<typeof connLog.emit>[2]): void {
    try {
      connLog.emit(name, { device: this.logDevice, ...fields }, level);
    } catch {
      // nothing logged, nothing else changed
    }
  }

  /**
   * The carrier path is gone by the rule (`relayPathGone`: its oldest probe
   * out at least `RELAY_SILENT_MS`, nothing at all come over it for as long,
   * and no direct path carrying — {@link relayGone}): this page closes the
   * channel. Said with the numbers and the decision, from `watchRelayPath`'s
   * hook, before the close.
   */
  private logRelaySilent(on: string, driver: ChannelDriver, stats: PathStats): void {
    closeRules.set(driver, "relay_silent");
    relayGoneStats.set(driver, { relayProbeAgeMs: stats.relayProbeAgeMs, relayQuietMs: stats.relayQuietMs });
    try {
      const d = driver.diagnostics();
      this.log("timeout", {
        timer: "RELAY_SILENT_MS",
        rule: "relayPathGone",
        decision: "gone",
        limitMs: RELAY_SILENT_MS,
        relayProbeAgeMs: stats.relayProbeAgeMs,
        relayQuietMs: stats.relayQuietMs,
        elapsedMs: stats.relayQuietMs,
        on,
        // Gone with a direct path attached only once nothing has come in either way for as long.
        direct: stats.direct,
        lastRxAgoMs: d.lastRxAgoMs,
        inflight: d.inflight,
        queued: d.queued + d.heldTx,
      });
    } catch {
      // the close goes on all the same
    }
  }

  /**
   * The rule's other decision, for the log: a carrier path whose oldest probe
   * is `RELAY_SILENT_MS` old while things still arrive over it, or while a
   * direct path carries the channel (`direct`, `lastRxAgoMs`), is kept
   * (`path.slow`, `decision=kept`). Said once each time the path turns slow,
   * not on every change of its numbers; reads only.
   */
  private relaySlowLogger(on: string, driver: ChannelDriver): (stats: PathStats) => void {
    let said = false;
    return (stats) => {
      try {
        const old = stats.relayProbeAgeMs !== null && stats.relayProbeAgeMs >= RELAY_SILENT_MS;
        if (!old) {
          said = false;
          return;
        }
        if (said || relayGone(driver, stats)) return;
        said = true;
        this.log("path.slow", {
          on,
          rule: "relayPathGone",
          decision: "kept",
          limitMs: RELAY_SILENT_MS,
          relayProbeAgeMs: stats.relayProbeAgeMs,
          relayQuietMs: stats.relayQuietMs,
          relayRttMs: stats.relayRttMs,
          // Kept because the direct path carries (the hub path may be as quiet as it likes then), or because the hub path still delivers.
          direct: stats.direct,
          lastRxAgoMs: driver.diagnostics().lastRxAgoMs,
        });
      } catch {
        // nothing logged
      }
    };
  }

  /**
   * A channel ended: who ended it (`by`: this page, the peer, or the carrier
   * under it going away) and why (`rule`, when this page did), how long it
   * lived, what it moved, and what was still waiting on it then. Called once
   * the channel is cleaned up, and never throws: logging cannot skip cleanup.
   */
  private logClose(on: string, driver: ChannelDriver, end: { code: number; reason: string; local: boolean; openedAt: number; route: ChannelRoute }): void {
    try {
      this.logCloseNow(on, driver, end);
    } catch {
      // nothing logged
    }
  }

  private logCloseNow(on: string, driver: ChannelDriver, end: { code: number; reason: string; local: boolean; openedAt: number; route: ChannelRoute }): void {
    const now = performance.now();
    this.lastCloseAt = now;
    const d = driver.diagnostics();
    const rule = closeRules.get(driver) ?? (end.local ? "error" : end.code === LINK_LOST ? "link_lost" : "peer");
    // The relay rule's numbers as it judged them (a close by it), else the path's as they stand.
    const judged = relayGoneStats.get(driver);
    this.log("channel.close", {
      on,
      via: routeName(end.route),
      code: end.code,
      reason: end.reason,
      by: end.local ? "local" : end.code === LINK_LOST ? "link" : "peer",
      rule,
      uptimeMs: now - end.openedAt,
      rxBytes: d.relayRx + d.directRx,
      txBytes: d.relayTx + d.directTx,
      lastRxAgoMs: d.lastRxAgoMs,
      oldestProbeAgeMs: d.oldestProbeMs,
      probesOut: d.probesOut,
      relayRttMs: driver.paths?.relayRttMs ?? null,
      relayProbeAgeMs: judged ? judged.relayProbeAgeMs : (driver.paths?.relayProbeAgeMs ?? null),
      relayQuietMs: judged ? judged.relayQuietMs : (driver.paths?.relayQuietMs ?? null),
      decision: rule === "relay_silent" ? "gone" : undefined,
      sendCreditLeft: d.credit,
      inflight: d.inflight,
      recvUnacked: d.recvUnacked,
      queued: d.queued + d.heldTx,
      unacked: d.unackedBytes,
      openStreams: d.streams,
      pipes: this.pipesOpen,
    }, rule === "done" || rule === "moved" ? "info" : "warn");
  }

  /** What the channel's paths said (`PathEvent`), in the connection log: probes and their answers kept apart from a round trip raised to an unanswered probe's age. */
  private logPathEvent(on: string, driver: ChannelDriver, event: PathEvent): void {
    try {
      this.logPathEventNow(on, driver, event);
    } catch {
      // nothing logged
    }
  }

  private logPathEventNow(on: string, driver: ChannelDriver, event: PathEvent): void {
    switch (event.type) {
      case "probe":
        connLog.emit("probe.sent", { on, path: event.via, nonce: event.nonce });
        return;
      case "rtt":
        connLog.emit("probe.answer", { on, path: event.via, nonce: event.nonce ?? null, rttMs: event.ms });
        return;
      case "late":
        connLog.emit("probe.late", { on, path: event.via, nonce: event.nonce, ageMs: event.ageMs, quietMs: event.quietMs, unsent: event.unsent });
        return;
      case "paused":
        this.log("page.pause", { on, gapMs: event.gapMs, forgotten: event.forgotten });
        return;
      case "directUp":
        this.log("path.switch", { on, to: "direct" });
        return;
      case "directDown": {
        const stats = driver.paths;
        if (event.timer) this.log("timeout", { timer: event.timer, limitMs: event.timer === "DIRECT_TIMEOUT_MS" ? DIRECT_TIMEOUT_MS : UNACKED_TIMEOUT_MS, elapsedMs: event.elapsedMs, on: `${on}.direct` });
        this.log("path.switch", {
          on,
          to: "carrier",
          cause: event.reason,
          remote: event.remote,
          timer: event.timer,
          unackedBytes: event.unackedBytes,
          directRttMs: stats?.directRttMs ?? null,
          resent: stats?.resent ?? null,
        }, event.reason === "closed" ? "info" : "warn");
        return;
      }
    }
  }

  onStatus(fn: (status: MachineStatus) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  /**
   * Hear each time a channel is installed — the first, and every one after
   * a loss or a move: what was in flight on the one before may be asked
   * about now (`lib/transport` `onReconnect`).
   */
  onOpen(fn: () => void): () => void {
    this.openListeners.add(fn);
    return () => this.openListeners.delete(fn);
  }

  onPaths(fn: (paths: PathStats | null) => void): () => void {
    this.pathListeners.add(fn);
    return () => this.pathListeners.delete(fn);
  }

  /** Hear {@link step} change while a channel is being opened. */
  onStep(fn: (step: ConnectStep | null) => void): () => void {
    this.stepListeners.add(fn);
    return () => this.stepListeners.delete(fn);
  }

  private setStep(step: ConnectStep | null): void {
    if (this.step === step) return;
    this.step = step;
    if (step) this.log("channel.step", { step: step.kind, city: step.kind === "relay" ? step.city : undefined });
    for (const fn of [...this.stepListeners]) fn(step);
  }

  /** The open channel's direct-path manager, if it has one (diagnostics). */
  get directPaths(): DirectPaths | null {
    return this.direct;
  }

  /**
   * The machine's static DH key the open channel was handshaken to — what the
   * chain gave it then, and still gives it: a channel whose key the chain has
   * replaced since is closed at once. `null` while no channel is open.
   */
  get responderKey(): Uint8Array | null {
    return this.live() && this.responder ? copy(this.responder) : null;
  }

  private setPaths(paths: PathStats | null): void {
    if (this.paths === paths) return;
    this.paths = paths;
    for (const fn of this.pathListeners) fn(paths);
  }

  private setRoute(route: ChannelRoute | null): void {
    this.route = route;
    for (const fn of this.pathListeners) fn(this.paths);
  }

  private setStatus(status: MachineStatus, error: string | null = null): void {
    if (status !== this.status || error !== this.error) this.log("channel.status", { status, error: error ?? undefined }, status === "error" ? "warn" : undefined);
    this.status = status;
    this.error = error;
    for (const fn of this.listeners) fn(status);
  }

  open(kind: PipeKind, meta: PipeMeta): Pipe {
    // A head that cannot be written (a path over 64 KiB) throws here, before anything is counted.
    let head: Uint8Array;
    try {
      head = encodeRequestHead({ method: meta.method ?? "GET", path: meta.path, headers: (meta.headers ?? []).slice(0, 48) });
    } catch (error) {
      this.log("pipe.refused", { kind, ...errorFields(error) }, "warn");
      throw error;
    }
    const pipe = new BridgePipe(kind, meta, () => {
      this.pipesOpen = Math.max(0, this.pipesOpen - 1);
    });
    this.pipesOpen += 1;
    this.ensure().then(
      (driver) => {
        // The channel refusing the stream (a head over `MAX_META`, every stream in use) fails the pipe: never one left waiting.
        let stream: ReturnType<ChannelDriver["open"]>;
        try {
          stream = driver.open(kind, head);
        } catch (error) {
          this.log("pipe.refused", { kind, ...errorFields(error) }, "warn");
          queueMicrotask(() => pipe.fail(PIPE_REFUSED));
          return;
        }
        pipe.attach(stream);
      },
      () => queueMicrotask(() => pipe.fail(4502)),
    );
    return pipe;
  }

  /** The running channel, opening one if there is none. */
  ensure(): Promise<ChannelDriver> {
    // Never reused to a host isolated meanwhile — another tab may have kept the dispute — nor to a key renewed away.
    this.checkKey();
    if (this.account.isolated?.(this.machine) && !this.options.isolatedRenewal) return Promise.reject(new ChannelIsolated(this.machine));
    const driver = this.live();
    if (driver) return Promise.resolve(driver);
    this.connecting ??= this.connect().finally(() => {
      this.connecting = null;
    });
    return this.connecting;
  }

  /**
   * The channel, while new pipes can go on it: not once its close has been
   * handed to it. The driver reads its frames in turn (WebCrypto is
   * asynchronous), so a `close` — the relay or the hub connection went away
   * ({@link LINK_LOST}) — reaches it only after what it was still busy with;
   * a pipe opened meanwhile would be sent on a connection that is gone and
   * fail with the channel instead of opening a new one.
   */
  private live(): ChannelDriver | null {
    const driver = this.driver;
    return driver && !driver.isClosed && !this.ending.has(driver) ? driver : null;
  }

  /** Drop the channel (a machine let go of, a sign-out). Whoever watches its status hears that it is no longer open. */
  close(): void {
    this.direct?.close();
    this.direct = null;
    if (this.driver) closeRules.set(this.driver, "done");
    this.driver?.close(1000, "done");
    this.driver = null;
    this.responder = null;
    this.unwatchKey();
    this.carrierOff?.();
    this.carrierOff = null;
    this.setPaths(null);
    this.setRoute(null);
    // A device let go of is asked afresh next time: a refusal from before must not keep it off the pickers meanwhile.
    const refused = this.refusal !== null || this.lastRefusal !== null;
    this.refusal = null;
    this.lastRefusal = null;
    if (this.status === "open" || (refused && this.status === "error")) this.setStatus("idle");
  }

  /**
   * Open a new channel (excluding `exclude`d relays) and move onto it before
   * closing the old one: what a `goaway` and the picker's "reconnect now" do.
   * Whether it moved.
   */
  move(exclude: ReadonlySet<string> = new Set()): Promise<boolean> {
    if (!this.live()) return this.ensure().then(() => true);
    this.moving ??= (async () => {
      try {
        const old = this.driver;
        const oldDirect = this.direct;
        const oldOff = this.carrierOff;
        const from = this.route;
        this.log("path.switch", { why: "move", from: from ? routeName(from) : null, avoid: [...exclude].join(",") || undefined });
        const next = await this.openChannel(exclude);
        this.log("path.switch", { why: "moved", from: from ? routeName(from) : null, to: routeName(next.route) });
        this.install(next);
        this.checkKey();
        // The old channel lives on a moment for what is in flight on it, then goes.
        setTimeout(() => {
          oldDirect?.close();
          if (old) closeRules.set(old, "moved");
          old?.close(1000, "moved");
          oldOff?.();
        }, MOVE_GRACE_MS);
        return true;
      } catch (error) {
        console.warn("cloud: moving the channel:", error);
        this.log("path.switch", { why: "move failed", ...errorFields(error) }, "warn");
        return false;
      } finally {
        this.moving = null;
      }
    })();
    return this.moving;
  }

  /**
   * The relay under `driver` is draining (`goaway`): move off it before it
   * closes, without the user doing anything. The machine moves too (the Hub
   * replaces the relay in the account's set at once), so a first try that
   * finds it nowhere yet is tried again while the relay lasts.
   */
  private async moveOff(driver: ChannelDriver, relay: string, retryMs: number): Promise<void> {
    const until = Date.now() + Math.min(Math.max(retryMs, MOVE_RETRY_MS), MOVE_RETRY_MAX_MS);
    while (this.driver === driver) {
      if (await this.move(new Set([relay]))) return;
      if (Date.now() + MOVE_RETRY_MS >= until || this.driver !== driver) return;
      await new Promise((resolve) => setTimeout(resolve, MOVE_RETRY_MS));
    }
  }

  /**
   * The carrier for a new channel: a relay (chosen, ticketed, the machine on
   * it) when one can carry it, else the hub — unless the hub said on this
   * connection that it carries none: then the channel is blocked. A relay
   * that cannot be reached goes into `skip` (this try dials it no more).
   *
   * It is placed on what the hub says of the machine now: after the link
   * (re)connected, or the page came back from a pause, it first waits for
   * that ({@link PRESENCE_WAIT_MS}) — what the page heard before (on no relay,
   * say, while the machine renewed its key) is not where it is now. And with
   * nowhere to go while the hub carries none, it looks once more (`again`) —
   * the relay list fetched again if it is old, relays given up on after
   * failures (while the phone slept or was offline, say) tried after all —
   * before saying no relay can carry it; and it says that only on presence
   * heard now ({@link ChannelPending} otherwise).
   *
   * What it waits on is said to `report` as it goes (the `presence` and
   * `relay` steps).
   */
  private async carrier(skip: Set<string>, again = false, waited = new Set<string>(), report: StepReport = quiet): Promise<Carried> {
    const pool = this.options.relays;
    const presence = () => this.link.presence.get(this.machine);
    if (pool && !this.link.presenceFresh) {
      report({ kind: "presence" });
      const waitedAt = performance.now();
      if (!(await this.link.waitPresence(PRESENCE_WAIT_MS))) {
        this.log("timeout", { timer: "PRESENCE_WAIT_MS", limitMs: PRESENCE_WAIT_MS, elapsedMs: performance.now() - waitedAt, on: "hub", link: this.link.state }, "warn");
      }
    }
    const hub = async (): Promise<Carried> => {
      if (this.link.carriesChannels) return { carrier: this.link, route: { kind: "hub" }, link: null };
      if (pool && !again && (pool.ageMs() > DIRECTORY_FRESH_MS || pool.benched().length > 0)) return this.carrier(skip, true, waited, report);
      if (pool && !this.link.presenceFresh) throw new ChannelPending();
      throw this.blockedNow();
    };
    // An agent from before relays signs in on none.
    if (presence()?.relays === false) return hub();
    if (pool) report({ kind: "relay", city: null });
    const dir = pool ? await pool.ready(again ? DIRECTORY_FRESH_MS : undefined) : null;
    if (!pool || !dir || dir.relays.length === 0) return hub();
    const via = () => presence()?.via.map((v) => v.relay) ?? [];
    const benched = again;
    let choice = pool.choose(this.machine, skip, new Set(via()), { benched });
    this.logChoice(pool, choice, via(), skip, again ? "again" : "first");
    if (!choice) return hub();
    report({ kind: "relay", city: choice.relay.city });
    if (!via().includes(choice.relay.id)) {
      // A ticket puts it in the account's set; the hub tells the agent, which signs in there.
      let ticketed = true;
      try {
        await pool.ticket(choice.relay.id);
      } catch (error) {
        ticketed = false;
        if (!(error instanceof RelayRefused)) pool.noteFailure(choice.relay.id, error instanceof Error ? error.message : String(error));
      }
      // With nowhere else to go (no other relay the machine is on, no hub), it is worth waiting for.
      const chosen = choice.relay.id;
      const alone = !via().some((r) => r !== chosen && !skip.has(r)) && !this.link.carriesChannels;
      // (Not twice in one try: a second look does not wait again for a relay the first waited for.)
      const wait = waited.has(chosen) ? 0 : alone ? ATTACH_WAIT_ALONE_MS : ATTACH_WAIT_MS;
      waited.add(chosen);
      const waitedAt = performance.now();
      if (!ticketed || !(await this.waitVia(chosen, wait))) {
        if (ticketed && wait > 0) {
          this.log(
            "timeout",
            { timer: alone ? "ATTACH_WAIT_ALONE_MS" : "ATTACH_WAIT_MS", limitMs: wait, elapsedMs: performance.now() - waitedAt, on: `relay:${chosen}` },
            "warn",
          );
        }
        // A manual choice the machine is not on counts against it (three, and it gives way).
        if (choice.mode === "manual") pool.noteFailure(choice.relay.id, "the device is not on this relay");
        // The best relay the machine is already on.
        const on = new Set(via());
        const fallback = pool.choose(this.machine, new Set([...skip, ...dir.relays.filter((r) => !on.has(r.id)).map((r) => r.id)]), undefined, { benched });
        this.logChoice(pool, fallback, via(), skip, "fallback");
        if (!fallback) return hub();
        choice = fallback;
        report({ kind: "relay", city: choice.relay.city });
      }
    }
    let link: RelayLink;
    try {
      link = await pool.link(choice.relay);
    } catch {
      // It failed (counted: a manual choice gives way after three); this channel goes elsewhere.
      if (skip.has(choice.relay.id)) return hub();
      skip.add(choice.relay.id);
      return this.carrier(skip, again, waited, report);
    }
    return {
      carrier: link,
      link,
      route: { kind: "relay", relay: choice.relay, mode: choice.mode, emergency: choice.emergency || pool.emergency(choice.relay.id) },
    };
  }

  /**
   * The connection log's word on a relay choice (`pass`: the first look, the
   * second, or the fallback to a relay the machine is on): what was chosen,
   * and every listed relay with this device's round trip to it, the
   * machine's, and whether the machine is on it, it is skipped or benched.
   */
  private logChoice(pool: RelayPool, choice: RelayChoice | null, on: readonly string[], skip: ReadonlySet<string>, pass: string): void {
    try {
      this.logChoiceNow(pool, choice, on, skip, pass);
    } catch {
      // the choice stands, logged or not
    }
  }

  private logChoiceNow(pool: RelayPool, choice: RelayChoice | null, on: readonly string[], skip: ReadonlySet<string>, pass: string): void {
    const dir = pool.directory;
    const agent = dir?.agents[this.machine];
    const candidates = (dir?.relays ?? [])
      .map((r) => {
        const mine = pool.rtt(r.id);
        const theirs = agent?.[r.id];
        const marks = [on.includes(r.id) ? "on" : "", skip.has(r.id) ? "skip" : "", pool.usable(r.id) ? "" : "benched", r.state === "up" ? "" : r.state].filter(Boolean).join("+");
        return `${r.id}=${mine === null ? "-" : Math.round(mine)}/${theirs === undefined ? "-" : Math.round(theirs)}${marks ? `(${marks})` : ""}`;
      })
      .join(" ");
    this.log("relay.choose", {
      pass,
      relay: choice?.relay.id ?? null,
      mode: choice?.mode,
      emergency: choice?.emergency,
      score: choice?.score,
      deviceOn: on.join(",") || null,
      candidates: candidates || null,
    });
  }

  /** Why no channel can open while the hub carries none: the agent is too old for relays, or no relay can carry it. */
  private blockedNow(): ChannelBlocked {
    return new ChannelBlocked(this.link.presence.get(this.machine)?.relays === false ? "agentTooOld" : "noRelay");
  }

  /** Wait until the machine says it is on `relay` (at most `ms`). */
  private waitVia(relay: string, ms: number): Promise<boolean> {
    const on = () => (this.link.presence.get(this.machine)?.via ?? []).some((v) => v.relay === relay);
    if (on()) return Promise.resolve(true);
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        off();
        resolve(on());
      }, ms);
      const off = this.link.onPresence(() => {
        if (!on()) return;
        clearTimeout(timer);
        off();
        resolve(true);
      });
    });
  }

  private async connect(): Promise<ChannelDriver> {
    this.setStatus("connecting");
    try {
      await this.link.waitOpen();
      const opened = await this.openChannel(new Set(), (step) => this.setStep(step));
      this.setStep(null);
      this.blocked = null;
      this.install(opened);
      this.setStatus("open");
      // The chain may have moved on while it opened (the listener is only on now).
      this.checkKey();
      return opened.driver;
    } catch (error) {
      this.setStep(null);
      this.blocked = error instanceof ChannelBlocked ? error.kind : null;
      // A timeout or a lost link after an earlier refusal is not that refusal any more.
      if (!this.refusedThisTry) this.refusal = null;
      if (this.status !== "offline") this.setStatus("error", error instanceof Error ? error.message : String(error));
      throw error;
    }
  }

  /** Make `opened` the page's channel. */
  private install(opened: OpenedChannel): void {
    const { driver } = opened;
    const route = opened.route;
    this.log("channel.open", {
      on: opened.logOn,
      via: routeName(route),
      mode: route.kind === "relay" ? route.mode : undefined,
      emergency: route.kind === "relay" ? route.emergency === true : undefined,
      handshakeMs: this.handshakeMs,
      attempt: opened.attempt,
      sinceLastCloseMs: this.lastCloseAt === null ? null : performance.now() - this.lastCloseAt,
      paths: driver.hasPaths,
    });
    this.driver = driver;
    this.direct = opened.direct;
    this.carrierOff = opened.off;
    this.responder = opened.responder;
    this.watchKey();
    this.setPaths(driver.paths);
    this.setRoute(opened.route);
    if (opened.direct) opened.direct.start();
    // After this turn: a listener asking at once opens its pipe on this channel, not inside `install`.
    queueMicrotask(() => {
      if (this.driver !== driver) return;
      // Who asks again what was in flight on the channel before (the transport's `onReconnect`).
      this.log("reconnect.replay", { on: opened.logOn, why: "channel open", listeners: this.openListeners.size });
      for (const fn of [...this.openListeners]) fn();
    });
  }

  /** A channel to the machine — handshake, driver, direct path — not installed yet; what it waits on is said to `report`. */
  private async openChannel(exclude: ReadonlySet<string>, report: StepReport = quiet): Promise<OpenedChannel> {
    const skip = new Set(exclude);
    this.lastRefusal = null;
    this.refusedThisTry = false;
    // A handshake that failed other than by the agent's refusal has had the chain synced and been tried again.
    let resynced = false;
    for (let attempt = 0; attempt < ATTEMPTS; attempt++) {
      // A view that forked from what this browser verified opens nothing.
      this.account.assertNotForked();
      // An isolated host is not connected to at all (§3.8): not even a handshake goes to it — but for the one
      // `/rh/rotate` of a renewal confirmed on the device (`isolatedRenewal`).
      if (this.account.isolated?.(this.machine) && !this.options.isolatedRenewal) throw new ChannelIsolated(this.machine);
      const chain = this.account.chain;
      if (!chain) throw new Error("this device has no account chain");
      if (!chain.machine(this.machine)) await this.account.sync();
      // Only a device that manages the account vouches for the chain head; the hub refuses anyone else's (as `ChainFollower` does).
      if (this.account.isAdmin) this.link.attest(await this.account.attestation());
      const { carrier, route, link: relayLink } = await this.carrier(skip, false, new Set(), report);
      report({ kind: "handshake" });
      const relay = route.kind === "relay" ? route.relay.id : null;
      const queue = new FrameQueue();
      let routeTo = (f: Frame) => queue.push(f);
      const ch = carrier.reserve((f) => routeTo(f));
      // The channel as the connection log names it: its carrier and its `ch` there.
      const on = `ch:${relay ?? "hub"}/${ch}`;
      this.log("channel.try", { attempt: attempt + 1, via: routeName(route), on });
      const started = performance.now();
      carrier.send({ type: "open", ch, peer: this.machine });
      const { initiator, message } = await startHandshake(chain, this.account.noiseKeys, this.account.deviceId, this.machine, Date.now(), utf8(CLIENT_INFO));
      carrier.send({ type: "data", ch, payload: message });
      this.log("handshake.start", { on });
      const first = await queue.next(REPLY_TIMEOUT_MS);
      if (!first) {
        this.log("timeout", { timer: "REPLY_TIMEOUT_MS", limitMs: REPLY_TIMEOUT_MS, elapsedMs: performance.now() - started, on });
        carrier.release(ch);
        if (!relay) throw new Error("the device did not answer");
        this.options.relays?.noteFailure(relay, "the device did not answer");
        skip.add(relay);
        continue;
      }
      if (first.type === "close") {
        this.log("handshake.fail", { on, code: first.code, reason: first.reason, ms: performance.now() - started });
        carrier.release(ch);
        if (first.code === AGENT_UNSAVED) {
          this.refuse({ kind: "unsaved" });
          throw new Error("the device could not save its record of who connects to it");
        }
        if (first.code === AGENT_HANDSHAKE_REFUSED && !resynced) {
          // The agent could not open the hello: it may have renewed its key
          // before this device heard. The chain says, then one more try.
          resynced = true;
          await this.resync();
          continue;
        }
        if (relay) {
          if (first.code === RELAY_OFFLINE || first.code === LINK_LOST) {
            // Not on this relay (any more): another one.
            skip.add(relay);
            continue;
          }
          if (first.code === RELAY_FORBIDDEN) {
            this.refuse({ kind: "notAccepted", by: "relay", why: first.reason || null });
            throw new ChannelRefused("relay", first.reason || null);
          }
          continue;
        }
        // The hub carries no channels (`RH_HUB_CHANNEL_RELAY=off`): the link
        // remembers it until it reconnects. With relays to go to, the next
        // pass looks for one as a channel with nowhere else to go does (it
        // waits longer for the machine to come on one, and looks again before
        // giving up); with none, no relay can carry this one.
        if (first.code === HUB_NO_CHANNELS) {
          if (this.options.relays) continue;
          throw this.blockedNow();
        }
        if (first.code === HUB_OFFLINE) {
          this.setStatus("offline", first.reason);
          throw new Error("the device is offline");
        }
        if (first.code === HUB_FORBIDDEN) {
          this.refuse({ kind: "notAccepted", by: "hub", why: first.reason || null });
          throw new ChannelRefused("hub", first.reason || null);
        }
        continue;
      }
      if (first.type !== "data") continue;
      try {
        // Entries the agent has that this device lacks (a removal, a
        // pending recovery, its renewal) are merged into a copy of the account's chain,
        // which becomes its view once held against what this browser
        // verified — on a refusal too. A fork either shows is latched.
        const { transport, reply } = await this.account.completeHandshake((merged) =>
          finishHandshake(initiator, merged.chain, merged.clock, first.payload, DEFAULT_REKEY, Date.now()),
        );
        const responder = copy(transport.remoteStatic);
        if (!this.isCurrentKey(responder)) {
          // The reply taught this device that the machine renewed its key
          // (its entries were merged just now): no channel to the old key.
          // It is closed, and the next try goes to the new one.
          carrier.send({ type: "close", ch, code: 1000, reason: "a renewed key" });
          carrier.release(ch);
          continue;
        }
        this.handshakeMs = performance.now() - started;
        this.lastRefusal = null;
        this.refusal = null;
        const features = serverFeatures(reply.server);
        const paths = features.includes(CLIENT_FEATURE);
        this.log("handshake.done", { on, ms: this.handshakeMs, paths, direct: features.includes(DIRECT_FEATURE) });
        const driver = new ChannelDriver(ch, transport, "initiator", (f) => void carrier.send(f), Date.now, paths ? { paths: { probeRelay: true } } : {});
        const openedAt = performance.now();
        const offSample = connLog.addSampler(on, channelSampler(driver, () => this.pipesOpen));
        driver.onpathevent = (event) => this.logPathEvent(on, driver, event);
        const take = (f: Frame) => {
          if (f.type === "close") this.ending.add(driver);
          driver.receive(f);
        };
        for (const f of queue.drain()) take(f);
        routeTo = take;
        let direct: DirectPaths | null = null;
        // The connection log hears the rule's decisions: gone (the channel is closed), and slow but kept.
        const watchRelay = watchRelayPath(driver, relayLink, (stats) => this.logRelaySilent(on, driver, stats));
        const slowLog = this.relaySlowLogger(on, driver);
        driver.onpaths = (stats) => {
          if (this.driver === driver) this.setPaths(stats);
          slowLog(stats);
          watchRelay(stats);
        };
        const offGoaway = relayLink?.onGoaway(({ retryMs }) => {
          // The relay is draining: move before it closes.
          if (this.driver === driver && relay) void this.moveOff(driver, relay, retryMs);
        });
        driver.onclose = (code, reason, local) => {
          carrier.release(ch);
          offGoaway?.();
          direct?.close();
          if (this.direct === direct) this.direct = null;
          // Logged once what the close must do is done (and it never throws), before the status it leads to.
          offSample();
          this.logClose(on, driver, { code, reason, local: local === true, openedAt, route });
          if (this.driver === driver) {
            this.driver = null;
            this.responder = null;
            this.unwatchKey();
            this.setPaths(null);
            this.setRoute(null);
            // Unless a successor is already opening (a pipe came while this
            // one was ending), or failed to: that status is the one to show.
            if (this.status === "open") this.setStatus("idle");
          }
        };
        if (paths && features.includes(DIRECT_FEATURE) && this.options.direct) {
          direct = this.directPathsFor(driver, { account: chain.account, device: this.account.deviceId, machine: this.machine });
        }
        if (relay) this.options.relays?.noteSuccess(relay);
        return { driver, direct, route, off: () => offGoaway?.(), responder, logOn: on, attempt: attempt + 1 };
      } catch (error) {
        this.log("handshake.fail", { on, ...errorFields(error, "why"), refused: error instanceof HandshakeError && error.kind === "refused" });
        carrier.release(ch);
        if (error instanceof HandshakeError && error.kind === "refused") {
          this.lastRefusal = error.reply?.status === "stale" ? "stale" : "needEntries";
          // Stale: our attestation had not reached the agent; NeedEntries:
          // it is behind. Either way: sync, attest again, retry.
          await this.account.sync();
          await new Promise((resolve) => setTimeout(resolve, 200 * (attempt + 1)));
          continue;
        }
        if (!resynced) {
          // Any other failure — the reply does not open, say: the machine may
          // have renewed its key before this device heard. The chain says,
          // then one more try.
          resynced = true;
          await this.resync();
          continue;
        }
        throw error;
      }
    }
    if (this.lastRefusal === "stale") this.refuse({ kind: "unconfirmed" });
    throw new Error("the device refused the channel");
  }

  /** Sync the account's chain before a retry; one that cannot be had now leaves the retry to what the chain says already. */
  private async resync(): Promise<void> {
    try {
      await this.account.sync();
    } catch {
      // the retry goes on what this device has
    }
  }

  /** Whether `key` is the machine's DH key as the chain gives it now (one the chain does not name a key for is left to the agent). */
  private isCurrentKey(key: Uint8Array): boolean {
    const now = this.account.chain?.deviceRecord(this.machine)?.info.dhKey;
    return !now || equal(now, key);
  }

  /** Listen to the account while a channel is installed. */
  private watchKey(): void {
    this.offAccount ??= this.account.onChange(() => this.checkKey());
  }

  private unwatchKey(): void {
    this.offAccount?.();
    this.offAccount = null;
  }

  /**
   * The chain gives the machine another DH key than the installed channel was
   * handshaken to — it renewed its identity key (§3.5 "关通道"): that channel
   * and its direct paths are closed now, and the next pipe opens one to the
   * new key. What was in flight on it fails as it would on a lost channel.
   * So is one to a machine that became isolated (§3.8): nothing stays
   * connected to it.
   */
  private checkKey(): void {
    const driver = this.driver;
    const isolated = this.account.isolated?.(this.machine) === true && !this.options.isolatedRenewal;
    if (!driver || !this.responder || (this.isCurrentKey(this.responder) && !isolated)) return;
    this.ending.add(driver);
    this.direct?.close();
    this.direct = null;
    this.driver = null;
    this.responder = null;
    this.unwatchKey();
    this.carrierOff?.();
    this.carrierOff = null;
    closeRules.set(driver, "key");
    driver.close(1000, "the device renewed its key");
    this.setPaths(null);
    this.setRoute(null);
    if (this.status === "open") this.setStatus("idle");
  }

  /** Note how this try was refused; a refusal by the hub or a relay is not the agent's `stale` any more. */
  private refuse(refusal: ChannelRefusal): void {
    this.log("channel.refused", { kind: refusal.kind, by: refusal.kind === "notAccepted" ? refusal.by : "agent", why: refusal.kind === "notAccepted" ? refusal.why : null });
    this.refusal = refusal;
    this.refusedThisTry = true;
    if (refusal.kind !== "unconfirmed") this.lastRefusal = null;
  }

  /** A direct path for `driver`'s channel: the same handshake as the hub's, under the direct binding. */
  private directPathsFor(driver: ChannelDriver, binding: Binding): DirectPaths {
    const account = this.account;
    // A direct handshake that failed other than by the agent's refusal has had the chain synced (once a channel).
    let resynced = false;
    return new DirectPaths({
      driver,
      binding,
      env: this.options.directEnv,
      history: this.directHistory,
      handshake: {
        start: async (binding) => {
          account.assertNotForked();
          const chain = account.chain;
          if (!chain) throw new Error("this device has no account chain");
          return startDirectHandshake(chain, account.noiseKeys, binding, Date.now(), utf8(CLIENT_INFO));
        },
        finish: async (initiator, reply) => {
          try {
            return (await account.completeHandshake((merged) => finishHandshake(initiator, merged.chain, merged.clock, reply, DEFAULT_REKEY, Date.now()))).transport;
          } catch (error) {
            // As on the hub path: a reply that does not open may be the
            // machine's renewed key. The chain is synced (a renewal closes
            // this channel), and the next attempt is the retry.
            if (!(error instanceof HandshakeError && error.kind === "refused") && !resynced && account.forked === null) {
              resynced = true;
              await this.resync();
            }
            throw error;
          }
        },
        // As on the hub path: the agent is behind, or our attestation had not reached it.
        refused: async () => {
          await account.sync();
          if (account.isAdmin) this.link.attest(await account.attestation());
        },
      },
    });
  }
}
