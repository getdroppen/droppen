// The cross-device "needs you" (S3-2; `docs/local-aggregator.md` §6.4):
// on droppen.app under 「我的设备」's list, and each device's row there with
// its sessions at a glance; in the local app the sessions index.
//
// What these pin: what needs you is worked out from every device's session
// list as the page already has it — asking first, then unread, then working,
// newest first in each, only the rows the list shows — and the tab's unread
// number is the same sum the single-machine page made (`totalUnread`) when
// there is only `local`; it stays cheap at 2,000 rows. On droppen.app those
// lists are the machines' copies (`HubMachine.sessions`): a question on a
// machine out of view reaches the top of the ledger without dialling any
// machine and without subscribing to anything beyond each machine's index and
// status; opening it brings that machine into view and only then dials it.
// The ledger draws what it is given, the same on droppen.app and locally; a
// device's row says its sessions; the badge shows the count; every string is
// translated.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { renderToStaticMarkup } from "react-dom/server";
import i18next from "i18next";
import { I18nextProvider, initReactI18next } from "react-i18next";
import { QueryClient } from "@tanstack/react-query";
import type { components } from "../src/api/schema.js";
import { FMT_INDEX_V1, FMT_STATUS_V1, hex, indexStream, MemoryKeyStore, statusStream } from "../src/lib/e2e/index.js";
import type { PresenceState } from "../src/lib/hub/link.js";
import type { MachineStatus } from "../src/lib/hub/machine.js";
import { homeMachine } from "../src/lib/machines/registry.js";
import { HubShell, sessionPath } from "../src/features/cloud/hub-shell.js";
import { hubShell, setHubShell } from "../src/features/machines/shells.js";
import { totalUnread, UNREAD_CAP } from "../src/features/sessions/session-attention.js";
import { copySessions } from "../src/features/machines/copy-sessions.js";
import { constantStore } from "../src/lib/machines/store.js";
import { useDeviceSessions } from "../src/features/machines/device-sessions.js";
import { agentVersionOf, osLabel, systemLabel, versionLabel } from "../src/features/machines/device-facts.js";
import { type StatusState, statusFold } from "../src/lib/hub/streams.js";
import { DeviceRow, type DeviceRowView, rowMenuItems } from "../src/features/cloud/device-row.js";
import { type DeviceSessions, ledgerKind, needsYou, recentSessions, unreadAcross } from "../src/features/machines/needs-you.js";
import { NeedsYouLedger } from "../src/features/machines/needs-you-ledger.js";
import { BADGE_CAP, setAppBadgeCount } from "../src/lib/badge.js";
import { cloudDevicesJa, cloudDevicesZhCN, cloudDevicesZhTW } from "../src/i18n/cloud-devices.js";
import { crossDeviceJa, crossDeviceZhCN, crossDeviceZhTW } from "../src/i18n/cross-device.js";
import { ja } from "../src/i18n/ja.js";
import { zhCN } from "../src/i18n/zh-CN.js";
import { zhTW } from "../src/i18n/zh-TW.js";
import { FakeKv, FakeStreamLink, flush, type Machine, makeMachine, StreamWriter, until } from "./stream-fixtures.js";

type SessionSummary = components["schemas"]["SessionSummary"];

const i18n = i18next.createInstance();
await i18n.use(initReactI18next).init({
  lng: "en",
  resources: { en: { translation: {} }, "zh-CN": { translation: { ...cloudDevicesZhCN, ...zhCN } } },
  interpolation: { escapeValue: false },
  keySeparator: false,
  nsSeparator: false,
});

function draw(node: React.ReactNode, lng = "en"): string {
  void i18n.changeLanguage(lng);
  return renderToStaticMarkup(<I18nextProvider i18n={i18n}>{node}</I18nextProvider>);
}

const NOW = Date.parse("2026-10-18T12:00:00.000Z");
const at = (minutes: number) => new Date(NOW - minutes * 60_000).toISOString();

type Kind = "asking" | "working" | "queued" | "background" | "idle" | number;

