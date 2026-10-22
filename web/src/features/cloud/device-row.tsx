/*
 * One device in 「我的设备」's list (docs/local-aggregator.md §3.7): a name a
 * person knows it by, what it is as badges (本设备 · 可运行 AI · 完整设备 or
 * 受限设备), whether it is up and when it was last seen, and — for a device
 * that runs AI — how many of its sessions need you and the way into it.
 * What only a careful look needs (its id, fingerprint, path, system, who may
 * control it, its latest sessions, a re-enroll warning's evidence) is behind
 * 「详情」; what can be done with it is in the row's 「⋯」 menu, each action as
 * a confirmation or a panel under the row (`devices.tsx` draws those).
 *
 * The menu's items are worked out apart from the drawing (`rowMenuItems`), so
 * which action a row offers — and why one cannot be pressed — is plain to
 * test without opening a menu.
 */
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Menu } from "@base-ui-components/react/menu";
import { ArrowRight, ChevronRight, Ellipsis, Globe, MonitorSmartphone, Server, Smartphone } from "lucide-react";
import type { components } from "@/api/schema";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";
import type { Accept } from "@/lib/e2e";
import { compatModeActive } from "@/lib/e2e/compat";
import { formatAsOf, formatExactTime, formatRelative } from "@/lib/format";
import type { PauseEnding, PromoteBlock, ReEnrollLine } from "@/lib/hub/account";
import type { PresenceState } from "@/lib/hub/link";
import type { RenewalStatus } from "@/lib/hub/renewal";
import type { DeviceAttention } from "@/features/machines/needs-you";
import { ActivityChipView, StateCountChip, UnreadPill } from "@/features/sessions/attention-badges";
import { unreadCountOf } from "@/features/sessions/session-attention";
import { CompatBadge } from "./browser-support";
import { Presence } from "./machine-name";
import { pauseHeldText, RenewalLine } from "./renewal";

type SessionSummary = components["schemas"]["SessionSummary"];
type T = (key: string, opts?: Record<string, unknown>) => string;

/** A device as the 「我的设备」 list draws it (docs/local-aggregator.md §3.7). */
export interface DeviceRowView {
  id: string;
  fingerprint: string;
  /**
   * The name to know it by: a device that runs AI, its own (from its status);
   * a browser or phone, what the hub made of its User-Agent ("iPhone ·
   * Chrome"). `null`: none known — a short id stands in (`deviceName`).
   */
  name: string | null;
  self: boolean;
  /** What it is, for its icon: a computer with Droppen, a phone, a browser, or (another browser or phone: the hub is not told which) either. */
  kind: "computer" | "phone" | "browser" | "browserOrPhone";
  status: "active" | "pending" | "removed";
  /** 「可运行 AI」 now. */
  host: boolean;
  /** It ran AI and was stopped (`setHost false`): it may run it again. */
  stoppedHost: boolean;
  /** A full device of the account (chain v3's admin, v2's full device): it can add and remove devices. */
  full?: boolean;
  /** The account's main device (it keeps your screens and layouts). */
  main?: boolean;
  /** Online, offline or unknown: a host's presence, this device's own. */
  presence: PresenceState | null;
  /** The path to a host and its round trip (the first relay it is on). */
  path: { relay: string; rttMs: number | null } | null;
  /** An offline host: when its offline copy was last written. */
  copyAsOf: number | null;
  lastSeenMs: number | null;
  /**
   * A limited device (`limitedDevice`): a browser or phone of the account
   * that is not a full device — and why "make it a full device" cannot be
   * pressed here, if it cannot. `null`: a full device, a device that runs AI
   * (or did), or one that is not current.
   */
  limited: { block: PromoteBlock | null } | null;
  /** A host's accept policy (who may control it besides the full devices). */
  accept: Accept | null;
  /**
   * A host a device that could control it no longer can (review D3): it may
   * have kept the computer's identity key — re-enroll it for a new one. One
   * line per such device, with what the host's record says of it, and those
   * a person dismissed (shown quietly, to undo).
   */
  reEnroll?: ReEnrollLine[];
  /** A host that renews its identity key here: where its renewal stands (docs/host-identity-rotation.md §4.4). */
  renewal?: RenewalStatus;
  /** It renews its identity key here at all (v3, the hub takes renewals, its Droppen declared it): the menu's "renew identity key". */
  renewable?: boolean;
  /**
   * The hub takes identity key renewals (its switch is on): only then does the
   * menu offer "remove it, then add it again" as the way for a host that
   * cannot renew. Off, the row is as it was before renewals.
   */
  renewalsOn?: boolean;
  /** Exposed hosts it lets control it that must renew first (§4.1): until then "renew identity key" cannot be pressed, and says so. */
  renewFirst?: string[];
  /** Isolated hosts it lets control it: until they are removed, whoever holds their current key may drive it (§3.8). */
  isolatedControllers?: string[];
  /** When a key it renewed away from was last used to sign in (checked here): information only, never isolation (§3.8). */
  retiredLoginMs?: number | null;
  /** The devices whose control of each other is paused while their identity keys are renewed (§4.1), it among them. */
  pausedWith?: string[];
  /** The ids its list names whose control of it a renewal pause holds now (§3.4 rule 10): said apart from the list itself. */
  pausedControllers?: string[];
  /** The open renewal pauses on its list: what ending each now gives back, and what must stay held, and why (§4.1). */
  pauses?: { op: Uint8Array; ending: PauseEnding }[];
  /** A host's facts for 「详情」: its system, agent version, hostname, the newest content key's generation. */
  facts?: { os?: string | null; agent?: string | null; agentFull?: string | null; hostname?: string | null; keyEpoch?: number | null };
  /** A host's sessions at a glance (off its copy): `null` while there is no list of them. */
  attention?: DeviceAttention | null;
  /** A host's latest sessions, asking first. */
  recent?: readonly SessionSummary[];
  /** The page has a copy of a host to show (it can be read while the host is away). */
  hasCopy?: boolean;
}

