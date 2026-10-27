import { type ReactNode, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, ArrowRight, ChevronRight, KeyRound, Loader2, MonitorSmartphone, Plus, Server } from "lucide-react";
import { osLabel, versionLabel } from "@/features/machines/device-facts";
import { useDeviceSessions } from "@/features/machines/device-sessions";
import { needsYou, recentSessions } from "@/features/machines/needs-you";
import { NeedsYouLedger } from "@/features/machines/needs-you-ledger";
import { type DeviceRecord, hex } from "@/lib/e2e";
import { type CloudAccount, deviceFingerprint, hasRunAi, hostsExposedBy, limitedDevice, type ReEnrollLine, type RotationReport } from "@/lib/hub/account";
import { chainV3Writable } from "@/lib/hub/api";
import { hubOrigin } from "@/lib/hub/config";
import type { MachineKeyCourier } from "@/lib/hub/deliver";
import type { HubLink, PresenceState } from "@/lib/hub/link";
import { deviceMirrorStore } from "@/lib/hub/mirror";
import type { StatusState } from "@/lib/hub/streams";
import { uaLabel, uaLabelOf } from "@/lib/hub/ua-label";
import { useHomeMachine } from "@/lib/machines/context";
import type { MachineInfo } from "@/lib/machines/runtime";
import { useStore } from "@/lib/machines/store";
import { cn } from "@/lib/cn";
import { copyWarning, offlineSince, retentionOf } from "@/lib/hub/plan";
import { AddComputer, NewDeviceReady } from "./add-computer";
import { useAgentKeyState, usePresenceVersion, useShellCopies } from "./context";
import { DeviceRow, type DeviceRowView, shortDeviceId } from "./device-row";
import { askedPlace } from "./devices-route";
import { FirstHostCard } from "./first-host";
import { sessionPath, shellCourier } from "./hub-shell";
import { hubShell, useFocusedMachine, useShellMachines } from "@/features/machines/shells";
import { MachineNotes, noteRenamed, RenameMachineForm, renameThroughNewChannel, shownName, useMachineStatuses, useRenamed } from "./machine-name";
import { addAsked } from "./onboarding";
import { AddDeviceHint } from "./onboarding-hints";
import { RelaySection } from "./relays";
import { AlwaysConfirmSetting, EndPauseConfirm, RenewalsAhead, RenewPanel, useRenewals } from "./renewal";
import { UsageSummary, useMe } from "./plan";
import { RecoveryWords } from "./recovery";
import { danger, ErrorLine, errorText, Fingerprint, Note, primary, quiet, secondary, SectionHead } from "./ui";
import { Button } from "@/components/ui/button";
import { CHECKBOX_INPUT, CHECKBOX_LABEL, CheckboxMark } from "@/components/ui/checkbox";
import { useAccountVersion } from "./context";
import { AddDevice, blockedByHold } from "./add-device";
import { HostControl } from "./host-control";
import { ReportLines } from "./rotation-report";
import { RelayStandbySetting, ThisDeviceSection } from "./this-device";
import { RotateKeys } from "./account-alerts";
// Moved to their own modules; re-exported so callers keep one path.
export { AddDevice, blockedByHold };
export { ConfirmCandidate, IN_USE_MS, RetireReplacedPrompt, retireOffer } from "./add-device";
export { HostControl };
export { ControlConfirm, controlSaveStep } from "./host-control";
export { ReportLines };
export { RelayStandbySetting, ThisDeviceSection };
export { DirectConnectionSetting } from "./this-device";
export { RotateKeys };
export { AccountNotices, alertLevel, alertText, ChainAlerts, LimitedDeviceNote, MissingUserKey, PendingRecoveryBanner, UpgradeAccount, UpgradeConfirm, V3Note, VetoChoice, alsoTrustedFor, trustedShown } from "./account-alerts";
export type { AddMode, RetireOffer } from "./add-device";
export { useAccountVersion };
/*
 * 「我的设备」 on droppen.app, from an active device (architecture doc §1;
 * docs/local-aggregator.md §3.7), one page (`my-devices.tsx` draws the frame:
 * the alerts, the way back): what the plan uses (`UsageSummary`), this
 * device's own settings (`ThisDeviceSection`), then every device the chain
 * lists in one list — this one marked, the devices that run AI among them
 * with the way into each, one waiting after a recovery; removed ones folded
 * below (`device-row.tsx` draws a row). Until the account has a device that
 * runs AI (just signed up), a first-run card leads with how to connect one
 * (`first-host.tsx`), and this device's settings and the keys' rotation wait
 * folded under 「更多设置」 (`MoreSettings`).
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
  /** The 「添加可运行 AI 的设备」 guide; `code`: opened on its one-time code (a device with no browser). */
  | { kind: "addComputer"; code?: boolean }
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
}

