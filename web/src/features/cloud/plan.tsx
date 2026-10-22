import { type ReactNode, useEffect, useState, useSyncExternalStore } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight } from "lucide-react";
import { cn } from "@/lib/cn";
import { formatExactTime, formatMoment, formatRelative, formatResetAt } from "@/lib/format";
import type { CloudAccount } from "@/lib/hub/account";
import type { PlanView, SignInChange } from "@/lib/hub/api";
import type { HubLink } from "@/lib/hub/link";
import { type MeStore, meStore } from "@/lib/hub/me";
import { deviceMirrorStore } from "@/lib/hub/mirror";
import {
  type CopyWarning,
  DEVICES_KEY,
  downgradeOf,
  formatPlanBytes,
  formatStoragePair,
  HOSTS_KEY,
  hostsLimit,
  type LimitHit,
  MACHINES_KEY,
  registrationAlerts,
  retentionOf,
  STORAGE_KEY,
  storageView,
} from "@/lib/hub/plan";
import { copyWindow } from "@/lib/hub/streams";
import { Button } from "@/components/ui/button";
import { CapsLabel } from "@/components/ui/caps-label";
import { useMachine } from "@/lib/machines/context";
import { mirrorFor, useShellCopies, useShowingCopy } from "./context";
import { hubShell } from "./hub-shell";
import { shownName, useMachineStatuses } from "./machine-name";

/*
 * What the account's plan means, where it matters (relay-and-plans §3.8,
 * §4.3): the device and machine limits where devices and machines are added;
 * the cloud storage with each machine's share and, plainly, how long a
 * machine's offline copy is kept once it is away; a downgrade's end and what
 * it takes away; and the devices someone registered with the password.
 *
 * Every number is the plan's (`lib/hub/plan.ts`), kept live by the hub link
 * (`lib/hub/me.ts`). No "upgrade" anywhere yet: there is nothing to pay with.
 */

type T = (key: string, opts?: Record<string, unknown>) => string;

/** The live `/v1/me` for `account` (read when first shown, then kept current through `link`). */
export function useMe(account: CloudAccount | null | undefined, link?: HubLink | null): MeStore | null {
  const store = account ? meStore(account, link) : null;
  useSyncExternalStore(
    (notify) => (store ? store.onChange(notify) : () => undefined),
    () => store?.version ?? 0,
    () => 0,
  );
  useEffect(() => {
    store?.refreshIfStale();
  }, [store]);
  return store;
}

/** A plan's name as people read it. */
export function planLabel(id: string, t: T): string {
  if (id === "free") return t("Free plan");
  if (id === "pro") return t("Pro plan");
  if (id === "internal") return t("Internal plan");
  return id;
}

/** What a used-up device or machine limit (or a full storage) says. */
export function limitText(hit: LimitHit, t: T): string {
  if (hit.key === DEVICES_KEY) return t("Device limit reached ({{n}} devices): first remove an old device in my devices, on a device already in the account", { n: hit.limit ?? hit.used });
  if (hit.key === HOSTS_KEY || hit.key === MACHINES_KEY) return t("Limit reached for devices that can run AI ({{n}})", { n: hit.limit ?? hit.used });
  if (hit.key === STORAGE_KEY) return t("Cloud storage is full");
  return hit.key;
}

/** A limit hit, said where the adding was, with the way on (to the devices page, back to the machine list). */
export function LimitNote({ hit, action }: { hit: LimitHit; action?: { label: string; onClick: () => void } }) {
  const { t } = useTranslation();
  return (
    <div
      role="alert"
      data-testid="plan-limit"
      data-limit={hit.key}
      data-signal="warn"
      className="flex flex-wrap items-center gap-x-3 gap-y-2 border border-border bg-warn/7 p-3 shadow-[inset_2px_0_0_var(--color-warn)]"
    >
      <span className="flex min-w-[min(100%,14rem)] flex-1 items-start gap-2">
        <span className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("limit")}</span>
        <span className="min-w-0 flex-1 text-xs leading-5 text-fg">{limitText(hit, t)}</span>
      </span>
      {action && (
        <Button variant="secondary" onClick={action.onClick}>
          {action.label}
        </Button>
      )}
    </div>
  );
}

