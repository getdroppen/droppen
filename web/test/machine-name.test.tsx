// What a machine is called in cloud mode.
//
// What these pin: the name is read from the machine's sealed status
// (`displayName`), and the id stands in when there is none — a status from an
// agent that sends no name, a blank one, or no status at all; the machine list
// reads each machine's status stream on its own and keeps it exactly where
// and how the offline copy keeps the status (so either reads the other's);
// a name is checked as the agent checks it; renaming is `POST
// /rh/display-name` on an rpc pipe, then the name the machine says it goes by;
// a refusal to a device that may not rename says so; and the list and the bar
// show the name with the id beside it, offering a rename only while the
// machine is online.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import { HubCopySource } from "../src/lib/hub/copy-source.js";
import { FMT_STATUS_V1, hex, MemoryKeyStore, statusStream } from "../src/lib/e2e/index.js";
import { MachineStatusReader } from "../src/lib/hub/machine-status.js";
import { isStoredCopy, MachineMirror, streamRecordKey } from "../src/lib/hub/mirror.js";
import { machineDisplayName, recentAlarms, type StatusState, statusFold, StreamClient } from "../src/lib/hub/streams.js";
import {
  BarMachineName,
  checkDisplayName,
  MachineNotes,
  noteRenamed,
  RenameError,
  renameErrorText,
  renameMachine,
  shownName,
  type StatusAccount,
  StatusReaders,
  followCopyStatuses,
} from "../src/features/cloud/machine-name.js";
import { DeviceRow, rowMenuItems } from "../src/features/cloud/device-row.js";
import type { PresenceState } from "../src/lib/hub/link.js";
import { catchUp, FakeKv, FakeStreamLink, makeMachine, readCopy, StreamWriter, until } from "./stream-fixtures.js";
import { fakeMachine, json } from "./fake-machine.js";
import { makeI18n, pageDraw } from "./render.js";

const i18n = makeI18n({
  lng: "en",
  resources: { en: { translation: {} } }
});
const t = i18n.t.bind(i18n);

const draw = pageDraw(i18n);

const status = (body: Record<string, unknown>): StatusState => statusFold.apply(statusFold.empty(), { asOf: 1, ...body });

/** A machine on 「我的设备」: its row, named from its status, with droppen.app's notes under its name. */
const view = (id: string, presence: PresenceState, st: StatusState | null) => ({
  id,
  fingerprint: "",
  name: shownName(id, st),
  self: false,
  kind: "computer" as const,
  status: "active" as const,
  host: true,
  stoppedHost: false,
  presence,
  path: null,
  copyAsOf: null,
  lastSeenMs: null,
  limited: null,
  accept: null,
  hasCopy: true,
});
const panel = (id: string, presence: PresenceState, st: StatusState | null) => (
  <ul>
    <DeviceRow view={view(id, presence, st)} viewerAdmin upgraded writable notes={<MachineNotes id={id} status={st} />} onOpen={() => {}} onRename={() => {}} />
  </ul>
);

test("the name comes from the status; with none to give, the id stands in", () => {
  assert.equal(machineDisplayName(status({ displayName: "studio mac" })), "studio mac");
  assert.equal(machineDisplayName(status({ displayName: "  " })), null, "blank is no name");
  assert.equal(machineDisplayName(status({})), null, "an agent that sends none");
  assert.equal(machineDisplayName(status({ displayName: 42 })), null);
  assert.equal(machineDisplayName(null), null, "no status read yet");
  assert.equal(machineDisplayName({ asOf: 1, settings: null, accounts: null, quota: null, cli: null }), null, "a copy saved before names existed");
  assert.equal(shownName("m-1", status({ displayName: "studio mac" })), "studio mac");
  assert.equal(shownName("m-1", null), null);
});

test("a rename shows at once, and gives way to the next status the machine sends", () => {
  const before = status({ displayName: "old name" });
  noteRenamed("m-9", "new name", before);
  assert.equal(shownName("m-9", before), "new name", "over the status it was set on");
  const after = status({ displayName: "new name (as the machine has it)" });
  assert.equal(shownName("m-9", after), "new name (as the machine has it)");
  noteRenamed("m-9", null, after);
  assert.equal(shownName("m-9", after), null, "reset, and the hostname not known yet: the id");
});