/** A session row as the index and the feed carry it. */
function row(id: string, kind: Kind, minutes: number, over: Partial<SessionSummary> = {}): SessionSummary {
  const base = {
    id,
    source: "claude",
    title: `session ${id}`,
    status: "idle",
    activity: "idle",
    asking: false,
    attentionAt: at(minutes),
    createdAt: at(minutes + 60),
    updatedAt: at(minutes),
    cwd: "/home/demo/project",
    filePath: `/home/demo/${id}.jsonl`,
    gitBranch: null,
    repoKey: null,
    resident: null,
    scope: "owned",
    sizeBytes: 1,
    unread: false,
    unreadCount: 0,
  } as unknown as SessionSummary;
  const resident = (o: Record<string, unknown>) => ({ asking: false, background: 0, phase: "working", queued: 0, queuedMessages: [], stalled: false, tasks: [], ...o });
  const shaped: Record<string, unknown> =
    kind === "asking"
      ? { activity: "working", status: "running", asking: true, resident: resident({ asking: true }) }
      : kind === "working"
        ? { activity: "working", status: "running", resident: resident({ turnStartedAt: at(minutes + 5) }) }
        : kind === "queued"
          ? { activity: "queued", status: "running", resident: resident({ queued: 1 }) }
          : kind === "background"
            ? { activity: "background", status: "waiting", resident: resident({ phase: "idle", background: 2 }) }
            : typeof kind === "number"
              ? { unread: true, unreadCount: kind }
              : {};
  return { ...base, ...shaped, ...over } as SessionSummary;
}

const list = (machine: string, rows: SessionSummary[] | null, from: DeviceSessions["from"] = "copy"): DeviceSessions => ({ machine, rows, from, asOf: from === "copy" ? NOW : null });

// ---------------------------------------------------------------------------
// The selectors
// ---------------------------------------------------------------------------

test("what needs you: asking first, then unread, then working, newest first in each; idle, background and external rows are not in it", () => {
  const a = list("m-a", [
    row("a-idle", "idle", 1),
    row("a-work-old", "working", 40),
    row("a-unread", 2, 30),
    row("a-ask", "asking", 50),
    row("a-bg", "background", 2),
    row("a-ext", 9, 1, { scope: "external" } as Partial<SessionSummary>),
  ]);
  const b = list("m-b", [row("b-ask", "asking", 5), row("b-queued", "queued", 3), row("b-unread", 1, 10), row("b-work-unread", "working", 20, { unreadCount: 4 } as Partial<SessionSummary>)]);
  const needs = needsYou([a, b]);
  assert.deepEqual(
    needs.entries.map((e) => `${e.kind}:${e.ref.id}`),
    ["asking:b-ask", "asking:a-ask", "unread:b-unread", "unread:b-work-unread", "unread:a-unread", "working:b-queued", "working:a-work-old"],
  );
  assert.deepEqual(needs.entries[0]!.ref, { machine: "m-b", source: "claude", id: "b-ask" }, "every entry says which device it is on");
  assert.equal(needs.entries[0]!.key, "m-b|claude/b-ask");
  assert.equal(needs.asking, 2);
  assert.equal(needs.unread, 2 + 1 + 4, "the external row's 9 does not count, as in the tab before");
  assert.equal(needs.working, 3, "a working session holding unread answers is still working");
  assert.deepEqual({ ...needs.byMachine.get("m-a") }, { working: 1, asking: 1, unread: 2, strongest: "asking", sessions: 5 });
  assert.equal(ledgerKind(row("x", "background", 1)), null);
  assert.equal(ledgerKind(row("x", "idle", 1)), null);
});

test("with only `local`, the tab's number is the one the single-machine page showed", () => {
  const rows = [row("1", 3, 1), row("2", "working", 2, { unreadCount: 2 } as Partial<SessionSummary>), row("3", 5, 3, { scope: "external" } as Partial<SessionSummary>), row("4", "idle", 4)];
  assert.equal(unreadAcross([list("local", rows, "live")]), totalUnread(rows));
  assert.equal(unreadAcross([list("local", null, "live")]), 0, "before the first snapshot");
  assert.equal(unreadAcross([]), 0);
  assert.equal(needsYou([list("local", rows, "live")]).unread, totalUnread(rows));
});

test("unread sums across devices, and a device's share is worked out once per list", () => {
  const rows = [row("1", 2, 1)];
  const lists = [list("m-a", rows), list("m-b", [row("2", 5, 1)]), list("m-c", null)];
  assert.equal(unreadAcross(lists), 7);
  const first = needsYou(lists);
  const again = needsYou(lists);
  assert.equal(first.entries[0], again.entries.find((e) => e.key === first.entries[0]!.key), "the same entry objects for the same rows");
  assert.equal(first.byMachine.get("m-c")!.sessions, 0, "a device with no list yet has nothing");
});

test("2,000 rows across 20 devices stay cheap", () => {
  const kinds: Kind[] = ["asking", "working", 3, "idle", "background", "idle", "idle", 1, "queued", "idle"];
  const lists = Array.from({ length: 20 }, (_, d) => list(`m-${d}`, Array.from({ length: 100 }, (_, i) => row(`${d}-${i}`, kinds[i % kinds.length]!, i + d))));
  const started = performance.now();
  const needs = needsYou(lists);
  const cold = performance.now() - started;
  assert.equal(needs.entries.length, 20 * 50);
  assert.equal(needs.byMachine.size, 20);
  // One device changes: only its share is worked out again.
  lists[3] = list("m-3", [...lists[3]!.rows!]);
  const t2 = performance.now();
  needsYou(lists);
  const warm = performance.now() - t2;
  assert.ok(cold < 500, `cold ${cold} ms`);
  assert.ok(warm < 200, `warm ${warm} ms`);
});

