// The connection log (`lib/connection-log.ts`, `connection-log-format.ts`):
// what the page's way to its devices did, for the owner to send for analysis.
//
// What these pin: the log keeps at most its bounds — the newest events, the
// chatter (samples and probes) in a ring of its own so it never pushes out a
// close, nothing older than its age — and a page reloaded in the middle of a
// reconnect loop reads back what the page before kept, within its budget;
// nothing it holds names a device, a session, a token or a path: ids are
// short hashes, routes are templates with the ids masked and query values
// dropped but for positions and counts, and every text is scrubbed on the way
// in; the hub link says how its connection closed and who closed it (the hub
// or this page's silence timer), and when it tries again; the paths say a
// probe sent, a round trip that is only an unanswered probe's age
// (`aged`, not an answer), a direct path given up and on which deadline, and
// a pause of the page's timers; the once-a-second sampler runs while a
// connection is up and stops with it; and the text export has its header
// (the release, the browser, the network, the connection) and one compact
// line per event.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { CONN_EVENTS, ConnectionLog, maskId, routeTemplate, safeText } from "../src/lib/connection-log.js";
import { COPY_MAX_CHARS, eventLine, filterEvents, headerLines, logText, type LogHeader, PRIVACY_LINE } from "../src/features/cloud/connection-log-format.js";
import {
  DEFAULT_REKEY,
  DIRECT_TIMEOUT_MS,
  generateDeviceKeys,
  Initiator,
  type NoiseKeyPair,
  RELAY_PROBE_MS,
  Responder,
  Session,
  type Transport,
  utf8,
} from "../src/lib/e2e/index.js";
import { connLog } from "../src/lib/connection-log.js";
import { decodeFrame, encodeFrame, type Frame } from "../src/lib/e2e/index.js";
import { HubLink } from "../src/lib/hub/link.js";

/** A log on clocks the test moves. */
function clocked(opts: { maxEvents?: number; maxChatter?: number; maxAgeMs?: number; sampleMs?: number; quietSampleMs?: number } = {}) {
  const clock = { t: 1_000, wall: Date.UTC(2026, 10, 24, 9, 0, 0) };
  const log = new ConnectionLog({ ...opts, now: () => clock.t, wall: () => clock.wall });
  const step = (ms: number) => {
    clock.t += ms;
    clock.wall += ms;
  };
  return { log, clock, step };
}

test("the log keeps the newest events and samples within their bounds, each ring apart, and nothing older than its age", () => {
  const { log, step } = clocked({ maxEvents: 5, maxRequests: 4, maxChatter: 3, maxAgeMs: 60_000 });
  for (let i = 0; i < 12; i++) {
    log.emit("hub.retry", { attempt: i });
    log.emit("sample", { "hub.tx": i });
    // A page's burst of requests, many more than its events.
    for (let r = 0; r < 5; r++) log.emit("req", { method: "GET", route: "/api/sessions", status: 200, ms: r });
    step(10);
  }
  const events = log.events();
  assert.equal(events.length, 12, "five events, four requests and three samples");
  assert.equal(events.filter((e) => e.name === "req").length, 4, "requests are pushed out by requests, never a close by them");
  assert.deepEqual(
    events.filter((e) => e.name === "hub.retry").map((e) => e.fields.attempt),
    [7, 8, 9, 10, 11],
    "the newest five, oldest first",
  );
  assert.deepEqual(
    events.filter((e) => e.name === "sample").map((e) => e.fields["hub.tx"]),
    [9, 10, 11],
    "samples are pushed out by samples, never a close by them",
  );
  assert.deepEqual(
    events.map((e) => e.seq),
    [...events.map((e) => e.seq)].sort((a, b) => a - b),
    "in the order they were logged",
  );
  // Past the age: gone, whatever the count.
  step(61_000);
  log.emit("hub.open", { attempt: 12 });
  assert.deepEqual(
    log.events().map((e) => e.name),
    ["hub.open"],
  );
  log.clear();
  assert.equal(log.size, 0);
});

