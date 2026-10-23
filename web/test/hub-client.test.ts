// The cloud client end to end, with the hub and the agent played in-process:
// `HubApi` + `CloudAccount` against a fake hub's HTTP, `HubLink` against a
// fake `/v1/client` socket, `MachineChannel` handshaking with a TypeScript
// agent (Noise responder, `ChannelDriver`, a bridge that answers rpc and
// echoes sockets), and the tunnel transport running on top.
//
// What these pin: sign-in answers the hub's challenge with a HubLogin the hub
// verifies under the device key, for the origin the page dialled; genesis,
// enrollment (the agent's proof is checked before "add machine" is signed) and
// the sealed machine key reach the hub in the shapes it stores; a channel is
// opened only after an attestation, retried when the agent says "stale"; typed
// fetches and sockets run over it; and a hub restart (1012) is survived — the
// link signs in again and the next request re-handshakes. The hello says
// `web/1 paths/1`: an older agent gets plain frames, one listing `paths/1`
// sequenced frames and a measured hub path, and a direct path is asked of it
// only when the device allows one.
import assert from "node:assert/strict";
import test from "node:test";
import {
  bridgeMessage,
  type Bytes,
  ChannelDriver,
  type ChannelStream,
  decodeAttestation,
  decodeEntry,
  decodeFrame,
  decodeHelloBody,
  decodeRequestHead,
  DEFAULT_REKEY,
  encodeFrame,
  encodeReplyBody,
  encodeResponseHead,
  enrollProof,
  type EnrollCode,
  type Frame,
  fromHex,
  entryHash,
  generateDeviceKeys,
  type HelloBody,
  hex,
  MemoryKeyStore,
  MessageReader,
  type NoiseKeyPair,
  openFrom,
  type ReplyBody,
  Responder,
  signDraft,
  utf8,
  fromUtf8,
  verifyHubLogin,
} from "../src/lib/e2e/index.js";
import { HubApi } from "../src/lib/hub/api.js";
import { CloudAccount } from "../src/lib/hub/account.js";
import { deliverMachineKey, linkCourier, mailMachineKey } from "../src/lib/hub/deliver.js";
import { HubLink, LINK_LOST } from "../src/lib/hub/link.js";
import { SHORT_LIVED_LIMIT } from "../src/lib/transport/p2p.js";
import { AGENT_UNSAVED, MachineChannel } from "../src/lib/hub/machine.js";
import { type RelayDirectory, type RelayEntry, RelayPool } from "../src/lib/hub/relays.js";
import { RelayLink } from "../src/lib/hub/relay-link.js";
import { tunnelTransport } from "../src/lib/transport/tunnel.js";
import { ModeController, SwitchingTransport } from "../src/features/cloud/cloud-transport.js";

const ORIGIN = "wss://hub.test";
/** The relay channels go through when one can carry them (relay-and-plans §2.15). */
const RELAY_ORIGIN = "wss://r-1.test";
const R1: RelayEntry = { id: "r-1", city: "singapore", region: "sin", origin: RELAY_ORIGIN, state: "up", load: 0 };
/** A second relay, where the account's set moves when `r-1` drains (relay-and-plans §2.7). */
const RELAY2_ORIGIN = "wss://r-2.test";
const R2: RelayEntry = { id: "r-2", city: "singapore", region: "sin", origin: RELAY2_ORIGIN, state: "up", load: 0 };
/** The hub's close for an `OPEN` while it carries no channels (`RH_HUB_CHANNEL_RELAY=off`). */
const HUB_NO_CHANNELS = 4426;
const flush = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

/** Wait for `cond`; `ms` only keeps a broken test from hanging (it is no claim about how fast). */
async function until(cond: () => boolean, ms = 10_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!cond()) {
    if (Date.now() > deadline) throw new Error("timed out waiting");
    await flush(2);
  }
}

/** `promise`, or a failure naming `what` once `ms` have gone by (a guard against hanging, not a claim about how fast). */
async function within<T>(promise: Promise<T>, what: string, ms = 10_000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`timed out: ${what}`)), ms);
  });
  try {
    return await Promise.race([promise, late]);
  } finally {
    clearTimeout(timer);
  }
}

/** What a channel came over: the hub's own connection (`hub`), or a relay's (`relay`, `relay2`). */
type CarrierKind = string;

/** The hub's HTTP, enough of it: devices, device sessions, chain, enrollments, mailbox, keyring. */
class FakeHubHttp {
  devices = new Map<string, { sign: string; dh: string }>();
  chain: string[] = [];
  enrollments = new Map<string, { device: string; machine?: string; sign?: Bytes; dh?: Bytes; proof?: Bytes }>();
  mail: Record<string, unknown>[] = [];
  keys = new Map<string, string>();
  sessions: { device: string; signed: boolean }[] = [];
  nonces = new Map<string, string>();

  fetch = async (input: string, init?: RequestInit): Promise<Response> => {
    const url = new URL(input, "https://hub.test");
    const body = init?.body ? JSON.parse(init.body as string) : null;
    const json = (status: number, value: unknown) => new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
    if (url.pathname === "/v1/devices") {
      const id = `d-${this.devices.size + 1}`;
      this.devices.set(id, { sign: body.sign_key, dh: body.dh_key });
      return json(200, { device: id });
    }
    if (url.pathname === "/v1/session/challenge") {
      const nonce = hex(crypto.getRandomValues(new Uint8Array(32)));
      this.nonces.set(nonce, body.device);
      return json(200, { nonce });
    }
    if (url.pathname === "/v1/session") {
      const reg = this.devices.get(body.device);
      const signed =
        !!reg &&
        this.nonces.get(body.nonce) === body.device &&
        (await verifyHubLogin({ role: "deviceSession", account: body.account, id: body.device, hub: ORIGIN, nonce: fromHex(body.nonce), timeMs: body.time_ms }, fromHex(reg.sign), fromHex(body.signature)));
      this.nonces.delete(body.nonce);
      this.sessions.push({ device: body.device, signed });
      if (!signed) return json(401, { error: "denied", message: "bad signature" });
      return json(200, { account: body.account, device: body.device, token: `dev-${body.device}`, expires_ms: Date.now() + 3_600_000 });
    }
    if (url.pathname === "/v1/keys" && (init?.method ?? "GET") === "GET") {
      return json(200, { keys: [...this.keys].map(([name, blob]) => ({ name, blob, updated_ms: 1 })) });
    }
    if (url.pathname.startsWith("/v1/keys/")) {
      this.keys.set(url.pathname.slice("/v1/keys/".length), body.blob);
      return json(200, { name: url.pathname.slice("/v1/keys/".length), stored: true });
    }
    if (url.pathname === "/v1/mailbox" && (init?.method ?? "GET") === "GET") return json(200, { mail: [] });
    // `/v1/me`, as much as creating the account reads: a hub that takes chain v3 entries.
    if (url.pathname === "/v1/me") {
      const seq = this.chain.length ? this.chain.length - 1 : null;
      return json(200, { account: "a-1", email: { address: "someone@example.test", verified: true, verification: "off" }, plan: null, chain_seq: seq, chain_v3: { mode: "write", writable: true, waiting: [], in_use: seq !== null }, devices: [], machines: [] });
    }
    if (url.pathname === "/v1/chain" && init?.method === "POST") {
      this.chain.push(...(body.entries as string[]));
      return json(200, { seq: this.chain.length - 1 });
    }
    if (url.pathname === "/v1/chain") {
      const from = Number(url.searchParams.get("from") ?? 0);
      return json(200, { entries: this.chain.slice(from), seq: this.chain.length ? this.chain.length - 1 : null });
    }
    if (url.pathname === "/v1/enrollments") {
      this.enrollments.set(body.token, { device: body.device });
      return json(200, { expires_ms: Date.now() + 600_000 });
    }
    if (url.pathname.startsWith("/v1/enrollments/")) {
      const e = this.enrollments.get(url.pathname.split("/").pop()!);
      if (!e) return json(404, { error: "not_found" });
      if (!e.proof) return json(200, { state: "waiting" });
      return json(200, { state: "proved", machine: e.machine, sign_key: hex(e.sign!), dh_key: hex(e.dh!), proof: hex(e.proof) });
    }
    if (url.pathname === "/v1/mailbox") {
      this.mail.push(body);
      return json(200, { id: 1 });
    }
    return json(404, { error: "not_found" });
  };
}

