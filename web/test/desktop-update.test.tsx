// The update status the app's bridge hands the page (`app/desktop-update.tsx`):
// read defensively, and the rail's mark is only what it should be — ready
// until that version is put away, downloading, or a failure worth saying.
import assert from "node:assert/strict";
import test from "node:test";
import {
  parseUpdateStatus,
  updateBadge,
  updateVersion,
  type UpdateStatus,
} from "../src/app/desktop-update.js";

const base: UpdateStatus = {
  current: "0.2.130",
  state: "idle",
  release: null,
  percent: null,
  place: "updatable",
  lastChecked: null,
  problem: null,
  notes: null,
};

const release = (version = "0.2.131") => ({ version, notes: "https://github.com/getdroppen/droppen/releases/tag/v0.2.131" });

test("a status is read down to the shapes the page draws", () => {
  const ready = parseUpdateStatus({
    current: "0.2.130",
    state: "ready",
    release: release(),
    place: "updatable",
    lastChecked: "2026-10-25T01:00:00Z",
    problem: null,
    notes: { version: "0.2.131", english: "## a", chinese: "## 甲" },
  });
  assert.deepEqual(ready, {
    current: "0.2.130",
    state: "ready",
    release: release(),
    percent: null,
    place: "updatable",
    lastChecked: "2026-10-25T01:00:00Z",
    problem: null,
    notes: { version: "0.2.131", english: "## a", chinese: "## 甲" },
  });
  const downloading = parseUpdateStatus({ current: "0.2.130", state: "downloading", release: release(), percent: 42.6 });
  assert.equal(downloading?.percent, 43);
  // Whatever else the bridge might hold: nothing to draw.
  for (const odd of [undefined, null, "ready", 42, {}, { state: "ready" }, { current: "v0.2.130" }, { current: "<b>" }]) {
    assert.equal(parseUpdateStatus(odd), null, JSON.stringify(odd));
  }
  const odd = parseUpdateStatus({ current: "0.2.130", state: "what", release: { version: "v0.2.131" }, percent: -3, place: "x", problem: "nope" });
  assert.equal(odd?.state, "idle");
  assert.equal(odd?.release, null);
  assert.equal(odd?.percent, null);
  assert.equal(odd?.place, "notABundle");
  assert.equal(odd?.problem, null);
});

test("the page is about the release under way, else this version", () => {
  assert.equal(updateVersion({ ...base, state: "ready", release: release() }), "0.2.131");
  assert.equal(updateVersion({ ...base, state: "downloading", release: release() }), "0.2.131");
  assert.equal(updateVersion({ ...base, state: "idle", release: release() }), "0.2.130");
  assert.equal(updateVersion({ ...base, state: "checking" }), "0.2.130");
});

test("the rail marks a ready update until it is put away, a download, and a failure worth saying", () => {
  assert.equal(updateBadge({ ...base, state: "ready", release: release() }, null), "ready");
  assert.equal(updateBadge({ ...base, state: "ready", release: release() }, "0.2.131"), null, "put away");
  assert.equal(updateBadge({ ...base, state: "ready", release: release("0.2.132") }, "0.2.131"), "ready", "a newer one shows again");
  assert.equal(updateBadge({ ...base, state: "downloading", release: release() }, null), "downloading");
  for (const state of ["idle", "checking", "installing", "restarting"] as const) {
    assert.equal(updateBadge({ ...base, state, release: release() }, null), null, state);
  }
  // A network or service hiccup is retried silently; these three are said.
  for (const problem of ["offline", "service", "other"] as const) {
    assert.equal(updateBadge({ ...base, problem }, null), null, problem);
  }
  for (const problem of ["signature", "location", "install"] as const) {
    assert.equal(updateBadge({ ...base, problem }, null), "problem", problem);
  }
  assert.equal(updateBadge(null, null), null);
});
