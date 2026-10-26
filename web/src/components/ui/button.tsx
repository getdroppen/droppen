import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/cn";

/**
 * The four buttons of the drafting sheet: primary is solid ink (one per view or
 * dialog), secondary is a `border-strong` outline, ghost has no frame, danger is
 * red words in a red outline. Square, no shadow. Heights follow docs/ui-spec.md:
 * 28px by default, 24px inside a 32px header, 36px on cloud pages.
 */
const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-1.5 border font-medium whitespace-nowrap select-none disabled:pointer-events-none disabled:opacity-40",
  {
    variants: {
      variant: {
        primary: "border-fg bg-fg text-bg hover:bg-fg/85",
        secondary: "border-border-strong bg-bg text-fg hover:bg-hover",
        ghost: "border-transparent text-muted hover:bg-hover hover:text-fg",
        danger: "border-danger/55 bg-bg text-danger hover:bg-danger/8",
      },
      size: {
        sm: "h-6 px-2 text-2xs [&_svg]:size-3",
        md: "h-7 px-3 text-xs [&_svg]:size-3.5",
        lg: "h-9 px-4 text-sm [&_svg]:size-4",
      },
    },
    defaultVariants: { variant: "secondary", size: "md" },
  },
);

export function Button({
  className,
  variant,
  size,
  type = "button",
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & VariantProps<typeof buttonVariants>) {
  return (
    <button
      type={type}
      data-variant={variant ?? "secondary"}
      className={cn(buttonVariants({ variant, size }), className)}
      {...props}
    />
  );
}
