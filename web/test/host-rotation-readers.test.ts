// Reading a host that renewed its identity key (`docs/host-identity-rotation.md`
// §3.5 "关通道", §5.1, §5.4 "读的一方", §8 items 8 and 9), browser side, over
// Rust's renewal chain (the `hostRotation` vectors: h-2 renews from its first
// keys to its second at "rotate").
//
// Streams: a head or manifest from the hub counts under the host's CURRENT key
// alone — one under a retired key is refused, after the chain was synced once
// in case the host renewed again; one under a key the host renewed to before
// this device heard is taken after that one sync. A position this device
// verified and kept (a saved copy's head, a copy mark) still checks under the
// key it was signed with, so the offline copy survives the renewal. The
// stream client follows the account's chain for the host's key.
//
// Channels: only ever to the DH key the chain gives the host now; a live one
// to a key the chain has replaced is closed at once (the account listener
// with it); a handshake that fails other than by the agent's refusal syncs the
// chain and is tried once more.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  bridgeMessage,
  type Bytes,
  ChainClock,
  ChainState,
  ChannelDriver,
  type ChannelStream,
  decodeEntry,
  decodeHelloBody,
  decodeSignedHead,
  DEFAULT_REKEY,
  encodeReplyBody,
  encodeResponseHead,
  type Entry,
  FMT_INDEX_V1,
  type Frame,
  fromHex,
  generateDeviceKeys,
  hex,
  importEd25519Private,
  indexStream,
  keyPairFromSecret,
  LogReader,
  MessageReader,
  type NoiseKeyPair,
  RecordError,
  Responder,
  utf8,
} from "../src/lib/e2e/index.js";
import type { CloudAccount } from "../src/lib/hub/account.js";
import { HubCopySource } from "../src/lib/hub/copy-source.js";
import type { HubLink } from "../src/lib/hub/link.js";
import { AGENT_HANDSHAKE_REFUSED, ChannelIsolated, MachineChannel } from "../src/lib/hub/machine.js";
import { MachineMirror } from "../src/lib/hub/mirror.js";
import { type IndexState, indexFold, StreamClient, type StreamClientOptions } from "../src/lib/hub/streams.js";
import { tunnelTransport } from "../src/lib/transport/tunnel.js";
import { FakeKv, FakeStreamLink, flush, makeMachine, type Machine, putCopy, rotate, StreamWriter, until } from "./stream-fixtures.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;
const V: Json = JSON.parse(readFileSync(new URL("../../server-rs/crates/rh-proto/vectors/rust.json", import.meta.url), "utf8"));
const R: Json = V.hostRotation;
const h = (s: string) => fromHex(s);
const hist = () => ChainClock.history();
const entriesOf = (list: string[]): Entry[] => list.map((e) => decodeEntry(h(e)));
const seqOf = (label: string): number => R.steps.find((s: Json) => s.label === label).seq;
const HOST = "h-2";
/** h-2's sign key before the renewal, and after it. */
const signer = async (i: number) => ({ key: await importEd25519Private(h(R.hostKeys[i].signSeed)), pub: h(R.hostKeys[i].signKey) });
/** h-2's DH key pair before the renewal, and after it. */
const dhOf = (i: number) => keyPairFromSecret(h(R.hostKeys[i].dhSecret));

/**
 * The page's account over Rust's renewal chain, standing at `label`.
 * `advance` takes it on and tells its listeners (what a chain frame from the
 * hub does); `sync` is counted, and does `onSync`.
 */
