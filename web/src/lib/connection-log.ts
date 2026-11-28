/*
 * The connection log: what this page's way to its devices did, for the owner
 * to read and to send for analysis (the bar's "connection log", and the row
 * under 「这台设备」: `features/cloud/connection-log.tsx`).
 *
 * The hub link, the relays, each device's end-to-end channel and its paths,
 * the requests over it, the bar's words and the page's own state (network,
 * visibility) say what happened as it happens: {@link ConnectionLog.emit},
 * one structured event each — a name from {@link CONN_EVENTS}, a monotonic
 * `t` (ms since the page loaded, `performance.now`), the wall clock, and a
 * flat object of fields. Once a second while a connection is up, the
 * sampler ({@link ConnectionLog.addSampler}) adds one `sample` of the bytes
 * that moved and of what waits in each buffer; a second with nothing moving
 * and nothing waiting is not recorded (one every {@link QUIET_SAMPLE_MS}
 * still is, so a quiet stretch reads as quiet, not as a gap).
 *
 * Bounded, in memory: the last {@link MAX_EVENTS} events or
 * {@link MAX_AGE_MS}, whichever is less, with the requests
 * ({@link MAX_REQUESTS}) and the chatter — samples and probes, which come
 * every second ({@link MAX_CHATTER}) — in rings of their own, so neither
 * pushes out a close or a timeout. When
 * the page goes away (a reload during a reconnect loop) the newest of it is
 * kept in `sessionStorage` (at most {@link PERSIST_BYTES}) and read back by
 * the next page of the tab ({@link ConnectionLog.restore}), marked as the
 * page before.
 *
 * Cost: an event is a small object pushed into an array; nothing is logged
 * per frame — only per event, and the samples. Logging never throws and
 * never changes what it logs: no timer, order or decision depends on it.
 *
 * PRIVACY — what may go into an event, and what never does. An export is
 * pasted into chats and issues: a leak there cannot be taken back.
 *
 * - Never a token, key, ticket, signature, cookie or session (hub or device),
 *   nothing of what a session or a message says, no file or folder name or
 *   path, no device or account name, no email, no IP address or host name,
 *   no SDP or ICE candidate, and no full URL.
 * - Free text — a reason, a cause, an error's words ({@link FREE_TEXT}) — is
 *   kept only when it is Droppen's own words ({@link ownWords}: an
 *   allowlist of what this program and the relay say). Anything else — what
 *   a hub, a relay or an agent says, a browser's error, which may quote an
 *   SDP line with an IP address in it — is `<withheld>`, with its length;
 *   an error also says its class and code ({@link errorFields}).
 * - A request is its method and its route template, ids masked
 *   (`/api/sessions/{source}/{id}/history`, {@link routeTemplate}); of its
 *   query, the keys the API names alone, and the value only of the numeric
 *   positions and counts ({@link NUMERIC_QUERY}: `before=1200`).
 * - A device's id is a short hash ({@link maskId}: `#3fa9c1`) salted with a
 *   random value this browser keeps: the same on every export of this
 *   browser, so one device can be followed through a log, and neither
 *   readable nor guessable. The account's, this device's and each machine's
 *   ids are masked wherever they turn up, in any case ({@link ConnectionLog.hide}).
 *   Relay ids (`nrt-1`) and close codes are infrastructure, kept as they are.
 * - Every other string field is the log's own (a link's name, a state, a
 *   relay's round trips): kept in its set shape ({@link SHAPED}), else put
 *   through {@link safeText}, the second net — URLs, emails, IP addresses,
 *   candidates and SDP, paths, file and host names, anything token-shaped.
 */

/** How bad an event is: `warn` and `error` are what the log's "problems" view keeps. */
export type ConnLevel = "info" | "warn" | "error";

/**
 * Every event the log knows, and its level unless the emitter says
 * otherwise. To log something new, name it here (`area.what`) and emit it
 * with whatever fields it needs.
 */
export const CONN_EVENTS = {
  // The page itself: its start, the network, being in view, its timers held.
  "page.start": "info",
  "page.visible": "info",
  "page.hidden": "info",
  "page.show": "info",
  "page.hide": "info",
  "page.pause": "warn",
  "net.change": "info",
  "net.online": "info",
  "net.offline": "warn",
  // The hub's websocket (`lib/hub/link.ts`) and its HTTP API (`lib/hub/api.ts`).
  "hub.connecting": "info",
  "hub.open": "info",
  "hub.close": "warn",
  "hub.error": "warn",
  "hub.retry": "info",
  "hub.renew": "info",
  "hub.presence": "info",
  "hub.noChannels": "warn",
  "hub.req": "info",
  // A machine's offline copy downloading a checkpoint's pages from the hub (`lib/hub/streams.ts`): its start
  // (how many pages, in which lane, and why), how far it is at most every 10 s, and its end (bytes, time, how it ended).
  "stream.checkpoint": "info",
  "stream.progress": "info",
  "stream.done": "info",
  // The page's queue of those downloads (`lib/hub/page-queue.ts`): background held back for live traffic and let go
  // again, and the limits it runs at as the link changes.
  "pages.pause": "info",
  "pages.resume": "info",
  "pages.limits": "info",
  // Either link (the hub's, a relay's): a beat far later than due (the page's timers were held), the page back.
  "link.late": "warn",
  "link.wake": "info",
  // A timer that fired and gave something up: which constant, after how long, on what.
  timeout: "error",
  // Relays (`lib/hub/relays.ts`, `relay-link.ts`).
  "relay.list": "info",
  "relay.measure": "info",
  "relay.choose": "info",
  "relay.ticket": "info",
  "relay.connecting": "info",
  "relay.open": "info",
  "relay.close": "warn",
  "relay.goaway": "warn",
  "relay.failure": "warn",
  // A device and its end-to-end channel (`lib/hub/machine.ts`), the page between it and its copy (`lib/transport/switching.ts`).
  "device.select": "info",
  "channel.status": "info",
  "channel.step": "info",
  "channel.try": "info",
  "channel.retry": "info",
  "channel.refused": "warn",
  "channel.open": "info",
  "channel.close": "warn",
  // A request or socket the channel could not open a stream for (a head too large, every stream in use): refused here, the channel fine.
  "pipe.refused": "warn",
  "handshake.start": "info",
  "handshake.done": "info",
  "handshake.fail": "warn",
  "path.switch": "info",
  "path.slow": "warn",
  // The agent's word that it let the direct path go, come after this page had (its reason for it).
  "path.peerDown": "warn",
  "mode.switch": "info",
  "reconnect.replay": "info",
  // The direct path (`lib/transport/p2p.ts`).
  "direct.state": "info",
  "direct.try": "info",
  "direct.up": "info",
  "direct.fail": "warn",
  "direct.down": "warn",
  "direct.retry": "info",
  // Liveness probes, on every link and path: one sent, its answer (a measured round trip), and a hub
  // probe out a whole probe interval, said once (`probe.late`: its age and the path's quiet, no round trip).
  "probe.sent": "info",
  "probe.answer": "info",
  "probe.late": "warn",
  // What the bar says.
  bar: "info",
  // Traffic, once a second while connected.
  sample: "info",
  // Requests and sockets over a channel (`lib/transport/tunnel.ts`).
  "req.start": "info",
  req: "info",
  "socket.open": "info",
  "socket.close": "info",
  // The local gateway's socket (`lib/gateway/link.ts`).
  "gateway.connecting": "info",
  "gateway.open": "info",
  "gateway.close": "warn",
  "gateway.retry": "info",
} as const satisfies Record<string, ConnLevel>;