test("a device's latest sessions put the ones asking you first", () => {
  const rows = [row("new", "idle", 1), row("ask", "asking", 90), row("mid", 2, 30), row("ext", "idle", 0, { scope: "external" } as Partial<SessionSummary>)];
  assert.deepEqual(recentSessions(rows, 3).map((r) => r.id), ["ask", "new", "mid"]);
  assert.deepEqual(recentSessions(null, 3), []);
});

test("a copy's session list is a store replaced when its index changes, and only then", async () => {
  let rows: SessionSummary[] | null = null;
  const listeners = new Set<(c: { kind: string }) => void>();
  const copy = { rows: () => rows, freshestMs: null as number | null, loaded: false, onChange: (fn: (c: { kind: string }) => void) => (listeners.add(fn), () => listeners.delete(fn)), whenLoaded: () => Promise.resolve() };
  const store = copySessions("m-1", copy);
  let heard = 0;
  store.subscribe(() => heard++);
  assert.deepEqual(store.get(), { machine: "m-1", rows: null, from: "copy", asOf: null, pending: true }, "not read off the device yet");
  rows = [row("1", 1, 1)];
  const at = Date.now();
  copy.freshestMs = at;
  copy.loaded = true;
  for (const fn of listeners) fn({ kind: "status" });
  assert.equal(heard, 0, "a status record is not the list");
  for (const fn of listeners) fn({ kind: "index" });
  assert.equal(heard, 1);
  assert.deepEqual(store.get(), { machine: "m-1", rows, from: "copy", asOf: at });
  const before = store.get();
  assert.equal(store.get(), before, "the same value until it changes");
  store.close();
  for (const fn of listeners) fn({ kind: "index" });
  assert.equal(heard, 1, "closed: deaf");
});

// ---------------------------------------------------------------------------
// Drawing: the same components locally and on droppen.app
// ---------------------------------------------------------------------------

test("the ledger draws asking first with a way to answer, says which device each row is on, and leaves the device out with only one", () => {
  const needs = needsYou([list("m-a", [row("a-unread", 2, 30)]), list("m-b", [row("b-ask", "asking", 5, { title: "which changelog range?" } as Partial<SessionSummary>)])]);
  const names: Record<string, string> = { "m-a": "studio", "m-b": "gpu-node" };
  const html = draw(<NeedsYouLedger needs={needs} nameOf={(m) => names[m] ?? m} onOpen={() => {}} />);
  const ask = html.indexOf("which changelog range?");
  const unread = html.indexOf("session a-unread");
  assert.ok(ask > 0 && unread > ask, "asking above unread");
  assert.match(html, /data-kind="asking" data-session-key="m-b\|claude\/b-ask"/);
  assert.ok(html.includes(">gpu-node<") && html.includes(">studio<"));
  assert.ok(html.includes(">answer<"));
  assert.ok(html.includes("asking you 1 · unread 2"));
  // One "working": a working session holding unread answers is listed with the unread, its working chip on it, and counted as working.
  const busy = needsYou([list("m-a", [row("w-unread", "working", 3, { unreadCount: 2 } as Partial<SessionSummary>)])]);
  const busyHtml = draw(<NeedsYouLedger needs={busy} nameOf={(m) => m} onOpen={() => {}} />);
  assert.equal(busy.working, 1);
  assert.ok(busyHtml.includes("unread 2 · working 1"), "the header says the summary's working count");
  assert.match(busyHtml, /data-kind="unread"[\s\S]*data-activity="working"/);
  const local = draw(<NeedsYouLedger needs={needsYou([list("local", [row("x", 1, 1)], "live")])} nameOf={(m) => m} onOpen={() => {}} devices={false} />);
  assert.ok(!local.includes(">local<"), "one device: no device column");
  assert.match(draw(<NeedsYouLedger needs={needsYou([])} nameOf={(m) => m} onOpen={() => {}} />), /data-testid="needs-you-empty"/);
  assert.ok(draw(<NeedsYouLedger needs={needs} nameOf={(m) => names[m] ?? m} onOpen={() => {}} />, "zh-CN").includes("需要你 · 所有设备"));
});

