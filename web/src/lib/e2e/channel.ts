/*
 * An established end-to-end channel on one hub `ch`: Noise transport and
 * sub-stream mux glued together (`droppen-proto/src/channel.rs`). Without paths,
 * every mux frame is one transport message and one hub `DATA` frame.
 *
 * Two paths: a channel whose ends negotiated `CLIENT_FEATURE` is made with
 * `Session.withPaths`; every mux frame then travels in a sequenced envelope
 * (`path.ts` `Lanes`), and a second transport — a Noise session run over a
 * WebRTC DataChannel — can be attached with `attachDirect`. While it is,
 * new frames go that way, kept until the peer acknowledges them. When it
 * fails (the DataChannel closes, nothing comes over it for
 * `DIRECT_TIMEOUT_MS`, what it carries waits for an acknowledgement while the
 * path makes no progress for `UNACKED_TIMEOUT_MS` or ten round trips — both
 * stretched by what the path carries, `directDeadline` — a message on it
 * does not open, or the peer says so with a `down` envelope), `detachDirect`
 * sends everything unacknowledged again over the hub and the receiver drops
 * what it already had.
 *
 * Path times are milliseconds of a monotonic clock (`performance.now()` in
 * the driver); the transports seal with wall-clock milliseconds for
 * rekeying. Rust keeps one clock in microseconds; the rules are the same.
 */
import { type Frame, INITIAL_CREDIT } from "./frame";
import { decodeMuxFrame, encodeMuxFrame, Mux, type MuxEvent, type MuxFrame, type Role } from "./mux";
import type { Transport } from "./noise";
import {
  ASSUMED_BYTES_PER_MS,
  BULK_RECENT_MS,
  DeadlineRate,
  decodeDcMessage,
  decodeEnvelope,
  DIRECT_PROBE_MS,
  type DirectDeadline,
  directDeadline,
  type DirectLoad,
  type DirectTimer,
  type DownReason,
  encodeDcMessage,
  encodeEnvelope,
  type Envelope,
  type LaneStats,
  Lanes,
  PathError,
  RATE_SAMPLE_BYTES,
  RELAY_BULK_AHEAD_MAX_MS,
  RELAY_PROBE_MS,
  UNACKED_LIMIT,
  type Via,
} from "./path";

/** Which paths can take a message right now. */
export interface Room {
  relay: boolean;
  direct: boolean;
}

/** How a two-path session behaves. */
export interface PathOptions {
  /** Probe the hub path every `RELAY_PROBE_MS` for its round-trip time (the device does, for the indicator). */
  probeRelay: boolean;
}

/**
 * Something about the paths the owner may want to know: a direct path
 * attached or let go (a timeout says which deadline, how long the path had
 * gone without, and what the deadline was made of), the peer's word that it
 * let the direct path go after this side already had (`peerDown`: why the
 * peer did, when its DataChannel's closing came first), a probe sent and the
 * round trip of its answer, a hub probe that has gone a whole probe interval
 * unanswered (`late`, once per probe: no answer, so no round trip — the
 * path's `relayProbeAgeMs` and `relayQuietMs` say how old and how quiet),
 * and a pause of this page's timers that made the probes before it
 * forgotten.
 */
export type PathEvent =
  | { type: "directUp" }
  | {
      type: "directDown";
      reason: DownReason;
      remote: boolean;
      /** For a timeout on this side: which deadline (`DIRECT_TIMEOUT_MS` of silence, or `UNACKED_TIMEOUT_MS` with frames unacknowledged), and how long since the path last showed it was there (ms). */
      timer?: DirectTimer;
      elapsedMs?: number;
      unackedBytes?: number;
      /** For a timeout: the deadline it missed (`directDeadline`), and what the path carried then. */
      deadline?: DirectDeadline;
      load?: DirectLoad;
    }
  | { type: "peerDown"; reason: DownReason }
  | { type: "probe"; via: Via; nonce: number }
  | { type: "rtt"; via: Via; ms: number; nonce?: number }
  /** A hub probe out a whole probe interval (`unsent`: still waiting for credit to leave), said once: its age, and how long nothing had come over the hub then. */
  | { type: "late"; via: Via; nonce: number; ageMs: number; quietMs: number; unsent: boolean }
  | { type: "paused"; gapMs: number; forgotten: number };

