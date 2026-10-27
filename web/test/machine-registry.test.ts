// The machines a page reaches (`lib/machines`): every transport, typed client,
// query key and feed is one machine's, and the local page — which boots no
// shell at all, or the local one — is exactly the page from before: one
// machine, `local`, on the direct transport, and the deprecated page-wide
// aliases pointing at it.
import assert from "node:assert/strict";
import test from "node:test";
import { QueryClient } from "@tanstack/react-query";
import { feedHubFor } from "../src/app/global-feed.js";
import { LOCAL, parseSessionKey, sessionKey, sessionRef } from "../src/lib/machines/key.js";
import { invalidateMachine, machineQueries, mq } from "../src/lib/machines/query.js";
import { apiFor, closeMachine, homeMachine, homeMachineStore, isOpen, openMachine, openMachines, perMachine, runtimeFor, setHome, setShell, shellKind, transportFor } from "../src/lib/machines/registry.js";
import { constantStore } from "../src/lib/machines/store.js";
import { openHubMachine } from "../src/lib/machines/shells/hub.js";
import { directTransport, type SocketLike, type Transport } from "../src/lib/transport/index.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
// An absolute base: a Request outside a page has no origin to resolve "/" against.
const BASE = "http://machine.test";

class FakeSocket implements SocketLike {
  readyState = 0;
  binaryType = "blob";
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  constructor(readonly url: string) {}
  send() {}
  close() {
    this.readyState = 3;
  }
}

/** A transport that records what went through it, under `name`. */
function recording(name: string, log: string[]): Transport {
  return {
    fetch: async (request) => {
      log.push(`${name} fetch ${new URL(request.url).pathname}`);
      return new Response("[]", { headers: { "content-type": "application/json" } });
    },
    socketUrl: async (path) => `${name}:${path}`,
    openSocket: (url) => {
      log.push(`${name} socket ${url}`);
      return new FakeSocket(url);
    },
  };
}

test("a page that boots no shell is the local page: one machine, local, reached directly", () => {
  assert.equal(shellKind(), "local");
  assert.equal(homeMachine(), LOCAL);
  assert.deepEqual(openMachines(), [LOCAL]);
  assert.equal(runtimeFor(LOCAL).transport, directTransport);
  assert.equal(apiFor(LOCAL), runtimeFor(LOCAL).api, "one client per machine, for the life of the page");
  assert.equal(homeMachine(), LOCAL);
  assert.equal(transportFor(LOCAL), directTransport);
});

test("a machine the page has not opened is a bug, not a silent fallback to the page's own server", () => {
  assert.throws(() => runtimeFor("m-unknown"), /no machine "m-unknown"/);
  assert.throws(() => openHubMachine(LOCAL, directTransport), /not a device id/);
});

test("two machines: separate transports, clients and per-machine singletons, and the aliases follow the home", async (t) => {
  const log: string[] = [];
  openMachine("m-a", recording("a", log));
  openMachine("m-b", recording("b", log));
  t.after(() => {
    setShell("local", LOCAL);
    closeMachine("m-a");
    closeMachine("m-b");
  });

  await apiFor("m-a").GET("/api/terminals", { baseUrl: BASE });
  await apiFor("m-b").GET("/api/apps", { baseUrl: BASE });
  assert.deepEqual(log.splice(0), ["a fetch /api/terminals", "b fetch /api/apps"]);

  const made: string[] = [];
  const disposed: string[] = [];
  const thing = perMachine(
    (machine) => {
      made.push(machine);
      return { machine };
    },
    (value) => disposed.push(value.machine),
  );
  assert.equal(thing("m-a"), thing("m-a"), "made once per machine");
  assert.notEqual(thing("m-a"), thing("m-b"));
  assert.deepEqual(made, ["m-a", "m-b"]);

  // Each machine's feed socket goes through that machine's transport.
  const hubA = feedHubFor("m-a");
  const hubB = feedHubFor("m-b");
  assert.notEqual(hubA, hubB);
  hubA.start({ onFrame: () => undefined, onOpen: () => undefined, onClose: () => undefined });
  hubB.start({ onFrame: () => undefined, onOpen: () => undefined, onClose: () => undefined });
  t.after(() => {
    hubA.stop();
    hubB.stop();
  });
  await flush();
  assert.deepEqual(log.splice(0).sort(), ["a socket a:/api/events?v=2&z=1", "b socket b:/api/events?v=2&z=1"].sort());

  // The home is the shell's to say.
  openHubMachine("m-b", runtimeFor("m-b").transport);
  setHome("m-b");
  assert.equal(homeMachine(), "m-b");
  assert.equal(shellKind(), "hub");
  assert.deepEqual(openMachines()[0], "m-b", "home first");
  assert.throws(() => runtimeFor(LOCAL), /no machine "local"/, "droppen.app has no local machine");
  await apiFor(homeMachine()).GET("/api/prefs", { baseUrl: BASE });
  assert.deepEqual(log.splice(0), ["b fetch /api/prefs"]);
  assert.throws(() => closeMachine("m-b"), /home machine cannot be closed/);

  // Closing a machine drops what was made for it.
  setShell("local", LOCAL);
  closeMachine("m-a");
  assert.deepEqual(disposed, ["m-a"]);
  openMachine("m-a", recording("a", log));
});

