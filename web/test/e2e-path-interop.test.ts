// The direct path, Rust → TypeScript: replay the `path` section of
// `server-rs/crates/droppen-proto/vectors/rust.json` through `lib/e2e/path.ts`.
//
// What these pin: envelopes, signals and DataChannel messages encode to
// Rust's bytes and decode back, and the junk Rust refuses is refused here;
// SDP fingerprints parse the same way, and a tampered one is refused; the
// direct prologue is the same bytes; given the same keys and ephemerals the
// WebCrypto Noise IK under that prologue writes exactly Rust's first message,
// reads Rust's reply, and seals the same sequenced envelopes; the device's
// direct hello is a normal chain hello the machine reads only under the very
// binding the device used.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  ChainClock,
  ChainState,
  checkFingerprint,
  CLIENT_FEATURE,
  clientWantsPaths,
  DEFAULT_REKEY,
  decodeDcMessage,
  decodeEntry,
  decodeEnvelope,
  decodeHelloBody,
  decodeSignal,
  directPrologue,
  type DcMessage,
  type DirectBinding,
  encodeDcMessage,
  encodeEnvelope,
  encodeSignal,
  ENVELOPE_HEADER,
  type Envelope,
  fingerprintHex,
  fromHex,
  fromUtf8,
  HandshakeError,
  hex,
  Initiator,
  keyPairFromSecret,
  MAX_CANDIDATE,
  MAX_CHUNK,
  MAX_PAYLOAD_LEN,
  MAX_SDP,
  parseFingerprintHex,
  PathError,
  prologue,
  Responder,
  sdpFingerprint,
  serverFeatures,
  type Signal,
  SIGNAL_PATH,
  startDirectHandshake,
  stripCandidates,
  utf8,
} from "../src/lib/e2e/index.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Json = any;
const V: Json = JSON.parse(readFileSync(new URL("../../server-rs/crates/droppen-proto/vectors/rust.json", import.meta.url), "utf8"));
const P: Json = V.path;
const h = (s: string) => fromHex(s);
const DOWN = ["closed", "timeout", "failed"] as const;
const BYE = ["done", "disabled", "busy", "failed", "badFingerprint", "refused"] as const;

function envelopeFromJson(j: Json): Envelope {
  switch (j.type) {
    case "data":
      return { type: "data", seq: j.seq, ack: j.ack, frame: h(j.frame) };
    case "ack":
      return { type: "ack", ack: j.ack };
    case "probe":
    case "probeAck":
      return { type: j.type, nonce: j.nonce };
    case "down":
      return { type: "down", reason: DOWN[j.reason]! };
  }
  throw new Error(`unknown envelope ${j.type}`);
}

function signalFromJson(j: Json): Signal {
  switch (j.type) {
    case "offer":
      return { type: "offer", id: j.id, fp: h(j.fp), sdp: j.sdp };
    case "answer":
      return { type: "answer", id: j.id, fp: h(j.fp), nonce: h(j.nonce), sdp: j.sdp };
    case "candidate":
      return { type: "candidate", id: j.id, candidate: j.candidate };
    case "bye":
      return { type: "bye", id: j.id, reason: BYE[j.reason]!, detail: j.detail };
  }
  throw new Error(`unknown signal ${j.type}`);
}

test("the path constants are Rust's", () => {
  assert.equal(P.feature, CLIENT_FEATURE);
  assert.equal(P.signalPath, SIGNAL_PATH);
  assert.equal(P.envelopeHeader, ENVELOPE_HEADER);
  assert.equal(P.maxChunk, MAX_CHUNK, "a mux chunk leaves room for the envelope");
});