test("every event has a monotonic t, a wall clock, a level from the table unless said otherwise, and fields that are plain values", () => {
  const { log, step } = clocked();
  log.emit("hub.connecting", { attempt: 1 });
  step(1234.56);
  log.emit("timeout", { timer: "SILENCE_MS", limitMs: 25_000, elapsedMs: 25_012.345, on: "hub" });
  log.emit("channel.status", { status: "error" }, "warn");
  log.emit("hub.open", { gone: undefined, nan: Number.NaN });
  const [a, b, c, d] = log.events();
  assert.equal(a!.level, CONN_EVENTS["hub.connecting"]);
  assert.equal(b!.level, "error", "a timeout is an error");
  assert.equal(c!.level, "warn", "the emitter's word wins");
  assert.equal(b!.t - a!.t, 1235, "milliseconds since the page loaded, whole");
  assert.equal(b!.fields.elapsedMs, 25012.3, "a fraction to one place");
  assert.deepEqual(d!.fields, { nan: null }, "nothing undefined, no NaN");
  assert.ok(b!.wall > a!.wall);
});

test("a device id is a short stable hash; nothing token-, path- or URL-shaped survives in a text field", () => {
  const id = "0f9a3c2e-5d1b-4a8e-9c77-12ab34cd56ef";
  assert.match(maskId(id)!, /^#[0-9a-f]{6}$/);
  assert.equal(maskId(id), maskId(id), "the same id, the same mask");
  assert.notEqual(maskId(id), maskId(`${id}x`));
  assert.equal(maskId(null), null);

  // Made up as the test runs, so no secrets scanner takes a literal here for a real one (`.gitleaks.toml`).
  const secret = [`ey${"J".repeat(18)}`, `ey${"K".repeat(18)}`, "L".repeat(16)].join(".");
  const hexKey = "9f86".repeat(8);
  const ticket = ["ticket", "abc"].join("=");
  const said = safeText(`refused ${secret} for wss://nrt-1.relay.example/v1/connect?${ticket} at /Users/ada/project/secret.txt, ${id}, ada@example.com, key ${hexKey}`);
  for (const leak of [secret, "nrt-1.relay.example", ticket, "/Users/ada", "secret.txt", id, "ada@example.com", hexKey]) {
    assert.ok(!said.includes(leak), `${leak} is not in ${said}`);
  }
  assert.equal(said, "refused <redacted> for <url> at <path>", "a path runs to the end of the line, whatever follows it");
  assert.ok(safeText("x".repeat(500)).length <= 160, "cut short");

  // Through `emit`: free text is kept only when it is Droppen's own words; another string field is scrubbed.
  const { log } = clocked();
  log.emit("hub.close", { code: 4000, reason: `the token ${secret} went silent`, route: "/Users/ada/project" });
  log.emit("hub.close", { code: 4000, reason: "the hub went silent", by: "page" });
  const [e, own] = log.events();
  assert.equal(e!.fields.reason, "<withheld>", "words that are not Droppen's own are withheld");
  assert.equal(e!.fields.reasonLen, `the token ${secret} went silent`.length, "their length is kept");
  assert.equal(e!.fields.route, "<path>", "a route that is not its own template is text like any other");
  assert.equal(own!.fields.reason, "the hub went silent", "Droppen's own words are kept");
});

test("a request's route is a template: ids masked, query values dropped but for positions and counts", () => {
  const cases: [string, string][] = [
    ["/api/sessions/claude/0f9a3c2e-5d1b-4a8e-9c77-12ab34cd56ef/history?before=1200&fp=ab12cd&from=800", "/api/sessions/{source}/{id}/history?before=1200&fp&from=800"],
    ["/api/sessions/acp/s-1/questions/q-7/answer", "/api/sessions/{source}/{id}/questions/{requestId}/answer"],
    ["/api/fs/download?path=%2FUsers%2Fada%2Fsecret.txt&inline=1", "/api/fs/download?path&inline"],
    ["/api/sessions?q=my%20secret%20project&limit=200", "/api/sessions?q&limit=200"],
    ["/api/plugins/todo-list/docs/notes%2Fplan/history?offset=10", "/api/plugins/{id}/docs/{name}/history?offset=10"],
    ["/api/auth/sessions/sess-123", "/api/auth/sessions/{id}"],
    ["/api/events?v=2&z=1", "/api/events?v=2&z=1"],
    ["/api/terminals/t-9/ws", "/api/terminals/{id}/ws"],
    ["https://hub.example/v1/streams/a1b2c3d4e5f6a7b8/pages/00ff00ff00ff", "/v1/streams/{stream}/pages/{page}"],
    ["/v1/keys/mk/m-1/0/0/dev", "/v1/keys/{name}/{id}/{id}/{id}/{id}"],
    ["/api/sessions/gone-cwds", "/api/sessions/gone-cwds"],
  ];
  for (const [path, template] of cases) assert.equal(routeTemplate(path), template, path);
  for (const [, template] of cases) assert.equal(routeTemplate(template), template, `${template} is its own template`);
});

test("no route of the local program's or the hub's API leaks the ids put into it", () => {
  // An id no API word could be, made up as the test runs.
  const secret = ["s3cr3t", "0f9a".repeat(3)].join("-");
  for (const file of ["../../openapi/openapi.json", "../../openapi/hub.json"]) {
    const spec = JSON.parse(readFileSync(new URL(file, import.meta.url), "utf8")) as { paths: Record<string, unknown> };
    for (const path of Object.keys(spec.paths)) {
      const filled = path.replace(/\{[^}]+\}/g, secret);
      const template = routeTemplate(`${filled}?path=${secret}&before=12`);
      assert.ok(!template.includes(secret), `${path} → ${template}`);
      assert.ok(template.endsWith("?path&before=12"));
      // With the values the API's own words never are, the template is the API's.
      assert.equal(template.split("?")[0], path.replace(/\{[^}]+\}/g, (name) => (name === "{source}" ? "{source}" : name)), path);
    }
  }
});

