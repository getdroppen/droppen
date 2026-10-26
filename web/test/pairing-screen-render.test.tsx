// The pairing screen (`features/cloud/pairing-screen.tsx`) drawn for real:
// every way a pairing stops, in zh-CN and English, in words — never the
// hub's message — with the way on that can work; the code's countdown and
// the window's; the screen saying the window has shut before asking the hub
// (and asking nothing of it); "sign in again" handing over the tab it was
// on; and the sign-in screen saying why the reader is back there.
//
// React renders into a stand-in DOM (`fake-dom.ts`) where effects run; the
// static states are drawn with `react-dom/server`. `import * as React`
// because the test directory is outside the tsconfig's `include`.
import * as React from "react";
import assert from "node:assert/strict";
import test, { after } from "node:test";
import { installFakeDom, type FakeNode } from "./fake-dom.js";

Object.assign(globalThis, { location: { pathname: "/", search: "", hash: "", reload: () => undefined, replace: () => undefined } });
const container = installFakeDom();
after(() => void setTimeout(() => process.exit(), 50));

const { createRoot } = await import("react-dom/client");
const { renderToStaticMarkup } = await import("react-dom/server");
const { I18nextProvider } = await import("react-i18next");
const { default: i18n } = await import("./i18n-all.js");
const { CodeShown, countdown, PairingScreen, PairingStopped, pairingActions } = await import("../src/features/cloud/pairing-screen.js");
const { AuthScreen } = await import("../src/features/cloud/cloud-app.js");
const { PAIRING_RESUME_KEY } = await import("../src/features/cloud/sign-out.js");
const { HubApiError } = await import("../src/lib/hub/api.js");
type CloudAccount = import("../src/lib/hub/account.js").CloudAccount;
type PairingCause = import("../src/lib/hub/pairing.js").PairingCause;

const MIN = 60_000;
const flush = (ms = 20) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(done: () => boolean, ms = 5_000): Promise<void> {
  const end = Date.now() + ms;
  while (!done()) {
    if (Date.now() > end) throw new Error("timed out waiting");
    await flush(5);
  }
}

/** Text without markup, as a reader sees it. */
const text = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/&#x27;/g, "'").replace(/&quot;/g, '"').replace(/&amp;/g, "&").replace(/\s+/g, " ");

async function draw(node: React.ReactElement, lng: "en" | "zh-CN"): Promise<string> {
  await i18n.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
}

const noop = () => undefined;
const HUB_WORDS = /registered too long ago|too many pairings are open|no such pairing|has had all its messages|sign in again on it/;

test("countdown: m:ss, rounded up, never below 0:00", () => {
  assert.equal(countdown(0), "0:00");
  assert.equal(countdown(-5_000), "0:00");
  assert.equal(countdown(1), "0:01");
  assert.equal(countdown(61_000), "1:01");
  assert.equal(countdown(9 * MIN + 41_000), "9:41");
  assert.equal(countdown(30 * MIN), "30:00");
});

test("the ways on: starting again only where it can work", () => {
  const open = { again: false, codePath: true, codeClosed: false };
  const shut = { again: false, codePath: true, codeClosed: true };
  assert.deepEqual(pairingActions("windowClosed", open), ["signInAgain", "useQr"], "a new device: sign in again (a new window), or the QR code");
  assert.deepEqual(pairingActions("windowClosed", { ...open, again: true }), ["useQr"], "a device of the chain cannot sign in as a new one");
  assert.deepEqual(pairingActions("tooMany", open), ["restart", "useQr"], "the QR code needs none of the account's codes");
  for (const cause of ["expired", "full", "wrongCode", "badDelivery", "rate", "offline", "unavailable", "other"] as const) {
    assert.deepEqual(pairingActions(cause, open), ["restart"], cause);
    assert.deepEqual(pairingActions(cause, shut), ["signInAgain", "useQr"], `${cause}, the window shut: a new code needs a sign-in`);
    assert.deepEqual(pairingActions(cause, { ...shut, codePath: false }), ["restart"], `${cause} on the QR code: no window`);
  }
});