test("envelopes encode to Rust's bytes and decode back; junk is refused", () => {
  assert.equal(P.envelopes.length, 6);
  for (const { envelope, hex: bytes } of P.envelopes) {
    const built = envelopeFromJson(envelope);
    assert.equal(hex(encodeEnvelope(built)), bytes, envelope.type);
    assert.deepEqual(decodeEnvelope(h(bytes)), built, envelope.type);
  }
  assert.equal(encodeEnvelope({ type: "data", seq: 1, ack: 0, frame: new Uint8Array(10) }).length, ENVELOPE_HEADER + 10);
  assert.throws(() => decodeEnvelope(new Uint8Array(0)));
  assert.throws(() => decodeEnvelope(Uint8Array.of(9, 0)), "unknown kind");
  const zero = encodeEnvelope({ type: "data", seq: 1, ack: 0, frame: new Uint8Array(0) });
  zero[8] = 0;
  assert.throws(() => decodeEnvelope(zero), "seq 0");
  const trailing = Uint8Array.of(...encodeEnvelope({ type: "ack", ack: 1 }), 0);
  assert.throws(() => decodeEnvelope(trailing), "trailing byte");
  const big = encodeEnvelope({ type: "ack", ack: 1 });
  big[1] = 0xff;
  assert.throws(() => decodeEnvelope(big), "above 2^53 - 1");
  assert.throws(() => decodeEnvelope(Uint8Array.of(0x05, 9)), "unknown reason");
});

test("DataChannel messages are Rust's; an empty or oversized one is refused", () => {
  for (const { message, hex: bytes } of P.dc) {
    const built: DcMessage = { type: message.type, body: h(message.body) };
    assert.equal(hex(encodeDcMessage(built)), bytes);
    assert.deepEqual(decodeDcMessage(h(bytes)), built);
  }
  assert.throws(() => decodeDcMessage(new Uint8Array(0)));
  assert.throws(() => decodeDcMessage(Uint8Array.of(4, 1)), "unknown kind");
  const huge = new Uint8Array(MAX_PAYLOAD_LEN + 2);
  huge[0] = 3;
  assert.throws(() => decodeDcMessage(huge));
});

test("signals are Rust's, byte for byte; bounds and junk are refused", () => {
  assert.equal(P.signals.length, 5);
  for (const { signal, hex: bytes } of P.signals) {
    const built = signalFromJson(signal);
    assert.equal(hex(encodeSignal(built)), bytes, signal.type);
    assert.deepEqual(decodeSignal(h(bytes)), built, signal.type);
    const raw = h(bytes);
    assert.throws(() => decodeSignal(Uint8Array.of(...raw, 0)), "trailing byte");
    assert.throws(() => decodeSignal(raw.subarray(0, raw.length - 1)), "truncated");
  }
  assert.throws(() => encodeSignal({ type: "offer", id: 1, fp: new Uint8Array(32), sdp: "x".repeat(MAX_SDP + 1) }));
  assert.throws(() => encodeSignal({ type: "candidate", id: 1, candidate: "c".repeat(MAX_CANDIDATE + 1) }));
  assert.throws(() => encodeSignal({ type: "offer", id: 1, fp: new Uint8Array(31), sdp: "" }), "a fingerprint is 32 bytes");
  assert.throws(() => decodeSignal(Uint8Array.of(9, 0, 0, 0, 1)), "unknown kind");
  assert.throws(() => decodeSignal(Uint8Array.of(0x04, 0, 0, 0, 1, 9, 0, 0)), "unknown reason");
  // A length past the bound is refused before it is read.
  const long = new Uint8Array(5 + 2 + MAX_CANDIDATE + 1).fill(0x63);
  long.set([0x03, 0, 0, 0, 1, (MAX_CANDIDATE + 1) >> 8, (MAX_CANDIDATE + 1) & 0xff]);
  assert.throws(() => decodeSignal(long));
});

const sdpWith = (fpLine: string) => `v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n${fpLine}\r\na=setup:actpass\r\n`;

