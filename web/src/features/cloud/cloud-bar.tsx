import { type ReactNode, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { useNavigate } from "@tanstack/react-router";
import { Check, ChevronsUpDown, CloudOff, KeyRound, LayoutGrid, LogOut } from "lucide-react";
import { Button } from "@/components/ui/button";
import { BlobTransfers } from "@/components/blob-transfers";
import { cn } from "@/lib/cn";
import { formatExactTime, formatTime } from "@/lib/format";
import type { PathStats } from "@/lib/e2e";
import type { PresenceState, LinkState } from "@/lib/hub/link";
import type { ChannelRefusal, ChannelRoute, ConnectStep, MachineChannel, MachineStatus } from "@/lib/hub/machine";
import type { RelayPool } from "@/lib/hub/relays";
import { isCatchingUp, type StreamProgress } from "@/lib/hub/streams";
import type { MachineKey } from "@/lib/machines/key";
import type { MachineInfo } from "@/lib/machines/runtime";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { CloudMode } from "@/lib/transport/switching";
import {
  type CatchUpSource,
  type CopyView,
  useAgentKeyState,
  useCatchingUp,
  useCatchUp,
  useChannelStatus,
  useChannelStep,
  useCopyView,
  useHubOnly,
  useLinkState,
  useMachinePresence,
  useRelayPool,
  useRetryAt,
} from "./context";
import { useCloudMode } from "@/features/machines/copy-mode";
import { DeviceDot } from "@/features/machines/device-picker";
import { AccountAlerts } from "./account-alerts";
import { AccountButton, DevicesButton, openAccount, openMyDevices, removePasskeyThere } from "./my-devices";
import { cityName, noticeText } from "./relays";
import { BarMachineName } from "./machine-name";
import { storageFullText, useMe } from "./plan";
import { CompatBadge } from "./browser-support";
import { storageView } from "@/lib/hub/plan";
import { SignOutChoice } from "./sign-out-dialog";
import { addressOn, type HubMachine, type HubShell } from "./hub-shell";
import { hubShell, useFocusedMachine, useShellMachines } from "@/features/machines/shells";
import { openAddDevice } from "./onboarding";
import { AddDeviceHint, useAddDeviceHint } from "./onboarding-hints";
import { useRouteMachine } from "@/app/machine-route";
import { PushMovedNotice } from "@/features/move/push-moved-notice";
import { useHomeMachine } from "@/lib/machines/context";

/**
 * In cloud mode, a thin strip above the app: which machine it is about,
 * whether that machine is online, how this page reaches it — or, until it
 * does, what the page is doing about it: the step it is on, or when it tries
 * again (`barState`, `ConnectionLabel`) — a switch to any other machine of
 * the account (no reload: `hub-shell.ts`), and the one way to 「我的设备」
 * (`my-devices.tsx`). Renders nothing in local mode.
 *
 * When the machine cannot be reached the app shows its offline copy
 * (`lib/transport/offline.ts`), read-only, and a notice under the strip says so
 * and how old the copy is — and, while the copy takes a while to catch up from
 * the hub, how far it has got (`catchUpText`). Only a device with no copy at
 * all is covered ("no offline copy yet") until the machine is back.
 */
export function CloudBar() {
  const shell = hubShell();
  // The device the app's address is about (`/m/<device>/…`, else the main
  // device): the bar is about it, and the shell's focus follows it.
  const routeMachine = useRouteMachine();
  const focus = useFocusedMachine();
  useEffect(() => {
    shell?.follow(routeMachine);
  }, [shell, routeMachine]);
  const machine = shell?.machine(routeMachine) ?? shell?.machine(focus) ?? null;
  if (!shell || !machine) return null;
  return <Bar shell={shell} machine={machine} />;
}

/**
 * What the strip says about the page's way to the machine (`label`): nothing
 * in words while the channel is live (`e2e`, the path indicator says how);
 * otherwise what the page is doing — connecting to the hub (`hub`), or
 * waiting out the hub link's backoff (`hubRetry`); opening the channel
 * (`connecting`, which the bar says by step), or waiting out its backoff
 * after a try that failed (`deviceRetry`) — or why it is not: the hub says
 * the device is offline (`offline`, so nothing is tried), or the device turns
 * this browser away (`refused`, `unconfirmed`, `unsaved`). For the two that
 * wait, `retryAt` is when the next try is.
 */
export function barState(input: {
  link: LinkState;
  presence: PresenceState | null;
  channel: MachineStatus;
  mode: CloudMode;
  copy: CopyView;
  /** How the device last refused this browser's channel, if it did (`MachineChannel.refusal`). */
  refusal?: ChannelRefusal["kind"] | null;
  /** When the hub link tries again, while it waits out a backoff (`HubLink.retryAt`). */
  linkRetryAt?: number | null;
  /** When the channel is tried again, while that waits out a backoff (`ModeController.retryAt`). */
  channelRetryAt?: number | null;
}): { label: BarLabel; retryAt: number | null; notice: boolean; cover: boolean } {
  const { link, presence, channel, mode, copy, refusal = null, linkRetryAt = null, channelRetryAt = null } = input;
  const online = presence === "online";
  // A live channel needs no words, whatever the hub's connection is doing: it
  // goes through a relay (relay-and-plans §2.9).
  const live = channel === "open" && mode === "live";
  // Online and answering, but not taking this browser: say why, rather than
  // "connecting…" on every retry (the refusal outlives them).
  const refused = !live && link === "open" && online && refusal !== null;
  const why = refusal === "notAccepted" ? "refused" : refusal === "unsaved" ? "unsaved" : "unconfirmed";
  // Between tries, say when the next one is; a try under way (a pipe may
  // start one before the backoff is out) is said by its step instead.
  const waiting = channel !== "connecting" && channelRetryAt !== null;
  const label: BarLabel = live ? "e2e" : link !== "open" ? (linkRetryAt !== null ? "hubRetry" : "hub") : !online ? "offline" : refused ? why : waiting ? "deviceRetry" : "connecting";
  // The machine is out of reach (the hub says so, or the channel keeps
  // failing) — rather than merely still being connected to, or reached and
  // refusing this browser (which `RefusalNotice` explains instead).
  const unreachable = (link === "open" && presence !== null && !online) || channel === "offline" || (channel === "error" && !refused) || (link !== "open" && mode === "offline");
  return {
    label,
    retryAt: label === "hubRetry" ? linkRetryAt : label === "deviceRetry" ? channelRetryAt : null,
    notice: mode === "offline" && copy.hasCopy && unreachable,
    cover: mode === "offline" && copy.loaded && !copy.hasCopy && link === "open" && presence !== null && !online,
  };
}

export type BarLabel = "hub" | "hubRetry" | "offline" | "e2e" | "connecting" | "deviceRetry" | "refused" | "unconfirmed" | "unsaved";

function Bar({ shell, machine }: { shell: HubShell; machine: HubMachine }) {
  const { t } = useTranslation();
  const presence = useMachinePresence(shell.link, shell.relays, machine.key);
  const link = useLinkState(shell.link);
  const channel = useChannelStatus(machine.channel);
  const mode = useCloudMode(machine.transport);
  const copy = useCopyView(machine.mirror);
  const keyState = useAgentKeyState(machine.key);
  const me = useMe(shell.account, shell.link);
  const machines = useShellMachines();
  const linkRetryAt = useRetryAt(shell.link);
  const channelRetryAt = useRetryAt(machine.controller);
  const step = useChannelStep(machine.channel);
  const refusal = machine.channel.refusal;
  const state = barState({ link, presence, channel, mode, copy, refusal: refusal?.kind ?? null, linkRetryAt, channelRetryAt });
  const catchUp = useCatchUpShown(machine.source, { mode, link });
  // A full cloud storage: the hub took no more of this machine's copy after its last write (§3.8).
  const storage = storageView(me?.plan, me?.view?.machines ?? []);
  const storageFull = storage?.level === "full" ? { stoppedMs: me?.view?.machines.find((m) => m.id === machine.key)?.copy?.lastWriteMs ?? storage.lastWriteMs } : null;
  const refusalShown = state.label === "refused" || state.label === "unconfirmed" || state.label === "unsaved";
  return (
    <>
      <div data-cloud-bar="" className={BAR_STRIP}>
        <DeviceDot state={presence ?? "offline"} title={presence ? undefined : t("unknown")} />
        <BarMachineName machine={machine.key} mirror={machine.mirror} />
        <MachineSwitcher shell={shell} current={machine.key} />
        <ConnectionLabel
          label={state.label}
          step={channel === "connecting" ? step : null}
          retryAt={state.retryAt}
          link={shell.link}
          controller={machine.controller}
        />
        {catchUp && !state.notice && <BarCatchUp source={machine.source} />}
        {state.label === "e2e" && <LivePathIndicator channel={machine.channel} relays={shell.relays} />}
        <span className="flex-1" />
        <CompatBadge compact />
        <DevicesButton />
        <AccountButton />
        <SignOutChoice>{(ask) => <SignOutButton onClick={ask} />}</SignOutChoice>
      </div>
      {catchUp && !state.notice && <BarCatchUpLine source={machine.source} />}
      <AccountAlerts account={shell.account} link={shell.link} where="bar" onRemovePasskey={removePasskeyThere} onReview={() => openAccount()} />
      <PushMovedNotice />
      {keyState === "behind" && <OldKeyNotice onRetry={() => machine.recheckKey(true)} />}
      {channel === "error" && machine.channel.blocked && <BlockedNotice kind={machine.channel.blocked} />}
      {refusalShown && refusal && (
        <RefusalNotice
          refusal={refusal}
          limited={shell.account.isAdmin === false}
          others={(shell.account.chain?.admins() ?? []).map((d) => d.info.id).filter((id) => id !== shell.account.deviceId)}
          onDevices={() => openMyDevices()}
        />
      )}
      {state.notice && <OfflineNotice freshestMs={copy.freshestMs} storageFull={storageFull} catchUp={catchUp ? machine.source : null} />}
      {state.cover && <NoCopyCover onDevices={() => openMyDevices()} />}
      <BlobTransfers labelOf={(from) => (from && from !== machine.key ? (machines.find((m) => m.key === from)?.name ?? from) : null)} />
    </>
  );
}

/** The strip itself: one 28px line of 11px mono (the ui-audit fixture draws it with this too). */
export const BAR_STRIP = "flex h-7 shrink-0 items-center gap-2 border-b border-border bg-bg px-3 font-mono text-2xs";

/** The bar's sign-out icon; `onClick` asks first (`SignOutChoice`). */
export function SignOutButton({ onClick }: { onClick: () => void }) {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      data-testid="cloud-bar-sign-out"
      onClick={onClick}
      className="flex h-5 items-center gap-1 px-1.5 text-muted hover:bg-hover hover:text-fg"
      title={t("sign out")}
      aria-label={t("sign out")}
    >
      <LogOut className="size-3" />
    </button>
  );
}

