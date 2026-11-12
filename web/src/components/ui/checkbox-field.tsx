import { cn } from "@/lib/cn";
import { Checkbox } from "./checkbox";

/**
 * shadcn's `Checkbox` is only the box. This adds the app's label row and the
 * few classes the drafting-sheet grid needs (14px, square, ink border); the
 * box itself stays the generated component.
 */
export function CheckboxField({
  label,
  className,
  labelClassName,
  title,
  ...props
}: Omit<React.ComponentProps<typeof Checkbox>, "className"> & {
  label?: React.ReactNode;
  /** The label row (defaults to the app's 11px muted voice). */
  className?: string;
  /** The label text after the box. */
  labelClassName?: string;
  title?: string;
}) {
  return (
    <label
      className={cn("flex cursor-pointer items-start gap-2 text-2xs text-muted", className)}
      title={title}
    >
      <Checkbox
        data-audit-checkbox=""
        className="mt-0.5 size-3.5 rounded-none border-border-strong bg-bg focus-visible:outline-solid focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-fg focus-visible:ring-0 pointer-coarse:size-4"
        {...props}
      />
      {label != null && <span className={cn("min-w-0", labelClassName)}>{label}</span>}
    </label>
  );
}
