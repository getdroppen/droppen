import { CheckIcon } from "@phosphor-icons/react";
import { cn } from "@/lib/cn";

export const CHECKBOX_LABEL = "flex cursor-pointer items-start gap-2 text-2xs text-muted";
export const CHECKBOX_INPUT = "peer sr-only";
export const CHECKBOX_BOX = cn(
  "mt-0.5 flex size-3.5 shrink-0 items-center justify-center border border-border-strong bg-bg text-bg",
  "peer-checked:border-fg peer-checked:bg-fg peer-checked:[&>svg]:opacity-100",
  "peer-focus-visible:outline peer-focus-visible:outline-2 peer-focus-visible:outline-offset-1 peer-focus-visible:outline-fg",
  "peer-disabled:border-border pointer-coarse:size-4",
);

export function CheckboxMark({ className }: { className?: string }) {
  return (
    <span aria-hidden="true" data-audit-checkbox="" className={cn(CHECKBOX_BOX, className)}>
      <CheckIcon className="size-2.5 opacity-0" />
    </span>
  );
}
