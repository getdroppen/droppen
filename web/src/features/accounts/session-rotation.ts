import type { components } from "@/api/schema";
import {
  formatRestUntil,
  REST_SCOPE_ALL,
  restLabel,
  restUntilText,
  type Translate,
} from "@/features/accounts/account-rest";

/**
 * The server moving a session off an account that hit a limit and carrying the
 * conversation on under another one, as the session view reports it: the latest
 * such move (or failed attempt at one) per session. Since `trigger` exists the
 * move may also have been made *ahead* of the limit, between turns, because a
 * window of the old account was about to run out — told as calmly as any
 * successful move, with how full that window was and when it resets.
 *
 * Kept free of React, like the rest helpers it borrows its wording from, so
 * which notice a session earns and how it reads are pinned by plain tests.
 */

/** One rotation; the schema type is nullable because the field is. */
export type SessionRotation = NonNullable<components["schemas"]["SessionRotation"]>;
type AccountRef = components["schemas"]["AccountRef"];

/**
 * Whether the server moved the session *ahead* of a limit — at a quiet point,
 * because a window of the old account was about to run out — rather than after
 * a turn failed on it. A server too old to say is one that only ever did the
 * latter, so a missing trigger reads as `limit`.
 */
export function rotationWasAhead(rotation: Pick<SessionRotation, "trigger"> | null | undefined): boolean {
  return rotation?.trigger === "ahead";
}

/** `fable` → `Fable`, the way the server names a family's window. */
function familyTitle(scope: string): string {
  return scope.charAt(0).toUpperCase() + scope.slice(1);
}

/**
 * The window a move ahead of a limit was about: `5-hour window`, `weekly
 * window`, or a family's own weekly window. Anything else (a row this build
 * does not expect to be `ahead`) falls back to the rest chip's label.
 */
export function aheadWindowLabel(rotation: Pick<SessionRotation, "class" | "scope">, t: Translate): string {
  if (rotation.class === "session") return t("5-hour window");
  if (rotation.class === "weekly") {
    return rotation.scope === REST_SCOPE_ALL
      ? t("weekly window")
      : t("{{family}} weekly window", { family: familyTitle(rotation.scope) });
  }
  return restLabel(rotation, t);
}

/** The near window's used percent, whole; `?` when the server did not say. */
export function aheadPercentText(rotation: Pick<SessionRotation, "usedPercent">): string {
  const used = rotation.usedPercent;
  return typeof used === "number" && Number.isFinite(used) ? String(Math.round(Math.min(Math.max(used, 0), 100))) : "?";
}

/** When the near window resets, as a clock reading; empty when unknown. */
function aheadResetText(rotation: Pick<SessionRotation, "untilAt">, now: number): string {
  return formatRestUntil(rotation.untilAt, now);
}

/** How long a rotation stays worth a notice; older ones are history. */
export const ROTATION_NOTICE_WINDOW_MS = 24 * 60 * 60_000;

/** How many accounts one turn tries before the server gives up and pauses. */
export const ROTATION_ATTEMPT_CAP = 3;

/**
 * Whether a rotation happened within the notice window. A timestamp slightly
 * ahead of this clock is a skewed clock, not an old event, so it counts as
 * recent; an unreadable one counts as nothing.
 */
export function rotationIsRecent(
  rotation: Pick<SessionRotation, "at"> | null | undefined,
  now: number = Date.now(),
): boolean {
  if (!rotation) return false;
  const at = Date.parse(rotation.at);
  if (Number.isNaN(at)) return false;
  return now - at < ROTATION_NOTICE_WINDOW_MS;
}

/** An account as a notice names it: its label, else its id. */
export function accountRefName(ref: AccountRef | undefined): string {
  return ref?.label || ref?.id || "?";
}

/**
 * When the old account's limit lifts, in the rest chip's words. A notice
 * outlives the limit it reports, unlike a chip, so an end already behind us
 * reads as the clock time it came rather than as "1 min" of a rate limit.
 */
export function rotationUntilText(
  rotation: Pick<SessionRotation, "class" | "untilAt">,
  t: Translate,
  now: number = Date.now(),
): string {
  const until = rotation.untilAt === null ? Number.NaN : Date.parse(rotation.untilAt);
  if (!Number.isNaN(until) && until <= now) {
    return t("resumes {{at}}", { at: formatRestUntil(rotation.untilAt, now) });
  }
  return restUntilText(rotation, t, now);
}

export type RotationTone = "info" | "warn";

export interface RotationNoticeText {
  /** `info` when the session carried on; `warn` when it could not. */
  tone: RotationTone;
  text: string;
  /** Hover text: which account hit what, and when. */
  title: string;
}

/**
 * The composer banner for a rotation. A move that worked is information, told
 * calmly; one that found no account or ran out of attempts is a warning,
 * because the session is waiting on the old account again. `null` for an
 * outcome this build does not know, rather than a guess at what it meant.
 */
