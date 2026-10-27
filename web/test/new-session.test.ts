// The new-session view's decisions, without drawing it: the directories it
// offers and the one it starts in, the draft it keeps per device, the chord
// that opens it, the request it sends, and the pane it is.
import assert from "node:assert/strict";
import test from "node:test";
import { onMachine, paneMachine, usesMachine, withMachine, type PaneContent } from "../src/app/pane-tree.js";
import {
  applyNewSessionPrefill,
  clearNewSessionDraft,
  directoryLabel,
  directoryRows,
  EMPTY_NEW_SESSION_DRAFT,
  isNewSessionChord,
  newSessionShortcut,
  prefillFromSearch,
  prefillNewSessionDraft,
  readNewSessionDraft,
  recentDirectories,
  SHOWN_DIRECTORIES,
  startingDirectory,
  stepDirectory,
  writeNewSessionDraft,
  type NewSessionDraft,
} from "../src/features/sessions/new-session.js";
import { creationAttempt, creationRequest, type NewConversation } from "../src/features/sessions/new-session-flow.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { MemoryStorage } from "./storage.js";

const row = (cwd: string | null, createdAt: string, updatedAt: string) => ({ cwd, createdAt, updatedAt });

test("the directories offered are where sessions were last started, with how many ran there and when one last did anything", () => {
  const dirs = recentDirectories([
    row("/w/a", "2026-10-01T00:00:00Z", "2026-10-18T00:00:00Z"),
    row("/w/b", "2026-10-10T00:00:00Z", "2026-10-11T00:00:00Z"),
    row("/w/a", "2026-10-05T00:00:00Z", "2026-10-06T00:00:00Z"),
    row(null, "2026-10-19T00:00:00Z", "2026-10-19T00:00:00Z"),
    row("/w/c", "2026-09-01T00:00:00Z", "2026-09-02T00:00:00Z"),
  ]);
  assert.deepEqual(dirs.map((d) => d.cwd), ["/w/b", "/w/a", "/w/c"], "newest start first; a row without a directory is no directory");
  assert.deepEqual(dirs[1], { cwd: "/w/a", sessions: 2, lastActivity: "2026-10-18T00:00:00Z", lastStarted: "2026-10-05T00:00:00Z" });
  assert.deepEqual(recentDirectories(null), []);
  const many = Array.from({ length: 12 }, (_, i) => row(`/w/${i}`, `2026-10-${String(i + 1).padStart(2, "0")}T00:00:00Z`, "2026-10-19T00:00:00Z"));
  assert.equal(recentDirectories(many).length, 8, "as many as the server's own recent list");
});

test("a new session starts in the directory picked, else the one last started in", () => {
  const recent = recentDirectories([row("/w/last", "2026-10-10T00:00:00Z", "2026-10-10T00:00:00Z"), row("/w/old", "2026-10-01T00:00:00Z", "2026-10-01T00:00:00Z")]);
  assert.equal(startingDirectory(null, recent), "/w/last");
  assert.equal(startingDirectory("/w/elsewhere", recent), "/w/elsewhere");
  assert.equal(startingDirectory("   ", recent), "/w/last", "a blank pick is no pick");
  assert.equal(startingDirectory(null, []), "", "a device with no sessions asks");
});

