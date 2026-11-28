import { type ConnFields, connLog, errorFields, routeTemplate } from "@/lib/connection-log";
import type { FetchOptions, SocketLike, Transport } from "./types";

/*
 * The same `Transport`, carried over sub-streams ("pipes") of an end-to-end
 * channel to the machine instead of over the network to the page's own origin
 * — what the page uses in cloud mode (`lib/hub`).
 *
 * The agent bridges each pipe to its own loopback listener — `rpc` to
 * `/api/*`, `mux` and `term` to the matching websocket route — so the server's
 * handlers see an ordinary request or upgrade
 * (`docs/cross-machine-architecture.md` §5, "MVP 的 agent 侧桥接"). What a
 * pipe carries is `lib/e2e/bridge.ts`, byte for byte the agent's
 * (`droppen-proto/src/bridge.rs`); a {@link Pipe} here sees it already framed:
 *
 * - A pipe opens with its request head ({@link PipeMeta}): an `rpc` pipe's
 *   method, route and headers; a socket pipe's route alone.
 * - `rpc`: the request body goes up as `binary` messages, then `end`. The
 *   agent answers with one `head` (status and headers), the response body as
 *   `binary` messages, and closes the pipe with code 0. A pipe that closes
 *   with any other code — or before the head — is a request that failed, the
 *   way a dropped connection is.
 * - `blob`: exactly an `rpc` pipe, for the routes that move file bytes —
 *   `GET /api/fs/download`, `POST /api/fs/upload`, `GET /api/media/github` —
 *   on the channel's lowest-priority lane, so a download is drained only when
 *   terminals and the app's own calls have nothing to send. The agent takes
 *   no other route on it, and buffers at most {@link BLOB_UPLOAD_MAX} of a
 *   request body.
 * - `mux` / `term`: the agent's `head` with status 101 is the socket's `open`;
 *   any other status (or a close before it) is a refused handshake. After it,
 *   every websocket message either way is one `text` or `binary` message.
 */

export type PipeKind = "rpc" | "mux" | "term" | "blob";

/** The most of a request body the agent buffers on a `blob` pipe; a larger one is refused here, before it is sent. */
export const BLOB_UPLOAD_MAX = 64 * 1024 * 1024;

/** A request body past what its pipe may carry. */
export class BodyTooLargeError extends Error {
  constructor(readonly limit: number) {
    super(`the request body is larger than ${limit} bytes`);
    this.name = "BodyTooLargeError";
  }
}

/** What a pipe is opened for: the route the bridge connects it to on the machine's loopback listener. */
export interface PipeMeta {
  /** Path and query, e.g. `/api/events?v=2`. */
  path: string;
  /** `rpc` only. */
  method?: string;
  /** `rpc` only; pairs, as `Headers` iterates them. */
  headers?: [string, string][];
}

/** One message on a pipe (`lib/e2e/bridge.ts` kinds). */
export type PipeMessage =
  | { kind: "text"; data: string }
  | { kind: "binary"; data: Uint8Array }
  | { kind: "head"; status: number; headers: [string, string][] }
  | { kind: "end" };

/** One sub-stream of the channel to the machine: ordered messages each way. */
export interface Pipe {
  send(message: PipeMessage): void;
  /** Wait until this pipe's send queue has drained below its backpressure threshold. */
  drain?(): Promise<void>;
  /** Acknowledge `bytes` of received binary payload consumed by the reader (flow control). */
  ack?(bytes: number): void;
  /**
   * Done with it, both ways. Whether `onclose` then fires is up to the
   * channel; the adapters here cope with either.
   */
  close(): void;
  onmessage: ((message: PipeMessage) => void) | null;
  /** The far end closed it (code 0: done), or the channel under it went away. */
  onclose: ((code: number) => void) | null;
}