test("a device that runs AI says on its row what it is and how many of its sessions need you; offline it offers the copy, and its terminal and rename say why not", () => {
  const attention = { working: 1, asking: 1, unread: 3, strongest: "asking" as const, sessions: 14 };
  const view = (presence: PresenceState, over: Partial<DeviceRowView> = {}): DeviceRowView => ({
    id: "m-7",
    fingerprint: "ABCD-EFGH-IJKL-MNOP",
    name: "gpu-node",
    self: false,
    kind: "computer",
    status: "active",
    host: true,
    stoppedHost: false,
    presence,
    path: null,
    copyAsOf: presence === "online" ? null : NOW - 3 * 3600_000,
    lastSeenMs: presence === "online" ? null : NOW - 3 * 3600_000,
    limited: null,
    accept: { type: "admins" },
    facts: { os: osLabel("linux"), agent: versionLabel("0.2.112+g930dd8f"), agentFull: "0.2.112+g930dd8f", hostname: "gpu-node.local", keyEpoch: 2 },
    attention,
    recent: [row("r1", "asking", 1)],
    hasCopy: true,
    ...over,
  });
  const drawRow = (v: DeviceRowView, lng = "en") => draw(<ul><DeviceRow view={v} viewerAdmin upgraded writable onOpen={() => {}} onTerminal={() => {}} onRename={() => {}} /></ul>, lng);
  const online = drawRow(view("online"));
  assert.match(online, /data-testid="open-m-7"[\s\S]*?>open</, "the button the acceptance harness clicks");
  assert.ok(online.includes("m-7 · Linux") && online.includes(">v0.2.112<") && online.includes(">gpu-node.local<"), "system on the id line, agent and hostname in the details");
  assert.ok(online.includes(">14 sessions<"));
  assert.match(online, /data-activity="working"/);
  assert.match(online, /data-unread="3"/);
  assert.match(online, /data-testid="recent-sessions-m-7"/);
  assert.ok(!online.includes("copy-as-of"), "online: the machine itself, no copy to date");
  const t = i18n.getFixedT("en");
  const menu = (v: DeviceRowView) => rowMenuItems(v, { viewerAdmin: true, upgraded: true, writable: true, t, onTerminal: () => {}, onRename: () => {} });
  assert.deepEqual(menu(view("online")).filter((i) => i.blocked).map((i) => i.id), [], "online: its terminal and rename go");
  const offline = drawRow(view("offline"));
  assert.match(offline, /data-testid="copy-as-of"/);
  assert.ok(offline.includes("offline copy · as of "));
  assert.match(offline, /data-testid="open-m-7"[\s\S]*?>read the copy</);
  assert.deepEqual(
    menu(view("offline")).filter((i) => i.blocked).map((i) => [i.id, i.blocked]),
    [
      ["terminal-m-7", "the device is offline — this needs it to be online"],
      ["rename-m-7", "the device is offline — this needs it to be online"],
    ],
  );
  assert.ok(drawRow(view("offline"), "zh-CN").includes("截至 "), "「截至 …」");
  const none = drawRow(view("unknown", { attention: null, hasCopy: false, copyAsOf: null }));
  assert.ok(none.includes("no copy on this device yet"));
  assert.equal(osLabel("darwin"), "macOS");
  assert.equal(versionLabel("0.2.112+g930dd8f"), "v0.2.112");
  assert.equal(versionLabel(undefined), null);
  // The system with its version and architecture, as the status stream gives it (#108).
  const full = drawRow(view("online", { facts: { os: osLabel("macOS 15.3.1 aarch64"), agent: versionLabel("0.2.131"), agentFull: "0.2.131", hostname: null, keyEpoch: null } }));
  assert.ok(full.includes("m-7 · macOS 15.3.1 aarch64") && full.includes(">macOS 15.3.1 aarch64<") && full.includes(">v0.2.131<"), "on the id line and in the details");
});

