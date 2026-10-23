// The browser key move end to end in one process (`lib/move`): push and
// pull between two origins of a fake browser (`move-world.ts`), and what
// must never happen — keys handed to another site, keys planted by one, a
// copy forgotten before the app host signed in with it, a move cut off that
// loses anything or cannot be finished. The same flows run in Chromium,
// Firefox and WebKit on two real hosts in `e2e/move/`.
import assert from "node:assert/strict";
import test from "node:test";
import { hex } from "../src/lib/e2e/bytes.js";
import { readDeviceKeys } from "../src/lib/e2e/keystore.js";
import { SEEN_DB, SEEN_STORE } from "../src/lib/hub/seen-store.js";
import { servePullSender, servePushReceiver, startMove, startPull, type PullSender, type ReceiverView, type SenderView } from "../src/lib/move/flows.js";
import { MSG } from "../src/lib/move/protocol.js";
import { MOVE_MARKER, readMarker } from "../src/lib/move/receiver.js";
import { ACCOUNT, APP, DEVICE, EVIL, LEGACY, ORIGINS, seedDevice, tick, until, World } from "./move-world.js";

function setUp(opts: { autoConfirm?: boolean } = {}) {
  const w = new World();
  const views = { receiver: [] as ReceiverView[], sender: [] as SenderView[] };
  const pull: { sender: PullSender | null } = { sender: null };
  w.pages.set(`${APP}/move`, (env) => void servePushReceiver({ origins: ORIGINS, env, onView: (v) => views.receiver.push(v) }));
  w.pages.set(`${LEGACY}/move`, (env) => {
    pull.sender = servePullSender({
      origins: ORIGINS,
      env,
      onView: (v) => {
        views.sender.push(v);
        if (v.step === "confirm" && opts.autoConfirm !== false) queueMicrotask(() => pull.sender?.confirm());
      },
    });
  });
  const legacyTab = w.window(`${LEGACY}/`);
  const appTab = w.window(`${APP}/`);
  return { w, views, pull, legacyTab, appTab, legacy: w.site(LEGACY), app: w.site(APP) };
}

/** Everything an origin holds, to compare before and after. */
function stateOf(site: ReturnType<World["site"]>) {
  return {
    keys: site.keyNames(),
    seen: [...(site.records(SEEN_DB, SEEN_STORE).data?.keys() ?? [])],
    local: Object.fromEntries([...site.storage.values.entries()].sort()),
  };
}

async function finish(w: World) {
  w.closeAll();
  await tick(300);
}

test("push: the app host takes the device, signs in with it, and only then does droppen.app forget it", async () => {
  const { w, legacy, app, legacyTab, views } = setUp();
  const { signPublic } = await seedDevice(w);
  const keyNames = legacy.keyNames();
  const steps: string[] = [];
  const out = await startMove({ origins: ORIGINS, env: w.env(legacyTab), onStep: (s) => steps.push(s) });
  assert.deepEqual(out, { kind: "moved", account: ACCOUNT, device: DEVICE, pushWasOn: "host-1", resumed: false, senderWiped: true });
  assert.deepEqual(steps, ["opening", "waiting", "sending", "verifying", "wiping"]);

  // The app host: the same records, keys that open and are the device's, a session of its own.
  assert.deepEqual(app.keyNames(), keyNames);
  const read = await readDeviceKeys(app.keyStore());
  assert.equal(read.kind, "found");
  assert.equal(read.kind === "found" && hex(read.keys.signPublic), signPublic);
  assert.deepEqual([...(app.records(SEEN_DB, SEEN_STORE).data?.keys() ?? [])], [ACCOUNT], "the pinned genesis moved");
  assert.equal(app.storage.getItem(`rh.hub.device.${ACCOUNT}`), DEVICE);
  assert.equal(app.storage.getItem("rh.theme"), "dark");
  assert.equal(app.storage.getItem("rh.draft.note"), null, "not on the list: not moved");
  assert.equal(app.storage.getItem("rh.hub.push.subscribed"), null, "the subscription record never moves");
  assert.equal(app.storage.getItem("rh.hub.push.moved"), "host-1", "only that notifications were on, and from where");
  const session = JSON.parse(app.storage.getItem("rh.hub.session")!);
  assert.equal(session.account, ACCOUNT);
  assert.notEqual(session.token, "old-token", "the old session does not move: the app host signed in afresh");
  assert.equal(readMarker(app.storage)?.state, "verified");
  assert.deepEqual(w.hub.signIns, [{ host: "my.droppen.test", account: ACCOUNT, device: DEVICE }], "signed in naming its own host");

  // droppen.app: nothing of the device left; its old session ended at the hub.
  assert.deepEqual(stateOf(legacy), { keys: [], seen: [], local: { "rh.draft.note": "not moved" } });
  assert.deepEqual(w.hub.loggedOut, ["old-token"]);
  assert.equal(legacy.pushDropped, 1);
  assert.deepEqual(legacy.events, [{ t: "moving" }, { t: "moved", to: APP }]);
  assert.deepEqual(app.events, [{ t: "arrived", account: ACCOUNT }]);
  await until(() => views.receiver.some((v) => v.step === "moved" && v.wiped === true));
  await finish(w);
});

