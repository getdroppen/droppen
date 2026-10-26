// What this browser keeps for a machine (`lib/machines/storage.ts`), against a
// fixture of the keys a browser holds today, from before machines had keys.
//
// - The local page reads and writes exactly today's keys: an upgrade changes
//   nothing in its storage, and a rolled-back build still finds everything.
// - On droppen.app — no local machine; the keys were written for whichever
//   machine was open — each key is taken over (moved, once) by the first
//   machine that asks for it: a session's draft by that session's machine, a
//   page-wide one by the machine open first. This browser's own keys (theme,
//   panels, the hub account, the login) are never touched.
// - When both keys are there — a tab still on an older build wrote the old one
//   after the move — nothing it wrote is dropped: a page-wide value from it
//   wins, the newer draft wins, and two outboxes are merged by message id.
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { stashPendingMessage, takePendingMessage } from "../src/features/chat/pending-message.js";
import { clearLastFilesDir, LAST_DIR_NAME, readLastFilesDir, writeLastFilesDir } from "../src/features/files/files-prefs.js";
import { mergeStoredOutboxes, readOutbox } from "../src/features/sessions/send-receipts.js";
import { claimSessionDraft, readSessionDraftSnapshot, sessionDraftKey, writeSessionDraft } from "../src/features/sessions/session-draft.js";
import { rotationDismissName } from "../src/features/accounts/session-rotation.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { closeMachine, openMachine, setHome, setShell } from "../src/lib/machines/registry.js";
import { openHubMachine } from "../src/lib/machines/shells/hub.js";
import {
  claimMachineItem,
  machineCacheKey,
  machineStorageKey,
  readMachineItem,
  removeMachineItem,
  writeMachineItem,
} from "../src/lib/machines/storage.js";
import { directTransport } from "../src/lib/transport/index.js";

class MemoryStorage {
  readonly values = new Map<string, string>();
  constructor(initial: Record<string, string> = {}) {
    for (const [k, v] of Object.entries(initial)) this.values.set(k, v);
  }
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.values.set(key, value);
  }
  removeItem(key: string) {
    this.values.delete(key);
  }
  snapshot(): Record<string, string> {
    return Object.fromEntries([...this.values].sort(([a], [b]) => a.localeCompare(b)));
  }
}

/** This browser's own keys, as a browser holds them today. */
const DEVICE = {
  "rh.auth.token": "tok-1",
  "rh.theme": "dark",
  "rh.fontSize": "large",
  "rh.lang": "zh-CN",
  "rh.panel.width.sessions": "320",
  "rh.panel.sessions": "1",
  "rh.sessions.backgroundPanelOpen": "1",
  "rh.files.showHidden": "1",
  "rh.hub.session": '{"account":"a-1","token":"t"}',
  "rh.hub.machine.a-1": "m-a",
  "rh.relays.manual": "r-1",
};

/** A machine's keys, as a browser holds them today (no machine named). */
const MACHINE = {
  "rh.draft.claude.s-1": '{"version":1,"text":"half a thought","revision":"r1"}',
  "rh.draft.codex.s-2": "a draft from before the versioned format",
  "rh.queue.claude.s-1": '[{"clientMessageId":"c-1","text":"queued","state":"unknown"}]',
  "rh.rotation-dismissed.claude.s-1": "2026-10-16T10:00:00.000Z",
  "rh.files.lastDir": "/Users/me/Projects",
  "rh.sessions.collapsedDirs": '["owned:/Users/me/Projects/app"]',
  "rh.composer.enterToSend": "modifier",
  "rh.composer.model.claude": "opus",
};

const TODAY = { ...DEVICE, ...MACHINE };

/** Everything the app reads for a machine, through the same helpers it uses. */
function readEverything(storage: MemoryStorage, machine: string) {
  const s1 = { machine, source: "claude", id: "s-1" };
  claimSessionDraft(storage, s1);
  return {
    draft: readSessionDraftSnapshot(storage, sessionDraftKey(s1)).text,
    queue: readMachineItem(storage, machine, "queue.claude.s-1"),
    dismissed: readMachineItem(storage, machine, rotationDismissName("claude", "s-1")),
    lastDir: readLastFilesDir(storage, machine),
    collapsed: readMachineItem(storage, machine, "sessions.collapsedDirs"),
    enter: readMachineItem(storage, machine, "composer.enterToSend"),
    model: readMachineItem(storage, machine, "composer.model.claude"),
  };
}

/** droppen.app with `machines` open, the first one home, for the length of a test. */
function hubPage(t: TestContext, ...machines: string[]): void {
  for (const machine of machines) openMachine(machine, directTransport);
  openHubMachine(machines[0]!, directTransport);
  setHome(machines[0]!);
  t.after(() => {
    setShell("local", LOCAL);
    for (const machine of machines) closeMachine(machine);
  });
}

