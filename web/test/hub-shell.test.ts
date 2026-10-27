// droppen.app's shell (`features/cloud/hub-shell.ts`): every machine of the
// account on the page at once — each on its offline copy until something
// wants it live, so listing them dials nothing — one of them in view (the one
// the app's address is about), others held by what shows them (panes), and
// one the main device (the registry's home: screens, plain addresses).
// Moving between them is a matter of the address and nothing else: no
// reload, the other machines' channels, copies and queries left as they
// were. A switch between a machine and its copy re-asks that machine's
// queries only; the chain gaining or losing a machine opens or closes it.
import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient } from "@tanstack/react-query";
import { addressOn, endHubShell, HubShell, mainMachine, selectedMachine, switchPath } from "../src/features/cloud/hub-shell.js";
import { type AccountMain, HubApiError } from "../src/lib/hub/api.js";
import type { PresenceState } from "../src/lib/hub/link.js";
import type { MachineStatus } from "../src/lib/hub/machine.js";
import { mq } from "../src/lib/machines/query.js";
import { hubShell, setHubShell } from "../src/features/machines/shells.js";
import { renewalRunner, setRenewalRunner } from "../src/lib/hub/renewal.js";
import { homeMachine, homeMachineStore, isOpen, openMachines, perMachine, runtimeFor, shellKind, transportFor } from "../src/lib/machines/registry.js";
import { MemoryKeyStore } from "../src/lib/e2e/index.js";
import { FakeKv, FakeStreamLink, flush, type Machine, makeMachine, until } from "./stream-fixtures.js";

class MemoryStorage {
  values = new Map<string, string>();
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.values.set(key, value);
  }
  removeItem(key: string) {
    this.values.delete(key);
  }
}
const storage = new MemoryStorage();
const reloads: string[] = [];
Object.assign(globalThis, { localStorage: storage, location: { reload: () => void reloads.push("reload"), pathname: "/sessions", search: "" } });

class FakeLink {
  state = "open" as const;
  presence = new Map<string, { state: PresenceState }>();
  carriesChannels = true;
  private presenceListeners = new Set<(p: unknown) => void>();
  private controlListeners = new Set<(m: Record<string, unknown>) => void>();
  private stateListeners = new Set<(s: string) => void>();
  onPresence(fn: (p: unknown) => void) {
    this.presenceListeners.add(fn);
    return () => this.presenceListeners.delete(fn);
  }
  onState(fn: (s: string) => void) {
    this.stateListeners.add(fn);
    return () => void this.stateListeners.delete(fn);
  }
  onControl(fn: (m: Record<string, unknown>) => void) {
    this.controlListeners.add(fn);
    return () => void this.controlListeners.delete(fn);
  }
  set(machine: string, state: PresenceState) {
    this.presence.set(machine, { state });
    for (const fn of [...this.presenceListeners]) fn(this.presence);
  }
  /** A control message from the hub (`main_device`, …). */
  control(message: Record<string, unknown>) {
    for (const fn of [...this.controlListeners]) fn(message);
  }
  /** The link (re)connected and signed in. */
  opened() {
    for (const fn of [...this.stateListeners]) fn("open");
  }
}

/**
 * The hub's `/v1/main-device` as a page sees it: every write recorded, and
 * answered by `answer` — by default as the hub would for a lone page (the
 * machine written; with `ifUnset`, the one already set if there is one).
 */
class FakeMainApi {
  writes: { machine: string; ifUnset: boolean }[] = [];
  account: string | null = null;
  clock = 100;
  answer: (machine: string, ifUnset: boolean) => Promise<AccountMain> = async (machine, ifUnset) => {
    if (!ifUnset || this.account === null) this.account = machine;
    return { machine: this.account, setMs: ++this.clock };
  };
  setMainDevice = (_token: string, machine: string, opts: { ifUnset?: boolean } = {}) => {
    this.writes.push({ machine, ifUnset: opts.ifUnset === true });
    return this.answer(machine, opts.ifUnset === true);
  };
}

function fakeAccount(hosts: { id: string; active?: boolean }[]) {
  const listeners = new Set<() => void>();
  const account = {
    account: "a-1",
    deviceId: "d-1",
    isAdmin: true,
    session: { token: "t" },
    hosts,
    chain: {
      hosts: () => account.hosts.map((h) => ({ info: { id: h.id }, status: { type: h.active === false ? "removed" : "active" } })),
      latestMachineEpoch: () => null,
    },
    // No keys for any machine: every copy is inert, which is all these tests need.
    machine: () => undefined,
    machineKeys: async () => new Map(),
    refreshKeys: async () => undefined,
    onChange(fn: () => void) {
      listeners.add(fn);
      return () => void listeners.delete(fn);
    },
    changed() {
      for (const fn of [...listeners]) fn();
    },
  };
  return account;
}

/** Nothing remembered from an earlier test: no device last in view, no main device. */
function forget() {
  storage.removeItem("rh.hub.machine.a-1");
  storage.removeItem("rh.hub.home.a-1");
}

function world(hosts: { id: string; active?: boolean }[]) {
  forget();
  const link = new FakeLink();
  const account = fakeAccount(hosts);
  const queryClient = new QueryClient();
  const relays = { online: () => false, onChange: () => () => undefined, notice: null };
  const api = new FakeMainApi();
  const shell = new HubShell({
    api: api as never,
    account: account as never,
    link: link as never,
    relays: relays as never,
    store: {} as never,
    queryClient,
  });
  return { link, account, queryClient, shell, api };
}

/** A stream of the fake channel: what a pipe attaches to. */
class FakeStream {
  ondata: ((data: Uint8Array) => void) | null = null;
  onclose: ((code: number) => void) | null = null;
  closed = false;
  send() {}
  close() {
    this.closed = true;
  }
}

/** Stand in for the machine end of `key`'s channel: count the dials, open when told to, and hand pipes streams that stay open. */
function dialer(shell: HubShell, key: string, outcome: "open" | "hang" = "open") {
  const channel = shell.machine(key)!.channel;
  const counted = { ensures: 0, streams: [] as FakeStream[] };
  const driver = {
    open: () => {
      const stream = new FakeStream();
      counted.streams.push(stream);
      return stream;
    },
  };
  channel.ensure = () => {
    counted.ensures++;
    (channel as unknown as { setStatus(s: MachineStatus): void }).setStatus("connecting");
    if (outcome === "hang") return new Promise(() => undefined);
    (channel as unknown as { setStatus(s: MachineStatus): void }).setStatus("open");
    return Promise.resolve(driver as never);
  };
  return counted;
}

