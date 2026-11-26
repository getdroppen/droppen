import assert from "node:assert/strict";
import test from "node:test";
import {
  buildDeviceList,
  buildSessionList,
  childTally,
  collapseKey,
  deviceCollapseKey,
  expandKey,
  groupKeyOf,
  matchesQuery,
  NO_PROJECT,
  repoDir,
  rowKey,
  type SessionListInput,
} from "../src/features/sessions/session-list-model.js";
import { externalSendBlock } from "../src/features/sessions/send-block.js";
import { LOCAL } from "../src/lib/machines/key.js";

const key = (row: Parameters<typeof rowKey>[0]) => rowKey(row, LOCAL);

type Session = SessionListInput["sessions"][number];

function session(over: Partial<Session> & Pick<Session, "id">): Session {
  return {
    accountId: null,
    attentionAt: null,
    createdAt: "2024-05-01T10:00:00.000Z",
    cwd: "/Users/k/Projects/app",
    filePath: `/root/${over.id}.jsonl`,
    gitBranch: null,
    repoKey: null,
    scope: "owned",
    sizeBytes: 10,
    source: "claude",
    status: "idle",
    title: "a session",
    unread: false,
    updatedAt: "2024-05-01T10:00:00.000Z",
    ...over,
  } as Session;
}

function input(over: Partial<SessionListInput>): SessionListInput {
  return {
    sessions: [],
    source: "all",
    query: "",
    grouped: false,
    collapsedDirs: new Set<string>(),
    homeDir: "/Users/k",
    ...over,
  };
}

test("a repository key names a .git directory; the heading wants the checkout", () => {
  assert.equal(repoDir("/Users/k/Projects/app/.git"), "/Users/k/Projects/app");
  // A bare repository has no `.git` segment to strip.
  assert.equal(repoDir("/srv/git/app.git"), "/srv/git/app.git");
  assert.equal(repoDir("/.git"), "/");
});

test("pins precede ordinary rows before the cap, preserving order within each section", () => {
  const sessions = [session({ id: "new" }), session({ id: "pin-a", pinned: true }), session({ id: "old" }), session({ id: "pin-b", pinned: true })];
  assert.deepEqual(buildSessionList(input({ sessions })).rows.map(key), ["local|claude/pin-a", "local|claude/pin-b", "local|claude/new", "local|claude/old"]);
  const limited = buildSessionList(input({ sessions, limit: 1 }));
  assert.deepEqual(limited.rows.map(key), ["local|claude/pin-a"]);
  assert.deepEqual([limited.ownedShown, limited.ownedTotal], [1, 4]);
  assert.equal(sessions[0].id, "new", "never mutate the shared feed");
});

test("grouped pins sit above every directory, even when their own directory is collapsed", () => {
  const dir = "/Users/k/Projects/app";
  const sessions = [session({ id: "normal" }), session({ id: "pin", pinned: true })];
  const model = buildSessionList(input({ sessions, grouped: true, collapsedDirs: new Set([collapseKey(dir)]) }));
  assert.deepEqual(model.rows.map(key), ["local|claude/pin", `h:${collapseKey(dir)}`]);
  const header = model.rows[1];
  assert.equal(header.type, "header");
  if (header.type === "header") assert.deepEqual(header.list.map(s => s.id), ["normal"], "pins are not duplicated inside directory groups");
  assert.deepEqual(buildSessionList(input({ sessions: [sessions[1]], grouped: true })).rows.map(key), ["local|claude/pin"], "no empty directory heading");
});

test("pins respect source, search and ownership, and unpin restores ordinary order", () => {
  const sessions = [session({ id: "new", title: "match" }), session({ id: "same", pinned: true, title: "match" }), session({ id: "same", source: "codex", pinned: true }), session({ id: "external", scope: "external", pinned: true })];
  assert.deepEqual(buildSessionList(input({ sessions, source: "claude", query: "match" })).rows.map(key), ["local|claude/same", "local|claude/new"]);
  assert.deepEqual(buildSessionList(input({ sessions, source: "codex" })).rows.map(key), ["local|codex/same"]);
  assert.deepEqual(buildSessionList(input({ sessions, query: "absent" })).rows, []);
  assert.deepEqual(buildSessionList(input({ sessions: sessions.map(s => ({ ...s, pinned: false })), source: "claude" })).rows.map(key), ["local|claude/new", "local|claude/same"]);
});

