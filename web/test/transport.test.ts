// The transport seam, and the call sites wired through it.
//
// What these pin: the direct transport is exactly what the call sites did
// inline before it existed — the global `fetch` handed the very `Request`
// openapi-fetch built, `wsUrl`'s address, a `new WebSocket` on it — and the
// typed api client and the page's feed hub look the current transport up on
// every request and every attempt, so switching it in one place moves them,
// with the auth middleware still running on top of whichever it is.
import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { feedHubFor } from "../src/app/global-feed.js";
import { LOCAL } from "../src/lib/machines/key.js";
import { apiFor, homeMachine, openMachine, transportFor } from "../src/lib/machines/registry.js";
import { setToken } from "../src/lib/auth.js";
import { directTransport, type SocketLike, type Transport } from "../src/lib/transport/index.js";
import { tunnelTransport, type Pipe, type PipeKind, type PipeMessage, type PipeMeta } from "../src/lib/transport/tunnel.js";
import { wsUrl } from "../src/lib/ws-url.js";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

/** Stand `value` in for the global `name` for the length of one test. */
function stubGlobal(t: TestContext, name: string, value: unknown): void {
  const before = Object.getOwnPropertyDescriptor(globalThis, name);
  Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
  t.after(() => {
    if (before) Object.defineProperty(globalThis, name, before);
    else delete (globalThis as Record<string, unknown>)[name];
  });
}

/** Point the home machine at `next` for the length of one test. */
function useTransport(t: TestContext, next: Transport): void {
  openMachine(homeMachine(), next);
  t.after(() => openMachine(homeMachine(), directTransport));
}

class FakeSocket implements SocketLike {
  readyState = 0;
  binaryType = "blob";
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  constructor(readonly url: string) {}
  send() {}
  close() {
    this.readyState = 3;
  }
}

test("the direct transport hands the request itself to the global fetch", async (t) => {
  const seen: unknown[][] = [];
  const answer = new Response("ok");
  stubGlobal(t, "fetch", async (...args: unknown[]) => {
    seen.push(args);
    return answer;
  });
  const request = new Request("http://machine.test/api/accounts", { method: "POST", body: "{}" });
  assert.equal(await directTransport.fetch(request), answer);
  assert.equal(seen.length, 1);
  assert.equal(seen[0]![0], request, "the same Request, untouched");
});

test("the direct transport's address is wsUrl's, and its socket a WebSocket on it", async (t) => {
  assert.equal(directTransport.socketUrl, wsUrl, "the very function the call sites used to call");
  stubGlobal(t, "location", { protocol: "https:", host: "machine.test" });
  // No login required (the auth status has never been read): no ticket.
  assert.equal(await directTransport.socketUrl("/api/events?v=2&z=1"), "wss://machine.test/api/events?v=2&z=1");

  const made: FakeSocket[] = [];
  stubGlobal(
    t,
    "WebSocket",
    class extends FakeSocket {
      constructor(url: string) {
        super(url);
        made.push(this);
      }
    },
  );
  const socket = directTransport.openSocket("wss://machine.test/api/terminals/t1/ws");
  assert.equal(made.length, 1);
  assert.equal(socket, made[0]);
  assert.equal(made[0]!.url, "wss://machine.test/api/terminals/t1/ws");
});

test("the page starts on the direct transport, and one call moves it", (t) => {
  assert.equal(transportFor(homeMachine()), directTransport);
  const other: Transport = {
    fetch: async () => new Response(null),
    socketUrl: async (path) => path,
    openSocket: (url) => new FakeSocket(url),
  };
  useTransport(t, other);
  assert.equal(transportFor(homeMachine()), other);
});

test("typed calls go through the current transport, after the auth middleware", async (t) => {
  const seen: Request[] = [];
  useTransport(t, {
    fetch: async (request) => {
      seen.push(request);
      return new Response(JSON.stringify([{ id: "a" }]), { headers: { "content-type": "application/json" } });
    },
    socketUrl: async (path) => path,
    openSocket: (url) => new FakeSocket(url),
  });
  setToken("T");
  // An absolute base: a Request outside a page has no origin to resolve "/" against.
  const { data, response } = await apiFor(homeMachine()).GET("/api/accounts", { baseUrl: "http://machine.test" });
  assert.equal(seen.length, 1);
  assert.equal(seen[0]!.url, "http://machine.test/api/accounts");
  assert.equal(seen[0]!.headers.get("Authorization"), "Bearer T");
  assert.equal(response.status, 200);
  assert.deepEqual(data, [{ id: "a" }]);
});

test("typed calls run unchanged over a tunnel", async (t) => {
  const pipes: { kind: PipeKind; meta: PipeMeta; pipe: Pipe; sent: PipeMessage[] }[] = [];
  useTransport(
    t,
    tunnelTransport({
      open(kind, meta) {
        const sent: PipeMessage[] = [];
        const pipe: Pipe = {
          onmessage: null,
          onclose: null,
          send: (data) => void sent.push(data),
          close: () => undefined,
        };
        pipes.push({ kind, meta, pipe, sent });
        return pipe;
      },
    }),
  );
  setToken("T");
  const pending = apiFor(homeMachine()).GET("/api/accounts", { baseUrl: "http://machine.test" });
  await flush();
  const [rpc] = pipes;
  assert.equal(rpc?.kind, "rpc");
  assert.equal(rpc.meta.method, "GET");
  assert.equal(rpc.meta.path, "/api/accounts");
  assert.equal(new Headers(rpc.meta.headers).get("authorization"), "Bearer T");
  assert.deepEqual(rpc.sent, [{ kind: "end" }]);
  rpc.pipe.onmessage?.({ kind: "head", status: 200, headers: [["content-type", "application/json"]] });
  rpc.pipe.onmessage?.({ kind: "binary", data: new TextEncoder().encode('[{"id":"b"}]') });
  rpc.pipe.onclose?.(0);
  const { data } = await pending;
  assert.deepEqual(data, [{ id: "b" }]);
});

test("the page's feed hub opens its socket through the current transport", async (t) => {
  const addresses: string[] = [];
  const sockets: FakeSocket[] = [];
  useTransport(t, {
    fetch: async () => new Response(null),
    socketUrl: async (path) => `minted:${path}`,
    openSocket: (url) => {
      addresses.push(url);
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
  });
  const events: string[] = [];
  const feedHub = feedHubFor(LOCAL);
  feedHub.start({ onFrame: () => undefined, onOpen: () => events.push("open"), onClose: () => undefined });
  t.after(() => feedHub.stop());
  await flush();
  assert.equal(addresses.length, 1);
  assert.match(addresses[0]!, /^minted:\/api\/events\?v=2/, "the address the transport minted, handed back to it");
  sockets[0]!.readyState = 1;
  sockets[0]!.onopen?.({});
  assert.deepEqual(events, ["open"]);
});