test("the list shows six directories, then all; the pick is on screen wherever it is", () => {
  const recent = recentDirectories(
    Array.from({ length: 9 }, (_, i) => row(`/w/${i}`, `2026-10-${String(19 - i).padStart(2, "0")}T00:00:00Z`, "2026-10-19T00:00:00Z")),
    Infinity,
  );
  assert.equal(SHOWN_DIRECTORIES, 6);
  const collapsed = directoryRows(recent, "/w/0", false);
  assert.deepEqual(collapsed.rows.map((d) => d.cwd), ["/w/0", "/w/1", "/w/2", "/w/3", "/w/4", "/w/5"]);
  assert.equal(collapsed.hidden, 3, "what 「显示全部」 adds");
  assert.equal(collapsed.other, null);
  assert.equal(directoryRows(recent, "/w/0", true).rows.length, 9);
  assert.equal(directoryRows(recent, "/w/0", true).hidden, 0);
  assert.deepEqual(directoryRows(recent, "/w/7", false).rows.map((d) => d.cwd).slice(-2), ["/w/5", "/w/7"], "a pick further down joins the end");
  const typed = directoryRows(recent, "/srv/new", false);
  assert.equal(typed.other, "/srv/new", "a path no session ran in is a row of its own");
  assert.equal(typed.rows.length, 6);
  assert.deepEqual(directoryRows([], "", false), { rows: [], other: null, hidden: 0 });
});

test("↑/↓ step through the rows on screen and stop at the ends", () => {
  const paths = ["/a", "/b", "/c"];
  assert.equal(stepDirectory(paths, "/a", 1), "/b");
  assert.equal(stepDirectory(paths, "/b", -1), "/a");
  assert.equal(stepDirectory(paths, "/c", 1), null, "no wrapping round");
  assert.equal(stepDirectory(paths, "/a", -1), null);
  assert.equal(stepDirectory(paths, "/elsewhere", 1), "/a", "from a pick not on screen, ↓ takes the first");
  assert.equal(stepDirectory(paths, "", -1), "/c", "and ↑ the last");
  assert.equal(stepDirectory([], "/a", 1), null);
});

test("a row reads the directory's name and its parent, home as ~, cut in the middle when narrow", () => {
  assert.deepEqual(directoryLabel("/home/demo/Documents/Codex/2026-10-15/jki", "/home/demo"), { name: "jki", head: "~/Documents/Codex/", tail: "2026-10-15" });
  assert.deepEqual(directoryLabel("/home/demo/project", "/home/demo"), { name: "project", head: "", tail: "~" });
  assert.deepEqual(directoryLabel("/home/demo/project/", "/home/demo"), { name: "project", head: "", tail: "~" }, "a trailing slash is no folder");
  assert.deepEqual(directoryLabel("/srv/app", ""), { name: "app", head: "/", tail: "srv" });
  assert.deepEqual(directoryLabel("/srv", ""), { name: "srv", head: "", tail: "/" });
  assert.deepEqual(directoryLabel("/home/demo", "/home/demo"), { name: "~", head: "", tail: "" });
  assert.deepEqual(directoryLabel("/", ""), { name: "/", head: "", tail: "" });
});

test("a prefill (folder, message, agent) replaces what it names and keeps the rest; the address names one or none", () => {
  const draft: NewSessionDraft = {
    text: "old words", attachments: [{ path: "/u/a.png", name: "a.png", isImage: true }], source: "claude", cwd: "/w/old", account: "acct-1",
    attempt: { key: "{}", clientMessageId: "6f1c2a52-3b7e-4d1f-9a0c-2b8f1e4d5c6a" },
  };
  assert.equal(applyNewSessionPrefill(draft, null), draft);
  assert.deepEqual(applyNewSessionPrefill(draft, { cwd: "/w/new" }), { ...draft, cwd: "/w/new" }, "a folder alone keeps the message");
  assert.deepEqual(
    applyNewSessionPrefill(draft, { text: "Explain this project" }),
    { ...draft, text: "Explain this project", attachments: [], attempt: null },
    "a new message is a new start: its attachments and attempt go",
  );
  assert.deepEqual(applyNewSessionPrefill(draft, { source: "codex" }), { ...draft, source: "codex", account: null }, "the other CLI cannot use this one's account");
  assert.deepEqual(applyNewSessionPrefill(draft, { source: "claude" }), draft);

  assert.equal(prefillFromSearch({}), null);
  assert.deepEqual(prefillFromSearch({ cwd: "/w/a", draft: "hi", agent: "codex" }), { cwd: "/w/a", text: "hi", source: "codex" });
  assert.deepEqual(prefillFromSearch({ draft: "" }), { text: "" }, "an empty message empties the composer");

  const storage = new MemoryStorage();
  prefillNewSessionDraft(storage, LOCAL, { cwd: "/w/practice", text: "Make a one-page site", source: "codex" });
  assert.deepEqual(readNewSessionDraft(storage, LOCAL), { ...EMPTY_NEW_SESSION_DRAFT, cwd: "/w/practice", text: "Make a one-page site", source: "codex" });
});

