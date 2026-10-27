/*
 * The page's transport while its machine cannot be reached: every request and
 * socket answered from the machine's offline copy (`lib/hub/mirror.ts`), in
 * exactly the shapes the machine itself would use, so the app above renders
 * unchanged — and read-only.
 *
 * - `GET /api/sessions`, a session's detail and its history pages, settings,
 *   accounts and quota come from the copy. A session's state says it cannot
 *   send, and why (`sendBlockReason`), which the composer already shows.
 * - A session's search and its first unread message are worked out over the
 *   copy's items as the machine works them out over the transcript
 *   (`offline-search.ts`); a seen report records nothing but answers with the
 *   read mark the copy's unread ledger implies, so the jump to the first
 *   unread message is offered offline too.
 * - Everything else — sending, terminals, files, writes of any kind — is a
 *   `503 {error: "machine_offline", message, hint}`; `hint` is what the app's
 *   `errorText` shows.
 * - `/api/events?v=2` is a socket played here: `hello`, `settings` (marked
 *   `fromCopy`), `sessions:snapshot`, then session feeds on `sub` (a snapshot
 *   built like the server's, or — when the window the page holds is this
 *   copy's — a resume), `resync`, `ping`; and whatever the copy learns from
 *   the hub afterwards (records the machine wrote just before it went) goes
 *   out as `sessions:delta`, feed deltas and `state` frames.
 * - Terminal sockets close at once: they need the machine.
 *
 * Fingerprints: the copy holds the agent's `fp` (of every item it has) and
 * `prefixFp` (of all but the last three) — the same marks a live server sends
 * — and nothing in between, so windows cut here carry no `startFp`, and a
 * history page is served only to a window that is this copy's own (its resume
 * marker matches); any other is told `409 transcript_changed` and resyncs.
 */
import type { components } from "@/api/schema";
import type { SessionCopy, MirrorChange } from "@/lib/hub/mirror";
import type { FeedCopy, StatusState } from "@/lib/hub/streams";
import { DEFAULT_LIMIT, firstUnreadIn, MAX_LIMIT, readMarkOf, searchCopy, validateQuery } from "./offline-search";
import type { SocketLike, Transport } from "@/lib/transport";

type SessionSummary = components["schemas"]["SessionSummary"];
type TranscriptItem = components["schemas"]["TranscriptItem"];

/** What the transport reads of the copy (`MachineMirror` is one). */
export interface OfflineSource {
  whenLoaded(): Promise<void>;
  rows(): SessionSummary[] | null;
  row(source: string, id: string): SessionSummary | undefined;
  readonly status: { state: StatusState | null } | null;
  session(source: string, id: string): Promise<SessionCopy | null>;
  sessionNow(source: string, id: string): SessionCopy | null;
  touch(source: string, id: string): void;
  onChange(fn: (change: MirrorChange) => void): () => void;
}

export interface OfflineOptions {
  /** Who `/api/auth/status` says is signed in (the machine says `cloud:<device>`). */
  user: string | null;
  messages: {
    /** A session's `sendBlockReason`. */
    readOnly: string;
    /** A session the device has no copy of. */
    noCopy: string;
    /** The body of every refused request. */
    unavailable: string;
  };
  /** Whether the machine is up (its rows' activity is then still news, not a leftover). */
  machineOnline?: () => boolean;
}

/** The server's page size (`SESSION_PAGE_SIZE`), and the least a `sub` may ask for. */
export const PAGE_SIZE = 200;
const MIN_LIMIT = 20;
/** `RESUME_OVERLAP`: a resume re-sends the last few items the client holds. */
export const RESUME_OVERLAP = 3;

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

export function machineOffline(message: string): Response {
  return json(503, { error: "machine_offline", message, hint: message });
}

/** A row as an offline machine has it: nothing can be running on it. */
export function restingRow(row: SessionSummary): SessionSummary {
  return { ...row, activity: "idle", status: "idle", asking: false, resident: null as unknown as SessionSummary["resident"] };
}

function rowOf(source: OfflineSource, opts: OfflineOptions, s: string, id: string): SessionSummary | undefined {
  const row = source.row(s, id);
  return row && !opts.machineOnline?.() ? restingRow(row) : row;
}

