/*
 * The connection log as text (`lib/connection-log.ts`): what Copy puts on the
 * clipboard (at most {@link COPY_MAX_CHARS}, the newest) and Download saves
 * (all of it) — a header that says which web build and device agent, which
 * browser and network, how the page is connected, and what the export
 * leaves out ({@link PRIVACY_LINE}), then one compact line per event:
 *
 *   09:30:12.345 +1812.345 W hub.close code=1006 reason="" clean=false openMs=120034
 *
 * Wall-clock time (UTC), seconds since the page loaded (the monotonic `t`,
 * which a clock change does not move), the level (`I`, `W`, `E`), the
 * event's name and its fields as `key=value` — a string in quotes when it
 * has spaces, `-` for none. What the page before a reload logged comes first,
 * under a line of its own. Nothing here adds anything the events do not
 * hold: the redaction is the log's (`connection-log.ts`, PRIVACY), and the
 * header's fields go through it too (`ConnectionLog.clean`).
 */
import { CHATTER, type ConnEvent, type ConnValue } from "@/lib/connection-log";

/** Which events a view shows: all of them, all but the samples and probes, or only warnings and errors. */
export type ConnFilter = "all" | "events" | "problems";

export function filterEvents(events: readonly ConnEvent[], filter: ConnFilter): ConnEvent[] {
  if (filter === "all") return [...events];
  if (filter === "events") return events.filter((e) => !CHATTER.has(e.name));
  return events.filter((e) => e.level !== "info");
}

/** `HH:MM:SS.mmm`, UTC. */
export function clockOf(wall: number): string {
  return new Date(wall).toISOString().slice(11, 23);
}

/** Seconds since the page loaded, three decimals: `+1812.345`. */
export function sinceLoad(t: number): string {
  return `+${(t / 1000).toFixed(3)}`;
}

const LEVEL = { info: "I", warn: "W", error: "E" } as const;

function valueText(v: ConnValue): string {
  if (v === null) return "-";
  if (typeof v === "string") return v === "" || /[\s"=]/.test(v) ? JSON.stringify(v) : v;
  return String(v);
}

/** An event's fields as `key=value …`. */
export function fieldsText(fields: Readonly<Record<string, ConnValue>>): string {
  const parts: string[] = [];
  for (const key in fields) parts.push(`${key}=${valueText(fields[key]!)}`);
  return parts.join(" ");
}

/** One event, one line. */
export function eventLine(e: ConnEvent): string {
  const fields = fieldsText(e.fields);
  return `${clockOf(e.wall)} ${sinceLoad(e.t)} ${LEVEL[e.level]} ${e.name}${fields ? ` ${fields}` : ""}`;
}

/** What the header says, besides the events. */
export interface LogHeader {
  /** The web build the page runs (`buildId`), and the release the device in view's agent says it runs, if it said one. */
  build: string;
  agentVersion?: string | null;
  userAgent: string;
  /** When the page loaded (wall-clock ms). */
  loadedAt: number;
  exportedAt: number;
  /** `navigator.connection` and friends now (`networkFields`). */
  network: Readonly<Record<string, ConnValue>>;
  /** How the page is connected now, as fields (`hub=open device=#3fa9c1 …`), if it is in cloud mode. */
  connection?: Readonly<Record<string, ConnValue>> | null;
  filter: ConnFilter;
  /** How many events the log holds (the export may hold fewer: `filter`). */
  total: number;
}

/** What the export says it holds and leaves out (`lib/connection-log.ts`, PRIVACY), word for word as the rules are. */
export const PRIVACY_LINE =
  "privacy: device ids are salted hashes; requests are route templates; text that is not Droppen's own words is withheld; no tokens, keys, message content, file names, paths, IP or host names, or full URLs. This header names the browser and the network.";

/** The header's lines. */
export function headerLines(h: LogHeader, shown: number): string[] {
  return [
    `Droppen connection log · web build ${h.build} · device agent ${h.agentVersion ?? "-"} · exported ${new Date(h.exportedAt).toISOString()}`,
    `times UTC; +seconds since the page loaded at ${new Date(h.loadedAt).toISOString()}`,
    `browser: ${h.userAgent || "-"}`,
    `network: ${fieldsText(h.network) || "-"}`,
    `connection: ${h.connection ? fieldsText(h.connection) || "-" : "-"}`,
    `events: ${shown} of ${h.total} (${h.filter})`,
    PRIVACY_LINE,
  ];
}

/** At most this many characters in a copy: under what an issue or a chat takes (a GitHub issue: 65,536). Download has everything. */
export const COPY_MAX_CHARS = 60_000;

/** The line that heads a page's events in the text. */
function pageLine(prev: number | null, loadedAt: number): string {
  return prev === null ? `--- this page (loaded ${new Date(loadedAt).toISOString()}) ---` : `--- the page before a reload (loaded ${new Date(prev).toISOString()}) ---`;
}

/**
 * The log as text: the header, then `events` (already filtered), the page
 * before a reload's first, under a line of their own. With `maxChars`, the
 * newest events that fit, and a line saying how many earlier ones are left
 * out and that Download has them.
 */
export function logText(events: readonly ConnEvent[], header: LogHeader, maxChars: number | null = null): string {
  let kept: readonly ConnEvent[] = events;
  const head = headerLines(header, events.length);
  if (maxChars !== null) {
    let budget = maxChars - head.join("\n").length - 200;
    let from = events.length;
    for (let i = events.length - 1; i >= 0; i--) {
      const cost = eventLine(events[i]!).length + 1 + (i === 0 || (events[i - 1]!.prev ?? null) !== (events[i]!.prev ?? null) ? 80 : 0);
      if (cost > budget) break;
      budget -= cost;
      from = i;
    }
    kept = events.slice(from);
  }
  const lines = [...head];
  const left = events.length - kept.length;
  if (left > 0) lines.push(`--- ${left} earlier events are not in this copy (a copy holds at most ${(maxChars ?? 0).toLocaleString("en")} characters): Download has the whole log ---`);
  let page: number | null | undefined;
  for (const e of kept) {
    const at = e.prev ?? null;
    if (at !== page) {
      lines.push(pageLine(at, header.loadedAt));
      page = at;
    }
    lines.push(eventLine(e));
  }
  return `${lines.join("\n")}\n`;
}

/** A file name for the downloaded log: `droppen-connection-log-20261124-093012.txt` (UTC). */
export function logFileName(at: number): string {
  const iso = new Date(at).toISOString();
  return `droppen-connection-log-${iso.slice(0, 10).replace(/-/g, "")}-${iso.slice(11, 19).replace(/:/g, "")}.txt`;
}