test("the draft is kept per device, read back whole, and an empty one leaves nothing behind", () => {
  const storage = new MemoryStorage();
  const draft: NewSessionDraft = {
    text: "fix the login page",
    attachments: [{ path: "/u/abc/shot.png", name: "shot.png", isImage: true }, { path: "/u/abc/log.txt", name: "log.txt", isImage: false }],
    source: "codex",
    cwd: "/w/app",
    account: "",
    attempt: { key: "{}", clientMessageId: "6f1c2a52-3b7e-4d1f-9a0c-2b8f1e4d5c6a" },
  };
  writeNewSessionDraft(storage, LOCAL, draft);
  assert.deepEqual(readNewSessionDraft(storage, LOCAL), draft);
  assert.ok(storage.values.has("rh.draft.new-session"), "the local device's, under the key such drafts always had");
  assert.deepEqual(readNewSessionDraft(storage, "d-other"), EMPTY_NEW_SESSION_DRAFT, "another device has its own");

  clearNewSessionDraft(storage, LOCAL);
  assert.deepEqual(readNewSessionDraft(storage, LOCAL), { ...draft, text: "", attachments: [], attempt: null }, "a start spends the message, not the choices");

  writeNewSessionDraft(storage, LOCAL, EMPTY_NEW_SESSION_DRAFT);
  assert.equal(storage.values.size, 0);
});

test("a draft that is not one is read as empty, and anything malformed in one is dropped", () => {
  const storage = new MemoryStorage();
  storage.setItem("rh.draft.new-session", "{not json");
  assert.deepEqual(readNewSessionDraft(storage, LOCAL), EMPTY_NEW_SESSION_DRAFT);
  storage.setItem("rh.draft.new-session", JSON.stringify({ text: 7, source: "gemini", attachments: [{ path: "", name: "x", isImage: true }, { path: "/u/a", name: "a", isImage: "yes" }, null], attempt: { key: 1 } }));
  assert.deepEqual(readNewSessionDraft(storage, LOCAL), EMPTY_NEW_SESSION_DRAFT);
});

test("Cmd+Shift+O (Ctrl+Shift+O elsewhere) opens a new session everywhere, Cmd+N (Ctrl+N) only in the desktop app", () => {
  const key = (over: Partial<Parameters<typeof isNewSessionChord>[0]>) => ({ key: "n", metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...over });
  const o = (over: Partial<Parameters<typeof isNewSessionChord>[0]>) => key({ key: "O", shiftKey: true, ...over });
  // The browser-safe chord, in a browser and in the app.
  for (const desktop of [false, true]) {
    assert.equal(isNewSessionChord(o({ metaKey: true }), true, desktop), true);
    assert.equal(isNewSessionChord(o({ ctrlKey: true }), false, desktop), true);
    assert.equal(isNewSessionChord(o({ ctrlKey: true }), true, desktop), false, "Ctrl on a Mac is the text fields'");
    assert.equal(isNewSessionChord(o({ metaKey: true, altKey: true }), true, desktop), false);
    assert.equal(isNewSessionChord(o({ key: "щ", code: "KeyO", metaKey: true }), true, desktop), true, "the physical key on another layout");
    assert.equal(isNewSessionChord(key({ key: "o", metaKey: true }), true, desktop), false, "Cmd+O without Shift is not it");
  }
  // Cmd+N: a browser keeps it for a new window, the desktop app's window hands it over.
  assert.equal(isNewSessionChord(key({ metaKey: true }), true, false), false);
  assert.equal(isNewSessionChord(key({ ctrlKey: true }), false, false), false);
  assert.equal(isNewSessionChord(key({ metaKey: true }), true, true), true);
  assert.equal(isNewSessionChord(key({ ctrlKey: true }), false, true), true);
  assert.equal(isNewSessionChord(key({ key: "N", metaKey: true }), true, true), true, "caps lock");
  assert.equal(isNewSessionChord(key({ ctrlKey: true }), true, true), false, "Ctrl+N on a Mac is the next line");
  assert.equal(isNewSessionChord(key({ metaKey: true, shiftKey: true }), true, true), false, "Cmd+Shift+N is the incognito window");
  assert.equal(isNewSessionChord(key({ metaKey: true, isComposing: true }), true, true), false);
  assert.equal(isNewSessionChord(key({ key: "d", metaKey: true }), true, true), false, "Cmd+D stays the split");
  assert.equal(isNewSessionChord(key({ key: "т", code: "KeyN", metaKey: true }), true, true), true, "the physical key on another layout");
});