/** The machine list's line for a machine away for days: its cloud copy goes soon, or went (§3.8). */
export function CopyWarningLine({ warning, plan }: { warning: CopyWarning; plan: string }) {
  const { t } = useTranslation();
  const deleted = warning.kind === "deleted";
  return (
    <span
      data-testid="copy-warning"
      data-copy={warning.kind}
      data-signal={deleted ? "offline" : "warn"}
      className="flex min-w-0 items-start gap-2 text-2xs leading-4 text-muted"
    >
      <span
        className={cn(
          "inline-flex h-4 shrink-0 items-center px-1 font-mono text-[10px] leading-none font-semibold text-on-signal",
          deleted ? "bg-danger" : "bg-warn",
        )}
      >
        {t("offline copy")}
      </span>
      <span className="min-w-0">
        {warning.kind === "soon"
          ? t("Offline for {{days}} days: in {{left}} days the cloud offline copy will be deleted ({{plan}} keeps {{retention}} days)", {
              days: warning.offlineDays,
              left: warning.daysLeft,
              plan: planLabel(plan, t),
              retention: warning.retentionDays,
            })
          : t("Offline for more than {{retention}} days: the cloud offline copy was deleted. Once the device is back online, the last {{retention}} days come back on their own.", {
              retention: warning.retentionDays,
            })}
      </span>
    </span>
  );
}

/** Said once the first machine is added: what the offline copy is, and how long it lasts. */
export function FirstMachineNote({ plan }: { plan: PlanView | null }) {
  const { t } = useTranslation();
  const days = retentionOf(plan);
  if (!plan || days === null) return null;
  return (
    <p data-testid="first-machine-note" className="text-xs leading-5 text-muted">
      {t("Offline, you can still read the last {{days}} days of sessions.", { days })}{" "}
      {t("{{plan}}: a device offline for more than {{days}} days has its cloud copy deleted.", { plan: planLabel(plan.id, t), days })}
    </p>
  );
}

/** The storage-full line (the offline-copy notice, the settings): when the copies stopped, if known. */
export function storageFullText(stoppedMs: number | null, t: T): string {
  return stoppedMs === null
    ? t("Cloud storage is full: everything works while the device is online, but the offline copy is no longer updated")
    : t("Cloud storage is full: everything works while the device is online, the offline copy stopped at {{time}}", {
        time: formatResetAt(new Date(stoppedMs).toISOString()),
      });
}

/** "Pro plan ends on 24 Oct: from then on …" — the downgrade's end and what it takes away. */
export function DowngradeText({ plan, now = Date.now() }: { plan: PlanView | null; now?: number }) {
  const { t, i18n } = useTranslation();
  const down = downgradeOf(plan, now);
  if (!down) return null;
  const date = formatDate(down.untilMs, i18n.language);
  const from = planLabel(down.from, t);
  return (
    <>
      {down.after && down.after.transferBytes !== null
        ? t(
            "{{plan}} ends on {{date}}: from then on the cloud keeps only {{days}} days (a device offline for more than {{days}} days has its copy deleted), storage up to {{storage}}, transfer {{transfer}} a month",
            {
              plan: from,
              date,
              days: down.after.retentionDays,
              storage: formatPlanBytes(down.after.storageBytes),
              transfer: formatPlanBytes(down.after.transferBytes),
            },
          )
        : t("{{plan}} ends on {{date}}", { plan: from, date })}
    </>
  );
}

function formatDate(ms: number, lang: string): string {
  try {
    return new Intl.DateTimeFormat(lang, { month: "long", day: "numeric" }).format(new Date(ms));
  } catch {
    return new Date(ms).toISOString().slice(0, 10);
  }
}

