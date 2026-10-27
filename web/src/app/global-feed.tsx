import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useSyncExternalStore,
} from "react";
import type { components } from "@/api/schema";
import { compareSessions } from "@/features/sessions/session-list-model";
import { readLocally } from "@/features/sessions/session-attention";
import { screensFeedEvent, type ScreensFeedEvent } from "./screen-sync";
import { holdsPageLogin, invalidateDownloadTicket, onSignedOut, serverRestarted } from "@/lib/auth";
import { useMachine } from "@/lib/machines/context";
import { LOCAL, sessionKey, type MachineKey, type SessionRef } from "@/lib/machines/key";
import { homeMachine, isOpen, perMachine, transportFor } from "@/lib/machines/registry";
import { noteWsFailure } from "@/lib/ws-url";
import { browserInflate, compressionAllowed, FeedHub } from "./feed-hub";
import { loadSessionList, saveSessionList } from "@/features/sessions/feed-persist";
import { pluginEvents } from "@/plugins/events";
import type { PluginStatus } from "@/plugins/types";

/**
 * A machine's one websocket — its session list, and every session feed riding
 * on it (`feed-hub.ts`). Started and stopped by that machine's
 * {@link GlobalFeedProvider}; `session-detail.tsx` opens its feeds' channels on
 * it. Each attempt goes through whatever the machine's transport is at the time
 * (`lib/machines`).
 */
export function feedHubFor(machine: MachineKey): FeedHub {
  return feedHubs(machine).hub;
}

const feedHubs = perMachine(
  (machine: MachineKey) => {
    const hub = new FeedHub({
      socketUrl: (path) => transportFor(machine).socketUrl(path),
      openSocket: (url) => transportFor(machine).openSocket(url),
      noteFailure: (streak) => noteWsFailure(streak, machine),
      inflate: browserInflate(),
      compress: () => compressionAllowed(transportFor(machine)),
    });
    // Nothing the dead login's socket says may reach the page — or the device
    // copy being cleared — in the moment before the login page replaces it.
    const off = onSignedOut(() => hub.shutdown());
    return { hub, off };
  },
  ({ hub, off }) => {
    off();
    hub.shutdown();
  },
);

type SessionSummary = components["schemas"]["SessionSummary"];

export interface ServerInfo {
  hostname: string;
  platform: string;
  nodeVersion: string;
  /** Server build, e.g. "0.1.0+g26305ca"; absent on servers older than self-update. */
  serverVersion?: string;
  homeDir: string;
  claudeDir: string;
  codexDir: string;
  serverStartedAt: string;
  /** The plugins installed in this instance and whether each is on; absent on older servers. */
  plugins?: PluginStatus[];
  /** Played from the machine's offline copy (`lib/transport/offline.ts`) rather than said by the machine itself. */
  fromCopy?: boolean;
}

export interface GlobalFeedState {
  /** Whose feed this is: every row in it is a session on this machine. */
  machine: MachineKey;
  connected: boolean;
  settings: ServerInfo | null;
  /** All sessions, newest first; null until the first snapshot arrives. */
  sessions: SessionSummary[] | null;
}

