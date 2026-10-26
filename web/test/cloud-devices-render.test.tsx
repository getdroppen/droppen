// The cloud screens for devices, pairing and recovery words
// (`features/cloud/devices.tsx`, `pairing-screen.tsx`, `recovery.tsx`,
// `qr.tsx`): the QR code they draw reads back to the offer, the words screen
// lists the words in order, the phrase check says what is wrong in words, the
// pending-recovery warning offers approve and remove, and every string they
// use is translated.
//
// `react-dom/server` with no DOM, like the other render tests.
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import i18next from "i18next";
import jsQR from "jsqr";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { ForkScreen, RefusedScreen } from "../src/features/cloud/cloud-app.js";
import { OldKeyNotice } from "../src/features/cloud/cloud-bar.js";
import { ApiError, errorText as apiErrorText, hostRefusalText } from "../src/api/errors.js";
import { deliverMachineKey, type MachineKeyCourier } from "../src/lib/hub/deliver.js";
import { RenameError, renameErrorText } from "../src/features/cloud/machine-name.js";
import { formatAsOf } from "../src/lib/format.js";
import {
  alertLevel,
  alertText,
  blockedByHold,
  ChainAlerts,
  ConfirmCandidate,
  ControlConfirm,
  controlSaveStep,
  AddChooser,
  DeviceRow,
  DismissReEnroll,
  type DeviceRowView,
  AccountNotices,
  DevicesPanel,
  HostControl,
  hostSwitchSection,
  KeysNotSent,
  LimitedDeviceNote,
  MissingUserKey,
  PendingRecoveryBanner,
  PromoteDevice,
  RelayStandbySetting,
  RowConfirm,
  rowActions,
  rowMenuItems,
  type RowMenuItem,
  StopHost,
  UpgradeAccount,
  UpgradeConfirm,
  V3Note,
  alsoTrustedFor,
  trustedShown,
  VetoChoice,
} from "../src/features/cloud/devices.js";
import { AddComputer, NewDeviceReady, V3Pending, v3Blocker } from "../src/features/cloud/add-computer.js";
import { FirstHostCard } from "../src/features/cloud/first-host.js";
import { type AccountView, chainV3View, HubApiError } from "../src/lib/hub/api.js";
import { joinAction, type ReEnrollLine } from "../src/lib/hub/account.js";
import { meStore } from "../src/lib/hub/me.js";
import { addPasskeySlot, createV2Genesis, FakeCourier, FakeKeysHub, passkeyActivateEarly, passkeyJoin } from "./fake-keys-hub.js";
import { MachineNotes } from "../src/features/cloud/machine-name.js";
import { shortDeviceId } from "../src/features/cloud/device-row.js";
import { qrModules, qrPath } from "../src/features/cloud/qr.js";
import { phraseMessage, RecoveryWords } from "../src/features/cloud/recovery.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { crossDeviceJa, crossDeviceZhCN, crossDeviceZhTW } from "../src/i18n/cross-device.js";
import { onboardingJa, onboardingZhCN, onboardingZhTW } from "../src/i18n/onboarding.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import type { CloudAccount } from "../src/lib/hub/account.js";
import type { HubLink } from "../src/lib/hub/link.js";

const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: { ...cloudDevicesZhCN, ...zhCN } }, ja: { translation: { ...cloudDevicesJa, ...ja } } },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

function draw(node: React.ReactElement, lng = "en"): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
}

test("the QR code drawn for an offer reads back to the same text", async () => {
  const text = "RHP1-AEACAZDFMVZXIAIA2E4D4J4ANSMMNGBTVVBW3SCJZBGZLJYMVIIBG4KRHUBV7VTXEFUP3V24OYGRAZ7XW3NRL2BKOOMXSVW5DSXIVBUFXVPXW7YDXUQCX5MUFFR3EQUUSZA5IIKXGF2BE43ZYO6DQTCT2KSI";
  const { size, path } = await qrModules(text);
  assert.ok(size >= 21);
  // Paint it (4 px a module, a 4-module quiet zone) and read it as a camera would.
  const quiet = 4;
  const scale = 4;
  const n = (size + 2 * quiet) * scale;
  const dark = new Set<string>();
  for (const m of path.matchAll(/M(\d+) (\d+)h1v1h-1z/g)) dark.add(`${Number(m[2]) - quiet},${Number(m[1]) - quiet}`);
  const pixels = new Uint8ClampedArray(n * n * 4);
  for (let y = 0; y < n; y++) {
    for (let x = 0; x < n; x++) {
      const r = Math.floor(y / scale) - quiet;
      const c = Math.floor(x / scale) - quiet;
      const v = dark.has(`${r},${c}`) ? 0 : 255;
      pixels.set([v, v, v, 255], (y * n + x) * 4);
    }
  }
  assert.equal(jsQR(pixels, n, n)?.data, text);
  assert.equal(qrPath(2, (r, c) => r === c, 1), "M1 1h1v1h-1zM2 2h1v1h-1z");
});

test("the words screen lists the twelve words in order, then asks some back", () => {
  const words = "abandon ability able about above absent absorb abstract absurd abuse access accident".split(" ");
  const html = draw(<RecoveryWords words={words} onDone={() => undefined} />);
  assert.match(html, /data-testid="recovery-words"/);
  let at = 0;
  for (const w of words) {
    const i = html.indexOf(`>${w}<`, at);
    assert.ok(i > at, `${w} in order`);
    at = i;
  }
  assert.ok(html.includes("I have written them down"));
  assert.ok(draw(<RecoveryWords words={words} onDone={() => undefined} />, "zh-CN").includes("我已抄好"));
});

test("the phrase check says what is wrong, in words", () => {
  const t = (key: string, opts?: Record<string, unknown>) => key.replace(/\{\{(\w+)\}\}/g, (_, k: string) => String(opts?.[k]));
  assert.equal(phraseMessage({ kind: "empty" }, t), null);
  assert.deepEqual(phraseMessage({ kind: "incomplete", count: 5 }, t), { text: "5 of 12 words", bad: false });
  assert.deepEqual(phraseMessage({ kind: "unknownWord", word: "xyzzy", index: 2 }, t), { text: "word 3, “xyzzy”, is not a recovery word", bad: true });
  assert.deepEqual(phraseMessage({ kind: "checksum" }, t), { text: "these words do not check out: one is wrong or out of place", bad: true });
  assert.deepEqual(phraseMessage({ kind: "wordCount", count: 13 }, t), { text: "a recovery phrase is 12 words, not 13", bad: true });
  assert.equal(phraseMessage({ kind: "ok", legacy: false }, t)?.bad, false);
});

test("every active device warns about a pending recovery, with approve and remove", () => {
  const fake = (pending: { id: string; joinSeq: number }[], standing = "active") =>
    ({ standing, chain: { pendingRecoveries: () => pending }, onChange: () => () => undefined }) as unknown as CloudAccount;
  const link = {} as HubLink;
  const html = draw(<PendingRecoveryBanner account={fake([{ id: "d-xyz", joinSeq: 4 }])} link={link} />);
  assert.match(html, /data-testid="pending-recovery"/);
  assert.ok(html.includes("d-xyz"));
  assert.ok(html.includes(">approve<") && html.includes(">remove<"));
  assert.equal(draw(<PendingRecoveryBanner account={fake([])} link={link} />), "");
  assert.equal(draw(<PendingRecoveryBanner account={fake([{ id: "d-xyz", joinSeq: 4 }], "pending")} link={link} />), "", "only an active device can act on it");
});

test("a self-activation this browser holds back says so, still with approve and remove", () => {
  const account = {
    standing: "active",
    chain: { pendingRecoveries: () => [{ id: "d-xyz", joinSeq: 4 }] },
    heldBack: { seq: 5, device: "d-xyz", readyMs: Date.UTC(2026, 9, 18, 12, 0), entries: [] },
    heldChanges: () => [
      { seq: 5, kind: "activateRecovered", by: null, subject: "d-xyz" },
      { seq: 6, kind: "addDevice", by: "d-xyz", subject: "d-z2" },
    ],
    onChange: () => () => undefined,
  } as unknown as CloudAccount;
  const html = draw(<PendingRecoveryBanner account={account} link={{} as HubLink} />);
  assert.ok(html.includes("already activated itself"), html);
  // M2 re-review N1: what approving would take in is shown, and removing says it takes out what came with it.
  assert.match(html, /data-testid="held-changes"/);
  assert.ok(html.includes("d-xyz added d-z2"));
  assert.ok(html.includes(">approve<") && html.includes(">remove it and what it brought in<"));
  assert.equal(blockedByHold(account, "d-other"), true, "other removals wait");
  assert.equal(blockedByHold(account, "d-xyz"), false);
});

test("a veto lists what it takes out before it is signed; what the devices this browser trusted added meanwhile is a choice apart, off by default (#17)", () => {
  const plan = {
    device: "d-xyz",
    scope: [
      { id: "d-xyz", host: false, by: null },
      { id: "d-z2", host: false, by: "d-xyz" },
    ],
    trusted: [
      { id: "d-c", host: false, by: "d-b" },
      { id: "m-9", host: true, by: "d-b" },
    ],
    voidsWords: true,
    trustedVoidsWords: false,
    voidsPasskeys: false,
    trustedVoidsPasskeys: false,
    seq: 5,
    view: ["d-1", "d-b"],
  };
  const choice = (over: Partial<typeof plan> = {}, alsoTrusted = false, lng = "en") =>
    draw(<VetoChoice plan={{ ...plan, ...over }} alsoTrusted={alsoTrusted} onAlsoTrusted={() => undefined} busy={false} onConfirm={() => undefined} onCancel={() => undefined} />, lng);
  const html = choice();
  assert.ok(html.includes("remove d-xyz?") && html.includes("This removes:"));
  assert.match(html, /data-testid="veto-target-d-z2"[^]*brought in by d-xyz/);
  assert.match(html, /data-testid="veto-voids-words"/, "the recovery words go with it: said before it is signed");
  // The choice: the list, by whom, which runs AI, and what taking them out costs.
  assert.match(html, /data-testid="veto-also-trusted"(?![^>]*checked)/, "off by default");
  assert.ok(html.includes("Also remove what your other devices added while this browser held it back, and what those added:"));
  assert.match(html, /data-testid="veto-target-d-c"[^]*brought in by d-b/);
  assert.match(html, /data-testid="veto-target-m-9"[^]*can run AI/);
  assert.ok(html.includes("one that runs AI has to be added again"));
  assert.ok(!html.includes("veto-trusted-voids-words") && !html.includes("veto-voids-passkeys") && !html.includes("veto-trusted-voids-passkeys"));
  assert.ok(html.includes(">remove it and what it brought in<"), "unchecked: the default scope");
  assert.ok(choice({}, true).includes(">remove all of them<"), "checked: all of them");
  const words = choice({ voidsWords: false, trustedVoidsWords: true, trusted: [{ id: "d-c", host: false, by: "d-b" }] }, true);
  assert.ok(!words.includes('data-testid="veto-voids-words"') && words.includes('data-testid="veto-trusted-voids-words"'), "the words go only with the wider choice");
  assert.ok(words.includes("one of them set the current ones, or came back with them"), "a device a trusted one approved may have come back with them");
  assert.ok(!words.includes("has to be added again"), "nothing that runs AI among them");
  // Passkey slots the removals turn off are said too.
  const slots = choice({ voidsPasskeys: true, trustedVoidsPasskeys: true }, true);
  assert.ok(slots.includes('data-testid="veto-voids-passkeys"') && slots.includes("Joining with the passkeys they set up, or came in with, is turned off too."));
  assert.ok(slots.includes('data-testid="veto-trusted-voids-passkeys"'));
  // The choice holds only for the very list it was made on: one that grew is unticked.
  const listed = { ...plan, trusted: [{ id: "d-c", host: false, by: "d-b" }] };
  const grown = { ...plan, trusted: [...listed.trusted, { id: "d-late", host: false, by: "d-b" }] };
  assert.equal(alsoTrustedFor(trustedShown(listed), listed), true);
  assert.equal(alsoTrustedFor(trustedShown(listed), grown), false, "d-late was never shown when it was ticked");
  assert.equal(alsoTrustedFor(null, listed), false);
  assert.equal(alsoTrustedFor("", { ...plan, trusted: [] }), false);
  // Nothing trusted was added: no choice to make.
  const none = choice({ trusted: [] }, true);
  assert.ok(!none.includes("veto-also-trusted") && none.includes(">remove it and what it brought in<"));
  assert.ok(choice({}, false, "zh-CN").includes("同时移除你的其他设备在本浏览器暂缓它期间添加的设备"));
  assert.ok(choice({}, false, "ja").includes("すべて削除") === false && choice({}, true, "ja").includes("すべて削除"));
});

test("a device missing the newest user key says so, and can ask again or rotate", () => {
  const account = { onChange: () => () => undefined } as unknown as CloudAccount;
  const html = draw(<MissingUserKey account={account} link={{} as HubLink} epoch={3} />);
  assert.match(html, /data-testid="missing-user-key"/);
  assert.ok(html.includes("newest user key (epoch 3)"));
  assert.ok(html.includes(">ask again<") && html.includes("rotate keys again"));
});