export type ConnEventName = keyof typeof CONN_EVENTS;
export type ConnValue = string | number | boolean | null;
export type ConnFields = Readonly<Record<string, ConnValue | undefined>>;

export interface ConnEvent {
  /** Order of logging (on this page). */
  readonly seq: number;
  /** Milliseconds since the page loaded (monotonic; of the page before, for one restored from it). */
  readonly t: number;
  /** Wall-clock milliseconds. */
  readonly wall: number;
  readonly name: ConnEventName;
  readonly level: ConnLevel;
  readonly fields: Readonly<Record<string, ConnValue>>;
  /** Restored from the page before a reload: when that page loaded (wall-clock ms). */
  readonly prev?: number;
}

/** The newest events kept (requests apart: {@link MAX_REQUESTS}; samples and probes apart: {@link MAX_CHATTER}). */
export const MAX_EVENTS = 2000;
/** The newest requests and sockets kept: a page's burst of calls never pushes out a close or a timeout. */
export const MAX_REQUESTS = 2000;
/**
 * The newest samples and probes kept: enough for {@link MAX_AGE_MS} at their
 * busiest — a sample a second, a direct path's probe and answer every second,
 * the carrier's every five, the links' pings — about four a second.
 */
export const MAX_CHATTER = 7200;
/** Nothing older than this is kept. */
export const MAX_AGE_MS = 30 * 60_000;
/** How often the sampler looks. */
export const SAMPLE_MS = 1000;
/** A sample is recorded at least this often while connected, even with nothing moving. */
export const QUIET_SAMPLE_MS = 15_000;
/** The most of the log (JSON) kept in `sessionStorage` for the next page of the tab. */
export const PERSIST_BYTES = 256 * 1024;
const PERSIST_KEY = "rh.cloud.connection-log";

/** The events that come every second or so (samples and probes), kept in a ring of their own. */
export const CHATTER: ReadonlySet<ConnEventName> = new Set<ConnEventName>(["sample", "probe.sent", "probe.answer"]);
/** Requests and sockets, kept in a ring of their own. */
export const REQUESTS: ReadonlySet<ConnEventName> = new Set<ConnEventName>(["req", "req.start", "hub.req", "socket.open", "socket.close"]);

/** A ring of the newest `capacity` events. */
class Ring {
  private readonly items: (ConnEvent | undefined)[];
  private start = 0;
  size = 0;

  constructor(readonly capacity: number) {
    this.items = new Array<ConnEvent | undefined>(capacity);
  }

  push(e: ConnEvent): void {
    if (this.size < this.capacity) {
      this.items[(this.start + this.size) % this.capacity] = e;
      this.size++;
      return;
    }
    this.items[this.start] = e;
    this.start = (this.start + 1) % this.capacity;
  }

  /** Forget the events older than `wall`. */
  dropBefore(wall: number): void {
    while (this.size > 0) {
      const first = this.items[this.start]!;
      if (first.wall >= wall) return;
      this.items[this.start] = undefined;
      this.start = (this.start + 1) % this.capacity;
      this.size--;
    }
  }

  toArray(): ConnEvent[] {
    const out: ConnEvent[] = [];
    for (let i = 0; i < this.size; i++) out.push(this.items[(this.start + i) % this.capacity]!);
    return out;
  }

