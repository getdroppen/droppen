// Moving the page between its machine and the machine's offline copy
// (`features/cloud/cloud-transport.ts`): the switching transport sends each
// request and socket to the side the page is on and closes the other side's
// sockets on a switch; the controller goes live the moment the channel opens,
// back to the copy only after the channel has stayed down, and keeps trying
// the channel while the machine is online.
import assert from "node:assert/strict";
import test from "node:test";
import { type CloudMode, ModeController, SwitchingTransport } from "../src/features/cloud/cloud-transport.js";
import type { LinkState, PresenceState } from "../src/lib/hub/link.js";
import type { MachineStatus } from "../src/lib/hub/machine.js";
import type { SocketLike, Transport } from "../src/lib/transport/index.js";
import { flush, until } from "./stream-fixtures.js";

class StubSocket implements SocketLike {
  readyState = 1;
  binaryType = "blob";
  sent: string[] = [];
  onopen: ((e: unknown) => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  onclose: ((e: unknown) => void) | null = null;
  constructor(readonly url: string) {}
  send(data: string) {
    this.sent.push(data);
  }
  close() {
    this.readyState = 3;
    this.onclose?.({});
  }
}

function stub(name: string): Transport & { sockets: StubSocket[] } {
  const sockets: StubSocket[] = [];
  return {
    sockets,
    fetch: async () => new Response(name),
    socketUrl: async (path) => path,
    openSocket: (url) => {
      const s = new StubSocket(url);
      sockets.push(s);
      return s;
    },
  };
}

test("the switching transport: each request goes to the current side, and a switch closes the other side's sockets", async () => {
  const live = stub("live");
  const offline = stub("offline");
  const opened: string[] = [];
  const t = new SwitchingTransport({ live, offline }, "offline", { onOpened: (s, id) => opened.push(`${s}/${id}`) });
  const modes: CloudMode[] = [];
  t.onMode((m) => modes.push(m));
  assert.equal(await (await t.fetch(new Request("http://x/api/sessions"))).text(), "offline");
  const feed = t.openSocket(await t.socketUrl("/api/events?v=2"));
  const term = t.openSocket("/api/terminals/t/ws");
  let closed = 0;
  feed.onclose = () => closed++;
  term.onclose = () => closed++;

  t.setMode("live");
  assert.deepEqual(modes, ["live"]);
  assert.equal(closed, 2, "the copy's sockets are closed, so their owners reconnect through the machine");
  assert.equal(await (await t.fetch(new Request("http://x/api/sessions"))).text(), "live");
  const liveFeed = t.openSocket("/api/events?v=2&z=1");
  assert.equal(live.sockets.length, 1);
  // A session subscribed on the live feed socket counts as opened on this device.
  liveFeed.send(JSON.stringify({ type: "sub", feeds: [{ sub: 1, source: "claude", id: "abc" }, { sub: 2, source: "codex", id: "x/y" }] }));
  liveFeed.send(JSON.stringify({ type: "ping" }));
  assert.deepEqual(opened, ["claude/abc", "codex/x/y"]);
  assert.equal(live.sockets[0]!.sent.length, 2, "and the messages still go out");

  t.setMode("live");
  assert.deepEqual(modes, ["live"], "no switch to the side it is on");
  t.setMode("offline");
  assert.equal(live.sockets[0]!.readyState, 3);
});

class FakeLink {
  state: LinkState = "open";
  presence = new Map<string, { state: PresenceState }>();
  private s = new Set<(s: LinkState) => void>();
  private p = new Set<(p: unknown) => void>();
  onState(fn: (s: LinkState) => void) {
    this.s.add(fn);
    return () => this.s.delete(fn);
  }
  onPresence(fn: (p: unknown) => void) {
    this.p.add(fn);
    return () => this.p.delete(fn);
  }
  setPresence(state: PresenceState) {
    this.presence.set("m-1", { state });
    for (const fn of this.p) fn(this.presence);
  }
}

class FakeChannel {
  status: MachineStatus = "idle";
  ensures = 0;
  /** What the next `ensure` does. */
  outcome: "open" | "fail" | "hang" = "open";
  private l = new Set<(s: MachineStatus) => void>();
  onStatus(fn: (s: MachineStatus) => void) {
    this.l.add(fn);
    return () => this.l.delete(fn);
  }
  set(status: MachineStatus) {
    this.status = status;
    for (const fn of [...this.l]) fn(status);
  }
  ensure(): Promise<unknown> {
    this.ensures++;
    this.set("connecting");
    if (this.outcome === "hang") return new Promise(() => undefined);
    return new Promise((resolve, reject) =>
      setTimeout(() => {
        if (this.outcome === "open") {
          this.set("open");
          resolve(undefined);
        } else {
          this.set("offline");
          reject(new Error("the machine is offline"));
        }
      }, 1),
    );
  }
}

function controller(downMs = 40) {
  const link = new FakeLink();
  const channel = new FakeChannel();
  const transport = new SwitchingTransport({ live: stub("live"), offline: stub("offline") }, "offline");
  const switches: CloudMode[] = [];
  const c = new ModeController({ link, channel, machine: "m-1", transport, onSwitch: (m) => switches.push(m), downMs, retryMs: () => 5 });
  return { link, channel, transport, switches, c };
}

test("the page starts on the copy and goes live as soon as the channel to an online machine is open", async () => {
  const { link, channel, transport, switches, c } = controller();
  link.setPresence("offline");
  c.start();
  await flush(5);
  assert.equal(transport.current, "offline");
  assert.equal(channel.ensures, 0, "nobody dials a machine the hub says is offline");
  link.setPresence("online");
  await until(() => transport.current === "live");
  assert.equal(channel.ensures, 1);
  assert.deepEqual(switches, ["live"]);
  c.stop();
});

test("live → copy only after the channel has stayed down; a blip is reconnected through", async (t) => {
  // The window is what is tested, so time is the test's: on the real clock a
  // loaded machine stretches a 20 ms blip past a 40 ms window.
  t.mock.timers.enable({ apis: ["setTimeout"] });
  /** Let `ms` go by, a millisecond at a time, with what each turn set off run in between. */
  const pass = async (ms: number) => {
    for (let i = 0; i < ms; i++) {
      t.mock.timers.tick(1);
      await new Promise((resolve) => setImmediate(resolve));
    }
  };
  const { link, channel, transport, switches, c } = controller(40);
  link.setPresence("online");
  c.start();
  await pass(1);
  assert.equal(transport.current, "live");

  // A blip: down, and back within the window.
  channel.set("idle");
  await pass(10);
  channel.set("connecting");
  await pass(10);
  channel.set("open");
  await pass(50);
  assert.equal(transport.current, "live", "no flap for a drop shorter than the window");
  assert.deepEqual(switches, ["live"]);

  // Gone for good: the machine went away.
  channel.outcome = "fail";
  link.setPresence("offline");
  channel.set("offline");
  await pass(39);
  assert.equal(transport.current, "live", "not at once, nor before the window is up");
  await pass(1);
  assert.equal(transport.current, "offline", "the copy, once it is");
  assert.deepEqual(switches, ["live", "offline"]);
  const tried = channel.ensures;
  await pass(30);
  assert.equal(channel.ensures, tried, "and nobody dials a machine the hub says is offline");

  // It comes back.
  channel.outcome = "open";
  link.setPresence("online");
  await pass(1);
  assert.equal(transport.current, "live");
  assert.deepEqual(switches, ["live", "offline", "live"]);
  c.stop();
});

test("on the copy with the machine online, a channel that will not open is retried with a backoff", async () => {
  const { link, channel, transport, c } = controller();
  channel.outcome = "fail";
  link.setPresence("online");
  c.start();
  await until(() => channel.ensures >= 3);
  assert.equal(transport.current, "offline");
  channel.outcome = "open";
  await until(() => transport.current === "live");
  c.stop();
  const after = channel.ensures;
  await flush(30);
  assert.equal(channel.ensures, after, "a stopped controller does nothing");
});

test("a channel that would not open is tried again the moment the page comes back, not at the end of a backoff the phone held", async () => {
  const link = new FakeLink();
  const channel = new FakeChannel();
  const transport = new SwitchingTransport({ live: stub("live"), offline: stub("offline") }, "offline");
  const resumed = new Set<() => void>();
  // A backoff far longer than the test: only the page coming back can bring the next try.
  const c = new ModeController({
    link,
    channel,
    machine: "m-1",
    transport,
    downMs: 40,
    retryMs: () => 600_000,
    resume: (fn) => {
      resumed.add(fn);
      return () => resumed.delete(fn);
    },
  });
  channel.outcome = "fail";
  link.setPresence("online");
  c.start();
  await until(() => channel.ensures === 1 && channel.status === "offline");
  await flush(20);
  assert.equal(channel.ensures, 1, "waiting out its backoff");
  // The phone is unlocked (visible, focused, online again): a try at once, and it opens.
  channel.outcome = "open";
  for (const fn of resumed) fn();
  await until(() => transport.current === "live");
  assert.equal(channel.ensures, 2);
  c.stop();
  assert.equal(resumed.size, 0, "a stopped controller no longer listens for the page coming back");
});

test("a machine the hub has said nothing about yet is tried, not given up on", async () => {
  const { transport, channel, c } = controller();
  c.start();
  await until(() => transport.current === "live");
  assert.equal(channel.ensures, 1);
  c.stop();
});

test("the switching transport passes on the live side's new connections, whichever side the page is on", () => {
  const listeners = new Set<() => void>();
  const live: Transport = { ...stub("live"), onReconnect: (fn) => (listeners.add(fn), () => listeners.delete(fn)) };
  const t = new SwitchingTransport({ live, offline: stub("offline") }, "offline");
  let heard = 0;
  const off = t.onReconnect(() => heard++);
  for (const fn of listeners) fn();
  assert.equal(heard, 1, "a new channel while the page still shows the offline copy");
  t.setMode("live");
  for (const fn of listeners) fn();
  assert.equal(heard, 2);
  off();
  assert.equal(listeners.size, 0, "and the subscription goes with its caller");
  // A live side with no connection of its own says nothing, and costs nothing.
  assert.doesNotThrow(() => new SwitchingTransport({ live: stub("live"), offline: stub("offline") }).onReconnect(() => undefined)());
});

test("a machine nobody wants live is shown from its copy and never dialled, online or not, until it is wanted", async () => {
  const link = new FakeLink();
  const channel = new FakeChannel();
  const transport = new SwitchingTransport({ live: stub("live"), offline: stub("offline") }, "offline");
  let wanted = false;
  const c = new ModeController({ link, channel, machine: "m-1", transport, wanted: () => wanted, retryMs: () => 5 });
  link.setPresence("online");
  c.start();
  await flush(10);
  assert.equal(channel.ensures, 0, "online, but not wanted: nothing dialled");
  assert.equal(transport.current, "offline");
  link.setPresence("online");
  await flush(10);
  assert.equal(channel.ensures, 0);
  wanted = true;
  c.poke();
  await until(() => transport.current === "live");
  assert.equal(channel.ensures, 1, "dialled the moment it is wanted");
  c.stop();
});

test("a disposed switching transport closes what either side opened and refuses whatever comes after", async () => {
  const live = stub("live");
  const offline = stub("offline");
  const t = new SwitchingTransport({ live, offline }, "offline");
  const a = t.openSocket("/api/events");
  t.setMode("live");
  const b = t.openSocket("/api/events");
  let switched = 0;
  t.onMode(() => switched++);
  t.dispose();
  assert.equal(a.readyState, 3);
  assert.equal(b.readyState, 3);
  await assert.rejects(t.fetch(new Request("http://machine.test/api/prefs")), /no longer open/);
  await assert.rejects(t.socketUrl("/api/events"), /no longer open/);
  assert.throws(() => t.openSocket("/api/events"), /no longer open/);
  t.setMode("offline");
  assert.equal(switched, 0, "and it moves no more");
  assert.equal(live.sockets.length + offline.sockets.length, 2, "nothing new opened");
});
