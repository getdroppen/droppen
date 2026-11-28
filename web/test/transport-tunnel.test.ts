// The tunnel transport: the same seam, over pipes of a channel, speaking the
// agent bridge's messages (`lib/e2e/bridge.ts`).
//
// What these pin: a request and its answer survive the trip whole — method,
// path and query, headers, a body each way in chunks — and a pipe that fails
// reads as `fetch` failing, at whichever point it fails; a socket pipe behaves
// as a socket — open only once the far end says it upgraded, frames each way
// as they were, binary as the caller's `binaryType` asks, a close from either
// end heard once — and the feed hub, unchanged, runs its life over one:
// subscribe, hello, routed and compressed frames, a drop and a reconnect.
import assert from "node:assert/strict";
import test from "node:test";
import { deflateRawSync, inflateRawSync } from "node:zlib";
import { FeedHub } from "../src/app/feed-hub.js";
import { tunnelTransport, type Pipe, type PipeKind, type PipeMessage, type PipeMeta } from "../src/lib/transport/tunnel.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const bytes = (text: string) => new TextEncoder().encode(text);

/** The machine's end of one pipe, played by the test. */
class FarEnd {
  readonly received: PipeMessage[] = [];
  /** The page let go of it. */
  closedHere = false;
  onReceive: (() => void) | null = null;
  readonly pipe: Pipe;

  constructor(
    readonly kind: PipeKind,
    readonly meta: PipeMeta,
  ) {
    const end = this;
    this.pipe = {
      onmessage: null,
      onclose: null,
      send(message) {
        if (end.closedHere) throw new Error("sent on a pipe the page closed");
        end.received.push(message);
        end.onReceive?.();
      },
      close() {
        end.closedHere = true;
      },
    };
  }

  say(message: PipeMessage): void {
    this.pipe.onmessage?.(message);
  }

  head(status: number, headers: [string, string][] = []): void {
    this.say({ kind: "head", status, headers });
  }

  /** The socket's `open`: the bridge's 101. */
  upgraded(): void {
    this.head(101);
  }

  text(value: unknown): void {
    this.say({ kind: "text", data: JSON.stringify(value) });
  }

  binary(data: Uint8Array): void {
    this.say({ kind: "binary", data });
  }

  /** The answer is complete (the agent closes with 0). */
  done(): void {
    this.pipe.onclose?.(0);
  }

  hangUp(code = 4502): void {
    this.pipe.onclose?.(code);
  }

  /** The text messages the page sent, parsed. */
  sent(): unknown[] {
    return this.received.flatMap((m) => (m.kind === "text" ? [JSON.parse(m.data)] : []));
  }

  /** The request body, once the page has said it ended. */
  body(): Promise<string> {
    return new Promise((resolve) => {
      const check = () => {
        if (this.received.at(-1)?.kind !== "end") return;
        const chunks = this.received.flatMap((m) => (m.kind === "binary" ? [m.data] : []));
        resolve(Buffer.concat(chunks).toString());
      };
      this.onReceive = check;
      check();
    });
  }
}

function channel() {
  const ends: FarEnd[] = [];
  const transport = tunnelTransport({
    open(kind, meta) {
      const end = new FarEnd(kind, meta);
      ends.push(end);
      return end.pipe;
    },
  });
  return { ends, transport };
}

test("a request and its answer make the round trip whole", async () => {
  const { ends, transport } = channel();
  const pending = transport.fetch(
    new Request("http://machine.test/api/sessions/claude/a/send?x=1&y=2", {
      method: "POST",
      headers: { "content-type": "application/json", "x-trace": "abc" },
      body: JSON.stringify({ text: "hello" }),
    }),
  );
  const end = ends[0]!;
  assert.equal(end.kind, "rpc");
  assert.equal(end.meta.method, "POST");
  assert.equal(end.meta.path, "/api/sessions/claude/a/send?x=1&y=2", "the route and its query, not the origin");
  const headers = new Headers(end.meta.headers);
  assert.equal(headers.get("content-type"), "application/json");
  assert.equal(headers.get("x-trace"), "abc");
  assert.equal(await end.body(), '{"text":"hello"}');

  end.head(201, [
    ["content-type", "application/json"],
    ["x-id", "7"],
  ]);
  const response = await pending;
  assert.equal(response.status, 201);
  assert.equal(response.headers.get("x-id"), "7");
  // The body in pieces, as a channel would deliver a large one.
  end.binary(bytes('{"ok":'));
  end.binary(bytes("true}"));
  end.done();
  assert.deepEqual(await response.json(), { ok: true });
  assert.ok(end.closedHere, "the page lets go of the pipe once the answer has ended");
});