test("a device's system and agent version read off its status as the local page's directory reads them, an older agent's settings standing in", () => {
  const status = (body: Record<string, unknown>): StatusState => statusFold.apply(statusFold.empty(), { asOf: 1, ...body });
  const settings = { platform: "darwin", serverVersion: "0.2.120" };
  // The same cases as the local server's directory (`server-rs/src/aggregator/live.rs`, `os_of`).
  assert.equal(systemLabel(status({ os: { name: "macos", version: "15.3.1", arch: "aarch64" }, settings })), "macOS 15.3.1 aarch64");
  assert.equal(systemLabel(status({ os: { name: "linux", version: "24.04", distro: "ubuntu", arch: "x86_64" } })), "ubuntu 24.04 x86_64", "a Linux by its distribution");
  assert.equal(systemLabel(status({ os: { name: "linux", version: null, arch: "aarch64" } })), "Linux aarch64", "no os-release: Linux, and what is known");
  assert.equal(systemLabel(status({ os: { name: "windows", version: null, arch: "x86_64" } })), "Windows x86_64");
  assert.equal(systemLabel(status({ os: { name: "freebsd", version: "", arch: "x86_64" } })), "freebsd x86_64", "another system by the agent's name for it");
  // An agent from before S1-5, or an `os` that is not one: the settings' platform.
  assert.equal(systemLabel(status({ settings })), "darwin");
  assert.equal(osLabel(systemLabel(status({ settings }))), "macOS", "named on the row as before");
  assert.equal(systemLabel(status({ os: { version: "15.1" }, settings })), "darwin");
  assert.equal(systemLabel(status({ os: "macos", settings: { platform: "linux" } })), "linux");
  assert.equal(systemLabel(status({})), null, "nothing said: no line");
  assert.equal(systemLabel(null), null);
  assert.equal(agentVersionOf(status({ agentVersion: "0.2.131", settings })), "0.2.131");
  assert.equal(agentVersionOf(status({ agentVersion: " ", settings })), "0.2.120", "blank is not a version");
  assert.equal(agentVersionOf(status({ agentVersion: 131, settings })), "0.2.120");
  assert.equal(agentVersionOf(status({ settings })), "0.2.120", "an agent from before");
  assert.equal(agentVersionOf(status({})), null);
  // A copy saved before the page kept the fields reads as an older agent's.
  assert.equal(systemLabel({ asOf: 1, settings, accounts: null, quota: null, cli: null, mkEpoch: null }), "darwin");
});

test("the app badge shows the unread count and clears at none", () => {
  const calls: string[] = [];
  const nav = {
    setAppBadge: (n?: number) => (calls.push(`set ${n}`), Promise.resolve()),
    clearAppBadge: () => (calls.push("clear"), Promise.resolve()),
  };
  const before = Object.getOwnPropertyDescriptor(globalThis, "navigator");
  Object.defineProperty(globalThis, "navigator", { value: nav, configurable: true });
  try {
    setAppBadgeCount(3);
    setAppBadgeCount(3);
    setAppBadgeCount(5);
    setAppBadgeCount(150);
    setAppBadgeCount(120);
    setAppBadgeCount(0);
  } finally {
    if (before) Object.defineProperty(globalThis, "navigator", before);
  }
  assert.deepEqual(calls, ["set 3", "set 5", "set 99", "clear"], "capped as the title's 99+ is, and nothing new to say at 120");
  assert.equal(BADGE_CAP, UNREAD_CAP);
});

test("every string of the ledger and a device's row is translated into zh-CN, zh-TW and ja", () => {
  const files = [
    "features/machines/needs-you-ledger.tsx",
    "features/cloud/device-row.tsx",
    "features/sessions/attention-badges.tsx",
  ].map((f) => readFileSync(new URL(`../src/${f}`, import.meta.url), "utf8"));
  const keys = new Set<string>();
  for (const src of files) for (const m of src.matchAll(/\bt\(\s*"((?:[^"\\]|\\.)*)"/g)) keys.add(m[1]!);
  assert.ok(keys.size > 30);
  for (const [name, table] of [
    ["zh-CN", { ...crossDeviceZhCN, ...cloudDevicesZhCN, ...zhCN }],
    ["zh-TW", { ...crossDeviceZhTW, ...cloudDevicesZhTW, ...zhTW }],
    ["ja", { ...crossDeviceJa, ...cloudDevicesJa, ...ja }],
  ] as const) {
    const missing = [...keys].filter((k) => !(k in table));
    assert.deepEqual(missing, [], `${name} lacks these`);
  }
});

// ---------------------------------------------------------------------------
// The lists the title and the dashboard read (`useDeviceSessions`)
// ---------------------------------------------------------------------------

/** A shell of `lists`, one per device, each its copy's store, in that order. */
function listShell(lists: DeviceSessions[]) {
  const infos = lists.map((l) => ({ key: l.machine, name: l.machine, state: "online" as const, since: 0, canManage: true, live: "copy" as const }));
  const machines = constantStore(infos);
  const held = new Map(lists.map((l) => [l.machine, { sessions: constantStore(l) }]));
  return { machines: () => machines, machine: (key: string) => held.get(key) ?? null } as never;
}

/** What `useDeviceSessions(live)` gives, rendered once. */
function listsWith(live: DeviceSessions | null): readonly DeviceSessions[] {
  let seen: readonly DeviceSessions[] = [];
  function Probe() {
    seen = useDeviceSessions(live);
    return null;
  }
  renderToStaticMarkup(<Probe />);
  return seen;
}

