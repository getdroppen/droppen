// The hub's origin once the app has moved to its app host
// (`features/hosts/legacy.tsx`, droppen.app → my.droppen.app): the move notice.
//
// What these pin: whether this browser keeps a device there is told from its
// notes and its key store without creating or upgrading a database, and what a
// removal leaves behind does not count; a browser with nothing there goes
// straight on (path, query and fragment kept), any other is told and offered
// move / remove / continue — move only where the move module says it can, and
// never in a Safari home-screen app, which is told to add the app host and
// pair instead; a sign-in window an older Droppen opened is told to close and
// continue in a browser tab; the move's outcome takes the tab on, or says why
// not; the words are there in all four languages.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test, { after } from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { installJsdom } from "./jsdom.js";

const { window, document } = installJsdom();
after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("./i18n-all.js");
const { deviceHere, KEY_DB } = await import("../src/lib/hub/device-here.js");
const { appleHomeScreenApp, goesStraightOn, MoveNotice } = await import("../src/features/hosts/legacy.js");
const { hostsJa, hostsZhCN, hostsZhTW } = await import("../src/i18n/hosts.js");
const { moveJa, moveZhCN, moveZhTW } = await import("../src/i18n/move.js");
const { signOutJa, signOutZhCN, signOutZhTW } = await import("../src/i18n/sign-out.js");
const { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } = await import("../src/i18n/cloud-devices.js");
const { ja } = await import("../src/i18n/ja.js");
type MoveOutcome = import("../src/features/move/index.js").MoveOutcome;
type DeviceHere = import("../src/lib/hub/device-here.js").DeviceHere;

class Notes {
  readonly values = new Map<string, string>();
  constructor(entries: Record<string, string> = {}) {
    for (const [k, v] of Object.entries(entries)) this.values.set(k, v);
  }
  get length() {
    return this.values.size;
  }
  key(i: number) {
    return [...this.values.keys()][i] ?? null;
  }
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
}

/** An IndexedDB that lists `listed`; `rh-e2e` exists (else opening it would create it) with `records` in its `keys` store. */
function keyStoreDb(opts: { listed: string[]; exists?: boolean; records?: string[]; databases?: boolean }) {
  const log: string[] = [];
  const idb = {
    log,
    databases: opts.databases === false ? undefined : async () => opts.listed.map((name) => ({ name, version: 1 })),
    open(name: string, version?: number) {
      log.push(`open ${name}${version === undefined ? "" : ` v${version}`}`);
      const req: Record<string, unknown> & { onupgradeneeded?: () => void; onsuccess?: () => void; onerror?: () => void } = { result: null, transaction: null };
      queueMicrotask(() => {
        if (opts.exists === false) {
          let aborted = false;
          req.transaction = { abort: () => void (aborted = true) };
          req.onupgradeneeded?.();
          log.push(aborted ? "aborted the creation" : "created");
          if (aborted) return req.onerror?.();
        }
        req.result = {
          objectStoreNames: { contains: (store: string) => store === "keys" },
          close: () => void log.push("closed"),
          transaction: (store: string, mode: string) => {
            log.push(`${mode} ${store}`);
            return {
              objectStore: () => ({
                count: (key: string) => {
                  const r: Record<string, unknown> & { onsuccess?: () => void } = {};
                  queueMicrotask(() => {
                    r.result = (opts.records ?? []).includes(key) ? 1 : 0;
                    r.onsuccess?.();
                  });
                  return r;
                },
                put: () => assert.fail("nothing is written"),
                delete: () => assert.fail("nothing is deleted"),
              }),
            };
          },
        };
        req.onsuccess?.();
      });
      return req;
    },
  };
  return idb;
}

