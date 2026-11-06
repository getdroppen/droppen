/*
 * What each plan includes, as droppen.app/pricing shows it. The numbers are
 * Droppen Hub's own entitlements — its `free` and `pro` plans, which the hub
 * enforces — copied here because the page is static: the hub's own tests read
 * this file (`PLAN_NUMBERS`, `THROTTLED_KB_PER_SEC`) and fail the moment the
 * two say different things, so keep one plan to a line and its values plain
 * numbers, `* GIB` or `null`. The price is not the hub's: there is nothing to
 * pay with yet (docs/relay-and-plans.md §3.10, M4), so it is decided here.
 */

/** A GiB, as the hub counts transfer; the page says "GB". */
export const GIB = 1024 ** 3;

interface PlanNumbers {
  /** `hosts.max`: devices that run AI. */
  hosts: number;
  /** `devices.max`: every device in the account, the ones that run AI included. */
  devices: number;
  /** `storage.retention_days`: how far back the cloud's encrypted copy reaches. */
  retentionDays: number;
  /** `transfer.included_bytes`: a month's transfer through Droppen's relays. */
  transferBytes: number;
  /** `transfer.overage`: past it, slowed (0) or billed by use (1). */
  overage: 0 | 1;
}

export const PLAN_NUMBERS: { free: PlanNumbers; pro: PlanNumbers } = {
  free: { hosts: 3, devices: 8, retentionDays: 7, transferBytes: 10 * GIB, overage: 0 },
  pro: { hosts: 20, devices: 40, retentionDays: 30, transferBytes: 100 * GIB, overage: 1 },
};

/** How fast a free account's relayed traffic goes once its month's transfer is used (the hub's throttle), in KB/s. */
export const THROTTLED_KB_PER_SEC = 256;

/** Pro's price, in US dollars (the user's decision of 2026-10-19). */
export const PRO_PRICE = { monthly: 10, yearly: 96 } as const;

/** What paying for a year saves against twelve months, in whole percent. */
export function yearlySaving(price: { monthly: number; yearly: number } = PRO_PRICE): number {
  return Math.round((1 - price.yearly / (12 * price.monthly)) * 100);
}

/** Transfer as the page says it: whole GB. */
export function gb(bytes: number): number {
  return Math.round(bytes / GIB);
}
