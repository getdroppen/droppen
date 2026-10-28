import type { components } from "../../api/schema.js";
import {
  applyStateFrame,
  applyTranscriptDelta,
  historyQueryFor,
  prependHistoryPage,
  resumeMarksOf,
  resumeQuery,
  resyncMessage,
  transcriptDeltaOf,
  transcriptWindowOf,
  trimLiveWindow,
  type TranscriptHistoryPage,
  type TranscriptWindow,
} from "./session-feed-state.js";
import { createDraftStore, thinkingOf, type DraftStore } from "./stream-draft.js";
import { isSessionSource } from "./new-session-flow";

/*
 * The session websockets, owned by the page rather than by the view that shows
 * one of them.
 *
 * A transcript view used to own its socket outright: mounting it opened one,
 * unmounting closed it, and the window it had been sent went with it. So every
 * switch between two sessions was a fresh connection — on a phone behind the
 * tunnel that is a TCP and a TLS handshake, a ticket round-trip and a whole
 * snapshot (a few hundred kilobytes of JSON, uncompressed on a websocket) —
 * and switching *back* paid all of it again for a session that had been on
 * screen a moment ago.
 *
 * Here a session's feed outlives its view, in two tiers:
 *
 * - the last few sessions read keep their socket open ([`FeedPoolLimits.idleSockets`],
 *   for [`FeedPoolLimits.idleMs`]): switching back costs nothing on the wire
 *   beyond the frames that would have arrived anyway, and the window is
 *   current the moment it is drawn;
 * - a longer tail keeps the window alone ([`FeedPoolLimits.cached`]): it is
 *   drawn at once on the way back, and the socket that reopens resumes from it
 *   (`?since=&fp=`) and is sent the few items it is missing instead of a page.
 *
 * Nothing about a feed that nobody is looking at reaches the reader: marking a
 * session read, the first-unread jump and the unread pill all belong to the
 * view (`use-seen.ts`, `first-unread-view.tsx`), which only exists while the
 * session is on screen. A pooled socket only keeps the window current. One
 * that drops while hidden is not reconnected — it resumes when it is next
 * shown — so a server restart is not a reconnect storm of sessions nobody has
 * open.
 */

type Detail = components["schemas"]["SessionDetail"];

/** Client-side session state fed over the websocket. */
export type FeedState = Omit<Detail, "itemsChanged" | "reset"> &
  Pick<TranscriptWindow, "prefixFp" | "startFp">;

/**
 * Whether the socket behind this view is delivering.
 *
 * `gone` is reached only after several attaches in a row were answered with a
 * `closed` frame, because one of them means almost nothing: the server
 * broadcasts `Closed` to every feed when it restarts to update itself, so
 * treating the first one as "this session does not exist" blanked every open
 * session in every open tab whenever a new build shipped. `cached` is a window
 * drawn from the copy kept on this device ([`FeedPersist`]) that no frame has
 * confirmed yet.
 */
export type FeedConnection = "live" | "reconnecting" | "gone" | "cached";

/** Everything a view renders from a feed; replaced, never mutated. */
export interface FeedView {
  data: FeedState | null;
  connection: FeedConnection;
  isLoadingEarlier: boolean;
  historyError: string | null;
}

/**
 * How many `closed` frames in a row before the session is called missing.
 *
 * A self-update takes one restart, so a handful of attempts at the backoff
 * below spans well past it. An archived or deleted session answers `closed`
 * every time and reaches the verdict in under half a minute.
 */
export const CLOSED_FRAMES_BEFORE_GONE = 5;

/** The smallest slice of a browser `WebSocket` the feed uses. */
export interface FeedSocket {
  readonly readyState: number;
  send(data: string): void;
  close(): void;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: unknown) => void) | null;
}

/** `WebSocket.OPEN`, spelled out so this module runs where there is no `WebSocket`. */
const SOCKET_OPEN = 1;

/** What a feed asks the page's shared socket for; see `app/feed-hub.ts`. */
export interface FeedOpen {
  source: string;
  id: string;
  since?: number;
  fp?: string | null;
  limit?: number;
}

