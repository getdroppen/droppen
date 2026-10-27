// 「我的设备」 on droppen.app (docs/local-aggregator.md §3.7): one page at
// `/devices`, one way to it from the bar (「设备」), and one list on it —
// every device, the ones that run AI among them — under what the plan uses
// and this device's own settings. No filter: an old `?view=` link opens the
// same page.
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { askedPlace, DEVICES_PATH, isDevicesPath } from "../src/features/cloud/devices-route.js";
import { AccountName, DevicesButton, MyDevicesScreen } from "../src/features/cloud/my-devices.js";
import { AccountNotices, DevicesPanel, ThisDeviceSection } from "../src/features/cloud/devices.js";
import { openRelayPicker } from "../src/features/cloud/cloud-bar.js";
import { setHubShell } from "../src/features/machines/shells.js";
import { RelaySection } from "../src/features/cloud/relays.js";
import { constantStore } from "../src/lib/machines/store.js";
import { meStore } from "../src/lib/hub/me.js";
import { cloudDevicesZhCN } from "../src/i18n/cloud-devices.js";
import { zhCN } from "../src/i18n/zh-CN.js";

class MemoryStorage {
  values = new Map<string, string>();
  getItem(key: string) {
    return this.values.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.values.set(key, value);
  }
  removeItem(key: string) {
    this.values.delete(key);
  }
}
const storage = new MemoryStorage();
const place = { pathname: "/devices", search: "" };
Object.assign(globalThis, { localStorage: storage, location: place });

const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: { ...cloudDevicesZhCN, ...zhCN } } },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

function draw(node: React.ReactElement, lng = "en"): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
}

test("「我的设备」 is /devices", () => {
  assert.ok(isDevicesPath("/devices") && isDevicesPath("/devices/"));
  assert.ok(!isDevicesPath("/sessions") && !isDevicesPath("/devices/x") && !isDevicesPath("/"));
  assert.equal(DEVICES_PATH, "/devices");
});

/** An account with `hosts` computers that run AI and `others` browsers or phones. */
function account(hosts: number, others = 1) {
  const record = (id: string, host: boolean) => ({ info: { id }, host, admin: !host, status: { type: "active" } });
  const devices = [...Array.from({ length: others }, (_, i) => record(`d-${i}`, false)), ...Array.from({ length: hosts }, (_, i) => record(`m-${i}`, true))];
  return {
    account: "a-1",
    deviceId: "d-0",
    isAdmin: true,
    chain: { hosts: () => devices.filter((d) => d.host), allDevices: () => devices },
    onChange: () => () => undefined,
  };
}

/** What 「我的设备」 puts in its frame for `acct`. */
function shown(acct: ReturnType<typeof account>) {
  let out: React.ReactElement | null = null;
  function Probe() {
    out = MyDevicesScreen({ account: acct as never, link: {} as never }) as React.ReactElement;
    return null;
  }
  draw(<Probe />);
  const frame = out as unknown as React.ReactElement<{ children: React.ReactElement<Record<string, unknown>> }>;
  return { body: frame.props.children.type, props: frame.props.children.props };
}

