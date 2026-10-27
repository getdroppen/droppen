// What the account's plan makes the page say (relay-and-plans §3.8, §4.3;
// `lib/hub/plan.ts`, `lib/hub/me.ts`, `features/cloud/plan.tsx`): the device
// and machine limits checked before anything is signed and the hub's 409
// `limit` read back into the same words; how long a machine away for days
// keeps its offline copy in the cloud — from the plan, so free and pro differ;
// the storage thresholds; a downgrade's end; and the devices someone
// registered with the password.
//
// `react-dom/server` with no DOM, like the other render tests; `import * as
// React` because the test directory is outside the tsconfig's `include`.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import i18next from "i18next";
import { OfflineNotice } from "../src/features/cloud/cloud-bar.js";
import {
  CopyWarningLine,
  CopyWindowLine,
  FirstMachineNote,
  LimitNote,
  limitText,
  UsagePanel,
  PlanStrips,
  RegistrationList,
} from "../src/features/cloud/plan.js";
import { errorText } from "../src/features/cloud/ui.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { type AccountView, HubApi, HubApiError, type PlanView } from "../src/lib/hub/api.js";
import { MeStore } from "../src/lib/hub/me.js";
import {
  atLimit,
  copyWarning,
  DAY_MS,
  downgradeOf,
  formatPlanBytes,
  formatStoragePair,
  limitOf,
  offlineSince,
  registrationAlerts,
  retentionOf,
  storageView,
} from "../src/lib/hub/plan.js";
import { makeI18n, pageDraw } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: { ...cloudDevicesZhCN, ...zhCN } } },
  keySeparator: false,
  nsSeparator: false
});

const draw = pageDraw(i18n);

const GIB = 1024 ** 3;

function plan(over: Partial<PlanView> & { usage?: Partial<PlanView["usage"]>; entitlements?: Record<string, number | null> } = {}): PlanView {
  const { usage, entitlements, ...rest } = over;
  return {
    id: "free",
    source: "default",
    status: "active",
    periodStartMs: null,
    periodEndMs: null,
    gracePlan: null,
    graceUntilMs: null,
    ...rest,
    entitlements: { "storage.retention_days": 7, "storage.bytes": 5 * GIB, "devices.max": 5, "machines.max": 3, "transfer.included_bytes": 10 * GIB, ...entitlements },
    usage: { storageBytes: 0, devices: 1, machines: 0, relays: 0, transfer: null, ...usage },
  };
}

const pro = (over: Parameters<typeof plan>[0] = {}) =>
  plan({ id: "pro", ...over, entitlements: { "storage.retention_days": 30, "storage.bytes": 100 * GIB, "devices.max": 20, "machines.max": 20, ...over.entitlements } });

test("the device and machine limits: at the plan's number, not before; none without a limit or a plan", () => {
  assert.equal(atLimit(plan({ usage: { devices: 4 } }), "devices.max"), null);
  assert.deepEqual(atLimit(plan({ usage: { devices: 5 } }), "devices.max"), { key: "devices.max", limit: 5, used: 5 });
  // Past it (a recovered device may go one past, or a downgrade): still at the limit.
  assert.deepEqual(atLimit(plan({ usage: { devices: 6 } }), "devices.max"), { key: "devices.max", limit: 5, used: 6 });
  assert.equal(atLimit(plan({ usage: { machines: 2 } }), "machines.max"), null);
  assert.deepEqual(atLimit(plan({ usage: { machines: 3 } }), "machines.max"), { key: "machines.max", limit: 3, used: 3 });
  assert.equal(atLimit(pro({ usage: { machines: 3 } }), "machines.max"), null, "pro has room for twenty");
  assert.equal(atLimit(plan({ usage: { devices: 99 }, entitlements: { "devices.max": null } }), "devices.max"), null, "null: no limit");
  assert.equal(atLimit(null, "devices.max"), null, "no plan known: the hub still checks");
  // Chain v3 (local-aggregator §3.8): hosts.max over usage.hosts; machines.max is read as its old name.
  assert.deepEqual(atLimit(plan({ usage: { hosts: 3, machines: 3 }, entitlements: { "hosts.max": 3 } }), "hosts.max"), { key: "hosts.max", limit: 3, used: 3 });
  assert.equal(atLimit(plan({ usage: { hosts: 3, machines: 3 }, entitlements: { "hosts.max": 20 } }), "hosts.max"), null, "hosts.max wins over the old name");
  assert.deepEqual(atLimit(plan({ usage: { machines: 3 } }), "hosts.max"), { key: "hosts.max", limit: 3, used: 3 }, "an older hub: machines.max and usage.machines");
  assert.equal(atLimit(plan({ usage: { hosts: 9 }, entitlements: { "hosts.max": null } }), "hosts.max"), null, "null: no limit");
});