/** A strip across the page (the machine list): amber or red, a chip, the words. */
function Strip({ tone, chip, testid, children }: { tone: "warn" | "danger"; chip: string; testid: string; children: ReactNode }) {
  return (
    <div
      role="alert"
      data-testid={testid}
      data-signal={tone === "warn" ? "warn" : "offline"}
      className={cn(
        "flex items-start gap-3 border-b border-border px-4 py-2.5",
        tone === "warn" ? "bg-warn/7 shadow-[inset_2px_0_0_var(--color-warn)]" : "bg-danger/4 shadow-[inset_2px_0_0_var(--color-danger)]",
      )}
    >
      <span
        className={cn(
          "mt-0.5 inline-flex h-4 shrink-0 items-center px-1 font-mono text-[10px] leading-none font-semibold text-on-signal",
          tone === "warn" ? "bg-warn" : "bg-danger",
        )}
      >
        {chip}
      </span>
      <span className="min-w-0 flex-1 text-xs leading-5 text-fg">{children}</span>
    </div>
  );
}

/** The machine list's strips: a downgrade in its grace, and a full cloud storage. Nothing otherwise. */
export function PlanBanner({ account, link }: { account: CloudAccount; link: HubLink }) {
  const store = useMe(account, link);
  return <PlanStrips plan={store?.plan ?? null} machines={store?.view?.machines ?? []} />;
}

/** The strips themselves (exported for the render tests). */
export function PlanStrips({ plan, machines, now = Date.now() }: { plan: PlanView | null; machines: { id: string; copy: { streams: number; bytes: number; lastWriteMs: number | null } | null }[]; now?: number }) {
  const { t } = useTranslation();
  const storage = storageView(plan, machines);
  const down = downgradeOf(plan, now);
  if (!down && storage?.level !== "full") return null;
  return (
    <>
      {down && (
        <Strip tone="warn" chip={t("plan")} testid="plan-downgrade">
          <DowngradeText plan={plan} now={now} />
        </Strip>
      )}
      {storage?.level === "full" && (
        <Strip tone="danger" chip={t("storage")} testid="storage-full">
          {storageFullText(storage.lastWriteMs, t)}
        </Strip>
      )}
    </>
  );
}

/**
 * 「我的设备」's top (relay-and-plans §3.8): what the plan allows and what the
 * account uses of it — the plan, the transfer through the relays this
 * period, the cloud storage (each device's offline copy, by its name, behind
 * 「按设备」), the devices and those that run AI — and how long an offline
 * copy is kept. Nothing until `/v1/me` says the plan.
 */
export function UsageSummary({ account, link }: { account: CloudAccount; link: HubLink }) {
  const store = useMe(account, link);
  const machines = store?.view?.machines ?? [];
  // Each machine's sealed status, for its name, as the device list reads it (`machine-name.tsx`).
  const statuses = useMachineStatuses(account, link, deviceMirrorStore(), machines.map((m) => m.id), useShellCopies());
  if (!store?.plan) return null;
  const names = new Map(machines.map((m) => [m.id, shownName(m.id, statuses.get(m.id) ?? null)]));
  return <UsagePanel plan={store.plan} machines={machines} names={names} />;
}

/** A device whose name is not known yet: "aaaa…" for `m-aaaabbbbccccdddd`, never the raw id. */
export function shortMachineId(id: string): string {
  const bare = id.replace(/^[dm]-/, "");
  return bare.length > 5 ? `${bare.slice(0, 4)}…` : bare;
}

/** "Mac mini（离线副本）": a long name gives way, the words around it stay. */
function CopyLabel({ name }: { name: string }) {
  const { t } = useTranslation();
  const [before = "", after = ""] = t("{{name}} (offline copy)", { name: "\u0000" }).split("\u0000");
  return (
    <span className="flex min-w-0 flex-1 text-fg">
      {before && <span className="shrink-0 whitespace-pre">{before}</span>}
      <span className="min-w-0 truncate">{name}</span>
      {after && <span className="shrink-0 whitespace-pre">{after}</span>}
    </span>
  );
}

type Level = "ok" | "high" | "full";

