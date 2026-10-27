// droppen.app's consent page answering a browser hand-off (docs/local-aggregator.md
// §4.9), clicked for real — mounted into a real DOM (jsdom) with
// an account that answers `/v1/me` and issues codes when asked:
//
// - the tab that hands the code over is opened in the click, and closed again
//   on every way out before a code is sealed (the plan's limit, a cancel);
// - a second click while the first is under way issues no second code;
// - while the device is expected to take the code, the person can stop
//   waiting (the device says a refusal in its own tab, not to this one).
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { installLiveDom, until, walk } from "./live-dom.js";

// The page is droppen.app's `/link`.
(globalThis as { location?: unknown }).location = { origin: "https://droppen.app", pathname: "/link", hash: "", search: "", href: "https://droppen.app/link" };
const container = installLiveDom();
const flush = (ms = 30) => new Promise((resolve) => setTimeout(resolve, ms));

const i18next = (await import("i18next")).default;
const { initReactI18next } = await import("react-i18next");
await i18next.use(initReactI18next).init({ lng: "en", resources: {}, keySeparator: false, nsSeparator: false, interpolation: { escapeValue: false } });
const { createRoot } = await import("react-dom/client");
const { handoffHandshake, LinkFlow, setHandshakeForTest } = await import("../src/features/link/link-popup.js");
type HandoffRequest = import("../src/features/link/handoff.js").HandoffRequest;
type CloudAccount = import("../src/lib/hub/account.js").CloudAccount;
type HubLink = import("../src/lib/hub/link.js").HubLink;

const HANDOFF: HandoffRequest = {
  attempt: "0123456789abcdef0123456789abcdef",
  key: "ab".repeat(32),
  state: "cd".repeat(16),
  origin: "http://127.0.0.1:51234",
  expiresMs: Date.now() + 10 * 60_000,
  machine: { hostname: "laptop", signKey: "11".repeat(32), dhKey: "22".repeat(32), enrolled: null },
};

/** The tab `window.open` gives: what it was told, whether it was closed or sent anywhere. */
interface Tab {
  closed: boolean;
  opener: unknown;
  sentTo: string[];
  close(): void;
  document: { title: string; body: { style: { cssText: string }; textContent: string } };
  location: { replace(url: string): void };
}
let tabs: Tab[] = [];
(window as unknown as { open: () => Tab }).open = () => {
  const tab: Tab = {
    closed: false,
    opener: window,
    sentTo: [],
    close() {
      this.closed = true;
    },
    document: { title: "", body: { style: { cssText: "" }, textContent: "" } },
    location: { replace: (url) => void tab.sentTo.push(url) },
  };
  tabs.push(tab);
  return tab;
};

function plan(hosts: number, limit: number | null) {
  return {
    id: "free",
    source: "default",
    status: "active",
    periodStartMs: null,
    periodEndMs: null,
    gracePlan: null,
    graceUntilMs: null,
    entitlements: { "hosts.max": limit },
    usage: { storageBytes: 0, devices: 1, hosts, machines: hosts },
  };
}

/** An account whose `/v1/me` says `hosts` of `limit` used, and whose codes come when `issue` says. */
function account(hosts: number, limit: number | null, issue: () => Promise<unknown>) {
  const asked = { codes: 0 };
  const view = { account: "a-1", email: { address: "me@example.com", verified: true }, plan: plan(hosts, limit), chainSeq: 1, chainV3: null, machines: [], devices: [] };
  const value = {
    account: "a-1",
    api: { me: async () => view },
    session: { token: "t" },
    chain: null,
    startEnrollment: () => {
      asked.codes++;
      return issue();
    },
    completeEnrollment: () => new Promise(() => undefined),
  } as unknown as CloudAccount;
  return { value, asked };
}

const byTestId = (id: string) => walk(container).find((n) => n.getAttribute("data-testid") === id);
function click(id: string) {
  const node = byTestId(id);
  assert.ok(node, `${id} is on the page`);
  const key = Object.keys(node).find((k) => k.startsWith("__reactProps$"));
  assert.ok(key, "rendered by React");
  (node as unknown as Record<string, { onClick: () => void }>)[key].onClick();
}

async function mount(acc: CloudAccount) {
  tabs = [];
  sessionStorage.clear();
  setHandshakeForTest(handoffHandshake(HANDOFF));
  const root = createRoot(container as never);
  root.render(<LinkFlow account={acc} link={null as unknown as HubLink} />);
  await until("the consent page", () => !!byTestId("link-confirm"));
  return root;
}

test("the plan's limit, after the tab was opened in the click: the tab closes and no code is issued", async (t) => {
  const { value, asked } = account(1, 1, async () => ({}));
  const root = await mount(value);
  t.after(() => root.unmount());
  click("link-confirm");
  await until("the limit", () => !!byTestId("plan-limit"));
  assert.equal(tabs.length, 1, "the tab was opened in the click");
  assert.equal(tabs[0].closed, true, "and closed again: no tab left saying it hands a code over");
  assert.deepEqual(tabs[0].sentTo, []);
  assert.equal(asked.codes, 0);
});

test("a second click while the first is under way issues no second code and opens no second tab", async (t) => {
  const { value, asked } = account(0, 3, () => new Promise(() => undefined));
  const root = await mount(value);
  t.after(() => root.unmount());
  click("link-confirm");
  click("link-confirm");
  await until("the code being issued", () => asked.codes > 0);
  await flush();
  assert.equal(asked.codes, 1);
  assert.equal(tabs.length, 1);
  assert.equal(byTestId("link-confirm"), undefined, "no button to click again while it runs");
});

test("cancelled while the code is issued: the code goes nowhere and the tab closes", async (t) => {
  let issue: (code: unknown) => void = () => undefined;
  const { value, asked } = account(0, 3, () => new Promise((resolve) => (issue = resolve)));
  const root = await mount(value);
  t.after(() => root.unmount());
  click("link-confirm");
  await until("the code being issued", () => asked.codes === 1 && !!byTestId("link-handoff-waiting"));
  assert.match(byTestId("link-handoff-waiting")!.textContent, /If the device's tab says it did not take the code, cancel here/);
  click("link-cancel");
  await until("cancelled", () => !!byTestId("link-cancelled"));
  issue({ token: () => new Uint8Array(16) });
  await flush();
  assert.equal(tabs.length, 1);
  assert.deepEqual(tabs[0].sentTo, [], "nothing handed over after the cancel");
  assert.equal(tabs[0].closed, true);
});