test("the local machine's keys are today's keys", () => {
  assert.equal(machineStorageKey(LOCAL, "draft.claude.s-1"), "rh.draft.claude.s-1");
  assert.equal(sessionDraftKey({ machine: LOCAL, source: "claude", id: "s-1" }), "rh.draft.claude.s-1");
  assert.equal(machineStorageKey(LOCAL, LAST_DIR_NAME), "rh.files.lastDir");
  assert.equal(machineCacheKey(LOCAL, "sessions"), "sessions");
  assert.equal(machineCacheKey(LOCAL, "feed:claude/s-1"), "feed:claude/s-1");
  // Any other machine's are its own, whatever its id is made of.
  assert.equal(machineStorageKey("m-a", "draft.claude.s-1"), "rh.m/m-a/draft.claude.s-1");
  assert.equal(machineStorageKey("m/a.b", "files.lastDir"), "rh.m/m%2Fa.b/files.lastDir");
  assert.equal(machineCacheKey("m-a", "sessions"), "m/m-a/sessions");
});

test("local page: reading and writing through every helper leaves today's storage byte for byte as it was", () => {
  const storage = new MemoryStorage(TODAY);
  assert.deepEqual(readEverything(storage, LOCAL), {
    draft: "half a thought",
    queue: MACHINE["rh.queue.claude.s-1"],
    dismissed: "2026-10-16T10:00:00.000Z",
    lastDir: "/Users/me/Projects",
    collapsed: '["owned:/Users/me/Projects/app"]',
    enter: "modifier",
    model: "opus",
  });
  assert.deepEqual(storage.snapshot(), new MemoryStorage(TODAY).snapshot(), "nothing moved, nothing added");

  // Writes land on the very keys an older build reads.
  writeLastFilesDir(storage, LOCAL, "/Users/me/Elsewhere");
  writeMachineItem(storage, LOCAL, "sessions.collapsedDirs", "[]");
  assert.equal(storage.getItem("rh.files.lastDir"), "/Users/me/Elsewhere");
  assert.equal(storage.getItem("rh.sessions.collapsedDirs"), "[]");
  assert.deepEqual(
    [...storage.values.keys()].filter((k) => k.startsWith("rh.m/")),
    [],
    "no machine-named key on the local page",
  );
});

test("droppen.app: the machine that asks first takes over today's keys, and this browser's own keys stay put", (t) => {
  hubPage(t, "m-a", "m-b");
  const storage = new MemoryStorage(TODAY);

  const seen = readEverything(storage, "m-a");
  assert.deepEqual(seen, {
    draft: "half a thought",
    queue: MACHINE["rh.queue.claude.s-1"],
    dismissed: "2026-10-16T10:00:00.000Z",
    lastDir: "/Users/me/Projects",
    collapsed: '["owned:/Users/me/Projects/app"]',
    enter: "modifier",
    model: "opus",
  });
  const after = storage.snapshot();
  for (const [key, value] of Object.entries(DEVICE)) assert.equal(after[key], value, `${key} is this browser's, untouched`);
  for (const key of ["rh.draft.claude.s-1", "rh.queue.claude.s-1", "rh.files.lastDir", "rh.sessions.collapsedDirs"]) {
    assert.equal(after[key], undefined, `${key} moved`);
  }
  assert.equal(after["rh.m/m-a/draft.claude.s-1"], MACHINE["rh.draft.claude.s-1"], "moved as it was");
  assert.equal(after["rh.m/m-a/files.lastDir"], "/Users/me/Projects");
  assert.equal(after["rh.draft.codex.s-2"], MACHINE["rh.draft.codex.s-2"], "a session nobody asked about yet waits for its machine");

  // Another machine: nothing of the first one's, and its own session's draft is its own to take.
  assert.equal(readLastFilesDir(storage, "m-b"), null);
  const s2 = { machine: "m-b", source: "codex", id: "s-2" };
  claimSessionDraft(storage, s2);
  assert.equal(readSessionDraftSnapshot(storage, sessionDraftKey(s2)).text, "a draft from before the versioned format");
  assert.equal(storage.getItem("rh.draft.codex.s-2"), null);
  assert.equal(storage.getItem("rh.m/m-b/draft.codex.s-2"), MACHINE["rh.draft.codex.s-2"]);

  // Once: asking again moves nothing and reads the same.
  const settled = storage.snapshot();
  assert.deepEqual(readEverything(storage, "m-a"), seen);
  assert.deepEqual(storage.snapshot(), settled);
});