export function rotationNotice(
  rotation: SessionRotation,
  t: Translate,
  now: number = Date.now(),
): RotationNoticeText | null {
  const from = accountRefName(rotation.from);
  if (rotationWasAhead(rotation)) return aheadNotice(rotation, from, t, now);
  const limit = restLabel(rotation, t);
  const until = rotationUntilText(rotation, t, now);
  let tone: RotationTone;
  let text: string;
  switch (rotation.outcome) {
    case "switched":
      tone = "info";
      text = t("switched automatically from {{from}} to {{to}} · {{from}}: {{limit}}, {{until}}", {
        from,
        to: accountRefName(rotation.to),
        limit,
        until,
      });
      break;
    case "no_candidate":
      tone = "warn";
      text = t("{{from}} ran out and no account is free to switch to · {{until}}", { from, until });
      break;
    case "cap_reached":
      tone = "warn";
      text = t("still failing after {{n}} switches in a row, so it has paused · {{until}}", {
        n: ROTATION_ATTEMPT_CAP,
        until,
      });
      break;
    default:
      return null;
  }
  // Said whatever the outcome: a subagent the move cut short is work the user
  // may have to start again, and only the server knows it happened.
  if (rotation.interruptedTasks > 0) {
    text += ` · ${t("{{n}} background subtasks were interrupted", { n: rotation.interruptedTasks })}`;
  }
  const title = t("{{from}}: {{limit}} · {{at}}", {
    from,
    limit,
    at: formatRestUntil(rotation.at, now) || rotation.at,
  });
  return { tone, text, title };
}

/**
 * The banner for a move ahead of a limit: information, never a warning — the
 * session carried on and nothing failed. Only a move that worked is reported
 * by the server this way; any other outcome under `ahead` is one this build
 * does not know, and earns no notice rather than a guess.
 */
function aheadNotice(rotation: SessionRotation, from: string, t: Translate, now: number): RotationNoticeText | null {
  if (rotation.outcome !== "switched") return null;
  const vars = {
    from,
    to: accountRefName(rotation.to),
    window: aheadWindowLabel(rotation, t),
    pct: aheadPercentText(rotation),
    at: aheadResetText(rotation, now),
  };
  const text = vars.at
    ? t("switched ahead of the limit from {{from}} to {{to}} · {{from}}: {{window}} {{pct}}% used, resets {{at}}", vars)
    : t("switched ahead of the limit from {{from}} to {{to}} · {{from}}: {{window}} {{pct}}% used", vars);
  const title = t("{{from}}: {{window}} {{pct}}% used · {{at}}", {
    ...vars,
    at: formatRestUntil(rotation.at, now) || rotation.at,
  });
  return { tone: "info", text, title };
}

/**
 * What a session's dismissed rotation is remembered under, on its machine
 * (`machineStorageKey`: `rh.rotation-dismissed.<source>.<id>` locally).
 */
export function rotationDismissName(source: string, id: string): string {
  return `rotation-dismissed.${source}.${id}`;
}

/**
 * Whether a stored dismissal covers this rotation. What is stored is the
 * dismissed rotation's own timestamp, so closing one notice never hides the
 * next rotation of the same session.
 */
export function rotationDismissed(stored: string | null, rotation: Pick<SessionRotation, "at">): boolean {
  return stored !== null && stored === rotation.at;
}

/** Seconds a rotation notice stays up before it closes itself, as if dismissed. */
export const ROTATION_NOTICE_SECONDS = 15;

/**
 * One second of a notice's countdown. It only runs while the page is on
 * screen — a notice nobody could see must not close unseen — and stands still
 * while the pointer or focus is on the close button, which then reads ×.
 */
export function countdownTick(left: number, { visible, held }: { visible: boolean; held: boolean }): number {
  if (!visible || held) return left;
  return Math.max(0, left - 1);
}

/**
 * Whether the header marks the bound account as one the session was moved
 * onto: only after a move that worked, only while it is recent, and only while
 * the session is still on the account it was moved to — once someone switches
 * it by hand the marker would be describing a different account.
 */
export function rotationMarkerShown(
  rotation: SessionRotation | null | undefined,
  accountId: string | null | undefined,
  now: number = Date.now(),
): rotation is SessionRotation {
  return (
    !!rotation &&
    rotation.outcome === "switched" &&
    !!accountId &&
    rotation.to?.id === accountId &&
    rotationIsRecent(rotation, now)
  );
}

/**
 * The header marker's hover text: where the session came from, and why — and,
 * for a move ahead of a limit, that it was made before anything failed.
 */
export function rotationMarkerTitle(rotation: SessionRotation, t: Translate, now: number = Date.now()): string {
  const from = accountRefName(rotation.from);
  if (rotationWasAhead(rotation)) {
    const vars = {
      from,
      at: formatRestUntil(rotation.at, now) || rotation.at,
      window: aheadWindowLabel(rotation, t),
      pct: aheadPercentText(rotation),
      until: aheadResetText(rotation, now),
    };
    return vars.until
      ? t("switched here ahead of the limit from {{from}} at {{at}} · {{from}}: {{window}} {{pct}}% used, resets {{until}}", vars)
      : t("switched here ahead of the limit from {{from}} at {{at}} · {{from}}: {{window}} {{pct}}% used", vars);
  }
  return t("switched here automatically from {{from}} at {{at}} · {{from}}: {{limit}}, {{until}}", {
    from,
    at: formatRestUntil(rotation.at, now) || rotation.at,
    limit: restLabel(rotation, t),
    until: rotationUntilText(rotation, t, now),
  });
}
