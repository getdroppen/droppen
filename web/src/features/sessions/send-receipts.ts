import type { components } from "../../api/schema.js";

/**
 * Idempotent sends, from the composer's side.
 *
 * # The bug
 *
 * A phone POSTs a message, the server takes it, and the answer is lost on the
 * way back. The composer used to read every failure the same way — "not sent"
 * — so it put the message back in the queue, restored the draft, and the retry
 * ran it a second time.
 *
 * # The fix
 *
 * Every message is named once, when it is created (`clientMessageId`), and the
 * name travels with it through every retry; the server answers a name it has
 * already accepted with the first answer and enqueues nothing. What the
 * composer then has to stop doing is *guessing*: a send that got no answer is
 * neither sent nor failed, it is **unknown**, and it stays that way — never
 * retried by the drain — until the server says which it was
 * (`GET …/receipts?ids=`): seen, and the composer adopts what the server says
 * happened to it; never seen, and it is safe to send again, under the same name.
 *
 * Everything here is pure, so the state machine can be tested without a DOM.
 */

export type ReceiptStatus = components["schemas"]["ReceiptStatus"];
export type MessageReceipt = components["schemas"]["MessageReceipt"];
export type ReceiptLookup = components["schemas"]["ReceiptLookup"];

/**
 * Where an outgoing message is, as the UI says it.
 *
 * - `saved` — only on this device; nothing has been sent (已保存).
 * - `sending` — a POST is in flight.
 * - `delivered` — the server has it, waiting in its queue (已送达).
 * - `executed` — handed to the agent (已执行).
 * - `unknown` — sent, and no answer came back (未知).
 * - `unsent` — the server says it never saw it: safe to send again (未送达).
 * - `failed` — refused, or dropped by the server without being delivered.
 */
export type DeliveryState = "saved" | "sending" | "delivered" | "executed" | "unknown" | "unsent" | "failed";

/** The states a message can be in while this device still holds it. */
export type OutboxState = "saved" | "sending" | "unknown" | "unsent" | "failed";

/** One message this tab is holding: its outbox, persisted in localStorage. */
export interface OutgoingMessage {
  /** Minted when the message was created, and kept through every retry. */
  clientMessageId: string;
  text: string;
  images?: string[];
  files?: string[];
  /** Absent means `saved`, which is every entry an older build wrote. */
  state?: OutboxState;
}

/**
 * A UUID v4, without needing a secure context.
 *
 * `crypto.randomUUID` only exists on HTTPS and localhost, and this app is just
 * as often reached over plain HTTP on a LAN; `getRandomValues` is available on
 * both. `Math.random` is the last resort for an environment with neither.
 */