/**
 * How many items a feed's opening snapshot asks for on the shared socket.
 *
 * A screen of transcript is a fraction of a page — tool steps fold into one
 * line each — and on a weak link every item past what is drawn is part of the
 * wait. Earlier ones are a `history` page away, which scrolling up or a search
 * fetches anyway.
 */
export const OPENING_ITEMS = 100;

/** How long a feed waits for its device copy before connecting without it. */
export const HYDRATE_WAIT_MS = 300;

/** How many of a window's newest items are kept on the device. */
export const PERSIST_ITEMS = 60;

/**
 * A copy of the last few windows kept on the device, so that a cold start — a
 * reload, the installed app opened again — draws the transcript before the
 * network has said anything, and resumes it from there.
 */
export interface FeedPersist {
  load(key: string): Promise<FeedState | null>;
  save(key: string, data: FeedState): void;
}

/**
 * The part of a window worth keeping on the device: its newest
 * {@link PERSIST_ITEMS} items. Cutting the front leaves the resume marker —
 * `endIndex` and `prefixFp` — exactly as it was, so the copy resumes like the
 * window it came from; only `startFp` goes unknown, which "load earlier"
 * already handles by proving the page against the resume marker instead.
 */
export function persistable(data: FeedState, keep: number = PERSIST_ITEMS): FeedState {
  if (data.items.length <= keep) return data;
  const cut = data.items.length - keep;
  const startIndex = data.startIndex + cut;
  return {
    ...data,
    items: data.items.slice(cut),
    startIndex,
    omittedItems: startIndex,
    hasMoreBefore: startIndex > 0,
    startFp: null,
  };
}

/** What the feeds need from the page: sockets, and the two REST reads. */
export interface FeedPoolDeps {
  /** The device copy, where the page keeps one. */
  persist?: FeedPersist;
  /**
   * A channel on the page's shared socket, or `null` to open a socket of the
   * feed's own (no shared socket, or a server too old to carry feeds on it).
   * The frames a channel delivers may already be parsed.
   */
  openFeed?(target: FeedOpen): FeedSocket | null;
  /** The URL to open `path` with (minting a ticket where a login is required). */
  socketUrl(path: string): Promise<string>;
  openSocket(url: string): FeedSocket;
  /** A failed attempt, with the streak it ends; see `noteWsFailure`. */
  noteFailure(streak: number): void;
  /** `GET …/history`: the page, or the status that refused it. */
  fetchHistory(
    source: string,
    id: string,
    query: ReturnType<typeof historyQueryFor>,
  ): Promise<{ page?: TranscriptHistoryPage; status: number; error?: string }>;
  /** `GET /api/sessions/{source}/{id}` with `after`/`sig`, for its state fields. */
  fetchState(source: string, id: string, held: { after: number; sig: string }): Promise<Partial<FeedState> | null>;
  now?: () => number;
}

export interface FeedPoolLimits {
  /** Sessions whose window is kept for an instant switch back. */
  cached: number;
  /** Of those, how many nobody is looking at may keep their socket open. */
  idleSockets: number;
  /** How long such a socket stays open with nobody looking. */
  idleMs: number;
}

/**
 * Four sockets is the page plus the three sessions most likely to be switched
 * back to, and on a weak link an idle feed costs a ping every thirty seconds
 * and the frames of sessions that are actually moving — the same frames the
 * reader would be sent on arrival anyway. Five minutes covers going to look
 * something up in another session and coming back.
 */
export const DEFAULT_FEED_LIMITS: FeedPoolLimits = { cached: 8, idleSockets: 3, idleMs: 5 * 60_000 };

export function updateFeedWindow(state: FeedState, window: TranscriptWindow): FeedState {
  if (window === state) return state;
  return { ...state, ...window, omittedItems: window.startIndex };
}