/** A device id without its kind's prefix, cut short: "aaaa…" for `d-aaaabbbbccccdddd`. */
export function shortDeviceId(id: string): string {
  const bare = id.replace(/^[dm]-/, "");
  return bare.length > 5 ? `${bare.slice(0, 4)}…` : bare;
}

/**
 * The name a row shows: the device's own, else "device aaaa…". One waiting
 * for approval goes by its id, whatever it calls itself: a device coming
 * back with the recovery words chose its own User-Agent, and could pose as a
 * familiar "iPhone · Safari" right under the banner that asks to approve it
 * (`DevicesPanel` gives it no name).
 */
export function deviceName(view: Pick<DeviceRowView, "id" | "name" | "status">, t: T): string {
  if (view.status === "pending") return view.id;
  return view.name ?? t("device {{id}}", { id: shortDeviceId(view.id) });
}

/** One action of a row's 「⋯」 menu. */
export interface RowMenuItem {
  /** Its test id: `promote-<id>`, `host-<id>`, `remove-<id>`… */
  id: string;
  label: string;
  /** Why it cannot be pressed now, said under it (a tooltip does not reach a finger); `null`: it can. */
  blocked: string | null;
  /** What it does, said under it when that is not plain from its label. */
  hint?: string;
  /** Something else is under way: not pressable, nothing to say. */
  busy?: boolean;
  danger?: boolean;
  onSelect: () => void;
}

export interface RowHandlers {
  /** "Make it a full device", for a limited device: it opens the confirmation. */
  onPromote?: () => void;
  onRemove?: () => void;
  /** Stop or restart running AI. */
  onHost?: (host: boolean) => void;
  onControl?: () => void;
  onRename?: () => void;
  /** Its terminals (a host). */
  onTerminal?: () => void;
  /** Mail it every key again. */
  onResend?: () => void;
  /** "Renew identity key" (a host that renews here): opens the choice of how (docs/host-identity-rotation.md §4.4). */
  onRenew?: () => void;
  /** "Remove it, then add it again": the way to a new identity (review D3; for a host that cannot renew, §7 decision 7). */
  onReEnroll?: () => void;
}

/**
 * What row `view`'s 「⋯」 menu offers, in order — to a full device looking
 * (`viewerAdmin`; a limited device's list is read-only: only a host's
 * terminal). A control that cannot move now is still listed, with why
 * (review D4: until the account is upgraded; review 14: the hub gates running
 * AI again; M2 re-review N1: a held-back activation holds every change to the
 * chain; a host that is offline cannot be renamed or reached).
 */
