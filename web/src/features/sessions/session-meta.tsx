/*
 * The session header's secondary facts, formatted in one place and shown
 * behind a chip.
 */
import { Check, Copy, Info } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { formatBytes, formatCompact, formatInt, formatTime } from "@/lib/format";

/** The session facts the header keeps behind a chip, already formatted. */
export type SessionMetaFacts = {
  /** Absolute working directory, or null when the session reports none. */
  cwd: string | null;
  /** When it last moved, empty when the server sent no usable stamp. */
  updated: string;
  /** Transcript size on disk, empty when unknown. */
  size: string;
  /** Everything the transcript holds, not just the loaded window. */
  items: number;
  /** How much of that the loaded window is missing; 0 means all of it is here. */
  omitted: number;
};

/**
 * Read the header's secondary facts off a session, in one place that can be
 * tested without a DOM.
 *
 * Formatting is the whole job — every field here used to be interpolated into
 * one line of the header. The counts arrive from a paging window that can
 * briefly disagree with itself (a delta can land before the page that explains
 * it), so a negative "omitted" is a state this has to survive rather than
 * print.
 */
export function sessionMetaFacts(input: {
  cwd?: string | null;
  updatedAt: string | null;
  sizeBytes: number | null;
  totalItems: number;
  omittedItems: number;
}): SessionMetaFacts {
  return {
    cwd: input.cwd ? input.cwd : null,
    updated: formatTime(input.updatedAt),
    size: formatBytes(input.sizeBytes),
    items: Math.max(0, input.totalItems),
    omitted: Math.max(0, input.omittedItems),
  };
}

/** One `label   value` line of the session-details popover. */
function MetaRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-baseline gap-2 font-mono text-2xs">
      <span className="shrink-0 text-faint">{label}</span>
      <span className="min-w-0 flex-1 text-right break-all text-muted">{value}</span>
    </div>
  );
}

/**
 * The session's reference facts, one tap away instead of two lines down.
 *
 * The header used to spend two of its lines on the working directory and a
 * time·size·count run-on — on a phone that is half the chrome above a
 * transcript, for facts nobody reads twice. The count stays on the chip itself
 * because it is the one number that says how much history is above the fold;
 * the rest opens on demand, with the path copyable, since a truncated path is
 * useless and this one was always truncated.
 *
 * Deliberately not the same popover as the drift mark beside it: a downgrade is
 * a warning about what is running now, this is reference about what the session
 * is, and folding both into one chip would bury the warning.
 */
export function SessionMetaChip({ facts }: { facts: SessionMetaFacts }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const cwd = facts.cwd;

  const copy = async (path: string) => {
    try {
      await navigator.clipboard.writeText(path);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // No clipboard permission (or no secure context): the path in the popover
      // is selectable text, which is the fallback that always works.
    }
  };

  return (
    <Popover>
      <PopoverTrigger
        title={t("session details")}
        aria-label={t("session details")}
        className="flex h-5 shrink-0 items-center gap-1 px-1.5 font-mono text-2xs text-faint hover:bg-hover hover:text-fg"
      >
        <Info className="size-3 shrink-0" />
        <span className="tabular-nums">{formatCompact(facts.items)}</span>
      </PopoverTrigger>
      <PopoverContent side="bottom" className="w-72 max-w-[calc(100vw-2rem)]">
        <div className="flex flex-col gap-2">
          {cwd && (
            <div className="flex flex-col gap-1">
              <span className="font-mono text-2xs text-faint">{t("working directory")}</span>
              <div className="flex items-start gap-1.5">
                <span className="min-w-0 flex-1 font-mono text-2xs break-all text-fg select-all">
                  {cwd}
                </span>
                <button
                  type="button"
                  title={copied ? t("copied") : t("copy")}
                  aria-label={copied ? t("copied") : t("copy")}
                  onClick={() => void copy(cwd)}
                  className="flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg"
                >
                  {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
                </button>
              </div>
            </div>
          )}
          <div className="flex flex-col gap-0.5">
            {facts.updated && <MetaRow label={t("updated")} value={facts.updated} />}
            {facts.size && <MetaRow label={t("size")} value={facts.size} />}
            <MetaRow
              label={t("items")}
              value={
                facts.omitted > 0
                  ? `${formatInt(facts.items)} (${t("{{n}} omitted", { n: facts.omitted })})`
                  : formatInt(facts.items)
              }
            />
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}
