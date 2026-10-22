/*
 * What the two Noise handshake payloads carry (`rh-proto/src/handshake.rs` —
 * read that for the agent's order of checks).
 *
 * - Hello (device → agent): the device's chain view `(seq, head)`, its last
 *   few chain entries (so an agent the hub keeps behind can catch up from the
 *   device it is talking to), and client metadata. Never a request: the first
 *   message is replayable and open to key-compromise impersonation, so
 *   anything that changes state goes in the first transport message.
 * - Reply (agent → device): accepted or "send entries from N", the agent's
 *   own chain view (checked for a fork), and the entries after the device's
 *   view (up to 16) — merged here, so a device the hub keeps behind learns of
 *   a removal or a pending recovery from the agent itself.
 */
import { type Bytes, DecodeError, Reader, Writer } from "./bytes";
import { type ChainClock, ChainError, type ChainState, CONTROL_FRESHNESS_MS, decodeEntry, encodeEntry, type Entry } from "./chain";
import { Initiator, type NoiseKeyPair, type RekeyPolicy, type Transport } from "./noise";
import { type DirectBinding, directPrologue } from "./path";

export const MAX_HELLO_ENTRIES = 16;
export const MAX_INFO = 1024;
/**
 * How recently an admin device must have vouched for an agent's head for the
 * agent to open a channel at all (`rh-proto` `FRESHNESS_MAX_AGE_MS`): control's
 * window, seven days. Management inside a channel is held to
 * `MANAGE_FRESHNESS_MS` (`chain.ts`) by the agent.
 */
export const FRESHNESS_MAX_AGE_MS = CONTROL_FRESHNESS_MS;

export interface HelloBody {
  chainSeq: number;
  chainHead: Bytes;
  entries: Entry[];
  client: Bytes;
}

export function helloFor(chain: ChainState, client: Uint8Array): HelloBody {
  return { chainSeq: chain.seq, chainHead: chain.head, entries: chain.suffix(MAX_HELLO_ENTRIES), client: Uint8Array.from(client) };
}

/** Each entry behind its u16 length: one too long for it is refused, never cut (Rust's encoders refuse the same). */
function putEntries(w: Writer, entries: Entry[]): void {
  w.u16(entries.length);
  for (const e of entries) {
    const bytes = encodeEntry(e);
    if (bytes.length > 0xffff) throw new DecodeError(`a chain entry of ${bytes.length} bytes is too long for a handshake`);
    w.u16(bytes.length).raw(bytes);
  }
}

export function encodeHelloBody(b: HelloBody): Bytes {
  if (b.entries.length > MAX_HELLO_ENTRIES || b.client.length > MAX_INFO) throw new DecodeError("hello too large");
  const w = new Writer().u64(b.chainSeq).raw(b.chainHead);
  putEntries(w, b.entries);
  return w.u16(b.client.length).raw(b.client).finish();
}

export function decodeHelloBody(data: Uint8Array): HelloBody {
  const r = new Reader(data);
  const chainSeq = r.u64();
  const chainHead = r.bytes(32);
  const count = r.u16();
  if (count > MAX_HELLO_ENTRIES) throw new DecodeError("too many hello entries");
  const entries: Entry[] = [];
  for (let i = 0; i < count; i++) entries.push(decodeEntry(r.bytes(r.u16())));
  const len = r.u16();
  if (len > MAX_INFO) throw new DecodeError("client info too large");
  const client = r.bytes(len);
  r.finish();
  return { chainSeq, chainHead, entries, client };
}

/**
 * `stale`: no admin device has vouched for the agent's chain head recently —
 * send an attestation (an `attest` frame to the hub) and handshake again.
 */
export type ReplyStatus = "accepted" | "needEntries" | "stale";
const STATUS_TAGS: Record<ReplyStatus, number> = { accepted: 0, needEntries: 1, stale: 2 };
const STATUS_NAMES: ReplyStatus[] = ["accepted", "needEntries", "stale"];

export interface ReplyBody {
  status: ReplyStatus;
  chainSeq: number;
  chainHead: Bytes;
  needFrom: number;
  server: Bytes;
  /** The agent's entries after the device's view (at most {@link MAX_HELLO_ENTRIES}). */
  entries: Entry[];
}

export function encodeReplyBody(b: ReplyBody): Bytes {
  if (b.server.length > MAX_INFO) throw new DecodeError("server info too large");
  if (b.entries.length > MAX_HELLO_ENTRIES) throw new DecodeError("too many reply entries");
  const w = new Writer()
    .u8(STATUS_TAGS[b.status])
    .u64(b.chainSeq)
    .raw(b.chainHead)
    .u64(b.needFrom)
    .u16(b.server.length)
    .raw(b.server);
  putEntries(w, b.entries);
  return w.finish();
}

