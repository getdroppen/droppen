/*
 * The move's messages between its two windows, and the one way they are
 * heard. Every message goes with the exact origin of the window it is for as
 * `targetOrigin` (never `*`): a window that went elsewhere meanwhile, or an
 * opener of another site, is handed nothing. Every message is taken only from
 * the one window expected (`event.source`) at the one origin expected
 * (`event.origin`): anything else is counted and dropped.
 *
 * push (the legacy origin opens the app host's `/move`):
 *   app → legacy  ready {id}
 *   legacy → app  bundle {id, bundle}
 *   app → legacy  verified {id, digest, …} | refused {id, reason, detail}
 *   legacy → app  wiped {id} | kept {id, reason}
 * pull (the app host opens the legacy origin's `/move`):
 *   legacy → app  hello {account, device} | none
 *   app → legacy  request {id}
 *   legacy → app  bundle {id, bundle} | cancel {id}       (after a click there)
 *   app → legacy  verified … | refused …
 *   legacy → app  wiped {id} | kept {id, reason}
 * `id` is the receiving side's: 128 random bits, one per window.
 */
import { hex } from "@/lib/e2e/bytes";
import type { PeerMessage, PeerWindow, MoveEnv } from "./env";
import type { MoveOrigins } from "./types";

export const MSG = {
  ready: "rh-move/ready",
  hello: "rh-move/hello",
  none: "rh-move/none",
  request: "rh-move/request",
  bundle: "rh-move/bundle",
  cancel: "rh-move/cancel",
  verified: "rh-move/verified",
  refused: "rh-move/refused",
  wiped: "rh-move/wiped",
  kept: "rh-move/kept",
} as const;
export type MsgType = (typeof MSG)[keyof typeof MSG];

/** What the receiving side may refuse a bundle for. */
export type Refusal = "targetHasDevice" | "verifyFailed" | "badBundle" | "busy";

/** A store's records as they cross. */
export interface BundleStore {
  db: string;
  store: string;
  version: number;
  records: { key: string; value: unknown }[];
}

/** What crosses from the legacy origin: its stores as they are, the moved `localStorage` entries, and who to sign in as. */
export interface MoveBundle {
  v: 1;
  /** The sending origin. */
  from: string;
  /** The account and device the receiving side signs in as. */
  account: string;
  device: string;
  stores: BundleStore[];
  local: { key: string; value: string }[];
  /** The device this browser's notifications came from (the subscription itself never crosses). */
  pushWasOn: string | null;
  /** `digestOf` the stores and entries. */
  digest: string;
}

export interface Message {
  v: 1;
  t: MsgType;
  id?: string;
  [field: string]: unknown;
}

const ID = /^[0-9a-f]{32}$/;

/** A fresh window id (128 random bits, hex). */
export function newId(): string {
  return hex(crypto.getRandomValues(new Uint8Array(16)));
}

/** `data` as a move message of one of `types` (and of window `id`, when given), or `null`. */
export function asMessage(data: unknown, types: readonly MsgType[], id?: string): Message | null {
  if (!data || typeof data !== "object") return null;
  const m = data as Partial<Message>;
  if (m.v !== 1 || typeof m.t !== "string" || !types.includes(m.t as MsgType)) return null;
  if (m.id !== undefined && (typeof m.id !== "string" || !ID.test(m.id))) return null;
  if (id !== undefined && m.id !== id) return null;
  return m as Message;
}

/**
 * Why `origins` are not the move's two (`null`: they are, and this page is
 * the `role` side): exactly the hub's own origin and its app host as this
 * page's hub pinned them (`pinned`, `/.well-known/droppen-hub`), each an
 * exact https origin — no suffix or parent-domain match of any kind.
 */
export function originsProblem(origins: MoveOrigins, here: string, role: "legacy" | "app", pinned: MoveOrigins | null): string | null {
  const exact = (s: string) => {
    try {
      const u = new URL(s);
      return u.origin === s && u.protocol === "https:";
    } catch {
      return false;
    }
  };
  if (!exact(origins.legacy) || !exact(origins.app)) return "the move's origins are not two https origins";
  if (origins.legacy === origins.app) return "the move's two origins are the same";
  if (!pinned) return "this page's hub names no app host to move to";
  if (origins.legacy !== pinned.legacy || origins.app !== pinned.app) return "the move's origins are not the hub's own and its app host";
  if (here !== origins[role]) return `this page is not at ${origins[role]}`;
  return null;
}

/** Why a link to the other window ended. */
export class LinkEnded extends Error {
  constructor(readonly why: "closed" | "timeout" | "stopped") {
    super(`the other window ${why === "closed" ? "was closed" : why === "timeout" ? "did not answer in time" : "is no longer listened to"}`);
    this.name = "LinkEnded";
  }
}

/** How often the other window is looked at to see whether it was closed. */
const CLOSED_POLL_MS = 250;

/**
 * The one window this page talks to: what it says is queued until asked for
 * ({@link PeerLink.next}); whatever else reaches this page is dropped
 * (counted in `ignored`).
 */
export interface PeerLink {
  send(message: Omit<Message, "v">): void;
  /** The next message of one of `types` (of window `id`, when given): rejects with {@link LinkEnded} when the window is closed, after `withinMs`, or on `stop`. */
  next(types: readonly MsgType[], withinMs: number, id?: string): Promise<Message>;
  readonly ignored: number;
  stop(): void;
}

export function peerLink(env: MoveEnv, peerOrigin: string, peer: () => PeerWindow | null): PeerLink {
  const queue: Message[] = [];
  const waiters = new Set<() => void>();
  let ignored = 0;
  let stopped = false;
  const wake = () => {
    for (const w of [...waiters]) w();
  };
  const off = env.onMessage((e: PeerMessage) => {
    const p = peer();
    if (e.origin !== peerOrigin || !p || e.source !== p) {
      ignored++;
      return;
    }
    const m = asMessage(e.data, Object.values(MSG));
    if (!m) {
      ignored++;
      return;
    }
    queue.push(m);
    wake();
  });
  return {
    get ignored() {
      return ignored;
    },
    send(message) {
      const p = peer();
      if (!p || p.closed) return;
      try {
        p.postMessage({ v: 1, ...message }, peerOrigin);
      } catch {
        // a window going away: the next wait says so
      }
    },
    next(types, withinMs, id) {
      return new Promise<Message>((resolve, reject) => {
        let timer: ReturnType<typeof setTimeout> | undefined;
        let poll: ReturnType<typeof setInterval> | undefined;
        const finish = () => {
          waiters.delete(check);
          clearTimeout(timer);
          clearInterval(poll);
        };
        const check = () => {
          const i = queue.findIndex((m) => asMessage(m, types, id) !== null);
          if (i >= 0) {
            finish();
            resolve(queue.splice(i, 1)[0]!);
          } else if (stopped) {
            finish();
            reject(new LinkEnded("stopped"));
          }
        };
        waiters.add(check);
        check();
        if (!waiters.has(check)) return;
        timer = setTimeout(() => {
          finish();
          reject(new LinkEnded("timeout"));
        }, withinMs);
        poll = setInterval(() => {
          const p = peer();
          if (!p || p.closed) {
            // What it said before it closed still counts.
            check();
            if (!waiters.has(check)) return;
            finish();
            reject(new LinkEnded("closed"));
          }
        }, CLOSED_POLL_MS);
      });
    },
    stop() {
      stopped = true;
      off();
      wake();
    },
  };
}
