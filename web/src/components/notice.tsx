import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * A line over a composer that something is in the way, with its state edge:
 * red for what stops the send, amber for what it should know (`local`: the
 * local login is signed out).
 *
 * Shared by the surfaces that start a conversation — the new-session view and
 * the instant-chat home — so their warnings read the same.
 */
export function Notice({ signal, local = false, children }: { signal: "offline" | "failed" | "warn"; local?: boolean; children: ReactNode }) {
  return (
    <div
      data-signal={signal}
      data-local-signed-out={local ? "" : undefined}
      className={cn(
        "mb-1.5 px-2.5 py-1 font-mono text-2xs whitespace-pre-wrap",
        signal === "warn" ? "text-warn-fg shadow-[inset_2px_0_0_var(--color-warn)]" : "text-danger shadow-[inset_2px_0_0_var(--color-danger)]",
      )}
    >
      {children}
    </div>
  );
}