test("query keys start with their machine, and invalidating one machine leaves the others alone", async () => {
  assert.deepEqual(mq(LOCAL, "prefs"), ["m", "local", "prefs"]);
  assert.deepEqual(mq("m-a", "fs-list", "~"), ["m", "m-a", "fs-list", "~"]);
  assert.deepEqual(machineQueries("m-a"), ["m", "m-a"]);
  const client = new QueryClient();
  client.setQueryData(mq(LOCAL, "prefs"), { n: 1 });
  client.setQueryData(mq("m-a", "prefs"), { n: 2 });
  client.setQueryData(mq("m-a", "fs-list", "~"), []);
  await invalidateMachine(client, "m-a");
  assert.equal(client.getQueryState(mq(LOCAL, "prefs"))?.isInvalidated, false);
  assert.equal(client.getQueryState(mq("m-a", "prefs"))?.isInvalidated, true);
  assert.equal(client.getQueryState(mq("m-a", "fs-list", "~"))?.isInvalidated, true);
  client.clear();
});

test("a session's key names its machine, and splits back even when a device id has a bar in it", () => {
  const ref = sessionRef(LOCAL, "claude", "0b6f-4c1e");
  assert.equal(sessionKey(ref), "local|claude/0b6f-4c1e");
  assert.deepEqual(parseSessionKey(sessionKey(ref)), ref);
  const odd = sessionRef("m-a|b", "codex", "t-1");
  assert.deepEqual(parseSessionKey(sessionKey(odd)), odd);
  assert.equal(parseSessionKey("claude/0b6f"), null, "a bare source/id is not a session key");
  assert.equal(parseSessionKey("local|claude/"), null);
  assert.deepEqual(ref, { machine: LOCAL, source: "claude", id: "0b6f-4c1e" });
  assert.notDeepEqual(sessionRef("m-a", "claude", "0b6f-4c1e"), ref, "the same id on another machine is another session");
});

test("the home moves without anything being rebuilt, and a machine closed goes with what its shell kept for it", async (t) => {
  const log: string[] = [];
  const disposed: string[] = [];
  const driver = (key: string) => ({
    info: constantStore({ key, name: key, state: "online" as const, since: 0, canManage: true, live: "copy" as const }),
    ensureLive: () => void log.push(`live ${key}`),
    dispose: () => void disposed.push(key),
  });
  t.after(() => {
    setShell("local", LOCAL);
    for (const m of ["m-x", "m-y"]) if (isOpen(m)) closeMachine(m);
  });
  const x = openMachine("m-x", recording("x", log), driver("m-x"));
  openMachine("m-y", recording("y", log), driver("m-y"));
  setShell("hub", "m-x");
  assert.equal(isOpen(LOCAL), false, "droppen.app has no local machine");

  const homes: string[] = [];
  const off = homeMachineStore().subscribe(() => homes.push(homeMachine()));
  setHome("m-y");
  setHome("m-y");
  setHome("m-x");
  off();
  assert.deepEqual(homes, ["m-y", "m-x"], "heard once per move, not for a move to where it already is");
  assert.equal(runtimeFor("m-x"), x, "the runtime is the same one: nothing rebuilt");
  assert.equal(x.info().get().live, "copy");
  x.ensureLive();
  assert.deepEqual(log, ["live m-x"]);
  assert.throws(() => setHome("m-unknown"), /no machine "m-unknown"/);

  assert.throws(() => closeMachine("m-x"), /home machine cannot be closed/);
  closeMachine("m-y");
  assert.deepEqual(disposed, ["m-y"]);
  assert.equal(isOpen("m-y"), false);

  // The shell's last machine, or a page being left: the home goes too, its
  // runtime kept, retired, for the views of it still on their way out.
  closeMachine("m-x", { evenTheHome: true });
  assert.deepEqual(disposed, ["m-y", "m-x"]);
  assert.equal(isOpen("m-x"), false);
  assert.equal(runtimeFor("m-x"), x, "still found");
  assert.deepEqual(openMachines(), [], "but not one of the page's machines");
  openMachine("m-x", recording("x2", log), driver("m-x"));
  assert.equal(isOpen("m-x"), true, "opened again, it is back");

  // A hub shell with no machine yet has no home to speak of: nothing reaches `local`.
  setShell("local", LOCAL);
  closeMachine("m-x");
  setShell("hub");
  assert.throws(() => runtimeFor(homeMachine()), /no machine "local"/);
});