test("the hub's 409 limit reads back into the same words as the check before it", async () => {
  const answers = [
    { error: "limit", key: "devices.max", limit: 5, used: 5, message: "this account has 5 devices, as many as its plan allows: remove one first" },
    { error: "limit", key: "machines.max", limit: 3, used: 3, message: "this account has 3 machines (and 0 being added): its plan allows 3" },
    { error: "limit", key: "relay.max", limit: 2, used: 2, message: "free accounts use at most 2 relays" },
  ];
  let i = 0;
  const api = new HubApi("https://hub.test", async () => new Response(JSON.stringify(answers[i++]), { status: 409, headers: { "content-type": "application/json" } }));
  const errors: unknown[] = [];
  for (let n = 0; n < answers.length; n++) {
    try {
      await api.appendChain("tok", [new Uint8Array([1])]);
    } catch (error) {
      errors.push(error);
    }
  }
  assert.ok(errors[0] instanceof HubApiError);
  assert.deepEqual(limitOf(errors[0]), { key: "devices.max", limit: 5, used: 5 });
  assert.deepEqual(limitOf(errors[1]), { key: "machines.max", limit: 3, used: 3 });
  assert.equal(limitOf(new HubApiError(409, "exists", "no")), null, "another 409 is not a limit");
  // `errorText` speaks through the page's own i18next: English here (the keys).
  await i18next.init({ lng: "en", resources: {}, keySeparator: false, nsSeparator: false, interpolation: { escapeValue: false } });
  assert.equal(errorText(errors[0]), "Device limit reached (5 devices): first remove an old device in my devices, on a device already in the account");
  assert.equal(errorText(errors[1]), "Limit reached for devices that can run AI (3)");
  assert.equal(errorText(errors[2]), "free accounts use at most 2 relays", "a limit with no words here: the hub's own message");
  const t = (key: string, opts?: Record<string, unknown>) => i18n.t(key, opts);
  void i18n.changeLanguage("zh-CN");
  assert.equal(limitText({ key: "devices.max", limit: 5, used: 5 }, t), "设备已达上限（5 台）：先在已有设备的「我的设备」里移除一台旧设备");
  assert.equal(limitText({ key: "machines.max", limit: 3, used: 3 }, t), "可运行 AI 的设备已达上限（3 台）");
  assert.equal(limitText({ key: "hosts.max", limit: 3, used: 3 }, t), "可运行 AI 的设备已达上限（3 台）", "hosts.max, the new name, says the same");
});

test("the limit note says it, and offers the way on", () => {
  const html = draw(<LimitNote hit={{ key: "machines.max", limit: 3, used: 3 }} action={{ label: "machines", onClick: () => undefined }} />);
  assert.match(html, /data-testid="plan-limit"/);
  assert.match(html, /data-signal="warn"/);
  assert.ok(html.includes("Limit reached for devices that can run AI (3)"));
  assert.ok(html.includes(">machines<"));
  const zh = draw(<LimitNote hit={{ key: "devices.max", limit: 5, used: 5 }} />, "zh-CN");
  assert.ok(zh.includes("设备已达上限（5 台）：先在已有设备的「我的设备」里移除一台旧设备"));
});

