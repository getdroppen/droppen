/*
 * The landing diagrams tell their stories frame by frame, and these check
 * them.
 *
 * The hero (`hero-agents.tsx`): what a computer's session does reaches the
 * phone only with a packet down its wire, and what the phone answers reaches
 * the computer only with a packet up; the phone's composer is the question
 * exactly while the phone shows that session asking; and the loop ends where
 * it starts.
 *
 * The offline copy: a message is one fixed pair (plaintext P, ciphertext C);
 * the very same C is the chip on the wire and the value in Droppen Hub; the
 * receiver opens that C into that P. One message moves at a time: a card
 * still holding the one before shows it dimmed, never blank, and never a
 * value that is not the message in flight or the one just delivered.
 */
import assert from "node:assert/strict";
import test from "node:test";
import { AGENTS_LOOP, AGENTS_T, agentsFrame, agentsStill, type AgentsFrame } from "../src/features/landing/hero-agents.js";
import { OFFLINE_CIPHERS, OFFLINE_LOOP, OFFLINE_STARTS, offlineFrame, offlineStill } from "../src/features/landing/offline-copy.js";
import { chipLabel, currentMessage, type Message, type Shown, textOf } from "../src/features/landing/story.js";
import { morph } from "../src/features/landing/motion.js";

const STEP = 0.02;

/** Times over two loops; only the second is checked (the first one delivers what the second starts from). */
function* samples(loop: number): Generator<{ t: number; round: number; checked: boolean }> {
  for (let i = 0; i * STEP < 2 * loop; i++) {
    const abs = i * STEP;
    yield { t: abs % loop, round: Math.floor(abs / loop), checked: abs >= loop };
  }
}

const isCipher = (s: Shown) => s.kind !== "plain";

/**
 * The rules, for one diagram: `cards(t)` names every row on screen; `sender`
 * and `receivers` are the rows that seal and open; `chips` what is on a wire.
 */
function checkStory({
  name,
  loop,
  starts,
  messages,
  frame,
  sender,
  middle,
  receivers,
}: {
  name: string;
  loop: number;
  starts: readonly number[];
  messages: Message[];
  frame: (t: number, round: number) => { chips: { msg: number; wire: string; at: number }[] } & Record<string, unknown>;
  sender?: (f: ReturnType<typeof frame>) => Shown;
  middle: (f: ReturnType<typeof frame>) => Shown[];
  receivers: (f: ReturnType<typeof frame>) => Shown[];
}) {
  /** Messages whose ciphertext the receiver has shown (so their plaintext may stand there undimmed). */
  const opened = new Set<number>();
  /** Messages the sender has sealed this time round (a chip may only leave after). */
  const sealed = new Set<number>();
  let before = -1;
  for (const { t, round, checked } of samples(loop)) {
    const f = frame(t, round);
    const current = currentMessage(starts, t);
    // A message's turn starts (again, the next time round): it has to be sealed, sent and opened anew.
    const crossed = starts.some((start) => (before <= t ? before < start && start <= t : before < start || start <= t));
    if (before >= 0 && crossed) {
      sealed.delete(current);
      opened.delete(current);
    }
    before = t;
    const at = `${name} at ${t.toFixed(2)}s`;
    const s = sender?.(f);
    const all = [...(s ? [s] : []), ...middle(f), ...receivers(f)];
    for (const shown of all) {
      // Never blank, and the text is exactly the message's P or C (or one turning into the other).
      const text = textOf(shown, messages, Math.floor(t * 14));
      assert.ok(text.trim().length > 0, `${at}: a blank row`);
      const m = messages[shown.msg]!;
      if (shown.kind === "cipher") assert.equal(text, m.c, at);
      if (shown.kind === "plain" && shown.typed === undefined) assert.equal(text, m.p, at);
    }
    if (s) {
      assert.notEqual(s.kind, "open", `${at}: the sender never opens`);
      if (s.kind === "cipher" || (s.kind === "seal" && (s.p ?? 0) >= 1)) sealed.add(s.msg);
    }
    for (const shown of middle(f)) {
      // The card in the middle (the Droppen relay, or Droppen Hub handing out sealed pages) only ever holds ciphertext, and changes nothing.
      assert.equal(shown.kind, "cipher", `${at}: the middle card shows ciphertext only`);
      assert.equal(shown.lock, undefined, `${at}: nothing is sealed or opened in the middle`);
    }
    for (const shown of receivers(f)) {
      assert.notEqual(shown.kind, "seal", `${at}: the receiver never seals`);
      if (shown.kind === "cipher" || shown.kind === "open") opened.add(shown.msg);
    }
    if (!checked) continue;
    // Every ciphertext on screen that is not dimmed is the message in flight's.
    for (const shown of all) {
      if (!shown.dim && isCipher(shown)) assert.equal(shown.msg, current, `${at}: undimmed ciphertext of message ${shown.msg}, but ${current} has the turn`);
    }
    // Every chip on a wire is the message in flight's ciphertext, and leaves only once the sender sealed it.
    for (const chip of f.chips) {
      assert.equal(chip.msg, current, `${at}: a chip of message ${chip.msg} on ${chip.wire}, but ${current} has the turn`);
      assert.equal(chipLabel(messages[chip.msg]!.c), messages[current]!.c.slice(0, 4), at);
      if (sender) assert.ok(sealed.has(chip.msg), `${at}: a chip of message ${chip.msg} left before it was sealed`);
    }
    // Plaintext on the receiver, undimmed, is a delivered message whose ciphertext it showed and opened.
    for (const shown of receivers(f)) {
      if (shown.kind === "plain" && !shown.dim) assert.ok(opened.has(shown.msg), `${at}: plaintext of message ${shown.msg} the receiver never opened`);
    }
  }
}

