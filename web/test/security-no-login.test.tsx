// Settings → Security where there is no account to manage, and what it says
// instead. Authentication switched off (`auth.enabled = false`): the setting,
// and how to undo it. This device's owner, by a page opened signed in from the
// device itself while it has no account (`user: "local:owner"`): the first
// account, without the setup code — under the desktop app too, whose "Open in
// Browser" opens such a page. The desktop app's own window: the app signs in.
// Anybody signed in to an account, under the desktop app too: the usual view. A status the server did
// not give: said so, never taken for authentication switched off.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { askAuthStatus, SecuritySection } from "../src/features/settings/security-section.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { mq } from "../src/lib/machines/query.js";
import { setupZhCN } from "../src/i18n/setup.js";
import { makeI18n } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: setupZhCN } },
  keySeparator: false,
  nsSeparator: false
});

function draw(status: Record<string, unknown>, lng = "en"): string {
  void i18n.changeLanguage(lng);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "auth-status"), { setupRequired: false, user: null, desktopApp: false, ...status });
  return render(client);
}

/**
 * The tab after its status request failed, asked the way the tab asks it
 * (`askAuthStatus`) of a server that answered with an error and no status.
 */
async function drawFailed(): Promise<string> {
  const failing = { GET: async () => ({ data: undefined, error: { error: "boom", hint: "the status route failed" } }) };
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  await client.prefetchQuery({
    queryKey: mq(LOCAL, "auth-status"),
    queryFn: () => askAuthStatus(failing as unknown as Parameters<typeof askAuthStatus>[0]),
  });
  return render(client);
}

function render(client: QueryClient): string {
  client.setQueryData(mq(LOCAL, "auth-sessions"), []);
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>
        <SecuritySection />
      </QueryClientProvider>
    </I18nextProvider>,
  );
  client.clear();
  return html;
}

test("authentication switched off: the setting that did it, and how to undo it", () => {
  const html = draw({ authRequired: false, authOff: true });
  assert.match(html, /data-testid="security-auth-off"/);
  assert.match(html, /Local login is switched off on this device \(auth\.enabled = false\)/);
  assert.match(html, /APP_AUTH__ENABLED=false/);
  assert.doesNotMatch(html, /type="password"/);
  // From a server that predates `authOff`, `authRequired: false` meant the same.
  assert.match(draw({ authRequired: false }), /data-testid="security-auth-off"/);
  assert.match(draw({ authRequired: false, authOff: true }, "zh-CN"), /这台设备关闭了本地登录/);
});

test("this device's owner is offered the first account, without the setup code", () => {
  const html = draw({ authRequired: true, authOff: false, setupRequired: true, user: "local:owner" });
  assert.match(html, /data-testid="security-local-owner"/);
  assert.match(html, /no account yet/);
  assert.match(html, /You opened this page as this device&#x27;s owner\. This device has no account yet: create one to sign in from other devices; from then on every browser signs in, this one too\./);
  assert.match(html, /data-testid="setup-form" data-with-code="false"/);
  assert.doesNotMatch(html, /data-testid="setup-code"/);
  for (const id of ["setup-username", "setup-password", "setup-confirm", "setup-submit"]) assert.match(html, new RegExp(`data-testid="${id}"`), id);
  // Nothing to sign out of or change: no account yet.
  assert.doesNotMatch(html, /sign out|current password/);
  assert.match(draw({ authRequired: true, authOff: false, setupRequired: true, user: "local:owner" }, "zh-CN"), /你以这台设备主人的身份打开了这个页面/);
});

test("under the desktop app, the owner a page opened from the app is offered the first account; the app's window is not", () => {
  // "Open in Browser" opens the page signed in as this device's owner.
  const owner = draw({ authRequired: true, authOff: false, setupRequired: true, user: "local:owner", desktopApp: true });
  assert.match(owner, /data-testid="security-local-owner"/);
  assert.match(owner, /data-testid="setup-form" data-with-code="false"/);
  assert.doesNotMatch(owner, /security-desktop-app/);
  // The app's own window, signed in with its own account.
  const app = draw({ authRequired: true, authOff: false, setupRequired: true, user: "droppen-app", desktopApp: true });
  assert.match(app, /data-testid="security-desktop-app"/);
  assert.doesNotMatch(app, /security-local-owner|type="password"/);
  // A person's own account on the app's server: the account, its password, its logins.
  const persons = draw({ authRequired: true, authOff: false, setupRequired: false, user: "ada", desktopApp: true });
  assert.doesNotMatch(persons, /security-desktop-app|security-local-owner/);
  assert.match(persons, /type="password"/);
});

test("a status the server did not give is a failure, not authentication switched off", async () => {
  void i18n.changeLanguage("en");
  const html = await drawFailed();
  assert.match(html, /data-testid="security-status-failed"/);
  assert.match(html, /Could not ask this device&#x27;s server about its login\. the status route failed/);
  assert.doesNotMatch(html, /security-auth-off|switched off|APP_AUTH__ENABLED/);
});

test("signed in to an account: the account, the password and the logins", () => {
  const html = draw({ authRequired: true, authOff: false, user: "me" });
  assert.doesNotMatch(html, /security-(auth-off|local-owner)/);
  assert.match(html, /type="password"/);
  assert.match(html, /sign out/);
});