test("each device change another device made is an alert, with “that was me” and, where it applies, remove", () => {
  const alerts = [
    { seq: 5, kind: "addDevice", by: "d-1", subject: "d-9", atMs: Date.now() - 60_000, remove: "d-9" },
    { seq: 6, kind: "removeDevice", by: "d-1", subject: "d-3", atMs: Date.now(), remove: null },
    { seq: 7, kind: "setRecoveryKey", by: "d-1", subject: null, atMs: Date.now(), remove: "d-1" },
  ];
  const fake = (list: unknown[]) => ({ alerts: () => list, onChange: () => () => undefined }) as unknown as CloudAccount;
  const html = draw(<ChainAlerts account={fake(alerts)} link={{} as HubLink} />);
  assert.match(html, /data-testid="chain-alerts"[^>]*data-signal="warn"|data-signal="warn"[^>]*data-testid="chain-alerts"/);
  assert.ok(html.includes("d-1 added d-9 to this account"));
  assert.ok(html.includes("d-1 removed d-3 from this account"));
  assert.ok(html.includes("d-1 made new recovery words for this account"));
  assert.equal(html.split(">that was me<").length - 1, 3);
  assert.ok(html.includes(">remove d-9<") && html.includes(">remove d-1<"));
  assert.ok(!html.includes(">remove d-3<"), "nothing to remove for a removal");
  assert.ok(draw(<ChainAlerts account={fake(alerts)} link={{} as HubLink} />, "zh-CN").includes("是我操作的"));
  assert.equal(draw(<ChainAlerts account={fake([])} link={{} as HubLink} />), "");
});

test("a machine on an old key is amber, needs-you; a refused device keeps its keys and is told why", () => {
  const row = (oldKey: boolean) => draw(<MachineNotes id="m-1" status={null} oldKey={oldKey} />);
  const old = row(true);
  assert.ok(old.includes("it still uses an old key — open it to update"));
  assert.match(old, /data-signal="warn"/);
  assert.equal(row(false), "", "nothing to say");
  const notice = draw(<OldKeyNotice onRetry={() => undefined} />);
  assert.match(notice, /data-testid="old-key-notice"/);
  assert.ok(notice.includes(">try again<"));
  const refused = draw(<RefusedScreen reason="removed" />);
  assert.match(refused, /data-testid="device-refused"/);
  assert.ok(refused.includes("its keys stay here"));
  // A fork is an alarm of its own, with what conflicted — not "something went wrong".
  const fork = draw(<ForkScreen message="the hub served less of the chain than this device has seen" />);
  assert.match(fork, /data-signal="fork"/);
  assert.ok(fork.includes("the hub served less of the chain than this device has seen"));
  assert.ok(fork.includes("the account does not match what this browser verified"));
});

test("every string these screens use is translated into zh-CN, zh-TW and ja", () => {
  const files = ["devices.tsx", "renewal.tsx", "add-computer.tsx", "pairing-screen.tsx", "recovery.tsx", "qr.tsx", "ui.tsx", "cloud-app.tsx", "cloud-bar.tsx", "machine-name.tsx", "device-row.tsx", "my-devices.tsx", "email-verify.tsx", "plan.tsx", "onboarding-hints.tsx", "browser-support.tsx"].map((f) => readFileSync(new URL(`../src/features/cloud/${f}`, import.meta.url), "utf8"));
  const keys = new Set<string>();
  for (const src of files) for (const m of src.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)) keys.add(m[1]!);
  assert.ok(keys.size > 50);
  for (const [name, table] of [
    ["zh-CN", { ...onboardingZhCN, ...crossDeviceZhCN, ...cloudDevicesZhCN, ...zhCN }],
    ["zh-TW", { ...onboardingZhTW, ...crossDeviceZhTW, ...cloudDevicesZhTW, ...zhTW }],
    ["ja", { ...onboardingJa, ...crossDeviceJa, ...cloudDevicesJa, ...ja }],
  ] as const) {
    const missing = [...keys].filter((k) => !(k in table));
    assert.deepEqual(missing, [], `${name} lacks these`);
  }
});

/** A row of 「我的设备」 as the panel would make it. */
function row(over: Partial<DeviceRowView> = {}): DeviceRowView {
  return {
    id: "d-1",
    fingerprint: "ABCD-EFGH-IJKL-MNOP",
    name: null,
    self: false,
    kind: "browserOrPhone",
    status: "active",
    host: false,
    stoppedHost: false,
    presence: null,
    path: null,
    copyAsOf: null,
    lastSeenMs: null,
    limited: null,
    accept: null,
    ...over,
  };
}
const list = (node: React.ReactElement, lng = "en") => draw(<ul>{node}</ul>, lng);

/** What a row's 「⋯」 menu offers (it draws only once opened): every handler there, unless `ctx` says otherwise. */
function menu(view: DeviceRowView, ctx: Partial<Parameters<typeof rowMenuItems>[1]> = {}, lng = "en"): RowMenuItem[] {
  const noop = () => undefined;
  return rowMenuItems(view, {
    viewerAdmin: true,
    upgraded: true,
    writable: true,
    t: i18n.getFixedT(lng),
    onPromote: noop,
    onRemove: noop,
    onHost: noop,
    onControl: noop,
    onRename: noop,
    onTerminal: noop,
    onResend: noop,
    ...ctx,
  });
}
const item = (items: RowMenuItem[], id: string) => items.find((i) => i.id === id);

test("my devices: a computer that runs AI shows the badge, where it is, and — away — how fresh its offline copy is; its id and fingerprint are in its details", () => {
  const online = list(<DeviceRow view={row({ id: "m-1", name: "studio", kind: "computer", host: true, presence: "online", path: { relay: "tokyo", rttMs: 42.4 } })} viewerAdmin upgraded writable />);
  assert.match(online, /data-testid="device-m-1"[^>]*data-host=""/);
  assert.ok(online.includes(">can run AI<"), online);
  assert.match(online, /data-testid="name-m-1"[^>]*>studio</, "the name a person knows it by");
  assert.match(online, /<details data-testid="details-m-1"/, "the rest behind 「详情」");
  const summary = online.slice(online.indexOf("<summary"), online.indexOf("</summary>"));
  assert.ok(!/>m-1</.test(summary), "the id is not shown on the 56px line: it is in the disclosure");
  assert.match(online, /<details[^>]*>[\s\S]*>m-1<[\s\S]*<\/details>/, "the id is in the disclosure");
  assert.ok(online.includes("ABCD-EFGH-IJKL-MNOP"), "the fingerprint, inside the details");
  assert.match(online, /data-presence="online"/);
  assert.ok(online.includes("tokyo · 42 ms"), "the path and its round trip");
  assert.ok(!online.includes("copy-as-of"), "online: no offline copy line");
  const actions = menu(row({ id: "m-1", kind: "computer", host: true, presence: "online" }));
  assert.equal(item(actions, "control-m-1")?.label, "who can control it");
  assert.equal(item(actions, "host-m-1")?.label, "stop running AI");
  const at = new Date(2026, 9, 17, 14, 2).getTime();
  const away = list(<DeviceRow view={row({ id: "m-1", kind: "computer", host: true, presence: "offline", copyAsOf: at })} viewerAdmin upgraded writable />);
  assert.match(away, /data-testid="copy-as-of"/);
  assert.ok(away.includes(`offline copy · as of ${formatAsOf(at, at)}`) || away.includes(`offline copy · as of ${formatAsOf(at)}`), away);
  assert.match(formatAsOf(at, at), /02/, "a time of day the same day");
  assert.ok(formatAsOf(at, at + 3 * 86_400_000).length > formatAsOf(at, at).length, "with the date on another day");
  // A browser: no badge, no hosting controls.
  const browser = list(<DeviceRow view={row()} viewerAdmin upgraded writable />);
  assert.ok(!browser.includes("can run AI"));
  assert.deepEqual(menu(row()).map((i) => i.id), ["resend-d-1", "remove-d-1"], "re-send its keys, remove it");
  const zh = list(<DeviceRow view={row({ id: "m-1", kind: "computer", host: true, presence: "online" })} viewerAdmin upgraded writable />, "zh-CN");
  assert.ok(zh.includes(">可运行 AI<"), zh);
  const zhMenu = menu(row({ id: "m-1", kind: "computer", host: true, presence: "online" }), {}, "zh-CN").map((i) => i.label);
  assert.ok(zhMenu.includes("停止运行 AI") && zhMenu.includes("谁可以控制它") && zhMenu.includes("打开终端"), zhMenu.join(","));
  assert.ok(!zh.includes("可管理账号") && !zh.includes('role="switch"'), "no 「可管理账号」 switch");
  // Stopped: it may run AI again (behind the same v3 gate).
  const stopped = list(<DeviceRow view={row({ id: "m-2", kind: "computer", stoppedHost: true })} viewerAdmin upgraded writable />);
  assert.ok(stopped.includes(">stopped running AI<"));
  assert.equal(item(menu(row({ id: "m-2", kind: "computer", stoppedHost: true })), "host-m-2")?.label, "run AI again");
});

test("at a wide container a device is a framed card, and hover marks the frame and the name, never a full-row fill", () => {
  const html = list(<DeviceRow view={row({ id: "m-1", kind: "computer", host: true, presence: "online" })} viewerAdmin upgraded writable />);
  assert.match(html, /@min-\[44rem\]\/devices:border/);
  assert.match(html, /@min-\[44rem\]\/devices:p-4/);
  assert.match(html, /@min-\[44rem\]\/devices:hover:border-border-strong/);
  assert.match(html, /group-hover\/summary:underline/);
  const summary = html.slice(html.indexOf("<summary"), html.indexOf("</summary>"));
  assert.ok(!summary.includes("hover:bg-hover"), "hover does not paint the 1101px row");
  assert.match(html, /grid-cols-\[1rem_minmax\(0,1fr\)_auto\]/, "icon, name and one action column");
});