test("a device here is told from the browser's notes and its key store, without creating or changing a database", async () => {
  const session = JSON.stringify({ account: "a-2", token: "t", expiresMs: Date.now() + 60_000, device: "d-2" });
  const none = keyStoreDb({ listed: [] });
  // Notes: a device id noted for an account, or a kept device session. The key store is not opened then.
  assert.deepEqual(await deviceHere({ storage: new Notes({ "rh.hub.device.a-1": "d-1", "rh.theme": "dark" }), idb: none }), { held: true, accounts: ["a-1"], session: null });
  const both = await deviceHere({ storage: new Notes({ "rh.hub.device.a-1": "d-1", "rh.hub.devicekey.a-1": "ab", "rh.hub.device.a-2": "d-2", "rh.hub.session": session }), idb: none });
  assert.equal(both.held, true);
  assert.deepEqual(both.accounts, ["a-2", "a-1"], "the kept session's account first");
  assert.equal(both.session?.device, "d-2");
  assert.deepEqual(none.log, []);
  // No notes: the key store, only where the browser lists it.
  assert.deepEqual(await deviceHere({ storage: new Notes(), idb: none }), { held: false, accounts: [], session: null });
  assert.deepEqual(none.log, [], "not listed: never opened (opening would create it)");
  const seenOnly = keyStoreDb({ listed: ["rh-hub-seen", "rh-local-cache"] });
  assert.equal((await deviceHere({ storage: new Notes(), idb: seenOnly })).held, false, "the seen record outlives a removal: not a device");
  assert.deepEqual(seenOnly.log, []);
  const keys = keyStoreDb({ listed: [KEY_DB], records: ["device-keys/v2", "local-wrap/v1"] });
  assert.equal((await deviceHere({ storage: new Notes(), idb: keys })).held, true);
  assert.deepEqual(keys.log, ["open rh-e2e", "readonly keys", "closed"], "opened at its own version, read only, closed");
  const legacy = keyStoreDb({ listed: [KEY_DB], records: ["device-keys/v1"] });
  assert.equal((await deviceHere({ storage: new Notes(), idb: legacy })).held, true);
  const emptied = keyStoreDb({ listed: [KEY_DB], records: ["local-wrap/v1"] });
  assert.equal((await deviceHere({ storage: new Notes(), idb: emptied })).held, false, "a removed browser's emptied store");
  const gone = keyStoreDb({ listed: [KEY_DB], exists: false });
  assert.equal((await deviceHere({ storage: new Notes(), idb: gone })).held, false);
  assert.deepEqual(gone.log, ["open rh-e2e", "aborted the creation"], "deleted since it was listed: not made again");
  assert.equal((await deviceHere({ storage: new Notes(), idb: keyStoreDb({ listed: [KEY_DB], records: ["device-keys/v2"], databases: false }) })).held, false, "a browser that cannot list its databases: its notes only");
  assert.equal((await deviceHere({ storage: null, idb: null })).held, false);
});

const nothing: DeviceHere = { held: false, accounts: [], session: null };
const device: DeviceHere = { held: true, accounts: ["a-1"], session: null };

test("who goes straight on: a browser with nothing here, outside a sign-in window and a Safari home-screen app", () => {
  assert.equal(goesStraightOn({ here: nothing, popup: false, standalone: false }), true);
  assert.equal(goesStraightOn({ here: device, popup: false, standalone: false }), false);
  assert.equal(goesStraightOn({ here: nothing, popup: true, standalone: false }), false, "an older Droppen's window: told to close it");
  assert.equal(goesStraightOn({ here: nothing, popup: false, standalone: true }), false, "a home-screen app: told to add the app host instead");
});

test("a Safari home-screen app (iPhone, iPad, a Mac's Dock) is told apart from a browser and from another browser's installed app", () => {
  const mac = (vendor: string, standalone: boolean) => ({
    navigator: { userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)", vendor } as Navigator,
    matchMedia: () => ({ matches: standalone }),
  });
  assert.equal(appleHomeScreenApp({ navigator: { standalone: true, userAgent: "iPhone", vendor: "Apple Computer, Inc." } as unknown as Navigator }), true);
  assert.equal(appleHomeScreenApp(mac("Apple Computer, Inc.", true)), true);
  assert.equal(appleHomeScreenApp(mac("Apple Computer, Inc.", false)), false, "Safari itself");
  assert.equal(appleHomeScreenApp(mac("Google Inc.", true)), false, "Chrome's installed app shares Chrome's storage: it can move");
  assert.equal(appleHomeScreenApp({ navigator: { userAgent: "Mozilla/5.0 (Windows NT 10.0)", vendor: "Google Inc." } as Navigator, matchMedia: () => ({ matches: true }) }), false);
  assert.equal(appleHomeScreenApp({}), false);
});

const APP = "https://my.droppen.app";

function draw(props: Partial<React.ComponentProps<typeof MoveNotice>>, lng = "en"): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <MoveNotice app={APP} continueTo="/sessions/claude/x?y=1#z" here={device} popup={false} standalone={false} canMove={false} {...props} />
    </I18nextProvider>,
  );
}
const has = (html: string, id: string) => html.includes(`data-testid="${id}"`);