async function account(label: string) {
  const entries = entriesOf(R.entries);
  const state = await ChainState.verify(entries.slice(0, seqOf(label) + 1), undefined, hist());
  const listeners = new Set<() => void>();
  const emit = () => {
    for (const fn of [...listeners]) fn();
  };
  const device = await generateDeviceKeys();
  const acct = {
    syncs: 0,
    onSync: null as (() => Promise<void>) | null,
    deviceId: "d-web",
    isAdmin: false,
    noiseKeys: { privateKey: device.dh.privateKey, publicKey: device.dhPublic } as NoiseKeyPair,
    chainClock: hist(),
    get chain(): ChainState {
      return state;
    },
    /** How many listen to it now. */
    get watchers(): number {
      return listeners.size;
    },
    machine: (id: string) => state.machine(id)?.info,
    onChange(fn: () => void) {
      listeners.add(fn);
      return () => void listeners.delete(fn);
    },
    async sync() {
      acct.syncs++;
      await acct.onSync?.();
      return 0;
    },
    /** Never forked here. */
    forked: null,
    assertNotForked() {},
    /** A handshake's entries merged into the view (in place here), and told when it took any. */
    async completeHandshake<T>(finish: (merged: { chain: ChainState; clock: ReturnType<typeof hist> }) => Promise<T>): Promise<T> {
      const before = state.seq;
      try {
        return await finish({ chain: state, clock: acct.chainClock });
      } finally {
        if (state.seq !== before) emit();
      }
    },
    /** Whether the page holds the host isolated (docs/host-identity-rotation.md §3.8). */
    isolatedNow: false,
    isolated: (_id: string) => acct.isolatedNow,
    /** The account changed (what the renewal runner notes, say). */
    poke() {
      emit();
    },
    async advance(to: string) {
      await state.merge(entries.slice(state.seq + 1, seqOf(to) + 1), hist());
      emit();
    },
  };
  return acct;
}

const row = (id: string, updatedAt: string) => ({ source: "claude", id, title: `t ${id}`, updatedAt, cwd: "/w" });
const badHead = (e: unknown) => e instanceof RecordError && e.kind === "badHead";

/** A copy source following the account's keys for h-2, over a link the test plays, and what the streams said. */
async function following(acct: Awaited<ReturnType<typeof account>>, machine: Machine, client: Partial<StreamClientOptions> = {}) {
  const link = new FakeStreamLink();
  const pages = new Map<string, Bytes>();
  const source = HubCopySource.following({
    account: acct,
    machine: HOST,
    keys: async () => machine.epochs,
    client: { link, fetchPage: async (_stream, page) => pages.get(hex(page))!, retryDelay: () => 0, ...client },
  });
  await source.ready();
  assert.ok(source.client, "keyed");
  return { link, pages, source, client: source.client! };
}

test("a head or manifest from the hub under the host's retired key is refused, after one sync; under its current key it is taken", async () => {
  const [old, cur] = [await signer(0), await signer(1)];
  const acct = await account("rotateKey");
  const machine = await makeMachine();
  const { link, pages, source, client } = await following(acct, machine);
  assert.equal(hex(client.machineSignKey), hex(cur.pub), "the chain's current key");
  const writer = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  await writer.append({ upserts: [row("a", "2026-10-15T00:00:00.000Z")], removed: [], asOf: 1 });
  const states: IndexState[] = [];
  const errors: unknown[] = [];
  client.subscribe(writer.stream, indexFold, { onState: (s) => states.push(s), onError: (e) => errors.push(e) });
  await until(() => link.subs().length === 1);

  // The hub (or someone holding the retired key) serves a head under the old key.
  let sub = link.lastSub();
  link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records: writer.records });
  link.deliver({ type: "head", ch: sub.ch, head: await writer.head(1_000, undefined, old.key) });
  await until(() => errors.length === 1);
  assert.ok(badHead(errors[0]), String(errors[0]));
  assert.equal(acct.syncs, 1, "the chain was synced once before the refusal stood");
  assert.equal(states.length, 0, "nothing under the retired key is handed out");

  // A manifest under the old key: refused too (the stream starts over, and asks again).
  await until(() => link.subs().length === 2);
  sub = link.lastSub();
  const forged = await writer.manifest([{ upserts: [row("x", "2026-10-15T00:00:00.000Z")], removed: [], asOf: 1 }], 1_000, old.key);
  for (const [k, v] of forged.pages) pages.set(k, v);
  link.deliver({ type: "checkpoint", ch: sub.ch, stream: writer.stream, uptoSeq: forged.upto, manifest: forged.manifest });
  await until(() => errors.length === 2);
  assert.ok(badHead(errors[1]), String(errors[1]));
  assert.equal(states.length, 0);
  assert.equal(acct.syncs, 1, "at most one sync in `resyncEveryMs`, however many refusals");

  // The host's manifest under its current key is taken, and so is its next head.
  await until(() => link.subs().length === 3);
  sub = link.lastSub();
  const good = await writer.manifest([{ upserts: [row("a", "2026-10-15T00:00:00.000Z")], removed: [], asOf: 1 }], 2_000, cur.key);
  for (const [k, v] of good.pages) pages.set(k, v);
  link.deliver({ type: "checkpoint", ch: sub.ch, stream: writer.stream, uptoSeq: good.upto, manifest: good.manifest });
  await until(() => states.length === 1);
  assert.deepEqual(Object.keys(states[0]!.rows), ["claude/a"]);
  await writer.append({ upserts: [row("b", "2026-10-16T00:00:00.000Z")], removed: [], asOf: 2 });
  link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records: writer.records.slice(1) });
  link.deliver({ type: "head", ch: sub.ch, head: await writer.head(3_000, undefined, cur.key) });
  await until(() => states.length === 2);
  assert.deepEqual(Object.keys(states[1]!.rows).sort(), ["claude/a", "claude/b"]);
  assert.equal(errors.length, 2);
  source.close();
});

