import assert from "node:assert/strict";
import test from "node:test";
import {
  readSessionDraftSnapshot,
  sessionDraftKey,
  subscribeSessionDraft,
  writeSessionDraft,
  type DraftStorage,
} from "../src/features/sessions/session-draft.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { MemoryStorage } from "./storage.js";

test("drafts are isolated by session and empty drafts remove their key", () => {
  const storage = new MemoryStorage();
  const first = sessionDraftKey({ machine: LOCAL, source: "codex", id: "one" });
  const second = sessionDraftKey({ machine: LOCAL, source: "codex", id: "two" });

  writeSessionDraft(storage, first, "first draft");
  writeSessionDraft(storage, second, "second draft");

  assert.equal(readSessionDraftSnapshot(storage, first).text, "first draft");
  assert.equal(readSessionDraftSnapshot(storage, second).text, "second draft");

  writeSessionDraft(storage, first, "");
  assert.equal(readSessionDraftSnapshot(storage, first).text, "");
  assert.equal(storage.values.has(first), false);
});

test("unavailable storage falls back without throwing", () => {
  const unavailable: DraftStorage = {
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

  assert.equal(readSessionDraftSnapshot(unavailable, "draft").text, "");
  assert.doesNotThrow(() => writeSessionDraft(unavailable, "draft", "text"));
  assert.doesNotThrow(() => writeSessionDraft(unavailable, "draft", ""));
});

test("a write is heard by its session's subscribers, and only its own", () => {
  const storage = new MemoryStorage();
  const key = sessionDraftKey({ machine: LOCAL, source: "claude", id: "heard" });
  const other = sessionDraftKey({ machine: LOCAL, source: "claude", id: "other" });
  const states: Array<{ text: string }> = [];
  const unsubscribe = subscribeSessionDraft(key, (snapshot) => {
    states.push(snapshot);
  });
  writeSessionDraft(storage, other, "not this one");
  writeSessionDraft(storage, key, "pending");
  writeSessionDraft(storage, key, "");
  unsubscribe();
  writeSessionDraft(storage, key, "after unsubscribe");
  assert.deepEqual(states, [{ text: "pending" }, { text: "" }]);
});