test("one page whatever the account has: the list of every device, devices that run AI or not", () => {
  storage.values.clear();
  for (const acct of [account(2, 3), account(0, 2)]) {
    const page = shown(acct);
    assert.equal(page.body, DevicesPanel);
    assert.deepEqual(Object.keys(page.props).sort(), ["account", "link"], "the panel reads the hub origin itself (hubOrigin())");
    assert.match(
      readFileSync(new URL("../src/features/cloud/devices.tsx", import.meta.url), "utf8"),
      /origin=\{hubOrigin\(\) \?\? ""\}/,
      "the guide's one-time code takes the hub from the page's marker",
    );
  }
  // The same with a link of the old filter in the address.
  place.search = "?view=ai";
  assert.equal(shown(account(2)).body, DevicesPanel);
  place.search = "";
  const frame = readFileSync(new URL("../src/features/cloud/my-devices.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(frame, /TabsList|DevicesViewSwitch|DevicesDashboard/, "no filter, no dashboard");
  const alerts = readFileSync(new URL("../src/features/cloud/account-alerts.tsx", import.meta.url), "utf8");
  assert.equal(alerts.match(/<AccountNotices /g)?.length, 1, "the account's notices, once, above the list");
});

test("the bar has one way to 「我的设备」 — 「设备」, a grid — and nothing else leaves the app for a machine list", () => {
  const html = draw(<DevicesButton />);
  assert.match(html, /data-testid="my-devices"/);
  assert.match(html, /lucide-layout-grid/);
  assert.ok(html.includes(">devices<"));
  assert.ok(draw(<DevicesButton />, "zh-CN").includes(">设备<"));
  const bar = readFileSync(new URL("../src/features/cloud/cloud-bar.tsx", import.meta.url), "utf8");
  assert.equal(bar.match(/<DevicesButton \/>/g)?.length, 1);
  assert.doesNotMatch(bar, /select\(null\)/, "the old 「机器」 button (and the cover's) went to the list directly");
  assert.doesNotMatch(bar, /setDevicesOpen|DevicesHost/, "the devices page drawn over the app is gone");
});

test("the bar names the account by its e-mail once /v1/me says it, its id until then", () => {
  const acct = { account: "a-aaaabbbbccccdddd", api: { me: async () => ({}) }, session: { token: "t" } };
  const before = draw(<AccountName account={acct as never} link={null as never} />);
  assert.match(before, /data-testid="account-name"[^>]*>a-aaaabbbbccccdddd</);
  const store = meStore(acct as never);
  store.view = { account: "a-aaaabbbbccccdddd", email: { address: "owner@example.com" } } as never;
  const after = draw(<AccountName account={acct as never} link={null as never} />);
  assert.match(after, />owner@example\.com</);
  assert.match(after, /title="a-aaaabbbbccccdddd"/, "the id stays the tooltip");
});

test("“change relay…” opens 「我的设备」 at the relay picker, under 「这台设备」, unfolded", () => {
  const selected: (string | null)[] = [];
  setHubShell({ kind: "hub", select: (key: string | null) => void selected.push(key) } as never);
  try {
    openRelayPicker();
    assert.deepEqual(selected, [null], "out of the app, onto the page");
    assert.deepEqual(askedPlace.get(), { anchor: "relays" });
    // The relay picker is folded until asked for: asked, it opens.
    const html = draw(<ThisDeviceSection name="Mac · Safari" relays={<span data-testid="picker" />} />);
    assert.match(html, /<details open=""/);
    askedPlace.set(null);
    const folded = draw(<ThisDeviceSection name="Mac · Safari" relays={<span data-testid="picker" />} />);
    assert.doesNotMatch(folded, /<details open=""/);
    assert.match(folded, /data-testid="picker"/, "still on the page, folded");
  } finally {
    askedPlace.set(null);
    setHubShell(null);
  }
});

/** Every element of a tree a component returned, its props' elements included (a section given as a prop). */
function find(el: unknown, type: unknown): React.ReactElement | null {
  if (!el || typeof el !== "object") return null;
  if (Array.isArray(el)) {
    for (const k of el) {
      const hit = find(k, type);
      if (hit) return hit;
    }
    return null;
  }
  const e = el as React.ReactElement<Record<string, unknown>>;
  if (e.type === type) return e;
  for (const value of Object.values(e.props ?? {})) {
    const hit = find(value, type);
    if (hit) return hit;
  }
  return null;
}

test("the relay picker on 「我的设备」 is about the device the app last showed", () => {
  const relays = { directory: null, refresh: async () => undefined, measure: async () => undefined, onChange: () => () => undefined, manual: null };
  const channel = { status: "open" };
  setHubShell({
    kind: "hub",
    focus: constantStore(null),
    lastShown: "m-0",
    relays,
    machine: (key: string | null) => (key === "m-0" ? { channel } : null),
    machines: () => constantStore([]),
  } as never);
  try {
    let out: React.ReactElement | null = null;
    const base = account(1);
    const chain = { ...base.chain, isV3: true, admins: () => [], acceptPolicy: () => ({ type: "admins" }), machineKeyEpochs: () => 0, latestMachineEpoch: () => null, device: () => undefined, currentRecoveryKey: true };
    const acct = { ...base, chain, standing: "active", missingHostKeys: [], missingUserKey: null, rotationDue: false, reEnrollDue: () => new Set(), reEnrollWarnings: () => new Map(), heldBack: null, api: {}, session: { token: "t" }, machine: () => undefined, machineKeys: async () => new Map() };
    const link = new Proxy({ presence: new Map() } as Record<string, unknown>, { get: (t, k) => (k in t ? t[k as string] : () => () => undefined) });
    function Probe() {
      out = DevicesPanel({ account: acct as never, link: link as never }) as React.ReactElement;
      return null;
    }
    draw(<Probe />);
    const picker = find(out, RelaySection) as React.ReactElement<{ machine: string | null; channel: unknown; bare: boolean }> | null;
    assert.ok(picker, "the relay picker is on the page");
    assert.equal(picker.props.machine, "m-0", "no device in view: the one the app last showed");
    assert.equal(picker.props.channel, channel, "its channel, for “reconnect now”");
    assert.equal(picker.props.bare, true, "inside 「这台设备」, which names it already");
  } finally {
    setHubShell(null);
  }
});

test("the add guides: one way in on the page, the same chooser a hint opens", () => {
  const panel = readFileSync(new URL("../src/features/cloud/devices.tsx", import.meta.url), "utf8");
  assert.doesNotMatch(panel, /<AddMachine\b/, "not the one-time code straight away");
  assert.equal(panel.match(/<AddComputer\b/g)?.length, 1);
  assert.equal(panel.match(/data-testid="add-device"/g)?.length, 1, "one 「添加设备」");
});

test("what the account needs said is above the list, a key rotation due included", () => {
  let out: React.ReactElement | null = null;
  const acct = { ...account(1), isAdmin: true, standing: "active", missingHostKeys: [], missingUserKey: 3, rotationDue: true, reEnrollDue: () => new Set(["m-0"]), chain: { ...account(1).chain, isV3: true } };
  const link = new Proxy({ presence: new Map() } as Record<string, unknown>, { get: (t, k) => (k in t ? t[k as string] : () => () => undefined) });
  function Probe() {
    out = AccountNotices({ account: acct as never, link: link as never }) as React.ReactElement;
    return null;
  }
  draw(<Probe />);
  const kids = ([] as unknown[]).concat((out as unknown as React.ReactElement<{ children: unknown }>).props.children).filter(Boolean) as React.ReactElement[];
  const types = kids.map((k) => (typeof k.type === "function" ? k.type.name : k.props && (k.props as Record<string, unknown>)["data-testid"]));
  assert.ok(types.includes("RotateKeys"), "the due rotation");
  assert.ok(types.includes("MissingUserKey"), "the missing user key");
  assert.ok(types.includes("V3Note"), "the hub not taking v3 yet");
  assert.ok(!types.includes("re-enroll-due"), "a device to enroll again says so on its own row, on the same page");
});