/** A machine: its keys, and a TypeScript stand-in for the agent's handshake and bridge. */
class FakeAgent {
  attestations = 0;
  ignoreAttestations = 0;
  handshakes: string[] = [];
  /** What each hello's client info said. */
  clients: string[] = [];
  /** Its features, as its server info lists them; `null`: an older agent (no JSON). */
  features: string[] | null = null;
  /** The requests its bridge saw. */
  requests: string[] = [];
  /** What each handshake came over. */
  carriers: CarrierKind[] = [];
  /** What its next replies say otherwise (another chain view, entries it teaches, a refusal). */
  shapeReply: ((hello: HelloBody) => Partial<ReplyBody> | undefined) | null = null;
  private drivers = new Map<string, ChannelDriver>();
  constructor(
    readonly id: string,
    readonly sign: { publicKey: Bytes },
    readonly dh: NoiseKeyPair,
    private readonly sends: Record<CarrierKind, (f: Frame) => void>,
    readonly account: string,
  ) {}

  static async create(id: string, account: string, sends: Record<CarrierKind, (f: Frame) => void>): Promise<FakeAgent> {
    const keys = await generateDeviceKeys();
    return new FakeAgent(id, { publicKey: keys.signPublic }, { privateKey: keys.dh.privateKey, publicKey: keys.dhPublic }, sends, account);
  }

  /** A frame for it, over `via` (its answers go back the same way). */
  async onFrame(frame: Frame, device: string, via: CarrierKind): Promise<void> {
    if (frame.type === "attest") {
      if (this.ignoreAttestations > 0) this.ignoreAttestations--;
      else this.attestations++;
      return;
    }
    if (frame.type === "open") return;
    const key = `${via}:${frame.ch}`;
    const driver = this.drivers.get(key);
    if (driver) return driver.receive(frame);
    if (frame.type !== "data") return;
    const send = this.sends[via];
    const r = await Responder.create(this.dh, { account: this.account, device, machine: this.id });
    const hello = await r.readHello(frame.payload);
    const body = decodeHelloBody(hello.payload);
    this.clients.push(fromUtf8(body.client));
    const stale = this.attestations === 0;
    this.handshakes.push(stale ? "stale" : "open");
    this.carriers.push(via);
    const server = this.features ? utf8(JSON.stringify({ agent: "fake", version: "0", features: this.features })) : utf8("fake agent");
    const shaped: ReplyBody = { status: stale ? "stale" : "accepted", chainSeq: body.chainSeq, chainHead: body.chainHead, needFrom: 0, server, entries: [], ...this.shapeReply?.(body) };
    const reply = encodeReplyBody(shaped);
    const { message, transport } = await r.writeReply(reply, DEFAULT_REKEY, Date.now());
    send({ type: "data", ch: frame.ch, payload: message });
    if (shaped.status !== "accepted") {
      send({ type: "close", ch: frame.ch, code: 4412, reason: "stale" });
      return;
    }
    const paths = this.features?.includes("paths/1") ? { paths: { probeRelay: false } } : {};
    const d = new ChannelDriver(frame.ch, transport, "responder", send, Date.now, paths);
    d.onincoming = (stream) => this.serve(stream);
    this.drivers.set(key, d);
  }

  /** The bridge: rpc answers with what it was asked; sockets open and echo. */
  private serve(stream: ChannelStream): void {
    const head = decodeRequestHead(stream.meta);
    this.requests.push(`${head.method} ${head.path}`);
    if (head.path === "/rh/direct") {
      // Like the real agent with direct paths turned off.
      stream.send(bridgeMessage("head", encodeResponseHead({ status: 403, headers: [] })));
      stream.close(4403);
      return;
    }
    const reader = new MessageReader();
    const body: number[] = [];
    let opened = false;
    stream.ondata = (data) => {
      reader.push(data);
      for (let m = reader.next(); m; m = reader.next()) {
        if (stream.cls === "rpc") {
          if (m.kind === "binary") body.push(...m.payload);
          if (m.kind === "end") {
            const answer = utf8(JSON.stringify({ method: head.method, path: head.path, body: fromUtf8(Uint8Array.from(body)) }));
            stream.send(bridgeMessage("head", encodeResponseHead({ status: 200, headers: [["content-type", "application/json"]] })));
            stream.send(bridgeMessage("binary", answer));
            stream.close(0);
          }
        } else if (m.kind === "text") stream.send(bridgeMessage("text", utf8(`echo:${fromUtf8(m.payload)}`)));
      }
    };
    if (stream.cls !== "rpc" && !opened) {
      opened = true;
      stream.send(bridgeMessage("head", encodeResponseHead({ status: 101, headers: [] })));
    }
  }

  dropChannels(): void {
    this.drivers.clear();
  }

  /** Where its answers to what came over `via` go. */
  route(via: CarrierKind, send: (f: Frame) => void): void {
    this.sends[via] = send;
  }
}

/** The hub's `/v1/client` socket, played by the test. */
class FakeSocket {
  binaryType = "blob";
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  readonly nonce = crypto.getRandomValues(new Uint8Array(32));
  signedIn = false;

  constructor(private readonly hub: FakeHub) {
    queueMicrotask(() => this.text({ t: "challenge", nonce: hex(this.nonce), hub: "wss://evil.example", version: 1 }));
  }

  text(value: unknown): void {
    this.onmessage?.({ data: JSON.stringify(value) });
  }

  frame(f: Frame): void {
    this.onmessage?.({ data: encodeFrame(f).slice().buffer });
  }

  send(data: string | Uint8Array): void {
    void this.hub.received(this, data);
  }

  close(code = 1000): void {
    this.onclose?.({ code });
  }
}

class FakeHub {
  sockets: FakeSocket[] = [];
  agent!: FakeAgent;
  device = "";
  deviceSignKey = new Uint8Array();
  account = "";
  /** Whether it carries channels (`RH_HUB_CHANNEL_RELAY`, on by default); off, an `OPEN` gets 4426 and channel frames are dropped. */
  carries = true;
  /** Set: it refuses every `OPEN` with 4403 and this reason, as `may_route` does for a device the machine does not accept. */
  refuseOpens: string | null = null;
  /** The close code for a refused `OPEN` (4403: not accepted; 4404: the device is offline). */
  refuseCode = 4403;
  /** The machine each `OPEN` it saw named. */
  opens: string[] = [];
  /** What presence says of the machine besides "online": the relays it is on, and whether its agent speaks `relays/1`. */
  machine: { via: { relay: string; rtt_ms: number }[]; relays?: boolean } = { via: [{ relay: "r-1", rtt_ms: 1 }], relays: true };
  /** Whether it says where the machines are right after a sign-in's `welcome` (off: only when the test calls {@link announce}). */
  presenceOnSignIn = true;
  /** Sign-ins it refuses (4401, a device session that ran out) before it takes one again. */
  refuseSignIns = 0;
  /** Agent → page frames, mapped back onto the page's ch (they are the same here). */
  constructor(readonly http: FakeHubHttp) {}

  make = (): FakeSocket => {
    const s = new FakeSocket(this);
    this.sockets.push(s);
    return s;
  };

  get live(): FakeSocket {
    return this.sockets.at(-1)!;
  }

  /** Presence, as the hub sends it after a sign-in and on every change. */
  announce(socket: FakeSocket = this.live): void {
    socket.text({ t: "presence", machines: [{ id: this.agent.id, state: "online", since_ms: 1, ...this.machine }] });
  }

  async received(socket: FakeSocket, data: string | Uint8Array): Promise<void> {
    if (typeof data === "string") {
      const m = JSON.parse(data);
      assert.equal(m.t, "login");
      const ok = await verifyHubLogin(
        { role: "device", account: m.account, id: m.id, hub: ORIGIN, nonce: socket.nonce, timeMs: m.time_ms },
        this.deviceSignKey,
        fromHex(m.sig),
      );
      assert.ok(ok, "the sign-in names the origin the page dialled, signed by the device key");
      assert.equal(m.session, `dev-${this.device}`, "and carries this device's own session");
      if (this.refuseSignIns > 0) {
        this.refuseSignIns--;
        return socket.close(4401);
      }
      socket.signedIn = true;
      socket.text({ t: "welcome", account: m.account, id: m.id, chain_seq: 1 });
      if (this.presenceOnSignIn) this.announce(socket);
      return;
    }
    const frame = decodeFrame(data);
    if (frame.type === "ping") return socket.frame({ type: "pong", nonce: frame.nonce });
    if (frame.type === "attest") {
      assert.equal(decodeAttestation(frame.attestation).device, this.device);
      return this.agent.onFrame(frame, this.device, "hub");
    }
    if (frame.type === "open") {
      this.opens.push(frame.peer);
      if (!this.carries) {
        queueMicrotask(() => socket.frame({ type: "close", ch: frame.ch, code: HUB_NO_CHANNELS, reason: "channels go through a relay" }));
        return;
      }
      if (this.refuseOpens !== null) {
        const reason = this.refuseOpens;
        const code = this.refuseCode;
        queueMicrotask(() => socket.frame({ type: "close", ch: frame.ch, code, reason }));
        return;
      }
    }
    // Switched off, it drops DATA, CREDIT and CLOSE.
    if (this.carries) await this.agent.onFrame(frame, this.device, "hub");
  }
}