test("the device the app is live on counts from its feed instead of its copy — never both — and from its copy until the feed has rows", (t) => {
  const home = list("m-home", [row("h1", 2, 1)]);
  const other = list("m-other", [row("o1", 5, 1)]);
  setHubShell(listShell([home, other]));
  t.after(() => setHubShell(null));
  const live = list("m-home", [row("h1", 3, 0)], "live");
  const withLive = listsWith(live);
  assert.deepEqual(withLive.map((l) => `${l.machine}:${l.from}`), ["m-home:live", "m-other:copy"]);
  assert.equal(unreadAcross(withLive), 3 + 5, "the feed's 3 replaces the copy's 2");
  // The app has just come up on a new home: its feed has no snapshot yet.
  const early = listsWith(list("m-home", null, "live"));
  assert.deepEqual(early.map((l) => `${l.machine}:${l.from}`), ["m-home:copy", "m-other:copy"]);
  assert.equal(unreadAcross(early), 2 + 5, "no dip to 5 while the feed starts");
  assert.equal(unreadAcross(listsWith(null)), 2 + 5, "the dashboard: every device off its copy");
});

test("only devices that are up count in the title and the badge; the dashboard keeps everyone's unread", (t) => {
  const up = list("m-up", [row("u1", 2, 1)]);
  const away: DeviceSessions = { ...list("m-away", [row("a1", 4, 1)]), away: true };
  setHubShell(listShell([up, away]));
  t.after(() => setHubShell(null));
  const lists = listsWith(null);
  assert.equal(unreadAcross(lists), 2, "a shut laptop does not hold the badge");
  assert.equal(needsYou(lists).unread, 6, "its panel and the ledger still say 4");
  assert.ok(needsYou(lists).entries.some((e) => e.ref.id === "a1"));
  // The live home, offline (the app on its copy), is left out the same way.
  assert.equal(unreadAcross(listsWith({ ...list("m-up", [row("u1", 2, 1)], "live"), away: true })), 0);
  // The local app: no shell, only `local`, always up.
  setHubShell(null);
  assert.deepEqual(listsWith(list("local", [row("l1", 3, 1)], "live")).map((l) => l.machine), ["local"]);
});

test("a copy that has not moved for longer than the heartbeat allows is shown at rest, as of when it last moved", async () => {
  const now = Date.now();
  const rows = [row("ask", "asking", 1), row("work", "working", 2), row("unread", 2, 3)];
  const copy = { rows: () => rows, freshestMs: now as number | null, loaded: true, onChange: () => () => undefined, whenLoaded: () => Promise.resolve() };
  const store = copySessions("m-1", copy, { online: () => true, staleMs: 40 });
  assert.equal(store.get().away, undefined, "fresh: as the device says");
  assert.equal(needsYou([store.get()]).entries[0]?.kind, "asking");
  let heard = 0;
  store.subscribe(() => heard++);
  await until(() => store.get().stale === true, 2000);
  assert.equal(heard, 1, "goes stale by itself, once");
  assert.equal(store.get().away, true);
  const needs = needsYou([store.get()]);
  assert.deepEqual([needs.asking, needs.working, needs.unread], [0, 0, 2], "nothing asking or working; unread stays");
  assert.equal(unreadAcross([store.get()]), 0, "and out of the title");
  store.close();
  // A copy as old, of a device not online: at rest, not "stale" (it is simply away).
  const away = copySessions("m-2", { ...copy, freshestMs: now - 3_600_000 }, { online: () => false });
  assert.deepEqual([away.get().away, away.get().stale], [true, undefined]);
  away.close();
  // Online and current: not at rest.
  const fresh = copySessions("m-3", { ...copy, freshestMs: Date.now() }, { online: () => true });
  assert.deepEqual([fresh.get().away, fresh.get().stale], [undefined, undefined]);
  fresh.close();
});

// ---------------------------------------------------------------------------
// droppen.app: every machine off its copy, nothing dialled
// ---------------------------------------------------------------------------

/** Each address set, with the machine in view at the time. */
const replaced: { url: string; focus: string | null }[] = [];
Object.assign(globalThis, {
  location: { pathname: "/sessions", search: "", reload: () => undefined },
  history: { state: null, replaceState: (_s: unknown, _t: string, url: string) => void replaced.push({ url, focus: hubShell()?.focus.get() ?? null }) },
});

class StreamShellLink extends FakeStreamLink {
  presence = new Map<string, { state: PresenceState }>();
  carriesChannels = true;
  private presenceListeners = new Set<(p: unknown) => void>();
  onPresence(fn: (p: unknown) => void) {
    this.presenceListeners.add(fn);
    return () => this.presenceListeners.delete(fn);
  }
  set(machine: string, state: PresenceState) {
    this.presence.set(machine, { state });
    for (const fn of [...this.presenceListeners]) fn(this.presence);
  }
}