/** What each cause says (title, then a piece of the explanation), and its main button, in both languages. */
const SAID: Record<PairingCause, { en: [string, string, string]; zh: [string, string, string] }> = {
  windowClosed: {
    en: ["sign in again to use a code", "This browser signed in more than 30 minutes ago. To keep the account safe, sign in again before adding it with a code", "sign in again"],
    zh: ["需要重新登录才能用数字码", "这个浏览器登录已超过 30 分钟，为了安全，需要重新登录后再用数字码添加。也可以改用二维码，不用重新登录。", "重新登录"],
  },
  tooMany: {
    en: ["too many codes are open", "This account has as many codes open as it may.", "start again"],
    zh: ["同时打开的数字码太多", "这个账户同时打开的数字码已达上限，每个数字码 10 分钟后失效", "重新开始"],
  },
  expired: { en: ["the code expired", "Nobody entered it within 10 minutes.", "start again"], zh: ["数字码已过期", "10 分钟内没有人输入它。", "重新开始"] },
  full: { en: ["this code was tried too often", "It can no longer be used.", "start again"], zh: ["这个数字码尝试次数过多", "它已不能再使用。", "重新开始"] },
  wrongCode: {
    en: ["the code did not work", "The other device entered a different code, or did not confirm it in time.", "start again"],
    zh: ["数字码无效", "另一台设备输入的数字码不对，或没有及时确认。这个数字码已作废。", "重新开始"],
  },
  badDelivery: {
    en: ["the keys that arrived do not check out: not using them", "check that the other device shows the same fingerprint", "start again"],
    zh: ["收到的钥匙校验未通过，不会使用", "请重新开始，并核对另一台设备上显示的指纹与这里一致。", "重新开始"],
  },
  rate: { en: ["pairing stopped", "too many attempts: try again in 3 s", "start again"], zh: ["配对已中止", "操作太频繁", "重新开始"] },
  offline: { en: ["pairing stopped", "Droppen could not be reached. Check the connection, then start again.", "start again"], zh: ["配对已中止", "连不上 Droppen。请检查网络连接，然后重新开始。", "重新开始"] },
  unavailable: { en: ["pairing stopped", "Droppen is not answering properly right now.", "start again"], zh: ["配对已中止", "Droppen 暂时无法正常响应，请稍后重新开始。", "重新开始"] },
  other: { en: ["pairing stopped", "Something went wrong while pairing. Start again.", "start again"], zh: ["配对已中止", "配对时出了问题，请重新开始。", "重新开始"] },
};

test("every way a pairing stops, in zh-CN and English: what happened, in words, and the way on", async () => {
  for (const [cause, said] of Object.entries(SAID) as [PairingCause, (typeof SAID)[PairingCause]][]) {
    const actions = pairingActions(cause, { again: false, codePath: true, codeClosed: false });
    for (const [lng, [title, why, main]] of [["en", said.en], ["zh-CN", said.zh]] as const) {
      const html = await draw(<PairingStopped cause={cause} retryAfterMs={cause === "rate" ? 2_500 : undefined} actions={actions} again={false} windowLeftMs={null} onAction={noop} />, lng);
      const shown = text(html);
      assert.ok(shown.includes(title), `${cause} ${lng}: ${title} — in ${shown}`);
      assert.ok(shown.includes(why), `${cause} ${lng}: ${why} — in ${shown}`);
      assert.match(html, new RegExp(`data-variant="primary" data-action="${actions[0]}"[^>]*>${main}<`), `${cause} ${lng}: the main button`);
      assert.match(html, new RegExp(`data-cause="${cause}"`));
      assert.doesNotMatch(shown, HUB_WORDS, "never the hub's message");
      if (lng === "zh-CN") assert.doesNotMatch(shown, /\b(the|code|pairing|sign)\b/i, `${cause}: no English left in zh-CN`);
    }
  }
});

test("the window shut: sign in again (or the QR code) for a new browser; the QR code alone for a device of the chain", async () => {
  const fresh = await draw(<PairingStopped cause="windowClosed" actions={pairingActions("windowClosed", { again: false, codePath: true, codeClosed: true })} again={false} windowLeftMs={null} onAction={noop} />, "zh-CN");
  assert.match(fresh, /data-testid="pairing-window-closed"/);
  assert.match(fresh, /data-signal="warn"/, "a warning, not a failure");
  assert.match(fresh, /data-action="signInAgain"[^>]*>重新登录</);
  assert.match(fresh, /data-action="useQr"[^>]*>改用二维码</);
  assert.doesNotMatch(fresh, /重新开始/, "starting again as this device would only be refused again");
  const again = text(await draw(<PairingStopped cause="windowClosed" actions={pairingActions("windowClosed", { again: true, codePath: true, codeClosed: true })} again windowLeftMs={null} onAction={noop} />, "zh-CN"));
  assert.ok(again.includes("请改用二维码"));
  assert.ok(again.includes("这台设备登录已超过 30 分钟，为了安全，不能再用数字码配对，请改用二维码。"));
  assert.doesNotMatch(again, /重新登录|重新开始/);
  const en = text(await draw(<PairingStopped cause="windowClosed" actions={["useQr"]} again windowLeftMs={null} onAction={noop} />, "en"));
  assert.ok(en.includes("use the QR code instead") && en.includes("it can no longer pair with a code"));
});