/** A relay (`/v1/connect`), played by the test: it signs the device in on its ticket and hands frames to the agent. */
class FakeRelay {
  constructor(
    readonly id = "r-1",
    readonly origin = RELAY_ORIGIN,
    /** How the agent knows what came over it. */
    readonly via: CarrierKind = "relay",
  ) {}
  sockets: FakeRelaySocket[] = [];
  /** Unreachable: every connection to it closes before its challenge. */
  down = false;
  device = "";
  deviceSignKey = new Uint8Array();
  agent!: FakeAgent;

  make = (): FakeRelaySocket => {
    const s = new FakeRelaySocket(this);
    this.sockets.push(s);
    return s;
  };

  get live(): FakeRelaySocket {
    return this.sockets.at(-1)!;
  }

  async received(socket: FakeRelaySocket, data: string | Uint8Array): Promise<void> {
    if (typeof data === "string") {
      const m = JSON.parse(data);
      if (m.t !== "login") return;
      assert.equal(m.ticket, `TICKET-${this.id}`);
      const ok = await verifyHubLogin(
        { role: "relayDevice", account: this.agent.account, id: this.device, hub: this.origin, nonce: socket.nonce, timeMs: m.time_ms },
        this.deviceSignKey,
        fromHex(m.sig),
      );
      assert.ok(ok, "a relay sign-in: the device key over the relay's own origin");
      socket.text({ t: "welcome", relay: this.id, id: this.device, expires_ms: Date.now() + 900_000 });
      socket.text({ t: "presence", machines: [{ id: this.agent.id, state: "online" }] });
      return;
    }
    const frame = decodeFrame(data);
    if (frame.type === "ping") return socket.frame({ type: "pong", nonce: frame.nonce });
    await this.agent.onFrame(frame, this.device, this.via);
  }
}

class FakeRelaySocket {
  binaryType = "blob";
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code?: number; reason?: string }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  readonly nonce = crypto.getRandomValues(new Uint8Array(32));

  constructor(private readonly relay: FakeRelay) {
    queueMicrotask(() => (relay.down ? this.close(1006) : this.text({ t: "challenge", nonce: hex(this.nonce), relay: relay.id, origin: relay.origin, version: 1 })));
  }

  text(value: unknown): void {
    this.onmessage?.({ data: JSON.stringify(value) });
  }

  frame(f: Frame): void {
    this.onmessage?.({ data: encodeFrame(f).slice().buffer });
  }

  send(data: string | Uint8Array): void {
    void this.relay.received(this, data);
  }

  close(code = 1000): void {
    this.onclose?.({ code });
  }
}

/** The hub's relay list: `r-1` (or what a test lists), in the account's set. */
function relayDirectory(relays: RelayEntry[] = [R1]): RelayDirectory {
  return { relays, limit: 2, accountSet: [{ id: "r-1", lastUsedMs: Date.now() }], emergency: [], agents: {}, throttled: false };
}

/**
 * The page's relay pool, with the relays played by `relay` (and `others`)
 * (the list as `directory` has it when asked; `tickets` counts the tickets
 * asked for).
 */
function relayPool(
  account: CloudAccount,
  relay: FakeRelay,
  directory: RelayDirectory = relayDirectory(),
  tickets = { count: 0 },
  others: FakeRelay[] = [],
  /** How long its tickets last (15 minutes, as the hub's). */
  ticketMs = 900_000,
): RelayPool {
  const played = (id: string) => [relay, ...others].find((r) => r.id === id) ?? relay;
  const api = {
    relays: async () => structuredClone(directory),
    relayTickets: async (_token: string, ids: string[]) => {
      tickets.count++;
      return {
        tickets: ids.map((id) => ({ relay: id, origin: played(id).origin, ticket: `TICKET-${id}`, expiresMs: Date.now() + ticketMs, emergency: false, throttled: false })),
        errors: [],
      };
    },
  };
  return new RelayPool({
    api: api as never,
    account,
    storage: null,
    fetchImpl: async () => new Response(null, { status: 204 }),
    makeLink: (entry, ticket) =>
      new RelayLink({ relay: entry.id, origin: entry.origin, account: account.account, deviceId: account.deviceId, signKey: account.keys.sign.privateKey, ticket, makeSocket: played(entry.id).make as never }),
  });
}

async function setUp() {
  const http = new FakeHubHttp();
  const api = new HubApi("https://hub.test", http.fetch);
  const store = new MemoryKeyStore();
  const session = { account: "a-test", token: "login-tok", expiresMs: Date.now() + 60_000 };
  const account = await CloudAccount.open(api, session, store, { origin: ORIGIN });
  assert.deepEqual(http.sessions, [{ device: account.deviceId, signed: true }], "a device session, minted by the device key's signature for this hub");
  assert.equal(account.session.token, `dev-${account.deviceId}`);
  const hub = new FakeHub(http);
  hub.account = account.account;
  hub.device = account.deviceId;
  hub.deviceSignKey = account.keys.signPublic;
  const relay = new FakeRelay();
  relay.device = account.deviceId;
  relay.deviceSignKey = account.keys.signPublic;
  // The agent answers each channel the way it came: through the relay, or on the hub.
  hub.agent = await FakeAgent.create("m-1", account.account, {
    hub: (f) => queueMicrotask(() => hub.live.frame(f)),
    relay: (f) => queueMicrotask(() => relay.live.frame(f)),
  });
  relay.agent = hub.agent;
  return { http, api, store, account, hub, relay };
}

/** A set-up account with `m-1` enrolled, and the page's hub link to it, signed in. */
async function linked(t: { after(fn: () => void): void }) {
  const set = await setUp();
  const { http, account, hub } = set;
  await account.createGenesis(false);
  const code = await account.startEnrollment();
  await enroll(account, http, hub.agent, code);
  await account.completeEnrollment(code, undefined, 5, undefined, { writable: true });
  const link = new HubLink({
    url: "wss://hub.test/v1/client",
    origin: ORIGIN,
    account: account.account,
    deviceId: account.deviceId,
    token: () => account.session.token,
    signKey: account.keys.sign.privateKey,
    // A sign-in refused (4401) renews the device session first, as the page's.
    renew: () => account.renewSession(),
    makeSocket: hub.make,
    random: () => 0,
  });
  t.after(() => link.stop());
  link.start();
  await link.waitOpen(10_000);
  return { ...set, link };
}

/** An rpc over `channel` that the fake agent's bridge echoes. */
async function echo(channel: MachineChannel, body: string): Promise<unknown> {
  const res = await tunnelTransport(channel).fetch(new Request("http://machine/api/sessions", { method: "POST", body }));
  return res.json();
}

async function enroll(account: CloudAccount, http: FakeHubHttp, agent: FakeAgent, code: EnrollCode): Promise<void> {
  const token = [...http.enrollments.keys()][0]!;
  const proof = await enrollProof(code, account.account, agent.id, agent.sign.publicKey, agent.dh.publicKey);
  http.enrollments.set(token, { device: account.deviceId, machine: agent.id, sign: agent.sign.publicKey, dh: agent.dh.publicKey, proof });
}