test("a name is checked as the agent checks it", () => {
  assert.deepEqual(checkDisplayName("  studio  "), { ok: true, name: "studio" });
  assert.deepEqual(checkDisplayName("   "), { ok: true, name: null }, "empty goes back to the hostname");
  assert.deepEqual(checkDisplayName("界".repeat(64)), { ok: true, name: "界".repeat(64) }, "characters, not bytes");
  assert.deepEqual(checkDisplayName("x".repeat(65)), { ok: false, reason: "tooLong" });
  assert.deepEqual(checkDisplayName("a\tb"), { ok: false, reason: "control" });
  assert.deepEqual(checkDisplayName("a\u0085b"), { ok: false, reason: "control" });
});

test("renaming is POST /rh/display-name on an rpc pipe, then the name the machine goes by", async () => {
  const machine = fakeMachine((e) => {
    if (e.meta.path === "/rh/display-name") return json(200, { ok: true });
    if (e.meta.path === "/rh/info") return json(200, { displayName: "macbook-pro", hostname: "macbook-pro", mkEpoch: 0, machine: "m-1" });
    return json(404, { error: "no" });
  });
  assert.equal(await renameMachine(machine.opener, null), "macbook-pro", "a reset is the hostname, which only the machine knows");
  const [set, info] = machine.exchanges;
  assert.equal(set!.kind, "rpc");
  assert.equal(set!.meta.method, "POST");
  assert.equal(set!.meta.path, "/rh/display-name");
  assert.deepEqual(JSON.parse(new TextDecoder().decode(set!.body)), { name: null });
  assert.equal(new Headers(set!.meta.headers).get("content-type"), "application/json");
  assert.deepEqual([info!.kind, info!.meta.method, info!.meta.path], ["rpc", "GET", "/rh/info"]);

  const answering = fakeMachine(() => json(200, { displayName: "studio" }));
  assert.equal(await renameMachine(answering.opener, "studio"), "studio");
  assert.equal(answering.exchanges.length, 1, "an answer that names it needs no second question");
});

test("a device that may not rename is told so; a machine that cannot be reached says that", async () => {
  const refusing = fakeMachine(() => json(403, { error: "only a current full device may rename this machine" }));
  await assert.rejects(renameMachine(refusing.opener, "x"), (error: unknown) => {
    assert.ok(error instanceof RenameError);
    assert.equal(error.kind, "forbidden");
    assert.equal(renameErrorText(error, t), "only a full device of the account can rename it");
    return true;
  });
  // Chain v3's refusals of a route that manages the computer, in words (never "attest and retry").
  const limited = fakeMachine(() => json(403, { error: "limited_device", message: "this device cannot manage the account: ask an admin device" }));
  await assert.rejects(renameMachine(limited.opener, "x"), (error: unknown) => error instanceof RenameError && error.kind === "forbidden" && error.message === "a limited device cannot do this: do it from a full device");
  const stale = fakeMachine(() => json(412, { error: "stale", reason: "behind", message: "attest and retry" }));
  await assert.rejects(renameMachine(stale.opener, "x"), (error: unknown) => {
    assert.ok(error instanceof RenameError && error.kind === "stale");
    assert.equal(renameErrorText(error, t), "this device has seen more of the account than the device it reaches: sync, then try again");
    return true;
  });
  // A pipe that closes before the head: the request never got an answer.
  const unreachable = {
    open() {
      const pipe = { onmessage: null, onclose: null as ((code: number) => void) | null, send() {}, close() {} };
      queueMicrotask(() => pipe.onclose?.(4502));
      return pipe;
    },
  };
  await assert.rejects(renameMachine(unreachable, "x"), (error: unknown) => error instanceof RenameError && error.kind === "unreachable");
  assert.equal(renameErrorText(new RenameError("unreachable", "pipe closed"), t), "the device could not be reached");
});

