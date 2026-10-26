// File bytes in cloud mode: the channel's `blob` lane.
//
// What these pin: a request asked for on the blob lane opens a `blob` pipe
// (the agent takes file routes only there, at the lowest priority), and
// everything else stays `rpc`; the body streams both ways in pieces; the
// bounds — a picture 25 MB, a download 256 MB, an upload 64 MB — are enforced
// here, before or while bytes move, with an error that says so; an offline
// copy's refusal reads as its own sentence; and the direct transport (local
// mode) is untouched: same request, same global fetch, no limits.
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { directTransport, type Transport } from "../src/lib/transport/index.js";
import { homeMachine, openMachine, transportFor } from "../src/lib/machines/registry.js";
import { BlobCache, BlobError, BLOB_LIMITS, dispositionName, fetchBlob, formatLimit, sendUpload } from "../src/lib/transport/blob.js";
import { BLOB_UPLOAD_MAX, BodyTooLargeError } from "../src/lib/transport/tunnel.js";
import { offlineTransport } from "../src/features/cloud/offline-transport.js";
import { fakeMachine, json } from "./fake-machine.js";

const MiB = 1024 * 1024;

function useTransport(t: TestContext, next: Transport): void {
  openMachine(homeMachine(), next);
  t.after(() => openMachine(homeMachine(), directTransport));
}

test("the blob lane opens a blob pipe; the app's calls stay on rpc", async () => {
  const machine = fakeMachine(() => ({ status: 200, headers: [["content-type", "image/png"]], chunks: ["\x89PNG", "rest"] }));
  const res = await machine.transport.fetch(new Request("http://machine.invalid/api/fs/download?path=%2Fa.png&inline=true"), { class: "blob" });
  assert.equal(res.status, 200);
  assert.equal(await res.text(), "\x89PNGrest");
  await machine.transport.fetch(new Request("http://machine.invalid/api/sessions"));
  await machine.transport.fetch(new Request("http://machine.invalid/api/settings"), { class: "rpc" });
  assert.deepEqual(
    machine.exchanges.map((e) => [e.kind, e.meta.method, e.meta.path]),
    [
      ["blob", "GET", "/api/fs/download?path=%2Fa.png&inline=true"],
      ["rpc", "GET", "/api/sessions"],
      ["rpc", "GET", "/api/settings"],
    ],
  );
  assert.ok(machine.transport.tunneled, "the tunnel says the machine is not at the page's origin");
});

test("fetchBlob reads the answer whole, with its type, its name and its progress", async () => {
  const machine = fakeMachine(() => ({
    status: 200,
    headers: [
      ["content-type", "application/pdf"],
      ["content-length", "10"],
      ["content-disposition", "attachment; filename*=UTF-8''%E6%8A%A5%E5%91%8A.pdf"],
    ],
    chunks: ["01234", "56789"],
  }));
  const progress: [number, number | null][] = [];
  const got = await fetchBlob("/api/fs/download?path=%2Fr.pdf", {
    limit: BLOB_LIMITS.download,
    via: machine.transport,
    onProgress: (received, total) => progress.push([received, total]),
  });
  assert.equal(machine.exchanges[0]!.kind, "blob");
  assert.equal(got.blob.type, "application/pdf");
  assert.equal(await got.blob.text(), "0123456789");
  assert.equal(got.name, "报告.pdf");
  assert.deepEqual(progress, [
    [0, 10],
    [5, 10],
    [10, 10],
  ]);
});

test("a picture past its bound is refused from its declared size, before its body is read", async () => {
  const machine = fakeMachine(() => ({
    status: 200,
    headers: [["content-length", String(BLOB_LIMITS.image + 1)]],
    chunks: ["x", "y", "z"],
  }));
  await assert.rejects(fetchBlob("/api/fs/download?path=%2Fbig.png&inline=true", { limit: BLOB_LIMITS.image, via: machine.transport }), (error: unknown) => {
    assert.ok(error instanceof BlobError);
    assert.equal(error.kind, "tooLarge");
    assert.equal(error.limit, 25 * MiB);
    return true;
  });
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.ok(machine.exchanges[0]!.closedHere, "the pipe is let go of, not drained");
});