test("genesis, enrollment with a checked proof, and a sealed machine key", async () => {
  const { http, account, hub, store } = await setUp();
  assert.equal(account.chain, null, "a new account has no chain");
  const words = await account.createGenesis(true);
  assert.equal(words!.length, 12, "twelve recovery words to write down");
  assert.equal(http.chain.length, 2, "genesis, and user key epoch 0 committed with it");
  // Chain v3 from the first entry: a new account has nothing to upgrade.
  const genesis = decodeEntry(fromHex(http.chain[0]!));
  assert.equal(genesis.draft.version, 3);
  assert.equal(genesis.draft.action.type, "genesis3");
  assert.ok(genesis.draft.action.type === "genesis3" && genesis.draft.action.recoveryKey, "RSK's public half is in genesis");
  assert.ok(genesis.draft.action.type === "genesis3" && genesis.draft.action.device.flags.admin && !genesis.draft.action.device.flags.host, "the first device manages the account and runs no AI");
  const commit = decodeEntry(fromHex(http.chain[1]!));
  assert.equal(commit.draft.version, 3);
  assert.ok(commit.draft.action.type === "keyEpochs" && commit.draft.action.user?.epoch === 0 && commit.draft.action.machines.length === 0);
  assert.ok(account.chain?.isV3 && account.chain.v3From === 0);
  assert.ok(account.inChain);
  assert.ok(account.pinned);

  // A forged proof is refused and nothing is signed.
  const code = await account.startEnrollment();
  const token = [...http.enrollments.keys()][0]!;
  http.enrollments.set(token, { device: account.deviceId, machine: "m-evil", sign: hub.agent.sign.publicKey, dh: hub.agent.dh.publicKey, proof: new Uint8Array(32) });
  await assert.rejects(account.completeEnrollment(code, undefined, 5, undefined, { writable: true }), { name: "AccountError" });
  assert.equal(http.chain.length, 2);

  await enroll(account, http, hub.agent, code);
  const machine = await account.completeEnrollment(code, undefined, 5, undefined, { writable: true });
  assert.equal(machine.id, "m-1");
  assert.equal(http.chain.length, 4, "the device that runs AI added, and its key epoch 0 committed with it");
  const added = decodeEntry(fromHex(http.chain[2]!)).draft.action;
  assert.ok(added.type === "addDevice3" && added.device.flags.host && !added.device.flags.admin, "a host that does not manage the account (D1)");
  assert.equal(decodeEntry(fromHex(http.chain[3]!)).draft.action.type, "keyEpochs");
  assert.ok(http.keys.has(`mk/m-1/0/0/${account.deviceId}`), "its blob put (under a name ending with this device) before anything hands it over");

  const sent = crypto.getRandomValues(new Uint8Array(32));
  await mailMachineKey(account.api, account.session.token, account.account, account.deviceId, account.noiseKeys, "m-1", hub.agent.dh.publicKey, 0, sent);
  const mail = http.mail[0] as { recipient: string; sender: string; epoch: number; sealed: string };
  assert.equal(mail.recipient, "m-1");
  const mk = await openFrom(hub.agent.dh, account.keys.dhPublic, { purpose: "machineKey", account: account.account, sender: account.deviceId, recipient: "m-1", epoch: 0 }, fromHex(mail.sealed));
  assert.equal(hex(mk), hex(sent), "the agent opens it from this device's key");
  assert.equal(await store.get("mk/m-1/0"), undefined, "no machine key kept as bytes");
});

test("the page reaches a machine through a relay: attest on the hub, handshake on the relay (retrying a stale view), rpc, sockets, restarts", async (t) => {
  const { http, account, hub, relay } = await setUp();
  await account.createGenesis(false);
  const code = await account.startEnrollment();
  await enroll(account, http, hub.agent, code);
  await account.completeEnrollment(code, undefined, 5, undefined, { writable: true });

  const link = new HubLink({
    url: "wss://hub.test/v1/client",
    origin: ORIGIN,
    account: account.account,
    deviceId: account.deviceId,
    token: () => account.session.token,
    signKey: account.keys.sign.privateKey,
    makeSocket: hub.make,
    random: () => 0,
  });
  t.after(() => link.stop());
  link.start();
  await link.waitOpen(10_000);
  assert.equal(link.presence.get("m-1")?.state, "online");

  // The first attestation is "lost": the agent answers stale, the page re-attests.
  hub.agent.ignoreAttestations = 1;
  const pool = relayPool(account, relay);
  t.after(() => pool.stop());
  const channel = new MachineChannel(link, account, "m-1", { relays: pool });
  const transport = tunnelTransport(channel);
  const res = await transport.fetch(new Request("http://machine/api/sessions?scope=all", { method: "POST", body: "hi" }));
  assert.deepEqual(await res.json(), { method: "POST", path: "/api/sessions?scope=all", body: "hi" });
  assert.deepEqual(hub.agent.handshakes, ["stale", "open"]);
  assert.deepEqual(hub.agent.clients, ["web/1 paths/1 streams/2", "web/1 paths/1 streams/2"], "the page says it can sequence frames and reads streams named per key epoch");
  assert.equal(channel.status, "open");
  assert.equal(channel.paths, null, "an older agent: plain frames, as before");

  const socket = transport.openSocket("/api/terminals/t1/ws");
  const heard: unknown[] = [];
  await new Promise<void>((resolve) => {
    socket.onopen = () => {
      socket.send('{"type":"input","data":"x"}');
    };
    socket.onmessage = (e) => {
      heard.push(e.data);
      resolve();
    };
  });
  assert.deepEqual(heard, ['echo:{"type":"input","data":"x"}']);

  assert.equal(channel.route?.relay.id, "r-1", "through the relay");

  // The hub restarts: its socket closes with 1012 and signs in again by
  // itself — the channel, on the relay, does not notice.
  let lost = 0;
  let heardLoss = () => undefined as void;
  const loss = new Promise<void>((resolve) => (heardLoss = resolve));
  socket.onclose = () => {
    lost++;
    heardLoss();
  };
  hub.live.close(1012);
  await flush(5);
  assert.equal(lost, 0, "a hub restart leaves the relay's channel alone");
  await link.waitOpen(10_000);
  assert.equal((await transport.fetch(new Request("http://machine/api/sessions"))).status, 200);
  assert.deepEqual(hub.agent.handshakes, ["stale", "open"], "no new handshake");
  assert.equal(lost, 0, "and the socket on it is still open");

  // The relay goes away: the channel is gone, and the next request opens a
  // new one — even one made at once, while the old channel is still working
  // through what came before its close (it is never sent on the old one).
  hub.agent.dropChannels();
  relay.live.close(1012);
  const again = await transport.fetch(new Request("http://machine/api/sessions"));
  assert.equal(again.status, 200);
  assert.equal(relay.sockets.length, 2);
  assert.deepEqual(hub.agent.handshakes, ["stale", "open", "open"]);
  await within(loss, `the socket hears the channel go (${LINK_LOST})`);
  assert.equal(lost, 1, `the socket heard the channel go (${LINK_LOST})`);
});

/**
 * A relay deploy, after the cut-over: the relay the channel is on says
 * `goaway`; the Hub moves the account's set to another relay and the agent
 * signs in there a moment later (here 2.5 s, slower than a first try waits);
 * the page moves the open channel there by itself, make-before-break —
 * every request made meanwhile is answered, on the old channel until the new
 * one is in place.
 */
test("a relay that drains: the open channel moves to the relay the machine moves to, nothing asked of the user, nothing lost", async (t) => {
  const { link, account, hub, relay } = await linked(t);
  hub.carries = false;
  const relay2 = new FakeRelay("r-2", RELAY2_ORIGIN, "relay2");
  relay2.device = account.deviceId;
  relay2.deviceSignKey = account.keys.signPublic;
  relay2.agent = hub.agent;
  hub.agent.route("relay2", (f) => queueMicrotask(() => relay2.live.frame(f)));
  const pool = relayPool(account, relay, relayDirectory([R1, R2]), { count: 0 }, [relay2]);
  t.after(() => pool.stop());
  const channel = new MachineChannel(link, account, "m-1", { relays: pool });
  assert.deepEqual(await echo(channel, "before"), { method: "POST", path: "/api/sessions", body: "before" });
  assert.equal(channel.route?.relay.id, "r-1", "where the machine is (presence: r-1)");

  const drained = Date.now();
  relay.live.text({ t: "goaway", retry_ms: 30_000, reason: "restart" });
  const moved = setTimeout(() => {
    hub.machine = { via: [{ relay: "r-1", rtt_ms: 1 }, { relay: "r-2", rtt_ms: 1 }], relays: true };
    hub.announce();
  }, 2500);
  t.after(() => clearTimeout(moved));
  // Requests keep going while it moves: every one answered.
  let answered = 0;
  while (channel.route?.relay.id !== "r-2") {
    if (Date.now() - drained > 15_000) throw new Error("timed out: the channel never moved");
    assert.deepEqual(await echo(channel, `during ${answered}`), { method: "POST", path: "/api/sessions", body: `during ${answered}` });
    answered++;
    await flush(100);
  }
  const took = Date.now() - drained;
  assert.ok(answered > 5, `requests went on while it moved (${answered})`);
  assert.equal(channel.status, "open");
  assert.ok(took >= 2500 && took < 8000, `moved once the machine was on r-2 (${took} ms after the goaway)`);
  assert.deepEqual(await echo(channel, "after"), { method: "POST", path: "/api/sessions", body: "after" });
  assert.equal(hub.agent.carriers.at(-1), "relay2", "the new handshake came over r-2");
  // At most one OPEN on the hub, the page's first since the cut-over: it answers 4426, and from then on the page waits for a relay.
  assert.ok(hub.opens.length <= 1, `${hub.opens.length} OPENs on the hub`);
});