test("a head under a key the host renewed to before this device heard: the chain is synced once and the head taken; the old key's no longer is", async () => {
  const [old, cur] = [await signer(0), await signer(1)];
  const acct = await account("removalKeys");
  // The hub has the renewal; this device has not heard yet.
  acct.onSync = () => acct.advance("rotateKey");
  const machine = await makeMachine();
  const { link, source, client } = await following(acct, machine, { resyncEveryMs: 0 });
  assert.equal(hex(client.machineSignKey), hex(old.pub));
  const writer = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  await writer.append({ upserts: [row("a", "2026-10-15T00:00:00.000Z")], removed: [], asOf: 1 });
  const states: IndexState[] = [];
  const errors: unknown[] = [];
  client.subscribe(writer.stream, indexFold, { onState: (s) => states.push(s), onError: (e) => errors.push(e) });
  await until(() => link.subs().length === 1);
  let sub = link.lastSub();
  link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records: writer.records });
  link.deliver({ type: "head", ch: sub.ch, head: await writer.head(1_000, undefined, cur.key) });
  await until(() => states.length === 1);
  assert.equal(acct.syncs, 1);
  assert.deepEqual(errors, []);
  assert.equal(hex(client.machineSignKey), hex(cur.pub), "the client follows the chain's new key");

  // From now on the retired key's heads are refused, the chain synced again first.
  await writer.append({ upserts: [row("b", "2026-10-16T00:00:00.000Z")], removed: [], asOf: 2 });
  link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records: writer.records.slice(1) });
  link.deliver({ type: "head", ch: sub.ch, head: await writer.head(2_000, undefined, old.key) });
  await until(() => errors.length === 1);
  assert.ok(badHead(errors[0]));
  assert.equal(acct.syncs, 2);
  assert.equal(states.length, 1);
  // Rebuilt from the start, under the current key.
  await until(() => link.subs().length === 2);
  sub = link.lastSub();
  assert.equal(sub.fromSeq, 0);
  link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records: writer.records });
  link.deliver({ type: "head", ch: sub.ch, head: await writer.head(3_000, undefined, cur.key) });
  await until(() => states.length === 2);
  assert.deepEqual(Object.keys(states[1]!.rows).sort(), ["claude/a", "claude/b"]);
  source.close();
});

test("the copy source picks up the host's new key when the account's chain changes, with no sync", async () => {
  const [old, cur] = [await signer(0), await signer(1)];
  const acct = await account("removalKeys");
  const machine = await makeMachine();
  const { link, source, client } = await following(acct, machine);
  assert.equal(hex(client.machineSignKey), hex(old.pub));
  // A chain frame from the hub brings the renewal.
  await acct.advance("rotateKey");
  await until(() => hex(client.machineSignKey) === hex(cur.pub));
  const writer = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  await writer.append({ upserts: [row("a", "2026-10-15T00:00:00.000Z")], removed: [], asOf: 1 });
  const states: IndexState[] = [];
  client.subscribe(writer.stream, indexFold, { onState: (s) => states.push(s) });
  await until(() => link.subs().length === 1);
  const sub = link.lastSub();
  link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records: writer.records });
  link.deliver({ type: "head", ch: sub.ch, head: await writer.head(1_000, undefined, cur.key) });
  await until(() => states.length === 1);
  assert.equal(acct.syncs, 0, "the account told it: nothing to sync");
  source.close();
  assert.equal(acct.watchers, 0, "a closed source no longer listens");
});