test("a session groups under its repository, and falls back to its own cwd", () => {
  assert.equal(
    groupKeyOf(session({ id: "a", cwd: "/Users/k/Projects/app-wt", repoKey: "/Users/k/Projects/app/.git" })),
    "/Users/k/Projects/app",
  );
  assert.equal(groupKeyOf(session({ id: "b", cwd: "/tmp/x" })), "/tmp/x");
  assert.equal(groupKeyOf(session({ id: "c", cwd: null })), NO_PROJECT);
});

test("the filter box covers title and cwd, case-insensitively", () => {
  const s = session({ id: "a", title: "Refactor Feed", cwd: "/Users/k/Projects/app" });
  assert.equal(matchesQuery(s, "refactor"), true);
  assert.equal(matchesQuery(s, "projects/app"), true);
  assert.equal(matchesQuery(s, "nope"), false);
  assert.equal(matchesQuery(session({ id: "b", cwd: null }), "any"), false);
});

test("external sessions are not listed at all", () => {
  const sessions = [
    session({ id: "own-1" }),
    session({ id: "ext-1", scope: "external" }),
    session({ id: "ext-2", scope: "external", source: "codex" }),
  ];
  const model = buildSessionList(input({ sessions }));
  assert.deepEqual(
    model.rows.map(key),
    ["local|claude/own-1"],
    "browsing somebody else's session is not the sidebar's job any more",
  );
  assert.deepEqual([model.ownedShown, model.ownedTotal], [1, 1]);
});

test("a machine with nothing of its own has an empty list, not a divider", () => {
  const model = buildSessionList(input({ sessions: [session({ id: "ext-1", scope: "external" })] }));
  assert.deepEqual(model.rows, []);
});

test("search covers the list, which is all there is to search", () => {
  const sessions = [
    session({ id: "own-1", title: "keep" }),
    session({ id: "own-2", title: "drop" }),
    session({ id: "ext-1", scope: "external", title: "keep" }),
  ];
  const model = buildSessionList(input({ sessions, query: "keep" }));
  assert.deepEqual(model.rows.map(key), ["local|claude/own-1"]);
  assert.deepEqual([model.ownedShown, model.ownedTotal], [1, 1]);
});

test("the source tab narrows the list", () => {
  const sessions = [
    session({ id: "own-1" }),
    session({ id: "own-2", source: "codex" }),
    session({ id: "ext-1", scope: "external", source: "codex" }),
  ];
  const model = buildSessionList(input({ sessions, source: "claude" }));
  assert.deepEqual(model.rows.map(key), ["local|claude/own-1"]);
});

test("worktrees of one repository land under one heading, each row naming its checkout", () => {
  const sessions = [
    session({ id: "main", cwd: "/Users/k/Projects/app", repoKey: "/Users/k/Projects/app/.git" }),
    session({ id: "wt", cwd: "/Users/k/Projects/app-hotfix", repoKey: "/Users/k/Projects/app/.git" }),
    session({ id: "other", cwd: "/Users/k/Projects/other", repoKey: null }),
  ];
  const model = buildSessionList(input({ sessions, grouped: true }));
  assert.deepEqual(
    model.rows.map((r) => (r.type === "header" ? `H:${r.dir}` : key(r))),
    ["H:/Users/k/Projects/app", "local|claude/main", "local|claude/wt", "H:/Users/k/Projects/other", "local|claude/other"],
  );
  const labels = model.rows.flatMap((r) => (r.type === "session" ? [r.dirLabel] : []));
  assert.deepEqual(labels, ["~/Projects/app", "~/Projects/app-hotfix", "~/Projects/other"]);
});

