// The cloud app (`features/cloud/cloud-app.tsx`) on a browser that cannot run
// the end-to-end encryption, driven in a real DOM: the Android phone of the
// report, whose WebCrypto knows no Ed25519 ("NotSupportedError: Algorithm:
// Unrecognized name"), signed in and opening the app.
//
// Pinned: the support check runs before the device keys are read; a browser
// that fails it gets "this browser cannot run Droppen's end-to-end
// encryption" with what it lacks and the browsers that have it — never "this
// browser's storage did not answer", never "try again" or "use this browser as
// a new device", always "sign out"; a browser lacking only the curves is
// offered compatibility mode, its risk said, and nothing is made until the
// person ticks that they understand it and trusts the browser — then the keys
// are made, in JavaScript, and the agreement is kept. An algorithm failing at
// key generation past the check is the same screen; a store that fails is
// still the storage message. Signed out, the screen comes before the sign-in
// form. Apps' built-in browsers are told to open the system browser.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after, afterEach } from "node:test";
import { installJsdom } from "./jsdom.js";

const { window, document, container } = installJsdom();
// The page's own listeners (the app's path follows `popstate`), on the window as in a browser.
for (const name of ["addEventListener", "removeEventListener", "dispatchEvent"] as const) {
  Object.defineProperty(globalThis, name, { configurable: true, writable: true, value: window[name].bind(window) });
}
// Cloud mode: the hub's tag in the page's shell (`lib/hub/config.ts`).
const meta = document.createElement("meta");
meta.name = "rh-hub";
meta.content = "wss://hub.test";
document.head.appendChild(meta);
// An IndexedDB for the support check to find; the app's key store is each test's own, so this one is never opened.
let idbOpened = 0;
Object.defineProperty(globalThis, "indexedDB", {
  configurable: true,
  writable: true,
  value: {
    open() {
      idbOpened++;
      throw new Error("these tests give the app a key store of their own");
    },
  },
});
// No hub: nothing here may need one.
const fetched: string[] = [];
globalThis.fetch = (async (input: RequestInfo | URL) => {
  fetched.push(String(input));
  throw new TypeError("offline");
}) as typeof fetch;

after(() => {
  (window as unknown as { close(): void }).close();
  setTimeout(() => process.exit(), 50);
});

const { act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("../src/i18n/index.js");
const { CloudApp } = await import("../src/features/cloud/cloud-app.js");
const { inAppBrowser, UnsupportedBrowserScreen } = await import("../src/features/cloud/browser-support.js");
const { clearSession, saveSession } = await import("../src/lib/hub/api.js");
const { MemoryKeyStore } = await import("../src/lib/e2e/index.js");
const { COMPAT_CONSENT_KEY, compatModeActive, forgetCompatConsent } = await import("../src/lib/e2e/compat.js");
const { removeNative } = await import("./no-native-curves.js");

type Root = ReturnType<typeof createRoot>;
let root: Root | null = null;
const restores: (() => void)[] = [];
afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  for (const restore of restores.splice(0).reverse()) restore();
  forgetCompatConsent();
  clearSession();
  localStorage.clear();
  fetched.length = 0;
  await i18n.changeLanguage("en");
});

/** Every call `crypto.subtle` gets (`<method> <algorithm> <extractable>`), and every key store access, in order. */
const log: string[] = [];
function recordSubtle(): void {
  const subtle = crypto.subtle as unknown as Record<string, (...a: unknown[]) => Promise<unknown>>;
  const at: Record<string, number> = { generateKey: 0, importKey: 2, sign: 0, verify: 0, deriveBits: 0, deriveKey: 0, encrypt: 0, decrypt: 0 };
  for (const [method, i] of Object.entries(at)) {
    const before = Object.getOwnPropertyDescriptor(subtle, method);
    const own = subtle[method]!.bind(subtle);
    Object.defineProperty(subtle, method, {
      configurable: true,
      writable: true,
      value: (...args: unknown[]) => {
        const alg = args[i];
        log.push(`${method} ${typeof alg === "string" ? alg : (alg as { name?: string }).name}${method === "generateKey" ? ` ${String(args[1])}` : ""}`);
        return own(...args);
      },
    });
    restores.push(() => (before ? Object.defineProperty(subtle, method, before) : delete subtle[method]));
  }
}