/**
 * The feed as an external store rather than as a context *value*.
 *
 * The distinction is the whole point of this file's shape, and it cost real
 * CPU. A context value that changes re-renders every consumer of that context,
 * unconditionally — `React.memo` guards props, and a context read is not a
 * prop, so nothing downstream of `useContext` can opt out. The feed used to
 * hold `{connected, settings, sessions}` in a `useState` and publish that one
 * object, so a `sessions:delta` — which arrives for *any* of the sessions on
 * the machine, several times a second while one of them is working — re-rendered
 * all ten consumers. The expensive one was every mounted session transcript,
 * including the panes of screens the user had switched away from and sessions
 * that had not changed in hours: each re-render re-derives its grouping and
 * pairing across the whole loaded window and builds an element per item, for a
 * title string that had not moved.
 *
 * So the value handed down the context is this store, whose identity never
 * changes, and consumers subscribe through {@link useGlobalFeedSelector} with
 * `useSyncExternalStore`. React re-renders a subscriber only when its selector
 * returns something new under `Object.is`, which is the property the whole fix
 * rests on: a hook that selects one session's title is woken by every delta,
 * compares two identical strings, and renders nothing. Selectors must therefore
 * return a primitive or a value whose identity is stable while the thing it
 * describes has not changed — a fresh object or array on each call would defeat
 * the comparison and re-render on every frame, which is what this replaced.
 *
 * The provider also stops re-rendering at all, since it no longer holds state:
 * its `children` element is created by its parent, so a subtree that reads
 * nothing from the feed is not visited on a delta either.
 */
export interface GlobalFeedStore {
  subscribe: (onChange: () => void) => () => void;
  getSnapshot: () => GlobalFeedState;
  /**
   * Merge a patch into the snapshot and wake the subscribers.
   *
   * A patch that changes nothing notifies nobody. That is not just an
   * optimisation: `ws.onclose` publishes `connected: false` on every failed
   * reconnect attempt, and a server that is down produces one of those every
   * few seconds for as long as it stays down.
   */
  publish: (patch: Partial<GlobalFeedState>) => void;
  /**
   * Listen for the screens' events. Kept apart from the snapshot because they
   * are news rather than state: nothing renders them, the screens provider
   * acts on each one (see `screen-sync.ts`).
   */
  onScreens: (listener: (event: ScreensFeedEvent) => void) => () => void;
  /** Hand a screens event to every {@link GlobalFeedStore.onScreens} listener. */
  emitScreens: (event: ScreensFeedEvent) => void;
  /**
   * Say that a session is being read right now; returns the call that says it
   * no longer is.
   *
   * While any reader holds it, the session's row in `sessions` shows nothing
   * unread — see `readLocally` for why. Counted, so two panes showing one
   * session do not cancel each other.
   */
  markReading: (session: SessionRef) => () => void;
}

type FeedContent = Omit<GlobalFeedState, "machine">;

const EMPTY: FeedContent = { connected: false, settings: null, sessions: null };

/** `machine`'s feed, starting from `initial` (empty by default). */
export function createGlobalFeedStore(machine: MachineKey, initial: FeedContent = EMPTY): GlobalFeedStore {
  let snapshot: GlobalFeedState = { ...initial, machine };
  // The rows as the server sent them; `snapshot.sessions` is these as read
  // locally (`readLocally`), which is what every consumer sees.
  let raw = initial.sessions;
  const reading = new Map<string, number>();
  const listeners = new Set<() => void>();
  const screenListeners = new Set<(event: ScreensFeedEvent) => void>();
  // Remembered against the two things it is made of, so a patch that touches
  // neither (a reconnect) hands back the very same array and wakes nobody.
  let readingVersion = 0;
  let made: { from: SessionSummary[] | null; version: number; rows: SessionSummary[] | null } = {
    from: raw,
    version: 0,
    rows: raw,
  };
  const visible = () => {
    if (made.from !== raw || made.version !== readingVersion) {
      made = { from: raw, version: readingVersion, rows: raw === null ? null : readLocally(raw, new Set(reading.keys()), machine) };
    }
    return made.rows;
  };
  const commit = (next: GlobalFeedState) => {
    if (
      next.connected === snapshot.connected &&
      next.settings === snapshot.settings &&
      next.sessions === snapshot.sessions
    ) {
      return;
    }
    snapshot = next;
    // Copied first: a listener is free to unsubscribe as it runs, and a `Set`
    // mutated mid-iteration would skip whoever came after it.
    for (const listener of [...listeners]) listener();
  };
  return {
    subscribe(onChange) {
      listeners.add(onChange);
      return () => {
        listeners.delete(onChange);
      };
    },
    getSnapshot: () => snapshot,
    publish(patch) {
      if ("sessions" in patch) raw = patch.sessions ?? null;
      commit({ ...snapshot, ...patch, machine, sessions: visible() });
    },
    markReading(session) {
      if (session.machine !== machine) return () => undefined;
      const key = sessionKey(session);
      reading.set(key, (reading.get(key) ?? 0) + 1);
      readingVersion += 1;
      commit({ ...snapshot, sessions: visible() });
      let done = false;
      return () => {
        if (done) return;
        done = true;
        const left = (reading.get(key) ?? 1) - 1;
        if (left > 0) reading.set(key, left);
        else reading.delete(key);
        readingVersion += 1;
        commit({ ...snapshot, sessions: visible() });
      };
    },
    onScreens(listener) {
      screenListeners.add(listener);
      return () => {
        screenListeners.delete(listener);
      };
    },
    emitScreens(event) {
      for (const listener of [...screenListeners]) listener(event);
    },
  };
}

