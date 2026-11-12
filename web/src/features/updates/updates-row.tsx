import { Link } from "@tanstack/react-router";
import { CaretRightIcon } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { useDesktopUpdate, type UpdateStatus } from "@/app/desktop-update";
import { SettingsSection } from "@/features/settings/layout";
import { cn } from "@/lib/cn";

/*
 * Settings → General's one line to the update page (`updates-view.tsx`): the
 * rail only shows its update control when there is something to say, so this
 * is where the page is reached the rest of the time. In a browser there is no
 * bridge and no line; the page itself says updates are the app's.
 */

export type UpdatesLineState = "up-to-date" | "ready" | "downloading" | "attention";

/** Which version the line is about, and what it says about it. */
export function updatesLine(status: UpdateStatus | null): { version: string; state: UpdatesLineState } | null {
  if (!status) return null;
  if (status.state === "downloading" && status.release) return { version: status.release.version, state: "downloading" };
  if (status.state === "ready" && status.release) return { version: status.release.version, state: "ready" };
  if (status.problem && ["signature", "location", "install"].includes(status.problem)) {
    return { version: status.current, state: "attention" };
  }
  return { version: status.current, state: "up-to-date" };
}

/** The line itself, given the app's status. */
export function UpdatesRow({ status }: { status: UpdateStatus | null }) {
  const { t } = useTranslation();
  const line = updatesLine(status);
  if (!line) return null;
  const state =
    line.state === "ready"
      ? t("is ready")
      : line.state === "downloading"
        ? t("Downloading…")
        : line.state === "attention"
          ? t("Needs attention")
          : t("up to date");
  return (
    <SettingsSection title={t("Updates")} data-testid="settings-updates">
      <Link
        to="/updates"
        aria-label={t("Open the update page")}
        data-testid="settings-updates-row"
        className="flex h-7 max-w-xl items-center justify-between gap-3 pr-1 hover:bg-hover"
      >
        <span className="flex min-w-0 items-baseline gap-2">
          <span className="truncate font-mono text-xs text-fg">{t("Droppen {{version}}", { version: line.version })}</span>
          <span className="font-mono text-2xs text-faint">·</span>
          <span
            data-testid="settings-updates-state"
            className={cn("font-mono text-2xs", line.state === "ready" ? "text-fg" : "text-faint")}
          >
            {state}
          </span>
        </span>
        <CaretRightIcon className="size-3.5 shrink-0 text-faint" />
      </Link>
    </SettingsSection>
  );
}

/** The line in Settings → General, following the app's bridge. */
export function SettingsUpdatesRow() {
  const { status } = useDesktopUpdate();
  return <UpdatesRow status={status} />;
}