test("a machine that refuses this browser shows why: refused outright, or waiting for another device's word", async (t) => {
  const { link, shell } = world([{ id: "m-a" }]);
  link.set("m-a", "online");
  shell.start();
  t.after(() => shell.stop());
  const channel = shell.machine("m-a")!.channel;
  const fail = (message: string) => {
    (channel as unknown as { setStatus(s: MachineStatus, e?: string): void }).setStatus("error", message);
    return Promise.reject(new Error(message));
  };
  channel.ensure = () => {
    channel.refusal = { kind: "notAccepted", by: "hub", why: "that device does not accept control from this one" };
    return fail("the hub refused the channel: that device does not accept control from this one");
  };
  const live = () => shell.machines().get().find((m) => m.key === "m-a")!.live;
  shell.select("m-a");
  await until(() => live() === "refused");
  channel.refusal = { kind: "unconfirmed" };
  (channel as unknown as { setStatus(s: MachineStatus, e?: string): void }).setStatus("error", "the device refused the channel");
  await until(() => live() === "stale");
});

test("every active machine of the account is on the page at once, on its copy, and none is dialled until one is wanted", async (t) => {
  const { link, shell } = world([{ id: "m-a" }, { id: "m-b" }, { id: "m-gone", active: false }]);
  link.set("m-a", "online");
  link.set("m-b", "online");
  shell.start();
  t.after(() => shell.stop());
  const a = dialer(shell, "m-a");
  const b = dialer(shell, "m-b");

  assert.equal(shellKind(), "hub");
  assert.deepEqual(openMachines().sort(), ["m-a", "m-b"], "the active machines, and no `local`");
  assert.equal(isOpen("m-gone"), false);
  assert.equal(shell.focus.get(), null, "nothing remembered: the machine list");
  assert.deepEqual(
    shell.machines().get().map((m) => [m.key, m.state, m.live]),
    [
      ["m-a", "online", "copy"],
      ["m-b", "online", "copy"],
    ],
  );
  for (const key of ["m-a", "m-b"]) assert.equal(transportFor(key), shell.machine(key)!.transport, "each through its own switching transport");
  await flush(10);
  assert.deepEqual([a.ensures, b.ensures], [0, 0], "online, but nobody asked for them: nothing dialled");

  // Bring one into view: it goes live, and only it. The main device (the
  // home) is the account's first device that runs AI, in the chain's order,
  // on a browser that remembers none — not the one opened.
  shell.select("m-b");
  assert.equal(homeMachine(), "m-a");
  assert.equal(mainMachine("a-1"), "m-a");
  assert.equal(shell.focus.get(), "m-b");
  await until(() => shell.machine("m-b")!.transport.current === "live");
  assert.equal(shell.machines().get().find((m) => m.key === "m-b")!.live, "live");
  assert.deepEqual([a.ensures, b.ensures], [0, 1]);
  assert.equal(selectedMachine("a-1"), "m-b", "remembered for the next load");

  // Switch: the main device stays, the other machine stays exactly as it was.
  const before = shell.machine("m-b")!;
  shell.select("m-a");
  assert.equal(homeMachine(), "m-a", "the main device does not follow the device in view");
  assert.equal(shell.focus.get(), "m-a");
  await until(() => shell.machine("m-a")!.transport.current === "live");
  assert.equal(shell.machine("m-b"), before, "the machine left is not rebuilt");
  assert.equal(before.transport.current, "live", "and keeps its channel");
  assert.equal(runtimeFor("m-b").transport, before.transport);
  assert.ok(shell.movesAway("m-b"), "back to the other machine leaves this one's page");
  assert.ok(!shell.movesAway("m-a"));

  // The list: the app goes, the machines stay.
  shell.select(null);
  assert.equal(shell.focus.get(), null);
  assert.equal(homeMachine(), "m-a", "the home stays the main device");
  assert.equal(selectedMachine("a-1"), null);
  assert.deepEqual(reloads, [], "and nothing ever reloaded the page");
});