/** What the strip says while the channel is being opened: the step it is on (`MachineChannel.step`), or plainly "connecting…" between them. */
function connectingText(t: TFunction, step: ConnectStep | null): string {
  if (step?.kind === "presence") return t("finding the device…");
  if (step?.kind === "relay") return step.city ? t("{{city}} relay…", { city: cityName(t, step.city) }) : t("reaching a relay…");
  if (step?.kind === "handshake") return t("handshake…");
  return t("connecting…");
}

/**
 * The strip's words for `label` (`barState`): short enough that at a phone's
 * width (390 px) every state, its countdown included, shows whole beside a
 * short device name and the device switcher in every language, rather than
 * both truncating — the UI audit's cloud-bar fixture measures that, with a
 * relay city as long as San Francisco. "no reply" is how a try ended, beside
 * "device offline", which is the hub's word, so nothing is tried. A refusal
 * gets a short label of its own here: its full sentence is the notice under
 * the strip (`RefusalNotice`), and the device switcher's (`switcherNote`).
 * While a try is waited for (`retryAt`), the countdown to it follows, and is
 * also the way to try now: the hub link's `wake`, or the channel's
 * controller's `poke`. Open and live needs no words: every path is
 * end-to-end encrypted, so the strip only says which path it is and how fast
 * (`LivePathIndicator`).
 */
