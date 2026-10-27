// The update page (`features/updates/updates-view.tsx`): the version in hand,
// what changed in it, and the one action that restarts into it. Each state is
// drawn from the app's status, and the notes are the release body's two
// languages — never raw HTML or a fetched image.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { I18nextProvider } from "react-i18next";
import { UpdatesPage } from "../src/features/updates/updates-view.js";
import type { UpdateStatus } from "../src/app/desktop-update.js";
import { ja } from "../src/i18n/ja.js";
import { updatesZhCN } from "../src/i18n/updates.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { makeI18n } from "./render.js";

const base: UpdateStatus = {
  current: "0.2.130",
  state: "idle",
  release: null,
  percent: null,
  place: "updatable",
  lastChecked: new Date().toISOString(),
  problem: null,
  notes: null,
};

const release = (version = "0.2.131") => ({ version, notes: `https://github.com/getdroppen/droppen/releases/tag/v${version}` });

const notes = {
  version: "0.2.131",
  english: "### Your account\n\n- **Delete your account.** It is gone for good. [Notes](https://github.com/getdroppen/droppen/releases/tag/v0.2.131)\n",
  chinese: "### 你的账户\n\n- **可以删除账户。** 会被永久抹去。\n",
};

async function html(status: UpdateStatus | null, language = "en", dismissed: string | null = null): Promise<string> {
  const resources: Record<string, Record<string, string>> = {
    en: {},
    "zh-CN": { ...zhCN, ...updatesZhCN },
    "zh-TW": zhTW,
    ja,
  };
  const i18n = makeI18n({ lng: language, resources: { [language]: { translation: resources[language] ?? {} } } });
  return renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <UpdatesPage
        status={status}
        dismissed={dismissed}
        onDismiss={() => undefined}
        onRestart={() => undefined}
        onCheck={() => undefined}
        serverVersion="0.2.130"
        language={language}
      />
    </I18nextProvider>,
  );
}

