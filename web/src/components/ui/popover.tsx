import { Popover as BasePopover } from "@base-ui/react/popover";
import { cn } from "@/lib/cn";

export const Popover = BasePopover.Root;
export const PopoverTrigger = BasePopover.Trigger;

export function PopoverContent({
  side = "top",
  className,
  children,
}: {
  side?: "top" | "bottom" | "left" | "right";
  className?: string;
  children: React.ReactNode;
}) {
  return (
    <BasePopover.Portal>
      <BasePopover.Positioner side={side} sideOffset={4} className="z-50 outline-none">
        <BasePopover.Popup
          className={cn("popup-motion border border-border-strong bg-bg p-3 outline-none", className)}
        >
          {children}
        </BasePopover.Popup>
      </BasePopover.Positioner>
    </BasePopover.Portal>
  );
}
