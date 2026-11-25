import assert from "node:assert/strict";
import test from "node:test";
import {
  clearQuestionDraft,
  questionDraftKey,
  readQuestionDraft,
  writeQuestionDraft,
} from "../src/features/sessions/question-draft.js";
import { EMPTY_DRAFT, togglePick, typeOther } from "../src/features/sessions/session-questions.js";
import type { DraftStorage } from "../src/features/sessions/session-draft.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { MemoryStorage } from "./storage.js";

const session = { source: "claude", id: "s1" };
const key = questionDraftKey(LOCAL, session, "toolu_1");

test("a card's draft is kept, and is its own per call and per session", () => {
  const storage = new MemoryStorage();
  const sibling = questionDraftKey(LOCAL, session, "toolu_2");
  const elsewhere = questionDraftKey(LOCAL, { source: "claude", id: "s2" }, "toolu_1");

  const draft = typeOther(togglePick(EMPTY_DRAFT, 0, 1, true), 1, "no cheese", false);
  writeQuestionDraft(storage, key, draft);
  writeQuestionDraft(storage, sibling, typeOther(EMPTY_DRAFT, 0, "other card", false));

  const back = readQuestionDraft(storage, key);
  assert.deepEqual(back.other, draft.other);
  assert.deepEqual(back.picked[0], [1]);
  // Empty per-question entries are not stored; they read back as none.
  assert.equal(back.picked[1], undefined);
  assert.deepEqual(readQuestionDraft(storage, sibling), { picked: {}, other: { 0: "other card" } });
  assert.deepEqual(readQuestionDraft(storage, elsewhere), EMPTY_DRAFT);
  // The same call on two machines does not meet either.
  assert.notEqual(questionDraftKey("m2", session, "toolu_1"), key);
});

test("a draft with nothing picked or typed removes its key instead of storing an empty one", () => {
  const storage = new MemoryStorage();
  writeQuestionDraft(storage, key, typeOther(EMPTY_DRAFT, 0, "typed", false));
  assert.equal(storage.values.has(key), true);

  writeQuestionDraft(storage, key, typeOther(EMPTY_DRAFT, 0, "", false));
  assert.equal(storage.values.has(key), false);
});

test("reading an unknown, malformed or foreign value is an empty draft", () => {
  const storage = new MemoryStorage({
    "plain": "plain text",
    "json": JSON.stringify({ picked: {} }),
    "wrong-version": JSON.stringify({ version: 2, picked: {}, other: {} }),
    "bad-index": JSON.stringify({ version: 1, picked: { x: [1] }, other: { 0: 2 } }),
  });
  for (const name of ["plain", "json", "wrong-version", "bad-index", "missing"]) {
    assert.deepEqual(readQuestionDraft(storage, name), EMPTY_DRAFT, name);
  }
});

test("clearing removes the key, and unavailable storage never throws", () => {
  const storage = new MemoryStorage();
  writeQuestionDraft(storage, key, typeOther(EMPTY_DRAFT, 0, "typed", false));
  clearQuestionDraft(storage, key);
  assert.equal(storage.values.has(key), false);

  const blocked: DraftStorage = {
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
  assert.deepEqual(readQuestionDraft(blocked, "k"), EMPTY_DRAFT);
  assert.doesNotThrow(() => writeQuestionDraft(blocked, "k", typeOther(EMPTY_DRAFT, 0, "x", false)));
  assert.doesNotThrow(() => clearQuestionDraft(blocked, "k"));
});
