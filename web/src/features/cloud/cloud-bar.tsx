import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "@tanstack/react-router";
import { Check, ChevronsUpDown, CloudOff, KeyRound, LayoutGrid, LogOut } from "lucide-react";
import { Button } from "@/components/ui/button";
import { BlobTransfers } from "@/components/blob-transfers";
import { cn } from "@/lib/cn";
import { formatExactTime, formatTime } from "@/lib/format";
import type { PathStats } from "@/lib/e2e";
import type { PresenceState, LinkState } from "@/lib/hub/link";
import type { ChannelRefusal, ChannelRoute, MachineChannel, MachineStatus } from "@/lib/hub/machine";
import type { RelayPool } from "@/lib/hub/relays";
import type { MachineKey } from "@/lib/machines/key";
import type { MachineInfo } from "@/lib/machines/runtime";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import type { CloudMode } from "./cloud-transport";
import {
  type CopyView,
  useAgentKeyState,
  useChannelStatus,
  useCloudMode,
  useCopyView,
  useHubOnly,
  useLinkState,
  useMachinePresence,
  useRelayPool,
} from "./context";
import { ChainAlerts, PendingRecoveryBanner } from "./devices";
import { AccountButton, DevicesButton, openAccount, openMyDevices, removePasskeyThere } from "./my-devices";
import { PendingPasskeyBanner } from "./passkey-join";
import { cityName, noticeText } from "./relays";
import { BarMachineName } from "./machine-name";
import { RegisteredDeviceAlerts, SignInChangeAlerts, storageFullText, useMe } from "./plan";
import { CompatBadge } from "./browser-support";
import { storageView } from "@/lib/hub/plan";
import { signOut } from "./sign-out";
import { addressOn, type HubMachine, type HubShell, hubShell, useFocusedMachine, useShellMachines } from "./hub-shell";
import { openAddDevice } from "./onboarding";
import { AddDeviceHint, useAddDeviceHint } from "./onboarding-hints";
import { useRouteMachine } from "@/app/machine-route";
import { useHomeMachine } from "@/lib/machines/context";