class LoggedStore extends MemoryKeyStore {
  failGets = false;
  override async get(name: string): Promise<unknown> {
    log.push(`store.get ${name}`);
    if (this.failGets) throw new DOMException("the connection is closing", "InvalidStateError");
    return super.get(name);
  }
  override async put(name: string, value: unknown): Promise<void> {
    log.push(`store.put ${name}`);
    return super.put(name, value);
  }
  override async delete(name: string): Promise<void> {
    log.push(`store.delete ${name}`);
    return super.delete(name);
  }
}

/** Signed in on this browser before (a kept device session), opening the app. */
function signedIn(): void {
  saveSession({ account: "a-test", token: "t-1", expiresMs: Date.now() + 3_600_000, device: "d-1" });
  window.history.replaceState(null, "", "/sessions");
}

async function render(store: LoggedStore): Promise<void> {
  log.length = 0;
  root = createRoot(container);
  await act(async () =>
    root!.render(
      <QueryClientProvider client={new QueryClient()}>
        <I18nextProvider i18n={i18n}>
          <CloudApp store={store} keyReadRetryMs={[0, 0, 0]}>
            <div data-testid="the-app" />
          </CloudApp>
        </I18nextProvider>
      </QueryClientProvider>,
    ),
  );
}

const byTestId = (id: string) => container.querySelector<HTMLElement>(`[data-testid="${id}"]`);
async function waitFor(what: string, ok: () => boolean, ms = 5_000): Promise<void> {
  const end = Date.now() + ms;
  while (!ok()) {
    if (Date.now() > end) assert.fail(`timed out waiting for ${what}: ${container.innerHTML.slice(0, 800)}`);
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
  }
}
const click = (el: HTMLElement | null) =>
  act(async () => {
    assert.ok(el, "the control is on the page");
    el.dispatchEvent(new window.MouseEvent("click", { bubbles: true, cancelable: true }));
  });

/** Neither of the device-keys screen's ways on, which can never help a browser that lacks the algorithms. */
function noRetryNoNewDevice(): void {
  assert.equal(byTestId("keys-retry"), null, "no “try again”");
  assert.equal(byTestId("keys-new-device"), null, "no “use this browser as a new device”");
  assert.ok(!container.textContent!.includes("storage did not answer"), "not the storage message");
}

test("no Ed25519 (and no HKDF): the unsupported screen before any key is read — what is missing, which browsers, sign out", async () => {
  restores.push(removeNative(["Ed25519", "HKDF"]));
  recordSubtle();
  signedIn();
  const store = new LoggedStore();
  await render(store);
  await waitFor("the unsupported screen", () => byTestId("unsupported-why") !== null);
  const text = container.textContent!;
  assert.ok(text.includes("this browser cannot run Droppen's end-to-end encryption"));
  assert.equal(byTestId("unsupported-missing")!.textContent, "missing: Ed25519, HKDF");
  assert.match(byTestId("unsupported-browsers")!.textContent!, /Chrome 137, Edge 137, Firefox 130 or Safari 17/);
  noRetryNoNewDevice();
  assert.equal(byTestId("compat-offer"), null, "HKDF missing: no compatibility mode");
  assert.ok(byTestId("unsupported-sign-out"), "sign out is there");
  // Nothing read, nothing made: no store access, no key generated but the check's own probes (non-extractable).
  assert.deepEqual(log.filter((l) => l.startsWith("store.")), []);
  assert.deepEqual(log.filter((l) => l.startsWith("generateKey") && l.endsWith(" true")), [], "no key made to keep");
  assert.equal(idbOpened, 0);
  assert.deepEqual(fetched, [], "the hub is not asked");
});

