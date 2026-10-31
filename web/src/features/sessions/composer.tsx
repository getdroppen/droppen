/*
 * The session's composer: what is typed, the messages this tab holds back,
 * and sending them.
 */
import { useMutation } from "@tanstack/react-query";
import { useAcpAgentLook } from "@/features/sessions/acp-agent-icons";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText, EXTERNAL_SESSION } from "@/api/errors";
import { useShowingCopy } from "@/features/machines/copy-mode";
import { ChecklistChip, ComposerChoiceChips, ComposerQuota, ContextRing, useComposerChoice } from "@/features/sessions/composer-controls";
import { attachmentPaths, ComposerBox, useComposerAttachments, useSendHint } from "@/features/sessions/composer-box";
import { claimSessionDraft, readSessionDraftSnapshot, sessionDraftKey, subscribeSessionDraft, writeSessionDraft } from "@/features/sessions/session-draft";
import type { FeedState } from "@/features/sessions/session-feed-pool";
import {
  activityBanner,
  backgroundPanel,
  bannerCanStop,
  residentOf,
  willQueueSend,
  workingProgress,
  type LiveSubtask,
} from "@/features/sessions/session-activity";
import { injectionFailurePausesQueue, localQueueHeadAction, shouldDrainQueue } from "@/features/sessions/queue-drain";
import { ServerQueue } from "@/features/sessions/server-queue";
import { RotationNotice } from "@/features/sessions/rotation-notice";
import { AnswerNotice } from "@/features/sessions/question-card";
import { composerAnswersQuestion, sentOutcome } from "@/features/sessions/session-questions";
import { DeliveryPreviews, useDeliveryPreviews } from "@/features/sessions/delivery-preview-view";
import { nextDeliveryId } from "@/features/sessions/delivery-preview";
import { OutboxList } from "@/features/sessions/outbox-list";
import {
  afterSend,
  classifySendFailure,
  headSendable,
  mergeStoredOutboxes,
  newClientMessageId,
  prepareResend,
  readOutbox,
  SendUnknown,
  thrownFailure,
  timeoutSignal,
  withoutMessage,
  type OutgoingMessage,
} from "@/features/sessions/send-receipts";
import { useOutboxResolution } from "@/features/sessions/use-outbox-resolution";
import { externalSendBlock } from "@/features/sessions/send-block";
import { ContinueOnAccountButton, looksLikeLoginFailure, type OpenSession } from "@/features/sessions/continue-on-account";
import { activeRests } from "@/features/accounts/account-rest";
import { localLoginOf, localLoginSignedOut, useAccounts } from "@/features/accounts/use-accounts";
import type { DraftStore } from "@/features/sessions/stream-draft";
import { useInfoOf, useMachine, useMachineRuntime, useSessionRef } from "@/lib/machines/context";
import { sessionKey, type SessionRef } from "@/lib/machines/key";
import { machineStorageKey, readMachineItem } from "@/lib/machines/storage";
import { ActivityBar, AwaitingBackgroundNotice } from "@/features/sessions/activity-bar";
import type { Source } from "@/features/sessions/use-session-feed";

interface SendRequest {
  message: OutgoingMessage;
  /**
   * Typed into the composer and sent at once, rather than taken off the queue:
   * a refusal puts the text back where it was typed, when that is still free.
   */
  typed?: boolean;
}

/**
 * Composer messages this *tab* is holding back, surviving reloads.
 *
 * Not the session's queue — that one lives in the resident, is reported as
 * `summary.resident.queuedMessages`, and is drawn by `ServerQueue`. The two
 * are named apart in the UI ("waiting to send" here, "queued in the session"
 * there) because they promise different things: a message here exists only in
 * this browser's localStorage and goes nowhere if the tab is closed, while one
 * there has been accepted and will run regardless.
 *
 * Since the server grew a real queue this one only covers what it cannot:
 *
 * - an attachment is still uploading, so there is nothing to POST yet;
 * - a one-shot Codex process is still running, because Codex has no resident
 *   or server-side queue;
 * - a send failed, and the rest is paused so the user sees the failure before
 *   the next message lands on top of it.
 *
 * Everything else is POSTed straight out and, for resident drivers, queued
 * server-side.
 */
