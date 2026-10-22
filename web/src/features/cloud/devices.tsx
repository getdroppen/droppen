import { type ReactNode, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, ArrowLeft, ArrowRight, ChevronRight, ClipboardPaste, ExternalLink, Hash, KeyRound, Loader2, MonitorSmartphone, Plus, RefreshCw, Server } from "lucide-react";
import { DOWNLOAD_PATH } from "@/features/landing/get-droppen";
import { osLabel, versionLabel } from "@/features/machines/device-facts";
import { useDeviceSessions } from "@/features/machines/device-sessions";
import { needsYou, recentSessions } from "@/features/machines/needs-you";
import { NeedsYouLedger } from "@/features/machines/needs-you-ledger";
import { type Accept, type ChainState, type DeviceRecord, hex, offerFromText } from "@/lib/e2e";
import { formatAsOf, formatExactTime, formatRelative } from "@/lib/format";
import { type ChainAlert, type CloudAccount, deviceFingerprint, hasRunAi, type HeldChange, hostsExposedBy, limitedDevice, type ReEnrollLine, type RotationReport } from "@/lib/hub/account";
import { hostRefusalText } from "@/api/errors";
import { chainV3Writable, type PairingSummary, type RegisteredDevice } from "@/lib/hub/api";
import { hubOrigin } from "@/lib/hub/config";
import type { MachineKeyCourier } from "@/lib/hub/deliver";
import type { HubLink, PresenceState } from "@/lib/hub/link";
import { deviceMirrorStore } from "@/lib/hub/mirror";
import { abandonCandidate, type Candidate, candidateFromOffer, runCodeInitiator } from "@/lib/hub/pairing";
import type { StatusState } from "@/lib/hub/streams";
import { uaLabel, uaLabelOf } from "@/lib/hub/ua-label";
import { useHomeMachine } from "@/lib/machines/context";
import { useStore } from "@/lib/machines/store";
import { cn } from "@/lib/cn";
import { setHubOnly } from "@/lib/transport/p2p";
import { atLimit, copyWarning, DEVICES_KEY, type LimitHit, limitOf, offlineSince, retentionOf } from "@/lib/hub/plan";
import { AddComputer, V3Pending, v3Blocker, type V3Blocker } from "./add-computer";
import { useAgentKeyState, useHubOnly, usePresenceVersion, useShellCopies } from "./context";
import { DeviceRow, type DeviceRowView, shortDeviceId } from "./device-row";
import { askedPlace } from "./devices-route";
import { hubShell, sessionPath, shellCourier, useFocusedMachine, useShellMachines } from "./hub-shell";
import { MachineNotes, noteRenamed, RenameMachineForm, renameThroughNewChannel, shownName, useMachineStatuses, useRenamed } from "./machine-name";
import { addAsked } from "./onboarding";
import { AddDeviceHint } from "./onboarding-hints";
import { RelaySection } from "./relays";
import { AlwaysConfirmSetting, EndPauseConfirm, PushRenewNotice, RenewalNotice, RenewalsAhead, RenewPanel, useRenewals } from "./renewal";
import { LimitNote, UsageSummary, useMe } from "./plan";
import { QrScanner } from "./qr";
import { RecoveryWords } from "./recovery";
import { danger, ErrorLine, errorText, field, Fingerprint, knownText, Note, primary, quiet, secondary, Tabs } from "./ui";
import { Button } from "@/components/ui/button";
import { CapsLabel } from "@/components/ui/caps-label";

export { DeviceRow, type DeviceRowView, ReEnrollWarning, type RowMenuItem, rowMenuItems } from "./device-row";

/*
 * 「我的设备」 on droppen.app, from an active device (architecture doc §1;
 * docs/local-aggregator.md §3.7), one page (`my-devices.tsx` draws the frame:
 * the alerts, the way back): what the plan uses (`UsageSummary`), this
 * device's own settings (`ThisDeviceSection`), then every device the chain
 * lists in one list — this one marked, the devices that run AI among them
 * with the way into each, one waiting after a recovery; removed ones folded
 * below (`device-row.tsx` draws a row).
 *
 * From here: adding a device (a browser or phone: paste its text, scan its QR
 * code, or type the code it shows — `lib/hub/pairing.ts`; a device that runs
 * AI: `AddComputer`); removing one, which retires the keys it held (a new
 * user key epoch for the remaining devices, a new machine key per machine —
 * `CloudAccount.removeDevice`); rotating the keys again (offered first when a
 * removal was not followed by new keys); approving or vetoing a device that
 * came back with the recovery words; and new recovery words when the account
 * has none (any more).
 *
 * Every page of an active device also carries the account's alerts
 * (`ChainAlerts`): each device added or removed, recovery join and new
 * recovery words that another device made, until this one says "that was me".
 */

const versions = new WeakMap<CloudAccount, number>();

/** Re-render when the account's chain or keys change. */
export function useAccountVersion(account: CloudAccount | null | undefined): number {
  return useSyncExternalStore(
    (notify) =>
      account
        ? account.onChange(() => {
            versions.set(account, (versions.get(account) ?? 0) + 1);
            notify();
          })
        : () => undefined,
    () => (account ? (versions.get(account) ?? 0) : 0),
    () => 0,
  );
}

function useFingerprints(devices: DeviceRecord[]): Map<string, string> {
  const [prints, setPrints] = useState(new Map<string, string>());
  // The ids, and the keys: a renewed identity key is a new fingerprint for the same id.
  const key = devices.map((d) => `${d.info.id}:${d.info.signKey ? hex(d.info.signKey.slice(0, 8)) : ""}`).join(",");
  useEffect(() => {
    let live = true;
    void Promise.all(devices.map(async (d) => [d.info.id, await deviceFingerprint(d.info.signKey, d.info.dhKey)] as const)).then((list) => {
      if (live) setPrints(new Map(list));
    });
    return () => {
      live = false;
    };
    // the ids say whether the list changed
  }, [key]);
  return prints;
}

/** What 「我的设备」 shows besides the list: one thing at a time. */
export type Section =
  | { kind: "list" }
  /** 「添加设备」: which kind, a browser or phone or a device that runs AI. */
  | { kind: "add" }
  | { kind: "addBrowser" }
  | { kind: "addComputer" }
  | { kind: "remove"; device: string }
  /** Making a limited device a full device: confirmed first (§3.7). */
  | { kind: "grant"; device: string }
  /** Stopping AI on a computer: confirmed first (review 14). */
  | { kind: "stopHost"; device: string }
  /** 「谁可以控制这台电脑」 of a host. */
  | { kind: "control"; host: string }
  /** "I'm sure it never connected": a re-enroll warning about `culprit` on host `device` dropped, confirmed first (review D3). */
  | { kind: "dismissReEnroll"; device: string; culprit: string; lostSeq: number }
  /** Renewing a host's identity key now (docs/host-identity-rotation.md §4.4); `fixed`: only confirmed on the device. */
  | { kind: "renew"; device: string; fixed: boolean }
  /** 「结束暂停」 of an open renewal pause on host `device`'s list, asked first (docs/host-identity-rotation.md §4.1). */
  | { kind: "endPause"; device: string; op: Uint8Array }
  | { kind: "words"; words: string[] };

/** A time of day ("14:02"), with the date when it is not today. */
export const asOfTime = formatAsOf;

/**
 * A square on/off switch (the drafting sheet has no round controls). A
 * switch that cannot move is `aria-disabled` rather than disabled, so its
 * tooltip still says why. `labelHidden`: the words beside it say what it is,
 * so its label is for screen readers only.
 */
export function SquareSwitch({
  on,
  blocked,
  title,
  label,
  onChange,
  testId,
  labelHidden = false,
}: {
  on: boolean;
  blocked: boolean;
  title?: string;
  label: string;
  onChange: (on: boolean) => void;
  testId?: string;
  labelHidden?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-disabled={blocked || undefined}
      aria-label={label}
      title={title}
      data-testid={testId}
      onClick={() => {
        if (!blocked) onChange(!on);
      }}
      className={cn("flex h-7 shrink-0 items-center gap-2 px-1 font-mono text-2xs pointer-coarse:h-9", blocked ? "cursor-not-allowed text-faint" : "text-muted hover:text-fg")}
    >
      <span aria-hidden="true" className={cn("relative inline-flex h-3.5 w-6 shrink-0 border", on ? "border-fg bg-fg" : "border-border-strong bg-bg", blocked && "opacity-40")}>
        <span className={cn("absolute top-[2px] size-2", on ? "left-[12px] bg-bg" : "left-[2px] bg-faint")} />
      </span>
      <span className={labelHidden ? "sr-only" : "max-sm:sr-only"}>{label}</span>
    </button>
  );
}

/** What this browser is, for its own row: a phone or a browser (the hub is not told). */
function useIsPhone(): boolean {
  return typeof navigator !== "undefined" && /Mobi|Android|iPhone|iPad/i.test(navigator.userAgent ?? "");
}

/** This page's own label ("Mac · Safari"; an iPad asking for the desktop site: "iPad · Safari"), as the hub would make it of its User-Agent. */
function ownLabel(): string | null {
  return typeof navigator === "undefined" ? null : uaLabel(navigator.userAgent, (navigator.maxTouchPoints ?? 0) > 1);
}

/**
 * Mailing a device every user key again (purpose 1, to its chain key) — for
 * one that missed a rotation's mail — from its row's menu, and how it went,
 * by device. It takes them only if it confirmed this account before; a device
 * that never did pairs again instead.
 */
type ResendState = { state: "busy" | "sent" | "error"; error?: string };

function useResend(account: CloudAccount): { states: ReadonlyMap<string, ResendState>; send: (device: string) => void } {
  const [states, setStates] = useState<ReadonlyMap<string, ResendState>>(new Map());
  const set = (device: string, state: ResendState) => setStates((prev) => new Map(prev).set(device, state));
  return {
    states,
    send: (device) => {
      // Not a device of the chain (any more): nothing to send it.
      if (!account.chain?.device(device)) return;
      set(device, { state: "busy" });
      // An admin: every user key; a device that is not one: the hosts' machine keys (S1-4: its device role).
      account.resendKeys(device).then(
        () => set(device, { state: "sent" }),
        (err: unknown) => set(device, { state: "error", error: errorText(err) }),
      );
    },
  };
}

/** How re-sending a device its keys went, on its row. */
function ResendLine({ state }: { state: ResendState | undefined }) {
  const { t } = useTranslation();
  if (!state) return null;
  if (state.state === "busy") {
    return (
      <span className="flex items-center gap-1.5 font-mono text-2xs text-muted">
        <Loader2 className="size-3 animate-spin" />
        {t("re-send keys")}
      </span>
    );
  }
  if (state.state === "sent") {
    return (
      <span data-testid="keys-sent" data-signal="ok" className="font-mono text-2xs text-ok">
        {t("keys sent")}
      </span>
    );
  }
  return (
    <span role="alert" data-signal="failed" className="font-mono text-2xs break-words text-danger">
      {t("could not send keys")}
      {state.error ? ` · ${state.error}` : ""}
    </span>
  );
}

export interface DevicesPanelProps {
  account: CloudAccount;
  link: HubLink;
  /** The hub's origin, for the 「添加可运行 AI 的设备」 guide's one-time code. */
  origin?: string;
  /** Open a device that runs AI (the app on it). */
  onSelect?: (machine: string) => void;
  /** Open a device that runs AI on one of its pages (a session, its terminals). */
  onOpenAt?: (machine: string, path: string) => void;
}

/**
 * 「我的设备」: what the plan uses, this device's own settings, every device
 * of the account as one list — the ones that run AI among them, each with
 * the way into it — and what can be done with them.
 */
