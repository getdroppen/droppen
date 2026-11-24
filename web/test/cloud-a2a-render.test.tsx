// An A2A proxy on 「我的设备」 (docs/a2a.md §7.2, §7.3): its row carries the
// 「A2A 代理」 badge and never "make it a full device" (nor keys to re-send),
// its details say which devices take its calls and a host's say which proxies
// may call it, its menu opens the panel that changes that, and the steps of
// adding one say what they must — where the code goes and that it goes away,
// the fingerprint pasted from the proxy's own log, which devices must be
// updated first and why.
//
// `react-dom/server` with no DOM, like the other render tests.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { A2aAcceptPanel, A2aStepView, type A2aAddState, A2A_PROXY_DOCS_URL } from "../src/features/cloud/add-a2a-proxy.js";
import { DeviceRow, type DeviceRowView, rowMenuItems } from "../src/features/cloud/device-row.js";
import { AddChooser, DevicesPanel, HostControl } from "../src/features/cloud/devices.js";
import { deviceFingerprint, type CloudAccount } from "../src/lib/hub/account.js";
import { generateA2aEnrollCode } from "../src/lib/e2e/index.js";
import type { HubLink } from "../src/lib/hub/link.js";
import { cloudDevicesJa, cloudDevicesZhCN } from "../src/i18n/cloud-devices.js";
import { onboardingJa, onboardingZhCN } from "../src/i18n/onboarding.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { createV2Genesis, FakeKeysHub } from "./fake-keys-hub.js";
import { driveWith, type El, makeI18n, pageDraw } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: { ...onboardingZhCN, ...cloudDevicesZhCN, ...zhCN } }, ja: { translation: { ...onboardingJa, ...cloudDevicesJa, ...ja } } },
  keySeparator: false,
  nsSeparator: false,
});
const draw = pageDraw(i18n);
const drive = driveWith(draw);
const noop = () => undefined;

/** An admin browser on a v3 account with host m-1, and A2A proxy a-1 added the way the page adds one, m-1 taking its calls. */
async function withProxy() {
  const hub = new FakeKeysHub();
  hub.chainV3Mode = "write";
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  await a.upgradeToV3({ writable: true });
  await hub.enroll(a, "m-2", { writable: true });
  const { code } = await a.startA2aEnrollment();
  const proxy = await hub.claimA2a(a, code, { id: "a-1" });
  const claim = await a.awaitA2aClaim(code, undefined, 1);
  await a.addA2aProxy(claim, await deviceFingerprint(proxy.signKey, proxy.dhKey), ["m-1"]);
  const calls: unknown[][] = [];
  Object.assign(a, {
    setA2aHosts: async (...args: unknown[]) => {
      calls.push(["setA2aHosts", ...args.slice(0, 2)]);
      return { widened: [], narrowed: [] };
    },
  });
  const link = new Proxy({ presence: new Map() } as Record<string, unknown>, { get: (t, k) => (k in t ? t[k as string] : () => () => undefined) }) as unknown as HubLink;
  return { a, link, calls, props: { account: a as CloudAccount, link } };
}

const rowOf = (tree: El[], id: string): El => {
  const row = tree.find((e) => (e.props.view as DeviceRowView | undefined)?.id === id && "onPromote" in e.props);
  assert.ok(row, `a row for ${id}`);
  return row;
};
const itemsOf = (row: El) => rowMenuItems(row.props.view as DeviceRowView, { ...(row.props as Parameters<typeof rowMenuItems>[1]), t: i18n.getFixedT("en") });