/** What a tree with no provider above it reads: empty, and never notifies. */
const INERT_STORE: GlobalFeedStore = createGlobalFeedStore(LOCAL);

export const GlobalFeedContext = createContext<GlobalFeedStore>(INERT_STORE);

/**
 * The store itself, for the callers that want to *watch* the feed without
 * rendering on it — see `useReportSeen`, which only needs an effect to fire.
 */
export function useGlobalFeedStore(): GlobalFeedStore {
  return useContext(GlobalFeedContext);
}

/**
 * Subscribe to one derived slice of the feed.
 *
 * `select` must return a value that compares equal under `Object.is` whenever
 * the part of the feed it reads has not changed; see {@link GlobalFeedStore}
 * for why. Prefer a module-level or `useCallback`-wrapped selector: an inline
 * closure is correct but makes React re-check the snapshot after every render
 * of the consumer.
 */
export function useGlobalFeedSelector<T>(select: (state: GlobalFeedState) => T): T {
  const store = useContext(GlobalFeedContext);
  const read = useCallback(() => select(store.getSnapshot()), [store, select]);
  // The same reader for the server snapshot: this store is built client-side and
  // starts empty, so a server render sees the empty state and nothing tears.
  return useSyncExternalStore(store.subscribe, read, read);
}

const selectConnected = (state: GlobalFeedState) => state.connected;
const selectSettings = (state: GlobalFeedState) => state.settings;
const selectSessions = (state: GlobalFeedState) => state.sessions;

/** Whether the global websocket is up; doubles as the app's health signal. */
export function useFeedConnected(): boolean {
  return useGlobalFeedSelector(selectConnected);
}

/** Server info, or null until the first `settings` frame lands. */
export function useFeedSettings(): ServerInfo | null {
  return useGlobalFeedSelector(selectSettings);
}

/**
 * Every session row, newest first.
 *
 * This one really does change identity on every delta, so its consumers really
 * do re-render on every delta — a list of sessions has to. Reach for a narrower
 * selector wherever a component wants one row, or one field of one row.
 */
export function useFeedSessions(): SessionSummary[] | null {
  return useGlobalFeedSelector(selectSessions);
}

/** One session's row, or undefined while the feed has not carried it (or it is another machine's). */
export function feedRow(state: GlobalFeedState, session: SessionRef): SessionSummary | undefined {
  if (session.machine !== state.machine) return undefined;
  return state.sessions?.find((s) => s.source === session.source && s.id === session.id);
}

/**
 * One session's title, as the feed currently has it.
 *
 * A string rather than the row, deliberately. The row object is replaced every
 * time the server bumps anything about that session — `updatedAt` moves on
 * every streamed message — so a hook returning the row would re-render the
 * transcript of the session being worked on twice a second for a title that
 * only ever changes on a rename.
 */
