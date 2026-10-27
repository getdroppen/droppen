/**
 * A byte count: `512 B`, `1.5 KB`, `25.0 MB`, `2.0 GB`.
 *
 * `compact` is for a bound quoted in a sentence ("larger than 25 MB"): always
 * in MB, and whole megabytes without a trailing `.0`.
 */
export function formatBytes(bytes: number | null, compact = false): string {
  if (bytes === null) return "";
  if (compact) {
    const mb = bytes / 1024 / 1024;
    return `${Number.isInteger(mb) ? mb : mb.toFixed(1)} MB`;
  }
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(1)} GB`;
}

export function formatRelative(iso: string | null): string {
  if (!iso) return "";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const diff = Date.now() - then;
  const minutes = Math.floor(diff / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo`;
  return `${Math.floor(months / 12)}y`;
}

/**
 * How long something has been going: "<1m" · "12m" · "1h 20m" · "2d 3h".
 *
 * Not [`formatRelative`], which answers "when" and says `now` for anything
 * under a minute — right for a timestamp column, wrong under a spinner, where
 * "running now" reads as a status rather than as a duration. Two units at most,
 * because the second one stops mattering as soon as the first is large.
 */
export function formatElapsed(iso: string | null, now: number = Date.now()): string {
  if (!iso) return "";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  return formatElapsedMs(now - then);
}

/**
 * The same shape as [`formatElapsed`], for a span that arrived already measured.
 *
 * The CLI reports a background task's `durationMs` itself, and that number is
 * right where `now - startedAt` is not: `startedAt` is when *this server* first
 * heard of the task, which after a restart is the restart.
 */
export function formatElapsedMs(ms: number): string {
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  if (minutes < 1) return "<1m";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const rest = minutes % 60;
    return rest ? `${hours}h ${rest}m` : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  const rest = hours % 24;
  return rest ? `${days}d ${rest}h` : `${days}d`;
}

/**
 * A span of wall time, at the resolution the span deserves: `5s` · `1m32s` ·
 * `92m`.
 *
 * Not [`formatElapsed`], which floors at `<1m` because it labels a spinner. A
 * tool call is the other extreme — most of them are over in seconds, and the
 * ones that are not are the whole reason to print anything (a single `Agent`
 * call in real history ran for ninety-two minutes and rendered as an ordinary
 * row). Seconds are dropped past the hour, where they are noise.
 */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  if (total < 60) return `${total}s`;
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  if (minutes < 60) return seconds ? `${minutes}m${seconds}s` : `${minutes}m`;
  return `${minutes}m`;
}

/** The span between two ISO instants, or null when either is unusable. */
export function durationBetween(from: string | null, to: string | null): number | null {
  if (!from || !to) return null;
  const a = Date.parse(from);
  const b = Date.parse(to);
  if (Number.isNaN(a) || Number.isNaN(b) || b < a) return null;
  return b - a;
}