test("the hint names the chord that works where the page runs", () => {
  assert.equal(newSessionShortcut(true, false), "⇧⌘O");
  assert.equal(newSessionShortcut(false, false), "Ctrl+Shift+O");
  assert.equal(newSessionShortcut(true, true), "⌘N");
  assert.equal(newSessionShortcut(false, true), "Ctrl+N");
});

const start: NewConversation = { kind: "session", source: "claude", cwd: "/w", text: "look", model: "fable", effort: null, permission: "acceptEdits", accountId: null };

test("the first message's attachments go in the start request as a message's do, and only when there are any", () => {
  const body = creationRequest({ ...start, images: ["/u/a.png"], files: ["/u/b.txt"] }, "id-1").body;
  assert.deepEqual(body, {
    source: "claude", cwd: "/w", text: "look", model: "fable", permission: "acceptEdits", accountId: null,
    images: ["/u/a.png"], files: ["/u/b.txt"], clientMessageId: "id-1",
  });
  const bare = creationRequest({ ...start, images: [], files: [] }).body;
  assert.equal("images" in bare || "files" in bare, false, "nothing attached is the body an older server always took");
});

test("a retry of the same start keeps its message id; one with other attachments is another start", () => {
  let n = 0;
  const mint = () => `id-${++n}`;
  const first = creationAttempt(null, { ...start, images: ["/u/a.png"] }, mint);
  assert.equal(creationAttempt(first, { ...start, images: ["/u/a.png"] }, mint), first);
  assert.notEqual(creationAttempt(first, { ...start, images: ["/u/b.png"] }, mint).clientMessageId, first.clientMessageId);
});

test("a session being started is pane content on its device, like the session it becomes", () => {
  const content: PaneContent = { kind: "new-session" };
  assert.equal(onMachine(content), true);
  assert.equal(usesMachine(content), true, "the pane holds its device while it is shown");
  assert.deepEqual(withMachine(content, "d-b", "d-a"), { kind: "new-session", machine: "d-b" });
  assert.deepEqual(withMachine({ kind: "new-session", machine: "d-b" }, "d-a", "d-a"), { kind: "new-session" });
  assert.equal(paneMachine({ kind: "new-session", machine: "d-b" }, "d-a"), "d-b");
});

test("an attachment no longer on the device reads as that, in words, wherever a send or a start is refused", async () => {
  const { ApiError, ATTACHMENT_MISSING, errorText } = await import("../src/api/errors.js");
  const refused = new ApiError({ error: ATTACHMENT_MISSING, hint: "an attachment is no longer on this device; attach it again" });
  assert.equal(errorText(refused), "an attachment is no longer on this device; attach it again");
  assert.equal(errorText(new ApiError({ error: "attachment paths must come from /api/uploads" })), "attachment paths must come from /api/uploads", "other refusals as they were");
});
