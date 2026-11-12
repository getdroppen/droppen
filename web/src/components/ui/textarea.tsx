import { cn } from "@/lib/cn";

/** A multi-line field: the same frame and type as `Input`, sized by its rows. */
export function Textarea({ className, ...props }: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cn(
        "w-full resize-y border border-border bg-bg px-3 py-2 text-[13px] leading-[18px] text-fg",
        "placeholder:text-faint hover:border-border-strong focus:border-fg max-md:text-base",
        className,
      )}
      {...props}
    />
  );
}