test("pull: my. asks, the droppen.app popup sends on its click, and forgets once my. signed in", async () => {
  const { w, legacy, app, appTab, views } = setUp();
  const { signPublic } = await seedDevice(w);
  const steps: string[] = [];
  const out = await startPull({ origins: ORIGINS, env: w.env(appTab), onStep: (s) => steps.push(s) });
  assert.deepEqual(out, { kind: "moved", account: ACCOUNT, device: DEVICE, pushWasOn: "host-1", resumed: false, senderWiped: true });
  assert.deepEqual(steps, ["opening", "waiting", "confirm", "verifying", "wiping"]);
  const read = await readDeviceKeys(app.keyStore());
  assert.equal(read.kind === "found" && hex(read.keys.signPublic), signPublic);
  assert.deepEqual(stateOf(legacy).keys, []);
  assert.ok(views.sender.some((v) => v.step === "confirm" && v.account === ACCOUNT && v.device === DEVICE), "the popup said which device it would send");
  await until(() => views.sender.some((v) => v.step === "done" && v.outcome.kind === "moved"));
  await finish(w);
});

test("pull: cancelled in the droppen.app popup, nothing moves", async () => {
  const { w, legacy, app, appTab, views, pull } = setUp({ autoConfirm: false });
  await seedDevice(w);
  const before = stateOf(legacy);
  const out = startPull({ origins: ORIGINS, env: w.env(appTab) });
  await until(() => views.sender.some((v) => v.step === "confirm"));
  pull.sender!.cancel();
  assert.deepEqual(await out, { kind: "cancelled" });
  assert.deepEqual(stateOf(legacy), before);
  assert.deepEqual(stateOf(app), { keys: [], seen: [], local: {} });
  await finish(w);
});

test("a blocked popup: nothing happens on either side", async () => {
  const { w, legacy, app, legacyTab, appTab } = setUp();
  await seedDevice(w);
  const before = stateOf(legacy);
  w.blockPopups = true;
  assert.deepEqual(await startMove({ origins: ORIGINS, env: w.env(legacyTab) }), { kind: "popupBlocked" });
  assert.deepEqual(await startPull({ origins: ORIGINS, env: w.env(appTab) }), { kind: "popupBlocked" });
  assert.deepEqual(stateOf(legacy), before);
  assert.deepEqual(stateOf(app), { keys: [], seen: [], local: {} });
  await finish(w);
});

test("a foreign opener of droppen.app/move gets nothing, even asking and with the click", async () => {
  const { w, legacy, views, pull } = setUp();
  await seedDevice(w);
  const before = stateOf(legacy);
  const evilTab = w.window(`${EVIL}/`);
  const evil = w.env(evilTab);
  const popup = evil.open(`${LEGACY}/move`, "x")!;
  const id = "0".repeat(32);
  for (let i = 0; i < 10; i++) {
    popup.postMessage({ v: 1, t: MSG.request, id }, "*");
    popup.postMessage({ v: 1, t: MSG.verified, id, digest: "x", account: ACCOUNT, device: DEVICE }, "*");
    await tick(20);
  }
  pull.sender!.confirm(); // the person is talked into clicking
  await tick(200);
  assert.deepEqual(evilTab.received, [], "nothing reached the other site");
  assert.ok(!views.sender.some((v) => v.step === "confirm" || v.step === "sending"), "it never got as far as asking");
  assert.deepEqual(stateOf(legacy), before);
  await finish(w);
});

