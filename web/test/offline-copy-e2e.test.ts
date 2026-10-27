// The offline copy end to end, in-process: what an agent seals and signs,
// read by the stream client over a hub link the test plays, folded and kept
// by the mirror, and served by the offline transport to the app's own feed
// reader — a session's window read-only, and the records the machine wrote
// just before it went arriving as feed deltas.
import assert from "node:assert/strict";
import test from "node:test";
import { HubCopySource } from "../src/lib/hub/copy-source.js";
import { FeedHub } from "../src/app/feed-hub.js";
import { offlineTransport, type OfflineOptions } from "../src/lib/transport/offline.js";
import { FMT_FEED_V1, FMT_INDEX_V1, FMT_STATUS_V1, generateDeviceKeys, hex, importMachineKey, indexStream, sealEphemeral, statusStream, streamId, streamKey, utf8 } from "../src/lib/e2e/index.js";
import { MachineMirror } from "../src/lib/hub/mirror.js";
import { feedFold, StreamClient } from "../src/lib/hub/streams.js";
import { FakeKv, FakeStreamLink, flush, makeMachine, StreamWriter, until } from "./stream-fixtures.js";

const opts: OfflineOptions = {
  user: "cloud:d-1",
  messages: { readOnly: "read-only copy", noCopy: "no copy", unavailable: "machine offline" },
  machineOnline: () => false,
};

const row = (id: string, updatedAt: string) => ({ source: "claude", id, title: `t ${id}`, updatedAt, cwd: "/w", scope: "owned", activity: "idle", status: "idle" });
const item = (n: number) => ({ role: "user", kind: "text", text: `item ${n}` });

test("sealed streams → mirror → offline transport → the app's feed reader", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const mirror = new MachineMirror({ account: "a-1", machine: "m-1", source: HubCopySource.over({ mk: machine.mk, client }), transact: new FakeKv().transact, saveDelayMs: { small: 1, session: 1 }, sessionWaitMs: 2000 });
  const index = await StreamWriter.open(machine, await indexStream(machine.mk), FMT_INDEX_V1);
  const status = await StreamWriter.open(machine, await statusStream(machine.mk), FMT_STATUS_V1);
  const session = await StreamWriter.open(machine, await streamId(machine.mk, "claude", "a"), FMT_FEED_V1);
  await index.append({ upserts: [row("a", "2026-10-15T00:00:00.000Z"), row("b", "2026-10-14T00:00:00.000Z")], removed: [], asOf: 1 });
  await status.append({ asOf: 1, settings: { hostname: "mac", serverStartedAt: "x" }, accounts: null, quota: null, cli: null });
  await session.append({ fromIndex: 0, items: [item(0), item(1), item(2)], endIndex: 3, fp: "f3", prefixFp: "f0" });

  /** Play the hub for `writer`: everything from the subscription's position, a head, synced. */
  const serve = async (writer: StreamWriter) => {
    await until(() => link.subs().some((s) => hex(s.stream) === hex(writer.stream)));
    const sub = link.subs().filter((s) => hex(s.stream) === hex(writer.stream)).at(-1)!;
    const records = writer.records.slice(sub.fromSeq);
    if (records.length) link.deliver({ type: "recs", ch: sub.ch, stream: writer.stream, records });
    link.deliver({ type: "head", ch: sub.ch, head: await writer.head() });
    link.control({ t: "stream_synced", ch: sub.ch, stream: hex(writer.stream), next_seq: writer.seq, exists: true });
    return sub.ch;
  };

  await mirror.start();
  await serve(index);
  await serve(status);
  await until(() => mirror.hasCopy && mirror.status?.state != null);
  const transport = offlineTransport(mirror, opts);
  const list = (await (await transport.fetch(new Request("http://page/api/sessions"))).json()) as { sessions: { id: string }[] };
  assert.deepEqual(list.sessions.map((s) => s.id), ["a", "b"]);

  const hub = new FeedHub({ socketUrl: (p) => transport.socketUrl(p), openSocket: (u) => transport.openSocket(u), noteFailure: () => undefined, inflate: null });
  const frames: Record<string, unknown>[] = [];
  hub.start({ onFrame: (f) => frames.push(f), onOpen: () => undefined, onClose: () => undefined });
  const got: Record<string, any>[] = [];
  const channel = hub.open({ source: "claude", id: "a", limit: 100 })!;
  channel.onmessage = (e) => got.push(e.data as Record<string, any>);
  // Session "a" is among the warm ones (or opened now): its stream is asked for.
  const ch = await serve(session);
  await until(() => got.length >= 1);
  assert.equal(got[0]!.type, "snapshot");
  assert.deepEqual(got[0]!.items.map((i: { text: string }) => i.text), ["item 0", "item 1", "item 2"]);
  assert.equal(got[0]!.canSend, false);
  assert.equal(got[0]!.sendBlockReason, "read-only copy");
  assert.equal(got[0]!.sig, "offline:1");
  assert.ok(frames.some((f) => f.type === "sessions:snapshot"));

  // A record the machine wrote before it went, arriving now: item 2 rewritten, item 3 added.
  await session.append({ fromIndex: 2, items: [{ ...item(2), text: "item 2 (done)" }, item(3)], endIndex: 4, fp: "f4", prefixFp: "f1" });
  link.deliver({ type: "recs", ch, stream: session.stream, records: session.records.slice(1) });
  await flush(5);
  assert.equal(got.length, 1, "nothing before the machine's head covers it");
  link.deliver({ type: "head", ch, head: await session.head() });
  await until(() => got.length >= 2);
  assert.deepEqual([got[1]!.type, got[1]!.fromIndex, got[1]!.endIndex, got[1]!.sig], ["delta", 2, 4, "offline:2"]);
  assert.deepEqual(got[1]!.items.map((i: { text: string }) => i.text), ["item 2 (done)", "item 3"]);
  hub.stop();
  mirror.close();
});

