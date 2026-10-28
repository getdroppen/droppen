import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * A card's whole status: a 7px square and a few words, as the device rows
 * draw theirs (`features/cloud/device-row.tsx`). Only `danger` (the thing
 * cannot run at all) colours its words; the square says the rest.
 */
export type StatusTone = "ok" | "danger" | "faint" | "muted";

export function StatusWord({ tone, children, className }: { tone: StatusTone; children: ReactNode; className?: string }) {
  return (
    <span
      data-signal={tone === "danger" ? "failed" : undefined}
      className={cn(
        "flex min-w-0 items-center gap-1.5 font-mono text-2xs",
        tone === "ok" ? "text-fg" : tone === "danger" ? "text-danger" : tone === "faint" ? "text-faint" : "text-muted",
        className,
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "size-[7px] shrink-0",
          tone === "ok" ? "bg-fg" : tone === "danger" ? "bg-danger" : "shadow-[inset_0_0_0_1.25px_currentColor]",
        )}
      />
      <span className="truncate">{children}</span>
    </span>
  );
}
