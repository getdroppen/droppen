import { cn } from "@/lib/cn";

export function Switch({
  on,
  blocked = false,
  title,
  label,
  onChange,
  testId,
  labelHidden = false,
  className,
}: {
  on: boolean;
  blocked?: boolean;
  title?: string;
  label: string;
  onChange: (on: boolean) => void;
  testId?: string;
  labelHidden?: boolean;
  className?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-disabled={blocked || undefined}
      aria-label={label}
      title={title}
      data-testid={testId}
      data-audit-switch=""
      onClick={() => {
        if (!blocked) onChange(!on);
      }}
      className={cn(
        "flex h-7 shrink-0 items-center gap-2 px-1 font-mono text-2xs pointer-coarse:h-9",
        blocked ? "cursor-not-allowed text-faint" : "text-muted hover:text-fg",
        className,
      )}
    >
      <span
        aria-hidden="true"
        data-audit-switch-track=""
        className={cn(
          "relative inline-flex h-3.5 w-6 shrink-0 border",
          on ? "border-fg bg-fg" : "border-border-strong bg-bg",
          blocked && "opacity-40",
        )}
      >
        <span className={cn("absolute top-[2px] size-2", on ? "left-[12px] bg-bg" : "left-[2px] bg-faint")} />
      </span>
      <span className={labelHidden ? "sr-only" : "max-sm:sr-only"}>{label}</span>
    </button>
  );
}