test("no Ed25519 only: compatibility mode is offered with its risk, and nothing is made until the person agrees", async () => {
  restores.push(removeNative(["Ed25519"]));
  signedIn();
  const store = new LoggedStore();
  await render(store);
  await waitFor("the unsupported screen", () => byTestId("unsupported-why") !== null);
  assert.ok(container.textContent!.includes("this browser does not support Droppen's strong key protection"));
  assert.equal(byTestId("unsupported-missing")!.textContent, "missing: Ed25519");
  const offer = byTestId("compat-offer");
  assert.ok(offer, "compatibility mode is offered");
  // What really changes, said as it is, ending with the risk — before the agreement is asked for.
  assert.match(offer.textContent!, /not hardened against side channels/);
  assert.match(offer.textContent!, /one more library is loaded into the page/);
  assert.match(offer.textContent!, /private keys are plain bytes in the page's memory instead of keys the browser holds, so malicious code that gets into the page could copy them\./);
  const said = offer.textContent!;
  assert.ok(said.indexOf("could copy them") < said.indexOf("I understand the risk") && said.indexOf("I understand the risk") < said.indexOf("trust this browser"), "the risk, then the agreement");
  assert.doesNotMatch(said, /not just use them/, "no contrast with native mode that does not hold");
  const trust = byTestId("compat-trust") as HTMLButtonElement;
  assert.equal(trust.disabled, true, "not before “I understand the risk”");
  await click(trust);
  noRetryNoNewDevice();
  assert.ok(byTestId("unsupported-sign-out"));
  assert.equal(localStorage.getItem(COMPAT_CONSENT_KEY), null, "nothing agreed");
  assert.deepEqual(log.filter((l) => l.startsWith("store.")), [], "no key read or made");
  assert.equal(compatModeActive(), false);
});

test("an algorithm failing when the keys are made, past the check: the same screen, not the storage message", async () => {
  // Knows Ed25519 for the check's non-extractable probe, but will not make an extractable key (what the keystore asks for).
  restores.push(removeNative(["Ed25519"], (method, args) => method === "generateKey" && args[1] === true));
  signedIn();
  const store = new LoggedStore();
  await render(store);
  await waitFor("the unsupported screen", () => byTestId("unsupported-why") !== null);
  assert.ok(container.textContent!.includes("this browser cannot run Droppen's end-to-end encryption"));
  noRetryNoNewDevice();
  assert.ok(byTestId("unsupported-sign-out"));
  // "Nothing was stored or changed" holds: the keys are made before anything is written or set aside.
  assert.match(byTestId("unsupported-why")!.textContent!, /Nothing was stored or changed/);
  assert.deepEqual(log.filter((l) => l.startsWith("store.put") || l.startsWith("store.delete")), []);
  assert.equal(await store.get("device-keys/v2"), undefined);
});

test("a store that fails is still the storage message, with its retry — and the support check ran before the store was read", async () => {
  recordSubtle();
  signedIn();
  const store = new LoggedStore();
  store.failGets = true;
  await render(store);
  await waitFor("the device keys screen", () => byTestId("keys-unreadable-why") !== null);
  assert.equal(byTestId("keys-unreadable-why")!.textContent, "This browser's storage did not answer when its device keys were read. Nothing was replaced: try again in a moment.");
  assert.ok(byTestId("keys-retry"));
  assert.equal(byTestId("unsupported-why"), null);
  // The check's probes (X25519, Ed25519, AES-GCM, HKDF, HMAC) all came before the first read of the store.
  const firstRead = log.findIndex((l) => l.startsWith("store."));
  assert.ok(firstRead > 0, "the store was read");
  for (const probe of ["generateKey X25519 false", "generateKey Ed25519 false", "generateKey AES-GCM false", "importKey HKDF", "importKey HMAC"]) {
    const at = log.indexOf(probe);
    assert.ok(at >= 0 && at < firstRead, `${probe} before the store (${log.slice(0, firstRead + 1).join(" | ")})`);
  }
});

test("signed out on a browser that cannot: told first, not shown the sign-in form", async () => {
  restores.push(removeNative(["Ed25519", "HKDF"]));
  window.history.replaceState(null, "", "/signin");
  await render(new LoggedStore());
  await waitFor("the unsupported screen", () => byTestId("unsupported-why") !== null);
  assert.equal(byTestId("auth-email"), null, "no sign-in form");
  assert.equal(byTestId("compat-offer"), null);
  assert.equal(byTestId("unsupported-sign-out"), null, "nothing to sign out of");
});

test("signed out, lacking only the curves: agreeing to compatibility mode leads on to the sign-in form, and is kept", async () => {
  restores.push(removeNative(["Ed25519", "X25519"]));
  window.history.replaceState(null, "", "/signin");
  await render(new LoggedStore());
  await waitFor("the offer", () => byTestId("compat-offer") !== null);
  assert.equal(byTestId("auth-email"), null);
  await click(byTestId("compat-understood"));
  await click(byTestId("compat-trust"));
  await waitFor("the sign-in form", () => byTestId("auth-email") !== null);
  assert.deepEqual(JSON.parse(localStorage.getItem(COMPAT_CONSENT_KEY)!).missing, ["X25519", "Ed25519"]);
});

test("apps' built-in browsers are told to open the system browser", async () => {
  const ANDROID = "Mozilla/5.0 (Linux; Android 13; V2111A Build/TP1A.220624.014; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/107.0.5304.141 Mobile Safari/537.36";
  assert.equal(inAppBrowser(`${ANDROID} XWEB/5023 MMWEBSDK/20230701 MMWEBID/1234 MicroMessenger/8.0.40.2420(0x28002837) WeChat/arm64 Weixin NetType/WIFI Language/zh_CN ABI/arm64`), "wechat");
  assert.equal(inAppBrowser(`${ANDROID} V1_AND_SQ_8.9.68_4264_YYB_D QQ/8.9.68.11565 NetType/WIFI WebP/0.3.0 AppId/537168313`), "qq");
  assert.equal(inAppBrowser(`${ANDROID} UCBS/2.11.1.28 Nebula AlipayDefined(nt:WIFI,ws:393|0|2.75) AliApp(AP/10.5.36.8000) AlipayClient/10.5.36.8000 Language/zh-Hans`), "alipay");
  assert.equal(inAppBrowser(`${ANDROID} aweme_270400 JsSdk/1.0 NetType/WIFI Channel/douyin_tengxun_wzl_1108 app_version/27.4.0 ByteLocale/zh-CN`), "douyin");
  assert.equal(inAppBrowser(`${ANDROID} Weibo (Xiaomi-2201123C__weibo__13.6.3__android__android13)`), "weibo");
  assert.equal(inAppBrowser(ANDROID), "webview");
  assert.equal(inAppBrowser("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.6367.54 Mobile Safari/537.36"), null);
  assert.equal(inAppBrowser("Mozilla/5.0 (Linux; U; Android 13; zh-cn; 2201123C Build/TKQ1) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/109.0.5414.86 MQQBrowser/14.2 Mobile Safari/537.36"), null, "QQ Browser is a browser of its own");
  assert.equal(inAppBrowser("Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1"), null);

  await i18n.changeLanguage("zh-CN");
  root = createRoot(container);
  await act(async () =>
    root!.render(
      <I18nextProvider i18n={i18n}>
        <UnsupportedBrowserScreen missing={["Ed25519"]} userAgent={`${ANDROID} MicroMessenger/8.0.40.2420`} />
      </I18nextProvider>,
    ),
  );
  assert.match(byTestId("in-app-browser")!.textContent!, /微信内置的浏览器.*系统浏览器/);
  assert.equal(byTestId("unsupported-missing")!.textContent, "缺少：Ed25519");
});

// Last: it puts this page in compatibility mode for good.
test("agreed after signing in: the keys are made only then, in compatibility mode, and the agreement is kept", async () => {
  restores.push(removeNative(["Ed25519", "X25519"]));
  signedIn();
  const store = new LoggedStore();
  await render(store);
  await waitFor("the offer", () => byTestId("compat-offer") !== null);
  assert.deepEqual(log.filter((l) => l.startsWith("store.")), [], "nothing read or made before");
  await click(byTestId("compat-understood"));
  assert.equal((byTestId("compat-trust") as HTMLButtonElement).disabled, false);
  log.push("agreed");
  await click(byTestId("compat-trust"));
  await waitFor("the device keys", () => log.includes("store.put device-keys/v2"));
  assert.ok(log.indexOf("agreed") < log.findIndex((l) => l.startsWith("store.")), "the store is touched only after the agreement");
  assert.equal(compatModeActive(), true);
  assert.deepEqual(JSON.parse(localStorage.getItem(COMPAT_CONSENT_KEY)!).missing, ["X25519", "Ed25519"]);
  const v2 = (await store.get("device-keys/v2")) as { version: number; sign: { sealed: Uint8Array } };
  assert.equal(v2.version, 2, "kept as sealed PKCS #8, as natively");
  assert.equal(v2.sign.sealed.length, 48 + 16);
  // From here on the page says so — in the bar, on a phone's width too (compact) — and tapped, offers to stop.
  await act(async () => root?.unmount());
  const { CompatBadge } = await import("../src/features/cloud/browser-support.js");
  root = createRoot(container);
  await act(async () =>
    root!.render(
      <I18nextProvider i18n={i18n}>
        <CompatBadge compact />
      </I18nextProvider>,
    ),
  );
  const badge = byTestId("compat-mode")!;
  assert.deepEqual([...badge.querySelectorAll("span")].map((e) => [e.className, e.textContent]), [
    ["md:hidden", "compat."],
    ["max-md:hidden", "compatibility mode"],
  ]);
  await click(badge);
  const withdraw = document.querySelector<HTMLElement>('[data-testid="compat-withdraw"]');
  assert.ok(withdraw, "the badge opens what it means, with a way to stop");
  assert.match(document.querySelector('[data-testid="compat-card"]')!.textContent!, /plain bytes in the page's memory/);
  await click(withdraw);
  assert.equal(localStorage.getItem(COMPAT_CONSENT_KEY), null, "the agreement is withdrawn: the next page asks again");
});