/** What waits on a session's paths now, for logs. */
export interface SessionPathDiagnostics {
  /** Sent on the direct path, not acknowledged yet. */
  unackedBytes: number;
  unackedFrames: number;
  /** Received ahead of a gap, held. */
  heldRxBytes: number;
  /** Envelopes queued for the hub path outside the multiplexer's order (resends, probes, their answers). */
  outRelay: number;
  /** Probes unanswered, and the age of the oldest (ms). */
  probesOut: number;
  oldestProbeMs: number | null;
}

/** The paths' state, for logs and the path indicator. */
export interface SessionPathStats {
  direct: boolean;
  /** The latest round trip a probe measured on the hub path; never a guess. */
  relayRttMs: number | null;
  directRttMs: number | null;
  /**
   * How long the oldest hub probe still out has gone unanswered, or waited
   * for credit to leave (whole ms), as of the latest probe tick or answer;
   * `null` with none out then.
   */
  relayProbeAgeMs: number | null;
  /** How long nothing had arrived on the hub path at that moment (whole ms); `null` with no probe out. */
  relayQuietMs: number | null;
  ups: number;
  downs: number;
  lanes: LaneStats;
}

/** Where a sealed message goes: a `DATA` frame for the hub, or one DataChannel message. */
export type Wire = { type: "relay"; frame: Frame } | { type: "direct"; message: Uint8Array<ArrayBuffer> };

/** A sealed message, and the stream data it carried, if any. */
export interface Polled {
  wire: Wire;
  data: { sid: number; len: number } | null;
}

/**
 * The most probes outstanding at once on each path (the oldest of that
 * path's is forgotten): a direct path's probes, seconds late behind its own
 * traffic, never push out the hub path's, whose age judges the channel.
 */
const MAX_PROBES = 16;

function dataOf(frame: MuxFrame): { sid: number; len: number } | null {
  return frame.type === "data" ? { sid: frame.sid, len: frame.data.length } : null;
}

class Paths {
  readonly lanes = new Lanes();
  direct: Transport | null = null;
  /** Envelopes for the hub outside the multiplexer's order: a `down`, resends, probes and their answers. */
  outRelay: Envelope[] = [];
  /** Probes and their answers for the direct path. */
  outDirect: Envelope[] = [];
  readonly probes = new Map<number, [Via, number]>();
  /**
   * Hub probes queued and not sent yet, and since when: the hub window is
   * spent (this page is uploading) and the peer has handed no credit back. A
   * probe that cannot leave says the hub path is as slow as one unanswered,
   * and on a path that died while this page was sending it is the only probe
   * out.
   */
  readonly unsentRelayProbes = new Map<number, number>();
  private nextNonce = 1;
  lastDirectRxMs: number;
  /**
   * When the direct path last showed it delivers: the peer's data arrived
   * over it, or an acknowledgement freed frames sent over it. Probes do not
   * count: they say the peer is there, not that what we sent got through.
   */
  lastDirectProgressMs: number;
  nextDirectProbeMs: number;
  nextRelayProbeMs: number;
  /**
   * When anything last arrived for this channel over the hub: a `DATA` frame
   * (data, an acknowledgement, a probe or its answer) or a `CREDIT`. While it
   * keeps coming the hub path carries, however late its probes are answered.
   */
  lastRelayRxMs: number;
  /** When `tick` last ran: a long gap means this page's timers were held, not that the peer went silent. */
  lastTickMs: number;
  /** The rate the direct path's deadlines time the bytes ahead at (`DeadlineRate`). */
  readonly deadlineRate = new DeadlineRate();
  /** When bulk (a message of `RATE_SAMPLE_BYTES` or more) last came over each path. */
  private readonly bulkIn: Record<Via, number | null> = { relay: null, direct: null };
  /** When this side last sealed bulk for the hub path. */
  private relayBulkOutMs: number | null = null;
  /** What the direct path carried as it was last let go (`directLoad`). */
  lastLoad: DirectLoad | null = null;
  events: PathEvent[] = [];
  readonly counts = {
    relayRttMs: null as number | null,
    directRttMs: null as number | null,
    relayProbeAgeMs: null as number | null,
    relayQuietMs: null as number | null,
    ups: 0,
    downs: 0,
  };

