import { Link } from "@tanstack/react-router";
import { ChevronRight } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useDesktopUpdate, type UpdateStatus } from "@/app/desktop-update";
import { cn } from "@/lib/cn";

/*
 * Settings → General's one line to the update page (`updates-view.tsx`): the
 * rail only shows its update control when there is something to say, so this
 * is where the page is reached the rest of the time. In a browser there is no
 * bridge and no line; the page itself says updates are the app's.
 */

const sectionTitle = "label-caps -mx-3 -mt-3 mb-3 flex h-9 items-center border-b border-border px-3 text-2xs text-fg";

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
    <section className="border-b border-border p-3" data-testid="settings-updates">
      <h2 data-label="" className={sectionTitle}>
        {t("Updates")}
      </h2>
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
        <ChevronRight className="size-3.5 shrink-0 text-faint" strokeWidth={1.5} />
      </Link>
    </section>
  );
}

/** The line in Settings → General, following the app's bridge. */
export function SettingsUpdatesRow() {
  const { status } = useDesktopUpdate();
  return <UpdatesRow status={status} />;
}
