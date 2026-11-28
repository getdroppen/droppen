/**
 * Tool permission prompts an agent stopped to ask (`session/request_permission`
 * for ACP, `can_use_tool` for Claude Code).
 *
 * Pure, and free of React.
 */
import type { components } from "@/api/schema";
import { ApiError } from "@/api/errors";

export type SessionPermission = components["schemas"]["SessionPermission"];
export type PermissionOption = components["schemas"]["PermissionOption"];
export type PermissionOptionKind = components["schemas"]["PermissionOptionKind"];
export type PermissionState = components["schemas"]["PermissionState"];

/** The token a `409` carries when another device (or a double tap) settled it first. */
export const PERMISSION_SETTLED = "permission_settled";

/**
 * The record a card should draw: the feed's, unless this screen already heard
 * a settled outcome directly from the POST response (or a `409`).
 */
export function effectivePermission(
  record: SessionPermission | null | undefined,
  local: SessionPermission | null | undefined,
): SessionPermission | null {
  if (!local) return record ?? null;
  if (!record) return local;
  if (local.requestId !== record.requestId) return record;
  return record.state === "pending" && local.state !== "pending" ? local : record;
}

/**
 * The permission record a `409 permission_settled` refusal came back with, or
 * `null` for any other error.
 */
export function settledPermissionOf(error: unknown): SessionPermission | null {
  if (!(error instanceof ApiError) || error.code !== PERMISSION_SETTLED) return null;
  const permission = error.body.permission as Partial<SessionPermission> | null | undefined;
  if (!permission || typeof permission !== "object" || typeof permission.requestId !== "string") return null;
  if (typeof permission.state !== "string" || !Array.isArray(permission.options)) return null;
  return permission as SessionPermission;
}

/** Pending permissions in the order the agent asked them. */
export function pendingPermissionsOf(
  permissions: readonly SessionPermission[] | null | undefined,
): SessionPermission[] {
  if (!permissions?.length) return [];
  return permissions.filter((permission) => permission.state === "pending");
}

/** Whether the agent already offered an explicit reject/deny option. */
export function hasRejectOption(options: readonly PermissionOption[]): boolean {
  return options.some((option) => option.kind === "reject_once" || option.kind === "reject_always");
}

/** Label of the option the user chose, if any. */
export function selectedOptionName(permission: SessionPermission): string | null {
  if (!permission.selectedOptionId) return null;
  return permission.options.find((option) => option.id === permission.selectedOptionId)?.name ?? permission.selectedOptionId;
}