test("a foreign opener of my./move can plant nothing", async () => {
  const { w, app, views } = setUp();
  const evilTab = w.window(`${EVIL}/`);
  const popup = w.env(evilTab).open(`${APP}/move`, "x")!;
  await tick(50);
  const bundle = { v: 1, from: LEGACY, account: "attacker", device: "theirs", stores: [], local: [{ key: "rh.hub.device.attacker", value: "theirs" }], pushWasOn: null, digest: "00" };
  for (let i = 0; i < 10; i++) {
    popup.postMessage({ v: 1, t: MSG.bundle, id: "0".repeat(32), bundle }, "*");
    await tick(20);
  }
  assert.deepEqual(evilTab.received, [], "its ready went to droppen.app's origin only: dropped");
  assert.ok(!views.receiver.some((v) => v.step === "verifying"), "nothing was taken in");
  assert.deepEqual(stateOf(app), { keys: [], seen: [], local: {} });
  await finish(w);
});

test("a failed verify: my. takes back what it wrote, droppen.app removes nothing", async () => {
  const { w, legacy, app, legacyTab } = setUp();
  await seedDevice(w);
  const before = stateOf(legacy);
  w.hub.deny = true;
  const out = await startMove({ origins: ORIGINS, env: w.env(legacyTab) });
  assert.equal(out.kind, "verifyFailed");
  assert.match(out.kind === "verifyFailed" ? out.reason : "", /did not sign this device in/);
  assert.deepEqual(stateOf(legacy), before, "everything still at droppen.app");
  assert.deepEqual(w.hub.loggedOut, []);
  assert.deepEqual(stateOf(app), { keys: [], seen: [], local: {} }, "nothing left at my.: no keys, no notes, no session, no marker");
  await finish(w);
});

test("interrupted while my. signs in: nothing removed; the move started again finishes it (resumed)", async () => {
  const { w, legacy, app, legacyTab, views } = setUp();
  const { signPublic } = await seedDevice(w);
  const before = stateOf(legacy);
  let release!: () => void;
  w.hub.gate = new Promise<void>((resolve) => (release = resolve));
  const first = startMove({ origins: ORIGINS, env: w.env(legacyTab) });
  await until(() => views.receiver.some((v) => v.step === "verifying"));
  w.close(w.windows.find((x) => x.origin === APP && x.path === "/move")!);
  assert.deepEqual(await first, { kind: "unfinished", why: "closed" }, "cut off after the keys went: not just closed");
  const noted = stateOf(legacy);
  assert.equal(JSON.parse(noted.local["rh.hub.move.out"]!).state, "sent", "droppen.app noted the move unfinished");
  delete noted.local["rh.hub.move.out"];
  assert.deepEqual(noted, before, "nothing removed at droppen.app");
  // The popup's sign-in still lands (its page was not gone yet): my. holds the device, droppen.app too.
  w.hub.gate = null;
  release();
  await until(() => readMarker(app.storage)?.state === "verified");

  const again = await startMove({ origins: ORIGINS, env: w.env(legacyTab) });
  assert.deepEqual(again, { kind: "moved", account: ACCOUNT, device: DEVICE, pushWasOn: "host-1", resumed: true, senderWiped: true });
  assert.deepEqual(stateOf(legacy).keys, []);
  const read = await readDeviceKeys(app.keyStore());
  assert.equal(read.kind === "found" && hex(read.keys.signPublic), signPublic);
  await finish(w);
});