function rowsOf(source: OfflineSource, opts: OfflineOptions): SessionSummary[] {
  const rows = source.rows() ?? [];
  return opts.machineOnline?.() ? rows : rows.map(restingRow);
}

/** The state half of a session as the offline copy has it: read-only. */
export function offlineState(row: SessionSummary, reason: string) {
  return {
    summary: row,
    injection: null,
    canSend: false,
    sendBlockReason: reason,
    cliChoice: null,
    contextTokens: null,
    contextWindow: 200_000,
    claudeRoot: row.claudeRoot ?? null,
    account: null,
    rotation: null,
    receipts: [],
    questions: [],
  };
}

const EMPTY_COPY: FeedCopy = { started: false, startIndex: 0, endIndex: 0, items: [], fp: null, prefixFp: null };

/** The copy's version, as a session signature: it changes with every record released. */
export const offlineSig = (seq: number) => `offline:${seq}`;

/** The last `size` items before `before`, as the server pages them. */
export function pageOf(copy: FeedCopy, before: number, size = PAGE_SIZE): { items: TranscriptItem[]; startIndex: number; endIndex: number } {
  const endIndex = Math.max(copy.startIndex, Math.min(before, copy.endIndex));
  const startIndex = Math.max(copy.startIndex, endIndex - size);
  return { items: copy.items.slice(startIndex - copy.startIndex, endIndex - copy.startIndex), startIndex, endIndex };
}

/**
 * The window fields of a `snapshot`/`reset` frame or a detail: the last page.
 * A long session's copy holds only its newest items (the agent caps it), so
 * nothing is offered before where the copy starts.
 */
function windowFields(copy: FeedCopy, size: number) {
  const page = pageOf(copy, copy.endIndex, size);
  return {
    items: page.items,
    startIndex: page.startIndex,
    endIndex: page.endIndex,
    totalItems: copy.endIndex,
    hasMoreBefore: page.startIndex > copy.startIndex,
    omittedItems: page.startIndex,
    prefixFp: copy.prefixFp ?? "",
    startFp: "",
  };
}

/** Whether a client holding `since`/`fp` holds a window of exactly this copy. */
function holdsThisCopy(copy: FeedCopy, since: unknown, fp: unknown): since is number {
  return typeof since === "number" && typeof fp === "string" && fp !== "" && since === copy.endIndex && fp === copy.prefixFp;
}

function deltaFrame(copy: FeedCopy, seq: number, from: number) {
  return {
    type: "delta",
    sig: offlineSig(seq),
    fromIndex: from,
    items: copy.items.slice(from - copy.startIndex),
    endIndex: copy.endIndex,
    totalItems: copy.endIndex,
    prefixFp: copy.prefixFp ?? "",
  };
}

function segments(path: string, prefix: string): string[] | null {
  if (!path.startsWith(prefix)) return null;
  try {
    return path
      .slice(prefix.length)
      .split("/")
      .map((part) => decodeURIComponent(part));
  } catch {
    return null;
  }
}

