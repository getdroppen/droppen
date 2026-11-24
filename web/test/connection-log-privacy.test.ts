// What a connection log export must never hold (`lib/connection-log.ts`,
// PRIVACY): an export is pasted into chats and issues, and a leak there
// cannot be taken back.
//
// What these pin: hostile values — a token in a URL, a short token, an email,
// paths (with spaces, Windows, one segment, percent-encoded), a file name, IP
// addresses v4 and v6, an ICE candidate and an SDP line, `.local` and
// `.ts.net` host names, a device's name, a UUID — fed through every place the
// log takes free text from the far end or from an error (the hub's close
// reason and a socket that cannot be made, a relay's close reason, error and
// goaway, the relay list, a ticket and a failure, the hub's HTTP API, a
// request whose body fails and a socket over a tunnel with ids in its route)
// come out of the export as none of themselves: free text that is not
// Droppen's own words is withheld (its length kept), and every other field is
// scrubbed; Droppen's own words survive; a query key without `=`, or one the
// API does not name, is not kept; the ids the log knows are masked wherever
// they turn up and in any case (`hide`); and a device's mask is salted per
// browser and blind to case.
import assert from "node:assert/strict";
import test from "node:test";
import { CODE_FIELDS, CONN_EVENTS, type ConnEventName, ConnectionLog, connLog, FREE_TEXT, maskId, routeTemplate, setMaskSalt } from "../src/lib/connection-log.js";
import { logText } from "../src/features/cloud/connection-log-format.js";
import { decodeFrame, encodeFrame, type Frame, generateDeviceKeys } from "../src/lib/e2e/index.js";
import { HubApi } from "../src/lib/hub/api.js";
import { HubLink } from "../src/lib/hub/link.js";
import { RelayLink } from "../src/lib/hub/relay-link.js";
import { RelayPool } from "../src/lib/hub/relays.js";
import { type Pipe, type PipeMessage, tunnelTransport } from "../src/lib/transport/tunnel.js";

/**
 * The credential-shaped values, made up as the test runs: no secrets scanner
 * takes a literal here for a real one (`.gitleaks.toml`, which prefers that
 * to an allowance).
 */
const KEY = ["s3cr3t", "key"].join("-");
const JWT = [`ey${"Q".repeat(18)}`, `ey${"R".repeat(18)}`, "S".repeat(12)].join(".");
const SHORT_TOKEN = ["tok", "3", "nAb9"].join("");
const TICKET = ["ticket", "never", "logged"].join("-");

/** Every kind of thing an export must not hold, each with the fragments that would give it away. */
const HOSTILE = {
  url: [`https://ada.example.com/private?key=${KEY}`, "ada.example.com", KEY],
  token: [JWT, JWT.slice(0, 12)],
  shortToken: [SHORT_TOKEN, SHORT_TOKEN],
  email: ["ada.lovelace@example.com", "lovelace"],
  spacedPath: ["/Users/ada/My Secret Project/notes final.txt", "My Secret Project", "notes final", "/Users/ada"],
  windowsPath: ["C:\\Users\\ada\\secret.docx", "secret.docx", "C:\\Users"],
  oneSegment: ["/secretproject", "secretproject"],
  encoded: ["%2FUsers%2Fada%2Fplan.key", "%2FUsers", "plan.key"],
  file: ["budget 2026.xlsx", "budget 2026"],
  ipv4: ["192.168.1.23", "192.168.1.23"],
  ipv6: ["fe80::1c2b:3a4d:5e6f:7a8b", "fe80::1c2b"],
  candidate: ["candidate:842163049 1 udp 1677729535 203.0.113.7 52123 typ srflx raddr 10.0.0.5 rport 52123", "203.0.113.7", "10.0.0.5"],
  sdp: ["c=IN IP4 198.51.100.9", "198.51.100.9"],
  local: ["adas-macbook.local", "adas-macbook"],
  tailnet: ["studio.tail1234.ts.net", "tail1234"],
  deviceName: ["Ada's MacBook Pro", "Ada's MacBook", "MacBook Pro"],
  uuid: ["0f9a3c2e-5d1b-4a8e-9c77-12ab34cd56ef", "0f9a3c2e-5d1b"],
} as const;
const VALUES = Object.values(HOSTILE).map(([value]) => value as string);
const FRAGMENTS = Object.values(HOSTILE).flatMap(([, ...fragments]) => fragments as readonly string[]);
/** All of it in one far-end reason (a close reason is short: a few at a time). */
const ALL = VALUES.join(" | ");
const SHORT = [HOSTILE.deviceName[0], HOSTILE.spacedPath[0], HOSTILE.ipv4[0], HOSTILE.email[0], HOSTILE.shortToken[0]].join(" ");

