// The browser's end-to-end layer on its own: handshake, transport, mux, relay,
// key storage and the feature check.
//
// What these pin: a channel only forms between the right keys under the same
// binding; once formed, a forged, replayed, reordered or dropped message kills
// it rather than being accepted; rekeying keeps both ends in step and a peer
// that never rekeys is cut off; a relay that forwards hub frames between a
// device and an agent never sees a canary sent through it; the mux honours
// credit and refuses a peer that ignores it; device keys are non-extractable
// once loaded and come back from the store; and the support check names what is missing.
import assert from "node:assert/strict";
import test from "node:test";
import {
  type Binding,
  ChannelError,
  checkE2eSupport,
  decodeFrame,
  encodeFrame,
  FrameDecoder,
  fromRaw,
  generateDeviceKeys,
  Initiator,
  loadDeviceKeys,
  MAX_PAYLOAD_LEN,
  MAX_TRANSPORT_DATA,
  MemoryKeyStore,
  Mux,
  MuxError,
  type NoiseKeyPair,
  Responder,
  Session,
  type Frame,
  type RekeyPolicy,
  DEFAULT_REKEY,
  utf8,
  fromUtf8,
  concat,
  sealRecord,
  importMachineKey,
  streamId,
  streamKey,
  openRecord,
  BUCKETS,
} from "../src/lib/e2e/index.js";

const binding: Binding = { account: "acct", device: "dev-0", machine: "m-1" };

async function pair(): Promise<NoiseKeyPair> {
  const keys = await generateDeviceKeys();
  return { privateKey: keys.dh.privateKey, publicKey: keys.dhPublic };
}

async function handshake(policy: RekeyPolicy = DEFAULT_REKEY, responderPolicy: RekeyPolicy = policy) {
  const device = await pair();
  const agent = await pair();
  const i = await Initiator.create(device, agent.publicKey, binding);
  const r = await Responder.create(agent, binding);
  const hello = await r.readHello(await i.writeHello(1000, utf8("first")));
  assert.equal(fromUtf8(hello.payload), "first");
  const { message, transport: rt } = await r.writeReply(utf8("ok"), responderPolicy, 1000);
  const { transport: it } = await i.readReply(message, policy, 1000);
  return { it, rt, device, agent };
}

const isChannel = (kind: ChannelError["kind"]) => (e: unknown) => e instanceof ChannelError && e.kind === kind;

test("a handshake under a different binding or to the wrong agent fails", async () => {
  const device = await pair();
  const agent = await pair();
  for (const other of [{ ...binding, account: "acct-2" }, { ...binding, device: "dev-1" }, { ...binding, machine: "m-2" }]) {
    const i = await Initiator.create(device, agent.publicKey, binding);
    const r = await Responder.create(agent, other);
    await assert.rejects(r.readHello(await i.writeHello(1, utf8("x"))), isChannel("decrypt"));
  }
  const impostor = await pair();
  const i = await Initiator.create(device, agent.publicKey, binding);
  await assert.rejects((await Responder.create(impostor, binding)).readHello(await i.writeHello(1, utf8("x"))), isChannel("decrypt"));
  await assert.rejects(Initiator.create(device, new Uint8Array(32), binding), isChannel("lowOrderKey"));
});

test("forged, replayed, reordered and dropped transport messages kill the channel", async () => {
  let { it, rt } = await handshake();
  const m = await it.seal(1, utf8("a"));
  m[5]! ^= 1;
  await assert.rejects(rt.open(m), isChannel("decrypt"));
  await assert.rejects(rt.open(await it.seal(1, utf8("b"))), isChannel("poisoned"));
  assert.equal(rt.isPoisoned, true);

  ({ it, rt } = await handshake());
  const a = await it.seal(1, utf8("a"));
  await rt.open(a);
  await assert.rejects(rt.open(a), isChannel("decrypt"), "replay");

  ({ it, rt } = await handshake());
  await it.seal(1, utf8("a"));
  const b = await it.seal(1, utf8("b"));
  await assert.rejects(rt.open(b), isChannel("decrypt"), "reorder / drop");

  ({ it, rt } = await handshake());
  await assert.rejects(it.open(await it.seal(1, utf8("a"))), isChannel("decrypt"), "reflection");
  ({ it, rt } = await handshake());
  await assert.rejects(it.seal(1, new Uint8Array(MAX_TRANSPORT_DATA + 1)), isChannel("tooLarge"));
  assert.equal(fromUtf8(await rt.open(await it.seal(1, utf8("still fine")))), "still fine", "an oversize refusal is not a failure");
});