export interface PipeOpener {
  /**
   * Never throws: a channel that is down answers with a pipe that closes,
   * which a socket reads as a refused handshake and a request as a network
   * error — the same failures a direct connection would report, and the ones
   * every reconnect loop above already knows how to back off from.
   */
  open(kind: PipeKind, meta: PipeMeta): Pipe;
  /** Hear each time a new channel is up to open pipes on (the first, and each one after a loss). */
  onOpen?(fn: () => void): () => void;
}

/** `WebSocket.CONNECTING` … `CLOSED`, spelled out so this runs where there is no `WebSocket`. */
const CONNECTING = 0;
const OPEN = 1;
const CLOSING = 2;
const CLOSED = 3;

/** Statuses a `Response` may not be given a body for, even an empty one. */
const NULL_BODY = new Set([101, 103, 204, 205, 304]);

function abortReason(signal: AbortSignal): unknown {
  return signal.reason ?? new DOMException("The operation was aborted.", "AbortError");
}

/**
 * Terminals get a pipe kind of their own so the channel can put keystrokes
 * ahead of feed traffic. Every other websocket route — the list's socket, and
 * a session's own feed socket on a server too old to carry feeds on it — is a
 * feed, and rides as `mux`.
 */
export function socketKind(path: string): "mux" | "term" {
  return path.startsWith("/api/terminals/") ? "term" : "mux";
}

/** What the connection log is told of each request and socket over a tunnel (its device, masked). */
export interface TunnelLog {
  device?: string | null;
}

/**
 * Whether a request is a big one worth a line when it starts too (not only
 * when it ends): file bytes (`blob`), a transcript's history, or a range of it.
 */
function bigRequest(kind: PipeKind, route: string): boolean {
  return kind === "blob" || route.includes("/history") || /[?&]from=/.test(route);
}

function tunnelFetch(opener: PipeOpener, request: Request, kind: "rpc" | "blob" = "rpc", log: TunnelLog = {}): Promise<Response> {
  const signal = request.signal;
  if (signal.aborted) return Promise.reject(abortReason(signal));
  const url = new URL(request.url);
  // The request as the connection log has it: the route's template only, never its ids or query values.
  const route = routeTemplate(url.pathname + url.search);
  const startedAt = performance.now();
  const moved = { rx: 0, tx: 0 };
  let headAt: number | null = null;
  let answered: number | null = null;
  const logged = (fields: ConnFields, level?: "warn") => {
    try {
      connLog.emit(
        "req",
        {
          device: log.device,
          kind,
          method: request.method,
          route,
          status: answered,
          ms: performance.now() - startedAt,
          ttfbMs: headAt === null ? null : headAt - startedAt,
          rx: moved.rx,
          tx: moved.tx,
          ...fields,
        },
        level,
      );
    } catch {
      // the request goes on as it would
    }
  };
  if (bigRequest(kind, route)) connLog.emit("req.start", { device: log.device, kind, method: request.method, route });
  return new Promise<Response>((resolve, reject) => {
    const pipe = opener.open(kind, {
      path: url.pathname + url.search,
      method: request.method,
      headers: [...request.headers],
    });
    // `head` until the answer's head is in, `body` while its body streams,
    // `done` once it has ended or failed, from either side.
    let state: "head" | "body" | "done" = "head";
    let body: ReadableStreamDefaultController<Uint8Array> | null = null;
    const rxQueue: Uint8Array[] = [];
    let rxClosed = false;
    let pulling = false;

    const settle = () => {
      state = "done";
      signal.removeEventListener("abort", onAbort);
      pipe.onmessage = null;
      pipe.onclose = null;
      pipe.close();
    };
    const fail = (error: unknown) => {
      if (state === "done") return;
      const before = state;
      rxQueue.length = 0;
      settle();
      const aborted = signal.aborted;
      logged({ stage: before, ...(aborted ? { error: "aborted" } : errorFields(error)) }, aborted ? undefined : "warn");
      if (before === "head") reject(error);
      else body?.error(error);
    };
    const onAbort = () => fail(abortReason(signal));
    signal.addEventListener("abort", onAbort, { once: true });

    pipe.onmessage = (message) => {
      if (state === "head") {
        if (message.kind !== "head") {
          fail(new TypeError("tunnel: the answer did not start with a head"));
          return;
        }
        const status = message.status;
        answered = status;
        headAt = performance.now();
        const stream = new ReadableStream<Uint8Array>({
          start: (controller) => {
            body = controller;
          },
          pull: pipe.ack
            ? (controller) => {
                const next = rxQueue.shift();
                if (next) {
                  controller.enqueue(next);
                  pipe.ack?.(next.byteLength);
                  if (rxClosed && rxQueue.length === 0) controller.close();
                  return;
                }
                if (rxClosed) {
                  controller.close();
                  return;
                }
                pulling = true;
              }
            : undefined,
          // The reader walked away: so does the request.
          cancel: () => {
            rxQueue.length = 0;
            if (state === "done") return;
            settle();
            logged({ cancelled: true });
          },
        });
        let response: Response;
        try {
          response = new Response(NULL_BODY.has(status) ? null : stream, { status, headers: message.headers });
        } catch (error) {
          fail(error);
          return;
        }
        state = "body";
        resolve(response);
        return;
      }
      if (state === "body" && message.kind === "binary") {
        moved.rx += message.data.length;
        if (pipe.ack) {
          if (pulling && body) {
            pulling = false;
            body.enqueue(message.data);
            pipe.ack(message.data.byteLength);
          } else {
            rxQueue.push(message.data);
          }
        } else {
          body?.enqueue(message.data);
        }
      }
    };
    pipe.onclose = (code) => {
      if (state === "body" && code === 0) {
        settle();
        logged({});
        if (pipe.ack) {
          rxClosed = true;
          if (rxQueue.length === 0) {
            pulling = false;
            body?.close();
          }
        } else {
          body?.close();
        }
        return;
      }
      fail(new TypeError(`tunnel: the pipe closed before the answer ended (${code})`));
    };

    sendBody(pipe, request, () => state === "done", kind === "blob" ? BLOB_UPLOAD_MAX : null, moved).catch(fail);
  });
}