test("the host's new key reaches every stream at once — not behind an earlier change's key loading that a hub stalls with page fetches", async () => {
  const [old, cur] = [await signer(0), await signer(1)];
  const acct = await account("removalKeys");
  const machine = await makeMachine();
  let fetches = 0;
  const { link, source, client } = await following(acct, machine, { fetchPage: () => (fetches++, new Promise<Bytes>(() => undefined)) });
  try {
    assert.equal(hex(client.machineSignKey), hex(old.pub));
    // Stream A: the hub hands a checkpoint and never the pages it names — A's queue waits on the fetch.
    const a = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
    await a.append({ upserts: [row("a", "2026-10-15T00:00:00.000Z")], removed: [], asOf: 1 });
    client.subscribe(a.stream, indexFold, {});
    await until(() => link.subs().length === 1);
    const stalled = await a.manifest([{ upserts: [row("a", "2026-10-15T00:00:00.000Z")], removed: [], asOf: 1 }], 1_000, old.key);
    link.deliver({ type: "checkpoint", ch: link.lastSub().ch, stream: a.stream, uptoSeq: stalled.upto, manifest: stalled.manifest });
    await until(() => fetches > 0);
    // Stream B: another stream of the host, followed meanwhile.
    const b = await StreamWriter.open(machine, crypto.getRandomValues(new Uint8Array(32)), FMT_INDEX_V1);
    await b.append({ upserts: [row("b", "2026-10-15T00:00:00.000Z")], removed: [], asOf: 1 });
    const states: IndexState[] = [];
    const errors: unknown[] = [];
    client.subscribe(b.stream, indexFold, { onState: (s) => states.push(s), onError: (e) => errors.push(e) });
    await until(() => link.subs().length === 2);
    const subB = link.lastSub();
    // A new key epoch: its loading waits on every stream's queue, A's among them.
    await rotate(machine, 1);
    acct.poke();
    await flush();
    // The renewal lands next; then the hub (or whoever holds the retired key) serves B a head under the old key: refused.
    await acct.advance("rotateKey");
    link.deliver({ type: "recs", ch: subB.ch, stream: b.stream, records: b.records });
    link.deliver({ type: "head", ch: subB.ch, head: await b.head(2_000, undefined, old.key) });
    await until(() => states.length + errors.length > 0);
    assert.equal(states.length, 0, "nothing under the retired key is handed out");
    assert.ok(badHead(errors[0]), String(errors[0]));
    assert.equal(hex(client.machineSignKey), hex(cur.pub), "the client holds the chain's new key, whatever is still loading");
  } finally {
    source.close();
  }
});

test("a position this device kept under the host's retired key still holds after the renewal (keysAt), and is read on from", async () => {
  const [old, cur] = [await signer(0), await signer(1)];
  const stranger = await generateDeviceKeys();
  const machine = await makeMachine();
  const writer = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  await writer.append({ upserts: [row("a", "2026-10-15T00:00:00.000Z")], removed: [], asOf: 1 });
  const kept = await writer.head(5_000, undefined, old.key);
  const acct = await account("rotateKey");
  // The retired key is the one `keysAt` gives for the chain before the renewal.
  assert.equal(hex(acct.chain.keysAt(HOST, seqOf("removalKeys"))!.signKey), hex(old.pub));

  // LogReader: a kept head may carry a retired key; every head after it must carry the current one.
  await assert.rejects(LogReader.fromHead(cur.pub, decodeSignedHead(kept)), badHead, "not as a head from the network");
  const reader = await LogReader.fromHead(cur.pub, decodeSignedHead(kept), [old.pub]);
  assert.equal(hex(reader.machineKey), hex(cur.pub));
  await reader.addEpoch(0, machine.mk);
  await writer.append({ upserts: [row("b", "2026-10-16T00:00:00.000Z")], removed: [], asOf: 2 });
  await reader.push(writer.records[1]!);
  await assert.rejects(reader.confirm(decodeSignedHead(await writer.head(6_000, undefined, old.key))), badHead);
  assert.equal((await reader.confirm(decodeSignedHead(await writer.head(6_000, undefined, cur.key)))).length, 1);

  // The copy source: a mark kept under the old key resumes; a stranger's does not.
  const { source } = await following(acct, machine);
  const mark = await source.resume(hex(writer.stream), hex(kept));
  assert.equal(mark?.seq, 1);
  assert.equal(mark?.timeMs, 5_000);
  assert.equal(await source.resume(hex(writer.stream), hex(await writer.head(5_000, undefined, stranger.sign.privateKey))), null);
  source.close();

  // The offline copy saved before the renewal is shown at once, and followed on from where it stands.
  const kv = new FakeKv();
  await putCopy(kv, "a-1", HOST, hex(writer.stream), { kind: "index", head: hex(kept), state: { rows: { "claude/a": row("a", "2026-10-15T00:00:00.000Z") }, asOf: 1 }, freshestMs: 5_000, savedAt: 1 });
  const world = await following(acct, machine);
  const mirror = new MachineMirror({ account: "a-1", machine: HOST, source: world.source, transact: kv.transact, saveDelayMs: { small: 1, session: 1 } });
  await mirror.start();
  assert.equal(mirror.hasCopy, true, "the copy did not vanish with the key");
  assert.deepEqual(mirror.rows()?.map((r) => r.id), ["a"]);
  await until(() => world.link.subs().some((s) => hex(s.stream) === hex(writer.stream)));
  const sub = world.link.subs().find((s) => hex(s.stream) === hex(writer.stream))!;
  assert.equal(sub.fromSeq, 1, "from the kept position, not from the start");
  world.link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records: writer.records.slice(1) });
  world.link.deliver({ type: "head", ch: sub.ch, head: await writer.head(7_000, undefined, cur.key) });
  await until(() => mirror.rows()?.length === 2);
  assert.deepEqual(mirror.rows()?.map((r) => r.id), ["b", "a"]);
  assert.equal(acct.syncs, 0);
  mirror.close();
  world.source.close();
});