test("an A2A proxy's row: its badge, never “make it a full device” nor keys to re-send; its menu changes who takes its calls", async () => {
  const { props, calls } = await withProxy();
  const page = drive(DevicesPanel, props, []);
  const row = rowOf(page.tree, "a-1");
  const view = row.props.view as DeviceRowView;
  assert.equal(view.kind, "a2a");
  assert.equal(view.limited, null, "never a limited device to promote");
  assert.equal(view.full, false);
  assert.equal(view.host, false);
  assert.deepEqual(view.a2aHosts, ["m-1"]);
  assert.equal(view.name, "A2A proxy a-1");
  assert.match(page.html, /data-testid="a2a-a-1"[^>]*>A2A proxy</);
  assert.doesNotMatch(page.html, /data-testid="limited-a-1"|data-testid="full-a-1"/);
  const ids = itemsOf(row).map((i) => i.id);
  assert.deepEqual(ids, ["a2a-accept-a-1", "remove-a-1"]);
  assert.ok(!ids.includes("promote-a-1") && !ids.includes("resend-a-1") && !ids.includes("control-a-1"));
  // A host's details say which A2A proxies may call it.
  assert.deepEqual((rowOf(page.tree, "m-1").props.view as DeviceRowView).a2aProxies, ["a-1"]);
  assert.deepEqual((rowOf(page.tree, "m-2").props.view as DeviceRowView).a2aProxies, []);
  // Its menu opens the panel; saving changes the hosts through the account (narrowing with a courier).
  const open = (tree: El[]) => (rowOf(tree, "a-1").props.onA2aAccept as () => void)();
  const opened = drive(DevicesPanel, props, [open]);
  assert.match(opened.html, /data-testid="a2a-accept-a-1"/);
  const panel = opened.tree.find((e) => e.type === A2aAcceptPanel);
  assert.ok(panel);
  assert.deepEqual(panel.props.accepted, ["m-1"]);
  assert.deepEqual(
    (panel.props.hosts as { id: string }[]).map((h) => h.id),
    ["m-1", "m-2"],
  );
  drive(DevicesPanel, props, [open, (tree) => (tree.find((e) => e.type === A2aAcceptPanel)!.props.onSave as (p: string[]) => void)(["m-2"])]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls, [["setA2aHosts", "a-1", ["m-2"]]]);
  // Its removal is the row's usual one, with the exposure it leaves said: m-1 took its calls.
  const removing = drive(DevicesPanel, props, [(tree) => (rowOf(tree, "a-1").props.onRemove as () => void)()]);
  assert.match(removing.html, /data-testid="remove-a2a-exposed"/);
  assert.ok(removing.html.includes("could call the A2A of m-1"));
  // Who may control a host is never offered an A2A proxy to pick.
  const control = drive(DevicesPanel, props, [(tree) => (rowOf(tree, "m-1").props.onControl as () => void)()]);
  const hostControl = control.tree.find((e) => e.type === HostControl);
  assert.ok(hostControl);
  assert.ok(!(hostControl.props.candidates as { id: string }[]).some((c) => c.id === "a-1"));
});

test("a row and its details, as drawn: the badge, what an A2A proxy is, and who takes its calls; a host's A2A proxies", () => {
  const base: DeviceRowView = { id: "a-1", fingerprint: "ABCD-EFGH-IJKL-MNOP", name: "A2A proxy a-1", self: false, kind: "a2a", status: "active", host: false, stoppedHost: false, presence: null, path: null, copyAsOf: null, lastSeenMs: null, limited: null, accept: null };
  const html = draw(<DeviceRow view={{ ...base, a2aHosts: ["m-1", "m-2"] }} viewerAdmin upgraded writable onRemove={noop} onA2aAccept={noop} onResend={noop} onPromote={noop} />);
  assert.match(html, /data-testid="a2a-a-1"/);
  assert.ok(html.includes("It controls no device and holds none of your keys."));
  assert.ok(html.includes("devices that take its calls") && html.includes("m-1, m-2"));
  assert.ok(draw(<DeviceRow view={{ ...base, a2aHosts: [] }} viewerAdmin upgraded writable />).includes("no device yet"));
  const zh = draw(<DeviceRow view={{ ...base, a2aHosts: ["m-1"] }} viewerAdmin upgraded writable />, "zh-CN");
  assert.ok(zh.includes("A2A 代理") && zh.includes("接受它调用的设备"));
  assert.doesNotMatch(zh, /机器|电脑/);
  const host = draw(<DeviceRow view={{ ...base, id: "m-1", name: "Mac mini", kind: "computer", host: true, a2aProxies: ["a-1"] }} viewerAdmin upgraded writable />);
  assert.ok(host.includes("A2A proxies") && host.includes(">a-1<"));
  // Not upgraded: the menu item says why it cannot move.
  const blocked = rowMenuItems({ ...base, a2aHosts: [] }, { viewerAdmin: true, upgraded: false, writable: false, t: i18n.getFixedT("en"), onA2aAccept: noop });
  assert.equal(blocked.find((i) => i.id === "a2a-accept-a-1")?.blocked, "upgrade the account first");
});