/** Used against a limit, as a bar: ink, amber from 90 % (or at a count's limit), red when full. */
function Meter({ ratio, level, label }: { ratio: number; level: Level; label: string }) {
  const percent = Math.min(100, Math.max(0, ratio * 100));
  return (
    <div data-signal="quota" className="h-1 overflow-hidden bg-border" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(percent)} aria-label={label}>
      <div className={cn("h-full", level === "full" ? "bg-danger" : level === "high" ? "bg-warn" : "bg-fg/70")} style={{ width: `${percent}%` }} />
    </div>
  );
}

/** One number of the summary: what it counts, how much, against what limit, and a line under it. */
function Tile({ testId, label, used, limit, meter, level = "ok", className, children }: { testId: string; label: string; used: ReactNode; limit?: string | null; meter?: number | null; level?: Level; className?: string; children?: ReactNode }) {
  return (
    <div data-testid={testId} className={cn("flex min-w-0 flex-col gap-1.5 border-r border-b border-border px-6 py-3.5 max-md:px-4", className)}>
      <CapsLabel>{label}</CapsLabel>
      <span className="flex min-w-0 items-baseline gap-1.5 whitespace-nowrap">
        <span
          data-signal={level === "ok" ? undefined : level === "high" ? "warn" : "failed"}
          className={cn("text-[20px] leading-6 font-semibold tracking-[-0.02em]", level === "ok" ? "text-fg" : level === "high" ? "text-warn-fg" : "text-danger")}
        >
          {used}
        </span>
        {limit && <span className="truncate font-mono text-2xs text-muted">/ {limit}</span>}
      </span>
      {meter != null && <Meter ratio={meter} level={level} label={label} />}
      {children}
    </div>
  );
}

/** A count against its limit (`null`/`undefined`: none): amber at the limit, where nothing more can be added. */
function countLevel(used: number, max: number | null | undefined): Level {
  return max != null && max > 0 && used >= max ? "high" : "ok";
}