/** Chunk size when slicing large request body buffers so send backpressure kicks in smoothly. */
const SEND_CHUNK = 64 * 1024;

/**
 * The request body up the pipe, then its end. It goes up while the answer may
 * already be coming down: a server is free to answer early, and once the
 * exchange is `finished` the rest is not wanted.
 */
async function sendBody(pipe: Pipe, request: Request, finished: () => boolean, limit: number | null, moved: { tx: number }): Promise<void> {
  if (request.body) {
    const reader = request.body.getReader();
    let sent = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (finished()) {
        void reader.cancel();
        return;
      }
      sent += value.byteLength;
      // Past what the far end would buffer: refused here rather than after
      // the whole of it has crossed the channel. No `end` follows, so what
      // did go up is never taken for a request.
      if (limit !== null && sent > limit) {
        void reader.cancel();
        throw new BodyTooLargeError(limit);
      }
      if (pipe.drain && value.byteLength > SEND_CHUNK) {
        for (let at = 0; at < value.byteLength; at += SEND_CHUNK) {
          if (finished()) {
            void reader.cancel();
            return;
          }
          pipe.send({ kind: "binary", data: value.subarray(at, Math.min(at + SEND_CHUNK, value.byteLength)) });
          await pipe.drain();
        }
      } else {
        pipe.send({ kind: "binary", data: value });
        if (pipe.drain) await pipe.drain();
      }
      moved.tx = sent;
    }
  }
  if (!finished()) pipe.send({ kind: "end" });
}