/** A shell over three machines this device holds keys for, each with an index and a status written. */
async function world() {
  const ids = ["m-1", "m-2", "m-3"];
  const machines = new Map<string, Machine>();
  for (const id of ids) machines.set(id, await makeMachine());
  const link = new StreamShellLink();
  link.presence.set("m-1", { state: "online" });
  link.presence.set("m-2", { state: "online" });
  link.presence.set("m-3", { state: "offline" });
  const account = {
    account: "a-1",
    deviceId: "d-1",
    isAdmin: true,
    session: { token: "t" },
    chain: {
      hosts: () => ids.map((id) => ({ info: { id }, status: { type: "active" } })),
      latestMachineEpoch: () => null,
    },
    machine: (id: string) => (machines.has(id) ? { signKey: machines.get(id)!.signPublic } : undefined),
    machineKeys: async (id: string) => machines.get(id)?.epochs ?? new Map(),
    refreshKeys: async () => undefined,
    onChange: () => () => undefined,
  };
  const kv = new FakeKv();
  const shell = new HubShell({
    api: { streamPage: async () => new Uint8Array() } as never,
    account: account as never,
    link: link as never,
    relays: { online: () => false, onChange: () => () => undefined } as never,
    store: new MemoryKeyStore() as never,
    queryClient: new QueryClient(),
    transact: kv.transact,
    // The copies are written at NOW: read them on the same clock, or they go
    // stale (STALE_COPY_MS) once the real time is a quarter hour past it.
    now: () => NOW,
  });
  const writers = new Map<string, { index: StreamWriter; status: StreamWriter; sent: Map<StreamWriter, number> }>();
  for (const id of ids) {
    const m = machines.get(id)!;
    const index = await StreamWriter.open(m, await indexStream(m.mk), FMT_INDEX_V1);
    const status = await StreamWriter.open(m, await statusStream(m.mk), FMT_STATUS_V1);
    await status.append({ asOf: 1, settings: { hostname: `${id}.local`, platform: "darwin", serverVersion: "0.2.112" }, accounts: null, quota: null, cli: null, displayName: id, mkEpoch: 0 });
    writers.set(id, { index, status, sent: new Map() });
  }
  return { shell, link, writers };
}

/** The `sub` the link last sent for `writer`'s stream. */
const subFor = (link: FakeStreamLink, writer: StreamWriter) => link.subs().filter((s) => hex(s.stream) === hex(writer.stream)).at(-1);

/** The hub hands over what `writer` has that it has not yet: its catch-up, then each new record live. */
async function push(link: FakeStreamLink, writer: StreamWriter, sent: Map<StreamWriter, number>) {
  await until(() => subFor(link, writer) !== undefined);
  const sub = subFor(link, writer)!;
  const from = sent.get(writer) ?? sub.fromSeq;
  const records = writer.records.slice(from);
  if (records.length) link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records });
  link.deliver({ type: "head", ch: sub.ch, head: await writer.head(NOW) });
  if (!sent.has(writer)) link.control({ t: "stream_synced", ch: sub.ch, stream: hex(writer.stream), next_seq: writer.seq, exists: true });
  sent.set(writer, writer.seq);
}

/** Count `key`'s dials (its channel's `ensure`), opening at once when one comes. */
function dials(shell: HubShell, key: string) {
  const channel = shell.machine(key)!.channel;
  const counted = { n: 0 };
  channel.ensure = () => {
    counted.n++;
    (channel as unknown as { setStatus(s: MachineStatus): void }).setStatus("open");
    return Promise.resolve({ open: () => ({ send() {}, close() {}, ondata: null, onclose: null }) } as never);
  };
  return counted;
}

