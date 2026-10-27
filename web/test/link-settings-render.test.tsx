// The local settings' "Droppen" section for a computer that is still in the
// account but no longer runs AI there (`GET /api/cloud` `state: "not_hosting"`,
// chain v3 `SetHost false`): it reads as linked — not signed out — says it runs
// no AI, and keeps the controls to disconnect it.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { AggregatorLine, cloudKey, DroppenSection } from "../src/features/link/link-settings.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { makeI18n } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

function status(state: string) {
  return {
    state,
    localAuth: true,
    canManage: true,
    hub: "wss://hub.example",
    hubWeb: "https://hub.example",
    owner: "me",
    remoteAccess: [],
    aggregator: { enabled: false },
    account: { id: "a-1", email: "me@example.com", fingerprint: "aa bb" },
    machine: { hostname: "laptop", id: "m-1", fingerprint: "cc dd" },
    link: { running: true, connected: false, sinceMs: null, error: null, refused: null, alarms: [], machines: 1, devices: 2 },
  };
}

function draw(state: string): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(cloudKey(LOCAL), status(state));
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>
        <DroppenSection />
      </QueryClientProvider>
    </I18nextProvider>,
  );
  client.clear();
  return html;
}

test("a computer that stopped running AI reads as linked, says so, and can still be disconnected", () => {
  const html = draw("not_hosting");
  assert.match(html, /data-testid="droppen-linked"/);
  assert.match(html, /in the account · not running AI in it/);
  assert.match(html, /data-testid="droppen-unlink"/);
  assert.doesNotMatch(html, /droppen-removed/);
});

test("a linked computer is unchanged", () => {
  const html = draw("linked");
  assert.match(html, /data-testid="droppen-linked"/);
  assert.doesNotMatch(html, /not running AI in it/);
  assert.match(html, /data-testid="droppen-unlink"/);
});

// S1-4: the aggregator's line — this computer controlling the other devices
// with its one identity, and the content keys it holds of those that accept it.
function line(aggregator: Record<string, unknown>, canManage = true): string {
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <AggregatorLine aggregator={aggregator as never} canManage={canManage} busy={false} onSet={() => undefined} />
    </I18nextProvider>,
  );
}

test("the aggregator says how many devices that accept its control it holds the content keys of, what is missing and what is too old", () => {
  const on = (hosts: unknown[], extra: Record<string, unknown> = {}) => line({ enabled: true, state: "on", signedIn: true, hosts, ...extra });
  assert.match(on([]), /no device accepts control from this one yet/);
  const all = on([
    { id: "m-2", latestEpoch: 3, epochs: [0, 1, 2, 3], policy: true },
    { id: "m-3", latestEpoch: 0, epochs: [0], policy: true },
  ]);
  assert.match(all, /content keys of 2 of 2 devices that accept its control/);
  assert.doesNotMatch(all, /missing/);
  const gap = on(
    [
      { id: "m-2", latestEpoch: 4, epochs: [0, 1, 2, 3], policy: true },
      { id: "m-3", latestEpoch: 0, epochs: [0], policy: false },
    ],
    { askedMs: Date.UTC(2026, 9, 18, 12, 0) },
  );
  assert.match(gap, /content keys of 1 of 2/);
  assert.match(gap, /missing: m-2 · asked the account(&#x27;|')s full devices at/);
  assert.match(gap, /update Droppen on m-3/);
  assert.match(gap, /data-testid="droppen-aggregate"[^>]*>turn off</);
  // Off, from an older server (no hosts field), and to another local account: no button.
  const off = line({ enabled: false, state: "off" }, false);
  assert.match(off, /off: this device does not control other devices/);
  assert.doesNotMatch(off, /droppen-aggregate/);
  assert.match(line({ enabled: false, state: "off" }), />turn on</);
});