/** Full local timestamp for a hover: the anchor a long tool run has none of. */
export function formatExactTime(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

/** A time of day ("14:02") — with the date when it is not today: how current an offline copy is. */
export function formatAsOf(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const sameDay = new Date(now).toDateString() === d.toDateString();
  return sameDay
    ? d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

/**
 * A moment in the app's language (`lang`), not the browser's: 「10月18日
 * 14:02」 in Chinese and Japanese, "Oct 18, 14:02" in English, 24-hour —
 * `short`: the time alone when it is today.
 */
export function formatMoment(ms: number, lang: string, short = false, now = Date.now()): string {
  const at = new Date(ms);
  const locale = lang && lang !== "auto" ? lang : "en";
  const today = new Date(now).toDateString() === at.toDateString();
  const opts: Intl.DateTimeFormatOptions =
    short && today
      ? { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }
      : { month: locale.startsWith("en") ? "short" : "long", day: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23" };
  try {
    return new Intl.DateTimeFormat(locale, opts).format(at);
  } catch {
    return new Intl.DateTimeFormat("en", opts).format(at);
  }
}

export function formatTime(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** Compact token/count formatting: 942 · 1.2K · 34K · 8.4M · 1.3B. */
export function formatCompact(n: number): string {
  if (!Number.isFinite(n)) return "0";
  const abs = Math.abs(n);
  if (abs < 1000) return String(Math.round(n));
  if (abs < 1_000_000) return `${(n / 1000).toFixed(abs < 10_000 ? 1 : 0)}K`;
  if (abs < 1_000_000_000) return `${(n / 1_000_000).toFixed(abs < 10_000_000 ? 1 : 0)}M`;
  return `${(n / 1_000_000_000).toFixed(1)}B`;
}

/** Full grouped integer for tooltips: 1,234,567. */
export function formatInt(n: number): string {
  return Math.round(n).toLocaleString();
}

/** Estimated cost: $0.42 · $12.80 · $1,260. */
export function formatUSD(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "$0";
  if (n >= 100) return `$${Math.round(n).toLocaleString()}`;
  return `$${n.toFixed(2)}`;
}

/** Time remaining until a future ISO instant: "now" · "45m" · "6h" · "5d". */
export function formatUntil(iso: string | null): string {
  if (!iso) return "";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";
  const diff = then - Date.now();
  if (diff <= 0) return "now";
  const minutes = Math.floor(diff / 60_000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

/**
 * The instant a quota window resets, as a clock reading: "14:30" today,
 * "Sat 07:30" later this week, "Sep 13 07:30" beyond it.
 *
 * `formatUntil` alone answers "how long" at one significant figure — a window
 * that says `2h` for the best part of two hours cannot be planned around. The
 * absolute time is what a person actually schedules against, so it is printed
 * beside the countdown rather than instead of it. `now` is a parameter so the
 * today/other-day split is testable.
 *
 * The weekday form exists for width: a weekly window nearly always resets on
 * another day, and the dated form pushed the row past the width of a phone
 * popover, crushing the window's own label to nothing. Inside the coming week
 * a weekday name cannot be misread — the same name never comes round twice —
 * so it says as much in less space. Past resets and anything a week or more
 * out keep the date, where a bare weekday would be a genuine ambiguity.
 */
export function formatResetAt(iso: string | null, now: number = Date.now()): string {
  if (!iso) return "";
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  const today = new Date(now);
  const sameDay =
    at.getFullYear() === today.getFullYear() &&
    at.getMonth() === today.getMonth() &&
    at.getDate() === today.getDate();
  if (sameDay) return at.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
  // Whole calendar days apart, not 24-hour chunks: "tomorrow at 07:30" is one
  // day away whether it is 20 hours off or four. Rounding absorbs a DST shift.
  const midnight = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const daysAway = Math.round((midnight(at) - midnight(today)) / 86_400_000);
  if (daysAway >= 1 && daysAway <= 6) {
    return at.toLocaleString(undefined, { weekday: "short", hour: "2-digit", minute: "2-digit" });
  }
  return at.toLocaleString(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

/** "Jul 22" style short day label from a YYYY-MM-DD date. */
export function formatDay(date: string): string {
  const d = new Date(`${date}T00:00:00`);
  if (Number.isNaN(d.getTime())) return date;
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/**
 * `/Users/ada/Projects/x` → `~/Projects/x`, so a list of absolute working
 * directories reads as the paths the user types rather than as a column of
 * identical prefixes. An empty or non-matching home leaves the path alone.
 */
export function shortenHome(path: string, homeDir: string): string {
  if (!homeDir || !path.startsWith(homeDir)) return path;
  if (path.length === homeDir.length) return "~";
  return path[homeDir.length] === "/" ? `~${path.slice(homeDir.length)}` : path;
}

export function basename(path: string | null): string {
  if (!path) return "";
  const parts = path.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