test("what an unfinished move left at my. (never signed in with) is written over by the next one", async () => {
  const { w, legacy, app, legacyTab } = setUp();
  const { signPublic } = await seedDevice(w);
  // A page that died between its write and its sign-in.
  await app.keyStore().put("device-keys/v2", { half: "written" });
  app.storage.setItem(`rh.hub.device.${ACCOUNT}`, DEVICE);
  app.storage.setItem(MOVE_MARKER, JSON.stringify({ v: 1, state: "writing", account: ACCOUNT, device: DEVICE, from: LEGACY, atMs: 1 }));
  const out = await startMove({ origins: ORIGINS, env: w.env(legacyTab) });
  assert.equal(out.kind, "moved");
  const read = await readDeviceKeys(app.keyStore());
  assert.equal(read.kind === "found" && hex(read.keys.signPublic), signPublic);
  assert.deepEqual(stateOf(legacy).keys, []);
  await finish(w);
});

test("cut off between my.'s two writes (keys written, no device id noted yet): the note stands and the next move writes over it", async () => {
  const { w, legacy, app, legacyTab } = setUp();
  const { signPublic } = await seedDevice(w);
  await app.keyStore().put("device-keys/v2", { half: "written" });
  app.storage.setItem(MOVE_MARKER, JSON.stringify({ v: 1, state: "writing", account: ACCOUNT, device: DEVICE, from: LEGACY, atMs: 1 }));
  const out = await startMove({ origins: ORIGINS, env: w.env(legacyTab) });
  assert.equal(out.kind, "moved", JSON.stringify(out));
  const read = await readDeviceKeys(app.keyStore());
  assert.equal(read.kind === "found" && hex(read.keys.signPublic), signPublic);
  assert.deepEqual(stateOf(legacy).keys, []);
  await finish(w);
});

test("droppen.app changed while the keys were on their way: nothing removed; the next move adds what changed", async () => {
  const { w, legacy, app, legacyTab, views } = setUp();
  await seedDevice(w);
  let release!: () => void;
  w.hub.gate = new Promise<void>((resolve) => (release = resolve));
  const first = startMove({ origins: ORIGINS, env: w.env(legacyTab) });
  await until(() => views.receiver.some((v) => v.step === "verifying"));
  // Another tab (one from before this version, which takes no lock) keeps a new user key epoch meanwhile.
  await legacy.keyStore().put(`uk/${ACCOUNT}/1`, { iv: new Uint8Array(12), sealed: new Uint8Array([4]) });
  const before = stateOf(legacy);
  delete before.local["rh.hub.move.out"];
  w.hub.gate = null;
  release();
  assert.deepEqual(await first, { kind: "unfinished", why: "changed" });
  const noted = stateOf(legacy);
  assert.equal(JSON.parse(noted.local["rh.hub.move.out"]!).state, "verified", "droppen.app knows my. signed in with it");
  delete noted.local["rh.hub.move.out"];
  assert.deepEqual(noted, before, "nothing removed");
  assert.equal(readMarker(app.storage)?.state, "verified");
  assert.ok(!app.keyNames().includes(`uk/${ACCOUNT}/1`));
  const again = await startMove({ origins: ORIGINS, env: w.env(legacyTab) });
  assert.equal(again.kind === "moved" && again.resumed, true);
  assert.ok(app.keyNames().includes(`uk/${ACCOUNT}/1`), "what it lacked was added");
  assert.deepEqual(stateOf(legacy).keys, []);
  await finish(w);
});

test("my. holding another device takes none over it", async () => {
  const { w, legacy, app, legacyTab, appTab } = setUp();
  await seedDevice(w);
  const before = stateOf(legacy);
  await app.keyStore().put("device-keys/v2", { someone: "else" });
  app.storage.setItem("rh.hub.device.acct-2", "dev-9");
  const mine = stateOf(app);
  assert.deepEqual(await startMove({ origins: ORIGINS, env: w.env(legacyTab) }), { kind: "targetHasDevice" });
  assert.deepEqual(await startPull({ origins: ORIGINS, env: w.env(appTab) }), { kind: "targetHasDevice" });
  assert.deepEqual(stateOf(legacy), before);
  assert.deepEqual(stateOf(app), mine);
  await finish(w);
});

