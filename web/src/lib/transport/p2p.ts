/*
 * The direct path, device side: a WebRTC DataChannel to the machine next to
 * the hub, carrying the same end-to-end channel faster when the two can reach
 * each other (`lib/e2e/path.ts`; `droppen-proto/src/path.rs` for the why). Not to
 * be confused with `direct.ts`, which is the same-origin server.
 *
 * One `DirectPaths` per channel whose agent offered `direct/1`:
 *
 * 1. Signaling rides a `mux` sub-stream of the channel itself
 *    (`GET /rh/direct`): the agent answers the bridge head with 101, then
 *    every signal either way is one bridge `binary` message. Any other answer,
 *    or the stream closing, means no direct paths on this channel.
 * 2. One attempt at a time, each on a fresh `RTCPeerConnection` (STUN only:
 *    the hub is the fallback, there is no TURN): an offer stating this
 *    device's DTLS fingerprint, trickled candidates, and the agent's answer —
 *    refused unless its SDP names exactly the fingerprint stated next to it
 *    (the hub, a STUN provider or the network cannot swap a certificate).
 * 3. When the DataChannel opens, a fresh Noise IK handshake on it, bound to
 *    the account, device and machine, the hub channel's handshake hash, the
 *    agent's nonce and both fingerprints; admitted by the agent exactly like
 *    a hub channel. Then the driver carries the channel over it
 *    (`ChannelDriver.attachDirect`) until it fails, falling back to the hub
 *    with nothing lost.
 * 4. An attempt that has no handshake done after `ATTEMPT_TIMEOUT_MS` fails;
 *    the next comes after a backoff (2 s doubling to 5 min, jittered; back to
 *    2 s after a path that lived a minute), at once when the network changes
 *    or the page comes back to the foreground — unless the last
 *    `SHORT_LIVED_LIMIT` paths in a row carried the channel and died young
 *    (before `STABLE_MS`): then the backoff stands, whatever the page or the
 *    network says, until a path lives a minute or the user turns direct paths
 *    back on. Each flap costs every request in flight a stall and a resend
 *    over the hub, so a network that keeps killing direct paths (a phone on
 *    mobile data) is left on the hub for longer and longer. A `DirectHistory`
 *    keeps the backoff and the streak for one machine across its channels, so
 *    a new channel (the hub connection came back) does not start from 2 s.
 *
 * "Hub only" (`hubOnly`, a setting of this device kept in `localStorage`)
 * stops all of it: direct connections show this device's IP address to the
 * machine and to the STUN providers. Turning it on lets a direct path go at
 * once; turning it off starts trying again.
 */
import {
  type Binding,
  bridgeMessage,
  type ByeReason,
  type Bytes,
  type ChannelDriver,
  type ChannelStream,
  checkFingerprint,
  DC_LABEL,
  decodeDcMessage,
  decodeResponseHead,
  decodeSignal,
  DIRECT_BUDGET,
  type DirectBinding,
  type DirectLink,
  type DownReason,
  encodeDcMessage,
  encodeRequestHead,
  encodeSignal,
  equal,
  HandshakeError,
  type Initiator,
  MessageReader,
  sdpFingerprint,
  type Signal,
  SIGNAL_PATH,
  stripCandidates,
  type Transport,
} from "@/lib/e2e";

/** Public STUN only; the hub is the fallback, so there is no TURN. */
export const STUN_SERVERS: { urls: string }[] = [{ urls: "stun:stun.cloudflare.com:3478" }, { urls: "stun:stun.l.google.com:19302" }];
/** An attempt that has not finished its Noise handshake by then fails. */
export const ATTEMPT_TIMEOUT_MS = 15_000;
/** The first wait before trying again, doubling per failure… */
export const RETRY_FIRST_MS = 2_000;
/** …up to this. */
export const RETRY_MAX_MS = 5 * 60_000;
/** A direct path that lived this long resets the backoff. */
export const STABLE_MS = 60_000;
/**
 * After this many direct paths in a row that carried the channel and died
 * before `STABLE_MS`, nothing but the backoff starts the next attempt: not a
 * network change, the page coming back, or a new channel.
 */
export const SHORT_LIVED_LIMIT = 3;
/** How long the agent has to answer the signaling stream's head. */
export const SIGNAL_HEAD_TIMEOUT_MS = 15_000;

export interface DirectTiming {
  attemptTimeoutMs: number;
  retryFirstMs: number;
  retryMaxMs: number;
  stableMs: number;
  signalHeadTimeoutMs: number;
}

