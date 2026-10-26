/*
 * The offline copy's story, frame by frame: the computer goes off first; then
 * one page at a time rides sealed (a padlocked packet, no words) from the
 * encrypted copy through Droppen Hub to the phone, Droppen Hub lit exactly
 * while it passes one on; a row of the phone's transcript is fresh only once
 * its page has arrived, and before that still reads, dimmed — never blank,
 * never ciphertext.
 */
import assert from "node:assert/strict";
import test from "node:test";
import { OFFLINE_LOOP, OFFLINE_STARTS, offlineFrame, offlineStill } from "../src/features/landing/offline-copy.js";
import { morph } from "../src/features/landing/motion.js";

const STEP = 0.02;

test("offline copy: the computer goes off, then each page rides sealed through Droppen Hub to the phone, and its row is fresh only once it arrives", () => {
  const arrived = new Set<number>();
  let before = -1;
  for (let i = 0; i * STEP < 2 * OFFLINE_LOOP; i++) {
    const t = (i * STEP) % OFFLINE_LOOP;
    if (t < before) arrived.clear();
    before = t;
    const f = offlineFrame(t);
    const at = `offline at ${t.toFixed(2)}s`;
    // One page on the way at a time, and only once the computer is off.
    assert.ok(f.chips.length <= 1, `${at}: two pages at once`);
    if (f.chips.length > 0) assert.ok(f.off, `${at}: a page fetched while the computer is on`);
    // Droppen Hub is lit exactly while it passes a page on.
    assert.equal(f.passing, f.chips.length > 0, at);
    for (const chip of f.chips) if (chip.wire === "hubDevice" && chip.at > 0.95) arrived.add(chip.msg);
    f.rows.forEach((row, r) => {
      // Never ciphertext on the phone: the session reads as it was.
      assert.equal(row.kind, "plain", `${at}: row ${r} is ${row.kind}`);
      if (!row.dim) assert.ok(arrived.has(r) || t >= OFFLINE_STARTS[r]! + 1.2, `${at}: row ${r} fresh before its page arrived`);
    });
  }
  const still = offlineStill();
  assert.deepEqual(still.rows.map((r) => [r.kind, r.dim ?? false]), [["plain", false], ["plain", false]]);
  assert.equal(still.chips.length, 0);
});

test("sealing and opening turn one value into the other in place, never through a blank", () => {
  for (const [from, to] of [["sign-in fixed", "5c1e 88d0 a3f7"], ["登录 bug 已修复", "5c1e 88d0 a3f7"], ["2930 bf24 1db9", "テスト全件通過"]] as const) {
    assert.equal(morph(from, to, 0), from);
    assert.equal(morph(from, to, 1), to);
    for (let p = 0.05; p < 1; p += 0.05) {
      const mid = morph(from, to, p, 3);
      assert.ok(mid.trim().length > 0);
      assert.ok([...mid].length <= Math.max([...from].length, [...to].length));
    }
  }
});
