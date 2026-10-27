// Several devices on one page (`docs/local-aggregator.md` §6.5, §6.7 M-2):
// addresses that name a device, pane content that names one, a list feed
// shared by every view of a device, devices held while something uses them,
// and the device a new session defaults to.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { GlobalFeedProvider, machineFeedFor, useGlobalFeedSelector, type GlobalFeedState } from "../src/app/global-feed.js";
import { MachineView } from "../src/app/machine-route.js";
import { paneMachine, reducePaneTree, treeMachines, usesMachine, withMachine, type PaneContent, type PaneNode } from "../src/app/pane-tree.js";
import { canStartOn, defaultDevice, deviceNote } from "../src/features/machines/device-picker.js";
import { MachineProvider } from "../src/lib/machines/context.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { closeMachine, isOpen, openMachine, openMachines, openMachinesStore, runtimeFor, setHome, setShell } from "../src/lib/machines/registry.js";
import { machineOfPath, machinePath, machinePrefix, machineTarget, plainHomeAddress, splitMachinePath } from "../src/lib/machines/routes.js";
import type { MachineInfo } from "../src/lib/machines/runtime.js";
import { constantStore } from "../src/lib/machines/store.js";
import type { SocketLike, Transport } from "../src/lib/transport/index.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

class FakeSocket implements SocketLike {
  readyState = 0;
  binaryType = "blob";
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  constructor(
    readonly url: string,
    private readonly log: string[],
  ) {}
  send() {}
  close() {
    this.readyState = 3;
    this.log.push(`closed ${this.url}`);
  }
}

function recording(name: string, log: string[]): Transport {
  return {
    fetch: async () => new Response("{}", { status: 503, headers: { "content-type": "application/json" } }),
    socketUrl: async (path) => `${name}:${path}`,
    openSocket: (url) => {
      log.push(`${name} socket ${url}`);
      return new FakeSocket(url, log);
    },
  };
}

function info(key: string, over: Partial<MachineInfo> = {}): MachineInfo {
  return { key, name: key, state: "online", since: 0, canManage: true, live: "copy", ...over };
}

test("an address names its device under /m/, and the home's addresses stay as they were", () => {
  assert.equal(machinePrefix("d-7f3a"), "/m/d-7f3a");
  assert.equal(machinePath("d-b", "/sessions/claude/abc", "d-a"), "/m/d-b/sessions/claude/abc");
  assert.equal(machinePath("d-a", "/terminals?id=1", "d-a"), "/terminals?id=1", "the home's page keeps its plain address");
  assert.equal(machinePath(LOCAL, "/files", LOCAL), "/files", "the local page never writes /m/");
  assert.equal(machinePath("odd id/x", "/files", "d-a"), "/m/odd%20id%2Fx/files");

  assert.deepEqual(splitMachinePath("/m/d-b/sessions/claude/abc"), { machine: "d-b", path: "/sessions/claude/abc" });
  assert.deepEqual(splitMachinePath("/m/odd%20id%2Fx/files"), { machine: "odd id/x", path: "/files" });
  assert.deepEqual(splitMachinePath("/m/d-b"), { machine: "d-b", path: "/sessions" }, "a bare device address is its sessions");
  assert.deepEqual(splitMachinePath("/m/d-b/"), { machine: "d-b", path: "/sessions" });
  assert.deepEqual(splitMachinePath("/sessions/claude/abc"), { machine: null, path: "/sessions/claude/abc" });
  assert.deepEqual(splitMachinePath("/media/x"), { machine: null, path: "/media/x" }, "only the segment `m` names a device");
  assert.deepEqual(splitMachinePath("/m/%E0%A4%A/x"), { machine: null, path: "/m/%E0%A4%A/x" }, "a broken escape names nothing");

  assert.equal(machineOfPath("/m/d-b/terminals", "d-a"), "d-b");
  assert.equal(machineOfPath("/terminals", "d-a"), "d-a");

  assert.deepEqual(machineTarget("d-a", "/sessions/$source/$id", { source: "claude", id: "x" }, "d-a"), {
    to: "/sessions/$source/$id",
    params: { source: "claude", id: "x" },
  });
  assert.deepEqual(machineTarget("d-b", "/sessions/$source/$id", { source: "claude", id: "x" }, "d-a"), {
    to: "/m/$machine/sessions/$source/$id",
    params: { source: "claude", id: "x", machine: "d-b" },
  });
  assert.deepEqual(machineTarget("d-b", "/terminals", undefined, "d-a"), { to: "/m/$machine/terminals", params: { machine: "d-b" } });
  assert.deepEqual(machineTarget("d-a", "/terminals", undefined, "d-a"), { to: "/terminals" });

  assert.equal(plainHomeAddress("/m/d-a/terminals", "?id=1", "d-a"), "/terminals?id=1", "the home under /m/ goes to its plain address");
  assert.equal(plainHomeAddress("/m/d-b/terminals", "", "d-a"), null);
  assert.equal(plainHomeAddress("/terminals", "", "d-a"), null);
});