export function useFeedSessionTitle(session: SessionRef): string | undefined {
  const select = useCallback(
    (state: GlobalFeedState) => feedRow(state, session)?.title,
    [session],
  );
  return useGlobalFeedSelector(select);
}

function sortSessions(map: Map<string, SessionSummary>): SessionSummary[] {
  // Moving first, then unread: whatever is working — or holding a message that
  // has not gone in yet — floats to the top of the sidebar (and survives its
  // 300-row cut), because that is the session the user is waiting on rather
  // than the other way round. Unread is next, and recency breaks ties inside
  // each band. Background work is not promoted; `activitySortRank` says why.
  return [...map.values()].sort(compareSessions);
}

/**
 * One machine's session list, shared by every part of the page that shows
 * that machine: the root (the home), a page of another machine
 * (`/m/<machine>/…`), a pane holding one of its sessions. The store is the
 * machine's and outlives any one of them (a remount, a second pane draws its
 * rows at once); the socket's list half runs while at least one of them is
 * there ({@link MachineFeed.acquire}) and stops when the last one goes —
 * which is also what lets the machine's session feeds, waiting on a list
 * that stopped (`feed-hub.ts`), go again as soon as anything shows it.
 */
export class MachineFeed {
  readonly store: GlobalFeedStore;
  private users = 0;
  private stopList: (() => void) | null = null;
  private disposed = false;

  constructor(readonly machine: MachineKey) {
    this.store = createGlobalFeedStore(machine);
  }

  /** Somebody shows this machine's list: start it if nobody did. Returns the release. */
  acquire(): () => void {
    if (this.disposed) return () => undefined;
    if (this.users++ === 0) this.stopList = startList(this.machine, this.store);
    let released = false;
    return () => {
      if (released) return;
      released = true;
      if (--this.users > 0 || this.disposed) return;
      this.stopList?.();
      this.stopList = null;
      // Nothing hears the socket close once its list is stopped: say it here.
      this.store.publish({ connected: false });
    };
  }

  /** How many parts of the page show it now. */
  get holders(): number {
    return this.users;
  }

  /** @internal The machine was closed: stop, whoever still holds it. */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.stopList?.();
    this.stopList = null;
  }
}

const machineFeeds = perMachine(
  (machine: MachineKey) => new MachineFeed(machine),
  (feed) => feed.dispose(),
);

/** `machine`'s list: its store, and the way to say the page shows it. */
export function machineFeedFor(machine: MachineKey): MachineFeed {
  return machineFeeds(machine);
}

/**
 * One slice of `machine`'s feed, wherever in the tree this is — for a view
 * that is about a machine other than its provider's (a pane's frame, the
 * rail's health dot for the page's machine). Same rules as
 * {@link useGlobalFeedSelector}.
 */
export function useMachineFeedSelector<T>(machine: MachineKey, select: (state: GlobalFeedState) => T): T {
  const store = machineFeedFor(machine).store;
  const read = useCallback(() => select(store.getSnapshot()), [store, select]);
  return useSyncExternalStore(store.subscribe, read, read);
}

/**
 * Start `machine`'s list into `store`: the device copy's list until the
 * socket's snapshot replaces it, then the socket's. Returns the stop.
 */