test("a request without a body sends only its end, and a 204 is an answer without one", async () => {
  const { ends, transport } = channel();
  const pending = transport.fetch(new Request("http://machine.test/api/accounts/a", { method: "DELETE" }));
  await flush();
  assert.deepEqual(ends[0]!.received, [{ kind: "end" }]);
  ends[0]!.head(204);
  ends[0]!.done();
  const response = await pending;
  assert.equal(response.status, 204);
  assert.equal(response.body, null);
});

test("a pipe that fails reads as fetch failing, before the answer or during its body", async () => {
  const { ends, transport } = channel();
  const before = transport.fetch(new Request("http://machine.test/api/accounts"));
  ends[0]!.hangUp();
  await assert.rejects(before, TypeError);

  const during = transport.fetch(new Request("http://machine.test/api/accounts"));
  ends[1]!.head(200);
  const response = await during;
  ends[1]!.binary(bytes("half of it"));
  ends[1]!.hangUp();
  await assert.rejects(response.text(), TypeError);

  // An answer that does not start with a head is not one.
  const garbled = transport.fetch(new Request("http://machine.test/api/accounts"));
  ends[2]!.binary(bytes("HTTP/1.1 200 OK"));
  await assert.rejects(garbled, TypeError);
  assert.ok(ends[2]!.closedHere);
});

test("aborting a request lets go of its pipe", async () => {
  const { ends, transport } = channel();
  const controller = new AbortController();
  const pending = transport.fetch(new Request("http://machine.test/api/accounts", { signal: controller.signal }));
  controller.abort();
  await assert.rejects(pending, { name: "AbortError" });
  assert.ok(ends[0]!.closedHere);
  // Late news from the machine reaches nobody.
  ends[0]!.head(200);
});

test("a socket's address is its route, and the route picks the pipe's kind", async () => {
  const { ends, transport } = channel();
  assert.equal(await transport.socketUrl("/api/events?v=2&z=1"), "/api/events?v=2&z=1", "no ticket over a channel");
  transport.openSocket("/api/events?v=2&z=1");
  transport.openSocket("/api/sessions/claude/a/ws?since=3&fp=x");
  transport.openSocket("/api/terminals/t1/ws");
  assert.deepEqual(
    ends.map((end) => [end.kind, end.meta]),
    [
      ["mux", { path: "/api/events?v=2&z=1" }],
      ["mux", { path: "/api/sessions/claude/a/ws?since=3&fp=x" }],
      ["term", { path: "/api/terminals/t1/ws" }],
    ],
  );
});

test("a socket pipe opens when the far end says it upgraded, and carries frames as they were", async () => {
  const { ends, transport } = channel();
  const socket = transport.openSocket("/api/terminals/t1/ws");
  const end = ends[0]!;
  const heard: unknown[] = [];
  socket.onopen = () => heard.push("open");
  socket.onmessage = (e) => heard.push(e.data);
  assert.equal(socket.readyState, 0);
  assert.throws(() => socket.send("too early"), { name: "InvalidStateError" }, "like a browser socket still connecting");

  end.upgraded();
  assert.equal(socket.readyState, 1);
  socket.send('{"type":"input","data":"ls\\r"}');
  assert.deepEqual(end.received, [{ kind: "text", data: '{"type":"input","data":"ls\\r"}' }]);
  end.say({ kind: "text", data: "\u001b[2Jprompt$ " });

  // Binary as the caller asked for it, and exactly the frame's own bytes.
  socket.binaryType = "arraybuffer";
  end.binary(new Uint8Array([9, 1, 2, 3]).subarray(1));
  socket.binaryType = "blob";
  end.binary(bytes("as a blob"));

  assert.equal(heard[0], "open");
  assert.equal(heard[1], "\u001b[2Jprompt$ ");
  assert.ok(heard[2] instanceof ArrayBuffer);
  assert.deepEqual([...new Uint8Array(heard[2] as ArrayBuffer)], [1, 2, 3]);
  assert.ok(heard[3] instanceof Blob);
  assert.equal(await (heard[3] as Blob).text(), "as a blob");
});