export function ConnectionLabel({
  label,
  step = null,
  retryAt = null,
  link,
  controller,
}: {
  label: BarLabel;
  /** What the channel being opened waits on, for `connecting`. */
  step?: ConnectStep | null;
  retryAt?: number | null;
  link?: { wake(): void } | null;
  controller?: { poke(): void } | null;
}) {
  const { t } = useTranslation();
  const text = {
    hub: t("reaching the hub…"),
    hubRetry: t("no hub"),
    offline: t("device offline"),
    e2e: null,
    connecting: connectingText(t, step),
    deviceRetry: t("no reply"),
    refused: t("refuses this browser"),
    unconfirmed: t("confirmation pending"),
    unsaved: t("access not saved"),
  }[label];
  if (text === null) return null;
  const refusal = label === "refused" || label === "unconfirmed" || label === "unsaved";
  const retry = label === "hubRetry" ? () => link?.wake() : () => controller?.poke();
  return (
    <span
      data-connection={label}
      data-signal={label === "offline" ? "offline" : refusal ? "warn" : undefined}
      className={cn("flex min-w-0 items-center gap-1", label === "offline" ? "text-danger" : refusal ? "text-warn-fg" : "text-faint")}
    >
      <span className="truncate">{text}</span>
      {retryAt !== null && (
        <>
          <span aria-hidden="true">·</span>
          <RetryCountdown at={retryAt} onRetry={retry} />
        </>
      )}
    </span>
  );
}

/** Whole seconds until `at`, rounded up and never 0: a try still waited for is at least "1 s" away. */
export function secondsUntil(at: number, now: number): number {
  return Math.max(1, Math.ceil((at - now) / 1000));
}

/**
 * The countdown to the next try, which is also the button that makes it now.
 * It says "retry 16 s" — the seconds to the try, in as few words as fit
 * beside the switcher on a phone — and its name says both what it does and
 * when the try comes by itself. It ticks once a second, on the second of what
 * is left (so the number changes when it should), and only this button
 * re-renders, not the bar: as with `LivePathIndicator`, a repaint of the
 * whole strip for each tick is work nobody reads.
 */
function RetryCountdown({ at, onRetry }: { at: number; onRetry: () => void }) {
  const { t } = useTranslation();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = () => {
      const current = Date.now();
      setNow(current);
      const left = at - current;
      if (left > 0) timer = setTimeout(tick, left % 1000 || 1000);
    };
    tick();
    return () => clearTimeout(timer);
  }, [at]);
  const n = secondsUntil(at, now);
  const name = t("retry now · next try in {{n}} s", { n });
  return (
    <button
      type="button"
      data-testid="retry-now"
      onClick={onRetry}
      aria-label={name}
      title={name}
      className="flex h-5 shrink-0 items-center whitespace-nowrap text-muted tabular-nums underline-offset-2 hover:text-fg hover:underline"
    >
      {t("retry {{n}} s", { n })}
    </button>
  );
}

/** A catch-up is said only once it has gone on this long (ms): a plain resume, caught up in a moment, says nothing. */
export const CATCH_UP_DELAY_MS = 800;
/** Once said, it stays at least this long (ms), so it does not blink. */
export const CATCH_UP_MIN_MS = 600;
/** A download is big enough for its percentage with this many pages in the catch-up… */
export const BIG_DOWNLOAD_PAGES = 8;
/** …or once its pages have been coming for this long (ms). */
export const BIG_DOWNLOAD_MS = 1_500;

/** What the page says about the copy catching up, and the tooltip that says what its counts are. */
export interface CatchUpLine {
  text: string;
  title: string | null;
}

/**
 * What the page says while the machine's offline copy catches up from the
 * hub (`StreamClient.progress`), or `null` while nothing is: "updating from
 * hub", then what — the session list while only the index catches up, or
 * "sessions 3/7" while sessions do (3 caught up of the 7 followed now: the
 * recent sessions this device keeps, which may grow as the index brings in
 * newer ones) — and the share of a checkpoint's pages fetched while a
 * download big enough to matter runs ("· 42%"). Short, as the strip's own
 * words are, and whole wherever it is put; what the counts are is in the
 * tooltip.
 *
 * Sessions, not messages: no count of messages is true while it is shown.
 * The hub sends a stream's head only after every record it catches up on, so
 * there is no total until the catch-up is over; and a record is one of the
 * agent's write batches (a few seconds of a session), not a message, whose
 * items show only once the head arrives. A session's stream is either caught
 * up or not, and a checkpoint's signed manifest names all of its pages.
 *
 * The percentage counts every page of the catch-up so far (`episode`), so a
 * second download carries on from the first instead of starting at 0 %; when
 * a total really grows, the percentage drops rather than pretending.
 */