test("the page before a reload: kept within its budget, read back once and marked, and its age still counts", () => {
  const store = new Map<string, string>();
  const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => void store.set(k, v), removeItem: (k: string) => void store.delete(k) };
  const before = clocked();
  for (let i = 0; i < 200; i++) before.log.emit("hub.retry", { attempt: i, waitMs: 500 });
  before.log.persist(storage, 4_000);
  const raw = store.get("rh.cloud.connection-log")!;
  assert.ok(raw.length <= 4_100, `within the budget (${raw.length})`);

  const after = clocked();
  after.clock.wall = before.clock.wall + 2_000;
  const read = after.log.restore(storage);
  assert.ok(read > 10 && read < 200, `the newest of them (${read})`);
  assert.equal(store.size, 0, "read once");
  const events = after.log.events();
  assert.equal(events.at(-1)!.fields.attempt, 199, "up to the last");
  assert.ok(events.every((e) => e.prev === before.log.loadedAt), "marked as the page before's");
  after.log.emit("page.start", {});
  assert.equal(after.log.events().at(-1)!.name, "page.start", "this page's come after");
  const seqs = after.log.events().map((e) => e.seq);
  assert.equal(new Set(seqs).size, seqs.length, "every event, restored or not, has a seq of its own (a view keys its rows by it)");
  assert.ok(seqs.every((s) => s > 0));
  assert.equal(after.log.restore(storage), 0);

  // A page restored long after: nothing older than the log's age.
  before.log.persist(storage);
  const late = clocked();
  late.clock.wall = before.clock.wall + 31 * 60_000;
  assert.equal(late.log.restore(storage), 0);
  // Garbage is ignored.
  store.set("rh.cloud.connection-log", '{"v":1,"events":[[1,2,"no.such.event","info",{},3],"x"]}');
  assert.equal(late.log.restore(storage), 0);
});

test("the sampler runs while a source is added and stops with the last; a quiet second is not recorded, a busy one is", async () => {
  const log = new ConnectionLog({ sampleMs: 15, quietSampleMs: 60_000 });
  assert.equal(log.sampling, false);
  let tx = 0;
  const offHub = log.addSampler("hub", () => ({ fields: { tx, buf: 0 }, busy: tx > 0 }));
  assert.equal(log.sampling, true, "running once a connection is up");
  await new Promise((resolve) => setTimeout(resolve, 60));
  const first = log.events().filter((e) => e.name === "sample");
  assert.equal(first.length, 1, "one quiet sample, then nothing while nothing moves");
  assert.deepEqual(Object.keys(first[0]!.fields), ["lagMs", "hub.tx", "hub.buf"], "fields under the source's name, with the timers' lag");
  tx = 512;
  const offRelay = log.addSampler("relay:nrt-1", () => ({ fields: { rx: 1 }, busy: true }));
  await new Promise((resolve) => setTimeout(resolve, 50));
  const busy = log.events().filter((e) => e.name === "sample").slice(1);
  assert.ok(busy.length >= 2, "every busy second");
  assert.equal(busy[0]!.fields["hub.tx"], 512);
  assert.equal(busy[0]!.fields["relay:nrt-1.rx"], 1);
  offHub();
  assert.equal(log.sampling, true, "still a connection up");
  offRelay();
  assert.equal(log.sampling, false, "and stopped with the last");
  const count = log.size;
  await new Promise((resolve) => setTimeout(resolve, 40));
  assert.equal(log.size, count, "nothing after it stopped");
});

