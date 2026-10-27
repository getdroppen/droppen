// The rail's 「+ → vscode」 on a device without code-server: the server answers
// `409 code_server_missing` with a hint (`server-rs/src/apps/mod.rs`), and the
// web app says what to install and where, in the reader's language, with a
// link to code-server's install guide, instead of quoting the OS error
// (`features/apps/code-server-missing.tsx`).
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { ApiError } from "../src/api/errors.js";
import {
  appLaunchErrorText,
  CODE_SERVER_INSTALL_URL,
  CODE_SERVER_MISSING,
  CODE_SERVER_MISSING_TITLE,
  CodeServerMissingNotice,
  isCodeServerMissing,
} from "../src/features/apps/code-server-missing.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { makeI18n } from "./render.js";

/** What `POST /api/apps` answers on a device with no code-server on its server's PATH. */
const MISSING = { error: "code_server_missing", hint: "code-server is not installed on this device, or is not on the server's PATH" };

const TABLES = { "zh-CN": zhCN, "zh-TW": zhTW, ja } as const;

async function notice(lng: string): Promise<string> {
  const i18n = makeI18n({
    lng,
    resources: { en: { translation: {} }, "zh-CN": { translation: zhCN }, "zh-TW": { translation: zhTW }, ja: { translation: ja } },
    // As `i18n/index.ts`: the keys are English sentences, with dots and colons in them.
    keySeparator: false,
    nsSeparator: false
  });
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <CodeServerMissingNotice />
    </I18nextProvider>,
  );
}

test("the server's refusal is told apart from every other failure to start an app", () => {
  assert.equal(CODE_SERVER_MISSING, MISSING.error, "the token server-rs/src/apps/mod.rs answers with");
  const t = (key: string) => `«${key}»`;

  const missing = new ApiError(MISSING);
  assert.ok(isCodeServerMissing(missing));
  assert.equal(appLaunchErrorText(missing, t), `«${CODE_SERVER_MISSING_TITLE}»`, "the 「+」 says what is missing, in the reader's words");

  // Every other refusal reads as the server's sentence, as it did before.
  const late = new ApiError({ error: "app did not become ready in time" });
  assert.ok(!isCodeServerMissing(late));
  assert.equal(appLaunchErrorText(late, t), "app did not become ready in time");

  // A server from before the token said it in a sentence; it is quoted as it is.
  const older = new ApiError({ error: "cannot launch code-server (is it installed?): No such file or directory (os error 2)" });
  assert.ok(!isCodeServerMissing(older));
  assert.equal(appLaunchErrorText(older, t), older.code);

  assert.ok(!isCodeServerMissing(new Error(CODE_SERVER_MISSING)), "only the server's answer, not any error that happens to say it");
});

test("the notice links to code-server's install guide and says where to put it", async () => {
  assert.equal(CODE_SERVER_INSTALL_URL, "https://coder.com/docs/code-server/install");
  const html = await notice("en");
  assert.ok(html.includes(`href="${CODE_SERVER_INSTALL_URL}"`), html);
  assert.match(html, /target="_blank"/, "opened outside the app");
  assert.match(html, /rel="noopener noreferrer"/);
  assert.match(html, />How to install code-server</);
  assert.match(html, /Droppen does not install/);
  assert.match(html, /~\/\.local\/bin/, "a place on the server's PATH");
  assert.match(html, /does not need a restart/);
});

test("every language says it in its own words", async () => {
  for (const [lng, table] of Object.entries(TABLES)) {
    assert.ok(table[CODE_SERVER_MISSING_TITLE], `${lng} has the title`);
    const html = await notice(lng);
    assert.doesNotMatch(html, /How to install|Droppen does not install|does not need a restart/, `${lng} has no English left: ${html}`);
    assert.ok(html.includes(table["How to install code-server"]!), `${lng}: ${html}`);
    assert.ok(html.includes("~/.local/bin") && html.includes("PATH") && html.includes("code-server"), `${lng} keeps the names a person types`);
  }
});