  constructor(
    readonly opts: PathOptions,
    nowMs: number,
  ) {
    this.lastRelayRxMs = nowMs;
    this.lastDirectRxMs = nowMs;
    this.lastDirectProgressMs = nowMs;
    this.nextDirectProbeMs = nowMs;
    // The first hub probe goes out at once, so the indicator has a number.
    this.nextRelayProbeMs = nowMs;
    this.lastTickMs = nowMs;
  }

  nonce(): number {
    return this.nextNonce++;
  }

  /** What the direct path carries now, for its deadlines (`directDeadline`). */
  load(): DirectLoad {
    const heard = Math.max(this.lastDirectRxMs, this.lastRelayRxMs);
    const recent = (at: number | null) => at !== null && heard - at <= BULK_RECENT_MS;
    const oldest = this.lanes.oldestUnackedMs();
    const bytesPerMs = this.deadlineRate.value(heard);
    // A whole hub window this side sent may still be on its way for as long
    // as the link takes to carry one — and no longer than RELAY_BULK_AHEAD_MAX_MS.
    const windowMs = Math.min(RELAY_BULK_AHEAD_MAX_MS, Math.max(BULK_RECENT_MS, INITIAL_CREDIT / (bytesPerMs ?? ASSUMED_BYTES_PER_MS)));
    return {
      rttMs: this.counts.directRttMs,
      unackedBytes: this.lanes.unackedBytes,
      bytesPerMs,
      relayBulkSent: oldest !== null && this.relayBulkOutMs !== null && oldest - this.relayBulkOutMs <= windowMs,
      relayBulkReceived: recent(this.bulkIn.relay),
      directBulkReceived: recent(this.bulkIn.direct),
    };
  }

  /**
   * The direct path's deadlines now: silence, counted from the last thing it
   * brought; and, while frames sent on it wait for an acknowledgement, no
   * progress, counted from the later of the oldest such frame leaving and the
   * path's last progress. Counted from the frame alone, a backlog the link
   * drains more slowly than that (the DataChannel's own buffer, or the peer's
   * acknowledgement queued behind its own backlog) gave a healthy path up.
   */
  directDeadlines(): { load: DirectLoad; silence: DirectDeadline; progress: { since: number; deadline: DirectDeadline } | null } {
    const load = this.load();
    const sent = this.lanes.oldestUnackedMs();
    return {
      load,
      silence: directDeadline("DIRECT_TIMEOUT_MS", load),
      progress: sent === null ? null : { since: Math.max(sent, this.lastDirectProgressMs), deadline: directDeadline("UNACKED_TIMEOUT_MS", load) },
    };
  }

  /**
   * An acknowledgement came (on either path): progress on the direct one when
   * it freed frames sent there, and a step of the deadlines' rate.
   */
  acked(ack: number, nowMs: number): void {
    const before = this.lanes.unackedBytes;
    const oldest = this.lanes.oldestUnackedMs();
    if (this.lanes.onAck(ack) === 0) return;
    this.lastDirectProgressMs = nowMs;
    this.deadlineRate.acked(before - this.lanes.unackedBytes, oldest ?? nowMs, this.lanes.unackedBytes, nowMs, this.counts.directRttMs);
  }