test("droppen.app: a draft cleared after the move does not come back", (t) => {
  hubPage(t, "m-a");
  const storage = new MemoryStorage({ "rh.draft.claude.s-1": "old" });
  const s1 = { machine: "m-a", source: "claude", id: "s-1" };
  claimSessionDraft(storage, s1);
  writeSessionDraft(storage, sessionDraftKey(s1), "");
  claimSessionDraft(storage, s1);
  assert.equal(readSessionDraftSnapshot(storage, sessionDraftKey(s1)).text, "", "cleared stays cleared");
  assert.deepEqual(storage.snapshot(), {}, "a clear removes the key, and the old one went with the move");
});

test("droppen.app: a page-wide value an older tab wrote after the move wins, and the old key goes", (t) => {
  hubPage(t, "m-a");
  const storage = new MemoryStorage({ "rh.files.lastDir": "/before", "rh.sessions.collapsedDirs": '["before"]' });
  assert.equal(readLastFilesDir(storage, "m-a"), "/before");
  writeLastFilesDir(storage, "m-a", "/this-build");
  writeMachineItem(storage, "m-a", "sessions.collapsedDirs", '["this-build"]');

  // A tab still on the older build writes the old keys again. Every write here
  // claims first, so that is the newer value.
  storage.setItem("rh.files.lastDir", "/older-tab");
  storage.setItem("rh.sessions.collapsedDirs", '["older-tab"]');
  assert.equal(readLastFilesDir(storage, "m-a"), "/older-tab");
  assert.equal(readMachineItem(storage, "m-a", "sessions.collapsedDirs"), '["older-tab"]');
  assert.equal(storage.getItem("rh.files.lastDir"), null);
  assert.equal(storage.getItem("rh.sessions.collapsedDirs"), null);

  // A write here is newer than anything the older tab left behind.
  storage.setItem("rh.files.lastDir", "/older-tab-again");
  writeLastFilesDir(storage, "m-a", "/newest");
  assert.equal(readLastFilesDir(storage, "m-a"), "/newest");
  storage.setItem("rh.files.lastDir", "/older-tab-again");
  clearLastFilesDir(storage, "m-a");
  assert.equal(readLastFilesDir(storage, "m-a"), null);
  removeMachineItem(storage, "m-a", "sessions.collapsedDirs");
  assert.deepEqual(storage.snapshot(), {});
});

test("droppen.app: when an older tab wrote a draft after the move, the newer of the two is kept", (t) => {
  hubPage(t, "m-a");
  const s1 = { machine: "m-a", source: "claude", id: "s-1" };
  const MINE = "rh.m/m-a/draft.claude.s-1";
  const OLD = "rh.draft.claude.s-1";
  const stored = (text: string, revision: string) => JSON.stringify({ version: 1, text, revision });
  const at = (ms: number, seq = 1) => `${ms.toString(36)}-${seq.toString(36)}`;
  const claim = (storage: MemoryStorage) => {
    claimSessionDraft(storage, s1);
    assert.equal(storage.getItem(OLD), null, "the old key goes either way");
    return readSessionDraftSnapshot(storage, sessionDraftKey(s1)).text;
  };

  // This build typed, then the older tab did.
  let storage = new MemoryStorage({ [OLD]: stored("from before", at(Date.now() - 120_000)) });
  claimSessionDraft(storage, s1);
  writeSessionDraft(storage, sessionDraftKey(s1), "this build");
  storage.setItem(OLD, stored("older tab, later", at(Date.now() + 60_000)));
  assert.equal(claim(storage), "older tab, later");

  // The older tab typed, then this build did — without claiming again.
  storage = new MemoryStorage({ [OLD]: stored("from before", at(Date.now() - 120_000)) });
  claimSessionDraft(storage, s1);
  storage.setItem(OLD, stored("older tab, earlier", at(Date.now() - 60_000)));
  writeSessionDraft(storage, sessionDraftKey(s1), "this build, later");
  assert.equal(claim(storage), "this build, later");

  // Times, not strings: "z" is 35 ms and "10" is 36, though "z-1" > "10-1".
  storage = new MemoryStorage({ [MINE]: stored("mine", "z-1"), [OLD]: stored("old", "10-1") });
  assert.equal(claim(storage), "old");
  // A tie stays the machine's, whatever the sequence says.
  storage = new MemoryStorage({ [MINE]: stored("mine", at(1_000, 1)), [OLD]: stored("old", at(1_000, 9)) });
  assert.equal(claim(storage), "mine");
  // A plain-text draft, from before revisions, is older than any with one.
  storage = new MemoryStorage({ [MINE]: stored("mine", at(1_000)), [OLD]: "plain old" });
  assert.equal(claim(storage), "mine");
  storage = new MemoryStorage({ [MINE]: "plain mine", [OLD]: stored("old", at(1_000)) });
  assert.equal(claim(storage), "old");
  storage = new MemoryStorage({ [MINE]: "plain mine", [OLD]: "plain old" });
  assert.equal(claim(storage), "plain mine");
});

