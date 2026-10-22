// The /link consent page (S1-4, docs/local-aggregator.md §4.3 step 4) has no
// 「可管理账号」 switch any more. The computer that asks runs AI, so it is
// added as a device that runs AI and never as a full device (review D1) —
// there is nothing to choose. The page is driven as the browser would: the
// computer's page sends its request, the person clicks "add this device".
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { LinkFlow, startHandshake } from "../src/features/link/link-popup.js";
import type { CloudAccount } from "../src/lib/hub/account.js";
import type { HubLink } from "../src/lib/hub/link.js";
import type { KeyStore } from "../src/lib/e2e/index.js";

type El = React.ReactElement<Record<string, unknown>>;

/** Every element in a tree a component returned (not rendered further). */
function elements(node: unknown, out: El[] = []): El[] {
  if (Array.isArray(node)) for (const n of node) elements(n, out);
  else if (node && typeof node === "object" && "props" in node) {
    out.push(node as El);
    elements((node as El).props.children, out);
  }
  return out;
}

test("the consent page asks nothing about what the device may do, and adds it with admin false", async () => {
  const i18n = i18next.createInstance();
  await i18n.use(initReactI18next).init({ lng: "en", resources: { en: { translation: {} } }, interpolation: { escapeValue: false }, keySeparator: false, nsSeparator: false });
  // The computer's page that opened the popup (on this device's loopback), and the popup's window.
  const posted: { t: string }[] = [];
  const opener = { postMessage: (message: { t: string }) => void posted.push(message) };
  const listeners: ((event: MessageEvent) => void)[] = [];
  const win = { opener, addEventListener: (_: string, listener: (event: MessageEvent) => void) => void listeners.push(listener) } as unknown as Window;
  const g = globalThis as Record<string, unknown>;
  const saved = { window: g.window, location: g.location };
  g.window = win;
  g.location = { origin: "https://droppen.app" };
  try {
    startHandshake(win);
    const request = { t: "rh-link/request", v: 1, nonce: "0".repeat(32), machine: { hostname: "studio", signKey: "ab".repeat(32), dhKey: "cd".repeat(32) }, aggregator: null, aggregate: false };
    for (const listener of listeners) listener({ source: opener, origin: "http://127.0.0.1:5173", data: request } as unknown as MessageEvent);

    // An account that records what adding asks the hub for, and stops there.
    const enrollments: unknown[][] = [];
    const account = {
      account: "acct-1",
      session: { token: "tok" },
      api: { me: async () => ({ account: "acct-1", email: { address: "me@example.test" }, plan: null, chainSeq: 0, chainV3: null, machines: [], devices: [] }) },
      chain: { isV3: false },
      startEnrollment: async () => ({ accountFingerprint: new Uint8Array(16), secret: new Uint8Array(16) }),
      completeEnrollment: async (...args: unknown[]) => {
        enrollments.push(args);
        throw new Error("stop here");
      },
    } as unknown as CloudAccount;
    const link = new Proxy({ presence: new Map() } as Record<string, unknown>, { get: (t, k) => (k in t ? t[k as string] : () => () => undefined) }) as unknown as HubLink;

    let tree: El[] = [];
    function Probe() {
      const out = LinkFlow({ account, link, store: {} as KeyStore });
      tree = elements(out);
      return out;
    }
    const html = renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <Probe />
      </I18nextProvider>,
    );
    assert.match(html, /data-testid="link-consent"/, html);
    assert.match(html, /data-testid="link-runs-ai"/, "it says the device runs AI");
    assert.doesNotMatch(html, /role="switch"|type="checkbox"|link-manages|manage the account|full device/, "nothing to choose, nothing said about it");

    // "add this device": the code goes to the computer's page, and the enrollment is asked for as a device that is not a full device.
    const add = tree.find((e) => e.type === "button" && e.props["data-testid"] === "link-confirm");
    assert.ok(add, "add this device");
    assert.equal(add.props.disabled, false);
    (add.props.onClick as () => void)();
    for (let i = 0; i < 50 && enrollments.length === 0; i++) await new Promise((resolve) => setTimeout(resolve, 0));
    assert.equal(enrollments.length, 1, "the enrollment was asked for");
    assert.deepEqual(enrollments[0]![4], { writable: false, admin: false });
    assert.ok(posted.some((m) => m.t === "rh-link/code"), "the code went to the computer's page");
  } finally {
    g.window = saved.window;
    g.location = saved.location;
  }
});
