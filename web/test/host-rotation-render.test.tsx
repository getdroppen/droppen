// Renewing a host's identity key on screen (docs/host-identity-rotation.md §4.4,
// §3.7, §3.8, §7 decisions 1, 5, 7, 8, 9): each row state of
// 「我的设备」, the "more" menu that keeps re-enrolling, the person's own
// renewal and the dialog where the device's code is TYPED, the account
// setting, the notice, the confirmations that say who will renew, a host that
// must renew first refused in another's list, and the host's own Droppen
// settings (the code, the last renewal, "renewed elsewhere" with the way back).
// Every new string is in zh-CN, zh-TW and ja.
//
// `react-dom/server` with no DOM, like the other render tests.
import * as React from "react";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { renderToStaticMarkup } from "react-dom/server";
import { ControlConfirm, DeviceRow, type DeviceRowView, HostControl, rowMenuItems } from "../src/features/cloud/devices.js";
import { AlwaysConfirmSetting, EndPauseConfirm, EnterCode, PushRenewNotice, RenewalLine, RenewalNotice, RenewalsAhead, RenewPanel } from "../src/features/cloud/renewal.js";
import { renewalErrorText } from "../src/features/cloud/ui.js";
import { RotationNote, linkRotation, type LinkRotation } from "../src/features/link/link-settings.js";
import { RenewalError, type RenewalFailure } from "../src/lib/hub/account.js";
import type { RenewalRunner, RenewalStatus } from "../src/lib/hub/renewal.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { linkJa, linkZhCN, linkZhTW } from "../src/i18n/link.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";