test("a body that runs past the bound without saying its size is cut off where it passes it", async () => {
  const chunk = new Uint8Array(400 * 1024);
  const machine = fakeMachine(() => ({ status: 200, chunks: [chunk, chunk, chunk, chunk] }));
  await assert.rejects(
    fetchBlob("/api/fs/download?path=%2Fstream.bin", { limit: MiB, via: machine.transport }),
    (error: unknown) => error instanceof BlobError && error.kind === "tooLarge" && error.limit === MiB,
  );
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.ok(machine.exchanges[0]!.closedHere);
});

test("an error answer is a BlobError carrying the server's sentence and status", async () => {
  const machine = fakeMachine(() => json(404, { error: "no such file" }));
  await assert.rejects(fetchBlob("/api/fs/download?path=%2Fgone", { limit: BLOB_LIMITS.download, via: machine.transport }), {
    name: "BlobError",
    kind: "status",
    status: 404,
    message: "no such file",
  });
  const dropped = fakeMachine(() => ({ status: 200, chunks: ["half"], hangUp: 4502 }));
  await assert.rejects(fetchBlob("/api/fs/download?path=%2Fx", { limit: BLOB_LIMITS.download, via: dropped.transport }), { name: "BlobError", kind: "network" });
});

test("on the offline copy a file is the copy's 503, said in its own words", async () => {
  const offline = offlineTransport(null, {
    user: null,
    messages: { readOnly: "read-only", noCopy: "no copy", unavailable: "the device is offline — this needs it to be online" },
  });
  assert.ok(offline.tunneled);
  await assert.rejects(fetchBlob("/api/fs/download?path=%2Fa.png&inline=true", { limit: BLOB_LIMITS.image, via: offline }), {
    name: "BlobError",
    kind: "status",
    status: 503,
    message: "the device is offline — this needs it to be online",
  });
});

test("an upload past 64 MB is refused before anything is sent; under it rides the blob lane", async (t) => {
  const machine = fakeMachine((e) => json(200, { ok: true, bytes: e.body.length }));
  useTransport(t, machine.transport);
  const big = () => new Request("http://machine.invalid/api/fs/upload?dir=%2Ftmp", { method: "POST", body: "x" });
  await assert.rejects(sendUpload(big(), BLOB_LIMITS.upload + 1, transportFor(homeMachine())), (error: unknown) => {
    assert.ok(error instanceof BlobError);
    assert.equal(error.kind, "tooLarge");
    assert.equal(formatLimit(error.limit!), "64 MB");
    return true;
  });
  assert.equal(machine.exchanges.length, 0, "nothing went out");

  const form = new FormData();
  form.append("file", new Blob(["hello upload"]), "a.txt");
  const res = await sendUpload(new Request("http://machine.invalid/api/fs/upload?dir=%2Ftmp", { method: "POST", body: form }), 12, transportFor(homeMachine()));
  assert.equal(res.status, 200);
  const exchange = machine.exchanges[0]!;
  assert.equal(exchange.kind, "blob");
  assert.equal(exchange.meta.method, "POST");
  assert.equal(exchange.meta.path, "/api/fs/upload?dir=%2Ftmp");
  assert.match(new TextDecoder().decode(exchange.body), /hello upload/);
});

test("the tunnel itself stops a blob body at what the agent buffers, and sends no end", async () => {
  const machine = fakeMachine(() => json(200, {}));
  const piece = new Uint8Array(4 * MiB);
  let sent = 0;
  const body = new ReadableStream<Uint8Array>({
    pull(controller) {
      if (sent > BLOB_UPLOAD_MAX + 8 * MiB) return controller.close();
      sent += piece.length;
      controller.enqueue(piece);
    },
  });
  const request = new Request("http://machine.invalid/api/fs/upload?dir=%2Ftmp", { method: "POST", body, duplex: "half" } as RequestInit);
  await assert.rejects(machine.transport.fetch(request, { class: "blob" }), (error: unknown) => error instanceof BodyTooLargeError);
  const exchange = machine.exchanges[0]!;
  assert.equal(exchange.ended, false, "what went up is never taken for a request");
  assert.ok(exchange.closedHere);
  assert.ok(exchange.body.length <= BLOB_UPLOAD_MAX);
});