test("a close from either end is heard once, and the page's own asynchronously", async () => {
  const { ends, transport } = channel();
  let closes = 0;
  const dropped = transport.openSocket("/api/events?v=2");
  dropped.onclose = () => closes++;
  ends[0]!.upgraded();
  ends[0]!.hangUp();
  assert.equal(dropped.readyState, 3);
  dropped.close();
  assert.equal(closes, 1);

  let ownCloses = 0;
  const closed = transport.openSocket("/api/events?v=2");
  closed.onclose = () => ownCloses++;
  ends[1]!.upgraded();
  closed.close();
  assert.equal(closed.readyState, 2, "closing");
  assert.equal(ownCloses, 0, "not from inside the call that caused it");
  assert.ok(ends[1]!.closedHere);
  closed.send("after close"); // silently nothing, as a browser socket does
  await flush();
  assert.equal(closed.readyState, 3);
  assert.equal(ownCloses, 1);
  ends[1]!.hangUp(); // the channel confirming it
  assert.equal(ownCloses, 1);
  assert.deepEqual(ends[1]!.received, []);
});

test("a refused upgrade is a socket that closes without ever opening", async () => {
  const { ends, transport } = channel();
  const heard: string[] = [];
  const refused = transport.openSocket("/api/terminals/gone/ws");
  refused.onopen = () => heard.push("open");
  refused.onclose = () => heard.push("close");
  ends[0]!.hangUp();
  assert.deepEqual(heard, ["close"]);
  assert.equal(refused.readyState, 3);

  // A far end that speaks before saying it upgraded is refused too.
  const confused = transport.openSocket("/api/terminals/t1/ws");
  confused.onopen = () => heard.push("open");
  confused.onclose = () => heard.push("close");
  ends[1]!.say({ kind: "text", data: "frame before open" });
  await flush();
  assert.deepEqual(heard, ["close", "close"]);
  assert.ok(ends[1]!.closedHere);

  // And a refused upgrade the bridge reports with its status.
  const missing = transport.openSocket("/api/terminals/nope/ws");
  missing.onopen = () => heard.push("open");
  missing.onclose = () => heard.push("close");
  ends[2]!.head(404);
  await flush();
  assert.deepEqual(heard, ["close", "close", "close"]);
});

test("the feed hub runs unchanged over a tunnel socket: subscribe, hello, frames, a drop and a reconnect", async (t) => {
  const { ends, transport } = channel();
  const frames: Record<string, unknown>[] = [];
  const events: string[] = [];
  const failures: number[] = [];
  const hub = new FeedHub({
    socketUrl: (path) => transport.socketUrl(path),
    openSocket: (url) => transport.openSocket(url),
    noteFailure: (streak) => failures.push(streak),
    inflate: async (data) => inflateRawSync(Buffer.from(data)).toString(),
    random: () => 0,
  });
  t.after(() => hub.stop());

  // A feed asks for its channel before the list starts the socket — the order
  // React runs their effects in.
  const feed = hub.open({ source: "claude", id: "a", since: 4, fp: "f4", limit: 100 })!;
  const feedHeard: unknown[] = [];
  feed.onopen = () => feedHeard.push("open");
  feed.onmessage = (e) => feedHeard.push(e.data);
  feed.onclose = () => feedHeard.push("close");
  hub.start({
    onFrame: (m) => frames.push(m),
    onOpen: () => events.push("open"),
    onClose: () => events.push("close"),
  });
  await flush();
  assert.equal(ends.length, 1);
  const end = ends[0]!;
  assert.equal(end.kind, "mux");
  assert.equal(end.meta.path, "/api/events?v=2&z=1");

  end.upgraded();
  assert.deepEqual(events, ["open"]);
  assert.deepEqual(end.sent(), [
    { type: "sub", feeds: [{ sub: 1, source: "claude", id: "a", since: 4, fp: "f4", limit: 100 }] },
  ]);

  end.text({ type: "hello", compress: "deflate-raw" });
  assert.equal(hub.mode, "mux");
  assert.ok(hub.compressed);
  assert.deepEqual(feedHeard, ["open"]);

  end.text({ sub: 1, type: "delta", fromIndex: 4, items: [] });
  assert.deepEqual(feedHeard.at(-1), { sub: 1, type: "delta", fromIndex: 4, items: [] });

  // A large frame comes compressed, as binary: the tunnel socket has to hand it
  // over as the `ArrayBuffer` the hub asked for, or it would not decode.
  end.binary(deflateRawSync(Buffer.from(JSON.stringify({ type: "sessions:delta", sessions: [] }))));
  await flush();
  assert.deepEqual(frames, [{ type: "sessions:delta", sessions: [] }]);

  // The feed talks on its channel; the hub puts it on the one pipe.
  feed.send(JSON.stringify({ type: "resync" }));
  assert.deepEqual(end.sent().at(-1), { type: "resync", sub: 1 });

  // The pipe goes: the list hears it, the feed's channel closes, and the hub
  // starts its backoff — which a kick (the network is back) cuts short.
  end.hangUp();
  assert.deepEqual(events, ["open", "close"]);
  assert.deepEqual(feedHeard.at(-1), "close");
  assert.deepEqual(failures, [1]);
  hub.kick();
  await flush();
  assert.equal(ends.length, 2, "a fresh pipe");
  const again = ends[1]!;

  // The feed's own reconnect asks for a channel again, before the pipe is up.
  const reopened = hub.open({ source: "claude", id: "a" })!;
  again.upgraded();
  assert.deepEqual(events, ["open", "close", "open"]);
  assert.deepEqual(again.sent(), [{ type: "sub", feeds: [{ sub: 2, source: "claude", id: "a" }] }]);
  again.text({ type: "hello" });
  assert.equal(reopened.readyState, 1);
});

