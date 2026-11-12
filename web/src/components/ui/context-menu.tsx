import { ContextMenu as BaseContextMenu } from "@base-ui/react/context-menu";
import { cn } from "@/lib/cn";

export const ContextMenu = BaseContextMenu.Root;
export const ContextMenuTrigger = BaseContextMenu.Trigger;

export function ContextMenuContent({ children }: { children: React.ReactNode }) {
  return (
    <BaseContextMenu.Portal>
      <BaseContextMenu.Positioner className="z-50 outline-none">
        <BaseContextMenu.Popup className="min-w-44 border border-border-strong bg-bg py-1 outline-none">
          {children}
        </BaseContextMenu.Popup>
      </BaseContextMenu.Positioner>
    </BaseContextMenu.Portal>
  );
}

export function ContextMenuItem({
  danger = false,
  className,
  ...props
}: React.ComponentProps<typeof BaseContextMenu.Item> & { danger?: boolean }) {
  return (
    <BaseContextMenu.Item
      className={cn(
        "flex h-7 w-full cursor-pointer items-center gap-2 px-2.5 text-left text-xs outline-none",
        "data-[highlighted]:bg-hover",
        danger ? "text-danger" : "text-fg",
        className,
      )}
      {...props}
    />
  );
}
