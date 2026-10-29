import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { AcpAgentChip, SourceIcon } from "@/components/source-icon";
import { useParams } from "@tanstack/react-router";
import {
  ArrowDown,
  Bot,
  ChevronLeft,
  GitBranch,
  Loader2,
  Square,
  X,
} from "lucide-react";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import { LiveDot } from "@/components/live-dot";
import { DiagramRendering, drawsDiagrams, MathRendering } from "@/components/markdown";
import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import { SessionHeader } from "@/features/sessions/session-header";
import { cloudParts } from "@/app/cloud-parts";
import { SessionActionsMenu } from "@/features/sessions/session-actions-menu";
import { ModelDriftBadge } from "@/features/sessions/composer-controls";
import { scrollTopAfterPrepend } from "@/features/sessions/session-feed-state";
import type { FeedState } from "@/features/sessions/session-feed-pool";
import {
  NO_UNREAD,
  pinAfterScroll,
  pinOnSettle,
  shouldTopUpHistory,
  showJumpToBottom,
  trackUnread,
  unreadLabel,
  type ScrollBox,
  type UnreadCursor,
} from "@/features/sessions/transcript-scroll";
import {
  activityBanner,
  backgroundPanel,
  liveSubtasks,
  residentAbsenceIsFinal,
  residentOf,
  runningTaskIds,
  type LiveSubtask,
} from "@/features/sessions/session-activity";
import { BackgroundTaskPanel } from "@/features/sessions/background-panel";
import { questionsByToolUse } from "@/features/sessions/session-questions";
import { usePendingDeliveries } from "@/features/sessions/delivery-preview-view";
import { SessionAccountBadge } from "@/features/sessions/session-account-badge";
import type { OpenSession } from "@/features/sessions/continue-on-account";
import { StatusBadge } from "@/features/sessions/sessions-layout";
import { RenameSessionDialog } from "@/features/sessions/rename-dialog";
import {
  SearchButton,
  SessionSearchBar,
  useFindShortcut,
  useSessionSearch,
} from "@/features/sessions/session-search-view";
import { SessionPinButton } from "@/features/sessions/session-pin";
import { useReportSeen } from "@/features/sessions/use-seen";
import { FirstUnreadButton, useFirstUnread } from "@/features/sessions/first-unread-view";
import { PaneUnreadPill, usePaneSeat } from "@/app/pane-seat";
import {
  bubbleRuns,
  chunkItems,
  firstTimestamp,
  pairToolResults,
  type Chunk,
} from "@/features/sessions/transcript-groups";
import { useFeedSessionTitle } from "@/app/global-feed";
import { MachineLink, useMachineNavigate } from "@/app/machine-route";
import { cn } from "@/lib/cn";
import { useApi, useEnsureLive, useInfoOf, useMachine, useSessionRef } from "@/lib/machines/context";
import { DeviceChip } from "@/features/machines/device-picker";
import { sessionKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";
import { Composer } from "@/features/sessions/composer";
import { SessionMetaChip, sessionMetaFacts } from "@/features/sessions/session-meta";
import { type Source, useSessionFeed } from "@/features/sessions/use-session-feed";
import { BubbleRun } from "@/features/sessions/bubble-run";
import {
  LiveContext,
  PairingContext,
  type QuestionIndex,
  QuestionsContext,
  RunningTasksContext,
  type SubagentCtx,
  type TranscriptStyle,
} from "@/features/sessions/transcript-context";
import { ItemBlock, StepGroup } from "@/features/sessions/transcript-rows";

export function SessionDetail() {
  // The home's `/sessions/$source/$id` or another machine's (`/m/$machine/…`): the same page.
  const { source, id } = useParams({ strict: false }) as { source: string; id: string };
  const navigate = useMachineNavigate();
  const machine = useMachine();
  return (
    <SessionDetailInner
      key={sessionKey({ machine, source, id })}
      source={source}
      id={id}
      onArchived={() => void navigate({ to: "/sessions" })}
      onOpenSession={(next, nextId) => void navigate({ to: "/sessions/$source/$id", params: { source: next, id: nextId } })}
    />
  );
}

/**
 * The way back from a subagent's thread to the session that delegated it
 * (`summary.parentId`): its title as the feed has it, or its id while the feed
 * does not carry it — the parent of a nested subagent is itself not listed.
 */
function ParentSessionLink({ source, id }: { source: Source; id: string }) {
  const { t } = useTranslation();
  const machine = useMachine();
  const ref = useMemo(() => ({ machine, source, id }), [machine, source, id]);
  const title = useFeedSessionTitle(ref) ?? id.slice(0, 8);
  return (
    <span data-parent-session={id} className="flex min-w-0 shrink items-center gap-1">
      <Bot aria-hidden="true" className="size-3 shrink-0" />
      <span className="shrink-0">{t("subagent of")}</span>
      <MachineLink to="/sessions/$source/$id" params={{ source, id }} className="min-w-0 truncate text-fg hover:underline" title={title}>
        {title}
      </MachineLink>
    </span>
  );
}

/**
 * Separator for the running-task key below.
 *
 * A `tool_use` id is `toolu_` and base62, so any punctuation would do; a
 * character that cannot occur in one at all is the version that stays correct
 * if claude ever changes the shape.
 */
const TASK_ID_SEP = "\u0000";

/** The empty list handed out when a departed process took its children with it. */
const NO_SUBTASKS: LiveSubtask[] = [];

/** The three numbers the scroll decisions are made from, off a live element. */
function boxOf(el: HTMLElement): ScrollBox {
  return {
    scrollTop: el.scrollTop,
    scrollHeight: el.scrollHeight,
    clientHeight: el.clientHeight,
  };
}

/**
 * What the transcript's row list is rebuilt for.
 *
 * The dependency array of the `rows` memo below, as a value rather than as a
 * literal, because this is the half of that memo which can be wrong: a memo
 * over a list of elements is only worth having if the things it is keyed on
 * hold still between frames, and an input the builder reads but this does not
 * list is a stale row on screen. Written out here so `web/test` can hold both
 * ends of a `state` frame against it and assert that every entry is the same
 * object afterwards — there is no DOM renderer in this project, so a
 * component's *second* render is not otherwise observable from a test.
 *
 * Everything listed is something a frame changes and nothing is something a
 * render creates. `chunks` is memoized on the same `items` array this reads,
 * and `subagentSession` on the route.
 *
 * What is *not* listed matters as much. `canSend` and `sendBlockReason` used to
 * be, for a question card that answered through the composer; they flip at the
 * start and end of every turn, and each flip rebuilt every row in the window.
 * A question card now answers into the turn itself and finds its record
 * through [`QuestionsContext`], which reaches the one card it concerns without
 * touching this memo at all.
 */
export function transcriptRowDeps(
  chunks: Chunk[],
  transcriptStyle: TranscriptStyle,
  data: Pick<FeedState, "items" | "startIndex"> | null,
  subagentSession: SubagentCtx | undefined,
): readonly unknown[] {
  return [chunks, transcriptStyle, data?.items, data?.startIndex, subagentSession];
}

export function SessionDetailInner({
  source,
  id,
  chat = false,
  onArchived,
  paneActions,
  onOpenSession,
}: {
  source: string;
  id: string;
  /** Instant-chat presentation: back goes to /chat, cwd and permission hidden. */
  chat?: boolean;
  /** Called after a successful archive; omitting it hides the archive action. */
  onArchived?: () => void;
  /** Split-pane controls share the title row, including during loading/errors. */
  paneActions?: ReactNode;
  /**
   * Show another session in this one's place — where a session stuck on the
   * local login continues on an account. Omitting it hides that action.
   */
  onOpenSession?: OpenSession;
}) {
  const { t } = useTranslation();
  const api = useApi();
  const machine = useMachine();
  const session = useSessionRef(source, id);
  const CopyWindowNotice = cloudParts()?.CopyWindowNotice;
  // Another device's session in the local app, from its copy (S1-7): read-only here.
  const readOnly = useInfoOf(machine).readOnly === true;
  // A transcript open is the machine itself in use: live, not only its copy.
  useEnsureLive();
  const scrollRef = useRef<HTMLDivElement>(null);
  // The one box that holds every row, and the only thing whose height changing
  // means "the transcript got taller" — the panel and the load-earlier control
  // are siblings of it inside the same scroller.
  const rowsRef = useRef<HTMLDivElement>(null);
  const prependAnchor = useRef<{
    startIndex: number;
    scrollHeight: number;
  } | null>(null);
  // Stick to the newest messages until the user scrolls up to read history.
  const stickToBottom = useRef(true);
  // Where the last scroll event left the view. `pinAfterScroll` compares
  // against it to tell a user scrolling up from the bottom growing downwards.
  const lastScrollTop = useRef(0);
  const unreadCursor = useRef<UnreadCursor>(NO_UNREAD);
  // Which window the automatic top-up below has already been fired for, so a
  // page the reducer refuses cannot be asked for again on the next render.
  // See [`shouldTopUpHistory`], which owns the reasoning.
  const autoLoadedFrom = useRef(-1);
  // Whether a page of history was loading as of the last layout pass, so the
  // pass where it stops can settle the pin.
  const loadingWas = useRef(false);
  // Mirrors of the two refs above for rendering the floating button. Kept as
  // refs *and* state so a socket frame that changes neither costs no render.
  const jumpShownRef = useRef(false);
  const [jumpShown, setJumpShown] = useState(false);
  const [unread, setUnread] = useState(0);

  const { data, drafts, isError, connection, loadEarlier, isLoadingEarlier, historyError, refreshState } =
    useSessionFeed(source, id, stickToBottom);
  // Messages this tab sent that the transcript has not shown yet.
  const pendingSends = usePendingDeliveries(sessionKey(session));
  const isPending = !data && !isError;
  // The global feed carries the override-applied title, so the header updates
  // reactively after a rename without re-fetching the per-session summary.
  //
  // One string, through a selector, rather than the whole session array. This
  // component is the reason the feed is an external store at all: every mounted
  // transcript used to re-render on every `sessions:delta` from any session on
  // the machine — panes on screens the user had switched away from, sessions
  // idle for hours — because a changed context value re-renders every consumer
  // whatever `memo` says. Selecting a string means the delta wakes this hook,
  // compares two equal strings, and stops there.
  const feedTitle = useFeedSessionTitle(session);
  // Read means *being read*: the pane has the reader's focus (always true
  // outside a split) and the transcript is at its newest message. Scrolled up
  // into history, or in a pane beside the one being typed in, new answers stay
  // unread — and the pane's frame says so — until the reader gets to them.
  const seat = usePaneSeat();
  const [atBottom, setAtBottom] = useState(true);
  const atBottomRef = useRef(true);
  useEffect(() => {
    seat?.reportAtBottom(atBottom);
  }, [seat, atBottom]);
  // A failed archive used to raise `window.alert`, which is a modal the user has
  // to dismiss before they can even read the transcript behind it — and on iOS
  // standalone it is the one piece of chrome that says "web page". The page has
  // notice strips already; this is one of them.
  const [actionError, setActionError] = useState<string | null>(null);
  // Which rows are waiting on a stop, keyed by the CLI's `task_id`.
  //
  // A set rather than the mutation's own `isPending` because several children
  // can be stopped in a row and only the ones asked about should spin. Cleared
  // by the task leaving the resident's list, which is also what removes the row
  // — so an entry that outlives its task is harmless.
  const [stoppingTasks, setStoppingTasks] = useState<ReadonlySet<string>>(new Set());
  const stopTask = useMutation({
    // The summary's own `source`, for the same reason `archive` takes one: this
    // component's prop is a bare string off the route.
    mutationFn: async (target: { source: Source; taskId: string }) => {
      const res = await api.POST("/api/sessions/{source}/{id}/tasks/{taskId}/stop", {
        params: { path: { source: target.source, id, taskId: target.taskId } },
      });
      if (res.error) throw new ApiError(res.error);
    },
    onSettled: (_data, _error, target) => {
      // Off on failure as well as success: a spinner that never stops is a
      // worse report of "that did not work" than the row simply staying put.
      setStoppingTasks((prev) => {
        const next = new Set(prev);
        next.delete(target.taskId);
        return next;
      });
    },
  });

  const archive = useMutation({
    mutationFn: async (archiveSource: Source) => {
      const res = await api.POST("/api/sessions/{source}/{id}/archive", {
        params: { path: { source: archiveSource, id } },
      });
      if (res.error) throw new ApiError(res.error);
    },
  });
  const [renameOpen, setRenameOpen] = useState(false);
  const { data: prefs } = useQuery({
    queryKey: mq(machine, "prefs"),
    queryFn: async () => (await api.GET("/api/prefs")).data,
    // The settings page writes through this cache; a refetch per mounted view
    // was a request on every session switch for a value that had not moved.
    staleTime: 5 * 60_000,
  });
  const transcriptStyle: TranscriptStyle = prefs?.transcriptStyle ?? "flat";
  // Typeset unless asked not to: a server from before the pref sends none.
  const renderMath = prefs?.math !== "source";
  // Drawn unless asked not to, and only once the prefs are in (`drawsDiagrams`).
  const renderDiagrams = drawsDiagrams(prefs);
  // Recomputed only when the window itself changes; item identity is what the
  // renderer consults, so it must come from this exact array.
  const pairing = useMemo(() => pairToolResults(data?.items ?? []), [data?.items]);
  // One scan for the floating panel and the composer strip both, so the two
  // cannot end up with different opinions about which children are still out.
  const scanned = useMemo(() => liveSubtasks(data?.items ?? []), [data?.items]);
  // And neither of them believes a transcript over a process that has gone: a
  // background child belongs to the CLI that launched it, so where this server
  // would be holding that CLI and is not, every child the transcript still
  // shows as unfinished died with it. See [`residentAbsenceIsFinal`], which is
  // careful about when that is knowable at all.
  //
  // The same constant every time rather than a fresh `[]`, so the composer's
  // props do not change identity twice a second for a session with nothing
  // running.
  const subtasks =
    data?.summary && residentAbsenceIsFinal(data.summary) ? NO_SUBTASKS : scanned;
  // Which children the resident is minding, for the ✓/⊘ on each subagent card.
  //
  // Two steps because of what a context costs: a state frame arrives twice a
  // second and rebuilding the set on each one would hand every consumer a new
  // object, which re-renders them all regardless of `memo`. The first memo
  // recomputes freely and yields a *string*; the second only runs when that
  // string changes, which is when a child actually starts or finishes.
  const residentTaskKey = useMemo(
    () => [...runningTaskIds(residentOf(data?.summary ?? { status: "idle" }))].join(TASK_ID_SEP),
    [data?.summary],
  );
  const residentTasks = useMemo<ReadonlySet<string>>(
    () => new Set(residentTaskKey ? residentTaskKey.split(TASK_ID_SEP) : []),
    [residentTaskKey],
  );
  // `status` is a guess derived from file mtimes; `activity` (through the same
  // banner decision the composer draws) is the driver's own answer. The old
  // reading meant that during the exact 37 minutes a session spent working
  // without its status flipping, the newest tool call — "what is it doing right
  // now" — was folded away with the rest.
  //
  // `working` and not merely "there is a banner": the other banner is `queued`,
  // which is reached with the resident at `phase: "idle"` — see `bannerCanStop`,
  // which refuses a Stop for exactly that reason. Everything downstream of this
  // takes it as "a turn is in flight", so counting a queue as one put a spinner
  // on tool calls of a session that had stopped working.
  const live =
    activityBanner(data?.summary ?? { status: "idle" }, data?.injection?.state === "running")?.kind === "working";
  // Stable identity, so the memoized rows below are not handed a fresh object
  // on every frame the socket delivers.
  const subagentSession = useMemo(
    () => (source === "claude" ? ({ source, id } as SubagentCtx) : undefined),
    [source, id],
  );
  // What became of each question the agent asked, for the cards in the
  // transcript. Keyed on the list itself, which `applyStateFrame` holds by
  // identity until a question opens or closes — so a state frame twice a second
  // hands the context the same object, and no card hears about it.
  const questionIndex = useMemo<QuestionIndex>(
    () => ({ session: subagentSession, byToolUse: questionsByToolUse(data?.questions) }),
    [subagentSession, data?.questions],
  );
  // Chunking is pure and depends only on the window, so a state frame — which
  // arrives twice a second and leaves `items` untouched — reuses these objects
  // and every memoized row below bails out instead of reconciling.
  const chunks = useMemo(() => chunkItems(data?.items ?? [], live), [data?.items, live]);

  /**
   * The rows, built once per window instead of once per render.
   *
   * The elements themselves, not just the list: React reconciles a subtree it
   * is handed the *same element objects* for by skipping it entirely, so this
   * memo is what makes a render that did not touch the transcript cost nothing
   * below this point. Without it, every render — a state frame, a resize, a
   * keystroke in the composer — walked the whole window creating an element per
   * chunk and re-running [`ItemBlock`] for each one. `ItemBlock` is
   * deliberately not memoized (it is the context read that lets [`ItemRow`]
   * below it be), so "re-running it" is the real cost, and the window is capped
   * at `LIVE_WINDOW_CAP` rather than at anything a viewport implies, because
   * none of this is virtualized.
   *
   * What it is keyed on is [`transcriptRowDeps`], and that every entry of it is
   * something a *frame* changes rather than something a render creates is the
   * difference between a memo that holds and a memo that is decorative: one
   * callback rebuilt inline in this list would invalidate it on every render
   * and the elements would all be new objects again.
   *
   * The context providers around this in the JSX still reach the rows when
   * their values change, which is what makes bailing out safe: a subtree React
   * skipped is still re-rendered for the consumers of a context that moved, so
   * a new pairing or a task that started reaches the row it belongs to even
   * though the elements did not change.
   */
  const rows = useMemo(() => {
    const startIndex = data?.startIndex ?? 0;
    // Every row carries its absolute index (`data-ti`): the one thing the
    // in-session search needs to find the row a match is drawn in. A wrapper
    // rather than a prop, because a row is a dozen different components.
    const renderChunk = (chunk: Chunk) =>
      chunk.kind === "group" ? (
        <StepGroup
          key={`group:${startIndex + chunk.index}`}
          items={chunk.items}
          base={startIndex + chunk.index}
        />
      ) : (
        <div key={`item:${startIndex + chunk.index}`} data-ti={startIndex + chunk.index}>
          <ItemBlock item={chunk.item} subagentSession={subagentSession} />
        </div>
      );

    if (transcriptStyle !== "bubble") return chunks.map(renderChunk);
    return bubbleRuns(chunks).map((run) => {
      const key = `run:${startIndex + run.chunks[0]!.index}`;
      if (run.role === "system") return run.chunks.map(renderChunk);
      return (
        <BubbleRun key={key} role={run.role} model={run.model} timestamp={firstTimestamp(run.chunks)}>
          {run.chunks.map(renderChunk)}
        </BubbleRun>
      );
    });
  }, transcriptRowDeps(chunks, transcriptStyle, data, subagentSession));

  /** Publish the pin and what arrived while it was off, without idle renders. */
  const publishPin = useCallback((pinned: boolean, box: ScrollBox) => {
    stickToBottom.current = pinned;
    // Mirrored into state only on a change, so the steady stream of pinned
    // publishes while a turn streams in costs no render.
    if (pinned !== atBottomRef.current) {
      atBottomRef.current = pinned;
      setAtBottom(pinned);
    }
    const next = showJumpToBottom(pinned, box);
    if (next !== jumpShownRef.current) {
      jumpShownRef.current = next;
      setJumpShown(next);
    }
    if (pinned && unreadCursor.current.count !== 0) {
      unreadCursor.current = { ...unreadCursor.current, count: 0 };
      setUnread(0);
    }
  }, []);

  /** Put the view back on the newest row and follow it again. */
  const glueToBottom = useCallback(
    (smooth: boolean) => {
      const el = scrollRef.current;
      if (!el) return;
      if (smooth) el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
      else el.scrollTop = el.scrollHeight;
      lastScrollTop.current = el.scrollTop;
      // Pinned from this instant, not when the animation lands: the height can
      // change while it runs, and the observer below has to keep chasing it.
      publishPin(true, { ...boxOf(el), scrollTop: el.scrollHeight });
    },
    [publishPin],
  );

  const beginLoadEarlier = useCallback(
    // `keepPinned` is the automatic top-up below, which runs while the view is
    // still glued to the newest row: a page arriving above must not read as the
    // user walking away from the bottom.
    //
    // Answers the request, or `null` when none went out — which the search's
    // page-in needs to tell "a page is coming" from "nothing will happen".
    (keepPinned = false): Promise<void> | null => {
      const el = scrollRef.current;
      if (!el || !data?.hasMoreBefore || isLoadingEarlier) return null;
      if (!keepPinned) publishPin(false, boxOf(el));
      prependAnchor.current = {
        startIndex: data.startIndex,
        scrollHeight: el.scrollHeight,
      };
      return loadEarlier();
    },
    [data?.hasMoreBefore, data?.startIndex, isLoadingEarlier, loadEarlier, publishPin],
  );

  /**
   * In-session search, over the whole transcript rather than this window. A
   * jump to an older match pages history in through `beginLoadEarlier` — the
   * same anchored request scrolling up makes — so the window stays contiguous
   * and the resume marker is untouched; see `session-search-view.tsx`.
   */
  const unpinForSearch = useCallback(() => {
    const el = scrollRef.current;
    if (el) publishPin(false, boxOf(el));
  }, [publishPin]);
  const search = useSessionSearch({
    source,
    id,
    window: data,
    isLoadingEarlier,
    historyError,
    loadEarlier: beginLoadEarlier,
    scrollRef,
    rowsRef,
    unpin: unpinForSearch,
  });
  const rootRef = useRef<HTMLDivElement>(null);
  useFindShortcut(rootRef, search.openBar, !!data);
  // The way back to where the unread messages begin for a reader arriving at
  // the newest of them. The report their arrival sends is what marks those
  // messages read, so it is also what says where they began (`onArrival`);
  // the jump is the search's.
  const firstUnread = useFirstUnread({
    source,
    id,
    scrollRef,
    rowsRef,
    jumpToItem: search.jumpToItem,
    cancelJump: search.cancelJump,
    itemPaging: search.itemPaging,
  });
  useReportSeen(session, { attentive: seat?.attentive ?? true, atBottom, onArrival: firstUnread.onArrival });

  /** Take the pin back if what took it away left the view at the newest row. */
  const settlePin = useCallback(() => {
    const el = scrollRef.current;
    if (!el || stickToBottom.current) return;
    const box = boxOf(el);
    if (pinOnSettle(false, box)) publishPin(true, box);
  }, [publishPin]);
  // A search that closes: the jump to a match unpinned, and nothing re-pins a
  // view too short to scroll. See `pinOnSettle`.
  const searchWasOpen = useRef(false);
  useEffect(() => {
    if (searchWasOpen.current && !search.open) settlePin();
    searchWasOpen.current = search.open;
  }, [search.open, settlePin]);

  useLayoutEffect(() => {
    stickToBottom.current = true;
    prependAnchor.current = null;
    lastScrollTop.current = 0;
    autoLoadedFrom.current = -1;
    unreadCursor.current = NO_UNREAD;
    jumpShownRef.current = false;
    setJumpShown(false);
    setUnread(0);
  }, [source, id]);

  /**
   * Chase the bottom while the height under it is still settling.
   *
   * A transcript is pinned by assigning `scrollHeight` to `scrollTop`, and the
   * height that lands there is only as good as the moment it was read: an image
   * decodes, a fold opens, a web font swaps, the phone keyboard shortens the
   * box. Each one leaves the view halfway up — and an idle session gets no
   * further frames to re-stick from, because the server only pushes a state
   * frame when the state changed.
   *
   * It never moves a view the user took off the bottom, so prepend anchoring —
   * which only runs while unpinned — cannot collide with it. `pinAfterScroll`
   * is what keeps that line honest: any upward move that is not the browser
   * clamping against shrinking content surrenders the pin on the same frame,
   * before this observer can pull the reader back down.
   */
  // `hasFeed` is the dependency that matters: the transcript is only in the DOM
  // once the first frame has arrived, because the pending branch below renders
  // a spinner and no scroller at all.
  const hasFeed = !!data;
  useEffect(() => {
    const el = scrollRef.current;
    const rows = rowsRef.current;
    if (!hasFeed || !el || !rows) return;
    const observer = new ResizeObserver(() => {
      if (stickToBottom.current) {
        el.scrollTop = el.scrollHeight;
        lastScrollTop.current = el.scrollTop;
      }
      publishPin(stickToBottom.current, boxOf(el));
    });
    // The viewport as well as the content: a phone keyboard opening, or the
    // composer growing a line, shortens the box without touching the rows.
    observer.observe(el);
    observer.observe(rows);
    return () => observer.disconnect();
  }, [hasFeed, publishPin]);

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || !data) return;

    const cursor = trackUnread(unreadCursor.current, data.endIndex, stickToBottom.current);
    if (cursor.count !== unreadCursor.current.count) setUnread(cursor.count);
    unreadCursor.current = cursor;

    const anchor = prependAnchor.current;
    if (stickToBottom.current) {
      prependAnchor.current = null;
      el.scrollTop = el.scrollHeight;
      lastScrollTop.current = el.scrollTop;
    } else if (anchor) {
      if (data.startIndex < anchor.startIndex) {
        el.scrollTop = scrollTopAfterPrepend(
          el.scrollTop,
          anchor.scrollHeight,
          el.scrollHeight,
        );
        prependAnchor.current = null;
      } else {
        // Ignore live tail growth while an earlier-page request is in flight.
        anchor.scrollHeight = el.scrollHeight;
      }
      lastScrollTop.current = el.scrollTop;
    }
    publishPin(stickToBottom.current, boxOf(el));
    // A page of history that has settled in — or a request that came back with
    // nothing — leaves the view where the anchor put it; if that is the bottom
    // (a short transcript), it is pinned again. See `pinOnSettle`.
    if (loadingWas.current && !isLoadingEarlier) settlePin();
    loadingWas.current = isLoadingEarlier;

    if (
      shouldTopUpHistory({
        hasMoreBefore: data.hasMoreBefore,
        loading: isLoadingEarlier,
        failed: historyError !== null,
        startIndex: data.startIndex,
        alreadyLoadedFrom: autoLoadedFrom.current,
        loadedItems: data.items.length,
        box: boxOf(el),
      })
    ) {
      autoLoadedFrom.current = data.startIndex;
      beginLoadEarlier(true);
    }
    // A transcript too short to scroll never fires a scroll event, and its
    // first unread message may be on screen from the start.
    firstUnread.check();
  }, [source, id, data, isLoadingEarlier, historyError, beginLoadEarlier, publishPin, settlePin, firstUnread.check]);

  if (isPending || isError || !data) {
    return (
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {paneActions && (
          <SessionHeader
            titleRow={<span className="min-w-0 flex-1 truncate text-xs font-medium">{feedTitle ?? id.slice(0, 8)}</span>}
            paneActions={paneActions}
          />
        )}
        {isPending ? <Spinner className="flex-1" /> : (
          <div data-signal="failed" className="flex flex-1 items-center justify-center font-mono text-xs text-danger">
            {t("session not found")}
          </div>
        )}
      </div>
    );
  }

  const { summary, items } = data;
  const displayTitle = feedTitle ?? summary.title;
  return (
    <div
      ref={rootRef}
      // Esc anywhere in the view closes an open search, except in a text field,
      // where it belongs to the field (the search box handles its own).
      onKeyDown={(e) => {
        if (!search.open || e.key !== "Escape" || e.defaultPrevented || e.nativeEvent.isComposing) return;
        const target = e.target as HTMLElement;
        if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable) return;
        search.close();
      }}
      className="flex min-h-0 flex-1 flex-col"
    >
      <SessionHeader
        paneActions={paneActions}
        titleRow={<>
          {!paneActions && <MachineLink
            to={chat ? "/chat" : "/sessions"}
            title={t("back")}
            className="flex size-9 shrink-0 items-center justify-center text-muted hover:bg-hover md:hidden"
          >
            <ChevronLeft className="size-4" />
          </MachineLink>}
          <span className="min-w-0 flex-1 truncate text-[13px] font-semibold max-md:text-[15px]" title={displayTitle}>{displayTitle}</span>
          <PaneUnreadPill />
          <StatusBadge session={summary} />
          {!chat && !readOnly && summary.scope === "owned" && <SessionPinButton session={summary} onError={setActionError} />}
          <SearchButton onClick={search.openBar} />
          {!chat && !readOnly && (
            <SessionActionsMenu
              onRename={() => setRenameOpen(true)}
              archiving={archive.isPending}
              onArchive={onArchived ? () => archive.mutate(summary.source, {
                onSuccess: () => onArchived(),
                onError: (err) => setActionError(t("archive failed: {{msg}}", { msg: errorText(err) })),
              }) : undefined}
            />
          )}
        </>}
        metadata={<>
          {readOnly && !paneActions && <DeviceChip machine={machine} />}
          {chat ? (
            // An instant chat is Claude's or Codex's; the ACP agent has none.
            <Badge variant={summary.source === "acp" ? "default" : summary.source} className="shrink-0">
              {t("chat")}
            </Badge>
          ) : summary.source === "acp" ? (
            // Its agent's mark and name: the session's own, whatever else is configured.
            <AcpAgentChip agent={summary.acpAgent} />
          ) : (
            <SourceIcon source={summary.source} className="text-muted" />
          )}
          {summary.parentId && <ParentSessionLink source={summary.source} id={summary.parentId} />}
          {/* An ACP agent runs on its own configuration: there is no account to show or bind. */}
          {!readOnly && summary.source !== "acp" && (
            <SessionAccountBadge
              source={summary.source}
              id={id}
              claudeRoot={data.claudeRoot}
              account={data.account}
              scope={summary.scope}
              model={data.cliChoice?.model ?? null}
              rotation={data.rotation}
              onOpenSession={onOpenSession}
            />
          )}
          {summary.gitBranch && (
            <span className="flex min-w-0 items-center gap-1 font-mono text-2xs text-muted" title={summary.gitBranch}>
              <GitBranch aria-hidden="true" className="size-3 shrink-0" />
              <span className="truncate">{summary.gitBranch}</span>
            </span>
          )}
          <ModelDriftBadge source={summary.source} id={id} cliChoice={data.cliChoice} />
          <SessionMetaChip
            facts={sessionMetaFacts({
              // Chat sessions never showed their working directory and still
              // do not: the instant-chat presentation has no project to speak of.
              cwd: chat ? null : summary.cwd,
              updatedAt: summary.updatedAt,
              sizeBytes: summary.sizeBytes,
              totalItems: data.totalItems,
              omittedItems: data.omittedItems,
            })}
          />
        </>}
      />
      <RenameSessionDialog
        target={renameOpen ? { source: summary.source, id, title: displayTitle } : null}
        onOpenChange={(o) => !o && setRenameOpen(false)}
      />
      {/* The transcript below stays exactly as it was while this is up. The
          server broadcasts `Closed` to every feed when it restarts to install
          its own update, and blanking the page on it meant a release wiped
          every open session in every open tab. */}
      {connection !== "live" && (
        <div
          className={cn(
            "flex shrink-0 items-center gap-1.5 border-b border-border px-4 py-1 font-mono text-2xs",
            connection === "gone"
              ? "text-danger shadow-[inset_2px_0_0_var(--color-danger)]"
              : "text-warn-fg shadow-[inset_2px_0_0_var(--color-warn)]",
          )}
          data-signal={connection === "gone" ? "offline" : "reconnecting"}
        >
          {connection === "reconnecting" ? (
            <>
              {/* Amber from the wrapper. A backoff that has given up on a
                  restarting server can sit here for minutes, so this is a
                  long-lived indicator too, not a submit spinner. */}
              <LiveDot />
              {t("connection lost — reconnecting…")}
            </>
          ) : connection === "cached" ? (
            // Drawn from the copy this device kept, before the server has
            // answered: honest about it, and gone with the first frame.
            <>
              <LiveDot />
              {t("showing the copy saved on this device — updating…")}
            </>
          ) : (
            // No retry button: the socket keeps going back on its own, and the
            // only "reconnect now" this component has is `refresh`, which
            // re-attaches by clearing the window first — which would throw away
            // the copy this banner exists to preserve.
            <span className="min-w-0 flex-1">
              {t("this session is no longer available on the server — what is below is the last copy loaded")}
            </span>
          )}
        </div>
      )}
      {/* Not in the floating panel, which is about work that is running *now*
          and is gone by the time you go looking for what a session actually
          did. This is history, so it lives with the rest of the session's
          chrome and is one folded line until it is asked for. */}
      {actionError && (
        <div data-signal="failed" className="flex shrink-0 items-start gap-1.5 border-b border-border px-4 py-1 font-mono text-2xs text-danger shadow-[inset_2px_0_0_var(--color-danger)]">
          <span className="min-w-0 flex-1 break-words">{actionError}</span>
          <button
            type="button"
            aria-label={t("dismiss")}
            onClick={() => setActionError(null)} data-signal="failed" className="flex size-4 shrink-0 items-center justify-center rounded hover:bg-danger/15"
          >
            <X className="size-3" />
          </button>
        </div>
      )}
      <SessionSearchBar search={search} />
      {/* The positioning context for the jump-to-bottom button: it floats over
          the transcript and must stay clear of the composer, so it hangs off
          the scroller's box rather than scrolling with the rows inside it. */}
      <div className="relative flex min-h-0 flex-1 flex-col">
      <div
        ref={scrollRef}
        onScroll={() => {
          const el = scrollRef.current;
          if (!el) return;
          const box = boxOf(el);
          const pinned = pinAfterScroll(stickToBottom.current, box, lastScrollTop.current);
          lastScrollTop.current = box.scrollTop;
          publishPin(pinned, box);
          // Only while the view is off the bottom, which is the only way the
          // top can be what the user is heading for. A pinned view passing
          // through the same coordinates — the jump button animating down from
          // the top of a short window, a stick that landed under 120px — is
          // going the other way, and used to load a page and unpin for it.
          if (!pinned && el.scrollTop < 120) beginLoadEarlier();
          // Scrolling up to the first unread message by hand retires its button.
          firstUnread.check();
        }}
        // `transcript-scroll` turns the browser's own scroll anchoring off:
        // the prepend anchor below is the only thing allowed to compensate for
        // history arriving above the viewport, and two of them double-count.
        // `overflow-x-hidden` is load-bearing, not tidiness. CSS has no state
        // for "scrolls on one axis, visible on the other": the `visible` side
        // computes to `auto`, so `overflow-y-auto` on its own also made this a
        // horizontal scroller, and any one row too wide out of a thousand — a
        // forty-six character MCP tool name, a thumbnail in a narrow split —
        // set the whole stream sliding sideways under a vertical drag. Every
        // row below is built to give instead (truncate, wrap, or scroll inside
        // its own box), so nothing should reach this clip. It is here so the
        // next row that forgets loses its own right edge rather than the
        // transcript's vertical scroll.
        className="transcript-scroll min-h-0 flex-1 overflow-x-hidden overflow-y-auto"
      >
        {/* First child of the scroll container, so `sticky top-0` pins it to
            the top of the transcript wherever the history is scrolled to —
            which is the point: a session minding background work writes
            nothing, so scrolling up to read while you wait would otherwise
            take away the only evidence that anything is still happening. */}
        <BackgroundTaskPanel
          panel={backgroundPanel({
            row: summary,
            subtasks,
            injectionRunning: data.injection?.state === "running",
          })}
          stopping={stoppingTasks}
          onStop={(subtask) => {
            const taskId = subtask.taskId;
            if (taskId === undefined) return;
            // Asked before it is done, because it cannot be undone: a stopped
            // subagent's work is gone, and a stopped shell leaves whatever it
            // had half-written on disk. The question names the child rather
            // than the count, so a mis-tap on a phone is caught by reading it.
            if (!window.confirm(t("Stop the background task \"{{name}}\"? It cannot be resumed.", { name: subtask.text }))) {
              return;
            }
            setStoppingTasks((prev) => new Set(prev).add(taskId));
            stopTask.mutate({ source: summary.source, taskId });
          }}
        />
        {/* The top of an offline copy the plan's retention trimmed: the rest is on the computer (cloud mode only). */}
        {!data.hasMoreBefore && CopyWindowNotice && <CopyWindowNotice source={summary.source} id={id} />}
        {data.hasMoreBefore && (
          <>
            <button
              type="button"
              onClick={() => beginLoadEarlier()}
              disabled={isLoadingEarlier}
              className="flex h-7 w-full items-center justify-center gap-1.5 border-b border-border font-mono text-2xs text-faint hover:bg-hover hover:text-muted disabled:pointer-events-none"
            >
              {isLoadingEarlier && <Loader2 className="size-3 animate-spin" />}
              {t(
                isLoadingEarlier
                  ? "loading earlier history…"
                  : historyError
                    ? "retry loading earlier history"
                    : "load earlier history",
              )}
            </button>
            {/* Used to be a `title` on the button, which is a hover — so on the
                phones this app is mostly used from, the reason a page would not
                load was unreachable. */}
            {historyError && (
              <div data-signal="failed" className="border-b border-border px-3 py-1 font-mono text-2xs break-words text-danger">
                {historyError}
              </div>
            )}
          </>
        )}
        <PairingContext.Provider value={pairing}>
        <RunningTasksContext.Provider value={residentTasks}>
        <QuestionsContext.Provider value={questionIndex}>
        <LiveContext.Provider value={live}>
        <MathRendering.Provider value={renderMath}>
        <DiagramRendering.Provider value={renderDiagrams}>
        {items.length === 0 && (pendingSends > 0 ? (
          // A first message is on its way in (a session just started, its
          // message pending below): it is starting, not empty.
          <div data-starting="" className="flex items-center justify-center gap-1.5 px-3 py-10 text-center font-mono text-2xs text-faint">
            <span data-signal="working" className="flex shrink-0 text-ok">
              <LiveDot />
            </span>
            <span>{t("starting the session…")}</span>
          </div>
        ) : (
          <div className="flex flex-col items-center gap-1 px-3 py-10 text-center font-mono text-2xs text-faint">
            <span>{t("nothing in this transcript yet")}</span>
            <span className="text-faint/70">{t("send a message below to start the session")}</span>
          </div>
        ))}
        {/* One box around every row, so the observer below can tell "the
            transcript grew" from "the panel above it did". Deliberately not a
            virtual list — dynamic heights, stick-to-bottom and prepend
            anchoring all fight it, and it would take the rows out of the DOM,
            so Ctrl+F would stop finding them. `content-visibility` was the
            cheap half of that trade and it is gone too: see `index.css`.
            What keeps the bill for that finite is `trimLiveWindow`: every row
            here stays laid out, so the number of them has to be bounded by
            something, and it is the window rather than the renderer.
            And the last thing in the scroller: nothing provisional goes below
            it. What the agent is doing and has not written yet — a think in
            flight — is said in the activity bar above the composer, so the
            transcript only grows by written entries and nothing in it changes
            height under the reader while a turn runs (`turn-status.tsx`). */}
        <div ref={rowsRef} className="transcript-rows">{rows}</div>
        </DiagramRendering.Provider>
        </MathRendering.Provider>
        </LiveContext.Provider>
        </QuestionsContext.Provider>
        </RunningTasksContext.Provider>
        </PairingContext.Provider>
      </div>
      {/* Always rendered so it can fade both ways, and `inert` while hidden so
          a button nobody can see is not in the tab order or read out. It only
          appears once the newest row is properly off screen, so it never sits
          over the message it would take you to. */}
      <button
        type="button"
        inert={!jumpShown}
        aria-label={t("jump to the newest messages")}
        title={t("jump to the newest messages")}
        onClick={() => glueToBottom(true)}
        className={cn(
          "absolute right-3 bottom-3 z-30 flex h-8 min-w-8 items-center justify-center gap-1 border border-border-strong bg-bg px-2 text-muted transition-opacity hover:bg-hover hover:text-fg",
          jumpShown ? "opacity-100" : "pointer-events-none opacity-0",
        )}
      >
        <ArrowDown className="size-4 shrink-0" />
        {/* What arrived while you were reading history — the reason to press it. */}
        {unread > 0 && (
          <span className="pr-0.5 font-mono text-2xs text-accent">{unreadLabel(unread)}</span>
        )}
      </button>
      {/* Where this visit's unread messages begin, for a reader who arrived at
          the newest of them. Centred, and narrow enough never to reach the
          jump-to-bottom button in the corner. */}
      <FirstUnreadButton
        view={firstUnread.view}
        paging={firstUnread.paging}
        onJump={firstUnread.jump}
        onCancel={firstUnread.cancel}
        className="absolute bottom-3 left-1/2 z-30 max-w-[calc(100%-10rem)] -translate-x-1/2"
      />
      </div>
      <Composer
        key={`${source}/${id}`}
        source={source as Source}
        id={id}
        detail={data}
        drafts={drafts}
        subtasks={subtasks}
        chat={chat}
        onQueueChanged={refreshState}
        // Another device shown read-only here takes no action, this one included.
        onOpenSession={readOnly ? undefined : onOpenSession}
      />
    </div>
  );
}