export function rowMenuItems(
  view: DeviceRowView,
  ctx: {
    viewerAdmin: boolean;
    /** The chain holds a v3 entry (review D4): what only v3 says can be changed. */
    upgraded: boolean;
    /** The hub takes v3 entries now: what it gates on a v3 chain (running AI again) can be done. */
    writable: boolean;
    /** A change to the chain is under way: what writes one waits, with nothing to say. */
    busy?: boolean;
    /** A held-back activation: what writes to the chain waits for its approval or veto (M2 re-review N1), and says so. */
    held?: boolean;
    /** Its keys are being mailed again. */
    resending?: boolean;
    t: T;
  } & RowHandlers,
): RowMenuItem[] {
  const { id, status } = view;
  const { t, viewerAdmin, upgraded, writable, busy = false } = ctx;
  const items: RowMenuItem[] = [];
  // An item with nothing to do is not offered; one that cannot move now does nothing when pressed, whatever draws it.
  const add = (item: Omit<RowMenuItem, "onSelect"> & { onSelect?: () => void }) => {
    const run = item.onSelect;
    if (run) items.push({ ...item, onSelect: () => (item.busy || item.blocked !== null ? undefined : run()) });
  };
  const offline = view.presence === "online" ? null : t("the device is offline — this needs it to be online");
  // What writes to the chain: its own reason first, then a held-back activation's.
  const held = ctx.held ? t("approve or remove the device whose activation is held back first") : null;
  if (view.host && status === "active") {
    add({ id: `terminal-${id}`, label: t("open its terminal"), blocked: offline, onSelect: ctx.onTerminal });
  }
  if (viewerAdmin && status === "active") {
    // Renamed over its own channel (not the chain): it only needs to be online.
    if (view.host) add({ id: `rename-${id}`, label: t("rename device"), blocked: offline, onSelect: ctx.onRename });
    if (view.host) {
      add({ id: `control-${id}`, label: t("who can control it"), blocked: upgraded ? held : t("upgrade the account first"), busy, onSelect: ctx.onControl });
    }
    // Its identity key, renewed now (§4.4) — never ahead of an exposed host it lets control it (§4.1): that one first, said why.
    if (view.host && view.renewable) {
      const first = view.renewFirst ?? [];
      add({
        id: `renew-${id}`,
        label: t("renew identity key"),
        blocked: first.length ? t("renew the identity key of {{ids}} first: it lets them control it", { ids: first.join(", ") }) : held,
        busy,
        onSelect: ctx.onRenew,
      });
    }
    if ((view.host || view.stoppedHost) && !view.self) {
      // Stopping needs the account upgraded; running AI again needs the hub to take it too (it gates a host made one again).
      const moves = upgraded && (view.host || writable);
      const onHost = ctx.onHost;
      add({
        id: `host-${id}`,
        label: view.host ? t("stop running AI") : t("run AI again"),
        blocked: !upgraded ? t("upgrade the account first") : moves ? held : t("not available until Droppen is updated on every device of the account that runs AI"),
        busy,
        onSelect: onHost && (() => onHost(!view.host)),
      });
    }
    // A limited device: a full device may make it one (not its own row: the note above the list says so), once the account is upgraded.
    if (view.limited && !view.self) {
      add({
        id: `promote-${id}`,
        label: t("make it a full device"),
        blocked: view.limited.block === null ? held : view.limited.block === "notUpgraded" ? t("upgrade the account first") : t("only a full device can do this"),
        busy,
        onSelect: ctx.onPromote,
      });
    }
    // Mail, not the chain: neither a change under way nor a hold stops it; one sending at a time.
    if (!view.self) {
      add({ id: `resend-${id}`, label: t("re-send keys"), hint: t("send this device every key again (if it missed new ones)"), blocked: null, busy: ctx.resending, onSelect: ctx.onResend });
    }
  }
  // A new identity (§7 decision 7): for a host that cannot renew its identity key, or a fresh start — a removal, then adding it again.
  // Only while the hub takes renewals: with its switch off, no host renews, and "its Droppen cannot renew" would be untrue.
  if (viewerAdmin && status === "active" && view.host && !view.self && view.renewalsOn) {
    add({
      id: `re-enroll-${id}`,
      label: t("remove it, then add it again"),
      hint: view.renewable ? t("a new identity: only if renewing its identity key is not the way") : t("a new identity: its Droppen cannot renew its identity key"),
      blocked: held,
      busy,
      onSelect: ctx.onReEnroll,
    });
  }
  if (viewerAdmin && (status === "active" || status === "pending") && !view.self) {
    add({ id: `remove-${id}`, label: t("remove"), blocked: held, busy, danger: true, onSelect: ctx.onRemove });
  }
  return items;
}

