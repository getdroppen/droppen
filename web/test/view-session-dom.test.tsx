// A session view says so to its machine (`useViewSession`,
// `MachineRuntime.viewSession`): what droppen.app needs to fetch the copy of a
// session being looked at ahead of the background. A view mounted before its
// machine is open — the chain had not given it yet — says so the moment it
// opens; and lets go as it leaves. A view's hold and its session follow the
// machine as it is now: closed and opened again in one moment, or given
// another driver, it is held anew, not left on the one disposed.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, document } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { useHoldMachine, useViewSession } = await import("../src/lib/machines/context.js");
const { closeMachine, openMachine } = await import("../src/lib/machines/registry.js");
const { constantStore } = await import("../src/lib/machines/store.js");

function View({ machine, source, id }: { machine: string; source: string; id: string }) {
  useViewSession(machine, source, id);
  return null;
}

test("a session view of a machine not open yet tells it once it opens, follows the session it shows, and lets go as it leaves", async () => {
  const said: string[] = [];
  const driver = {
    info: constantStore({ key: "m-late", name: "m-late", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    dispose: () => undefined,
    viewSession: (source: string, id: string) => {
      said.push(`view ${source}/${id}`);
      return () => void said.push(`leave ${source}/${id}`);
    },
  };
  const transport = { tunneled: true, fetch: async () => new Response(), socketUrl: async (path: string) => path, openSocket: () => null as never };
  const root = createRoot(document.body.appendChild(document.createElement("div")));
  await act(async () => root.render(<View machine="m-late" source="claude" id="s-1" />));
  assert.deepEqual(said, [], "no machine yet: nobody to tell");
  await act(async () => void openMachine("m-late", transport as never, driver as never));
  assert.deepEqual(said, ["view claude/s-1"], "the machine opened after the view: told then");
  await act(async () => root.render(<View machine="m-late" source="claude" id="s-2" />));
  assert.deepEqual(said, ["view claude/s-1", "leave claude/s-1", "view claude/s-2"]);
  await act(async () => root.unmount());
  assert.deepEqual(said.at(-1), "leave claude/s-2");
  closeMachine("m-late");
});

function Holder({ machine }: { machine: string }) {
  useHoldMachine(machine);
  useViewSession(machine, "claude", "s-1");
  return null;
}

/** A driver that says what was asked of it, as `name`. */
function driverNamed(name: string, said: string[]) {
  return {
    info: constantStore({ key: "m-swap", name: "m-swap", state: "online" as const, since: 0, canManage: true, live: "live" as const }),
    ensureLive: () => undefined,
    dispose: () => void said.push(`${name} disposed`),
    hold: () => {
      said.push(`${name} held`);
      return () => void said.push(`${name} let go`);
    },
    viewSession: (source: string, id: string) => {
      said.push(`${name} view ${source}/${id}`);
      return () => void said.push(`${name} leave ${source}/${id}`);
    },
  };
}

test("a machine closed and opened again in one moment, or given another driver, is held and viewed anew, not left on the one disposed", async () => {
  const said: string[] = [];
  const transport = { tunneled: true, fetch: async () => new Response(), socketUrl: async (path: string) => path, openSocket: () => null as never };
  openMachine("m-swap", transport as never, driverNamed("a", said) as never);
  const root = createRoot(document.body.appendChild(document.createElement("div")));
  await act(async () => root.render(<Holder machine="m-swap" />));
  assert.deepEqual(said, ["a held", "a view claude/s-1"]);
  // The shell replaced: the same key closed and opened again before anything renders — the open list never changes.
  said.length = 0;
  await act(async () => {
    closeMachine("m-swap");
    openMachine("m-swap", transport as never, driverNamed("b", said) as never);
  });
  assert.deepEqual(said.filter((x) => x.startsWith("b")), ["b held", "b view claude/s-1"], "held and viewed on the new one");
  assert.ok(said.includes("a let go") && said.includes("a leave claude/s-1"), "and let go of on the old one");
  // Pointed at another driver, the runtime kept.
  said.length = 0;
  await act(async () => void openMachine("m-swap", transport as never, driverNamed("c", said) as never));
  assert.deepEqual(said.filter((x) => x.startsWith("c")), ["c held", "c view claude/s-1"]);
  await act(async () => root.unmount());
  assert.ok(said.includes("c let go") && said.includes("c leave claude/s-1"));
  closeMachine("m-swap");
});