test("hero: a computer's news reaches the phone only down its wire, and the phone's answer reaches the computer only up it", () => {
  // What each end knew last, and the packet each wire last carried, to see every change follow its packet.
  let last: AgentsFrame | null = null;
  const carried: Record<"mac" | "linux", { up: boolean; at: number } | null> = { mac: null, linux: null };
  let pressedBeforeUp = { mac: false, linux: false };
  for (const { t, checked } of samples(AGENTS_LOOP)) {
    const f = agentsFrame(t);
    const at = `hero at ${t.toFixed(2)}s`;
    // One packet per wire at a time.
    for (const wire of ["mac", "linux"] as const) assert.ok(f.chips.filter((c) => c.wire === wire).length <= 1, `${at}: two packets on the ${wire} wire`);
    // The composer is the question exactly while the phone shows the MacBook's session asking.
    assert.equal(f.panel === "question", f.phoneMac === "asking", at);
    // A button is held only on the face that has it, and the prompt is typed only into its composer.
    if (f.pressed) assert.notEqual(f.panel, "idle", at);
    if (f.typed > 0) assert.equal(f.panel, "prompt", at);
    if (f.pressed && f.panel === "question") pressedBeforeUp.mac = true;
    if (f.pressed && f.panel === "prompt") pressedBeforeUp.linux = true;
    if (checked && last) {
      for (const [wire, computer, phone] of [
        ["mac", "mac", "phoneMac"],
        ["linux", "linux", "phoneLinux"],
      ] as const) {
        const c = carried[wire];
        // The phone changes only as a packet down this wire arrives; the computer only as a packet up arrives.
        if (f[phone] !== last[phone] && f[phone] !== "working") assert.ok(c && !c.up && c.at > 0.9, `${at}: the phone's ${wire} row changed with no packet down`);
        if (f[computer] === "working" && last[computer] !== "working") assert.ok(c && c.up && c.at > 0.9, `${at}: the ${wire} computer went back to work with no packet up`);
        // And a packet up leaves only once its button was pressed.
        const chip = f.chips.find((x) => x.wire === wire);
        if (chip?.up && !carried[wire]?.up) assert.ok(pressedBeforeUp[wire], `${at}: an answer left the phone with no button pressed`);
      }
    }
    for (const wire of ["mac", "linux"] as const) {
      const chip = f.chips.find((c) => c.wire === wire);
      if (chip) carried[wire] = { up: chip.up, at: chip.at };
      if (chip?.up === false) pressedBeforeUp = { ...pressedBeforeUp, [wire]: false };
    }
    last = f;
  }
});

test("hero: the loop ends where it starts, and its first frame is the whole story — one session asking, one done, the question waiting", () => {
  const first = agentsFrame(0);
  const end = agentsFrame(AGENTS_LOOP - 0.001);
  for (const key of ["mac", "phoneMac", "linux", "phoneLinux", "panel"] as const) assert.equal(end[key], first[key], key);
  assert.deepEqual(agentsStill(), first);
  assert.deepEqual([first.mac, first.phoneMac, first.linux, first.phoneLinux, first.panel, first.chips.length], ["asking", "asking", "done", "done", "question", 0]);
  // In between, every session is back at work at least once, and the next prompt is typed out whole before it is sent.
  const mid = agentsFrame(AGENTS_T.sendUp[1] + 0.1);
  assert.deepEqual([mid.mac, mid.linux, mid.phoneMac, mid.phoneLinux, mid.panel], ["working", "working", "working", "working", "idle"]);
  assert.equal(agentsFrame(AGENTS_T.send).typed, 1);
});

test("offline copy: each page's ciphertext rides to Droppen Hub and the phone, and opens there into its line", () => {
  const messages: Message[] = [
    { p: "make it ink", c: OFFLINE_CIPHERS[0] },
    { p: "done, tests pass", c: OFFLINE_CIPHERS[1] },
  ];
  checkStory({
    name: "offline",
    loop: OFFLINE_LOOP,
    starts: OFFLINE_STARTS,
    messages,
    frame: (t) => offlineFrame(t) as never,
    middle: (f) => [(f as unknown as ReturnType<typeof offlineFrame>).hub],
    receivers: (f) => (f as unknown as ReturnType<typeof offlineFrame>).rows,
  });
  const still = offlineStill();
  assert.deepEqual(still.rows.map((r) => [r.kind, r.dim ?? false]), [["plain", false], ["plain", false]]);
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