function startList(machine: MachineKey, store: GlobalFeedStore): () => void {
  const byKey = new Map<string, SessionSummary>();
  /**
   * Which server instance the last `settings` frame came from, or null before
   * the first one. This is the only place in the client that can notice a
   * restart, so it is also where the download-ticket cache is dropped: those
   * tickets live in the server's memory and do not survive one, and nothing
   * else would ever find out — the sockets reconnect with fresh `ws` tickets
   * of their own while every download quietly 401s until the ticket is due.
   */
  let serverBoot: string | null = null;

  // The list as this device last saw it, drawn until the socket's snapshot
  // replaces it: a cold start on a weak link otherwise shows an empty sidebar
  // for as long as the handshake takes.
  let live = false;
  void loadSessionList(machine).then((rows) => {
    if (!live && rows && store.getSnapshot().sessions === null) {
      for (const s of rows) byKey.set(`${s.source}/${s.id}`, s);
      store.publish({ sessions: sortSessions(byKey) });
    }
  });
  const publishSessions = () => {
    const rows = sortSessions(byKey);
    store.publish({ sessions: rows });
    saveSessionList(machine, rows);
  };

  const feedHub = feedHubFor(machine);
  // Plugins live on the home machine (`lib/machines/registry.ts`): their
  // documents' news is only ever its feed's.
  const plugins = machine === homeMachine() ? pluginEvents : null;
  feedHub.start({
    onOpen: () => store.publish({ connected: true }),
    onClose: () => store.publish({ connected: false }),
    onFrame: (m) => {
      if (m.type === "settings") {
        const info = m as unknown as ServerInfo;
        // The frame rather than the socket opening, because the frame carries
        // `serverStartedAt` and the open event carries nothing: a reconnect
        // to the same instance — a phone changing networks, a laptop waking —
        // must not throw away a ticket that still works and re-fetch every
        // image on the page with the replacement.
        // Only the server the page's tickets are from: another machine
        // restarting leaves them as good as they were.
        if (serverRestarted(serverBoot, info.serverStartedAt) && holdsPageLogin(machine)) invalidateDownloadTicket();
        serverBoot = info.serverStartedAt ?? "";
        store.publish({ settings: info });
      } else if (m.type === "sessions:snapshot") {
        live = true;
        byKey.clear();
        for (const s of (m.sessions as SessionSummary[]) ?? []) {
          byKey.set(`${s.source}/${s.id}`, s);
        }
        publishSessions();
        // A snapshot opens every connection and replaces the deltas a client
        // fell too far behind to receive — and the screens' news travels the
        // same socket, so whatever it said meanwhile is re-read, not trusted.
        store.emitScreens({ type: "screens:resync" });
        // Plugins' documents too: a watcher re-reads what it watches.
        plugins?.resync();
      } else if (m.type === "sessions:delta") {
        for (const s of (m.upserts as SessionSummary[]) ?? []) {
          byKey.set(`${s.source}/${s.id}`, s);
        }
        for (const key of (m.removed as string[]) ?? []) byKey.delete(key);
        publishSessions();
      } else if (!plugins?.takeFrame(m)) {
        const screens = screensFeedEvent(m);
        if (screens) store.emitScreens(screens);
      }
    },
  });
  // A phone that changed networks, or a laptop that woke, is back the moment
  // the browser says so, not at the end of a backoff that grew while it was
  // gone.
  const kick = () => feedHub.kick();
  const onVisible = () => {
    if (document.visibilityState === "visible") kick();
  };
  window.addEventListener("online", kick);
  document.addEventListener("visibilitychange", onVisible);

  return () => {
    window.removeEventListener("online", kick);
    document.removeEventListener("visibilitychange", onVisible);
    feedHub.stop();
  };
}

/**
 * The feed of the machine this part of the tree is about (`useMachine()`):
 * that machine's {@link MachineFeed}, shown for as long as this is mounted.
 * The context value is the machine's store, stable while the machine is the
 * same, so this component does not re-render on the feed; a provider whose
 * machine changes under it hands down that machine's store, never the first
 * machine's rows under the new machine's name.
 */
export function GlobalFeedProvider({ children }: { children: React.ReactNode }) {
  const machine = useMachine();
  const feed = machineFeedFor(machine);
  // Not for a machine gone between the commit and this effect: its transport
  // is gone with it, and the view re-renders without it.
  useEffect(() => (isOpen(machine) ? feed.acquire() : undefined), [feed, machine]);
  return <GlobalFeedContext.Provider value={feed.store}>{children}</GlobalFeedContext.Provider>;
}