test("an agent that sequences frames gets them sequenced, the relay path measured, and a direct path asked for only when allowed", async (t) => {
  const { http, account, hub, relay } = await setUp();
  await account.createGenesis(false);
  const code = await account.startEnrollment();
  await enroll(account, http, hub.agent, code);
  await account.completeEnrollment(code, undefined, 5, undefined, { writable: true });
  const link = new HubLink({
    url: "wss://hub.test/v1/client",
    origin: ORIGIN,
    account: account.account,
    deviceId: account.deviceId,
    token: () => account.session.token,
    signKey: account.keys.sign.privateKey,
    makeSocket: hub.make,
    random: () => 0,
  });
  t.after(() => link.stop());
  link.start();
  await link.waitOpen(10_000);
  hub.agent.features = ["rpc", "mux", "term", "blob", "local", "paths/1", "direct/1"];
  let allowed = false;
  const allowedFns = new Set<() => void>();
  let peers = 0;
  const pool = relayPool(account, relay);
  t.after(() => pool.stop());
  const channel = new MachineChannel(link, account, "m-1", {
    relays: pool,
    direct: true,
    directEnv: {
      allowed: () => allowed,
      onAllowedChange: (fn) => {
        allowedFns.add(fn);
        return () => allowedFns.delete(fn);
      },
      onNetworkChange: () => () => undefined,
      createPeer: () => {
        peers++;
        throw new Error("no WebRTC here");
      },
      log: () => undefined,
    },
  });
  t.after(() => channel.close());
  let heard = 0;
  channel.onPaths(() => heard++);
  const transport = tunnelTransport(channel);
  const res = await transport.fetch(new Request("http://machine/api/sessions", { method: "POST", body: "seq" }));
  assert.deepEqual(await res.json(), { method: "POST", path: "/api/sessions", body: "seq" }, "rpc works over sequenced frames");
  assert.ok(channel.paths, "the channel reports its paths");
  assert.equal(channel.paths.direct, false);
  await until(() => channel.paths?.relayRttMs != null);
  assert.ok(channel.paths?.relayRttMs != null, "the relay path's round trip, measured by a probe");
  assert.ok(heard > 0, "and listeners heard it");
  assert.equal(channel.directPaths?.state, "off", "hub only: nothing asked");
  assert.ok(!hub.agent.requests.includes("GET /rh/direct"));

  // Allowed now: the agent is asked, says no (403), and nothing more is tried on this channel.
  allowed = true;
  for (const fn of allowedFns) fn();
  await until(() => channel.directPaths?.state === "unsupported");
  assert.equal(channel.directPaths?.state, "unsupported");
  assert.ok(hub.agent.requests.includes("GET /rh/direct"));
  assert.equal(peers, 0, "no peer connection was made");
});

test("a machine's channels share one direct-path history, so a new channel carries on with the backoff", async (t) => {
  const { account, hub, link } = await linked(t);
  hub.agent.features = ["rpc", "mux", "term", "blob", "local", "paths/1", "direct/1"];
  const channel = new MachineChannel(link, account, "m-1", {
    direct: true,
    // Hub only on this device: no attempt, only the bookkeeping under test.
    directEnv: { allowed: () => false, onAllowedChange: () => () => undefined, onNetworkChange: () => () => undefined, log: () => undefined },
  });
  t.after(() => channel.close());
  assert.deepEqual(await echo(channel, "first"), { method: "POST", path: "/api/sessions", body: "first" });
  const first = channel.directPaths;
  assert.ok(first, "the first channel has direct paths");
  // Its paths kept dying young.
  first.history.shortLived = SHORT_LIVED_LIMIT;
  first.history.backoffMs = 64_000;
  // The hub restarts: the channel goes with its connection, and the next request opens another.
  hub.live.close(1012);
  await link.waitOpen(10_000);
  assert.deepEqual(await echo(channel, "again"), { method: "POST", path: "/api/sessions", body: "again" });
  const next = channel.directPaths;
  assert.ok(next && next !== first, "a new channel, with direct paths of its own");
  assert.equal(next.history, first.history, "but the same history");
  assert.ok(next.history.flapping);
  assert.equal(next.history.backoffMs, 64_000, "so it carries on with the backoff rather than from 2 s");
});

test("a machine key goes inside the channel (POST /rh/machine-key) when the machine is online", async (t) => {
  const { http, account, hub, relay } = await setUp();
  await account.createGenesis(false);
  const code = await account.startEnrollment();
  await enroll(account, http, hub.agent, code);
  await account.completeEnrollment(code, undefined, 5, undefined, { writable: true });
  const link = new HubLink({
    url: "wss://hub.test/v1/client",
    origin: ORIGIN,
    account: account.account,
    deviceId: account.deviceId,
    token: () => account.session.token,
    signKey: account.keys.sign.privateKey,
    makeSocket: hub.make,
    random: () => 0,
  });
  t.after(() => link.stop());
  link.start();
  await link.waitOpen(10_000);
  const pool = relayPool(account, relay);
  t.after(() => pool.stop());
  const courier = linkCourier(link, account, undefined, pool);
  assert.equal(await courier.waitOnline("m-1", 10), true, "presence says online");
  assert.equal(await courier.waitOnline("m-2", 10), false);
  const mk = crypto.getRandomValues(new Uint8Array(32));
  // The fake agent's bridge echoes what it was asked.
  const res = await courier.postInChannel("m-1", { epoch: 1, key: hex(mk), reencrypt: true });
  assert.equal(res.status, 200);
  const echoed = res.body as { method: string; path: string; body: string };
  assert.equal(echoed.method, "POST");
  assert.equal(echoed.path, "/rh/machine-key");
  assert.deepEqual(JSON.parse(echoed.body), { epoch: 1, key: hex(mk), reencrypt: true });
  assert.deepEqual(await deliverMachineKey(courier, "m-1", 1, mk, true), { via: "channel", changed: true });
  assert.equal(http.mail.length, 0, "nothing went to the mailbox");
});

// The hub carries a channel only when no relay can (RH_HUB_CHANNEL_RELAY, on
// by default); switched off, it answers an OPEN with CLOSE 4426, and the page
// remembers that until its hub connection is made again.

