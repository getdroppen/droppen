// The facts the session header keeps behind its details chip.
//
// The header shows a count and nothing else; everything under test here is what
// the popover has to be able to print, including the states the old one-line
// header rendered as `Invalid Date` or ` · · ` because it interpolated whatever
// the server sent.
import assert from "node:assert/strict";
import test from "node:test";
import { sessionMetaFacts } from "../src/features/sessions/session-meta.js";

test("sessionMetaFacts formats the session's reference facts", () => {
  const facts = sessionMetaFacts({
    cwd: "/Users/ada/Projects/remote-hand",
    updatedAt: "2026-09-02T08:11:00Z",
    sizeBytes: 3_460_000,
    totalItems: 674,
    omittedItems: 474,
  });
  assert.equal(facts.cwd, "/Users/ada/Projects/remote-hand");
  assert.equal(facts.size, "3.3 MB");
  assert.equal(facts.items, 674);
  assert.equal(facts.omitted, 474);
  // Local-time formatting is the platform's; all this asserts is that a
  // usable stamp produces something to print.
  assert.notEqual(facts.updated, "");
});

test("sessionMetaFacts reports missing facts as absent, not as junk", () => {
  const facts = sessionMetaFacts({
    cwd: null,
    updatedAt: null,
    sizeBytes: null,
    totalItems: 0,
    omittedItems: 0,
  });
  assert.equal(facts.cwd, null);
  assert.equal(facts.updated, "");
  assert.equal(facts.size, "");
  assert.equal(facts.items, 0);
  assert.equal(facts.omitted, 0);
});

test("sessionMetaFacts treats an empty cwd as no cwd", () => {
  // A session with no project reports "" rather than null over the wire, and an
  // empty "working directory" block is worse than none.
  assert.equal(sessionMetaFacts({ cwd: "", updatedAt: null, sizeBytes: null, totalItems: 1, omittedItems: 0 }).cwd, null);
  assert.equal(
    sessionMetaFacts({ updatedAt: null, sizeBytes: null, totalItems: 1, omittedItems: 0 }).cwd,
    null,
  );
});

test("sessionMetaFacts never reports a negative count", () => {
  // The paging window can hold a delta that arrived before the page explaining
  // it, which makes `startIndex` — and so `omittedItems` — briefly negative.
  const facts = sessionMetaFacts({
    cwd: "/tmp/x",
    updatedAt: "nonsense",
    sizeBytes: 0,
    totalItems: -3,
    omittedItems: -12,
  });
  assert.equal(facts.items, 0);
  assert.equal(facts.omitted, 0);
  // An unparseable stamp prints nothing rather than "Invalid Date".
  assert.equal(facts.updated, "");
  assert.equal(facts.size, "0 B");
});