/** One request, answered from the copy. */
export async function offlineFetch(source: OfflineSource | null, opts: OfflineOptions, request: Request): Promise<Response> {
  const url = new URL(request.url);
  const path = url.pathname;
  const seen = request.method.toUpperCase() === "POST" ? segments(path, "/api/sessions/") : null;
  if (source && seen?.length === 3 && seen[2] === "seen") return offlineSeen(source, opts, seen[0]!, seen[1]!);
  if (request.method.toUpperCase() !== "GET") return machineOffline(opts.messages.unavailable);
  if (path === "/api/auth/status") {
    return json(200, { authRequired: false, setupRequired: false, user: opts.user, desktopApp: false, authOff: false });
  }
  if (!source) return machineOffline(opts.messages.unavailable);
  await source.whenLoaded();
  const q = url.searchParams;

  if (path === "/api/sessions") {
    const limit = q.has("limit") ? Number(q.get("limit")) : 200;
    if (!Number.isInteger(limit) || limit < 1 || limit > 1000) return json(400, { error: "limit must be between 1 and 1000" });
    const scope = q.get("scope") ?? "owned";
    const only = q.get("source");
    const needle = (q.get("q") ?? "").toLowerCase();
    const rows = rowsOf(source, opts).filter(
      (row) =>
        (!only || row.source === only) &&
        (scope === "all" || (row.scope ?? "owned") === scope) &&
        (!needle || row.title.toLowerCase().includes(needle) || (row.cwd ?? "").toLowerCase().includes(needle)),
    );
    return json(200, { sessions: rows.slice(0, limit), total: rows.length });
  }

  const parts = segments(path, "/api/sessions/");
  if (parts?.length === 3 && (parts[2] === "search" || parts[2] === "first-unread")) {
    const [s, id] = parts as [string, string];
    const row = rowOf(source, opts, s, id);
    if (!row) return json(404, { error: "session not found" });
    const copy = (await source.session(s, id))?.state ?? null;
    if (parts[2] === "first-unread") {
      // A session with no copy here has nothing to point at: nothing unread to jump to.
      return json(200, copy ? firstUnreadIn(copy, q.get("after"), row.updatedAt) : { index: null, count: 0 });
    }
    const checked = validateQuery(q.get("q") ?? "");
    if ("error" in checked) return json(400, { error: checked.error });
    const asked = q.has("limit") ? Number(q.get("limit")) : DEFAULT_LIMIT;
    const limit = Number.isInteger(asked) && asked >= 0 ? Math.min(asked, MAX_LIMIT) : DEFAULT_LIMIT;
    // Only what the copy holds is searched; with none, no item matches.
    return json(200, searchCopy(copy ?? EMPTY_COPY, q.get("q") ?? "", checked.terms, limit));
  }
  if (parts && (parts.length === 2 || (parts.length === 3 && parts[2] === "history"))) {
    const [s, id] = parts as [string, string];
    const row = rowOf(source, opts, s, id);
    const copy = (await source.session(s, id)) ?? null;
    if (!row || (parts.length === 3 && !copy)) return json(404, { error: "session not found" });
    const state = copy?.state ?? EMPTY_COPY;
    const sig = offlineSig(copy?.seq ?? 0);
    if (parts.length === 3) {
      const before = Number(q.get("before"));
      if (!Number.isInteger(before) || before < 0) return json(400, { error: "before must be a non-negative integer" });
      const fp = q.get("fp");
      if (fp) {
        const since = q.has("since") ? Number(q.get("since")) : null;
        const holds = since !== null ? since - RESUME_OVERLAP >= before && holdsThisCopy(state, since, fp) : before === state.endIndex && fp === state.fp;
        if (!holds) return json(409, { error: "transcript_changed" });
      }
      const page = pageOf(state, before);
      // Before where the copy starts: nothing here to serve.
      if (page.startIndex === page.endIndex && before > 0) return json(404, { error: "not in the offline copy" });
      // Nothing before where the copy starts (the agent's cap, or the plan's retention window).
      return json(200, { sig, startFp: "", ...page, totalItems: state.endIndex, hasMoreBefore: page.startIndex > state.startIndex });
    }
    const reason = copy ? opts.messages.readOnly : opts.messages.noCopy;
    const after = q.get("after");
    if (after !== null && q.get("sig") === sig) {
      const at = Number(after);
      return json(200, {
        ...offlineState(row, reason),
        sig,
        items: [],
        startIndex: at,
        endIndex: at,
        totalItems: at,
        hasMoreBefore: at > 0,
        omittedItems: 0,
        itemsChanged: false,
        reset: false,
      });
    }
    return json(200, { ...offlineState(row, reason), sig, ...windowFields(state, PAGE_SIZE), itemsChanged: true, reset: true });
  }

  const status = source.status?.state ?? null;
  if (path === "/api/settings" && status?.settings) return json(200, status.settings);
  if (path === "/api/stats/quota" && status?.quota) return json(200, status.quota);
  if (path === "/api/accounts" && status?.accounts) return json(200, accountList(status.accounts));
  return machineOffline(opts.messages.unavailable);
}