function usePersistedQueue(
  session: SessionRef,
): [OutgoingMessage[], (q: OutgoingMessage[]) => void, React.RefObject<OutgoingMessage[]>] {
  // `rh.queue.<source>.<id>` on the local machine, as it always was.
  const name = `queue.${session.source}.${session.id}`;
  const key = machineStorageKey(session.machine, name);
  const read = (): OutgoingMessage[] => {
    try {
      // Every older shape is read, every entry gets a message id, and one that
      // was in flight when the page went away comes back `unknown`.
      // An outbox an older tab wrote under the old key since is merged in.
      return readOutbox(JSON.parse(readMachineItem(localStorage, session.machine, name, mergeStoredOutboxes) ?? "[]"));
    } catch {
      return [];
    }
  };
  const [state, setState] = useState(() => ({ key, value: read() }));
  const ref = useRef(state.value);
  if (state.key !== key) {
    const value = read();
    ref.current = value;
    setState({ key, value });
  }
  const update = (q: OutgoingMessage[]) => {
    ref.current = q;
    setState({ key, value: q });
    try {
      localStorage.setItem(key, JSON.stringify(q));
    } catch {
      // storage unavailable: the queue still works for this page
    }
  };
  return [state.value, update, ref];
}

export function Composer({
  source,
  id,
  detail,
  drafts,
  subtasks,
  chat = false,
  onQueueChanged,
  onOpenSession,
}: {
  source: Source;
  id: string;
  detail: FeedState;
  /** The feed's draft store: what the agent is doing that the transcript does not show. */
  drafts: DraftStore;
  /** Computed once by the caller, which also draws the panel that lists them. */
  subtasks: LiveSubtask[];
  /** Instant chat: tools are disabled server-side, so hide the permission pick. */
  chat?: boolean;
  /** Re-read session state after a queue route changed it. */
  onQueueChanged: () => void;
  /** Show another session in this one's place (a continuation onto an account). */
  onOpenSession?: OpenSession;
}) {
  const { t } = useTranslation();
  const runtime = useMachineRuntime();
  const api = runtime.api;
  const session = useSessionRef(source, id);
  const draftKey = sessionDraftKey(session);
  const [initialDraft] = useState(() => {
    try {
      claimSessionDraft(localStorage, session);
      return readSessionDraftSnapshot(localStorage, draftKey);
    } catch {
      return { text: "" };
    }
  });
  const [text, setTextState] = useState(initialDraft.text);
  // The machine's offline copy is read-only: nothing is queued for later
  // (architecture doc §4: no offline queue), so the composer is shut, not deferred.
  const readOnlyCopy = useShowingCopy();
  // So is an ACP session whose agent was removed: it is read, not continued,
  // until an agent with its id is added again — nothing waits for that.
  const agentRemoved = useAcpAgentLook(source === "acp" ? detail.summary.acpAgent : null).removed;
  const shut = readOnlyCopy || agentRemoved;
  // Another device's session in the local app (its gateway, S1-7): nothing
  // here can go to it — not a message, a model or an attachment — until S2.
  const otherDevice = useInfoOf(useMachine()).readOnly === true;
  useEffect(() => {
    return subscribeSessionDraft(draftKey, (snapshot) => {
      setTextState(snapshot.text);
    });
  }, [draftKey]);
  const setText = (value: string) => {
    setTextState(value);
    try {
      writeSessionDraft(localStorage, draftKey, value);
    } catch {
      // Accessing localStorage itself can be blocked; the in-memory draft still works.
    }
  };
  const sendHint = useSendHint();

  // The models offered are the ones the account this session is *bound to* may
  // run — the same value `ComposerQuota` below is given, for the same reason.
  const choice = useComposerChoice(source, id, detail.cliChoice, detail.account?.id ?? null, undefined, detail.summary.acpAgent);
  const { activeModel, activeEffort, activePermission } = choice;

  // The checklist sidecar is only ever rewritten by a tool call, so the number
  // of calls in the window is the cheapest "it may have changed" signal there
  // is. Loading earlier history moves it too; the chip debounces, so the extra
  // refetch costs one stat of a small file.
  const toolTick = useMemo(
    () => detail.items.reduce((n, i) => n + (i.kind === "tool_use" ? 1 : 0), 0),
    [detail.items],
  );

  const [queue, setQueue, queueRef] = usePersistedQueue(session);
  // claude instant chats run without tools, so they cannot read attachments.
  const allowAttach = !(chat && source === "claude");
  const attachmentState = useComposerAttachments();
  const attachments = attachmentState.attachments;
  // Enter pressed while an upload is still in flight: send as soon as it lands.
  const [sendQueued, setSendQueued] = useState(false);

  const deliveryQueue = useMemo(() => detail.summary.resident?.queuedMessages ?? [], [detail.summary.resident]);
  const delivery = useDeliveryPreviews(sessionKey(session), {
    items: detail.items, startIndex: detail.startIndex, totalItems: detail.totalItems, queue: deliveryQueue,
    // Codex and the ACP agent write every queued message into the transcript when it
    // runs; claude folds a steered one into its turn without an echo.
    receipts: detail.receipts, adoptQueue: source !== "claude",
  }, true);
  // Every message carries the id it was created with (`clientMessageId`), so a
  // retry of one the server already has is answered, not run again — see
  // `send-receipts.ts`. It stays in the outbox until the server has said yes.
  const send = useMutation({
    onMutate: ({ message }: SendRequest) => {
      const receiptId = nextDeliveryId();
      delivery.change({ type: "start", entry: {
        id: receiptId, text: message.text, after: detail.totalItems, sending: true,
        attachments: (message.images?.length ?? 0) + (message.files?.length ?? 0),
        clientMessageId: message.clientMessageId,
        resend: { text: message.text, images: message.images, files: message.files },
      } });
      return receiptId;
    },
    mutationFn: async ({ message }: SendRequest) => {
      let res;
      try {
        res = await api.POST("/api/sessions/{source}/{id}/message", {
          params: { path: { source, id } },
          body: {
            text: message.text,
            ...(message.images?.length ? { images: message.images } : {}),
            ...(message.files?.length ? { files: message.files } : {}),
            // Omitted when empty: an ACP agent may choose its own model and has no
            // permission modes, and refuses a message that names either.
            ...(activeModel.id ? { model: activeModel.id } : {}),
            // Omitted, not null: a model with no effort ladder refuses `--effort`
            // outright, and the flag is only built when a value is present.
            ...(activeEffort ? { effort: activeEffort } : {}),
            ...(activePermission ? { permission: activePermission } : {}),
            clientMessageId: message.clientMessageId,
          },
          signal: timeoutSignal(),
        });
      } catch (error) {
        // No answer at all. It may well have arrived; it is not a failure.
        throw new SendUnknown(thrownFailure(error));
      }
      if (!res.response.ok) {
        const failure = { kind: "http" as const, status: res.response.status, body: res.error };
        if (classifySendFailure(failure) === "unknown") throw new SendUnknown(failure);
        // ApiError, not Error: an `external_session` refusal carries the sentence
        // the block below shows.
        throw new ApiError(res.error);
      }
      return res.data;
    },
    onSuccess: (data, request, receiptId) => {
      const outcome = sentOutcome(data);
      if (receiptId) {
        // Taken as the answer to the question the agent was blocked on. The
        // question card is where it now shows — "answered", with these words
        // — and the transcript will never echo it as a message of its own, so
        // a preview here would sit waiting for an echo that is not coming.
        // Anything else is an ordinary accepted message, queued or not —
        // including one the notice promised as an answer and the server could
        // not take as one.
        if (outcome === "answered") delivery.change({ type: "dismiss", id: receiptId });
        else delivery.change({ type: "accepted", id: receiptId, queueId: data?.qid });
      }
      // The question this screen thought was open is not: answered from
      // another screen, stopped, or still being reconnected. Ask for the state
      // now rather than at the next frame, so the card settles, the notice goes,
      // and the message shows in the queue it actually joined.
      if (outcome === "queued instead") onQueueChanged();
      setQueue(afterSend(queueRef.current, request.message.clientMessageId, "accepted"));
      // Transcript and injection state arrive over the session websocket.
    },
    onError: (error, request, receiptId) => {
      if (receiptId) delivery.change({ type: "failed", id: receiptId });
      const outcome = error instanceof SendUnknown ? "unknown" : "refused";
      // Refused means nothing was enqueued, so a typed message goes back where
      // it was typed. Unknown means it may have been, so it stays in the outbox
      // — never in the composer, where sending it again would be a new message.
      const restored = outcome === "refused" && request.typed === true && restoreTyped(request.message);
      setQueue(afterSend(queueRef.current, request.message.clientMessageId, outcome, restored));
    },
  });

  // A refused typed message, back into a composer nobody has typed into since.
  // Text only: attachments live in the outbox entry, which then stays.
  const restoreTyped = (message: OutgoingMessage): boolean => {
    if (message.images?.length || message.files?.length) return false;
    try {
      if (readSessionDraftSnapshot(localStorage, draftKey).text !== "") return false;
    } catch {
      return false;
    }
    setText(message.text);
    return true;
  };

  const { check: checkReceipts, checking: checkingReceipts } = useOutboxResolution({
    source,
    id,
    outbox: queue,
    outboxRef: queueRef,
    setOutbox: setQueue,
    receipts: detail.receipts,
    // The server had it after all. One still waiting in its queue is shown by
    // that queue and then by the preview; one already handed over is in the
    // transcript, where a preview would only wait for an echo that has passed.
    onAdopted: ({ message, receipt }) => {
      if (receipt.status !== "queued") return;
      delivery.change({ type: "start", entry: {
        id: nextDeliveryId(), text: message.text, after: detail.totalItems, sending: false,
        attachments: (message.images?.length ?? 0) + (message.files?.length ?? 0),
        clientMessageId: message.clientMessageId, receipt: receipt.status,
        ...(receipt.qid ? { queueId: receipt.qid } : {}),
        resend: { text: message.text, images: message.images, files: message.files },
      } });
    },
  });

  // An `external_session` refusal describes the moment it was made, not the
  // session, and a spent one would keep the read-only block over a composer that
  // can send again. Dropping it when the frame stops saying external covers the
  // session released by whoever owned it.
  const scope = detail.summary.scope;
  const resetSend = send.reset;
  useEffect(() => {
    if (scope !== "external") resetSend();
  }, [scope, resetSend]);

  const interrupt = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/sessions/{source}/{id}/interrupt", {
        params: { path: { source, id } },
      });
      if (res.error) throw new Error(res.error.error);
    },
  });
  const [codexSteering, setCodexSteering] = useState(false);

  const injectionRunning = detail.injection?.state === "running";
  // Legacy Codex exec runs inherited during an upgrade still need a
  // two-step handoff: end that process, then let the ordinary queue
  // drain start a continuation against the same thread. Keep the requested
  // state until the running frame actually clears; the interrupt route returns
  // before the child has necessarily exited. App Server sessions accept sends
  // while busy and use the native steer action in ServerQueue instead.
  useEffect(() => {
    if (!injectionRunning) setCodexSteering(false);
  }, [injectionRunning]);
  // The resident is asked first and the injection record second. For a claude
  // session they are the same fact seen twice, but only the resident sees the
  // half a turn spends `starting`, and only the injection record covers codex,
  // which has no resident at all. Either one saying "running" is running — which
  // is the whole of the "it said the task finished while it was thinking" bug.
  const banner = activityBanner(detail.summary, injectionRunning);
  const interrupted = detail.injection?.state === "failed" && detail.injection.interrupted;
  const failed = injectionFailurePausesQueue(detail.injection);
  // A message that failed on the local login, while that login is signed out or
  // resting (or the failure says it was the credential): the session cannot go
  // on as it is, so the failure line offers to carry it onto an account.
  const { data: accountList } = useAccounts();
  const localLogin = localLoginOf(accountList, "claude");
  const continueOffered =
    failed &&
    !!onOpenSession &&
    source === "claude" &&
    detail.claudeRoot === "local" &&
    detail.summary.scope === "owned" &&
    (localLoginSignedOut(localLogin) || activeRests(localLogin ?? undefined).length > 0 || looksLikeLoginFailure(detail.injection?.error));
  // An actual failure pauses the queue so it can be reviewed. A user interrupt
  // does not: abandoning the current turn is often done specifically so the
  // correction already queued behind it can run. A POST refusal still pauses,
  // because the level-triggered drain would otherwise retry it forever.
  // A send with no answer is not a failure: it holds the queue as `unknown`
  // (see `headBlocked` below) without calling the queue paused.
  const sendUnknown = send.error instanceof SendUnknown;
  const sendRefused = send.isError && !sendUnknown;
  const queuePaused = (failed || sendRefused) && queue.length > 0;
  const queueHeadAction = localQueueHeadAction({
    source,
    canSend: detail.canSend,
    injectionRunning,
    sending: send.isPending,
    steering: codexSteering,
  });
  // The agent is blocked on a question, so what goes in next is its answer:
  // the server takes a message sent now as the question's free-text answer
  // rather than queueing it behind the turn that is waiting for it. The notice
  // says so whenever that is true; the button and the placeholder only when
  // *this* message is the one that will go — one typed behind messages this tab
  // is still holding goes after them, and the first of those is the answer.
  //
  // Only the words (and attachments) go in as the answer. The model, effort and
  // permission pickers below stay as they are and are still sent, but the
  // server sets them aside for an answer: they describe a process, and the one
  // answering is blocked mid-turn and cannot change. They take effect with the
  // next message that starts a turn, which is why nothing here claims more.
  const answering = composerAnswersQuestion({ questions: detail.questions, row: detail.summary });
  const sendsAnswer = answering && detail.canSend && queue.length === 0;
  const willQueue =
    !sendsAnswer &&
    willQueueSend({
      canSend: detail.canSend,
      localQueue: queue.length,
      row: detail.summary,
      injectionRunning,
    });
  // The floating panel over the transcript names every live child, so the
  // strip's own one-liner would be the same sentence twice on one screen. It
  // keeps the line only for the case the panel stays away from: a turn in
  // flight whose children the loaded window cannot see.
  const panel = backgroundPanel({ row: detail.summary, subtasks, injectionRunning });
  const subtask = banner && !panel ? (subtasks.at(-1) ?? null) : null;
  // `injection.startedAt` has been on the wire since injections existed and
  // nothing had ever read it; it is the only clock that knows when the *user's
  // message* went in rather than when the transcript happened to gain a line —
  // while that injection is running. See `workingProgress` for the rest.
  const injection = detail.injection;
  const resident = residentOf(detail.summary);
  const progress = useMemo(
    () => workingProgress(detail.items, injection, resident),
    [detail.items, injection, resident],
  );

  // Guards the window between `mutate` being called and `isPending` becoming
  // observable. The drain below is level-triggered, so that window is a render
  // it could fire in twice — and because `dispatchNext` marks the head as
  // sending synchronously, the second shot would not be a duplicate but
  // something worse: message #2 overtaking #1 on the wire.
  const dispatching = useRef(false);
  // The entry stays in the outbox, marked `sending`, until the server answers:
  // a page closed mid-send then knows to ask about it, rather than forgetting it.
  const dispatchEntry = (entry: OutgoingMessage | undefined) => {
    if (dispatching.current || entry === undefined || !detail.canSend || send.isPending) {
      return;
    }
    // `unsent` keeps its id, `failed` gets a new one; see `prepareResend`.
    const message = prepareResend(entry);
    dispatching.current = true;
    setQueue(queueRef.current.map((queued) =>
      queued.clientMessageId === entry.clientMessageId ? { ...message, state: "sending" as const } : queued));
    send.mutate(
      { message },
      {
        onSettled: () => {
          dispatching.current = false;
        },
      },
    );
  };
  const dispatchNext = () => dispatchEntry(queueRef.current[0]);

  // Drain the local queue whenever it *can* be drained, rather than on the
  // moment the session becomes sendable. See `queue-drain.ts`: the edge this
  // used to wait for stopped happening the day a busy resident started
  // accepting messages, and a queued message would then sit there until
  // somebody pressed ▶.
  useEffect(() => {
    if (
      shouldDrainQueue({
        queued: queue.length,
        canSend: detail.canSend,
        failed,
        sendFailed: sendRefused,
        sending: send.isPending,
        // Never past a message whose fate is unknown, or one waiting on a decision.
        headBlocked: !headSendable(queue),
      })
    ) {
      dispatchNext();
    }
  });

  const { anyUploading, anyFailed } = attachmentState;

  const submit = () => {
    const submittedText = text;
    const message = submittedText.trim();
    if (
      (!message && attachments.length === 0) ||
      send.isPending ||
      shut ||
      anyFailed
    ) {
      return;
    }
    if (anyUploading) {
      // Uploads are already racing; fire the message the moment they land.
      setSendQueued(true);
      return;
    }
    setSendQueued(false);

    const { images, files } = attachmentPaths(attachments);
    attachmentState.clear();
    // Named here, once: every retry of this message is the same message.
    const out: OutgoingMessage = {
      clientMessageId: newClientMessageId(),
      text: message || t("Please look at the attached file(s)."),
      images: images.length > 0 ? images : undefined,
      files: files.length > 0 ? files : undefined,
    };
    // The outbox holds it from here on, so the draft goes: a reload mid-send
    // must find it in one place, not in the outbox *and* the composer.
    setText("");
    // Busy, or already queued messages waiting: preserve order by appending.
    if (!detail.canSend || queue.length > 0) {
      setQueue([...queueRef.current, out]);
      return;
    }
    setQueue([...queueRef.current, { ...out, state: "sending" }]);
    send.mutate({ message: out, typed: true });
  };

  useEffect(() => {
    if (!sendQueued) return;
    if (anyFailed) {
      setSendQueued(false);
      return;
    }
    if (!anyUploading) submit();
  }); // runs each render; guarded by sendQueued

  if (otherDevice) {
    return (
      <div data-testid="composer-read-only" className="pb-safe shrink-0 border-t border-border bg-panel p-3">
        <p className="font-mono text-2xs leading-4 text-muted">{t("read-only here: this session is on another device")}</p>
      </div>
    );
  }
  const block = externalSendBlock({
    scope: detail.summary.scope,
    sendBlockReason: detail.sendBlockReason,
    refusal: send.error instanceof ApiError && send.error.code === EXTERNAL_SESSION ? send.error : null,
  });
  if (block) {
    return (
      <div className="pb-safe shrink-0 border-t border-border bg-panel p-3">
        <p data-signal="warn" className="font-mono text-2xs leading-4 text-warn-fg">
          {block.reason ?? t("this session belongs to another instance")}
        </p>
      </div>
    );
  }

  return (
    <div className="pb-safe shrink-0 border-t border-border bg-panel">
      <ActivityBar
        banner={banner}
        subtask={subtask}
        progress={progress}
        drafts={drafts}
        stopping={interrupt.isPending}
        onStop={bannerCanStop(banner) ? () => interrupt.mutate() : undefined}
      />
      <AnswerNotice show={answering} />
      <AwaitingBackgroundNotice resident={residentOf(detail.summary)} />
      <RotationNotice source={source} id={id} rotation={detail.rotation} />
      {interrupted && (
        <div className="border-t border-border px-4 py-1 font-mono text-2xs text-muted">
          {t("message cancelled")}
        </div>
      )}
      {failed && detail.injection?.error && (
        <div
          data-signal="failed"
          className="border-t border-border px-4 py-1 font-mono text-2xs whitespace-pre-wrap text-danger shadow-[inset_2px_0_0_var(--color-danger)]"
          title="last message failed"
        >
          {t("send failed:")} {detail.injection.error}
          {continueOffered && <ContinueOnAccountButton id={id} onOpenSession={onOpenSession} />}
        </div>
      )}
      {sendRefused && (
        // `errorText`, not `.message`: an `ApiError`'s message is the bare
        // machine token, so a refused send used to read `external_session` at a
        // user while the server had written a whole sentence for them.
        <div data-signal="failed" className="border-t border-border px-4 py-1 font-mono text-2xs whitespace-pre-wrap text-danger shadow-[inset_2px_0_0_var(--color-danger)]">
          {t("send failed:")} {errorText(send.error)}
        </div>
      )}
      {sendUnknown && queue.some((message) => message.state === "unknown") && (
        <div data-signal="unknown" className="border-t border-border px-4 py-1 font-mono text-2xs text-warn-fg shadow-[inset_2px_0_0_var(--color-warn)]">
          {t("no answer from the server — the message is kept below until it is clear whether it arrived")}
        </div>
      )}
      {/* The server's queue, then this tab's. They are drawn apart and named
          differently on purpose: everything below has been accepted and will
          run whether or not this browser is open, and everything above is
          still only a note to self. The order between them is also the order
          they run in — the local queue drains into the server one. */}
      <DeliveryPreviews
        messages={delivery.previews}
        // Dropped by the server: back into the queue as a new message, since
        // the old id already has an answer on file.
        onRequeue={(preview) => {
          if (!preview.resend) return;
          setQueue([...queueRef.current, { ...preview.resend, clientMessageId: newClientMessageId() }]);
          delivery.change({ type: "dismiss", id: preview.id });
        }}
        onDismiss={(previewId) => delivery.change({ type: "dismiss", id: previewId })}
      />
      <ServerQueue
        source={source}
        id={id}
        resident={residentOf(detail.summary)}
        receipts={detail.receipts}
        onChanged={onQueueChanged}
        onDropped={(queueId) => delivery.change({ type: "drop", queueId })}
      />
      <OutboxList
        queue={queue}
        paused={queuePaused}
        steering={codexSteering}
        headAction={queueHeadAction}
        checking={checkingReceipts}
        onSendHead={dispatchNext}
        onSteerHead={() => {
          setCodexSteering(true);
          interrupt.mutate(undefined, { onError: () => setCodexSteering(false) });
        }}
        onResend={(message) => {
          // The head goes now, past a refusal's pause, as ▶ always did; one
          // further back waits its turn as an ordinary queued message.
          if (message === queueRef.current[0]) dispatchEntry(message);
          else setQueue(queueRef.current.map((queued) => (queued === message ? prepareResend(message) : queued)));
        }}
        onCheck={checkReceipts}
        onRemove={(message) => setQueue(withoutMessage(queueRef.current, message.clientMessageId))}
      />
      {anyFailed && (
        <div data-signal="failed" className="border-t border-border px-4 py-1 font-mono text-2xs text-danger shadow-[inset_2px_0_0_var(--color-danger)]">
          {attachments.find((a) => a.error)?.error ?? t("upload failed")} — {t("remove the failed attachment to send")}
        </div>
      )}
      <div className="px-3 pt-0 pb-3 max-md:px-2 max-md:pb-2">
        <ComposerBox
          attachments={attachmentState}
          allowAttach={allowAttach}
          text={text}
          onText={setText}
          onTextLive={setTextState}
          disabled={shut}
          onSubmit={submit}
          placeholder={
            shut
              ? (detail.sendBlockReason ?? t("the device is offline — this is a read-only copy"))
              : detail.canSend
                ? `${sendsAnswer ? t("type your answer…") : t("send a message to this session…")} (${sendHint})`
                : `${detail.sendBlockReason ?? t("session is busy")} — ${t("messages are queued")}`
          }
          chips={<>
            <ComposerChoiceChips source={source} choice={choice} permission={!chat} />
            <ComposerQuota source={source} accountId={detail.account?.id ?? null} />
            <ContextRing used={detail.contextTokens} contextWindow={detail.contextWindow} />
            <ChecklistChip source={source} id={id} toolTick={toolTick} />
          </>}
          send={{
            disabled: send.isPending || shut || anyFailed || (!text.trim() && attachments.length === 0),
            label: sendsAnswer ? t("send as your answer") : willQueue ? t("add to queue") : t("send"),
            busy: send.isPending || sendQueued,
            queues: willQueue,
          }}
        />
      </div>
    </div>
  );
}