test("a machine going live or back to its copy re-asks that machine's queries, and no other machine's", async (t) => {
  const { link, queryClient, shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  link.set("m-a", "online");
  link.set("m-b", "online");
  shell.start();
  t.after(() => shell.stop());
  dialer(shell, "m-a");
  dialer(shell, "m-b");
  queryClient.setQueryData(mq("m-a", "prefs"), { n: 1 });
  queryClient.setQueryData(mq("m-b", "prefs"), { n: 2 });

  shell.select("m-a");
  await until(() => queryClient.getQueryState(mq("m-a", "prefs"))?.isInvalidated === true);
  assert.equal(queryClient.getQueryState(mq("m-b", "prefs"))?.isInvalidated, false, "the other machine's cache is untouched");
});

test("the chain gaining a machine opens it; losing the one in view goes back to the list and closes it", async (t) => {
  const { link, account, shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  link.set("m-a", "offline");
  link.set("m-b", "offline");
  storage.setItem("rh.hub.home.a-1", "m-b");
  shell.start();
  t.after(() => shell.stop());
  assert.equal(shell.focus.get(), "m-b", "the main device, brought back");
  assert.equal(homeMachine(), "m-b");

  const disposed: string[] = [];
  const kept = perMachine(
    (machine) => machine,
    (machine) => disposed.push(machine),
  );
  kept("m-b");

  account.hosts.push({ id: "m-c" });
  account.changed();
  assert.ok(isOpen("m-c"));
  assert.deepEqual(shell.machines().get().map((m) => m.key), ["m-a", "m-b", "m-c"]);

  const leaving = shell.machine("m-b")!;
  account.hosts.splice(1, 1);
  account.changed();
  assert.equal(shell.focus.get(), null, "the machine in view went: the list");
  assert.notEqual(homeMachine(), "m-b");
  assert.equal(isOpen("m-b"), false);
  assert.throws(() => runtimeFor("m-b"), /no machine "m-b"/);
  assert.deepEqual(disposed, ["m-b"], "what was made for it per machine is let go (its feed socket, its session feeds)");
  assert.equal((leaving.mirror as unknown as { closed: boolean }).closed, true, "its copy stops following");
  assert.equal(shell.machine("m-b"), null);
  assert.deepEqual(shell.machines().get().map((m) => m.key), ["m-a", "m-c"]);
});

test("what the page knows of a machine follows its presence, without asking it anything", async (t) => {
  let now = 1_000;
  const link = new FakeLink();
  const account = fakeAccount([{ id: "m-a" }]);
  const shell = new HubShell({
    api: {} as never,
    account: account as never,
    link: link as never,
    relays: { online: () => false, onChange: () => () => undefined } as never,
    store: {} as never,
    queryClient: new QueryClient(),
    now: () => now,
  });
  shell.start();
  t.after(() => shell.stop());
  const info = shell.runtime("m-a").info();
  assert.equal(info.get().state, "unknown");
  assert.equal(info.get().name, "m-a", "no copy, no name: its id");
  assert.equal(info.get().canManage, true);
  let heard = 0;
  info.subscribe(() => heard++);
  now = 5_000;
  link.set("m-a", "offline");
  assert.equal(info.get().state, "offline");
  assert.equal(info.get().since, 5_000);
  assert.equal(info.get().live, "copy");
  assert.equal(heard, 1);
  link.set("m-a", "offline");
  assert.equal(heard, 1, "nothing new: nobody woken");
});

test("a switch leaves the old machine's page for the same section on the new one", () => {
  assert.equal(switchPath("/sessions/claude/abc"), "/sessions");
  assert.equal(switchPath("/sessions"), "/sessions");
  assert.equal(switchPath("/chat/claude/x"), "/chat");
  assert.equal(switchPath("/terminals"), "/terminals");
  assert.equal(switchPath("/files"), "/files");
  assert.equal(switchPath("/settings"), "/settings");
  assert.equal(switchPath("/screens/s-1"), "/sessions", "a screen is the old machine's");
  assert.equal(switchPath("/apps/vscode-1"), "/sessions");
  assert.equal(switchPath("/p/tasks/2026-w42"), "/p/tasks/");
  assert.equal(switchPath("/"), "/sessions");
  assert.equal(switchPath("/m/m-b/terminals"), "/terminals", "another device's page: its section");
  assert.equal(switchPath("/m/m-b/sessions/claude/x"), "/sessions");
});

test("an address on another device: the same page when it is that device's, else that device's page of the same section", () => {
  // The main device is m-a: its pages are plain, every other device's under /m/.
  assert.equal(addressOn("m-a", "/sessions/claude/x?q=1", "m-a"), "/sessions/claude/x?q=1");
  assert.equal(addressOn("m-b", "/sessions/claude/x", "m-a"), "/m/m-b/sessions");
  assert.equal(addressOn("m-b", "/m/m-b/files?path=%2Fsrv", "m-a"), "/m/m-b/files?path=%2Fsrv");
  assert.equal(addressOn("m-a", "/m/m-b/terminals?id=t", "m-a"), "/terminals");
  assert.equal(addressOn("m-c", "/m/m-b/terminals", "m-a"), "/m/m-c/terminals");
  assert.equal(addressOn("m-a", "/m/m-a/files", "m-a"), "/files", "the main device under /m/ is its plain address");
  assert.equal(addressOn("m-b", "/screens/s-1", "m-a"), "/m/m-b/sessions", "a screen is the main device's");
  assert.equal(addressOn("m-b", "/p/tasks/", "m-a"), "/m/m-b/sessions", "so is a plugin's page");
});

// ---------------------------------------------------------------------------
// Bounds and lifetimes
// ---------------------------------------------------------------------------

/** A hub link the streams can run over, with presence, for a shell. */
class StreamShellLink extends FakeStreamLink {
  presence = new Map<string, { state: PresenceState }>();
  carriesChannels = true;
  private presenceListeners = new Set<(p: unknown) => void>();
  onPresence(fn: (p: unknown) => void) {
    this.presenceListeners.add(fn);
    return () => this.presenceListeners.delete(fn);
  }
}

/** A shell over an account whose machines this device holds keys for. */
async function keyedWorld(ids: string[], kv: FakeKv) {
  forget();
  const machines = new Map<string, Machine>();
  for (const id of ids) machines.set(id, await makeMachine());
  const link = new StreamShellLink();
  for (const id of ids) link.presence.set(id, { state: "offline" });
  const account = {
    ...fakeAccount(ids.map((id) => ({ id }))),
    machine: (id: string) => (machines.has(id) ? { signKey: machines.get(id)!.signPublic } : undefined),
    machineKeys: async (id: string) => machines.get(id)?.epochs ?? new Map(),
  };
  const shell = new HubShell({
    api: { streamPage: async () => new Uint8Array() } as never,
    account: account as never,
    link: link as never,
    relays: { online: () => false, onChange: () => () => undefined } as never,
    store: new MemoryKeyStore() as never,
    queryClient: new QueryClient(),
    transact: kv.transact,
    idleMs: 30,
  });
  return { shell, link, machines, account };
}

test("the device is swept of other accounts' copies once per shell, not once per machine", async (t) => {
  const kv = new FakeKv();
  kv.data.set("stream/someone-else/m-z/00", { v: 2 });
  const { shell, account } = await keyedWorld(["m-1", "m-2", "m-3"], kv);
  // One machine at first; the others arrive one at a time, as machines are added.
  const all = [...account.hosts];
  account.hosts.splice(1);
  shell.start();
  t.after(() => shell.stop());
  await until(() => !kv.data.has("stream/someone-else/m-z/00"));
  for (const host of all.slice(1)) {
    account.hosts.push(host);
    account.changed();
    await until(() => shell.machine(host.id)?.mirror.loaded === true);
    await flush(20);
  }
  assert.equal(kv.keyReads.filter((p) => p === "stream/").length, 1, "one sweep for three machines");
});

test("only the machine in view follows its sessions; the others their index and status", async (t) => {
  const { shell } = await keyedWorld(["m-1", "m-2"], new FakeKv());
  shell.start();
  t.after(() => shell.stop());
  const follows = (key: string) => (shell.machine(key)!.mirror as unknown as { followingSessions: boolean }).followingSessions;
  assert.deepEqual([follows("m-1"), follows("m-2")], [false, false], "the list: none");
  shell.select("m-1");
  assert.deepEqual([follows("m-1"), follows("m-2")], [true, false]);
  shell.select("m-2");
  assert.deepEqual([follows("m-1"), follows("m-2")], [false, true]);
  shell.select(null);
  assert.deepEqual([follows("m-1"), follows("m-2")], [false, false]);
  assert.ok(shell.budget.count <= shell.budget.limit);
});

test("a machine out of view lets its channel go after a while, and comes back live when it is in view again", async (t) => {
  const { link, shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  Object.assign(shell.opts, { idleMs: 30 });
  link.set("m-a", "online");
  link.set("m-b", "online");
  shell.start();
  t.after(() => shell.stop());
  const a = dialer(shell, "m-a");
  dialer(shell, "m-b");
  shell.select("m-a");
  await until(() => shell.machine("m-a")!.transport.current === "live");
  shell.select("m-b");
  // Out of view, then idle: the channel goes, the copy is shown, nothing is dialled again.
  await until(() => shell.machine("m-a")!.channel.status === "idle", 2000);
  await until(() => shell.machine("m-a")!.transport.current === "offline", 6000);
  const dialled = a.ensures;
  await flush(50);
  assert.equal(a.ensures, dialled, "not wanted: not dialled");
  assert.equal(shell.machines().get().find((m) => m.key === "m-a")!.live, "copy");
  // Back in view: live again.
  shell.select("m-a");
  await until(() => shell.machine("m-a")!.transport.current === "live");
  assert.equal(a.ensures, dialled + 1);
});

test("a machine switched away from and back within the idle time keeps its channel", async (t) => {
  const { link, shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  Object.assign(shell.opts, { idleMs: 200 });
  link.set("m-a", "online");
  link.set("m-b", "online");
  shell.start();
  t.after(() => shell.stop());
  const a = dialer(shell, "m-a");
  dialer(shell, "m-b");
  shell.select("m-a");
  await until(() => shell.machine("m-a")!.transport.current === "live");
  shell.select("m-b");
  shell.select("m-a");
  await flush(300);
  assert.equal(shell.machine("m-a")!.channel.status, "open");
  assert.equal(a.ensures, 1);
});

test("the last machine going while it is the home lets go of everything made for it, and its transport refuses what comes late", async (t) => {
  const { account, shell } = world([{ id: "m-solo" }]);
  shell.start();
  t.after(() => shell.stop());
  shell.select("m-solo");
  const disposed: string[] = [];
  const kept = perMachine(
    (machine) => machine,
    (machine) => disposed.push(machine),
  );
  kept("m-solo");
  const leaving = shell.machine("m-solo")!;
  account.hosts.splice(0, 1);
  account.changed();
  assert.equal(shell.focus.get(), null);
  assert.equal(isOpen("m-solo"), false, "no longer open");
  assert.doesNotThrow(() => runtimeFor("m-solo"), "but still found by a view on its way out");
  assert.deepEqual(disposed, ["m-solo"], "its per-machine things are let go");
  await assert.rejects(leaving.transport.fetch(new Request("http://machine.test/api/sessions")), /no longer open/);
  assert.throws(() => leaving.transport.openSocket("/api/events"), /no longer open/);
});

test("a machine out of view keeps its channel while it still carries a download, and lets it go once that is done", async (t) => {
  const { link, shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  Object.assign(shell.opts, { idleMs: 30 });
  link.set("m-a", "online");
  link.set("m-b", "online");
  shell.start();
  t.after(() => shell.stop());
  dialer(shell, "m-a");
  dialer(shell, "m-b");
  shell.select("m-a");
  await until(() => shell.machine("m-a")!.transport.current === "live");
  // A download still under way when the person switches away.
  const abort = new AbortController();
  const download = shell.machine("m-a")!.transport.fetch(new Request("http://machine.test/api/fs/download?path=big.iso", { signal: abort.signal }), { class: "blob" });
  download.catch(() => undefined);
  shell.select("m-b");
  await flush(200);
  assert.equal(shell.machine("m-a")!.channel.status, "open", "not cut from under the download");
  assert.equal(shell.machine("m-a")!.transport.current, "live");
  abort.abort();
  await until(() => shell.machine("m-a")!.channel.status === "idle", 2000);
  assert.equal(shell.machine("m-a")!.transport.current, "offline", "on the copy at once, not after the channel has stayed down");
});

test("wanted again just after it was let go: dialled at once; open while unwanted: let go again", async (t) => {
  const { link, shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  Object.assign(shell.opts, { idleMs: 30 });
  link.set("m-a", "online");
  link.set("m-b", "online");
  shell.start();
  t.after(() => shell.stop());
  const a = dialer(shell, "m-a");
  dialer(shell, "m-b");
  shell.select("m-a");
  await until(() => shell.machine("m-a")!.transport.current === "live");
  shell.select("m-b");
  await until(() => shell.machine("m-a")!.channel.status === "idle", 2000);
  // Back within the old three-second window: no wait for a down timer.
  const before = a.ensures;
  const started = Date.now();
  shell.select("m-a");
  await until(() => shell.machine("m-a")!.transport.current === "live", 2000);
  assert.equal(a.ensures, before + 1);
  assert.ok(Date.now() - started < 1000, "dialled straight away");

  // Something dials a machine nobody wants (out of view, released): it is let go again.
  shell.select("m-b");
  await until(() => shell.machine("m-a")!.channel.status === "idle", 2000);
  void shell.machine("m-a")!.channel.ensure();
  assert.equal(shell.machine("m-a")!.channel.status, "open");
  await until(() => shell.machine("m-a")!.channel.status === "idle", 2000);
});

/** A location and history the shell can move, and the page's popstate listeners to fire (Back). */
function addressBar(t: { after(fn: () => void): void }) {
  const place = globalThis.location as unknown as { pathname: string; search: string };
  const listeners = new Map<string, Set<() => void>>();
  const history = {
    state: null,
    replaceState(_state: unknown, _title: string, url: string) {
      const to = new URL(url, "https://droppen.test");
      place.pathname = to.pathname;
      place.search = to.search;
    },
  };
  Object.assign(globalThis, {
    history,
    addEventListener: (type: string, fn: () => void) => void (listeners.get(type) ?? listeners.set(type, new Set()).get(type)!).add(fn),
    removeEventListener: (type: string, fn: () => void) => void listeners.get(type)?.delete(fn),
  });
  t.after(() => {
    for (const name of ["history", "addEventListener", "removeEventListener"]) delete (globalThis as Record<string, unknown>)[name];
    place.pathname = "/sessions";
    place.search = "";
    storage.values.clear();
  });
  storage.values.clear();
  /** The browser's Back (or Forward) to `path`: the address moves, then the page hears of it. */
  const back = (path: string) => {
    place.pathname = path;
    for (const fn of [...(listeners.get("popstate") ?? [])]) fn();
  };
  return { place, back };
}

test("「我的设备」 is /devices: the app's address is kept to go back to, and opening another machine keeps only its section", (t) => {
  const { place } = addressBar(t);
  // A deep link, nothing in view before: 「我的设备」, the link kept.
  place.pathname = "/sessions/claude/s-1";
  const first = world([{ id: "m-a" }, { id: "m-b" }]);
  first.shell.start();
  let firstStopped = false;
  t.after(() => {
    if (!firstStopped) first.shell.stop();
  });
  assert.equal(first.shell.focus.get(), null);
  assert.equal(place.pathname, "/devices");
  assert.equal(first.shell.lastShown, null, "no way back yet");
  first.shell.select("m-a");
  assert.equal(place.pathname, "/sessions/claude/s-1", "the link opens on the machine picked");
  // Back to 「我的设备」 and in again: where the app was.
  place.pathname = "/terminals";
  first.shell.select(null);
  assert.equal(place.pathname, "/devices");
  assert.equal(first.shell.lastShown, "m-a", "the way back");
  first.shell.select("m-a");
  assert.equal(place.pathname, "/terminals");
  // From a session of one machine to another machine: its page of the section, not the session.
  place.pathname = "/sessions/claude/s-1";
  first.shell.select(null);
  first.shell.select("m-b");
  assert.equal(place.pathname, "/m/m-b/sessions");
  assert.equal(homeMachine(), "m-a", "the main device stays the first in the chain");
  // In the app, bringing another machine into view moves the address to its page of the section.
  place.pathname = "/m/m-b/files";
  first.shell.select("m-a");
  assert.equal(place.pathname, "/files");
  first.shell.select("m-b");
  assert.equal(place.pathname, "/m/m-b/files");
  first.shell.stop();
  firstStopped = true;
  assert.equal(selectedMachine("a-1"), "m-b", "the device last in view, remembered");
  assert.equal(mainMachine("a-1"), "m-a", "and the main device, apart from it");
  // `/devices` opens on 「我的设备」 even with a machine remembered from last time.
  place.pathname = "/devices";
  const second = world([{ id: "m-a" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  second.shell.start();
  assert.equal(mainMachine("a-1"), "m-a", "the main device, remembered by this browser");
  t.after(() => second.shell.stop());
  assert.equal(second.shell.focus.get(), null);
  assert.equal(place.pathname, "/devices");
  second.shell.select("m-a");
  assert.equal(place.pathname, "/sessions", "nothing to go back to: the app's start");
});

test("the account's settings are /account, a page of the shell like 「我的设备」: no device in view, its address kept; Back and Forward between the two keep it so", (t) => {
  const { place, back } = addressBar(t);
  // Opened at /account (a reload, a link), with a device remembered: the page, not the device.
  place.pathname = "/account";
  const { shell } = world([{ id: "m-a" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  shell.start();
  t.after(() => shell.stop());
  assert.equal(shell.focus.get(), null);
  assert.equal(place.pathname, "/account", "kept: not moved to /devices");
  // Back to 「我的设备」 and Forward to the account: still no device in view, neither address moved.
  back("/devices");
  assert.equal(shell.focus.get(), null);
  assert.equal(place.pathname, "/devices");
  back("/account");
  assert.equal(shell.focus.get(), null);
  assert.equal(place.pathname, "/account");
  // A device opened from the account's page: the app's start (nothing to go back to).
  shell.select("m-a");
  assert.equal(place.pathname, "/sessions");
  // Back to /account from the app: off the device, onto the page.
  back("/account");
  assert.equal(shell.focus.get(), null);
  assert.equal(place.pathname, "/account");
});

test("Back on 「我的设备」 goes into the app on the device it last showed, Forward comes back to it; an address moved under it gives another device only its section", (t) => {
  const { place, back } = addressBar(t);
  const { shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  shell.start();
  t.after(() => shell.stop());
  // Nothing shown yet: Back to an app page keeps 「我的设备」 at its address.
  back("/sessions/claude/s-0");
  assert.equal(shell.focus.get(), null);
  assert.equal(place.pathname, "/devices");
  // On m-a's session, then 「我的设备」, then Back: the app again, on m-a, where Back went.
  shell.select("m-a");
  place.pathname = "/sessions/claude/s-a";
  shell.select(null);
  back("/sessions/claude/s-a2");
  assert.equal(shell.focus.get(), "m-a", "Back is into the app");
  assert.equal(place.pathname, "/sessions/claude/s-a2", "at the address Back went to");
  // Forward again: `/devices` with m-a in view is 「我的设备」, not the app's Not Found;
  // opening m-a from there comes back to where Back had taken the app.
  back("/devices");
  assert.equal(shell.focus.get(), null, "Forward onto /devices shows 「我的设备」");
  assert.equal(place.pathname, "/devices");
  shell.select("m-a");
  assert.equal(place.pathname, "/sessions/claude/s-a2");
  // An address moved under 「我的设备」 some other way (no popstate heard): opening
  // another device does not take m-a's session with it, only its section.
  shell.select(null);
  place.pathname = "/sessions/claude/s-a3";
  shell.select("m-b");
  assert.equal(place.pathname, "/m/m-b/sessions");
  // Opening a session from the dashboard sets the address itself: on its device's own address.
  shell.select(null);
  shell.openAt("m-a", "/sessions/codex/x-1");
  assert.equal(shell.focus.get(), "m-a");
  assert.equal(place.pathname, "/sessions/codex/x-1", "the main device's: plain");
  shell.select(null);
  shell.openAt("m-b", "/sessions/codex/x-2");
  assert.equal(shell.focus.get(), "m-b");
  assert.equal(place.pathname, "/m/m-b/sessions/codex/x-2");
  // Back and Forward across all three: off 「我的设备」 onto another device's
  // page, onto the main device's plain page, onto 「我的设备」 again.
  shell.select(null);
  assert.equal(place.pathname, "/devices");
  back("/m/m-b/terminals");
  assert.equal(shell.focus.get(), "m-b", "Back onto another device's page: that device");
  assert.equal(place.pathname, "/m/m-b/terminals");
  back("/devices");
  assert.equal(shell.focus.get(), null);
  back("/files");
  assert.equal(shell.focus.get(), "m-a", "Back onto a plain address: the main device");
  assert.equal(place.pathname, "/files");
  // Opening a device from 「我的设备」 lands on its own address, the section kept.
  back("/devices");
  shell.select("m-b");
  assert.equal(place.pathname, "/m/m-b/files");
});

test("a device something holds (a pane on screen) is live and never let go; let go of by everything and out of view, it idles out", async (t) => {
  const { link, shell } = world([{ id: "m-a" }, { id: "m-b" }, { id: "m-c" }]);
  Object.assign(shell.opts, { idleMs: 30 });
  for (const key of ["m-a", "m-b", "m-c"]) link.set(key, "online");
  shell.start();
  t.after(() => shell.stop());
  const a = dialer(shell, "m-a");
  const b = dialer(shell, "m-b");
  const c = dialer(shell, "m-c");
  shell.select("m-a");
  await until(() => shell.machine("m-a")!.transport.current === "live");
  // A pane of the screen shows m-b's terminal: m-b is in use, dialled; m-c is not.
  const release = runtimeFor("m-b").hold();
  await until(() => shell.machine("m-b")!.transport.current === "live");
  assert.equal(shell.machine("m-b")!.holders, 1);
  const follows = (key: string) => (shell.machine(key)!.mirror as unknown as { followingSessions: boolean }).followingSessions;
  assert.equal(follows("m-b"), true, "a held device's sessions are followed, as the one in view's");
  await flush(100);
  assert.equal(c.ensures, 0, "out of view and held by nothing: never dialled");
  // Long past the idle time, still held: still live.
  await flush(200);
  assert.equal(shell.machine("m-b")!.channel.status, "open");
  assert.equal(b.ensures, 1);
  // The device in view moves to m-c (the router went to its page), and m-a goes
  // out of view; m-b stays held, so stays.
  const place = globalThis.location as unknown as { pathname: string };
  place.pathname = "/m/m-c/sessions";
  t.after(() => {
    place.pathname = "/sessions";
  });
  shell.follow("m-c");
  assert.equal(shell.focus.get(), "m-c");
  await until(() => shell.machine("m-a")!.channel.status === "idle", 2000);
  assert.equal(shell.machine("m-b")!.channel.status, "open", "never released while a pane holds it");
  // The pane goes: m-b idles out like any device out of view.
  release();
  release();
  assert.equal(shell.machine("m-b")!.holders, 0, "a release counts once");
  assert.equal(follows("m-b"), false);
  await until(() => shell.machine("m-b")!.channel.status === "idle", 2000);
  assert.equal(shell.machine("m-b")!.transport.current, "offline");
  assert.ok(a.ensures >= 1 && c.ensures === 1);
});

test("the device in view follows the app's address, and the main device does not", (t) => {
  const { place } = addressBar(t);
  const { shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  place.pathname = "/m/m-b/terminals";
  shell.start();
  t.after(() => shell.stop());
  assert.equal(homeMachine(), "m-a", "the main device remembered");
  assert.equal(shell.focus.get(), "m-b", "a deep link to another device's page: that device in view");
  assert.equal(place.pathname, "/m/m-b/terminals", "the link kept");
  // The router moved (a link, the bar's switch, Back): the shell follows.
  place.pathname = "/sessions";
  shell.follow("m-a");
  assert.equal(shell.focus.get(), "m-a");
  assert.equal(homeMachine(), "m-a");
  shell.follow("m-gone");
  assert.equal(shell.focus.get(), "m-a", "a device the shell does not have: nothing");
  shell.select(null);
  shell.follow("m-b");
  assert.equal(shell.focus.get(), null, "「我的设备」: the app is not on the page, nothing to follow");
});

test("setting the main device moves the home, keeps the page in view, and is remembered", (t) => {
  const { place } = addressBar(t);
  const { shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  place.pathname = "/terminals";
  place.search = "?id=t-1";
  shell.start();
  t.after(() => shell.stop());
  assert.equal(shell.focus.get(), "m-a");
  // On m-a's terminal, m-b becomes the main device: the page stays m-a's, under its own address now.
  shell.setMain("m-b");
  assert.equal(homeMachine(), "m-b");
  assert.equal(mainMachine("a-1"), "m-b");
  assert.equal(`${place.pathname}${place.search}`, "/m/m-a/terminals?id=t-1");
  // On m-b's page under /m/, m-b's own: it becomes plain.
  place.pathname = "/m/m-a/files";
  place.search = "";
  shell.setMain("m-a");
  assert.equal(place.pathname, "/m/m-a/files".replace("/m/m-a", ""), "the new main device's page is plain");
  // A screen is the old main device's alone: the new one's sessions.
  place.pathname = "/screens/s-1";
  shell.setMain("m-b");
  assert.equal(place.pathname, "/sessions");
  shell.setMain("m-unknown");
  assert.equal(homeMachine(), "m-b", "not a device of the account: nothing");
});

test("the main device leaving the account: the device in view becomes the main device, else the chain's first left", (t) => {
  const { place } = addressBar(t);
  const { account, shell } = world([{ id: "m-a" }, { id: "m-b" }, { id: "m-c" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  place.pathname = "/m/m-c/terminals";
  shell.start();
  t.after(() => shell.stop());
  assert.equal(homeMachine(), "m-a");
  assert.equal(shell.focus.get(), "m-c");
  account.hosts.splice(0, 1);
  account.changed();
  assert.equal(homeMachine(), "m-c", "the device in view, not the first held");
  assert.equal(mainMachine("a-1"), "m-c");
  assert.equal(place.pathname, "/terminals", "its page, now the main device's, is plain");
  // In 「我的设备」 (nothing in view): the chain's first.
  shell.select(null);
  account.hosts.splice(account.hosts.findIndex((h) => h.id === "m-c"), 1);
  account.changed();
  assert.equal(homeMachine(), "m-b");
});

test("an account with no device that runs AI when the page starts: its first one becomes the main device", (t) => {
  addressBar(t);
  const { account, shell } = world([]);
  shell.start();
  t.after(() => shell.stop());
  assert.equal(mainMachine("a-1"), null);
  account.hosts.push({ id: "m-new" });
  account.changed();
  assert.equal(mainMachine("a-1"), "m-new");
  assert.equal(homeMachine(), "m-new");
});

test("a main device chosen on a page whose storage keeps nothing stays chosen as the chain changes", (t) => {
  addressBar(t);
  const { account, shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  shell.start();
  t.after(() => shell.stop());
  const real = globalThis.localStorage;
  const refusing = {
    getItem: () => {
      throw new Error("storage refused");
    },
    setItem: () => {
      throw new Error("storage refused");
    },
    removeItem: () => {
      throw new Error("storage refused");
    },
  };
  Object.assign(globalThis, { localStorage: refusing });
  t.after(() => Object.assign(globalThis, { localStorage: real }));
  shell.setMain("m-b");
  assert.equal(homeMachine(), "m-b");
  account.hosts.push({ id: "m-c" });
  account.changed();
  assert.equal(homeMachine(), "m-b", "not reset to the chain's first");
});

// The main device is the account's (`GET`/`PUT /v1/main-device`, the link's
// `main_device`): every browser and phone of it shows the same screens. This
// browser keeps a copy (`rh.hub.home.<account>`) for before the hub answers,
// or when it cannot.

/** How many times the home (the main device) changed: `app/root-layout.tsx` starts over once for each. */
function homeChanges(t: { after(fn: () => void): void }) {
  const seen: string[] = [];
  const off = homeMachineStore().subscribe(() => void seen.push(homeMachine()));
  t.after(off);
  return seen;
}

test("reading: the account's main device from the hub replaces this browser's copy, which keeps it; the page in view stays", async (t) => {
  const { place } = addressBar(t);
  const { link, shell, api } = world([{ id: "m-a" }, { id: "m-b" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  place.pathname = "/terminals";
  shell.start();
  t.after(() => shell.stop());
  assert.equal(homeMachine(), "m-a", "before the hub answers: this browser's copy");
  assert.equal(shell.focus.get(), "m-a");
  const changes = homeChanges(t);
  // Signed in on the link: the hub says the account's is m-b.
  link.control({ t: "main_device", machine: "m-b", set_ms: 5 });
  assert.equal(homeMachine(), "m-b");
  assert.equal(mainMachine("a-1"), "m-b", "kept as this browser's copy for the next load");
  assert.deepEqual(changes, ["m-b"], "the tree starts over once");
  assert.equal(place.pathname, "/m/m-a/terminals", "the page in view stays m-a's, under its own address now");
  assert.equal(shell.focus.get(), "m-a");
  await flush();
  assert.deepEqual(api.writes, [], "the hub's word is not written back");
  // Messages that are not about it, or are not one, change nothing.
  link.control({ t: "plan", plan: "free" });
  link.control({ t: "main_device", machine: 7 });
  assert.equal(homeMachine(), "m-b");
});

test("a change from another device: followed at once; the same again, or an older one, changes nothing", async (t) => {
  const { place } = addressBar(t);
  const { link, shell } = world([{ id: "m-a" }, { id: "m-b" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  place.pathname = "/files";
  shell.start();
  t.after(() => shell.stop());
  link.control({ t: "main_device", machine: "m-a", set_ms: 5 });
  const changes = homeChanges(t);
  assert.deepEqual(changes, [], "the hub agrees with this browser's copy: nothing starts over");
  link.control({ t: "main_device", machine: "m-b", set_ms: 9 });
  assert.deepEqual(changes, ["m-b"]);
  assert.equal(place.pathname, "/m/m-a/files", "the page kept");
  link.control({ t: "main_device", machine: "m-b", set_ms: 9 });
  assert.deepEqual(changes, ["m-b"], "the same again: no new tree");
  // A snapshot read before a change but sent after it (`set_ms` older): not taken.
  link.control({ t: "main_device", machine: "m-a", set_ms: 7 });
  assert.equal(homeMachine(), "m-b");
  assert.deepEqual(changes, ["m-b"]);
  link.control({ t: "main_device", machine: "m-a", set_ms: 12 });
  assert.deepEqual(changes, ["m-b", "m-a"]);
  assert.equal(place.pathname, "/files", "back to plain: m-a is the main device again");
});

test("a main device this page's chain does not have yet waits for the chain; the hub's word alone opens nothing", (t) => {
  addressBar(t);
  const { link, account, shell } = world([{ id: "m-a" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  shell.start();
  t.after(() => shell.stop());
  link.control({ t: "main_device", machine: "m-new", set_ms: 5 });
  assert.equal(homeMachine(), "m-a");
  assert.equal(isOpen("m-new"), false);
  account.hosts.push({ id: "m-new" });
  account.changed();
  assert.equal(homeMachine(), "m-new", "once the verified chain has it");
});

test("seeding: the hub has none — this browser's main device is claimed if unset, and the one that stands is followed", async (t) => {
  addressBar(t);
  const { link, shell, api } = world([{ id: "m-a" }, { id: "m-b" }, { id: "m-c" }]);
  storage.setItem("rh.hub.home.a-1", "m-b");
  shell.start();
  t.after(() => shell.stop());
  // Another browser was first: the hub answers its main device, not this one's.
  api.answer = async () => ({ machine: "m-c", setMs: 20 });
  link.control({ t: "main_device", machine: null, set_ms: null });
  assert.deepEqual(api.writes, [{ machine: "m-b", ifUnset: true }], "only if the account still has none");
  await until(() => homeMachine() === "m-c");
  assert.equal(mainMachine("a-1"), "m-c");
});

test("seeding: a browser that remembers nothing does not decide for the others", async (t) => {
  addressBar(t);
  const { link, shell, api } = world([{ id: "m-a" }, { id: "m-b" }]);
  shell.start();
  t.after(() => shell.stop());
  assert.equal(homeMachine(), "m-a", "its default: the chain's first");
  link.control({ t: "main_device", machine: null, set_ms: null });
  await flush();
  assert.deepEqual(api.writes, []);
  // Another browser seeds it: followed.
  link.control({ t: "main_device", machine: "m-b", set_ms: 3 });
  assert.equal(homeMachine(), "m-b");
});

test("「设为主设备」 is written for the account; a word from the hub arriving meanwhile does not undo it", async (t) => {
  addressBar(t);
  const { link, shell, api } = world([{ id: "m-a" }, { id: "m-b" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  shell.start();
  t.after(() => shell.stop());
  link.control({ t: "main_device", machine: "m-a", set_ms: 5 });
  let answer!: (main: AccountMain) => void;
  api.answer = () => new Promise((resolve) => (answer = resolve));
  shell.setMain("m-b");
  assert.equal(homeMachine(), "m-b", "at once on this page");
  assert.deepEqual(api.writes, [{ machine: "m-b", ifUnset: false }], "for the whole account");
  // The link reconnects meanwhile: its snapshot (the old one) is not taken over the choice on its way.
  link.control({ t: "main_device", machine: "m-a", set_ms: 6 });
  assert.equal(homeMachine(), "m-b");
  answer({ machine: "m-b", setMs: 8 });
  await flush();
  assert.equal(homeMachine(), "m-b");
  // From then on, other devices' changes are followed again.
  link.control({ t: "main_device", machine: "m-a", set_ms: 9 });
  assert.equal(homeMachine(), "m-a");
});

test("fallback: a choice made offline stays this browser's and reaches the hub when the link opens; an older hub leaves it per browser", async (t) => {
  const warned = t.mock.method(console, "warn", () => undefined);
  addressBar(t);
  const { link, shell, api } = world([{ id: "m-a" }, { id: "m-b" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  shell.start();
  t.after(() => shell.stop());
  // Offline: the request fails.
  api.answer = async () => {
    throw new TypeError("Failed to fetch");
  };
  shell.setMain("m-b");
  await flush();
  assert.equal(homeMachine(), "m-b");
  assert.equal(mainMachine("a-1"), "m-b", "this browser's copy");
  // Online again: the snapshot (still the old one) is not taken; the choice is sent again.
  api.answer = async (machine) => ({ machine, setMs: 30 });
  link.control({ t: "main_device", machine: "m-a", set_ms: 10 });
  link.opened();
  await flush();
  assert.deepEqual(
    api.writes.map((w) => w.machine),
    ["m-b", "m-b"],
  );
  assert.equal(homeMachine(), "m-b");
  link.control({ t: "main_device", machine: "m-b", set_ms: 30 });
  link.opened();
  await flush();
  assert.equal(api.writes.length, 2, "taken: not sent again");
  // A hub from before the route: refused (404). The choice stays this browser's; nothing is retried.
  api.answer = async () => {
    throw new HubApiError(404, "error", "Not Found");
  };
  shell.setMain("m-a");
  await flush();
  assert.equal(homeMachine(), "m-a");
  link.opened();
  await flush();
  assert.equal(api.writes.length, 3);
  assert.equal(warned.mock.callCount(), 2, "each failure said in the console");
});

test("the main device leaving the account: the page's pick is claimed if unset, and the account's answer is followed", async (t) => {
  const { place } = addressBar(t);
  const { link, account, shell, api } = world([{ id: "m-a" }, { id: "m-b" }, { id: "m-c" }]);
  storage.setItem("rh.hub.home.a-1", "m-a");
  place.pathname = "/m/m-c/terminals";
  shell.start();
  t.after(() => shell.stop());
  link.control({ t: "main_device", machine: "m-a", set_ms: 5 });
  // Another page repointed first, to m-b.
  api.answer = async () => ({ machine: "m-b", setMs: 7 });
  account.hosts.splice(0, 1);
  account.changed();
  assert.equal(homeMachine(), "m-c", "at once: the device in view");
  assert.deepEqual(api.writes, [{ machine: "m-c", ifUnset: true }]);
  // The hub cleared the one that left (said of itself), then the claim's answer.
  link.control({ t: "main_device", machine: null, set_ms: 6 });
  await until(() => homeMachine() === "m-b");
  assert.equal(place.pathname, "/m/m-c/terminals", "the page in view stays m-c's");
});

test("a choice the hub could not take now is sent again a little later, without waiting for the link to reconnect", async (t) => {
  addressBar(t);
  t.mock.method(console, "warn", () => undefined);
  const { shell, api } = world([{ id: "m-a" }, { id: "m-b" }]);
  Object.assign(shell.opts, { mainRetryMs: 20 });
  storage.setItem("rh.hub.home.a-1", "m-a");
  shell.start();
  t.after(() => shell.stop());
  // The hub's database is away for a moment (503), then back.
  let down = 2;
  api.answer = async (machine) => {
    if (down-- > 0) throw new HubApiError(503, "unavailable", "the database is unavailable; try again");
    return { machine, setMs: 40 };
  };
  shell.setMain("m-b");
  await until(() => api.writes.length === 3);
  await flush(80);
  assert.equal(api.writes.length, 3, "taken on the third try: no more");
  assert.deepEqual(new Set(api.writes.map((w) => `${w.machine}/${w.ifUnset}`)), new Set(["m-b/false"]));
  assert.equal(homeMachine(), "m-b");
  // Stopped (signed out) with a try waiting: it is not sent.
  api.answer = async () => {
    throw new TypeError("Failed to fetch");
  };
  shell.setMain("m-a");
  await flush(5);
  assert.equal(api.writes.length, 4);
  shell.stop();
  await flush(80);
  assert.equal(api.writes.length, 4);
});

test("leaving the account ends the shell, its slot and the renewal runner together (a fork included)", () => {
  const stopped: string[] = [];
  setHubShell({ kind: "hub", stop: () => void stopped.push("shell") } as never);
  setRenewalRunner({ stop: () => void stopped.push("renewals") } as never);
  try {
    endHubShell();
    assert.deepEqual(stopped, ["shell", "renewals"]);
    assert.equal(hubShell(), null);
    assert.equal(renewalRunner(), null);
  } finally {
    setHubShell(null);
    setRenewalRunner(null);
  }
});