test("another failure once the window shut says why starting again is not offered", async () => {
  const actions = pairingActions("expired", { again: false, codePath: true, codeClosed: true });
  const zh = text(await draw(<PairingStopped cause="expired" actions={actions} again={false} windowLeftMs={null} onAction={noop} />, "zh-CN"));
  assert.ok(zh.includes("数字码已过期"));
  assert.ok(zh.includes("这个浏览器登录已超过 30 分钟：请重新登录，再获取新的数字码。"));
  assert.ok(zh.includes("重新登录") && !zh.includes("重新开始"));
  const en = text(await draw(<PairingStopped cause="expired" actions={actions} again={false} windowLeftMs={null} onAction={noop} />, "en"));
  assert.ok(en.includes("This browser signed in more than 30 minutes ago: sign in again for a new code."));
});

test("start again on the code tab says how long a new code can still be made", async () => {
  const node = <PairingStopped cause="expired" actions={["restart"]} again={false} windowLeftMs={12 * MIN + 34_000} onAction={noop} />;
  assert.ok(text(await draw(node, "zh-CN")).includes("这里还能在 12:34 内获取新的数字码"));
  assert.ok(text(await draw(node, "en")).includes("a new code can be made here for another 12:34"));
  const last = <PairingStopped cause="expired" actions={["restart"]} again={false} windowLeftMs={10_000} onAction={noop} />;
  assert.doesNotMatch(await draw(last, "en"), /pairing-window-left/, "not with seconds left");
});

test("the code's countdown, and — when the window shuts first — that a new code would need a fresh sign-in", async () => {
  const code = { kind: "code" as const, code: "12345678", fingerprint: "ABCD-EFGH", expiresMs: 0 };
  const plain = (lng: "en" | "zh-CN") => draw(<CodeShown state={code} again={false} codeLeftMs={9 * MIN + 41_000} windowLeftMs={25 * MIN} onRestart={noop} />, lng);
  assert.ok(text(await plain("zh-CN")).includes("等待输入数字码：还剩 9:41，仅限一次"));
  assert.ok(text(await plain("en")).includes("waiting for the code to be entered: 9:41 left, once"));
  assert.doesNotMatch(await plain("en"), /pairing-window-left/, "the window outlasts the code: nothing to say");
  const closing = (lng: "en" | "zh-CN", again = false) => draw(<CodeShown state={code} again={again} codeLeftMs={9 * MIN} windowLeftMs={3 * MIN + 59_000} onRestart={noop} />, lng);
  assert.ok(text(await closing("zh-CN")).includes("3:59 后，再要新的数字码就需要重新登录；这个数字码在过期前仍然有效。"));
  assert.ok(text(await closing("en")).includes("In 3:59 a new code needs a fresh sign-in: this one still works until it lapses."));
  assert.ok(text(await closing("zh-CN", true)).includes("3:59 后，这台设备就不能再获取数字码，之后请改用二维码。"));
  const shut = (lng: "en" | "zh-CN") => draw(<CodeShown state={code} again={false} codeLeftMs={5 * MIN} windowLeftMs={-MIN} onRestart={noop} />, lng);
  assert.ok(text(await shut("zh-CN")).includes("这个数字码仍然有效；再要新的需要重新登录。"));
  assert.ok(text(await shut("en")).includes("This code still works; a new one needs a fresh sign-in."));
  const typed = text(await draw(<CodeShown state={{ kind: "answered", code: "12345678", fingerprint: "ABCD-EFGH" }} again={false} codeLeftMs={null} windowLeftMs={null} onRestart={noop} />, "zh-CN"));
  assert.ok(typed.includes("另一台设备已输入数字码，等待它确认…"));
});

// ------------------------------------------------------------ mounted

