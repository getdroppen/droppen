// What a page holding every machine of the account follows from the hub, and
// how much it reads of the device: bounded.
//
// - A copy out of view follows its index and its status, and no session; in
//   view, its warm sessions too; out of view again, those go.
// - The page's subscriptions share one hub link, which the hub caps (256);
//   a page-wide budget keeps under it by refusing a session's stream, never an
//   index or a status — and a session refused is not waited on.
// - A save reads only its own machine's keys, as a key range; the sweep only
//   the mirror's.
// - A copy closed while a save is on its way does not write it.
import assert from "node:assert/strict";
import test from "node:test";
import { FMT_INDEX_V1, FMT_STATUS_V1, hex, indexStream, statusStream, streamId } from "../src/lib/e2e/index.js";
import { HubCopySource, StreamBudget } from "../src/lib/hub/copy-source.js";
import { MachineMirror, type MirrorSeal, mirrorSeal, streamPrefix, streamRecordKey, sweepCopies } from "../src/lib/hub/mirror.js";
import { StreamClient } from "../src/lib/hub/streams.js";
import { MemoryKeyStore } from "../src/lib/e2e/index.js";
import { FakeKv, FakeStreamLink, flush, makeMachine, type Machine, rotate, StreamWriter, until } from "./stream-fixtures.js";

const row = (id: string, updatedAt: string) => ({ source: "claude", id, title: `t ${id}`, updatedAt, cwd: "/w" });

/** The streams the link is subscribed to right now (by hex). */
function following(link: FakeStreamLink): Set<string> {
  const out = new Set<string>();
  for (const sub of link.subs()) if (link.routes.has(sub.ch)) out.add(hex(sub.stream));
  return out;
}

/** Play the hub for `writer`'s latest subscription: its records, a head, synced. */
async function serve(link: FakeStreamLink, writer: StreamWriter) {
  const at = () => link.subs().filter((s) => hex(s.stream) === hex(writer.stream) && link.routes.has(s.ch)).at(-1);
  await until(() => at() !== undefined);
  const sub = at()!;
  link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records: writer.records.slice(sub.fromSeq) });
  link.deliver({ type: "head", ch: sub.ch, head: await writer.head() });
  link.control({ t: "stream_synced", ch: sub.ch, stream: hex(writer.stream), next_seq: writer.seq, exists: true });
}

async function machineWith(ids: string[]) {
  const machine = await makeMachine();
  const index = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  const status = await StreamWriter.open(machine, await statusStream(machine.mk), FMT_STATUS_V1);
  await index.append({ upserts: ids.map((id, i) => row(id, `2026-10-1${i}T00:00:00.000Z`)), removed: [], asOf: 1 });
  await status.append({ asOf: 1, settings: { hostname: "h" }, accounts: null, quota: null, cli: null });
  return { machine, index, status };
}

const sessionHex = async (machine: Machine, id: string) => hex(await streamId(machine.mk, "claude", id));

test("a copy out of view follows its index and status only; in view its warm sessions too; out again they go", async () => {
  const { machine, index, status } = await machineWith(["a", "b"]);
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: machine.mk, client }), transact: new FakeKv().transact, followSessions: false, saveDelayMs: { small: 1, session: 1 } });
  await mirror.start();
  await serve(link, index);
  await serve(link, status);
  await until(() => mirror.rows()?.length === 2);
  await flush(10);
  const core = new Set([hex(index.stream), hex(status.stream)]);
  assert.deepEqual(following(link), core, "out of view: the index and the status, no session");

  mirror.setFollowSessions(true);
  const a = await sessionHex(machine, "a");
  const b = await sessionHex(machine, "b");
  await until(() => following(link).has(a) && following(link).has(b));

  mirror.setFollowSessions(false);
  await until(() => !following(link).has(a) && !following(link).has(b));
  assert.deepEqual(following(link), core);
  mirror.close();
  client.close();
});