  /** A message of `len` bytes came over `via`: bulk when it weighs `RATE_SAMPLE_BYTES` or more. */
  heardBulk(via: Via, len: number, nowMs: number): void {
    this.deadlineRate.heard(via, len, nowMs, this.counts.directRttMs);
    if (len >= RATE_SAMPLE_BYTES) this.bulkIn[via] = nowMs;
  }

  /** This side sealed `len` bytes for the hub path. */
  sentRelay(len: number, nowMs: number): void {
    if (len >= RATE_SAMPLE_BYTES) this.relayBulkOutMs = nowMs;
  }

  /** A new direct path: what was measured on the one before is not this one's. */
  attached(): void {
    this.deadlineRate.reset();
    this.bulkIn.direct = null;
  }

  /**
   * A hub probe still unanswered (or still waiting for credit to leave) says
   * the hub path is at least that slow: note how long the oldest one has been
   * out, and how long nothing has arrived on the hub path, as of `nowMs`.
   * Kept apart from the round trip, which only a probe's answer sets: the
   * indicator says an old probe as "slow" rather than as a round trip nobody
   * measured (`relaySlowMs`), and the two together tell a slow path, which
   * keeps delivering, from one that is gone (`relayPathGone`).
   */
  ageRelayProbe(nowMs: number): void {
    let oldest: number | null = null;
    for (const [via, at] of this.probes.values()) if (via === "relay" && (oldest === null || at < oldest)) oldest = at;
    for (const at of this.unsentRelayProbes.values()) if (oldest === null || at < oldest) oldest = at;
    this.counts.relayProbeAgeMs = oldest === null ? null : Math.round(nowMs - oldest);
    this.counts.relayQuietMs = oldest === null ? null : Math.round(Math.max(0, nowMs - this.lastRelayRxMs));
    this.noteLateRelayProbes(nowMs);
  }

  /** Hub probes already said to be late (`late` events), so each is said once. */
  private readonly lateRelayProbes = new Set<number>();

  /**
   * Each hub probe out (sent, or waiting for credit to leave) that has just
   * gone a whole probe interval unanswered is said once, as a `late` event,
   * for logs: what the path's age and quiet are made of, probe by probe.
   * Reads only; the path's state is untouched.
   */
  private noteLateRelayProbes(nowMs: number): void {
    const quietMs = Math.round(Math.max(0, nowMs - this.lastRelayRxMs));
    const look = (nonce: number, at: number, unsent: boolean) => {
      if (this.lateRelayProbes.has(nonce) || nowMs - at < RELAY_PROBE_MS) return;
      this.lateRelayProbes.add(nonce);
      if (this.lateRelayProbes.size > 2 * MAX_PROBES) this.lateRelayProbes.delete(this.lateRelayProbes.values().next().value as number);
      this.events.push({ type: "late", via: "relay", nonce, ageMs: Math.round(nowMs - at), quietMs, unsent });
    };
    for (const [nonce, [via, at]] of this.probes) if (via === "relay") look(nonce, at, false);
    for (const [nonce, at] of this.unsentRelayProbes) look(nonce, at, true);
  }

  /**
   * This page's timers were held (a background tab, a phone asleep): the
   * probes sent before the pause were not answered because nothing was read,
   * not because the path died. Forget them; the tick that noticed sends a
   * fresh one, which the watchdog can judge on its own.
   */
  forgetProbesAfterPause(nowMs: number): number {
    let forgotten = 0;
    for (const [nonce, [via]] of this.probes) {
      if (via !== "relay") continue;
      this.probes.delete(nonce);
      forgotten++;
    }
    // Still queued, they are timed afresh from when they leave.
    this.unsentRelayProbes.clear();
    this.lateRelayProbes.clear();
    // Nor is what was being timed for the deadlines' rate the link's.
    this.deadlineRate.restart();
    this.ageRelayProbe(nowMs);
    return forgotten;
  }

