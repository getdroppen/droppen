import { useMutation } from "@tanstack/react-query";
import { CircleNotchIcon, ShieldWarningIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import type { components } from "@/api/schema";
import { cn } from "@/lib/cn";
import { formatExactTime, formatTime } from "@/lib/format";
import { useApi } from "@/lib/machines/context";
import {
  effectivePermission,
  hasRejectOption,
  selectedOptionName,
  settledPermissionOf,
  type PermissionOptionKind,
  type SessionPermission,
} from "./session-permissions";

type Source = components["schemas"]["Source"];

export interface PermissionSession {
  source: Source;
  id: string;
}

type Decision = { kind: "select"; optionId: string } | { kind: "cancel" };

/**
 * Every pending tool permission prompt for the current session, docked above
 * the composer so it is immediately actionable on phone and desktop alike.
 */
export function PendingPermissions({
  permissions,
  session,
}: {
  permissions: readonly SessionPermission[];
  session: PermissionSession;
}) {
  if (permissions.length === 0) return null;
  return (
    <div data-permissions-dock="" className="flex flex-col">
      {permissions.map((permission) => (
        <PermissionCard key={permission.requestId} record={permission} session={session} />
      ))}
    </div>
  );
}

/**
 * One interactive tool permission card, wired to the device API.
 */
export function PermissionCard({
  record,
  session,
}: {
  record: SessionPermission;
  session?: PermissionSession;
}) {
  const api = useApi();
  const [local, setLocal] = useState<SessionPermission | null>(null);
  const decide = useMutation({
    mutationFn: async (decision: Decision) => {
      if (!session) throw new Error("no session to respond to");
      const res = await api.POST("/api/sessions/{source}/{id}/permissions/{requestId}", {
        params: { path: { source: session.source, id: session.id, requestId: record.requestId } },
        body: decision.kind === "select" ? { optionId: decision.optionId } : { cancel: true },
      });
      if (res.error || !res.data) throw new ApiError(res.error);
      return res.data.permission;
    },
    onSuccess: (settled) => setLocal(settled),
    onError: (error) => {
      const settled = settledPermissionOf(error);
      if (settled) setLocal(settled);
    },
  });
  const permission = effectivePermission(record, local) ?? record;
  const refusal = decide.error && !settledPermissionOf(decide.error) ? errorText(decide.error) : null;
  return (
    <PermissionCardView
      permission={permission}
      busy={decide.isPending}
      error={refusal}
      onSelect={session ? (optionId) => decide.mutate({ kind: "select", optionId }) : undefined}
      onCancel={session ? () => decide.mutate({ kind: "cancel" }) : undefined}
    />
  );
}

function optionClass(kind: PermissionOptionKind): string {
  switch (kind) {
    case "allow_once":
      return "bg-fg text-bg font-medium hover:bg-fg/85";
    case "allow_always":
      return "border border-border-strong bg-bg text-fg font-medium hover:bg-hover";
    case "reject_once":
    case "reject_always":
      return "border border-border bg-bg text-danger hover:bg-hover";
    default:
      return "border border-border bg-bg text-fg hover:bg-hover";
  }
}

/**
 * Pure presentation of a tool permission prompt, testable without a server.
 */
export function PermissionCardView({
  permission,
  busy = false,
  error = null,
  onSelect,
  onCancel,
}: {
  permission: SessionPermission;
  busy?: boolean;
  error?: string | null;
  onSelect?: (optionId: string) => void;
  onCancel?: () => void;
}) {
  const { t } = useTranslation();
  const open = permission.state === "pending" && onSelect !== undefined;
  const showCancel = open && onCancel !== undefined && !hasRejectOption(permission.options);
  const chosen = selectedOptionName(permission);

  return (
    <div
      data-permission={permission.state}
      data-permission-id={permission.requestId}
      data-signal="asking"
      className="border-t border-border bg-warn/7 px-4 py-2.5 shadow-[inset_2px_0_0_var(--color-warn)]"
    >
      <div className="mb-1.5 flex items-center gap-2 font-mono text-[10px] leading-[14px]">
        {open ? (
          <span data-signal="warn" className="inline-flex h-4 items-center gap-1 bg-warn px-1 font-semibold text-on-signal">
            <ShieldWarningIcon className="size-3" />
            {t("permission required")}
          </span>
        ) : null}
        <span data-label="" className="label-caps font-semibold text-fg">
          {t("PERMISSION")}
        </span>
        {permission.toolName && <span className="border border-border bg-bg px-1 text-muted">{permission.toolName}</span>}
        {permission.createdAt && (
          <span className="ml-auto tabular-nums text-faint">{formatTime(permission.createdAt)}</span>
        )}
      </div>

      <div className="flex min-w-0 flex-col gap-1.5">
        <div className="font-mono text-xs leading-5 font-medium break-words text-fg">{permission.title}</div>
        {permission.reason && <div className="text-2xs leading-4 break-words text-muted">{permission.reason}</div>}
        {permission.input && (
          <pre className="max-h-36 overflow-y-auto border border-border bg-raised px-2 py-1.5 font-mono text-2xs leading-4 break-words whitespace-pre-wrap text-fg select-text">
            {permission.input}
          </pre>
        )}

        {open ? (
          <fieldset disabled={busy} className="mt-0.5 flex flex-wrap items-center gap-1.5">
            {permission.options.map((option) => (
              <button
                key={option.id}
                type="button"
                data-permission-option={option.id}
                data-permission-kind={option.kind}
                onClick={() => onSelect!(option.id)}
                className={cn(
                  "h-7 px-2.5 text-xs transition-colors disabled:cursor-not-allowed disabled:opacity-40",
                  optionClass(option.kind),
                )}
              >
                {option.name}
              </button>
            ))}
            {showCancel && (
              <button
                type="button"
                data-permission-cancel=""
                onClick={onCancel}
                className="h-7 px-2.5 text-xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40"
              >
                {t("cancel")}
              </button>
            )}
            {busy && <CircleNotchIcon className="size-3 animate-spin text-muted" />}
          </fieldset>
        ) : (
          <div
            data-settled={permission.state}
            title={formatExactTime(permission.answeredAt) || undefined}
            className={cn(
              "font-mono text-[10px]",
              permission.state === "resolved" ? "text-ok/80" : "text-muted",
            )}
          >
            {permission.state === "resolved" ? "✓" : "⊘"}{" "}
            {[
              permission.state === "resolved"
                ? chosen ?? t("decided")
                : t("cancelled"),
              permission.answeredBy,
              formatTime(permission.answeredAt),
            ]
              .filter((part): part is string => !!part)
              .join(" · ")}
          </div>
        )}

        {error && (
          <p data-signal="failed" className="font-mono text-2xs leading-4 break-words text-danger">
            {t("send failed:")} {error}
          </p>
        )}
      </div>
    </div>
  );
}
