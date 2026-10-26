// The outbox's `useOutboxResolution` hook, mounted for real: its effects run
// (with just enough of a DOM stood in for React and the page's listeners),
// and every request goes through the page's transport registry and the typed
// api client, as on the page.
//
// What this pins: an `unknown` send is settled the moment the transport says
// a new connection to the machine is up (a new channel after the old one was
// lost with its hub connection) — not at the checker's next backoff retry or
// the fifteen-second re-check.
//
// `import * as React` because the test directory is outside the tsconfig's
// `include`, so tsx compiles JSX here with the classic runtime.
import * as React from "react";
import assert from "node:assert/strict";
import test from "node:test";
import type { AdoptedMessage, OutgoingMessage } from "../src/features/sessions/send-receipts.js";
import type { Transport } from "../src/lib/transport/index.js";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(what: () => boolean, label: string, timeoutMs = 5_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!what()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
    await sleep(2);
  }
}

/**
 * Stand `value` in for the global `name`. Left in place: this file runs in a
 * process of its own, and React's last scheduled work after an unmount still
 * reads `window`.
 */
function stubGlobal(name: string, value: unknown): void {
  Object.defineProperty(globalThis, name, { value, configurable: true, writable: true });
}

/**
 * The little of a DOM React's client renderer and the hook touch: a document
 * and window that take listeners, and a container to render nothing into.
 * The typed api client builds its requests on `/`, which a `Request` outside
 * a page cannot resolve: they are resolved against the machine's address.
 */
function standInDom(): unknown {
  const listeners = { addEventListener: () => undefined, removeEventListener: () => undefined };
  const doc: Record<string, unknown> = { nodeType: 9, activeElement: null, body: null, visibilityState: "visible", ...listeners };
  const win = { ...listeners, document: doc, HTMLIFrameElement: class {}, event: undefined };
  doc.defaultView = win;
  stubGlobal("window", win);
  stubGlobal("document", doc);
  stubGlobal("IS_REACT_ACT_ENVIRONMENT", false);
  const NativeRequest = globalThis.Request;
  stubGlobal(
    "Request",
    class extends NativeRequest {
      constructor(input: RequestInfo | URL, init?: RequestInit) {
        super(typeof input === "string" && input.startsWith("/") ? new URL(input, "http://machine.test") : input, init);
      }
    },
  );
  return { nodeType: 1, tagName: "DIV", nodeName: "DIV", namespaceURI: "http://www.w3.org/1999/xhtml", ownerDocument: doc, textContent: "", firstChild: null, childNodes: [], removeChild: () => undefined, ...listeners };
}

test("an unknown send is settled as soon as a new channel is up, not at the next retry", async (t) => {
  const container = standInDom();
  // After the stand-ins: the api client takes its `Request` when it is made.
  const { createRoot } = await import("react-dom/client");
  const { useOutboxResolution } = await import("../src/features/sessions/use-outbox-resolution.js");
  const { directTransport } = await import("../src/lib/transport/index.js");
  const { homeMachine, openMachine } = await import("../src/lib/machines/registry.js");

  // The page's transport: its channel is down (a request fails at once, the
  // way one does when its channel went with the hub connection) until a new
  // one comes up and says so.
  let up = false;
  const reconnects = new Set<() => void>();
  const asked: number[] = [];
  const live: Transport = {
    fetch: async (request) => {
      const url = new URL(request.url);
      assert.equal(`${url.pathname}${url.search}`, "/api/sessions/claude/s-1/receipts?ids=m-1");
      asked.push(Date.now());
      if (!up) throw new TypeError("tunnel: the pipe closed before the answer ended (4410)");
      return Response.json({ receipts: [{ clientMessageId: "m-1", status: "queued", at: "2026-10-17T12:00:00Z", qid: "q-1" }], unknown: [] });
    },
    socketUrl: async (path) => path,
    openSocket: () => {
      throw new Error("no sockets in this test");
    },
    onReconnect: (fn) => {
      reconnects.add(fn);
      return () => reconnects.delete(fn);
    },
  };
  openMachine(homeMachine(), live);
  t.after(() => openMachine(homeMachine(), directTransport));

  // The composer's outbox, holding one send whose answer was lost with its channel.
  let outbox: OutgoingMessage[] = [{ clientMessageId: "m-1", text: "hello", state: "unknown" }];
  const adopted: AdoptedMessage[] = [];
  let mounted = false;
  function Composer() {
    const [queue, setQueue] = React.useState(outbox);
    const queueRef = React.useRef(queue);
    useOutboxResolution({
      source: "claude",
      id: "s-1",
      outbox: queue,
      outboxRef: queueRef,
      setOutbox: (next) => {
        queueRef.current = next;
        outbox = next;
        setQueue(next);
      },
      receipts: undefined,
      onAdopted: (a) => adopted.push(a),
    });
    React.useEffect(() => {
      mounted = true;
      return () => {
        mounted = false;
      };
    }, []);
    return null;
  }
  const root = createRoot(container as Element);
  root.render(<Composer />);
  try {
    await settledOnReconnect();
  } finally {
    // Before the stand-ins go (the test's `after` hooks): unmounting stops the
    // hook's timers and listeners, in React's next passive-effect flush.
    root.unmount();
    await until(() => !mounted, "unmounted");
  }

  async function settledOnReconnect(): Promise<void> {

  // On mount it asks, and the channel being down, gets no answer: the next try is a backoff away (1 s).
  await until(() => asked.length === 1, "the check on mount");
  await sleep(20);
  assert.equal(outbox.length, 1, "still unknown");

  // A new channel is up.
  up = true;
  const at = Date.now();
  for (const fn of [...reconnects]) fn();
  await until(() => outbox.length === 0, "the send settled");
  const tookMs = Date.now() - at;
  assert.ok(tookMs < 500, `settled ${tookMs} ms after the new channel, not at the next retry (1 s)`);
  assert.deepEqual(
    adopted.map((a) => [a.message.clientMessageId, a.receipt.status]),
    [["m-1", "queued"]],
    "the server had it: shown as sent, never sent again",
  );
  assert.equal(asked.length, 2, "one question on the new channel");
  }
});