/** A browser outside the chain, as the screen uses one: `/v1/me` lists it registered `registeredAgoMs` ago (`null`: not listed). */
function browser(opts: { registeredAgoMs: number | null; openPairing?: () => Promise<string>; again?: boolean; isAdmin?: boolean }) {
  const calls = { openPairing: 0, closePairing: 0, me: 0 };
  const key = (b: number) => new Uint8Array(32).fill(b);
  const api = {
    me: async () => {
      calls.me++;
      return {
        account: "a-1",
        email: { address: null, verified: true, verification: "off", dueMs: null, delivery: null, deliveryError: null },
        plan: null,
        chainSeq: 0,
        chainV3: null,
        machines: [],
        devices: opts.registeredAgoMs === null ? [] : [{ id: "d-new", createdMs: Date.now() - opts.registeredAgoMs, lastSeenMs: null, inChain: false, kind: null }],
      };
    },
    openPairing: async () => {
      calls.openPairing++;
      return opts.openPairing ? opts.openPairing() : "p-1";
    },
    closePairing: async () => void calls.closePairing++,
    mailbox: async () => [],
    ackMail: async () => undefined,
    pairingMessages: async () => [],
    postPairingMessage: async () => 1,
  };
  const account = {
    account: "a-1",
    deviceId: "d-new",
    session: { token: "t" },
    api,
    keys: { signPublic: key(7), dhPublic: key(9) },
    noiseKeys: { publicKey: key(9), privateKey: key(3) },
    sync: async () => 0,
    chain: null,
    storeUserKeys: async () => undefined,
    pinned: false,
    standing: opts.again ? "active" : "absent",
    isAdmin: opts.isAdmin ?? false,
    pairing: false,
  };
  return { account: account as unknown as CloudAccount, calls };
}

function find(node: FakeNode, match: (n: FakeNode) => boolean): FakeNode | null {
  if (match(node)) return node;
  for (const c of node.childNodes) {
    const hit = find(c, match);
    if (hit) return hit;
  }
  return null;
}

/** Press a button as React would hear it: its own `onClick`. */
function press(action: string): void {
  const button = find(container, (n) => n.getAttribute?.("data-action") === action);
  assert.ok(button, `a button for ${action}`);
  const props = Object.entries(button as unknown as Record<string, unknown>).find(([k]) => k.startsWith("__reactProps$"))?.[1] as { onClick: (e: unknown) => void };
  props.onClick({ preventDefault: noop, stopPropagation: noop });
}

