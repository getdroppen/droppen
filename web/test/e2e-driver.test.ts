// The browser's channel driver (`lib/e2e/driver.ts`), two of them back to
// back through a relay that forwards frames the way the hub does — and
// enforces the hub's credit window the way the hub does, so a driver that
// overran it would fail here.
//
// What these pin: streams carry data both ways, more than any window, in
// order; hub credit is handed back and never overrun; a stream either end
// closes ends for both readers; a channel the hub closes, or one that fails to
// decrypt, ends every stream on it.
import assert from "node:assert/strict";
import test from "node:test";
import {
  ChannelDriver,
  type ChannelStream,
  DEFAULT_REKEY,
  type Frame,
  generateDeviceKeys,
  INITIAL_CREDIT,
  Initiator,
  type NoiseKeyPair,
  Responder,
  utf8,
} from "../src/lib/e2e/index.js";

const binding = { account: "a-1", device: "d-1", machine: "m-1" };

async function pair(): Promise<NoiseKeyPair> {
  const keys = await generateDeviceKeys();
  return { privateKey: keys.dh.privateKey, publicKey: keys.dhPublic };
}

/** A device and an agent driver joined by a credit-enforcing relay. */
async function wire() {
  const device = await pair();
  const agent = await pair();
  const i = await Initiator.create(device, agent.publicKey, binding);
  const r = await Responder.create(agent, binding);
  await r.readHello(await i.writeHello(1, utf8("")));
  const { message, transport: rt } = await r.writeReply(utf8(""), DEFAULT_REKEY, 1);
  const { transport: it } = await i.readReply(message, DEFAULT_REKEY, 1);
  const outstanding = { toAgent: 0, toDevice: 0, maxToAgent: 0 };
  let agentDriver: ChannelDriver;
  let deviceDriver: ChannelDriver;
  const relay = (from: "device" | "agent", frame: Frame) => {
    // Delivered asynchronously, as a network would.
    setTimeout(() => {
      const key = from === "device" ? "toAgent" : "toDevice";
      const other = from === "device" ? "toDevice" : "toAgent";
      if (frame.type === "data") {
        outstanding[key] += frame.payload.length;
        if (key === "toAgent") outstanding.maxToAgent = Math.max(outstanding.maxToAgent, outstanding.toAgent);
        assert.ok(outstanding[key] <= INITIAL_CREDIT, "a sender overran the hub's window");
      }
      if (frame.type === "credit") outstanding[other] -= frame.bytes;
      (from === "device" ? agentDriver : deviceDriver).receive(frame);
    }, 0);
  };
  deviceDriver = new ChannelDriver(7, it, "initiator", (f) => relay("device", f));
  agentDriver = new ChannelDriver(7, rt, "responder", (f) => relay("agent", f));
  return { deviceDriver, agentDriver, outstanding };
}

function collect(stream: ChannelStream): { data: number[]; closed: Promise<number> } {
  const data: number[] = [];
  const closed = new Promise<number>((resolve) => {
    stream.onclose = resolve;
  });
  stream.ondata = (chunk) => data.push(...chunk);
  return { data, closed };
}

test("streams carry more than any window both ways, in order, without overrunning the hub's credit", async () => {
  const { deviceDriver, agentDriver, outstanding } = await wire();
  const accepted = new Promise<ChannelStream>((resolve) => {
    agentDriver.onincoming = resolve;
  });
  const stream = deviceDriver.open("rpc", utf8("GET /api/sessions"));
  const size = 3 * 1024 * 1024;
  const big = new Uint8Array(size).map((_, i) => i % 251);
  stream.send(big);
  const theirs = await accepted;
  assert.equal(new TextDecoder().decode(theirs.meta), "GET /api/sessions");
  let got = 0;
  let ok = true;
  await new Promise<void>((resolve) => {
    theirs.ondata = (chunk) => {
      for (const b of chunk) ok &&= b === got++ % 251;
      if (got === size) resolve();
    };
  });
  assert.ok(ok, "every byte in order");
  assert.ok(outstanding.maxToAgent > INITIAL_CREDIT / 2, "the window was used");
  // And back, then a close the device hears.
  const mine = collect(stream);
  theirs.send(utf8("pong"));
  theirs.close(0);
  assert.equal(await mine.closed, 0);
  assert.equal(new TextDecoder().decode(Uint8Array.from(mine.data)), "pong");
});

test("a stream this end closes ends for both readers", async () => {
  const { deviceDriver, agentDriver } = await wire();
  const accepted = new Promise<ChannelStream>((resolve) => {
    agentDriver.onincoming = resolve;
  });
  const stream = deviceDriver.open("term", utf8("/api/terminals/t/ws"));
  stream.send(utf8("ls\r"));
  const theirs = collect(await accepted);
  const mine = collect(stream);
  stream.close(0);
  assert.equal(await theirs.closed, 0);
  assert.equal(await mine.closed, 0, "our own close is answered");
  assert.equal(new TextDecoder().decode(Uint8Array.from(theirs.data)), "ls\r");
});

