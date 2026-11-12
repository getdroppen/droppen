import { Dialog as SheetPrimitive } from "@base-ui/react/dialog";
import { XIcon } from "@phosphor-icons/react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";

function Sheet({ ...props }: SheetPrimitive.Root.Props) {
  return <SheetPrimitive.Root data-slot="sheet" {...props} />;
}

function SheetTrigger({ ...props }: SheetPrimitive.Trigger.Props) {
  return <SheetPrimitive.Trigger data-slot="sheet-trigger" {...props} />;
}

function SheetClose({ ...props }: SheetPrimitive.Close.Props) {
  return <SheetPrimitive.Close data-slot="sheet-close" {...props} />;
}

function SheetContent({
  className,
  children,
  showCloseButton = true,
  ...props
}: SheetPrimitive.Popup.Props & {
  showCloseButton?: boolean;
}) {
  return (
    <SheetPrimitive.Portal>
      <SheetPrimitive.Backdrop className="fixed inset-0 z-40 bg-black/20 transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0 dark:bg-black/60" />
      <SheetPrimitive.Popup
        data-slot="sheet-content"
        className={cn(
          "fixed inset-x-0 bottom-0 z-50 flex max-h-[calc(100dvh-2rem)] flex-col gap-3 overflow-y-auto border-t border-border-strong bg-bg text-fg outline-none transition duration-150 ease-out data-ending-style:translate-y-2 data-ending-style:opacity-0 data-starting-style:translate-y-2 data-starting-style:opacity-0",
          className,
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <SheetPrimitive.Close
            data-slot="sheet-close"
            render={<Button variant="ghost" className="absolute top-3 right-3 size-5 p-0" size="md" />}
          >
            <XIcon className="size-3.5" />
            <span className="sr-only">Close</span>
          </SheetPrimitive.Close>
        )}
      </SheetPrimitive.Popup>
    </SheetPrimitive.Portal>
  );
}

function SheetTitle({ className, ...props }: SheetPrimitive.Title.Props) {
  return (
    <SheetPrimitive.Title
      data-slot="sheet-title"
      className={cn("text-[13px] leading-5 font-semibold text-fg", className)}
      {...props}
    />
  );
}

export { Sheet, SheetTrigger, SheetClose, SheetContent, SheetTitle };
