// The merged settings tab: "access" is this device's identity in one page —
// its local login, then (on the computer's own page, never on droppen.app) its
// Droppen link. The old ids `?section=security` and `?section=Droppen` still
// open it, and the two halves share one sheet instead of stacking two.
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
import { AccessSection } from "../src/features/settings/access-section.js";
import { askedSection } from "../src/features/settings/sections.js";
import { cloudKey } from "../src/features/link/link-settings.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { mq } from "../src/lib/machines/query.js";

const i18n = i18next.createInstance();
await i18n.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} } },
  interpolation: { escapeValue: false },
});

function cloud(state: string) {
  return {
    state,
    localAuth: true,
    canManage: true,
    hub: "wss://hub.example",
    hubWeb: "https://hub.example",
    owner: "me",
    accounts: [{ user: "me", owner: true, remote: true }],
    aggregator: { enabled: false },
    account: null,
    machine: { hostname: "laptop", id: "m-1", fingerprint: "cc dd" },
    link: null,
  };
}

/** Render the tab as a computer's own page serves it (the default: no rh-hub meta). */
function draw(): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "auth-status"), { authRequired: true, setupRequired: false, user: "me", desktopApp: false });
  client.setQueryData(mq(LOCAL, "auth-sessions"), []);
  client.setQueryData(cloudKey(LOCAL), cloud("none"));
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>
        <AccessSection />
      </QueryClientProvider>
    </I18nextProvider>,
  );
  client.clear();
  return html;
}

test("the old section ids open access; unknown ones do not", () => {
  assert.equal(askedSection("security"), "access");
  assert.equal(askedSection("Droppen"), "access");
  assert.equal(askedSection("access"), "access");
  assert.equal(askedSection("plugins"), "plugins");
  assert.equal(askedSection("nope"), undefined);
  assert.equal(askedSection(undefined), undefined);
});

test("the login comes first and the Droppen link follows on the same page", () => {
  const html = draw();
  // One sheet, not two: the halves are sections inside the tab's own frame.
  assert.equal(html.match(/data-sheet=""/g)?.length, 1);
  assert.match(html, />signed in</);
  assert.match(html, /data-testid="droppen-section"/);
  assert.ok(html.indexOf(">signed in<") < html.indexOf('data-testid="droppen-section"'));
  assert.match(html, />signed-in devices</);
  assert.match(html, /data-testid="droppen-sign-in"/);
});

test("on droppen.app the tab keeps the login and leaves the Droppen half out", () => {
  // Cloud mode is the hub's `<meta name="rh-hub">` in the shell (`lib/hub/config.ts`).
  const before = globalThis.document;
  globalThis.document = {
    querySelector: (selector: string) => (selector === 'meta[name="rh-hub"]' ? { getAttribute: () => "wss://hub.example" } : null),
  } as unknown as Document;
  try {
    const html = draw();
    assert.match(html, />signed in</);
    assert.match(html, /type="password"/);
    assert.doesNotMatch(html, /data-testid="droppen-section"/);
  } finally {
    globalThis.document = before;
  }
});