export const DIRECT_TIMING: DirectTiming = {
  attemptTimeoutMs: ATTEMPT_TIMEOUT_MS,
  retryFirstMs: RETRY_FIRST_MS,
  retryMaxMs: RETRY_MAX_MS,
  stableMs: STABLE_MS,
  signalHeadTimeoutMs: SIGNAL_HEAD_TIMEOUT_MS,
};

// ---------------------------------------------------------------------------
// The "hub only" setting
// ---------------------------------------------------------------------------

export const HUB_ONLY_KEY = "rh.hub.hubOnly";
const settingListeners = new Set<() => void>();
/** Where the setting lives when this browser has no usable storage. */
let hubOnlyMemory = false;
let watchingStorage = false;

function storage(): Storage | null {
  try {
    const s = globalThis.localStorage;
    return s && typeof s.getItem === "function" ? s : null;
  } catch {
    return null;
  }
}

/** Whether this device keeps to the hub (no direct paths). Off by default. */
export function hubOnly(): boolean {
  const s = storage();
  if (!s) return hubOnlyMemory;
  try {
    return s.getItem(HUB_ONLY_KEY) === "1";
  } catch {
    return hubOnlyMemory;
  }
}

export function setHubOnly(on: boolean): void {
  hubOnlyMemory = on;
  const s = storage();
  try {
    if (on) s?.setItem(HUB_ONLY_KEY, "1");
    else s?.removeItem(HUB_ONLY_KEY);
  } catch {
    // kept in memory for this page
  }
  for (const fn of [...settingListeners]) fn();
}

/** Hear the setting change (here or in another tab). */
export function onHubOnly(fn: () => void): () => void {
  settingListeners.add(fn);
  if (!watchingStorage && typeof globalThis.addEventListener === "function") {
    watchingStorage = true;
    globalThis.addEventListener("storage", (e: Event) => {
      if ((e as StorageEvent).key === HUB_ONLY_KEY) for (const listener of [...settingListeners]) listener();
    });
  }
  return () => settingListeners.delete(fn);
}

// ---------------------------------------------------------------------------
// What the manager needs from WebRTC and the page
// ---------------------------------------------------------------------------

/** The part of `RTCDataChannel` used here. */
export interface DataChannelLike {
  binaryType: string;
  readonly bufferedAmount: number;
  bufferedAmountLowThreshold: number;
  onopen: (() => void) | null;
  onmessage: ((ev: { data: unknown }) => void) | null;
  onclose: (() => void) | null;
  onerror: (() => void) | null;
  onbufferedamountlow: (() => void) | null;
  send(data: Bytes): void;
  close(): void;
}

/** The part of `RTCPeerConnection` used here. */
export interface PeerLike {
  createDataChannel(label: string, init: { ordered: boolean }): DataChannelLike;
  createOffer(): Promise<{ type: string; sdp?: string }>;
  setLocalDescription(description: { type: "offer"; sdp?: string }): Promise<void>;
  readonly localDescription: { sdp: string } | null;
  setRemoteDescription(description: { type: "answer"; sdp: string }): Promise<void>;
  addIceCandidate(candidate: { candidate: string; sdpMLineIndex: number }): Promise<void>;
  readonly connectionState: string;
  onicecandidate: ((ev: { candidate: { candidate: string } | null }) => void) | null;
  onconnectionstatechange: (() => void) | null;
  readonly sctp?: { transport?: { getRemoteCertificates?(): ArrayBuffer[] } | null } | null;
  close(): void;
}

export interface DirectEnv {
  createPeer(config: { iceServers: { urls: string }[] }): PeerLike;
  /** The setting allows direct paths now. */
  allowed(): boolean;
  onAllowedChange(fn: () => void): () => void;
  /** Worth trying again at once: the network may have changed (`online`, `navigator.connection` `change`), or the page came back to the foreground. */
  onNetworkChange(fn: () => void): () => void;
  random(): number;
  now(): number;
  log(message: string): void;
}