test("directory headings are ranked by when work was started, not by what is loudest", () => {
  const loud = session({
    id: "loud",
    cwd: "/Users/k/Projects/old",
    createdAt: "2024-04-01T10:00:00.000Z",
    updatedAt: "2024-05-01T12:00:00.000Z",
    unread: true,
  });
  const quiet = session({ id: "quiet", cwd: "/Users/k/Projects/new", createdAt: "2024-04-02T10:00:00.000Z", updatedAt: "2024-04-02T10:00:00.000Z" });
  const headings = (sessions: Session[]) =>
    buildSessionList(input({ sessions, grouped: true })).rows.map((r) => (r.type === "header" ? `H:${r.dir}` : key(r)));
  // The louder directory is listed first by activity, and still sits second.
  assert.deepEqual(headings([loud, quiet]), [
    "H:/Users/k/Projects/new",
    "local|claude/quiet",
    "H:/Users/k/Projects/old",
    "local|claude/loud",
  ]);
  // A message makes `old` the loudest; only its row moves, never the heading.
  const after = [{ ...loud, unread: true, updatedAt: "2024-05-01T13:00:00.000Z" }, quiet];
  assert.deepEqual(headings(after), [
    "H:/Users/k/Projects/new",
    "local|claude/quiet",
    "H:/Users/k/Projects/old",
    "local|claude/loud",
  ]);
});

test("rows keep the feed's order inside their heading", () => {
  const dir = "/Users/k/Projects/app";
  const newer = session({ id: "newer", cwd: dir, updatedAt: "2024-05-01T12:00:00.000Z", unread: true });
  const older = session({ id: "older", cwd: dir, updatedAt: "2024-05-01T09:00:00.000Z" });
  const model = buildSessionList(input({ sessions: [newer, older], grouped: true }));
  assert.deepEqual(model.rows.map(key), [`h:${collapseKey(dir)}`, "local|claude/newer", "local|claude/older"]);
});

test("headings started at the same moment order by directory, so the list never shuffles on a tie", () => {
  const sessions = [
    session({ id: "b", cwd: "/Users/k/Projects/b", createdAt: "2024-05-01T10:00:00.000Z" }),
    session({ id: "a", cwd: "/Users/k/Projects/a", createdAt: "2024-05-01T10:00:00.000Z" }),
  ];
  assert.deepEqual(
    buildSessionList(input({ sessions, grouped: true })).rows.map((r) => (r.type === "header" ? `H:${r.dir}` : key(r))),
    ["H:/Users/k/Projects/a", "local|claude/a", "H:/Users/k/Projects/b", "local|claude/b"],
  );
});

test("flat mode still labels rows with the basename", () => {
  const model = buildSessionList(input({ sessions: [session({ id: "a" })] }));
  assert.deepEqual(
    model.rows.flatMap((r) => (r.type === "session" ? [r.dirLabel] : [])),
    ["app"],
  );
});

test("a collapsed group hides its rows but keeps its header", () => {
  const sessions = [
    session({ id: "a", cwd: "/Users/k/Projects/app" }),
    session({ id: "b", cwd: "/Users/k/Projects/other" }),
  ];
  const model = buildSessionList(
    input({
      sessions,
      grouped: true,
      collapsedDirs: new Set([collapseKey("/Users/k/Projects/app")]),
    }),
  );
  assert.deepEqual(model.rows.map(key), [
    "h:owned:/Users/k/Projects/app",
    "h:owned:/Users/k/Projects/other",
    "local|claude/b",
  ]);
});

test("the composer blocks on a stated scope, preferring the state frame's reason", () => {
  assert.deepEqual(
    externalSendBlock({
      scope: "external",
      sendBlockReason: "not started by this instance",
      refusal: null,
    }),
    { reason: "not started by this instance" },
  );
});

test("a send that lost the race blocks with the refusal's own hint", () => {
  assert.deepEqual(
    externalSendBlock({
      scope: "owned",
      // Stale: the frame still believes the session is ours, which is the race.
      sendBlockReason: null,
      refusal: { hint: "continue it where it was started" },
    }),
    { reason: "continue it where it was started" },
  );
});

test("a hintless refusal blocks with no reason of its own", () => {
  assert.deepEqual(externalSendBlock({ scope: "external", sendBlockReason: null, refusal: null }), { reason: null });
});

test("an owned session with no refusal is not blocked at all", () => {
  // The shape of the fix: the composer drops the refusal once the frame stops
  // saying external, and passing that dropped refusal here unblocks it.
  const refusal = { hint: "continue it where it was started" };
  assert.notEqual(externalSendBlock({ scope: "owned", sendBlockReason: null, refusal }), null);
  assert.equal(externalSendBlock({ scope: "owned", sendBlockReason: null, refusal: null }), null);
});

test("the list is capped, and says how many matched behind the cap", () => {
  const sessions = Array.from({ length: 3 }, (_, i) => session({ id: `own-${i}` }));
  const model = buildSessionList(input({ sessions, limit: 2 }));
  assert.deepEqual([model.ownedShown, model.ownedTotal], [2, 3]);
});