export function catchUpText(progress: StreamProgress, t: TFunction, now = Date.now()): CatchUpLine | null {
  if (!isCatchingUp(progress)) return null;
  const { catchingUp, synced, waiting, retrying, episode, downloadingSince } = progress;
  const parts = [t("updating from hub")];
  const titles: string[] = [];
  if (catchingUp.session > 0) {
    const counts = { done: synced.session, total: waiting.session + retrying.session + catchingUp.session + synced.session };
    parts.push(t("sessions {{done}}/{{total}}", counts));
    titles.push(t("recent sessions kept on this device: {{done}} of {{total}} up to date", counts));
  } else if (catchingUp.index > 0) {
    parts.push(t("session list"));
  }
  const big = episode.total >= BIG_DOWNLOAD_PAGES || (downloadingSince !== null && now - downloadingSince >= BIG_DOWNLOAD_MS);
  if (downloadingSince !== null && episode.total > 0 && big) {
    parts.push(`${Math.floor((100 * episode.done) / episode.total)}%`);
    titles.push(t("{{done}} of {{total}} pieces fetched", { done: episode.done, total: episode.total }));
  }
  return { text: parts.join(" · "), title: titles.length ? titles.join(" · ") : null };
}

/**
 * `active`, once it has held for `delayMs`, and kept true for at least
 * `minMs` once it is: what lets the bar say a catch-up only when there is
 * one worth saying, and not blink when it ends just after.
 */
export function useShownAfter(active: boolean, delayMs = CATCH_UP_DELAY_MS, minMs = CATCH_UP_MIN_MS): boolean {
  const [shown, setShown] = useState(false);
  const shownAt = useRef(0);
  useEffect(() => {
    if (active === shown) return;
    if (active) {
      const timer = setTimeout(() => {
        shownAt.current = Date.now();
        setShown(true);
      }, delayMs);
      return () => clearTimeout(timer);
    }
    const timer = setTimeout(() => setShown(false), Math.max(0, minMs - (Date.now() - shownAt.current)));
    return () => clearTimeout(timer);
  }, [active, shown, delayMs, minMs]);
  return shown;
}

/**
 * Whether the bar says how far the copy of the machine in view has caught up:
 * only while the page shows that copy (`mode` offline) over an open hub link,
 * and something of it is being caught up on (a stream before the hub's
 * `stream_synced`, or a checkpoint's pages) — never live, where the copy is
 * not what is shown, and never with the link down, which the bar's own words
 * say. A stream waiting out a backoff does not count, so one that keeps
 * failing cannot keep it on screen. Said after `CATCH_UP_DELAY_MS`, kept for
 * `CATCH_UP_MIN_MS` (`useShownAfter`), but gone at once with the link or
 * into live.
 */
export function useCatchUpShown(source: CatchUpSource | null | undefined, page: { mode: CloudMode; link: LinkState }): boolean {
  const eligible = page.mode === "offline" && page.link === "open";
  const busy = useCatchingUp(source);
  return useShownAfter(eligible && busy) && eligible;
}

/** The catch-up's words: tabular, so a count that moves does not shift what follows it. */
export function CatchUpWords({ line }: { line: CatchUpLine }) {
  return (
    <span data-testid="catch-up" title={line.title ?? undefined} className="tabular-nums">
      {line.text}
    </span>
  );
}

/**
 * The catch-up's words for `source`, kept current on their own: a
 * checkpoint's pages move the count many times a second, and only this span
 * re-renders for it, not the bar (as with `RetryCountdown`). Once the
 * catch-up is over it keeps its last words for as long as the bar keeps it up
 * (`CATCH_UP_MIN_MS`). `children` places the words (with whatever leads into
 * them), and is not called while there are none.
 */
export function CatchUpNote({ source, children = (words) => words }: { source: CatchUpSource; children?: (words: ReactNode) => ReactNode }) {
  const { t } = useTranslation();
  const progress = useCatchUp(source);
  const [, redraw] = useState(0);
  // A small download earns its percentage by lasting: draw again when it has.
  const since = progress.downloadingSince;
  useEffect(() => {
    if (since === null) return;
    const left = since + BIG_DOWNLOAD_MS - Date.now();
    if (left <= 0) return;
    const timer = setTimeout(() => redraw((n) => n + 1), left);
    return () => clearTimeout(timer);
  }, [since]);
  const line = catchUpText(progress, t);
  // The last words said for this source (another machine's are not this one's).
  const [last, setLast] = useState<{ source: CatchUpSource; line: CatchUpLine } | null>(null);
  if (line && (last?.source !== source || line.text !== last.line.text || line.title !== last.line.title)) setLast({ source, line });
  const shown = line ?? (last?.source === source ? last.line : null);
  return shown ? children(<CatchUpWords line={shown} />) : null;
}

/**
 * The catch-up in the strip, after the connection's words, when there is no
 * offline notice to carry it (the device is online and being connected to,
 * or turns this browser away; or no copy of it is on this device yet). Only
 * where the strip has room for it beside the longest connection words and
 * countdown (1024 px and up); narrower, it is a line of its own under the
 * strip (`BarCatchUpLine`) — at a phone's width the strip's own words leave
 * it a few pixels at most. Should a long device name still crowd it, it is
 * the first thing to give way, before the name and the connection's words,
 * and the retry countdown never does. Its dot sits as close as the one
 * before a countdown.
 */
export function BarCatchUp({ source }: { source: CatchUpSource }) {
  return (
    <CatchUpNote source={source}>
      {(words) => (
        <span data-catch-up="bar" className="-ml-1 flex min-w-0 shrink-[999] items-center gap-1 text-faint max-lg:hidden">
          <span aria-hidden="true">·</span>
          <span className="truncate">{words}</span>
        </span>
      )}
    </CatchUpNote>
  );
}

