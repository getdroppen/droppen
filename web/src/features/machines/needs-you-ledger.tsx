/**
 * 「需要你」: every session that needs you, across every device that runs AI,
 * as one ledger (`docs/local-aggregator.md` §6.4) — asking you first,
 * then unread, then working, newest first in each. One row is one session on
 * its device; picking it opens that session there.
 *
 * Shared by droppen.app (every machine of the account) and the local app
 * (only `local`, where the device column says nothing and is left out): it
 * draws a {@link NeedsYou} and knows nothing of where the rows came from.
 */
import { useTranslation } from "react-i18next";
import { ArrowRight } from "lucide-react";
import { CapsLabel } from "@/components/ui/caps-label";
import { cn } from "@/lib/cn";
import { formatRelative } from "@/lib/format";
import { useTicker } from "@/lib/ticker";
import type { MachineKey } from "@/lib/machines/key";
import { ActivityChipView, EdgeBar, UnreadPill } from "@/features/sessions/attention-badges";
import type { LedgerEntry, NeedsYou } from "./needs-you";

/** How many rows the ledger draws before it says how many more there are. */
export const LEDGER_SHOWN = 40;

export interface NeedsYouLedgerProps {
  needs: NeedsYou;
  /** A device's name to show. */
  nameOf: (machine: MachineKey) => string;
  /** Open the entry's session on its device. */
  onOpen: (entry: LedgerEntry) => void;
  /** Show which device each row is on (off with only one device). */
  devices?: boolean;
  /**
   * Whether a device is shown read-only here (the local app's other devices,
   * S1-7): a session asking you there opens to be read, and says the answer
   * is given on that device.
   */
  readOnly?: (machine: MachineKey) => boolean;
  /** Where the rows were read: said quietly in the heading. */
  note?: string | null;
  limit?: number;
  className?: string;
}

const EDGE = { asking: "asking", unread: "unread", working: "working" } as const;

export function NeedsYouLedger({ needs, nameOf, onOpen, devices = true, readOnly, note, limit = LEDGER_SHOWN, className }: NeedsYouLedgerProps) {
  const { t } = useTranslation();
  // Ages are minutes: a 30 s clock keeps them honest on a page left open.
  const now = useTicker(30_000);
  const shown = needs.entries.slice(0, limit);
  const more = needs.entries.length - shown.length;
  // The summary's own numbers: sessions asking, unread messages, sessions
  // working — a working session holding unread answers is listed with the
  // unread, its working chip on it, and counted as working all the same.
  const counts = [
    needs.asking > 0 ? t("asking you {{n}}", { n: needs.asking }) : null,
    needs.unread > 0 ? t("unread {{n}}", { n: needs.unread }) : null,
    needs.working > 0 ? t("working {{n}}", { n: needs.working }) : null,
    note ?? null,
  ].filter((part): part is string => !!part);
  return (
    <section data-testid="needs-you-ledger" aria-label={t("what needs you")} className={cn("flex min-w-0 flex-col", className)}>
      <div className="flex h-10 min-w-0 shrink-0 items-center gap-2.5 border-b border-border px-6 max-md:px-4">
        <CapsLabel className="shrink-0 text-fg">{devices ? t("needs you · every device") : t("what needs you")}</CapsLabel>
        <span className="min-w-0 truncate font-mono text-2xs text-faint">{counts.join(" · ")}</span>
      </div>
      {shown.length === 0 ? (
        <p data-testid="needs-you-empty" className="border-b border-border px-6 py-4 font-mono text-2xs text-faint max-md:px-4">
          {t("nothing needs you: no session is asking, unread or working")}
        </p>
      ) : (
        <ul className="flex flex-col">
          {shown.map((entry) => (
            <li key={entry.key} className="border-b border-border">
              <LedgerRow
                entry={entry}
                device={devices ? nameOf(entry.ref.machine) : null}
                readOnly={readOnly?.(entry.ref.machine) === true}
                now={now}
                onOpen={() => onOpen(entry)}
              />
            </li>
          ))}
        </ul>
      )}
      {more > 0 && (
        <p className="border-b border-border px-6 py-2.5 font-mono text-2xs text-faint max-md:px-4">{t("and {{n}} more", { n: more })}</p>
      )}
    </section>
  );
}

function LedgerRow({
  entry,
  device,
  readOnly,
  now,
  onOpen,
}: {
  entry: LedgerEntry;
  device: string | null;
  readOnly: boolean;
  now: number;
  onOpen: () => void;
}) {
  const { t } = useTranslation();
  const { row, kind } = entry;
  // Asking you on a device shown read-only here: opened to be read; answered there.
  const elsewhere = kind === "asking" && readOnly;
  const age = entry.at > 0 ? formatRelative(new Date(Math.min(entry.at, now)).toISOString()) : "";
  return (
    <button
      type="button"
      data-testid="needs-you-row"
      data-kind={kind}
      data-session-key={entry.key}
      onClick={onOpen}
      className={cn(
        "relative grid w-full min-w-0 items-center gap-x-4 text-left hover:bg-hover",
        "min-h-13 px-6 py-2 max-md:min-h-0 max-md:gap-y-0.5 max-md:px-4",
        device
          ? "grid-cols-[minmax(0,10rem)_minmax(0,1fr)_minmax(0,9rem)_3.5rem_auto] max-md:grid-cols-[minmax(0,1fr)_auto]"
          : "grid-cols-[minmax(0,1fr)_minmax(0,9rem)_3.5rem_auto] max-md:grid-cols-[minmax(0,1fr)_auto]",
      )}
    >
      <EdgeBar state={EDGE[kind]} />
      {device && <span className="truncate font-mono text-2xs text-fg max-md:order-3 max-md:text-faint">{device}</span>}
      <span className="flex min-w-0 flex-col">
        <span className={cn("min-w-0 truncate text-[13px] leading-4.5 text-fg", entry.unread > 0 && "font-semibold")}>{row.title}</span>
        {elsewhere && (
          <span data-answer-elsewhere="" className="min-w-0 truncate font-mono text-2xs text-faint">
            {t("answer it on that device itself")}
          </span>
        )}
      </span>
      <span className="flex min-w-0 items-center gap-1.5 max-md:justify-end">
        <ActivityChipView row={row} />
        <UnreadPill count={entry.unread} />
      </span>
      <span className="font-mono text-2xs text-faint tabular-nums max-md:order-4 max-md:text-right">{age}</span>
      <span
        className={cn(
          "flex h-7 items-center justify-center gap-1 justify-self-end px-2.5 text-xs whitespace-nowrap max-md:hidden",
          kind === "asking" ? "border border-border-strong font-medium text-fg" : "text-muted",
        )}
      >
        {kind === "asking" && !elsewhere ? t("answer") : t("open")}
        <ArrowRight aria-hidden="true" className="size-3" />
      </span>
    </button>
  );
}
