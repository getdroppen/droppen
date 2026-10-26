// The local app's gateway (S1-7, `docs/local-aggregator.md` §6.2–§6.4): the
// socket to the local install's `/api/agg/ws`, the copy source a device's
// mirror follows through it, and the shell that puts the account's other
// devices on the local page beside `local`.
//
// What these pin: the page folds the install's snapshot and changes with
// droppen.app's own folds; a reconnect, or a subscription the install
// dropped for a slow page, subscribes again from where the page stands; the
// install sending the page away ends it for good. Nothing of another device
// is ever written to the browser's storage, and nothing needs WebCrypto (a
// phone at the computer's LAN address is no secure context). The shell keeps
// `local` the home and as it was; other devices come and go with the
// install's directory, read-only.
import assert from "node:assert/strict";
import test from "node:test";
import { GatewayCopySource } from "../src/lib/gateway/copy-source.js";
import { GatewayLink, parseDir } from "../src/lib/gateway/link.js";
import { MachineMirror } from "../src/lib/hub/mirror.js";
import { LOCAL } from "../src/lib/machines/key.js";
import * as registry from "../src/lib/machines/registry.js";
import { GatewayShell, startGatewayShell } from "../src/features/gateway/gateway-shell.js";
import { gatewayShell, stopGatewayShell } from "../src/features/machines/shells.js";
import { showsOtherDevices } from "../src/features/gateway/gateway-boot.js";
import { canStartOn, deviceNote } from "../src/features/machines/device-picker.js";
import type { SocketLike } from "../src/lib/transport/types.js";
import { flush, until } from "./stream-fixtures.js";

/** A socket the test plays the install's side of. */
class FakeSocket implements SocketLike {
  readyState = 0;
  binaryType = "blob";
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  sent: Record<string, unknown>[] = [];
  constructor(readonly url: string) {}
  send(data: string): void {
    this.sent.push(JSON.parse(data) as Record<string, unknown>);
  }
  close(): void {
    this.readyState = 3;
  }
  open(): void {
    this.readyState = 1;
    this.onopen?.({});
  }
  push(frame: unknown): void {
    this.onmessage?.({ data: JSON.stringify(frame) });
  }
  drop(): void {
    this.readyState = 3;
    this.onclose?.({});
  }
  subs(): Record<string, unknown>[] {
    return this.sent.filter((m) => m.t === "sub");
  }
}