test("local mode: the direct transport takes the lane hint and hands the same request to fetch, with no bound", async (t) => {
  const seen: Request[] = [];
  const original = globalThis.fetch;
  globalThis.fetch = (async (input: Request) => {
    seen.push(input);
    return new Response("ok");
  }) as typeof fetch;
  t.after(() => {
    globalThis.fetch = original;
  });
  assert.equal(directTransport.tunneled, undefined, "the page's origin is the machine");
  const request = new Request("http://machine.test/api/fs/upload?dir=%2Ftmp", { method: "POST", body: "x" });
  const res = await sendUpload(request, BLOB_LIMITS.upload * 4, directTransport);
  assert.equal(await res.text(), "ok");
  assert.equal(seen[0], request, "the very request, unchanged");
});

test("Content-Disposition names: the encoded one first, then the quoted or bare one", () => {
  assert.equal(dispositionName(`attachment; filename="a \\"b\\".txt"`), 'a "b".txt');
  assert.equal(dispositionName("inline; filename=plain.png"), "plain.png");
  assert.equal(dispositionName(`attachment; filename="fallback.txt"; filename*=UTF-8''%C3%A9t%C3%A9.txt`), "été.txt");
  assert.equal(dispositionName("inline"), null);
  assert.equal(dispositionName(null), null);
});

test("the picture cache: least recently used out first, by count and by bytes; loads shared; failures not kept", async () => {
  const cache = new BlobCache(3, 10);
  const blob = (n: number) => new Blob([new Uint8Array(n)]);
  cache.put("a", blob(2));
  cache.put("b", blob(2));
  cache.put("c", blob(2));
  assert.ok(cache.get("a"), "a is now the most recently used");
  cache.put("d", blob(2));
  assert.equal(cache.get("b"), undefined, "b was the least recently used");
  assert.deepEqual([cache.size, cache.totalBytes], [3, 6]);
  cache.put("e", blob(9));
  assert.deepEqual([cache.size, cache.totalBytes], [1, 9], "bytes bound the cache too");
  cache.put("huge", blob(11));
  assert.equal(cache.get("huge"), undefined, "larger than the whole bound: handed out, not kept");

  let loads = 0;
  const load = async () => {
    loads++;
    await new Promise((resolve) => setTimeout(resolve, 5));
    return blob(1);
  };
  const [one, two] = await Promise.all([cache.load("x", load), cache.load("x", load)]);
  assert.equal(one, two);
  assert.equal(loads, 1, "one fetch for two pictures of the same file");
  assert.equal(await cache.load("x", load), one, "and none for a third");

  await assert.rejects(cache.load("bad", async () => Promise.reject(new Error("offline"))));
  assert.equal(await cache.load("bad", async () => blob(1)).then((b) => b.size), 1, "a failure is asked again");
});

test("cloud mode's switching transport: the copy's 503 while offline, the machine's blob lane once live, and a word on each switch", async () => {
  const { SwitchingTransport } = await import("../src/features/cloud/cloud-transport.js");
  const machine = fakeMachine(() => ({ status: 200, headers: [["content-type", "image/png"]], chunks: ["png"] }));
  const offline = offlineTransport(null, { user: null, messages: { readOnly: "", noCopy: "", unavailable: "offline" } });
  const switching = new SwitchingTransport({ live: machine.transport, offline }, "offline");
  assert.ok(switching.tunneled);
  let changes = 0;
  const off = switching.onChange(() => changes++);
  const route = "/api/fs/download?path=%2Fa.png&inline=true";
  await assert.rejects(fetchBlob(route, { limit: BLOB_LIMITS.image, via: switching }), { name: "BlobError", status: 503, message: "offline" });
  switching.setMode("live");
  assert.equal(changes, 1, "a picture that failed on the copy hears it is worth asking again");
  const got = await fetchBlob(route, { limit: BLOB_LIMITS.image, via: switching });
  assert.equal(await got.blob.text(), "png");
  assert.equal(machine.exchanges[0]!.kind, "blob", "the lane survives the switch");
  off();
  switching.setMode("offline");
  assert.equal(changes, 1);
});