test("R6: a kept position verifies under the host's key of the seq it was stored at (keysAt) — not under any key it ever held", async () => {
  const keys = [await signer(0), await signer(1), await signer(2)];
  const machine = await makeMachine();
  const writer = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  await writer.append({ upserts: [row("a", "2026-10-15T00:00:00.000Z")], removed: [], asOf: 1 });
  // Two renewals: key 0 until "rotate" (11), key 1 until "rotateAgain" (13), key 2 since.
  const acct = await account("rotateAgainKey");
  assert.equal(hex(acct.chain.keysAt(HOST, seqOf("rotateKey"))!.signKey), hex(keys[1]!.pub));
  const { source } = await following(acct, machine);
  const at = async (i: number, keySeq: number | null) => {
    const head = hex(await writer.head(5_000, undefined, keys[i]!.key));
    return source.resume(hex(writer.stream), keySeq === null ? head : `${head}@${keySeq}`);
  };
  const stored = seqOf("rotateKey");
  const kept = await at(1, stored);
  assert.equal(kept?.seq, 1, "key 1, kept while key 1 was the host's");
  assert.equal(kept?.mark.endsWith(`@${stored}`), true, "and kept so again");
  assert.equal(await at(0, stored), null, "key 0 is retired too, but it was not the key at that seq");
  assert.ok(await at(2, stored), "the current key always is");
  // A mark from before marks carried a seq: the host's first key only.
  assert.ok(await at(0, null));
  assert.equal(await at(1, null), null);
  // Not a mark at all.
  assert.equal(await source.resume(hex(writer.stream), `${hex(await writer.head(5_000, undefined, keys[1]!.key))}@x`), null);
  source.close();
});

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

/** The hub's connection as the channel uses it: frames to the agent, the agent's back to the page. */
class Hub {
  agent!: Agent;
  readonly carriesChannels = true;
  readonly presence = new Map<string, never>();
  private readonly routes = new Map<number, (f: Frame) => void>();
  private next = 1;
  waitOpen(): Promise<void> {
    return Promise.resolve();
  }
  attest(): void {}
  onPresence(): () => void {
    return () => undefined;
  }
  reserve(route: (f: Frame) => void): number {
    const ch = this.next++;
    this.routes.set(ch, route);
    return ch;
  }
  release(ch: number): void {
    this.routes.delete(ch);
  }
  send(frame: Frame): boolean {
    queueMicrotask(() => void this.agent.onFrame(frame));
    return true;
  }
  toPage(frame: Frame & { ch: number }): void {
    queueMicrotask(() => this.routes.get(frame.ch)?.(frame));
  }
}

/**
 * h-2's agent, played here: it answers a hello sealed to the DH key it holds
 * now and refuses (4401, as `link.rs` does) one it cannot open; its bridge
 * answers every request with the key it holds.
 */
class Agent {
  /** What became of each hello: "open", "refused" (4401), "garbage" (a reply that does not open). */
  hellos: string[] = [];
  /** The channels the page closed. */
  closed: number[] = [];
  /** Answer this many hellos with a reply that does not open. */
  garbage = 0;
  /** Hand the next hello these entries (its renewal), then hold `then` (it promotes). */
  teach: { entries: Entry[]; then: NoiseKeyPair } | null = null;
  private readonly drivers = new Map<number, ChannelDriver>();

