// A session view says so to its machine (`useViewSession`,
// `MachineRuntime.viewSession`): what droppen.app needs to fetch the copy of a
// session being looked at ahead of the background. A view mounted before its
// machine is open — the chain had not given it yet — says so the moment it
// opens; and lets go as it leaves.
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
const { useViewSession } = await import("../src/lib/machines/context.js");
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
