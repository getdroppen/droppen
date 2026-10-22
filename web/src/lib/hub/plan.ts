/*
 * What the account's plan means for what the page says (relay-and-plans
 * §3.8, §4.3): the device and machine limits checked before anything is
 * signed, the hub's 409 `limit` answers read back, how long a machine's
 * offline copy is kept once the machine is away, how full the cloud storage
 * is, and what a downgrade will take away when its grace ends.
 *
 * Every number comes from the plan the hub sends (`/v1/me`'s `plan`, or the
 * `plan` control message's `detail`) — never a hard-coded 7. The one table
 * here mirrors the hub's plans for the plan an account drops to after a
 * downgrade: while the grace lasts, the hub's entitlements are the larger of
 * the two plans, so they cannot say what is coming. The hub's own tests read
 * `PLAN_TABLE` from this file and fail when it and the hub's plans differ:
 * keep one plan to a line, its values plain numbers, `* GIB`, `TIB` or `null`.
 *
 * Pure functions only; the live copy of `/v1/me` is `me.ts`.
 */
import { HubApiError, type LimitDetail, type MachineCopy, type PlanView } from "./api";

export const DAY_MS = 24 * 60 * 60 * 1000;
const GIB = 1024 ** 3;
const TIB = 1024 ** 4;

/** The plans' values the page needs, as the hub defines them (`null`: no limit). */
/** `devices`: every current device, hosts included (local-aggregator §3.8); `hosts`: the ones that run AI (`machines`, their old name). */
export const PLAN_TABLE: Record<string, { retentionDays: number; storageBytes: number; transferBytes: number | null; devices: number; hosts: number; machines: number }> = {
  free: { retentionDays: 7, storageBytes: 5 * GIB, transferBytes: 10 * GIB, devices: 8, hosts: 3, machines: 3 },
  pro: { retentionDays: 30, storageBytes: 100 * GIB, transferBytes: 100 * GIB, devices: 40, hosts: 20, machines: 20 },
  internal: { retentionDays: 3650, storageBytes: TIB, transferBytes: null, devices: 200, hosts: 100, machines: 100 },
};

export const RETENTION_KEY = "storage.retention_days";
export const STORAGE_KEY = "storage.bytes";
export const DEVICES_KEY = "devices.max";
/** The devices that run AI (local-aggregator §3.8). */
export const HOSTS_KEY = "hosts.max";
/** `hosts.max`'s old name, which a hub still sends for one version (and an older hub only). */
export const MACHINES_KEY = "machines.max";

/** An entitlement used up: what the page says about it. */
export type LimitHit = LimitDetail;

/** `hosts.max`, or (from a hub that says only the old name) `machines.max`; `undefined` when neither is said. */
export function hostsLimit(plan: PlanView): number | null | undefined {
  if (HOSTS_KEY in plan.entitlements) return plan.entitlements[HOSTS_KEY];
  return plan.entitlements[MACHINES_KEY];
}

/**
 * Whether adding one more device (`devices.max`: every current device of the
 * chain, hosts included) or one more device that runs AI (`hosts.max`;
 * `machines.max` is read as its old name) would go past the plan: the hit,
 * or `null` (room, no limit, or no plan known — the hub still checks).
 */
export function atLimit(plan: PlanView | null | undefined, key: typeof DEVICES_KEY | typeof HOSTS_KEY | typeof MACHINES_KEY): LimitHit | null {
  if (plan == null) return null;
  if (key === DEVICES_KEY) {
    const limit = plan.entitlements[DEVICES_KEY];
    if (limit == null) return null;
    return plan.usage.devices >= limit ? { key, limit, used: plan.usage.devices } : null;
  }
  const limit = hostsLimit(plan);
  if (limit == null) return null;
  const used = plan.usage.hosts ?? plan.usage.machines;
  return used >= limit ? { key, limit, used } : null;
}

/** The hub's 409 `limit` (the check above raced, or another device filled the place): what is used up, or `null`. */
export function limitOf(error: unknown): LimitHit | null {
  if (!(error instanceof HubApiError) || error.status !== 409 || error.code !== "limit") return null;
  return error.limit ?? { key: "", limit: null, used: 0 };
}

