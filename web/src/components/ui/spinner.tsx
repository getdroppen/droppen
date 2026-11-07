import { CircleNotchIcon } from "@phosphor-icons/react";
import { cn } from "@/lib/cn";

export function Spinner({ className }: { className?: string }) {
  return (
    <div className={cn("flex items-center justify-center p-6 text-faint", className)}>
      <CircleNotchIcon className="size-4 animate-spin" />
    </div>
  );
}