test("offline since: the hub's last sighting, the live presence when it went away after /v1/me, nothing while online", () => {
  const offline = { presence: { state: "offline" as const, since_ms: 9_000 }, lastSeenMs: 5_000 };
  assert.equal(offlineSince(offline), 5_000);
  assert.equal(offlineSince(offline, { state: "online", sinceMs: 10_000 }), null, "back online");
  assert.equal(offlineSince({ presence: { state: "online", since_ms: 1 }, lastSeenMs: 2 }, { state: "offline", sinceMs: 7_000 }), 7_000);
  assert.equal(offlineSince({ presence: { state: "unknown", since_ms: 3_000 }, lastSeenMs: null }), 3_000);
  assert.equal(offlineSince({ presence: { state: "online", since_ms: 1 }, lastSeenMs: 2 }), null);
});

test("the offline copy's warning follows the plan's retention: free warns from day 5, pro from day 28", () => {
  const now = 1_000 * DAY_MS;
  const copy = { streams: 4, bytes: 1_000, lastWriteMs: 0 };
  const gone = { streams: 0, bytes: 0, lastWriteMs: null };
  const at = (days: number, retentionDays: number | null, c: typeof copy | typeof gone | null = copy) =>
    copyWarning({ retentionDays, offlineSinceMs: now - days * DAY_MS, copy: c, now });
  // Free: 7 days.
  assert.equal(at(4.9, 7), null);
  assert.deepEqual(at(5, 7), { kind: "soon", offlineDays: 5, daysLeft: 2, retentionDays: 7 });
  assert.deepEqual(at(6.5, 7), { kind: "soon", offlineDays: 6, daysLeft: 1, retentionDays: 7 });
  // Past 7 days the hub's hourly sweep (retention + 1) may not have run: still "soon" while it holds streams…
  assert.deepEqual(at(7.5, 7), { kind: "soon", offlineDays: 7, daysLeft: 1, retentionDays: 7 });
  // …"deleted" once it holds none, or the extra day is over too.
  assert.deepEqual(at(7.5, 7, gone), { kind: "deleted", retentionDays: 7 });
  assert.deepEqual(at(8.5, 7), { kind: "deleted", retentionDays: 7 });
  assert.deepEqual(at(10, 7, null), { kind: "deleted", retentionDays: 7 });
  // Pro: 30 days — a week away says nothing.
  assert.equal(at(5, 30), null);
  assert.equal(at(8.5, 30), null);
  assert.equal(at(27.9, 30), null);
  assert.deepEqual(at(28, 30), { kind: "soon", offlineDays: 28, daysLeft: 2, retentionDays: 30 });
  assert.deepEqual(at(31, 30, gone), { kind: "deleted", retentionDays: 30 });
  // Online, a retention the page does not know, a clock behind: nothing.
  assert.equal(copyWarning({ retentionDays: 7, offlineSinceMs: null, copy, now }), null);
  assert.equal(at(9, null), null);
  assert.equal(copyWarning({ retentionDays: 7, offlineSinceMs: now + DAY_MS, copy, now }), null);
  assert.equal(retentionOf(plan()), 7);
  assert.equal(retentionOf(pro()), 30);
  assert.equal(retentionOf(plan({ entitlements: { "storage.retention_days": null } })), null);
});

