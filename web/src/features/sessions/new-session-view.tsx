/*
 * A new session: an empty session view, the same header and composer as any
 * session's, that becomes the session once its first message is sent.
 *
 * It is a route (`/sessions/new`, and `/m/<device>/sessions/new` for another
 * device) and pane content (`{kind: "new-session"}`), and on a phone it is the
 * same full-screen view as a session. The composer is the session composer
 * (`composer-box.tsx`) — attachments pasted, dropped or picked, the draft kept
 * across reloads — with two chips only a session about to start has: which
 * agent, in which directory (and, on a page with more than one device, which
 * device). Under it, the directories the device's sessions ran in.
 *
 * Laid out as a new chat is (docs/ui-spec.md §3): one column as wide as the
 * transcript's measure, the question and the composer about the middle of the
 * view and the directories right under the composer; the composer's top never
 * moves while it grows. On a phone the composer is pinned to the bottom, where
 * the keyboard is, and the directories scroll above it.
 *
 * Sending starts the session with the message and its attachments; the view
 * shows the message as pending, and once the session is listed it is replaced
 * by the session itself — the same place, the URL or the pane content swapped
 * — whose composer carries the message on as pending until the transcript has
 * it (`seedDeliveryPreview`).
 */
import { useSearch } from "@tanstack/react-router";
import { ChevronDown, ChevronLeft, Folder } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useFeedSessions, useFeedSettings } from "@/app/global-feed";
import { MachineLink, useMachineNavigate } from "@/app/machine-route";
import { CapsLabel } from "@/components/ui/caps-label";
import { Input } from "@/components/ui/input";
import { LiveDot } from "@/components/live-dot";
import { AccountPicker, localLoginHint } from "@/features/accounts/account-picker";
import {
  accountsForProvider,
  localLoginOf,
  localLoginSignedOut,
  useAccounts,
  useStartRecheck,
  withoutAccount,
  type StartVerdict,
} from "@/features/accounts/use-accounts";
import { canStartOn, DevicePicker, deviceNote } from "@/features/machines/device-picker";
import { errorText } from "@/api/errors";
import { cn } from "@/lib/cn";
import { basename, formatRelative } from "@/lib/format";
import { isImeKeydown } from "@/lib/ime";
import { useEnsureLive, useHomeMachine, useInfoOf, useMachine, useOpenMachines } from "@/lib/machines/context";
import { sessionKey, type MachineKey } from "@/lib/machines/key";
import {
  attachmentPaths,
  ComposerBox,
  MAX_ATTACHMENTS,
  restoredAttachments,
  storedAttachments,
  useComposerAttachments,
  useSendHint,
  type ComposerAttachment,
} from "./composer-box";
import { ComposerChoiceChips, useComposerChoice, useComposerStart } from "./composer-controls";
import { DeliveryPreviews, seedDeliveryPreview } from "./delivery-preview-view";
import { nextDeliveryId, type DeliveryPreview } from "./delivery-preview";
import { DirBrowser } from "./dir-browser";
import { SessionHeader } from "./session-header";
import { IS_MAC, type NewSessionSearch } from "./new-session-open";
import {
  applyNewSessionPrefill,
  clearNewSessionDraft,
  directoryLabel,
  directoryRows,
  prefillFromSearch,
  readNewSessionDraft,
  recentDirectories,
  rememberDevice,
  startingDirectory,
  stepDirectory,
  writeNewSessionDraft,
  type NewSessionPrefill,
  type RecentDirectory,
} from "./new-session";
import { agentKey, parseAgentKey, type CreationAttempt, type NewConversation, type SessionSource } from "./new-session-flow";
import { useCreateConversation, type AcceptedConversation } from "./use-create-conversation";
import { useAcpAgents } from "./acp-agent";
import { acpAgentGlyph } from "./acp-agent-icons";
import { AgentSelect, type AgentChoice } from "./agent-select";
import { cliGlyph } from "@/components/source-icon";
import { agentsSearch, agoKey, checkFailed, TAB_NAME } from "@/features/settings/agents/agents-tabs";

/** The CLIs that are always offered, by their marks and names; then one entry per configured ACP agent. */
const CLI_CHOICES: AgentChoice[] = (["claude", "codex"] as const).map((source) => ({
  key: source,
  label: TAB_NAME[source],
  glyph: cliGlyph(source),
  group: "cli",
}));

/** A message on its way to becoming a session's first. */
interface Starting {
  text: string;
  images: string[];
  files: string[];
  /** The composer as it was, for a start the server refused. */
  composer: { text: string; attachments: ComposerAttachment[] };
  /** The server took it: a session is on its way. */
  accepted: boolean;
}

function isWide(): boolean {
  return typeof window !== "undefined" && !!window.matchMedia?.("(min-width: 768px)").matches;
}

/** The column the view is laid out in: the transcript's 48em measure, centred (docs/ui-spec.md §3). */
const COLUMN = "mx-auto w-full max-w-[48rem]";