test("the router answers every section of another device under /m/$machine", async () => {
  const { readFileSync } = await import("node:fs");
  const router = readFileSync(new URL("../src/app/router.tsx", import.meta.url), "utf8");
  assert.match(router, /path: "m\/\$machine"/);
  for (const section of ["sessions", "chat", "terminals", "files", "settings", "stats"]) {
    assert.match(router, new RegExp(`getParentRoute: \\(\\) => machineRoute,\\s*path: "${section}"`), section);
  }
  // That /m is one of the reserved top-level paths, which no plugin alias may
  // take, is checked against the server's list in plugin-reserved-paths.test.ts.
});

test("pane content names its device only off the home, and content from before reads as the home's", () => {
  const home = "d-a";
  const old: PaneContent = { kind: "session", source: "claude", id: "s1" };
  assert.equal(paneMachine(old, home), home, "a layout from before machines had keys is the home's");
  assert.deepEqual(withMachine(old, home, home), old, "on the home nothing is added");
  assert.deepEqual(withMachine(old, "d-b", home), { ...old, machine: "d-b" });
  assert.deepEqual(withMachine({ ...old, machine: "d-b" }, home, home), old, "moved to the home, the name goes");
  assert.equal(paneMachine({ kind: "terminal", id: "t1", machine: "d-c" }, home), "d-c");
  assert.equal(paneMachine({ kind: "files", path: "/x", machine: "" }, home), home, "an empty name is no name");
  const plugin: PaneContent = { kind: "plugin", plugin: "tasks", pane: "checklist" };
  assert.equal(withMachine(plugin, "d-b", home), plugin, "a plugin's pane is the home's wherever it is");
  assert.equal(paneMachine(plugin, home), home);
  assert.equal(usesMachine({ kind: "picker", machine: "d-b" }), false, "a picker uses nothing yet");
  assert.equal(usesMachine({ kind: "terminal", id: "t" }), true);

  // A three-device screen survives the server's JSON round trip.
  const root: PaneNode = {
    type: "split",
    id: "s1",
    direction: "row",
    ratio: 0.5,
    children: [
      { type: "leaf", id: "p1", content: withMachine({ kind: "session", source: "claude", id: "a1" }, home, home) },
      {
        type: "split",
        id: "s2",
        direction: "column",
        ratio: 0.5,
        children: [
          { type: "leaf", id: "p2", content: withMachine({ kind: "terminal", id: "t1" }, "d-b", home) },
          { type: "leaf", id: "p3", content: withMachine({ kind: "session", source: "codex", id: "c1" }, "d-c", home) },
        ],
      },
    ],
  };
  const back = JSON.parse(JSON.stringify(root)) as PaneNode;
  assert.deepEqual(back, root);
  assert.deepEqual([...treeMachines(back, home)].sort(), ["d-a", "d-b", "d-c"]);
  assert.equal(JSON.stringify(root).includes('"machine":"d-a"'), false, "the home is never written");

  // One device only: nothing in it says which, and the tree is not mixed.
  const single: PaneNode = { type: "split", id: "s", direction: "row", ratio: 0.5, children: [{ type: "leaf", id: "m", content: { kind: "main" } }, { type: "leaf", id: "x", content: old }] };
  assert.equal(treeMachines(single, home).size, 1);
  assert.equal(treeMachines(single, home, "d-b").size, 2, "the routed view's device counts for the main leaf");

  // The reducer keeps what it is handed: a pane moved to another device is that device's.
  const state = { root: back, focusedId: "p2" };
  const moved = reducePaneTree(state, { type: "content", id: "p1", content: withMachine({ kind: "files" }, "d-b", home) });
  assert.deepEqual((moved.root as Extract<PaneNode, { type: "split" }>).children[0], { type: "leaf", id: "p1", content: { kind: "files", machine: "d-b" } });
});