/** A `WebSocket`-shaped view of a socket pipe, for the callers that hold a {@link SocketLike}. */
class PipeSocket implements SocketLike {
  readyState = CONNECTING;
  binaryType = "blob";
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  /** For the connection log: when it was asked for and opened (`performance.now`), what it moved, who closed it. */
  private readonly startedAt = performance.now();
  private openedAt: number | null = null;
  private readonly moved = { rx: 0, tx: 0 };
  private by: "page" | "peer" = "peer";

  constructor(
    private readonly pipe: Pipe,
    private readonly log: { route: string; kind: PipeKind; device?: string | null } | null = null,
  ) {
    pipe.onmessage = (message) => this.received(message);
    pipe.onclose = () => this.finish();
  }

  send(data: string): void {
    // As a browser socket does: an error while still connecting, and silently
    // nothing once it is closing.
    if (this.readyState === CONNECTING) throw new DOMException("Still connecting", "InvalidStateError");
    if (this.readyState === OPEN) {
      this.moved.tx += data.length;
      this.pipe.send({ kind: "text", data });
    }
  }

  close(): void {
    if (this.readyState === CLOSING || this.readyState === CLOSED) return;
    this.readyState = CLOSING;
    this.by = "page";
    this.pipe.close();
    // Asynchronously, like a browser socket's: a caller reads the close as
    // news, and must not be re-entered from inside the call that caused it.
    queueMicrotask(() => this.finish());
  }

  private received(message: PipeMessage): void {
    if (this.readyState === CONNECTING) {
      if (message.kind === "head" && message.status === 101) {
        this.readyState = OPEN;
        this.openedAt = performance.now();
        if (this.log) connLog.emit("socket.open", { device: this.log.device, kind: this.log.kind, route: this.log.route, ms: this.openedAt - this.startedAt });
        this.onopen?.({});
      } else {
        if (this.log) connLog.emit("socket.close", { device: this.log.device, kind: this.log.kind, route: this.log.route, refused: message.kind === "head" ? message.status : message.kind }, "warn");
        // A refused upgrade, or a far end that speaks before saying it
        // upgraded: either way not a socket this can use.
        this.close();
      }
      return;
    }
    if (this.readyState !== OPEN) return;
    if (message.kind === "text") {
      this.moved.rx += message.data.length;
      this.onmessage?.({ data: message.data });
      return;
    }
    if (message.kind !== "binary") return;
    this.moved.rx += message.data.length;
    // A copy, so the frame is an `ArrayBuffer` of exactly its own bytes.
    const bytes = new Uint8Array(message.data).buffer;
    this.onmessage?.({ data: this.binaryType === "arraybuffer" ? bytes : new Blob([bytes]) });
  }

  private finish(): void {
    if (this.readyState === CLOSED) return;
    if (this.log && this.openedAt !== null) {
      connLog.emit("socket.close", {
        device: this.log.device,
        kind: this.log.kind,
        route: this.log.route,
        by: this.by,
        openMs: performance.now() - this.openedAt,
        rx: this.moved.rx,
        tx: this.moved.tx,
      });
    }
    this.readyState = CLOSED;
    this.pipe.onmessage = null;
    this.pipe.onclose = null;
    this.onclose?.({});
  }
}

/** A transport over pipes from `opener`; `log` says which device it reaches, for the connection log. */
export function tunnelTransport(opener: PipeOpener, log: TunnelLog = {}): Transport {
  return {
    tunneled: true,
    fetch: (request, options?: FetchOptions) => tunnelFetch(opener, request, options?.class === "blob" ? "blob" : "rpc", log),
    // No ticket: a page that can open a pipe has already been through the
    // channel's own authentication, and the bridge authenticates the loopback
    // upgrade with its own credential. The address is the route itself.
    socketUrl: async (path) => path,
    openSocket: (path) => {
      const kind = socketKind(path);
      return new PipeSocket(opener.open(kind, { path }), { route: routeTemplate(path), kind, device: log.device });
    },
    ...(opener.onOpen ? { onReconnect: (fn: () => void) => opener.onOpen!(fn) } : {}),
  };
}