export function DevicesPanel({ account, link, origin, onSelect, onOpenAt }: DevicesPanelProps) {
  const { t } = useTranslation();
  useAccountVersion(account);
  usePresenceVersion(link);
  useRenamed();
  const me = useMe(account, link);
  const [section, setSection] = useState<Section>({ kind: "list" });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  const resend = useResend(account);
  const phone = useIsPhone();
  const shell = hubShell();
  // The relay picker's machine: the one in view, or the one the app last showed (「我的设备」 is out of the app).
  const inView = useFocusedMachine() ?? shell?.lastShown ?? null;
  const chain = account.chain;
  // Everyone the chain names: browsers, phones and the devices that run AI, in one list.
  const devices = chain?.allDevices() ?? [];
  const prints = useFingerprints(devices);
  const hostIds = devices.filter((d) => d.host && d.status.type === "active").map((d) => d.info.id);
  const statuses = useMachineStatuses(account, link, deviceMirrorStore(), hostIds, useShellCopies());
  // What the page follows of each device that runs AI (its info, its session list off its copy): nothing here dials it.
  const machines = useShellMachines();
  const lists = useDeviceSessions();
  const needs = useMemo(() => needsYou(lists), [lists]);
  const home = useHomeMachine();
  const viewerAdmin = account.isAdmin;
  // What is written follows the chain (review D4): v3 only once it holds a v3 entry; the hub's say gates the upgrade and new hosts.
  const upgraded = chain?.isV3 === true;
  const writable = chainV3Writable(me?.view);
  // Hosts whose identity a device that could control them may have kept (review D3), each device with what the host's status says of it —
  // those that cannot renew their identity key; the others renew it (docs/host-identity-rotation.md §4).
  const reEnroll = viewerAdmin ? account.reEnrollWarnings((h) => statuses.get(h)?.controllers) : new Map<string, ReEnrollLine[]>();
  const renewals = useRenewals();
  // The hosts no other host may be made to accept until they renewed their identity key (§7 decision 8).
  const mustRenew = hostIds.filter((h) => account.mustRenewFirst?.(h) === true);
  // What the last confirmed change rotated, if it did (a narrowed 「谁可以控制它」: review D2).
  const [done, setDone] = useState<{ title: string; report: RotationReport } | null>(null);
  // A device made a full device whose keys were not mailed: said on its row, with "re-send keys".
  const [keysNotSent, setKeysNotSent] = useState<{ device: string; why: string } | null>(null);
  // A hint elsewhere on the page asked for the 「添加可运行 AI 的设备」 guide (`openAddDevice`): open it here.
  const asked = useStore(addAsked);
  useEffect(() => {
    if (!asked) return;
    addAsked.set(false);
    if (account.isAdmin) setSection({ kind: "addComputer" });
  }, [asked, account]);

  if (section.kind === "words") {
    // A page of its own, over 「我的设备」 until they are written down.
    return (
      <div className="fixed inset-0 z-50 bg-bg">
        <RecoveryWords words={section.words} title={t("your new recovery words")} onDone={() => setSection({ kind: "list" })} />
      </div>
    );
  }

  const run = async (work: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await work();
      me?.soon();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  const newWords = async () => {
    setBusy(true);
    setError(null);
    try {
      setSection({ kind: "words", words: await account.setRecoveryWords() });
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  const courier = () => shellCourier(link, account);
  const open = (id: string) => (onSelect ? onSelect(id) : shell?.select(id));
  const openAt = (id: string, path: string) => (onOpenAt ? onOpenAt(id, path) : shell?.openAt(id, path));
  const listOf = new Map(lists.map((l) => [l.machine, l]));
  const view = (d: DeviceRecord): DeviceRowView => {
    const id = d.info.id;
    const self = id === account.deviceId;
    const live = link.presence.get(id) ?? null;
    const seenMachine = me?.view?.machines.find((m) => m.id === id) ?? null;
    const seenDevice = me?.view?.devices.find((x) => x.id === id) ?? null;
    const presence: PresenceState | null = self ? "online" : d.host ? (live?.state ?? "unknown") : null;
    const lastSeen = [seenMachine?.lastSeenMs ?? null, seenDevice?.lastSeenMs ?? null].filter((x): x is number => x !== null);
    // It ran AI, as the chain judges it (D1 extended): never offered the account.
    const ranAi = chain ? hasRunAi(chain, d) : d.ranAi;
    const host = d.host && d.status.type === "active";
    const limited = limitedDevice({ viewer: account.deviceId, viewerAdmin, device: d, upgraded, ranAi });
    const info = host ? (machines.find((m) => m.key === id) ?? null) : null;
    const list = host ? (listOf.get(id) ?? null) : null;
    const rows = list?.rows ?? null;
    const status = host ? (statuses.get(id) ?? null) : null;
    const hostname = typeof status?.settings?.hostname === "string" && status.settings.hostname ? status.settings.hostname : null;
    return {
      id,
      fingerprint: prints.get(id) ?? "",
      // A device that runs AI goes by its own name; a browser or phone by what the hub made of its User-Agent (only its words: `uaLabelOf`).
      // One waiting for approval gets none: a device coming back with the recovery words chose its own User-Agent (`deviceName`).
      name: d.host ? shownName(id, statuses.get(id) ?? null) : d.status.type === "pending" ? null : (uaLabelOf(seenDevice?.uaLabel) ?? (self ? ownLabel() : null)),
      self,
      kind: d.host || ranAi ? "computer" : self ? (phone ? "phone" : "browser") : "browserOrPhone",
      status: d.status.type,
      host,
      stoppedHost: !d.host && d.status.type === "active" && ranAi,
      full: d.admin && !d.host && !ranAi,
      main: host && id === home,
      presence,
      path: live?.via?.[0] ? { relay: live.via[0].relay, rttMs: live.via[0].rttMs } : null,
      copyAsOf: seenMachine?.copy?.lastWriteMs ?? null,
      lastSeenMs: lastSeen.length ? Math.max(...lastSeen) : null,
      limited,
      accept: d.host && chain ? chain.acceptPolicy(id) : null,
      reEnroll: d.host ? reEnroll.get(id) : undefined,
      renewal: d.host && renewals ? renewals.status(id) : undefined,
      renewable: d.host && upgraded && account.renews?.(id) === true,
      renewalsOn: d.host && renewals?.view?.enabled === true,
      renewFirst: d.host ? (account.renewFirst?.(id) ?? []) : undefined,
      isolatedControllers: d.host && renewals ? renewals.isolatedControllers(id) : undefined,
      retiredLoginMs: d.host && renewals ? renewals.retiredLoginAt(id) : undefined,
      pausedWith: d.host && renewals ? renewals.pausedWith(id) : undefined,
      // The pause held apart from the list (its base): what is paused, and the pauses on it.
      pausedControllers: d.host && chain ? (chain.pausedEdges?.(id) ?? []) : undefined,
      pauses: d.host && renewals ? renewals.pausesOf(id) : undefined,
      ...(host
        ? {
            facts: { os: osLabel(info?.os), agent: versionLabel(info?.agentVersion), agentFull: info?.agentVersion ?? null, hostname, keyEpoch: chain?.latestMachineEpoch(id) ?? null },
            attention: rows ? (needs.byMachine.get(id) ?? null) : null,
            recent: recentSessions(rows, 3),
            hasCopy: rows !== null,
          }
        : {}),
    };
  };

  // This device first, then the devices that run AI, then the other browsers and phones; one waiting for approval last. Removed ones go below, folded.
  const rank = (d: DeviceRecord) => (d.info.id === account.deviceId ? 0 : d.status.type === "pending" ? 3 : d.host ? 1 : 2);
  const current = devices.filter((d) => d.status.type !== "removed").sort((a, b) => rank(a) - rank(b));
  const removed = devices.filter((d) => d.status.type === "removed");
  const hostsNow = hostIds.length;
  // No device that runs AI yet (just signed up): adding one is what comes next.
  const firstHost = hostsNow === 0;
  const selfRecord = devices.find((d) => d.info.id === account.deviceId);
  const selfName = selfRecord ? view(selfRecord).name : ownLabel();

  const row = (d: DeviceRecord) => {
    const v = view(d);
    const id = d.info.id;
    // What its controls open and what each confirmation writes (review 14): `rowActions`, `RowConfirm`.
    const acts = rowActions({ account, id, writable, courier, run, setSection, onKeysNotSent: (why) => setKeysNotSent({ device: id, why }) });
    const held = blockedByHold(account, id);
    return (
      <DeviceRow
        key={id}
        view={v}
        viewerAdmin={viewerAdmin}
        upgraded={upgraded}
        writable={writable}
        busy={busy}
        held={held}
        resending={resend.states.get(id)?.state === "busy"}
        onPromote={acts.onPromote}
        onRemove={() => setSection({ kind: "remove", device: id })}
        onReEnroll={() => setSection({ kind: "remove", device: id })}
        onDismissReEnroll={(culprit, lostSeq) => setSection({ kind: "dismissReEnroll", device: id, culprit, lostSeq })}
        onWarnAgain={(culprit) => account.warnAgainReEnroll(id, culprit)}
        onHost={acts.onHost}
        onControl={() => setSection(section.kind === "control" && section.host === id ? { kind: "list" } : { kind: "control", host: id })}
        onRename={d.host ? () => setRenaming(renaming === id ? null : id) : undefined}
        onTerminal={v.host ? () => openAt(id, "/terminals") : undefined}
        onResend={() => resend.send(id)}
        onRenew={renewals ? () => setSection({ kind: "renew", device: id, fixed: renewals.needsCode(id) !== null || account.isolated?.(id) === true }) : undefined}
        onRenewCode={renewals ? () => setSection({ kind: "renew", device: id, fixed: true }) : undefined}
        onRenewRetry={renewals ? () => void run(() => renewals.renew(id, { assurance: 0 })) : undefined}
        onAddComputer={() => setSection({ kind: "addComputer" })}
        onEndPause={(op) => setSection({ kind: "endPause", device: id, op })}
        onOpen={v.host ? () => open(id) : undefined}
        onOpenSession={v.host ? (s) => openAt(id, sessionPath(s.source, s.id)) : undefined}
        notes={v.host ? <HostNotes account={account} link={link} id={id} status={statuses.get(id) ?? null} /> : null}
        keys={<ResendLine state={resend.states.get(id)} />}
      >
        {section.kind === "endPause" && section.device === id && (
          <div className="border-b border-border px-6 py-4 max-md:px-3">
            <div className="max-w-xl">
              {(() => {
                // Exactly what is shown is what is asked for (M7): if ending it would give back anything else by then, it is
                // refused, and this shows the new set to confirm again.
                const ending = account.pauseEnding?.(section.op) ?? { edges: [], all: [], held: [], orphan: false };
                return (
                  <EndPauseConfirm
                    ending={ending}
                    busy={busy}
                    error={error}
                    onCancel={() => setSection({ kind: "list" })}
                    onConfirm={() =>
                      void run(async () => {
                        await account.endPause(section.op, ending.edges);
                        setSection({ kind: "list" });
                      })
                    }
                  />
                );
              })()}
            </div>
          </div>
        )}
        {section.kind === "renew" && section.device === id && renewals && (
          <div className="border-b border-border px-6 py-4 max-md:px-3">
            <div className="max-w-xl">
              <RenewPanel host={id} runner={renewals} fixed={section.fixed} onDone={() => setSection({ kind: "list" })} />
            </div>
          </div>
        )}
        {keysNotSent?.device === id && (
          <KeysNotSent
            device={id}
            why={keysNotSent.why}
            resend={
              <button type="button" className={cn(quiet, "h-7 px-2")} disabled={resend.states.get(id)?.state === "busy"} onClick={() => resend.send(id)}>
                {t("re-send keys")}
              </button>
            }
            onClose={() => setKeysNotSent(null)}
          />
        )}
        {renaming === id && (
          <div className="border-b border-border px-6 py-3 max-md:px-3">
            <RenameMachineForm
              initial={v.name ?? ""}
              onRename={async (next) => noteRenamed(id, await renameThroughNewChannel(link, account, id, next, shell?.relays), statuses.get(id) ?? null)}
              onDone={() => setRenaming(null)}
            />
          </div>
        )}
        {"device" in section && section.kind !== "renew" && section.kind !== "endPause" && section.device === id && <RowConfirm section={section} id={id} busy={busy} actions={acts} onCancel={() => setSection({ kind: "list" })} />}
        {section.kind === "dismissReEnroll" && section.device === id && (
          <DismissReEnroll
            host={id}
            device={section.culprit}
            onCancel={() => setSection({ kind: "list" })}
            onConfirm={() => {
              account.dismissReEnroll(id, section.culprit, section.lostSeq);
              setSection({ kind: "list" });
            }}
          />
        )}
        {section.kind === "control" && section.host === id && chain && (
          <HostControl
            host={id}
            policy={chain.baseAccept?.(id) ?? chain.acceptPolicy(id)}
            paused={chain.pausedEdges?.(id) ?? []}
            declaresPolicy={me?.view?.machines.find((m) => m.id === id)?.readsV3}
            candidates={devices
              .filter((x) => x.status.type === "active" && !x.admin && x.info.id !== id)
              .map((x) => ({ id: x.info.id, host: x.host, limited: limitedDevice({ viewer: account.deviceId, viewerAdmin, device: x, upgraded, ranAi: hasRunAi(chain, x) }) !== null }))}
            upgraded={upgraded}
            mustRenew={mustRenew}
            renewable={upgraded && account.renews?.(id) === true}
            renewsIfNarrowed={hostsExposedBy(chain, "", [id]).filter((h) => account.renews?.(h) === true)}
            busy={busy}
            onCancel={() => setSection({ kind: "list" })}
            onSave={(from) =>
              void run(async () => {
                // A narrowing hands the computer a new key (review D2): the courier takes it.
                const change = await account.setAccept(id, from, courier());
                if (change.narrowed && change.mkEpoch !== undefined) {
                  // A new user key came with it if the newest was stale (a device that held it is no longer a full device).
                  const uk = change.userKey;
                  setDone({
                    title: t("{{host}} has a new content key", { host: id }),
                    report: {
                      ukEpoch: uk?.epoch ?? null,
                      sealedTo: uk?.sealedTo ?? [],
                      failedTo: uk?.failedTo ?? [],
                      failedWhy: uk?.failedWhy ?? {},
                      recovery: uk?.recovery ?? false,
                      machines: [{ machine: id, mkEpoch: change.mkEpoch, result: change.result }],
                      hostKeysTo: change.hostKeysTo ?? [],
                    },
                  });
                }
                setSection({ kind: "list" });
              })
            }
          />
        )}
        {section.kind === "remove" && section.device === id && (
          <div className="border-b border-border px-6 py-4 max-md:px-3">
            <div className="max-w-xl">
              <RemoveDevice
                account={account}
                link={link}
                device={id}
                record={d}
                fingerprint={prints.get(id) ?? ""}
                onDone={() => setSection({ kind: "list" })}
                onAddAgain={d.host ? () => setSection({ kind: "addComputer" }) : undefined}
              />
            </div>
          </div>
        )}
      </DeviceRow>
    );
  };

  const adding = section.kind === "add" || section.kind === "addBrowser" || section.kind === "addComputer";
  return (
    <>
      <div className="flex flex-wrap items-end justify-between gap-x-6 gap-y-4 border-b border-border px-6 pt-8 pb-5 max-md:px-4 max-md:pt-6">
        <div className="flex min-w-0 flex-col gap-1.5">
          {/* The counts are the summary's, under it: the title names the page. */}
          <h1 data-testid="devices-title" className="text-[32px] leading-[1.1] font-semibold tracking-[-0.03em] text-fg max-md:text-[26px]">
            {t("my devices")}
          </h1>
          <p className="text-xs leading-5 text-muted">{t("Every device holds its own keys. Compare fingerprints when you add one.")}</p>
          {firstHost && viewerAdmin && <FirstHostLine />}
        </div>
        {viewerAdmin && !adding && (
          // One primary action a view: while a rotation is due, that is it.
          <button
            type="button"
            data-variant={account.rotationDue ? "secondary" : "primary"}
            data-testid="add-device"
            className={cn(account.rotationDue ? secondary : primary, "max-md:w-full")}
            onClick={() => setSection(firstHost ? { kind: "addComputer" } : { kind: "add" })}
          >
            <Plus className="size-3.5" />
            {t("add a device")}
          </button>
        )}
      </div>
      {section.kind === "add" && <AddChooser onPick={(kind) => setSection({ kind })} onCancel={() => setSection({ kind: "list" })} />}
      {section.kind === "addBrowser" && (
        <section data-testid="add-browser-section" className="border-b border-border">
          <AddHead title={t("add a browser or phone")} onBack={() => setSection({ kind: "add" })} />
          <div className="flex max-w-xl flex-col gap-3 px-6 py-5 max-md:px-4">
            <AddDevice account={account} link={link} onDone={() => setSection({ kind: "list" })} />
          </div>
        </section>
      )}
      {section.kind === "addComputer" && (
        <section data-testid="add-computer-section" className="border-b border-border">
          <AddHead title={t("add a device that runs AI")} onBack={() => setSection({ kind: "add" })} />
          {/* Two ways to add (§3.7): the device's own local settings first, a one-time code second. */}
          <AddComputer
            account={account}
            link={link}
            origin={origin ?? hubOrigin() ?? ""}
            onDone={() => setSection({ kind: "list" })}
            pairWith={(done) => <AddDevice account={account} link={link} onDone={done} />}
          />
        </section>
      )}
      <UsageSummary account={account} link={link} />
      <ThisDeviceSection
        name={selfName}
        relays={
          shell?.relays ? (
            <>
              <RelayStandbySetting account={account} link={link} />
              <RelaySection pool={shell.relays} machine={inView} channel={shell.machine(inView)?.channel} bare />
            </>
          ) : null
        }
      />
      <section data-testid="devices-section" className="max-md:pb-2">
        <div className="flex h-10 items-center gap-2 border-b border-border px-6 max-md:border-b-0 max-md:px-4">
          <CapsLabel className="text-fg">{t("devices")}</CapsLabel>
          <span className="font-mono text-2xs text-faint tabular-nums">{current.length}</span>
        </div>
        <ul data-testid="device-list" className="max-md:flex max-md:flex-col max-md:gap-2 max-md:px-4">
          {current.map(row)}
        </ul>
      </section>
      {section.kind === "list" && viewerAdmin && upgraded && renewals?.view?.enabled && <AlwaysConfirmSetting runner={renewals} account={account} />}
      {section.kind === "list" && done && (
        <div data-testid="change-report" data-signal="ok" className="mx-6 my-4 flex flex-col gap-1 border border-border p-3 shadow-[inset_2px_0_0_var(--color-ok)] max-md:mx-4">
          <span className="font-mono text-xs text-ok">{done.title}</span>
          {reportLines(done.report, t).map((line, i) => (
            <span key={i} className={cn("font-mono text-2xs", alarmLine(done.report, line) ? "text-danger" : "text-muted")} data-signal={alarmLine(done.report, line) ? "alarm" : undefined}>
              {line}
            </span>
          ))}
          <Button variant="ghost" className="self-start" onClick={() => setDone(null)}>
            {t("done")}
          </Button>
        </div>
      )}
      {/* One device that runs AI: a second one puts both on one screen (put away with its ×). */}
      {!adding && <AddDeviceHint variant="row" onAdd={() => setSection({ kind: "addComputer" })} className="max-md:mt-2 max-md:border-t" />}
      {needs.entries.length > 0 && (
        <NeedsYouLedger
          className="border-t border-border max-md:mt-2"
          needs={needs}
          nameOf={(key) => shownName(key, statuses.get(key) ?? null) ?? t("device {{id}}", { id: shortDeviceId(key) })}
          devices
          note={t("from each device's encrypted index")}
          onOpen={(entry) => openAt(entry.ref.machine, sessionPath(entry.ref.source, entry.ref.id))}
        />
      )}
      <div className="flex max-w-xl flex-col gap-3 px-6 py-6 max-md:px-4">
        {viewerAdmin && !account.rotationDue && section.kind === "list" && <RotateKeys account={account} link={link} due={false} />}
        {viewerAdmin && chain && !chain.currentRecoveryKey && section.kind === "list" && (
          <Note tone="warn">
            <span data-signal="warn" className="flex items-center gap-1.5 font-mono text-xs text-warn-fg">
              <KeyRound className="size-3.5" />
              {t("this account has no recovery words")}
            </span>
            <p className="text-xs leading-5 text-muted">
              {t("Without them, losing every device means losing everything encrypted. (Words are cleared when the device that made them, or one that came back with them, is removed.)")}
            </p>
            <button type="button" className={cn(secondary, "self-start")} disabled={busy} onClick={() => void newWords()}>
              {busy && <Loader2 className="size-3.5 animate-spin" />}
              {t("create new recovery words")}
            </button>
          </Note>
        )}
        <ErrorLine message={error} />
      </div>
      {removed.length > 0 && (
        <details data-testid="removed-devices" className="group/removed border-t border-border">
          <summary className="flex h-10 cursor-pointer list-none items-center gap-2 px-6 select-none hover:bg-hover max-md:px-4 [&::-webkit-details-marker]:hidden">
            <ChevronRight aria-hidden="true" className="size-3 text-muted transition-transform group-open/removed:rotate-90" />
            <CapsLabel>{t("removed devices ({{n}})", { n: removed.length })}</CapsLabel>
          </summary>
          <ul className="border-t border-border max-md:flex max-md:flex-col max-md:gap-2 max-md:border-t-0 max-md:px-4 max-md:pb-4">{removed.map(row)}</ul>
        </details>
      )}
    </>
  );
}

/** The head of an add flow: what is being added, and the way back to the choice. */
function AddHead({ title, onBack }: { title: string; onBack: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="flex h-10 items-center gap-3 border-b border-border px-6 max-md:px-4">
      <button type="button" onClick={onBack} title={t("back")} aria-label={t("back")} className="flex size-6 items-center justify-center text-muted hover:bg-hover hover:text-fg pointer-coarse:size-9">
        <ArrowLeft className="size-3.5" />
      </button>
      <CapsLabel className="text-fg">{title}</CapsLabel>
    </div>
  );
}

/** 「添加设备」: a browser or phone (paired from here), or a device that runs AI (its guide). */
export function AddChooser({ onPick, onCancel }: { onPick: (kind: "addBrowser" | "addComputer") => void; onCancel: () => void }) {
  const { t } = useTranslation();
  const choices = [
    { kind: "addBrowser" as const, testId: "add-browser", Icon: MonitorSmartphone, title: t("a browser or phone"), body: t("Sign in to this account on it, then scan its QR code, paste its text or type the code it shows.") },
    { kind: "addComputer" as const, testId: "add-computer", Icon: Server, title: t("a device that runs AI"), body: t("The device your AI CLIs run on, with Droppen installed: your sessions run there, and you open it from here.") },
  ];
  return (
    <section data-testid="add-chooser" className="border-b border-border">
      <div className="flex h-10 items-center justify-between gap-3 border-b border-border px-6 max-md:px-4">
        <CapsLabel className="text-fg">{t("add a device")}</CapsLabel>
        <button type="button" className={cn(quiet, "h-7 px-2")} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
      <div className="grid grid-cols-2 max-md:grid-cols-1">
        {choices.map(({ kind, testId, Icon, title, body }) => (
          <button
            key={kind}
            type="button"
            data-testid={testId}
            onClick={() => onPick(kind)}
            className="group flex items-start gap-3 border-border px-6 py-4 text-left first:border-r hover:bg-hover max-md:px-4 max-md:first:border-r-0 max-md:first:border-b"
          >
            <Icon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-muted group-hover:text-fg" />
            <span className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-[13px] font-medium text-fg">{title}</span>
              <span className="text-xs leading-5 text-muted">{body}</span>
            </span>
            <ArrowRight aria-hidden="true" className="mt-0.5 size-3.5 shrink-0 text-faint group-hover:text-fg" />
          </button>
        ))}
      </div>
    </section>
  );
}

/** What only droppen.app says of a device that runs AI, on its row: a chain alarm, an old key, its offline copy's retention. */
function HostNotes({ account, link, id, status }: { account: CloudAccount; link: HubLink; id: string; status: StatusState | null }) {
  const checked = useAgentKeyState(id);
  const me = useMe(account, link);
  const latest = account.chain?.latestMachineEpoch(id) ?? null;
  // The machine's own signed status says which epoch it uses; a check in this page says whether handing it the new one failed.
  const behind = latest !== null && ((typeof status?.mkEpoch === "number" && status.mkEpoch < latest) || checked === "behind");
  const seen = me?.view?.machines.find((m) => m.id === id) ?? null;
  const warning = me?.plan && seen ? copyWarning({ retentionDays: retentionOf(me.plan), offlineSinceMs: offlineSince(seen, link.presence.get(id) ?? null), copy: seen.copy, now: Date.now() }) : null;
  return <MachineNotes id={id} status={status} oldKey={behind} copyWarning={warning && me?.plan ? { warning, plan: me.plan.id } : null} />;
}

/**
 * 「这台设备」: the settings this browser keeps for itself — whether it may
 * connect straight to a device (faster; its IP address is seen), and which
 * relay carries it when it does not. A place for more of its own settings.
 */
export function ThisDeviceSection({ name, relays }: { name: string | null; relays?: ReactNode }) {
  const { t } = useTranslation();
  const asked = useStore(askedPlace);
  // The relay picker, folded until asked for (“change relay…” opens the page on it).
  const [relaysOpen, setRelaysOpen] = useState(() => askedPlace.get()?.anchor === "relays");
  useEffect(() => {
    if (asked?.anchor === "relays") setRelaysOpen(true);
  }, [asked]);
  return (
    <section data-testid="this-device" className="border-b border-border">
      <div className="flex h-10 items-center gap-2.5 border-b border-border px-6 max-md:px-4">
        <CapsLabel className="text-fg">{t("This device")}</CapsLabel>
        {name && <span className="min-w-0 truncate font-mono text-2xs text-muted">{name}</span>}
      </div>
      <DirectConnectionSetting />
      {relays && (
        <details open={relaysOpen} onToggle={(e) => setRelaysOpen(e.currentTarget.open)} className="group/relays border-t border-border">
          <summary className="flex min-h-10 cursor-pointer list-none items-center gap-2 px-6 py-2 select-none hover:bg-hover max-md:px-4 [&::-webkit-details-marker]:hidden">
            <ChevronRight aria-hidden="true" className="size-3 shrink-0 text-muted transition-transform group-open/relays:rotate-90" />
            <span className="text-xs font-medium text-fg">{t("Relays")}</span>
            <span className="min-w-0 truncate text-xs text-muted">{t("what carries a connection that is not direct")}</span>
          </summary>
          {relays}
        </details>
      )}
    </section>
  );
}

/** What comes after signing up: Droppen on the device the AI runs on, from the download page, then added here. */
export function FirstHostLine() {
  const { t } = useTranslation();
  return (
    <p data-testid="first-host" className="flex flex-wrap items-center gap-x-2 text-xs leading-5 text-fg">
      <span>{t("Next: install Droppen on the device your AI runs on, then add it here.")}</span>
      <a href={DOWNLOAD_PATH} target="_blank" rel="noopener" data-testid="first-host-download" className="inline-flex items-center gap-1 text-fg underline underline-offset-2 hover:no-underline">
        {t("download page")}
        <ExternalLink className="size-3" />
      </a>
    </p>
  );
}

/**
 * What the account needs said above 「我的设备」's list (`my-devices.tsx` draws
 * it at the top of the page): this browser is a limited device; the account
 * is still on chain v2, or waits for the hub; keys are due to be rotated; the
 * newest user key has not reached this device. A device that runs AI and
 * should be enrolled again says so on its own row.
 */
export function AccountNotices({ account, link }: { account: CloudAccount; link: HubLink }) {
  useAccountVersion(account);
  const me = useMe(account, link);
  const viewerAdmin = account.isAdmin;
  const upgraded = account.chain?.isV3 === true;
  const writable = chainV3Writable(me?.view);
  const blocker = v3Blocker(me?.view);
  const renewals = useRenewals();
  return (
    <>
      {!viewerAdmin && account.standing === "active" && <LimitedDeviceNote missing={account.missingHostKeys} upgraded={upgraded} />}
      {viewerAdmin && !upgraded && <UpgradeAccount account={account} writable={writable} blocker={blocker} waiting={me?.view?.chainV3?.waiting ?? []} onDone={() => me?.soon()} />}
      {viewerAdmin && upgraded && !writable && <V3Note blocker={blocker} waiting={me?.view?.chainV3?.waiting ?? []} />}
      {viewerAdmin && account.rotationDue && <RotateKeys account={account} link={link} due />}
      {account.missingUserKey !== null && <MissingUserKey account={account} link={link} epoch={account.missingUserKey} />}
      {renewals && account.standing === "active" && <RenewalNotice runner={renewals} />}
      <PushRenewNotice />
    </>
  );
}

/** "I'm sure it never connected — don't warn about it" (review D3), asked first: what it means, and that it can be undone. */
export function DismissReEnroll({ host, device, onConfirm, onCancel }: { host: string; device: string; onConfirm: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  return (
    <div role="alertdialog" data-testid="re-enroll-dismiss" data-signal="warn" className="border-b border-border bg-warn/7 px-6 py-4 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4">
      <div className="flex max-w-xl flex-col gap-2">
        <span className="font-mono text-xs text-fg">{t("stop warning that {{device}} may have copied the identity of {{host}}?", { device, host })}</span>
        <ul className="flex list-disc flex-col gap-1 pl-4 text-xs leading-5 text-muted">
          <li>{t("Only if you are sure {{device}} never connected to {{host}}. If it did, it may still hold the identity key of {{host}}: it can sign in as {{host}} and receive each new content key, and nothing here will warn you about it again.", { device, host })}</li>
          <li>{t("If {{host}} reports that {{device}} connected to it, the warning comes back.", { device, host })}</li>
          <li>{t("Remembered in this browser only: other devices still warn. It stays listed here, with a way to warn about it again.")}</li>
        </ul>
        <div className="flex gap-2">
          <button type="button" className={danger} onClick={onConfirm} data-testid="re-enroll-dismiss-confirm">
            {t("don't warn about it")}
          </button>
          <button type="button" className={quiet} onClick={onCancel}>
            {t("cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** What a row's AI control opens (review 14): stopping AI is confirmed first (`stopHost`); running it again goes at once (`null`: the hub gates it instead). */
export function hostSwitchSection(device: string, host: boolean): Section | null {
  return host ? null : { kind: "stopHost", device };
}

/** What saving 「谁可以控制这台电脑」 does (review D3): a list that lets a device in or shuts one out is confirmed first; one that changes nobody is saved at once. */
export function controlSaveStep(change: { admitted: string[]; shutOut: string[] }): "confirm" | "save" {
  return change.admitted.length > 0 || change.shutOut.length > 0 ? "confirm" : "save";
}

/** A row's actions in 「我的设备」: what its controls open, and what each confirmation then writes. */
export interface RowActions {
  /** "Make it a full device": opens its confirmation (`grant`). */
  onPromote: () => void;
  onHost: (host: boolean) => void;
  /** A limited device made a full device, confirmed. */
  grant: () => void;
  /** Stop running AI, confirmed. */
  stopHost: () => void;
}

/**
 * Row `id`'s actions, as the panel wires them (kept apart, and free of
 * hooks, so which control opens which confirmation — and what a confirmation
 * writes — is plain to test): making a limited device a full device, and
 * stopping AI, only open a confirmation; the confirmation writes. A device
 * made a full device is mailed every user key at once (`promote`): it need
 * not be online, its keys wait in its mailbox — and when they could not be
 * mailed, `onKeysNotSent` says why (its row offers "re-send keys").
 */
export function rowActions(deps: {
  account: Pick<CloudAccount, "promote" | "setHost">;
  id: string;
  writable: boolean;
  courier: () => MachineKeyCourier;
  run: (work: () => Promise<unknown>) => unknown;
  setSection: (section: Section) => void;
  /** It is a full device now, but its keys were not mailed: why, in words. */
  onKeysNotSent?: (why: string) => void;
}): RowActions {
  const { account, id, writable, courier, run, setSection } = deps;
  const list = () => setSection({ kind: "list" });
  return {
    onPromote: () => setSection({ kind: "grant", device: id }),
    onHost: (host) => {
      const next = hostSwitchSection(id, host);
      if (next) setSection(next);
      else void run(() => account.setHost(id, true, courier(), { writable }));
    },
    grant: () =>
      void run(async () => {
        const { keysFailed } = await account.promote(id);
        list();
        if (keysFailed != null) deps.onKeysNotSent?.(errorText(keysFailed));
      }),
    stopHost: () =>
      void run(async () => {
        await account.setHost(id, false, courier(), { writable });
        list();
      }),
  };
}

/** The confirmation `section` asks of row `id`, if it is this row's (review 14): making a limited device a full device, or stopping AI. */
export function RowConfirm({
  section,
  id,
  busy,
  actions,
  onCancel,
}: {
  section: Section;
  id: string;
  busy: boolean;
  actions: Pick<RowActions, "grant" | "stopHost">;
  onCancel: () => void;
}) {
  if (!("device" in section) || section.device !== id) return null;
  switch (section.kind) {
    case "grant":
      return <PromoteDevice device={id} busy={busy} onCancel={onCancel} onConfirm={actions.grant} />;
    case "stopHost":
      return <StopHost device={id} busy={busy} onCancel={onCancel} onConfirm={actions.stopHost} />;
    default:
      return null;
  }
}

/**
 * This browser is a limited device (not chain v3's admin: an older pairing,
 * or a passkey's join): what it can do, and that another of the account's
 * devices makes it a full device — once the account is upgraded to chain v3
 * (`upgraded`), which that device does first otherwise. The list below is
 * read-only here.
 */
export function LimitedDeviceNote({ missing, upgraded = true }: { missing: string[]; upgraded?: boolean }) {
  const { t } = useTranslation();
  return (
    <div data-testid="limited-device-note" className="flex flex-col gap-1 border-b border-border px-6 py-3 max-md:px-4">
      <span className="font-mono text-xs text-fg">{t("this browser is a limited device")}</span>
      <span className="text-xs leading-5 text-muted">
        {upgraded
          ? t("It can control the devices that let it and read their offline copies. To add or remove devices here, make it a full device from another of your devices.")
          : t("It can control the devices that let it and read their offline copies. To add or remove devices here, upgrade the account to chain v3 from another of your devices, then make this browser a full device there.")}
      </span>
      {missing.length > 0 && (
        <span data-signal="warn" className="text-2xs text-warn-fg">
          {t("waiting for the keys of {{hosts}} from a full device", { hosts: missing.join(", ") })}
        </span>
      )}
    </div>
  );
}

/** On a v3 account while the hub takes no v3 entry: adding a computer and running AI again wait (the hub gates new hosts) — for the hub, or for computers not updated yet (`blocker`). */
export function V3Note({ waiting, blocker = "computers" }: { waiting: string[]; blocker?: V3Blocker | null }) {
  const { t } = useTranslation();
  return (
    <div data-testid="v3-note" className="flex flex-col gap-1 border-b border-border px-6 py-3 max-md:px-4">
      <span className="text-xs leading-5 text-muted">
        {blocker === "hub"
          ? t("Adding a device that runs AI, and running AI again on one, wait until the hub takes chain v3 changes for this account.")
          : t("Adding a device that runs AI, and running AI again on one, wait until Droppen is updated on every device of the account that runs AI.")}
      </span>
      <V3Pending waiting={waiting} blocker={blocker} />
    </div>
  );
}

/**
 * The upgrade to chain v3 of an account made before it (review D4,
 * local-aggregator §3.5; a new account is v3 from its genesis and never
 * shows this): a step of its own, from a full device, while the chain
 * holds no v3 entry — offered only while the hub takes v3 entries, and
 * written only once the person has read what it means and what it writes.
 */
export function UpgradeAccount({
  account,
  writable,
  waiting,
  blocker = "computers",
  onDone,
}: {
  account: Pick<CloudAccount, "upgradePlan" | "upgradeToV3">;
  writable: boolean;
  waiting: string[];
  /** Why it cannot upgrade now (`v3Blocker`): the hub (`/v1/me`'s `chain_v3.mode` is not `write`), or computers not updated yet. */
  blocker?: V3Blocker | null;
  onDone?: () => void;
}) {
  const { t } = useTranslation();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<RotationReport | null>(null);
  const plan = account.upgradePlan();
  if (report) {
    return (
      <div data-testid="upgraded" data-signal="ok" className="flex flex-col gap-1 border-b border-border px-6 py-3 shadow-[inset_2px_0_0_var(--color-ok)] max-md:px-4">
        <span className="font-mono text-xs text-ok">{t("the account is on chain v3")}</span>
        {reportLines(report, t).map((line, i) => (
          <span key={i} className="font-mono text-2xs text-muted">
            {line}
          </span>
        ))}
      </div>
    );
  }
  if (!plan) return null;
  return (
    <div data-testid="upgrade-account" className="flex flex-col gap-2 border-b border-border px-6 py-3 max-md:px-4">
      <span className="font-mono text-xs text-fg">{t("this account is still on chain v2")}</span>
      <span className="text-xs leading-5 text-muted">
        {t("Upgrading it lets you stop AI on a device, decide who may control each device that runs AI, and make a limited device a full device.")}
      </span>
      {!writable && <V3Pending waiting={waiting} blocker={blocker} />}
      {writable && !confirming && (
        <button type="button" data-testid="upgrade-start" className={cn(secondary, "self-start")} onClick={() => setConfirming(true)}>
          {t("upgrade this account to chain v3")}
        </button>
      )}
      {writable && confirming && (
        <UpgradeConfirm
          ukEpoch={plan.ukEpoch}
          busy={busy}
          error={error}
          onCancel={() => setConfirming(false)}
          onConfirm={async () => {
            setBusy(true);
            setError(null);
            try {
              setReport(await account.upgradeToV3({ writable }));
              onDone?.();
            } catch (err) {
              setError(errorText(err));
            } finally {
              setBusy(false);
            }
          }}
        />
      )}
    </div>
  );
}

/** The upgrade's confirmation: what it means, that it cannot be undone, and exactly what it writes. */
export function UpgradeConfirm({ ukEpoch, busy, error, onConfirm, onCancel }: { ukEpoch: number; busy: boolean; error: string | null; onConfirm: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  return (
    <div role="alertdialog" data-testid="upgrade-confirm" data-signal="warn" className="flex max-w-xl flex-col gap-2 border border-border bg-warn/7 p-3 shadow-[inset_2px_0_0_var(--color-warn)]">
      <span className="font-mono text-xs text-fg">{t("upgrade this account to chain v3?")}</span>
      <ul className="flex list-disc flex-col gap-1 pl-4 text-xs leading-5 text-muted">
        <li>{t("Older Droppen apps, and older Droppen on your devices that run AI, cannot follow the account afterwards: update them first.")}</li>
        <li className="text-fg">{t("This cannot be undone.")}</li>
      </ul>
      <p data-testid="upgrade-writes" className="text-2xs leading-4 text-muted">
        {t("What is written: one entry, a new user key (epoch {{n}}), as chain v3. Every full device of the account gets the new key; the recovery words keep working.", { n: ukEpoch })}
      </p>
      <ErrorLine message={error} />
      <div className="flex gap-2">
        <button type="button" data-testid="upgrade-confirm-button" className={danger} disabled={busy} onClick={onConfirm}>
          {busy && <Loader2 className="size-3.5 animate-spin" />}
          {t("upgrade the account")}
        </button>
        <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}

/**
 * Making a limited device a full device (§3.7): what it means first. Every
 * browser or phone is meant to be one; a device that runs AI is never
 * offered it (review D1). Its keys are mailed at once, so it need not be
 * online.
 */
export function PromoteDevice({ device, busy, onConfirm, onCancel }: { device: string; busy: boolean; onConfirm: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  return (
    <div role="alertdialog" data-testid="promote-device" data-signal="warn" className="border-b border-border bg-warn/7 px-6 py-4 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4">
      <div className="flex max-w-xl flex-col gap-2">
        <span className="font-mono text-xs text-fg">{t("make {{id}} a full device?", { id: device })}</span>
        <ul className="flex list-disc flex-col gap-1 pl-4 text-xs leading-5 text-muted">
          <li>{t("It gets the account's keys and can add or remove devices, like this one.")}</li>
          <li>{t("It need not be online: its keys wait for it at the hub.")}</li>
        </ul>
        <div className="flex gap-2">
          <button type="button" data-variant="primary" className={primary} disabled={busy} onClick={onConfirm} data-testid="promote-device-confirm">
            {busy && <Loader2 className="size-3.5 animate-spin" />}
            {t("make it a full device")}
          </button>
          <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
            {t("cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * A device just made a full device whose user keys were not mailed (its
 * mailbox at the hub full, the hub holding back this device's mail…): it is
 * a full device all the same, and "re-send keys" sends them once that is
 * fixed. Said under its row.
 */
export function KeysNotSent({ device, why, resend, onClose }: { device: string; why: string; resend: ReactNode; onClose: () => void }) {
  const { t } = useTranslation();
  return (
    <div
      role="alert"
      data-testid={`keys-not-sent-${device}`}
      data-signal="warn"
      className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-border bg-warn/7 px-6 py-2.5 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4"
    >
      <span className="flex min-w-[min(100%,16rem)] flex-1 flex-col gap-0.5 text-xs leading-5">
        <span className="text-fg">{t("{{id}} is a full device now, but its keys were not sent", { id: device })}</span>
        <span className="text-muted">{why}</span>
      </span>
      {resend}
      <button type="button" className={cn(quiet, "h-7 px-2")} onClick={onClose}>
        {t("close")}
      </button>
    </div>
  );
}

/** Stopping AI on a computer (review 14): asked first. */
export function StopHost({ device, busy, onConfirm, onCancel }: { device: string; busy: boolean; onConfirm: () => void; onCancel: () => void }) {
  const { t } = useTranslation();
  return (
    <div role="alertdialog" data-testid="stop-host" data-signal="warn" className="border-b border-border bg-warn/7 px-6 py-4 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4">
      <div className="flex max-w-xl flex-col gap-2">
        <span className="font-mono text-xs text-fg">{t("stop running AI on {{id}}?", { id: device })}</span>
        <ul className="flex list-disc flex-col gap-1 pl-4 text-xs leading-5 text-muted">
          <li>{t("Its agent stops: nobody can control it or read new sessions from it until it runs AI again.")}</li>
          <li>{t("It stays a device of the account. Running AI again gives it a new content key, once Droppen is updated on every device that runs AI.")}</li>
        </ul>
        <div className="flex gap-2">
          <button type="button" className={danger} disabled={busy} onClick={onConfirm} data-testid="stop-host-confirm">
            {busy && <Loader2 className="size-3.5 animate-spin" />}
            {t("stop running AI")}
          </button>
          <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
            {t("cancel")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** Whether `from` names device `id` (admins aside, which every host accepts). */
function acceptsId(from: Accept, id: string): boolean {
  return from.type === "all" || (from.type === "devices" && from.ids.includes(id));
}

/**
 * Before a new 「谁可以控制这台电脑」 is written (review D3): who it lets in —
 * full access to that computer, its identity key included — and who it shuts
 * out, which gives the computer a new machine key and calls for a new
 * identity.
 */
export function ControlConfirm({
  host,
  admitted,
  shutOut,
  renewable = false,
  renews = [],
  busy,
  onConfirm,
  onCancel,
}: {
  host: string;
  admitted: string[];
  shutOut: string[];
  /** It renews its identity key here (docs/host-identity-rotation.md §4): a narrowing renews it, rather than asking to re-enroll it. */
  renewable?: boolean;
  /** The devices that run AI a narrowing starts renewing (it, and those that let it control them). */
  renews?: string[];
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div role="alertdialog" data-testid="control-confirm" data-signal="warn" className="flex flex-col gap-2 border border-border bg-warn/7 p-3 shadow-[inset_2px_0_0_var(--color-warn)]">
      {admitted.length > 0 && (
        <span className="text-xs leading-5 text-fg">{t("Let {{ids}} control {{host}}? Each gets full access to it, its identity key included.", { ids: admitted.join(", "), host })}</span>
      )}
      {shutOut.length > 0 && !renewable && (
        <span className="text-xs leading-5 text-muted">
          {t("{{ids}} can no longer control it: {{host}} gets a new content key now — but a device that copied its identity key can still sign in as {{host}} and receive that key. Re-enroll {{host}} afterwards: remove it, then add it again.", { ids: shutOut.join(", "), host })}
        </span>
      )}
      {shutOut.length > 0 && renewable && (
        <>
          <span className="text-xs leading-5 text-muted">
            {t("{{ids}} can no longer control it: {{host}} gets a new content key now, and a new identity key, so a copy of the old one is no use.", { ids: shutOut.join(", "), host })}
          </span>
          <RenewalsAhead ids={renews} />
        </>
      )}
      <div className="flex gap-2">
        <button type="button" className={danger} disabled={busy} onClick={onConfirm} data-testid="control-confirm-button">
          {busy && <Loader2 className="size-3.5 animate-spin" />}
          {admitted.length > 0 ? t("allow full access") : t("save and rotate its key")}
        </button>
        <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}

/**
 * 「谁可以控制这台电脑」 (`setAccept`): only your full devices (chain v3's
 * `admins`, the default), every device, or the full devices and the ones
 * picked (a limited device, or another device that runs AI) — with what
 * allowing another computer that runs AI means.
 */
export function HostControl({
  host,
  policy,
  declaresPolicy,
  candidates,
  upgraded,
  mustRenew,
  renewable,
  renewsIfNarrowed,
  paused = [],
  busy,
  onSave,
  onCancel,
}: {
  host: string;
  policy: Accept;
  /**
   * Its agent declared `policy/1` (refuses devices that are not admins its
   * admin routes: local-aggregator §5.4, S1-4); `undefined` when the hub did
   * not say. `false`: nobody but admins may be let in — an older agent would
   * let them reach every route, self-upgrade included (the hub refuses to
   * route them there anyway).
   */
  declaresPolicy?: boolean;
  /** The devices that may be picked: current, not full devices (they always may), not the host itself — `limited`: a browser or phone that is a limited device. */
  candidates: { id: string; host: boolean; limited?: boolean }[];
  /** The chain holds a v3 entry (review D4): only then can this change. */
  upgraded: boolean;
  /** Hosts that must renew their identity key before another host may let them in (`CloudAccount.mustRenewFirst`). */
  mustRenew?: string[];
  /** This host renews its identity key here: a narrowing renews it (else: re-enroll it). */
  renewable?: boolean;
  /** The devices that run AI a narrowing would start renewing. */
  renewsIfNarrowed?: string[];
  /** The ids a renewal pause holds from controlling it now: shown apart from the list, which is what stays (§3.4 rule 10). */
  paused?: string[];
  busy: boolean;
  onSave: (from: Accept) => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const [kind, setKind] = useState<Accept["type"]>(policy.type);
  const [picked, setPicked] = useState<Set<string>>(new Set(policy.type === "devices" ? policy.ids : []));
  const [confirming, setConfirming] = useState(false);
  const hostPicked = kind === "all" ? candidates.some((c) => c.host) : candidates.some((c) => c.host && picked.has(c.id));
  const from: Accept = kind === "devices" ? { type: "devices", ids: [...picked] } : { type: kind };
  const admitted = candidates.filter((c) => acceptsId(from, c.id) && !acceptsId(policy, c.id)).map((c) => c.id);
  const shutOut = candidates.filter((c) => acceptsId(policy, c.id) && !acceptsId(from, c.id)).map((c) => c.id);
  // The `policy/1` gate: widening who controls an agent that has no refusal
  // list — or one not known yet (`/v1/me` still loading: security review of S1-4, N1) — is refused here.
  const outdated = declaresPolicy !== true && admitted.length > 0;
  // A host that may have had its identity key copied is let in nowhere until it renewed it (docs/host-identity-rotation.md §7 decision 8).
  const exposed = admitted.filter((id) => mustRenew?.includes(id));
  return (
    <div data-testid={`host-control-${host}`} className="border-b border-border px-6 py-4 max-md:px-4">
      <div className="flex max-w-xl flex-col gap-2">
        <CapsLabel>{t("who can control this device")}</CapsLabel>
        {(
          [
            ["admins", t("only your full devices")],
            ["all", t("every device of the account")],
            ["devices", t("your full devices, and the devices picked here")],
          ] as const
        ).map(([value, label]) => (
          <label key={value} className="flex items-center gap-2 text-xs text-fg">
            <input type="radio" name={`accept-${host}`} checked={kind === value} disabled={!upgraded} onChange={() => setKind(value)} />
            {label}
          </label>
        ))}
        {kind === "devices" && (
          <ul className="ml-6 flex flex-col gap-1">
            {candidates.length === 0 && <li className="text-2xs text-faint">{t("no other device to pick")}</li>}
            {candidates.map((c) => (
              <li key={c.id}>
                <label className="flex items-center gap-2 font-mono text-2xs text-fg">
                  <input
                    type="checkbox"
                    disabled={!upgraded}
                    checked={picked.has(c.id)}
                    onChange={(e) => {
                      const next = new Set(picked);
                      if (e.target.checked) next.add(c.id);
                      else next.delete(c.id);
                      setPicked(next);
                    }}
                  />
                  {c.id}
                  {c.host && <span className="text-faint">· {t("can run AI")}</span>}
                  {c.limited && <span className="text-faint">· {t("limited device")}</span>}
                </label>
              </li>
            ))}
          </ul>
        )}
        <p data-testid="control-full-access" className="text-2xs leading-4 text-fg">
          {t("Whoever may control this device has full access to it — its files, terminals, sessions and its own identity key. The routes it keeps for full devices only are a convenience, not a boundary.")}
        </p>
        <p data-testid="control-warning" data-signal={hostPicked ? "warn" : undefined} className={cn("text-2xs leading-4", hostPicked ? "text-warn-fg" : "text-muted")}>
          {t("Letting another device that runs AI control this one means that device — and every AI CLI on it — can drive this one.")}
        </p>
        {!upgraded && <p className="text-2xs leading-4 text-muted">{t("upgrade the account first")}</p>}
        {declaresPolicy === false ? (
          <p data-testid="control-outdated" data-signal={outdated ? "warn" : undefined} className={cn("text-2xs leading-4", outdated ? "text-warn-fg" : "text-muted")}>
            {t("Update Droppen on this device first: an older version lets every device that controls it use the routes it keeps for full devices, self-upgrade included. Until then only your full devices may control it.")}
          </p>
        ) : (
          outdated && (
            <p data-testid="control-outdated" data-signal="warn" className="text-2xs leading-4 text-warn-fg">
              {t("Checking which Droppen this device runs: until then only your full devices may control it.")}
            </p>
          )
        )}
        {paused.length > 0 && (
          <p data-testid="control-paused" className="text-2xs leading-4 text-muted">
            {t("Paused while identity keys are renewed: {{ids}}. They come back when the pause ends; the list here is what stays.", { ids: paused.join(", ") })}
          </p>
        )}
        {exposed.length > 0 && (
          <p data-testid="control-exposed" data-signal="warn" className="text-2xs leading-4 text-warn-fg">
            {t("Renew the identity key of {{ids}} first: a device that could control it no longer can, and may have copied it. Until then it may control no other device.", { ids: exposed.join(", ") })}
          </p>
        )}
        {confirming ? (
          <ControlConfirm
            host={host}
            admitted={admitted}
            shutOut={shutOut}
            renewable={renewable}
            renews={renewsIfNarrowed}
            busy={busy}
            onCancel={() => setConfirming(false)}
            onConfirm={() => onSave(from)}
          />
        ) : (
          <div className="flex gap-2">
            <button
              type="button"
              data-variant="primary"
              className={primary}
              disabled={busy || !upgraded || outdated || exposed.length > 0}
              onClick={() => (controlSaveStep({ admitted, shutOut }) === "confirm" ? setConfirming(true) : onSave(from))}
            >
              {busy && <Loader2 className="size-3.5 animate-spin" />}
              {t("save")}
            </button>
            <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
              {t("cancel")}
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * This device's direct connections (the stored switch is "hub only", from
 * before relays: `setHubOnly`). Allowed, a connection goes straight to the
 * device when it can — faster, and not counted as transfer — and that device
 * and the STUN providers see this one's IP address; not allowed, everything
 * goes through a relay, and the IP address stays between this device and the
 * relays. Kept on this device only; takes effect at once (a direct path in
 * use is let go).
 */
export function DirectConnectionSetting({ allowed, onChange }: { allowed?: boolean; onChange?: (allowed: boolean) => void }) {
  const { t } = useTranslation();
  const hubOnly = useHubOnly();
  const on = allowed ?? !hubOnly;
  const label = t("Allow direct connections (faster)");
  return (
    <div data-testid="direct-connections" data-allowed={on ? "" : undefined} className="flex items-start justify-between gap-6 px-6 py-3 max-md:gap-3 max-md:px-4">
      <div className="flex min-w-0 max-w-[48rem] flex-col gap-0.5">
        <span className="text-xs font-medium text-fg">{label}</span>
        <span className="text-xs leading-5 text-muted">
          {t(
            "Direct connections skip our servers: faster, and not counted as transfer. Your devices that run AI, and the STUN providers Cloudflare and Google, then see this device's IP address. Either way, everything stays end-to-end encrypted.",
          )}
        </span>
      </div>
      <SquareSwitch on={on} blocked={false} label={label} labelHidden testId="direct-switch" onChange={(next) => (onChange ? onChange(next) : setHubOnly(!next))} />
    </div>
  );
}

/**
 * 「备用中继」 (relay-and-plans §2.7), for the whole account, under 「这台设备」's
 * relays: off (the default), the Hub keeps one relay in the account's set —
 * each device that runs AI holds one relay connection, and a relay that fails
 * or restarts is replaced at once; on, two, so the move is quicker. Shown
 * only when the hub says what the account has (`/v1/me` `relay_prefs`).
 */
export function RelayStandbySetting({ account, link }: { account: CloudAccount; link: HubLink }) {
  const { t } = useTranslation();
  const me = useMe(account, link);
  const stored = me?.view?.relayStandby ?? null;
  const [pending, setPending] = useState<boolean | null>(null);
  const [error, setError] = useState<string | null>(null);
  if (stored === null && pending === null) return null;
  const on = pending ?? stored ?? false;
  const label = t("Keep a standby relay");
  const change = async (next: boolean) => {
    setPending(next);
    setError(null);
    try {
      await account.api.setRelayStandby(account.session.token, next);
      await me?.refresh();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setPending(null);
    }
  };
  return (
    <div
      data-testid="relay-standby"
      data-on={on ? "" : undefined}
      className="flex items-start justify-between gap-6 py-3 pr-6 pl-11 max-md:gap-3 max-md:pr-4 max-md:pl-9"
    >
      <div className="flex min-w-0 max-w-[48rem] flex-col gap-0.5">
        <span className="text-xs font-medium text-fg">{label}</span>
        <span className="text-xs leading-5 text-muted">
          {t(
            "For the whole account: each of your devices that runs AI also stays connected to a second relay, so a relay that fails or restarts is switched faster. One more idle connection per device; an idle connection is not counted as transfer.",
          )}
        </span>
        <ErrorLine message={error} />
      </div>
      <SquareSwitch on={on} blocked={pending !== null} label={label} labelHidden testId="relay-standby-switch" onChange={(next) => void change(next)} />
    </div>
  );
}

/**
 * "Missing key": the chain commits a newer user key than this device holds
 * (it missed the rotation's mail, or the rotation died after its commit). It
 * wraps no machine key until it has it (M2 re-review #27) — it has asked the
 * account's other devices to send it; if none has it, rotating again mends it.
 */
export function MissingUserKey({ account, link, epoch }: { account: CloudAccount; link: HubLink; epoch: number }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState<"ask" | "rotate" | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const run = async (what: "ask" | "rotate") => {
    setBusy(what);
    setError(null);
    setNote(null);
    try {
      if (what === "ask") {
        const asked = await account.requestUserKeys(epoch, true);
        setNote(asked.length ? t("asked {{ids}}", { ids: asked.join(", ") }) : t("no other device to ask"));
      } else {
        await account.rotate(shellCourier(link, account), true);
        await account.checkUserKey();
      }
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(null);
    }
  };
  return (
    <div
      role="alert"
      data-testid="missing-user-key"
      data-signal="warn"
      className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border bg-warn/7 px-6 py-3 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4"
    >
      <span className="flex min-w-[min(100%,16rem)] flex-1 items-start gap-3">
        <span className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("missing key")}</span>
        <span className="min-w-0 flex-1 text-xs leading-5 text-muted">
          <span className="text-fg">{t("This device does not hold the account's newest user key (epoch {{n}}).", { n: epoch })}</span>{" "}
          {t("It wraps no new content key until it does, and has asked the account's other devices to send it. If none of them has it, rotate the keys again.")}
          {note && <span className="font-mono text-2xs text-faint"> · {note}</span>}
        </span>
      </span>
      <Button disabled={busy !== null} onClick={() => void run("ask")}>
        {busy === "ask" && <Loader2 className="animate-spin" />}
        {t("ask again")}
      </Button>
      <Button variant="primary" disabled={busy !== null} onClick={() => void run("rotate")}>
        {busy === "rotate" ? <Loader2 className="animate-spin" /> : <RefreshCw />}
        {busy === "rotate" ? t("rotating keys…") : t("rotate keys again")}
      </Button>
      <ErrorLine message={error} />
    </div>
  );
}

function mailWhy(code: string | undefined, t: (key: string, opts?: Record<string, unknown>) => string): string {
  if (code === "mailbox_full") return t("its mailbox at the hub is full");
  if (code === "mail_rate") return t("the hub is holding back this device's mail for a minute");
  return code ?? "";
}

/** Whether a report line is one to say loudly: a machine whose new key the keyring did not take. */
function alarmLine(report: RotationReport, line: string): boolean {
  // The machine's line, in any language: Chinese and Japanese put a full-width colon after its id.
  return report.machines.some((m) => m.keyring && (line.startsWith(`${m.machine}:`) || line.startsWith(`${m.machine}：`)) && line.includes(m.keyring));
}

function reportLines(report: RotationReport, t: (key: string, opts?: Record<string, unknown>) => string): string[] {
  const lines =
    report.ukEpoch === null
      ? report.machines.length
        ? [t("new content keys only: the removed device held no user key")]
        : [t("no keys to rotate: the removed device held none but its own")]
      : [t("new user key (epoch {{n}}) sent to {{count}} device(s)", { n: report.ukEpoch, count: report.sealedTo.length })];
  if (report.hostKeysTo?.length) lines.push(t("new content keys sealed for {{count}} device(s) that hold no user key", { count: report.hostKeysTo.length }));
  const hostFailed = Object.keys(report.hostKeysFailed ?? {});
  if (hostFailed.length) lines.push(t("could not reach: {{ids}}", { ids: hostFailed.map((id) => `${id} (${mailWhy(report.hostKeysFailed![id], t)})`).join(", ") }));
  if (report.failedTo.length) {
    lines.push(t("could not reach: {{ids}}", { ids: report.failedTo.map((id) => (report.failedWhy[id] ? `${id} (${mailWhy(report.failedWhy[id], t)})` : id)).join(", ") }));
    lines.push(t("send it again from here with “re-send keys” once that is fixed"));
  }
  if (report.recovery) lines.push(t("and sealed for the recovery words"));
  for (const m of report.machines) {
    if (m.error) lines.push(t("{{machine}}: {{error}}", { machine: m.machine, error: knownText(m.error) }));
    else if (m.result?.via === "channel") lines.push(t("{{machine}}: new content key handed over end to end", { machine: m.machine }));
    else if (m.result?.via === "mailbox") {
      const { reason, pending } = m.result;
      lines.push(
        reason === "offline"
          ? t("{{machine}}: offline — the new content key waits in its mailbox", { machine: m.machine })
          : reason === "unconfirmed"
            ? t("{{machine}}: could not open a fresh channel yet (waiting for another device to confirm the change) — the new content key waits in its mailbox", { machine: m.machine })
            : t("{{machine}}: the channel failed — the new content key waits in its mailbox", { machine: m.machine }),
      );
      if (pending) lines.push(t("{{machine}}: re-encryption runs when this device next connects to it", { machine: m.machine }));
    }
    else if (m.result?.via === "refused") {
      lines.push(t("{{machine}}: refused the new key ({{message}})", { machine: m.machine, message: hostRefusalText(m.result.code) ?? m.result.message }));
    }
    if (m.keyring) {
      lines.push(
        t("{{machine}}: NOT IN THE KEYRING ({{why}}) — the new key is in use, but other devices cannot read what it writes from now on. Make room, then rotate the keys again.", {
          machine: m.machine,
          why: m.keyring,
        }),
      );
    }
  }
  if (report.vetoed?.devices.length) lines.push(t("also removed what it brought in: {{ids}}", { ids: report.vetoed.devices.join(", ") }));
  if (report.vetoed?.machines.length) lines.push(t("and the devices it added: {{ids}}", { ids: report.vetoed.machines.join(", ") }));
  return lines;
}

function RemoveDevice({
  account,
  link,
  device,
  record,
  fingerprint,
  onDone,
  onAddAgain,
}: {
  account: CloudAccount;
  link: HubLink;
  device: string;
  record?: DeviceRecord;
  fingerprint: string;
  onDone: () => void;
  /** A computer that runs AI: add it again (a new identity: review D3). */
  onAddAgain?: () => void;
}) {
  const { t } = useTranslation();
  const [reencrypt, setReencrypt] = useState(false);
  const [rotateMachineKeys, setRotateMachineKeys] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<RotationReport | null>(null);
  const v3 = account.chain?.isV3 === true;
  // What its removal rotates (§3.3): a device that ever held the user key (by the chain's history, review 1) —
  // everything; one a computer accepts control from — that computer's machine keys, always (review D2);
  // any other, machine keys only if asked; chain v2's machine held its own key alone.
  const heldUserKey = record ? account.heldUserKey(device) : true;
  // The account's recovery words go with it only when it knows the current ones (v3) — as the chain will rule.
  const voidsWords = account.chain?.voidsRecoveryWords(device, "removal") ?? false;
  const rotation: "all" | "machine" | "choice" | "none" = heldUserKey ? "all" : !v3 ? "none" : account.controlsAHost(device) ? "machine" : "choice";
  // The devices that run AI it could control (and those that let them control them) renew their identity key after it (§4.1, §4.4).
  const renewing = v3 && account.chain ? hostsExposedBy(account.chain, device).filter((h) => account.renews?.(h) === true) : [];
  if (report) {
    return (
      <div data-signal="ok" className="flex flex-col gap-1 border border-border p-3 shadow-[inset_2px_0_0_var(--color-ok)]">
        <span className="font-mono text-xs text-ok">{t("removed {{id}}", { id: device })}</span>
        {reportLines(report, t).map((line, i) => (
          <span key={i} className={cn("font-mono text-2xs", alarmLine(report, line) ? "text-danger" : "text-muted")} data-signal={alarmLine(report, line) ? "alarm" : undefined}>
            {line}
          </span>
        ))}
        <div className="flex gap-2">
          <button type="button" className={quiet} onClick={onDone}>
            {t("done")}
          </button>
          {onAddAgain && (
            <button type="button" className={secondary} onClick={onAddAgain} data-testid="add-again">
              {t("add it again")}
            </button>
          )}
        </div>
      </div>
    );
  }
  return (
    <div data-signal="failed" data-testid="remove-device" className="flex flex-col gap-2 border border-border bg-danger/4 p-3 shadow-[inset_2px_0_0_var(--color-danger)]">
      <span className="font-mono text-xs text-danger">{t("remove {{id}}?", { id: device })}</span>
      {fingerprint && <Fingerprint value={fingerprint} />}
      <p className="text-xs leading-5 text-muted">
        {rotation === "all"
          ? t("It loses access at once. The keys it held are retired: a new user key goes to the other devices and a new content key to each device that runs AI. What it already copied cannot be taken back.")
          : rotation === "machine"
            ? t("It loses access at once. It never held the user key, but a device that runs AI accepts its control: the content keys are rotated. What it already copied cannot be taken back.")
            : rotation === "choice"
              ? t("It loses access at once. It never held the user key, and no device accepts its control now: rotating the content keys covers any it was handed before.")
              : t("It loses access at once. It held only its own content key, which goes with it.")}
      </p>
      {voidsWords && (
        <p className="text-xs text-fg" data-testid="remove-voids-words">
          {t("The account's recovery words stop working with it: it set them, or came back with them. Make new recovery words afterwards.")}
        </p>
      )}
      <RenewalsAhead ids={renewing} />
      {rotation !== "none" && (
        <label className="flex items-start gap-2 text-2xs text-muted">
          <input
            type="checkbox"
            data-testid="rotate-machine-keys"
            checked={rotation !== "choice" || rotateMachineKeys}
            disabled={rotation !== "choice"}
            onChange={(e) => setRotateMachineKeys(e.target.checked)}
            className="mt-0.5"
          />
          {rotation === "all"
            ? t("also rotate the content keys (always, for a device that held the user key)")
            : rotation === "machine"
              ? t("also rotate the content keys (always, for a device another device accepts control from)")
              : t("also rotate the content keys")}
        </label>
      )}
      {rotation !== "none" && (rotation !== "choice" || rotateMachineKeys) && (
        <label className="flex items-start gap-2 text-2xs text-muted">
          <input type="checkbox" checked={reencrypt} onChange={(e) => setReencrypt(e.target.checked)} className="mt-0.5" />
          {t("also re-encrypt history: each device that runs AI rebuilds its encrypted copy under the new key and the old one is deleted")}
        </label>
      )}
      <ErrorLine message={error} />
      <div className="flex gap-2">
        <button
          type="button"
          className={danger}
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError(null);
            try {
              setReport(
                await account.removeDevice(device, shellCourier(link, account), reencrypt, { rotateMachineKeys }),
              );
            } catch (err) {
              setError(errorText(err));
              setBusy(false);
            }
          }}
        >
          {busy && <Loader2 className="size-3.5 animate-spin" />}
          {busy ? t("removing and rotating keys…") : t("remove")}
        </button>
        <button type="button" className={quiet} disabled={busy} onClick={onDone}>
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}

/**
 * "Rotate keys again": a new user key for every device and a new key for
 * every machine (`CloudAccount.rotate`), committed in the chain first. Always
 * here; drawn as the thing to do (`due`) when the chain's last removal was
 * not followed by new keys — a rotation that never ran, or died half-way.
 */
function RotateKeys({ account, link, due }: { account: CloudAccount; link: HubLink; due: boolean }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [report, setReport] = useState<RotationReport | null>(null);
  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      setReport(await account.rotate(shellCourier(link, account), true));
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };
  if (report) {
    return (
      <div data-testid="rotated" data-signal="ok" className="mx-6 my-4 flex flex-col gap-1 border border-border p-3 shadow-[inset_2px_0_0_var(--color-ok)] max-md:mx-4">
        <span className="font-mono text-xs text-ok">{t("keys rotated")}</span>
        {reportLines(report, t).map((line, i) => (
          <span key={i} className={cn("font-mono text-2xs", alarmLine(report, line) ? "text-danger" : "text-muted")} data-signal={alarmLine(report, line) ? "alarm" : undefined}>
            {line}
          </span>
        ))}
        <Button variant="ghost" className="self-start" onClick={() => setReport(null)}>
          {t("done")}
        </Button>
      </div>
    );
  }
  if (due) {
    return (
      <div
        role="alert"
        data-testid="rotation-due"
        data-signal="warn"
        className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border bg-warn/7 px-6 py-3 shadow-[inset_2px_0_0_var(--color-warn)] max-md:px-4"
      >
        <span className="flex min-w-[min(100%,16rem)] flex-1 items-start gap-3">
          <span className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("needs you")}</span>
          <span className="min-w-0 flex-1 text-xs leading-5 text-muted">
            <span className="text-fg">{t("A device that held the user key was removed or made a limited device, but its keys were not retired.")}</span>{" "}
            {t("Until they are, it can still read what is new. Rotate them now: every device gets a new user key and every device that runs AI a new content key.")}
          </span>
        </span>
        <Button variant="primary" disabled={busy} onClick={() => void run()}>
          {busy ? <Loader2 className="animate-spin" /> : <RefreshCw />}
          {busy ? t("rotating keys…") : t("rotate keys again")}
        </Button>
        <ErrorLine message={error} />
      </div>
    );
  }
  return (
    <div data-testid="rotate-keys" className="flex flex-col gap-2 border-t border-border pt-4">
      <CapsLabel>{t("keys")}</CapsLabel>
      <p className="text-xs leading-5 text-muted">
        {t("Rotate the keys if a removal did not finish, or you think a key got out: every device gets a new user key and every device that runs AI a new content key. Old history stays readable here.")}
      </p>
      <Button className="self-start" disabled={busy} onClick={() => void run()}>
        {busy ? <Loader2 className="animate-spin" /> : <RefreshCw />}
        {busy ? t("rotating keys…") : t("rotate keys again")}
      </Button>
      <ErrorLine message={error} />
    </div>
  );
}

/** How loudly an alert speaks (local-aggregator §3.2): "is this you?", louder for a device made a full device (it gets the account's keys), a note for the rest. */
export function alertLevel(alert: ChainAlert): "confirm" | "high" | "info" {
  if (alert.kind === "setAdmin") return alert.on ? "high" : "info";
  if (alert.kind === "setHost" || alert.kind === "setAccept" || alert.kind === "removeDevice" || alert.kind === "keyEpochs" || alert.kind === "upgrade" || alert.kind === "rotateHostKey" || alert.kind === "renewalPause") return "info";
  // Renewals confirmed on the device turned off (or never on): "is this you?" on every device (#178 L1). Turned on: a note.
  if (alert.kind === "renewalPolicy") return alert.on ? "info" : "confirm";
  if (alert.kind === "passkey") return alert.passkey === "join" || alert.passkey === "slotSet" ? "confirm" : "info";
  return "confirm";
}

/** What an alert says happened, in words (`fingerprint`: a renewed identity key's new fingerprint, once computed). */
export function alertText(alert: ChainAlert, t: (key: string, opts?: Record<string, unknown>) => string, fingerprint?: string): string {
  const by = alert.by ?? "?";
  const device = alert.subject ?? "?";
  switch (alert.kind) {
    case "addDevice": {
      const f = alert.flags;
      // A full device (chain v2's every addition, v3's admin) is what a browser or phone is: no more to say. A host is never one (review D1).
      if (!f || (f.admin && !f.host)) return t("{{by}} added {{device}} to this account", { by, device });
      if (f.host) return t("{{by}} added {{device}} to this account: it can run AI", { by, device });
      return t("{{by}} added {{device}} to this account as a limited device", { by, device });
    }
    case "removeDevice":
      return t("{{by}} removed {{device}} from this account", { by, device });
    case "recoveryJoin":
      return t("{{device}} came back into this account with the recovery words", { device });
    case "setRecoveryKey":
      return t("{{by}} made new recovery words for this account", { by });
    case "keyEpochs":
      return t("{{by}} made new keys for this account, on its own", { by });
    case "upgrade":
      return t("{{by}} upgraded this account to chain v3 (with a new user key)", { by });
    case "setAdmin":
      return alert.on ? t("{{by}} made {{device}} a full device", { by, device }) : t("{{by}} made {{device}} a limited device", { by, device });
    case "setHost":
      return alert.on ? t("{{device}} runs AI again ({{by}} restarted it)", { by, device }) : t("{{device}} no longer runs AI ({{by}} stopped it)", { by, device });
    case "setAccept": {
      const a = alert.accept;
      if (a?.type === "all") return t("{{by}} let every device of this account control {{host}}", { by, host: device });
      if (a?.type === "devices") return t("{{by}} let {{ids}} control {{host}}", { by, host: device, ids: a.ids.join(", ") });
      return t("{{by}} set {{host}} to take control only from your full devices", { by, host: device });
    }
    case "passkey":
      switch (alert.passkey) {
        case "slotSet":
          return t("{{by}} let new devices join with a passkey", { by });
        case "slotRemove":
          return t("{{by}} stopped new devices joining with a passkey", { by });
        case "join":
          return t("a passkey added {{device}} to this account (it waits to be approved)", { device });
        default:
          return t("{{device}} activated itself with a passkey", { device });
      }
    case "renewalPause": {
      // Host identity rotation §3.4 rule 10: control paused over hosts while identity keys are renewed, a note.
      const edges = (alert.pause?.removals ?? []).map((r) => t("{{ids}} over {{host}}", { ids: r.ids.join(", "), host: r.host })).join("; ") || "?";
      if (alert.pause?.phase === "completed") {
        if (alert.pause.removals.length === 0) return t("a renewal pause was completed; none of the control it paused comes back (by {{by}})", { by });
        return t("control by {{edges}} resumes (by {{by}})", { edges, by });
      }
      if (alert.pause?.phase === "cancelled") return t("the pause of control by {{edges}} was ended (by {{by}})", { edges, by });
      return t("control by {{edges}} is paused while identity keys are renewed (by {{by}})", { edges, by });
    }
    case "renewalPolicy":
      // Host identity rotation §3.4 rule 9: the account's renewal policy, a note.
      return alert.on
        ? t("identity key renewals must now be confirmed on the device (set by {{by}})", { by })
        : t("identity key renewals may be automatic again (set by {{by}})", { by });
    case "rotateHostKey":
      // Host identity rotation §3.4 "告警": a note, with the new fingerprint as the device list shows it — and, as the
      // agent's notice says, whether the renewal was confirmed on the device itself (assurance 1).
      if (alert.renewed?.onDevice) {
        return fingerprint
          ? t("{{device}} renewed its identity key, confirmed on the device · new fingerprint {{fingerprint}}", { device, fingerprint })
          : t("{{device}} renewed its identity key, confirmed on the device", { device });
      }
      return fingerprint ? t("{{device}} renewed its identity key · new fingerprint {{fingerprint}}", { device, fingerprint }) : t("{{device}} renewed its identity key", { device });
  }
}

/**
 * The account's alerts on this device: every change to who is in the account
 * — a device added or removed, a recovery join, new recovery words — that
 * another device made and this one has not acknowledged. "That was me"
 * dismisses one here (and only here); where it applies, the device it is
 * about can be removed at once (which also retires the keys it held).
 */
export function ChainAlerts({ account, link }: { account: CloudAccount; link: HubLink }) {
  const { t } = useTranslation();
  useAccountVersion(account);
  const [busy, setBusy] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const alerts = account.alerts();
  // A renewed identity key is said with its new fingerprint, which takes a hash: worked out here, by the entry's seq.
  const renewals = alerts.filter((a) => a.kind === "rotateHostKey" && a.renewed);
  const renewalSeqs = renewals.map((a) => a.seq).join(",");
  const [prints, setPrints] = useState<ReadonlyMap<number, string>>(new Map());
  useEffect(() => {
    let live = true;
    void Promise.all(renewals.map(async (a) => [a.seq, await deviceFingerprint(a.renewed!.signKey, a.renewed!.dhKey)] as const)).then((list) => {
      if (live) setPrints(new Map(list));
    });
    return () => {
      live = false;
    };
    // The renewals are the entries at these seqs: nothing else changes what they say.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [renewalSeqs]);
  if (!alerts.length) return null;
  return (
    <div role="alert" data-testid="chain-alerts" data-signal="warn" className="flex flex-col border-b border-border bg-warn/7 shadow-[inset_2px_0_0_var(--color-warn)]">
      {alerts.map((a) => {
        const iso = new Date(a.atMs).toISOString();
        return (
          <div key={a.seq} data-testid={`alert-${a.seq}`} className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border px-4 py-2.5 last:border-b-0">
            {/* The words keep a readable width; on a narrow page the actions go under them. */}
            <span className="flex min-w-[min(100%,16rem)] flex-1 items-start gap-3">
              {alertLevel(a) === "high" ? (
                <span data-signal="alarm" data-level="high" className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-danger px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">
                  {t("important")}
                </span>
              ) : alertLevel(a) === "info" ? (
                <span data-level="info" className="mt-0.5 inline-flex h-4 shrink-0 items-center px-1 font-mono text-[10px] leading-none font-semibold text-fg shadow-[inset_0_0_0_1px_var(--color-border-strong)]">
                  {t("note")}
                </span>
              ) : (
                <span data-level="confirm" className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("check")}</span>
              )}
              <span className="min-w-0 flex-1 text-xs leading-5 text-muted">
                <span className="text-fg">{alertText(a, t, prints.get(a.seq))}</span>
                <span className="text-faint"> · </span>
                <span className="font-mono text-2xs text-faint" title={formatExactTime(iso)}>
                  {formatRelative(iso)}
                </span>
                <span className="text-faint"> · </span>
                {a.remove ? t("not you? remove it, then make new recovery words") : t("not you? check this account's devices")}
              </span>
            </span>
            <span className="flex shrink-0 flex-wrap items-center gap-2">
              <Button variant="secondary" disabled={busy !== null} onClick={() => account.acknowledge(a.seq)}>
                {t("that was me")}
              </Button>
              {a.remove && (
                <Button
                  variant="danger"
                  disabled={busy !== null || blockedByHold(account, a.remove)}
                  title={blockedByHold(account, a.remove) ? t("approve or remove the device whose activation is held back first") : undefined}
                  onClick={async () => {
                    setBusy(a.seq);
                    setError(null);
                    try {
                      await account.removeDevice(a.remove!, shellCourier(link, account), true);
                      account.acknowledge(a.seq);
                    } catch (err) {
                      setError(errorText(err));
                    } finally {
                      setBusy(null);
                    }
                  }}
                >
                  {busy === a.seq && <Loader2 className="animate-spin" />}
                  {t("remove {{id}}", { id: a.remove })}
                </Button>
              )}
            </span>
          </div>
        );
      })}
      {error && (
        <div className="px-4 pb-2.5">
          <ErrorLine message={error} />
        </div>
      )}
    </div>
  );
}

export type AddMode = "paste" | "scan" | "code";

/** How recently a device signed in for it to count as in use: never offered for removal ({@link retireOffer}). */
export const IN_USE_MS = 10 * 60_000;

/** The device a new one says it replaces, as offered for removal: what a person needs to recognise it. */
export interface RetireOffer {
  id: string;
  /** Its browser by name, as the hub has it (`uaLabelOf`): the same as the new device's, or it is not offered. */
  label: string;
  /** When it registered, and when it last signed in (`null`: not known). */
  addedMs: number | null;
  lastSeenMs: number | null;
}

/**
 * The device `added` says it takes the place of — its browser lost that
 * one's keys and said so when it registered (`/v1/me` `devices[].replaces`,
 * the new device's claim, nothing more) — when it is worth offering to
 * remove:
 * - a browser or phone the chain still lists as active: never this device,
 *   the new one, or one that runs or ran AI;
 * - not one whose removal voids the account's recovery words (it set them,
 *   or came back with them): that one is left to the list's own removal,
 *   which says so;
 * - the same browser as the new one, by the hub's name for each
 *   (`ua_label`): a browser that lost its keys is still the same browser,
 *   so a different (or unknown) one is not offered;
 * - not signed in now (`online`), nor within {@link IN_USE_MS}.
 * Else `null`. Only ever an offer: the removal is the person's to confirm.
 */
export function retireOffer({
  chain,
  devices,
  added,
  self,
  nowMs,
}: {
  chain: ChainState | null;
  devices: readonly RegisteredDevice[] | null | undefined;
  added: string;
  self: string;
  nowMs: number;
}): RetireOffer | null {
  const fresh = devices?.find((d) => d.id === added);
  const old = fresh?.replaces ?? null;
  if (!old || old === self || old === added || !chain) return null;
  const record = chain.device(old);
  if (!record || record.host || hasRunAi(chain, record)) return null;
  if (chain.voidsRecoveryWords(old, "removal")) return null;
  const row = devices?.find((d) => d.id === old);
  const label = uaLabelOf(row?.uaLabel);
  if (!label || label !== uaLabelOf(fresh?.uaLabel)) return null;
  if (row?.online === true) return null;
  if (row?.lastSeenMs != null && nowMs - row.lastSeenMs < IN_USE_MS) return null;
  return { id: old, label, addedMs: row?.createdMs ?? null, lastSeenMs: row?.lastSeenMs ?? null };
}

/**
 * After adding device `added`, which says it took the place of `offer` (its
 * browser lost that one's keys): the claim, put as the new device's, with
 * what tells the old device apart — its browser, when it was added and last
 * seen, its fingerprint — and one question: is that one no longer usable,
 * and may it go? Removing is the ordinary removal (`onRemove`), only on the
 * person's tap, and says what it rotated as the list's removal does, alarms
 * included; "not now" leaves it listed, to remove from the list later.
 * Exported for the render tests.
 */
export function RetireReplacedPrompt({
  added,
  offer,
  fingerprint,
  onRemove,
}: {
  added: string;
  offer: RetireOffer;
  fingerprint: string | null;
  onRemove: () => Promise<RotationReport>;
}) {
  const { t } = useTranslation();
  const [state, setState] = useState<"ask" | "busy" | "kept">("ask");
  const [report, setReport] = useState<RotationReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const device = offer.id;
  if (state === "kept") return null;
  if (report) {
    return (
      <div data-testid="retire-replaced-done" className="flex flex-col gap-1">
        <span className="font-mono text-xs text-ok">{t("removed {{id}}", { id: device })}</span>
        {reportLines(report, t).map((line, i) => (
          <span key={i} className={cn("font-mono text-2xs", alarmLine(report, line) ? "text-danger" : "text-muted")} data-signal={alarmLine(report, line) ? "alarm" : undefined}>
            {line}
          </span>
        ))}
      </div>
    );
  }
  const facts = [
    offer.label,
    offer.addedMs !== null ? t("added on {{when}}", { when: formatAsOf(offer.addedMs) }) : null,
    offer.lastSeenMs !== null ? t("seen {{when}}", { when: formatRelative(new Date(offer.lastSeenMs).toISOString()) }) : null,
  ].filter((x): x is string => !!x);
  return (
    <div role="alertdialog" data-testid="retire-replaced" className="flex flex-col gap-2 border border-border bg-warn/7 p-3 shadow-[inset_2px_0_0_var(--color-warn)]">
      <span className="text-xs leading-5 text-fg">
        {t("The new device {{new}} says it replaces {{old}}. Confirm that {{old}} can no longer be used, and remove it?", { new: added, old: device })}
      </span>
      <span data-testid="retire-replaced-facts" className="font-mono text-2xs text-muted">
        {facts.join(" · ")}
      </span>
      {fingerprint && <Fingerprint value={fingerprint} />}
      <ErrorLine message={error} />
      <div className="flex gap-2">
        <button
          type="button"
          data-testid="retire-replaced-remove"
          className={danger}
          disabled={state === "busy"}
          onClick={async () => {
            setState("busy");
            setError(null);
            try {
              setReport(await onRemove());
            } catch (err) {
              setError(errorText(err));
              setState("ask");
            }
          }}
        >
          {state === "busy" && <Loader2 className="size-3.5 animate-spin" />}
          {state === "busy" ? t("removing and rotating keys…") : t("remove {{id}}", { id: device })}
        </button>
        <button type="button" className={quiet} disabled={state === "busy"} onClick={() => setState("kept")}>
          {t("not now")}
        </button>
      </div>
    </div>
  );
}

/**
 * Pairing a browser or phone from this device — and approving one that
 * joined with a passkey and waits, the same way (`initialMode`: the tab it
 * opens on). Exported for the 「添加可运行 AI 的设备」 guide, whose sign-in
 * window may need it, and for the passkey join's notice.
 */
export function AddDevice({ account, link, onDone, initialMode = "paste" }: { account: CloudAccount; link: HubLink; onDone: () => void; initialMode?: AddMode }) {
  const { t } = useTranslation();
  const [mode, setMode] = useState<AddMode>(initialMode);
  const [candidate, setCandidate] = useState<Candidate | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [added, setAdded] = useState<string | null>(null);
  const me = useMe(account, link);
  // The plan's device limit (§4.3), as last read: said before anything is
  // scanned or typed. A device pairing again is not added, so it may go on.
  const full = atLimit(me?.plan, DEVICES_KEY);
  // Once a device is added: the one it says it replaces, if that is worth offering (never one an activation hold keeps), and its fingerprint.
  const retire = added ? retireOffer({ chain: account.chain, devices: me?.view?.devices, added, self: account.deviceId, nowMs: Date.now() }) : null;
  const retiring = retire && !blockedByHold(account, retire.id) ? retire : null;
  const [retiredPrint, setRetiredPrint] = useState<{ id: string; print: string } | null>(null);
  useEffect(() => {
    const record = retiring ? account.chain?.device(retiring.id) : undefined;
    if (!record) return;
    let live = true;
    void deviceFingerprint(record.info.signKey, record.info.dhKey).then((print) => {
      if (live) setRetiredPrint({ id: record.info.id, print });
    });
    return () => {
      live = false;
    };
  }, [account, retiring?.id]);

  const fromText = async (text: string) => {
    setError(null);
    try {
      setCandidate(await candidateFromOffer(text, account.chain!));
    } catch (err) {
      setError(errorText(err));
    }
  };

  if (added) {
    // The device it took the place of, if any (never one in use, nor while an activation is held back).
    const old = retiring;
    return (
      <div data-signal="ok" className="flex flex-col gap-2 border border-border p-3 shadow-[inset_2px_0_0_var(--color-ok)]">
        <span className="font-mono text-xs text-ok">{t("added {{id}}", { id: added })}</span>
        <p className="text-xs leading-5 text-muted">{t("Its keys are on their way: the new device says “paired” once it has checked them.")}</p>
        {old && (
          <RetireReplacedPrompt
            key={old.id}
            added={added}
            offer={old}
            fingerprint={retiredPrint?.id === old.id ? retiredPrint.print : null}
            onRemove={() => account.removeDevice(old.id, shellCourier(link, account))}
          />
        )}
        <button type="button" className={quiet} onClick={onDone}>
          {t("done")}
        </button>
      </div>
    );
  }
  if (candidate) {
    return (
      <ConfirmCandidate
        account={account}
        me={me}
        candidate={candidate}
        onAdded={() => setAdded(candidate.device)}
        onCancel={() => {
          abandonCandidate(candidate);
          setCandidate(null);
        }}
      />
    );
  }
  return (
    <div className="flex flex-col gap-2 border border-border p-3">
      {full && <LimitNote hit={full} />}
      <Tabs
        value={mode}
        onChange={(m) => {
          setMode(m);
          setError(null);
        }}
        options={[
          { id: "paste", label: t("paste its text") },
          { id: "scan", label: t("scan its QR code") },
          { id: "code", label: t("enter a code") },
        ]}
      />
      {mode === "paste" && <PasteOffer onText={(text) => void fromText(text)} />}
      {mode === "scan" && (
        <QrScanner
          accept={(text) => {
            try {
              offerFromText(text);
              return true;
            } catch {
              return false;
            }
          }}
          onText={(text) => void fromText(text)}
        />
      )}
      {mode === "code" && <EnterCode account={account} onCandidate={setCandidate} />}
      <ErrorLine message={error} />
      <button type="button" className={quiet} onClick={onDone}>
        {t("cancel")}
      </button>
    </div>
  );
}

function PasteOffer({ onText }: { onText: (text: string) => void }) {
  const { t } = useTranslation();
  const [text, setText] = useState("");
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        if (text.trim()) onText(text);
      }}
    >
      <p className="text-xs leading-5 text-muted">{t("On the new browser, sign in to this account and copy the text under its QR code.")}</p>
      <textarea
        value={text}
        onChange={(e) => setText(e.target.value)}
        rows={3}
        spellCheck={false}
        autoComplete="off"
        placeholder={t("the text under its QR code")}
        className={cn(field, "h-auto py-1.5 break-all")}
      />
      <div className="flex gap-2">
        <button type="submit" data-variant="primary" className={primary} disabled={!text.trim()}>
          <ClipboardPaste className="size-3.5" />
          {t("read it")}
        </button>
        {navigator.clipboard?.readText && (
          <button
            type="button"
            className={quiet}
            onClick={() => {
              void navigator.clipboard.readText().then((v) => {
                setText(v);
                if (v.trim()) onText(v);
              });
            }}
          >
            {t("paste from clipboard")}
          </button>
        )}
      </div>
    </form>
  );
}

function EnterCode({ account, onCandidate }: { account: CloudAccount; onCandidate: (c: Candidate) => void }) {
  const { t } = useTranslation();
  const [pairings, setPairings] = useState<PairingSummary[] | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [wrong, setWrong] = useState(false);
  useEffect(() => {
    let live = true;
    const load = async () => {
      try {
        const list = await account.api.pairings(account.session.token);
        if (!live) return;
        setPairings(list);
        setPicked((p) => (p && list.some((x) => x.id === p) ? p : (list.at(-1)?.id ?? null)));
      } catch (err) {
        if (live) setError(errorText(err));
      }
    };
    void load();
    const timer = setInterval(() => void load(), 3000);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [account]);
  const digits = code.replace(/\D/g, "");
  const pairing = pairings?.find((p) => p.id === picked) ?? null;
  return (
    <form
      className="flex flex-col gap-2"
      onSubmit={async (e) => {
        e.preventDefault();
        if (busy || !pairing || digits.length !== 8) return;
        setBusy(true);
        setError(null);
        setWrong(false);
        try {
          onCandidate(await runCodeInitiator({ api: account.api, token: account.session.token, account: account.account }, pairing, digits));
        } catch (err) {
          // One guess per code: whatever went wrong, this code is spent.
          setWrong(true);
          setError(errorText(err));
          setCode("");
          setBusy(false);
        }
      }}
    >
      <p className="text-xs leading-5 text-muted">{t("On the new browser, choose “use a code instead”, then type the 8 digits it shows.")}</p>
      {pairings === null ? (
        <Loader2 className="size-4 animate-spin text-faint" />
      ) : pairings.length === 0 ? (
        <p className="flex items-center gap-2 text-2xs text-faint">
          <Loader2 className="size-3 animate-spin" />
          {t("no device is showing a code yet…")}
        </p>
      ) : (
        <ul className="flex flex-col gap-1">
          {pairings.map((p) => (
            <li key={p.id}>
              <label className="flex items-center gap-2 font-mono text-2xs text-muted">
                <input type="radio" name="pairing" checked={picked === p.id} onChange={() => setPicked(p.id)} />
                <span className="text-fg">{p.device}</span>
                <span className="text-faint">{formatRelative(new Date(p.createdMs).toISOString())}</span>
                {/* Any full device may close one it does not recognise (M2 re-review #28). */}
                <button
                  type="button"
                  className={cn(quiet, "ml-auto h-5 px-1.5 text-2xs")}
                  title={t("close this pairing (not a device of yours?)")}
                  onClick={() => {
                    void account.api
                      .closePairing(account.session.token, p.id)
                      .then(() => setPairings((list) => (list ?? []).filter((x) => x.id !== p.id)))
                      .catch((err: unknown) => setError(errorText(err)));
                  }}
                >
                  {t("close")}
                </button>
              </label>
            </li>
          ))}
        </ul>
      )}
      <input
        value={code}
        onChange={(e) => setCode(e.target.value)}
        inputMode="numeric"
        autoComplete="one-time-code"
        placeholder="0000 0000"
        maxLength={9}
        className={cn(field, "text-center text-base tracking-[0.3em]")}
      />
      {wrong && (
        <p className="flex items-start gap-1.5 text-2xs text-danger">
          <AlertTriangle className="mt-0.5 size-3 shrink-0" />
          {t("That did not work, and the code is spent. On the new device, start again for a new code.")}
        </p>
      )}
      <ErrorLine message={wrong ? null : error} />
      <button type="submit" data-variant="primary" className={primary} disabled={busy || !pairing || digits.length !== 8}>
        {busy ? <Loader2 className="size-3.5 animate-spin" /> : <Hash className="size-3.5" />}
        {t("check the code")}
      </button>
    </form>
  );
}

/**
 * The candidate's fingerprint, compared, and the pairing: a browser or phone
 * is added as a full device, always — there is no choice to make (chain v2's
 * full device, v3's admin). One that pairs again keeps what it is.
 * Exported for the render tests.
 */
export function ConfirmCandidate({
  account,
  me,
  candidate,
  onAdded,
  onCancel,
}: {
  account: Pick<CloudAccount, "chain" | "pairDevice"> & Partial<Pick<CloudAccount, "onChange">>;
  me: ReturnType<typeof useMe>;
  candidate: Candidate;
  onAdded: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  // The chain as it moves on while this is open (a join replaced meanwhile is said at once).
  useAccountVersion(account.onChange ? (account as CloudAccount) : null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [limit, setLimit] = useState<LimitHit | null>(null);
  const record = account.chain?.deviceRecord(candidate.device);
  // Waiting after a passkey join when this was read: approving it is what this does.
  const [joining] = useState(() => record?.viaPasskey != null && record.status.type === "pending");
  // …and a newer join (or a removal) took its place meanwhile: nothing to approve any more.
  const replaced = joining && !busy && record?.status.type !== "pending";
  if (replaced) {
    return (
      <div data-testid="join-replaced" className="flex flex-col gap-2 border border-fg p-3">
        <Note tone="danger">
          <p className="text-xs leading-5 text-muted">
            {t("{{id}} no longer waits to join: a newer join with the same passkey replaced it, or a device removed it. Nothing was approved.", { id: candidate.device })}
          </p>
        </Note>
        <button type="button" className={quiet} onClick={onCancel}>
          {t("close")}
        </button>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2 border border-fg p-3">
      <span className="font-mono text-xs text-fg">
        {joining
          ? t("approve {{id}}, which asked to join with a passkey?", { id: candidate.device })
          : record
            ? t("{{id}} is in this account already: send it its keys again?", { id: candidate.device })
            : t("add {{id}} to this account?", { id: candidate.device })}
      </span>
      <p className="text-xs leading-5 text-muted">{t("Check that the new device shows the same fingerprint:")}</p>
      <div className="border border-border bg-raised p-2.5 text-center">
        <Fingerprint value={candidate.fingerprint} />
      </div>
      {limit && <LimitNote hit={limit} action={{ label: t("devices"), onClick: onCancel }} />}
      <ErrorLine message={error} />
      <div className="flex gap-2">
        <button
          type="button"
          data-variant="primary" className={primary}
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError(null);
            setLimit(null);
            try {
              // An addition (not a device pairing again): the plan's device
              // limit, freshly read, before anything is signed (§4.3).
              if (me && !account.chain?.deviceRecord(candidate.device)) {
                try {
                  await me.refresh();
                } catch (err) {
                  console.warn("cloud: /v1/me before adding a device:", err);
                }
                const hit = atLimit(me.plan, DEVICES_KEY);
                if (hit) {
                  setLimit(hit);
                  setBusy(false);
                  return;
                }
              }
              // Every browser or phone is a full device (the owner's decision of 2026-10-20).
              await account.pairDevice(candidate, { admin: true });
              onAdded();
              me?.soon();
            } catch (err) {
              const hit = limitOf(err);
              if (hit) setLimit(hit);
              else setError(errorText(err));
              setBusy(false);
            }
          }}
        >
          {busy && <Loader2 className="size-3.5 animate-spin" />}
          {joining ? t("the fingerprints match: approve it") : t("the fingerprints match: add it")}
        </button>
        <button type="button" className={quiet} disabled={busy} onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
    </div>
  );
}

/**
 * While an activation is held back, removing anyone but the device it
 * activates would go on the hub's chain past it (M2 re-review N1): only the
 * veto of that device is offered, and it takes out what the device brought in.
 */
export function blockedByHold(account: Pick<CloudAccount, "heldBack">, device: string): boolean {
  const held = account.heldBack;
  return !!held && held.device !== device;
}

/** What approving a held-back activation takes in: the hub's entries from it on. */
function HeldChanges({ account }: { account: CloudAccount }) {
  const { t } = useTranslation();
  const changes = account.heldChanges().filter((c) => !((c.kind === "activateRecovered" || c.kind === "passkeyActivate") && c.subject === account.heldBack?.device));
  if (!changes.length) return null;
  const say = (c: HeldChange): string => {
    const by = c.by ?? t("the recovery words");
    const subject = c.subject ?? "";
    switch (c.kind) {
      case "addDevice":
        return t("{{by}} added {{subject}}", { by, subject });
      case "removeDevice":
        return t("{{by}} removed {{subject}}", { by, subject });
      case "approveDevice":
        return t("{{by}} approved {{subject}}", { by, subject });
      case "activateRecovered":
      case "passkeyActivate":
        return t("{{subject}} activated itself", { subject });
      case "recoveryJoin":
        return t("{{subject}} joined with the recovery words", { subject });
      case "addMachine":
        return t("{{by}} added {{subject}} (can run AI)", { by, subject });
      case "removeMachine":
        return t("{{by}} removed {{subject}} (can run AI)", { by, subject });
      case "setRecoveryKey":
        return t("{{by}} changed the recovery words", { by });
      case "keyEpochs":
        return t("{{by}} made new keys", { by });
      default:
        return c.kind;
    }
  };
  return (
    <span data-testid="held-changes" className="mt-1 block text-2xs text-muted">
      {t("After it, the hub's chain has:")}{" "}
      {changes.map((c, i) => (
        <span key={c.seq} className="font-mono text-fg">
          {i ? "; " : ""}
          {say(c)}
        </span>
      ))}
      {". "}
      {t("Approving takes all of it in; removing takes out the device and what it brought in.")}
    </span>
  );
}

/**
 * While someone is coming back with the recovery words, every full device
 * says so — with the means to approve it, or veto it (remove it, which also
 * retires the keys it opened).
 */
export function PendingRecoveryBanner({ account, link }: { account: CloudAccount; link: HubLink }) {
  const { t } = useTranslation();
  useAccountVersion(account);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const pending = account.standing === "active" ? (account.chain?.pendingRecoveries() ?? []) : [];
  if (!pending.length) return null;
  return (
    <div role="alert" data-testid="pending-recovery" data-signal="alarm" className="flex flex-col gap-1.5 border-b border-border bg-danger/4 px-4 py-2.5 shadow-[inset_2px_0_0_var(--color-danger)]">
      {pending.map((p) => (
        <div key={p.id} className="flex flex-wrap items-center gap-2 text-xs">
          <AlertTriangle className="size-3.5 shrink-0 text-danger" />
          <span className="min-w-0 flex-1 text-muted">
            {t("A device is coming back into this account with the recovery words:")} <span className="font-mono text-fg">{p.id}</span>.{" "}
            {account.heldBack?.device === p.id
              ? t("The hub says it already activated itself, but this browser only just saw it come back and holds it back until {{time}}. If that was not you, remove it now; otherwise approve it.", {
                  time: formatExactTime(new Date(account.heldBack.readyMs).toISOString()),
                })
              : t("If that was not you, remove it now; otherwise approve it (or it activates itself after 24 hours).")}
            {account.heldBack?.device === p.id && <HeldChanges account={account} />}
          </span>
          <button
            type="button"
            className={cn(primary, "h-7 px-3 text-xs")}
            disabled={busy !== null}
            onClick={async () => {
              setBusy(p.id);
              setError(null);
              try {
                await account.approveDevice(p.id);
              } catch (err) {
                setError(errorText(err));
              } finally {
                setBusy(null);
              }
            }}
          >
            {t("approve")}
          </button>
          <button
            type="button"
            className={cn(danger, "h-7 px-3 text-xs")}
            disabled={busy !== null || blockedByHold(account, p.id)}
            title={blockedByHold(account, p.id) ? t("approve or remove the device whose activation is held back first") : undefined}
            onClick={async () => {
              setBusy(p.id);
              setError(null);
              try {
                await account.removeDevice(p.id, shellCourier(link, account));
              } catch (err) {
                setError(errorText(err));
              } finally {
                setBusy(null);
              }
            }}
          >
            {busy === p.id && <Loader2 className="size-3 animate-spin" />}
            {account.heldBack?.device === p.id ? t("remove it and what it brought in") : t("remove")}
          </button>
        </div>
      ))}
      <ErrorLine message={error} />
    </div>
  );
}

