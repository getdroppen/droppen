// The local app showing the account's other devices (S1-7,
// `docs/local-aggregator.md` §6.4): mounted for real into the stand-in DOM,
// with this device's feed played from a copy and the other devices coming
// through a gateway the test plays.
//
// What is pinned is what a person sees: the device bar under the filter —
// 「需要你」 while something needs you, every device, this device, each other
// device with its state and, when it is not online, how current its copy is
// (「离线副本 · 截至 …」) — the 「需要你 · 所有设备」 ledger where no session is
// open, asking first; nothing of it on the local app alone. And the settings'
// switches for who sees the other devices here.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { makeI18n } from "./render.js";
import { type FakeNode, findAll, installLiveDom, until } from "./live-dom.js";

const container = installLiveDom();
const flush = (ms = 30) => new Promise((resolve) => setTimeout(resolve, ms));

const { createRoot } = await import("react-dom/client");
const { QueryClient, QueryClientProvider } = await import("@tanstack/react-query");
const router = await import("@tanstack/react-router");
const { I18nextProvider } = await import("react-i18next");
const { SessionsLayout, SessionsIndex } = await import("../src/features/sessions/sessions-layout.js");
const { DeviceFilterBar, chipNote, saveDeviceChoice } = await import("../src/features/sessions/device-filter.js");
const { DevicePicker, readOnlyHere } = await import("../src/features/machines/device-picker.js");
const { formatMoment } = await import("../src/lib/format.js");
const { RemoteAccess, unencryptedOverNetwork } = await import("../src/features/link/link-settings.js");
const { GlobalFeedProvider } = await import("../src/app/global-feed.js");
const { MachineProvider } = await import("../src/lib/machines/context.js");
const { LOCAL } = await import("../src/lib/machines/key.js");
const registry = await import("../src/lib/machines/registry.js");
const { offlineTransport } = await import("../src/lib/transport/offline.js");
const { GatewayLink } = await import("../src/lib/gateway/link.js");
const { startGatewayShell } = await import("../src/features/gateway/gateway-shell.js");
const { stopGatewayShell } = await import("../src/features/machines/shells.js");
const { crossDeviceZhCN } = await import("../src/i18n/cross-device.js");
const { linkZhCN } = await import("../src/i18n/link.js");
const { zhCN } = await import("../src/i18n/zh-CN.js");
const { cloudDevicesZhCN } = await import("../src/i18n/cloud-devices.js");
type MachineInfo = import("../src/lib/machines/runtime.js").MachineInfo;

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: { ...linkZhCN, ...cloudDevicesZhCN, ...crossDeviceZhCN, ...zhCN } } },
  keySeparator: false,
  nsSeparator: false
});

const now = Date.now();
const ago = (m: number) => new Date(now - m * 60_000).toISOString();
const row = (id: string, minutes: number, extra: Record<string, unknown> = {}) => ({
  source: "claude",
  id,
  title: `title ${id}`,
  cwd: `/work/${id}`,
  updatedAt: ago(minutes),
  createdAt: ago(minutes + 10),
  attentionAt: ago(minutes),
  activity: "idle",
  status: "idle",
  asking: false,
  scope: "owned",
  unreadCount: 0,
  unread: false,
  resident: null,
  ...extra,
});

/** This device's server, played from a copy: its feed lists `rows`. */
function localFrom(rows: ReturnType<typeof row>[]) {
  const source = {
    whenLoaded: async () => undefined,
    rows: () => rows as never,
    row: (s: string, id: string) => rows.find((r) => r.source === s && r.id === id) as never,
    status: { state: { asOf: 1, settings: { hostname: "this-one", homeDir: "/work", serverStartedAt: "2026-10-19T00:00:00Z" }, accounts: null, quota: null, cli: null, mkEpoch: null } },
    session: async () => null,
    sessionNow: () => null,
    touch: () => undefined,
    onChange: () => () => undefined,
  };
  registry.openMachine(LOCAL, offlineTransport(source as never, { user: "alice", messages: { readOnly: "ro", noCopy: "nc", unavailable: "na" }, machineOnline: () => true }));
  registry.setShell("local", LOCAL);
}

const DEVICES = [
  { id: "m-studio", name: "studio", state: "online", sinceMs: 1, copyAsOfMs: now - 60_000, os: null, agentVersion: null },
  { id: "m-laptop", name: "laptop", state: "offline", sinceMs: 1, copyAsOfMs: Date.parse("2026-10-18T05:02:00Z"), os: null, agentVersion: null },
];