test("the page's stream budget gives way on a session's stream, never on an index or a status, and a refused session is not waited on", async () => {
  const budget = new StreamBudget(3);
  const link = new FakeStreamLink();
  const one = await machineWith(["a", "b", "c", "d"]);
  const client = new StreamClient({ link, machineSignKey: one.machine.signPublic, epochs: one.machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: one.machine.mk, client }, budget), transact: new FakeKv().transact, sessionWaitMs: 5000 });
  await mirror.start();
  await serve(link, one.index);
  await serve(link, one.status);
  await until(() => mirror.rows()?.length === 4);
  await flush(20);
  const sessions = await Promise.all(["a", "b", "c", "d"].map((id) => sessionHex(one.machine, id)));
  assert.equal(sessions.filter((s) => following(link).has(s)).length, 1, "the index, the status and one session: the budget is 3");
  assert.equal(budget.count, 3);

  // Another machine's index and status, over budget: taken all the same.
  const two = await machineWith(["x"]);
  const client2 = new StreamClient({ link, machineSignKey: two.machine.signPublic, epochs: two.machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror2 = new MachineMirror({ account: "a-1", machine: "m-2", source: HubCopySource.over({ mk: two.machine.mk, client: client2 }, budget), transact: new FakeKv().transact });
  await mirror2.start();
  await until(() => following(link).has(hex(two.index.stream)) && following(link).has(hex(two.status.stream)));
  assert.equal(budget.count, 5);

  // A session there is no room for: answered at once with what the device has (nothing), not after the wait.
  const refused = ["a", "b", "c", "d"].find((id, i) => !following(link).has(sessions[i]!))!;
  const started = Date.now();
  assert.equal(await mirror.session("claude", refused), null);
  assert.ok(Date.now() - started < 2000, "not waited on");

  // Room again once a stream goes.
  mirror2.close();
  assert.equal(budget.count, 3);
  mirror.close();
  assert.equal(budget.count, 0, "every stream given back");
  client.close();
  client2.close();
});

test("a session's save reads only its own machine's keys, and the sweep only the copies'", async () => {
  const { machine, index, status } = await machineWith(["a"]);
  const kv = new FakeKv();
  kv.data.set("feed:claude/elsewhere", { unrelated: true });
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: machine.mk, client }), transact: kv.transact, saveDelayMs: { small: 1, session: 1 } });
  await mirror.start();
  await serve(link, index);
  await serve(link, status);
  const session = await StreamWriter.open(machine, await streamId(machine.mk, "claude", "a"), 1);
  await session.append({ fromIndex: 0, items: [{ kind: "message", role: "user", text: "hi" }], endIndex: 1, fp: "f", prefixFp: "p" });
  await serve(link, session);
  await until(() => kv.keyReads.includes(streamPrefix("a-1", "m-1")));
  await sweepCopies(kv.transact, "a-1");
  assert.ok(!kv.keyReads.includes(null), `no read of every key: ${JSON.stringify(kv.keyReads)}`);
  assert.ok(kv.keyReads.every((p) => p === streamPrefix("a-1", "m-1") || p === "stream/" || p === "streammeta/"));
  assert.ok(kv.data.has("feed:claude/elsewhere"), "and nothing else is touched");
  mirror.close();
  client.close();
});

test("a copy closed while a save is on its way does not write it", async () => {
  const { machine, index } = await machineWith(["a"]);
  const kv = new FakeKv();
  const real = mirrorSeal(new MemoryKeyStore());
  let open!: () => void;
  const gate = new Promise<void>((resolve) => {
    open = resolve;
  });
  let sealing = 0;
  const seal: MirrorSeal = {
    async seal(at, value) {
      sealing++;
      await gate;
      return real.seal(at, value);
    },
    open: (at, sealed) => real.open(at, sealed),
  };
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: machine.mk, client }), transact: kv.transact, seal, saveDelayMs: { small: 1, session: 1 } });
  await mirror.start();
  await serve(link, index);
  await until(() => sealing > 0);
  mirror.close();
  open();
  await flush(20);
  assert.equal(kv.txs.filter((t) => t.puts.length > 0).length, 0, "nothing written after the close");
  client.close();
});