/** The summary itself (exported for the render tests). `names`: each machine's display name, where known. */
export function UsagePanel({
  plan,
  machines,
  names,
  now = Date.now(),
}: {
  plan: PlanView;
  machines: { id: string; copy: { streams: number; bytes: number; lastWriteMs: number | null } | null }[];
  names?: ReadonlyMap<string, string | null>;
  now?: number;
}) {
  const { t, i18n } = useTranslation();
  const [byDevice, setByDevice] = useState(false);
  const storage = storageView(plan, machines)!;
  const days = retentionOf(plan);
  const devicesMax = plan.entitlements[DEVICES_KEY];
  const hostsMax = hostsLimit(plan);
  const hostsUsed = plan.usage.hosts ?? plan.usage.machines;
  const transfer = plan.usage.transfer;
  const transferRatio = transfer && transfer.included != null && transfer.included > 0 ? transfer.bytes / transfer.included : null;
  const transferLevel: Level = transfer?.throttled || (transferRatio !== null && transferRatio >= 1) ? "full" : transferRatio !== null && transferRatio >= 0.9 ? "high" : "ok";
  const resets = transfer?.periodEndMs ?? plan.periodEndMs;
  const count = (used: number, max: number | null | undefined) => ({ used: String(used), limit: max == null ? t("no limit") : String(max), meter: max != null && max > 0 ? used / max : null, level: countLevel(used, max) });
  const devices = count(plan.usage.devices, devicesMax);
  const hosts = count(hostsUsed, hostsMax);
  return (
    <section data-testid="usage-summary" data-plan={plan.id} className="border-b border-border">
      <div className="overflow-hidden">
        <div className="-mr-px -mb-px grid grid-cols-5 max-[1180px]:grid-cols-3 max-md:grid-cols-2">
          <Tile testId="usage-plan" label={t("plan")} used={<span className="inline-flex h-6 items-center px-1.5 font-mono text-xs tracking-[0.08em] text-fg uppercase shadow-[inset_0_0_0_1px_var(--color-border-strong)]">{planLabel(plan.id, t)}</span>} className="max-md:col-span-2">
            <span data-testid="plan-limits" className="font-mono text-2xs text-muted">
              {days === null ? t("offline copies kept without a limit") : t("offline copies kept {{days}} days", { days })}
            </span>
          </Tile>
          <Tile
            testId="usage-transfer"
            label={t("transfer this period")}
            used={transfer ? formatPlanBytes(transfer.bytes) : "—"}
            limit={transfer ? (transfer.included == null ? t("no limit") : formatPlanBytes(transfer.included)) : null}
            meter={transferRatio}
            level={transferLevel}
          >
            {!transfer ? (
              <span className="font-mono text-2xs text-faint">{t("not reported by Droppen yet")}</span>
            ) : transfer.throttled ? (
              <span data-signal="warn" className="text-2xs leading-4 text-warn-fg">
                {t("used up: relayed connections are slowed until the period ends")}
              </span>
            ) : (
              <span className="font-mono text-2xs text-muted">
                {resets ? t("through relays · resets {{date}}", { date: formatDate(resets, i18n.language) }) : t("through relays")}
              </span>
            )}
          </Tile>
          <Tile
            testId="usage-storage"
            label={t("cloud storage")}
            used={<span data-testid="storage-used">{formatPlanBytes(storage.usedBytes)}</span>}
            limit={storage.limitBytes === null ? t("no limit") : formatPlanBytes(storage.limitBytes)}
            meter={storage.ratio}
            level={storage.level}
          >
            {(storage.machines.length > 0 || storage.otherBytes > 0) && (
              <button
                type="button"
                data-testid="storage-by-device"
                aria-expanded={byDevice}
                aria-controls="storage-machines"
                onClick={() => setByDevice(!byDevice)}
                className="flex w-fit items-center gap-1 font-mono text-2xs text-muted hover:text-fg pointer-coarse:py-1.5"
              >
                <ChevronRight aria-hidden="true" className={cn("size-3 transition-transform", byDevice && "rotate-90")} />
                {t("by device")}
              </button>
            )}
          </Tile>
          <Tile testId="usage-devices" label={t("devices")} used={devices.used} limit={devices.limit} meter={devices.meter} level={devices.level} />
          <Tile testId="usage-hosts" label={t("devices that can run AI")} used={hosts.used} limit={hosts.limit} meter={hosts.meter} level={hosts.level} />
        </div>
      </div>
      {byDevice && (
        <ul id="storage-machines" data-testid="storage-machines" className="flex max-w-xl flex-col border-t border-border px-6 py-2 max-md:px-4">
          {storage.machines.map((m) => (
            <li key={m.id} data-machine={m.id} title={m.id} className="flex items-baseline gap-3 border-b border-border py-1 font-mono text-2xs last:border-b-0">
              <CopyLabel name={names?.get(m.id) || t("device {{id}}", { id: shortMachineId(m.id) })} />
              <span className="shrink-0 text-right text-muted tabular-nums">{formatPlanBytes(m.bytes)}</span>
            </li>
          ))}
          {storage.otherBytes > 0 && (
            <li className="flex items-baseline gap-3 py-1 font-mono text-2xs">
              <span className="min-w-0 flex-1 truncate text-muted">{t("Account keys")}</span>
              <span className="shrink-0 text-right text-muted tabular-nums">{formatPlanBytes(storage.otherBytes)}</span>
            </li>
          )}
        </ul>
      )}
      <div className="flex flex-col gap-1 border-t border-border px-6 py-2.5 max-md:px-4">
        {storage.level === "high" && (
          <p data-testid="storage-high" data-signal="warn" className="text-xs leading-5 text-warn-fg">
            {t("Cloud storage {{used}} used: the oldest session copies are cleaned up first", { used: formatStoragePair(storage.usedBytes, storage.limitBytes) })}
          </p>
        )}
        {storage.level === "full" && (
          <p data-testid="storage-full-line" data-signal="failed" className="text-xs leading-5 text-danger">
            {storageFullText(storage.lastWriteMs, t)}
          </p>
        )}
        {downgradeOf(plan, now) && (
          <p data-testid="plan-downgrade-line" data-signal="warn" className="text-xs leading-5 text-warn-fg">
            <DowngradeText plan={plan} now={now} />
          </p>
        )}
        {days !== null && (
          <p data-testid="retention-note" className="text-2xs leading-4 text-muted">
            {t("A device offline for more than {{days}} days has its offline copy in the cloud deleted; what is on the device is not affected.", { days })}
          </p>
        )}
      </div>
    </section>
  );
}