test("tunnelFetch applies upload drain backpressure and pull-based download ack", async () => {
  const acked: number[] = [];
  let releaseDrain: (() => void) | null = null;
  let drainCalls = 0;
  const sent: PipeMessage[] = [];
  let onmsg: ((m: PipeMessage) => void) | null = null;
  let oncls: ((code: number) => void) | null = null;

  const transport = tunnelTransport({
    open() {
      return {
        get onmessage() {
          return onmsg;
        },
        set onmessage(fn) {
          onmsg = fn;
        },
        get onclose() {
          return oncls;
        },
        set onclose(fn) {
          oncls = fn;
        },
        send(m) {
          sent.push(m);
        },
        drain() {
          drainCalls += 1;
          if (drainCalls === 1) {
            return new Promise<void>((resolve) => {
              releaseDrain = resolve;
            });
          }
          return Promise.resolve();
        },
        ack(n) {
          acked.push(n);
        },
        close() {},
      };
    },
  });

  // Upload a 150 KiB buffer: sliced at 64 KiB and paused after the first slice while drain() is pending.
  const upload = new Uint8Array(150 * 1024).fill(7);
  const reqPromise = transport.fetch(new Request("http://machine.test/api/uploads", { method: "POST", body: upload }), { class: "blob" });
  await flush();
  assert.equal(sent.length, 1, "upload paused after first slice waiting on drain()");
  assert.equal(sent[0]?.kind, "binary");
  releaseDrain!();
  await flush();
  await flush();
  assert.equal(sent.at(-1)?.kind, "end", "remaining slices and end went once drain resolved");

  // Download: ReadableStream pulls at most its 1-chunk highWaterMark upfront; subsequent chunks stay queued and un-acked until read().
  onmsg!({ kind: "head", status: 200, headers: [] });
  const res = await reqPromise;
  onmsg!({ kind: "binary", data: bytes("chunk-1") });
  onmsg!({ kind: "binary", data: bytes("chunk-2") });
  onmsg!({ kind: "binary", data: bytes("chunk-3") });
  oncls!(0);
  assert.deepEqual(acked, [7], "at most one chunk is acked before the reader pulls");

  const reader = res.body!.getReader();
  const first = await reader.read();
  assert.equal(new TextDecoder().decode(first.value), "chunk-1");
  assert.deepEqual(acked, [7, 7], "credit granted for next chunk as reader advances");
  const second = await reader.read();
  assert.equal(new TextDecoder().decode(second.value), "chunk-2");
  assert.deepEqual(acked, [7, 7, 7], "credit granted for third chunk once pulled");
  const third = await reader.read();
  assert.equal(new TextDecoder().decode(third.value), "chunk-3");
  const end = await reader.read();
  assert.equal(end.done, true);
});