export function browserEnv(): DirectEnv {
  return {
    createPeer: (config) => new RTCPeerConnection(config) as unknown as PeerLike,
    allowed: () => !hubOnly(),
    onAllowedChange: onHubOnly,
    onNetworkChange: (fn) => {
      const offs: (() => void)[] = [];
      if (typeof globalThis.addEventListener === "function") {
        globalThis.addEventListener("online", fn);
        offs.push(() => globalThis.removeEventListener("online", fn));
      }
      const connection = (globalThis.navigator as { connection?: EventTarget } | undefined)?.connection;
      if (connection && typeof connection.addEventListener === "function") {
        connection.addEventListener("change", fn);
        offs.push(() => connection.removeEventListener("change", fn));
      }
      // Back from the background: its timers may have been held for minutes.
      const doc = globalThis.document;
      if (doc && typeof doc.addEventListener === "function") {
        const visible = () => {
          if (doc.visibilityState === "visible") fn();
        };
        doc.addEventListener("visibilitychange", visible);
        offs.push(() => doc.removeEventListener("visibilitychange", visible));
      }
      return () => offs.forEach((off) => off());
    },
    random: Math.random,
    now: () => performance.now(),
    log: (message) => console.debug(`cloud: direct path: ${message}`),
  };
}

/** The direct handshake, with this device's chain and keys (`startDirectHandshake` / `finishHandshake`). */
export interface DirectHandshake {
  start(binding: DirectBinding): Promise<{ initiator: Initiator; message: Bytes }>;
  /** Read the agent's reply; throws `HandshakeError` (`refused`) when it was not let in. */
  finish(initiator: Initiator, reply: Bytes): Promise<Transport>;
  /** After a refusal: sync the chain and vouch for its head again, as the hub path does. */
  refused?(error: HandshakeError): Promise<void>;
}

/**
 * How direct paths to one machine have fared, kept across that machine's
 * channels (the page's `MachineChannel` holds one): the wait before the next
 * attempt, and how many paths in a row carried the channel and died young.
 */
export class DirectHistory {
  /** The next wait before an attempt; `null` until an attempt has ended. */
  backoffMs: number | null = null;
  /** Direct paths in a row that carried the channel and died before `STABLE_MS`. */
  shortLived = 0;

  /** Whether direct paths have been dying young: only the backoff starts the next attempt. */
  get flapping(): boolean {
    return this.shortLived >= SHORT_LIVED_LIMIT;
  }
}

export interface DirectPathsOptions {
  /** The channel, running with paths. */
  driver: ChannelDriver;
  /** The hub channel's `account | device | machine`. */
  binding: Binding;
  handshake: DirectHandshake;
  env?: Partial<DirectEnv>;
  /** `DIRECT_TIMING` unless a test says otherwise. */
  timing?: Partial<DirectTiming>;
  /** Shared with the machine's earlier and later channels; a fresh one when not given. */
  history?: DirectHistory;
}

/**
 * - `off`: hub only on this device.
 * - `unsupported`: the agent will not do direct paths on this channel.
 * - `signaling`: asking it to (the signaling stream is opening).
 * - `connecting`: an attempt is in flight.
 * - `direct`: a direct path carries the channel.
 * - `waiting`: until the next attempt.
 * - `closed`: the channel ended.
 */
export type DirectState = "off" | "unsupported" | "signaling" | "connecting" | "direct" | "waiting" | "closed";

interface Attempt {
  id: number;
  pc: PeerLike;
  dc: DataChannelLike;
  fp: Bytes | null;
  offered: boolean;
  /** Own candidates found before the offer went. */
  outbox: (string | null)[];
  sentEnd: boolean;
  answer: { fp: Bytes; nonce: Bytes } | null;
  remoteSet: boolean;
  /** The agent's candidates before its answer was applied. */
  inbound: (string | null)[];
  initiator: Initiator | null;
  replied: boolean;
  /** DataChannel messages after the reply, before the driver took the path. */
  early: Uint8Array[];
  link: DirectLink | null;
  attachedAt: number | null;
  timer: ReturnType<typeof setTimeout> | null;
  done: boolean;
}

function messageBytes(data: unknown): Uint8Array | null {
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  return null;
}

function remoteCertificate(pc: PeerLike): ArrayBuffer | null {
  try {
    const certs = pc.sctp?.transport?.getRemoteCertificates?.();
    return certs && certs.length > 0 ? certs[0]! : null;
  } catch {
    return null;
  }
}

const errorText = (error: unknown) => (error instanceof Error ? error.message : String(error));