/** The gateway the test plays: the directory, and each device's index as asked for. */
function gateway(indexes: Record<string, ReturnType<typeof row>[]>) {
  const sockets: { send: (data: string) => void }[] = [];
  const link = new GatewayLink({
    socketUrl: async () => "ws://x/api/agg/ws",
    openSocket: () => {
      const socket = {
        readyState: 0,
        binaryType: "blob",
        onopen: null as ((e: unknown) => void) | null,
        onmessage: null as ((e: { data: unknown }) => void) | null,
        onclose: null as ((e: unknown) => void) | null,
        send(data: string) {
          const m = JSON.parse(data) as { t: string; s: number; machine: string; name: { kind: string } };
          if (m.t !== "sub" || m.name.kind !== "index") return;
          const rows = indexes[m.machine] ?? [];
          queueMicrotask(() =>
            socket.onmessage?.({ data: JSON.stringify({ t: "copy", s: m.s, reset: true, bodies: [{ reset: true, upserts: rows, removed: [] }], headSeq: 1, headTimeMs: now - 60_000 }) }),
          );
        },
        close() {},
      };
      sockets.push(socket);
      setTimeout(() => {
        socket.readyState = 1;
        socket.onopen?.({});
        socket.onmessage?.({ data: JSON.stringify({ t: "dir", machines: DEVICES }) });
      }, 0);
      return socket;
    },
  });
  startGatewayShell(link, { user: "alice" });
}

function reset() {
  stopGatewayShell();
  registry.setShell("local", LOCAL);
  for (const key of registry.openMachines()) if (key !== LOCAL) registry.closeMachine(key);
  localStorage.clear();
}

/** The sessions page as the app routes it (`/sessions`, its index where no session is open). */
function mount(lng = "en") {
  void i18n.changeLanguage(lng);
  const errors: unknown[] = [];
  const root = createRoot(container as never, {
    onUncaughtError: (error) => void errors.push(error),
    onCaughtError: (error) => void errors.push(error),
  });
  const rootRoute = router.createRootRoute({ component: () => <router.Outlet /> });
  const sessions = router.createRoute({ getParentRoute: () => rootRoute, path: "/sessions", component: SessionsLayout });
  const index = router.createRoute({ getParentRoute: () => sessions, path: "/", component: SessionsIndex });
  const app = router.createRouter({
    routeTree: rootRoute.addChildren([sessions.addChildren([index])]),
    history: router.createMemoryHistory({ initialEntries: ["/sessions"] }),
  });
  root.render(
    <I18nextProvider i18n={i18n}>
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MachineProvider machine={LOCAL}>
          <GlobalFeedProvider>
            <router.RouterProvider router={app} />
          </GlobalFeedProvider>
        </MachineProvider>
      </QueryClientProvider>
    </I18nextProvider>,
  );
  return { root, errors, loaded: app.load(), text: () => (container as FakeNode).textContent, find: (attr: string) => findAll(container, attr) };
}

test("the local app alone: no device bar, no ledger — the list as it always was", async (t) => {
  reset();
  t.after(reset);
  localFrom([row("a", 1), row("b", 5)]);
  const { root, errors, find, text, loaded } = mount();
  t.after(() => root.unmount());
  await loaded;
  await flush(60);
  assert.deepEqual(errors, []);
  assert.equal(find("data-testid").filter((n) => n.getAttribute("data-testid") === "device-filter").length, 0);
  assert.equal(find("data-row-device").length, 0);
  assert.match(text(), /pick one on the left/);
  assert.doesNotMatch(text(), /needs you · every device/);
});

