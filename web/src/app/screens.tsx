import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useRouterState } from "@tanstack/react-router";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { components } from "@/api/schema";
import { useGlobalFeedStore } from "./global-feed";
import { namesMachines, paneTreeApi, reducePaneTree, type PaneTreeApi, type PaneTreeState } from "./pane-tree";
import {
  applyLocalEdit,
  classifySave,
  dropScreen,
  EMPTY_COPY,
  hasPendingEdit,
  isRetrying,
  isUnsaved,
  metadataLanded,
  needsRefetch,
  reconcileScreens,
  resolveConflict,
  retryDelay,
  saveError,
  seedScreen,
  withSync,
  type ScreensWorkingCopy,
  type SentLayout,
} from "./screen-sync";
import { authHeader } from "@/lib/auth";
import { useApi, useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";
import { transportFor } from "@/lib/machines/registry";

export type ScreenInfo = components["schemas"]["Screen"];
type ScreenList = components["schemas"]["ScreenList"];

/** The screens are the home machine's (`lib/machines/registry.ts`), and so is their cache entry. */
const screensKey = (machine: MachineKey) => mq(machine, "screens");

interface ScreensApi {
  screens: ScreenInfo[] | undefined;
  activeScreenId: string | null;
  /** Null until the screen's layout has been seeded from the server list. */
  treeApiFor: (id: string) => PaneTreeApi | null;
  /** Whether the screen's last layout change has not reached the server: retrying, or refused. */
  isScreenUnsaved: (id: string) => boolean;
  /** Why the server refused the screen's last layout change, when it will not be retried. */
  screenSaveError: (id: string) => string | null;
  /** When the screen was last replaced by another device's newer copy, while it showed. */
  conflictNoticeAt: (id: string) => number | null;
  dismissConflictNotice: (id: string) => void;
  /**
   * Whether the device keeping the screen dropped its panes' other devices on
   * the last save: a build from before panes named their device
   * (`crates/droppen-server/src/screens`), which kept only what it knew.
   */
  machinesDropped: (id: string) => boolean;
  dismissMachinesDropped: (id: string) => void;
  createScreen: () => void;
  creating: boolean;
  renameScreen: (id: string, title: string) => Promise<void>;
  setScreenEmoji: (id: string, emoji: string) => Promise<void>;
  removeScreen: (id: string) => void;
}

const ScreensContext = createContext<ScreensApi | null>(null);

export function useScreens(): ScreensApi {
  const apiValue = useContext(ScreensContext);
  if (!apiValue) throw new Error("useScreens outside ScreensProvider");
  return apiValue;
}

const PERSIST_DELAY_MS = 600;
/** How long the "changed on another device" notice stays up on its own. */
const NOTICE_MS = 8_000;

/**
 * Screens are persistent pane layouts stored server-side, and may be open on
 * several devices at once. Each browser keeps a working copy of every layout:
 * edits apply locally first and are debounced into PATCHes that name the
 * revision they were made against, so a save based on a layout another device
 * has since replaced is refused rather than written over it. The rules — when
 * a newer server copy is adopted, what a refusal does, how a failed save is
 * retried — live in `screen-sync.ts`; this provider holds the state and does
 * the I/O.
 *
 * The list refreshes when the global feed says a screen changed, and on a
 * 15-second poll as the fallback for a feed that is down.
 */
export function ScreensProvider({ children }: { children: React.ReactNode }) {
  const machine = useMachine();
  const api = useApi();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const feed = useGlobalFeedStore();
  const activeScreenId = useRouterState({
    select: (s) => /^\/screens\/([\w-]+)/.exec(s.location.pathname)?.[1] ?? null,
  });

  const { data: list } = useQuery({
    queryKey: screensKey(machine),
    queryFn: async ({ signal }) => {
      const res = await api.GET("/api/screens", { signal });
      // Thrown rather than read as an empty list: an empty list would close
      // every open screen, and a failed fetch keeps the last good one instead.
      if (!res.data) throw new Error("failed to list screens");
      return res.data;
    },
    // The fallback: the feed says when something changed, but not while it is down.
    refetchInterval: 15_000,
  });

  /**
   * The working copy, mirrored in a ref so that the save machinery — timers
   * and requests that outlive any one render — always reads the newest value.
   */
  const copyRef = useRef<ScreensWorkingCopy>(EMPTY_COPY);
  const [copy, setCopy] = useState<ScreensWorkingCopy>(EMPTY_COPY);
  const commit = useCallback((next: ScreensWorkingCopy) => {
    if (next === copyRef.current) return;
    copyRef.current = next;
    setCopy(next);
  }, []);

  const [notices, setNotices] = useState<Record<string, number>>({});
  const raiseNotice = useCallback((id: string) => {
    const at = Date.now();
    setNotices((prev) => ({ ...prev, [id]: at }));
    setTimeout(() => {
      setNotices((prev) => {
        if (prev[id] !== at) return prev;
        const next = { ...prev };
        delete next[id];
        return next;
      });
    }, NOTICE_MS);
  }, []);
  const dismissConflictNotice = useCallback((id: string) => {
    setNotices((prev) => {
      if (!(id in prev)) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
  }, []);

  const [dropped, setDropped] = useState<Record<string, true>>({});
  const noteDropped = useCallback((id: string) => setDropped((prev) => (prev[id] ? prev : { ...prev, [id]: true })), []);
  const dismissMachinesDropped = useCallback((id: string) => {
    setDropped((prev) => {
      if (!(id in prev)) return prev;
      const next = { ...prev };
      delete next[id];
      return next;
    });
  }, []);
  const machinesDropped = useCallback((id: string) => dropped[id] === true, [dropped]);

  /** The newest list revision seen, so a slow response cannot roll the list back. */
  const listRevisionRef = useRef(0);
  useEffect(() => {
    if (!list || list.listRevision < listRevisionRef.current) return;
    listRevisionRef.current = list.listRevision;
    commit(reconcileScreens(copyRef.current, list.screens));
  }, [list, commit]);

  /** One server screen into the cached list, unless the list already holds a newer one. */
  const putInCache = useCallback(
    (screen: ScreenInfo) => {
      queryClient.setQueryData<ScreenList>(screensKey(machine), (old) =>
        old
          ? { ...old, screens: old.screens.map((s) => (s.id === screen.id && screen.revision >= s.revision ? screen : s)) }
          : old,
      );
    },
    [queryClient, machine],
  );

  const engine = useMemo(() => {
    const timers = new Map<string, ReturnType<typeof setTimeout>>();
    const chains = new Map<string, Promise<unknown>>();

    /** Writes to one screen go one at a time, each reading the revision the last one left. */
    function enqueue<T>(id: string, task: () => Promise<T>): Promise<T> {
      const run = (chains.get(id) ?? Promise.resolve()).then(task);
      chains.set(id, run.catch(() => undefined));
      return run;
    }

    function schedule(id: string, delay: number) {
      const timer = timers.get(id);
      if (timer) clearTimeout(timer);
      timers.set(
        id,
        setTimeout(() => {
          timers.delete(id);
          void enqueue(id, () => saveLayout(id));
        }, delay),
      );
    }

    function forget(id: string) {
      const timer = timers.get(id);
      if (timer) clearTimeout(timer);
      timers.delete(id);
      commit(dropScreen(copyRef.current, id));
    }

    /** Send the working tree, based on the revision it was edited against. */
    async function saveLayout(id: string): Promise<void> {
      const before = copyRef.current;
      const sync = before.syncs[id];
      const tree = before.trees[id];
      if (!sync || !tree || !hasPendingEdit(sync)) return;
      const sent: SentLayout = { seq: sync.editSeq, layout: tree.root };
      let status: number | undefined;
      let body: unknown;
      try {
        const res = await api.PATCH("/api/screens/{id}", {
          params: { path: { id } },
          body: { layout: tree.root, baseRevision: sync.revision },
        });
        status = res.response.status;
        body = res.data ?? res.error;
      } catch {
        status = undefined;
      }
      if (!copyRef.current.syncs[id]) return; // deleted while the save was out
      switch (classifySave(status)) {
        case "saved": {
          const screen = body as ScreenInfo;
          // Sent with panes on other devices, saved without: an older build keeps the screen.
          if (namesMachines(sent.layout) && !namesMachines(screen.layout)) noteDropped(id);
          commit(withSync(copyRef.current, id, { type: "saved", revision: screen.revision, seq: sent.seq, layout: screen.layout }));
          putInCache(screen);
          break;
        }
        case "conflict": {
          const screen = (body as components["schemas"]["ScreenConflict"]).screen;
          const resolved = resolveConflict(copyRef.current, screen, sent);
          commit(resolved.copy);
          putInCache(screen);
          if (resolved.notice) raiseNotice(id);
          break;
        }
        case "gone":
          forget(id);
          void queryClient.invalidateQueries({ queryKey: screensKey(machine) });
          return;
        case "retry":
          commit(withSync(copyRef.current, id, { type: "failed", sent }));
          schedule(id, retryDelay(copyRef.current.syncs[id]?.failures ?? 1));
          return;
        case "rejected": {
          const error = (body as { error?: string } | undefined)?.error;
          commit(withSync(copyRef.current, id, { type: "rejected", error: error || `HTTP ${status}` }));
          return;
        }
      }
      // Edits made while this save was out — or kept through a 409 that turned
      // out to be our own lost save, or a rename — go next, on the new base.
      const after = copyRef.current.syncs[id];
      if (after && hasPendingEdit(after)) schedule(id, PERSIST_DELAY_MS);
    }

    /** A layout edit was made: save it after the debounce, unless a retry is already waiting. */
    function edited(id: string) {
      if (isRetrying(copyRef.current.syncs[id]) && timers.has(id)) return;
      schedule(id, PERSIST_DELAY_MS);
    }

    /** Retry every failed save now — the feed just came back, so the server likely did too. */
    function retryFailedNow() {
      for (const [id, sync] of Object.entries(copyRef.current.syncs)) {
        if (isRetrying(sync)) schedule(id, 0);
      }
    }

    /**
     * A rename or icon change. It goes through the same queue as the layout
     * saves, so it never races one. A refusal is resolved like a save's, and
     * then the change is sent once more on top of the server's screen — unless
     * the server already shows it (its answer was lost and it landed anyway).
     */
    function writeMetadata(id: string, patch: { title?: string; emoji?: string }): Promise<void> {
      return enqueue(id, async () => {
        for (let attempt = 0; ; attempt++) {
          const res = await api.PATCH("/api/screens/{id}", {
            params: { path: { id } },
            body: { ...patch, baseRevision: copyRef.current.syncs[id]?.revision ?? 1 },
          });
          if (res.data) {
            commit(withSync(copyRef.current, id, { type: "rebased", revision: res.data.revision, layout: res.data.layout }));
            putInCache(res.data);
            return;
          }
          const error = res.error as { error?: string; screen?: ScreenInfo } | undefined;
          if (res.response.status === 409 && error?.screen && attempt === 0) {
            const resolved = resolveConflict(copyRef.current, error.screen);
            commit(resolved.copy);
            putInCache(error.screen);
            if (resolved.notice) raiseNotice(id);
            const after = copyRef.current.syncs[id];
            if (after && hasPendingEdit(after) && !timers.has(id)) schedule(id, PERSIST_DELAY_MS);
            if (metadataLanded(error.screen, patch)) return;
            continue;
          }
          throw new Error(error?.error ?? "failed to update screen");
        }
      });
    }

    /**
     * The page is going away with edits still waiting: send them with
     * `keepalive`, still based on the revision they were made against, so a
     * stale one is refused rather than written over another device's layout.
     *
     * Nothing is marked saved and no timer is cleared, because nothing will
     * hear the answer: a page that really unloads takes its timers with it,
     * and one restored from the back-forward cache resumes them and saves
     * again — a `409` against its own keepalive save, which
     * `resolveConflict` recognises as landed and tells nobody.
     */
    function flushOnPageHide() {
      const { trees, syncs } = copyRef.current;
      for (const [id, sync] of Object.entries(syncs)) {
        const tree = trees[id];
        if (!tree || !hasPendingEdit(sync)) continue;
        // Through the machine's transport, so in cloud mode it reaches the machine.
        const request = new Request(`/api/screens/${id}`, {
          method: "PATCH",
          // Raw fetch, so the token the api client injects has to go on by hand.
          headers: { "Content-Type": "application/json", ...authHeader() },
          body: JSON.stringify({ layout: tree.root, baseRevision: sync.revision }),
          keepalive: true,
        });
        void transportFor(machine)
          .fetch(request)
          .catch(() => undefined);
        // Remembered, so that if this page lives on and its next save is
        // refused by this one, that refusal reads as the save it is.
        commit(withSync(copyRef.current, id, { type: "unanswered", sent: { seq: sync.editSeq, layout: tree.root } }));
      }
    }

    return { edited, retryFailedNow, writeMetadata, flushOnPageHide, forget };
  }, [api, machine, commit, putInCache, queryClient, raiseNotice, noteDropped]);

  useEffect(() => {
    window.addEventListener("pagehide", engine.flushOnPageHide);
    return () => window.removeEventListener("pagehide", engine.flushOnPageHide);
  }, [engine]);

  // Another device (or another tab) changed something: look again.
  useEffect(
    () =>
      feed.onScreens((event) => {
        if (event.type === "screens:resync") engine.retryFailedNow();
        const known = Math.max(listRevisionRef.current, queryClient.getQueryData<ScreenList>(screensKey(machine))?.listRevision ?? 0);
        if (needsRefetch(event, known || undefined, (id) => copyRef.current.syncs[id]?.revision)) {
          void queryClient.invalidateQueries({ queryKey: screensKey(machine) });
        }
      }),
    [feed, queryClient, engine, machine],
  );

  const updateTree = useCallback(
    (id: string, fn: (state: PaneTreeState) => PaneTreeState) => {
      const { copy: next, edited } = applyLocalEdit(copyRef.current, id, fn);
      commit(next);
      if (edited) engine.edited(id);
    },
    [commit, engine],
  );

  const treeApiFor = useCallback(
    (id: string): PaneTreeApi | null => {
      const state = copy.trees[id];
      if (!state) return null;
      return paneTreeApi(state, (action) => updateTree(id, (s) => reducePaneTree(s, action)));
    },
    [copy.trees, updateTree],
  );

  const isScreenUnsaved = useCallback((id: string) => isUnsaved(copy.syncs[id]), [copy.syncs]);
  const screenSaveError = useCallback((id: string) => saveError(copy.syncs[id]), [copy.syncs]);
  const conflictNoticeAt = useCallback((id: string) => notices[id] ?? null, [notices]);

  const create = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/screens", { body: {} });
      if (!res.data) throw new Error("failed to create screen");
      return res.data;
    },
    onSuccess: (screen) => {
      // Write-through so the rail entry and layout exist before the refetch.
      queryClient.setQueryData<ScreenList>(screensKey(machine), (old) => ({
        listRevision: old?.listRevision ?? 0,
        screens: [...(old?.screens ?? []).filter((s) => s.id !== screen.id), screen],
      }));
      commit(seedScreen(copyRef.current, screen));
      void queryClient.invalidateQueries({ queryKey: screensKey(machine) });
      void navigate({ to: "/screens/$id", params: { id: screen.id } });
    },
  });

  const remove = useMutation({
    mutationFn: async (id: string) => {
      const res = await api.DELETE("/api/screens/{id}", { params: { path: { id } } });
      // Already gone is what was asked for.
      if (res.error && res.response.status !== 404) throw new Error(res.error.error);
      return id;
    },
    onSuccess: (id) => {
      queryClient.setQueryData<ScreenList>(screensKey(machine), (old) =>
        old ? { ...old, screens: old.screens.filter((s) => s.id !== id) } : old,
      );
      engine.forget(id);
      void queryClient.invalidateQueries({ queryKey: screensKey(machine) });
      if (activeScreenId === id) void navigate({ to: "/sessions" });
    },
  });

  const createScreen = useCallback(() => {
    if (!create.isPending) create.mutate();
  }, [create]);
  const renameScreen = useCallback((id: string, title: string) => engine.writeMetadata(id, { title }), [engine]);
  const setScreenEmoji = useCallback((id: string, emoji: string) => engine.writeMetadata(id, { emoji }), [engine]);
  const removeScreen = useCallback((id: string) => remove.mutate(id), [remove]);

  const screens = list?.screens;
  const value = useMemo<ScreensApi>(
    () => ({
      screens,
      activeScreenId,
      treeApiFor,
      isScreenUnsaved,
      screenSaveError,
      conflictNoticeAt,
      dismissConflictNotice,
      machinesDropped,
      dismissMachinesDropped,
      createScreen,
      creating: create.isPending,
      renameScreen,
      setScreenEmoji,
      removeScreen,
    }),
    [
      screens,
      activeScreenId,
      treeApiFor,
      isScreenUnsaved,
      screenSaveError,
      conflictNoticeAt,
      dismissConflictNotice,
      machinesDropped,
      dismissMachinesDropped,
      createScreen,
      create.isPending,
      renameScreen,
      setScreenEmoji,
      removeScreen,
    ],
  );

  return <ScreensContext.Provider value={value}>{children}</ScreensContext.Provider>;
}