test("leaving a machine while a warm session's save is pending keeps every warm copy on the device", async () => {
  const ids = ["a", "b", "c"];
  const { machine, index, status } = await machineWith(ids);
  const kv = new FakeKv();
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: machine.mk, client }), transact: kv.transact, saveDelayMs: { small: 1, session: 80 } });
  await mirror.start();
  await serve(link, index);
  await serve(link, status);
  const sessions = new Map<string, StreamWriter>();
  for (const id of ids) {
    const w = await StreamWriter.open(machine, await streamId(machine.mk, "claude", id), 1);
    await w.append({ fromIndex: 0, items: [{ kind: "message", role: "user", text: id }], endIndex: 1, fp: `f-${id}`, prefixFp: "p" });
    sessions.set(id, w);
    await serve(link, w);
  }
  const keyOf = (w: StreamWriter) => streamRecordKey("a-1", "m-1", hex(w.stream));
  await until(() => [...sessions.values()].every((w) => kv.data.has(keyOf(w))));

  // An update to one warm session: its save is pending (it trails by 80 ms)…
  const a = sessions.get("a")!;
  const sub = link.subs().filter((s) => hex(s.stream) === hex(a.stream) && link.routes.has(s.ch)).at(-1)!;
  await a.append({ fromIndex: 1, items: [{ kind: "message", role: "assistant", text: "more" }], endIndex: 2, fp: "f-a2", prefixFp: "p" });
  link.deliver({ type: "recs", ch: sub.ch, stream: a.stream, records: a.records.slice(1) });
  link.deliver({ type: "head", ch: sub.ch, head: await a.head() });
  // …and the person switches to another machine before it lands.
  await until(() => mirror.sessionNow("claude", "a")?.state.endIndex === 2);
  mirror.setFollowSessions(false);
  await flush(250);
  assert.ok(kv.txs.some((t) => t.puts.includes(keyOf(a))) && kv.data.has(keyOf(a)), "the pending save landed");
  for (const [id, w] of sessions) assert.ok(kv.data.has(keyOf(w)), `the warm copy of ${id} is still on the device`);
  mirror.close();
  client.close();
});

test("the budget counts an index or a status as the names actually subscribed, however many key epochs are held", async () => {
  const machine = await makeMachine();
  for (let epoch = 1; epoch <= 4; epoch++) await rotate(machine, epoch);
  const newest = machine.epochs.get(4)!;
  const index = await StreamWriter.openAt(machine, await indexStream(newest), FMT_INDEX_V1, 4);
  const status = await StreamWriter.openAt(machine, await statusStream(newest), FMT_STATUS_V1, 4);
  await index.append({ upserts: [row("a", "2026-10-10T00:00:00.000Z")], removed: [], asOf: 1 });
  await status.append({ asOf: 1, settings: { hostname: "h" }, accounts: null, quota: null, cli: null });
  const budget = new StreamBudget();
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: machine.mk, client }, budget), transact: new FakeKv().transact, followSessions: false });
  await mirror.start();
  await serve(link, index);
  await serve(link, status);
  await until(() => mirror.rows()?.length === 1);
  await flush(20);
  assert.equal(following(link).size, 2, "one name each is subscribed");
  assert.equal(budget.count, 2, "and that is what is counted — not one per epoch held (10)");
  mirror.close();
  assert.equal(budget.count, 0);
  client.close();
});

test("a session refused for want of room is asked for again as soon as there is room", async () => {
  const budget = new StreamBudget(3);
  const occupied = budget.require(1);
  const { machine, index, status } = await machineWith(["a"]);
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: machine.mk, client }, budget), transact: new FakeKv().transact });
  await mirror.start();
  await serve(link, index);
  await serve(link, status);
  await until(() => mirror.rows()?.length === 1);
  await flush(20);
  const a = await sessionHex(machine, "a");
  assert.equal(following(link).has(a), false, "no room: refused");
  occupied.release();
  await until(() => following(link).has(a));
  mirror.close();
  client.close();
});