/** A `snapshot` frame as the state it opens a window with, or `null` for one without a signature. */
export function snapshotOf(m: Record<string, unknown> & Partial<FeedState>): FeedState | null {
  if (typeof m.sig !== "string") return null;
  const items = m.items ?? [];
  const startIndex =
    m.startIndex ?? m.omittedItems ?? Math.max(0, (m.totalItems ?? items.length) - items.length);
  const endIndex = m.endIndex ?? startIndex + items.length;
  return {
    sig: m.sig,
    summary: m.summary!,
    injection: m.injection ?? null,
    canSend: m.canSend ?? false,
    sendBlockReason: m.sendBlockReason ?? null,
    cliChoice: m.cliChoice ?? null,
    contextTokens: m.contextTokens ?? null,
    // A snapshot from a server that predates the field leaves the ring
    // measured against the ordinary window rather than dividing by zero.
    contextWindow: m.contextWindow ?? 200_000,
    claudeRoot: m.claudeRoot ?? null,
    account: m.account ?? null,
    rotation: m.rotation ?? null,
    receipts: m.receipts ?? [],
    // Empty from a server that predates questions: every card then
    // draws the call read-only, which is all such a server could take.
    questions: m.questions ?? [],
    items,
    startIndex,
    endIndex,
    totalItems: m.totalItems ?? endIndex,
    hasMoreBefore: m.hasMoreBefore ?? startIndex > 0,
    omittedItems: startIndex,
    ...resumeMarksOf(m),
  };
}

/** One socket's worth of bookkeeping: everything that starts over with a new attach. */
interface Run {
  stopped: boolean;
  ws: FeedSocket | null;
  retry: number;
  // Counted apart from `retry` and never reset by a socket that merely opened:
  // a server that answers every attach with `closed` opens the socket every
  // time, so only a snapshot proves the session is really there.
  closedFrames: number;
  lastWasClosedFrame: boolean;
  timer: ReturnType<typeof setTimeout> | undefined;
  // One resync in flight per socket: every delta that misses while it is out
  // is covered by its answer, which is cut from the server's latest read.
  resyncPending: boolean;
  /** The server refused the subscription outright; it is not asked again. */
  refused: boolean;
}

/** Whether a view following the tail may let the window be trimmed. */
export interface PinSource {
  readonly current: boolean | null;
}

/** One session's feed: its window, its draft, and the socket keeping them current. */
export class SessionFeed {
  readonly key: string;
  readonly drafts: DraftStore = createDraftStore();
  private view: FeedView = { data: null, connection: "live", isLoadingEarlier: false, historyError: null };
  private readonly listeners = new Set<() => void>();
  private readonly viewers = new Set<PinSource>();
  private run: Run | null = null;
  /** Bumped whenever the window is thrown away, so a late page cannot land on its successor. */
  private epoch = 0;
  private idleTimer: ReturnType<typeof setTimeout> | undefined;

  /** The shared socket refused this feed for want of room: use a socket of its own. */
  private ownSocket = false;

