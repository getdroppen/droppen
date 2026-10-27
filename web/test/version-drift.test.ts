import assert from "node:assert/strict";
import test from "node:test";
import { liveVersion, NO_DRIFT, observeVersion, type VersionDriftState } from "../src/app/version-drift.js";

function fold(...versions: (string | null | undefined)[]): VersionDriftState {
  return versions.reduce<VersionDriftState>(observeVersion, NO_DRIFT);
}

test("the first version seen becomes the baseline without drifting", () => {
  assert.deepEqual(fold("0.1.0+gaaaaaaa"), { seen: "0.1.0+gaaaaaaa", drifted: false });
});

test("reconnecting to the same build is not drift", () => {
  assert.equal(fold("0.1.0+gaaaaaaa", "0.1.0+gaaaaaaa", "0.1.0+gaaaaaaa").drifted, false);
});

test("a different build after the baseline is drift", () => {
  assert.equal(fold("0.1.0+gaaaaaaa", "0.1.0+gbbbbbbb").drifted, true);
});

test("drift latches: it survives later frames, including a return to the baseline", () => {
  const drifted = fold("0.1.0+gaaaaaaa", "0.1.0+gbbbbbbb", "0.1.0+gaaaaaaa", "0.2.0");
  assert.equal(drifted.drifted, true);
  assert.equal(drifted.seen, "0.1.0+gaaaaaaa");
});

test("an unknown version on either side cannot prove drift", () => {
  assert.equal(fold(undefined, undefined).drifted, false);
  assert.equal(fold("0.1.0", undefined, "").drifted, false);
  // A server that only starts reporting a version mid-session sets the baseline
  // then, rather than counting the silent frames as a different build.
  assert.deepEqual(fold(undefined, "0.1.0"), { seen: "0.1.0", drifted: false });
});

test("observing an already-known version is a no-op, so repeated effects are safe", () => {
  const base = fold("0.1.0");
  assert.equal(observeVersion(base, "0.1.0"), base);
  assert.equal(observeVersion(base, undefined), base);
});

test("the offline copy's version is not the machine's, so it neither sets the baseline nor drifts", () => {
  // A cloud page: the copy plays the build the machine last wrote to its status, then the channel opens.
  const copy = { serverVersion: "0.2.126+gaaaaaaa", fromCopy: true };
  const live = { serverVersion: "0.2.133+gbbbbbbb" };
  assert.equal(liveVersion(copy), undefined);
  assert.equal(liveVersion(live), "0.2.133+gbbbbbbb");
  assert.equal(liveVersion(null), undefined);
  assert.deepEqual(fold(liveVersion(copy), liveVersion(live)), { seen: "0.2.133+gbbbbbbb", drifted: false });
  // Back on the copy while the machine restarts, then up on a new build: that one is drift.
  assert.equal(fold(liveVersion(copy), liveVersion(live), liveVersion(copy), "0.2.134+gccccccc").drifted, true);
});