test("with no relay to carry it, the channel opens on the hub: none listed, an agent too old for relays, or the machine on none", async (t) => {
  const { account, hub, relay, link } = await linked(t);

  // No relay listed: the hub, straight away.
  const empty = relayPool(account, relay, relayDirectory([]));
  t.after(() => empty.stop());
  const channel = new MachineChannel(link, account, "m-1", { relays: empty });
  t.after(() => channel.close());
  assert.deepEqual(await echo(channel, "on the hub"), { method: "POST", path: "/api/sessions", body: "on the hub" });
  assert.equal(channel.status, "open");
  assert.equal(channel.route?.kind, "hub", "the path indicator says the hub");
  assert.equal(channel.blocked, null);
  assert.deepEqual(hub.opens, ["m-1"]);
  assert.deepEqual(hub.agent.carriers, ["hub"]);
  assert.equal(relay.sockets.length, 0, "no relay dialled");
  // Sockets run over it too.
  const socket = tunnelTransport(channel).openSocket("/api/terminals/t1/ws");
  const heard = await new Promise<unknown>((resolve) => {
    socket.onopen = () => socket.send("x");
    socket.onmessage = (e) => resolve(e.data);
  });
  assert.equal(heard, "echo:x");

  // An agent from before relays (presence `relays: false`): the hub, nothing ticketed.
  hub.machine = { via: [], relays: false };
  hub.announce();
  const tickets = { count: 0 };
  const pool = relayPool(account, relay, relayDirectory(), tickets);
  t.after(() => pool.stop());
  const old = new MachineChannel(link, account, "m-1", { relays: pool });
  t.after(() => old.close());
  assert.deepEqual(await echo(old, "too old"), { method: "POST", path: "/api/sessions", body: "too old" });
  assert.equal(old.route?.kind, "hub");
  assert.equal(tickets.count, 0, "no ticket for an agent that signs in on no relay");
  assert.equal(relay.sockets.length, 0);

  // The machine on no relay: the best one is ticketed and waited for; it does not come, so the hub.
  hub.machine = { via: [], relays: true };
  hub.announce();
  const away = new MachineChannel(link, account, "m-1", { relays: pool });
  t.after(() => away.close());
  assert.deepEqual(await echo(away, "not on it"), { method: "POST", path: "/api/sessions", body: "not on it" });
  assert.equal(away.route?.kind, "hub");
  assert.equal(tickets.count, 1, "r-1 was asked for first");
  assert.deepEqual(hub.opens, ["m-1", "m-1", "m-1"]);
  assert.deepEqual(hub.agent.carriers, ["hub", "hub", "hub"]);

  // And with no pool at all (a page from before relays): the hub.
  const bare = new MachineChannel(link, account, "m-1");
  t.after(() => bare.close());
  await bare.ensure();
  assert.equal(bare.route?.kind, "hub");
});

test("a device that does not manage the account vouches for nothing when it opens a channel", async (t) => {
  const { account, hub, link } = await linked(t);
  // A limited device (chain v3, not an admin): the hub refuses its attestations, so it sends none.
  Object.defineProperty(account, "isAdmin", { get: () => false });
  const before = hub.agent.attestations;
  const channel = new MachineChannel(link, account, "m-1");
  t.after(() => channel.close());
  await assert.rejects(channel.ensure());
  assert.equal(hub.agent.attestations, before, "no attestation from it reached the agent");
  assert.equal(channel.lastRefusal, "stale", "the agent asked for another device's word instead");
  assert.deepEqual(channel.refusal, { kind: "unconfirmed" }, "and the page says it waits for another device to confirm");
});

test("a computer that cannot save who connects to it says so, at once, rather than retrying blind", async (t) => {
  const { account, hub, link } = await linked(t);
  // What the agent closes with when it took the handshake but its disk would not take the record.
  hub.refuseOpens = "this device cannot save its record of who connects to it: check its disk";
  hub.refuseCode = AGENT_UNSAVED;
  const opens = hub.opens.length;
  const channel = new MachineChannel(link, account, "m-1");
  t.after(() => channel.close());
  await assert.rejects(channel.ensure(), /could not save its record of who connects/);
  assert.deepEqual(channel.refusal, { kind: "unsaved" });
  assert.equal(hub.opens.length - opens, 1, "one try: the next would meet the same disk");
  hub.refuseOpens = null;
  hub.refuseCode = 4403;
  await channel.ensure();
  assert.equal(channel.refusal, null, "saved this time: open, no reason left");
});

test("a refused OPEN is kept as the reason across retries, and cleared once a channel opens", async (t) => {
  const { account, hub, link } = await linked(t);
  hub.refuseOpens = "that device does not accept control from this one";
  const channel = new MachineChannel(link, account, "m-1");
  t.after(() => channel.close());
  await assert.rejects(channel.ensure(), /the hub refused the channel/);
  assert.deepEqual(channel.refusal, { kind: "notAccepted", by: "hub", why: "that device does not accept control from this one" });
  assert.equal(channel.status, "error");
  // The next try: while it is "connecting" the reason is still there, and a second refusal keeps it.
  const seen: unknown[] = [];
  const off = channel.onStatus((status) => {
    if (status === "connecting") seen.push(channel.refusal?.kind);
  });
  await assert.rejects(channel.ensure());
  off();
  assert.deepEqual(seen, ["notAccepted"], "the bar keeps saying why through the retry");
  assert.equal(channel.refusal?.kind, "notAccepted");
  // Let go of (switched away from): asked afresh next time, not kept off the pickers meanwhile.
  channel.close();
  assert.equal(channel.refusal, null);
  assert.equal(channel.status, "idle");
  await assert.rejects(channel.ensure());
  // A try that fails some other way (the device went offline) does not keep the old reason.
  hub.refuseCode = 4404;
  await assert.rejects(channel.ensure(), /offline/);
  assert.equal(channel.refusal, null, "offline is not \"does not accept this browser\"");
  hub.refuseCode = 4403;
  hub.refuseOpens = null;
  await channel.ensure();
  assert.equal(channel.status, "open");
  assert.equal(channel.refusal, null, "a channel that opened is no longer refused");
});

test("the transport hears each new channel, so what was lost with the old one can be asked about at once", async (t) => {
  const { account, hub, link } = await linked(t);
  const channel = new MachineChannel(link, account, "m-1");
  t.after(() => channel.close());
  const transport = tunnelTransport(channel);
  assert.ok(transport.onReconnect, "a tunnel over a machine channel says when a new channel is up");
  let opened = 0;
  t.after(transport.onReconnect(() => opened++));
  assert.deepEqual(await echo(channel, "first"), { method: "POST", path: "/api/sessions", body: "first" });
  await flush(5);
  assert.equal(opened, 1, "the first channel");
  // The hub restarts (a deploy): the channel goes with its connection, and a
  // request on it would have failed without an answer.
  hub.live.close(1012);
  await link.waitOpen(10_000);
  assert.deepEqual(await echo(channel, "again"), { method: "POST", path: "/api/sessions", body: "again" });
  await flush(5);
  assert.equal(opened, 2, "and the one after the loss: the moment to settle sends left unknown");
});

test("a relay the page cannot reach gives way to the hub; with the hub switched off, the channel is blocked", async (t) => {
  const { account, hub, relay, link } = await linked(t);
  relay.down = true;
  const pool = relayPool(account, relay);
  t.after(() => pool.stop());
  const channel = new MachineChannel(link, account, "m-1", { relays: pool });
  t.after(() => channel.close());
  assert.deepEqual(await echo(channel, "around r-1"), { method: "POST", path: "/api/sessions", body: "around r-1" });
  assert.equal(channel.route?.kind, "hub");
  assert.equal(relay.sockets.length, 1, "r-1 was tried first");
  assert.ok(pool.failure("r-1"), "and its failure noted");
  assert.deepEqual(hub.agent.carriers, ["hub"]);

  channel.close();
  hub.carries = false;
  await assert.rejects(channel.ensure(), { name: "ChannelBlocked" });
  assert.equal(channel.blocked, "noRelay");
  assert.equal(relay.sockets.length, 2, "r-1 tried again first");
  assert.deepEqual(hub.opens, ["m-1", "m-1"]);
});

test("a hub that carries no channels (4426) is asked once: the channel is blocked, no relay or the agent too old, and nothing more goes to the hub", async (t) => {
  const { account, hub, relay, link } = await linked(t);
  hub.carries = false;
  const pool = relayPool(account, relay, relayDirectory([]));
  t.after(() => pool.stop());
  const channel = new MachineChannel(link, account, "m-1", { relays: pool });
  t.after(() => channel.close());
  await assert.rejects(channel.ensure(), { name: "ChannelBlocked" });
  assert.deepEqual(hub.opens, ["m-1"], "one OPEN, answered 4426");
  assert.equal(link.carriesChannels, false, "the link remembers it");
  assert.equal(channel.status, "error");
  assert.equal(channel.blocked, "noRelay", "no relay available");

  // Again and again (the mode controller's retries), and a short-lived channel next to it: none asks the hub.
  for (let i = 0; i < 5; i++) await assert.rejects(channel.ensure(), { name: "ChannelBlocked" });
  const other = new MachineChannel(link, account, "m-1", { relays: pool });
  t.after(() => other.close());
  await assert.rejects(other.ensure(), { name: "ChannelBlocked" });
  assert.deepEqual(hub.opens, ["m-1"], "nothing more on the hub");
  assert.equal(channel.blocked, "noRelay");

  // The agent is from before relays: it is too old to be reached.
  hub.machine = { via: [], relays: false };
  hub.announce();
  await assert.rejects(channel.ensure(), { name: "ChannelBlocked" });
  assert.equal(channel.blocked, "agentTooOld");
  assert.deepEqual(hub.opens, ["m-1"]);
  assert.deepEqual(hub.agent.carriers, [], "no handshake anywhere");
});