/**
 * The catch-up as a line of its own under the strip, below 1024 px, where
 * `BarCatchUp` has no room: lined up with the device's name above it (a gap
 * the dot's width), whole, as the offline notice's own line is.
 */
export function BarCatchUpLine({ source }: { source: CatchUpSource }) {
  return (
    <CatchUpNote source={source}>
      {(words) => (
        <div data-catch-up="line" className="flex shrink-0 items-center gap-2 border-b border-border bg-bg px-3 py-1 font-mono text-2xs text-muted lg:hidden">
          <span aria-hidden="true" className="size-[7px] shrink-0" />
          <span className="min-w-0 truncate">{words}</span>
        </div>
      )}
    </CatchUpNote>
  );
}

/** Whole milliseconds, never "0 ms": a sub-millisecond round trip reads as 1 ms. */
const wholeMs = (ms: number) => Math.max(1, Math.round(ms));
const roundTrip = (ms: number | null | undefined) => (ms == null ? "—" : `${wholeMs(ms)} ms`);

/** How fast a round trip reads: green under 100 ms, amber under 250 ms, red beyond. */
export function latencyTone(ms: number): "ok" | "warn" | "danger" {
  return ms < 100 ? "ok" : ms < 250 ? "warn" : "danger";
}
const TONE_CLASS = { ok: "text-ok", warn: "text-warn-fg", danger: "text-danger" } as const;