test("the open devices are a store, re-read as devices open, close and the home moves", async (t) => {
  const log: string[] = [];
  const heard: (readonly string[])[] = [];
  const off = openMachinesStore().subscribe(() => heard.push(openMachinesStore().get()));
  t.after(() => {
    off();
    setShell("local", LOCAL);
    for (const m of ["m-p", "m-q"]) if (isOpen(m)) closeMachine(m);
  });
  openMachine("m-p", recording("p", log), { info: constantStore(info("m-p")), ensureLive: () => undefined, dispose: () => undefined });
  openMachine("m-q", recording("q", log), { info: constantStore(info("m-q")), ensureLive: () => undefined, dispose: () => undefined });
  setShell("hub", "m-p");
  assert.deepEqual(openMachinesStore().get(), openMachines());
  assert.deepEqual(openMachinesStore().get(), ["m-p", "m-q"]);
  setHome("m-q");
  assert.deepEqual(openMachinesStore().get(), ["m-q", "m-p"], "home first");
  closeMachine("m-p");
  assert.deepEqual(openMachinesStore().get(), ["m-q"]);
  assert.ok(heard.length >= 3);
});

test("a device is held while something uses it: counted where the shell counts, ensureLive where it does not", async (t) => {
  const log: string[] = [];
  let holds = 0;
  t.after(() => {
    setShell("local", LOCAL);
    for (const m of ["m-h", "m-l"]) if (isOpen(m)) closeMachine(m);
  });
  openMachine("m-h", recording("h", log), {
    info: constantStore(info("m-h")),
    ensureLive: () => void log.push("live h"),
    hold: () => {
      holds += 1;
      log.push(`hold h ${holds}`);
      return () => {
        holds -= 1;
        log.push(`release h ${holds}`);
      };
    },
    dispose: () => undefined,
  });
  openMachine("m-l", recording("l", log), { info: constantStore(info("m-l")), ensureLive: () => void log.push("live l"), dispose: () => undefined });
  const one = runtimeFor("m-h").hold();
  const two = runtimeFor("m-h").hold();
  one();
  one();
  assert.equal(holds, 1, "a release counts once, however often it is called");
  two();
  assert.equal(holds, 0);
  const plain = runtimeFor("m-l").hold();
  plain();
  assert.deepEqual(log, ["hold h 1", "hold h 2", "release h 1", "release h 0", "live l"]);
  // The local page's own server is always live: holding it does nothing.
  runtimeFor(LOCAL).hold()();
});

test("a device's list runs once for every view of it, and stops when the last one goes", async (t) => {
  const log: string[] = [];
  const listeners: string[] = [];
  const target = { addEventListener: (name: string) => listeners.push(`+${name}`), removeEventListener: (name: string) => listeners.push(`-${name}`), visibilityState: "visible", querySelector: () => null };
  const g = globalThis as Record<string, unknown>;
  const hadWindow = "window" in g;
  const hadDocument = "document" in g;
  if (!hadWindow) g.window = target;
  if (!hadDocument) g.document = target;
  t.after(() => {
    if (!hadWindow) delete g.window;
    if (!hadDocument) delete g.document;
    setShell("local", LOCAL);
    for (const m of ["m-f", "m-g"]) if (isOpen(m)) closeMachine(m);
  });
  openMachine("m-f", recording("f", log));
  openMachine("m-g", recording("g", log));
  const feed = machineFeedFor("m-f");
  assert.equal(machineFeedFor("m-f"), feed, "one per device");
  assert.notEqual(machineFeedFor("m-g").store, feed.store, "another device's rows are another store");

  const root = feed.acquire();
  const pane = feed.acquire();
  await flush();
  assert.deepEqual(log.filter((l) => l.includes("socket")), ["f socket f:/api/events?v=2&z=1"], "one socket for both");
  assert.equal(feed.holders, 2);
  root();
  root();
  await flush();
  assert.equal(log.some((l) => l.startsWith("closed")), false, "still shown by the pane");
  feed.store.publish({ connected: true });
  pane();
  assert.equal(feed.holders, 0);
  assert.ok(log.some((l) => l.startsWith("closed f:")), "the last view gone, the socket goes");
  assert.equal(feed.store.getSnapshot().connected, false, "and the store says so");

  // Shown again, the list starts again (a feed waiting on a stopped list goes with it).
  const again = feed.acquire();
  await flush();
  assert.equal(log.filter((l) => l.startsWith("f socket")).length, 2);
  again();
  closeMachine("m-f");
  assert.notEqual(machineFeedFor("m-f"), feed, "closing the device drops its list");
});

const selectMachine = (state: GlobalFeedState) => state.machine;
function WhoseFeed() {
  return <b>{useGlobalFeedSelector(selectMachine)}</b>;
}