/**
 * A device someone registered with the account's password — not in the
 * chain (yet) — said next to the chain's own alerts until "that was me".
 */
export function RegisteredDeviceAlerts({ account, link }: { account: CloudAccount; link: HubLink }) {
  const store = useMe(account, link);
  if (!store || account.standing !== "active") return null;
  const alerts = registrationAlerts(store.registrations(), {
    self: account.deviceId,
    inChain: (id) => account.chain?.deviceRecord(id) !== undefined,
    dismissed: store.dismissed,
    now: Date.now(),
  });
  return <RegistrationList alerts={alerts} onDismiss={(device) => store.dismiss(device)} />;
}

/** The alerts themselves (exported for the render tests). */
export function RegistrationList({ alerts, onDismiss }: { alerts: { device: string; createdMs: number }[]; onDismiss: (device: string) => void }) {
  const { t } = useTranslation();
  if (!alerts.length) return null;
  return (
    <div role="alert" data-testid="registered-devices" data-signal="warn" className="flex flex-col border-b border-border bg-warn/7 shadow-[inset_2px_0_0_var(--color-warn)]">
      {alerts.map((a) => {
        const iso = new Date(a.createdMs).toISOString();
        return (
          <div key={a.device} data-testid={`registered-${a.device}`} className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border px-4 py-2.5 last:border-b-0">
            <span className="flex min-w-[min(100%,16rem)] flex-1 items-start gap-3">
              <span className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("check")}</span>
              <span className="min-w-0 flex-1 text-xs leading-5 text-fg" title={formatExactTime(iso)}>
                {t("A new device signed in with your password ({{device}}, {{time}}). If it was not you, change your password.", {
                  device: a.device,
                  time: formatRelative(iso),
                })}
              </span>
            </span>
            <span className="flex shrink-0 items-center gap-2">
              <Button variant="secondary" onClick={() => onDismiss(a.device)}>
                {t("that was me")}
              </Button>
            </span>
          </div>
        );
      })}
    </div>
  );
}

/**
 * A passkey or the password added, renamed, changed or removed — by which
 * device, when — said on every full device of the account but the one that
 * did it, until "that was me" (the hub records and pushes each:
 * `sign_in_changed`). `onReview`: the way to the account's sign-in methods
 * (none on that page itself).
 */
export function SignInChangeAlerts({ account, link, onReview }: { account: CloudAccount; link: HubLink; onReview?: () => void }) {
  const store = useMe(account, link);
  if (!store || account.standing !== "active" || !account.isAdmin) return null;
  const changes = store.signInChanges().filter((c) => c.device !== account.deviceId);
  const names = new Map((store.view?.devices ?? []).flatMap((d) => (d.uaLabel ? [[d.id, d.uaLabel] as const] : [])));
  return <SignInChangeList changes={changes} names={names} onDismiss={(id) => store.dismissSignInChange(id)} onReview={onReview} />;
}

/** The sentence for one change: what, by which device, when. */
export function signInChangeText(c: SignInChange, device: string, time: string, t: (key: string, vars?: Record<string, unknown>) => string): string {
  const vars = { device, time, label: c.label ?? "" };
  switch (c.kind) {
    case "passkey_added":
      return t("A passkey ({{label}}) was added to your account from {{device}}, {{time}}.", vars);
    case "passkey_renamed":
      return t("A passkey was renamed to {{label}} from {{device}}, {{time}}.", vars);
    case "passkey_removed":
      return t("A passkey ({{label}}) was removed from your account from {{device}}, {{time}}.", vars);
    case "password_set":
      return t("A password was set for your account from {{device}}, {{time}}.", vars);
    case "password_changed":
      return t("Your password was changed from {{device}}, {{time}}.", vars);
    case "password_removed":
      return t("Your password was removed from {{device}}, {{time}}.", vars);
  }
}

