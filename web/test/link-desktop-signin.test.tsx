// Inside the Droppen desktop app (`GET /api/auth/status` says `desktopApp:
// true`) the device signs in to Droppen through the system browser — its
// window has no popups — and the first-run card offers it again; elsewhere
// the popup stays the first way.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { makeI18n } from "./render.js";

class FakeRequest {
  url: string;
  method: string;
  headers: Headers;
  constructor(url: string, init: { method?: string; headers?: HeadersInit } = {}) {
    this.url = url;
    this.method = init.method ?? "GET";
    this.headers = new Headers(init.headers);
  }
}
(globalThis as unknown as { Request: unknown }).Request = FakeRequest;
let desktopApp = true;
(globalThis as unknown as { fetch: unknown }).fetch = async (req: { url?: string } | string) => {
  const url = typeof req === "string" ? req : (req.url ?? "");
  if (url === "/api/auth/status") return new Response(JSON.stringify({ authRequired: true, user: "droppen-app", desktopApp }), { status: 200, headers: { "content-type": "application/json" } });
  throw new Error(`unexpected request: ${url}`);
};

const { authStatus, forgetAuthStatus } = await import("../src/lib/auth.js");
const { signsInThroughBrowser, startDroppenSignIn } = await import("../src/features/link/browser-sign-in.js");
const { LinkFirstRun } = await import("../src/features/link/first-run.js");
const { cloudKey } = await import("../src/features/link/link-flow.js");
const { LOCAL } = await import("../src/lib/machines/key.js");

function fakeOpener() {
  const calls: string[] = [];
  return {
    calls,
    opener: {
      open: (hubWeb: string) => void calls.push(`popup ${hubWeb}`),
      openInBrowser: (opts: { hub?: string }) => void calls.push(`browser ${opts.hub ?? "-"}`),
    } as unknown as Parameters<typeof startDroppenSignIn>[0],
  };
}

test("inside the desktop app the sign-in goes through the system browser; elsewhere through the popup", async () => {
  desktopApp = true;
  forgetAuthStatus();
  await authStatus();
  assert.equal(signsInThroughBrowser(), true);
  const app = fakeOpener();
  startDroppenSignIn(app.opener, "https://droppen.app", { hub: "https://hub.example" });
  assert.deepEqual(app.calls, ["browser https://hub.example"]);

  desktopApp = false;
  forgetAuthStatus();
  await authStatus();
  assert.equal(signsInThroughBrowser(), false);
  const page = fakeOpener();
  startDroppenSignIn(page.opener, "https://droppen.app");
  assert.deepEqual(page.calls, ["popup https://droppen.app"]);
});

test("the first-run card offers signing in inside the desktop app too", async () => {
  desktopApp = true;
  forgetAuthStatus();
  await authStatus();
  const i18n = makeI18n({ lng: "en", resources: { en: { translation: {} } } });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(cloudKey(LOCAL), { state: "none", localAuth: true, canManage: true, hubWeb: "https://droppen.app" });
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={client}>
        <LinkFirstRun />
      </QueryClientProvider>
    </I18nextProvider>,
  );
  client.clear();
  assert.match(html, /data-testid="droppen-first-run"/);
  assert.match(html, /Sign in to Droppen/);
});