// S1-7: several devices' sessions as one list (the local app with its
// gateway's devices): one device's list is exactly the single-device one;
// every device's is the same order over all of them; grouped, each device's
// projects are headings of their own.
test("several devices' sessions make one list, and one device's is exactly the single list", () => {
  const mine = [
    session({ id: "m1", createdAt: "2024-05-01T10:00:00.000Z", updatedAt: "2024-05-01T10:00:00.000Z", cwd: "/Users/k/Projects/app", repoKey: "/Users/k/Projects/app/.git" }),
    session({ id: "m2", createdAt: "2024-05-01T08:00:00.000Z", updatedAt: "2024-05-01T08:00:00.000Z", cwd: "/Users/k/Projects/other", pinned: true } as Partial<Session> & { id: string }),
    session({ id: "mx", scope: "external" }),
  ];
  const theirs = [
    session({ id: "t1", createdAt: "2024-05-01T11:00:00.000Z", updatedAt: "2024-05-01T11:00:00.000Z", cwd: "/home/b/app", repoKey: "/home/b/app/.git", unread: true } as Partial<Session> & { id: string }),
    session({ id: "t2", createdAt: "2024-05-01T09:00:00.000Z", updatedAt: "2024-05-01T09:00:00.000Z", cwd: "/Users/k/Projects/app", repoKey: "/Users/k/Projects/app/.git" }),
  ];
  const lists = [
    { machine: LOCAL, rows: mine, homeDir: "/Users/k" },
    { machine: "m-b", rows: theirs, homeDir: "/home/b" },
    { machine: "m-c", rows: null },
  ];
  for (const grouped of [false, true]) {
    const single = buildSessionList(input({ sessions: mine, grouped }));
    const one = buildDeviceList({ lists, home: LOCAL, device: LOCAL, query: "", grouped, collapsedDirs: new Set() });
    assert.deepEqual(
      one.rows.map((r) => ({ ...r, machine: undefined })),
      single.rows.map((r) => ({ ...r, machine: undefined })),
      `the home's own list, grouped: ${grouped}`,
    );
    assert.deepEqual([one.ownedShown, one.ownedTotal], [single.ownedShown, single.ownedTotal]);
  }
  // Every device: pinned first, then unread, then newest; each row says its device.
  const all = buildDeviceList({ lists, home: LOCAL, device: "all", query: "", grouped: false, collapsedDirs: new Set() });
  assert.deepEqual(
    all.rows.map((r) => (r.type === "session" ? `${r.machine}:${r.session.id}` : "h")),
    ["local:m2", "m-b:t1", "local:m1", "m-b:t2"],
  );
  // The filter applies to every device.
  const found = buildDeviceList({ lists, home: LOCAL, device: "all", query: "/home/b", grouped: false, collapsedDirs: new Set() });
  assert.deepEqual(found.rows.map((r) => (r.type === "session" ? r.session.id : "h")), ["t1"]);
  // Grouped: the same path on two devices is two projects; another device's fold has its own key.
  const grouped = buildDeviceList({ lists, home: LOCAL, device: "all", query: "", grouped: true, collapsedDirs: new Set([deviceCollapseKey("m-b", LOCAL, "/Users/k/Projects/app")]) });
  const headers = grouped.rows.filter((r) => r.type === "header");
  assert.deepEqual(
    headers.map((h) => (h.type === "header" ? [h.machine, h.dir, h.isOpen] : null)),
    [
      ["m-b", "/home/b/app", true],
      [LOCAL, "/Users/k/Projects/app", true],
      ["m-b", "/Users/k/Projects/app", false],
    ],
  );
  assert.equal(deviceCollapseKey(LOCAL, LOCAL, "/x"), collapseKey("/x"), "the home's folds keep their keys from before devices");
  // Another device's cwd is shortened by its own home.
  const t1 = grouped.rows.find((r) => r.type === "session" && r.session.id === "t1");
  assert.equal(t1?.type === "session" && t1.dirLabel, "~/app");
  // Rows of different devices never share a key.
  assert.notEqual(rowKey({ type: "session", session: theirs[1]!, dirLabel: null, machine: "m-b" }, LOCAL), key({ type: "session", session: theirs[1]!, dirLabel: null }));
});