export class DirectPaths {
  onstate: ((state: DirectState) => void) | null = null;
  private current: DirectState = "waiting";
  private readonly env: DirectEnv;
  private readonly driver: ChannelDriver;
  private signal: ChannelStream | null = null;
  private signalOpen = false;
  /** The agent will not do direct paths on this channel (refused the stream, closed it, or said `disabled`). */
  private unsupported = false;
  private headTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly reader = new MessageReader();
  private nextId = 0;
  private attempt: Attempt | null = null;
  private retry: ReturnType<typeof setTimeout> | null = null;
  private readonly timing: DirectTiming;
  /** How direct paths to this machine have fared: the one given (shared across the machine's channels), or its own. */
  readonly history: DirectHistory;
  private closed = false;
  private readonly offs: (() => void)[] = [];

  constructor(private readonly opts: DirectPathsOptions) {
    this.env = { ...browserEnv(), ...opts.env };
    this.timing = { ...DIRECT_TIMING, ...opts.timing };
    this.history = opts.history ?? new DirectHistory();
    this.history.backoffMs ??= this.timing.retryFirstMs;
    this.driver = opts.driver;
  }

  get state(): DirectState {
    return this.current;
  }

  /** Attempts so far on this channel. */
  get attempts(): number {
    return this.nextId;
  }

  start(): void {
    if (this.closed || this.offs.length > 0) return;
    this.offs.push(this.env.onAllowedChange(() => this.allowedChanged()));
    this.offs.push(this.env.onNetworkChange(() => this.networkChanged()));
    this.refresh();
  }

  /** Read the setting again: let a direct path go at once when it says hub only, start trying when it no longer does. */
  refresh(): void {
    if (this.closed) return;
    if (this.driver.isClosed) return this.close();
    if (!this.env.allowed()) {
      this.cancelRetry();
      if (this.attempt) this.end(this.attempt, "disabled", "direct paths are turned off on this device", "closed", false);
      this.setState("off");
      return;
    }
    if (this.unsupported) return this.setState("unsupported");
    if (this.attempt || this.retry) return;
    // A machine whose direct paths keep dying young: a new channel waits its
    // turn like any other attempt rather than starting the churn again.
    if (this.history.flapping) return this.scheduleRetry(false);
    this.history.backoffMs = this.timing.retryFirstMs;
    this.attemptNow();
  }

  /** The user turned direct paths on (or off): on, a fresh start — they asked for it. */
  private allowedChanged(): void {
    if (this.env.allowed()) {
      this.history.shortLived = 0;
      this.history.backoffMs = this.timing.retryFirstMs;
      this.cancelRetry();
    }
    this.refresh();
  }