test("the machine list says the §3.8 words, in the plan's numbers", () => {
  const soon = draw(<CopyWarningLine warning={{ kind: "soon", offlineDays: 5, daysLeft: 2, retentionDays: 7 }} plan="free" />, "zh-CN");
  assert.ok(soon.includes("已离线 5 天：再过 2 天，云端离线副本将被删除（免费版保留 7 天）"), soon);
  assert.match(soon, /data-signal="warn"/);
  const deleted = draw(<CopyWarningLine warning={{ kind: "deleted", retentionDays: 7 }} plan="free" />, "zh-CN");
  assert.ok(deleted.includes("已离线超过 7 天，云端离线副本已删除。设备重新上线后会自动恢复最近 7 天。"), deleted);
  const en = draw(<CopyWarningLine warning={{ kind: "soon", offlineDays: 28, daysLeft: 2, retentionDays: 30 }} plan="pro" />);
  assert.ok(en.includes("Offline for 28 days: in 2 days the cloud offline copy will be deleted (Pro plan keeps 30 days)"), en);
  // After the first machine.
  const first = draw(<FirstMachineNote plan={plan()} />, "zh-CN");
  assert.ok(first.includes("离线时也能看最近 7 天的会话。") && first.includes("免费版：设备离线超过 7 天，云端副本会被删除。"), first);
  assert.ok(draw(<FirstMachineNote plan={pro()} />).includes("the last 30 days of sessions"));
  assert.equal(draw(<FirstMachineNote plan={null} />), "");
  // The top of a session copy the window trimmed.
  assert.ok(draw(<CopyWindowLine count={42} days={7} />, "zh-CN").includes("云端只保留最近 7 天；更早的 42 条在设备上，设备在线时可看"));
  assert.ok(draw(<CopyWindowLine count={3} days={null} />).includes("The 3 earlier entries are on the device"));
});

test("storage: amber from 90 %, red when full, each machine's share and the rest", () => {
  const machines = [
    { id: "m-small", copy: { streams: 2, bytes: 1 * GIB, lastWriteMs: 1_000 } },
    { id: "m-big", copy: { streams: 9, bytes: 3 * GIB, lastWriteMs: 5_000 } },
    { id: "m-new", copy: null },
  ];
  const at = (bytes: number) => storageView(plan({ usage: { storageBytes: bytes } }), machines)!;
  assert.equal(at(4.4 * GIB).level, "ok");
  assert.equal(at(4.5 * GIB).level, "high");
  assert.equal(at(5 * GIB).level, "full");
  assert.equal(at(5.5 * GIB).level, "full", "past it after a downgrade");
  const view = at(4.6 * GIB);
  assert.deepEqual(
    view.machines.map((m) => m.id),
    ["m-big", "m-small", "m-new"],
  );
  assert.equal(Math.round(view.otherBytes / (0.1 * GIB)), 6, "mail, keyring and the like");
  assert.equal(view.lastWriteMs, 5_000);
  assert.equal(formatStoragePair(view.usedBytes, view.limitBytes), "4.6 / 5 GB");
  assert.equal(formatStoragePair(300 * 1024 ** 2, 5 * GIB), "300 MB / 5 GB", "each in its own unit when they differ");
  assert.equal(formatStoragePair(0, 5 * GIB), "0 / 5 GB");
  assert.equal(formatPlanBytes(100 * GIB), "100 GB");
  assert.equal(storageView(plan({ entitlements: { "storage.bytes": null }, usage: { storageBytes: 99 * GIB } }), [])!.level, "ok", "no limit, never full");
  assert.equal(storageView(null, machines), null);
});

/** The words as read, without the markup. */
const plain = (html: string) => html.replace(/<[^>]+>/g, "");