test("an agent too old for relays, behind a hub that carries no channels: asked once, then the copy, with no loop against the hub; a relay once the machine is on one", async (t) => {
  const { account, hub, relay, link } = await linked(t);
  hub.carries = false;
  hub.machine = { via: [], relays: false };
  hub.announce();
  const directory = relayDirectory([]);
  const pool = relayPool(account, relay, directory);
  t.after(() => pool.stop());
  const channel = new MachineChannel(link, account, "m-1", { relays: pool });
  t.after(() => channel.close());
  let tries = 0;
  channel.onStatus((s) => {
    if (s === "connecting") tries++;
  });
  const transport = new SwitchingTransport({ live: tunnelTransport(channel), offline: tunnelTransport(channel) }, "offline");
  const controller = new ModeController({ link, channel, machine: "m-1", transport, downMs: 20, retryMs: () => 5, relayOnline: () => pool.online("m-1") });
  t.after(() => controller.stop());
  controller.start();
  await until(() => channel.blocked !== null);
  assert.equal(channel.blocked, "agentTooOld");
  await until(() => tries >= 6);
  assert.deepEqual(hub.opens, ["m-1"], `asked once; the ${tries} tries since asked the hub nothing`);
  assert.equal(transport.current, "offline", "the page stays on the copy");
  assert.equal(channel.status === "error" || channel.status === "connecting", true);

  // The agent is updated and signs in on r-1, which the list now has: through it, at once.
  directory.relays = [R1];
  await pool.refresh();
  hub.machine = { via: [{ relay: "r-1", rtt_ms: 1 }], relays: true };
  hub.announce();
  await until(() => transport.current === "live");
  assert.equal(channel.status, "open");
  assert.equal(channel.route?.kind, "relay");
  assert.equal(channel.route?.kind === "relay" ? channel.route.relay.id : null, "r-1");
  assert.equal(channel.blocked, null);
  assert.deepEqual(hub.opens, ["m-1"]);
  assert.deepEqual(hub.agent.carriers, ["relay"]);
  assert.deepEqual(await echo(channel, "through r-1"), { method: "POST", path: "/api/sessions", body: "through r-1" });
});

test("a new hub connection forgets the 4426: asked once per connection, and on the hub again once a deploy turned relaying back on", async (t) => {
  const { account, hub, relay, link } = await linked(t);
  hub.carries = false;
  const pool = relayPool(account, relay, relayDirectory([]));
  t.after(() => pool.stop());
  const channel = new MachineChannel(link, account, "m-1", { relays: pool });
  t.after(() => channel.close());
  await assert.rejects(channel.ensure(), { name: "ChannelBlocked" });
  await assert.rejects(channel.ensure(), { name: "ChannelBlocked" });
  assert.deepEqual(hub.opens, ["m-1"]);

  // The hub restarts, still switched off: asked once on the new connection, then remembered again.
  hub.live.close(1012);
  await flush(5);
  await link.waitOpen(10_000);
  assert.equal(link.carriesChannels, true, "a new connection: not known yet");
  await assert.rejects(channel.ensure(), { name: "ChannelBlocked" });
  await assert.rejects(channel.ensure(), { name: "ChannelBlocked" });
  assert.deepEqual(hub.opens, ["m-1", "m-1"]);
  assert.equal(link.carriesChannels, false);

  // A deploy turns relaying back on: the next connection carries the channel.
  hub.carries = true;
  hub.live.close(1012);
  await flush(5);
  await link.waitOpen(10_000);
  assert.deepEqual(await echo(channel, "back on the hub"), { method: "POST", path: "/api/sessions", body: "back on the hub" });
  assert.equal(channel.route?.kind, "hub");
  assert.equal(channel.blocked, null);
  assert.equal(channel.status, "open");
  assert.deepEqual(hub.opens, ["m-1", "m-1", "m-1"]);
});

test("a channel on the hub stays there when a relay appears (no flapping); the next one goes through the relay", async (t) => {
  const { account, hub, relay, link } = await linked(t);
  hub.machine = { via: [], relays: true };
  hub.announce();
  const directory = relayDirectory([]);
  const pool = relayPool(account, relay, directory);
  t.after(() => pool.stop());
  const channel = new MachineChannel(link, account, "m-1", { relays: pool });
  t.after(() => channel.close());
  const transport = new SwitchingTransport({ live: tunnelTransport(channel), offline: tunnelTransport(channel) }, "offline");
  const controller = new ModeController({ link, channel, machine: "m-1", transport, downMs: 20, retryMs: () => 5, relayOnline: () => pool.online("m-1") });
  t.after(() => controller.stop());
  controller.start();
  await until(() => transport.current === "live");
  assert.equal(channel.route?.kind, "hub");

  // r-1 is listed and the machine signs in there: the open channel is not moved.
  directory.relays = [R1];
  await pool.refresh();
  hub.machine = { via: [{ relay: "r-1", rtt_ms: 1 }], relays: true };
  hub.announce();
  await flush(30);
  assert.equal(channel.route?.kind, "hub", "an open channel is left where it is");
  assert.deepEqual(hub.agent.carriers, ["hub"]);
  assert.deepEqual(hub.opens, ["m-1"]);

  // It goes (the hub restarts under it): the next one prefers the relay.
  hub.live.close(1012);
  await until(() => channel.status === "open" && channel.route?.kind === "relay");
  assert.deepEqual(hub.agent.carriers, ["hub", "relay"]);
  assert.deepEqual(hub.opens, ["m-1"]);
  await flush(30);
  assert.equal(channel.route?.kind, "relay", "and stays there");
  assert.deepEqual(hub.agent.carriers, ["hub", "relay"]);
});

// A phone back from a long sleep (or from offline): its sockets died meanwhile,
// its device session and relay tickets ran out, and what it last heard of the
// machine is old. The hub carries no channels (`RH_HUB_CHANNEL_RELAY=off`, as
// in production). The page must find its own way back to the machine through
// a relay, and never say "no relay available" on what it heard before.

/** A page live on m-1 through r-1, with the hub carrying no channels, and its mode controller. */
async function liveThroughRelay(t: { after(fn: () => void): void }, ticketMs?: number) {
  const set = await linked(t);
  const { account, hub, relay, link } = set;
  hub.carries = false;
  const tickets = { count: 0 };
  const pool = relayPool(account, relay, relayDirectory([R1]), tickets, [], ticketMs);
  t.after(() => pool.stop());
  const channel = new MachineChannel(link, account, "m-1", { relays: pool });
  t.after(() => channel.close());
  const transport = new SwitchingTransport({ live: tunnelTransport(channel), offline: tunnelTransport(channel) }, "offline");
  // The page's own pace of retries is seconds; here a tenth of one. Nothing here is the page coming back (resume): the retries alone.
  const controller = new ModeController({ link, channel, machine: "m-1", transport, downMs: 20, retryMs: () => 100, relayOnline: () => pool.online("m-1"), resume: () => () => undefined });
  t.after(() => controller.stop());
  controller.start();
  await until(() => transport.current === "live");
  assert.equal(channel.route?.kind === "relay" ? channel.route.relay.id : null, "r-1");
  // Every "blocked" the bar would have shown from here on.
  const blocked: string[] = [];
  channel.onStatus(() => {
    if (channel.blocked) blocked.push(channel.blocked);
  });
  return { ...set, pool, tickets, channel, transport, blocked };
}