test("「添加设备」 offers an A2A proxy, below the two kinds of device", () => {
  const html = draw(<AddChooser onPick={noop} onCancel={noop} />);
  assert.match(html, /data-testid="add-a2a"/);
  assert.ok(html.includes("an A2A proxy"));
  assert.ok(html.indexOf('data-testid="add-computer"') < html.indexOf('data-testid="add-a2a"'));
  assert.ok(draw(<AddChooser onPick={noop} onCancel={noop} />, "zh-CN").includes("A2A 代理"));
});

const steps = (state: A2aAddState, extra: Partial<Parameters<typeof A2aStepView>[0]> = {}, lng = "en") =>
  draw(<A2aStepView state={state} hosts={[{ id: "m-1", name: "Mac mini" }, { id: "m-2", name: null }]} onCheck={noop} onAdd={noop} onNewCode={noop} onDone={noop} {...extra} />, lng);

test("the code step: DROPPEN_ENROLL_CODE on the proxy's own service only, gone once it joined, the docs, and the time left", () => {
  const code = generateA2aEnrollCode(new Uint8Array(16).fill(7));
  const html = steps({ kind: "waiting", code, deadline: 1_000_000 + 9 * 60_000 }, { now: 1_000_000 });
  assert.match(html, /data-testid="a2a-code-line"[^>]*>DROPPEN_ENROLL_CODE=DA1-[A-Z2-7]+-[A-Z2-7]+</);
  assert.ok(html.includes("own service only, never as a shared variable"));
  assert.ok(html.includes("Remove it once the proxy has joined."));
  assert.ok(html.includes(`href="${A2A_PROXY_DOCS_URL}"`) && A2A_PROXY_DOCS_URL.endsWith("/docs/a2a-proxy.md"));
  assert.match(html, /data-testid="a2a-countdown"[^>]*>The code expires in 9:00\./);
  assert.match(steps({ kind: "waiting", code, deadline: 1_000_000 }, { now: 1_000_000 }), /data-testid="a2a-expired"/, "run out by the page's count");
  assert.match(steps({ kind: "expired" }), /data-testid="a2a-expired"/);
  const zh = steps({ kind: "waiting", code, deadline: 1_000_000 + 60_000 }, { now: 1_000_000 }, "zh-CN");
  assert.ok(zh.includes("只设在 A2A 代理自己的服务上"));
  assert.doesNotMatch(zh, /机器|电脑|本机/);
});

test("the fingerprint is asked for, never shown; what is no fingerprint is said; a mismatch ends the round with nothing added", async () => {
  const code = generateA2aEnrollCode(new Uint8Array(16).fill(7));
  const signKey = new Uint8Array(32).fill(1);
  const dhKey = new Uint8Array(32).fill(2);
  const expected = await deviceFingerprint(signKey, dhKey);
  const claimed = steps({ kind: "claimed", claim: { code, device: "a-1", signKey, dhKey } });
  assert.match(claimed, /data-testid="a2a-fingerprint-input"/);
  assert.ok(claimed.includes("claimed the code as a-1"));
  assert.ok(!claimed.includes(expected), "the page never shows the fingerprint it expects");
  assert.match(steps({ kind: "claimed", claim: { code, device: "a-1", signKey, dhKey } }, { error: "format" }), /data-testid="a2a-fingerprint-format"/);
  const aborted = steps({ kind: "aborted", why: "fingerprint", message: "" });
  assert.match(aborted, /data-testid="a2a-aborted" data-why="fingerprint"/);
  assert.ok(aborted.includes("The fingerprint does not match: nothing was added."));
  assert.match(aborted, /data-testid="a2a-new-code"/);
});

