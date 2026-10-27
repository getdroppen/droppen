// "Remove this browser" on the apex after a move that did not finish
// (`features/hosts/remove-here.ts`, `features/move` `moveUnfinished`): the
// app host may be using the same device, so only the apex's copy goes — the
// account is not even opened, nothing is asked of the hub but the end of the
// apex's own session — and the notice says so, asking twice when the move
// was cut off before the app host said it had the keys. Without such a move,
// Remove opens the account to take the device off it, as before.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { I18nextProvider } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { loadDeviceKeys, MemoryKeyStore } from "../src/lib/e2e/keystore.js";
import { makeI18n } from "./render.js";

class MapStorage {
  readonly values = new Map<string, string>();
  get length() {
    return this.values.size;
  }
  key(i: number) {
    return [...this.values.keys()][i] ?? null;
  }
  getItem(k: string) {
    return this.values.get(k) ?? null;
  }
  setItem(k: string, v: string) {
    this.values.set(k, String(v));
  }
  removeItem(k: string) {
    this.values.delete(k);
  }
}

const local = new MapStorage();
Object.assign(globalThis, { localStorage: local, sessionStorage: new MapStorage(), location: { replace: () => undefined, origin: "https://droppen.app", host: "droppen.app" } });
// An account opened to be removed leaves its timers and channels behind: the file ends when its tests do.
after(() => void setTimeout(() => process.exit(), 50));

const { prepareRemoval, removeHere } = await import("../src/features/hosts/remove-here.js");
const { MoveNotice } = await import("../src/features/hosts/legacy.js");
const { moveJa, moveZhCN, moveZhTW } = await import("../src/i18n/move.js");

const ACCOUNT = "acct-1";
const DEVICE = "dev-1";
const unfinished = (state: "sent" | "verified") => JSON.stringify({ state, account: ACCOUNT, device: DEVICE, app: "https://my.droppen.app", atMs: 1 });

function seed(state: "sent" | "verified" | null) {
  local.values.clear();
  local.setItem(`rh.hub.device.${ACCOUNT}`, DEVICE);
  local.setItem("rh.hub.session", JSON.stringify({ account: ACCOUNT, device: DEVICE, token: "apex-token", expiresMs: Date.now() + 60_000 }));
  if (state) local.setItem("rh.hub.move.out", unfinished(state));
}

/** A hub that records every call; signing the device in is what opening the account (to remove it) starts with. */
function recordingApi() {
  const calls: string[] = [];
  const api = new Proxy(
    {},
    {
      // Not a thenable: an awaited API object must not be taken for a promise.
      get: (_, name: string) => name === "then" ? undefined : async (...args: unknown[]) => {
        calls.push(name === "logout" ? `logout ${String(args[0])}` : name);
        if (name !== "logout") throw new Error("not here");
      },
    },
  );
  return { api, calls };
}

const here = { held: true, accounts: [ACCOUNT], session: { account: ACCOUNT, device: DEVICE, token: "apex-token", expiresMs: Date.now() + 60_000 } };

test("after an unfinished move, Remove deletes only the apex copy and never opens the account", async () => {
  for (const state of ["sent", "verified"] as const) {
    seed(state);
    const store = new MemoryKeyStore();
    await store.put("device-keys/v2", { kept: "here" });
    const { api, calls } = recordingApi();
    const removal = await prepareRemoval(here as never, "wss://droppen.app", { api: api as never, store });
    assert.equal(removal.localOnly, state);
    assert.equal(removal.account, undefined, "the account is not opened");
    assert.deepEqual(calls, [], "nothing asked of the hub to prepare it");
    const result = await removeHere(removal);
    assert.equal(result.stillListed, null, "listed on purpose: the app host may be using it");
    assert.deepEqual(calls, ["logout apex-token"], "only the apex's own session ends; no removal is signed");
    assert.equal(await store.get("device-keys/v2"), undefined, "the apex copy of the keys is gone");
    assert.equal(local.getItem(`rh.hub.device.${ACCOUNT}`), null);
    assert.equal(local.getItem("rh.hub.session"), null);
    assert.equal(local.getItem("rh.hub.move.out"), null, "nothing unfinished is left to say");
  }
});

test("with no unfinished move, Remove opens the account to take the device off it (as before)", async () => {
  seed(null);
  const { api, calls } = recordingApi();
  const store = new MemoryKeyStore();
  await loadDeviceKeys(store);
  const removal = await prepareRemoval(here as never, "wss://droppen.app", { api: api as never, store });
  assert.equal(removal.localOnly, undefined);
  assert.ok(calls.length > 0 && !calls[0]!.startsWith("logout"), `it tried to open the account, signing the device in (${calls.join(", ")})`);
});

test("another device's unfinished move does not make this one's Remove local-only", async () => {
  seed(null);
  local.setItem("rh.hub.move.out", JSON.stringify({ state: "verified", account: ACCOUNT, device: "someone-else", app: "https://my.droppen.app", atMs: 1 }));
  const { api } = recordingApi();
  const removal = await prepareRemoval(here as never, "wss://droppen.app", { api: api as never, store: new MemoryKeyStore() });
  assert.equal(removal.localOnly, undefined);
});

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: moveZhCN }, "zh-TW": { translation: moveZhTW }, ja: { translation: moveJa } },
  keySeparator: false,
  nsSeparator: false
});
const draw = (node: React.ReactElement, lng = "en") => {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
};
const notice = (lng = "en") =>
  draw(<MoveNotice app="https://my.droppen.app" continueTo="/sessions" here={here as never} popup={false} standalone={false} canMove move={async () => ({ kind: "cancelled" })} />, lng);

test("the notice says the move did not finish, and what Remove does then, in every language", () => {
  seed("sent");
  const sent = notice();
  assert.match(sent, /data-testid="move-notice-unfinished" data-state="sent"/);
  assert.ok(sent.includes("my.droppen.app may already be using this browser&#x27;s device") || sent.includes("my.droppen.app may already be using this browser's device"));
  assert.ok(/Deletes only this browser(&#x27;|')s copy of its keys here/.test(sent), "Remove is described as local-only");
  seed("verified");
  assert.match(notice(), /data-state="verified"/);
  for (const lng of ["zh-CN", "zh-TW", "ja"]) assert.doesNotMatch(notice(lng), /is already using this browser/, `${lng} is translated`);
  seed(null);
  assert.doesNotMatch(notice(), /move-notice-unfinished/);
  assert.match(notice(), /Takes this browser off your account(&#x27;|')s devices/);
});