test("back from a long pause, with the hub's word on the machine coming after the first try: the channel waits for it and goes live through the relay, never 'no relay'", async (t) => {
  const { http, hub, relay, link, channel, transport, tickets, blocked } = await liveThroughRelay(t, 61_000);
  // The machine renews its identity key and signs in again: the last the page hears, it is on no relay yet.
  hub.machine = { via: [], relays: true };
  hub.announce();
  await flush(5);

  // The phone sleeps. The machine is soon back on r-1, but the page hears nothing of it: its sockets die
  // meanwhile, its device session runs out (the hub refuses its next sign-in once), and its ticket for r-1
  // too (less than the minute's margin left). Signed in again, the hub says where the machine is only a
  // while after its welcome — later than a relay the machine is not on is waited for (1.5 s).
  hub.machine = { via: [{ relay: "r-1", rtt_ms: 1 }], relays: true };
  hub.presenceOnSignIn = false;
  hub.refuseSignIns = 1;
  const sessions = http.sessions.length;
  const sockets = hub.sockets.length;
  const opens = hub.opens.length;
  const ticketed = tickets.count;
  hub.agent.dropChannels();
  relay.live.close(1006);
  hub.live.close(1006);

  await until(() => hub.sockets.length >= sockets + 2 && link.state === "open");
  assert.equal(http.sessions.length, sessions + 1, "the refused sign-in renewed the device session");
  await until(() => channel.status === "connecting");
  await flush(2500);
  assert.equal(transport.current, "offline", "nothing to go on yet: on the copy");
  assert.deepEqual(blocked, [], "and not 'no relay available': the hub has not said where the machine is");
  hub.announce();

  await until(() => transport.current === "live" && channel.status === "open", 15_000);
  assert.equal(channel.route?.kind === "relay" ? channel.route.relay.id : null, "r-1", "through the relay the machine is on");
  assert.ok(tickets.count > ticketed, "with a fresh ticket");
  assert.deepEqual(blocked, [], "never 'no relay available' on the way back");
  assert.equal(hub.opens.length, opens, "and nothing asked of a hub that carries no channels");
  assert.deepEqual(await echo(channel, "back"), { method: "POST", path: "/api/sessions", body: "back" });
});

test("a relay given up on while the phone was offline is tried again once it answers: the page is never stuck on 'no relay'", async (t) => {
  const { hub, relay, pool, channel, transport, blocked } = await liveThroughRelay(t);
  // Offline: the relay's socket dies, and every try since fails on it, until the pool gives up on it.
  relay.down = true;
  hub.agent.dropChannels();
  relay.live.close(1006);
  await until(() => !pool.usable("r-1"));
  await until(() => channel.blocked === "noRelay");
  assert.equal(transport.current, "offline", "on the copy, saying why");

  // Online again: the relay answers. The page goes live through it by itself — the minute a relay
  // stays given up on is not waited out, and nothing else (a reload, a click) is needed.
  relay.down = false;
  await until(() => transport.current === "live" && channel.status === "open", 10_000);
  assert.equal(channel.route?.kind === "relay" ? channel.route.relay.id : null, "r-1");
  assert.equal(channel.blocked, null);
  assert.ok(pool.usable("r-1"), "and it is counted as working again");
  assert.ok(blocked.length > 0, "('no relay available' was true while the relay could not be reached)");
});

test("a page that comes back forgives its relays' failures and fetches an old list again", async (t) => {
  const { account, relay } = await setUp();
  const directory = relayDirectory([R1]);
  let fetched = 0;
  let now = Date.now();
  const pool = new RelayPool({
    api: {
      relays: async () => {
        fetched++;
        return structuredClone(directory);
      },
    } as never,
    account,
    storage: null,
    now: () => now,
    fetchImpl: async () => new Response(null, { status: 204 }),
    makeLink: (entry, ticket) =>
      new RelayLink({ relay: entry.id, origin: entry.origin, account: account.account, deviceId: account.deviceId, signKey: account.keys.sign.privateKey, ticket, makeSocket: relay.make as never }),
  });
  t.after(() => pool.stop());
  await pool.ready();
  assert.equal(fetched, 1);
  for (let i = 0; i < 3; i++) pool.noteFailure("r-1", "no answer");
  assert.equal(pool.usable("r-1"), false, "three failures in a row: given up on");
  assert.deepEqual(pool.benched(), ["r-1"]);
  assert.equal(pool.choose("m-1"), null, "and not chosen");
  assert.equal(pool.choose("m-1", new Set(), undefined, { benched: true })?.relay.id, "r-1", "unless there is nowhere else to go");
  // Not for good: a minute later it is a candidate again.
  now += 60_000;
  assert.equal(pool.usable("r-1"), true);
  for (let i = 0; i < 3; i++) pool.noteFailure("r-1", "no answer");
  assert.equal(pool.usable("r-1"), false);
  // The page comes back: what failed before is forgiven, and a list two minutes old is fetched again.
  now += 120_000;
  pool.wake();
  assert.equal(pool.usable("r-1"), true);
  assert.equal(pool.failure("r-1"), null);
  await until(() => fetched === 2);
  // A list that was just fetched is not fetched again when asked for one at most a minute old.
  await pool.ready(60_000);
  assert.equal(fetched, 2);
  now += 61_000;
  await pool.ready(60_000);
  assert.equal(fetched, 3);
});

test("a relay connection that cannot be signed in to counts as one failure, not two", async (t) => {
  const { account, relay } = await setUp();
  relay.down = true;
  const pool = relayPool(account, relay);
  t.after(() => pool.stop());
  await pool.ready();
  await assert.rejects(pool.link(R1));
  assert.equal(pool.usable("r-1"), true);
  await assert.rejects(pool.link(R1));
  assert.equal(pool.usable("r-1"), true, "two failed sign-ins: not given up on yet");
  await assert.rejects(pool.link(R1));
  assert.equal(pool.usable("r-1"), false, "the third");
});

test("cloud mode is the hub's marker in the shell, and nothing else", async () => {
  const { hubOrigin } = await import("../src/lib/hub/config.js");
  const doc = (content: string | null) => ({
    querySelector: () => (content === null ? null : { getAttribute: () => content }),
  }) as unknown as Pick<Document, "querySelector">;
  assert.equal(hubOrigin(doc("wss://hub.example")), "wss://hub.example");
  assert.equal(hubOrigin(doc("ws://127.0.0.1:9000")), "ws://127.0.0.1:9000");
  assert.equal(hubOrigin(doc(null)), null, "a server's shell: local mode, as always");
  assert.equal(hubOrigin(doc("javascript:alert(1)")), null);
  assert.equal(hubOrigin(undefined), null);
});

test("the local sign-in's enrollment adds only the computer whose keys the request showed", async () => {
  const { http, account, hub } = await setUp();
  await account.createGenesis(false);
  const code = await account.startEnrollment();
  await enroll(account, http, hub.agent, code);
  const before = http.chain.length;
  const stranger = crypto.getRandomValues(new Uint8Array(32));
  await assert.rejects(account.completeEnrollment(code, undefined, 5, { signKey: stranger, dhKey: hub.agent.dh.publicKey }, { writable: true }), /not the one that asked for it/);
  assert.equal(http.chain.length, before, "nothing appended for a proof under other keys");
  const machine = await account.completeEnrollment(code, undefined, 5, { signKey: hub.agent.sign.publicKey, dhKey: hub.agent.dh.publicKey }, { writable: true });
  assert.equal(machine.id, "m-1");
  assert.equal(http.chain.length, before + 2, "added, with its first key epoch");
});

test("re-review: a fork a handshake shows latches the account: no channel, no attestation", async (t) => {
  const { account, hub, link } = await linked(t);
  // The agent's view at this device's seq is another entry.
  hub.agent.shapeReply = () => ({ chainHead: new Uint8Array(32).fill(7) });
  const channel = new MachineChannel(link, account, "m-1");
  t.after(() => channel.close());
  await assert.rejects(channel.ensure());
  assert.ok(account.forked, "the alarm is latched");
  await assert.rejects(account.attestation(), { name: "AccountError" });
  hub.agent.shapeReply = null;
  await assert.rejects(channel.ensure(), "and no channel opens on it after");
});

test("re-review: what a refused handshake taught (verified) is kept, not left to a hub that may withhold it", async (t) => {
  const { account, hub, link } = await linked(t);
  // An entry the hub has not served this device, which the agent learnt elsewhere: it teaches it, and refuses this once.
  const chain = account.chain!;
  const entry = await signDraft(chain.draft({ type: "device", id: account.deviceId }, { type: "setRecoveryKey", key: null }, Date.now()), account.keys.sign.privateKey);
  const head = await entryHash(entry);
  let first = true;
  hub.agent.shapeReply = () => {
    if (!first) return { chainSeq: entry.draft.seq, chainHead: head };
    first = false;
    return { status: "stale", chainSeq: entry.draft.seq, chainHead: head, entries: [entry] };
  };
  const channel = new MachineChannel(link, account, "m-1");
  t.after(() => channel.close());
  assert.deepEqual(await echo(channel, "hi"), { method: "POST", path: "/api/sessions", body: "hi" });
  assert.equal(account.chain!.seq, entry.draft.seq, "the refusal's entry is in this view");
  assert.equal(hex(account.chain!.head), hex(head));
});