// -- what the connections say ----------------------------------------------

/** A hub socket that signs the page in and answers pings — until it is told to close, or to go silent. */
class HubSocket {
  binaryType = "blob";
  readyState = 0;
  bufferedAmount = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code?: number; reason?: string; wasClean?: boolean }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  answering = true;

  constructor() {
    setTimeout(() => {
      this.readyState = 1;
      this.onopen?.({});
      this.text({ t: "challenge", nonce: "00".repeat(32) });
    }, 0);
  }

  private text(m: unknown): void {
    if (this.answering) this.onmessage?.({ data: JSON.stringify(m) });
  }

  send(data: string | ArrayBufferLike | ArrayBufferView): void {
    if (typeof data === "string") {
      if ((JSON.parse(data) as { t?: string }).t === "login") setTimeout(() => this.text({ t: "welcome" }), 0);
      return;
    }
    const frame = decodeFrame(data as Uint8Array);
    if (frame.type !== "ping") return;
    const pong = encodeFrame({ type: "pong", nonce: frame.nonce } as Frame);
    setTimeout(() => {
      if (this.answering) this.onmessage?.({ data: pong.buffer.slice(pong.byteOffset, pong.byteOffset + pong.byteLength) });
    }, 0);
  }

  close(): void {}

  /** The network drops it: an abnormal close. */
  drop(): void {
    this.onclose?.({ code: 1006, reason: "", wasClean: false });
  }
}

async function until(what: () => boolean, label: string, ms = 5_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!what()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await new Promise((resolve) => setTimeout(resolve, 2));
  }
}

async function hubLink(t: { after(fn: () => void): void }, opts: { heartbeatMs?: number; silenceMs?: number } = {}) {
  const sockets: HubSocket[] = [];
  const keys = await generateDeviceKeys();
  const link = new HubLink({
    url: "wss://hub.test/v1/client",
    origin: "https://hub.test",
    account: "a-1",
    deviceId: "d-1",
    token: ["session", "token", "that", "must", "never", "be", "logged"].join("-"),
    signKey: keys.sign.privateKey,
    makeSocket: () => {
      const s = new HubSocket();
      sockets.push(s);
      return s;
    },
    random: () => 0.5,
    ...opts,
  });
  t.after(() => link.stop());
  connLog.clear();
  link.start();
  await link.waitOpen(5_000);
  return { link, sockets };
}

const since = (seq: number) => connLog.events().filter((e) => e.seq > seq);
const lastSeq = () => connLog.events().at(-1)?.seq ?? 0;

test("a hub connection the network drops is logged with its close, by whom, and the try after it with its backoff; the sampler goes with it", async (t) => {
  const { link, sockets } = await hubLink(t);
  const opened = connLog.events().find((e) => e.name === "hub.open");
  assert.ok(opened, "the sign-in is logged");
  assert.equal(typeof opened.fields.ms, "number");
  assert.equal(connLog.sampling, true, "sampled while it is up");
  const mark = lastSeq();
  sockets[0]!.drop();
  const close = since(mark).find((e) => e.name === "hub.close");
  assert.deepEqual(
    { code: close?.fields.code, by: close?.fields.by, clean: close?.fields.clean, level: close?.level },
    { code: 1006, by: "socket", clean: false, level: "warn" },
  );
  assert.equal(typeof close!.fields.openMs, "number");
  const retry = since(mark).find((e) => e.name === "hub.retry");
  assert.ok(retry, "the next try, and when");
  assert.equal(retry.fields.waitMs, 250, "half of the first backoff (random 0.5)");
  assert.equal(retry.fields.backoffMs, 500);
  await until(() => link.state === "open", "the reconnect");
  assert.ok(since(mark).some((e) => e.name === "hub.connecting" && e.fields.attempt === 2));
  for (const e of connLog.events()) assert.ok(!JSON.stringify(e.fields).includes("session-token"), "no session in the log");
  link.stop();
  assert.equal(connLog.sampling, false, "the sampler stops with the connection");
});

