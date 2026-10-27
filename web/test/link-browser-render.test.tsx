// The browser hand-off's screens (docs/local-aggregator.md §4.9), rendered:
// Settings → Access → Droppen's "continue in your browser" (the desktop app's sign-in),
// droppen.app's consent page answering through the browser, and the device's
// /link/return page — in English and in Chinese, device-only wording.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { BrowserSignInWaiting } from "../src/features/link/browser-sign-in.js";
import { cloudKey, DroppenSection } from "../src/features/link/link-settings.js";
import { linkFlowFor } from "../src/features/link/link-flow.js";
import { handoffHandshake, LinkFlow, setHandshakeForTest } from "../src/features/link/link-popup.js";
import { LinkReturnView, type ReturnState } from "../src/features/link/link-return.js";
import type { HandoffRequest } from "../src/features/link/handoff.js";
import type { Attempt } from "../src/features/link/opener.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { apiFor } from "../src/lib/machines/registry.js";
import { cloudDevicesZhCN } from "../src/i18n/cloud-devices.js";
import { linkZhCN } from "../src/i18n/link.js";
import { welcomeZhCN } from "../src/i18n/welcome.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import type { CloudAccount } from "../src/lib/hub/account.js";
import type { HubLink } from "../src/lib/hub/link.js";

async function i18nIn(lng: string, translation: Record<string, string> = {}) {
  const i18n = i18next.createInstance();
  await i18n.use(initReactI18next).init({ lng, resources: { [lng]: { translation } }, interpolation: { escapeValue: false } });
  return i18n;
}
const ZH = { ...cloudDevicesZhCN, ...linkZhCN, ...welcomeZhCN, ...zhCN };
const FORBIDDEN_ZH = /机器|电脑|本机/;

const NOW = 1_800_000_000_000;
const ATTEMPT: Attempt = {
  attempt: "0123456789abcdef0123456789abcdef",
  nonce: "f".repeat(32),
  hubWeb: "https://droppen.app",
  expiresMs: NOW + 14 * 60_000 + 5_000,
  machine: { hostname: "laptop", signKey: "11".repeat(32), dhKey: "22".repeat(32), fingerprint: "a1b2 c3d4 e5f6 0718 293a 4b5c 6d7e 8f90", enrolled: null },
  handoffUrl: "https://droppen.app/link#droppen-link=eyJ2IjoxfQ",
};

test("continue in your browser: the steps, this device's fingerprint, how long the link works, open again, copy, cancel", async () => {
  const draw = async (lng: string, table?: Record<string, string>) =>
    renderToStaticMarkup(
      <I18nextProvider i18n={await i18nIn(lng, table)}>
        <BrowserSignInWaiting attempt={ATTEMPT} onReopen={() => undefined} onCancel={() => undefined} now={NOW} />
      </I18nextProvider>,
    );
  const html = await draw("en");
  assert.match(html, /Continue in your browser/);
  assert.match(html, /Sign in to droppen\.app, or create an account\./);
  assert.match(html, /Check that the fingerprint there matches this one/);
  assert.match(html, /this device joins your account by itself/);
  assert.match(html, /a1b2 c3d4 e5f6 0718 293a 4b5c 6d7e 8f90/);
  assert.match(html, /the link works for another 14:05/);
  assert.match(html, /data-testid="droppen-reopen-browser"/);
  assert.match(html, /data-testid="droppen-copy-link"/);
  assert.match(html, /data-testid="droppen-cancel"/);
  // The link is the attempt's credential while it lasts: said plainly next to "copy link".
  assert.match(html, /data-testid="droppen-link-private"[^>]*>Whoever opens this link can add this device to their own Droppen account: never send it to anyone/);
  const zh = await draw("zh-CN", ZH);
  assert.match(zh, /在浏览器里继续/);
  assert.match(zh, /不要发给任何人/);
  assert.match(zh, /重新打开浏览器/);
  assert.match(zh, /复制链接/);
  assert.match(zh, /链接还能用 14:05/);
  assert.doesNotMatch(zh, FORBIDDEN_ZH);
});

test("Settings → Access → Droppen shows the browser hand-off while it runs", async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(cloudKey(LOCAL), {
    state: "none",
    localAuth: true,
    canManage: true,
    hub: "wss://droppen.app",
    hubWeb: "https://droppen.app",
    owner: null,
    remoteAccess: [],
    aggregator: { enabled: true, state: "off", hosts: [] },
    account: null,
    machine: { hostname: "laptop", id: null, fingerprint: ATTEMPT.machine.fingerprint },
    link: null,
    attempt: { id: ATTEMPT.attempt, expiresMs: ATTEMPT.expiresMs, mine: true, handoffUrl: ATTEMPT.handoffUrl },
  });
  const flow = linkFlowFor(LOCAL, apiFor(LOCAL), client);
  flow.state.set({ kind: "browser", attempt: ATTEMPT });
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={await i18nIn("en")}>
      <QueryClientProvider client={client}>
        <DroppenSection />
      </QueryClientProvider>
    </I18nextProvider>,
  );
  flow.state.set({ kind: "idle" });
  flow.opener.close();
  client.clear();
  assert.match(html, /data-flow="browser"/);
  assert.match(html, /data-testid="droppen-browser-waiting"/);
  assert.match(html, /Continue in your browser/);
  assert.doesNotMatch(html, /data-testid="droppen-sign-in"/, "no second sign-in while one runs");
});