test("fingerprints parse like Rust's, and a tampered one is refused", () => {
  const f = P.fingerprint;
  assert.equal(hex(sdpFingerprint(f.sdp)), f.fp);
  assert.equal(fingerprintHex(h(f.fp)), f.text);
  checkFingerprint(f.sdp, h(f.fp));

  const fp = Uint8Array.from({ length: 32 }, (_, i) => (i * 7) & 0xff);
  const line = `a=fingerprint:sha-256 ${fingerprintHex(fp)}`;
  const sdp = sdpWith(line);
  assert.deepEqual(sdpFingerprint(sdp), fp);
  checkFingerprint(sdp, fp);
  // Lower-case hex and the hash name in capitals are the same thing.
  checkFingerprint(sdpWith(`a=fingerprint:SHA-256 ${fingerprintHex(fp).toLowerCase()}`), fp);

  // Someone swapped the certificate in the SDP: the stated one no longer matches…
  const other = Uint8Array.from(fp);
  other[31]! ^= 1;
  const tampered = sdpWith(`a=fingerprint:sha-256 ${fingerprintHex(other)}`);
  assert.throws(() => checkFingerprint(tampered, fp), (e: unknown) => e instanceof PathError && e.kind === "fingerprint");
  // …or stated a different one next to an honest SDP.
  assert.throws(() => checkFingerprint(sdp, other), (e: unknown) => e instanceof PathError && e.kind === "fingerprint");
  // Two different fingerprints, a weaker hash, none, or junk: refused.
  assert.throws(() => sdpFingerprint(sdpWith(`${line}\r\na=fingerprint:sha-256 ${fingerprintHex(other)}`)));
  assert.deepEqual(sdpFingerprint(sdpWith(`${line}\r\n${line}`)), fp, "a session- and a media-level copy");
  assert.throws(() => sdpFingerprint(sdpWith("a=fingerprint:sha-1 00:11")));
  assert.throws(() => sdpFingerprint(sdpWith("a=setup:active")));
  assert.throws(() => sdpFingerprint(sdpWith("a=fingerprint:sha-256 00:11:zz")));
  assert.equal(parseFingerprintHex(`${fingerprintHex(fp)}:00`), null, "33 bytes");
  // Candidates trickle as signals: the offer goes without them.
  const withCandidates = `${sdp}a=candidate:1 1 udp 2122260223 192.0.2.1 50000 typ host\r\na=end-of-candidates\r\n`;
  assert.equal(stripCandidates(withCandidates), sdp);
});

function bindingFromJson(b: Json): DirectBinding {
  return {
    binding: { account: b.account, device: b.device, machine: b.machine },
    relayHash: h(b.relayHash),
    nonce: h(b.nonce),
    deviceFp: h(b.deviceFp),
    agentFp: h(b.agentFp),
  };
}

test("the direct prologue binds every part, and never equals the hub channel's", () => {
  const bind = bindingFromJson(P.direct.binding);
  const p = directPrologue(bind);
  assert.equal(hex(p), P.direct.prologue);
  assert.notEqual(hex(p), hex(prologue(bind.binding)));
  const nine = new Uint8Array(32).fill(9);
  for (const v of [
    { ...bind, relayHash: nine },
    { ...bind, nonce: nine },
    { ...bind, deviceFp: nine },
    { ...bind, agentFp: nine },
    { ...bind, deviceFp: bind.agentFp, agentFp: bind.deviceFp },
  ]) {
    assert.notEqual(hex(directPrologue(v)), hex(p));
  }
  assert.throws(() => directPrologue({ ...bind, binding: { ...bind.binding, device: "" } }));
});

test("the direct handshake writes Rust's first message, reads Rust's reply, and seals the same envelopes", async () => {
  const d = P.direct;
  const p = h(d.prologue);
  const device = await keyPairFromSecret(h(d.initiator.static));
  const agent = await keyPairFromSecret(h(d.responder.static));
  assert.equal(hex(agent.publicKey), d.responder.staticPublic);

  const initiator = await Initiator.withPrologue(device, agent.publicKey, p, await keyPairFromSecret(h(d.initiator.ephemeral)));
  const m1 = await initiator.writeHello(d.hello.timestampMs, h(d.hello.payload));
  assert.equal(hex(m1), d.hello.message, "first message");
  const responder = await Responder.withPrologue(agent, p, await keyPairFromSecret(h(d.responder.ephemeral)));
  const hello = await responder.readHello(h(d.hello.message));
  assert.equal(hex(hello.payload), d.hello.payload);
  const reply = await responder.writeReply(h(d.reply.payload), DEFAULT_REKEY, d.hello.timestampMs);
  assert.equal(hex(reply.message), d.reply.message, "reply");
  const { transport: it, payload } = await initiator.readReply(h(d.reply.message), DEFAULT_REKEY, d.hello.timestampMs);
  assert.equal(hex(payload), d.reply.payload);
  assert.equal(hex(it.handshakeHash), d.handshakeHash);

  const rt = reply.transport;
  for (const msg of d.transport) {
    const env = envelopeFromJson(msg.envelope);
    assert.equal(hex(encodeEnvelope(env)), msg.plain);
    const [tx, rx] = msg.from === "initiator" ? [it, rt] : [rt, it];
    const sealed = encodeDcMessage({ type: "sealed", body: await tx.seal(msg.nowMs, encodeEnvelope(env)) });
    assert.equal(hex(sealed), msg.message, `${msg.from} ${msg.envelope.type}`);
    const back = decodeDcMessage(h(msg.message));
    assert.equal(back.type, "sealed");
    assert.deepEqual(decodeEnvelope(await rx.open(back.body)), env);
  }

  // The hub prologue does not open a direct hello.
  const hub = await Responder.create(agent, bindingFromJson(d.binding).binding);
  await assert.rejects(hub.readHello(h(d.hello.message)));
});