export function decodeReplyBody(data: Uint8Array): ReplyBody {
  const r = new Reader(data);
  const tag = r.u8();
  const status = STATUS_NAMES[tag];
  if (!status) throw new DecodeError(`unknown reply status ${tag}`);
  const body = { status, chainSeq: r.u64(), chainHead: r.bytes(32), needFrom: r.u64() };
  const len = r.u16();
  if (len > MAX_INFO) throw new DecodeError("server info too large");
  const server = r.bytes(len);
  const count = r.u16();
  if (count > MAX_HELLO_ENTRIES) throw new DecodeError("too many reply entries");
  const entries: Entry[] = [];
  for (let i = 0; i < count; i++) entries.push(decodeEntry(r.bytes(r.u16())));
  r.finish();
  return { ...body, server, entries };
}

export class HandshakeError extends Error {
  constructor(
    /** `notAccepted`: the host does not take control from this device (here: the device is the host itself). */
    readonly kind: "refused" | "fork" | "unknownMachine" | "unauthorized" | "notAccepted",
    message: string,
    readonly reply?: ReplyBody,
  ) {
    super(message);
    this.name = "HandshakeError";
  }
}

/** The device's first message, carrying its chain view. */
export async function startHandshake(
  chain: ChainState,
  local: NoiseKeyPair,
  device: string,
  machine: string,
  nowMs: number,
  client: Uint8Array,
): Promise<{ initiator: Initiator; message: Bytes }> {
  // One identity per device (chain v3): a host never opens a channel to itself.
  if (device === machine) throw new HandshakeError("notAccepted", "a device does not control itself");
  const m = chain.machine(machine);
  if (!m) throw new HandshakeError("unknownMachine", `no current device ${machine} that runs AI in the chain`);
  const initiator = await Initiator.create(local, m.info.dhKey, { account: chain.account, device, machine });
  return { initiator, message: await initiator.writeHello(nowMs, encodeHelloBody(helloFor(chain, client))) };
}

/**
 * The device's first message on a direct path (`path.ts`), under the direct
 * binding's prologue; the agent admits it exactly as it admits a hub channel.
 * The reply is read with {@link finishHandshake}.
 */
export async function startDirectHandshake(
  chain: ChainState,
  local: NoiseKeyPair,
  bind: DirectBinding,
  nowMs: number,
  client: Uint8Array,
): Promise<{ initiator: Initiator; message: Bytes }> {
  if (bind.binding.device === bind.binding.machine) throw new HandshakeError("notAccepted", "a device does not control itself");
  const m = chain.machine(bind.binding.machine);
  if (!m) throw new HandshakeError("unknownMachine", `no current device ${bind.binding.machine} that runs AI in the chain`);
  if (bind.binding.account !== chain.account) throw new HandshakeError("unauthorized", "a direct binding for another account");
  const initiator = await Initiator.withPrologue(local, m.info.dhKey, directPrologue(bind));
  return { initiator, message: await initiator.writeHello(nowMs, encodeHelloBody(helloFor(chain, client))) };
}

/**
 * The agent's reply. The entries it carries that this device lacks are merged
 * into `chain` (on the device's own `clock`) whatever the status — a refusal
 * can be exactly how a device learns it was removed, or that someone is
 * recovering into the account. Then: a fork is an alarm; anything but
 * "accepted" is a refusal carrying the agent's view.
 */
export async function finishHandshake(
  initiator: Initiator,
  chain: ChainState,
  clock: ChainClock,
  message: Uint8Array,
  policy: RekeyPolicy,
  nowMs: number,
): Promise<{ transport: Transport; reply: ReplyBody; learned: number }> {
  const { transport, payload } = await initiator.readReply(message, policy, nowMs);
  const reply = decodeReplyBody(payload);
  clock.nowMs = nowMs;
  let learned = 0;
  for (const entry of reply.entries) {
    if (entry.draft.seq > chain.seq + 1) break;
    try {
      learned += await chain.merge([entry], clock);
    } catch (error) {
      if (error instanceof ChainError && error.kind === "fork") throw new HandshakeError("fork", error.message, reply);
      break;
    }
  }
  const mine = chain.hashAt(reply.chainSeq);
  if (mine && !mine.every((b, i) => b === reply.chainHead[i])) throw new HandshakeError("fork", `the agent's chain forks from ours at ${reply.chainSeq}`, reply);
  if (reply.status === "stale") throw new HandshakeError("refused", "the agent needs a recent attestation of its chain head", reply);
  if (reply.status !== "accepted") throw new HandshakeError("refused", `the agent needs chain entries from ${reply.needFrom}`, reply);
  return { transport, reply, learned };
}