test("a browser that keeps a device here is told so, and offers to move it only where the move module says it can", () => {
  const plain = draw({});
  assert.ok(plain.includes("Droppen moved to my.droppen.app"));
  assert.ok(has(plain, "move-notice-who"));
  assert.ok(!has(plain, "move-notice-move"), "no move until the move module offers it");
  assert.ok(has(plain, "move-notice-remove") && plain.includes(">Remove this browser</button>"));
  assert.ok(has(plain, "move-notice-continue"));
  assert.ok(plain.includes("At my.droppen.app, sign in and add this browser as a new device"));
  const moving = draw({ canMove: true });
  assert.ok(has(moving, "move-notice-move") && moving.includes("Move this browser"));
  assert.ok(moving.includes("Remove this browser instead"));
  assert.ok(moving.includes("my.droppen.app can still bring them over later"));
  // A Safari home-screen app: its storage is its own. Remove, then add the app host and pair it.
  const app = draw({ canMove: true, standalone: true });
  assert.ok(!has(app, "move-notice-move") && !has(app, "move-notice-continue"));
  assert.ok(has(app, "move-notice-standalone") && has(app, "move-notice-remove"));
  assert.match(app, /<a href="https:\/\/my\.droppen\.app\/" target="_blank" rel="noopener" data-testid="move-notice-open"/);
  // Opened by an older Droppen's local settings: close it and continue in a browser tab; nothing to go on to here.
  const popupHtml = draw({ popup: true, here: nothing });
  assert.ok(has(popupHtml, "move-notice-popup") && popupHtml.includes("continue in a browser tab instead"));
  assert.ok(!has(popupHtml, "move-notice-remove") && !has(popupHtml, "move-notice-open") && !has(popupHtml, "move-notice-who"));
  const windowWithDevice = draw({ popup: true, canMove: true });
  assert.ok(has(windowWithDevice, "move-notice-popup") && has(windowWithDevice, "move-notice-move"), "a device here: the move first");
  // In every language.
  assert.ok(draw({ canMove: true }, "zh-CN").includes("Droppen 已搬到 my.droppen.app"));
  assert.ok(draw({ canMove: true }, "zh-TW").includes("搬移這個瀏覽器"));
  assert.ok(draw({ canMove: true }, "ja").includes("このブラウザを移す"));
  void i18n.changeLanguage("en");
});

test("every word of the notice is in zh-CN, zh-TW and ja", () => {
  const source = readFileSync(new URL("../src/features/hosts/legacy.tsx", import.meta.url), "utf8");
  const keys = [...source.matchAll(/\bt\("((?:[^"\\]|\\.)*)"/g)].map((m) => JSON.parse(`"${m[1]}"`) as string);
  assert.ok(keys.length >= 25, `${keys.length} keys`);
  const tables = {
    "zh-CN": { ...cloudDevicesZhCN, ...signOutZhCN, ...hostsZhCN, ...moveZhCN },
    "zh-TW": { ...cloudDevicesZhTW, ...signOutZhTW, ...hostsZhTW, ...moveZhTW },
    ja: { ...ja, ...cloudDevicesJa, ...signOutJa, ...hostsJa, ...moveJa },
  };
  for (const [lng, table] of Object.entries(tables)) {
    for (const key of keys) {
      const said = (table as Record<string, string>)[key];
      assert.ok(said, `${lng}: ${key}`);
      for (const name of key.match(/\{\{\w+\}\}/g) ?? []) assert.ok(said.includes(name), `${lng} keeps ${name}: ${key}`);
    }
  }
});

test("the move's outcome: moved goes on to the same page at the app host; blocked or failed says so and keeps everything; continue goes on", async () => {
  const container = document.body.appendChild(document.createElement("div"));
  const went: string[] = [];
  const asked: unknown[] = [];
  let answer: MoveOutcome = { kind: "moved" };
  const move = async (request: unknown) => (asked.push(request), answer);
  const root = createRoot(container);
  // Each scenario on a fresh notice (a page that went on is gone; here it stays on its last step).
  let fresh = 0;
  const mount = async (props: Partial<React.ComponentProps<typeof MoveNotice>> = {}) =>
    act(async () =>
      root.render(
        <I18nextProvider i18n={i18n}>
          <MoveNotice key={fresh++} app={APP} continueTo="/verify#t=abc" here={device} popup={false} standalone={false} canMove go={(url) => void went.push(url)} move={move} {...props} />
        </I18nextProvider>,
      ),
    );
  const click = (id: string) =>
    act(async () => {
      const el = document.querySelector<HTMLElement>(`[data-testid="${id}"]`);
      assert.ok(el, id);
      el.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
      await new Promise((resolve) => setTimeout(resolve, 10));
    });
  await mount();
  await click("move-notice-move");
  assert.deepEqual(asked, [{ app: APP, continueTo: "/verify#t=abc", account: "a-1" }]);
  assert.deepEqual(went, ["https://my.droppen.app/verify#t=abc"]);

  answer = { kind: "blocked" };
  await mount();
  await click("move-notice-move");
  assert.equal(went.length, 1);
  assert.match(document.querySelector('[role="alert"]')?.textContent ?? "", /blocked the my\.droppen\.app window/);
  answer = { kind: "failed", message: "the receiver refused" };
  await click("move-notice-move");
  assert.match(document.querySelector('[role="alert"]')?.textContent ?? "", /nothing was deleted here: the receiver refused/);
  answer = { kind: "cancelled" };
  await click("move-notice-move");
  assert.equal(document.querySelector('[role="alert"]'), null);

  await click("move-notice-continue");
  assert.deepEqual(went, ["https://my.droppen.app/verify#t=abc", "https://my.droppen.app/verify#t=abc"]);

  // In an older Droppen's sign-in window, a move that went through says to close it: it is not sent on.
  answer = { kind: "moved" };
  await mount({ popup: true });
  await click("move-notice-move");
  assert.ok(document.querySelector('[data-testid="move-notice-moved"]'));
  assert.equal(went.length, 2);
  await act(async () => root.unmount());
});
