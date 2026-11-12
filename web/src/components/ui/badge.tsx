import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "@/lib/cn";

const badgeVariants = cva(
  "inline-flex items-center px-1 font-mono text-[10px] leading-[14px] whitespace-nowrap shadow-[inset_0_0_0_1px_var(--color-border)]",
  {
    variants: {
      variant: {
        default: "text-muted",
        claude: "tracking-[0.08em] text-muted uppercase",
        codex: "tracking-[0.08em] text-muted uppercase",
        outline: "text-fg shadow-[inset_0_0_0_1px_var(--color-border-strong)]",
      },
    },
    defaultVariants: { variant: "default" },
  },
);

export function Badge({
  className,
  variant,
  ...props
}: React.HTMLAttributes<HTMLSpanElement> & VariantProps<typeof badgeVariants>) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}