function harness(opts: { onTrouble?: () => void; fail?: boolean } = {}) {
  const sockets: FakeSocket[] = [];
  const link = new GatewayLink({
    socketUrl: async () => {
      if (opts.fail) throw new Error("no ticket");
      return "ws://127.0.0.1:1/api/agg/ws?ticket=t";
    },
    openSocket: (url) => {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
    backoffMs: [1],
    pingMs: 60_000,
    againMs: 1,
    onTrouble: opts.onTrouble,
  });
  return { link, sockets };
}

const DEVICES = [
  { id: "m-studio", name: "studio", state: "online", sinceMs: 5, copyAsOfMs: 1_700_000_000_000, os: "macOS 15.1 arm64", agentVersion: "0.2.122" },
  { id: "m-laptop", name: null, state: "offline", sinceMs: 6, copyAsOfMs: 1_699_000_000_000, os: null, agentVersion: null },
];

const row = (id: string, extra: Record<string, unknown> = {}) => ({
  source: "claude",
  id,
  title: `title ${id}`,
  cwd: `/work/${id}`,
  updatedAt: "2026-10-19T00:00:00.000Z",
  activity: "idle",
  status: "idle",
  asking: false,
  scope: "owned",
  unreadCount: 0,
  ...extra,
});

test("the directory is what the page can use of `dir`", () => {
  const dir = parseDir([...DEVICES, { name: "no id" }, null, { id: "m-x", state: "weird" }]);
  assert.deepEqual(
    dir.map((d) => [d.id, d.state]),
    [
      ["m-studio", "online"],
      ["m-laptop", "offline"],
      ["m-x", "unknown"],
    ],
  );
  assert.equal(dir[1]!.name, null);
});

test("a subscription is sent once open, again after a reconnect from where the page stands, and again in a moment when overrun or busy", async (t) => {
  const { link, sockets } = harness();
  t.after(() => link.close());
  link.start();
  await until(() => sockets.length === 1);
  const got: { reset: boolean; bodies: unknown[]; headSeq: number; epoch: number }[] = [];
  let from: { epoch: number; seq: number } | null = null;
  const stop = link.sub("m-studio", { kind: "index" }, () => from, { onCopy: (c) => void got.push(c) });
  assert.equal(sockets[0]!.sent.length, 0, "nothing goes out before the socket opens");
  sockets[0]!.open();
  assert.deepEqual(sockets[0]!.subs(), [{ t: "sub", s: 1, machine: "m-studio", name: { kind: "index" } }]);
  sockets[0]!.push({ t: "copy", s: 1, reset: true, bodies: [{ reset: true, upserts: [row("a")] }], headSeq: 4, epoch: 2, headTimeMs: 9 });
  assert.equal(got.length, 1);
  assert.equal(got[0]!.epoch, 2);
  from = { epoch: 2, seq: 4 };
  // The connection drops: a new one, the same subscription, from where its copy stands (epoch and seq).
  sockets[0]!.drop();
  await until(() => sockets.length === 2);
  sockets[1]!.open();
  assert.deepEqual(sockets[1]!.subs(), [{ t: "sub", s: 1, machine: "m-studio", name: { kind: "index" }, from: 4, fromEpoch: 2 }]);
  // The page fell behind: the install dropped it; it asks again in a moment.
  sockets[1]!.push({ t: "copy_error", s: 1, code: "overrun", message: "" });
  await until(() => sockets[1]!.subs().length === 2);
  // Asked too fast: the same.
  sockets[1]!.push({ t: "copy_error", s: 1, code: "busy", message: "" });
  await until(() => sockets[1]!.subs().length === 3);
  stop();
  assert.deepEqual(sockets[1]!.sent.at(-1), { t: "unsub", s: 1 });
});

test("a subscription the install ended is let go of on both sides", async (t) => {
  const { link, sockets } = harness();
  t.after(() => link.close());
  link.start();
  await until(() => sockets.length === 1);
  sockets[0]!.open();
  const ends: string[] = [];
  link.sub("m-studio", { kind: "session", source: "claude", id: "x" }, () => null, { onCopy: () => undefined, onEnd: (code) => void ends.push(code) });
  sockets[0]!.push({ t: "copy_error", s: 1, code: "too_large", message: "" });
  assert.deepEqual(ends, ["too_large"]);
  assert.deepEqual(sockets[0]!.sent.at(-1), { t: "unsub", s: 1 }, "said once more, in case");
  await flush(10);
  assert.equal(sockets[0]!.subs().length, 1, "not asked for again");
});

test("the install sending the page away ends the link for good", async () => {
  const { link, sockets } = harness();
  const byes: string[] = [];
  link.onBye((bye) => void byes.push(bye.code));
  link.start();
  await until(() => sockets.length === 1);
  sockets[0]!.open();
  sockets[0]!.push({ t: "dir", machines: DEVICES });
  assert.equal(link.dir.get()?.length, 2);
  sockets[0]!.push({ t: "bye", code: "no_remote_access", message: "not any more" });
  assert.deepEqual(byes, ["no_remote_access"]);
  assert.deepEqual(link.dir.get(), [], "nothing of the other devices is shown");
  await flush(20);
  assert.equal(sockets.length, 1, "and it does not come back");
});

test("failing over and over, the page looks again at whether it should be open at all", async () => {
  let trouble = 0;
  const { link } = harness({ fail: true, onTrouble: () => void trouble++ });
  link.start();
  await until(() => trouble >= 1, 2000);
  link.close();
});

test("a device's copy through the gateway: folded with droppen.app's folds, memory only, no WebCrypto", async (t) => {
  // No storage and no crypto: a page on a plain-http LAN address has neither IndexedDB use nor `crypto.subtle`.
  const subtle = Object.getOwnPropertyDescriptor(globalThis.crypto, "subtle");
  Object.defineProperty(globalThis.crypto, "subtle", {
    configurable: true,
    get() {
      throw new Error("crypto.subtle used");
    },
  });
  t.after(() => {
    if (subtle) Object.defineProperty(globalThis.crypto, "subtle", subtle);
  });
  const { link, sockets } = harness();
  link.start();
  await until(() => sockets.length === 1);
  sockets[0]!.open();
  let touched = 0;
  const mirror = new MachineMirror({
    account: "gateway",
    machine: "m-studio",
    source: new GatewayCopySource(link, "m-studio"),
    transact: async () => {
      touched++;
      return undefined;
    },
    seal: { seal: () => Promise.reject(new Error("sealed")), open: () => Promise.reject(new Error("opened")) },
    persist: false,
    followSessions: false,
    sweep: false,
  });
  await mirror.start();
  const socket = sockets[0]!;
  const index = socket.subs().find((m) => (m.name as { kind: string }).kind === "index")!;
  socket.push({ t: "copy", s: index.s, reset: true, bodies: [{ reset: true, upserts: [row("a"), row("q", { asking: true })], removed: [], asOf: 3 }], headSeq: 5, headTimeMs: 1000 });
  assert.deepEqual(mirror.rows()?.map((r) => r.id).sort(), ["a", "q"]);
  assert.equal(mirror.freshestMs, 1000);
  // A change on top, as the device wrote it.
  socket.push({ t: "copy", s: index.s, bodies: [{ upserts: [], removed: ["claude/a"] }], headSeq: 6, headTimeMs: 2000 });
  assert.deepEqual(mirror.rows()?.map((r) => r.id), ["q"]);
  // A session, opened: subscribed, folded, there.
  const opened = mirror.session("claude", "q", 2000);
  await until(() => socket.subs().some((m) => (m.name as { kind: string }).kind === "session"));
  const sub = socket.subs().find((m) => (m.name as { kind: string }).kind === "session")!;
  assert.deepEqual(sub.name, { kind: "session", source: "claude", id: "q" });
  socket.push({ t: "copy", s: sub.s, reset: true, bodies: [{ fromIndex: 0, items: [{ kind: "user", text: "hi" }], endIndex: 1, fp: "f", prefixFp: "p" }], headSeq: 2, headTimeMs: 3000 });
  const copy = await opened;
  assert.equal(copy?.state.items.length, 1);
  assert.equal(copy?.state.prefixFp, "p");
  // A session the device does not have: answered, not waited on for long.
  const missing = mirror.session("claude", "nope", 2000);
  await until(() => socket.subs().some((m) => (m.name as { id?: string }).id === "nope"));
  socket.push({ t: "missing", s: socket.subs().find((m) => (m.name as { id?: string }).id === "nope")!.s });
  assert.equal(await missing, null);
  await mirror.flush();
  assert.equal(touched, 0, "nothing read from or written to the device's storage");
  mirror.close();
  link.close();
});

test("a copy the page does not hold is taken whole again, and one ended while its device is listed is asked for again", async (t) => {
  const { link, sockets } = harness();
  t.after(() => link.close());
  link.start();
  await until(() => sockets.length === 1);
  const socket = sockets[0]!;
  socket.open();
  socket.push({ t: "dir", machines: DEVICES });
  const source = new GatewayCopySource(link, "m-studio", [1]);
  const states: unknown[] = [];
  const follow = source.follow({ kind: "index" }, [], { at: 0, from: null }, { onState: (c) => void states.push(c.state) });
  t.after(() => follow?.close());
  const first = socket.subs().at(-1)!;
  // A change, with no copy under it (its snapshot never came): not folded onto nothing.
  socket.push({ t: "copy", s: first.s, bodies: [{ upserts: [row("b")], removed: [] }], headSeq: 7, epoch: 1, headTimeMs: 5 });
  assert.equal(states.length, 0);
  assert.deepEqual(socket.sent.at(-1), { t: "unsub", s: first.s });
  await until(() => socket.subs().length === 2);
  const again = socket.subs().at(-1)!;
  assert.equal(again.from, undefined, "from nothing: the whole copy");
  socket.push({ t: "copy", s: again.s, reset: true, bodies: [{ reset: true, upserts: [row("a")], removed: [] }], headSeq: 7, epoch: 1, headTimeMs: 5 });
  assert.equal(states.length, 1);
  // The install let go of it (its list was cleared a moment): the device is still listed, so it is asked for again — from where it stands.
  socket.push({ t: "copy_error", s: again.s, code: "gone", message: "" });
  await until(() => socket.subs().length === 3);
  assert.deepEqual([socket.subs().at(-1)!.from, socket.subs().at(-1)!.fromEpoch], [7, 1]);
  // A device no longer listed: not asked for again.
  socket.push({ t: "dir", machines: [DEVICES[1]] });
  socket.push({ t: "copy_error", s: socket.subs().at(-1)!.s, code: "gone", message: "" });
  await flush(10);
  assert.equal(socket.subs().length, 3);
});

test("the shell keeps `local` the home and puts the install's devices beside it, read-only, as they come and go", async (t) => {
  t.after(() => {
    stopGatewayShell();
    registry.setShell("local", LOCAL);
  });
  const { link, sockets } = harness();
  const shell = startGatewayShell(link, { user: "alice" });
  assert.equal(gatewayShell(), shell);
  await until(() => sockets.length === 1);
  sockets[0]!.open();
  assert.equal(registry.shellKind(), "local", "no other device yet: the local app as it was");
  assert.deepEqual(shell.machines().get(), []);
  sockets[0]!.push({ t: "dir", machines: DEVICES });
  assert.equal(registry.shellKind(), "gateway");
  assert.equal(registry.homeMachine(), LOCAL);
  assert.deepEqual(registry.openMachines(), [LOCAL, "m-studio", "m-laptop"]);
  const infos = shell.machines().get();
  assert.deepEqual(
    infos.map((i) => [i.key, i.state, i.readOnly ?? false]),
    [
      [LOCAL, "online", false],
      ["m-studio", "online", true],
      ["m-laptop", "offline", true],
    ],
  );
  assert.equal(infos[2]!.copyAsOf, 1_699_000_000_000, "how current the offline one's copy is");
  assert.equal(infos[2]!.name, "m-laptop", "no name: its id");
  // Nothing new starts on them here.
  const t9 = (key: string) => key;
  assert.equal(canStartOn(infos[1]!), false);
  assert.match(deviceNote(infos[1]!, t9, { forStarting: true }) ?? "", /read-only here/);
  // Its transport answers from the copy, and refuses to write.
  const api = registry.runtimeFor("m-studio").transport;
  const refused = await api.fetch(new Request("http://x/api/sessions/claude/a/message", { method: "POST", body: "{}" }));
  assert.equal(refused.status, 503);
  // One leaves the directory: closed; the last: the local app alone again.
  sockets[0]!.push({ t: "dir", machines: [DEVICES[0]] });
  assert.deepEqual(registry.openMachines(), [LOCAL, "m-studio"]);
  sockets[0]!.push({ t: "dir", machines: [] });
  assert.equal(registry.shellKind(), "local");
  assert.deepEqual(registry.openMachines(), [LOCAL]);
  stopGatewayShell();
  assert.equal(gatewayShell(), null);
});

test("stopping the shell closes every other device and goes back to the local app", async () => {
  const { link, sockets } = harness();
  const shell = new GatewayShell(link, { user: null });
  shell.start();
  await until(() => sockets.length === 1);
  sockets[0]!.open();
  sockets[0]!.push({ t: "dir", machines: DEVICES });
  assert.equal(registry.isOpen("m-studio"), true);
  shell.stop();
  assert.equal(registry.isOpen("m-studio"), false);
  assert.equal(registry.isOpen(LOCAL), true);
  assert.equal(registry.shellKind(), "local");
  assert.equal(sockets[0]!.readyState, 3, "its socket closed");
});

test("the page shows other devices only when the install says this account sees them and its aggregator runs", () => {
  assert.equal(showsOtherDevices(undefined), false);
  assert.equal(showsOtherDevices({ seesOtherDevices: true, aggregator: { enabled: true, state: "on" } }), true);
  assert.equal(showsOtherDevices({ seesOtherDevices: false, aggregator: { enabled: true, state: "on" } }), false);
  assert.equal(showsOtherDevices({ seesOtherDevices: true, aggregator: { enabled: false, state: "off" } }), false);
  assert.equal(showsOtherDevices({ seesOtherDevices: true, aggregator: { enabled: true, state: "pending" } }), false);
  // An older install, without the field: nothing.
  assert.equal(showsOtherDevices({ aggregator: { enabled: true, state: "on" } }), false);
});