  clear(): void {
    this.items.fill(undefined);
    this.start = 0;
    this.size = 0;
  }
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/** Where this browser keeps the salt its masks are made with (`localStorage`). */
const SALT_KEY = "rh.cloud.connection-log-salt";
let salt: string | null = null;

/** This browser's own random salt for {@link maskId}, made once and kept (in memory only where there is no storage). */
function maskSalt(): string {
  if (salt !== null) return salt;
  let kept: string | null = null;
  try {
    kept = globalThis.localStorage?.getItem(SALT_KEY) ?? null;
  } catch {
    kept = null;
  }
  if (!kept || !/^[0-9a-f]{32}$/.test(kept)) {
    const bytes = new Uint8Array(16);
    try {
      globalThis.crypto.getRandomValues(bytes);
    } catch {
      for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
    }
    kept = [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
    try {
      globalThis.localStorage?.setItem(SALT_KEY, kept);
    } catch {
      // this page's alone, then
    }
  }
  salt = kept;
  return kept;
}

/** Use `next` as the masks' salt (tests). */
export function setMaskSalt(next: string): void {
  salt = next;
}

/**
 * A short stand-in for an id (a device's): `#` and six hex digits of the
 * FNV-1a hash of this browser's random salt and the id, in lower case. The
 * same id gives the same mask on every export of this browser, so one device
 * can be followed through a log; another browser gives it another, and
 * without the salt (which never leaves this browser) the id can be neither
 * read back nor confirmed by guessing.
 */
export function maskId(id: string | null | undefined): string | null {
  if (!id) return null;
  const text = `${maskSalt()}:${id.toLowerCase()}`;
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return `#${(h >>> 8).toString(16).padStart(6, "0")}`;
}

/** The fields that hold free text — a reason, a cause, an error's words: kept only when they are Droppen's own words ({@link ownWords}). */
export const FREE_TEXT: ReadonlySet<string> = new Set(["reason", "why", "error", "detail"]);

/**
 * The fields that hold a code, a state or a name from a short list — a
 * ticket's refusal, a goodbye's reason, an error's class, a state, a relay's
 * id or city, a timer's name: kept when they look like one (a word, no
 * spaces), withheld like free text otherwise.
 */
export const CODE_FIELDS: ReadonlySet<string> = new Set([
  "refused",
  "bye",
  "errName",
  "errCode",
  "errKind",
  "pc",
  "stage",
  "feedResume",
  "city",
  "label",
  "step",
  "state",
  "relay",
  "status",
  "kind",
  "type",
  "mode",
  "timer",
  "rule",
  "by",
  "path",
  "method",
  "cause",
  "decision",
  "lane",
  "basis",
]);

/**
 * Droppen's own words for a reason, a failure or an error, as this program,
 * the relay (`droppen-relay-core`) and the browser's network errors say them:
 * the only free text the log keeps. Anything else in a free-text field — a
 * hub's or an agent's reason, a browser's error, which may quote an SDP line
 * and its IP address — is withheld: its length is kept, never its words. An
 * id the log knows ({@link ConnectionLog.hide}) is already its mask here.
 */
const OWN_WORDS: readonly RegExp[] = [
  // The hub's link.
  /^(?:the hub went silent|the wake-up ping went unanswered|bye|bad challenge|the hub is not reachable|the connection to the hub was lost)$/,
  // A relay's link and the pool.
  /^(?:the relay went silent|the relay did not answer the wake-up ping|done|timeout|silent|no ticket|socket error|the relay did not let this device in in time|the connection to the relay was lost)$/,
  /^the relay closed the connection \((?:\d{1,4}|\?)\)$/,
  /^closed \((?:\d{1,4}|\?)\)$/,
  /^no ticket for [\w.-]{1,40}: [\w.-]{1,40}$/,
  /^the manual choice gave way to automatic: (?:gone|failed)$/,
  // What the relay itself closes with.
  /^(?:no heartbeat|signed in again elsewhere|not reading fast enough|one whole frame per binary message|not on a relay|a malformed or unknown critical frame|a malformed frame)$/,
  // A device's channel.
  /^(?:the relay path stopped answering|moved|the device renewed its key|a renewed key|the device did not answer|the device is not on this relay|the device could not save its record of who connects to it|the device is offline|the device refused the channel|this device has no account chain|waiting for the hub to say where the device is|no relay available|the device's agent is too old to be reached|the device is no longer open on this page|the relay refused the channel|the hub refused the channel)$/,
  /^#[0-9a-f]{6} is isolated: renew its identity key on the device first$/,
  /^(?:a device does not control itself|a direct binding for another account|the agent needs a recent attestation of its chain head)$/,
  /^no current device #[0-9a-f]{6} that runs AI in the chain$/,
  /^the agent needs chain entries from \d{1,12}$/,
  /^the agent's chain forks from ours at \d{1,12}$/,
  // The direct path.
  /^(?:the data channel closed|the data channel failed|the data channel opened before an answer|a text message on the data channel|a message before the handshake|the DTLS certificate is not the one the answer stated|the signaling stream is gone|the signaling stream closed|the agent did not answer the signaling stream|the channel closed|direct paths are turned off on this device|no direct path)$/,
  /^the peer connection (?:failed|closed)$/,
  /^no direct path within \d{1,4} s$/,
  /^the direct path (?:timed out|failed|closed)$/,
  /^the agent answered the signaling stream with \d{3}$/,
  /^the first direct message is a (?:hello|reply|sealed), not a reply$/,
  // Requests over a channel.
  /^tunnel: the pipe closed before the answer ended \(\d{1,5}\)$/,
  /^(?:tunnel: the answer did not start with a head|aborted|The operation was aborted\.|signal is aborted without reason)$/,
  /^the request body is larger than \d{1,12} bytes$/,
  // A page download let go of (`lib/hub/streams.ts`, `page-queue.ts`): the request aborted, in the program's words.
  /^(?:the subscription was closed|another page of its download failed|the page queue is closed|the page stalled)$/,
  // A pipe the channel could not open (`pipe.refused`): the multiplexer's and the head encoder's own words.
  /^(?:open metadata too large|too many open streams|string too long)$/,
  // The browser's words for a request that never got an answer (`fetch`'s own TypeError).
  /^(?:Failed to fetch|Load failed|NetworkError when attempting to fetch resource\.|Network request failed|The network connection was lost\.)$/,
  // The log's own words for why it says something.
  /^(?:move|moved|move failed|switch|channel open)$/,
];

/** Droppen's own words before a cause it did not write (another error's words): the words are kept, the cause withheld. */
const OWN_PREFIXES: readonly string[] = [
  "no offer: ",
  "the answer was not taken: ",
  "no direct handshake: ",
  "the direct handshake failed: ",
  "the signaling stream broke: ",
  "no peer connection: ",
  "the relay refused the channel: ",
  "the hub refused the channel: ",
];

/** `text` if it is Droppen's own words ({@link OWN_WORDS}), its own words alone if a cause follows them, else `null`. */
export function ownWords(text: string): string | null {
  const t = text.trim();
  if (t === "" || t === "<withheld>") return t;
  if (t.length > 200) return null;
  for (const re of OWN_WORDS) if (re.test(t)) return t;
  for (const p of OWN_PREFIXES) if (t.startsWith(p)) return `${p}<withheld>`;
  return null;
}

/**
 * The second net, under the allowlist: what could identify or unlock
 * anything, replaced wherever it turns up in a field that is not free text
 * (and in what of free text the allowlist kept). In order: URLs; emails;
 * ICE candidates and SDP lines; IP addresses, v6 (whole address, before v4
 * so IPv4-mapped addresses collapse to one token) and v4; UUIDs; paths (from
 * a `/`, `~`, a drive or a share to the end of the quote or line, spaces and
 * all), percent-encoded ones; JWTs; host names with explicit local/tailnet
 * suffixes or multiple dots (before file names so `host.tail1234.ts.net` is
 * never split on `.ts`); file names with an extension; remaining host names;
 * long hex; mixed alphanumeric tokens (8+ chars); mixed-case letter tokens;
 * and long letter-only or token-alphabet runs.
 */
const SAFE_RULES: [RegExp, string][] = [
  [/\b[a-z][a-z0-9+.-]*:\/\/[^\s"'<>)]*/gi, "<url>"],
  [/[\w.+-]+@[\w-]+(?:\.[\w-]+)+/g, "<email>"],
  [/\b(?:a=)?candidate:[^\r\n"]*/gi, "<candidate>"],
  [/\b(?:c=IN IP[46]|a=[a-z-]+:|o=\S+ \d|m=(?:application|audio|video))[^\r\n"]*/g, "<sdp>"],
  [
    /(?:\[(?:(?:[0-9a-f]{1,4}:){3,7}[0-9a-f]{1,4}|(?:[0-9a-f]{1,4}:)*[0-9a-f]{0,4}::(?:(?:[0-9a-f]{1,4}:)*(?:(?:\d{1,3}\.){3}\d{1,3}|[0-9a-f]{1,4}))?)(?:%[\w.-]+)?\](?::\d{1,5})?|\b(?:[0-9a-f]{1,4}:){3,7}[0-9a-f]{1,4}(?:%[\w.-]+)?\b|(?<![A-Za-z0-9_])(?:(?:[0-9a-f]{1,4}:)+[0-9a-f]{1,4}|[0-9a-f]{1,4})?::(?:(?:[0-9a-f]{1,4}:)*(?:(?:\d{1,3}\.){3}\d{1,3}|[0-9a-f]{1,4}))?(?:%[\w.-]+)?(?![A-Za-z0-9_]))/gi,
    "<ip>",
  ],
  [/(?<![A-Za-z]\/)\b(?:\d{1,3}\.){3}\d{1,3}(?::\d{1,5})?\b/g, "<ip>"],
  [/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi, "<uuid>"],
  [/(?:^|(?<=[\s"'(=:,;[]))(?:~[\\/]|[A-Za-z]:\\|\\\\|\/)[^"'\r\n]*/g, "<path>"],
  [/\S*%[0-9A-Fa-f]{2}\S*/g, "<path>"],
  [/\b[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}(?:\.[A-Za-z0-9_-]{4,})*/g, "<redacted>"],
  [/\b(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+(?:local|lan|home|internal|localdomain|ts\.net)\b/gi, "<host>"],
  [/\b(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.){2,}[a-z]{2,24}\b/gi, "<host>"],
  [/[\w()-]+(?: [\w()-]+)*\.(?:txt|md|markdown|json|jsonl|toml|ya?ml|ini|cfg|conf|env|log|csv|tsv|ts|tsx|js|jsx|mjs|cjs|py|rb|rs|go|java|kt|swift|c|cc|cpp|h|hpp|cs|php|sh|zsh|bash|sql|html?|css|scss|svg|png|jpe?g|gif|webp|heic|pdf|zip|tar|gz|tgz|7z|docx?|xlsx?|pptx?|key|pem|crt|p12|sqlite|db)\b(?!\.[a-z0-9-])/gi, "<file>"],
  [/\b(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,24}\b/gi, "<host>"],
  [/\b[0-9a-f]{12,}\b/gi, "<hex>"],
  [/(?=[A-Za-z0-9_+/=-]*\d)(?=[A-Za-z0-9_+/=-]*[A-Za-z])[A-Za-z0-9_+/=-]{8,}/g, "<redacted>"],
  [/(?=[A-Za-z]*[a-z][A-Z][A-Za-z]*[a-z][A-Z][A-Za-z]*[a-z][A-Z])\b[A-Za-z]{8,}\b/g, "<redacted>"],
  [/\b[A-Za-z]{16,}\b/g, "<redacted>"],
  [/[A-Za-z0-9_+/=-]{32,}/g, "<redacted>"],
];

/**
 * `s` with what could identify or unlock anything replaced — URLs, emails,
 * ICE candidates and SDP, IP addresses, UUIDs, paths, file names, host
 * names, anything token-shaped — and cut to `max` characters.
 */
export function safeText(s: string, max = 160): string {
  let out = s;
  for (const [rule, by] of SAFE_RULES) out = out.replace(rule, by);
  return out.length > max ? `${out.slice(0, max - 1)}…` : out;
}

/**
 * The fields the log writes itself in a set shape (which link or channel,
 * the relays and their round trips): kept when they have it, which nothing
 * else could pass for; checked against {@link safeText} otherwise.
 */
const SHAPED: Readonly<Record<string, RegExp>> = {
  on: /^(?:hub|live|relay:[a-z][a-z0-9-]{0,15}|ch:[a-z][a-z0-9-]{0,15}\/\d{1,9}(?:\.direct)?|direct:(?:\d{1,6}|signal))$/,
  via: /^(?:hub|relay:[a-z][a-z0-9-]{0,15})$/,
  from: /^(?:hub|carrier|relay:[a-z][a-z0-9-]{0,15}|#[0-9a-f]{6}|[a-z]{1,16})$/,
  to: /^(?:hub|carrier|direct|live|copy|relay:[a-z][a-z0-9-]{0,15})$/,
  candidates: /^(?:[a-z][a-z0-9-]{0,15}=[\d-]{1,6}\/[\d-]{1,6}(?:\([a-z+]{1,40}\))? ?){1,40}$/,
  rtts: /^(?:[a-z][a-z0-9-]{0,15}=[\d-]{1,6} ?){1,40}$/,
  build: /^(?:\?|(?:index|connection-log)(?:-[A-Za-z0-9_-]{1,16})?)$/,
  agentVersion: /^v?\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]{1,24})?$/,
  relays: /^(?:[a-z][a-z0-9-]{0,15}(?:\([a-z]{1,16}\))?(?:=[\d-]{1,6})? ?){1,40}$/,
};

/**
 * Route segments that are never an id: the literal words of the local
 * program's API (`openapi/openapi.json`), its websocket routes and the hub's
 * (`openapi/hub.json`). Anything else in a path is a parameter and is masked.
 */
const ROUTE_WORDS = new Set(
  (
    "access account accounts acp add agents agg aggregator answer api api-chats apps archive auth batch cancel chain challenge " +
    "chat-provider chats check checklist claim client cloud code commands composer-prefs config connect continue default devices " +
    "dismiss disputes docs download email enabled enrollments env events finish first-unread fs git github gone-cwds grant history " +
    "install interrupt keys limit-events link list local-open login logout mailbox main-device me media message messages models new " +
    "notify options order pages pairings passkey passkeys password permissions pin ping plugins practice-folder prefs progress projects push questions " +
    "queue quota re-enroll-fresh read reauth receipts recent recent-cwds relay-id relays rescan resend rest return rotations " +
    "scheduled-tasks screens search seen self-upgrade server session sessions settings setup setup-token signup start stats status " +
    "steer stop streams subscriptions tasks terminals test ticket tickets title tools unlink update upload uploads usage usage-login " +
    "usage-verify use v1 vapid verify waitlist ws rh direct rotate"
  ).split(" "),
);

/** The CLIs a session's `{source}` names (not secret, but a parameter all the same). */
const SOURCES = new Set(["claude", "codex", "acp"]);

/** What the parameters after a literal are called, by the literal (or `parent/literal`); `{id}` otherwise. */
const ROUTE_PARAMS: Record<string, readonly string[]> = {
  sessions: ["source", "id"],
  "auth/sessions": ["id"],
  "composer-prefs": ["source", "id"],
  agents: ["agentId"],
  "acp/agents": ["id"],
  questions: ["requestId"],
  permissions: ["requestId"],
  queue: ["qid"],
  tasks: ["taskId"],
  tools: ["tool"],
  docs: ["name"],
  "setup-token": ["captureId"],
  enrollments: ["token"],
  keys: ["name"],
  rotations: ["host"],
  streams: ["stream"],
  pages: ["page"],
};

/** Query keys whose value is a position or a count (`before=1200`), kept when it is a plain number. */
export const NUMERIC_QUERY = new Set(["before", "from", "since", "after", "limit", "offset", "days", "v", "z"]);

/** The query keys the API names (`openapi/openapi.json`, `openapi/hub.json`, the socket routes): kept as keys; any other is `{key}`. */
const QUERY_KEYS = new Set([
  ...NUMERIC_QUERY,
  ..."fp sig q path dir inline url ids source accountId acpAgent provider model chooseModel scope prefix baseListRevision ticket cols rows".split(" "),
]);

/**
 * A request's route as a template: `/api/sessions/claude/0f9…/history?before=1200&fp=ab12`
 * → `/api/sessions/{source}/{id}/history?before=1200&fp`. Every segment that
 * is not one of the API's own words is a parameter; of the query, the keys
 * alone, and the numeric value of {@link NUMERIC_QUERY}. A full URL loses its
 * origin. Never throws.
 */
export function routeTemplate(pathAndQuery: string): string {
  let path = pathAndQuery;
  let query = "";
  try {
    if (/^[a-z][a-z0-9+.-]*:\/\//i.test(path)) {
      const url = new URL(path);
      path = url.pathname;
      query = url.search.slice(1);
    } else {
      const at = path.indexOf("?");
      if (at >= 0) {
        query = path.slice(at + 1);
        path = path.slice(0, at);
      }
    }
  } catch {
    return "<route>";
  }
  const hash = path.indexOf("#");
  if (hash >= 0) path = path.slice(0, hash);
  const segments = path.split("/");
  const out: string[] = [];
  let pending: string[] = [];
  let prev = "";
  for (const seg of segments) {
    if (seg === "") {
      out.push("");
      continue;
    }
    let word = seg;
    try {
      word = decodeURIComponent(seg);
    } catch {
      // kept as it is, and judged as it is
    }
    const sourceWanted = pending[0] === "source" && SOURCES.has(word);
    if (ROUTE_WORDS.has(word) && !sourceWanted) {
      out.push(word);
      pending = [...(ROUTE_PARAMS[`${prev}/${word}`] ?? ROUTE_PARAMS[word] ?? ["id"])];
      prev = word;
      continue;
    }
    out.push(`{${pending.shift() ?? "id"}}`);
  }
  let template = out.join("/") || "/";
  if (query) {
    const keys: string[] = [];
    for (const part of query.split("&")) {
      if (!part) continue;
      const eq = part.indexOf("=");
      let key = eq >= 0 ? part.slice(0, eq) : part;
      try {
        key = decodeURIComponent(key);
      } catch {
        // as it is
      }
      // A key the API does not name — a bare part with no `=` is one, whatever it says — is not kept as it is.
      if (!QUERY_KEYS.has(key)) {
        keys.push("{key}");
        continue;
      }
      const value = eq >= 0 ? part.slice(eq + 1) : "";
      keys.push(NUMERIC_QUERY.has(key) && /^\d{1,12}$/.test(value) ? `${key}=${value}` : key);
    }
    if (keys.length > 0) template += `?${keys.join("&")}`;
  }
  return template;
}

// ---------------------------------------------------------------------------
// The log
// ---------------------------------------------------------------------------

/** A source of the once-a-second sample: its fields now, and whether anything moved or waits (`busy`). */
export type SampleFn = () => { fields: ConnFields; busy: boolean } | null;

export interface ConnectionLogOptions {
  maxEvents?: number;
  maxRequests?: number;
  maxChatter?: number;
  maxAgeMs?: number;
  /** The monotonic clock (`performance.now`). */
  now?: () => number;
  /** The wall clock (`Date.now`). */
  wall?: () => number;
  sampleMs?: number;
  quietSampleMs?: number;
}

/** Every relay id in a shaped `relays` list must itself pass `safeText` untouched (blocks token-shaped single words). */
function safeRelayList(value: string): boolean {
  if (value.length > 400 || !SHAPED.relays!.test(value)) return false;
  for (const part of value.split(" ")) {
    const id = part.split(/[(=]/, 1)[0] ?? "";
    if (!id || safeText(id) !== id) return false;
  }
  return true;
}

/**
 * Scrub a browser `userAgent` with {@link safeText} while keeping benign
 * `Product/Version` pairs (such as `Mozilla/5.0`, `Chrome/131.0.0.0`,
 * `AppleWebKit/537.36`) whose `/` and four-part version would otherwise trip
 * the base64-token and IPv4 rules.
 */
function cleanUserAgent(value: string): string {
  const saved: string[] = [];
  const placeholder = (i: number) => `\u0000ua${i}\u0000`;
  // First strip control NULs and URLs so a `scheme://host` path never looks like a Product/Version pair.
  const withoutUrls = value.replace(/\u0000/g, "").replace(SAFE_RULES[0]![0], SAFE_RULES[0]![1]);
  const staged = withoutUrls.replace(/(?<![A-Za-z0-9_./:-])([A-Za-z][A-Za-z-]{1,15})\/(\d{1,4}(?:\.\d{1,4}){0,3})(?![A-Za-z0-9_./:-])/g, (match, name: string) => {
    if (safeText(name) !== name) return match;
    const idx = saved.length;
    saved.push(match);
    return placeholder(idx);
  });
  let out = safeText(staged, 240);
  for (let i = 0; i < saved.length; i++) out = out.replace(placeholder(i), saved[i]!);
  return out.length > 240 ? `${out.slice(0, 239)}…` : out;
}

/**
 * A field as the log keeps it, into `out`: free text ({@link FREE_TEXT})
 * only when it is Droppen's own words, else `<withheld>` and its length
 * (`reasonLen`); a `route` that is its own {@link routeTemplate}, and a
 * {@link SHAPED} field in its shape, as they are; every other string through
 * {@link safeText}; a number rounded to a tenth, never NaN.
 */
function keep(out: Record<string, ConnValue>, key: string, value: ConnValue | undefined): void {
  if (value === undefined) return;
  if (typeof value === "number") {
    out[key] = Number.isFinite(value) ? (Number.isInteger(value) ? value : Math.round(value * 10) / 10) : null;
    return;
  }
  if (typeof value !== "string") {
    out[key] = value;
    return;
  }
  const withhold = () => {
    out[key] = "<withheld>";
    out[`${key}Len`] = value.length;
  };
  if (FREE_TEXT.has(key)) {
    const own = ownWords(value);
    if (own === null) withhold();
    else out[key] = safeText(own);
    return;
  }
  if (CODE_FIELDS.has(key)) {
    if (/^[A-Za-z0-9_.:+-]{1,48}$/.test(value) && safeText(value) === value) out[key] = value;
    else withhold();
    return;
  }
  if (key === "agentVersion") {
    if (SHAPED.agentVersion!.test(value) && safeText(value) === value) out[key] = value;
    else withhold();
    return;
  }
  if (key === "userAgent") {
    out[key] = cleanUserAgent(value);
    return;
  }
  if (key === "route" && value.length <= 240 && routeTemplate(value) === value) out[key] = value;
  else if (key === "relays" && safeRelayList(value)) out[key] = value;
  else if (key !== "relays" && SHAPED[key]?.test(value) && value.length <= 400) out[key] = value;
  else out[key] = safeText(value);
}

/**
 * An error as fields: its words under `key` (free text: kept only when they
 * are Droppen's own, {@link ownWords}), its class (`errName`), and its code
 * or kind when it has a short one.
 */
export function errorFields(error: unknown, key = "error"): Record<string, ConnValue> {
  try {
    const out: Record<string, ConnValue> = { [key]: error instanceof Error ? error.message : String(error) };
    out.errName = error instanceof Error ? error.name : typeof error;
    const { code, kind } = (typeof error === "object" && error !== null ? error : {}) as { code?: unknown; kind?: unknown };
    if (typeof code === "number" || (typeof code === "string" && /^[\w.-]{1,40}$/.test(code))) out.errCode = code;
    if (typeof kind === "string" && /^[A-Za-z]{1,30}$/.test(kind)) out.errKind = kind;
    return out;
  } catch {
    return { [key]: "<unreadable>", errName: "unknown" };
  }
}

const perfNow = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

export class ConnectionLog {
  private readonly main: Ring;
  private readonly requests: Ring;
  private readonly chatter: Ring;
  private previous: ConnEvent[] = [];
  private seq = 0;
  private readonly listeners = new Set<() => void>();
  private readonly samplers = new Map<string, SampleFn>();
  private sampleTimer: ReturnType<typeof setInterval> | null = null;
  private lastSampleWall = Number.NEGATIVE_INFINITY;
  private readonly opts: Required<ConnectionLogOptions>;
  /** When the page loaded (wall-clock ms). */
  readonly loadedAt: number;
  /** Bumped on every change, for `useSyncExternalStore`. */
  version = 0;

  constructor(opts: ConnectionLogOptions = {}) {
    this.opts = {
      maxEvents: opts.maxEvents ?? MAX_EVENTS,
      maxRequests: opts.maxRequests ?? MAX_REQUESTS,
      maxChatter: opts.maxChatter ?? MAX_CHATTER,
      maxAgeMs: opts.maxAgeMs ?? MAX_AGE_MS,
      now: opts.now ?? perfNow,
      wall: opts.wall ?? Date.now,
      sampleMs: opts.sampleMs ?? SAMPLE_MS,
      quietSampleMs: opts.quietSampleMs ?? QUIET_SAMPLE_MS,
    };
    this.main = new Ring(this.opts.maxEvents);
    this.requests = new Ring(this.opts.maxRequests);
    this.chatter = new Ring(this.opts.maxChatter);
    this.loadedAt = Math.round(this.opts.wall() - this.opts.now());
  }

  /** The ids the page knows (the account, this device, each machine), each with what finds it in any case: never written as they are. */
  private readonly known = new Map<string, RegExp>();

  /**
   * Never write `id` as it is, in any case: in any text logged from now on (an
   * error's words, a close reason) it becomes its {@link maskId}. The links
   * and the channels name theirs as they are made.
   */
  hide(id: string | null | undefined): void {
    if (!id || id.length < 3 || this.known.has(id)) return;
    this.known.set(id, new RegExp(id.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "gi"));
  }

  /** `fields` held to the rules every event's are (for what is said beside the events: the export's header). */
  clean(fields: ConnFields): Record<string, ConnValue> {
    const kept: Record<string, ConnValue> = {};
    try {
      for (const key in fields) {
        const raw = fields[key];
        keep(kept, key, typeof raw === "string" ? this.masked(raw) : raw);
      }
    } catch {
      // what could be kept was
    }
    return kept;
  }

  /** `text` with every id the log knows as its mask, whatever its case. */
  private masked(text: string): string {
    let out = text;
    for (const [id, re] of this.known) out = out.replace(re, () => maskId(id)!);
    return out;
  }

  /** Log `name` with `fields` (redacted on the way in) — at its own level unless `level` says otherwise. Never throws. */
  emit(name: ConnEventName, fields: ConnFields = {}, level?: ConnLevel): void {
    try {
      const kept: Record<string, ConnValue> = {};
      for (const key in fields) {
        const raw = fields[key];
        keep(kept, key, typeof raw === "string" ? this.masked(raw) : raw);
      }
      const wall = this.opts.wall();
      const event: ConnEvent = { seq: ++this.seq, t: Math.round(this.opts.now()), wall, name, level: level ?? CONN_EVENTS[name] ?? "info", fields: kept };
      (CHATTER.has(name) ? this.chatter : REQUESTS.has(name) ? this.requests : this.main).push(event);
      const horizon = wall - this.opts.maxAgeMs;
      this.main.dropBefore(horizon);
      this.requests.dropBefore(horizon);
      this.chatter.dropBefore(horizon);
      if (this.previous.length > 0 && this.previous[0]!.wall < horizon) this.previous = this.previous.filter((e) => e.wall >= horizon);
      this.changed();
    } catch {
      // the log never stands in the way of what it logs
    }
  }

  /** Everything kept, oldest first: the page before's (restored), then this page's. */
  events(): ConnEvent[] {
    const mine = [...this.main.toArray(), ...this.requests.toArray(), ...this.chatter.toArray()].sort((a, b) => a.seq - b.seq);
    return this.previous.length > 0 ? [...this.previous, ...mine] : mine;
  }

  /** How many events are kept. */
  get size(): number {
    return this.previous.length + this.main.size + this.requests.size + this.chatter.size;
  }

  /** Hear every change (an event, a clear). */
  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private changed(): void {
    this.version++;
    for (const fn of [...this.listeners]) {
      try {
        fn();
      } catch {
        // one listener failing must not starve the others
      }
    }
  }

  /** Forget everything kept (the restored page's too). */
  clear(): void {
    this.main.clear();
    this.requests.clear();
    this.chatter.clear();
    this.previous = [];
    this.changed();
  }

  // -- the sampler ----------------------------------------------------------

  /**
   * Sample `fn` once a second, its fields under `key.` (`hub.tx`), until the
   * returned function: a connection adds itself when it is up, and goes when
   * it is down. The sampler runs while any source is added, and only then.
   */
  addSampler(key: string, fn: SampleFn): () => void {
    this.samplers.set(key, fn);
    if (this.sampleTimer === null) {
      this.sampleTimer = setInterval(() => this.sampleNow(), this.opts.sampleMs);
      (this.sampleTimer as { unref?: () => void }).unref?.();
    }
    return () => {
      if (this.samplers.get(key) !== fn) return;
      this.samplers.delete(key);
      if (this.samplers.size === 0 && this.sampleTimer !== null) {
        clearInterval(this.sampleTimer);
        this.sampleTimer = null;
        this.lastTick = null;
      }
    };
  }

  /** When the sampler last looked (the monotonic clock): how late the next look is says how far the page's event loop lags. */
  private lastTick: number | null = null;

  /** Whether the sampler is running (diagnostics, tests). */
  get sampling(): boolean {
    return this.sampleTimer !== null;
  }

  /**
   * One sample of every source now: recorded when anything moved or waits,
   * or the page's timers ran late (`lagMs`: how much later than its second
   * this look came — a busy or a background page), else once every
   * {@link QUIET_SAMPLE_MS}.
   */
  sampleNow(): void {
    try {
      const tick = this.opts.now();
      const lagMs = this.lastTick === null ? null : Math.max(0, Math.round(tick - this.lastTick - this.opts.sampleMs));
      this.lastTick = tick;
      const fields: Record<string, ConnValue | undefined> = { lagMs };
      let busy = (lagMs ?? 0) >= 250;
      for (const [key, fn] of this.samplers) {
        let got: ReturnType<SampleFn> = null;
        try {
          got = fn();
        } catch {
          got = null;
        }
        if (!got) continue;
        busy ||= got.busy;
        for (const name in got.fields) fields[`${key}.${name}`] = got.fields[name];
      }
      if (Object.keys(fields).length === 1) return;
      const wall = this.opts.wall();
      if (!busy && wall - this.lastSampleWall < this.opts.quietSampleMs) return;
      this.lastSampleWall = wall;
      this.emit("sample", fields);
    } catch {
      // as for emit
    }
  }

  // -- the page before ------------------------------------------------------

  /** Keep the newest of the log (at most `budget` bytes of JSON) in `storage`, for the next page of the tab. */
  persist(storage: Pick<Storage, "setItem"> | null = sessionStore(), budget = PERSIST_BYTES): void {
    if (!storage) return;
    try {
      const all = this.events();
      const kept: string[] = [];
      let bytes = 0;
      for (let i = all.length - 1; i >= 0; i--) {
        const e = all[i]!;
        const line = JSON.stringify([e.t, e.wall, e.name, e.level, e.fields, e.prev ?? this.loadedAt]);
        if (bytes + line.length + 1 > budget) break;
        bytes += line.length + 1;
        kept.push(line);
      }
      kept.reverse();
      storage.setItem(PERSIST_KEY, `{"v":1,"events":[${kept.join(",")}]}`);
    } catch {
      // storage full or refused: the next page starts without it
    }
  }

  /** Read back what the page before kept (once: it is removed), dropping what is older than {@link MAX_AGE_MS}. */
  restore(storage: Pick<Storage, "getItem" | "removeItem"> | null = sessionStore()): number {
    if (!storage) return 0;
    try {
      const raw = storage.getItem(PERSIST_KEY);
      if (!raw) return 0;
      storage.removeItem(PERSIST_KEY);
      const parsed = JSON.parse(raw) as { v?: unknown; events?: unknown };
      if (parsed.v !== 1 || !Array.isArray(parsed.events)) return 0;
      const horizon = this.opts.wall() - this.opts.maxAgeMs;
      const out: ConnEvent[] = [];
      for (const item of parsed.events) {
        if (!Array.isArray(item) || item.length !== 6) continue;
        const [t, wall, name, level, fields, prev] = item as unknown[];
        if (typeof t !== "number" || typeof wall !== "number" || typeof prev !== "number" || wall < horizon) continue;
        if (typeof name !== "string" || !(name in CONN_EVENTS)) continue;
        if (level !== "info" && level !== "warn" && level !== "error") continue;
        if (!fields || typeof fields !== "object" || Array.isArray(fields)) continue;
        // What storage hands back is held to the same rules as what is logged now.
        const kept: Record<string, ConnValue> = {};
        for (const [k, v] of Object.entries(fields as Record<string, unknown>)) {
          if (v === null || typeof v === "number" || typeof v === "boolean" || typeof v === "string") keep(kept, k, typeof v === "string" ? this.masked(v) : v);
        }
        // A seq of its own, so a view can tell every event apart.
        out.push({ seq: ++this.seq, t, wall, name: name as ConnEventName, level, fields: kept, prev });
      }
      this.previous = out;
      this.changed();
      return out.length;
    } catch {
      return 0;
    }
  }
}

function sessionStore(): Storage | null {
  try {
    const s = globalThis.sessionStorage;
    return s && typeof s.getItem === "function" ? s : null;
  } catch {
    return null;
  }
}

/** The page's one log. */
export const connLog = new ConnectionLog();

/** A counter's growth since the last look (for a sample): `take(total)` → what it grew by. */
export function deltaOf(): (total: number) => number {
  let last: number | null = null;
  return (total) => {
    const delta = last === null ? 0 : Math.max(0, total - last);
    last = total;
    return delta;
  };
}

// ---------------------------------------------------------------------------
// The page's own state
// ---------------------------------------------------------------------------

/**
 * The build this page runs: the name of the chunk this module is in, its
 * content hash in a build (`index-DV4Z0_d1`), which names exactly the web
 * bytes the page booted from — nothing is baked in for it, so a release that
 * leaves the web app alone leaves its bytes alone (`docs/web-integrity.md`).
 * The release a device's agent runs is in the export's header, as the
 * device reports it.
 */
export function buildId(): string {
  try {
    const file = new URL(import.meta.url).pathname.split("/").pop() ?? "";
    return file.replace(/\.[jt]sx?$/, "") || "?";
  } catch {
    return "?";
  }
}

interface NetworkInformationLike extends EventTarget {
  effectiveType?: string;
  type?: string;
  downlink?: number;
  rtt?: number;
  saveData?: boolean;
}

function connection(): NetworkInformationLike | null {
  return ((globalThis as { navigator?: { connection?: NetworkInformationLike } }).navigator?.connection as NetworkInformationLike | undefined) ?? null;
}

/** What `navigator.connection` says now (where the browser has it). */
export function networkFields(): Record<string, ConnValue> {
  const c = connection();
  const nav = (globalThis as { navigator?: { onLine?: boolean } }).navigator;
  return {
    online: nav?.onLine ?? null,
    type: c?.effectiveType ?? null,
    kind: c?.type ?? null,
    downlinkMbps: typeof c?.downlink === "number" ? c.downlink : null,
    rttMs: typeof c?.rtt === "number" ? c.rtt : null,
    saveData: c?.saveData ?? null,
  };
}

function visibility(): string | null {
  return (globalThis as { document?: { visibilityState?: string } }).document?.visibilityState ?? null;
}

let started: (() => void) | null = null;

/**
 * Start the page's log: read back the page before's, say this one started
 * (the app's release and build, the network, whether it is in view), and
 * from then on the network changing, going on- and offline, the page going
 * out of view and back (a background tab's timers are held), and keep the
 * log for the next page of the tab when this one goes. Once per page; the
 * returned function undoes it (tests).
 */
export function startConnectionLog(log: ConnectionLog = connLog): () => void {
  if (started) return started;
  log.restore();
  log.emit("page.start", { build: buildId(), visibility: visibility(), ...networkFields() });
  const offs: (() => void)[] = [];
  const on = (target: { addEventListener?: (type: string, fn: (e: Event) => void) => void; removeEventListener?: (type: string, fn: (e: Event) => void) => void } | null | undefined, type: string, fn: (e: Event) => void) => {
    if (!target?.addEventListener) return;
    target.addEventListener(type, fn);
    offs.push(() => target.removeEventListener?.(type, fn));
  };
  const win = globalThis as unknown as EventTarget;
  const doc = (globalThis as { document?: EventTarget }).document;
  let hiddenAt: number | null = null;
  on(win, "online", () => log.emit("net.online", networkFields()));
  on(win, "offline", () => log.emit("net.offline", networkFields()));
  on(connection(), "change", () => log.emit("net.change", networkFields()));
  on(doc, "visibilitychange", () => {
    if (visibility() === "hidden") {
      hiddenAt = Date.now();
      log.emit("page.hidden");
      log.persist();
    } else {
      log.emit("page.visible", { hiddenMs: hiddenAt === null ? null : Date.now() - hiddenAt });
      hiddenAt = null;
    }
  });
  on(win, "pageshow", (e) => {
    if ((e as PageTransitionEvent).persisted) log.emit("page.show", { fromCache: true });
  });
  on(win, "pagehide", (e) => {
    log.emit("page.hide", { cached: (e as PageTransitionEvent).persisted ?? null });
    log.persist();
  });
  started = () => {
    for (const off of offs.splice(0)) off();
    started = null;
  };
  return started;
}