test("a row's actions are in its 「⋯」 menu, not a row of buttons; a device that runs AI keeps 「打开」 at hand", () => {
  const host = row({ id: "m-1", name: "studio", kind: "computer", host: true, presence: "online", hasCopy: true });
  const html = list(<DeviceRow view={host} viewerAdmin upgraded writable onOpen={() => undefined} onRemove={() => undefined} onHost={() => undefined} onControl={() => undefined} onResend={() => undefined} onTerminal={() => undefined} />);
  assert.match(html, /data-testid="actions-m-1"[^>]*aria-label="more actions"/);
  assert.match(html, /data-testid="open-m-1"[\s\S]*?>open</);
  for (const word of [">remove<", ">who can control it<", ">stop running AI<", ">re-send keys<"]) assert.ok(!html.includes(word), `${word} is in the menu, not on the row`);
  // Offline with a copy: it opens on the copy.
  assert.match(list(<DeviceRow view={{ ...host, presence: "offline" }} viewerAdmin upgraded writable onOpen={() => undefined} />), /data-testid="open-m-1"[\s\S]*?>read the copy</);
  // In the menu, a destructive one last; one that needs the device online says so while it is not.
  const items = menu(host);
  assert.deepEqual(items.map((i) => i.id), ["terminal-m-1", "rename-m-1", "control-m-1", "host-m-1", "resend-m-1", "remove-m-1"]);
  assert.equal(items.at(-1)!.danger, true);
  assert.equal(item(menu({ ...host, presence: "offline" }), "terminal-m-1")?.blocked, "the device is offline — this needs it to be online");
  assert.equal(item(menu({ ...host, presence: "offline" }), "rename-m-1")?.blocked, "the device is offline — this needs it to be online", "renamed over its channel: shown, and why not while it is offline");
  // Re-sending its keys says what it does.
  assert.equal(item(items, "resend-m-1")?.hint, "send this device every key again (if it missed new ones)");
  // This device's own row: nothing to remove or re-send, so no menu at all.
  assert.deepEqual(menu(row({ self: true })), []);
  assert.doesNotMatch(list(<DeviceRow view={row({ self: true })} viewerAdmin upgraded writable onRemove={() => undefined} onResend={() => undefined} />), /data-testid="actions-/);
  // A limited device looking: a read-only list — only a host's terminal.
  assert.deepEqual(menu(host, { viewerAdmin: false }).map((i) => i.id), ["terminal-m-1"]);
  // Something else under way: listed, and does nothing.
  const seen: string[] = [];
  const busy = menu(row(), { busy: true, onRemove: () => void seen.push("remove") });
  item(busy, "remove-d-1")!.onSelect();
  assert.deepEqual(seen, []);
  // A held-back activation: every change to the chain waits for it, and each says so; renaming (over the channel) and mail do not wait.
  const hold = "approve or remove the device whose activation is held back first";
  const heldHost = menu(host, { held: true });
  for (const id of ["control-m-1", "host-m-1", "remove-m-1"]) assert.equal(item(heldHost, id)?.blocked, hold, id);
  for (const id of ["rename-m-1", "resend-m-1", "terminal-m-1"]) assert.equal(item(heldHost, id)?.blocked, null, id);
  assert.equal(item(menu(row({ id: "d-2", limited: { block: null } }), { held: true }), "promote-d-2")?.blocked, hold);
  // Mailing its keys again: one at a time.
  assert.equal(item(menu(row(), { resending: true }), "resend-d-1")?.busy, true);
});

test("a full and a limited device say what they may do in the details too — a finger does not hover; one waiting for approval goes by its id", () => {
  const full = list(<DeviceRow view={row({ full: true })} viewerAdmin upgraded writable />);
  assert.match(full, /data-testid="role-d-1"[^>]*>A full device holds the account&#x27;s keys/);
  assert.match(full, /data-testid="full-d-1"[^>]*title="A full device holds/);
  const limited = list(<DeviceRow view={row({ id: "d-2", limited: { block: null } })} viewerAdmin upgraded writable />);
  assert.match(limited, /data-testid="role-d-2"[^>]*>It can control the devices that let it/);
  assert.ok(list(<DeviceRow view={row({ full: true })} viewerAdmin upgraded writable />, "zh-CN").includes("完整设备持有账户的钥匙"));
  assert.doesNotMatch(list(<DeviceRow view={row({ id: "m-1", kind: "computer", host: true })} viewerAdmin upgraded writable />), /data-testid="role-/);
  // Waiting for approval: the id, whatever name the view carries.
  const pending = list(<DeviceRow view={row({ id: "d-ipad", status: "pending", name: "iPhone · Safari" })} viewerAdmin upgraded writable />);
  assert.match(pending, /data-testid="name-d-ipad"[^>]*>d-ipad</);
  assert.ok(!pending.includes("iPhone · Safari"));
});

test("badges say what a device is: this device, can run AI, the main device, a full or a limited device; a re-enroll warning is a chip on the name", () => {
  const self = list(<DeviceRow view={row({ self: true, full: true, name: "Mac · Safari", kind: "browser", presence: "online" })} viewerAdmin upgraded writable />);
  assert.ok(self.includes(">this device<") && self.includes(">full device<") && self.includes(">Mac · Safari<"), self);
  const zh = list(<DeviceRow view={row({ self: true, full: true, name: "Mac · Safari", kind: "browser", presence: "online" })} viewerAdmin upgraded writable />, "zh-CN");
  assert.ok(zh.includes(">本设备<") && zh.includes(">完整设备<"), zh);
  const main = list(<DeviceRow view={row({ id: "m-1", kind: "computer", host: true, main: true, presence: "online" })} viewerAdmin upgraded writable />);
  assert.ok(main.includes(">main device<") && main.includes(">can run AI<"));
  assert.ok(!main.includes(">full device<"), "a device that runs AI is never a full device (review D1)");
  // No name known: a short id stands in, the full id in the details.
  const unnamed = list(<DeviceRow view={row({ id: "d-aaaabbbbccccdddd" })} viewerAdmin upgraded writable />);
  assert.match(unnamed, /data-testid="name-d-aaaabbbbccccdddd"[^>]*>device aaaa…</);
  assert.ok(unnamed.includes("d-aaaabbbbccccdddd"));
  // Pending and removed say so where online would.
  assert.ok(list(<DeviceRow view={row({ status: "pending" })} viewerAdmin upgraded writable />).includes(">waiting for approval<"));
  const removed = list(<DeviceRow view={row({ status: "removed" })} viewerAdmin upgraded writable onRemove={() => undefined} />);
  assert.ok(removed.includes(">removed<") && !removed.includes("details-d-1") && !removed.includes("actions-d-1"), "nothing to do with a removed device");
  // A re-enroll warning: a chip on the name, and the details open with the evidence.
  const unknown: ReEnrollLine = { device: "d-old", lostSeq: 5, evidence: { kind: "unknown" }, dismissed: false };
  const warned = list(<DeviceRow view={row({ id: "m-1", kind: "computer", host: true, reEnroll: [unknown] })} viewerAdmin upgraded writable />);
  assert.match(warned, /data-signal="warn"[^>]*>re-enroll it</);
  assert.match(warned, /<details data-testid="details-m-1" open=""/);
  const dismissed = list(<DeviceRow view={row({ id: "m-1", kind: "computer", host: true, reEnroll: [{ ...unknown, dismissed: true }] })} viewerAdmin upgraded writable />);
  assert.doesNotMatch(dismissed, /<details data-testid="details-m-1" open=""/, "only dismissed ones: folded, quietly listed inside");
  assert.ok(dismissed.includes("re-enroll-dismissed-m-1"));
});

test("no 「可管理账号」 switch on any row; a limited device says so, and a full device may make it one — or is told to upgrade the account first", () => {
  // A full device's row: no switch, no chip, no promote — on any row, whatever it is.
  for (const view of [row(), row({ self: true }), row({ id: "m-1", kind: "computer", host: true, presence: "online" }), row({ id: "m-2", kind: "computer", stoppedHost: true })]) {
    const html = list(<DeviceRow view={view} viewerAdmin upgraded writable onPromote={() => undefined} />);
    assert.doesNotMatch(html, /role="switch"|data-testid="admin-|can manage the account/, html);
    assert.doesNotMatch(html, /data-testid="limited-/);
    assert.equal(item(menu(view), `promote-${view.id}`), undefined);
  }
  // A limited device, to a full device: the chip in its name line, and the action in its menu.
  const limited = list(<DeviceRow view={row({ id: "d-2", limited: { block: null } })} viewerAdmin upgraded writable onPromote={() => undefined} />);
  assert.match(limited, /data-testid="device-d-2"[^>]*data-limited=""/);
  assert.match(limited, /data-testid="limited-d-2"[^>]*>limited device</);
  const promote = item(menu(row({ id: "d-2", limited: { block: null } })), "promote-d-2");
  assert.equal(promote?.label, "make it a full device");
  assert.equal(promote?.blocked, null);
  // On chain v2 it cannot be one yet: shown, not pressable, and why.
  assert.equal(item(menu(row({ id: "d-2", limited: { block: "notUpgraded" } }), { upgraded: false }), "promote-d-2")?.blocked, "upgrade the account first");
  // To a limited device looking (a read-only list), and on its own row: the chip only.
  const readOnly = list(<DeviceRow view={row({ id: "d-2", limited: { block: "notAdmin" } })} viewerAdmin={false} upgraded writable onRemove={() => undefined} onPromote={() => undefined} />);
  assert.match(readOnly, /data-testid="limited-d-2"/);
  assert.deepEqual(menu(row({ id: "d-2", limited: { block: "notAdmin" } }), { viewerAdmin: false }), [], "no promote, remove or re-send");
  const own = list(<DeviceRow view={row({ id: "d-2", self: true, limited: { block: "self" } })} viewerAdmin upgraded writable onPromote={() => undefined} />);
  assert.match(own, /data-testid="limited-d-2"/);
  assert.equal(item(menu(row({ id: "d-2", self: true, limited: { block: "self" } })), "promote-d-2"), undefined, "another device makes it one: the note above the list says so");
  // In words.
  const zh = list(<DeviceRow view={row({ id: "d-2", limited: { block: null } })} viewerAdmin upgraded writable onPromote={() => undefined} />, "zh-CN");
  assert.ok(zh.includes(">受限设备<"), zh);
  assert.equal(item(menu(row({ id: "d-2", limited: { block: null } }), {}, "zh-CN"), "promote-d-2")?.label, "设为完整设备");
  assert.equal(item(menu(row({ id: "d-2", limited: { block: null } }), {}, "ja"), "promote-d-2")?.label, "フルデバイスにする");
  // Until the account is upgraded, hosting and who may control it are shown but do not move (review D4).
  const v2Host = menu(row({ id: "m-1", kind: "computer", host: true, presence: "online" }), { upgraded: false });
  assert.equal(item(v2Host, "host-m-1")?.blocked, "upgrade the account first");
  assert.equal(item(v2Host, "control-m-1")?.blocked, "upgrade the account first");
  // Upgraded, while the hub takes no v3 entry: stopping AI moves, running it again waits (the hub gates it).
  assert.equal(item(menu(row({ id: "m-1", kind: "computer", host: true, presence: "online" }), { writable: false }), "host-m-1")?.blocked, null);
  assert.equal(
    item(menu(row({ id: "m-2", kind: "computer", stoppedHost: true }), { writable: false }), "host-m-2")?.blocked,
    "not available until Droppen is updated on every device of the account that runs AI",
  );
});

test("the devices page for a limited device, and while the hub takes no v3 entry", () => {
  const note = draw(<LimitedDeviceNote missing={["m-1"]} />);
  assert.match(note, /data-testid="limited-device-note"/);
  assert.ok(note.includes("this browser is a limited device"), note);
  assert.ok(note.includes("To add or remove devices here, make it a full device from another of your devices."));
  assert.ok(note.includes("waiting for the keys of m-1 from a full device"));
  assert.doesNotMatch(note, /manage the account/);
  assert.ok(draw(<LimitedDeviceNote missing={[]} />, "zh-CN").includes("这个浏览器是受限设备"));
  assert.ok(draw(<LimitedDeviceNote missing={[]} />, "ja").includes("制限付きデバイス"));
  // On chain v2 another device has to upgrade the account first: said so.
  const v2 = draw(<LimitedDeviceNote missing={[]} upgraded={false} />);
  assert.ok(v2.includes("upgrade the account to chain v3 from another of your devices, then make this browser a full device there"), v2);
  assert.ok(draw(<LimitedDeviceNote missing={[]} upgraded={false} />, "zh-CN").includes("先在你的另一台设备上把账户升级到链 v3"));
  const v3 = draw(<V3Note waiting={["m-old", "m-older"]} />);
  assert.ok(v3.includes("update Droppen on m-old, m-older first"), v3);
  assert.ok(draw(<V3Note waiting={[]} />).includes("once it is updated on every device of the account that runs AI"));
});

test("D4: upgrading to chain v3 is its own step — offered only while the hub takes it, confirmed with what it means and what it writes", () => {
  const plan = { upgradePlan: () => ({ version: 3 as const, ukEpoch: 4, machines: 0 as const }), upgradeToV3: async () => { throw new Error("not in a render"); } };
  const waiting = draw(<UpgradeAccount account={plan} writable={false} waiting={["m-old"]} />);
  assert.match(waiting, /data-testid="upgrade-account"/);
  assert.ok(waiting.includes("this account is still on chain v2") && waiting.includes("update Droppen on m-old first"));
  assert.doesNotMatch(waiting, /upgrade-start/, "not offered while the hub takes no v3 entry");
  const open = draw(<UpgradeAccount account={plan} writable waiting={[]} />);
  assert.match(open, /data-testid="upgrade-start"/);
  assert.doesNotMatch(open, /upgrade-confirm/, "nothing is written on the first click");
  assert.equal(draw(<UpgradeAccount account={{ ...plan, upgradePlan: () => null }} writable waiting={[]} />), "", "an upgraded account: nothing to offer");
  const confirm = draw(<UpgradeConfirm ukEpoch={4} busy={false} error={null} onConfirm={() => undefined} onCancel={() => undefined} />);
  assert.match(confirm, /role="alertdialog"[^>]*data-testid="upgrade-confirm"/);
  assert.ok(confirm.includes("cannot follow the account afterwards") && confirm.includes("This cannot be undone."));
  assert.ok(confirm.includes("one entry, a new user key (epoch 4), as chain v3. Every full device of the account gets the new key"), "what is written");
  assert.doesNotMatch(open + confirm, /manage the account/);
  assert.ok(draw(<UpgradeConfirm ukEpoch={4} busy={false} error={null} onConfirm={() => undefined} onCancel={() => undefined} />, "zh-CN").includes("此操作无法撤销"));
});

test("making a limited device a full device asks first — it gets the account's keys, and need not be online", () => {
  const html = draw(<PromoteDevice device="d-2" busy={false} onConfirm={() => undefined} onCancel={() => undefined} />);
  assert.match(html, /role="alertdialog"[^>]*data-testid="promote-device"/);
  assert.ok(html.includes("make d-2 a full device?"), html);
  assert.ok(html.includes("It gets the account&#x27;s keys and can add or remove devices, like this one."), html);
  assert.ok(html.includes("It need not be online: its keys wait for it at the hub."));
  assert.match(html, /data-testid="promote-device-confirm"[^>]*>make it a full device</);
  assert.doesNotMatch(html, /manage the account/);
  assert.ok(draw(<PromoteDevice device="d-2" busy={false} onConfirm={() => undefined} onCancel={() => undefined} />, "zh-CN").includes("要把 d-2 设为完整设备吗"));
  assert.ok(draw(<PromoteDevice device="d-2" busy={false} onConfirm={() => undefined} onCancel={() => undefined} />, "ja").includes("d-2 をフルデバイスにしますか"));
});

test("a device made a full device whose keys were not mailed says so under its row, with “re-send keys” right there", async () => {
  if (!i18next.isInitialized) await i18next.init({ lng: "en", resources: {}, keySeparator: false, nsSeparator: false, interpolation: { escapeValue: false } });
  // The confirmation writes through `promote`; when mailing its keys failed, the row hears why, in words.
  const heard: string[] = [];
  const sections: unknown[] = [];
  const failing = { promote: async () => ({ keysFailed: new HubApiError(507, "mailbox_full", "the recipient's mailbox is full") }), setHost: async () => null };
  let work: Promise<unknown> | null = null;
  const acts = rowActions({
    account: failing as never,
    id: "d-2",
    writable: true,
    courier: () => ({}) as MachineKeyCourier,
    run: (w) => (work = w()),
    setSection: (s) => void sections.push(s),
    onKeysNotSent: (why) => void heard.push(why),
  });
  acts.grant();
  await work;
  assert.deepEqual(sections, [{ kind: "list" }], "it is a full device all the same: back to the list");
  assert.equal(heard.length, 1);
  assert.match(heard[0]!, /mailbox at the hub is full/, heard[0]);
  // Sent: nothing to say.
  const fine = rowActions({ account: { promote: async () => ({ keysFailed: null }), setHost: async () => null } as never, id: "d-2", writable: true, courier: () => ({}) as MachineKeyCourier, run: (w) => (work = w()), setSection: () => undefined, onKeysNotSent: (why) => void heard.push(why) });
  fine.grant();
  await work;
  assert.equal(heard.length, 1);
  // What the row then shows.
  const html = draw(<KeysNotSent device="d-2" why={heard[0]!} resend={<button type="button">re-send keys</button>} onClose={() => undefined} />);
  assert.match(html, /role="alert"[^>]*data-testid="keys-not-sent-d-2"/);
  assert.ok(html.includes("d-2 is a full device now, but its keys were not sent"), html);
  assert.ok(html.includes(">re-send keys<"));
  assert.ok(draw(<KeysNotSent device="d-2" why="x" resend={null} onClose={() => undefined} />, "zh-CN").includes("d-2 已是完整设备，但它的钥匙没有发出去"));
});

test("who can control this device: three choices, a picker, and the warning about other computers that run AI", () => {
  const html = draw(
    <HostControl
      host="m-1"
      policy={{ type: "devices", ids: ["m-2"] }}
      candidates={[
        { id: "d-plain", host: false, limited: true },
        { id: "m-2", host: true },
      ]}
      upgraded
      busy={false}
      onSave={() => undefined}
      onCancel={() => undefined}
    />,
  );
  assert.match(html, /data-testid="host-control-m-1"/);
  for (const label of ["only your full devices", "every device of the account", "your full devices, and the devices picked here"]) assert.ok(html.includes(label), label);
  assert.ok(html.includes("· limited device"), "a limited device picked is named so: it is not among the full devices");
  assert.doesNotMatch(html, /manage the account/);
  assert.ok(html.includes(">d-plain<") || html.includes("d-plain"));
  assert.match(html, /data-testid="control-warning"[^>]*data-signal="warn"/, "a computer that runs AI is picked: the warning stands out");
  assert.ok(html.includes("every AI CLI on it — can drive this one"));
  const zh = draw(<HostControl host="m-1" policy={{ type: "admins" }} candidates={[]} upgraded={false} busy={false} onSave={() => undefined} onCancel={() => undefined} />, "zh-CN");
  assert.ok(zh.includes("谁可以控制这台设备") && zh.includes("只有你的完整设备") && zh.includes("所有设备"), zh);
  assert.ok(!zh.includes("可管理账号"));
  assert.match(zh, /type="radio"[^>]*disabled=""/, "not upgraded: shown, not movable");
  assert.ok(zh.includes("请先升级账号"));
});

test("adding a browser or phone always makes it a full device — no choice, on chain v2 and v3 alike", async () => {
  const candidate = { device: "d-new", fingerprint: "ABCD-EFGH-IJKL-MNOP" } as unknown as Parameters<typeof ConfirmCandidate>[0]["candidate"];
  for (const isV3 of [true, false]) {
    const paired: unknown[] = [];
    const added: string[] = [];
    const account = { chain: { isV3, deviceRecord: () => undefined }, pairDevice: async (c: unknown, opts: unknown) => void paired.push([c, opts]) };
    const props = { account: account as never, me: null, candidate, onAdded: () => void added.push("added"), onCancel: () => undefined };
    const { html, tree } = drive(ConfirmCandidate, props, []);
    assert.ok(html.includes("add d-new to this account?"), html);
    assert.doesNotMatch(html, /type="checkbox"|manage-choice|can manage the account/, "nothing to choose");
    const add = tree.find((e) => e.type === "button" && e.props["data-variant"] === "primary");
    assert.ok(add, "the fingerprints match: add it");
    await (add.props.onClick as () => Promise<void>)();
    assert.deepEqual(paired, [[candidate, { admin: true }]], `chain v${isV3 ? 3 : 2}: a full device`);
    assert.deepEqual(added, ["added"]);
  }
});

test("while this device holds an activation back, the pairing says so and cannot be confirmed", () => {
  const candidate = { device: "d-again", fingerprint: "ABCD-EFGH-IJKL-MNOP" } as unknown as Parameters<typeof ConfirmCandidate>[0]["candidate"];
  const account = { chain: { isV3: true, deviceRecord: () => ({ status: { type: "active" }, viaPasskey: null }) }, heldBack: { device: "d-z", readyMs: 0, entries: [] }, pairDevice: async () => "redelivered" };
  const props = { account: account as never, me: null, candidate, onAdded: () => undefined, onCancel: () => undefined };
  const { html, tree } = drive(ConfirmCandidate, props, []);
  assert.match(html, /data-testid="pairing-held"/);
  assert.ok(html.includes("approve or remove the device whose activation is held back first"), html);
  const add = tree.find((e) => e.type === "button" && e.props["data-variant"] === "primary");
  assert.equal(add?.props.disabled, true);
});

test("chain v3's alerts in words: a full device added is just added, a limited one is named so; a device made a full device is loud, the rest are notes", () => {
  const t = (key: string, opts?: Record<string, unknown>) => key.replace(/\{\{(\w+)\}\}/g, (_, k: string) => String(opts?.[k]));
  const base = { seq: 1, by: "d-1", atMs: 0, remove: null };
  assert.equal(alertText({ ...base, kind: "addDevice", subject: "m-2", flags: { host: true, admin: false } }, t), "d-1 added m-2 to this account: it can run AI");
  assert.equal(alertText({ ...base, kind: "addDevice", subject: "d-2", flags: { host: false, admin: true } }, t), "d-1 added d-2 to this account", "a full device: nothing more to say");
  assert.equal(alertText({ ...base, kind: "addDevice", subject: "d-3", flags: { host: false, admin: false } }, t), "d-1 added d-3 to this account as a limited device");
  assert.equal(alertText({ ...base, kind: "addDevice", subject: "d-4" }, t), "d-1 added d-4 to this account", "chain v2: as before");
  assert.equal(alertText({ ...base, kind: "setAdmin", subject: "d-2", on: true }, t), "d-1 made d-2 a full device");
  assert.equal(alertText({ ...base, kind: "setAdmin", subject: "d-2", on: false }, t), "d-1 made d-2 a limited device");
  assert.equal(alertText({ ...base, kind: "setHost", subject: "m-2", on: false }, t), "m-2 no longer runs AI (d-1 stopped it)");
  assert.equal(alertText({ ...base, kind: "setAccept", subject: "m-1", accept: { type: "devices", ids: ["m-2"] } }, t), "d-1 let m-2 control m-1");
  assert.equal(alertText({ ...base, kind: "setAccept", subject: "m-1", accept: { type: "admins" } }, t), "d-1 set m-1 to take control only from your full devices");
  assert.equal(alertText({ ...base, kind: "passkey", passkey: "join", subject: "d-5" }, t), "a passkey added d-5 to this account (it waits to be approved)");
  assert.equal(alertLevel({ ...base, kind: "setAdmin", subject: "d-2", on: true }), "high");
  assert.equal(alertLevel({ ...base, kind: "setAdmin", subject: "d-2", on: false }), "info");
  assert.equal(alertLevel({ ...base, kind: "addDevice", subject: "m-2", flags: { host: true, admin: false } }), "confirm");
  const alerts = [
    { ...base, seq: 8, kind: "setAdmin", subject: "d-2", on: true, remove: "d-2" },
    { ...base, seq: 9, kind: "setHost", subject: "m-3", on: false },
  ];
  const html = draw(<ChainAlerts account={{ alerts: () => alerts, onChange: () => () => undefined } as unknown as CloudAccount} link={{} as HubLink} />);
  assert.match(html, /data-level="high"[^>]*>important</);
  assert.match(html, /data-level="info"[^>]*>note</);
  assert.ok(html.includes(">remove d-2<"));
  assert.doesNotMatch(html, /manage the account/);
  const ja = draw(<ChainAlerts account={{ alerts: () => alerts, onChange: () => () => undefined } as unknown as CloudAccount} link={{} as HubLink} />, "ja");
  assert.ok(ja.includes("d-1 が d-2 をフルデバイスにしました"), ja);
  assert.ok(draw(<ChainAlerts account={{ alerts: () => alerts, onChange: () => () => undefined } as unknown as CloudAccount} link={{} as HubLink} />, "zh-CN").includes("d-1 把 d-2 设为了完整设备"));
});

test("a host renewing its identity key is a note, with the new fingerprint once it is worked out (host identity rotation §3.4)", () => {
  const t = (key: string, opts?: Record<string, unknown>) => key.replace(/\{\{(\w+)\}\}/g, (_, k: string) => String(opts?.[k]));
  const renewal = { seq: 12, by: "d-1", atMs: 0, remove: null, kind: "rotateHostKey", subject: "m-2", renewed: { signKey: new Uint8Array(32), dhKey: new Uint8Array(32), onDevice: false } } as const;
  assert.equal(alertLevel(renewal), "info");
  assert.equal(alertText(renewal, t, "ABCD-EFGH-IJKL-MNOP"), "m-2 renewed its identity key · new fingerprint ABCD-EFGH-IJKL-MNOP");
  assert.equal(alertText(renewal, t), "m-2 renewed its identity key", "before the hash is done");
  const account = { alerts: () => [renewal], onChange: () => () => undefined } as unknown as CloudAccount;
  const html = draw(<ChainAlerts account={account} link={{} as HubLink} />);
  assert.match(html, /data-level="info"[^>]*>note</);
  assert.ok(html.includes("m-2 renewed its identity key"), html);
  assert.ok(draw(<ChainAlerts account={account} link={{} as HubLink} />, "zh-CN").includes("m-2 更换了身份密钥"));
  // (This file draws zh-CN and ja; zh-TW is the table's word.)
  assert.equal(cloudDevicesZhTW["{{device}} renewed its identity key · new fingerprint {{fingerprint}}"], "{{device}} 更換了身分金鑰 · 新指紋 {{fingerprint}}");
  assert.ok(draw(<ChainAlerts account={account} link={{} as HubLink} />, "ja").includes("m-2 が ID 鍵を更新しました"));
  // A renewal confirmed on the device says so, as the agent's notice does.
  const assured = { ...renewal, renewed: { ...renewal.renewed, onDevice: true } };
  assert.equal(alertLevel(assured), "info");
  assert.equal(alertText(assured, t, "ABCD-EFGH-IJKL-MNOP"), "m-2 renewed its identity key, confirmed on the device · new fingerprint ABCD-EFGH-IJKL-MNOP");
  assert.equal(alertText(assured, t), "m-2 renewed its identity key, confirmed on the device");
  const assuredAccount = { alerts: () => [assured], onChange: () => () => undefined } as unknown as CloudAccount;
  assert.ok(draw(<ChainAlerts account={assuredAccount} link={{} as HubLink} />).includes("m-2 renewed its identity key, confirmed on the device"));
  assert.ok(draw(<ChainAlerts account={assuredAccount} link={{} as HubLink} />, "zh-CN").includes("m-2 更换了身份密钥（已在那台设备上确认）"));
  assert.ok(draw(<ChainAlerts account={assuredAccount} link={{} as HubLink} />, "ja").includes("m-2 が ID 鍵を更新しました（そのデバイスで確認済み）"));
  assert.equal(cloudDevicesZhTW["{{device}} renewed its identity key, confirmed on the device"], "{{device}} 更換了身分金鑰（已在那台裝置上確認）");
});

test("the account's renewal policy on the chain — turned on, a note; turned off, “is this you?” on every device", () => {
  const t = (key: string, opts?: Record<string, unknown>) => key.replace(/\{\{(\w+)\}\}/g, (_, k: string) => String(opts?.[k]));
  const policy = (on: boolean) => ({ seq: 14, by: "d-1", atMs: 0, remove: null, kind: "renewalPolicy", subject: null, on }) as const;
  assert.equal(alertLevel(policy(true)), "info");
  assert.equal(alertLevel(policy(false)), "confirm");
  assert.equal(alertText(policy(true), t), "identity key renewals must now be confirmed on the device (set by d-1)");
  assert.equal(alertText(policy(false), t), "identity key renewals may now be automatic (set by d-1)");
  const account = { alerts: () => [policy(true)], onChange: () => () => undefined } as unknown as CloudAccount;
  assert.ok(draw(<ChainAlerts account={account} link={{} as HubLink} />, "zh-CN").includes("更换身份密钥现在都要在那台设备上确认（由 d-1 设置）"));
  assert.ok(draw(<ChainAlerts account={account} link={{} as HubLink} />, "ja").includes("ID 鍵の更新は、今後そのデバイスでの確認が必要です（d-1 が設定）"));
  assert.equal(cloudDevicesZhTW["identity key renewals may now be automatic (set by {{by}})"], "更換身分金鑰現在可以自動進行（由 {{by}} 設定）");
});

test("control paused over hosts while identity keys are renewed is a note — paused, resumes, ended", () => {
  const t = (key: string, opts?: Record<string, unknown>) => key.replace(/\{\{(\w+)\}\}/g, (_, k: string) => String(opts?.[k]));
  const removals = [
    { host: "m-1", ids: ["m-2", "d-3"] },
    { host: "m-2", ids: ["m-1"] },
  ];
  const pause = (phase: "start" | "completed" | "cancelled") => ({ seq: 15, by: "d-1", atMs: 0, remove: null, kind: "renewalPause", subject: null, pause: { phase, removals } }) as const;
  assert.equal(alertLevel(pause("start")), "info");
  assert.equal(alertText(pause("start"), t), "control by m-2, d-3 over m-1; m-1 over m-2 is paused while identity keys are renewed (by d-1)");
  assert.equal(alertText(pause("completed"), t), "control by m-2, d-3 over m-1; m-1 over m-2 resumes (by d-1)");
  assert.equal(alertText(pause("cancelled"), t), "the pause of control by m-2, d-3 over m-1; m-1 over m-2 was ended (by d-1)");
  const account = { alerts: () => [pause("start")], onChange: () => () => undefined } as unknown as CloudAccount;
  assert.ok(draw(<ChainAlerts account={account} link={{} as HubLink} />, "zh-CN").includes("m-2, d-3 对 m-1; m-1 对 m-2 的控制已暂停，正在更换身份密钥（d-1）"));
  assert.ok(draw(<ChainAlerts account={account} link={{} as HubLink} />, "ja").includes("m-2, d-3 から m-1 への操作; m-1 から m-2 への操作を一時停止しました。ID 鍵を更新しています（d-1）"));
  assert.equal(cloudDevicesZhTW["control by {{edges}} resumes (by {{by}})"], "{{edges}} 的控制已恢復（{{by}}）");
  // A completion whose paused control the base has since dropped names none of it.
  const none = { ...pause("completed"), pause: { phase: "completed", removals: [] } } as const;
  assert.equal(alertText(none, t), "a renewal pause was completed; none of the control it paused comes back (by d-1)");
  const noneAccount = { alerts: () => [none], onChange: () => () => undefined } as unknown as CloudAccount;
  assert.ok(draw(<ChainAlerts account={noneAccount} link={{} as HubLink} />, "zh-CN").includes("一次控制暂停已完成，暂停的控制都没有恢复（d-1）"));
  assert.equal(
    cloudDevicesZhTW["a renewal pause was completed; none of the control it paused comes back (by {{by}})"],
    "一次控制暫停已完成，暫停的控制都沒有恢復（{{by}}）",
  );
});

test("14: stopping AI is asked first", () => {
  const stop = draw(<StopHost device="m-1" busy={false} onConfirm={() => undefined} onCancel={() => undefined} />);
  assert.match(stop, /role="alertdialog"[^>]*data-testid="stop-host"/);
  assert.ok(stop.includes("stop running AI on m-1?") && stop.includes("nobody can control it"));
  assert.ok(draw(<StopHost device="m-1" busy={false} onConfirm={() => undefined} onCancel={() => undefined} />, "ja").includes("m-1 での AI の実行を停止しますか"));
});

test("D3: control is full access, said where it is granted; letting a device in or shutting one out is confirmed first", () => {
  const html = draw(<HostControl host="m-1" policy={{ type: "admins" }} candidates={[{ id: "d-plain", host: false }]} upgraded busy={false} onSave={() => undefined} onCancel={() => undefined} />);
  assert.match(html, /data-testid="control-full-access"/);
  assert.ok(html.includes("its own identity key") && html.includes("a convenience, not a boundary"), html);
  const letIn = draw(<ControlConfirm host="m-1" admitted={["d-plain"]} shutOut={[]} busy={false} onConfirm={() => undefined} onCancel={() => undefined} />);
  assert.match(letIn, /role="alertdialog"[^>]*data-testid="control-confirm"/);
  assert.ok(letIn.includes("Let d-plain control m-1? Each gets full access to it, its identity key included."));
  assert.ok(letIn.includes(">allow full access<"));
  const shut = draw(<ControlConfirm host="m-1" admitted={[]} shutOut={["d-old"]} busy={false} onConfirm={() => undefined} onCancel={() => undefined} />);
  assert.ok(shut.includes("d-old can no longer control it: m-1 gets a new content key now — but a device that copied its identity key can still sign in as m-1 and receive that key."), shut);
  assert.ok(shut.includes("Re-enroll m-1 afterwards: remove it, then add it again."), "D3: the way out, said where the key is rotated");
  assert.ok(shut.includes(">save and rotate its key<"));
  assert.ok(draw(<ControlConfirm host="m-1" admitted={["d-plain"]} shutOut={[]} busy={false} onConfirm={() => undefined} onCancel={() => undefined} />, "zh-CN").includes("包括它的身份密钥"));
});

test("D3: a host to re-enroll says so on its row, with the way to a new identity — to a full device", () => {
  const unknown: ReEnrollLine = { device: "d-old", lostSeq: 5, evidence: { kind: "unknown" }, dismissed: false };
  const host = row({ id: "m-1", kind: "computer", host: true, presence: "online", reEnroll: [unknown] });
  const html = list(<DeviceRow view={host} viewerAdmin upgraded writable onReEnroll={() => undefined} />);
  assert.match(html, /data-testid="re-enroll-m-1"[^>]*data-signal="warn"/);
  // Said plainly (D3): a copied identity key still signs in as the host and is handed every new machine key; only re-enrolling helps.
  assert.ok(html.includes("re-enroll it: rotating its keys does not cut off a device that copied its identity"), html);
  assert.ok(html.includes("can still sign in as this one, and each new content key goes to that identity too. Only a new identity fixes this:"));
  assert.match(html, /data-testid="re-enroll-start-m-1"[^>]*>remove it, then add it again</, "and the action itself, right there");
  assert.ok(!list(<DeviceRow view={host} viewerAdmin={false} upgraded writable />).includes("re-enroll-m-1"), "not on a read-only list");
  assert.ok(!list(<DeviceRow view={{ ...host, reEnroll: [] }} viewerAdmin upgraded writable />).includes("re-enroll-m-1"));
  assert.ok(list(<DeviceRow view={host} viewerAdmin upgraded writable onReEnroll={() => undefined} />, "ja").includes("解決できるのは新しい ID だけです"));
});

test("D3: each device a host should be re-enrolled for, with what the host's own record says — and a person's way to say it never connected", () => {
  const at = new Date(2026, 9, 18, 14, 2).getTime();
  const lines: ReEnrollLine[] = [
    { device: "d-a", lostSeq: 5, evidence: { kind: "connected", firstMs: at }, dismissed: false },
    { device: "d-b", lostSeq: 6, evidence: { kind: "notSeen", sinceMs: at }, dismissed: false },
    { device: "d-c", lostSeq: 7, evidence: { kind: "unknown" }, dismissed: false },
    { device: "d-d", lostSeq: 8, evidence: { kind: "unknown" }, dismissed: true },
  ];
  const host = (reEnroll: ReEnrollLine[]) => row({ id: "m-1", kind: "computer", host: true, reEnroll });
  const drawRow = (reEnroll: ReEnrollLine[], lng = "en") =>
    list(<DeviceRow view={host(reEnroll)} viewerAdmin upgraded writable onReEnroll={() => undefined} onDismissReEnroll={() => undefined} onWarnAgain={() => undefined} />, lng);
  const html = drawRow(lines);
  assert.match(html, /data-testid="re-enroll-m-1"[^>]*data-signal="warn"/, "still the warning, whatever the record says");
  assert.match(html, /data-testid="re-enroll-m-1-d-a"[^>]*data-evidence="connected"/);
  assert.ok(html.includes(`d-a connected to it (first seen ${formatAsOf(at)}) — re-enrolling is strongly recommended`), html);
  assert.ok(html.includes(`no record of d-b connecting since ${formatAsOf(at)} — though a device that did connect could have erased itself from that record`));
  assert.ok(html.includes("d-c could control it; whether it ever connected is not known"));
  assert.ok(!html.includes("re-enroll-dismiss-m-1-d-a"), "no dismissing a device its record says connected");
  assert.match(html, /data-testid="re-enroll-dismiss-m-1-d-b"[^>]*>I&#x27;m sure it never connected — don&#x27;t warn about it</);
  assert.match(html, /data-testid="re-enroll-dismiss-m-1-d-c"/);
  // Dismissed: not in the warning, listed quietly with the way back.
  assert.ok(!html.includes('data-testid="re-enroll-m-1-d-d"'));
  assert.match(html, /data-testid="re-enroll-dismissed-m-1"/);
  assert.ok(html.includes("not warning about d-d: you said it never connected"));
  assert.match(html, /data-testid="re-enroll-warn-again-m-1-d-d"[^>]*>warn again</);
  // Every one dismissed: no warning, only the quiet list.
  const quiet = drawRow([lines[3]!]);
  assert.ok(!quiet.includes('data-testid="re-enroll-m-1"') && quiet.includes("re-enroll-dismissed-m-1"));
  assert.ok(drawRow(lines, "zh-CN").includes("我确定它从没连接过——不再提醒"));
  assert.ok(drawRow(lines, "ja").includes("登録し直すことを強くおすすめします"));
});

test("D3: dismissing says what it means first; confirmed, the warning about that device goes and the row lists it quietly", async () => {
  const html = draw(<DismissReEnroll host="m-1" device="d-old" onConfirm={() => undefined} onCancel={() => undefined} />);
  assert.match(html, /role="alertdialog"[^>]*data-testid="re-enroll-dismiss"/);
  assert.ok(html.includes("stop warning that d-old may have copied the identity of m-1?"), html);
  assert.ok(html.includes("Only if you are sure d-old never connected to m-1. If it did, it may still hold the identity key of m-1"));
  assert.ok(html.includes("If m-1 reports that d-old connected to it, the warning comes back."));
  assert.ok(html.includes("Remembered in this browser only: other devices still warn."));
  assert.match(html, /data-testid="re-enroll-dismiss-confirm"[^>]*>don&#x27;t warn about it</);
  assert.ok(draw(<DismissReEnroll host="m-1" device="d-old" onConfirm={() => undefined} onCancel={() => undefined} />, "zh-CN").includes("只记在这个浏览器里"));

  const { a, other, props } = await panelAccount();
  await a.removeDevice(other, new FakeCourier());
  assert.deepEqual([...a.reEnrollDue()], ["m-1"], "an admin removed: m-1 is due");
  let asked: El | undefined;
  const shown = a.reEnrollWarnings().get("m-1")!.find((l) => l.device === other)!.lostSeq;
  drive(DevicesPanel, props, [
    (tree) => (rowOf(tree, "m-1").props.onDismissReEnroll as (device: string, lostSeq: number) => void)(other, shown),
    (tree) => {
      asked = tree.find((e) => e.type === DismissReEnroll);
      assert.ok(asked, "asked first");
      (asked.props.onConfirm as () => void)();
    },
  ]);
  assert.deepEqual([asked?.props.host, asked?.props.device], ["m-1", other]);
  assert.deepEqual([...a.reEnrollDue()], [], "dismissed");
  const after = drive(DevicesPanel, props, []);
  const line = (rowOf(after.tree, "m-1").props.view as DeviceRowView).reEnroll?.find((l) => l.device === other);
  assert.equal(line?.dismissed, true, "still listed, dismissed");
  (rowOf(after.tree, "m-1").props.onWarnAgain as (device: string) => void)(other);
  assert.deepEqual([...a.reEnrollDue()], ["m-1"], "and warned about again");
});

test("a computer's refusals of a route that manages it, in words: limited_device, and stale — behind and fork are not staleness", async () => {
  await i18next.init({ lng: "en", resources: {}, keySeparator: false, nsSeparator: false, interpolation: { escapeValue: false } });
  assert.equal(hostRefusalText("limited_device"), "a limited device cannot do this: do it from a full device");
  assert.match(hostRefusalText("stale")!, /in the last 12 hours: open Droppen on one, then try again$/);
  assert.match(hostRefusalText("stale")!, /^no full device of yours has confirmed/);
  assert.equal(hostRefusalText("stale", { reason: "behind" }), "this device has seen more of the account than the device it reaches: sync, then try again");
  assert.equal(hostRefusalText("stale", { reason: "fork" }), "the other device's view of the account conflicts with this device's");
  assert.equal(hostRefusalText("nope"), null);
  for (const text of [hostRefusalText("limited_device"), hostRefusalText("stale"), hostRefusalText("stale", { reason: "behind" })]) assert.doesNotMatch(text!, /attest/);
  // The app's errors, and a rename's.
  assert.equal(apiErrorText(new ApiError({ error: "limited_device", message: "this device cannot manage the account: ask an admin device" })), hostRefusalText("limited_device"));
  assert.equal(apiErrorText(new ApiError({ error: "external_session", hint: "adopt it" })), "adopt it", "anything else as before");
  const t = ((key: string) => key) as never;
  assert.equal(renameErrorText(new RenameError("stale", hostRefusalText("stale", { reason: "fork" })!), t), "the other device's view of the account conflicts with this device's");
  assert.equal(renameErrorText(new RenameError("forbidden", "x"), t), "only a full device of the account can rename it");
  // A machine key in the channel: 412 goes by mailbox (unconfirmed), 403 limited_device is a refusal with its code.
  const courier = (status: number, body: unknown, mailed: number[]): MachineKeyCourier => ({
    waitOnline: async () => true,
    postInChannel: async () => ({ status, body }),
    mail: async (_m, epoch) => void mailed.push(epoch),
  });
  const mailed: number[] = [];
  assert.deepEqual(await deliverMachineKey(courier(412, { error: "stale" }, mailed), "m-1", 3, new Uint8Array(32), false), { via: "mailbox", reason: "unconfirmed" });
  assert.deepEqual(mailed, [3]);
  const refused = await deliverMachineKey(courier(403, { error: "limited_device", message: "this device cannot manage the account" }, mailed), "m-1", 4, new Uint8Array(32), false);
  assert.deepEqual(refused, { via: "refused", status: 403, message: "this device cannot manage the account", code: "limited_device" });
  assert.deepEqual(mailed, [3], "a refusal is not retried by mail");
});

// ------------------------------------------------------------ the re-review: which switch opens which confirmation

type El = React.ReactElement<Record<string, unknown>>;

/** Every element in a tree a component returned (not rendered further: a child component's own output is not in it). */
function elements(node: unknown, out: El[] = []): El[] {
  if (Array.isArray(node)) for (const n of node) elements(n, out);
  else if (node && typeof node === "object" && "props" in node) {
    out.push(node as El);
    elements((node as El).props.children, out);
  }
  return out;
}

/**
 * Render `Component` inline in a probe — its hooks are the probe's, its
 * returned tree ours to walk — and at render `i` run `steps[i]` on that tree
 * (a click, called as the page would). A click that moves state makes React
 * render again at once (a render-phase update), so the next step sees what
 * it opened; one that does not, ends the steps. Returns the markup, the last
 * tree, and how many steps ran.
 */
function drive<P>(Component: (props: P) => React.ReactNode, props: P, steps: ((tree: El[]) => void)[], lng = "en"): { html: string; tree: El[]; ran: number } {
  let i = 0;
  let tree: El[] = [];
  function Probe() {
    const out = Component(props);
    tree = elements(out);
    if (i < steps.length) steps[i++]!(tree);
    return out;
  }
  const html = draw(<Probe />, lng);
  return { html, tree, ran: i };
}

/** A real account at the fake hub for the devices page: this device and another full device, a limited device, a computer that runs AI — with its writes recorded, not made. */
async function panelAccount() {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  await a.upgradeToV3({ writable: true });
  const other = await hub.device("tok");
  const plain = await hub.device("tok");
  const keys = (x: CloudAccount) => ({ id: x.deviceId, signKey: x.keys.signPublic, dhKey: x.keys.dhPublic });
  await a.append(joinAction(keys(other), { host: false, admin: true }, a.chain!));
  await a.append(joinAction(keys(plain), { host: false, admin: false }, a.chain!));
  const calls: unknown[][] = [];
  const record =
    (name: string) =>
    async (...args: unknown[]) => {
      calls.push([name, ...args]);
      return name === "setAccept" ? { narrowed: false, admitted: [], shutOut: [] } : name === "promote" ? { keysFailed: null } : null;
    };
  Object.assign(a, { setAdmin: record("setAdmin"), promote: record("promote"), setHost: record("setHost"), setAccept: record("setAccept") });
  const link = new Proxy({ presence: new Map() } as Record<string, unknown>, { get: (t, k) => (k in t ? t[k as string] : () => () => undefined) }) as unknown as HubLink;
  return { a, other: other.deviceId, plain: plain.deviceId, link, calls, props: { account: a, link } };
}

/** Row `id` of the devices page's tree. */
const rowOf = (tree: El[], id: string): El => {
  const row = tree.find((e) => (e.props.view as DeviceRowView | undefined)?.id === id && "onPromote" in e.props);
  assert.ok(row, `a row for ${id}`);
  return row;
};

/** What the page's row element offers in its 「⋯」 menu, as the row works it out. */
const itemsOf = (row: El): RowMenuItem[] => rowMenuItems(row.props.view as DeviceRowView, { ...(row.props as Parameters<typeof rowMenuItems>[1]), t: i18n.getFixedT("en") });

/** Confirm whatever row `id` asks (its `RowConfirm`, then the dialog's button): returns the dialog. */
const confirmRow = (tree: El[], id: string): El => {
  const asks = tree.find((e) => e.type === RowConfirm && e.props.id === id);
  assert.ok(asks, `row ${id} asks for a confirmation`);
  const dialog = RowConfirm(asks.props as Parameters<typeof RowConfirm>[0]) as El | null;
  assert.ok(dialog, "it is one of this row's");
  (dialog.props.onConfirm as () => void)();
  return dialog;
};

test("on the devices page, a limited device's “make it a full device” and stopping AI each open their confirmation; only confirming writes", async () => {
  const { a, other, plain, calls, props } = await panelAccount();
  const me = a.deviceId;
  const promote = (id: string) => (tree: El[]) => (rowOf(tree, id).props.onPromote as () => void)();
  const flipHost = (id: string, on: boolean) => (tree: El[]) => (rowOf(tree, id).props.onHost as (on: boolean) => void)(on);

  // Who is a limited device, as the page draws it: only the browser that is not an admin.
  const page = drive(DevicesPanel, props, []);
  const views = new Map([me, other, plain, "m-1"].map((id) => [id, rowOf(page.tree, id).props.view as DeviceRowView]));
  assert.deepEqual(views.get(plain)!.limited, { block: null }, "a browser that is not a full device");
  for (const id of [me, other, "m-1"]) assert.equal(views.get(id)!.limited, null, `${id} is no limited device`);
  assert.match(page.html, new RegExp(`data-testid="limited-${plain}"`));
  assert.ok(item(itemsOf(rowOf(page.tree, plain)), `promote-${plain}`), "its menu offers it");
  assert.doesNotMatch(page.html, /data-testid="admin-|can manage the account/, "no 「可管理账号」 anywhere on the page");
  assert.deepEqual([...page.html.matchAll(/role="switch"[^>]*data-testid="([^"]+)"/g)].map((m) => m[1]), ["direct-switch"], "the one switch is this device's direct connections");
  for (const id of [me, other, "m-1"]) {
    assert.doesNotMatch(page.html, new RegExp(`data-testid="limited-${id}"`));
    assert.equal(item(itemsOf(rowOf(page.tree, id)), `promote-${id}`), undefined);
  }

  // Making it a full device: asked first, in its words.
  const asked = drive(DevicesPanel, props, [promote(plain)]);
  assert.match(asked.html, /role="alertdialog"[^>]*data-testid="promote-device"/);
  assert.ok(asked.html.includes(`make ${plain} a full device?`));
  assert.deepEqual(calls, [], "the action writes nothing");
  // Confirmed: setAdmin true — and nothing else (it need not be online: its keys wait in its mailbox).
  let dialog: El | null = null;
  drive(DevicesPanel, props, [promote(plain), (tree) => void (dialog = confirmRow(tree, plain))]);
  assert.equal(dialog!.type, PromoteDevice);
  assert.deepEqual(calls.splice(0), [["promote", plain]], "setAdmin true, its keys mailed and how that went (`promote`)");

  // Stopping AI on a computer: asked first; confirmed, it stops.
  const stop = drive(DevicesPanel, props, [flipHost("m-1", false)]);
  assert.match(stop.html, /role="alertdialog"[^>]*data-testid="stop-host"/);
  assert.ok(stop.html.includes("stop running AI on m-1?"));
  assert.deepEqual(calls, []);
  drive(DevicesPanel, props, [flipHost("m-1", false), (tree) => void (dialog = confirmRow(tree, "m-1"))]);
  assert.equal(dialog!.type, StopHost);
  assert.deepEqual(calls.splice(0).map((c) => c.slice(0, 3)), [["setHost", "m-1", false]]);

  // Running AI again is not asked (the hub gates it): it goes at once.
  drive(DevicesPanel, props, [flipHost("m-1", true)]);
  assert.deepEqual(calls.splice(0).map((c) => c.slice(0, 3)), [["setHost", "m-1", true]]);
});

test("on chain v2 a limited device is shown, and making it a full device waits for the upgrade", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  const plain = await hub.device("tok");
  // A v2 device that is not full, as an older chain may list one (nothing writes one today: `joinAction`).
  await a.append({ type: "addDevice", device: { id: plain.deviceId, signKey: plain.keys.signPublic, dhKey: plain.keys.dhPublic, full: false } });
  assert.equal(a.chain!.isV3, false);
  assert.equal(a.chain!.admin(plain.deviceId), undefined, "not a full device");
  const link = new Proxy({ presence: new Map() } as Record<string, unknown>, { get: (t, k) => (k in t ? t[k as string] : () => () => undefined) }) as unknown as HubLink;
  const { tree } = drive(DevicesPanel, { account: a, link }, []);
  assert.deepEqual((rowOf(tree, plain.deviceId).props.view as DeviceRowView).limited, { block: "notUpgraded" });
  assert.equal(item(itemsOf(rowOf(tree, plain.deviceId)), `promote-${plain.deviceId}`)?.blocked, "upgrade the account first");
});

test("14: the row's own controls — “make it a full device” and the AI control — say what they change, and a blocked one says nothing", () => {
  const seen: string[] = [];
  const press = (view: DeviceRowView, id: string, ctx: Partial<Parameters<typeof rowMenuItems>[1]> = {}) => {
    const found = item(menu(view, { onPromote: () => void seen.push("promote"), onHost: (on: boolean) => void seen.push(`host ${on}`), ...ctx }), id);
    assert.ok(found, id);
    found.onSelect();
  };
  press(row({ id: "d-2", limited: { block: null } }), "promote-d-2");
  press(row({ id: "d-3", limited: { block: "notUpgraded" } }), "promote-d-3", { upgraded: false });
  press(row({ id: "m-1", kind: "computer", host: true }), "host-m-1");
  press(row({ id: "m-2", kind: "computer", stoppedHost: true }), "host-m-2", { writable: false });
  assert.deepEqual(seen, ["promote", "host false"], "promote, running → stop; the blocked promote and the gated restart are still");
});

test("D3 (U4): who may control a computer — a list that lets a device in or shuts one out is confirmed before it is saved; the page saves it with a courier", async () => {
  const saved: unknown[] = [];
  const props = { host: "m-1", policy: { type: "admins" } as const, candidates: [{ id: "d-2", host: false }], upgraded: true, busy: false, onSave: (from: unknown) => void saved.push(from), onCancel: () => undefined };
  const radio = (n: number) => (tree: El[]) => (tree.filter((e) => e.type === "input" && e.props.type === "radio")[n]!.props.onChange as () => void)();
  const save = (tree: El[]) => (tree.find((e) => e.type === "button" && e.props["data-variant"] === "primary")!.props.onClick as () => void)();
  // Every device may control it now: d-2 is let in — asked first, with what that means.
  const letIn = drive(HostControl, props, [radio(1), save]);
  assert.equal(letIn.ran, 2);
  assert.match(letIn.html, /role="alertdialog"[^>]*data-testid="control-confirm"/);
  assert.ok(letIn.html.includes("Let d-2 control m-1? Each gets full access to it, its identity key included."));
  assert.deepEqual(saved, [], "saving asks first");
  const confirm = letIn.tree.find((e) => e.type === ControlConfirm)!;
  assert.deepEqual(confirm.props.admitted, ["d-2"]);
  (confirm.props.onConfirm as () => void)();
  assert.deepEqual(saved.splice(0), [{ type: "all" }], "confirmed: saved");
  // Shutting d-2 out of a list that named it: asked first too.
  const shut = drive(HostControl, { ...props, policy: { type: "all" } as const }, [radio(0), save]);
  assert.match(shut.html, /data-testid="control-confirm"/);
  assert.ok(shut.html.includes("d-2 can no longer control it"));
  assert.deepEqual(saved, []);
  // Nobody let in or out: saved at once.
  drive(HostControl, props, [save]);
  assert.deepEqual(saved.splice(0), [{ type: "admins" }]);
  assert.equal(controlSaveStep({ admitted: [], shutOut: [] }), "save");
  assert.equal(controlSaveStep({ admitted: ["d-2"], shutOut: [] }), "confirm");
  assert.equal(controlSaveStep({ admitted: [], shutOut: ["d-2"] }), "confirm");
  // The page saves what was confirmed, with a courier for a narrowing's new key.
  const { calls, props: page } = await panelAccount();
  drive(DevicesPanel, page, [
    (tree) => (rowOf(tree, "m-1").props.onControl as () => void)(),
    (tree) => (tree.find((e) => e.type === HostControl)!.props.onSave as (from: unknown) => void)({ type: "all" }),
  ]);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0]!.slice(0, 3), ["setAccept", "m-1", { type: "all" }]);
  assert.ok(calls[0]![3], "with a courier");
});

test("S1-4, the policy/1 gate: on a computer whose Droppen has no refusal list nobody but admins may be let in — narrowing still may", () => {
  const saved: unknown[] = [];
  const props = { host: "m-1", policy: { type: "all" } as const, declaresPolicy: false, candidates: [{ id: "m-local", host: true }], upgraded: true, busy: false, onSave: (from: unknown) => void saved.push(from), onCancel: () => undefined };
  const radio = (n: number) => (tree: El[]) => (tree.filter((e) => e.type === "input" && e.props.type === "radio")[n]!.props.onChange as () => void)();
  const primary = (tree: El[]) => tree.find((e) => e.type === "button" && e.props["data-variant"] === "primary")!;
  // Already open to all: nothing is let in by keeping it, so it says so quietly; letting more in is refused.
  const widen = drive(HostControl, { ...props, policy: { type: "admins" } as const }, [radio(1)]);
  assert.match(widen.html, /data-testid="control-outdated"[^>]*data-signal="warn"|data-signal="warn"[^>]*data-testid="control-outdated"/);
  assert.ok(widen.html.includes("Update Droppen on this device first"));
  assert.equal(primary(widen.tree).props.disabled, true, "saving a widening is refused");
  // Narrowing to admins only is still possible.
  const narrow = drive(HostControl, props, [radio(0)]);
  assert.equal(primary(narrow.tree).props.disabled, false);
  // Not known yet (`/v1/me` loading): letting others in waits, and says why (N1).
  const unknown = drive(HostControl, { ...props, policy: { type: "admins" } as const, declaresPolicy: undefined }, [radio(1)]);
  assert.ok(unknown.html.includes("Checking which Droppen this device runs"));
  assert.equal(primary(unknown.tree).props.disabled, true, "unknown is not yes");
  // A computer that declared it: no note.
  const fine = drive(HostControl, { ...props, policy: { type: "admins" } as const, declaresPolicy: true }, [radio(1)]);
  assert.doesNotMatch(fine.html, /control-outdated/);
  assert.equal(primary(fine.tree).props.disabled, false);
});

test("14: which confirmation a section asks of a row — and of no other row", () => {
  assert.deepEqual(hostSwitchSection("m-1", false), { kind: "stopHost", device: "m-1" });
  assert.equal(hostSwitchSection("m-1", true), null, "running AI again: at once");
  const actions = { grant: () => undefined, stopHost: () => undefined };
  const ask = (section: Parameters<typeof RowConfirm>[0]["section"], id = "d-1") => RowConfirm({ section, id, busy: false, actions, onCancel: () => undefined }) as El | null;
  assert.equal(ask({ kind: "grant", device: "d-1" })?.type, PromoteDevice);
  assert.equal(ask({ kind: "grant", device: "d-1" })?.props.onConfirm, actions.grant);
  assert.equal(ask({ kind: "stopHost", device: "d-1" })?.type, StopHost);
  assert.equal(ask({ kind: "stopHost", device: "d-1" })?.props.onConfirm, actions.stopHost);
  assert.equal(ask({ kind: "grant", device: "d-2" }), null, "another row's");
  assert.equal(ask({ kind: "list" }), null);
  assert.match(list(<RowConfirm section={{ kind: "grant", device: "d-1" }} id="d-1" busy={false} actions={actions} onCancel={() => undefined} />), /data-testid="promote-device"/);
});

test("D1 extended: a device that ran AI and stopped is no limited device — nothing offers to make it a full device", async () => {
  const html = list(<DeviceRow view={row({ id: "m-2", kind: "computer", stoppedHost: true })} viewerAdmin upgraded writable onPromote={() => undefined} />);
  assert.doesNotMatch(html, /limited-m-2|promote-m-2/);
  const { a, props } = await panelAccount();
  // Really stopped (the recorded setHost writes nothing): append it.
  await a.append({ type: "setHost", id: "m-1", host: false });
  const page = drive(DevicesPanel, props, []);
  const view = rowOf(page.tree, "m-1").props.view as DeviceRowView;
  assert.equal(view.stoppedHost, true);
  assert.equal(view.kind, "computer");
  assert.equal(view.limited, null);
  assert.doesNotMatch(page.html, /data-testid="(limited|promote)-m-1"/);
});

test("the upgrade banner says the hub is why while it reads chain v3 only — and the computers when they are", () => {
  const plan = { upgradePlan: () => ({ version: 3 as const, ukEpoch: 4, machines: 0 as const }), upgradeToV3: async () => { throw new Error("not in a render"); } };
  const me = (mode: string, waiting: string[] = [], writable = false) => ({ chainV3: chainV3View({ mode, writable, waiting, in_use: false }) });
  assert.equal(v3Blocker(me("read")), "hub");
  assert.equal(v3Blocker(me("off")), "hub");
  assert.equal(v3Blocker({ chainV3: null }), "hub", "a hub that predates chain v3");
  assert.equal(v3Blocker(me("write", ["m-old"])), "computers");
  assert.equal(v3Blocker(me("write", [], true)), null);
  assert.equal(v3Blocker(undefined), null, "not known yet: nothing blamed");
  // Read only: the hub, not the computers.
  const read = draw(<UpgradeAccount account={plan} writable={false} blocker={v3Blocker(me("read"))} waiting={[]} />);
  assert.ok(read.includes("the hub does not take chain v3 changes for this account yet — that is on Droppen&#x27;s side, not your devices&#x27;."), read);
  assert.ok(!read.includes("updated on every device of the account that runs AI") && !read.includes("update Droppen on"), "computers are not blamed");
  assert.doesNotMatch(read, /upgrade-start/);
  // Read only and a computer behind too: both said.
  const both = draw(<UpgradeAccount account={plan} writable={false} blocker="hub" waiting={["m-old"]} />);
  assert.ok(both.includes("the hub does not take chain v3 changes for this account yet, and Droppen on m-old needs an update too."));
  // The computers, when they are why.
  const computers = draw(<UpgradeAccount account={plan} writable={false} blocker={v3Blocker(me("write", ["m-old"]))} waiting={["m-old"]} />);
  assert.ok(computers.includes("update Droppen on m-old first") && !computers.includes("the hub does not"));
  assert.ok(draw(<UpgradeAccount account={plan} writable={false} blocker="computers" waiting={[]} />).includes("once it is updated on every device of the account that runs AI"));
  // Not known yet: no reason at all.
  assert.doesNotMatch(draw(<UpgradeAccount account={plan} writable={false} blocker={null} waiting={[]} />), /v3-pending/);
  // On an upgraded account the same (adding a computer, running AI again).
  assert.ok(draw(<V3Note blocker="hub" waiting={[]} />).includes("wait until the hub takes chain v3 changes for this account"));
  assert.ok(draw(<V3Note blocker="computers" waiting={[]} />).includes("wait until Droppen is updated on every device of the account that runs AI"));
  // In words.
  assert.ok(draw(<V3Pending blocker="hub" waiting={[]} />, "zh-CN").includes("这是 Droppen 这边的原因"));
  assert.ok(draw(<V3Pending blocker="hub" waiting={[]} />, "ja").includes("原因は Droppen 側にあり"));
});

// ------------------------------------------------------------ before chain v3 `write`: the write gate

/** A page link that offers nothing (no presence, no messages). */
const quietLink = () => new Proxy({ presence: new Map() } as Record<string, unknown>, { get: (t, k) => (k in t ? t[k as string] : () => () => undefined) }) as unknown as HubLink;

test("on 「我的设备」, the upgrade's start button opens its confirmation — it upgrades nothing until that is confirmed", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  assert.equal(a.chain!.isV3, false, "a chain v2 account");
  const calls: unknown[][] = [];
  // Recorded, never made (and never settled: nothing is set after the render).
  Object.assign(a, { upgradeToV3: (...args: unknown[]) => (calls.push(args), new Promise(() => undefined)) });
  const link = quietLink();
  // The hub takes v3 entries for this account now (`/v1/me`'s `chain_v3`).
  meStore(a, link).view = { account: a.account, plan: null, chainSeq: a.chain!.seq, chainV3: chainV3View({ mode: "write", writable: true, waiting: [], in_use: false }), machines: [], devices: [] } as unknown as AccountView;
  // Above the list (`AccountNotices`).
  const page = drive(AccountNotices, { account: a, link }, []);
  const banner = page.tree.find((e) => e.type === UpgradeAccount);
  assert.ok(banner, "the page offers the upgrade");
  assert.equal(banner.props.account, a);
  assert.equal(banner.props.writable, true);
  assert.match(page.html, /data-testid="upgrade-start"/);
  assert.doesNotMatch(page.html, /upgrade-confirm/);
  const props = banner.props as Parameters<typeof UpgradeAccount>[0];
  const click = (testId: string) => (tree: El[]) => {
    const button = tree.find((e) => e.type === "button" && e.props["data-testid"] === testId);
    assert.ok(button, testId);
    (button.props.onClick as () => void)();
  };
  // Start: the confirmation, with what it writes — and nothing written.
  const started = drive(UpgradeAccount, props, [click("upgrade-start")]);
  assert.equal(started.ran, 1);
  assert.match(started.html, /role="alertdialog"[^>]*data-testid="upgrade-confirm"/);
  assert.ok(started.html.includes("one entry, a new user key (epoch 1), as chain v3"), "what it writes");
  assert.doesNotMatch(started.html, /data-testid="upgrade-start"/);
  assert.deepEqual(calls, [], "the start button upgrades nothing");
  assert.equal(a.chain!.isV3, false);
  // Cancel: back to the start button, still nothing written.
  const cancelled = drive(UpgradeAccount, props, [click("upgrade-start"), (tree) => (tree.find((e) => e.type === UpgradeConfirm)!.props.onCancel as () => void)()]);
  assert.match(cancelled.html, /data-testid="upgrade-start"/);
  assert.doesNotMatch(cancelled.html, /upgrade-confirm/);
  assert.deepEqual(calls, []);
  // Only confirming upgrades — as the hub said it may.
  drive(UpgradeAccount, props, [click("upgrade-start"), (tree) => (tree.find((e) => e.type === UpgradeConfirm)!.props.onConfirm as () => void)()]);
  assert.deepEqual(calls, [[{ writable: true }]]);
});

test("a new account's 「我的设备」 offers no upgrade — it is chain v3 from its genesis; an account from before chain v3 still is offered one", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await a.createGenesis(true);
  await hub.enroll(a, "m-1", { writable: true });
  assert.equal(a.chain!.isV3, true);
  const link = quietLink();
  const view = (account: CloudAccount, inUse: boolean) =>
    ({ account: account.account, plan: null, chainSeq: account.chain!.seq, chainV3: chainV3View({ mode: "write", writable: true, waiting: [], in_use: inUse }), machines: [], devices: [] }) as unknown as AccountView;
  meStore(a, link).view = view(a, true);
  const page = drive(AccountNotices, { account: a, link }, []);
  assert.equal(page.tree.find((e) => e.type === UpgradeAccount), undefined, "no upgrade on offer");
  assert.doesNotMatch(page.html, /data-testid="upgrade-/);
  // Nothing on the list waits for one either.
  const { tree } = drive(DevicesPanel, { account: a, link }, []);
  assert.ok(!itemsOf(rowOf(tree, "m-1")).some((i) => i.blocked === "upgrade the account first"));
  // An account made before chain v3, the hub taking v3 entries just the same: its one step is offered.
  const before = await new FakeKeysHub().device("tok");
  await createV2Genesis(before, true);
  meStore(before, link).view = view(before, false);
  assert.ok(drive(AccountNotices, { account: before, link }, []).tree.find((e) => e.type === UpgradeAccount));
});

test("write gate: a passkey device's self-activation this browser holds back can be vetoed from the devices page — its remove is enabled, every other removal waits", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  await hub.enroll(a, "m-1");
  await a.upgradeToV3({ writable: true });
  const other = await hub.device("tok");
  await a.append(joinAction({ id: other.deviceId, signKey: other.keys.signPublic, dhKey: other.keys.dhPublic }, { host: false, admin: true }, a.chain!));
  // p joins through a passkey slot; the hub (its clock a day ahead) takes p's self-activation at once.
  const slot = await addPasskeySlot(a);
  const p = await hub.device("tok");
  await passkeyJoin(p, slot);
  await passkeyActivateEarly(hub, p, slot);
  await a.sync();
  assert.equal(a.heldBack?.device, p.deviceId, "held back here, naming p");
  assert.equal(a.chain!.deviceRecord(p.deviceId)?.status.type, "pending");
  const removals: string[] = [];
  Object.assign(a, { removeDevice: async (id: string) => void removals.push(id) });
  const link = quietLink();
  // p says it is a familiar phone (it chose its own User-Agent): its row goes by its id all the same.
  meStore(a, link).view = {
    account: a.account,
    plan: null,
    chainSeq: a.chain!.seq,
    chainV3: null,
    machines: [],
    devices: [{ id: p.deviceId, createdMs: null, lastSeenMs: null, inChain: true, kind: null, uaLabel: "iPhone · Safari" }],
  } as unknown as AccountView;
  const props = { account: a, link };
  const page = drive(DevicesPanel, props, []);
  assert.equal((rowOf(page.tree, p.deviceId).props.view as DeviceRowView).name, null, "waiting for approval: no name of its own choosing");
  assert.match(page.html, new RegExp(`data-testid="name-${p.deviceId}"[^>]*>${p.deviceId}<`));
  // p's row: its remove is there and enabled (the veto); another device's waits for it.
  assert.equal(rowOf(page.tree, p.deviceId).props.held, false);
  const pRemove = item(itemsOf(rowOf(page.tree, p.deviceId)), `remove-${p.deviceId}`);
  assert.ok(pRemove, "p's row offers remove");
  assert.equal(pRemove.busy || pRemove.blocked !== null, false, "the held device can be removed");
  assert.equal(blockedByHold(a, p.deviceId), false);
  assert.equal(rowOf(page.tree, other.deviceId).props.held, true);
  assert.equal(item(itemsOf(rowOf(page.tree, other.deviceId)), `remove-${other.deviceId}`)?.blocked, "approve or remove the device whose activation is held back first", "removing anyone else waits for the veto or the approval");
  // Clicking it opens p's removal.
  const opened = drive(DevicesPanel, props, [(tree) => (rowOf(tree, p.deviceId).props.onRemove as () => void)()]);
  assert.match(opened.html, /data-testid="remove-device"/);
  assert.ok(opened.html.includes(`remove ${p.deviceId}?`));
  // The alert about p's join: its remove is enabled too, and removes p.
  const alerts = drive(ChainAlerts, { account: a, link }, []);
  assert.ok(alerts.html.includes(`remove ${p.deviceId}`), alerts.html);
  const button = alerts.tree.find((e) => e.props.variant === "danger");
  assert.ok(button, "the alert's remove");
  assert.equal(button.props.disabled, false, "the alert's remove is enabled");
  (button.props.onClick as () => void)();
  assert.deepEqual(removals, [p.deviceId]);
});

test("a browser or phone goes by what the hub made of its User-Agent — only those words; a device that runs AI by its own name", async () => {
  const { a, other, plain, link } = await panelAccount();
  // `/v1/me` as the hub lists the devices: one label it knows, one it should never send (an older hub sends none).
  meStore(a, link).view = {
    account: a.account,
    plan: null,
    chainSeq: a.chain!.seq,
    chainV3: null,
    machines: [],
    devices: [
      { id: other, createdMs: null, lastSeenMs: null, inChain: true, kind: null, uaLabel: "iPhone · Chrome" },
      { id: plain, createdMs: null, lastSeenMs: null, inChain: true, kind: null, uaLabel: "Click here to remove your Mac" },
    ],
  } as unknown as AccountView;
  const page = drive(DevicesPanel, { account: a, link }, []);
  const name = (id: string) => (rowOf(page.tree, id).props.view as DeviceRowView).name;
  assert.equal(name(other), "iPhone · Chrome");
  assert.equal(name(plain), null, "not a label's words: not shown");
  assert.match(page.html, new RegExp(`data-testid="name-${plain}"[^>]*>device ${shortDeviceId(plain)}<`), "a short id stands in");
  assert.equal(name("m-1"), null, "a device that runs AI: its own name, from its status (none known here)");
  // What each is: this full device; the limited one says so instead; a device that runs AI is never a full device.
  assert.equal((rowOf(page.tree, a.deviceId).props.view as DeviceRowView).full, true);
  assert.equal((rowOf(page.tree, plain).props.view as DeviceRowView).full, false);
  assert.equal((rowOf(page.tree, "m-1").props.view as DeviceRowView).full, false);
  // This device first, then the device that runs AI, then the other browsers and phones.
  const order = [...page.html.matchAll(/<li data-testid="device-([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(order.slice(0, 2), [a.deviceId, "m-1"], order.join(","));
});

test("the hierarchy of 「我的设备」: a summary line, 16px section titles, 56px rows and space instead of rules", async () => {
  const { a, props } = await panelAccount();
  const page = drive(DevicesPanel, props, []);
  const html = page.html;
  // Summary first: what the account has, not a generic security sentence.
  assert.match(html, /data-testid="devices-summary"[^>]*>\d+ devices/);
  assert.match(html, /data-testid="devices-title"[^>]*text-\[32px\]/);
  // Section titles are sans 16/24 semibold behind a numbered eyebrow; each has the one rule of its group.
  const heads = [...html.matchAll(/data-section-head=""[^>]*>[\s\S]*?<h2[^>]*class="([^"]*)"/g)].map((m) => m[1]!);
  assert.ok(heads.length >= 2, `this device and devices: ${heads.length}`);
  for (const cls of heads) assert.ok(cls.includes("text-base") && cls.includes("font-semibold"), cls);
  assert.ok((html.match(/border-b border-border/g) ?? []).length <= 6, "the groups' rules, not a wall of lines");
  // Rows are one 56px beat, separated by 10px of space, with no rule between them.
  assert.match(html, /<ul data-testid="device-list"[^>]*class="[^"]*gap-2\.5/);
  assert.equal((html.match(/\bh-14\b/g) ?? []).length, 4, "this device, the AI computer, the full device and the limited one");
  for (const row of [...html.matchAll(/<li data-testid="device-[\s\S]*?<\/li>/g)].map((m) => m[0])) {
    assert.ok(!/\bborder-b\b/.test(row), `no row rules: the gap separates them: ${row.slice(0, 300)}`);
  }
  // The current device is visible without colour: an ink edge and aria-current.
  const self = html.slice(html.indexOf(`data-testid="device-${a.deviceId}"`), html.indexOf(`data-testid="device-${a.deviceId}"`) + 600);
  assert.match(self, /data-current=""[^>]*aria-current="true"/);
  assert.match(self, /shadow-\[inset_1px_0_0_var\(--color-fg\)\]/);
  // Container queries reflow the rows rather than filling 2400px (the usage's own are in cloud-plan).
  assert.match(html, /@container\/devices/);
  assert.match(html, /@min-\[44rem\]\/devices:grid-cols/);
});

test("pending and removed devices keep the 56px beat as their own compact rows, not normal rows plus badges", () => {
  const pending = list(<DeviceRow view={row({ id: "d-pending", status: "pending" })} viewerAdmin upgraded writable onRemove={() => undefined} />);
  assert.match(pending, /data-status="pending"[\s\S]*?h-14/);
  assert.ok(pending.includes(">waiting for approval<"));
  assert.doesNotMatch(pending, /<details|>this device<|>full device<|>limited device<|>can run AI</);
  const removed = list(<DeviceRow view={row({ id: "d-gone", status: "removed" })} viewerAdmin upgraded writable onRemove={() => undefined} />);
  assert.match(removed, /data-status="removed"[\s\S]*?h-14/);
  assert.ok(removed.includes(">removed<"));
  assert.doesNotMatch(removed, /<details|data-testid="actions-/);
});

test("removed devices are folded at the bottom, out of the list; 「添加设备」 asks which kind first", async () => {
  const { a, other, props } = await panelAccount();
  await a.removeDevice(other, new FakeCourier());
  const page = drive(DevicesPanel, props, []);
  const listed = page.html.slice(page.html.indexOf('data-testid="device-list"'), page.html.indexOf('data-testid="removed-devices"'));
  assert.ok(!listed.includes(`device-${other}`), "not in the list");
  assert.match(page.html, /<details data-testid="removed-devices"/);
  assert.doesNotMatch(page.html, /<details data-testid="removed-devices"[^>]*open/, "folded");
  assert.ok(page.html.includes("removed devices (1)"));
  assert.ok(page.html.slice(page.html.indexOf('data-testid="removed-devices"')).includes(`data-testid="device-${other}"`));
  // One 「添加设备」, the view's one primary action; it asks a browser or phone, or a device that runs AI.
  assert.match(page.html, /data-variant="primary"[^>]*data-testid="add-device"/);
  const press = (testId: string) => (tree: El[]) => (tree.find((e) => e.props["data-testid"] === testId)!.props.onClick as () => void)();
  const pick = (kind: string) => (tree: El[]) => (tree.find((e) => e.type === AddChooser)!.props.onPick as (kind: string) => void)(kind);
  const asked = drive(DevicesPanel, props, [press("add-device")]);
  assert.match(asked.html, /data-testid="add-chooser"/);
  assert.doesNotMatch(asked.html, /data-testid="add-device"/, "while adding, no second way in");
  const html = draw(<AddChooser onPick={() => undefined} onCancel={() => undefined} />);
  assert.match(html, /data-testid="add-browser"/);
  assert.match(html, /data-testid="add-computer"/);
  assert.ok(html.includes("a browser or phone") && html.includes("a device that runs AI"));
  assert.ok(draw(<AddChooser onPick={() => undefined} onCancel={() => undefined} />, "zh-CN").includes("浏览器或手机"));
  assert.match(drive(DevicesPanel, props, [press("add-device"), pick("addBrowser")]).html, /data-testid="add-browser-section"/);
  assert.match(drive(DevicesPanel, props, [press("add-device"), pick("addComputer")]).html, /data-testid="add-computer-section"/);
});

test("just signed up, with no device that runs AI: the first-run card leads, 「添加设备」 steps back, and the advanced settings wait folded", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  const link = new Proxy({ presence: new Map() } as Record<string, unknown>, { get: (t, k) => (k in t ? t[k as string] : () => () => undefined) }) as unknown as HubLink;
  const props = { account: a, link };
  const page = drive(DevicesPanel, props, []);
  assert.match(page.html, /data-testid="first-host"/);
  assert.doesNotMatch(page.html, /Next: install Droppen/, "the one line it replaces");
  // One primary action a view: the card's; 「添加设备」 is there, as a secondary.
  assert.match(page.html, /data-variant="secondary" data-testid="add-device"/);
  assert.equal((page.html.match(/data-variant="primary"/g) ?? []).length, 1);
  assert.ok(page.html.indexOf('data-testid="first-host"') < page.html.indexOf('data-testid="device-list"'), "the card above the list");
  // This device's settings and the keys' rotation: folded under 「更多设置」, below the list — nothing above it.
  assert.match(page.html, /<details data-testid="more-settings"/);
  assert.doesNotMatch(page.html, /<details data-testid="more-settings"[^>]*open/, "folded");
  assert.ok(page.html.includes("more settings"));
  const folded = page.html.slice(page.html.indexOf('data-testid="more-settings"'));
  for (const id of ["this-device", "direct-connections", "rotate-keys"]) assert.ok(folded.includes(`data-testid="${id}"`), `${id} in 「更多设置」`);
  const above = page.html.slice(0, page.html.indexOf('data-testid="device-list"'));
  assert.doesNotMatch(above, /data-testid="(this-device|direct-connections|rotate-keys)"/);
  // The card's ways in: the full guide, or the guide on its one-time code — and while adding, the card steps aside.
  const cardOf = (tree: El[]) => tree.find((e) => e.type === FirstHostCard)!;
  const guide = drive(DevicesPanel, props, [(tree) => (cardOf(tree).props.onGuide as () => void)()]);
  assert.match(guide.html, /data-testid="add-computer-section"/);
  assert.doesNotMatch(guide.html, /data-testid="first-host"/);
  assert.equal(guide.tree.find((e) => e.type === AddComputer)?.props.initialMode, "guide");
  const code = drive(DevicesPanel, props, [(tree) => (cardOf(tree).props.onCode as () => void)()]);
  assert.equal(code.tree.find((e) => e.type === AddComputer)?.props.initialMode, "code");
  assert.equal(typeof cardOf(page.tree).props.pairWith, "function", "a sign-in window's browser new to the account is paired from the card");
  // Once a device runs AI: no card, nothing folded, this device's settings above the list, 「添加设备」 the primary again.
  await hub.enroll(a, "m-1");
  const after = drive(DevicesPanel, props, []);
  assert.doesNotMatch(after.html, /data-testid="first-host"|data-testid="more-settings"/);
  assert.match(after.html, /data-variant="primary"[^>]*data-testid="add-device"/);
  assert.ok(after.html.indexOf('data-testid="direct-connections"') < after.html.indexOf('data-testid="device-list"'));
  assert.match(after.html, /data-testid="rotate-keys"/);
});

test("the first-run card goes with a word that the device joined, as the guide's last step says it; and it waits for the chain and a full device", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  const link = new Proxy({ presence: new Map() } as Record<string, unknown>, { get: (t, k) => (k in t ? t[k as string] : () => () => undefined) }) as unknown as HubLink;
  // The chain as the page first read it (no device that runs AI), and once one joined.
  const before = a.chain!;
  await hub.enroll(a, "m-1");
  const after = a.chain!;
  assert.notEqual(before, after);
  const at = (chain: unknown) =>
    new Proxy(a, {
      get: (t, k) => {
        if (k === "chain") return chain;
        const v = Reflect.get(t, k, t);
        return typeof v === "function" ? v.bind(t) : v;
      },
    }) as CloudAccount;
  const [empty, joined] = [at(before), at(after)];
  // One page across the join: drawn with the card up, then again with the chain that has the new device (a render-phase update, as React makes it).
  function Page({ then }: { then?: (tree: El[]) => void }) {
    const [round, setRound] = React.useState(0);
    const out = DevicesPanel({ account: round === 0 ? empty : joined, link });
    const tree = elements(out);
    if (round === 0) {
      assert.ok(tree.some((e) => e.type === FirstHostCard), "the card is up first");
      setRound(1);
    } else if (round === 1 && then && tree.some((e) => e.type === NewDeviceReady)) {
      then(tree);
      setRound(2);
    }
    return out;
  }
  const html = draw(<Page />);
  assert.doesNotMatch(html, /data-testid="first-host"[^-]/, "the card has gone");
  assert.match(html, /data-testid="first-host-joined"/);
  assert.match(html, /data-testid="add-ready" data-device="m-1"/);
  assert.ok(html.includes("m-1 is in your account"), html.slice(html.indexOf("first-host-joined"), html.indexOf("first-host-joined") + 600));
  assert.match(html, /data-variant="primary" data-testid="open-new-device"/, "open it: the one primary");
  assert.equal((html.match(/data-variant="primary"/g) ?? []).length, 1);
  assert.match(html, /data-variant="secondary" data-testid="add-device"/);
  assert.doesNotMatch(html, /data-testid="more-settings"/, "nothing folded any more");
  // Put away ("done"): the page as it is with a device that runs AI.
  const done = draw(<Page then={(t) => (t.find((e) => e.type === NewDeviceReady)!.props.onDone as () => void)()} />);
  assert.doesNotMatch(done, /first-host-joined/);
  assert.match(done, /data-variant="primary"[^>]*data-testid="add-device"/);

  // Opened the guide instead: it says so itself (its own last step), so the page adds nothing when the device joins.
  function ViaGuide() {
    const [round, setRound] = React.useState(0);
    const out = DevicesPanel({ account: round < 2 ? empty : joined, link });
    const t = elements(out);
    if (round === 0) {
      (t.find((e) => e.type === FirstHostCard)!.props.onGuide as () => void)();
      setRound(1);
    } else if (round === 1) setRound(2);
    return out;
  }
  const guided = draw(<ViaGuide />);
  assert.match(guided, /data-testid="add-computer-section"/);
  assert.doesNotMatch(guided, /first-host-joined/);

  // Not before the chain is read, nor for a limited device: no card, nothing folded.
  const unread = new Proxy(a, { get: (t, k) => (k === "chain" ? null : k === "isAdmin" ? true : typeof Reflect.get(t, k, t) === "function" ? (Reflect.get(t, k, t) as (...x: unknown[]) => unknown).bind(t) : Reflect.get(t, k, t)) }) as CloudAccount;
  const limited = new Proxy(a, { get: (t, k) => (k === "chain" ? before : k === "isAdmin" ? false : typeof Reflect.get(t, k, t) === "function" ? (Reflect.get(t, k, t) as (...x: unknown[]) => unknown).bind(t) : Reflect.get(t, k, t)) }) as CloudAccount;
  for (const account of [unread, limited]) {
    const page = drive(DevicesPanel, { account, link }, []);
    assert.doesNotMatch(page.html, /data-testid="first-host"|data-testid="more-settings"/);
    assert.match(page.html, /data-testid="this-device"/, "this device's settings in their place");
  }
});

// ------------------------------------------------------------ 「备用中继」: the account's standby relay

test("「备用中继」: the account's standby relay preference — off by default, said in every language, set through the hub, hidden when the hub does not say", async () => {
  const hub = new FakeKeysHub();
  const a = await hub.device("tok");
  await createV2Genesis(a, true);
  const link = quietLink();
  const view = (relayStandby: boolean | null) =>
    ({ account: a.account, plan: null, chainSeq: a.chain!.seq, chainV3: null, machines: [], devices: [], relayStandby }) as unknown as AccountView;
  meStore(a, link).view = view(null);
  assert.equal(drive(RelayStandbySetting, { account: a, link }, []).html, "", "a hub that does not say: nothing to set");
  meStore(a, link).view = view(false);
  const off = drive(RelayStandbySetting, { account: a, link }, []);
  assert.match(off.html, /data-testid="relay-standby"/);
  assert.doesNotMatch(off.html, /data-on/);
  assert.match(off.html, /role="switch" aria-checked="false"/);
  assert.ok(off.html.includes("Keep a standby relay"));
  assert.ok(off.html.includes("an idle connection is not counted as transfer"), "what it costs: one more idle connection, no transfer");
  assert.ok(drive(RelayStandbySetting, { account: a, link }, [], "zh-CN").html.includes("保留备用中继"));
  assert.ok(drive(RelayStandbySetting, { account: a, link }, [], "zh-CN").html.includes("空闲连接不计流量"));
  // (This file's i18n has no zh-TW: its table, then.)
  assert.equal(cloudDevicesZhTW["Keep a standby relay"], "保留備用中繼");
  assert.ok(Object.keys(cloudDevicesZhTW).some((k) => k.includes("an idle connection is not counted as transfer")));
  assert.ok(drive(RelayStandbySetting, { account: a, link }, [], "ja").html.includes("予備のリレーを保つ"));
  meStore(a, link).view = view(true);
  assert.match(drive(RelayStandbySetting, { account: a, link }, []).html, /data-on=""/);
  // Switching asks the hub (`PATCH /v1/relays/prefs`).
  meStore(a, link).view = view(false);
  const asked: boolean[] = [];
  Object.assign(a.api, { setRelayStandby: async (_token: string, on: boolean) => (asked.push(on), on) });
  drive(RelayStandbySetting, { account: a, link }, [
    (tree) => {
      const sw = tree.find((e) => e.props.testId === "relay-standby-switch");
      assert.ok(sw, "the switch");
      (sw.props.onChange as (on: boolean) => void)(true);
    },
  ]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(asked, [true]);
});