test("a channel the hub closes, or one that stops decrypting, ends every stream on it", async () => {
  const { deviceDriver, agentDriver } = await wire();
  agentDriver.onincoming = () => undefined;
  const a = collect(deviceDriver.open("mux", utf8("/api/events")));
  const ended = new Promise<number>((resolve) => {
    deviceDriver.onclose = (code) => resolve(code);
  });
  deviceDriver.receive({ type: "close", ch: 7, code: 4410, reason: "the other end disconnected" });
  assert.equal(await ended, 4410);
  assert.equal(await a.closed, 4410);
  assert.ok(deviceDriver.isClosed);
  // A stream opened on a dead channel ends at once.
  assert.equal(await collect(deviceDriver.open("rpc", utf8("x"))).closed, 4502);

  const second = await wire();
  const b = collect(second.deviceDriver.open("rpc", utf8("y")));
  second.deviceDriver.receive({ type: "data", ch: 7, payload: new Uint8Array(40) });
  assert.equal(await b.closed, 4400, "a forged message kills the channel");
});

test("ChannelStream.drain waits for queued send bytes to drop and manualCredit defers receive credit", async () => {
  const { deviceDriver, agentDriver } = await wire();
  const accepted = new Promise<ChannelStream>((resolve) => {
    agentDriver.onincoming = resolve;
  });
  const stream = deviceDriver.open("blob", utf8("POST /api/uploads"));
  const theirs = await accepted;
  theirs.manualCredit = true;
  const received: Uint8Array[] = [];
  theirs.ondata = (chunk) => received.push(chunk);

  // Queue 600 KiB — larger than the 256 KiB stream window so draining must wait for receiver credit.
  const payload = new Uint8Array(600 * 1024).fill(42);
  stream.send(payload);
  let drained = false;
  const waiting = stream.drain(128 * 1024).then(() => {
    drained = true;
  });
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(drained, false, "drain stays pending while receiver has not acknowledged its window");
  const firstBatch = received.reduce((sum, c) => sum + c.length, 0);
  assert.ok(firstBatch > 0 && firstBatch <= 256 * 1024, `sender stopped at the receive window (${firstBatch} bytes)`);

  // Consume chunks incrementally until all 600 KiB arrive and drain resolves.
  let totalGot = 0;
  while (totalGot < payload.length) {
    const chunk = received.shift();
    if (chunk) {
      totalGot += chunk.length;
      theirs.consumed(chunk.length);
    } else {
      await new Promise((resolve) => setTimeout(resolve, 5));
    }
  }
  await waiting;
  assert.equal(drained, true, "drain resolved once queued bytes dropped below budget");
});

test("a throwing DirectLink.bufferedAmount detaches the direct path without closing the channel", async () => {
  const device = await pair();
  const agent = await pair();
  const i = await Initiator.create(device, agent.publicKey, binding);
  const r = await Responder.create(agent, binding);
  await r.readHello(await i.writeHello(1, utf8("")));
  const { message, transport: rt } = await r.writeReply(utf8(""), DEFAULT_REKEY, 1);
  const { transport: it } = await i.readReply(message, DEFAULT_REKEY, 1);

  const di = await Initiator.create(device, agent.publicKey, binding);
  const dr = await Responder.create(agent, binding);
  await dr.readHello(await di.writeHello(2, utf8("")));
  const { message: dmsg } = await dr.writeReply(utf8(""), DEFAULT_REKEY, 2);
  const { transport: directTransport } = await di.readReply(dmsg, DEFAULT_REKEY, 2);

  let agentDriver: ChannelDriver;
  let deviceDriver: ChannelDriver;
  deviceDriver = new ChannelDriver(7, it, "initiator", (f) => setTimeout(() => agentDriver.receive(f), 0), Date.now, {
    paths: { probeRelay: false },
  });
  agentDriver = new ChannelDriver(7, rt, "responder", (f) => setTimeout(() => deviceDriver.receive(f), 0), Date.now, {
    paths: { probeRelay: false },
  });

  let released = false;
  let shouldThrow = false;
  deviceDriver.attachDirect(directTransport, {
    send: () => undefined,
    bufferedAmount: () => {
      if (shouldThrow) throw new Error("DataChannel closed");
      return 0;
    },
    released: () => {
      released = true;
    },
  });
  await new Promise((resolve) => setTimeout(resolve, 10));
  shouldThrow = true;
  const stream = deviceDriver.open("rpc", utf8("GET /api/health"));
  stream.send(utf8("ping"));
  await new Promise((resolve) => setTimeout(resolve, 30));
  assert.equal(released, true, "direct path was detached cleanly");
  assert.equal(deviceDriver.isClosed, false, "channel stayed alive on the carrier path");
});