/**
 * The empty session view on the view's device (`useMachine()`). The caller
 * keys it by the device: everything here — the draft, the directories, the
 * accounts — is that device's.
 */
export function NewSessionView({
  prefill = null,
  paneActions,
  onCreated,
  onDevice,
}: {
  /**
   * A directory, a first message or an agent to open with ("new session here",
   * the desktop app's first run), over the draft's. Taken when the view mounts,
   * and again each time a new object is passed.
   */
  prefill?: NewSessionPrefill | null;
  /** Split-pane controls, in the title row as a session's are. */
  paneActions?: ReactNode;
  /** The session is listed: show it in this view's place. */
  onCreated: (source: SessionSource, id: string) => void;
  /** Start on another device instead; the device chip is offered only with it. */
  onDevice?: (machine: MachineKey) => void;
}) {
  const { t } = useTranslation();
  const machine = useMachine();
  const info = useInfoOf(machine);
  const several = useOpenMachines().length > 1;
  // Starting a session is using the device itself, not reading its copy.
  useEnsureLive();

  const [draft] = useState(() => applyNewSessionPrefill(readNewSessionDraft(localStorage, machine), prefill));
  const [text, setText] = useState(draft.text);
  const [pickedSource, setSourceState] = useState<SessionSource>(draft.source);
  const [pickedAcpAgent, setAcpAgent] = useState<string | null>(draft.source === "acp" ? (draft.acpAgent ?? null) : null);
  const [cwdPick, setCwdPick] = useState<string | null>(draft.cwd);
  // null while untouched, so the stored default stays in force until picked.
  const [accountChoice, setAccountChoice] = useState<string | null>(draft.account);
  // Whether the account was picked in this view, rather than carried in by the
  // draft from an earlier start.
  const [accountPickedHere, setAccountPickedHere] = useState(false);
  const attachments = useComposerAttachments(() => restoredAttachments(draft.attachments));
  // The composer as of the latest render, for a start accepted after it.
  const composerNow = useRef({ text, attachments: attachments.attachments });
  composerNow.current = { text, attachments: attachments.attachments };
  const attempt = useRef<CreationAttempt | null>(draft.attempt);
  const [starting, setStarting] = useState<Starting | null>(null);
  // Send pressed, and held — for an upload still in flight, or for the start
  // answer — with what was pressed: the message as it was and the attachments
  // it carried (by key). That is what goes out once it can, whatever is typed
  // meanwhile; emptying the composer cancels it.
  const [pressed, setPressed] = useState<{ text: string; keys: string[] } | null>(null);
  const sendQueued = pressed !== null;
  const [browsing, setBrowsing] = useState(false);
  const [expanded, setExpanded] = useState(false);
  const composerRegion = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  // Each row on screen, by its directory: to focus or scroll to.
  const rowNodes = useRef(new Map<string, HTMLButtonElement>());

  const sessions = useFeedSessions();
  const recent = useMemo(() => recentDirectories(sessions, Infinity), [sessions]);
  const cwd = startingDirectory(cwdPick, recent);
  const homeDir = useFeedSettings()?.homeDir ?? "";
  const shown = directoryRows(recent, cwd, expanded);
  const onScreen = [...shown.rows.map((dir) => dir.cwd), ...(shown.other ? [shown.other] : [])];
  // The one row Tab lands on, as in any radio group: the pick, else the first.
  const tabStop = onScreen.includes(cwd) ? cwd : (onScreen[0] ?? null);

  const { data: accountList } = useAccounts();
  // Every configured ACP agent is an agent of its own here, by its name and
  // mark. A draft may name one since removed, or none (`?agent=acp`): once
  // the list is in, a removed one falls back to Claude Code, and none is the
  // first agent there is.
  const { data: acpList } = useAcpAgents();
  const acpAgents = acpList?.agents ?? [];
  const acpAgent =
    pickedSource !== "acp"
      ? null
      : acpList === undefined
        ? pickedAcpAgent
        : (acpAgents.find((agent) => agent.id === (pickedAcpAgent ?? acpAgents[0]?.id))?.id ?? null);
  const source: SessionSource = pickedSource === "acp" && acpList !== undefined && acpAgent === null ? "claude" : pickedSource;
  // Each CLI runs under its own accounts: a Claude login cannot start a Codex
  // session. An ACP agent runs on its own configuration and has none.
  const acp = source === "acp";
  // An agent whose last check failed is offered in red and can still be
  // picked; picked, the composer says why, with the way to its details.
  const failingWord = (agent: (typeof acpAgents)[number]) =>
    checkFailed(agent.lastCheck) ? t(agent.lastCheck?.state === "not_found" ? "command not found" : "does not start") : null;
  const choices: AgentChoice[] = [
    ...CLI_CHOICES,
    ...acpAgents.map((agent): AgentChoice => ({
      key: agentKey({ source: "acp", acpAgent: agent.id }),
      label: agent.name,
      glyph: acpAgentGlyph(agent.id),
      group: "acp",
      failing: failingWord(agent),
    })),
    // The pick while the list is on its way, so the picker never shows a blank.
    ...(acp && acpAgent && !acpAgents.some((agent) => agent.id === acpAgent)
      ? [{ key: agentKey({ source, acpAgent }), label: acpAgent, glyph: acpAgentGlyph(acpAgent), group: "acp" as const }]
      : []),
  ];
  const failing = acp ? acpAgents.find((agent) => agent.id === acpAgent && checkFailed(agent.lastCheck)) : undefined;
  const manage = useMachineNavigate();
  const accounts = acp ? [] : accountsForProvider(accountList?.accounts ?? [], source);
  // Untouched, the picker shows where the server will start a session that
  // names no account — the default (moved off a rest), the local login, or,
  // while that is signed out and nobody chose it, an account in the
  // auto-rotation pool — for the model this start will send; with nothing
  // stored, that model is the one the server paired with the login
  // (`useComposerStart`). The start names no account, so the server's rule
  // decides, and it lands where it is shown.
  const startQuery = useComposerStart(source);
  const { start: startAccount } = startQuery;
  // A start left to the server is sent on an answer asked again at the press.
  const startCheck = useStartRecheck(startQuery);
  // The draft keeps the account across starts, so a local login picked weeks
  // ago, while it still worked, would otherwise go on deciding every session
  // after it stopped. Picked here and now, it is honoured — and labelled.
  const localSignedOutNow = !acp && localLoginSignedOut(localLoginOf(accountList, source));
  const staleLocalPick = accountChoice === "" && !accountPickedHere && localSignedOutNow;
  // `null` is untouched; "" the local login, picked; an id an account, picked.
  const picked = staleLocalPick ? null : accountChoice;
  const accountValue = picked ?? startAccount?.accountId ?? "";
  // Starting on the local login while it is signed out: allowed — the user may
  // be signing in right now — but said, with how to fix it.
  const localSignedOut = !accountValue && localSignedOutNow;
  // The models on offer follow the account picked here: this is the one
  // surface where the credentials are chosen before the session exists.
  const choice = useComposerChoice(source, undefined, null, accountValue || null, picked === null ? (startAccount?.model ?? null) : null, acpAgent);

  // The draft, kept as it changes — except while a start is in flight, when
  // the composer is empty only because the message is on its way: a reload
  // then finds the message still in the draft, with the attempt it went under.
  const ready = storedAttachments(attachments.attachments);
  const readyKey = JSON.stringify(ready);
  useEffect(() => {
    if (starting) return;
    writeNewSessionDraft(localStorage, machine, {
      text, attachments: ready, source, ...(acpAgent ? { acpAgent } : {}), cwd: cwdPick, account: accountChoice, attempt: attempt.current,
    });
    // `ready` is read through its key: a new array every render, the same list.
  }, [machine, text, readyKey, source, acpAgent, cwdPick, accountChoice, starting]);

  const setSource = (next: SessionSource, nextAcpAgent: string | null = null) => {
    setSourceState(next);
    setAcpAgent(next === "acp" ? nextAcpAgent : null);
    // A pick made for the other CLI names an account this one cannot use.
    setAccountChoice(null);
    setAccountPickedHere(false);
    // And a start pressed for one CLI is not sent to another once its answer lands.
    dropPressed();
  };

  // A prefill passed to a view already open (the address names one again):
  // what it names replaces what is there, as on a first open.
  const appliedPrefill = useRef(prefill);
  useEffect(() => {
    if (!prefill || prefill === appliedPrefill.current) return;
    appliedPrefill.current = prefill;
    if (prefill.cwd) setCwdPick(prefill.cwd);
    if (prefill.source && (prefill.source !== source || (prefill.acpAgent && prefill.acpAgent !== acpAgent))) {
      setSource(prefill.source, prefill.acpAgent ?? null);
    }
    if (prefill.text !== undefined) {
      setText(prefill.text);
      attachments.clear();
      attempt.current = null;
    }
    // Only a new prefill is acted on; the rest is read as it is then.
  }, [prefill]);

  const input = (message: string, images: string[], files: string[]): NewConversation => ({
    kind: "session", source, acpAgent, cwd, text: message, model: choice.activeModel.id, effort: choice.activeEffort,
    permission: choice.activePermission, images, files,
    // Only a pick is sent; untouched, the server's start rule decides — unless
    // the pair shown is one no model-less start reaches, which has to be named.
    ...(acp ? {} : picked !== null
      ? { accountId: picked || null }
      : startAccount?.placed === false
        ? { accountId: startAccount.accountId ?? null, accountFrom: "start" as const }
        : {}),
  });

  const onAccepted = ({ submitted, id, clientMessageId }: AcceptedConversation) => {
    if (submitted.kind !== "session") return;
    // Anything written or added after the press is what the draft keeps now —
    // read as the composer is now, not as it was when the start was sent.
    const laterText = composerNow.current.text;
    const later = storedAttachments(composerNow.current.attachments);
    if (laterText.trim() || later.length) {
      writeNewSessionDraft(localStorage, machine, { ...readNewSessionDraft(localStorage, machine), text: laterText, attachments: later, attempt: null });
    } else {
      clearNewSessionDraft(localStorage, machine);
    }
    if (several) rememberDevice(localStorage, machine);
    // The session's own composer takes the message on as pending, thumbnails
    // and all, until its transcript has it.
    const images = submitted.images ?? [];
    const files = submitted.files ?? [];
    seedDeliveryPreview(sessionKey({ machine, source: submitted.source, id }), {
      id: nextDeliveryId(), text: submitted.text, after: 0, sending: false, clientMessageId,
      attachments: images.length + files.length,
      resend: { text: submitted.text, ...(images.length ? { images } : {}), ...(files.length ? { files } : {}) },
    });
    setStarting((now) => {
      now?.composer.attachments.forEach((a) => a.preview && URL.revokeObjectURL(a.preview));
      return now ? { ...now, accepted: true } : now;
    });
  };
  const onAttempt = useCallback((next: CreationAttempt | null) => {
    attempt.current = next;
    // Written at once, not with the rest: the composer is empty right now.
    writeNewSessionDraft(localStorage, machine, { ...readNewSessionDraft(localStorage, machine), attempt: next });
  }, [machine]);
  const start = useCreateConversation(input(text.trim(), [], []), onAccepted, onCreated, { attempt: draft.attempt, onAttempt });

  const busy = starting !== null || start.isPending;
  const canStart = canStartOn(info);
  const empty = !text.trim() && attachments.attachments.length === 0;

  // A held send dropped: nothing goes out, and the next press asks afresh.
  const dropPressed = () => {
    setPressed(null);
    startCheck.cancel();
  };

  // Attachments added after a press that did not fit back in beside a refused
  // message's own: named until the next press.
  const [noRoom, setNoRoom] = useState<string[]>([]);

  const submit = () => {
    setNoRoom([]);
    if (busy || empty || !cwd.trim() || !canStart || attachments.anyFailed) return;
    setPressed({ text, keys: attachments.attachments.map((a) => a.key) });
  };

  // The pressed send, each render until it goes out or is dropped. Uploads are
  // already racing: it starts the moment they land. Left to the server, it goes
  // out on the answer to what is asked now — none yet (a draft restored at load
  // can be sent before it is in), or one not asked again since the press: the
  // rule may have moved on while it sat on screen. A different answer is shown
  // first, and the send goes out on it — within a few re-asks and a few
  // seconds; past those, with no account, for the server to place
  // (`useStartRecheck`). A pick made meanwhile is sent as one.
  useEffect(() => {
    if (!pressed || busy) return;
    const held = attachments.attachments.filter((a) => pressed.keys.includes(a.key));
    // Anything that would not let it start is a cancel: the composer emptied
    // (the obvious way to call it off) — or just its text, when the press had
    // some — an attachment it carries failed or was taken out, no directory,
    // a device that cannot start it.
    const textCleared = !!pressed.text.trim() && !text.trim();
    if (empty || textCleared || !cwd.trim() || !canStart || held.length < pressed.keys.length || held.some((a) => a.status === "error")) {
      dropPressed();
      return;
    }
    if (held.some((a) => a.status === "uploading")) return;
    let verdict: StartVerdict = "send";
    if (picked === null) {
      verdict = startCheck.check();
      if (verdict === "wait") return;
    } else {
      startCheck.cancel();
    }
    setPressed(null);
    const { images, files } = attachmentPaths(held);
    const message = pressed.text.trim() || t("Please look at the attached file(s).");
    const composer = { text: pressed.text, attachments: held };
    setStarting({ text: message, images, files, composer, accepted: false });
    // What went out leaves the composer; what was typed or added after the
    // press stays: the text only clears if it is still what was pressed, and
    // only the attachments the message carries are taken.
    setText((now) => (now === pressed.text ? "" : now));
    attachments.take(pressed.keys);
    // As the composer stands once these land — an answer can beat the render.
    composerNow.current = {
      text: text === pressed.text ? "" : text,
      attachments: attachments.attachments.filter((a) => !pressed.keys.includes(a.key)),
    };
    const request = input(message, images, files);
    start.mutate(verdict === "unplaced" ? withoutAccount(request) : request, {
      // Refused: nothing started, so the message goes back where it was typed,
      // in front of anything written since.
      onError: () => {
        setStarting(null);
        setText((now) => (!now.trim() ? composer.text : now.includes(composer.text) ? now : `${composer.text}\n${now}`));
        const dropped = attachments.putBack(composer.attachments);
        setNoRoom(dropped.map((a) => a.name));
      },
    });
  }); // runs each render; guarded by `pressed`

  // What was typed goes along to the other device, unless something is
  // already typed there; the attachments stay, uploaded where they are.
  const switchDevice = (next: MachineKey) => {
    // A start pressed for this device is not sent from here once it moves.
    dropPressed();
    const there = readNewSessionDraft(localStorage, next);
    if (text.trim() && !there.text.trim()) writeNewSessionDraft(localStorage, next, { ...there, text, source, ...(acpAgent ? { acpAgent } : {}) });
    onDevice?.(next);
  };

  // Where the pick went: its row brought into view, and — when the pick was
  // made in the list by keyboard — focused, as a radio group's next item is.
  // The intent names its row: an effect still pending from the pick before
  // (keys pressed faster than effects run) leaves it to the right one.
  const reveal = useRef<{ path: string; focus: boolean } | null>(null);
  // In the commit that draws the pick, so the focus moves with the mark.
  useLayoutEffect(() => {
    const intent = reveal.current;
    if (!intent || intent.path !== cwd) return;
    reveal.current = null;
    const row = rowNodes.current.get(cwd);
    row?.scrollIntoView?.({ block: "nearest" });
    if (intent.focus) row?.focus({ preventScroll: true });
  }, [cwd]);

  const composerField = () => composerRegion.current?.querySelector?.<HTMLTextAreaElement>("[data-composer] textarea") ?? null;

  /** A row clicked or typed in: the pick, and on a desktop the message next. */
  const pick = (path: string) => {
    setCwdPick(path);
    setBrowsing(false);
    // Not on a phone: there it would raise the keyboard over the list.
    if (isWide()) composerField()?.focus();
  };

  // ↑/↓ move the pick among the rows on screen: in the list as in any radio
  // group, and from the composer with Alt (⌥) — or alone while nothing is
  // typed, when the caret has nowhere to go. Enter stays send.
  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    if (isImeKeydown(event) || event.metaKey || event.ctrlKey || event.shiftKey || busy) return;
    const target = event.target as HTMLElement;
    const inList = target.getAttribute("role") === "radio" && !!list.current?.contains(target);
    const inComposer = target.tagName === "TEXTAREA" && !!composerRegion.current?.contains(target);
    if (!inList && !(inComposer && (event.altKey || text === ""))) return;
    event.preventDefault();
    const next = stepDirectory(onScreen, cwd, event.key === "ArrowDown" ? 1 : -1);
    if (!next) return;
    reveal.current = { path: next, focus: inList };
    setCwdPick(next);
  };

  /** The folder chip: the list is where the directory is chosen — bring it into view, on the pick. */
  const showList = () => {
    const row = tabStop === null ? undefined : rowNodes.current.get(tabStop);
    if (!row) return;
    row.scrollIntoView?.({ block: "nearest" });
    row.focus({ preventScroll: true });
  };

  const preview: DeliveryPreview[] = starting
    ? [{
        id: "new-session", text: starting.text, after: 0, sending: !starting.accepted,
        attachments: starting.images.length + starting.files.length,
        resend: { text: starting.text, images: starting.images, files: starting.files },
      }]
    : [];
  const offline = !canStart ? deviceNote(info, t, { forStarting: true }) : null;
  const sendHint = useSendHint();
  const title = (
    <h2 className="text-[1.25rem] leading-[1.625rem] font-semibold tracking-[-0.02em] text-fg max-md:text-[1.125rem] max-md:leading-6">
      {t("Where do you want to start?")}
    </h2>
  );

  return (
    <div
      data-new-session={machine}
      onKeyDown={onKeyDown}
      className="flex min-h-0 min-w-0 flex-1 flex-col"
    >
      <SessionHeader
        paneActions={paneActions}
        titleRow={<>
          {!paneActions && (
            <MachineLink
              to="/sessions"
              title={t("back")}
              className="flex size-9 shrink-0 items-center justify-center text-muted hover:bg-hover md:hidden"
            >
              <ChevronLeft className="size-4" />
            </MachineLink>
          )}
          <span className="min-w-0 flex-1 truncate text-[13px] font-semibold max-md:text-[15px]">{t("new session")}</span>
        </>}
      />
      {/* Desktop: the question, about the middle of the view. Its height is
          the view's, never the composer's, so typing or pasting moves nothing
          above the composer. A phone asks above the directories instead. */}
      <div aria-hidden="true" className="shrink-0 max-md:hidden" style={{ height: "max(1rem, calc(50% - 12rem))" }} />
      <div data-new-session-title="" className="shrink-0 px-4 pb-3 max-md:hidden">
        <div className={COLUMN}>{title}</div>
      </div>
      {/* The composer: under the question on a desktop, pinned to the bottom on a phone. */}
      <div
        ref={composerRegion}
        data-new-session-composer=""
        className="pb-safe shrink-0 bg-panel px-4 max-md:order-last max-md:border-t max-md:border-border max-md:px-2 max-md:pt-2"
      >
        {/* The gap under the composer is the column's: `.pb-safe` owns this one's bottom padding. */}
        <div className={cn(COLUMN, "pb-3 max-md:pb-2")}>
          {offline && <Notice signal="offline">{offline}</Notice>}
          {start.isError && !starting && (
            <Notice signal="failed">{t("could not start the session:")} {errorText(start.error)}</Notice>
          )}
          {noRoom.length > 0 && (
            <Notice signal="warn">
              <span data-attachments-dropped="">
                {t("no room beside the returned message's attachments (at most {{max}}), so these were taken out: {{names}}", {
                  max: MAX_ATTACHMENTS,
                  names: noRoom.join(", "),
                })}
              </span>
            </Notice>
          )}
          {attachments.anyFailed && (
            <Notice signal="failed">
              {attachments.attachments.find((a) => a.error)?.error ?? t("upload failed")} — {t("remove the failed attachment to send")}
            </Notice>
          )}
          {sendQueued && picked === null && !attachments.anyUploading && (
            <div data-choosing-account="" className="mb-1.5 px-2.5 py-1 font-mono text-2xs text-faint">
              {t("choosing account…")}
            </div>
          )}
          {localSignedOut && !starting && (
            <Notice signal="warn" local>
              {localLoginHint(source, t)}
              {" — "}
              {accounts.length > 0 ? (
                t("or pick an account below")
              ) : (
                <>
                  {t("or add an account")}
                  {" · "}
                  <MachineLink to="/settings" search={agentsSearch(source === "codex" ? "codex" : "claude")} className="underline hover:text-fg">
                    {t("Settings")}
                  </MachineLink>
                </>
              )}
            </Notice>
          )}
          {failing?.lastCheck && !starting && (
            <Notice signal="failed">
              <span data-acp-failing={failing.id}>
                {t("{{name}} cannot start: {{why}} (checked {{ago}})", {
                  name: failing.name,
                  why: failingWord(failing),
                  ago: t(agoKey(failing.lastCheck.checkedAt).key, { n: agoKey(failing.lastCheck.checkedAt).n }),
                })}
              </span>
              {" · "}
              <MachineLink to="/settings" search={agentsSearch("acp", failing.id)} className="underline hover:text-fg">
                {t("agents & accounts")}
              </MachineLink>
            </Notice>
          )}
          <ComposerBox
            attachments={attachments}
            text={text}
            onText={setText}
            disabled={busy}
            autoFocus={isWide()}
            onSubmit={submit}
            placeholder={`${t("what should the agent do?")} (${sendHint})`}
            chips={<>
              {/* Never another device shown read-only here (the local app's gateway): nothing starts there. */}
              {onDevice && <DevicePicker ghost forStarting writable value={machine} onSelect={switchDevice} disabled={busy} />}
              <AgentSelect
                value={agentKey({ source, acpAgent })}
                choices={choices}
                onChange={(v) => {
                  const next = parseAgentKey(v);
                  if (next) setSource(next.source, next.acpAgent);
                }}
                onManage={() => void manage({ to: "/settings", search: agentsSearch(source) })}
              />
              <DirectoryChip cwd={cwd} onShow={showList} disabled={busy} />
              <ComposerChoiceChips
                source={source}
                choice={choice}
                account={!acp && accounts.length > 0 && (
                  <AccountPicker
                    ghost
                    accounts={accounts}
                    // "" is the local login, which the picker names `null`;
                    // untouched, it shows where the server will start.
                    value={accountValue || null}
                    onSelect={(id) => {
                      setAccountChoice(id ?? "");
                      setAccountPickedHere(true);
                    }}
                    allowLocal
                    provider={source}
                    disabled={busy}
                  />
                )}
              />
            </>}
            send={{
              disabled: busy || empty || !cwd.trim() || !canStart || attachments.anyFailed,
              label: t("start session"),
              busy: busy || sendQueued,
            }}
          />
        </div>
      </div>
      {/* The directories: under the composer on a desktop, scrolling on their
          own so the composer stays put; above it on a phone, by the thumb. */}
      <div className="transcript-scroll min-h-0 flex-1 overflow-x-hidden overflow-y-auto px-4 max-md:px-2">
        <div className={cn(COLUMN, "pb-6 max-md:flex max-md:min-h-full max-md:flex-col max-md:justify-end max-md:pt-4 max-md:pb-3")}>
          <div className="mb-3 md:hidden">{title}</div>
          {starting ? (
            // The message on its way, where the directories were: the composer
            // above (on a phone, below) stays where it is.
            <>
              <DeliveryPreviews messages={preview} className="" />
              <div data-starting="" className="flex items-center gap-1.5 pt-2 font-mono text-2xs text-faint">
                <span data-signal="working" className="flex shrink-0 text-ok">
                  <LiveDot />
                </span>
                <span>{t("starting the session in {{dir}}…", { dir: basename(cwd) || cwd })}</span>
              </div>
            </>
          ) : (
            <DirectoryList
              listRef={list}
              rowNodes={rowNodes.current}
              tabStop={tabStop}
              recent={recent}
              rows={shown.rows}
              other={shown.other}
              hidden={shown.hidden}
              expanded={expanded}
              cwd={cwd}
              homeDir={homeDir}
              browsing={browsing}
              onExpand={() => setExpanded((open) => !open)}
              onBrowse={() => setBrowsing((open) => !open)}
              onPick={pick}
            />
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * A line over the composer that something is in the way, with its state edge:
 * red for what stops the start, amber for what it should know (`local`: the
 * local login is signed out).
 */
function Notice({ signal, local = false, children }: { signal: "offline" | "failed" | "warn"; local?: boolean; children: ReactNode }) {
  return (
    <div
      data-signal={signal}
      data-local-signed-out={local ? "" : undefined}
      className={cn(
        "mb-1.5 px-2.5 py-1 font-mono text-2xs whitespace-pre-wrap",
        signal === "warn" ? "text-warn-fg shadow-[inset_2px_0_0_var(--color-warn)]" : "text-danger shadow-[inset_2px_0_0_var(--color-danger)]",
      )}
    >
      {children}
    </div>
  );
}

/** A radio dot, drawn: the row is the control (as an answer's option is, `question-card.tsx`). */
function RadioMark({ on }: { on: boolean }) {
  return (
    <span
      aria-hidden="true"
      data-shape="radio"
      className={cn("flex size-3 shrink-0 items-center justify-center rounded-full border", on ? "border-fg text-fg" : "border-border-strong")}
    >
      {on && <span data-shape="dot" className="size-1.5 rounded-full bg-current" />}
    </span>
  );
}

/**
 * The body before anything is sent: the directories this device's sessions
 * ran in, one line each, the pick marked as a radio group marks it; then
 * 「显示全部（N）」 and a way to any other directory.
 */
function DirectoryList({
  listRef,
  rowNodes,
  tabStop,
  recent,
  rows,
  other,
  hidden,
  expanded,
  cwd,
  homeDir,
  browsing,
  onExpand,
  onBrowse,
  onPick,
}: {
  listRef: React.RefObject<HTMLDivElement | null>;
  /** Where each row's button is kept, by directory. */
  rowNodes: Map<string, HTMLButtonElement>;
  /** The row Tab lands on. */
  tabStop: string | null;
  recent: RecentDirectory[];
  rows: RecentDirectory[];
  other: string | null;
  hidden: number;
  expanded: boolean;
  cwd: string;
  homeDir: string;
  browsing: boolean;
  onExpand: () => void;
  onBrowse: () => void;
  onPick: (path: string) => void;
}) {
  const { t } = useTranslation();
  const keys = IS_MAC ? "⌥↑↓" : "Alt+↑↓";
  return (
    <div data-recent-directories="">
      {recent.length > 0 && (
        <div className="mb-1.5 flex items-baseline justify-between gap-3 max-md:hidden">
          <CapsLabel>{t("recent directories")}</CapsLabel>
          <span
            className="font-mono text-2xs text-faint"
            title={t("Alt+↑/↓ (or ↑/↓ while the message is empty) moves the pick; Enter sends")}
          >
            {t("{{keys}} to switch", { keys })}
          </span>
        </div>
      )}
      {recent.length === 0 && (
        <p className="mb-2 font-mono text-2xs text-faint">{t("no sessions on this device yet — choose a directory to start in")}</p>
      )}
      <div ref={listRef} role="radiogroup" aria-label={t("working directory")} className="flex flex-col">
        {rows.map((dir) => (
          <DirectoryRow
            key={dir.cwd}
            path={dir.cwd}
            homeDir={homeDir}
            on={dir.cwd === cwd}
            tabStop={dir.cwd === tabStop}
            nodes={rowNodes}
            time={formatRelative(dir.lastActivity)}
            title={`${dir.cwd} · ${dir.sessions === 1 ? t("1 session") : t("{{n}} sessions", { n: dir.sessions })}`}
            onPick={onPick}
          />
        ))}
        {/* A directory typed or browsed to, where no session ran yet: it is the pick, so it shows as one. */}
        {other && <DirectoryRow path={other} homeDir={homeDir} on tabStop={other === tabStop} nodes={rowNodes} title={other} onPick={onPick} />}
      </div>
      <div className="mt-1 flex items-center justify-between gap-2">
        {hidden > 0 || expanded ? (
          <button
            type="button"
            aria-expanded={expanded}
            onClick={onExpand}
            className="-ml-1.5 h-7 px-1.5 text-xs text-muted hover:bg-hover hover:text-fg max-md:h-9"
          >
            {expanded ? t("show fewer") : t("show all ({{n}})", { n: recent.length })}
          </button>
        ) : <span />}
        <button
          type="button"
          aria-expanded={browsing}
          onClick={onBrowse}
          className="-mr-1.5 h-7 px-1.5 text-xs text-muted hover:bg-hover hover:text-fg max-md:h-9"
        >
          {t("choose another directory…")}
        </button>
      </div>
      {(browsing || recent.length === 0) && <OtherDirectory start={cwd.trim()} onPick={onPick} />}
    </div>
  );
}

/** One directory, one line: the radio, its name, where it is (cut in the middle), when it was last used. */
function DirectoryRow({
  path,
  homeDir,
  on,
  tabStop,
  nodes,
  time,
  title,
  onPick,
}: {
  path: string;
  homeDir: string;
  on: boolean;
  tabStop: boolean;
  nodes: Map<string, HTMLButtonElement>;
  time?: string;
  title: string;
  onPick: (path: string) => void;
}) {
  const { name, head, tail } = directoryLabel(path, homeDir);
  return (
    <button
      type="button"
      role="radio"
      aria-checked={on}
      tabIndex={tabStop ? 0 : -1}
      ref={(node) => {
        if (node) nodes.set(path, node);
        else nodes.delete(path);
      }}
      data-directory={path}
      title={title}
      onClick={() => onPick(path)}
      className={cn(
        // Boxes that share their lines, as an answer's options do; the pick's
        // ink box is drawn over its neighbours' hairlines.
        "relative -mt-px flex h-8 w-full min-w-0 items-center gap-2 border bg-bg px-2.5 text-left first:mt-0 max-md:h-10",
        on ? "z-[1] border-fg" : "border-border hover:bg-hover",
      )}
    >
      <RadioMark on={on} />
      <span className="max-w-[60%] shrink-0 truncate text-[13px] font-medium text-fg max-md:text-sm">{name}</span>
      <span className="flex min-w-0 flex-1 font-mono text-2xs text-faint">
        <span className="truncate">{head}</span>
        <span className="max-w-[75%] shrink-0 truncate">{tail}</span>
      </span>
      {time && <span className="shrink-0 font-mono text-2xs text-faint tabular-nums">{time}</span>}
    </button>
  );
}

/** Any other directory: a path typed in, or the folder browser. */
function OtherDirectory({ start, onPick }: { start: string; onPick: (path: string) => void }) {
  const { t } = useTranslation();
  const [typed, setTyped] = useState("");
  return (
    <div data-other-directory="" className="mt-1">
      <form
        className="flex gap-1"
        onSubmit={(e) => {
          e.preventDefault();
          if (typed.trim()) onPick(typed.trim());
        }}
      >
        <Input
          value={typed}
          onChange={(e) => setTyped(e.target.value)}
          placeholder="/path/to/project"
          aria-label={t("working directory")}
          spellCheck={false}
          autoComplete="off"
          className="font-mono text-2xs"
        />
        <button
          type="submit"
          disabled={!typed.trim()}
          className="h-6.5 shrink-0 border border-border-strong px-2 font-mono text-2xs text-fg hover:bg-hover disabled:opacity-40"
        >
          {t("use")}
        </button>
      </form>
      <DirBrowser start={start} onPick={onPick} />
    </div>
  );
}

/**
 * The working directory, as a chip: the one the session starts in. It is
 * chosen in the list under the composer, so the chip takes you there.
 */
function DirectoryChip({ cwd, onShow, disabled }: { cwd: string; onShow: () => void; disabled: boolean }) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onShow}
      title={cwd ? `${t("working directory")}: ${cwd}` : t("working directory")}
      aria-label={t("working directory")}
      data-chip="directory"
      className="flex h-6 max-w-48 shrink-0 items-center gap-1 rounded pr-1 pl-1.5 font-mono text-2xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40"
    >
      <Folder className="size-3 shrink-0" />
      <span className="truncate">{cwd ? basename(cwd) || cwd : t("working directory")}</span>
      <ChevronDown className="size-2.5 shrink-0 text-faint" />
    </button>
  );
}

