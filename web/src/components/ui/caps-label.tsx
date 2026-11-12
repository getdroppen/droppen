import { cn } from "@/lib/cn";

/** Latin capitals are tracked; Chinese and Japanese are not (ui-spec §1). */
export const CJK_TRACKING = "[&:lang(ja)]:tracking-normal [&:lang(zh)]:tracking-normal";

/**
 * A small capitals label. `label` is the app's `.label` voice: mono, 10–11px, 0.12em
 * (ui-spec §7 V6). `section` is a public page's panel title: sans, 12px, semibold —
 * the same caps vocabulary on the named size scale, not a `.label`.
 */
export function CapsLabel({
  className,
  size = "sm",
  face = "mono",
  variant = "label",
  ...props
}: React.HTMLAttributes<HTMLSpanElement> & {
  size?: "sm" | "xs";
  face?: "mono" | "sans";
  variant?: "label" | "section";
}) {
  if (variant === "section") {
    return <span className={cn("text-xs font-semibold tracking-label text-muted uppercase", CJK_TRACKING, className)} {...props} />;
  }
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
