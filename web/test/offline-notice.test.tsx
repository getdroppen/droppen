// What cloud mode says while it shows the machine's offline copy
// (`features/cloud/cloud-bar.tsx`): a notice strip with the copy's age — the
// acceptance run finds it by `data-testid="offline-notice"` — instead of the
// old full-screen cover, which is now only for a device with no copy at all;
// and every new string translated.
//
// `react-dom/server` with no DOM, like the other render tests; `import * as
// React` because the test directory is outside the tsconfig's `include`.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { barState, NoCopyCover, OfflineNotice, RefusalNotice } from "../src/features/cloud/cloud-bar.js";
import { formatTime } from "../src/lib/format.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { makeI18n, pageDraw } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: zhCN } },
  keySeparator: false,
  nsSeparator: false
});

const draw = pageDraw(i18n);

const at = Date.UTC(2026, 9, 15, 9, 30);

test("the notice says the copy is offline, how old it is, and that it is read-only", () => {
  const html = draw(<OfflineNotice freshestMs={at} />);
  assert.match(html, /data-testid="offline-notice"/);
  assert.ok(html.includes(`offline copy · as of ${formatTime(new Date(at).toISOString())}`), html);
  assert.ok(html.includes("read-only: sending and terminals need the device online"));
  assert.doesNotMatch(html, /fixed inset/, "a strip in the layout, not a cover over the app");
  assert.ok(draw(<OfflineNotice freshestMs={null} />).includes("offline copy</span>"), "a copy whose age is unknown still says what it is");
  const zh = draw(<OfflineNotice freshestMs={at} />, "zh-CN");
  assert.ok(zh.includes("离线副本 · 截至"));
  assert.ok(zh.includes("只读"));
});

test("the cover is only for a device with nothing to show", () => {
  const html = draw(<NoCopyCover onDevices={() => undefined} />);
  assert.match(html, /data-testid="no-offline-copy"/);
  assert.ok(html.includes("No offline copy of it on this device yet"));
  const base = { link: "open" as const, presence: "offline" as const, channel: "offline" as const, mode: "offline" as const };
  const withCopy = barState({ ...base, copy: { loaded: true, hasCopy: true, freshestMs: at } });
  assert.deepEqual([withCopy.notice, withCopy.cover, withCopy.label], [true, false, "offline"]);
  const none = barState({ ...base, copy: { loaded: true, hasCopy: false, freshestMs: null } });
  assert.deepEqual([none.notice, none.cover], [false, true]);
  const stillReading = barState({ ...base, copy: { loaded: false, hasCopy: false, freshestMs: null } });
  assert.equal(stillReading.cover, false, "no cover flashed while the device copy is still being read");
  // Online, on the copy only until the channel opens: no notice for the moment it takes.
  const connecting = barState({ link: "open", presence: "online", channel: "connecting", mode: "offline", copy: { loaded: true, hasCopy: true, freshestMs: at } });
  assert.deepEqual([connecting.notice, connecting.cover, connecting.label], [false, false, "connecting"]);
  const failing = barState({ link: "open", presence: "online", channel: "error", mode: "offline", copy: { loaded: true, hasCopy: true, freshestMs: at } });
  assert.equal(failing.notice, true, "a channel that keeps failing leaves the page on the copy, and says so");
  const live = barState({ link: "open", presence: "online", channel: "open", mode: "live", copy: { loaded: true, hasCopy: true, freshestMs: at } });
  assert.deepEqual([live.notice, live.cover, live.label], [false, false, "e2e"]);
  const noHub = barState({ link: "connecting", presence: "online", channel: "idle", mode: "offline", copy: { loaded: true, hasCopy: true, freshestMs: at } });
  assert.deepEqual([noHub.notice, noHub.label], [true, "hub"], "a device that cannot reach the hub reads its copy too");
});