/** How many days the cloud keeps a copy nothing wrote to (`null`: no limit, or not known). */
export function retentionOf(plan: PlanView | null | undefined): number | null {
  const days = plan?.entitlements[RETENTION_KEY];
  return days == null || days <= 0 ? null : days;
}

/** A machine as the page knows it: the hub's `/v1/me` view, and the link's live presence when there is one. */
export interface MachineSeen {
  presence: { state: "online" | "offline" | "unknown"; since_ms: number } | null;
  lastSeenMs: number | null;
}

/**
 * Since when a machine has been away, or `null` while it is online. The hub
 * notes `last_seen_ms` as the agent signs out, so that is the better clock;
 * the presence's `since` is when the hub (re)started for a machine it has not
 * seen since. The live presence decides whether it is online at all.
 */
export function offlineSince(machine: MachineSeen, live?: { state: "online" | "offline" | "unknown"; sinceMs: number } | null): number | null {
  const state = live?.state ?? machine.presence?.state ?? "unknown";
  if (state === "online") return null;
  // Seen online by /v1/me, gone since: the live presence's change is when.
  if (machine.presence?.state === "online" && live && live.state !== "online") return live.sinceMs;
  return machine.lastSeenMs ?? live?.sinceMs ?? machine.presence?.since_ms ?? null;
}

/**
 * What a machine away for a while is told about its offline copy in the
 * cloud (§3.8): from `retention − 2` days, how many days are left before it
 * is deleted; once away longer than the retention — and the hub holds none of
 * its streams any more, or a day past it, when its hourly sweep surely ran —
 * that it was deleted (and comes back once the machine does).
 */
export type CopyWarning =
  | { kind: "soon"; offlineDays: number; daysLeft: number; retentionDays: number }
  | { kind: "deleted"; retentionDays: number };

export function copyWarning(input: { retentionDays: number | null; offlineSinceMs: number | null; copy: MachineCopy | null; now: number }): CopyWarning | null {
  const { retentionDays, offlineSinceMs, copy, now } = input;
  if (retentionDays === null || offlineSinceMs === null) return null;
  const away = now - offlineSinceMs;
  if (away < 0) return null;
  // The hub deletes streams not written for retention + 1 days (the agent
  // trims its own window first): past the retention, "deleted" once the hub
  // says it holds nothing, or the extra day is over too.
  if (away > retentionDays * DAY_MS && (copy === null || copy.streams === 0 || away > (retentionDays + 1) * DAY_MS)) {
    return { kind: "deleted", retentionDays };
  }
  const offlineDays = Math.floor(away / DAY_MS);
  if (offlineDays < Math.max(retentionDays - 2, 1)) return null;
  return { kind: "soon", offlineDays, daysLeft: Math.max(retentionDays - offlineDays, 1), retentionDays };
}

/** The account's cloud storage as the settings show it. */
export interface StorageView {
  usedBytes: number;
  /** `null`: no limit. */
  limitBytes: number | null;
  /** used / limit, 0–1 (more past a downgrade), or `null` without a limit. */
  ratio: number | null;
  /** `high` from 90 %: the agents clean up their oldest session copies; `full`: writes are refused. */
  level: "ok" | "high" | "full";
  /** Each machine's offline copy, largest first. */
  machines: { id: string; bytes: number }[];
  /** The rest: mail, the keyring, and anything not counted per machine. */
  otherBytes: number;
  /** The newest write to any offline copy: where the copies stopped once full. */
  lastWriteMs: number | null;
}

export function storageView(plan: PlanView | null | undefined, machines: { id: string; copy: MachineCopy | null }[]): StorageView | null {
  if (!plan) return null;
  const usedBytes = Math.max(0, plan.usage.storageBytes);
  const limit = plan.entitlements[STORAGE_KEY];
  const limitBytes = limit == null || limit <= 0 ? null : limit;
  const ratio = limitBytes === null ? null : usedBytes / limitBytes;
  const level = ratio === null ? "ok" : ratio >= 1 ? "full" : ratio >= 0.9 ? "high" : "ok";
  const shares = machines
    .map((m) => ({ id: m.id, bytes: Math.max(0, m.copy?.bytes ?? 0) }))
    .sort((a, b) => b.bytes - a.bytes || (a.id < b.id ? -1 : 1));
  const perMachine = shares.reduce((n, m) => n + m.bytes, 0);
  const writes = machines.map((m) => m.copy?.lastWriteMs ?? null).filter((v): v is number => v !== null);
  return {
    usedBytes,
    limitBytes,
    ratio,
    level,
    machines: shares,
    otherBytes: Math.max(0, usedBytes - perMachine),
    lastWriteMs: writes.length ? Math.max(...writes) : null,
  };
}