test("a ready update says what it is, what changed, and one way to restart", async () => {
  const out = await html({ ...base, state: "ready", release: release(), notes });
  assert.match(out, /data-testid="updates-state" data-state="ready"/);
  assert.match(out, /data-testid="updates-target"[^>]*>0\.2\.131</);
  assert.match(out, /Droppen 0\.2\.131 is ready/);
  assert.match(out, /data-testid="updates-restart"/);
  assert.match(out, /data-testid="updates-later"/);
  assert.match(out, /Downloaded and verified\./);
  assert.match(out, /Sessions keep running while Droppen restarts\./);
  assert.match(out, /What&#x27;s new in 0\.2\.131|What's new in 0\.2\.131/);
  assert.match(out, /Your account/);
  assert.match(out, /Delete your account/);
  assert.match(out, /https:\/\/github\.com\/getdroppen\/droppen\/releases\/tag\/v0\.2\.131/);
  // One primary action on the page, and it is the restart.
  assert.equal(out.match(/data-variant="primary"/g)?.length, 1);
  assert.match(out, /data-testid="updates-restart"[^>]*data-variant="primary"|data-variant="primary"[^>]*data-testid="updates-restart"/);
  // The language toggle only when both languages are there.
  assert.match(out, />English</);
  assert.match(out, />中文</);
});

test("a put-away version keeps the restart and loses only the rail mark's offer", async () => {
  const out = await html({ ...base, state: "ready", release: release(), notes }, "en", "0.2.131");
  assert.match(out, /data-testid="updates-restart"/);
  assert.doesNotMatch(out, /data-testid="updates-later"/);
});

test("a download shows how far it is and offers no restart", async () => {
  const out = await html({ ...base, state: "downloading", release: release(), percent: 42 });
  assert.match(out, /data-testid="updates-state" data-state="downloading"/);
  assert.match(out, /42%/);
  assert.match(out, /role="progressbar"[^>]*aria-valuenow="42"/);
  assert.doesNotMatch(out, /updates-restart/);
  assert.doesNotMatch(out, /data-variant="primary"/);
});

test("up to date, the page is about this version and what changed in it", async () => {
  const out = await html({ ...base, notes: { ...notes, version: "0.2.130" } });
  assert.match(out, /You&#x27;re on the latest version, 0\.2\.130\.|You're on the latest version, 0\.2\.130\./);
  assert.match(out, /What changed in 0\.2\.130/);
  assert.match(out, /Delete your account/);
  assert.match(out, /data-testid="updates-check"/);
  assert.doesNotMatch(out, /data-variant="primary"/);
});

test("a failure is said in the person's terms, with a way to try again", async () => {
  const out = await html({ ...base, problem: "offline" });
  assert.match(out, /Could not check for updates: no network\./);
  assert.match(out, />Try again</);
  // And a failure worth saying unasked still says it.
  assert.match(await html({ ...base, problem: "signature" }), /did not verify/);
});

test("where it cannot update itself it says where to move it, and offers no check", async () => {
  const moved = await html({ ...base, place: "translocated" });
  assert.match(moved, /Move Droppen to Applications to update\./);
  assert.doesNotMatch(moved, /data-testid="updates-check"/);
  const copy = await html({ ...base, place: "notABundle" });
  assert.match(copy, /This copy of Droppen cannot update itself\./);
});

test("no bridge, no update machinery: the page says who handles it", async () => {
  const out = await html(null);
  assert.match(out, /Updates are handled by the Droppen app\./);
  assert.doesNotMatch(out, /data-testid="updates-state"/);
});

test("the notes are shown in the reader's language, from the two in the release", async () => {
  const zh = await html({ ...base, state: "ready", release: release(), notes }, "zh-CN");
  assert.match(zh, /你的账户/);
  assert.doesNotMatch(zh, /Delete your account/);
  assert.match(zh, /更新/);
  // Only one language in the release: that one, and no toggle.
  const onlyEnglish = { ...notes, chinese: "" };
  const zhOnly = await html({ ...base, state: "ready", release: release(), notes: onlyEnglish }, "zh-CN");
  assert.match(zhOnly, /Delete your account/);
  assert.doesNotMatch(zhOnly, /data-testid="updates-notes"[\s\S]*?>中文</);
});

test("notes for another version, or none, send the reader to GitHub instead", async () => {
  const out = await html({ ...base, state: "ready", release: release(), notes: { ...notes, version: "0.2.129" } });
  assert.match(out, /No notes for this release are available here\./);
  assert.match(out, /Read the release notes on GitHub/);
});

test("release notes are read as text: no raw HTML, no fetched image, no foreign link", async () => {
  const hostile = {
    version: "0.2.131",
    english:
      "### Hello\n\n<script>alert(1)</script>\n\n![x](https://evil.example/x.png)\n\n[bad](javascript:alert(1)) [ok](https://github.com/getdroppen/droppen)\n\n<img src=x onerror=alert(1)>\n",
    chinese: "",
  };
  const out = await html({ ...base, state: "ready", release: release(), notes: hostile });
  assert.doesNotMatch(out, /<script/i);
  assert.doesNotMatch(out, /<img/i);
  assert.doesNotMatch(out, /javascript:/i);
  assert.doesNotMatch(out, /evil\.example/);
  assert.match(out, /https:\/\/github\.com\/getdroppen\/droppen/);
});

// The one line in Settings → General, where the page is reached when the rail
// has nothing to say.
import { updatesLine } from "../src/features/updates/updates-row.js";

test("Settings' line names the version in hand and what it is doing", () => {
  const base = {
    current: "0.2.130",
    state: "idle",
    release: null,
    percent: null,
    place: "updatable",
    lastChecked: new Date().toISOString(),
    problem: null,
    notes: null,
  } as const;
  assert.deepEqual(updatesLine({ ...base }), { version: "0.2.130", state: "up-to-date" });
  assert.deepEqual(updatesLine({ ...base, state: "ready", release: release() }), { version: "0.2.131", state: "ready" });
  assert.deepEqual(updatesLine({ ...base, state: "downloading", release: release(), percent: 20 }), {
    version: "0.2.131",
    state: "downloading",
  });
  for (const problem of ["signature", "location", "install"] as const) {
    assert.deepEqual(updatesLine({ ...base, problem }), { version: "0.2.130", state: "attention" }, problem);
  }
  for (const problem of ["offline", "service", "other"] as const) {
    assert.deepEqual(updatesLine({ ...base, problem }), { version: "0.2.130", state: "up-to-date" }, problem);
  }
  assert.equal(updatesLine(null), null);
});