  /** Remember when a probe left (and on which path). */
  noteProbe(env: Envelope, via: Via, nowMs: number): void {
    if (env.type !== "probe") return;
    if (via === "relay") this.unsentRelayProbes.delete(env.nonce);
    let count = 0;
    let oldest: number | null = null;
    for (const [nonce, [on]] of this.probes) {
      if (on !== via) continue;
      count++;
      if (oldest === null || nonce < oldest) oldest = nonce;
    }
    if (count >= MAX_PROBES && oldest !== null) this.probes.delete(oldest);
    this.probes.set(env.nonce, [via, nowMs]);
    this.events.push({ type: "probe", via, nonce: env.nonce });
  }

  detach(reason: DownReason, tellPeer: boolean): void {
    this.lastLoad = this.load();
    this.direct = null;
    this.outDirect = [];
    for (const [nonce, [via]] of this.probes) if (via !== "relay") this.probes.delete(nonce);
    this.counts.downs += 1;
    // The resends go first, ahead of anything queued for the hub since (a
    // probe, a probe's answer), and the `down` ahead of them.
    const down: Envelope[] = tellPeer ? [{ type: "down", reason }] : [];
    this.outRelay = [...down, ...this.lanes.takeUnacked(), ...this.outRelay];
  }
}

export class Session {
  readonly mux: Mux;
  private paths: Paths | null = null;

  constructor(
    readonly ch: number,
    readonly transport: Transport,
    role: Role,
  ) {
    this.mux = new Mux(role);
  }

  /** A session whose ends negotiated `CLIENT_FEATURE`: every frame sequenced, and a direct path can be attached. */
  static withPaths(ch: number, transport: Transport, role: Role, opts: PathOptions, nowMs: number): Session {
    const s = new Session(ch, transport, role);
    s.paths = new Paths(opts, nowMs);
    return s;
  }

  get hasPaths(): boolean {
    return this.paths !== null;
  }

  /** Whether new frames go over a direct path now. */
  get directActive(): boolean {
    return this.paths?.direct != null;
  }

  stats(): SessionPathStats {
    const p = this.paths;
    if (!p) return { direct: false, relayRttMs: null, directRttMs: null, relayProbeAgeMs: null, relayQuietMs: null, ups: 0, downs: 0, lanes: new Lanes().stats() };
    return { ...p.counts, direct: p.direct !== null, lanes: p.lanes.stats() };
  }

  /** What waits on the paths now (`null` on a channel without paths). Reads only. */
  diagnostics(nowMs: number): SessionPathDiagnostics | null {
    const p = this.paths;
    if (!p) return null;
    let oldest: number | null = null;
    for (const [, at] of p.probes.values()) if (oldest === null || at < oldest) oldest = at;
    return {
      unackedBytes: p.lanes.unackedBytes,
      unackedFrames: p.lanes.unackedFrames,
      heldRxBytes: p.lanes.heldRxBytes,
      outRelay: p.outRelay.length,
      probesOut: p.probes.size,
      oldestProbeMs: oldest === null ? null : Math.max(0, nowMs - oldest),
    };
  }

  /**
   * What the direct path carries now — or, with none attached, carried as it
   * was last let go (`null` before any was): whether it went under load.
   */
  directLoad(): DirectLoad | null {
    const p = this.paths;
    if (!p) return null;
    return p.direct ? p.load() : p.lastLoad;
  }

  /** What happened to the paths since the last call. */
  takeEvents(): PathEvent[] {
    const p = this.paths;
    if (!p) return [];
    const out = p.events;
    p.events = [];
    return out;
  }