test("a question on a machine out of view tops the ledger, read off its copy — nothing dialled, nothing followed beyond index and status — and opening it brings that machine into view", async (t) => {
  const { shell, link, writers } = await world();
  shell.start();
  setHubShell(shell);
  t.after(() => {
    setHubShell(null);
    shell.stop();
  });
  const counted = new Map(["m-1", "m-2", "m-3"].map((id) => [id, dials(shell, id)]));
  // Every machine's index and status, as the hub has them.
  const w1 = writers.get("m-1")!;
  const w2 = writers.get("m-2")!;
  const w3 = writers.get("m-3")!;
  await w1.index.append({ upserts: [row("one-unread", 2, 30)], removed: [], asOf: 1 });
  await w2.index.append({ upserts: [row("two-idle", "idle", 40)], removed: [], asOf: 1 });
  // m-3 went offline in the middle of a question: its copy still says so.
  await w3.index.append({ upserts: [row("three-unread", 1, 300), row("three-ask", "asking", 200)], removed: [], asOf: 1 });
  for (const w of [w1, w2, w3]) {
    await push(link, w.index, w.sent);
    await push(link, w.status, w.sent);
  }
  const lists = () => ["m-1", "m-2", "m-3"].map((id) => shell.machine(id)!.sessions.get());
  await until(() => lists().every((l) => l.rows !== null));
  assert.equal(needsYou(lists()).unread, 3);
  // An offline machine is shown as the app shows its copy: nothing on it is
  // running or asking (its unread stays) — until it is back.
  assert.equal(needsYou(lists()).entries.find((e) => e.ref.id === "three-ask"), undefined, "offline: not asking anyone");
  assert.equal(needsYou(lists()).asking, 0);
  link.set("m-3", "online");
  assert.equal(needsYou(lists()).entries[0]?.ref.id, "three-ask", "back online: its question is there again");
  link.set("m-3", "offline");
  assert.equal(needsYou(lists()).asking, 0);

  // m-2's agent lists a session that stopped on a question: the next index record.
  await w2.index.append({ upserts: [row("two-ask", "asking", 0, { title: "which range?" } as Partial<SessionSummary>)], removed: [], asOf: 2 });
  await push(link, w2.index, w2.sent);
  await until(() => needsYou(lists()).entries[0]?.ref.id === "two-ask");
  const top = needsYou(lists()).entries[0]!;
  assert.deepEqual([top.kind, top.ref.machine, top.from], ["asking", "m-2", "copy"]);

  // The hook the dashboard reads gives the same lists, and the ledger draws it on top.
  function Probe() {
    const lists = useDeviceSessions();
    return <NeedsYouLedger needs={needsYou(lists)} nameOf={(m) => m} onOpen={() => {}} />;
  }
  const html = draw(<Probe />);
  assert.ok(html.indexOf("which range?") < html.indexOf("session one-unread"), "asking on top");

  // Nothing was dialled, and nothing beyond each machine's index and status was asked of the hub.
  await flush(20);
  assert.deepEqual([...counted.values()].map((c) => c.n), [0, 0, 0], "no machine dialled");
  for (const id of ["m-1", "m-2", "m-3"]) assert.notEqual(shell.machine(id)!.channel.status, "open");
  const named = new Set(link.subs().map((s) => hex(s.stream)));
  const expected = new Set([w1, w2, w3].flatMap((w) => [hex(w.index.stream), hex(w.status.stream)]));
  assert.deepEqual(named, expected, "only the indexes and statuses");
  assert.equal(shell.budget.count, 6);

  // Opening it from the dashboard: the address first (no app on the page) —
  // m-2's own, the main device being the chain's first, m-1 — then m-2 in
  // view, and only now dialled.
  shell.openAt("m-2", sessionPath(top.ref.source, top.ref.id));
  // (The shell started with nothing in view: 「我的设备」, at its own address, first.)
  assert.deepEqual(replaced, [
    { url: "/devices", focus: null },
    { url: "/m/m-2/sessions/claude/two-ask", focus: null },
  ]);
  assert.equal(shell.focus.get(), "m-2");
  assert.equal(homeMachine(), "m-1");
  await until(() => counted.get("m-2")!.n === 1);
  assert.deepEqual([counted.get("m-1")!.n, counted.get("m-3")!.n], [0, 0], "the others still not");
  // With the app on the page (m-2 in view), the main device's session is at
  // its plain address: the router follows it, the shell follows the router,
  // and the main device stays.
  shell.openAt("m-1", sessionPath("claude", "one-unread"));
  assert.deepEqual(replaced[2], { url: "/sessions/claude/one-unread", focus: "m-2" });
  assert.equal(shell.focus.get(), "m-1");
  assert.equal(homeMachine(), "m-1");
});

test("on droppen.app a device's system and agent version come off its status: the agent's own words, else an older agent's settings (#108)", async (t) => {
  const { shell, link, writers } = await world();
  shell.start();
  setHubShell(shell);
  t.after(() => {
    setHubShell(null);
    shell.stop();
  });
  const w1 = writers.get("m-1")!;
  const w2 = writers.get("m-2")!;
  await w2.status.append({
    asOf: 2,
    settings: { hostname: "m-2.local", platform: "darwin", serverVersion: "0.2.112" },
    accounts: null,
    quota: null,
    cli: null,
    displayName: "m-2",
    mkEpoch: 0,
    os: { name: "macos", version: "15.3.1", arch: "aarch64" },
    agentVersion: "0.2.131",
  });
  for (const w of [w1, w2]) await push(link, w.status, w.sent);
  const info = (id: string) => shell.runtime(id).info().get();
  await until(() => info("m-2").os === "macOS 15.3.1 aarch64");
  assert.equal(info("m-2").agentVersion, "0.2.131", "the agent's own version, not its settings'");
  await until(() => info("m-1").agentVersion !== undefined);
  assert.deepEqual([info("m-1").os, info("m-1").agentVersion], ["darwin", "0.2.112"], "an agent from before: its settings' platform and version");
});