const i18n = i18next.createInstance();
void i18n.use(initReactI18next).init({
  lng: "en",
  resources: {
    en: { translation: {} },
    "zh-CN": { translation: { ...linkZhCN, ...cloudDevicesZhCN, ...zhCN } },
    "zh-TW": { translation: { ...linkZhTW, ...cloudDevicesZhTW, ...zhTW } },
    ja: { translation: { ...linkJa, ...cloudDevicesJa, ...ja } },
  },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

function draw(node: React.ReactElement, lng = "en"): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
}

type El = React.ReactElement<Record<string, unknown>>;

function elements(node: unknown, out: El[] = []): El[] {
  if (Array.isArray(node)) for (const n of node) elements(n, out);
  else if (node && typeof node === "object" && "props" in node) {
    out.push(node as El);
    elements((node as El).props.children, out);
  }
  return out;
}

/** Render `Component` inline in a probe and run `steps[i]` on its tree at render `i` (as the devices page tests do). */
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

const host = (over: Partial<DeviceRowView> = {}): DeviceRowView => ({
  id: "m-1",
  fingerprint: "ABCD-EFGH-IJKL-MNOP",
  name: "laptop",
  self: false,
  kind: "computer",
  status: "active",
  host: true,
  stoppedHost: false,
  presence: "online",
  path: null,
  copyAsOf: null,
  lastSeenMs: null,
  limited: null,
  accept: { type: "admins" },
  ...over,
});
/** The English, with what it names put in (as a menu item says it). */
const tt = (key: string, opts?: Record<string, unknown>) => key.replace(/\{\{(\w+)\}\}/g, (_, k: string) => String(opts?.[k]));
const rowHtml = (view: DeviceRowView, extra: Partial<Parameters<typeof DeviceRow>[0]> = {}, lng = "en") =>
  draw(
    <ul>
      <DeviceRow view={view} viewerAdmin upgraded writable onRemove={() => undefined} onReEnroll={() => undefined} {...extra} />
    </ul>,
    lng,
  );

const STATES: [RenewalStatus, string, string][] = [
  [{ kind: "renewing", elsewhere: false }, "renewing its identity key… (about 2 minutes)", "正在更换身份密钥…（约 2 分钟）"],
  [{ kind: "renewing", elsewhere: true }, "renewing its identity key… (about 2 minutes)", "正在更换身份密钥"],
  [{ kind: "offline" }, "renews its identity key once it is back online", "上线后自动更换身份密钥"],
  [{ kind: "queued", after: ["m-2"], stuck: [] }, "renews its identity key after m-2", "等 m-2 更换之后再更换身份密钥"],
  [{ kind: "queued", after: ["m-2"], stuck: [{ host: "m-2", why: "needsCode" }] }, "m-2 needs confirming on the device itself first", "m-2 需要先在那台设备上确认"],
  [{ kind: "queued", after: ["m-2"], stuck: [{ host: "m-2", why: "isolated" }] }, "m-2 is isolated: settle it on its row first", "m-2 已被隔离"],
  [{ kind: "queued", after: ["m-2"], stuck: [{ host: "m-2", why: "blocked" }] }, "m-2 cannot renew until who can control it is narrowed", "m-2 要先收窄谁可以控制它"],
  [{ kind: "queued", after: ["m-2"], stuck: [{ host: "m-2", why: "offline" }] }, "m-2 is offline: this one renews after it is back and has renewed", "m-2 离线"],
  [{ kind: "waiting", hosts: ["m-9"] }, "renews its identity key once Droppen is updated on m-9", "m-9 更新 Droppen 之后更换身份密钥"],
  [{ kind: "retrying", atMs: Date.now() + 60_000, error: "the device did not answer" }, "could not renew its identity key: the device did not answer · trying again at", "没能更换身份密钥"],
  [{ kind: "stuck", error: "the device did not answer" }, "could not renew its identity key: the device did not answer", "换不成身份密钥"],
  [{ kind: "tooOld" }, "Droppen is too old: it renews its identity key once updated", "Droppen 版本过旧：更新后自动更换身份密钥"],
  [{ kind: "blocked", by: ["m-3"] }, "It lets m-3 control it, which may have had its identity key copied and cannot renew it: renewing this one would not help.", "它放行着 m-3 控制它"],
  [{ kind: "needsCode", why: "policy" }, "needs confirming on the device itself", "需要在那台设备上确认"],
  [{ kind: "needsCode", why: "evidence" }, "only the code its own screen shows tells them apart", "只有它自己屏幕上显示的代码能分清两者"],
  [{ kind: "isolated" }, "Nothing here connects to it until this is settled.", "处理好之前，这里不会连它"],
  [{ kind: "renewed", atMs: Date.now() - 60_000 }, "renewed identity key · ", "已更换身份密钥 · "],
];

test("each row state says where the renewal stands (§4.4), in English and in zh-CN", () => {
  for (const [status, en, zh] of STATES) {
    const html = rowHtml(host({ renewal: status, renewable: true }));
    assert.match(html, new RegExp(`data-renewal="${status.kind}"`), status.kind);
    assert.ok(html.includes(en), `${status.kind}: ${en}\n${html}`);
    assert.ok(rowHtml(host({ renewal: status, renewable: true }), {}, "zh-CN").includes(zh), `${status.kind} in zh-CN`);
  }
  // Nothing to say: nothing said.
  assert.doesNotMatch(rowHtml(host({ renewal: { kind: "none" } })), /data-renewal/);
  // Colour only for state: warn for what needs the person, red for isolated, none for progress.
  assert.match(rowHtml(host({ renewal: { kind: "isolated" } })), /data-renewal="isolated" data-signal="failed"/);
  assert.match(rowHtml(host({ renewal: { kind: "needsCode", why: "policy" } })), /data-renewal="needsCode" data-signal="warn"/);
  // A host an exposed device that cannot renew controls: no "renew again" — the way is its list (§4.1 case 3).
  const blocked = rowHtml(host({ renewal: { kind: "blocked", by: ["m-3"] } }), { onControl: () => undefined });
  assert.match(blocked, /data-renewal="blocked" data-signal="warn"/);
  assert.ok(blocked.includes("Narrow who can control it, or add m-3 again."));
  assert.match(blocked, /data-testid="renewal-narrow-m-1"/);
  assert.doesNotMatch(rowHtml(host({ renewal: { kind: "renewing", elsewhere: false } })), /data-renewal="renewing" data-signal/);
});

test("renewed: the new fingerprint, the time, and the reminder about what else that device held (§7 decision 9)", () => {
  const html = rowHtml(host({ fingerprint: "NEWF-INGE-RPRI-NTXX", renewal: { kind: "renewed", atMs: Date.now() } }));
  assert.ok(html.includes("NEWF-INGE-RPRI-NTXX"));
  assert.ok(html.includes("A device that could control m-1 may have read its Claude login and SSH keys: change them where they were issued."));
});

test("too old: re-enrolling is in the row's ⋯ menu (§7 decision 7); an admin renews from it, a limited device does not", () => {
  const view = host({ renewal: { kind: "tooOld" }, renewable: false, renewalsOn: true });
  assert.ok(rowHtml(view).includes("or, in its ⋯ menu: remove it, then add it again"));
  const asked: string[] = [];
  const ctx = { viewerAdmin: true, upgraded: true, writable: true, t: tt, onReEnroll: () => asked.push("re-enroll"), onRenew: () => asked.push("renew") };
  const items = rowMenuItems(view, ctx);
  const reEnroll = items.find((i) => i.id === "re-enroll-m-1")!;
  assert.equal(reEnroll.label, "remove it, then add it again");
  assert.equal(reEnroll.hint, "a new identity: its Droppen cannot renew its identity key");
  assert.equal(items.some((i) => i.id === "renew-m-1"), false, "no renewal for a host that cannot renew");
  reEnroll.onSelect();
  // A host that renews here: "renew identity key" in its menu, and re-enrolling only said to be the other way.
  const renewable = rowMenuItems(host({ renewable: true, renewalsOn: true }), ctx);
  const renew = renewable.find((i) => i.id === "renew-m-1")!;
  assert.equal(renew.label, "renew identity key");
  assert.equal(renew.blocked, null);
  renew.onSelect();
  assert.equal(renewable.find((i) => i.id === "re-enroll-m-1")!.hint, "a new identity: only if renewing its identity key is not the way");
  assert.deepEqual(asked, ["re-enroll", "renew"]);
  assert.equal(rowMenuItems(host({ renewable: true, renewalsOn: true }), { ...ctx, viewerAdmin: false }).some((i) => i.id === "renew-m-1" || i.id === "re-enroll-m-1"), false, "not for a limited device");
});

test("the hub's renewal switch off: a host's ⋯ menu is as before renewals — no “remove it, then add it again” saying its Droppen cannot renew", () => {
  const ctx = { viewerAdmin: true, upgraded: true, writable: true, t: tt, onReEnroll: () => undefined, onRenew: () => undefined, onRemove: () => undefined };
  // What devices.tsx gives a host while the hub's switch is off (or the hub has no renewals): nothing renews, `renewalsOn` unset.
  for (const view of [host(), host({ renewalsOn: false }), host({ renewable: false, renewalsOn: false })]) {
    const ids = rowMenuItems(view, ctx).map((i) => i.id);
    assert.equal(ids.includes("re-enroll-m-1"), false, ids.join(","));
    assert.equal(ids.includes("renew-m-1"), false);
    assert.ok(ids.includes("remove-m-1"), "removing stays");
  }
  // On, a host that cannot renew is offered the new identity, and says why.
  assert.equal(rowMenuItems(host({ renewalsOn: true }), ctx).find((i) => i.id === "re-enroll-m-1")?.hint, "a new identity: its Droppen cannot renew its identity key");
});

test("§4.1 “renew identity key” cannot be pressed while an exposed host it lets control it has not renewed, and says which", () => {
  let asked = 0;
  const ctx = { viewerAdmin: true, upgraded: true, writable: true, t: tt, onRenew: () => asked++ };
  const waiting = rowMenuItems(host({ id: "m-2", renewable: true, renewFirst: ["m-1"] }), ctx).find((i) => i.id === "renew-m-2")!;
  assert.equal(waiting.blocked, "renew the identity key of m-1 first: it lets them control it");
  waiting.onSelect();
  assert.equal(asked, 0, "pressing it renews nothing");
  const free = rowMenuItems(host({ id: "m-2", renewable: true, renewFirst: [] }), ctx).find((i) => i.id === "renew-m-2")!;
  assert.equal(free.blocked, null);
  free.onSelect();
  assert.equal(asked, 1, "a host not waiting for another renews when asked");
  const zh = rowMenuItems(host({ id: "m-2", renewable: true, renewFirst: ["m-1"] }), { ...ctx, t: i18n.getFixedT("zh-CN") as unknown as typeof tt }).find((i) => i.id === "renew-m-2")!;
  assert.match(zh.blocked ?? "", /请先更换 m-1 的身份密钥/);
});

test("isolated (§3.8 「页面」): first look at Droppen on it; “replaced elsewhere” makes removing the main action, a code makes confirming it; removing is one click away in both", () => {
  const calls: string[] = [];
  const props = { id: "m-1", status: { kind: "isolated" } as RenewalStatus, viewerAdmin: true, onCode: () => calls.push("code"), onRemove: () => calls.push("remove"), onAddComputer: () => calls.push("add") };
  const html = draw(<RenewalLine {...props} />);
  // Not alarming: the computer's own backup from before a renewal ends here too.
  assert.ok(html.includes("for example, it was restored from a backup made before the renewal, or another copy of its old key is in use"));
  assert.doesNotMatch(html, /impersonat|hijack/i);
  assert.match(html, /data-testid="renewal-fresh-m-1"/);
  const at = (needle: string) => html.indexOf(needle);
  // Said all along, until it is removed or confirmed.
  assert.ok(html.includes("Until m-1 is removed or confirmed on the device, the devices that let it control them still accept whoever holds its current key."));
  // Step 1: look at it.
  assert.ok(at('data-testid="renewal-look-m-1"') > 0 && at('data-testid="renewal-look-m-1"') < at('data-testid="renewal-replaced-m-1"'));
  assert.ok(html.includes("Look at Droppen on m-1 (Settings → Access → Droppen)."));
  // "Replaced elsewhere": removing is the main action (danger), then add it again under a new identity.
  assert.ok(html.includes("It says “this device&#x27;s identity key was replaced elsewhere”:") || html.includes("It says “this device's identity key was replaced elsewhere”:"));
  assert.match(html, /<button[^>]*data-testid="renewal-remove-m-1"[^>]*>remove m-1</);
  assert.ok(at('data-testid="renewal-remove-m-1"') < at("then, on that device, choose “add this device again with a new identity”"));
  assert.match(html, /data-testid="renewal-add-m-1"/);
  // A code: confirming on the device is the main action; removing still one click away.
  assert.ok(html.includes("It does not say “replaced elsewhere”: confirm on the device, with the code it shows."));
  assert.match(html, /<button[^>]*data-testid="renewal-code-m-1"[^>]*class="[^"]*"[^>]*>confirm on the device</);
  assert.match(html, /data-testid="renewal-remove-also-m-1"[^>]*>or remove m-1</);
  drive(RenewalLine, props, [
    (tree) => {
      for (const id of ["renewal-code-m-1", "renewal-remove-m-1", "renewal-remove-also-m-1", "renewal-add-m-1"]) (tree.find((e) => e.props["data-testid"] === id)!.props.onClick as () => void)();
    },
  ]);
  assert.deepEqual(calls, ["code", "remove", "remove", "add"]);
  for (const [lng, look] of [["zh-CN", "去看 m-1 上的 Droppen"], ["zh-TW", "去看 m-1 上的 Droppen"], ["ja", "m-1 の Droppen を見てください"]] as const) {
    assert.ok(draw(<RenewalLine {...props} />, lng).includes(look), lng);
  }
  // A device that is not an admin sees the state, and nothing to press.
  assert.doesNotMatch(draw(<RenewalLine id="m-1" status={{ kind: "isolated" }} viewerAdmin={false} onCode={() => undefined} />), /renewal-code-m-1|renewal-fresh-m-1/);
});

test("the person's renewal: how it is confirmed, then the code typed — never shown (§3.7, §8 item 14)", () => {
  const asked: unknown[] = [];
  const runner = { renew: (h: string, how: unknown) => (asked.push([h, how]), new Promise<void>(() => undefined)) } as unknown as Pick<RenewalRunner, "renew">;
  const open = draw(<RenewPanel host="m-1" runner={runner} onDone={() => undefined} />);
  assert.ok(open.includes("renew the identity key of m-1?"));
  assert.ok(open.includes("automatically (about 2 minutes)") && open.includes("confirm on the device: type the code it shows"));
  assert.match(open, /data-variant="primary"[^>]*data-testid="renew-start-m-1"/);
  // Confirmed on the device: the choice, then the button — the runner is asked for exactly that.
  drive(RenewPanel, { host: "m-1", runner, onDone: () => undefined }, [
    (tree) => (tree.filter((e) => e.type === "input" && e.props.type === "radio")[1]!.props.onChange as () => void)(),
    (tree) => (tree.find((e) => e.props["data-testid"] === "renew-start-m-1")!.props.onClick as () => void)(),
  ]);
  const [h, how] = asked.at(-1) as [string, { assurance: number; askCode: unknown }];
  assert.equal(h, "m-1");
  assert.equal(how.assurance, 1);
  assert.equal(typeof how.askCode, "function");
  // Only that way (`fixed`): no choice offered.
  const fixed = draw(<RenewPanel host="m-1" runner={runner} fixed onDone={() => undefined} />);
  assert.doesNotMatch(fixed, /type="radio"/);
});

test("the code dialog: typed (8 characters of the code's alphabet), confirm only then, cancel gives up", () => {
  const typed: string[] = [];
  const html = draw(<EnterCode host="m-1" onCode={(c) => typed.push(c)} onCancel={() => typed.push("cancel")} />);
  assert.ok(html.includes("type the code m-1 shows"));
  assert.ok(html.includes("If it shows none, or another code, cancel: nothing is changed then."));
  assert.match(html, /placeholder="XXXX-XXXX"/);
  assert.match(html, /data-testid="renew-code-confirm-m-1"[^>]*disabled=""|disabled=""[^>]*data-testid="renew-code-confirm-m-1"/, "nothing typed yet");
  const run = drive(EnterCode, { host: "m-1", onCode: (c: string) => typed.push(c), onCancel: () => typed.push("cancel") }, [
    (tree) => (tree.find((e) => e.type === "input")!.props.onChange as (e: unknown) => void)({ target: { value: "abcd-efgh" } }),
    (tree) => {
      assert.equal(tree.find((e) => e.props["data-testid"] === "renew-code-confirm-m-1")!.props.disabled, false);
      (tree.find((e) => e.type === "form")!.props.onSubmit as (e: unknown) => void)({ preventDefault: () => undefined });
    },
  ]);
  assert.equal(run.ran, 2);
  assert.deepEqual(typed, ["abcd-efgh"]);
});

test("the account's 「总在设备上确认」 is off by default, written on the chain, and a switch away; turning it off is asked first", async () => {
  const set: boolean[] = [];
  const runner = { setAlwaysAssured: async (on: boolean) => void set.push(on) } as unknown as RenewalRunner;
  const local: boolean[] = [];
  const account = { alwaysAssured: false, localAssured: () => false, setLocalAssured: (on: boolean) => void local.push(on) } as never;
  const html = draw(<AlwaysConfirmSetting runner={runner} account={account} />);
  assert.ok(html.includes("Always confirm on the device when renewing an identity key"));
  assert.ok(html.includes("On this device, only sign renewals confirmed on the device"));
  assert.doesNotMatch(html, /checked=""/);
  assert.ok(draw(<AlwaysConfirmSetting runner={runner} account={account} />, "zh-CN").includes("更换身份密钥时总在设备上确认"));
  assert.ok(draw(<AlwaysConfirmSetting runner={runner} account={account} />, "zh-TW").includes("更換身分金鑰時總在裝置上確認"));
  assert.ok(draw(<AlwaysConfirmSetting runner={runner} account={account} />, "ja").includes("ID 鍵の更新は常にデバイス上で確認する"));
  assert.ok(draw(<AlwaysConfirmSetting runner={runner} account={account} />, "zh-CN").includes("这台设备只签在设备上确认的换钥"));
  // The account's policy (on the chain) and this device's own guard, each its own switch.
  assert.match(draw(<AlwaysConfirmSetting runner={runner} account={{ alwaysAssured: true, localAssured: () => true, setLocalAssured: () => undefined } as never} />), /checked=""[\s\S]*checked=""/);
  drive(AlwaysConfirmSetting, { runner, account }, [
    (tree) => {
      const boxes = tree.filter((e) => e.type === "input");
      void (boxes[0]!.props.onChange as (e: unknown) => Promise<void>)({ target: { checked: true } });
      (boxes[1]!.props.onChange as (e: unknown) => void)({ target: { checked: true } });
    },
  ]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(set, [true]);
  assert.deepEqual(local, [true]);
});

test("turning confirming on the device off is asked before it is written; with no policy chosen, the admin is asked to choose first", async () => {
  const set: boolean[] = [];
  const runner = { setAlwaysAssured: async (on: boolean) => void set.push(on) } as unknown as RenewalRunner;
  const on = { alwaysAssured: true, renewalPolicyChosen: true, localAssured: () => false, setLocalAssured: () => undefined } as never;
  // Unchecking asks; nothing is written yet.
  const asked = drive(AlwaysConfirmSetting, { runner, account: on }, [
    (tree) => {
      const box = tree.find((e) => e.type === "input")!;
      void (box.props.onChange as (e: unknown) => Promise<void>)({ target: { checked: false } });
    },
  ]);
  void asked;
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(set, [], "not written before the person confirms");
  const confirming = drive(AlwaysConfirmSetting, { runner, account: on }, [
    (tree) => {
      const box = tree.find((e) => e.type === "input")!;
      void (box.props.onChange as (e: unknown) => Promise<void>)({ target: { checked: false } });
    },
    (tree) => (tree.find((e) => e.props["data-testid"] === "renewal-policy-off-yes")!.props.onClick as () => void)(),
  ]);
  assert.match(confirming.html, /data-testid="renewal-policy-off-confirm"/);
  assert.ok(confirming.html.includes("Let devices that run AI renew their identity key on their own?"));
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(set, [false], "written once confirmed");
  // Never chosen: the choice first — "always confirm" at once, "on their own" asked too.
  set.length = 0;
  const unset = { alwaysAssured: false, renewalPolicyChosen: false, localAssured: () => false, setLocalAssured: () => undefined } as never;
  const html = draw(<AlwaysConfirmSetting runner={runner} account={unset} />);
  assert.match(html, /data-testid="renewal-policy-choose"/);
  assert.ok(html.includes("Until you choose, no device that runs AI renews its identity key on its own."));
  assert.ok(draw(<AlwaysConfirmSetting runner={runner} account={unset} />, "zh-CN").includes("选好之前，可运行 AI 的设备都不会自动更换身份密钥。"));
  drive(AlwaysConfirmSetting, { runner, account: unset }, [
    (tree) => (tree.find((e) => e.props["data-testid"] === "renewal-policy-assured")!.props.onClick as () => void)(),
  ]);
  const auto = drive(AlwaysConfirmSetting, { runner, account: unset }, [
    (tree) => (tree.find((e) => e.props["data-testid"] === "renewal-policy-auto")!.props.onClick as () => void)(),
  ]);
  void auto;
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(set, [true], "always-confirm written at once; on-their-own only asked");
  // The row of a host waiting for that choice.
  assert.ok(rowHtml(host({ renewal: { kind: "policyUnset" }, renewable: true })).includes("renews its identity key once the account chose how renewals are confirmed"));
});

test("the notice: devices renewing now, and those that need the person (§4.4)", () => {
  const runner = (renewing: string[], needYou: string[]) => ({ notice: () => ({ renewing, needYou }) }) as unknown as RenewalRunner;
  assert.equal(draw(<RenewalNotice runner={runner([], [])} />), "");
  const html = draw(<RenewalNotice runner={runner(["m-1", "m-2"], ["m-3"])} />);
  assert.ok(html.includes("renewing the identity key of m-1, m-2"));
  assert.ok(html.includes("m-3 need confirming on the device for their identity key"));
  assert.match(html, /data-testid="renewal-needs-you" data-signal="warn"/);
  assert.ok(draw(<RenewalNotice runner={runner(["m-1"], [])} />, "zh-CN").includes("正在为 m-1 更换身份密钥"));
});

test("removing or narrowing says which devices will renew their identity key (§4.4)", () => {
  assert.equal(draw(<RenewalsAhead ids={[]} />), "");
  assert.ok(draw(<RenewalsAhead ids={["m-1"]} />).includes("m-1 will renew its identity key automatically (in about 2 minutes, once it is online)."));
  assert.ok(draw(<RenewalsAhead ids={["m-1", "m-2"]} />).includes("2 devices that run AI will renew their identity key automatically (those online in about 2 minutes): m-1, m-2."));
  const renewing = draw(<ControlConfirm host="m-1" admitted={[]} shutOut={["d-2"]} renewable renews={["m-1", "m-2"]} busy={false} onConfirm={() => undefined} onCancel={() => undefined} />);
  assert.ok(renewing.includes("and a new identity key, so a copy of the old one is no use"));
  assert.match(renewing, /data-testid="renewals-ahead"/);
  assert.doesNotMatch(renewing, /Re-enroll m-1 afterwards/);
  // A host that cannot renew: the re-enroll advice, as before.
  const old = draw(<ControlConfirm host="m-1" admitted={[]} shutOut={["d-2"]} busy={false} onConfirm={() => undefined} onCancel={() => undefined} />);
  assert.ok(old.includes("Re-enroll m-1 afterwards: remove it, then add it again."));
});

test("a host that must renew first is let control no other host: Save is off and it says why (§7 decision 8)", () => {
  const props = {
    host: "m-2",
    policy: { type: "admins" } as const,
    declaresPolicy: true,
    candidates: [{ id: "m-1", host: true }, { id: "m-3", host: true }],
    upgraded: true,
    mustRenew: ["m-1"],
    busy: false,
    onSave: () => undefined,
    onCancel: () => undefined,
  };
  const primary = (tree: El[]) => tree.find((e) => e.type === "button" && e.props["data-variant"] === "primary")!;
  const pick = (id: string) => [
    (tree: El[]) => (tree.filter((e) => e.type === "input" && e.props.type === "radio")[2]!.props.onChange as () => void)(),
    (tree: El[]) => (tree.filter((e) => e.type === "input" && e.props.type === "checkbox")[id === "m-1" ? 0 : 1]!.props.onChange as (e: unknown) => void)({ target: { checked: true } }),
  ];
  const exposed = drive(HostControl, props, pick("m-1"));
  assert.match(exposed.html, /data-testid="control-exposed" data-signal="warn"/);
  assert.ok(exposed.html.includes("Renew the identity key of m-1 first"));
  assert.equal(primary(exposed.tree).props.disabled, true);
  const fine = drive(HostControl, props, pick("m-3"));
  assert.doesNotMatch(fine.html, /control-exposed/);
  assert.equal(primary(fine.tree).props.disabled, false);
});

test("a refusal of a renewal reads in words, in the reader's language", () => {
  void i18n.changeLanguage("zh-CN");
  const t = i18n.t.bind(i18n) as (key: string, opts?: Record<string, unknown>) => string;
  const kinds: RenewalFailure[] = ["unconfirmed", "behind", "fork", "lapsed", "tooOld", "disabled", "needsAssurance", "inProgress", "offline", "hubNotReady", "waiting", "renewFirst", "superseded", "policyUnset", "refused", "badAnswer", "wrongCode", "cancelled", "voided", "rate", "failed"];
  for (const kind of kinds) {
    const said = renewalErrorText(new RenewalError(kind, "x", kind === "voided" ? "not_acked" : "why", ["m-9"]), t);
    assert.match(said, /[一-鿿]/, `${kind}: ${said}`);
  }
  // The host's 412 `stale` in words (the reported 「found no recent word on its chain head」): who it waits for, in every language.
  const waits = new RenewalError("unconfirmed", "m-1 waits for another admin device (d-2) to vouch for the account's latest change", "stale", ["d-2"]);
  assert.ok(renewalErrorText(waits, t).includes("请在 d-2 上打开 Droppen"), renewalErrorText(waits, t));
  for (const [lng, words] of [
    ["en", "open Droppen on d-2, then try again"],
    ["zh-TW", "請在 d-2 上打開 Droppen"],
    ["ja", "d-2 で Droppen を開いてから"],
  ] as const) {
    void i18n.changeLanguage(lng);
    assert.ok(renewalErrorText(waits, i18n.t.bind(i18n) as typeof t).includes(words), lng);
  }
  // `lapsed` may be this device no longer controlling it: the way out is renewing it from another full device.
  for (const [lng, words] of [
    ["en", "or renew it from that device"],
    ["zh-CN", "或直接从那台设备更换它的身份密钥"],
    ["zh-TW", "或直接從那台裝置更換它的身分金鑰"],
    ["ja", "そのデバイスから ID 鍵を更新してください"],
  ] as const) {
    void i18n.changeLanguage(lng);
    assert.ok(renewalErrorText(new RenewalError("lapsed", "x", "lapsed"), i18n.t.bind(i18n) as typeof t).includes(words), lng);
  }
  // ja: `このデバイス` is the reader's own device — never the host the fork is about.
  assert.doesNotMatch(renewalErrorText(new RenewalError("fork", "x", "fork"), i18n.t.bind(i18n) as typeof t), /^このデバイス/);
  void i18n.changeLanguage("en");
  assert.doesNotMatch(renewalErrorText(new RenewalError("unconfirmed", "x", "stale", []), t), /\{\{ids\}\}|open Droppen on/, "the sole admin: no one else to open");
  // The row says the failure itself in the reader's language, not its message.
  const row = draw(<RenewalLine id="m-1" status={{ kind: "retrying", atMs: Date.now() + 60_000, error: waits.message, cause: waits }} viewerAdmin />, "zh-CN");
  assert.ok(row.includes("没能更换身份密钥：它在等你的另一台完整设备确认账户的最新变更"), row);
  assert.doesNotMatch(row, /vouch/);
});

test("R6: a host that lets an isolated one control it says to remove that one first; notifications stopped by a renewal come back with one tap", async () => {
  const html = rowHtml(host({ id: "m-2", isolatedControllers: ["m-1"] }));
  assert.match(html, /data-testid="isolated-controllers-m-2" data-signal="warn"/);
  assert.ok(html.includes("It lets m-1 control it, which is isolated: settle it on its row first."));
  assert.doesNotMatch(html, /remove m-1 first/, "removing is not the only way any more");
  assert.ok(rowHtml(host({ id: "m-2", isolatedControllers: ["m-1"] }), {}, "zh-CN").includes("请先在那一行处理好它"));
  // A group renewing together (§4.1): control between them is paused, said on each row until the lists are back.
  const paused = rowHtml(host({ pausedWith: ["m-1", "m-2"] }));
  assert.match(paused, /data-testid="paused-m-1"/);
  assert.ok(paused.includes("Control between m-1, m-2 is paused while their identity keys are renewed."));
  assert.ok(rowHtml(host({ pausedWith: ["m-1", "m-2"] }), {}, "zh-CN").includes("m-1, m-2 之间的控制已暂停"));
  assert.doesNotMatch(rowHtml(host({ pausedWith: [] })), /paused-m-1/);
  // A sign-in with a retired key: a quiet note, not a warning (§3.8: information only).
  const login = rowHtml(host({ retiredLoginMs: Date.now() - 60_000 }));
  assert.match(login, /data-testid="retired-login-m-1"/);
  assert.ok(login.includes("its old identity key was used to sign in at"));
  assert.doesNotMatch(login, /data-testid="retired-login-m-1"[^>]*data-signal/);
  // The one-tap notice (§2.2 推送): only when a renewal left this browser without notifications.
  assert.equal(draw(<PushRenewNotice needed={() => null} />), "");
  const notice = draw(<PushRenewNotice needed={() => "m-1"} />);
  assert.ok(notice.includes("Notifications from m-1 stopped when it renewed its identity key."));
  assert.match(notice, /data-testid="push-renew-on"[^>]*>turn notifications back on</);
  const enabled: string[] = [];
  drive(PushRenewNotice, { needed: () => "m-1", enable: async (m: string) => void enabled.push(m) }, [
    (tree) => void (tree.find((e) => e.props["data-testid"] === "push-renew-on")!.props.onClick as () => Promise<void>)(),
  ]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(enabled, ["m-1"], "the tap subscribes again");
});

// ------------------------------------------------------------ the host's own settings

const note = (rotation: LinkRotation, canManage = true, onFresh = () => undefined, lng = "en") =>
  draw(<RotationNote rotation={rotation} canManage={canManage} busy={false} error={null} onFresh={onFresh} now={1_000} />, lng);

test("the host's own Droppen settings: the code while it lasts, the last renewal, nothing otherwise", () => {
  const staged = note({ state: "staged", code: "ABCD-EFGH", codeExpiresMs: 5_000 });
  assert.match(staged, /data-testid="droppen-rotation-code"[^>]*>ABCD-EFGH</);
  assert.ok(staged.includes("Type this code on the device that asked"));
  assert.equal(note({ state: "staged", code: "ABCD-EFGH", codeExpiresMs: 500 }), "", "expired: not shown");
  const renewed = note({ state: "renewed", renewedMs: 2_000, previousFingerprint: "OLD-FP" });
  assert.match(renewed, /identity key renewed · /);
  assert.ok(renewed.includes("previous fingerprint OLD-FP"));
  assert.equal(linkRotation({ link: null }), null);
  assert.equal(linkRotation({ link: { rotation: { state: "idle" } } } as never), null);
  assert.deepEqual(linkRotation({ link: { rotation: { state: "foreign", reason: "renewed_elsewhere" } } } as never), { state: "foreign", reason: "renewed_elsewhere" });
  // Refused while it syncs: said quietly — nothing is known yet.
  const refused = note({ state: "refused" });
  assert.match(refused, /data-state="refused"/);
  assert.doesNotMatch(refused, /data-signal|droppen-fresh/);
  assert.ok(refused.includes("syncing the account before trying again"));
});

test("replaced elsewhere: red, said without alarm, and — asked first — “add this device again with a new identity” (§3.8)", () => {
  const foreign = note({ state: "foreign", reason: "renewed_elsewhere" });
  assert.match(foreign, /data-state="foreign" data-signal="failed"/);
  assert.ok(foreign.includes("identity key was replaced elsewhere"));
  // Its own backup from before a renewal looks the same: no word of theft.
  assert.ok(foreign.includes("for example, it was restored from a backup made before a renewal, or another copy of its old key renewed it"));
  assert.doesNotMatch(foreign, /impersonat|hijack|stolen/i);
  assert.match(foreign, /data-testid="droppen-fresh"/);
  assert.doesNotMatch(note({ state: "foreign", reason: "renewed_elsewhere" }, false), /droppen-fresh/, "only who manages the link");
  let fresh = 0;
  const run = drive(RotationNote, { rotation: { state: "foreign", reason: "renewed_elsewhere" }, canManage: true, busy: false, error: null, onFresh: () => fresh++ }, [
    (tree) => (tree.find((e) => e.props["data-testid"] === "droppen-fresh")!.props.onClick as () => void)(),
    (tree) => (tree.find((e) => e.props["data-testid"] === "droppen-fresh-confirm")!.props.onClick as () => void)(),
  ]);
  assert.equal(run.ran, 2);
  assert.equal(fresh, 1);
  const foreignIn = (lng: string) => note({ state: "foreign", reason: "renewed_elsewhere" }, true, () => undefined, lng);
  assert.ok(foreignIn("zh-CN").includes("这台设备的身份密钥已在别处更换"));
  assert.ok(foreignIn("zh-TW").includes("以新身分重新新增這台裝置"));
  assert.ok(foreignIn("ja").includes("新しい ID でこのデバイスを追加し直す"));
});

test("a join or an enrollment refused for a retired key reads as “add it again under a new identity”", async () => {
  const { default: global } = await import("../src/i18n/index.js");
  const { errorText } = await import("../src/features/cloud/ui.js");
  const { AccountError } = await import("../src/lib/hub/account.js");
  const { RULES } = await import("../src/lib/e2e/index.js");
  const refused = new AccountError("invalidEntry", `the chain would not take this change: ${RULES.keyRetired}`, undefined, RULES.keyRetired);
  assert.ok(errorText(refused).includes("Add it again under a new identity"));
  await global.changeLanguage("zh-CN");
  try {
    assert.ok(errorText(refused).includes("请以新身份重新添加"));
  } finally {
    await global.changeLanguage("en");
  }
});

test("every string the renewal screens use is in zh-CN, zh-TW and ja", () => {
  const src = (f: string) => readFileSync(new URL(`../src/${f}`, import.meta.url), "utf8");
  const keysOf = (text: string) => new Set([...text.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)].map((m) => m[1]!));
  const renewal = keysOf(src("features/cloud/renewal.tsx"));
  const settings = keysOf(src("features/link/link-settings.tsx").slice(src("features/link/link-settings.tsx").indexOf("export interface LinkRotation")));
  const errors = keysOf(src("features/cloud/ui.tsx").slice(src("features/cloud/ui.tsx").indexOf("export function renewalErrorText")));
  assert.ok(renewal.size > 30 && settings.size > 8 && errors.size > 10);
  for (const [name, table] of [
    ["zh-CN", { ...linkZhCN, ...cloudDevicesZhCN, ...zhCN }],
    ["zh-TW", { ...linkZhTW, ...cloudDevicesZhTW, ...zhTW }],
    ["ja", { ...linkJa, ...cloudDevicesJa, ...ja }],
  ] as const) {
    const missing = [...renewal, ...settings, ...errors].filter((k) => !(k in table));
    assert.deepEqual(missing, [], `${name} lacks these`);
  }
  // The names the owner chose (§7 decision 5).
  assert.equal(cloudDevicesZhCN["renew identity key"], "更换身份密钥");
  assert.equal(cloudDevicesZhTW["renew identity key"], "更換身分金鑰");
  assert.equal(cloudDevicesJa["renew identity key"], "ID 鍵を更新");
});

test("§4.1 overlay UI: the list (base) and what a pause holds apart; 「结束暂停」 asks first, listing exactly what comes back, the `All` case, and what must stay held", () => {
  // The row: the base under "who can control it", the paused edges apart from it; why an edge stays held; "end pause".
  const ops: Uint8Array[] = [];
  const op = new Uint8Array(16).fill(7);
  const ending = { edges: [{ host: "m-1", id: "m-2" }], all: [], held: [{ host: "m-1", id: "m-2", who: "m-2", why: "exposed" as const }], orphan: true };
  const view = host({ accept: { type: "devices", ids: ["m-2", "m-3"] }, pausedControllers: ["m-2"], pausedWith: ["m-1", "m-2"], pauses: [{ op, ending }] });
  const html = rowHtml(view, { onEndPause: (o: Uint8Array) => ops.push(o) });
  assert.ok(html.includes("your full devices, and m-2, m-3"), "the base as set");
  assert.match(html, /paused now<\/dt><dd[^>]*>m-2</);
  assert.ok(html.includes("m-2 stays paused from controlling m-1: m-2 has not renewed its identity key since it may have been copied"));
  assert.match(html, /data-testid="end-pause-m-1"/);
  assert.ok(html.includes("Started by a device that is no longer a full device: it never ends by itself — end it here."), "an orphaned pause says so");
  // A host that can never renew says so (not merely "exposed").
  const old = rowHtml(host({ pauses: [{ op, ending: { ...ending, held: [{ host: "m-1", id: "m-3", who: "m-3", why: "cannotRenew" as const }], orphan: false } }] }));
  assert.ok(old.includes("m-3 stays paused from controlling m-1: m-3 cannot renew its identity key (its Droppen is too old): remove it, or update it"));
  drive(DeviceRow, { view, viewerAdmin: true, upgraded: true, writable: true, onEndPause: (o: Uint8Array) => ops.push(o) }, [
    (tree) => (tree.find((e) => e.props["data-testid"] === "end-pause-m-1")!.props.onClick as () => void)(),
  ]);
  assert.deepEqual(ops, [op]);
  assert.doesNotMatch(draw(<ul><DeviceRow view={view} viewerAdmin={false} upgraded writable onEndPause={() => undefined} /></ul>), /end-pause-m-1/, "not for a limited device");
  // The confirmation: exactly what comes back; the All note; refused while something must stay held.
  let confirmed = 0;
  const free = { edges: [{ host: "m-1", id: "m-2" }, { host: "m-3", id: "m-1" }], all: ["m-3"], held: [], orphan: false };
  const ask = draw(<EndPauseConfirm ending={free} busy={false} onConfirm={() => confirmed++} onCancel={() => undefined} />);
  assert.ok(ask.includes("m-2 can control m-1 again") && ask.includes("m-1 can control m-3 again"));
  assert.ok(ask.includes("m-3 lets every device of the account control it: ending the pause gives back only the control listed above."), "N1");
  assert.doesNotMatch(ask, /data-testid="end-pause-yes"[^>]*disabled=""/);
  drive(EndPauseConfirm, { ending: free, busy: false, onConfirm: () => confirmed++, onCancel: () => undefined }, [
    (tree) => (tree.find((e) => e.props["data-testid"] === "end-pause-yes")!.props.onClick as () => void)(),
  ]);
  assert.equal(confirmed, 1);
  const refused = draw(<EndPauseConfirm ending={ending} busy={false} onConfirm={() => confirmed++} onCancel={() => undefined} />);
  assert.match(refused, /data-testid="end-pause-yes"[^>]*disabled=""/);
  assert.ok(refused.includes("Not now: it would let a device control another while it must stay held."));
  assert.ok(draw(<EndPauseConfirm ending={free} busy={false} onConfirm={() => undefined} onCancel={() => undefined} />, "zh-CN").includes("m-2 将重新可以控制 m-1"));
  // The list editor shows the base, and the paused ids apart from it.
  const editor = draw(<HostControl host="m-1" policy={{ type: "devices", ids: ["m-2"] }} paused={["m-2"]} candidates={[{ id: "m-2", host: true }]} upgraded busy={false} onSave={() => undefined} onCancel={() => undefined} />);
  assert.ok(editor.includes("Paused while identity keys are renewed: m-2. They come back when the pause ends; the list here is what stays."));
});