  /**
   * Start sending over a direct path whose handshake has just completed. A
   * direct path already attached is let go first (its unacknowledged frames
   * go again over the hub).
   */
  attachDirect(transport: Transport, nowMs: number): void {
    const p = this.paths;
    if (!p) throw new PathError("protocol", "this channel did not negotiate paths");
    if (p.direct) p.detach("closed", false);
    p.direct = transport;
    p.lastDirectRxMs = nowMs;
    p.lastDirectProgressMs = nowMs;
    p.nextDirectProbeMs = nowMs;
    p.outDirect = [];
    p.attached();
    p.counts.ups += 1;
    p.events.push({ type: "directUp" });
  }

  /**
   * Stop using the direct path: what it did not deliver goes again over the
   * hub, after a `down` (`tellPeer`). With `nowMs`, the hub path's numbers
   * are taken afresh then: they alone judge it from now on (`relayPathGone`),
   * and as of the last probe tick they may miss what arrived since.
   */
  detachDirect(reason: DownReason, tellPeer: boolean, nowMs?: number): void {
    const p = this.paths;
    if (!p?.direct) return;
    p.detach(reason, tellPeer);
    if (nowMs !== undefined) p.ageRelayProbe(nowMs);
    p.events.push({ type: "directDown", reason, remote: false });
  }

  /** Everything the multiplexer can send now, sealed into `DATA` frames (the hub path only). */
  async flush(nowMs: number): Promise<Frame[]> {
    const out: Frame[] = [];
    const room = { relay: true, direct: false };
    for (let p = await this.poll(room, nowMs, nowMs); p; p = await this.poll(room, nowMs, nowMs)) {
      if (p.wire.type === "relay") out.push(p.wire.frame);
    }
    return out;
  }

  /**
   * The next sealed message for a path with room, if there is one. `nowMs`
   * is the path clock (probes, acknowledgements); `wallMs` is what the
   * transport seals with.
   */
  async poll(room: Room, nowMs: number, wallMs: number): Promise<Polled | null> {
    const p = this.paths;
    if (!p) {
      if (!room.relay) return null;
      const frame = this.mux.poll();
      if (!frame) return null;
      const data = dataOf(frame);
      return { wire: await this.relay(wallMs, encodeMuxFrame(frame)), data };
    }
    // Queued envelopes for each path first: on the hub path that is what a
    // fallback resends, which must go before anything newer.
    if (room.relay) {
      const env = p.outRelay.shift();
      if (env) {
        p.noteProbe(env, "relay", nowMs);
        const plain = encodeEnvelope(env);
        p.sentRelay(plain.length, nowMs);
        return { wire: await this.relay(wallMs, plain), data: null };
      }
    }
    if (room.direct && p.direct) {
      const env = p.outDirect.shift();
      if (env) {
        p.noteProbe(env, "direct", nowMs);
        return { wire: await this.directWire(p.direct, wallMs, env), data: null };
      }
    }
    const direct = p.direct;
    if (!this.pathRoom(room)) return null;
    // New frames on the direct path also wait while too much it carries is
    // unacknowledged; an acknowledgement never does (it is not kept).
    const dataRoom = direct === null || p.lanes.unackedBytes < UNACKED_LIMIT;
    let env: Envelope;
    let data: Polled["data"] = null;
    const frame = dataRoom ? this.mux.poll() : null;
    if (frame) {
      data = dataOf(frame);
      env = p.lanes.stampAt(encodeMuxFrame(frame), direct !== null, nowMs);
    } else {
      const due = p.lanes.ackDueMs();
      if (due === null || due > nowMs) return null;
      env = { type: "ack", ack: p.lanes.takeAck() };
    }
    if (direct) return { wire: await this.directWire(direct, wallMs, env), data };
    const plain = encodeEnvelope(env);
    p.sentRelay(plain.length, nowMs);
    return { wire: await this.relay(wallMs, plain), data };
  }

  /** Whether the path new envelopes go on can take one now (the hub path waits behind its own queue). */
  private pathRoom(room: Room): boolean {
    const p = this.paths!;
    return p.direct ? room.direct : room.relay && p.outRelay.length === 0;
  }