/** Two small solid squares and the line between them: device and machine, directly. */
function DirectIcon({ className }: { className?: string }) {
  return (
    <svg aria-hidden="true" viewBox="0 0 12 12" className={cn("size-3 shrink-0", className)} data-icon="direct">
      <rect x="0" y="4.5" width="3" height="3" fill="currentColor" />
      <rect x="9" y="4.5" width="3" height="3" fill="currentColor" />
      <line x1="3" y1="6" x2="9" y2="6" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}

/** The same, with a hollow square in the middle of the line: through a relay. */
function RelayIcon({ className }: { className?: string }) {
  return (
    <svg aria-hidden="true" viewBox="0 0 12 12" className={cn("size-3 shrink-0", className)} data-icon="relay">
      <rect x="0" y="4.5" width="3" height="3" fill="currentColor" />
      <rect x="9" y="4.5" width="3" height="3" fill="currentColor" />
      <line x1="3" y1="6" x2="4.25" y2="6" stroke="currentColor" strokeWidth="1.5" />
      <line x1="7.75" y1="6" x2="9" y2="6" stroke="currentColor" strokeWidth="1.5" />
      <rect x="4.5" y="4" width="3" height="4" fill="none" stroke="currentColor" strokeWidth="1" />
    </svg>
  );
}

export interface PathIndicatorProps {
  paths: PathStats | null;
  /** Which relay (or, when none could carry it, the hub) the channel goes through. */
  route?: ChannelRoute | null;
  /** A quiet line: the manual relay failed and the page went back to automatic. */
  notice?: string | null;
  /** This device does not connect directly. */
  noDirect?: boolean;
  /** Open the relay picker. */
  onChangeRelay?: () => void;
}

/**
 * How the channel reaches the machine (relay-and-plans §2.14): two 12 px
 * icons — direct, through a relay (or the hub, when no relay could carry the
 * channel) — the active one in ink, the other grey, then the active path's
 * latest round trip, coloured by how fast it is. No words: which relay,
 * chosen how, or the hub, is in the hover card (and the `aria-label`), with
 * "Change relay…". Nothing about encryption: every path is end-to-end
 * encrypted. The bar shows it only while the channel is live; until then it
 * says in words what the page is doing (`ConnectionLabel`).
 */
export function PathIndicator({ paths, route = null, notice = null, noDirect = false, onChangeRelay }: PathIndicatorProps) {
  const { t } = useTranslation();
  const direct = paths?.direct === true;
  const rtt = direct ? paths?.directRttMs : paths?.relayRttMs;
  const ms = rtt == null ? null : wholeMs(rtt);
  const tone = ms === null ? null : latencyTone(ms);
  const connecting = paths === null && route === null;
  const onHub = route?.kind === "hub";
  const kind = connecting ? "connecting" : direct ? "direct" : onHub ? "hub" : "relay";
  const relay = route?.kind === "relay" ? route : null;
  const emergency = relay?.emergency === true;
  const city = relay ? cityName(t, relay.relay.city) : "";
  const shown = ms === null ? "—" : `${ms} ms`;
  const relayLine = relay
    ? t("relay · {{city}} ({{id}}) · {{mode}} · {{ms}}", {
        city,
        id: relay.relay.id,
        mode: relay.mode === "manual" ? t("manual") : t("automatic"),
        ms: roundTrip(paths?.relayRttMs),
      })
    : onHub
      ? t("Hub · {{ms}}", { ms: roundTrip(paths?.relayRttMs) })
      : null;
  const lines = [
    direct ? t("direct · {{ms}}", { ms: roundTrip(paths?.directRttMs) }) : null,
    relayLine,
    emergency ? t("The relays in use are unavailable: temporarily through {{city}}", { city }) : null,
    notice,
    noDirect ? t("direct connections off") : null,
  ].filter((l): l is string => !!l);
  const label = connecting
    ? t("connecting…")
    : direct
      ? t("direct, round trip {{ms}}", { ms: shown })
      : onHub
        ? t("through the Hub, round trip {{ms}}", { ms: shown })
        : t("through the {{city}} relay, round trip {{ms}}", { city: city || "—", ms: shown });
  const [open, setOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        openOnHover
        delay={150}
        data-path={kind}
        aria-label={label}
        className="flex shrink-0 items-center gap-1 whitespace-nowrap tabular-nums"
      >
        <DirectIcon className={direct ? "text-fg" : "text-faint"} />
        <RelayIcon className={!direct && !connecting ? "text-fg" : "text-faint"} />
        {emergency && <span data-signal="warn" className="size-[5px] shrink-0 bg-warn" />}
        <span data-latency={tone ?? undefined} className={cn("ml-0.5", tone ? TONE_CLASS[tone] : "text-faint")}>
          {shown}
        </span>
      </PopoverTrigger>
      <PopoverContent side="bottom" className="w-72 max-w-[calc(100vw-2rem)]">
        <div data-testid="path-card" className="flex flex-col gap-1 font-mono text-2xs leading-4">
          {lines.map((line) => (
            <span key={line} className="text-muted">
              {line}
            </span>
          ))}
          {onChangeRelay && (
            <button
              type="button"
              className="mt-1 self-start text-fg underline-offset-2 hover:underline"
              onClick={() => {
                setOpen(false);
                onChangeRelay();
              }}
            >
              {t("Change relay…")}
            </button>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** How often the shown round trip may change; a change of path shows at once. */
export const SHOWN_EVERY_MS = 5_000;

/** Open 「我的设备」 at its relay picker (under 「这台设备」). */
export function openRelayPicker(): void {
  openMyDevices("relays");
}

/**
 * The bar's indicator, fed from the channel. A direct path is probed every
 * second; repainting for each probe is work nobody reads, so a new round trip
 * is taken at most every `SHOWN_EVERY_MS` — and only this span re-renders,
 * not the whole bar.
 */
function LivePathIndicator({ channel, relays }: { channel: MachineChannel | null | undefined; relays: RelayPool | null | undefined }) {
  const { t } = useTranslation();
  useRelayPool(relays);
  const noDirect = useHubOnly();
  const [shown, setShown] = useState<{ paths: PathStats | null; route: ChannelRoute | null }>(() => ({
    paths: channel?.paths ?? null,
    route: channel?.route ?? null,
  }));
  const taken = useRef({ at: 0, direct: false, measured: false, route: null as ChannelRoute | null });
  useEffect(() => {
    if (!channel) {
      setShown({ paths: null, route: null });
      return;
    }
    const take = (force: boolean) => {
      const next = channel.paths ?? null;
      const now = Date.now();
      const direct = next?.direct === true;
      const measured = (direct ? next?.directRttMs : next?.relayRttMs) != null;
      const last = taken.current;
      const moved = channel.route !== last.route;
      if (!force && !moved && direct === last.direct && measured === last.measured && now - last.at < SHOWN_EVERY_MS) return;
      taken.current = { at: now, direct, measured, route: channel.route };
      setShown({ paths: next, route: channel.route });
    };
    take(true);
    return channel.onPaths(() => take(false));
  }, [channel]);
  return (
    <PathIndicator
      paths={shown.paths}
      route={shown.route}
      notice={noticeText(t, relays?.notice ?? null)}
      noDirect={noDirect}
      onChangeRelay={relays ? openRelayPicker : undefined}
    />
  );
}

/**
 * The strip under the bar while the open machine still uses an older key
 * than the chain's newest for it, and handing it the new one in the channel
 * did not work: amber, "needs you" — what it has sealed since can be read by
 * whoever held the old key.
 */
export function OldKeyNotice({ onRetry }: { onRetry?: () => void }) {
  const { t } = useTranslation();
  return (
    <div
      role="alert"
      data-testid="old-key-notice"
      data-signal="warn"
      className="flex shrink-0 items-center gap-2 border-b border-border bg-warn/7 px-3 py-1.5 shadow-[inset_2px_0_0_var(--color-warn)]"
    >
      <span className="inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("old key")}</span>
      <KeyRound aria-hidden="true" className="size-3 shrink-0 text-muted" />
      <span className="min-w-0 flex-1 truncate text-xs text-muted">{t("the device in view still uses an old key, and could not be given the new one")}</span>
      {onRetry && (
        <Button size="sm" variant="secondary" onClick={onRetry}>
          {t("try again")}
        </Button>
      )}
    </div>
  );
}

/**
 * Why the machine cannot be reached live while the hub carries no channels
 * (it answered 4426, relay-and-plans §2.15) and no relay can carry this one:
 * no relay is available, or the machine's agent is from before relays and
 * has to be updated on the machine. The offline copy still reads.
 */
export function BlockedNotice({ kind }: { kind: "noRelay" | "agentTooOld" }) {
  const { t } = useTranslation();
  return (
    <div
      role="status"
      data-testid="channel-blocked"
      data-signal="warn"
      className="flex shrink-0 items-center gap-2 border-b border-border bg-warn/7 px-3 py-1.5 shadow-[inset_2px_0_0_var(--color-warn)]"
    >
      <CloudOff aria-hidden="true" className="size-3 shrink-0 text-muted" />
      <span className="min-w-0 flex-1 truncate text-xs text-muted">
        {kind === "agentTooOld"
          ? t("the device in view has an agent too old to be reached: update it on that device")
          : t("no relay available: live features are unavailable for now; the offline copy can be browsed")}
      </span>
    </div>
  );
}

/**
 * Why the device in view does not take this browser while it is online and
 * answering (`MachineChannel.refusal`), and what to do about it: it does not
 * accept control from a limited device (make it a full device elsewhere — a
 * full device is always accepted, so for one the hub's own words are all
 * there is to say), or it waits for another device of the account to vouch
 * for its newest list of devices — opening Droppen there once is enough.
 */
export function RefusalNotice({ refusal, limited, others, onDevices }: { refusal: ChannelRefusal; limited: boolean; others: string[]; onDevices: () => void }) {
  const { t } = useTranslation();
  const text =
    refusal.kind === "unsaved"
      ? t("The device could not save its record of who connects to it, so it turned this browser away. Check that its disk is not full and that Droppen can write its data there; this browser connects by itself afterwards.")
      : refusal.kind === "unconfirmed"
      ? others.length > 0
        ? t("Open Droppen once on {{devices}}: that device confirms the account's devices for this one, and this browser connects by itself afterwards.", {
            devices: others.join(", "),
          })
        : t("Open Droppen once on another device of the account: it confirms the account's devices for this one, and this browser connects by itself afterwards.")
      : limited
        ? t("This browser is a limited device of the account. On another of your devices, open my devices and make it a full device.")
        : refusal.why
          ? t("The hub would not connect this browser to the device: {{why}}", { why: refusal.why })
          : t("The hub would not connect this browser to the device. Reload the page; if it keeps happening, check my devices.");
  return (
    <div
      role="status"
      data-testid="channel-refused"
      data-kind={refusal.kind}
      data-signal="warn"
      className="flex shrink-0 items-center gap-2 border-b border-border bg-warn/7 px-3 py-1.5 shadow-[inset_2px_0_0_var(--color-warn)]"
    >
      <KeyRound aria-hidden="true" className="size-3 shrink-0 text-muted" />
      <span className="min-w-0 flex-1 text-xs text-muted" title={refusal.kind === "notAccepted" && limited ? (refusal.why ?? undefined) : undefined}>
        {text}
      </span>
      <button type="button" className="shrink-0 font-mono text-2xs text-muted underline-offset-2 hover:text-fg hover:underline" onClick={onDevices}>
        {t("my devices")}
      </button>
    </div>
  );
}

/**
 * The strip under the bar while the app shows the offline copy: how old it
 * is, and that it is read-only — and, with the account's cloud storage full,
 * a line saying the copy stopped there (relay-and-plans §3.8).
 *
 * While the copy catches up from the hub (`catchUp`, the machine's source,
 * given once `useCatchUpShown` says so) how far it has got follows the time
 * it is as of: it is that time the catch-up moves on, so the time changing
 * when it is over is all the "done" there is. Below the shell's desktop
 * width the time alone fills the line (it is cut there already), so the
 * catch-up takes a line of its own under it, as a full storage does.
 */
export function OfflineNotice({
  freshestMs,
  storageFull = null,
  catchUp = null,
}: {
  freshestMs: number | null;
  storageFull?: { stoppedMs: number | null } | null;
  catchUp?: CatchUpSource | null;
}) {
  const { t } = useTranslation();
  const iso = freshestMs === null ? null : new Date(freshestMs).toISOString();
  return (
    <div
      data-testid="offline-notice"
      data-signal="offline"
      className="flex shrink-0 flex-col border-b border-border bg-danger/4 px-3 py-1.5 shadow-[inset_2px_0_0_var(--color-danger)]"
    >
      <span className="flex min-w-0 items-center gap-2">
        <CloudOff className="size-3 shrink-0 text-danger" />
        <span className="min-w-0 flex-1 truncate text-xs text-muted">
          <span className="font-mono text-fg" title={iso ? formatExactTime(iso) : undefined}>
            {iso ? t("offline copy · as of {{time}}", { time: formatTime(iso) }) : t("offline copy")}
          </span>
          {catchUp && (
            <CatchUpNote source={catchUp}>
              {(words) => (
                <span data-catch-up="notice" className="font-mono max-md:hidden">
                  <span className="text-faint"> · </span>
                  {words}
                </span>
              )}
            </CatchUpNote>
          )}
          <span className="text-faint"> · </span>
          {t("read-only: sending and terminals need the device online")}
        </span>
      </span>
      {catchUp && (
        <CatchUpNote source={catchUp}>
          {(words) => (
            <span data-catch-up="notice" className="truncate pl-5 font-mono text-2xs text-muted md:hidden">
              {words}
            </span>
          )}
        </CatchUpNote>
      )}
      {storageFull && (
        <span data-testid="offline-notice-storage" className="truncate pl-5 text-2xs text-danger">
          {storageFullText(storageFull.stoppedMs, t)}
        </span>
      )}
    </div>
  );
}

/** Over the app when the machine is away and this device has nothing of it to show. */
export function NoCopyCover({ onDevices }: { onDevices: () => void }) {
  const { t } = useTranslation();
  return (
    <div data-testid="no-offline-copy" className="fixed inset-x-0 top-7 bottom-0 z-50 flex items-center justify-center bg-bg">
      <div className="flex max-w-[26rem] flex-col gap-3 p-6">
        <span data-label="" data-signal="offline" className="label-caps flex items-center gap-1.5 text-2xs text-danger">
          <CloudOff className="size-3" />
          {t("device offline")}
        </span>
        <span className="text-[28px] leading-[1.1] font-semibold tracking-[-0.03em] text-fg">{t("Nothing of it on this device yet.")}</span>
        <span className="text-xs leading-5 text-muted">
          {t("No offline copy of it on this device yet: one is made while it is online. This view comes back as soon as it reconnects.")}
        </span>
        <button
          type="button"
          data-variant="primary"
          onClick={onDevices}
          className="mt-1 flex h-9 items-center justify-center gap-1.5 self-start border border-fg bg-fg px-4 text-[13px] font-medium text-bg hover:bg-fg/85"
        >
          <LayoutGrid className="size-3.5" />
          {t("my devices")}
        </button>
      </div>
    </div>
  );
}

/**
 * 「设为主设备」's question: the screens are the main device's, and the main
 * device is the account's (`HubShell.setMain` writes it to the hub), so the
 * change is made on every one of the person's devices at once.
 */
export function MainDeviceConfirm({ next, old, onConfirm, onCancel }: { next: string; old: string; onConfirm: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  return (
    <div data-testid="set-main-confirm" className="mt-1 flex flex-col gap-2 border-t border-border px-2 pt-2 pb-1">
      <p className="text-2xs leading-4 text-muted">
        {t(
          "Screens are kept on the main device, which is the same on all your devices: with {{new}} as the main device, the screens on {{old}} are out of sight on all of them until you switch back to {{old}}.",
          { new: next, old },
        )}
      </p>
      <span className="flex gap-2">
        <button
          type="button"
          data-testid="set-main-device-confirm"
          data-variant="primary"
          onClick={onConfirm}
          className="flex h-6 items-center bg-fg px-2 font-mono text-2xs text-bg hover:bg-fg/85"
        >
          {t("set as main device")}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="flex h-6 items-center border border-border px-2 font-mono text-2xs text-muted hover:bg-hover hover:text-fg"
        >
          {t("cancel")}
        </button>
      </span>
    </div>
  );
}

/** What the switcher says about a machine besides its name: nothing while the page is live on it. */
function switcherNote(info: MachineInfo, t: (key: string, opts?: Record<string, unknown>) => string): string | null {
  if (info.state === "offline") {
    return info.copyAsOf ? t("offline copy · as of {{time}}", { time: formatTime(new Date(info.copyAsOf).toISOString()) }) : t("device offline");
  }
  if (info.live === "live") return null;
  if (info.live === "stale") return t("waiting for another of your devices to confirm");
  if (info.live === "refused") return t("this device does not accept control from this browser");
  if (info.live === "connecting") return t("connecting…");
  return t(info.state);
}

/**
 * The bar's switch between the account's machines: every one of them, with
 * whether it is up, and the one in view checked. Picking another brings it
 * into view in place — the app leaves the old machine's page for the same
 * section on the new one — and nothing reloads.
 */
export function MachineSwitcher({ shell, current }: { shell: HubShell; current: MachineKey }) {
  const { t } = useTranslation();
  const machines = useShellMachines();
  const home = useHomeMachine();
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  // 「设为主设备」 asks first: the screens are the main device's, the main device is the
  // account's, and the old one's screens go out of sight on every device of it.
  const [confirming, setConfirming] = useState(false);
  // With one device the switch has nowhere to go: it is there only to say a second one can be added (until put away).
  const hint = useAddDeviceHint();
  if (machines.length < 2 && !hint.shown) return null;
  const nameOf = (key: MachineKey) => machines.find((m) => m.key === key)?.name ?? key;
  // To the other device's page of the same section: an address of its own
  // (`/m/<device>/…`), so Back comes back here; the shell follows the address.
  const pick = (key: MachineKey) => {
    setOpen(false);
    if (key === current) return;
    void navigate({ href: addressOn(key, `${location.pathname}${location.search}`, home) });
  };
  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setConfirming(false);
      }}
    >
      <PopoverTrigger
        data-testid="machine-switcher"
        aria-label={t("switch device")}
        title={t("switch device")}
        className="flex h-5 shrink-0 items-center px-1 text-muted hover:bg-hover hover:text-fg"
      >
        <ChevronsUpDown className="size-3" />
      </PopoverTrigger>
      <PopoverContent side="bottom" className="w-80 max-w-[calc(100vw-2rem)] p-1">
        <ul data-testid="machine-switcher-list" className="flex flex-col">
          {machines.map((info) => {
            const note = switcherNote(info, t);
            return (
              <li key={info.key}>
                <button
                  type="button"
                  data-machine={info.key}
                  aria-current={info.key === current ? "true" : undefined}
                  onClick={() => pick(info.key)}
                  className="flex h-7 w-full items-center gap-2 px-2 text-left font-mono text-2xs text-fg hover:bg-hover"
                >
                  <DeviceDot state={info.state} title={null} />
                  <span className="max-w-[60%] shrink-0 truncate" title={info.key}>
                    {info.name}
                  </span>
                  {info.key === home && <span data-main-device="" className="shrink-0 border border-border px-1 text-[10px] leading-3.5 text-muted">{t("main device")}</span>}
                  <span title={note ?? undefined} className={cn("min-w-0 flex-1 truncate text-right", info.state === "offline" ? "text-danger" : "text-faint")}>
                    {note}
                  </span>
                  <Check className={cn("size-3 shrink-0", info.key === current ? "text-fg" : "invisible")} />
                </button>
              </li>
            );
          })}
        </ul>
        {machines.length < 2 && (
          <AddDeviceHint
            variant="line"
            className="mt-1 border-t border-border px-2 pt-2 pb-1"
            onAdd={() => {
              setOpen(false);
              openAddDevice();
            }}
          />
        )}
        {current !== home && !confirming && (
          <button
            type="button"
            data-testid="set-main-device"
            title={t("The main device keeps your screens and layouts.")}
            onClick={() => setConfirming(true)}
            className="mt-1 flex h-7 w-full items-center border-t border-border px-2 text-left font-mono text-2xs text-muted hover:bg-hover hover:text-fg"
          >
            {t("set as main device")}
          </button>
        )}
        {current !== home && confirming && (
          <MainDeviceConfirm
            next={nameOf(current)}
            old={nameOf(home)}
            onConfirm={() => {
              setOpen(false);
              setConfirming(false);
              shell.setMain(current);
            }}
            onCancel={() => setConfirming(false)}
          />
        )}
      </PopoverContent>
    </Popover>
  );
}