test("rekeying by count and age keeps both ends in step; a peer that never rekeys is cut off", async () => {
  const policy = { maxMessages: 3, maxAgeMs: 10_000 };
  const { it, rt } = await handshake(policy);
  for (let n = 0; n < 12; n++) {
    const now = n < 7 ? 1000 + n : 30_000 + n;
    assert.equal(fromUtf8(await rt.open(await it.seal(now, utf8(`m${n}`)))), `m${n}`);
    assert.equal(fromUtf8(await it.open(await rt.seal(now, utf8(`r${n}`)))), `r${n}`);
  }
  // The device thinks it may send 100 messages per key; the agent allows 3.
  const lax = await handshake({ maxMessages: 100, maxAgeMs: 1e12 }, { maxMessages: 3, maxAgeMs: 1e12 });
  await lax.rt.open(await lax.it.seal(1, utf8("1")));
  await lax.rt.open(await lax.it.seal(1, utf8("2")));
  await assert.rejects(lax.rt.open(await lax.it.seal(1, utf8("3"))), isChannel("rekeyOverdue"));
});

test("a device and an agent talk through a relay that never sees the canary", async () => {
  const CANARY = "RH-CANARY-ts-7a1f-do-not-relay";
  const device = await pair();
  const agent = await pair();
  const seen: Uint8Array[] = [];
  const relay = (frames: Frame[]): Frame[] => {
    // A relay that parses envelopes and nothing else.
    const decoder = new FrameDecoder();
    for (const f of frames) {
      const bytes = encodeFrame(f);
      seen.push(bytes);
      decoder.push(bytes);
    }
    const out: Frame[] = [];
    for (let raw = decoder.next(); raw; raw = decoder.next()) out.push(fromRaw(raw));
    return out;
  };

  const i = await Initiator.create(device, agent.publicKey, binding);
  const [, hello] = relay([{ type: "open", ch: 1, peer: "m-1" }, { type: "data", ch: 1, payload: await i.writeHello(1, utf8("hello")) }]);
  const r = await Responder.create(agent, binding);
  await r.readHello((hello as Extract<Frame, { type: "data" }>).payload);
  const reply = await r.writeReply(utf8("features"), DEFAULT_REKEY, 1);
  const [back] = relay([{ type: "data", ch: 1, payload: reply.message }]);
  const { transport } = await i.readReply((back as Extract<Frame, { type: "data" }>).payload, DEFAULT_REKEY, 1);

  const dev = new Session(1, transport, "initiator");
  const ag = new Session(1, reply.transport, "responder");
  const term = dev.mux.open("term", utf8("t-1"));
  dev.mux.send(term, utf8(CANARY));
  let received = "";
  for (const f of relay(await dev.flush(1))) {
    const event = await ag.receive((f as Extract<Frame, { type: "data" }>).payload);
    if (event?.type === "data") {
      received += fromUtf8(event.data);
      ag.mux.send(event.sid, concat(utf8("echo:"), event.data));
    }
  }
  let echoed = "";
  for (const f of relay(await ag.flush(1))) {
    const event = await dev.receive((f as Extract<Frame, { type: "data" }>).payload);
    if (event?.type === "data") echoed += fromUtf8(event.data);
  }
  assert.equal(received, CANARY);
  assert.equal(echoed, `echo:${CANARY}`);
  const wire = Buffer.concat(seen);
  assert.ok(wire.length > CANARY.length * 2);
  assert.equal(wire.includes(Buffer.from(CANARY)), false, "the relay never saw the canary");
  assert.equal(wire.includes(Buffer.from("hello")), false, "nor the 0-RTT payload");
});

test("the mux honours credit, and refuses a peer that ignores it or breaks id rules", () => {
  const a = new Mux("initiator");
  const b = new Mux("responder", 1000);
  const sid = b.open("blob");
  for (let f = b.poll(); f; f = b.poll()) a.receive(f);
  a.send(sid, new Uint8Array(2500));
  let sent = 0;
  for (let f = a.poll(); f; f = a.poll()) {
    const e = b.receive(f);
    if (e?.type === "data") sent += e.data.length;
  }
  assert.equal(sent, 1000);
  b.consumed(sid, 1000);
  for (let f = b.poll(); f; f = b.poll()) a.receive(f);
  for (let f = a.poll(); f; f = a.poll()) {
    const e = b.receive(f);
    if (e?.type === "data") sent += e.data.length;
  }
  assert.equal(sent, 2000);
  assert.throws(() => b.receive({ type: "data", sid, data: new Uint8Array(1001) }), (e: MuxError) => e.kind === "flowControl");
  assert.throws(() => b.receive({ type: "open", sid: 2, class: "rpc", window: 1, meta: new Uint8Array(0) }), MuxError);
  assert.throws(() => b.receive({ type: "data", sid: 99, data: new Uint8Array(0) }), (e: MuxError) => e.kind === "unknownStream");
});

