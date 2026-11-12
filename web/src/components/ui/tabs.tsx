import { Tabs as BaseTabs } from "@base-ui/react/tabs";
import { cn } from "@/lib/cn";

export const Tabs = BaseTabs.Root;
export const TabsPanel = BaseTabs.Panel;

export function TabsList({ className, ...props }: React.ComponentProps<typeof BaseTabs.List>) {
  return (
    <BaseTabs.List
      className={cn("flex h-8 items-stretch border border-border-strong bg-bg", className)}
      {...props}
    />
  );
}

export function TabsTab({ className, ...props }: React.ComponentProps<typeof BaseTabs.Tab>) {
  return (
    <BaseTabs.Tab
      className={cn(
        "flex flex-1 items-center justify-center border-r border-border px-3 text-[13px] font-medium text-muted last:border-r-0",
        "select-none hover:text-fg",
        "data-[active]:border-fg data-[active]:bg-fg data-[active]:text-bg",
        className,
      )}
      {...props}
    />
  );
}

export function TextTabsList({ className, ...props }: React.ComponentProps<typeof BaseTabs.List>) {
  return (
    <BaseTabs.List
      className={cn("flex h-9 items-stretch gap-5 overflow-x-auto border-b border-border", className)}
      {...props}
    />
  );
}

export function TextTab({ className, ...props }: React.ComponentProps<typeof BaseTabs.Tab>) {
  return (
    <BaseTabs.Tab
      className={cn(
        "flex shrink-0 items-center gap-1.5 whitespace-nowrap px-0.5 text-[13px] font-medium text-muted select-none hover:text-fg",
        "data-[active]:text-fg data-[active]:shadow-[inset_0_-2px_0_0_var(--color-fg)]",
        className,
      )}
      {...props}
    />
  );
}