const header = (log: ConnectionLog) => ({
  build: "index-test",
  agentVersion: null,
  userAgent: "test",
  loadedAt: log.loadedAt,
  exportedAt: Date.now(),
  network: {},
  connection: null,
  filter: "all" as const,
  total: log.size,
});

/** The export of everything logged after `seq`, and an assertion that none of the hostile values is in it. */
function assertClean(log: ConnectionLog, since: number, where: string): string {
  const events = log.events().filter((e) => e.seq > since);
  assert.ok(events.length > 0, `${where}: something was logged`);
  const text = logText(events, header(log));
  for (const fragment of FRAGMENTS) assert.ok(!text.includes(fragment), `${where}: the export holds ${JSON.stringify(fragment)}:\n${text}`);
  return text;
}

const lastSeq = (log: ConnectionLog) => log.events().at(-1)?.seq ?? 0;

async function until(what: () => boolean, label: string, ms = 5_000): Promise<void> {
  const deadline = Date.now() + ms;
  while (!what()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await new Promise((resolve) => setTimeout(resolve, 2));
  }
}

test("free text in any event is withheld unless it is Droppen's own words; every other field is scrubbed of all of it", () => {
  const log = new ConnectionLog();
  for (const name of Object.keys(CONN_EVENTS) as ConnEventName[]) {
    const fields: Record<string, string> = {};
    for (const key of FREE_TEXT) fields[key] = ALL;
    for (const key of CODE_FIELDS) fields[key] = ALL;
    // A field the log writes itself, given what it never writes: scrubbed (a device's name is not a thing safeText can know, so it is tried in free text and codes only).
    fields.on = VALUES.filter((v) => v !== HOSTILE.deviceName[0]).join(" | ");
    fields.via = HOSTILE.url[0];
    fields.candidates = HOSTILE.candidate[0];
    fields.route = HOSTILE.spacedPath[0];
    log.emit(name, fields);
  }
  const text = assertClean(log, 0, "every event");
  const first = log.events()[0]!;
  for (const key of FREE_TEXT) {
    assert.equal(first.fields[key], "<withheld>");
    assert.equal(first.fields[`${key}Len`], ALL.length, "its length, never its words");
  }
  for (const key of CODE_FIELDS) assert.equal(first.fields[key], "<withheld>", `${key} is a code, or nothing`);
  assert.equal(first.fields.route, "<path>");
  assert.ok(text.length > 0);

  // Droppen's own words are kept, a cause after them is not.
  log.clear();
  log.emit("channel.close", { reason: "the relay path stopped answering", why: "the relay closed the connection (1006)", error: "tunnel: the pipe closed before the answer ended (4000)" });
  log.emit("direct.fail", { why: `the answer was not taken: Failed to set remote answer sdp: ${HOSTILE.candidate[0]}` });
  const [own, prefixed] = log.events();
  assert.deepEqual(
    { reason: own!.fields.reason, why: own!.fields.why, error: own!.fields.error },
    { reason: "the relay path stopped answering", why: "the relay closed the connection (1006)", error: "tunnel: the pipe closed before the answer ended (4000)" },
  );
  assert.equal(prefixed!.fields.why, "the answer was not taken: <withheld>");
  assertClean(log, 0, "own words");
});

test("the second net: IP addresses, candidates, SDP, host and file names, every kind of path and short tokens", () => {
  const log = new ConnectionLog();
  for (const value of VALUES) if (value !== HOSTILE.deviceName[0]) log.emit("sample", { "hub.note": `before ${value} after` });
  assertClean(log, 0, "safeText");
});

test("a query keeps the keys the API names, with numbers only for positions and counts; a bare part or another key is not kept", () => {
  const leak = HOSTILE.shortToken[0];
  assert.equal(routeTemplate(`/api/sessions?${leak}`), "/api/sessions?{key}");
  assert.equal(routeTemplate(`/api/sessions?${leak}=1&limit=5`), "/api/sessions?{key}&limit=5");
  assert.equal(routeTemplate(`/api/fs/download?path=${encodeURIComponent(HOSTILE.spacedPath[0])}&inline`), "/api/fs/download?path&inline", "a key the API names, bare, is still that key");
  assert.equal(routeTemplate(`/api/fs/download?inline&${encodeURIComponent(HOSTILE.spacedPath[0])}`), "/api/fs/download?inline&{key}");
  assert.equal(routeTemplate(`/api/sessions/claude/${HOSTILE.uuid[0]}/history?before=12&fp=${HOSTILE.token[0]}`), "/api/sessions/{source}/{id}/history?before=12&fp");
});