  /** The device copy being read, which the first attach waits for. */
  private hydrating: Promise<void> | null = null;
  private saveTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    readonly source: string,
    readonly id: string,
    private readonly deps: FeedPoolDeps,
  ) {
    this.key = `${source}/${id}`;
    const persist = deps.persist;
    if (persist) {
      // Bounded: the copy is worth a moment's wait, never a transcript that
      // does not connect because the device's storage stopped answering. A
      // copy that turns up later is not used — by then the socket is asking
      // for a window of its own.
      this.hydrating = new Promise<void>((resolve) => {
        let done = false;
        const finish = (data: FeedState | null) => {
          if (done) return;
          done = true;
          clearTimeout(timer);
          this.hydrating = null;
          if (data && !this.view.data) this.patch({ data, connection: "cached" });
          resolve();
        };
        const timer = setTimeout(() => finish(null), HYDRATE_WAIT_MS);
        persist.load(this.key).then(finish, () => finish(null));
      });
    }
  }

  // ------------------------------------------------------------ the store

  readonly subscribe = (onChange: () => void): (() => void) => {
    this.listeners.add(onChange);
    return () => {
      this.listeners.delete(onChange);
    };
  };

  readonly getView = (): FeedView => this.view;

  private patch(next: Partial<FeedView>): void {
    const merged = { ...this.view, ...next };
    if (
      merged.data === this.view.data &&
      merged.connection === this.view.connection &&
      merged.isLoadingEarlier === this.view.isLoadingEarlier &&
      merged.historyError === this.view.historyError
    ) {
      return;
    }
    const dataChanged = merged.data !== this.view.data;
    this.view = merged;
    // Only a window somebody is reading: a pooled feed in the background moves
    // too, and saving it would push out of the kept few a session that was
    // actually read. Its copy is the one it had when it was last shown.
    if (dataChanged && this.viewers.size > 0) this.schedulePersist();
    for (const listener of [...this.listeners]) listener();
  }

  /** Keep the device copy a couple of seconds behind the window, never more often. */
  private schedulePersist(): void {
    if (!this.deps.persist || this.saveTimer !== undefined) return;
    this.saveTimer = setTimeout(() => this.persistNow(), 2000);
  }

  private persistNow(): void {
    clearTimeout(this.saveTimer);
    this.saveTimer = undefined;
    const data = this.view.data;
    // The copy itself, read back, is not news; nor is a window torn down.
    if (data && this.view.connection !== "cached") this.deps.persist?.save(this.key, persistable(data));
  }

  /** `setState`'s updater form, over the window. The updater runs exactly once. */
  private setData(update: (prev: FeedState | null) => FeedState | null): void {
    this.patch({ data: update(this.view.data) });
  }

  /** Whether anybody is showing this feed. */
  get watched(): boolean {
    return this.viewers.size > 0;
  }

  /** Whether a socket is open or being opened for it. */
  get running(): boolean {
    return this.run !== null;
  }

  /**
   * Whether a delta may trim the window's oldest items: every view showing it
   * is following the tail and none is waiting on an earlier page. With nobody
   * showing it, yes — the next view starts at the bottom.
   */
  private mayTrim(): boolean {
    if (this.view.isLoadingEarlier) return false;
    for (const viewer of this.viewers) if (viewer.current === false) return false;
    return true;
  }

  // ------------------------------------------------------------ lifecycle

  /** A view starts showing this feed; `pinned` is its follow-the-tail flag. */
  attach(pinned: PinSource): void {
    this.viewers.add(pinned);
    clearTimeout(this.idleTimer);
    this.idleTimer = undefined;
    this.start();
  }

  /** That view stops; `idleMs` later the socket follows, if nobody came back. */
  detach(pinned: PinSource, idleMs: number): void {
    this.viewers.delete(pinned);
    if (this.viewers.size > 0) return;
    // The window as the reader left it, rather than a save that would now be
    // skipped because nobody is reading.
    if (this.saveTimer !== undefined) this.persistNow();
    clearTimeout(this.idleTimer);
    this.idleTimer = setTimeout(() => this.stop(), idleMs);
  }

  /** Close the socket and keep the window, which the next attach resumes from. */
  stop(): void {
    clearTimeout(this.idleTimer);
    this.idleTimer = undefined;
    const run = this.run;
    if (!run) return;
    run.stopped = true;
    clearTimeout(run.timer);
    this.run = null;
    run.ws?.close();
    // Whatever was in flight is the next attach's to say; a count left over
    // from before would otherwise flash on the way back.
    this.drafts.clear();
    this.loadingEarlier = false;
    this.patch({ isLoadingEarlier: false });
  }

  /**
   * Re-attach from nothing: adoption rewrites which file this session *is*,
   * and the watcher behind the old socket has no reason to notice. Throws the
   * window away, earlier pages and all, which is why it is not how a view
   * comes back to a feed.
   */
  reattach(): void {
    this.stop();
    this.epoch++;
    this.drafts.clear();
    this.patch({ data: null, connection: "live", historyError: null, isLoadingEarlier: false });
    if (this.viewers.size > 0) this.start();
  }

  private start(): void {
    if (this.run) return;
    const run: Run = {
      stopped: false,
      ws: null,
      retry: 0,
      closedFrames: 0,
      lastWasClosedFrame: false,
      timer: undefined,
      resyncPending: false,
      refused: false,
    };
    this.run = run;
    this.drafts.clear();
    // A window read back from the device stays marked as such until a frame
    // has confirmed or replaced it.
    this.patch({ connection: this.view.connection === "cached" ? "cached" : "live", historyError: null });
    this.connect(run);
  }

  private connect(run: Run): void {
    if (run.stopped) return;
    // The device copy is what lets this attach resume rather than re-download
    // a page, so a read still in flight is worth the few milliseconds' wait.
    if (this.hydrating) {
      void this.hydrating.then(() => {
        if (!run.stopped && this.run === run) this.connect(run);
      });
      return;
    }
    const held = this.view.data;
    const channel = this.ownSocket ? null : this.deps.openFeed?.({
      source: this.source,
      id: this.id,
      since: held?.endIndex,
      fp: held?.prefixFp ?? null,
      limit: OPENING_ITEMS,
    });
    if (channel) {
      this.attachSocket(run, channel);
      return;
    }
    // On a server that requires a login the URL needs a freshly minted
    // one-shot ticket, so the socket opens a round-trip later — by which time
    // this run may have been stopped. A client that holds a window says so,
    // and is resumed with what it is missing instead of being re-sent a
    // snapshot that would replace its earlier pages.
    const path = `/api/sessions/${this.source}/${this.id}/ws${resumeQuery(this.view.data)}`;
    this.deps.socketUrl(path).then(
      (url) => this.open(run, url),
      () => {
        // No ticket, no socket: back off like a refused connection.
        if (run.stopped) return;
        run.retry += 1;
        this.deps.noteFailure(run.retry);
        run.timer = setTimeout(() => this.connect(run), Math.min(500 * 2 ** (run.retry - 1), 8000));
      },
    );
  }

  private requestResync(run: Run, held: TranscriptWindow | null): void {
    const ws = run.ws;
    if (run.resyncPending || !ws || ws.readyState !== SOCKET_OPEN) return;
    run.resyncPending = true;
    ws.send(resyncMessage(held));
  }

  private open(run: Run, url: string): void {
    if (run.stopped) return;
    this.attachSocket(run, this.deps.openSocket(url));
  }

  private attachSocket(run: Run, ws: FeedSocket): void {
    run.ws = ws;
    ws.onopen = () => {
      run.retry = 0;
      run.resyncPending = false;
    };
    // A shared-socket channel can be open by the time it is handed over; an
    // open this feed never hears of is a retry streak it never resets.
    if (ws.readyState === SOCKET_OPEN) ws.onopen({});
    ws.onmessage = (e) => {
      if (run.stopped) return;
      let m: Record<string, unknown> & Partial<FeedState>;
      try {
        m = typeof e.data === "string" ? JSON.parse(e.data) : (e.data as typeof m);
      } catch {
        return;
      }
      if (typeof m !== "object" || m === null) return;
      this.frame(run, m);
    };
    ws.onclose = () => {
      if (run.stopped || run.ws !== ws) return;
      // A think may have ended while nobody was listening; the next attach
      // opens with the count as it is then, if there is one.
      this.drafts.clear();
      // The server refused this subscription for good (see the `closed`
      // frame below): going back for it would only be refused again.
      if (run.refused) {
        this.stop();
        return;
      }
      // Nobody is looking: keep the window, let the socket go, and resume
      // when somebody does. Reconnecting hidden feeds is how a server restart
      // turns into a burst of sockets for sessions nobody has open.
      if (this.viewers.size === 0) {
        this.stop();
        return;
      }
      // Two backoffs, because the two causes are different. An ordinary drop
      // is a socket problem and is usually over immediately; a `closed` frame
      // means the server is deliberately refusing this feed, and hammering it
      // through a restart helps nobody — so that one backs off further, to
      // half a minute.
      let delay: number;
      if (run.lastWasClosedFrame) {
        delay = Math.min(1000 * 2 ** (run.closedFrames - 1), 30_000);
      } else {
        run.retry += 1;
        // A run of attaches that never open can also be the token having
        // expired under us; this is what notices that.
        this.deps.noteFailure(run.retry);
        delay = Math.min(500 * 2 ** (run.retry - 1), 8000);
      }
      run.lastWasClosedFrame = false;
      run.timer = setTimeout(() => this.connect(run), delay);
    };
  }

  /** One frame off the socket. */
  private frame(run: Run, m: Record<string, unknown> & Partial<FeedState>): void {
    if (m.type === "snapshot") {
      const snapshot = snapshotOf(m);
      if (!snapshot) return;
      // A reconnect snapshot has no verifiable lineage for pages loaded
      // before its tail, so replace the window instead of mixing versions.
      // (A reconnect the server could resume is answered with a delta.)
      run.resyncPending = false;
      this.setData(() => snapshot);
      // The session answered, so whatever the last few frames said is spent.
      run.closedFrames = 0;
      this.patch({ historyError: null, connection: "live" });
    } else if (m.type === "reset") {
      run.resyncPending = false;
      const window = transcriptWindowOf(m);
      this.setData((prev) => (prev && window ? updateFeedWindow(prev, window) : prev));
    } else if (m.type === "delta") {
      const delta = transcriptDeltaOf(m);
      // A resumed attach opens with a delta rather than a snapshot, and is
      // just as much proof that the session is there.
      run.closedFrames = 0;
      this.patch({ connection: "live" });
      this.setData((prev) => {
        if (!prev) return prev;
        const grown = applyTranscriptDelta(prev, delta);
        if (!grown) {
          // Never dropped silently: a window that stops matching the
          // session is worse than a moment of waiting. The answer is a
          // delta that fits, or a reset.
          queueMicrotask(() => this.requestResync(run, prev));
          return prev;
        }
        run.resyncPending = false;
        // The one place the window is allowed to get shorter. Three things
        // have to be true at once, and each of them is load-bearing: the
        // delta was accepted (a rejected one returned above and costs no
        // render), every reader is following the tail rather than parked in
        // the history that would be cut away, and no earlier-history request
        // is out — one of those was sent with a `before` that a trim would
        // move out from under it, and `prependHistoryPage` would refuse the
        // answer.
        const window = this.mayTrim() ? trimLiveWindow(grown) : grown;
        return updateFeedWindow(prev, window);
      });
    } else if (m.type === "state") {
      // Every merge rule, and the reason a frame that says nothing new has
      // to come back out by identity, is in [`applyStateFrame`].
      this.setData((prev) => (prev ? applyStateFrame(prev, m) : prev));
    } else if (m.type === "draft") {
      // Its own store, never the window's state, and only the count of a
      // think in flight out of it (`stream-draft.ts`): nothing in a draft
      // waits on the transcript, so it goes in the moment it arrives.
      this.drafts.receive(thinkingOf(m));
    } else if (m.type === "closed") {
      // NOT a terminal state. The server broadcasts this to every feed when
      // it restarts to install its own update, so treating it as "the
      // session is gone" took down every open transcript in every open tab
      // on each release, and only a manual reload brought them back. Keep
      // what is loaded, say the connection dropped, and go back for it.
      run.closedFrames++;
      run.lastWasClosedFrame = true;
      // The shared socket says why when it is the request itself it refused:
      // one it cannot read (`invalid`, or a subscription number used twice,
      // `reused`) is a verdict, and one past its limit (`limit`) goes on, on a
      // socket of the feed's own.
      const reason = typeof m.reason === "string" ? m.reason : null;
      if (reason === "invalid" || reason === "reused") {
        run.refused = true;
        run.closedFrames = CLOSED_FRAMES_BEFORE_GONE;
      } else if (reason === "limit") {
        this.ownSocket = true;
      }
      this.patch({ connection: run.closedFrames >= CLOSED_FRAMES_BEFORE_GONE ? "gone" : "reconnecting" });
      run.ws?.close();
    }
  }

  // ------------------------------------------------------------ the REST half

  private loadingEarlier = false;

  /** Page in the items before the window. */
  async loadEarlier(): Promise<void> {
    const data = this.view.data;
    const before = data?.startIndex ?? 0;
    if (
      !data?.hasMoreBefore ||
      before <= 0 ||
      this.loadingEarlier ||
      !isSessionSource(this.source)
    ) {
      return;
    }
    const requestedSig = data.sig;
    const requestedStartFp = data.startFp ?? null;
    const epoch = this.epoch;
    this.loadingEarlier = true;
    this.patch({ isLoadingEarlier: true, historyError: null });
    try {
      // The query proves which prefix the page has to hang off, so the server
      // can serve it however much the session has grown since.
      const query = historyQueryFor({
        startIndex: before,
        endIndex: data.endIndex,
        startFp: requestedStartFp,
        prefixFp: data.prefixFp,
      });
      const res = await this.deps.fetchHistory(this.source, this.id, query);
      if (this.epoch !== epoch) return;
      if (!res.page) {
        // The items before this window are not the ones it was cut from any
        // more, so it cannot be extended, only replaced.
        if (res.status === 409) {
          if (this.run) this.requestResync(this.run, null);
          return;
        }
        throw new Error(res.error ?? "empty history response");
      }
      const page = res.page;
      if (!page.startFp && page.sig !== requestedSig) {
        throw new Error("session changed while loading history");
      }
      this.setData((prev) => {
        // Whatever the socket did meanwhile, the page still fits a window
        // that starts where it did on the same fingerprint.
        if (
          !prev ||
          prev.startIndex !== before ||
          prev.startFp !== requestedStartFp ||
          (!page.startFp && prev.sig !== requestedSig)
        ) {
          return prev;
        }
        return updateFeedWindow(prev, prependHistoryPage(prev, page));
      });
    } catch (err) {
      if (this.epoch === epoch) this.patch({ historyError: err instanceof Error ? err.message : String(err) });
    } finally {
      if (this.epoch === epoch) {
        this.loadingEarlier = false;
        this.patch({ isLoadingEarlier: false });
      }
    }
  }

  /**
   * Re-read the state fields alone, leaving the transcript window untouched.
   *
   * Passing the window already held is what makes the re-read cheap: the
   * server answers `after`/`sig` with no transcript delta at all, so this
   * costs a state read and nothing else.
   */
  refreshState(): void {
    if (!isSessionSource(this.source)) return;
    const epoch = this.epoch;
    const held = { after: this.view.data?.totalItems ?? 0, sig: this.view.data?.sig ?? "" };
    void this.deps.fetchState(this.source, this.id, held).then(
      (fresh) => {
        if (this.epoch !== epoch || !fresh) return;
        // State only. The response can carry a transcript delta and merging it
        // here would be a second, unsynchronised writer into the window the
        // socket owns — the socket's own delta is still coming, and it is the
        // one with a signature chain behind it. [`applyStateFrame`] reads the
        // state fields off `fresh` and is blind to the rest of it, which is
        // what makes it the right merge for both writers.
        this.setData((prev) => (prev ? applyStateFrame(prev, fresh) : prev));
      },
      () => undefined,
    );
  }
}