test("with the gateway's devices: the bar says each device and how current an offline one's copy is, and the ledger puts the question first", async (t) => {
  reset();
  t.after(reset);
  localFrom([row("a", 1), row("u", 3, { unreadCount: 2, unread: true })]);
  gateway({ "m-studio": [row("q", 2, { asking: true, activity: "idle", status: "idle", title: "ship it?" }), row("s", 30)], "m-laptop": [row("l", 600, { asking: true })] });
  const { root, errors, find, text, loaded } = mount("zh-CN");
  t.after(() => root.unmount());
  await loaded;
  await flush(100);
  assert.deepEqual(errors, []);
  await until("the device bar", () => find("data-testid").some((n) => n.getAttribute("data-testid") === "device-filter"));
  const bar = find("data-testid").find((n) => n.getAttribute("data-testid") === "device-filter");
  assert.ok(bar, "the device bar is there");
  const choices = find("data-device-choice").map((n) => n.getAttribute("data-device-choice"));
  assert.deepEqual(choices, ["needs", "all", LOCAL, "m-studio", "m-laptop"]);
  assert.match(bar.textContent, /需要你/);
  assert.match(bar.textContent, /全部/);
  assert.match(bar.textContent, /本设备/);
  assert.match(bar.textContent, /studio/);
  // How current the offline device's copy is, in the app's language: today, the time; in full, the day too.
  assert.match(bar.textContent, /laptop截至 \d{2}:\d{2}/, "the offline device: how current its copy is");
  assert.match(find("data-device-choice").find((n) => n.getAttribute("data-device-choice") === "m-laptop")!.getAttribute("title") ?? "", /离线副本 · 截至 \d{1,2}月\d{1,2}日 \d{2}:\d{2}/);
  const laptop = find("data-device-choice").find((n) => n.getAttribute("data-device-choice") === "m-laptop")!;
  assert.equal(laptop.getAttribute("data-device-state"), "offline");
  // The ledger: the question on the online device first; the offline one's is at rest, not asking.
  const ledger = find("data-testid").find((n) => n.getAttribute("data-testid") === "needs-you-ledger");
  assert.ok(ledger, "the ledger where no session is open");
  const rows = find("data-session-key").map((n) => [n.getAttribute("data-session-key"), n.getAttribute("data-kind")]);
  assert.deepEqual(rows[0], ["m-studio|claude/q", "asking"]);
  // Asking on another device: opened to be read here, answered there (until S2).
  const asking = find("data-session-key").find((n) => n.getAttribute("data-session-key") === "m-studio|claude/q")!;
  assert.match(asking.textContent, /要在那台设备上回答/);
  assert.match(asking.textContent, /打开/);
  assert.doesNotMatch(asking.textContent, /去回答/);
  assert.ok(!rows.some(([key, kind]) => key === "m-laptop|claude/l" && kind === "asking"), "an offline device asks nothing");
  assert.ok(rows.some(([key, kind]) => key === "local|claude/u" && kind === "unread"));
  assert.match(text(), /需要你 · 所有设备/);
});

test("one other device chosen: its copy's time and that it is read-only here", async (t) => {
  reset();
  t.after(reset);
  localFrom([row("a", 1)]);
  // Chosen on this page: kept for it, not written down (nothing of another device is).
  saveDeviceChoice("m-laptop");
  assert.equal(localStorage.getItem("rh.sessions.device"), "all");
  t.after(() => saveDeviceChoice("all"));
  gateway({ "m-studio": [row("s", 30)], "m-laptop": [row("l", 600)] });
  const { root, errors, find, loaded } = mount();
  t.after(() => root.unmount());
  await loaded;
  await flush(100);
  assert.deepEqual(errors, []);
  const note = find("data-testid").find((n) => n.getAttribute("data-testid") === "device-filter-note");
  assert.ok(note);
  assert.match(note.textContent, /offline copy · as of/);
  assert.match(note.textContent, /read-only here: these sessions are on another device/);
  const chosen = find("data-device-choice").find((n) => n.getAttribute("data-device-choice") === "m-laptop")!;
  assert.equal(chosen.getAttribute("aria-pressed"), "true", "the laptop is the one chosen");
});

test("「截至」 says the moment in the app's language, not the browser's", () => {
  const at = new Date(2026, 9, 18, 14, 2).getTime();
  const later = new Date(2026, 9, 19, 9, 0).getTime();
  const sameDay = new Date(2026, 9, 18, 18, 0).getTime();
  assert.equal(formatMoment(at, "zh-CN", true, later), "10月18日 14:02");
  assert.equal(formatMoment(at, "zh-CN", true, sameDay), "14:02");
  assert.equal(formatMoment(at, "zh-CN", false, sameDay), "10月18日 14:02");
  assert.equal(formatMoment(at, "ja", true, later), "10月18日 14:02");
  assert.equal(formatMoment(at, "en", true, later), "Oct 18, 14:02");
  const info: MachineInfo = { key: "m-x", name: "x", state: "offline", since: 0, canManage: false, live: "copy", copyAsOf: at };
  const tz = (key: string, opts?: Record<string, unknown>) => i18n.getFixedT("zh-CN")(key, opts);
  assert.equal(chipNote(info, tz, "zh-CN", later), "截至 10月18日 14:02");
});