test("hub frames: the size limit holds and a split stream reassembles", () => {
  assert.equal(encodeFrame({ type: "data", ch: 1, payload: new Uint8Array(MAX_PAYLOAD_LEN) }).length, 64 * 1024);
  assert.throws(() => encodeFrame({ type: "data", ch: 1, payload: new Uint8Array(MAX_PAYLOAD_LEN + 1) }));
  assert.throws(() => decodeFrame(Uint8Array.of(0x7f, 0, 0, 0, 0, 0, 0)), /critical/);
  assert.equal(decodeFrame(Uint8Array.of(0x90, 0, 0, 0, 0, 0, 0)).type, "unknown");
  const frames: Frame[] = [
    { type: "open", ch: 1, peer: "m" },
    { type: "data", ch: 1, payload: utf8("x".repeat(300)) },
    { type: "ping", nonce: 5 },
  ];
  const wire = concat(...frames.map(encodeFrame));
  for (const size of [1, 5, 64]) {
    const d = new FrameDecoder();
    const out: Frame[] = [];
    for (let at = 0; at < wire.length; at += size) {
      d.push(wire.subarray(at, at + size));
      for (let raw = d.next(); raw; raw = d.next()) out.push(fromRaw(raw));
    }
    assert.deepEqual(out, frames);
  }
});

test("records seal and open in WebCrypto, padded to a bucket", async () => {
  const mk = await importMachineKey(new Uint8Array(32).fill(3));
  const stream = await streamId(mk, "claude", "s-1");
  const key = await streamKey(mk, stream);
  for (const body of [utf8(""), utf8("short"), utf8("y".repeat(20_000))]) {
    const sealed = await sealRecord(key, stream, 0, 0, 1, new Uint8Array(32), body);
    assert.ok((BUCKETS as readonly number[]).includes(sealed.ct.length - 16));
    assert.deepEqual((await openRecord(key, stream, sealed)).body, body);
  }
});

test("device keys are non-extractable once loaded, and the stored ones come back", async () => {
  const store = new MemoryKeyStore();
  const keys = (await loadDeviceKeys(store)).keys;
  assert.equal(keys.sign.privateKey.extractable, false);
  assert.equal(keys.dh.privateKey.extractable, false);
  await assert.rejects(crypto.subtle.exportKey("pkcs8", keys.dh.privateKey));
  assert.equal(keys.dhPublic.length, 32);
  const again = (await loadDeviceKeys(store)).keys;
  // Opened from the store again (a new `CryptoKey`), but the same keys, not new ones.
  assert.deepEqual(again.dhPublic, keys.dhPublic, "the stored key comes back, not a new one");
  assert.deepEqual(again.signPublic, keys.signPublic, "the stored key comes back, not a new one");
  assert.equal(again.dh.privateKey.extractable, false);
});

test("the support check passes on node's WebCrypto and names what a browser lacks", async () => {
  assert.deepEqual(await checkE2eSupport({ requireIndexedDb: false }), { ok: true, missing: [] });
  // A browser whose WebCrypto predates the 25519 curves.
  const subtle = crypto.subtle;
  const old = {
    subtle: new Proxy(subtle, {
      get(target, prop) {
        if (prop === "generateKey") {
          return (alg: { name: string }, ...rest: unknown[]) =>
            /25519/.test(alg.name) ? Promise.reject(new Error("NotSupportedError")) : (target.generateKey as (...a: unknown[]) => unknown)(alg, ...rest);
        }
        const value = Reflect.get(target, prop);
        return typeof value === "function" ? value.bind(target) : value;
      },
    }),
  } as unknown as Crypto;
  const result = await checkE2eSupport({ crypto: old, requireIndexedDb: false });
  assert.equal(result.ok, false);
  assert.deepEqual(result.missing, ["X25519", "Ed25519"]);
  assert.deepEqual((await checkE2eSupport({ crypto: {} as Crypto })).ok, false);
});