type Child = NonNullable<Session["children"]>[number];

function child(id: string, over: Partial<Child> = {}): Child {
  return { id, kind: "agent", name: id, role: null, status: "done", updatedAt: "2024-05-01T10:00:00.000Z", unread: false, children: [], ...over };
}

test("a session's subagents are listed under it only while it is unfolded, each level on its own", () => {
  const tree = [child("a1", { children: [child("a2", { status: "working", unread: true })] }), child("b1", { status: "failed" })];
  const sessions = [session({ id: "s", children: tree }), session({ id: "t" })];
  const folded = buildSessionList(input({ sessions }));
  assert.deepEqual(folded.rows.map(key), ["local|claude/s", "local|claude/t"], "folded by default");
  const first = folded.rows[0]!;
  assert.equal(first.type === "session" && first.expandKey, expandKey("claude", "s"));

  const open = buildSessionList(input({ sessions, expanded: new Set([expandKey("claude", "s")]) }));
  assert.deepEqual(open.rows.map((r) => (r.type === "child" ? `${r.depth}:${r.child.id}` : r.type)), ["session", "1:a1", "1:b1", "session"]);
  assert.deepEqual([open.ownedShown, open.ownedTotal], [2, 2], "subagents are not counted as sessions");

  const deep = buildSessionList(input({ sessions, expanded: new Set([expandKey("claude", "s"), expandKey("claude", "s", ["a1"])]) }));
  assert.deepEqual(deep.rows.map((r) => (r.type === "child" ? `${r.depth}:${r.child.id}` : r.type)), ["session", "1:a1", "2:a2", "1:b1", "session"]);
  const a2 = deep.rows[2]!;
  assert.equal(a2.type === "child" && a2.root.id, "s", "however deep, a row knows its session");
  assert.equal(new Set(deep.rows.map(key)).size, deep.rows.length, "no two rows share a key");

  // What the connectors draw: `├` for a1 (b1 follows), `└` for a2 and b1,
  // and a1's line running on past a2 to reach b1.
  assert.deepEqual(
    deep.rows.filter((r) => r.type === "child").map((r) => (r.type === "child" ? [r.child.id, r.last, r.through] : null)),
    [["a1", false, []], ["a2", true, [true]], ["b1", true, []]],
  );

  // An unfold under a folded one is kept, not shown.
  const hidden = buildSessionList(input({ sessions, expanded: new Set([expandKey("claude", "s", ["a1"])]) }));
  assert.equal(hidden.rows.length, 2);
  // Grouped, they follow their session into its directory.
  const grouped = buildSessionList(input({ sessions, grouped: true, expanded: new Set([expandKey("claude", "s")]) }));
  assert.deepEqual(grouped.rows.map((r) => r.type), ["header", "session", "child", "child", "session"]);
});

test("the filter sees sessions, not what they delegated", () => {
  const sessions = [session({ id: "s", title: "plan", children: [child("a1", { name: "needle" })] })];
  assert.deepEqual(buildSessionList(input({ sessions, query: "needle" })).rows, []);
});

test("subagents tally every level, for the folded row", () => {
  const tree = [
    child("a1", { status: "working", children: [child("a2", { status: "waiting", unread: true }), child("a3", { status: "failed", unread: true })] }),
    child("b1"),
  ];
  assert.deepEqual(childTally(tree), { total: 4, working: 1, waiting: 1, failed: 1, unread: 2 });
  assert.deepEqual(childTally([]), { total: 0, working: 0, waiting: 0, failed: 0, unread: 0 });
});

test("another device's unfolds are keyed by that device", () => {
  const ours = [session({ id: "s", children: [child("a1")] })];
  const theirs = [session({ id: "s", children: [child("a1")] })];
  const lists = [{ machine: LOCAL, rows: ours }, { machine: "m-b", rows: theirs }];
  const model = buildDeviceList({ lists, home: LOCAL, device: "all", query: "", grouped: false, collapsedDirs: new Set(), expanded: new Set([expandKey("claude", "s", [], "m-b")]) });
  assert.deepEqual(model.rows.map((r) => (r.type === "child" ? `child:${r.machine}` : `${r.type}:${r.machine}`)), ["session:local", "session:m-b", "child:m-b"]);
  assert.equal(new Set(model.rows.map(key)).size, model.rows.length);
});