/**
 * In cloud mode, a thin strip above the app: which machine it is about,
 * whether that machine is online, how this page reaches it, a switch to any
 * other machine of the account (no reload: `hub-shell.ts`), and the one way
 * to 「我的设备」 (`my-devices.tsx`). Renders nothing in local mode.
 *
 * When the machine cannot be reached the app shows its offline copy
 * (`offline-transport.ts`), read-only, and a notice under the strip says so
 * and how old the copy is. Only a device with no copy at all is covered
 * ("no offline copy yet") until the machine is back.
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

/** What the strip says about the page's way to the machine. */
export function barState(input: {
  link: LinkState;
  presence: PresenceState | null;
  channel: MachineStatus;
  mode: CloudMode;
  copy: CopyView;
  /** How the device last refused this browser's channel, if it did (`MachineChannel.refusal`). */
  refusal?: ChannelRefusal["kind"] | null;
}): { label: "hub" | "offline" | "e2e" | "connecting" | "refused" | "unconfirmed" | "unsaved"; notice: boolean; cover: boolean } {
  const { link, presence, channel, mode, copy, refusal = null } = input;
  const online = presence === "online";
  // A live channel needs no words, whatever the hub's connection is doing: it
  // goes through a relay (relay-and-plans §2.9).
  const live = channel === "open" && mode === "live";
  // Online and answering, but not taking this browser: say why, rather than
  // "connecting…" on every retry (the refusal outlives them).
  const refused = !live && link === "open" && online && refusal !== null;
  const label = live ? "e2e" : link !== "open" ? "hub" : !online ? "offline" : refused ? (refusal === "notAccepted" ? "refused" : refusal === "unsaved" ? "unsaved" : "unconfirmed") : "connecting";
  // The machine is out of reach (the hub says so, or the channel keeps
  // failing) — rather than merely still being connected to, or reached and
  // refusing this browser (which `RefusalNotice` explains instead).
  const unreachable = (link === "open" && presence !== null && !online) || channel === "offline" || (channel === "error" && !refused) || (link !== "open" && mode === "offline");
  return {
    label,
    notice: mode === "offline" && copy.hasCopy && unreachable,
    cover: mode === "offline" && copy.loaded && !copy.hasCopy && link === "open" && presence !== null && !online,
  };
}

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
  const online = presence === "online";
  const refusal = machine.channel.refusal;
  const state = barState({ link, presence, channel, mode, copy, refusal: refusal?.kind ?? null });
  // A full cloud storage: the hub took no more of this machine's copy after its last write (§3.8).
  const storage = storageView(me?.plan, me?.view?.machines ?? []);
  const storageFull = storage?.level === "full" ? { stoppedMs: me?.view?.machines.find((m) => m.id === machine.key)?.copy?.lastWriteMs ?? storage.lastWriteMs } : null;
  // Open and live needs no words: every path is end-to-end encrypted, so the
  // strip only says which path it is and how fast (`LivePathIndicator`).
  const label = {
    hub: t("connecting to the hub…"),
    offline: t("device offline"),
    e2e: null,
    connecting: t("connecting…"),
    refused: t("this device does not accept control from this browser"),
    unconfirmed: t("waiting for another of your devices to confirm"),
    unsaved: t("this device could not save who connects to it"),
  }[state.label];
  const refusalShown = state.label === "refused" || state.label === "unconfirmed" || state.label === "unsaved";
  return (
    <>
      <div data-cloud-bar="" className="flex h-7 shrink-0 items-center gap-2 border-b border-border bg-bg px-3 font-mono text-2xs">
        <span
          data-signal={!online && presence !== "unknown" ? "offline" : undefined}
          className={cn(
            "size-[7px] shrink-0",
            online ? "bg-fg" : presence === "unknown" ? "shadow-[inset_0_0_0_1.25px_var(--color-faint)]" : "bg-danger",
          )}
          title={presence ? t(presence) : t("unknown")}
        />
        <BarMachineName machine={machine.key} mirror={machine.mirror} channel={machine.channel} canRename={state.label === "e2e" && shell.account.isAdmin !== false} />
        <MachineSwitcher shell={shell} current={machine.key} />
        {label && (
          <span
            data-signal={state.label === "offline" ? "offline" : refusalShown ? "warn" : undefined}
            className={cn("flex items-center gap-1 truncate", state.label === "offline" ? "text-danger" : refusalShown ? "text-warn-fg" : "text-faint")}
          >
            {label}
          </span>
        )}
        {state.label === "e2e" && <LivePathIndicator channel={machine.channel} relays={shell.relays} />}
        <span className="flex-1" />
        <CompatBadge compact />
        <DevicesButton />
        <AccountButton />
        <button
          type="button"
          onClick={() => void signOut()}
          className="flex h-5 items-center gap-1 px-1.5 text-muted hover:bg-hover hover:text-fg"
          title={t("sign out")}
        >
          <LogOut className="size-3" />
        </button>
      </div>
      <PendingPasskeyBanner account={shell.account} link={shell.link} onRemovePasskey={removePasskeyThere} />
      <PendingRecoveryBanner account={shell.account} link={shell.link} />
      <ChainAlerts account={shell.account} link={shell.link} />
      <RegisteredDeviceAlerts account={shell.account} link={shell.link} />
      <SignInChangeAlerts account={shell.account} link={shell.link} onReview={() => openAccount()} />
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
      {state.notice && <OfflineNotice freshestMs={copy.freshestMs} storageFull={storageFull} />}
      {state.cover && <NoCopyCover onDevices={() => openMyDevices()} />}
      <BlobTransfers labelOf={(from) => (from && from !== machine.key ? (machines.find((m) => m.key === from)?.name ?? from) : null)} />
    </>
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
 * encrypted.
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
 */
export function OfflineNotice({ freshestMs, storageFull = null }: { freshestMs: number | null; storageFull?: { stoppedMs: number | null } | null }) {
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
          <span className="text-faint"> · </span>
          {t("read-only: sending and terminals need the device online")}
        </span>
      </span>
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
            const online = info.state === "online";
            return (
              <li key={info.key}>
                <button
                  type="button"
                  data-machine={info.key}
                  aria-current={info.key === current ? "true" : undefined}
                  onClick={() => pick(info.key)}
                  className="flex h-7 w-full items-center gap-2 px-2 text-left font-mono text-2xs text-fg hover:bg-hover"
                >
                  <span
                    data-signal={info.state === "offline" ? "offline" : undefined}
                    className={cn("size-[7px] shrink-0", online ? "bg-fg" : info.state === "unknown" ? "shadow-[inset_0_0_0_1.25px_var(--color-faint)]" : "bg-danger")}
                  />
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
