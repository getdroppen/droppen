import { cn } from "@/lib/cn";

/**
 * A machine label: uppercase, 0.12em tracking, muted (docs/ui-spec.md §1).
 * Section titles, table heads, message roles. `data-label` lets a UI scan
 * check the rule (docs/ui-spec.md V6). Uppercase is a no-op on CJK.
 *
 * `face`: the app's labels are mono; the public pages set theirs in the page's
 * own face, since what they label there are words a visitor reads, not the
 * app's machine vocabulary. Tracking and case are the same either way.
 */
export function CapsLabel({
  className,
  size = "sm",
  face = "mono",
  ...props
}: React.HTMLAttributes<HTMLSpanElement> & { size?: "sm" | "xs"; face?: "mono" | "sans" }) {
  return (
    <span
      data-label=""
      className={cn(
        face === "mono" ? "label-caps" : "uppercase tracking-label",
        "text-muted",
        size === "xs" ? "text-[10px] leading-[14px]" : "text-2xs",
        className,
      )}
      {...props}
    />
  );
}