/** Every session feed the page holds, most recently shown last. */
export interface SessionFeedPool {
  /** The feed for a session, created on first ask. Showing it is [`acquire`]'s job. */
  feed(source: string, id: string): SessionFeed;
  /** A view starts showing `feed`; the call returned is the view going away. */
  acquire(feed: SessionFeed, pinned: PinSource): () => void;
  /** The feeds held, oldest first; for tests and diagnostics. */
  held(): SessionFeed[];
  /**
   * Stop every feed and let go of it: the machine went (closed on the page,
   * dropped from the account). Their sockets close, and no backoff or idle
   * timer of theirs runs after this.
   */
  dispose(): void;
}

export function createSessionFeedPool(
  deps: FeedPoolDeps,
  limits: FeedPoolLimits = DEFAULT_FEED_LIMITS,
): SessionFeedPool {
  const feeds = new Map<string, SessionFeed>();

  /** Hold the newest few windows, and the newest few idle sockets among them. */
  const enforce = () => {
    let sockets = 0;
    let windows = 0;
    for (const feed of [...feeds.values()].reverse()) {
      if (feed.watched) continue;
      if (feed.running && ++sockets > limits.idleSockets) feed.stop();
      if (++windows > limits.cached) {
        feed.stop();
        feeds.delete(feed.key);
      }
    }
  };

  return {
    feed(source, id) {
      const key = `${source}/${id}`;
      let feed = feeds.get(key);
      if (!feed) {
        // Not trimmed here: this runs during a render, and stopping another
        // feed would wake its subscribers mid-render. The next release does it.
        feed = new SessionFeed(source, id, deps);
        feeds.set(key, feed);
      }
      return feed;
    },
    acquire(feed, pinned) {
      // Most recently shown last. A feed evicted between the render that
      // looked it up and the view that shows it is simply taken back.
      const other = feeds.get(feed.key);
      if (other && other !== feed) other.stop();
      feeds.delete(feed.key);
      feeds.set(feed.key, feed);
      feed.attach(pinned);
      let done = false;
      return () => {
        if (done) return;
        done = true;
        feed.detach(pinned, limits.idleMs);
        enforce();
      };
    },
    held: () => [...feeds.values()],
    dispose() {
      for (const feed of feeds.values()) feed.stop();
      feeds.clear();
    },
  };
}
