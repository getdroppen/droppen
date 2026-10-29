// The commands the app asks a person to type on a device (`<command> user add
// <name>` on the login page and in Settings → Security) name the server the
// way that device's server reports it: `/api/auth/status`'s `droppenPath`,
// the `droppen` link beside its binary or the binary itself, with `~` for the
// home directory. A server from before that field reports nothing, and answers
// to its old name only.
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
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { droppenCommand, LEGACY_COMMAND, shellWord } from "../src/lib/droppen-command.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { mq } from "../src/lib/machines/query.js";

const INSTALLED = "~/.local/share/remote-hand/bin/droppen";

// `openapi-fetch` builds a `Request` before it calls `fetch`, and node cannot
// parse the relative URL this app's client is configured with, so both are
// stubbed before the client module is first imported (hence the dynamic
// imports below).
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
/** What the stub server answers `/api/auth/status` with. */
let answer: Record<string, unknown> = {};
(globalThis as unknown as { fetch: unknown }).fetch = async (req: { url?: string } | string) => {
  const url = typeof req === "string" ? req : (req.url ?? "");
  if (url !== "/api/auth/status") throw new Error(`unexpected request: ${url}`);
  return new Response(JSON.stringify(answer), { status: 200, headers: { "content-type": "application/json" } });
};
const { authStatus, forgetAuthStatus, pageDroppenCommand } = await import("../src/lib/auth.js");
const { SecuritySection } = await import("../src/features/settings/security-section.js");

test("the command is the path the server reports, quoted only where a shell needs it", () => {
  assert.equal(droppenCommand({ droppenPath: INSTALLED }), INSTALLED);
  assert.equal(droppenCommand({ droppenPath: "/usr/local/bin/remote-hand-server" }), "/usr/local/bin/remote-hand-server");
  assert.equal(shellWord("~/Library/Application Support/Droppen/droppen"), "~/'Library/Application Support/Droppen/droppen'", "`~` stays outside the quotes");
  assert.equal(shellWord("/opt/o'brien/droppen"), "'/opt/o'\\''brien/droppen'");
  assert.equal(shellWord("/opt/$x/droppen"), "'/opt/$x/droppen'");
});

test("a server that is not the usual one says what the command needs to reach it", () => {
  // A second instance: its name, or `setup-code` would read the default one's code.
  assert.equal(
    droppenCommand({ droppenPath: INSTALLED, droppenEnv: [{ name: "APP_INSTANCE", value: "bob" }] }),
    `APP_INSTANCE=bob ${INSTALLED}`,
  );
  // A second install: where its files are, quoted where a shell needs it.
  assert.equal(
    droppenCommand({
      droppenPath: "~/.local/share/bob-remote-hand/bin/droppen",
      droppenEnv: [
        { name: "DROPPEN_DATA_DIR", value: "~/.local/share/bob-remote-hand" },
        { name: "DROPPEN_CONFIG_DIR", value: "~/My Config/bob" },
      ],
    }),
    "DROPPEN_DATA_DIR=~/.local/share/bob-remote-hand DROPPEN_CONFIG_DIR=~/'My Config/bob' ~/.local/share/bob-remote-hand/bin/droppen",
  );
  // An older server sends each directory under its old name too: only the
  // name every server reads is shown.
  assert.equal(
    droppenCommand({
      droppenPath: INSTALLED,
      droppenEnv: [
        { name: "DROPPEN_DATA_DIR", value: "~/d" },
        { name: "REMOTE_HAND_DATA_DIR", value: "~/d" },
        { name: "DROPPEN_CONFIG_DIR", value: "~/c" },
        { name: "REMOTE_HAND_CONFIG_DIR", value: "~/c" },
      ],
    }),
    `DROPPEN_DATA_DIR=~/d DROPPEN_CONFIG_DIR=~/c ${INSTALLED}`,
  );
  // Nothing for the usual install, or from a server older than the field; and
  // nothing but those few names — whatever else a device sends is dropped.
  assert.equal(droppenCommand({ droppenPath: INSTALLED, droppenEnv: [] }), INSTALLED);
  assert.equal(droppenCommand({ droppenPath: INSTALLED, droppenEnv: null }), INSTALLED);
  for (const name of ["DYLD_INSERT_LIBRARIES", "BASH_ENV", "LD_PRELOAD", "PATH", "X; echo hi", "app_instance"]) {
    assert.equal(droppenCommand({ droppenPath: INSTALLED, droppenEnv: [{ name, value: "/tmp/x" }] }), INSTALLED, name);
  }
  assert.equal(
    droppenCommand({
      droppenPath: INSTALLED,
      droppenEnv: [
        { name: "BASH_ENV", value: "/tmp/x" },
        { name: "APP_INSTANCE", value: "bob" },
      ],
    }),
    `APP_INSTANCE=bob ${INSTALLED}`,
  );
});

test("a server that reports no command is said by its old name", () => {
  for (const status of [undefined, null, {}, { droppenPath: "" }, { droppenPath: "  " }, { droppenPath: null }]) {
    assert.equal(droppenCommand(status), LEGACY_COMMAND, JSON.stringify(status));
  }
  assert.equal(LEGACY_COMMAND, "remote-hand-server");
});

test("the login page's command comes from this page's server", async () => {
  answer = { authRequired: true, user: null, desktopApp: false, droppenPath: INSTALLED };
  forgetAuthStatus();
  await authStatus();
  assert.equal(pageDroppenCommand(), INSTALLED);

  // An older server: no field.
  answer = { authRequired: true, user: null, desktopApp: false };
  forgetAuthStatus();
  await authStatus();
  assert.equal(pageDroppenCommand(), "remote-hand-server");
  forgetAuthStatus();
});

test("the login hint names the command in every language", async () => {
  const key = "Accounts are created on the server with `{{command}} user add <name>`.";
  const i18n = i18next.createInstance();
  await i18n.init({
    lng: "en",
    resources: { en: { translation: {} }, "zh-CN": { translation: zhCN }, "zh-TW": { translation: zhTW }, ja: { translation: ja } },
    interpolation: { escapeValue: false },
  });
  for (const lng of ["en", "zh-CN", "zh-TW", "ja"]) {
    const text = i18n.getFixedT(lng)(key, { command: INSTALLED });
    assert.ok(text.includes(`\`${INSTALLED} user add <`), `${lng}: ${text}`);
    assert.doesNotMatch(text, /remote-hand-server/, lng);
  }
  for (const table of [zhCN, zhTW, ja]) assert.ok(key in table, "the key is translated");
});

function drawSecurity(status: Record<string, unknown>): string {
  const i18n = i18next.createInstance();
  void i18n.use(initReactI18next).init({ lng: "en", resources: { en: { translation: {} } }, interpolation: { escapeValue: false } });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, retryOnMount: false, gcTime: Infinity } } });
  client.setQueryData(mq(LOCAL, "auth-status"), status);
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

test("Settings → Security says how to turn the login on with that device's command", () => {
  const html = drawSecurity({ authRequired: false, user: null, desktopApp: false, droppenPath: INSTALLED });
  assert.match(html, /~\/\.local\/share\/remote-hand\/bin\/droppen user add &lt;name&gt;/);
  assert.doesNotMatch(html, /remote-hand-server/);

  const older = drawSecurity({ authRequired: false, user: null, desktopApp: false });
  assert.match(older, /remote-hand-server user add &lt;name&gt;/, "an older server answers to its old name only");
});