/**
 * `/sessions/new` (`/m/<device>/sessions/new`): the view as the page. The
 * session it starts replaces it in the history, so Back leaves the session
 * rather than returning to a form that has already been sent.
 */
export function NewSessionRoute() {
  const machine = useMachine();
  const home = useHomeMachine();
  const navigate = useMachineNavigate();
  const { cwd, draft, agent } = useSearch({ strict: false }) as NewSessionSearch;
  // Another device shown read-only here (the local app's gateway, S1-7):
  // nothing starts there — the view is this device's instead.
  const readOnly = useInfoOf(machine).readOnly === true;
  useEffect(() => {
    if (readOnly && machine !== home) void navigate({ to: "/sessions/new", machine: home, replace: true });
  }, [readOnly, machine, home, navigate]);
  // A prefill ("new session here", the desktop app's first run) is named in
  // the address once: the view takes it into its draft, and a reload must not
  // undo what was changed since. A new one arriving while the view is open is
  // a new object, which the view takes again.
  const prefill = useMemo(() => prefillFromSearch({ cwd, draft, agent }), [cwd, draft, agent]);
  useEffect(() => {
    if (prefill) void navigate({ to: "/sessions/new", replace: true });
  }, [prefill, navigate]);
  return (
    <NewSessionView
      key={machine}
      prefill={prefill}
      onCreated={(source, id) => void navigate({ to: "/sessions/$source/$id", params: { source, id }, replace: true })}
      onDevice={(next) => void navigate({ to: "/sessions/new", machine: next, replace: true })}
    />
  );
}