/** A row's 「⋯」: its actions as the app's menus draw them (24px rows, 36px under a finger), a destructive one last and apart. */
export function RowMenu({ id, items }: { id: string; items: RowMenuItem[] }) {
  const { t } = useTranslation();
  if (!items.length) return null;
  return (
    <Menu.Root>
      <Menu.Trigger
        data-testid={`actions-${id}`}
        title={t("more actions")}
        aria-label={t("more actions")}
        className="flex size-7 shrink-0 items-center justify-center border border-transparent text-muted hover:border-border-strong hover:text-fg data-[popup-open]:border-border-strong data-[popup-open]:text-fg pointer-coarse:size-9"
      >
        <Ellipsis className="size-4" />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={4} collisionPadding={8} className="z-50 outline-none">
          <Menu.Popup data-testid={`actions-menu-${id}`} className="max-h-[min(var(--available-height),20rem)] min-w-44 max-w-[min(var(--available-width),20rem)] overflow-y-auto border border-border-strong bg-bg py-1 outline-none">
            {items.map((item, i) => (
              <div key={item.id}>
                {item.danger && i > 0 && <Menu.Separator className="my-1 h-px bg-border" />}
                <Menu.Item
                  data-testid={item.id}
                  data-signal={item.danger ? "failed" : undefined}
                  disabled={item.busy || item.blocked !== null}
                  onClick={item.onSelect}
                  className={cn(
                    "flex min-h-6 w-full cursor-pointer flex-col items-start justify-center gap-0.5 px-2 py-1 text-left font-mono text-2xs outline-none select-none data-[highlighted]:bg-hover",
                    "pointer-coarse:min-h-9 pointer-coarse:text-sm data-[disabled]:cursor-default",
                    item.danger ? "text-danger" : "text-fg",
                  )}
                >
                  <span className={cn((item.busy || item.blocked !== null) && "opacity-40")}>{item.label}</span>
                  {(item.blocked ?? item.hint) && <span className="max-w-64 text-[10px] leading-3.5 whitespace-normal text-muted pointer-coarse:text-xs">{item.blocked ?? item.hint}</span>}
                </Menu.Item>
              </div>
            ))}
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}

const badge = "tracking-[0.08em] uppercase";

/** What it is, as badges: this device, the main device, runs AI, a full or a limited device. */
function RowBadges({ view, warn }: { view: DeviceRowView; warn: ReactNode }) {
  const { t } = useTranslation();
  return (
    <>
      {view.self && <Badge variant="outline" className={badge}>{t("this device")}</Badge>}
      {view.self && compatModeActive() && (
        <Badge data-testid="compat-mode-row" variant="outline" className={badge}>
          <CompatBadge className="uppercase" />
        </Badge>
      )}
      {view.host && (
        <Badge data-testid="runs-ai" variant="outline" className={badge}>
          {t("can run AI")}
        </Badge>
      )}
      {view.main && <Badge className={badge}>{t("main device")}</Badge>}
      {view.full && view.status === "active" && (
        <Badge data-testid={`full-${view.id}`} title={roleText(view, t) ?? undefined} className={badge}>
          {t("full device")}
        </Badge>
      )}
      {view.limited && (
        <Badge data-testid={`limited-${view.id}`} title={roleText(view, t) ?? undefined} className={badge}>
          {t("limited device")}
        </Badge>
      )}
      {view.stoppedHost && <span className="font-mono text-2xs text-faint">{t("stopped running AI")}</span>}
      {warn}
    </>
  );
}

/** What a full or a limited device may do, in words: the badge's tooltip, and said in the details too (a finger does not hover). */
function roleText(view: DeviceRowView, t: T): string | null {
  if (view.limited) return t("It can control the devices that let it and read their offline copies, but cannot add or remove devices.");
  if (view.full && view.status === "active") return t("A full device holds the account's keys: it can read everything and add or remove devices.");
  return null;
}

/** Online or not, and since when it was last seen; pending and removed say so instead. */
function RowStatus({ view }: { view: DeviceRowView }) {
  const { t } = useTranslation();
  const { status } = view;
  const seen =
    view.lastSeenMs !== null && !view.self && view.presence !== "online" && status !== "removed" ? (
      <span data-testid={`recent-${view.id}`} className="text-faint" title={formatExactTime(new Date(view.lastSeenMs).toISOString())}>
        {t("seen {{when}}", { when: formatRelative(new Date(view.lastSeenMs).toISOString()) })}
      </span>
    ) : null;
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-2xs">
      {status === "pending" ? (
        <span data-signal="pending" className="text-warn-fg">
          {t("waiting for approval")}
        </span>
      ) : status === "removed" ? (
        <span className="text-faint">{t("removed")}</span>
      ) : view.presence ? (
        <Presence presence={view.presence} />
      ) : null}
      {view.path && view.presence === "online" && (
        <span className="text-faint" title={t("path")}>
          {view.path.relay}
          {view.path.rttMs !== null ? ` · ${Math.round(view.path.rttMs)} ms` : ""}
        </span>
      )}
      {seen}
    </span>
  );
}

/** Who may control a host, in words. */
function acceptText(accept: Accept, t: T): string {
  if (accept.type === "all") return t("every device of the account");
  if (accept.type === "devices" && accept.ids.length) return t("your full devices, and {{ids}}", { ids: accept.ids.join(", ") });
  return t("only your full devices");
}