test("a live draft (ephemeral) is opened for whoever wants it, and a garbled one is ignored", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const stream = await streamId(machine.mk, "claude", "d");
  const drafts: unknown[] = [];
  const errors: unknown[] = [];
  client.subscribe(stream, feedFold, { onDraft: (d) => drafts.push(d), onError: (e) => errors.push(e) });
  await until(() => link.subs().length === 1);
  const ch = link.lastSub().ch;
  const key = await streamKey(machine.mk, stream);
  link.deliver({ type: "ephemeral", ch, stream, payload: await sealEphemeral(key, stream, 0, utf8(JSON.stringify({ type: "draft", v: 1, blocks: [] })), machine.sign) });
  link.deliver({ type: "ephemeral", ch, stream, payload: new Uint8Array(120) });
  await until(() => drafts.length === 1);
  await flush(5);
  assert.deepEqual(drafts, [{ type: "draft", v: 1, blocks: [] }]);
  assert.equal(errors.length, 0, "a draft that does not open breaks nothing");
  assert.equal(link.subs().length, 1);
});

test("a draft is shown only when the machine signed it, under the newest key epoch held", async () => {
  const machine = await makeMachine();
  const link = new FakeStreamLink();
  const mk1 = await importMachineKey(crypto.getRandomValues(new Uint8Array(32)));
  const client = new StreamClient({ link, machineSignKey: machine.signPublic, epochs: machine.epochs, fetchPage: async () => new Uint8Array(), retryDelay: () => 0 });
  const stream = await streamId(machine.mk, "claude", "d");
  const drafts: unknown[] = [];
  client.subscribe(stream, feedFold, { onDraft: (d) => drafts.push(d) });
  await until(() => link.subs().length === 1);
  const ch = link.lastSub().ch;
  const key0 = await streamKey(machine.mk, stream);
  const key1 = await streamKey(mk1, stream);
  const body = (v: number) => utf8(JSON.stringify({ type: "draft", v, blocks: [] }));
  const send = async (payload: Uint8Array) => link.deliver({ type: "ephemeral", ch, stream, payload });

  // Somebody holding the stream key who is not the machine (a removed device, say).
  const stranger = await generateDeviceKeys();
  await send(await sealEphemeral(key0, stream, 0, body(1), stranger.sign.privateKey));
  // The machine, under epoch 0 while epoch 0 is the newest held: shown.
  await send(await sealEphemeral(key0, stream, 0, body(2), machine.sign));
  await until(() => drafts.length === 1);
  assert.deepEqual(drafts, [{ type: "draft", v: 2, blocks: [] }], "only the machine's own");

  // The machine's key rotated: a draft under the old epoch is somebody who still holds it.
  await client.addEpochs(new Map([[1, mk1]]));
  assert.equal(client.newestEpoch, 1);
  await send(await sealEphemeral(key0, stream, 0, body(3), machine.sign));
  await send(await sealEphemeral(key1, stream, 1, body(4), machine.sign));
  await until(() => drafts.length === 2);
  await flush(5);
  assert.deepEqual(drafts.at(-1), { type: "draft", v: 4, blocks: [] });
  assert.equal(drafts.length, 2, "the old epoch's draft was not shown");
  client.close();
});