test("a device's direct hello is a chain hello the machine reads only under the same binding", async () => {
  const c = V.chain;
  const chain = await ChainState.verify(
    c.entries.map((e: string) => decodeEntry(h(e))),
    undefined,
    ChainClock.history(),
  );
  const dev = c.devices.find((x: Json) => x.id === "dev-0");
  const device = await keyPairFromSecret(h(dev.dhSecret));
  const machine = await keyPairFromSecret(h(c.machine.dhSecret));
  const bind: DirectBinding = {
    binding: { account: chain.account, device: "dev-0", machine: c.machine.id },
    relayHash: new Uint8Array(32).fill(1),
    nonce: new Uint8Array(32).fill(2),
    deviceFp: new Uint8Array(32).fill(3),
    agentFp: new Uint8Array(32).fill(4),
  };
  const { message } = await startDirectHandshake(chain, device, bind, 2_000, utf8(`web/1 ${CLIENT_FEATURE}`));
  const r = await Responder.withPrologue(machine, directPrologue(bind));
  const hello = await r.readHello(message);
  assert.equal(hex(hello.initiatorStatic), hex(device.publicKey));
  const body = decodeHelloBody(hello.payload);
  assert.equal(body.chainSeq, chain.seq);
  assert.ok(clientWantsPaths(body.client));
  assert.equal(fromUtf8(body.client), "web/1 paths/1");

  // The agent saw another certificate than the device did: the hello does not open.
  const theirs = await Responder.withPrologue(machine, directPrologue({ ...bind, agentFp: new Uint8Array(32).fill(0xee) }));
  await assert.rejects(theirs.readHello(message));
  // A binding for another account is not started at all.
  await assert.rejects(
    startDirectHandshake(chain, device, { ...bind, binding: { ...bind.binding, account: "acct-other" } }, 2_000, utf8("")),
    (e: unknown) => e instanceof HandshakeError && e.kind === "unauthorized",
  );
  await assert.rejects(
    startDirectHandshake(chain, device, { ...bind, binding: { ...bind.binding, machine: "m-nope" } }, 2_000, utf8("")),
    (e: unknown) => e instanceof HandshakeError && e.kind === "unknownMachine",
  );
});

test("the feature token is matched whole; an agent's features come from its server info", () => {
  assert.ok(clientWantsPaths(utf8("web/1 paths/1")));
  assert.ok(clientWantsPaths(utf8("paths/1")));
  assert.ok(!clientWantsPaths(utf8("web/1")));
  assert.ok(!clientWantsPaths(utf8("web/1 paths/10")));
  assert.ok(!clientWantsPaths(Uint8Array.of(0xff, 0xfe)));
  assert.deepEqual(serverFeatures(utf8('{"agent":"remote-hand","version":"0.3.0","features":["rpc","paths/1","direct/1"]}')), ["rpc", "paths/1", "direct/1"]);
  assert.deepEqual(serverFeatures(utf8("fake agent")), [], "not JSON: an older agent");
  assert.deepEqual(serverFeatures(utf8('{"features":"paths/1"}')), []);
  assert.deepEqual(serverFeatures(utf8("null")), []);
});