/** 「详情」: what a careful look needs — and, for a host, its latest sessions. Open by itself while a re-enroll warning is up. */
function RowDetails({ view, open, warnings, onOpenSession }: { view: DeviceRowView; open: boolean; warnings?: ReactNode; onOpenSession?: (row: SessionSummary) => void }) {
  const { t } = useTranslation();
  const facts: { key: string; label: string; value: string; title?: string }[] = [{ key: "id", label: t("device ID"), value: view.id }];
  facts.push({ key: "fingerprint", label: t("fingerprint"), value: view.fingerprint || "…" });
  const f = view.facts;
  if (f?.hostname) facts.push({ key: "hostname", label: t("hostname"), value: f.hostname });
  if (f?.os) facts.push({ key: "os", label: t("system"), value: f.os });
  if (f?.agent) facts.push({ key: "agent", label: "agent", value: f.agent, title: f.agentFull ?? undefined });
  if (f?.keyEpoch != null) facts.push({ key: "key", label: t("content key"), value: t("generation {{n}}", { n: f.keyEpoch }) });
  if (view.path && view.presence === "online") facts.push({ key: "path", label: t("path"), value: `${view.path.relay}${view.path.rttMs !== null ? ` · ${Math.round(view.path.rttMs)} ms` : ""}` });
  // The list as set (its base), and apart from it what a renewal pause holds now: a widened list cannot hide behind a pause.
  if (view.host && view.accept) facts.push({ key: "control", label: t("who can control it"), value: acceptText(view.accept, t) });
  if (view.host && view.pausedControllers?.length) facts.push({ key: "paused", label: t("paused now"), value: view.pausedControllers.join(", ") });
  const recent = view.recent ?? [];
  return (
    <details data-testid={`details-${view.id}`} open={open || undefined} className="group/details min-w-0">
      {/* The row's quiet last line — its id, its system — opens the rest. */}
      <summary className="flex max-w-full min-w-0 cursor-pointer list-none items-center gap-1.5 font-mono text-2xs text-faint select-none hover:text-fg pointer-coarse:py-1.5 [&::-webkit-details-marker]:hidden">
        <span className="min-w-0 truncate">
          {view.id}
          {f?.os ? ` · ${f.os}` : ""}
        </span>
        <span className="flex shrink-0 items-center gap-0.5 text-muted group-hover/details:text-fg">
          <ChevronRight aria-hidden="true" className="size-3 transition-transform group-open/details:rotate-90" />
          {t("details")}
        </span>
      </summary>
      <div className="flex flex-col gap-2 pt-1.5 pb-0.5">
        {roleText(view, t) && (
          <p data-testid={`role-${view.id}`} className="text-2xs leading-4 text-muted">
            {roleText(view, t)}
          </p>
        )}
        <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-0.5 font-mono text-2xs">
          {facts.map((fact) => (
            <div key={fact.key} className="contents">
              <dt className="text-faint">{fact.label}</dt>
              <dd title={fact.title} className={cn("min-w-0 break-words text-fg", (fact.key === "id" || fact.key === "fingerprint") && "tracking-wider select-all")}>
                {fact.value}
              </dd>
            </div>
          ))}
        </dl>
        {warnings}
        {recent.length > 0 && (
          <div data-testid={`recent-sessions-${view.id}`} className="flex flex-col">
            <span className="font-mono text-2xs text-faint">{t("latest sessions")}</span>
            <ul className="mt-0.5 flex flex-col border-t border-border">
              {recent.map((row) => (
                <li key={`${row.source}/${row.id}`} className="border-b border-border">
                  <button
                    type="button"
                    onClick={() => onOpenSession?.(row)}
                    disabled={!onOpenSession}
                    className="grid w-full grid-cols-[minmax(0,1fr)_auto] items-center gap-2 py-1 text-left hover:bg-hover pointer-coarse:py-2"
                  >
                    <span className={cn("truncate text-xs text-fg", unreadCountOf(row) > 0 && "font-semibold")}>{row.title}</span>
                    <span className="flex items-center gap-1.5">
                      <ActivityChipView row={row} />
                      <UnreadPill count={unreadCountOf(row)} />
                      <span className="font-mono text-2xs text-faint tabular-nums">{formatRelative(row.updatedAt)}</span>
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </details>
  );
}

/** A device that runs AI, at a glance: how many sessions, and how many are working, asking you or unread. */
function SessionCounts({ view }: { view: DeviceRowView }) {
  const { t } = useTranslation();
  const a = view.attention;
  const online = view.presence === "online";
  if (!a) {
    return <span className="font-mono text-2xs text-faint">{view.hasCopy || online ? t("sessions not listed yet") : t("no copy on this device yet: it is made once the device is online")}</span>;
  }
  return (
    <span data-testid={`sessions-${view.id}`} className="flex flex-wrap items-center gap-1.5">
      <span className="font-mono text-2xs text-muted">{a.sessions === 1 ? t("1 session") : t("{{n}} sessions", { n: a.sessions })}</span>
      <StateCountChip kind="working" count={a.working} />
      <StateCountChip kind="asking" count={a.asking} />
      <UnreadPill count={a.unread} />
    </span>
  );
}

/** What a host's record says of one device it should be re-enrolled for (review D3), in words. */
function evidenceText(line: ReEnrollLine, t: (key: string, opts?: Record<string, unknown>) => string): string {
  const { device, evidence } = line;
  switch (evidence.kind) {
    case "connected":
      return t("{{device}} connected to it (first seen {{time}}) — re-enrolling is strongly recommended", { device, time: formatAsOf(evidence.firstMs) });
    case "notSeen":
      return t("no record of {{device}} connecting since {{time}} — though a device that did connect could have erased itself from that record", { device, time: formatAsOf(evidence.sinceMs) });
    case "unknown":
      return t("{{device}} could control it; whether it ever connected is not known", { device });
  }
}

/**
 * A host's re-enroll warning (review D3), said plainly: rotating keys does
 * not help against a copied identity key; only a new identity does. Each
 * device it is about, with what the host's own record says of it — evidence,
 * never a reason to stay quiet — and, unless the record says it connected,
 * a person's way to say it never did. Those dismissed stay listed, quietly,
 * with the way back.
 */
export function ReEnrollWarning({
  id,
  lines,
  onReEnroll,
  onDismiss,
  onWarnAgain,
}: {
  id: string;
  lines: ReEnrollLine[];
  onReEnroll?: () => void;
  onDismiss?: (device: string, lostSeq: number) => void;
  onWarnAgain?: (device: string) => void;
}) {
  const { t } = useTranslation();
  const open = lines.filter((l) => !l.dismissed);
  const dismissed = lines.filter((l) => l.dismissed);
  return (
    <>
      {open.length > 0 && (
        <span data-testid={`re-enroll-${id}`} data-signal="warn" className="flex flex-col items-start gap-0.5 text-2xs leading-4 text-warn-fg">
          <span>{t("re-enroll it: rotating its keys does not cut off a device that copied its identity")}</span>
          <span className="text-muted">
            {t("A device that could control it no longer can, and may have copied its identity key. With it, that device can still sign in as this one, and each new content key goes to that identity too. Only a new identity fixes this:")}
          </span>
          {open.map((line) => (
            <span key={line.device} data-testid={`re-enroll-${id}-${line.device}`} data-evidence={line.evidence.kind} className="flex flex-wrap items-baseline gap-x-2">
              <span className={line.evidence.kind === "connected" ? "text-danger" : undefined}>{evidenceText(line, t)}</span>
              {line.evidence.kind !== "connected" && onDismiss && (
                <button type="button" data-testid={`re-enroll-dismiss-${id}-${line.device}`} className="text-muted underline underline-offset-2 hover:text-fg" onClick={() => onDismiss(line.device, line.lostSeq)}>
                  {t("I'm sure it never connected — don't warn about it")}
                </button>
              )}
            </span>
          ))}
          {onReEnroll && (
            <button type="button" data-testid={`re-enroll-start-${id}`} className="underline underline-offset-2 hover:text-fg" onClick={onReEnroll}>
              {t("remove it, then add it again")}
            </button>
          )}
        </span>
      )}
      {dismissed.length > 0 && (
        <span data-testid={`re-enroll-dismissed-${id}`} className="flex flex-col items-start gap-0.5 text-2xs leading-4 text-faint">
          {dismissed.map((line) => (
            <span key={line.device} className="flex flex-wrap items-baseline gap-x-2">
              <span>{t("not warning about {{device}}: you said it never connected", { device: line.device })}</span>
              {onWarnAgain && (
                <button type="button" data-testid={`re-enroll-warn-again-${id}-${line.device}`} className="underline underline-offset-2 hover:text-fg" onClick={() => onWarnAgain(line.device)}>
                  {t("warn again")}
                </button>
              )}
            </span>
          ))}
        </span>
      )}
    </>
  );
}

const ICONS = { computer: Server, phone: Smartphone, browser: Globe, browserOrPhone: MonitorSmartphone } as const;

/**
 * One row of 「我的设备」: what the device is, may do and where it is — and,
 * for a full device looking, what can be changed (its 「⋯」). A row is a card
 * on a phone. `notes`: what only droppen.app says of a host (a chain alarm, an
 * old key, its copy's retention); `children`: whatever is open under it.
 */
export function DeviceRow({
  view,
  viewerAdmin,
  upgraded,
  writable,
  busy = false,
  held = false,
  resending = false,
  onPromote,
  onRemove,
  onHost,
  onControl,
  onRename,
  onTerminal,
  onResend,
  onOpen,
  onOpenSession,
  onReEnroll,
  onDismissReEnroll,
  onWarnAgain,
  onRenew,
  onRenewCode,
  onEndPause,
  onRenewRetry,
  onAddComputer,
  notes,
  keys,
  children,
}: {
  view: DeviceRowView;
  viewerAdmin: boolean;
  /** The chain holds a v3 entry (review D4): what only v3 says can be changed. */
  upgraded: boolean;
  /** The hub takes v3 entries now: what it gates on a v3 chain (running AI again) can be done. */
  writable: boolean;
  /** A change to the chain is under way: what writes one waits. */
  busy?: boolean;
  /** A held-back activation: what writes to the chain waits for its approval or veto, and the menu says so. */
  held?: boolean;
  /** Its keys are being mailed again. */
  resending?: boolean;
  /** Open it (a host): the app on it, or on its copy. */
  onOpen?: () => void;
  onOpenSession?: (row: SessionSummary) => void;
  /** "Remove it, then add it again": the way to a new identity (review D3). */
  onReEnroll?: () => void;
  /** "I'm sure it never connected": drop the re-enroll warning about one device, for the loss shown (asked first). */
  onDismissReEnroll?: (device: string, lostSeq: number) => void;
  /** Warn about a device dismissed before again. */
  onWarnAgain?: (device: string) => void;
  /** Its renewal goes only confirmed on the device: "enter the code shown on the device". */
  onRenewCode?: () => void;
  /** "Try again", once the automatic tries ran out. */
  onRenewRetry?: () => void;
  /** Where a new registration code is made (an isolated host added again with a new identity). */
  onAddComputer?: () => void;
  /** 「结束暂停」 of open renewal pause `op` on its list: opens the confirmation. */
  onEndPause?: (op: Uint8Array) => void;
  notes?: ReactNode;
  /** How re-sending its keys went, said on the row. */
  keys?: ReactNode;
  /** What is open for it, under it (a rename, a confirmation, its control settings, its removal). */
  children?: ReactNode;
} & RowHandlers) {
  const { t } = useTranslation();
  const { id, status } = view;
  const Icon = ICONS[view.kind];
  const items = rowMenuItems(view, { viewerAdmin, upgraded, writable, busy, held, resending, t, onPromote, onRemove, onHost, onControl, onRename, onTerminal, onResend, onRenew, onReEnroll });
  const lines = view.reEnroll && view.reEnroll.length > 0 && viewerAdmin && status === "active" ? view.reEnroll : null;
  const openWarnings = !!lines?.some((l) => !l.dismissed);
  const removed = status === "removed";
  const name = deviceName(view, t);
  return (
    <li
      data-testid={`device-${id}`}
      data-host={view.host ? "" : undefined}
      data-limited={view.limited ? "" : undefined}
      data-status={status}
      className="border-b border-border max-md:border max-md:border-border"
    >
      {/*
        Wide: icon · name and what follows · status (12rem) · 「打开」 · 「⋯」, the status and the actions on the name's line.
        A phone's card: the name's line keeps only 「⋯」, the rest takes the card's whole width, and 「打开」 has its own line under it.
      */}
      <div className="grid grid-cols-[1rem_minmax(0,1fr)_auto] items-start gap-x-3 gap-y-1.5 px-6 py-3 max-md:px-3 md:grid-cols-[1rem_minmax(0,1fr)_12rem_6.5rem_1.75rem]">
        <Icon aria-hidden="true" className={cn("col-start-1 row-start-1 mt-0.5 size-4 shrink-0", removed || view.presence === "offline" ? "text-faint" : "text-muted")} />
        <span className="col-start-2 row-start-1 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
          <span data-testid={`name-${id}`} title={id} className={cn("min-w-0 truncate text-[13px] leading-5 font-medium", removed ? "text-muted" : "text-fg")}>
            {name}
          </span>
          <RowBadges
            view={view}
            warn={
              openWarnings ? (
                <span data-signal="warn" className="inline-flex h-3.5 items-center bg-warn px-1 font-mono text-[10px] leading-none font-semibold text-on-signal">
                  {t("re-enroll it")}
                </span>
              ) : null
            }
          />
        </span>
        <span className="pt-0.5 max-md:hidden md:col-start-3 md:row-span-2 md:row-start-1">
          <RowStatus view={view} />
        </span>
        {onOpen && view.host && status === "active" && (
          <Button
            data-testid={`open-${id}`}
            onClick={onOpen}
            className="col-span-2 col-start-2 row-start-3 justify-self-start pointer-coarse:h-9 md:col-span-1 md:col-start-4 md:row-start-1 md:-my-1 md:justify-self-end md:pointer-coarse:-my-2"
          >
            {view.presence === "online" || !view.hasCopy ? t("open") : t("read the copy")}
            <ArrowRight aria-hidden="true" />
          </Button>
        )}
        {/* Taller than the name's line, the actions overhang it rather than push the line under it down. */}
        <span className="col-start-3 row-start-1 -my-1 flex justify-end pointer-coarse:-my-2 md:col-start-5">
          <RowMenu id={id} items={items} />
        </span>
        <div className="col-span-2 col-start-2 row-start-2 flex min-w-0 flex-col gap-1 md:col-span-1 md:col-start-2">
          {/* On a phone the status sits in the card, under the name. */}
          <span className="md:hidden">
            <RowStatus view={view} />
          </span>
          {view.host && status === "active" && <SessionCounts view={view} />}
          {view.host && view.presence !== "online" && view.copyAsOf !== null && (
            <span data-testid="copy-as-of" className="font-mono text-2xs text-muted">
              {t("offline copy · as of {{time}}", { time: formatAsOf(view.copyAsOf) })}
            </span>
          )}
          {view.renewal && view.renewal.kind !== "none" && status === "active" && (
            <RenewalLine
              id={id}
              status={view.renewal}
              viewerAdmin={viewerAdmin}
              onCode={onRenewCode}
              onRetry={onRenewRetry}
              onRemove={onRemove}
              onAddComputer={onAddComputer}
              onNarrow={upgraded ? onControl : undefined}
            />
          )}
          {view.pausedWith && view.pausedWith.length > 0 && status === "active" && (
            <span data-testid={`paused-${id}`} className="text-2xs leading-4 text-muted">
              {t("Control between {{ids}} is paused while their identity keys are renewed.", { ids: view.pausedWith.join(", ") })}
            </span>
          )}
          {status === "active" &&
            (view.pauses ?? []).map((p) => (
              <span key={p.ending.edges.map((e) => `${e.host}<${e.id}`).join(",") || id} data-testid={`pause-${id}`} className="flex flex-col items-start gap-0.5 text-2xs leading-4 text-muted">
                {p.ending.orphan && (
                  <span data-testid={`pause-orphan-${id}`}>{t("Started by a device that is no longer a full device: it never ends by itself — end it here.")}</span>
                )}
                {p.ending.held.map((h) => (
                  <span key={`${h.host}<${h.id}`} data-testid={`pause-held-${id}-${h.id}`} data-why={h.why} data-signal="warn" className="text-warn-fg">
                    {pauseHeldText(h, t)}
                  </span>
                ))}
                {viewerAdmin && onEndPause && (
                  <button type="button" data-testid={`end-pause-${id}`} className="text-muted underline underline-offset-2 hover:text-fg" onClick={() => onEndPause(p.op)}>
                    {t("end pause")}
                  </button>
                )}
              </span>
            ))}
          {view.retiredLoginMs != null && status === "active" && (
            <span data-testid={`retired-login-${id}`} className="font-mono text-2xs leading-4 text-muted">
              {t("its old identity key was used to sign in at {{time}}", { time: formatAsOf(view.retiredLoginMs) })}
            </span>
          )}
          {view.isolatedControllers && view.isolatedControllers.length > 0 && status === "active" && (
            <span data-testid={`isolated-controllers-${id}`} data-signal="warn" className="text-2xs leading-4 text-warn-fg">
              {t("It lets {{ids}} control it, which is isolated: settle it on its row first.", { ids: view.isolatedControllers.join(", ") })}
            </span>
          )}
          {notes}
          {keys}
          {removed ? (
            <span className="truncate font-mono text-2xs text-faint">{id}</span>
          ) : (
            <RowDetails
              view={view}
              open={openWarnings}
              onOpenSession={onOpenSession}
              warnings={lines ? <ReEnrollWarning id={id} lines={lines} onReEnroll={onReEnroll} onDismiss={onDismissReEnroll} onWarnAgain={onWarnAgain} /> : null}
            />
          )}
        </div>
      </div>
      {children ? <div className="border-t border-border [&>*:last-child]:border-b-0">{children}</div> : null}
    </li>
  );
}