/** A hub socket that signs the page in, and closes when told to, with the hub's words. */
class HubSocket {
  binaryType = "blob";
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code?: number; reason?: string; wasClean?: boolean }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;

  constructor() {
    setTimeout(() => {
      this.readyState = 1;
      this.onopen?.({});
      this.onmessage?.({ data: JSON.stringify({ t: "challenge", nonce: "00".repeat(32) }) });
    }, 0);
  }

  send(data: unknown): void {
    if (typeof data === "string" && (JSON.parse(data) as { t?: string }).t === "login") setTimeout(() => this.onmessage?.({ data: JSON.stringify({ t: "welcome" }) }), 0);
  }

  close(): void {}
}

test("the hub's link: a close reason the hub gives and a socket that cannot be made say nothing of themselves; the ids the log knows are masked in any case", async (t) => {
  const keys = await generateDeviceKeys();
  const deviceId = "Dev-Ada-MacBook-77";
  const sockets: HubSocket[] = [];
  let refuse = false;
  const link = new HubLink({
    url: "wss://hub.test/v1/client",
    origin: "https://hub.test",
    account: "acct-ada-lovelace",
    deviceId,
    token: "session",
    signKey: keys.sign.privateKey,
    makeSocket: () => {
      if (refuse) throw new Error(`cannot open ${ALL}`);
      const s = new HubSocket();
      sockets.push(s);
      return s;
    },
    random: () => 0,
  });
  t.after(() => link.stop());
  const since = lastSeq(connLog);
  link.start();
  await link.waitOpen(5_000);
  refuse = true;
  sockets[0]!.onclose?.({ code: 4001, reason: SHORT, wasClean: true });
  await until(() => connLog.events().some((e) => e.seq > since && e.name === "hub.error"), "the socket that could not be made");
  link.stop();
  const text = assertClean(connLog, since, "hub link");
  const close = connLog.events().find((e) => e.seq > since && e.name === "hub.close" && e.fields.by === "socket")!;
  assert.equal(close.fields.reason, "<withheld>");
  assert.equal(close.fields.reasonLen, SHORT.length);
  assert.equal(close.fields.code, 4001, "the code is kept");
  const error = connLog.events().find((e) => e.seq > since && e.name === "hub.error")!;
  assert.deepEqual({ why: error.fields.why, errName: error.fields.errName }, { why: "<withheld>", errName: "Error" });

  // `hide`: the link named its ids; anywhere they turn up, in any case, they are their masks.
  const mark = lastSeq(connLog);
  connLog.emit("channel.status", { error: `${deviceId.toLowerCase()} is isolated: renew its identity key on the device first`, note: `device ${deviceId.toUpperCase()} of acct-ada-lovelace` });
  const said = connLog.events().find((e) => e.seq > mark)!;
  assert.equal(said.fields.error, `${maskId(deviceId)} is isolated: renew its identity key on the device first`, "masked, and so Droppen's own words again");
  assert.equal(said.fields.note, `device ${maskId(deviceId)} of ${maskId("acct-ada-lovelace")}`);
  const hidden = logText([said], header(connLog));
  for (const id of [deviceId, deviceId.toLowerCase(), deviceId.toUpperCase(), "acct-ada-lovelace"]) assert.ok(!hidden.includes(id), id);
  assert.ok(text.length > 0);
});

test("a device's mask is salted per browser and blind to case", () => {
  setMaskSalt("0".repeat(32));
  const a = maskId("Studio-Mac");
  assert.equal(a, maskId("studio-mac"), "the same id in another case is the same device");
  assert.equal(a, maskId("STUDIO-MAC"));
  setMaskSalt("1".repeat(32));
  assert.notEqual(maskId("studio-mac"), a, "another browser's salt, another mask: a mask is no fingerprint across browsers");
});