test("choosing the devices: none picked to start with, what a call can do, and the add", () => {
  const code = generateA2aEnrollCode(new Uint8Array(16).fill(7));
  const html = steps({ kind: "choose", claim: { code, device: "a-1", signKey: new Uint8Array(32), dhKey: new Uint8Array(32) }, pasted: "x" });
  assert.match(html, /data-testid="a2a-choose"/);
  assert.ok(html.includes("Mac mini") && html.includes("m-2"));
  assert.doesNotMatch(html, /aria-checked="true"|data-checked=""/, "nothing picked for the person");
  assert.ok(html.includes("can read any file that user can"));
  assert.match(html, /data-testid="a2a-add"/);
  const done = steps({ kind: "done", device: "a-1", hosts: ["m-1"] });
  assert.match(done, /data-testid="a2a-remove-code"[^>]*>Now remove DROPPEN_ENROLL_CODE from the proxy&#x27;s service/);
});

test("the upgrade gate: the devices that run AI to update first are named, with why — and no code is made", () => {
  const html = steps({ kind: "blocked", block: { kind: "devices", hosts: ["m-1", "m-2"] } });
  assert.match(html, /data-testid="a2a-blocked" data-block="devices"/);
  assert.ok(html.includes("Update Droppen on m-1, m-2 first."));
  assert.ok(html.includes("cannot read the entry that adds an A2A proxy: it would stop following your account"));
  assert.match(html, /data-testid="a2a-check-again"/);
  assert.doesNotMatch(html, /DA1-|a2a-code-line/);
  assert.ok(steps({ kind: "blocked", block: { kind: "hub" } }).includes("the hub does not take A2A proxies for this account yet"));
  assert.ok(steps({ kind: "blocked", block: { kind: "notUpgraded" } }).includes("an A2A proxy needs chain v3"));
  assert.ok(steps({ kind: "blocked", block: { kind: "devices", hosts: ["m-1"] } }, {}, "ja").includes("先に m-1 の Droppen を更新してください"));
});

test("changing who takes its calls: a device taken off is said to get a new content key, before it is saved", () => {
  const hosts = [
    { id: "m-1", name: null },
    { id: "m-2", name: null },
  ];
  const same = draw(<A2aAcceptPanel proxy="a-1" hosts={hosts} accepted={["m-1"]} busy={false} onSave={noop} onCancel={noop} />);
  assert.match(same, /data-testid="a2a-accept-save-a-1"[^>]*disabled=""/, "nothing changed: nothing to save");
  assert.doesNotMatch(same, /a2a-accept-narrowing/);
  // m-2 taken off (its box cleared): said before the save, whose label says the keys turn over.
  const pick = (next: string[]) => (tree: El[]) => (tree.find((e) => "picked" in e.props && "onChange" in e.props)!.props.onChange as (s: Set<string>) => void)(new Set(next));
  const props = { proxy: "a-1", hosts, accepted: ["m-1", "m-2"], busy: false, onSave: noop, onCancel: noop };
  const narrowed = drive(A2aAcceptPanel, props, [pick(["m-1"])]);
  assert.match(narrowed.html, /data-testid="a2a-accept-narrowing"[^>]*>m-2 stop taking its calls and are treated as exposed/);
  assert.ok(narrowed.html.includes("They get a new content key now and renew their identity key after it"));
  assert.ok(narrowed.html.includes("save and rotate their keys"));
  // Which devices then renew their identity key on their own, as the account works it out.
  const renewing = drive(A2aAcceptPanel, { ...props, renewsIfNarrowed: (dropped: string[]) => [...dropped, "m-3"] }, [pick(["m-1"])]);
  assert.match(renewing.html, /data-testid="renewals-ahead"[^>]*>2 devices that run AI will renew their identity key automatically/);
  assert.ok(renewing.html.includes("m-2, m-3"));
  const widened = drive(A2aAcceptPanel, props, [pick(["m-1", "m-2", "m-3"])]);
  assert.doesNotMatch(widened.html, /a2a-accept-narrowing/, "adding one hands nothing over");
  const zh = drive(A2aAcceptPanel, props, [pick([])], "zh-CN");
  assert.ok(zh.html.includes("m-1, m-2 不再接受它的调用，并被当作已暴露"));
});
