// The desktop app's quiet "update ready" entry at the bottom of the rail
// (`app/desktop-update.tsx`): shown only for a version the app says is ready,
// until that version is put away; a newer one shows again.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { DesktopUpdateEntry, readyVersion } from "../src/app/desktop-update.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";

const ready = (version: unknown) => ({ current: "0.2.124", state: "ready", release: { version, notes: "https://github.com/getdroppen/droppen/releases/tag/v0.2.125" } });

test("only a ready update shows, and only until its version is put away", () => {
  assert.equal(readyVersion(ready("0.2.125"), null), "0.2.125");
  assert.equal(readyVersion(ready("0.2.125"), "0.2.125"), null, "put away");
  assert.equal(readyVersion(ready("0.2.126"), "0.2.125"), "0.2.126", "a newer one shows again");
  for (const state of ["idle", "checking", "downloading", "installing", "restarting"]) {
    assert.equal(readyVersion({ ...ready("0.2.125"), state }, null), null, state);
  }
  // Whatever else the bridge might hold: nothing.
  for (const odd of [undefined, null, "ready", 42, {}, { state: "ready" }, ready(125), ready("<img src=x>"), ready("v0.2.125")]) {
    assert.equal(readyVersion(odd, null), null, JSON.stringify(odd));
  }
});

async function draw(lng: string, resources: Record<string, string>): Promise<string> {
  const i18n = i18next.createInstance();
  await i18n.use(initReactI18next).init({ lng, resources: { [lng]: { translation: resources } }, interpolation: { escapeValue: false } });
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DesktopUpdateEntry version="0.2.125" onRestart={() => {}} onDismiss={() => {}} />
    </I18nextProvider>,
  );
}

test("the entry says the version and restart, in each language", async () => {
  const en = await draw("en", {});
  assert.match(en, /title="Droppen 0\.2\.125 is ready · restart to update"/);
  assert.match(en, />restart</);
  assert.match(en, /title="dismiss"/);
  assert.match(await draw("zh-CN", zhCN), /新版本 Droppen 0\.2\.125 已就绪 · 重启以更新/);
  assert.match(await draw("zh-TW", zhTW), /新版本 Droppen 0\.2\.125 已就緒/);
  assert.match(await draw("ja", ja), /再起動/);
  // Square corners and no state colour: it is news, not a state.
  assert.doesNotMatch(en, /rounded|text-ok|bg-ok|text-warn|bg-warn|danger/);
});