async function mount(node: React.ReactNode, lng: "en" | "zh-CN" = "en") {
  await i18n.changeLanguage(lng);
  const errors: unknown[] = [];
  const root = createRoot(container as never, { onUncaughtError: (e) => void errors.push(e), onCaughtError: (e) => void errors.push(e) });
  root.render(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
  return { root, errors, shown: () => (container as FakeNode).textContent };
}

test("back from signing in again on the code tab, the window shut already: said up front, and the hub is asked for nothing", async () => {
  sessionStorage.setItem(PAIRING_RESUME_KEY, "code");
  const { account, calls } = browser({ registeredAgoMs: 31 * MIN });
  const asked: string[] = [];
  const m = await mount(<PairingScreen account={account} onPaired={noop} onPending={noop} onSignOut={noop} onSignInAgain={(mode) => void asked.push(mode)} />, "zh-CN");
  try {
    await until(() => m.shown().includes("这个浏览器登录已超过 30 分钟"));
    assert.ok(m.shown().includes("需要重新登录才能用数字码"));
    assert.equal(sessionStorage.getItem(PAIRING_RESUME_KEY), null, "read once");
    await flush(50);
    assert.equal(calls.openPairing, 0, "never asked of the hub");
    press("signInAgain");
    assert.deepEqual(asked, ["code"], "comes back to the code tab");
    // The QR code needs no window.
    press("useQr");
    await until(() => m.shown().includes("scan this code") || m.shown().includes("扫描这个二维码"));
    assert.equal(calls.openPairing, 0);
    assert.deepEqual(m.errors, []);
  } finally {
    m.root.unmount();
  }
});

test("the code tab with the window open: a code, its countdown, and the window's once it shuts first", async () => {
  sessionStorage.setItem(PAIRING_RESUME_KEY, "code");
  const { account, calls } = browser({ registeredAgoMs: 27 * MIN });
  const m = await mount(<PairingScreen account={account} onPaired={noop} onPending={noop} onSignOut={noop} />);
  try {
    await until(() => /waiting for the code to be entered: \d+:\d\d left, once/.test(m.shown()));
    assert.match(m.shown(), /(10:00|9:5\d) left, once/);
    await until(() => /In 2:\d\d a new code needs a fresh sign-in/.test(m.shown()));
    assert.equal(calls.openPairing, 1);
  } finally {
    m.root.unmount();
  }
  await flush(20);
  assert.equal(calls.closePairing, 1, "the screen gone, its code closed");
});

/** Choose a tab as React would hear it: the tab's own `onClick`. */
function tab(label: string): void {
  const button = find(container, (n) => n.getAttribute?.("role") === "tab" && n.textContent === label);
  assert.ok(button, `a tab ${label}`);
  const props = Object.entries(button as unknown as Record<string, unknown>).find(([k]) => k.startsWith("__reactProps$"))?.[1] as { onClick: (e: unknown) => void };
  props.onClick({ preventDefault: noop, stopPropagation: noop });
}

test("the window shuts while a code is open: to the QR code and back, the code is still there — it works until it lapses", async () => {
  sessionStorage.setItem(PAIRING_RESUME_KEY, "code");
  // 16 s of window left: a code opens, and the window shuts (15 s before its end) a moment later.
  const { account, calls } = browser({ registeredAgoMs: 30 * MIN - 16_000 });
  const m = await mount(<PairingScreen account={account} onPaired={noop} onPending={noop} onSignOut={noop} />);
  try {
    await until(() => /left, once/.test(m.shown()));
    await until(() => /In 0:1[0-5] a new code needs a fresh sign-in/.test(m.shown()) || m.shown().includes("This code still works"));
    tab("QR code");
    await until(() => m.shown().includes("scan this code"));
    tab("use a code instead");
    await until(() => /left, once/.test(m.shown()) || m.shown().includes("sign in again to use a code"));
    assert.match(m.shown(), /left, once/, "the open code, not the closed window");
    assert.equal(calls.openPairing, 1, "the same code: none asked for");
    assert.deepEqual(m.errors, []);
  } finally {
    m.root.unmount();
  }
});

test("too many codes open in the account: said in words, start again or the QR code", async () => {
  sessionStorage.setItem(PAIRING_RESUME_KEY, "code");
  const { account } = browser({
    registeredAgoMs: 2 * MIN,
    openPairing: () => Promise.reject(new HubApiError(429, "too_many_pairings", "too many pairings are open for this account")),
  });
  const m = await mount(<PairingScreen account={account} onPaired={noop} onPending={noop} onSignOut={noop} />);
  try {
    await until(() => m.shown().includes("too many codes are open"));
    assert.doesNotMatch(m.shown(), HUB_WORDS);
    assert.ok(find(container, (n) => n.getAttribute?.("data-action") === "restart"));
    press("useQr");
    await until(() => m.shown().includes("scan this code"));
  } finally {
    m.root.unmount();
  }
});

test("the hub refuses a code as past the window although the screen could not tell: sign in again, never its message", async () => {
  sessionStorage.setItem(PAIRING_RESUME_KEY, "code");
  const { account } = browser({
    registeredAgoMs: null,
    openPairing: () => Promise.reject(new HubApiError(403, "pairing_window_closed", "this device registered too long ago to open a pairing: sign in again on it")),
  });
  const asked: string[] = [];
  const m = await mount(<PairingScreen account={account} onPaired={noop} onPending={noop} onSignOut={noop} onSignInAgain={(mode) => void asked.push(mode)} />, "zh-CN");
  try {
    await until(() => m.shown().includes("需要重新登录才能用数字码"));
    assert.ok(m.shown().includes("重新登录"));
    assert.doesNotMatch(m.shown(), HUB_WORDS);
    assert.ok(!find(container, (n) => n.getAttribute?.("data-action") === "restart"), "no 重新开始: it would be refused again");
    press("signInAgain");
    assert.deepEqual(asked, ["code"]);
  } finally {
    m.root.unmount();
  }
});

test("a device that manages the account has no window: its code tab opens a code however long ago it signed in", async () => {
  sessionStorage.setItem(PAIRING_RESUME_KEY, "code");
  const { account, calls } = browser({ registeredAgoMs: 3 * 24 * 60 * MIN, again: true, isAdmin: true });
  const m = await mount(<PairingScreen account={account} again onPaired={noop} onPending={noop} onSignOut={noop} />);
  try {
    await until(() => /left, once/.test(m.shown()));
    assert.equal(calls.openPairing, 1);
    assert.doesNotMatch(m.shown(), /fresh sign-in|sign in again/);
  } finally {
    m.root.unmount();
  }
});

test("the sign-in screen says why the reader is back, only after the pairing screen's sign in again", async () => {
  const auth = (lng: "en" | "zh-CN") => draw(<AuthScreen api={{} as never} onSignedIn={noop} />, lng);
  sessionStorage.removeItem(PAIRING_RESUME_KEY);
  assert.doesNotMatch(await auth("en"), /signin-to-pair/);
  sessionStorage.setItem(PAIRING_RESUME_KEY, "code");
  assert.ok(text(await auth("en")).includes("Sign in again to go on adding this browser: you come back to where you were."));
  assert.ok(text(await auth("zh-CN")).includes("重新登录后，会回到添加这个浏览器的页面。"));
  sessionStorage.removeItem(PAIRING_RESUME_KEY);
});
