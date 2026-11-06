import { cn } from "@/lib/cn";

export function Input({ className, ...props }: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={cn(
        "h-8 w-full border border-border bg-bg px-3 text-[13px] text-fg max-md:h-10",
        "placeholder:text-faint hover:border-border-strong focus:border-fg",
        className,
      )}
      {...props}
    />
  );
}