test("nothing to move: no device at droppen.app", async () => {
  const { w, legacyTab, appTab } = setUp();
  assert.deepEqual(await startMove({ origins: ORIGINS, env: w.env(legacyTab) }), { kind: "nothingToMove" });
  assert.deepEqual(await startPull({ origins: ORIGINS, env: w.env(appTab) }), { kind: "nothingToMove" });
  await finish(w);
});

test("another tab moving (its lock held): busy, nothing touched", async () => {
  const { w, legacy, legacyTab } = setUp();
  await seedDevice(w);
  const before = stateOf(legacy);
  let release!: () => void;
  const held = legacy.locks.request("rh.hub.move", { mode: "exclusive" }, () => new Promise<void>((resolve) => (release = resolve)));
  assert.deepEqual(await startMove({ origins: ORIGINS, env: w.env(legacyTab) }), { kind: "busy" });
  release();
  await held;
  assert.deepEqual(stateOf(legacy), before);
  await finish(w);
});

test("a counterpart other than the pinned one (a parent domain, a lookalike) is refused before any window opens", async () => {
  const { w, legacy, legacyTab, appTab } = setUp();
  await seedDevice(w);
  const before = stateOf(legacy);
  for (const app of ["https://test", "https://droppen.test.evil.test", "https://other.droppen.test"]) {
    const out = await startMove({ origins: { legacy: LEGACY, app }, env: w.env(legacyTab) });
    assert.equal(out.kind === "failed" && /not the hub's own/.test(out.reason), true, `${app}: ${JSON.stringify(out)}`);
  }
  const out = await startPull({ origins: { legacy: "https://test", app: APP }, env: w.env(appTab) });
  assert.equal(out.kind, "failed");
  w.pinned = null;
  assert.equal((await startMove({ origins: ORIGINS, env: w.env(legacyTab) })).kind, "failed", "no pinned app host: no move");
  assert.equal(w.windows.length, 2, "no window opened");
  assert.deepEqual(stateOf(legacy), before);
  await finish(w);
});

test("what an unfinished move of another device left is not written over, and a stale note is forgotten", async () => {
  const { w, legacy, app, legacyTab, appTab } = setUp();
  await seedDevice(w);
  const before = stateOf(legacy);
  // A device here, and a "writing" note naming another account's device: the next bundle may not write over it.
  await app.keyStore().put("device-keys/v2", { someone: "else" });
  app.storage.setItem("rh.hub.device.acct-2", "dev-9");
  app.storage.setItem(MOVE_MARKER, JSON.stringify({ v: 1, state: "writing", account: "acct-2", device: "dev-9", from: LEGACY, atMs: 1 }));
  assert.deepEqual(await startMove({ origins: ORIGINS, env: w.env(legacyTab) }), { kind: "targetHasDevice" });
  assert.deepEqual(await startPull({ origins: ORIGINS, env: w.env(appTab) }), { kind: "targetHasDevice" }, "nor pulled over");
  // The droppen.app popup hears the refusal a moment later, and forgets the note it made as the keys went.
  await until(() => legacy.storage.getItem("rh.hub.move.out") === null);
  assert.deepEqual(stateOf(legacy), before);
  // Its device gone (removed here since), the note says nothing any more: forgotten, and the move goes through.
  await app.keyStore().delete("device-keys/v2");
  app.storage.removeItem("rh.hub.device.acct-2");
  assert.equal((await startMove({ origins: ORIGINS, env: w.env(legacyTab) })).kind, "moved");
  assert.equal(readMarker(app.storage)?.account, ACCOUNT);
  await finish(w);
});

test("a page on neither origin, or the wrong side, refuses to start", async () => {
  const { w, legacyTab, appTab } = setUp();
  await seedDevice(w);
  assert.equal((await startMove({ origins: ORIGINS, env: w.env(appTab) })).kind, "failed");
  assert.equal((await startPull({ origins: ORIGINS, env: w.env(legacyTab) })).kind, "failed");
  assert.equal((await startMove({ origins: { legacy: LEGACY, app: EVIL }, env: w.env(legacyTab) })).kind, "failed");
  assert.equal(w.windows.length, 2, "no window opened");
  await finish(w);
});