test("a hub connection that goes silent is logged as the silence timer firing on it, and closed by this page", async (t) => {
  const { link, sockets } = await hubLink(t, { heartbeatMs: 30, silenceMs: 80 });
  await until(() => connLog.events().some((e) => e.name === "probe.answer" && e.fields.on === "hub"), "a ping answered");
  const answer = connLog.events().find((e) => e.name === "probe.answer")!;
  assert.equal(typeof answer.fields.rttMs, "number", "the hub's round trip");
  const mark = lastSeq();
  sockets[0]!.answering = false;
  await until(() => since(mark).some((e) => e.name === "timeout"), "the silence timer");
  const timeout = since(mark).find((e) => e.name === "timeout")!;
  assert.deepEqual({ timer: timeout.fields.timer, limitMs: timeout.fields.limitMs, on: timeout.fields.on, level: timeout.level }, { timer: "SILENCE_MS", limitMs: 80, on: "hub", level: "error" });
  assert.ok((timeout.fields.elapsedMs as number) >= 80);
  const close = since(mark).find((e) => e.name === "hub.close")!;
  assert.deepEqual({ code: close.fields.code, by: close.fields.by, reason: close.fields.reason }, { code: 4000, by: "page", reason: "the hub went silent" });
  await until(() => link.state === "open", "the reconnect");
});

async function noiseKeys(): Promise<NoiseKeyPair> {
  const k = await generateDeviceKeys();
  return { privateKey: k.dh.privateKey, publicKey: k.dhPublic };
}

async function transports(): Promise<[Transport, Transport]> {
  const binding = { account: "a-1", device: "d-1", machine: "m-1" };
  const device = await noiseKeys();
  const agent = await noiseKeys();
  const i = await Initiator.create(device, agent.publicKey, binding);
  const r = await Responder.create(agent, binding);
  await r.readHello(await i.writeHello(1, utf8("")));
  const { message, transport: rt } = await r.writeReply(utf8(""), DEFAULT_REKEY, 1);
  const { transport: it } = await i.readReply(message, DEFAULT_REKEY, 1);
  return [it, rt];
}

test("the paths say a probe sent, a round trip that is only an unanswered probe's age, a pause of the page's timers, and a direct path down on its deadline", async () => {
  const [relay] = await transports();
  const [direct] = await transports();
  const s = Session.withPaths(1, relay, "initiator", { probeRelay: true }, 0);
  s.tick(0);
  assert.ok(await s.poll({ relay: true, direct: false }, 0, 0), "the first hub probe goes out");
  assert.deepEqual(
    s.takeEvents().filter((e) => e.type === "probe"),
    [{ type: "probe", via: "relay", nonce: 1 }],
  );
  // No answer by the next probe: the round trip shown is that probe's age — and said so.
  s.tick(RELAY_PROBE_MS + 1);
  const aged = s.takeEvents().find((e) => e.type === "aged");
  assert.deepEqual(aged, { type: "aged", via: "relay", ageMs: RELAY_PROBE_MS + 1 });
  assert.equal(s.stats().relayRttMs, RELAY_PROBE_MS + 1, "the number the bar shows is that age");
  assert.equal(s.diagnostics(RELAY_PROBE_MS + 1)!.probesOut, 1, "one probe unanswered");

  // The page's timers were held: the probes before the pause are forgotten, and that is said.
  s.tick(RELAY_PROBE_MS + 1 + 4 * RELAY_PROBE_MS);
  const paused = s.takeEvents().find((e) => e.type === "paused");
  assert.deepEqual(paused, { type: "paused", gapMs: 4 * RELAY_PROBE_MS, forgotten: 1 });

  // A direct path that goes silent is let go on its deadline, which the event names.
  const at = 100_000;
  s.attachDirect(direct, at);
  s.takeEvents();
  s.tick(at + DIRECT_TIMEOUT_MS + 1);
  const down = s.takeEvents().find((e) => e.type === "directDown");
  assert.deepEqual(down, { type: "directDown", reason: "timeout", remote: false, timer: "DIRECT_TIMEOUT_MS", elapsedMs: DIRECT_TIMEOUT_MS + 1, unackedBytes: 0 });
});

// -- the export ---------------------------------------------------------------

function header(over: Partial<LogHeader> = {}): LogHeader {
  return {
    build: "index-B3x9aQ",
    agentVersion: "0.2.140",
    userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)",
    loadedAt: Date.UTC(2026, 10, 24, 9, 0, 0),
    exportedAt: Date.UTC(2026, 10, 24, 9, 30, 0),
    network: { online: true, type: "3g", downlinkMbps: 1.2, rttMs: 300 },
    connection: { hub: "open", device: "#3fa9c1", channel: "open", via: "relay:nrt-1", relayRttMs: 5001 },
    filter: "all",
    total: 3,
    ...over,
  };
}