/** Whole numbers as such, else one decimal: 5 · 4.6 · 0.3. */
function decimal(n: number): string {
  const fixed = n.toFixed(1);
  return fixed.endsWith(".0") ? fixed.slice(0, -2) : fixed;
}

/** A unit for `bytes` (GiB counted as "GB", as the plans say: §3.1). */
function unitFor(bytes: number): { size: number; name: string } {
  if (bytes >= TIB) return { size: TIB, name: "TB" };
  if (bytes >= GIB) return { size: GIB, name: "GB" };
  if (bytes >= 1024 ** 2) return { size: 1024 ** 2, name: "MB" };
  if (bytes >= 1024) return { size: 1024, name: "KB" };
  return { size: 1, name: "B" };
}

/** A plan's limit in bytes: "5 GB", "100 GB", "1 TB". */
export function formatPlanBytes(bytes: number): string {
  const unit = unitFor(bytes);
  return `${decimal(bytes / unit.size)} ${unit.name}`;
}

/** Used against a limit: "4.6 / 5 GB" in one unit, "91.8 MB / 100 GB" when the used bytes read better in their own (just the used bytes without a limit). */
export function formatStoragePair(used: number, limit: number | null): string {
  if (limit === null) return formatPlanBytes(used);
  const unit = unitFor(limit);
  if (unitFor(used).size !== unit.size && used > 0) return `${formatPlanBytes(used)} / ${formatPlanBytes(limit)}`;
  return `${decimal(used / unit.size)} / ${decimal(limit / unit.size)} ${unit.name}`;
}

/** A downgrade in its grace: the plan that ends, when, and what the account has after it. */
export interface Downgrade {
  /** The plan that ends (the grace plan). */
  from: string;
  /** The plan the account is on once the grace ends. */
  to: string;
  untilMs: number;
  /** What `to` keeps and allows (`null` when the page does not know the plan). */
  after: { retentionDays: number; storageBytes: number; transferBytes: number | null } | null;
}

/** The downgrade the account is in the grace of, or `null`. */
export function downgradeOf(plan: PlanView | null | undefined, now: number): Downgrade | null {
  if (!plan || plan.graceUntilMs === null || plan.graceUntilMs <= now || !plan.gracePlan) return null;
  const to = PLAN_TABLE[plan.id];
  return {
    from: plan.gracePlan,
    to: plan.id,
    untilMs: plan.graceUntilMs,
    after: to ? { retentionDays: to.retentionDays, storageBytes: to.storageBytes, transferBytes: to.transferBytes } : null,
  };
}

/** The devices registered with the account's password that the chain does not list yet — worth a look if it was not you. */
export interface Registration {
  device: string;
  createdMs: number;
}

/** How long a registered device that never joined the chain is kept (and so worth showing). */
export const REGISTRATION_TTL_MS = DAY_MS;

/**
 * The registrations to show: not this device, not one the chain knows (its
 * own alerts take over once it is added), not dismissed here, and recent.
 */
export function registrationAlerts(
  seen: Iterable<Registration>,
  opts: { self: string; inChain: (id: string) => boolean; dismissed: ReadonlySet<string>; now: number },
): Registration[] {
  const out = new Map<string, Registration>();
  for (const r of seen) {
    if (r.device === opts.self || opts.inChain(r.device) || opts.dismissed.has(r.device)) continue;
    if (opts.now - r.createdMs > REGISTRATION_TTL_MS) continue;
    const had = out.get(r.device);
    if (!had || r.createdMs > had.createdMs) out.set(r.device, r);
  }
  return [...out.values()].sort((a, b) => a.createdMs - b.createdMs);
}