  constructor(
    private readonly hub: Hub,
    public dh: NoiseKeyPair,
    private readonly chainAccount: string,
  ) {}

  async onFrame(frame: Frame): Promise<void> {
    if (frame.type === "open" || !("ch" in frame)) return;
    if (frame.type === "close") this.closed.push(frame.ch);
    const driver = this.drivers.get(frame.ch);
    if (driver) return driver.receive(frame);
    if (frame.type !== "data") return;
    const ch = frame.ch;
    const r = await Responder.create(this.dh, { account: this.chainAccount, device: "d-web", machine: HOST });
    let body: ReturnType<typeof decodeHelloBody>;
    try {
      body = decodeHelloBody((await r.readHello(frame.payload)).payload);
    } catch {
      this.hellos.push("refused");
      this.hub.toPage({ type: "close", ch, code: AGENT_HANDSHAKE_REFUSED, reason: "handshake refused" });
      return;
    }
    if (this.garbage > 0) {
      this.garbage--;
      this.hellos.push("garbage");
      this.hub.toPage({ type: "data", ch, payload: crypto.getRandomValues(new Uint8Array(96)) });
      return;
    }
    this.hellos.push("open");
    const teach = this.teach;
    this.teach = null;
    const entries = teach ? teach.entries.filter((e) => e.draft.seq > body.chainSeq) : [];
    const last = entries.at(-1);
    const reply = encodeReplyBody({
      status: "accepted",
      chainSeq: last ? last.draft.seq : body.chainSeq,
      chainHead: last ? h(R.hashes[last.draft.seq]) : body.chainHead,
      needFrom: 0,
      server: utf8("fake agent"),
      entries,
    });
    const { message, transport } = await r.writeReply(reply, DEFAULT_REKEY, Date.now());
    const holds = this.dh.publicKey;
    if (teach) this.dh = teach.then;
    this.hub.toPage({ type: "data", ch, payload: message });
    const d = new ChannelDriver(ch, transport, "responder", (f) => this.hub.toPage(f as Frame & { ch: number }), Date.now, {});
    d.onincoming = (stream) => this.serve(stream, holds);
    this.drivers.set(ch, d);
  }

  private serve(stream: ChannelStream, holds: Uint8Array): void {
    const reader = new MessageReader();
    stream.ondata = (data) => {
      reader.push(data);
      for (let m = reader.next(); m; m = reader.next()) {
        if (m.kind !== "end") continue;
        stream.send(bridgeMessage("head", encodeResponseHead({ status: 200, headers: [["content-type", "application/json"]] })));
        stream.send(bridgeMessage("binary", utf8(JSON.stringify({ key: hex(holds) }))));
        stream.close(0);
      }
    };
  }
}

/** A channel to h-2 for `acct`, its agent holding `dh`. */
function channelTo(acct: Awaited<ReturnType<typeof account>>, dh: NoiseKeyPair) {
  const hub = new Hub();
  const agent = new Agent(hub, dh, acct.chain.account);
  hub.agent = agent;
  const channel = new MachineChannel(hub as unknown as HubLink, acct as unknown as CloudAccount, HOST);
  /** Which key answered a request over the channel. */
  const ask = async () => ((await (await tunnelTransport(channel).fetch(new Request("http://machine/api/key", { method: "POST", body: "?" }))).json()) as { key: string }).key;
  return { hub, agent, channel, ask };
}

test("a live channel to a key the chain has replaced is closed at once, and the next request goes to the new key", async () => {
  const [k0, k1] = [await dhOf(0), await dhOf(1)];
  const acct = await account("removalKeys");
  const { agent, channel, ask } = channelTo(acct, k0);
  assert.equal(channel.responderKey, null, "no channel yet");
  assert.equal(await ask(), hex(k0.publicKey));
  assert.equal(channel.status, "open");
  assert.equal(hex(channel.responderKey!), hex(k0.publicKey), "opened to the chain's key");
  assert.equal(acct.watchers, 1, "it listens to the account while the channel is up");

  // The host renews and promotes; the page hears of it from the hub.
  agent.dh = k1;
  await acct.advance("rotateKey");
  assert.equal(channel.status, "idle", "closed the moment the account heard");
  assert.equal(channel.responderKey, null);
  assert.equal(acct.watchers, 0, "and it no longer listens");
  await until(() => agent.closed.length === 1);
  assert.equal(agent.closed[0], 1, "the agent was told the old channel is over");

  assert.equal(await ask(), hex(k1.publicKey), "the next request opens a channel to the new key");
  assert.equal(hex(channel.responderKey!), hex(k1.publicKey));
  assert.deepEqual(agent.hellos, ["open", "open"]);
  assert.equal(acct.syncs, 0);
  assert.equal(acct.watchers, 1);
  // A change that is not the host's key leaves the channel alone.
  await acct.advance("rotateKey");
  assert.equal(channel.status, "open");
  channel.close();
  assert.equal(acct.watchers, 0, "closing it lets go of the account");
  assert.equal(channel.responderKey, null);
});