test("a device shown read-only is never offered where something starts or is kept", async (t) => {
  reset();
  t.after(reset);
  localFrom([row("a", 1)]);
  gateway({ "m-studio": [row("s", 30)], "m-laptop": [row("l", 600)] });
  await flush(100);
  assert.deepEqual(registry.openMachines(), [LOCAL, "m-studio", "m-laptop"]);
  const draw = (writable: boolean) =>
    renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <MachineProvider machine={LOCAL}>
          <DevicePicker ghost forStarting writable={writable} value={LOCAL} onSelect={() => undefined} />
        </MachineProvider>
      </I18nextProvider>,
    );
  // The new-session view's device chip, and a pane's: only this device is left — no chip at all.
  assert.equal(draw(true), "");
  assert.equal(readOnlyHere("m-studio"), true);
  assert.equal(readOnlyHere(LOCAL), false);
  // (A picker that may show them, read-only, still does.)
  assert.match(draw(false), /data-testid="device-picker"/);
});

test("the bar on its own: no 「需要你」 while nothing needs you", () => {
  const info = (key: string, state: MachineInfo["state"], copyAsOf?: number): MachineInfo => ({ key, name: key, state, since: 0, canManage: false, live: "copy", ...(copyAsOf ? { copyAsOf } : {}) });
  void i18n.changeLanguage("en");
  const html = renderToStaticMarkup(
    <I18nextProvider i18n={i18n}>
      <DeviceFilterBar value="all" onChange={() => undefined} devices={[info(LOCAL, "online"), info("m-x", "unknown", now - 1000)]} needs={0} />
    </I18nextProvider>,
  );
  assert.doesNotMatch(html, /data-device-choice="needs"/);
  assert.match(html, /this device/);
  assert.match(html, /m-x/);
  assert.match(html, /as of/, "a device not known to be online says how current its copy is");
});

test("who sees the other devices here: switches for the owner, a sentence for anyone else, a claim where there is no owner", () => {
  const status = (over: Record<string, unknown>) =>
    ({
      state: "linked",
      localAuth: true,
      canManage: true,
      canClaim: false,
      owner: "alice",
      seesOtherDevices: true,
      accounts: [
        { user: "alice", remote: true, owner: true },
        { user: "bob", remote: false, owner: false },
        { user: "carol", remote: true, owner: false },
      ],
      ...over,
    }) as never;
  void i18n.changeLanguage("zh-CN");
  const draw = (s: never) =>
    renderToStaticMarkup(
      <I18nextProvider i18n={i18n}>
        <RemoteAccess status={s} busy={false} onSet={() => undefined} />
      </I18nextProvider>,
    );
  const manager = draw(status({}));
  assert.match(manager, /data-remote-user="alice"[\s\S]*登录了这台设备：总能看到/);
  assert.doesNotMatch(manager, /droppen-claim/);
  assert.match(manager, /aria-checked="false"[^>]*data-testid="droppen-remote-bob"/);
  assert.match(manager, /aria-checked="true"[^>]*data-testid="droppen-remote-carol"/);
  const other = draw(status({ canManage: false, seesOtherDevices: false, accounts: [] }));
  assert.match(other, /data-remote="off"/);
  assert.match(other, /让这台设备登录的 alice 可以在这里允许它/);
  // No owner (it joined from the command line): nobody sees them; whoever manages the link may say it added it.
  const ownerless = draw(status({ owner: null, canClaim: true, seesOtherDevices: false }));
  assert.match(ownerless, /data-testid="droppen-claim"[^>]*>这是我加入的</);
  assert.match(ownerless, /目前谁也不能在这里看到你的其它设备/);
  assert.doesNotMatch(ownerless, /droppen-remote-bob/, "no switches until it is claimed");
  const nobody = draw(status({ owner: null, canManage: false, seesOtherDevices: false, accounts: [] }));
  assert.match(nobody, /在加入这台设备的人在这里确认之前/);
  void i18n.changeLanguage("en");
});

test("over the network without encryption, the page says so beside who sees the other devices", () => {
  assert.equal(unencryptedOverNetwork({ protocol: "http:", hostname: "192.168.1.145" }), true);
  assert.equal(unencryptedOverNetwork({ protocol: "http:", hostname: "studio.local" }), true);
  assert.equal(unencryptedOverNetwork({ protocol: "http:", hostname: "localhost" }), false);
  assert.equal(unencryptedOverNetwork({ protocol: "http:", hostname: "127.0.0.1" }), false);
  assert.equal(unencryptedOverNetwork({ protocol: "http:", hostname: "[::1]" }), false);
  assert.equal(unencryptedOverNetwork({ protocol: "https:", hostname: "192.168.1.145" }), false);
  assert.equal(unencryptedOverNetwork(null), false);
});
