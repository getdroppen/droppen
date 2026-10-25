import { Link } from "@tanstack/react-router";
import { ArrowUpCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { RailHint } from "@/components/ui/rail-hint";
import { cn } from "@/lib/cn";

/*
 * Inside the Droppen desktop app, the rail's update entry and the status the
 * update page (`features/updates/updates-view.tsx`) reads.
 *
 * The entry is always there in the app: a rail item like the others, opening
 * the update page. It never restarts anything itself — a restart closes the
 * window, so it belongs on the page that says what is being installed. Its
 * only state is a small mark: an update ready (until that version is put
 * away with "Later" on the page), a download under way, or a failure worth
 * saying. In a browser, or under an app without the bridge, there is nothing
 * here.
 *
 * The app tells the page through the bridge its init script sets up
 * (`desktop/src-tauri/src/init.js`, part 4), until the app↔server control
 * channel replaces it: `window.__droppenDesktop.updateState()` is the app's
 * update status (only the app, which knows the launch's nonce, can set it),
 * a `droppen:update` event says it changed, `checkForUpdates()` and
 * `restartToUpdate()` ask for those, and `restartToUpdate()` only restarts
 * into a downloaded, verified update.
 */

/** What the app's bridge offers the page. */
type DesktopBridge = {
  updateState?: () => unknown;
  checkForUpdates?: () => void;
  restartToUpdate?: () => void;
};

/** Where a dismissal is kept: the version put away, so a newer one shows again. */
const DISMISSED_KEY = "droppen.desktop.update-dismissed";

const VERSION = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
const STATES = ["idle", "checking", "downloading", "ready", "installing", "restarting"] as const;
const PLACES = ["updatable", "notABundle", "translocated", "readOnly", "symlinked"] as const;
const PROBLEMS = ["offline", "service", "signature", "location", "install", "other"] as const;
/** A failure the rail says unasked; a network or service hiccup is retried silently. */
const SAID_UNASKED = ["signature", "location", "install"];

export type UpdateState = (typeof STATES)[number];
export type UpdatePlace = (typeof PLACES)[number];
export type UpdateProblem = (typeof PROBLEMS)[number];

/** The notes of a release, as the app read them from its release body. */
export interface UpdateNotes {
  version: string;
  english: string;
  chinese: string;
}

/** The app's update status, checked down to the shapes the page draws. */
export interface UpdateStatus {
  current: string;
  state: UpdateState;
  release: { version: string; notes: string | null } | null;
  percent: number | null;
  place: UpdatePlace;
  lastChecked: string | null;
  problem: UpdateProblem | null;
  notes: UpdateNotes | null;
}

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const oneOf = <T extends string>(values: readonly T[], value: unknown): T | null =>
  typeof value === "string" && (values as readonly string[]).includes(value) ? (value as T) : null;

/** The status, or null when the bridge has nothing (a browser), or nothing readable. */
export function parseUpdateStatus(raw: unknown): UpdateStatus | null {
  if (!isRecord(raw)) return null;
  const current = typeof raw.current === "string" && VERSION.test(raw.current) ? raw.current : null;
  if (!current) return null;
  const state = oneOf(STATES, raw.state) ?? "idle";
  const release =
    isRecord(raw.release) && typeof raw.release.version === "string" && VERSION.test(raw.release.version)
      ? { version: raw.release.version, notes: typeof raw.release.notes === "string" ? raw.release.notes : null }
      : null;
  const percent =
    typeof raw.percent === "number" && Number.isFinite(raw.percent) && raw.percent >= 0 && raw.percent <= 100
      ? Math.round(raw.percent)
      : null;
  const notes =
    isRecord(raw.notes) &&
    typeof raw.notes.version === "string" &&
    typeof raw.notes.english === "string" &&
    typeof raw.notes.chinese === "string"
      ? { version: raw.notes.version, english: raw.notes.english, chinese: raw.notes.chinese }
      : null;
  return {
    current,
    state,
    release,
    percent,
    place: oneOf(PLACES, raw.place) ?? "notABundle",
    lastChecked: typeof raw.lastChecked === "string" ? raw.lastChecked : null,
    problem: oneOf(PROBLEMS, raw.problem),
    notes,
  };
}

/** The version the page is about: the release under way, else this one. */
export function updateVersion(status: UpdateStatus): string {
  const underWay = ["downloading", "ready", "installing", "restarting"].includes(status.state);
  return underWay && status.release ? status.release.version : status.current;
}

export type UpdateBadge = "ready" | "downloading" | "problem";

/** What the rail's entry marks, if anything. */
export function updateBadge(status: UpdateStatus | null, dismissed: string | null): UpdateBadge | null {
  if (!status) return null;
  if (status.state === "ready" && status.release && status.release.version !== dismissed) return "ready";
  if (status.state === "downloading" && status.release) return "downloading";
  if (status.problem && SAID_UNASKED.includes(status.problem)) return "problem";
  return null;
}

function bridge(): DesktopBridge | undefined {
  if (typeof window === "undefined") return undefined;
  return (window as unknown as { __droppenDesktop?: DesktopBridge }).__droppenDesktop;
}

function dismissedVersion(): string | null {
  try {
    return localStorage.getItem(DISMISSED_KEY);
  } catch {
    return null;
  }
}

export interface DesktopUpdate {
  /** The app's status, or null in a browser (no bridge). */
  status: UpdateStatus | null;
  /** The version whose rail mark was put away with "Later" on the page. */
  dismissed: string | null;
  dismiss: (version: string) => void;
  restart: () => void;
  check: () => void;
}

/** The app's update status, following the bridge; null in a browser. */
export function useDesktopUpdate(): DesktopUpdate {
  const [status, setStatus] = useState<UpdateStatus | null>(() => parseUpdateStatus(bridge()?.updateState?.()));
  const [dismissed, setDismissed] = useState<string | null>(dismissedVersion);
  useEffect(() => {
    const read = () => {
      setStatus(parseUpdateStatus(bridge()?.updateState?.()));
      setDismissed(dismissedVersion());
    };
    read();
    window.addEventListener("droppen:update", read);
    return () => window.removeEventListener("droppen:update", read);
  }, []);
  const dismiss = (version: string) => {
    try {
      localStorage.setItem(DISMISSED_KEY, version);
    } catch {
      // put away for this page only
    }
    setDismissed(version);
    // Other listeners (the rail, an open page) read the same key: tell them,
    // as the app does when the status changes.
    window.dispatchEvent(new Event("droppen:update"));
  };
  return {
    status,
    dismissed,
    dismiss,
    restart: () => bridge()?.restartToUpdate?.(),
    check: () => bridge()?.checkForUpdates?.(),
  };
}

/** The rail entry's mark: a square, like every other state mark in the app. */
function Mark({ badge }: { badge: UpdateBadge | null }) {
  if (!badge) return null;
  return (
    <span
      data-signal={badge === "problem" ? "warn" : undefined}
      className={cn("absolute -top-0.5 -right-0.5 size-[3px]", badge === "problem" ? "bg-warn" : badge === "ready" ? "bg-fg" : "bg-faint")}
    />
  );
}

/**
 * The update entry: the rail's smallest kind of control, beside the health dot
 * and the theme — an arrow and the version. It is only there when it has
 * something to say: an update downloading or ready (until that version is put
 * away with "Later"), or a failure worth saying. Up to date, the rail stays
 * quiet and the update page is reached from Settings → General
 * (`features/updates/updates-row.tsx`).
 */
export function DesktopUpdate() {
  const { t } = useTranslation();
  const { status, dismissed } = useDesktopUpdate();
  if (!status) return null;
  const badge = updateBadge(status, dismissed);
  if (!badge) return null;
  const ready = badge === "ready" ? status.release : null;
  const version = ready ? ready.version : status.state === "downloading" && status.release ? status.release.version : status.current;
  const label =
    ready
      ? t("Droppen {{version}} is ready · see what's new", { version: ready.version })
      : status.state === "downloading" && status.release
        ? t("Downloading Droppen {{version}}…", { version: status.release.version })
        : t("Updates need attention");
  return (
    <RailHint
      label={label}
      render={
        <Link
          to="/updates"
          aria-label={label}
          data-testid="desktop-update"
          className="relative flex items-center gap-0.5 p-1 text-muted hover:bg-hover hover:text-fg max-[480px]:size-[28px] max-[480px]:justify-center max-[480px]:p-0 max-md:flex-row md:mb-1 md:w-full md:flex-col md:px-0.5"
        />
      }
    >
      <span data-signal={badge === "problem" ? "warn" : undefined} className="relative flex items-center justify-center">
        <ArrowUpCircle
          className={cn("size-3.5", ready ? "text-fg" : badge === "problem" ? "text-warn-fg" : undefined)}
          strokeWidth={1.5}
        />
        <Mark badge={badge} />
      </span>
      <span className={cn("font-mono text-[9px] leading-none max-[480px]:sr-only", ready ? "text-fg" : "text-faint")}>{version}</span>
    </RailHint>
  );
}