test("a hello the agent cannot open (it renewed before this device heard): the chain is synced once and the handshake tried once more", async () => {
  const k1 = await dhOf(1);
  const acct = await account("removalKeys");
  acct.onSync = () => acct.advance("rotateKey");
  const { agent, channel, ask } = channelTo(acct, k1);
  assert.equal(await ask(), hex(k1.publicKey));
  assert.deepEqual(agent.hellos, ["refused", "open"]);
  assert.equal(acct.syncs, 1);
  assert.equal(hex(channel.responderKey!), hex(k1.publicKey));
  channel.close();

  // A key the chain never gives (somebody else's renewal it has not seen): one sync, and no more than the usual tries.
  const stuck = await account("removalKeys");
  const other = channelTo(stuck, k1);
  await assert.rejects(other.channel.ensure(), /refused the channel/);
  assert.equal(stuck.syncs, 1, "synced once, however many tries");
  assert.ok(other.agent.hellos.every((x) => x === "refused"));
  assert.equal(other.channel.responderKey, null);
  assert.equal(stuck.watchers, 0, "no channel, no listener");
});

test("a reply that does not open: one sync and one more try, and then the failure stands", async () => {
  const k1 = await dhOf(1);
  {
    const acct = await account("rotateKey");
    const { agent, channel, ask } = channelTo(acct, k1);
    agent.garbage = 1;
    assert.equal(await ask(), hex(k1.publicKey));
    assert.deepEqual(agent.hellos, ["garbage", "open"]);
    assert.equal(acct.syncs, 1);
    channel.close();
  }
  {
    const acct = await account("rotateKey");
    const { agent, channel } = channelTo(acct, k1);
    agent.garbage = 5;
    await assert.rejects(channel.ensure());
    assert.deepEqual(agent.hellos, ["garbage", "garbage"], "tried once more, not again");
    assert.equal(acct.syncs, 1);
    assert.equal(channel.status, "error");
    assert.equal(acct.watchers, 0);
  }
});

test("a reply that teaches this device the host's renewal: no channel to the old key, the next try goes to the new one", async () => {
  const [k0, k1] = [await dhOf(0), await dhOf(1)];
  const acct = await account("removalKeys");
  const { agent, channel, ask } = channelTo(acct, k0);
  // The agent still holds its old key, has the renewal on its chain, and promotes right after this reply.
  agent.teach = { entries: entriesOf(R.entries).slice(0, seqOf("rotateKey") + 1), then: k1 };
  assert.equal(await ask(), hex(k1.publicKey));
  assert.equal(acct.chain.seq, seqOf("rotateKey"), "the reply's entries were merged");
  assert.deepEqual(agent.hellos, ["open", "open"]);
  await until(() => agent.closed.includes(1));
  assert.equal(hex(channel.responderKey!), hex(k1.publicKey));
  assert.equal(acct.syncs, 0);
  channel.close();
  await flush(5);
});

test("an isolated host gets no channel, and a live one to it closes — but for the one a renewal confirmed on the device opens for its call (§3.8)", async () => {
  const k0 = await dhOf(0);
  const acct = await account("removalKeys");
  const { hub, agent, channel, ask } = channelTo(acct, k0);
  // Open while it is not isolated; isolated, the channel goes at once.
  assert.equal(await ask(), hex(k0.publicKey));
  acct.isolatedNow = true;
  acct.poke();
  assert.equal(channel.status, "idle", "closed when it became isolated");
  // No new one: not even a handshake goes out.
  await assert.rejects(channel.ensure(), (e: unknown) => e instanceof ChannelIsolated);
  await assert.rejects(ask());
  assert.deepEqual(agent.hellos, ["open"], "nothing reached the agent");
  // The renewal's own channel (the hub routes it only for an open assurance-1 attempt).
  const renewal = new MachineChannel(hub as unknown as HubLink, acct as unknown as CloudAccount, HOST, { isolatedRenewal: true });
  const res = await tunnelTransport(renewal).fetch(new Request("http://machine/rh/rotate", { method: "POST", body: "{}" }));
  assert.equal(res.status, 200);
  assert.deepEqual(agent.hellos, ["open", "open"]);
  // A change of the account leaves it be (it is closed by its caller, after its one call).
  acct.poke();
  assert.equal(renewal.status, "open");
  renewal.close();
});