/**
 * 「我的设备」: what the plan uses, this device's own settings, every device
 * of the account as one list — the ones that run AI among them, each with
 * the way into it — and what can be done with them.
 */
export function DevicesPanel({ account, link }: DevicesPanelProps) {
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
  // Just signed up — the chain read, this a full device, no device that runs AI in it: the first-run card, and 「更多设置」 folded.
  const noHostYet = chain != null && viewerAdmin && hostIds.length === 0;
  const adding = section.kind === "add" || section.kind === "addBrowser" || section.kind === "addComputer";
  const cardNow = noHostYet && !adding;
  // The card goes once a device that runs AI joins: said, once, as the guide says it (the guide, open, says it itself).
  const [cardUp, setCardUp] = useState(cardNow);
  const [joined, setJoined] = useState<string | null>(null);
  if (cardNow !== cardUp) {
    setCardUp(cardNow);
    if (cardUp && chain != null && hostIds.length > 0) setJoined(hostIds[0]!);
  }

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
  const open = (id: string) => shell?.select(id);
  const openAt = (id: string, path: string) => shell?.openAt(id, path);
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
  // The header's one sentence of counts, not a generic security line (§3).
  const offlineNow = current.filter((d) => {
    const v = view(d);
    return v.host && v.status === "active" && v.presence === "offline";
  }).length;
  const needsNow = needs.entries.length;
  const summary = [
    t("{{n}} devices", { n: current.length }),
    offlineNow > 0 ? t("{{n}} offline", { n: offlineNow }) : null,
    needsNow > 0 ? t("{{n}} need you", { n: needsNow }) : null,
  ]
    .filter((part): part is string => part !== null)
    .join(" · ");
  // No device that runs AI yet (just signed up): adding one is what comes next.
  const firstHost = hostsNow === 0;
  // The device that joined while the first-run card was up, as the page follows it (its name, whether it is up).
  const joinedInfo: MachineInfo | null = joined
    ? (machines.find((m) => m.key === joined) ?? { key: joined, name: shownName(joined, statuses.get(joined) ?? null) ?? joined, state: "unknown", since: 0, canManage: true, live: "copy" })
    : null;
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

  // Just signed up: the card that says what comes next, until the first device that runs AI joins (or another way to add one is open).
  const firstRun = cardNow;
  // The word that it joined, in the card's place, until put away.
  const justJoined = joinedInfo !== null && section.kind === "list";
  const pairWith = (done: () => void) => <AddDevice account={account} link={link} onDone={done} />;
  // This device's own settings and the keys': folded under 「更多设置」 while there is no device that runs AI to use them with.
  const thisDevice = (
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
  );
  const rotateKeys = viewerAdmin && !account.rotationDue && section.kind === "list" ? <RotateKeys account={account} link={link} due={false} /> : null;
  const alwaysConfirm = section.kind === "list" && viewerAdmin && upgraded && renewals?.view?.enabled ? <AlwaysConfirmSetting runner={renewals} account={account} /> : null;
  return (
    <>
      <div className="flex flex-wrap items-end justify-between gap-x-6 gap-y-4 px-6 pt-8 pb-2 max-md:px-4 max-md:pt-6">
        <div className="flex min-w-0 flex-col gap-1.5">
          {/* Summary first: what the account has, before anything generic. */}
          <h1 data-testid="devices-title" className="text-[32px] leading-[1.1] font-semibold tracking-[-0.03em] text-fg max-md:text-[28px]">
            {t("my devices")}
          </h1>
          <p data-testid="devices-summary" className="text-[13px] leading-5 text-muted">
            {summary}
          </p>
        </div>
        {viewerAdmin && !adding && (
          // One primary action a view: while a rotation is due, that is it; while the first-run card shows, the card's.
          <button
            type="button"
            data-variant={account.rotationDue || firstRun || justJoined ? "secondary" : "primary"}
            data-testid="add-device"
            className={cn(account.rotationDue || firstRun || justJoined ? secondary : primary, "max-md:w-full")}
            onClick={() => setSection(firstHost ? { kind: "addComputer" } : { kind: "add" })}
          >
            <Plus className="size-3.5" />
            {t("add a device")}
          </button>
        )}
      </div>
      <div className="flex flex-col gap-8 pb-12">
      {section.kind === "add" && <AddChooser onPick={(kind) => setSection({ kind })} onCancel={() => setSection({ kind: "list" })} />}
      {section.kind === "addBrowser" && (
        <section data-testid="add-browser-section">
          <AddHead title={t("add a browser or phone")} onBack={() => setSection({ kind: "add" })} />
          <div className="flex max-w-xl flex-col gap-3 px-6 py-5 max-md:px-4">
            <AddDevice account={account} link={link} onDone={() => setSection({ kind: "list" })} />
          </div>
        </section>
      )}
      {section.kind === "addComputer" && (
        <section data-testid="add-computer-section">
          <AddHead title={t("add a device that runs AI")} onBack={() => setSection({ kind: "add" })} />
          {/* Two ways to add (§3.7): the device's own local settings first, a one-time code second. */}
          <AddComputer
            account={account}
            link={link}
            origin={hubOrigin() ?? ""}
            onDone={() => setSection({ kind: "list" })}
            pairWith={pairWith}
            initialMode={section.code ? "code" : "guide"}
          />
        </section>
      )}
      {firstRun && <FirstHostCard onGuide={() => setSection({ kind: "addComputer" })} onCode={() => setSection({ kind: "addComputer", code: true })} pairWith={pairWith} />}
      {justJoined && (
        <section data-testid="first-host-joined">
          <NewDeviceReady account={account} added={joinedInfo} machines={machines} onDone={() => setJoined(null)} />
        </section>
      )}
      <UsageSummary account={account} link={link} />
      {!noHostYet && thisDevice}
      <section data-testid="devices-section" className="@container/devices">
        <div className="px-6 max-md:px-4">
          <SectionHead index="03" title={t("devices")}>
            <span className="font-mono text-xs text-faint tabular-nums">{current.length}</span>
          </SectionHead>
        </div>
        {current.length > 0 ? (
          <ul
            data-testid="device-list"
            className="mt-4 grid grid-cols-1 items-start gap-2.5 px-6 max-md:px-4 @min-[44rem]/devices:grid-cols-[repeat(auto-fill,minmax(20rem,1fr))] @min-[44rem]/devices:gap-3"
          >
            {current.map(row)}
          </ul>
        ) : (
          <div data-testid="devices-empty" className="flex flex-col items-start gap-3 px-6 pt-4 max-md:px-4">
            <p className="text-[13px] leading-5 text-muted">{t("no devices yet")}</p>
            {viewerAdmin && !adding && (
              <button type="button" data-testid="add-device-empty" className={secondary} onClick={() => setSection({ kind: "add" })}>
                <Plus className="size-3.5" />
                {t("add a device")}
              </button>
            )}
          </div>
        )}
      </section>
      {!noHostYet && alwaysConfirm}
      {section.kind === "list" && done && (
        <div data-testid="change-report" data-signal="ok" className="mx-6 my-4 flex flex-col gap-1 border border-border p-3 shadow-[inset_2px_0_0_var(--color-ok)] max-md:mx-4">
          <span className="font-mono text-xs text-ok">{done.title}</span>
          <ReportLines report={done.report} t={t} />
          <Button variant="ghost" className="self-start" onClick={() => setDone(null)}>
            {t("done")}
          </Button>
        </div>
      )}
      {/* One device that runs AI: a second one puts both on one screen (put away with its ×). */}
      {!adding && <AddDeviceHint variant="row" onAdd={() => setSection({ kind: "addComputer" })} className="max-md:mt-2" />}
      {needs.entries.length > 0 && (
        <NeedsYouLedger
          needs={needs}
          nameOf={(key) => shownName(key, statuses.get(key) ?? null) ?? t("device {{id}}", { id: shortDeviceId(key) })}
          devices
          note={t("from each device's encrypted index")}
          onOpen={(entry) => openAt(entry.ref.machine, sessionPath(entry.ref.source, entry.ref.id))}
        />
      )}
      <div className="flex max-w-xl flex-col gap-3 px-6 max-md:px-4">
        {!noHostYet && rotateKeys}
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
      {noHostYet && (
        <MoreSettings>
          {thisDevice}
          {alwaysConfirm}
          {rotateKeys && <div className="flex max-w-xl flex-col gap-3 px-6 pb-6 max-md:px-4">{rotateKeys}</div>}
        </MoreSettings>
      )}
      {removed.length > 0 && (
        <details data-testid="removed-devices" className="group/removed">
          {/* A folded aside, not a section: 13px, muted, with its count. */}
          <summary className="flex h-10 cursor-pointer list-none items-center gap-2 px-6 text-muted select-none hover:text-fg max-md:px-4 [&::-webkit-details-marker]:hidden">
            <ChevronRight aria-hidden="true" className="size-3 transition-transform group-open/removed:rotate-90" />
            <span className="text-[13px] leading-5 font-medium">{t("removed devices ({{n}})", { n: removed.length })}</span>
          </summary>
          <ul className="mt-2 grid grid-cols-1 items-start gap-2.5 px-6 max-md:px-4 @min-[44rem]/devices:grid-cols-[repeat(auto-fill,minmax(20rem,1fr))] @min-[44rem]/devices:gap-3">
            {removed.map(row)}
          </ul>
        </details>
      )}
      </div>
    </>
  );
}