test("the usage at the top of 「我的设备」: the plan, the transfer this period, the cloud storage (by device, folded), the devices and those that run AI — in the plan's numbers", () => {
  const machines = [{ id: "m-1", copy: { streams: 3, bytes: 4 * GIB, lastWriteMs: Date.UTC(2026, 9, 17, 14, 2) } }];
  const high = draw(<UsagePanel plan={plan({ usage: { storageBytes: 4.6 * GIB, devices: 2, machines: 1 } })} machines={machines} names={new Map([["m-1", "Mac mini"]])} />, "zh-CN");
  assert.match(high, /data-testid="usage-summary"/);
  // One sentence first, then five numbers: the summary is not five framed boxes.
  assert.match(high, /data-testid="usage-summary-line"[^>]*>[^<]*免费版/);
  assert.match(high, /data-section-head=""[^>]*>[\s\S]*?<h2[^>]*class="[^"]*text-base[^"]*font-semibold/);
  assert.match(high, /@container\/usage/);
  assert.match(high, /@min-\[36rem\]\/usage:col-span-2/);
  assert.match(high, /@min-\[70rem\]\/usage:grid-cols-5/);
  assert.doesNotMatch(high, /data-testid="usage-storage"[^>]*border/, "cells are separated by the grid's 1px, not each framed");
  assert.ok(high.includes("免费版"));
  assert.match(high, /data-testid="usage-storage"[\s\S]*>4\.6 GB<[\s\S]*\/ 5 GB/);
  assert.match(high, /role="meter"[^>]*aria-valuenow="92"/);
  assert.match(high, /data-testid="usage-devices"[\s\S]*>2<[\s\S]*\/ 5</);
  assert.match(high, /data-testid="usage-hosts"[\s\S]*>1<[\s\S]*\/ 3</);
  assert.ok(high.includes("离线副本保留 7 天"), "the plan's retention, with the plan");
  assert.ok(!high.includes("机器") && !high.includes("电脑"), "设备, not 机器 or 电脑");
  assert.ok(high.includes("云端存储已用 4.6 / 5 GB，最旧的会话副本会先被清理"), high);
  assert.ok(high.includes("设备离线超过 7 天，云端的离线副本会被删除；设备上的记录不受影响。"));
  assert.ok(!high.includes("升级"), "nothing to upgrade with yet");
  // Each device's offline copy is folded behind 「按设备」 until asked for.
  assert.match(high, /data-testid="storage-by-device" aria-expanded="false"/);
  assert.ok(!high.includes('data-testid="storage-machines"'));
  const full = draw(<UsagePanel plan={plan({ usage: { storageBytes: 5 * GIB } })} machines={machines} />, "zh-CN");
  assert.match(full, /data-testid="storage-full-line"/);
  assert.ok(full.includes("云端存储已满：设备在线时一切照常，离线副本停在 "));
  const fresh = [{ id: "m-aaaabbbbccccdddd", copy: { streams: 2, bytes: 90.9 * 1024 ** 2, lastWriteMs: null } }];
  const ok = draw(<UsagePanel plan={pro({ usage: { storageBytes: 91.8 * 1024 ** 2, devices: 1, machines: 1 } })} machines={fresh} />, "zh-CN");
  assert.ok(ok.includes(">91.8 MB<") && ok.includes("/ 100 GB"), ok);
  assert.ok(ok.includes("离线副本保留 30 天"));
  assert.ok(!ok.includes("storage-high") && !ok.includes("storage-full-line"));
  const en = draw(<UsagePanel plan={pro({ usage: { storageBytes: GIB } })} machines={machines} names={new Map([["m-1", "studio"]])} />);
  assert.ok(en.includes("Pro plan") && en.includes("offline copies kept 30 days") && en.includes("A device offline for more than 30 days"), en);
  const open = draw(<UsagePanel plan={plan({ entitlements: { "storage.retention_days": null, "devices.max": null, "machines.max": null } })} machines={[]} />, "zh-CN");
  assert.ok(open.includes("离线副本不限期"), open);
  assert.match(open, /data-testid="usage-devices"[\s\S]*\/ 不限/);
  assert.ok(!open.includes("retention-note"));
  // hosts.max (and usage.hosts) when the hub says them; machines.max only from a hub that does not.
  const v3 = draw(<UsagePanel plan={pro({ usage: { storageBytes: GIB, devices: 3, hosts: 2, machines: 2 }, entitlements: { "devices.max": 40, "hosts.max": 20 } })} machines={machines} />);
  assert.match(v3, /data-testid="usage-devices"[\s\S]*>3<[\s\S]*\/ 40</);
  assert.match(v3, /data-testid="usage-hosts"[\s\S]*>2<[\s\S]*\/ 20</);
  // At a count's limit: amber, nothing more can be added.
  assert.match(draw(<UsagePanel plan={plan({ usage: { devices: 5 } })} machines={[]} />), /data-testid="usage-devices"[\s\S]*data-signal="warn"/);
});

test("the transfer this period: what went through the relays against the plan's share, when it resets, slowed once used up — and said when the hub does not say", () => {
  const period = { periodStartMs: Date.UTC(2026, 9, 1), periodEndMs: Date.UTC(2026, 10, 1) };
  const used = draw(<UsagePanel plan={pro({ usage: { transfer: { ...period, bytes: 3.4 * GIB, included: 100 * GIB, overageBytes: 0, throttled: false } } })} machines={[]} />, "zh-CN");
  assert.match(used, /data-testid="usage-transfer"[\s\S]*>3\.4 GB<[\s\S]*\/ 100 GB/);
  assert.ok(used.includes("经中继转发 · 11月1日重置"), used);
  assert.match(used, /data-testid="usage-transfer"[\s\S]*?role="meter"[^>]*aria-valuenow="3"/);
  const slowed = draw(<UsagePanel plan={plan({ usage: { transfer: { ...period, bytes: 10 * GIB, included: 10 * GIB, overageBytes: 0, throttled: true } } })} machines={[]} />);
  assert.ok(slowed.includes("used up: relayed connections are slowed until the period ends"));
  assert.match(slowed, /data-testid="usage-transfer"[\s\S]*data-signal="failed"/);
  const unknown = draw(<UsagePanel plan={plan()} machines={[]} />);
  assert.match(unknown, /data-testid="usage-transfer"[\s\S]*>—<[\s\S]*not reported by Droppen yet/);
  const unlimited = draw(<UsagePanel plan={pro({ usage: { transfer: { ...period, bytes: GIB, included: null, overageBytes: 0, throttled: false } } })} machines={[]} />);
  assert.match(unlimited, /data-testid="usage-transfer"[\s\S]*\/ no limit/);
});

test("a downgrade in its grace says when it ends and what the plan it drops to keeps", () => {
  const now = Date.UTC(2026, 9, 17);
  const until = Date.UTC(2026, 9, 24, 12);
  // During the grace the entitlements are the larger plan's: the numbers come from the plan dropped to.
  const graced = plan({ gracePlan: "pro", graceUntilMs: until, entitlements: { "storage.retention_days": 30, "storage.bytes": 100 * GIB } });
  assert.deepEqual(downgradeOf(graced, now), {
    from: "pro",
    to: "free",
    untilMs: until,
    after: { retentionDays: 7, storageBytes: 5 * GIB, transferBytes: 10 * GIB },
  });
  assert.equal(downgradeOf(graced, until + 1), null, "over");
  assert.equal(downgradeOf(plan(), now), null);
  const zh = draw(<PlanStrips plan={graced} machines={[]} now={now} />, "zh-CN");
  assert.match(zh, /data-testid="plan-downgrade"/);
  assert.match(zh, /Pro 版将于 10月24日 结束：届时云端只保留 7 天（设备离线超过 7 天，副本会被删除），存储上限 5 GB，流量每月 10 GB/);
  assert.equal(draw(<PlanStrips plan={plan()} machines={[]} now={now} />), "", "nothing to say");
  const full = draw(<PlanStrips plan={plan({ usage: { storageBytes: 6 * GIB } })} machines={[{ id: "m", copy: { streams: 1, bytes: GIB, lastWriteMs: null } }]} now={now} />);
  assert.match(full, /data-testid="storage-full"/);
  assert.ok(full.includes("the offline copy is no longer updated"));
});

test("the offline copy's notice says so when the cloud storage is full", () => {
  const at = Date.UTC(2026, 9, 15, 9, 30);
  assert.ok(!draw(<OfflineNotice freshestMs={at} />).includes("offline-notice-storage"));
  const html = draw(<OfflineNotice freshestMs={at} storageFull={{ stoppedMs: at }} />, "zh-CN");
  assert.match(html, /data-testid="offline-notice-storage"/);
  assert.ok(html.includes("云端存储已满：设备在线时一切照常，离线副本停在 "));
});

test("a device registered with the password is an alert until “that was me”, and only while it matters", () => {
  const now = 10 * DAY_MS;
  const seen = [
    { device: "d-new", createdMs: now - 60_000 },
    { device: "d-me", createdMs: now - 1_000 },
    { device: "d-added", createdMs: now - 5_000 },
    { device: "d-old", createdMs: now - 2 * DAY_MS },
    { device: "d-ok", createdMs: now - 3_000 },
  ];
  const alerts = registrationAlerts(seen, { self: "d-me", inChain: (id) => id === "d-added", dismissed: new Set(["d-ok"]), now });
  assert.deepEqual(alerts.map((a) => a.device), ["d-new"], "not this device, not one in the chain, not a dismissed or stale one");
  const html = draw(<RegistrationList alerts={alerts} onDismiss={() => undefined} />);
  assert.match(html, /data-testid="registered-devices"/);
  assert.ok(html.includes("A new device signed in with your password (d-new, "));
  assert.ok(html.includes("If it was not you, change your password."));
  assert.ok(html.includes(">that was me<"));
  assert.ok(draw(<RegistrationList alerts={alerts} onDismiss={() => undefined} />, "zh-CN").includes("有新设备用你的密码登录了（d-new，"));
  assert.equal(draw(<RegistrationList alerts={[]} onDismiss={() => undefined} />), "");
});

test("the /v1/me store: read, updated by the plan message, registrations heard and dismissed for good", async () => {
  const values = new Map<string, string>();
  const storage = { getItem: (k: string) => values.get(k) ?? null, setItem: (k: string, v: string) => void values.set(k, v) };
  let reads = 0;
  const view: AccountView = {
    account: "a-1",
    email: { address: "", verified: true, verification: "off", dueMs: null, delivery: null, deliveryError: null },
    plan: plan({ usage: { devices: 4 } }),
    chainSeq: 3,
    machines: [],
    devices: [
      { id: "d-1", createdMs: Date.now() - 10 * DAY_MS, lastSeenMs: null, inChain: true },
      { id: "d-9", createdMs: Date.now() - 60_000, lastSeenMs: null, inChain: false },
    ],
  };
  const store = new MeStore(async () => (reads++, view), { account: "a-1", storage, settleMs: 1 });
  let told = 0;
  store.onChange(() => told++);
  await store.refresh();
  assert.equal(reads, 1);
  assert.equal(store.plan?.usage.devices, 4);
  assert.deepEqual(store.registrations().map((r) => r.device), ["d-9"], "a registered device not in the chain");
  // The plan message: its detail is /v1/me's plan.
  assert.equal(
    store.control({
      t: "plan",
      plan: "free",
      retention_days: 7,
      storage_bytes: 5 * GIB,
      usage_bytes: 0,
      detail: {
        id: "free",
        entitlements: { "devices.max": 5, "storage.retention_days": 7 },
        usage: { storage_bytes: 0, devices: 5, machines: 0, relays: 0, transfer: null },
      },
    }),
    true,
  );
  assert.equal(store.plan?.usage.devices, 5);
  assert.ok(atLimit(store.plan, "devices.max"));
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(reads, 2, "the per-machine copies are read again after a plan message");
  assert.equal(store.control({ t: "device_registered", device: "d-10", created_ms: Date.now() }), true);
  assert.equal(store.control({ t: "presence" }), false);
  assert.deepEqual(store.registrations().map((r) => r.device).sort(), ["d-10", "d-9"]);
  store.dismiss("d-9");
  assert.ok(told >= 4);
  const again = new MeStore(async () => view, { account: "a-1", storage });
  assert.ok(again.dismissed.has("d-9"), "dismissed on this device for good");
  store.stop();
});

test("every new string has its zh-TW and ja", () => {
  for (const key of [
    "Device limit reached ({{n}} devices): first remove an old device in my devices, on a device already in the account",
    "Offline for {{days}} days: in {{left}} days the cloud offline copy will be deleted ({{plan}} keeps {{retention}} days)",
    "The cloud keeps the last {{days}} days; the {{count}} earlier entries are on the device, readable while it is online",
  ]) {
    assert.ok(cloudDevicesZhTW[key], `zh-TW: ${key}`);
    assert.ok(cloudDevicesJa[key], `ja: ${key}`);
  }
});
