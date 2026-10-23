// The browser key move's parts (`lib/move`): what crosses (the allowlist, held
// to "sign out and remove this browser"'s wipe list), the digest both sides
// compare, the bundle the app host takes or refuses, the two origins it runs
// between, and the "notifications were on" notice after a move (D7).
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { MemoryKeyStore } from "../src/lib/e2e/keystore.js";
import { CloudAccount } from "../src/lib/hub/account.js";
import { MemorySeenStore } from "../src/lib/hub/seen-store.js";
import { movesLocal } from "../src/lib/move/allowlist.js";
import { canonical, digestOf, Unmovable } from "../src/lib/move/digest.js";
import { originsProblem } from "../src/lib/move/protocol.js";
import { checkBundle } from "../src/lib/move/receiver.js";
import { takeSnapshot } from "../src/lib/move/sender.js";
import { PushMovedNotice } from "../src/features/move/push-moved-notice.js";
import { SenderPanel, ReceiverPanel, outcomeText } from "../src/features/move/move-page.js";
import { moveJa, moveZhCN, moveZhTW } from "../src/i18n/move.js";
import { ACCOUNT, DEVICE, LEGACY, ORIGINS, seedDevice, World } from "./move-world.js";

test("every note 'remove this browser' forgets is one the move carries", async () => {
  // droppen#73's wipe list is the move's: what a removal forgets, a move must not leave behind.
  const removed = new Set<string>();
  const values = new Map<string, string>([
    [`rh.hub.device.${ACCOUNT}`, DEVICE],
    [`rh.hub.devicekey.${ACCOUNT}`, "ab"],
  ]);
  const storage = {
    getItem: (k: string) => values.get(k) ?? null,
    setItem: (k: string, v: string) => void values.set(k, v),
    removeItem: (k: string) => {
      removed.add(k);
      values.delete(k);
    },
  };
  await CloudAccount.wipeLocal(new MemoryKeyStore(), ACCOUNT, { storage, seen: new MemorySeenStore() });
  assert.ok(removed.size >= 10, `the wipe forgets its notes (${removed.size})`);
  for (const key of removed) assert.ok(movesLocal(key), `${key} is wiped by a removal but not moved`);
  // Forgotten through the page's own storage (deliver.ts), outside the list above.
  assert.ok(movesLocal(`rh.hub.reencrypt.v2.${ACCOUNT}`));
});

test("the allowlist: the account's notes and a few settings move; the session, push and the move's own notes never", () => {
  for (const key of [
    `rh.hub.device.${ACCOUNT}`,
    `rh.hub.seen.${ACCOUNT}`,
    `rh.hub.genesis.${ACCOUNT}`,
    "rh.hub.seen-in-idb",
    `rh.hub.machine.${ACCOUNT}`,
    `rh.hub.home.${ACCOUNT}`,
    "rh.hub.knownClientHash:00ff",
    "rh.relays.manual",
    "droppen.e2e-compat",
    "rh.theme",
    "rh.fontSize",
  ])
    assert.ok(movesLocal(key), key);
  for (const key of ["rh.hub.session", "rh.hub.push.subscribed", "rh.hub.push.renew", "rh.hub.push.moved", "rh.hub.move.in", "rh.draft.x", "rh.hub.device.", "rh.auth.token", "rh.hub.knownClient:a@b.c", "rh.chat.pending.x"])
    assert.ok(!movesLocal(key), key);
});