  private async relay(wallMs: number, plain: Uint8Array): Promise<Wire> {
    return { type: "relay", frame: { type: "data", ch: this.ch, payload: await this.transport.seal(wallMs, plain) } };
  }

  private async directWire(direct: Transport, wallMs: number, env: Envelope): Promise<Wire> {
    return { type: "direct", message: encodeDcMessage({ type: "sealed", body: await direct.seal(wallMs, encodeEnvelope(env)) }) };
  }

  /** One `DATA` payload from the peer on a channel without paths; any error is fatal to the channel. */
  async receive(payload: Uint8Array): Promise<MuxEvent | null> {
    if (this.paths) throw new PathError("protocol", "a channel with paths is read with receiveRelay");
    return this.mux.receive(decodeMuxFrame(await this.transport.open(payload)));
  }

  /** One `DATA` payload from the hub. Any error is fatal to the channel. */
  async receiveRelay(payload: Uint8Array, nowMs: number): Promise<MuxEvent[]> {
    const plain = await this.transport.open(payload);
    if (!this.paths) {
      const event = this.mux.receive(decodeMuxFrame(plain));
      return event ? [event] : [];
    }
    this.heardRelay(nowMs);
    this.paths.heardBulk("relay", payload.length, nowMs);
    return this.onEnvelope(decodeEnvelope(plain), "relay", nowMs);
  }

  /** Something for this channel came over the hub (a `DATA` frame, or a `CREDIT`): the hub path still carries. */
  heardRelay(nowMs: number): void {
    const p = this.paths;
    if (p) p.lastRelayRxMs = Math.max(p.lastRelayRxMs, nowMs);
  }

  /**
   * One DataChannel message. Trouble with the direct path itself (a message
   * that does not open or parse) lets the direct path go — the hub path
   * carries on; only a violation in what it carried is fatal.
   */
  async receiveDirect(message: Uint8Array, nowMs: number): Promise<MuxEvent[]> {
    const p = this.paths;
    // Left over from a direct path already let go.
    if (!p?.direct) return [];
    const direct = p.direct;
    let env: Envelope | null = null;
    try {
      const m = decodeDcMessage(message);
      if (m.type === "sealed") env = decodeEnvelope(await direct.open(m.body));
    } catch {
      env = null;
    }
    // `down` belongs on the hub path only.
    if (!env || env.type === "down") {
      this.detachDirect("failed", true, nowMs);
      return [];
    }
    p.lastDirectRxMs = nowMs;
    p.heardBulk("direct", message.length, nowMs);
    return this.onEnvelope(env, "direct", nowMs);
  }

  private onEnvelope(env: Envelope, via: Via, nowMs: number): MuxEvent[] {
    const p = this.paths!;
    const events: MuxEvent[] = [];
    switch (env.type) {
      case "data":
        p.acked(env.ack, nowMs);
        if (via === "direct") p.lastDirectProgressMs = nowMs;
        for (const bytes of p.lanes.receive(env.seq, env.frame, via === "direct", nowMs)) {
          const event = this.mux.receive(decodeMuxFrame(bytes));
          if (event) events.push(event);
        }
        break;
      case "ack":
        p.acked(env.ack, nowMs);
        break;
      case "probe": {
        // Answered at once, with any acknowledgement owed: a side whose timers
        // are held back (this page in a background tab) still acknowledges
        // once per probe, which comes from the other side.
        const out = via === "relay" ? p.outRelay : p.outDirect;
        out.push({ type: "probeAck", nonce: env.nonce });
        if (p.lanes.owed > 0) out.push({ type: "ack", ack: p.lanes.takeAck() });
        break;
      }
      case "probeAck": {
        const sent = p.probes.get(env.nonce);
        if (sent) {
          p.probes.delete(env.nonce);
          if (sent[0] === via) {
            const ms = Math.max(0, nowMs - sent[1]);
            if (via === "relay") p.counts.relayRttMs = ms;
            else p.counts.directRttMs = ms;
            p.events.push({ type: "rtt", via, ms, nonce: env.nonce });
          }
          // What is still out is younger: say how old now.
          if (sent[0] === "relay") p.ageRelayProbe(nowMs);
        }
        break;
      }
      case "down":
        if (p.direct) {
          p.detach(env.reason, false);
          // The hub path alone judges it from now on, and it just delivered
          // this `down`: its numbers afresh, not as of the last probe tick.
          p.ageRelayProbe(nowMs);
          p.events.push({ type: "directDown", reason: env.reason, remote: true });
        } else {
          // The peer let a direct path go after this side had — its
          // DataChannel's closing (the peer tearing it down) came first over
          // the direct path, or this side's own deadline did, and the `down`
          // behind the hub path's backlog. Its reason is the only word of why
          // the peer let go.
          p.events.push({ type: "peerDown", reason: env.reason });
        }
        break;
    }
    return events;
  }

