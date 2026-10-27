// Settings → Security inside the Droppen desktop app (`GET /api/auth/status`
// says `desktopApp: true`, signed in as the app's own account `droppen-app`):
// the app made the password nobody knows and signs in again by itself, so
// there is no password change and no sign-out — only a line saying so, and
// the list of logins. A person's own account on the app's server is not the
// app's, and keeps both.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { SecuritySection } from "../src/features/settings/security-section.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { mq } from "../src/lib/machines/query.js";
import { makeI18n } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});

function draw(desktopApp: boolean, user = desktopApp ? "droppen-app" : "me"): string {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "auth-status"), { authRequired: true, setupRequired: false, user, desktopApp });
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

test("inside the desktop app there is no password change and no sign-out", () => {
  const html = draw(true);
  assert.match(html, /data-testid="security-desktop-app"/);
  assert.match(html, /The Droppen app signs in for you on this device\./);
  assert.doesNotMatch(html, /type="password"/);
  assert.doesNotMatch(html, /sign out/);
});

test("in a browser the password change and the sign-out are there as before", () => {
  const html = draw(false);
  assert.doesNotMatch(html, /security-desktop-app/);
  assert.match(html, /type="password"/);
  assert.match(html, /sign out/);
});

test("a person's own account on the desktop app's server keeps its password change and its sign-out", () => {
  const html = draw(true, "ada");
  assert.doesNotMatch(html, /security-desktop-app/);
  assert.match(html, /type="password"/);
  assert.match(html, /sign out/);
});
