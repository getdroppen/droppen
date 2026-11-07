import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { XIcon } from "@phosphor-icons/react";
import { cn } from "@/lib/cn";

export const Dialog = BaseDialog.Root;
export const DialogTrigger = BaseDialog.Trigger;

export function DialogContent({
  title,
  children,
  className,
  closeLabel,
}: {
  title: string;
  children: React.ReactNode;
  className?: string;
  closeLabel?: string;
}) {
  return (
    <BaseDialog.Portal>
      <BaseDialog.Backdrop className="fixed inset-0 z-40 bg-black/20 dark:bg-black/60" />
      <BaseDialog.Popup
        className={cn(
          "fixed top-1/2 left-1/2 z-50 max-h-[calc(100dvh-2rem)] w-[min(26rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 overflow-y-auto border border-border-strong bg-bg p-3 outline-none",
          className,
        )}
      >
        <div className="-mx-3 -mt-3 mb-3 flex min-h-9 items-center justify-between gap-2 border-b border-border px-3">
          <BaseDialog.Title className="min-w-0 py-1.5 text-[13px] leading-5 font-semibold break-words text-fg">
            {title}
          </BaseDialog.Title>
          <BaseDialog.Close aria-label={closeLabel ?? "close"} className="flex size-5 shrink-0 items-center justify-center text-muted hover:bg-hover hover:text-fg">
            <XIcon className="size-3.5" />
          </BaseDialog.Close>
        </div>
        {children}
      </BaseDialog.Popup>
    </BaseDialog.Portal>
  );
}