  /** Run the paths' timers: probes, the direct path's keep-alive and timeout. A due acknowledgement is sent by `poll`. */
  tick(nowMs: number): void {
    const p = this.paths;
    if (!p) return;
    // A gap of several probe intervals means this page was away, not that the
    // peer went quiet: do not age the probes nobody was there to read.
    const gapMs = nowMs - p.lastTickMs;
    const paused = gapMs > 3 * RELAY_PROBE_MS;
    p.lastTickMs = nowMs;
    if (paused) p.events.push({ type: "paused", gapMs, forgotten: p.forgetProbesAfterPause(nowMs) });
    if (p.direct) {
      const { load, silence, progress } = p.directDeadlines();
      const quietMs = nowMs - p.lastDirectRxMs;
      const silent = quietMs > silence.limitMs;
      const stuck = progress !== null && nowMs - progress.since > progress.deadline.limitMs;
      if (silent || stuck) {
        const deadline = silent ? silence : progress!.deadline;
        const elapsedMs = silent ? quietMs : nowMs - progress!.since;
        p.detach("timeout", true);
        p.ageRelayProbe(nowMs);
        p.events.push({
          type: "directDown",
          reason: "timeout",
          remote: false,
          timer: silent ? "DIRECT_TIMEOUT_MS" : "UNACKED_TIMEOUT_MS",
          elapsedMs,
          unackedBytes: load.unackedBytes,
          deadline,
          load,
        });
      } else if (nowMs >= p.nextDirectProbeMs) {
        p.nextDirectProbeMs = nowMs + DIRECT_PROBE_MS;
        p.outDirect.push({ type: "probe", nonce: p.nonce() });
      }
    }
    if (p.opts.probeRelay && nowMs >= p.nextRelayProbeMs) {
      p.nextRelayProbeMs = nowMs + RELAY_PROBE_MS;
      p.ageRelayProbe(nowMs);
      const nonce = p.nonce();
      p.outRelay.push({ type: "probe", nonce });
      p.unsentRelayProbes.set(nonce, nowMs);
    }
  }

  /**
   * When `tick` (or `poll`, for an acknowledgement) next has something to do.
   * Given the `room` there is now, an acknowledgement that could not go
   * anyway is left out: waking for it would only spin until room comes, and
   * whatever brings room (an ack, a drained DataChannel) sends it.
   */
  nextDeadlineMs(room?: Room): number | null {
    const p = this.paths;
    if (!p) return null;
    let next: number | null = room && !this.pathRoom(room) ? null : p.lanes.ackDueMs();
    const take = (t: number) => {
      next = next === null ? t : Math.min(next, t);
    };
    if (p.direct) {
      take(p.nextDirectProbeMs);
      const { silence, progress } = p.directDeadlines();
      take(p.lastDirectRxMs + silence.limitMs + 1);
      if (progress) take(progress.since + progress.deadline.limitMs + 1);
    }
    if (p.opts.probeRelay) take(p.nextRelayProbeMs);
    return next;
  }
}