test("the machine list reads a machine's status stream alone, and keeps it as the offline copy keeps it", async () => {
  const machine = await makeMachine();
  const kv = new FakeKv();
  const writer = await StreamWriter.open(machine, await statusStream(machine.mk), FMT_STATUS_V1);
  await writer.append({ asOf: 1, settings: { hostname: "mbp" }, accounts: null, quota: null, cli: null, displayName: "studio mac" });

  const open = () => {
    const link = new FakeStreamLink();
    const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
    const reader = new MachineStatusReader({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: machine.mk, client }), transact: kv.transact, saveDelayMs: 1 });
    return { link, client, reader };
  };

  const first = open();
  let changes = 0;
  first.reader.onChange(() => changes++);
  await first.reader.start();
  assert.equal(first.reader.state, null, "nothing on a fresh device");
  await until(() => first.link.subs().length > 0);
  assert.equal(hex(first.link.lastSub().stream), hex(writer.stream), "only the status stream");
  assert.equal(first.link.subs().length, 1);
  await catchUp(first.link, writer);
  await until(() => machineDisplayName(first.reader.state) === "studio mac");
  assert.ok(changes >= 2, "said when the device was read, and when the name came");
  const key = streamRecordKey("a-1", "m-1", hex(writer.stream));
  await until(() => kv.data.has(key));
  assert.ok(isStoredCopy(kv.data.get(key)), "sealed at rest");
  const record = (await readCopy<StatusState>(kv, "a-1", "m-1", hex(writer.stream), "status"))!;
  assert.equal(record.kind, "status");
  assert.equal(record.state.displayName, "studio mac");
  first.reader.close();
  first.client.close();

  // Read back at once on the next visit, and only what came after asked for.
  await writer.append({ asOf: 2, settings: {}, displayName: "renamed" });
  const second = open();
  await second.reader.start();
  assert.equal(machineDisplayName(second.reader.state), "studio mac", "drawn from the device before the hub says anything");
  await until(() => second.link.subs().length > 0);
  assert.equal(second.link.lastSub().fromSeq, 1);
  await catchUp(second.link, writer);
  await until(() => machineDisplayName(second.reader.state) === "renamed");
  // Saved in the background (sealed with WebCrypto): wait for it, however long the machine takes.
  for (const deadline = Date.now() + 10_000; (await readCopy<StatusState>(kv, "a-1", "m-1", hex(writer.stream), "status"))?.state.displayName !== "renamed"; ) {
    assert.ok(Date.now() < deadline, "the renamed status was saved");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  second.reader.close();
  second.client.close();

  // The machine's own offline copy reads the very same record.
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: machine.mk, client }), transact: kv.transact });
  await mirror.start();
  assert.equal(machineDisplayName(mirror.status?.state), "renamed");
  mirror.close();
  client.close();
});

test("a device with no key for the machine reads nothing and shows the id", async () => {
  const reader = new MachineStatusReader({ account: "a-1", machine: "m-1", source: null, transact: new FakeKv().transact });
  let told = false;
  reader.onChange(() => (told = true));
  await reader.start();
  assert.equal(reader.state, null);
  assert.ok(reader.loaded && told);
  const html = draw(panel("m-1", "online", reader.state));
  assert.match(html, /data-testid="open-m-1"/);
  assert.match(html, />m-1</, "its id, under a stand-in name");
});

test("the list shows the name with the id under it, and offers a rename only while the machine is online", () => {
  const named = status({ displayName: "studio mac" });
  const online = draw(panel("m-7", "online", named));
  assert.match(online, /data-testid="open-m-7"/, "the button the acceptance harness clicks");
  assert.match(online, /data-testid="name-m-7"[^>]*>studio mac</);
  assert.match(online, /<details[^>]*>[\s\S]*>m-7</, "the id, in the row's disclosure");
  const t = i18n.getFixedT("en");
  const rename = (presence: PresenceState) => rowMenuItems(view("m-7", presence, named), { viewerAdmin: true, upgraded: true, writable: true, t, onRename: () => {} }).find((i) => i.id === "rename-m-7");
  assert.equal(rename("online")?.blocked, null);
  assert.equal(rename("offline")?.blocked, "the device is offline — this needs it to be online", "renaming needs the machine: shown, and why not");
  const offline = draw(panel("m-7", "offline", named));
  assert.match(offline, /studio mac/, "the name from the last status, offline as well");
});

test("a machine's chain alarm (a fork it was shown) reaches every device through its status, and the list says so", () => {
  const now = Date.now();
  const alarmed = status({
    displayName: "studio mac",
    alarms: [
      { atMs: now - 8 * 24 * 60 * 60 * 1000, what: "an old one" },
      { atMs: now - 1000, what: "the hub served a device chain that conflicts with this device's (the chain forked at entry 4)" },
      { atMs: "soon", what: 1 },
    ],
  });
  assert.equal(alarmed.alarms?.length, 2, "well-formed alarms only");
  assert.deepEqual(
    recentAlarms(alarmed, now).map((a) => a.what),
    ["the hub served a device chain that conflicts with this device's (the chain forked at entry 4)"],
    "the last week's",
  );
  const html = draw(panel("m-7", "online", alarmed));
  assert.match(html, /data-testid="chain-alarm-m-7"/);
  assert.match(html, /chain alarm/);
  assert.match(html, /forked at entry 4/);
  const calm = draw(panel("m-7", "online", status({ displayName: "studio mac" })));
  assert.doesNotMatch(calm, /chain-alarm/);
  assert.equal(status({}).alarms, undefined, "no field when there are none");
});