test("the digest: canonical, order-free, and refusing what it cannot compare", async () => {
  assert.equal(canonical({ b: 1, a: [true, null, undefined] }), canonical({ a: [true, null, undefined], b: 1 }));
  assert.notEqual(canonical(new Uint8Array([1, 2])), canonical(new Uint8Array([1, 2]).buffer));
  assert.notEqual(canonical(0), canonical(-0));
  assert.notEqual(canonical("1"), canonical(1));
  const key = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, false, ["encrypt", "decrypt"]);
  assert.equal(canonical(key), canonical(structuredClone(key)), "a key counts by what it says of itself");
  assert.match(canonical(key), /AES-GCM/);
  const a = await digestOf([
    { where: "ls", key: "x", value: "1" },
    { where: "idb:rh-e2e/keys", key: "y", value: new Uint8Array([1]) },
  ]);
  const b = await digestOf([
    { where: "idb:rh-e2e/keys", key: "y", value: new Uint8Array([1]) },
    { where: "ls", key: "x", value: "1" },
  ]);
  assert.equal(a, b);
  assert.notEqual(a, await digestOf([{ where: "ls", key: "x", value: "1" }]));
  assert.throws(() => canonical(() => 1), Unmovable);
  assert.throws(() => canonical(new (class Thing {})()), Unmovable);
});

test("the origins: exactly the pinned pair, never a suffix or parent-domain match", () => {
  const pinned = { legacy: "https://droppen.app", app: "https://my.droppen.app" };
  assert.equal(originsProblem(pinned, "https://droppen.app", "legacy", pinned), null);
  assert.equal(originsProblem(pinned, "https://my.droppen.app", "app", pinned), null);
  assert.match(originsProblem(pinned, "https://my.droppen.app", "legacy", pinned)!, /not at/);
  // What the old suffix rule let through: a parent domain as either side, an app host under another.
  assert.match(originsProblem({ legacy: "https://app", app: "https://my.droppen.app" }, "https://my.droppen.app", "app", pinned)!, /not the hub's own/);
  assert.match(originsProblem({ legacy: "https://droppen.app", app: "https://app" }, "https://droppen.app", "legacy", pinned)!, /not the hub's own/);
  assert.match(originsProblem({ legacy: "https://droppen.app", app: "https://evil.droppen.app" }, "https://droppen.app", "legacy", pinned)!, /not the hub's own/);
  assert.match(originsProblem({ legacy: "https://droppen.app", app: "https://my.droppen.app.evil.test" }, "https://droppen.app", "legacy", pinned)!, /not the hub's own/);
  assert.match(originsProblem(pinned, "https://droppen.app", "legacy", null)!, /names no app host/);
  assert.match(originsProblem({ legacy: "http://droppen.app", app: "https://my.droppen.app" }, "http://droppen.app", "legacy", pinned)!, /https/);
  assert.match(originsProblem({ legacy: "https://droppen.app/", app: "https://my.droppen.app" }, "https://droppen.app", "legacy", pinned)!, /https origins/);
  assert.match(originsProblem({ legacy: "https://droppen.app", app: "https://droppen.app" }, "https://droppen.app", "legacy", pinned)!, /same/);
});

test("the hub-only setting moves (a move must not turn direct connections back on)", () => {
  assert.ok(movesLocal("rh.hub.hubOnly"));
});

test("the app host takes only a bundle from droppen.app, of the listed stores and entries, matching its digest", async () => {
  const w = new World();
  await seedDevice(w);
  const env = w.env(w.window(`${LEGACY}/`));
  const bundle = (await takeSnapshot(env, { account: ACCOUNT, device: DEVICE }))!;
  assert.ok(bundle);
  assert.equal(typeof (await checkBundle(structuredClone(bundle), ORIGINS)), "object");
  const variant = async (change: (b: typeof bundle) => void) => {
    const b = structuredClone(bundle);
    change(b);
    return checkBundle(b, ORIGINS);
  };
  assert.match(String(await variant((b) => (b.from = "https://evil.test"))), /another origin/);
  assert.match(String(await variant((b) => b.local.push({ key: "rh.hub.session", value: "{}" }))), /does not copy/);
  assert.match(String(await variant((b) => b.local.push({ key: "rh.theme", value: "light" }))), /malformed entry|does not match/);
  assert.match(String(await variant((b) => (b.stores[0]!.db = "rh-local-cache"))), /store the move does not copy/);
  assert.match(String(await variant((b) => (b.stores[0]!.records[0]!.value = "changed"))), /digest/);
  assert.match(String(await variant((b) => (b.device = "someone-else"))), /not the one it notes/);
  assert.match(String(await variant((b) => (b.stores = b.stores.filter((s) => s.db !== "rh-e2e")))), /without device keys|digest/);
  assert.equal(bundle.local.some((e) => e.key === "rh.hub.session" || e.key.startsWith("rh.hub.push.")), false, "the sender never puts them in");
});

const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: moveZhCN }, "zh-TW": { translation: moveZhTW }, ja: { translation: moveJa } },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});
function draw(node: React.ReactElement, lng = "en"): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
}

