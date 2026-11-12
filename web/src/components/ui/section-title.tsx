import { cn } from "@/lib/cn";

/**
 * A document section's title strip: full-bleed hairline, uppercase mono, on the
 * same 12px left edge as the page title. Settings and the update page share it
 * so the two documents cannot drift apart.
 */
export function SectionTitle({ className, children, ...props }: React.ComponentProps<"h2">) {
  return (
    <h2
      data-label=""
      className={cn(
        "label-caps -mx-3 -mt-3 mb-2 flex h-9 items-center border-b border-border px-3 text-2xs text-fg",
        className,
      )}
      {...props}
    >
      {children}
    </h2>
  );
}