test("a view of another device inside the home's tree reads that device's feed", () => {
  const html = renderToStaticMarkup(
    <MachineProvider machine="m-a">
      <GlobalFeedProvider>
        <WhoseFeed />
        <MachineView machine="m-b">
          <WhoseFeed />
        </MachineView>
        <MachineView machine="m-a">
          <WhoseFeed />
        </MachineView>
      </GlobalFeedProvider>
    </MachineProvider>,
  );
  assert.equal(html, "<b>m-a</b><b>m-b</b><b>m-a</b>");
});

test("a new session starts on the view's device, else the last one used, else the home — one that can take it", () => {
  const open = ["d-a", "d-b", "d-c"];
  const up = (m: string) => m !== "d-c";
  assert.equal(defaultDevice({ open, here: "d-b", last: "d-a", home: "d-a", usable: up }), "d-b");
  assert.equal(defaultDevice({ open, here: "d-c", last: "d-b", home: "d-a", usable: up }), "d-b", "the view's device is offline: the last used");
  assert.equal(defaultDevice({ open, here: "d-c", last: "d-gone", home: "d-a", usable: up }), "d-a", "the last one left the account: the home");
  assert.equal(defaultDevice({ open, here: null, last: null, home: "d-c", usable: up }), "d-a", "the home is offline: the first that is up");
  assert.equal(defaultDevice({ open, here: null, last: null, home: "d-c", usable: () => false }), "d-c", "nothing is up: the home, greyed");
  assert.equal(defaultDevice({ open: [LOCAL], here: LOCAL, home: LOCAL }), LOCAL);

  assert.equal(canStartOn(info("d", { state: "offline" })), false);
  assert.equal(canStartOn(info("d", { live: "stale" })), false);
  assert.equal(canStartOn(info("d", { live: "refused" })), false, "nothing new on a device that does not accept this browser");
  assert.equal(canStartOn(info("d", { state: "unknown" })), true, "unknown is tried: the hub may not have said yet");
  const t = (key: string) => key;
  assert.equal(deviceNote(info("d", { live: "live" }), t), null, "live says nothing");
  assert.equal(deviceNote(info("d", { state: "offline" }), t), "offline");
  assert.equal(deviceNote(info("d", { state: "offline" }), t, { forStarting: true }), "offline — a new session needs it online");
  assert.equal(deviceNote(info("d", { state: "offline", copyAsOf: 1 }), t), "offline copy · as of {{time}}");
  assert.equal(deviceNote(info("d", { live: "stale" }), t), "waiting for another of your devices to confirm");
  assert.equal(deviceNote(info("d", { live: "refused" }), t), "this device does not accept control from this browser");
  assert.equal(deviceNote(info("d", { live: "connecting" }), t), "connecting…");
});

test("a save that came back without the panes' devices is told apart from one that kept them", async () => {
  const { namesMachines } = await import("../src/app/pane-tree.js");
  const leaf = (content: PaneContent) => ({ type: "leaf", id: "p", content });
  assert.equal(namesMachines(leaf({ kind: "terminal", id: "t" })), false);
  assert.equal(namesMachines(leaf({ kind: "terminal", id: "t", machine: "d-b" })), true);
  assert.equal(namesMachines({ type: "split", id: "s", direction: "row", ratio: 0.5, children: [leaf({ kind: "picker" }), leaf({ kind: "files", machine: "d-c" })] }), true);
  assert.equal(namesMachines(null), false);
  assert.equal(namesMachines({ type: "leaf", content: { machine: "" } }), false);
});

test("the cross-device strings say device in every language, never machine or computer", async () => {
  const { crossDeviceJa, crossDeviceZhCN, crossDeviceZhTW } = await import("../src/i18n/cross-device.js");
  const keys = Object.keys(crossDeviceZhCN).sort();
  assert.deepEqual(Object.keys(crossDeviceZhTW).sort(), keys);
  assert.deepEqual(Object.keys(crossDeviceJa).sort(), keys);
  const words = (s: string) => s.replace(/\{\{[^}]*\}\}/g, "");
  for (const key of keys) assert.doesNotMatch(words(key), /machine|computer/i, key);
  for (const [table, forbidden] of [[crossDeviceZhCN, /机器|电脑/], [crossDeviceZhTW, /機器|電腦/], [crossDeviceJa, /マシン|コンピューター|パソコン/]] as const) {
    for (const [key, value] of Object.entries(table)) assert.doesNotMatch(words(value), forbidden, key);
  }
});