test("droppen.app: an outbox an older tab wrote after the move is merged in by message id", (t) => {
  hubPage(t, "m-a");
  const MINE = "rh.m/m-a/queue.claude.s-1";
  const OLD = "rh.queue.claude.s-1";
  // Read the way the composer reads it (`usePersistedQueue`).
  const read = (storage: MemoryStorage) => {
    let minted = 0;
    const entries = readOutbox(
      JSON.parse(readMachineItem(storage, "m-a", "queue.claude.s-1", mergeStoredOutboxes) ?? "[]"),
      () => `minted-${++minted}`,
    );
    assert.equal(storage.getItem(OLD), null, "the old key goes");
    return entries;
  };

  const storage = new MemoryStorage({
    [MINE]: JSON.stringify([
      { clientMessageId: "c-1", text: "one" },
      { clientMessageId: "c-2", text: "two", state: "failed" },
      { clientMessageId: "c-3", text: "three", state: "unknown" },
      "from before ids",
    ]),
    [OLD]: JSON.stringify([
      // The older tab sent c-1 and heard nothing back: ask before sending it again.
      { clientMessageId: "c-1", text: "one", state: "unknown" },
      // The machine's own say is kept for the rest of what both have.
      { clientMessageId: "c-2", text: "two" },
      { clientMessageId: "c-3", text: "three" },
      "from before ids",
      // What only the older tab holds is added, in its order.
      { clientMessageId: "c-4", text: "four", state: "unsent" },
      { clientMessageId: "c-5", text: "five", images: ["/u/a.png"] },
    ]),
  });
  assert.deepEqual(read(storage), [
    { clientMessageId: "c-1", text: "one", state: "unknown" },
    { clientMessageId: "c-2", text: "two", state: "failed" },
    { clientMessageId: "c-3", text: "three", state: "unknown" },
    { clientMessageId: "minted-1", text: "from before ids" },
    { clientMessageId: "c-4", text: "four", state: "unsent" },
    { clientMessageId: "c-5", text: "five", images: ["/u/a.png"] },
  ]);
  // Settled: asking again changes nothing.
  const settled = storage.snapshot();
  assert.equal(read(storage).length, 6);
  assert.deepEqual(storage.snapshot(), settled);
});

test("droppen.app: an outbox side that does not parse is not merged, and the other side is kept as it was", (t) => {
  hubPage(t, "m-a");
  const MINE = "rh.m/m-a/queue.claude.s-1";
  const OLD = "rh.queue.claude.s-1";
  const good = JSON.stringify([{ clientMessageId: "c-9", text: "nine" }]);
  for (const [mine, old, kept] of [
    [good, "{not json", good],
    [good, '{"clientMessageId":"c-1"}', good],
    ["{not json", good, good],
    ["null", good, good],
    ["{not json", "[not json either", "{not json"],
  ] as const) {
    const pair = new MemoryStorage({ [MINE]: mine, [OLD]: old });
    claimMachineItem(pair, "m-a", "queue.claude.s-1", mergeStoredOutboxes);
    assert.deepEqual(pair.snapshot(), { [MINE]: kept }, `${mine} + ${old}`);
  }
});

test("the local page claims nothing for other machines: today's keys are the local machine's", () => {
  const storage = new MemoryStorage(TODAY);
  assert.equal(claimMachineItem(storage, "m-a", "files.lastDir"), "rh.m/m-a/files.lastDir");
  assert.equal(readLastFilesDir(storage, "m-a"), null);
  assert.deepEqual(storage.snapshot(), new MemoryStorage(TODAY).snapshot());
});

test("storage that throws is left alone, and nothing here throws", (t) => {
  hubPage(t, "m-a");
  const blocked = {
    getItem: () => {
      throw new Error("blocked");
    },
    setItem: () => {
      throw new Error("blocked");
    },
    removeItem: () => {
      throw new Error("blocked");
    },
  };
  assert.equal(claimMachineItem(blocked, "m-a", "files.lastDir"), "rh.m/m-a/files.lastDir");
  assert.equal(readLastFilesDir(blocked, "m-a"), null);
});

test("a pending chat message is its machine's, in sessionStorage too", (t) => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "sessionStorage");
  const storage = new MemoryStorage({ "rh.chat.pending.c-1": "from before" });
  Object.defineProperty(globalThis, "sessionStorage", { configurable: true, value: storage });
  t.after(() => {
    if (original) Object.defineProperty(globalThis, "sessionStorage", original);
    else Reflect.deleteProperty(globalThis, "sessionStorage");
  });
  assert.equal(takePendingMessage(LOCAL, "c-1"), "from before", "the local page reads today's key");
  stashPendingMessage("m-a", "c-1", "elsewhere");
  assert.deepEqual(storage.snapshot(), { "rh.m/m-a/chat.pending.c-1": "elsewhere" });
  assert.equal(takePendingMessage(LOCAL, "c-1"), null);
});