/** The alerts themselves (exported for the render tests). */
export function SignInChangeList({
  changes,
  names,
  onDismiss,
  onReview,
}: {
  changes: SignInChange[];
  /** Device id → its browser's short name, where known. */
  names: ReadonlyMap<string, string>;
  onDismiss: (id: number) => void;
  onReview?: () => void;
}) {
  const { t, i18n } = useTranslation();
  if (!changes.length) return null;
  return (
    <div data-testid="sign-in-changes" data-signal="warn" className="flex flex-col border-b border-border bg-warn/7 shadow-[inset_2px_0_0_var(--color-warn)]">
      {changes.map((c) => {
        const iso = new Date(c.atMs).toISOString();
        const name = c.device ? names.get(c.device) : undefined;
        const device = c.device ? (name ? `${name} (${c.device})` : c.device) : t("a device of the account");
        return (
          <div key={c.id} data-testid={`sign-in-change-${c.id}`} data-kind={c.kind} className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border px-4 py-2.5 last:border-b-0">
            <span className="flex min-w-[min(100%,16rem)] flex-1 items-start gap-3">
              <span className="mt-0.5 inline-flex h-4 shrink-0 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">{t("check")}</span>
              <span role="status" className="min-w-0 flex-1 text-xs leading-5 text-fg" title={formatExactTime(iso)}>
                {signInChangeText(c, device, formatMoment(c.atMs, i18n.language), t)}
                <span className="ms-1 text-muted">{t("If it was not you, check how your account signs in.")}</span>
              </span>
            </span>
            <span className="flex shrink-0 items-center gap-2">
              {onReview && (
                <Button variant="secondary" onClick={onReview}>
                  {t("sign-in methods")}
                </Button>
              )}
              <Button variant="secondary" onClick={() => onDismiss(c.id)}>
                {t("that was me")}
              </Button>
            </span>
          </div>
        );
      })}
    </div>
  );
}

/**
 * Where a session's state stands in the open machine's offline copy: the
 * entries the retention window left out (`copyWindow`: `base`, and the
 * copy's own start past it), `0` when none were. A number, so the store's
 * snapshot is stable.
 */
function useLeftOut(source: string, id: string): number {
  const mirror = mirrorFor(useMachine());
  const key = `${source}/${id}`;
  return useSyncExternalStore(
    (notify) =>
      mirror
        ? mirror.onChange((change) => {
            if (change.kind === "session" && change.key === key) notify();
          })
        : () => undefined,
    () => {
      const state = mirror?.sessionNow(source, id)?.state ?? null;
      const window = copyWindow(state);
      return window ? Math.max(window.base, state?.startIndex ?? 0) : 0;
    },
    () => 0,
  );
}

/**
 * The top of an offline session copy the retention window trimmed (§3.4,
 * §3.8): the cloud keeps only the plan's days, and the earlier entries are on
 * the computer, readable while it is online. A quiet line, only while the page
 * shows the copy and its start is in view.
 */
export function CopyWindowNotice({ source, id }: { source: string; id: string }) {
  const shell = hubShell();
  const showing = useShowingCopy();
  const count = useLeftOut(source, id);
  const me = useMe(shell?.account, shell?.link);
  if (!showing || count <= 0) return null;
  return <CopyWindowLine count={count} days={retentionOf(me?.plan)} />;
}

/** The line itself (exported for the render tests). */
export function CopyWindowLine({ count, days }: { count: number; days: number | null }) {
  const { t } = useTranslation();
  return (
    <div data-testid="copy-window" className="border-b border-border px-3 py-1.5 font-mono text-2xs text-faint">
      {days !== null
        ? t("The cloud keeps the last {{days}} days; the {{count}} earlier entries are on the device, readable while it is online", { days, count })
        : t("The {{count}} earlier entries are on the device, readable while it is online", { count })}
    </div>
  );
}