  /** End everything (the channel is going). */
  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.cancelRetry();
    if (this.headTimer) clearTimeout(this.headTimer);
    for (const off of this.offs.splice(0)) off();
    if (this.attempt) this.end(this.attempt, this.driver.isClosed ? null : "done", "the channel closed", "closed", false);
    const signal = this.signal;
    this.signal = null;
    this.signalOpen = false;
    signal?.close(0);
    this.setState("closed");
  }

  private setState(state: DirectState): void {
    if (this.current === state) return;
    this.current = state;
    this.onstate?.(state);
  }

  private cancelRetry(): void {
    if (this.retry) clearTimeout(this.retry);
    this.retry = null;
  }

  /**
   * The network may have changed, or the page came back to the foreground:
   * try again at once — unless direct paths have been dying young, when the
   * backoff stands (on a phone the page comes and goes all the time, and
   * every new path that dies stalls what is in flight on it).
   */
  private networkChanged(): void {
    if (this.closed || this.attempt) return;
    if (this.history.flapping) {
      this.env.log(`${this.history.shortLived} direct paths in a row died young: not trying again before the backoff`);
      return;
    }
    this.history.backoffMs = this.timing.retryFirstMs;
    this.attemptNow();
  }

  /** Start an attempt now, if one may start. */
  private attemptNow(): void {
    this.cancelRetry();
    if (this.closed || this.attempt || this.unsupported || this.driver.isClosed || !this.env.allowed()) return;
    if (!this.signalOpen) return this.openSignal();
    let pc: PeerLike;
    try {
      pc = this.env.createPeer({ iceServers: STUN_SERVERS });
    } catch (error) {
      // No WebRTC in this browser (or it is turned off): the hub it is.
      this.env.log(`no peer connection: ${errorText(error)}`);
      this.unsupported = true;
      return this.setState("unsupported");
    }
    const dc = pc.createDataChannel(DC_LABEL, { ordered: true });
    dc.binaryType = "arraybuffer";
    const a: Attempt = {
      id: ++this.nextId,
      pc,
      dc,
      fp: null,
      offered: false,
      outbox: [],
      sentEnd: false,
      answer: null,
      remoteSet: false,
      inbound: [],
      initiator: null,
      replied: false,
      early: [],
      link: null,
      attachedAt: null,
      timer: null,
      done: false,
    };
    this.attempt = a;
    this.setState("connecting");
    const limit = this.timing.attemptTimeoutMs;
    a.timer = setTimeout(() => this.fail(a, "failed", `no direct path within ${limit / 1000} s`), limit);
    pc.onicecandidate = (ev) => this.ownCandidate(a, ev.candidate?.candidate || null);
    pc.onconnectionstatechange = () => {
      if (pc.connectionState === "failed" || pc.connectionState === "closed") this.fail(a, "failed", `the peer connection ${pc.connectionState}`);
    };
    dc.onopen = () => void this.opened(a);
    dc.onmessage = (ev) => this.dcMessage(a, ev.data);
    dc.onclose = () => this.fail(a, "failed", "the data channel closed");
    dc.onerror = () => this.fail(a, "failed", "the data channel failed");
    void this.offer(a);
  }

  // -- signaling --------------------------------------------------------------

  private openSignal(): void {
    if (this.signal) return;
    this.setState("signaling");
    const stream = this.driver.open("mux", encodeRequestHead({ method: "GET", path: SIGNAL_PATH, headers: [] }));
    this.signal = stream;
    stream.ondata = (data) => this.signalData(stream, data);
    stream.onclose = () => this.signalGone(stream, "the signaling stream closed");
    this.headTimer = setTimeout(() => {
      this.headTimer = null;
      if (!this.signalOpen) this.signalGone(stream, "the agent did not answer the signaling stream");
    }, this.timing.signalHeadTimeoutMs);
  }

  private signalData(stream: ChannelStream, data: Uint8Array): void {
    if (stream !== this.signal) return;
    this.reader.push(data);
    try {
      for (let m = this.reader.next(); m; m = this.reader.next()) {
        if (m.kind === "head") {
          const status = decodeResponseHead(m.payload).status;
          if (status !== 101) return this.signalGone(stream, `the agent answered the signaling stream with ${status}`);
          this.signalOpen = true;
          if (this.headTimer) clearTimeout(this.headTimer);
          this.headTimer = null;
          this.attemptNow();
        } else if (m.kind === "binary" && this.signalOpen) {
          let signal: Signal;
          try {
            signal = decodeSignal(m.payload);
          } catch (error) {
            this.env.log(`a signal that does not decode: ${errorText(error)}`);
            continue;
          }
          this.onSignal(signal);
        }
      }
    } catch (error) {
      this.signalGone(stream, `the signaling stream broke: ${errorText(error)}`);
    }
  }

  /** No direct paths on this channel any more. A path already attached stays until it fails. */
  private signalGone(stream: ChannelStream, why: string): void {
    if (stream !== this.signal || this.closed) return;
    this.env.log(why);
    this.signal = null;
    this.signalOpen = false;
    this.unsupported = true;
    if (this.headTimer) clearTimeout(this.headTimer);
    this.headTimer = null;
    stream.close(0);
    this.cancelRetry();
    if (this.attempt && !this.attempt.link) this.end(this.attempt, null, why, null, false);
    if (!this.attempt) this.setState(this.driver.isClosed ? "closed" : "unsupported");
  }

  private sendSignal(signal: Signal): boolean {
    if (!this.signal || !this.signalOpen) return false;
    this.signal.send(bridgeMessage("binary", encodeSignal(signal)));
    return true;
  }

  private onSignal(s: Signal): void {
    const a = this.attempt;
    // Another attempt's, or one already over.
    if (!a || a.done || s.id !== a.id) return;
    switch (s.type) {
      case "answer":
        void this.answered(a, s);
        break;
      case "candidate":
        if (a.remoteSet) this.addRemote(a, s.candidate);
        else a.inbound.push(s.candidate);
        break;
      case "bye":
        this.env.log(`the agent ended attempt ${a.id}: ${s.reason}${s.detail ? ` (${s.detail})` : ""}`);
        if (s.reason === "disabled") this.unsupported = true;
        // "done" and "disabled" are the agent's decision, not the path failing:
        // they do not count towards the streak of paths that died young.
        this.end(a, null, s.detail, "failed", s.reason !== "done" && s.reason !== "disabled");
        this.afterEnd();
        break;
      case "offer":
        break;
    }
  }

  // -- one attempt -----------------------------------------------------------

  private ownCandidate(a: Attempt, candidate: string | null): void {
    if (a.done) return;
    if (candidate === null) {
      if (a.sentEnd) return;
      a.sentEnd = true;
    }
    if (!a.offered) {
      a.outbox.push(candidate);
      return;
    }
    try {
      this.sendSignal({ type: "candidate", id: a.id, candidate });
    } catch {
      // a candidate line past the bound: the others may do
    }
  }

  private async offer(a: Attempt): Promise<void> {
    try {
      const offer = await a.pc.createOffer();
      await a.pc.setLocalDescription({ type: "offer", sdp: offer.sdp });
      if (a.done) return;
      const sdp = a.pc.localDescription?.sdp ?? offer.sdp ?? "";
      a.fp = sdpFingerprint(sdp);
      if (!this.sendSignal({ type: "offer", id: a.id, fp: a.fp, sdp: stripCandidates(sdp) })) throw new Error("the signaling stream is gone");
      a.offered = true;
      for (const c of a.outbox.splice(0)) this.ownCandidate(a, c);
    } catch (error) {
      this.fail(a, "failed", `no offer: ${errorText(error)}`);
    }
  }

  private async answered(a: Attempt, s: Extract<Signal, { type: "answer" }>): Promise<void> {
    if (a.answer) return;
    try {
      // The anti-MITM binding: the SDP must name the certificate stated next to it.
      checkFingerprint(s.sdp, s.fp);
    } catch (error) {
      return this.fail(a, "badFingerprint", errorText(error));
    }
    a.answer = { fp: s.fp, nonce: s.nonce };
    try {
      await a.pc.setRemoteDescription({ type: "answer", sdp: s.sdp });
    } catch (error) {
      return this.fail(a, "failed", `the answer was not taken: ${errorText(error)}`);
    }
    if (a.done) return;
    a.remoteSet = true;
    for (const c of a.inbound.splice(0)) this.addRemote(a, c);
  }

  private addRemote(a: Attempt, candidate: string | null): void {
    // An empty candidate is the end of them.
    const line = candidate === null ? "" : candidate.replace(/^a=/, "");
    a.pc.addIceCandidate({ candidate: line, sdpMLineIndex: 0 }).catch(() => undefined);
  }

  /** The DataChannel is open: the direct handshake. */
  private async opened(a: Attempt): Promise<void> {
    if (a.done) return;
    if (!a.answer || !a.fp) return this.fail(a, "failed", "the data channel opened before an answer");
    const answer = a.answer;
    // Defence in depth: the certificate DTLS saw is the one the answer stated.
    const cert = remoteCertificate(a.pc);
    if (cert) {
      const seen = new Uint8Array(await crypto.subtle.digest("SHA-256", cert));
      if (a.done) return;
      if (!equal(seen, answer.fp)) return this.fail(a, "badFingerprint", "the DTLS certificate is not the one the answer stated");
    }
    try {
      const binding: DirectBinding = { binding: this.opts.binding, relayHash: this.driver.handshakeHash, nonce: answer.nonce, deviceFp: a.fp, agentFp: answer.fp };
      const { initiator, message } = await this.opts.handshake.start(binding);
      if (a.done) return;
      a.initiator = initiator;
      a.dc.send(encodeDcMessage({ type: "hello", body: message }));
    } catch (error) {
      this.fail(a, "failed", `no direct handshake: ${errorText(error)}`);
    }
  }

  private dcMessage(a: Attempt, data: unknown): void {
    if (a.done) return;
    const bytes = messageBytes(data);
    if (!bytes) return this.fail(a, "failed", "a text message on the data channel");
    if (a.link) return this.driver.receiveDirect(bytes, a.link);
    if (!a.initiator) return this.fail(a, "failed", "a message before the handshake");
    if (!a.replied) {
      a.replied = true;
      void this.finish(a, bytes);
      return;
    }
    a.early.push(bytes);
  }

  /** The agent's reply: on `accepted`, the driver takes the path. */
  private async finish(a: Attempt, bytes: Uint8Array): Promise<void> {
    let transport: Transport;
    try {
      const m = decodeDcMessage(bytes);
      if (m.type !== "reply") throw new Error(`the first direct message is a ${m.type}, not a reply`);
      transport = await this.opts.handshake.finish(a.initiator!, m.body);
    } catch (error) {
      if (error instanceof HandshakeError && error.kind === "refused") {
        this.fail(a, "refused", error.message);
        void this.opts.handshake.refused?.(error).catch(() => undefined);
      } else {
        this.fail(a, "failed", `the direct handshake failed: ${errorText(error)}`);
      }
      return;
    }
    if (a.done) return;
    if (a.timer) clearTimeout(a.timer);
    a.timer = null;
    const link: DirectLink = {
      send: (message) => a.dc.send(message),
      bufferedAmount: () => a.dc.bufferedAmount,
      released: (reason) => this.released(a, reason),
    };
    a.link = link;
    a.attachedAt = this.env.now();
    a.dc.bufferedAmountLowThreshold = DIRECT_BUDGET / 2;
    a.dc.onbufferedamountlow = () => this.driver.wake();
    this.driver.attachDirect(transport, link);
    for (const m of a.early.splice(0)) this.driver.receiveDirect(m, link);
    this.env.log(`attempt ${a.id} carries the channel`);
    this.setState("direct");
  }

  /** The driver stopped using the path (it timed out, the peer gave it up, a message did not open, the channel ended). */
  private released(a: Attempt, reason: DownReason): void {
    if (a.done) return;
    this.env.log(`attempt ${a.id} let go: ${reason}`);
    // "closed": the channel ended, or another path replaced this one — not a failure of the path.
    this.end(a, this.driver.isClosed ? null : reason === "closed" ? "done" : "failed", `the direct path ${reason === "timeout" ? "timed out" : reason}`, null, reason !== "closed");
    this.afterEnd();
  }

  private fail(a: Attempt, bye: ByeReason, detail: string): void {
    if (a.done) return;
    this.env.log(`attempt ${a.id}: ${detail}`);
    this.end(a, bye, detail, "failed");
    this.afterEnd();
  }

  /**
   * Tear an attempt down: tell the agent (`bye`), let the driver drop its path
   * (`detach`), close the peer connection. `failed`: it ended because the path
   * did not hold, not because this side was done with it — which, for a path
   * that carried the channel and died young, counts towards the streak.
   */
  private end(a: Attempt, bye: ByeReason | null, detail: string, detach: DownReason | null, failed = true): void {
    if (a.done) return;
    a.done = true;
    if (a.timer) clearTimeout(a.timer);
    a.timer = null;
    if (bye) {
      try {
        this.sendSignal({ type: "bye", id: a.id, reason: bye, detail: detail.slice(0, 200) });
      } catch {
        // a detail past the bound: the agent times the attempt out instead
      }
    }
    if (a.link && detach) this.driver.detachDirect(detach, a.link);
    a.pc.onicecandidate = null;
    a.pc.onconnectionstatechange = null;
    a.dc.onopen = null;
    a.dc.onmessage = null;
    a.dc.onclose = null;
    a.dc.onerror = null;
    a.dc.onbufferedamountlow = null;
    try {
      a.dc.close();
    } catch {
      // already closed
    }
    try {
      a.pc.close();
    } catch {
      // already closed
    }
    if (this.attempt === a) this.attempt = null;
    if (a.attachedAt === null) return;
    if (this.env.now() - a.attachedAt >= this.timing.stableMs) {
      this.history.shortLived = 0;
      this.history.backoffMs = this.timing.retryFirstMs;
    } else if (failed) {
      this.history.shortLived += 1;
    }
  }

  /** After an attempt ended: wait, then try again (unless trying is over). */
  private afterEnd(): void {
    if (this.closed) return;
    if (this.driver.isClosed) return this.close();
    if (!this.env.allowed()) return this.setState("off");
    if (this.unsupported) return this.setState("unsupported");
    this.scheduleRetry(true);
  }

  /** The next attempt after the backoff (jittered); `grow`: the one after that waits twice as long. */
  private scheduleRetry(grow: boolean): void {
    this.cancelRetry();
    const backoff = this.history.backoffMs ?? this.timing.retryFirstMs;
    const delay = Math.min(this.timing.retryMaxMs, Math.round(backoff * (0.8 + 0.4 * this.env.random())));
    if (grow) this.history.backoffMs = Math.min(backoff * 2, this.timing.retryMaxMs);
    this.retry = setTimeout(() => {
      this.retry = null;
      this.attemptNow();
    }, delay);
    this.setState("waiting");
  }
}