export function newClientMessageId(source: Pick<Crypto, "getRandomValues"> | undefined = globalThis.crypto): string {
  const bytes = new Uint8Array(16);
  if (source?.getRandomValues) source.getRandomValues(bytes);
  else for (let i = 0; i < 16; i++) bytes[i] = Math.floor(Math.random() * 256);
  bytes[6] = (bytes[6]! & 0x0f) | 0x40;
  bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

const OUTBOX_STATES: readonly OutboxState[] = ["saved", "sending", "unknown", "unsent", "failed"];

/**
 * The outbox as it comes back out of localStorage.
 *
 * Tolerant of every shape an older build wrote — plain strings, objects with
 * no id — and minting an id for any entry without one: such an entry was never
 * sent under a name, so a fresh one cannot collide with anything.
 *
 * An entry that says `sending` was in flight when the page went away; nobody
 * will ever hear its answer, so it comes back `unknown`, to be asked about.
 */
export function readOutbox(parsed: unknown, mint: () => string = newClientMessageId): OutgoingMessage[] {
  if (!Array.isArray(parsed)) return [];
  const strings = (value: unknown) =>
    Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : undefined;
  return parsed.flatMap((raw): OutgoingMessage[] => {
    if (typeof raw === "string") return [{ clientMessageId: mint(), text: raw }];
    if (typeof raw !== "object" || raw === null) return [];
    const entry = raw as Record<string, unknown>;
    if (typeof entry.text !== "string") return [];
    const id = typeof entry.clientMessageId === "string" && entry.clientMessageId ? entry.clientMessageId : mint();
    const stored = OUTBOX_STATES.find((state) => state === entry.state);
    const state: OutboxState | undefined = stored === "sending" ? "unknown" : stored;
    const images = strings(entry.images);
    const files = strings(entry.files);
    return [
      {
        clientMessageId: id,
        text: entry.text,
        ...(images ? { images } : {}),
        ...(files ? { files } : {}),
        ...(state && state !== "saved" ? { state } : {}),
      },
    ];
  });
}

/**
 * Two stored outboxes of one session as one, for a claim that finds both the
 * machine's (`mine`) and one under the key from before machines had keys
 * (`old`), written by a tab still on an older build (`lib/machines/storage.ts`).
 * Either can be the newer — the outbox is written straight to the machine's
 * key, without claiming again — so neither replaces the other:
 *
 * - every entry of both is kept: the machine's in their order, then the old
 *   one's it does not have, in theirs. An entry is known by its
 *   `clientMessageId` (one from before ids, by its whole stored form), and the
 *   id travels with it, so an entry one side already sent is answered by the
 *   server, not run again (for the week the server remembers an id,
 *   `KEEP_FOR_MS` in `server-rs/src/inject/receipts.rs`);
 * - an entry on both sides is the machine's, unless the old tab may already
 *   have sent it (`sending`, `unknown`) while the machine's has not (`saved`):
 *   then the old one, so the server is asked about it before it goes again;
 * - a side that is not a stored outbox (not JSON, not a list) is not merged
 *   into anything, and the other side is kept as it was.
 *
 * Works on the stored form, before {@link readOutbox}, so nothing is minted
 * or rewritten that the next read would not do anyway.
 */
export function mergeStoredOutboxes(mine: string, old: string): string {
  const parse = (raw: string): unknown[] | null => {
    try {
      const parsed: unknown = JSON.parse(raw);
      return Array.isArray(parsed) ? parsed : null;
    } catch {
      return null;
    }
  };
  const ours = parse(mine);
  const theirs = parse(old);
  if (theirs === null) return mine;
  if (ours === null) return old;
  const field = (entry: unknown, name: string): unknown =>
    typeof entry === "object" && entry !== null ? (entry as Record<string, unknown>)[name] : undefined;
  const identity = (entry: unknown): string => {
    const id = field(entry, "clientMessageId");
    return typeof id === "string" && id ? `id:${id}` : `raw:${JSON.stringify(entry)}`;
  };
  const maybeSent = (entry: unknown) => {
    const state = field(entry, "state");
    return state === "sending" || state === "unknown";
  };
  const notSent = (entry: unknown) => {
    const state = field(entry, "state");
    return state === undefined || state === "saved";
  };
  const merged = [...ours];
  const at = new Map(ours.map((entry, index) => [identity(entry), index]));
  for (const entry of theirs) {
    const index = at.get(identity(entry));
    if (index === undefined) merged.push(entry);
    else if (maybeSent(entry) && notSent(merged[index])) merged[index] = entry;
  }
  return JSON.stringify(merged);
}

/** An entry's state, reading the absent one as what it means. */
export function outboxState(entry: OutgoingMessage): OutboxState {
  return entry.state ?? "saved";
}

/** A server receipt as the UI says it. */
export function receiptState(status: ReceiptStatus): DeliveryState {
  switch (status) {
    case "queued":
      return "delivered";
    case "sent":
      return "executed";
    case "failed":
      return "failed";
  }
}

/** How a send ended, when it did not end with a `200`. */
export type SendFailure =
  | { kind: "network" }
  | { kind: "timeout" }
  | { kind: "http"; status: number; body: unknown };

/** The `error` the server answers a retry with while the first attempt runs. */
export const IN_PROGRESS = "in_progress";

/**
 * Whether a failed send is a **refusal** — the server answered, and nothing
 * was enqueued — or **unknown**: the request may or may not have arrived.
 *
 * Only an answer the server can be seen to have written is a refusal. A
 * network error or a timeout never reached a verdict; a `5xx` with no body this
 * server writes is a proxy or a crash talking, not a decision; and
 * `in_progress` is the server itself saying the first attempt is still going.
 */
export function classifySendFailure(failure: SendFailure): "refused" | "unknown" {
  if (failure.kind !== "http") return "unknown";
  const body = failure.body as { error?: unknown } | null;
  const code = typeof body === "object" && body !== null && typeof body.error === "string" ? body.error : null;
  if (code === IN_PROGRESS) return "unknown";
  if (failure.status >= 500 && code === null) return "unknown";
  return "refused";
}

/** `entries` with the one named `id` changed, everything else untouched. */
export function withState(entries: readonly OutgoingMessage[], id: string, state: OutboxState): OutgoingMessage[] {
  return entries.map((entry) => {
    if (entry.clientMessageId !== id) return entry;
    if (state === "saved") {
      const { state: _state, ...rest } = entry;
      return rest;
    }
    return { ...entry, state };
  });
}

/** `entries` without the one named `id`. */
export function withoutMessage(entries: readonly OutgoingMessage[], id: string): OutgoingMessage[] {
  return entries.filter((entry) => entry.clientMessageId !== id);
}

/**
 * The outbox after a send of `id` ended.
 *
 * - accepted: the message is the server's now, and leaves the outbox.
 * - unknown: it stays, as `unknown`, and nothing retries it.
 * - refused: back to `saved` where it was — unless its text has gone back
 *   into the composer as a draft, in which case keeping it too would be the
 *   same message twice.
 */
export function afterSend(
  entries: readonly OutgoingMessage[],
  id: string,
  outcome: "accepted" | "refused" | "unknown",
  draftRestored = false,
): OutgoingMessage[] {
  if (outcome === "accepted" || (outcome === "refused" && draftRestored)) return withoutMessage(entries, id);
  return withState(entries, id, outcome === "unknown" ? "unknown" : "saved");
}

/** The ids the server should be asked about, oldest first, at most 100. */
export function unknownIds(entries: readonly OutgoingMessage[]): string[] {
  return entries
    .filter((entry) => outboxState(entry) === "unknown")
    .map((entry) => entry.clientMessageId)
    .slice(0, 100);
}

/** A message the server turned out to have, with what it says became of it. */
export interface AdoptedMessage {
  message: OutgoingMessage;
  receipt: MessageReceipt;
}

/**
 * Settle the outbox's `unknown` entries against what the server knows.
 *
 * - a receipt says the server has it: `queued` and `sent` leave the outbox
 *   and are handed back as `adopted`, for the caller to show where a sent
 *   message is shown; `failed` stays, as `failed`, for the user to decide.
 * - listed as never seen: `unsent`, which is safe to send again under the
 *   same id.
 * - neither: still unknown — its first attempt may still be arriving.
 *
 * `lookup.unknown` is empty when the receipts came from a state frame, which
 * only ever lists what the server has.
 */
export function resolveUnknowns(
  entries: readonly OutgoingMessage[],
  lookup: { receipts: readonly MessageReceipt[]; unknown?: readonly string[] },
): { outbox: OutgoingMessage[]; adopted: AdoptedMessage[]; changed: boolean } {
  const adopted: AdoptedMessage[] = [];
  let changed = false;
  const outbox = entries.flatMap((entry): OutgoingMessage[] => {
    if (outboxState(entry) !== "unknown") return [entry];
    const receipt = lookup.receipts.find((candidate) => candidate.clientMessageId === entry.clientMessageId);
    if (receipt) {
      changed = true;
      if (receipt.status === "failed") return [{ ...entry, state: "failed" }];
      const { state: _state, ...message } = entry;
      adopted.push({ message, receipt });
      return [];
    }
    if (lookup.unknown?.includes(entry.clientMessageId)) {
      changed = true;
      return [{ ...entry, state: "unsent" }];
    }
    return [entry];
  });
  return { outbox: changed ? outbox : [...entries], adopted, changed };
}

/**
 * The entry to send again, and under which id.
 *
 * `unsent` keeps its id — the server never saw it, and if it did after all
 * the id is what stops it running twice. `failed` gets a new one: the server
 * has an answer on file for the old id, and would only replay it.
 */
export function prepareResend(entry: OutgoingMessage, mint: () => string = newClientMessageId): OutgoingMessage {
  const { state: _state, ...message } = entry;
  return outboxState(entry) === "failed" ? { ...message, clientMessageId: mint() } : message;
}

/** Whether the drain may take the head of the outbox: only a `saved` one. */
export function headSendable(entries: readonly OutgoingMessage[]): boolean {
  const head = entries[0];
  return head !== undefined && outboxState(head) === "saved";
}

/**
 * A send that ended without an answer this client can act on.
 *
 * Deliberately not an `ApiError`: nothing about it is a refusal, and the code
 * that turns refusals into "adopt this session" buttons must not see one.
 */
export class SendUnknown extends Error {
  readonly failure: SendFailure;
  constructor(failure: SendFailure) {
    super(failure.kind === "http" ? `no usable answer (HTTP ${failure.status})` : `no answer (${failure.kind})`);
    this.name = "SendUnknown";
    this.failure = failure;
  }
}

/** How long a send may go without an answer before it is called unknown. */
export const SEND_TIMEOUT_MS = 30_000;

/** An abort signal that fires after `ms`, on browsers without `AbortSignal.timeout`. */
export function timeoutSignal(ms: number = SEND_TIMEOUT_MS): AbortSignal {
  if (typeof AbortSignal.timeout === "function") return AbortSignal.timeout(ms);
  const controller = new AbortController();
  setTimeout(() => controller.abort(new DOMException("timed out", "TimeoutError")), ms);
  return controller.signal;
}

/** What a thrown `fetch` failure was: a timeout, or the network. */
export function thrownFailure(error: unknown): SendFailure {
  const name = (error as { name?: unknown } | null)?.name;
  return name === "TimeoutError" || name === "AbortError" ? { kind: "timeout" } : { kind: "network" };
}