/**
 * A session looked at on the copy: nothing is recorded — the read mark lives
 * on the machine, which hears the next report once it is back — but the
 * answer says where the copy's unread messages begin (the mark it would have
 * replaced, from the row's unread ledger), so the way back to them
 * (`first-unread`) works offline too.
 */
async function offlineSeen(source: OfflineSource, opts: OfflineOptions, s: string, id: string): Promise<Response> {
  await source.whenLoaded();
  const row = rowOf(source, opts, s, id);
  if (!row) return json(404, { error: "session not found" });
  const copy = source.sessionNow(s, id) ?? (await source.session(s, id));
  return json(200, { ok: true, previousReadAt: readMarkOf(row, copy?.state ?? null) });
}

/**
 * The accounts as the machine last listed them — only what the status stream
 * carries (no credentials, no usage history) — with every field the app
 * expects of an account present.
 */
export function accountList(stored: Record<string, unknown>) {
  const accounts = Array.isArray(stored.accounts) ? stored.accounts : [];
  return {
    accounts: accounts
      .filter((a): a is Record<string, unknown> => typeof a === "object" && a !== null && typeof (a as { id?: unknown }).id === "string")
      .map((a) => ({
        createdAt: "",
        updatedAt: "",
        rest: [],
        usageEnabled: false,
        label: String(a.id),
        kind: "oauth_token",
        provider: "claude",
        ...a,
        // Carried by the status so an offline page knows which accounts may
        // stand in for a signed-out local login; an older machine's says nothing.
        rotation: a.rotation === true,
      })),
    defaultAccountId: typeof stored.defaultAccountId === "string" ? stored.defaultAccountId : null,
    defaultCodexAccountId: typeof stored.defaultCodexAccountId === "string" ? stored.defaultCodexAccountId : null,
    // Each local login's state and whether it was chosen as the default, when
    // the machine's status carries them (its rests it does not). Without them
    // the app reads the list as an older server's.
    ...(isRecord(stored.local) ? { local: { claude: localLogin(stored.local.claude), codex: localLogin(stored.local.codex) } } : {}),
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

/** One local login as the status carries it, with the fields the app reads it by. */
function localLogin(stored: unknown) {
  const login = isRecord(stored) ? stored : {};
  const state = login.state === "signed_in" || login.state === "signed_out" ? login.state : "unknown";
  return { state, rest: [], ...(typeof login.isDefault === "boolean" ? { isDefault: login.isDefault } : {}) };
}

const CONNECTING = 0;
const OPEN = 1;
const CLOSED = 3;

/** A socket the far end refuses: closed before it ever opens, as a refused upgrade is. */
export class RefusedSocket implements SocketLike {
  readyState = CONNECTING;
  binaryType = "blob";
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;

  constructor() {
    queueMicrotask(() => this.close());
  }

  send(): void {
    if (this.readyState === CONNECTING) throw new DOMException("Still connecting", "InvalidStateError");
  }

  close(): void {
    if (this.readyState === CLOSED) return;
    this.readyState = CLOSED;
    queueMicrotask(() => this.onclose?.({}));
  }
}

interface Channel {
  sub: number;
  source: string;
  id: string;
  /** The opening frame has gone out. */
  open: boolean;
  /** Where the window the client holds ends, as last sent. */
  endIndex: number;
  /** The row its last state frame carried. */
  rowJson: string;
  /** The `sendBlockReason` it was last sent. */
  reason: string;
}

/**
 * `/api/events?v=2`, played from the copy: the list, the server info and any
 * number of session feeds, in the frames `app/feed-hub.ts` and
 * `session-feed-pool.ts` read from the machine.
 */
export class OfflineMuxSocket implements SocketLike {
  readyState = CONNECTING;
  binaryType = "blob";
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: unknown) => void) | null = null;
  private readonly channels = new Map<number, Channel>();
  private highestSub = 0;
  /** `source/id` → the row's JSON as last sent, for `sessions:delta`. */
  private readonly seen = new Map<string, string>();
  private settingsJson: string | null = null;
  private off: (() => void) | null = null;

  constructor(
    private readonly source: OfflineSource,
    private readonly opts: OfflineOptions,
  ) {
    queueMicrotask(() => void this.start());
  }

  private emit(frame: Record<string, unknown>): void {
    if (this.readyState !== OPEN) return;
    const data = JSON.stringify(frame);
    this.onmessage?.({ data });
  }

  private async start(): Promise<void> {
    if (this.readyState !== CONNECTING) return;
    this.readyState = OPEN;
    this.onopen?.({});
    this.emit({ type: "hello", features: ["sub", "ping"], compress: null });
    await this.source.whenLoaded();
    if (this.readyState !== OPEN) return;
    this.sendSettings();
    const rows = rowsOf(this.source, this.opts);
    for (const row of rows) this.seen.set(`${row.source}/${row.id}`, JSON.stringify(row));
    this.emit({ type: "sessions:snapshot", sessions: rows });
    this.off = this.source.onChange((change) => this.changed(change));
  }

  private sendSettings(): void {
    const settings = this.source.status?.state?.settings;
    if (!settings) return;
    const text = JSON.stringify(settings);
    if (text === this.settingsJson) return;
    this.settingsJson = text;
    // Marked: its `serverVersion` is the build the machine ran when it last
    // wrote its status, not one this page is talking to (`version-drift.ts`).
    this.emit({ ...settings, fromCopy: true, type: "settings" });
  }

  send(data: string): void {
    if (this.readyState === CONNECTING) throw new DOMException("Still connecting", "InvalidStateError");
    if (this.readyState !== OPEN) return;
    let m: Record<string, unknown>;
    try {
      m = JSON.parse(data) as Record<string, unknown>;
    } catch {
      return;
    }
    switch (m.type) {
      case "ping":
        queueMicrotask(() => this.emit({ type: "pong" }));
        return;
      case "sub":
        for (const feed of Array.isArray(m.feeds) ? m.feeds : []) this.subscribe(feed as Record<string, unknown>);
        return;
      case "unsub":
        for (const sub of Array.isArray(m.subs) ? m.subs : []) if (typeof sub === "number") this.channels.delete(sub);
        return;
      case "resync":
        if (typeof m.sub === "number") void this.resync(m.sub, m.since, m.fp);
        return;
      default:
        return;
    }
  }

  close(): void {
    if (this.readyState === CLOSED) return;
    this.readyState = CLOSED;
    this.off?.();
    this.off = null;
    this.channels.clear();
    queueMicrotask(() => this.onclose?.({}));
  }

  private subscribe(feed: Record<string, unknown>): void {
    const sub = feed.sub;
    if (typeof sub !== "number" || !Number.isInteger(sub)) return;
    if (sub <= this.highestSub) {
      queueMicrotask(() => this.emit({ sub, type: "closed", reason: "reused" }));
      return;
    }
    this.highestSub = sub;
    if (typeof feed.source !== "string" || typeof feed.id !== "string") {
      queueMicrotask(() => this.emit({ sub, type: "closed", reason: "invalid" }));
      return;
    }
    const channel: Channel = { sub, source: feed.source, id: feed.id, open: false, endIndex: 0, rowJson: "", reason: "" };
    this.channels.set(sub, channel);
    const limit = typeof feed.limit === "number" ? Math.min(PAGE_SIZE, Math.max(MIN_LIMIT, Math.floor(feed.limit))) : PAGE_SIZE;
    void this.opening(channel, limit, feed.since, feed.fp);
  }

  private async opening(channel: Channel, limit: number, since: unknown, fp: unknown): Promise<void> {
    this.source.touch(channel.source, channel.id);
    await this.source.whenLoaded();
    const copy = await this.source.session(channel.source, channel.id);
    if (this.channels.get(channel.sub) !== channel) return;
    const row = rowOf(this.source, this.opts, channel.source, channel.id);
    if (!row) {
      this.channels.delete(channel.sub);
      this.emit({ sub: channel.sub, type: "closed" });
      return;
    }
    const state = copy?.state ?? EMPTY_COPY;
    const reason = copy ? this.opts.messages.readOnly : this.opts.messages.noCopy;
    channel.rowJson = JSON.stringify(row);
    channel.reason = reason;
    channel.open = true;
    channel.endIndex = state.endIndex;
    if (copy && holdsThisCopy(state, since, fp)) {
      // The window held is this copy's (it was cut here, or by a machine
      // that sent the same marks): resume it, earlier pages and all, and
      // replace the state the machine last sent with the read-only one.
      this.emit({ sub: channel.sub, ...deltaFrame(state, copy.seq, Math.max(state.startIndex, since - RESUME_OVERLAP)) });
      this.emit({ sub: channel.sub, type: "state", ...offlineState(row, reason) });
      return;
    }
    this.emit({ sub: channel.sub, type: "snapshot", sig: offlineSig(copy?.seq ?? 0), ...offlineState(row, reason), ...windowFields(state, limit) });
  }

  private async resync(sub: number, since: unknown, fp: unknown): Promise<void> {
    const channel = this.channels.get(sub);
    if (!channel?.open) return;
    const copy = this.source.sessionNow(channel.source, channel.id);
    const state = copy?.state ?? EMPTY_COPY;
    const seq = copy?.seq ?? 0;
    channel.endIndex = state.endIndex;
    if (holdsThisCopy(state, since, fp)) {
      this.emit({ sub, ...deltaFrame(state, seq, Math.max(state.startIndex, since - RESUME_OVERLAP)) });
      return;
    }
    this.emit({ sub, type: "reset", sig: offlineSig(seq), ...windowFields(state, PAGE_SIZE) });
  }

  private changed(change: MirrorChange): void {
    if (this.readyState !== OPEN) return;
    if (change.kind === "status") {
      this.sendSettings();
      return;
    }
    if (change.kind === "index") {
      const rows = rowsOf(this.source, this.opts);
      const upserts: SessionSummary[] = [];
      const now = new Set<string>();
      for (const row of rows) {
        const key = `${row.source}/${row.id}`;
        now.add(key);
        const text = JSON.stringify(row);
        if (this.seen.get(key) !== text) {
          this.seen.set(key, text);
          upserts.push(row);
        }
      }
      const removed = [...this.seen.keys()].filter((key) => !now.has(key));
      for (const key of removed) this.seen.delete(key);
      if (upserts.length || removed.length) this.emit({ type: "sessions:delta", upserts, removed });
      for (const channel of this.channels.values()) {
        if (!channel.open) continue;
        const row = rowOf(this.source, this.opts, channel.source, channel.id);
        const text = row ? JSON.stringify(row) : "";
        if (!row || text === channel.rowJson) continue;
        channel.rowJson = text;
        this.emit({ sub: channel.sub, type: "state", ...offlineState(row, channel.reason) });
      }
      return;
    }
    for (const channel of this.channels.values()) {
      if (!channel.open || `${channel.source}/${channel.id}` !== change.key) continue;
      const copy = this.source.sessionNow(channel.source, channel.id);
      if (!copy) continue;
      const state = copy.state;
      const from = change.fromIndex;
      if (from === null || from < state.startIndex || from > channel.endIndex) {
        this.emit({ sub: channel.sub, type: "reset", sig: offlineSig(copy.seq), ...windowFields(state, PAGE_SIZE) });
      } else {
        this.emit({ sub: channel.sub, ...deltaFrame(state, copy.seq, from) });
      }
      channel.endIndex = state.endIndex;
      // A copy that arrived for a session that had none: the reason changes.
      const row = channel.reason !== this.opts.messages.readOnly ? rowOf(this.source, this.opts, channel.source, channel.id) : undefined;
      if (row) {
        channel.reason = this.opts.messages.readOnly;
        channel.rowJson = JSON.stringify(row);
        this.emit({ sub: channel.sub, type: "state", ...offlineState(row, channel.reason) });
      }
    }
  }
}

/** The transport over the copy (`source` `null`: there is none — every request refused, an empty list). */
export function offlineTransport(source: OfflineSource | null, opts: OfflineOptions): Transport {
  return {
    tunneled: true,
    fetch: (request) => offlineFetch(source, opts, request),
    socketUrl: async (path) => path,
    openSocket: (url) => (source && url.startsWith("/api/events") ? new OfflineMuxSocket(source, opts) : new RefusedSocket()),
  };
}