test("the text export: a header with the web build and the device's agent, the browser, the network, the connection and what it leaves out, then one compact line per event", () => {
  const { log, step } = clocked();
  log.emit("channel.open", { device: "#3fa9c1", on: "ch:nrt-1/3", via: "relay:nrt-1", handshakeMs: 412.3 });
  step(5001);
  log.emit("probe.aged", { on: "ch:nrt-1/3", path: "relay", ageMs: 5001 });
  step(5000);
  log.emit("channel.close", { on: "ch:nrt-1/3", code: 4000, reason: "the relay path stopped answering", by: "local", rule: "relay_silent" });
  const text = logText(log.events(), header());
  const lines = text.trimEnd().split("\n");
  assert.equal(lines[0], "Droppen connection log · web build index-B3x9aQ · device agent 0.2.140 · exported 2026-11-24T09:30:00.000Z");
  assert.match(lines[2]!, /^browser: Mozilla\/5\.0 \(iPhone/);
  assert.equal(lines[3], "network: online=true type=3g downlinkMbps=1.2 rttMs=300");
  assert.equal(lines[4], "connection: hub=open device=#3fa9c1 channel=open via=relay:nrt-1 relayRttMs=5001");
  assert.equal(lines[5], "events: 3 of 3 (all)");
  assert.equal(lines[6], PRIVACY_LINE);
  assert.match(lines[7]!, /^--- this page \(loaded /);
  assert.equal(lines[8], "09:00:00.000 +1.000 I channel.open device=#3fa9c1 on=ch:nrt-1/3 via=relay:nrt-1 handshakeMs=412.3");
  assert.equal(lines[9], "09:00:05.001 +6.001 W probe.aged on=ch:nrt-1/3 path=relay ageMs=5001");
  assert.equal(lines[10], '09:00:10.001 +11.001 W channel.close on=ch:nrt-1/3 code=4000 reason="the relay path stopped answering" by=local rule=relay_silent');
  assert.equal(lines.length, 11);
  assert.equal(eventLine(log.events()[1]!), lines[9]);
  assert.equal(headerLines(header({ connection: null }), 0)[4], "connection: -", "no connection to say (local mode)");
});

test("the views: all, all but samples and probes, and the problems alone", () => {
  const { log } = clocked();
  log.emit("hub.open", {});
  log.emit("sample", { "hub.tx": 1 });
  log.emit("probe.sent", { on: "hub", nonce: 1 });
  log.emit("probe.answer", { on: "hub", nonce: 1, rttMs: 40 });
  log.emit("probe.aged", { on: "ch:hub/1", ageMs: 5001 });
  log.emit("timeout", { timer: "SILENCE_MS" });
  const names = (filter: "all" | "events" | "problems") => filterEvents(log.events(), filter).map((e) => e.name);
  assert.deepEqual(names("all"), ["hub.open", "sample", "probe.sent", "probe.answer", "probe.aged", "timeout"]);
  assert.deepEqual(names("events"), ["hub.open", "probe.aged", "timeout"], "an aged probe is no chatter");
  assert.deepEqual(names("problems"), ["probe.aged", "timeout"]);
});

test("a copy holds the newest events under its cap, and says that Download has the rest; Download holds everything", () => {
  const { log, step } = clocked({ maxEvents: 5000 });
  for (let i = 0; i < 3000; i++) {
    log.emit("hub.retry", { attempt: i, waitMs: 500, backoffMs: 1000 });
    step(100);
  }
  const all = log.events();
  const copy = logText(all, header({ total: all.length }), COPY_MAX_CHARS);
  assert.ok(copy.length <= COPY_MAX_CHARS, `under the cap (${copy.length})`);
  assert.ok(COPY_MAX_CHARS < 65_536, "and under what a GitHub issue takes");
  assert.match(copy, /--- \d+ earlier events are not in this copy \(a copy holds at most 60,000 characters\): Download has the whole log ---/);
  assert.match(copy.trimEnd().split("\n").at(-1)!, /attempt=2999 /, "the newest are the ones kept");
  const whole = logText(all, header({ total: all.length }));
  assert.equal(whole.trimEnd().split("\n").filter((l) => / I hub\.retry /.test(l)).length, 3000, "Download: every event");
  assert.doesNotMatch(whole, /not in this copy/);
});