/** The head of an add flow: what is being added, and the way back to the choice. */
function AddHead({ title, onBack }: { title: string; onBack: () => void }) {
  const { t } = useTranslation();
  return (
    <div className="flex h-12 items-center gap-3 border-b border-border px-6 max-md:px-4">
      <button type="button" onClick={onBack} title={t("back")} aria-label={t("back")} className="flex size-6 items-center justify-center text-muted hover:bg-hover hover:text-fg pointer-coarse:size-9">
        <ArrowLeft className="size-3.5" />
      </button>
      <h2 className="text-base leading-6 font-semibold text-fg">{title}</h2>
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
    <section data-testid="add-chooser">
      <div className="flex h-12 items-center justify-between gap-3 border-b border-border px-6 max-md:px-4">
        <h2 className="text-base leading-6 font-semibold text-fg">{t("add a device")}</h2>
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
 * 「更多设置」: what 「我的设备」 keeps folded while the account has no device
 * that runs AI — this device's direct connections and relays, and the keys'
 * rotation: nothing to use them with yet, and the first-run card is what
 * matters. Open when a way in asked for the relays (“change relay…”).
 */
export function MoreSettings({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const asked = useStore(askedPlace);
  const [open, setOpen] = useState(() => askedPlace.get()?.anchor === "relays");
  useEffect(() => {
    if (asked?.anchor === "relays") setOpen(true);
  }, [asked]);
  return (
    <details data-testid="more-settings" open={open} onToggle={(e) => setOpen(e.currentTarget.open)} className="group/more">
      <summary className="flex h-10 cursor-pointer list-none items-center gap-2 px-6 text-muted select-none hover:text-fg max-md:px-4 [&::-webkit-details-marker]:hidden">
        <ChevronRight aria-hidden="true" className="size-3 transition-transform group-open/more:rotate-90" />
        <span className="text-[13px] leading-5 font-medium">{t("more settings")}</span>
        <span className="min-w-0 truncate text-xs">{t("connections, relays and keys")}</span>
      </summary>
      {children}
    </details>
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
        <ReportLines report={report} t={t} />
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
        <label className={cn(CHECKBOX_LABEL, rotation !== "choice" && "cursor-not-allowed opacity-60")}>
          <input
            type="checkbox"
            className={CHECKBOX_INPUT}
            data-testid="rotate-machine-keys"
            checked={rotation !== "choice" || rotateMachineKeys}
            disabled={rotation !== "choice"}
            onChange={(e) => setRotateMachineKeys(e.target.checked)}
          />
          <CheckboxMark />
          <span className="min-w-0">
            {rotation === "all"
              ? t("also rotate the content keys (always, for a device that held the user key)")
              : rotation === "machine"
                ? t("also rotate the content keys (always, for a device another device accepts control from)")
                : t("also rotate the content keys")}
          </span>
        </label>
      )}
      {rotation !== "none" && (rotation !== "choice" || rotateMachineKeys) && (
        <label className={CHECKBOX_LABEL}>
          <input type="checkbox" className={CHECKBOX_INPUT} checked={reencrypt} onChange={(e) => setReencrypt(e.target.checked)} />
          <CheckboxMark />
          <span className="min-w-0">
            {t("also re-encrypt history: each device that runs AI rebuilds its encrypted copy under the new key and the old one is deleted")}
          </span>
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