test("the bar shows the open machine's name from its copy, and the id beside it", () => {
  const mirror = {
    status: { state: status({ displayName: "studio mac" }) },
    onChange: () => () => undefined,
  } as unknown as MachineMirror;
  const live = draw(<BarMachineName machine="m-3" mirror={mirror} channel={{ open: () => { throw new Error("no"); } }} canRename />);
  assert.match(live, /data-testid="bar-machine-name"[^>]*>studio mac</);
  assert.match(live, />m-3</);
  assert.match(live, /aria-label="rename device"/);
  const away = draw(<BarMachineName machine="m-3" mirror={null} channel={null} canRename={false} />);
  assert.match(away, /data-testid="bar-machine-name"[^>]*>m-3</, "no copy, no name: the id");
  assert.doesNotMatch(away, /rename device/);
});

test("a machine whose key lands after the list opened gets a reader then, and its name with it", async () => {
  const machine = await makeMachine();
  const kv = new FakeKv();
  const writer = await StreamWriter.open(machine, await statusStream(machine.mk), FMT_STATUS_V1);
  await writer.append({ asOf: 1, settings: {}, displayName: "adas-Mac-mini.local" });

  // The account as enrollment leaves it: the machine in the chain, its key not yet.
  let keys = new Map<number, typeof machine.mk>();
  const listeners = new Set<() => void>();
  const account = {
    account: "a-1",
    session: { token: "t" },
    api: { streamPage: async () => new Uint8Array() },
    machine: (id: string) => (id === "m-1" ? { id, signKey: machine.signPublic, dhKey: new Uint8Array(32) } : undefined),
    machineKeys: async () => new Map(keys),
    onChange: (fn: () => void) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
  } as unknown as StatusAccount;
  const link = new FakeStreamLink();
  const shown = new Map<string, StatusState | null>();
  const readers = new StatusReaders({ account, link, store: new MemoryKeyStore(), transact: kv.transact, onStatus: (m, state) => shown.set(m, state) });
  readers.set(["m-1"]);
  await readers.recheck();
  assert.equal(link.subs().length, 0, "no key, nothing to follow");
  assert.equal(machineDisplayName(shown.get("m-1")), null, "the id stands in");

  // The key lands (issued in the channel, put in the keyring) and the account says so.
  keys = new Map(machine.epochs);
  for (const fn of listeners) fn();
  await until(() => link.subs().length > 0);
  await catchUp(link, writer);
  await until(() => machineDisplayName(shown.get("m-1")) === "adas-Mac-mini.local");

  // A change that leaves the epochs as they were opens nothing new.
  for (const fn of listeners) fn();
  await readers.recheck();
  assert.equal(link.subs().length, 1);

  // A rotation adds an epoch: a reader with every epoch, the name kept meanwhile.
  keys = new Map([...machine.epochs, [1, machine.mk]]);
  for (const fn of listeners) fn();
  await until(() => link.subs().length === 2);
  assert.equal(machineDisplayName(shown.get("m-1")), "adas-Mac-mini.local");
  assert.equal(link.lastSub().fromSeq, 1, "the new reader goes on from what the old one saved");
  readers.close();
  assert.equal(listeners.size, 0, "closed: no longer listening to the account");
});

test("statuses of machines whose copies the page follows are read from those copies, not from readers of their own", async () => {
  const listeners = new Set<(c: { kind: string }) => void>();
  const copy = {
    status: { state: { displayName: "first" } as never as StatusState } as { state: StatusState | null } | null,
    onChange: (fn: (c: { kind: string }) => void) => (listeners.add(fn), () => listeners.delete(fn)),
    whenLoaded: () => Promise.resolve(),
  };
  const heard: [string, string | null][] = [];
  const off = followCopyStatuses([["m-1", copy as never]], (machine, state) => heard.push([machine, machineDisplayName(state)]));
  await new Promise((resolve) => setTimeout(resolve, 0));
  copy.status = { state: { displayName: "renamed" } as never };
  for (const fn of listeners) fn({ kind: "index" });
  for (const fn of listeners) fn({ kind: "status" });
  off();
  assert.equal(listeners.size, 0, "unsubscribed");
  assert.deepEqual(heard, [
    ["m-1", "first"],
    ["m-1", "first"],
    ["m-1", "renamed"],
  ]);
});