/** A relay socket: the sign-in, then whatever the test makes the relay say. */
class RelaySocket {
  binaryType = "blob";
  readyState = 0;
  onopen: ((ev: unknown) => void) | null = null;
  onmessage: ((ev: { data: unknown }) => void) | null = null;
  onclose: ((ev: { code?: number; reason?: string; wasClean?: boolean }) => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;

  constructor() {
    setTimeout(() => {
      this.readyState = 1;
      this.text({ t: "challenge", nonce: "00".repeat(32), relay: "r-1", origin: "wss://r-1.test" });
    }, 0);
  }

  text(m: unknown): void {
    this.onmessage?.({ data: JSON.stringify(m) });
  }

  send(data: unknown): void {
    if (typeof data === "string" && (JSON.parse(data) as { t?: string }).t === "login") setTimeout(() => this.text({ t: "welcome" }), 0);
    else if (typeof data !== "string") {
      const frame = decodeFrame(data as Uint8Array);
      if (frame.type === "ping") {
        const pong = encodeFrame({ type: "pong", nonce: frame.nonce } as Frame);
        setTimeout(() => this.onmessage?.({ data: pong.buffer.slice(pong.byteOffset, pong.byteOffset + pong.byteLength) }), 0);
      }
    }
  }

  close(): void {}
}

test("a relay's link: what the relay says in an error, a goaway and a close says nothing of itself", async () => {
  const keys = await generateDeviceKeys();
  let socket: RelaySocket | null = null;
  const link = new RelayLink({
    relay: "r-1",
    origin: "wss://r-1.test",
    account: "a-1",
    deviceId: "d-1",
    signKey: keys.sign.privateKey,
    ticket: async () => TICKET,
    makeSocket: () => (socket = new RelaySocket()),
  });
  const since = lastSeq(connLog);
  await link.open(5_000);
  socket!.text({ t: "error", code: "x", message: SHORT });
  socket!.text({ t: "goaway", retry_ms: 10, reason: SHORT });
  socket!.onclose?.({ code: 4001, reason: SHORT, wasClean: true });
  assertClean(connLog, since, "relay link");
  const close = connLog.events().find((e) => e.seq > since && e.name === "relay.close")!;
  assert.deepEqual({ reason: close.fields.reason, error: close.fields.error, code: close.fields.code }, { reason: "<withheld>", error: "<withheld>", code: 4001 });
  assert.equal(connLog.events().find((e) => e.seq > since && e.name === "relay.goaway")!.fields.reason, "<withheld>");
  for (const e of connLog.events()) assert.ok(!JSON.stringify(e.fields).includes(TICKET));
});

test("the relay pool and the hub's HTTP API: a list, a ticket, a failure and a request that fail with someone else's words say nothing of them", async () => {
  const since = lastSeq(connLog);
  const pool = new RelayPool({
    api: {
      relays: async () => {
        throw new Error(ALL);
      },
      relayTickets: async () => {
        throw new TypeError(ALL);
      },
    } as never,
    account: { account: "a-1", deviceId: "d-1", session: { token: "t" }, keys: { sign: { privateKey: null as never } } },
    storage: null,
  });
  await assert.rejects(pool.refresh());
  await assert.rejects(pool.ticket("r-1"));
  pool.noteFailure("r-1", ALL);
  const api = new HubApi("https://hub.test", async () => {
    throw new TypeError(ALL);
  });
  await assert.rejects(api.relays("t"));
  assertClean(connLog, since, "pool and API");
  const req = connLog.events().find((e) => e.seq > since && e.name === "hub.req")!;
  assert.deepEqual({ route: req.fields.route, error: req.fields.error, errName: req.fields.errName }, { route: "/v1/relays", error: "<withheld>", errName: "TypeError" });
  // The browser's own words for a request that got no answer are Droppen's to keep.
  const offline = new HubApi("https://hub.test", async () => {
    throw new TypeError("Failed to fetch");
  });
  const mark = lastSeq(connLog);
  await assert.rejects(offline.relays("t"));
  assert.equal(connLog.events().find((e) => e.seq > mark && e.name === "hub.req")!.fields.error, "Failed to fetch");
});

/** A pipe the test plays the far end of. */
class TestPipe implements Pipe {
  onmessage: ((message: PipeMessage) => void) | null = null;
  onclose: ((code: number) => void) | null = null;
  send(): void {}
  close(): void {}
}

test("a request and a socket over a tunnel: ids and paths in the route, and a body that fails with someone else's words, say nothing of them", async () => {
  const pipes: TestPipe[] = [];
  const transport = tunnelTransport(
    {
      open: () => {
        const p = new TestPipe();
        pipes.push(p);
        return p;
      },
    },
    { device: maskId("studio") },
  );
  const since = lastSeq(connLog);
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      controller.error(new Error(ALL));
    },
  });
  const url = `http://machine/api/fs/upload?dir=${encodeURIComponent(HOSTILE.spacedPath[0])}&${encodeURIComponent(HOSTILE.shortToken[0])}`;
  await assert.rejects(transport.fetch(new Request(url, { method: "POST", body, duplex: "half" } as RequestInit), { class: "blob" }));
  const socket = transport.openSocket(`/api/sessions/claude/${HOSTILE.uuid[0]}/ws?fp=${HOSTILE.token[0]}&${HOSTILE.ipv4[0]}`);
  pipes.at(-1)!.onmessage?.({ kind: "head", status: 101, headers: [] });
  socket.close();
  await new Promise((resolve) => setTimeout(resolve, 5));
  assertClean(connLog, since, "tunnel");
  const req = connLog.events().find((e) => e.seq > since && e.name === "req")!;
  assert.deepEqual({ route: req.fields.route, error: req.fields.error }, { route: "/api/fs/upload?dir&{key}", error: "<withheld>" });
  const closed = connLog.events().find((e) => e.seq > since && e.name === "socket.close")!;
  assert.equal(closed.fields.route, "/api/sessions/{source}/{id}/ws?fp&{key}");
});