const HANDOFF: HandoffRequest = {
  attempt: ATTEMPT.attempt,
  key: "ab".repeat(32),
  state: "cd".repeat(16),
  origin: "http://127.0.0.1:51234",
  expiresMs: Date.now() + 10 * 60_000,
  machine: { hostname: "laptop", signKey: "11".repeat(32), dhKey: "22".repeat(32), enrolled: null },
};

/** An account as far as the consent page reads it before anything is clicked. */
const ACCOUNT = { account: "a-1", api: { me: () => new Promise(() => undefined) }, session: { token: "t" }, chain: null } as unknown as CloudAccount;

async function consent(handoff: HandoffRequest, lng = "en", table?: Record<string, string>): Promise<string> {
  // The page is droppen.app's.
  (globalThis as { location?: unknown }).location ??= { origin: "https://droppen.app", pathname: "/link", hash: "", search: "" };
  setHandshakeForTest(handoffHandshake(handoff));
  return renderToStaticMarkup(
    <I18nextProvider i18n={await i18nIn(lng, table)}>
      <LinkFlow account={ACCOUNT} link={null as unknown as HubLink} />
    </I18nextProvider>,
  );
}

test("droppen.app's consent page answers a hand-off from the address: where it goes back, the fingerprint to compare, and how", async () => {
  const html = await consent(HANDOFF);
  assert.match(html, /data-testid="link-consent"/);
  assert.match(html, /data-testid="link-origin"[^>]*>http:\/\/127\.0\.0\.1:51234</);
  assert.doesNotMatch(html, /data-testid="link-warning"/, "this device's own loopback: no warning");
  assert.match(html, /data-testid="link-handoff-note"/);
  assert.match(html, /a new tab hands the code to that device, then closes/);
  assert.match(html, /data-testid="link-confirm"(?![^>]*disabled)/, "add this device, enabled");
  // An address on the network is warned about, as a popup's would be.
  const lan = await consent({ ...HANDOFF, origin: "http://192.168.1.20:8700" });
  assert.match(lan, /data-testid="link-warning"/);
  assert.match(lan, /data-testid="link-confirm"[^>]*disabled=""|disabled=""[^>]*data-testid="link-confirm"/);
  assert.match(lan, /data-testid="link-confirm-host"/, "a tick for the local network");
  assert.doesNotMatch(lan, /data-testid="link-type-host"/);
  // A public address: its host name typed out, not a tick; nothing to add until then.
  const pub = await consent({ ...HANDOFF, origin: "https://box.example.com" });
  assert.match(pub, /data-testid="link-warning"/);
  assert.match(pub, /data-testid="link-type-host"/);
  assert.doesNotMatch(pub, /data-testid="link-confirm-host"/);
  assert.match(pub, /To confirm, type its host name: box\.example\.com/);
  assert.match(pub, /data-testid="link-confirm"[^>]*disabled=""|disabled=""[^>]*data-testid="link-confirm"/);
  // The hub itself never.
  const hub = await consent({ ...HANDOFF, origin: "https://droppen.app" });
  assert.ok(hub.includes('data-testid="link-refused"') || hub.includes("link-refused"), hub.slice(0, 200));
  // Past its expiry (by more than the clocks may differ): start again.
  const old = await consent({ ...HANDOFF, expiresMs: Date.now() - 10 * 60_000 });
  assert.match(old, /data-testid="link-expired"/);
  assert.doesNotMatch(old, /data-testid="link-confirm"/);
  const zh = await consent(HANDOFF, "zh-CN", ZH);
  assert.match(zh, /新标签页/);
  assert.doesNotMatch(zh, FORBIDDEN_ZH);
});

test("the device's /link/return page says how handing the sign-in over went", async () => {
  const draw = async (state: ReturnState, lng = "en", table?: Record<string, string>) =>
    renderToStaticMarkup(
      <I18nextProvider i18n={await i18nIn(lng, table)}>
        <LinkReturnView state={state} />
      </I18nextProvider>,
    );
  assert.match(await draw({ kind: "sending" }), /handing the sign-in to Droppen on this device/);
  const done = await draw({ kind: "done" });
  assert.match(done, /data-state="done"/);
  assert.match(done, /This device has the sign-in\./);
  assert.match(await draw({ kind: "nothing" }), /Nothing to hand over here/);
  assert.match(await draw({ kind: "error", code: "expired", message: "" }), /This sign-in ran out of time/);
  assert.match(await draw({ kind: "error", code: "bad_reply", message: "" }), /Nothing was added/);
  assert.match(await draw({ kind: "error", code: "weird", message: "the disk is full" }), /could not take the sign-in: the disk is full/);
  const zh = await draw({ kind: "done" }, "zh-CN", ZH);
  assert.match(zh, /这台设备已收到登录/);
  assert.doesNotMatch(zh, FORBIDDEN_ZH);
});