test("a device that is online but refusing this browser says why, not \"connecting…\" and not \"needs the device online\"", () => {
  const copy = { loaded: true, hasCopy: true, freshestMs: at };
  const base = { link: "open" as const, presence: "online" as const, mode: "offline" as const, copy };
  const notAccepted = barState({ ...base, channel: "error", refusal: "notAccepted" });
  assert.deepEqual([notAccepted.label, notAccepted.notice], ["refused", false], "a refusal is explained by its own notice, not the offline one");
  const retrying = barState({ ...base, channel: "connecting", refusal: "notAccepted" });
  assert.equal(retrying.label, "refused", "the reason outlives the next retry's \"connecting\"");
  const unconfirmed = barState({ ...base, channel: "error", refusal: "unconfirmed" });
  assert.deepEqual([unconfirmed.label, unconfirmed.notice], ["unconfirmed", false]);
  const unsaved = barState({ ...base, channel: "error", refusal: "unsaved" });
  assert.deepEqual([unsaved.label, unsaved.notice], ["unsaved", false], "a computer that cannot save says so, not \"connecting…\"");
  const offline = barState({ ...base, presence: "offline", channel: "error", refusal: "notAccepted" });
  assert.equal(offline.label, "offline", "a device that went offline is offline, whatever it said before");
  const noHub = barState({ ...base, link: "connecting", channel: "error", refusal: "unconfirmed" });
  assert.equal(noHub.label, "hub");
  const live = barState({ ...base, channel: "open", mode: "live", refusal: "notAccepted" });
  assert.equal(live.label, "e2e", "a channel that opened is live, whatever came before");
  const plain = barState({ ...base, channel: "error" });
  assert.deepEqual([plain.label, plain.notice], ["connecting", true], "a failure that is not a refusal is as before");
});

test("the refusal notice says what to do: which devices to open, or making this browser a full device", () => {
  const none = () => undefined;
  const waiting = draw(<RefusalNotice refusal={{ kind: "unconfirmed" }} limited={false} others={["d-phone", "d-laptop"]} onDevices={none} />);
  assert.match(waiting, /data-testid="channel-refused"/);
  assert.match(waiting, /data-kind="unconfirmed"/);
  assert.ok(waiting.includes("Open Droppen once on d-phone, d-laptop"), waiting);
  const anyOther = draw(<RefusalNotice refusal={{ kind: "unconfirmed" }} limited={false} others={[]} onDevices={none} />);
  assert.ok(anyOther.includes("Open Droppen once on another device of the account"));
  const limited = draw(<RefusalNotice refusal={{ kind: "notAccepted", by: "hub", why: "that device does not accept control from this one" }} limited others={[]} onDevices={none} />);
  assert.ok(limited.includes("This browser is a limited device of the account"));
  assert.match(limited, /title="that device does not accept control from this one"/, "the hub's own words on hover");
  // A full device is always accepted (`may_control`): for one, the hub's own words are all there is to say.
  const full = draw(<RefusalNotice refusal={{ kind: "notAccepted", by: "hub", why: "this device is not current in the account's chain" }} limited={false} others={[]} onDevices={none} />);
  assert.ok(full.includes("The hub would not connect this browser to the device: this device is not current in the account"), full);
  assert.doesNotMatch(full, /who can control it/, "no hunt through a setting that does not apply to it");
  const unsaved = draw(<RefusalNotice refusal={{ kind: "unsaved" }} limited={false} others={[]} onDevices={none} />);
  assert.match(unsaved, /data-kind="unsaved"/);
  assert.ok(unsaved.includes("The device could not save its record of who connects to it, so it turned this browser away. Check that its disk is not full"), unsaved);
  const wordless = draw(<RefusalNotice refusal={{ kind: "notAccepted", by: "relay", why: null }} limited={false} others={[]} onDevices={none} />);
  assert.ok(wordless.includes("Reload the page; if it keeps happening, check my devices."));
});

test("every string the offline copy shows is translated", () => {
  const keys = [
    "offline copy · as of {{time}}",
    "offline copy",
    "read-only: sending and terminals need the device online",
    "No offline copy of it on this device yet: one is made while it is online. This view comes back as soon as it reconnects.",
    "the device is offline — this is a read-only copy",
    "the device is offline, and there is no copy of this session here yet",
    "the device is offline — this needs it to be online",
    "the device is offline — terminals need it",
  ];
  for (const [name, table] of [["zh-CN", zhCN], ["zh-TW", zhTW], ["ja", ja]] as const) {
    for (const key of keys) assert.ok(table[key], `${name} lacks "${key}"`);
    assert.ok(table["offline copy · as of {{time}}"]!.includes("{{time}}"), `${name} keeps the time`);
  }
});
