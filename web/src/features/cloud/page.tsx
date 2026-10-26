/*
 * The cloud screens' page and its parts (`ui.tsx` has the rest): the top bar
 * and the framed column, the narrow column of a step, its panel and notes.
 * Only the look, with nothing of the hub client, so a page that shows nothing
 * else (the local `/link/return`, `features/link/link-return.tsx`) loads this
 * and not the hub client `ui.tsx` reads errors with.
 */
import type { ReactNode } from "react";
import { BrandMark } from "@/components/brand-mark";
import { CapsLabel } from "@/components/ui/caps-label";
import { RegMark } from "@/components/reg-mark";
import { cn } from "@/lib/cn";

function CloudColumn({ children }: { children: ReactNode }) {
  return <div className="mx-auto w-full max-w-[1200px] md:border-x md:border-border">{children}</div>;
}

/**
 * A whole cloud page: the top bar with the mark, then the framed column. The
 * registration marks sit where the bar's bottom line meets the frame.
 *
 * The bar sticks inside the same scroller as the page, so the scrollbar's
 * gutters move both by the same amount and their frame lines stay one line.
 * Past 1280px the column stops at about 1200px instead of spreading over the
 * whole window (docs/ui-spec.md §3).
 */
export function CloudPage({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
  return (
    <div className="flex h-full w-full flex-col overflow-y-auto bg-bg [scrollbar-gutter:stable_both-edges]">
      <header className="sticky top-0 z-10 h-14 shrink-0 border-b border-border bg-bg xl:px-8">
        <div className="relative mx-auto flex h-full max-w-[1200px] items-center gap-3 px-6 max-md:px-4 md:border-x md:border-border">
          <BrandMark size={20} title="Droppen" />
          <span className="font-mono text-sm font-medium text-fg">Droppen</span>
          <span className="flex-1" />
          {aside}
          <RegMark className="-bottom-[5px] -left-[5px] md:hidden xl:block" />
          <RegMark className="-right-[5px] -bottom-[5px] md:hidden xl:block" />
        </div>
      </header>
      <div className="flex flex-1 flex-col xl:px-8">
        <CloudColumn>{children}</CloudColumn>
      </div>
    </div>
  );
}

/** A page with one narrow column of content: the set-up, recovery and pairing steps. */
export function Centered({ children, aside }: { children: ReactNode; aside?: ReactNode }) {
  return (
    <CloudPage aside={aside}>
      <div className="flex justify-center px-6 py-14 max-md:px-4 max-md:py-8">{children}</div>
    </CloudPage>
  );
}

/**
 * One step's column: a caps label, a display headline, then the content. The
 * headline is the screen's one large type moment (docs/ui-spec.md §1).
 */
export function Panel({ title, label, children, wide }: { title: string; label?: string; children: ReactNode; wide?: boolean }) {
  return (
    <div className={cn("flex w-full flex-col gap-3", wide ? "max-w-xl" : "max-w-[26rem]")}>
      <CapsLabel>{label ?? "Droppen cloud"}</CapsLabel>
      <h1 className="mb-2 text-[28px] leading-[1.1] font-semibold tracking-[-0.03em] text-fg">{title}</h1>
      {children}
    </div>
  );
}

/** A boxed note inside a step: success, a danger to confirm, or a warning. */
export function Note({ tone, children, className }: { tone?: "ok" | "danger" | "warn"; children: ReactNode; className?: string }) {
  return (
    <div
      data-signal={tone}
      className={cn(
        "flex flex-col gap-2 border border-border p-3",
        tone === "ok" && "shadow-[inset_2px_0_0_var(--color-ok)]",
        tone === "danger" && "bg-danger/4 shadow-[inset_2px_0_0_var(--color-danger)]",
        tone === "warn" && "bg-warn/7 shadow-[inset_2px_0_0_var(--color-warn)]",
        className,
      )}
    >
      {children}
    </div>
  );
}