test("D7: after a move with notifications on, one tap turns them back on (the subscription itself never moved)", async () => {
  assert.equal(draw(<PushMovedNotice moved={() => null} />), "");
  const html = draw(<PushMovedNotice moved={() => "host-1"} />);
  assert.ok(html.includes("Notifications from host-1 were on before this browser moved here."));
  assert.match(html, /data-testid="push-moved-on"[^>]*>turn notifications back on</);
  assert.match(draw(<PushMovedNotice moved={() => "host-1"} />, "zh-CN"), /重新打开|在这里需要重新打开/);
});

test("the /move popup says which device it would send, and how a move ended, in every language", () => {
  const hosts = { app: "my.droppen.app", legacy: "droppen.app" };
  const confirm = draw(<SenderPanel view={{ step: "confirm", account: ACCOUNT, device: DEVICE }} hosts={hosts} onConfirm={() => undefined} onCancel={() => undefined} />);
  assert.ok(confirm.includes(`${DEVICE} of account ${ACCOUNT}`));
  assert.match(confirm, /data-testid="move-confirm"[^>]*>Move to my.droppen.app</);
  assert.match(draw(<ReceiverPanel view={{ step: "noOpener" }} hosts={hosts} />), /Nothing to do here/);
  const t = (key: string, vars?: Record<string, unknown>) => i18n.t(key, vars);
  for (const lng of ["zh-CN", "zh-TW", "ja"]) {
    void i18n.changeLanguage(lng);
    for (const kind of ["popupBlocked", "closed", "cancelled", "nothingToMove", "targetHasDevice", "busy", "timeout"] as const) {
      const text = outcomeText(t, { kind }, hosts);
      assert.ok(!/^[\x20-\x7e]+$/.test(text), `${lng} ${kind} is translated: ${text}`);
    }
  }
  void i18n.changeLanguage("en");
});

test("the move notice's seam (features/move): every outcome as the notice takes it, nothing wiped unless moved", async () => {
  const { noticeOutcome, moveSupported } = await import("../src/features/move/index.js");
  const origins = { legacy: "https://droppen.app", app: "https://my.droppen.app" };
  assert.deepEqual(noticeOutcome({ kind: "moved", account: ACCOUNT, device: DEVICE, pushWasOn: null, resumed: false, senderWiped: true }, origins), { kind: "moved" });
  assert.deepEqual(noticeOutcome({ kind: "popupBlocked" }, origins), { kind: "blocked" });
  assert.deepEqual(noticeOutcome({ kind: "closed" }, origins), { kind: "cancelled" });
  assert.deepEqual(noticeOutcome({ kind: "cancelled" }, origins), { kind: "cancelled" });
  for (const ended of [{ kind: "verifyFailed", reason: "denied" }, { kind: "unfinished", why: "changed" }, { kind: "busy" }, { kind: "timeout" }, { kind: "targetHasDevice" }, { kind: "nothingToMove" }, { kind: "failed", reason: "x" }] as const) {
    const out = noticeOutcome(ended, origins);
    assert.equal(out.kind, "failed", ended.kind);
    assert.ok(out.kind === "failed" && out.message.length > 0);
  }
  assert.equal(moveSupported(), false, "no window here: not offered");
});