test("Codex 1: an open channel is not reused to a host isolated meanwhile — even when no change was heard", async () => {
  const k0 = await dhOf(0);
  const acct = await account("removalKeys");
  const { agent, channel, ask } = channelTo(acct, k0);
  assert.equal(await ask(), hex(k0.publicKey));
  // Isolated by another tab's dispute, with no change told here (no poke).
  acct.isolatedNow = true;
  await assert.rejects(channel.ensure(), (e: unknown) => e instanceof ChannelIsolated);
  assert.equal(channel.status, "idle", "the open one closed");
  await assert.rejects(ask());
  assert.deepEqual(agent.hellos, ["open"], "nothing new reached the agent");
});

test("Codex 7: a stream head checked under a key the machine renewed away from meanwhile is not committed — it is held to the new one", async () => {
  const machine = await makeMachine();
  const renewed = await generateDeviceKeys();
  const stream = crypto.getRandomValues(new Uint8Array(32));
  const writer = await StreamWriter.open(machine, stream, 1);
  const record = await writer.append({ a: 1 });
  const reader = LogReader.fromStart(stream, machine.signPublic);
  await reader.addEpoch(0, machine.mk);
  await reader.push(record);
  const head = decodeSignedHead(await writer.head());
  const subtle = crypto.subtle as unknown as { verify: (...args: unknown[]) => Promise<boolean> };
  const verify = subtle.verify;
  // The chain renews the machine's key while the head's signature is being checked.
  subtle.verify = function (...args: unknown[]) {
    delete (subtle as { verify?: unknown }).verify;
    reader.setMachineKey(renewed.signPublic);
    return verify.apply(crypto.subtle, args);
  };
  try {
    await assert.rejects(reader.confirm(head), (e: unknown) => e instanceof RecordError && e.kind === "badHead");
  } finally {
    delete (subtle as { verify?: unknown }).verify;
  }
  assert.equal(reader.confirmed, 0, "nothing released under the retired key");
  assert.equal(reader.confirmedHead, null);
});

test("M2: a reader built while the machine renews its key is held to the current key before it is installed — a head under the retired key is refused", async () => {
  const machine = await makeMachine();
  const renewed = await generateDeviceKeys();
  const stream = crypto.getRandomValues(new Uint8Array(32));
  const writer = await StreamWriter.open(machine, stream, FMT_INDEX_V1);
  await writer.append({ upserts: [{ source: "claude", id: "a", title: "a", updatedAt: "2026-10-15T00:00:00.000Z" }], removed: [] });
  const link = new FakeStreamLink();
  const states: IndexState[] = [];
  const errors: unknown[] = [];
  const client = new StreamClient({
    link,
    machineSignKey: machine.signPublic,
    epochs: machine.epochs,
    fetchPage: async () => {
      throw new Error("no pages here");
    },
    retryDelay: () => 60_000,
  });
  // The renewal lands while the reader's epochs are being installed: it reaches no reader yet.
  const keyed = client.keyed.bind(client);
  let once = true;
  client.keyed = async (reader) => {
    const out = await keyed(reader);
    if (once) {
      once = false;
      client.setMachineSignKey(renewed.signPublic);
    }
    return out;
  };
  client.subscribe(stream, indexFold, { onState: (st) => states.push(st), onError: (e) => errors.push(e) });
  await until(() => link.subs().length === 1);
  const ch = link.lastSub().ch;
  link.deliver({ type: "recs", ch, stream, records: writer.records });
  link.deliver({ type: "head", ch, head: await writer.head(1) });
  await until(() => errors.length > 0);
  assert.equal(states.length, 0, "nothing under the retired key");
  assert.ok(errors.some((e) => e instanceof RecordError && e.kind === "badHead"));
});
