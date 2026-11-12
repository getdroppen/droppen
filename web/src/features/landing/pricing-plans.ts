import { PLAN_TABLE } from "@/lib/hub/plan";

const GIB = 1024 ** 3;

function publicPlan(id: "free" | "pro") {
  const plan = PLAN_TABLE[id]!;
  return { hosts: plan.hosts, devices: plan.devices, retentionDays: plan.retentionDays, transferBytes: plan.transferBytes! };
}

export const PLAN_NUMBERS = { free: publicPlan("free"), pro: publicPlan("pro") };
export const THROTTLED_KB_PER_SEC = 256;
export const PRO_PRICE = { monthly: 10, yearly: 96 } as const;

export function yearlySaving(): number {
  return Math.round((1 - PRO_PRICE.yearly / (12 * PRO_PRICE.monthly)) * 100);
}

export function gb(bytes: number): number {
  return Math.round(bytes / GIB);
}
