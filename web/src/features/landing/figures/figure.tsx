import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { gridPaper } from "./paper";

/*
 * A landing figure's frame: a title, the drawing, its live state and only the
 * legend entries needed to read the visual encoding.
 */

type Mark = "raised" | "flat" | "lit" | "dim" | "dashed" | "dotted" | "key";

export interface LegendKey {
  mark: Mark;
  label: ReactNode;
}

function Swatch({ mark }: { mark: Mark }) {
  return (
    <svg aria-hidden="true" focusable="false" viewBox="0 0 16 10" className={cn("h-2.5 w-4 shrink-0 overflow-visible", mark === "lit" ? "text-fg" : mark === "key" ? "text-ok" : "text-muted")}>
      {mark === "raised" || mark === "flat" ? (
        <path
          d={mark === "flat" ? "M2 6.5 8 3.5 14 6.5 8 9.5Z" : "M2 4.5 8 1.5 14 4.5 14 6 8 9 2 6Z"}
          fill="none"
          stroke="currentColor"
          strokeWidth={1}
          strokeLinejoin="round"
        />
      ) : (
        <path
          d="M1.5 5h13"
          fill="none"
          stroke="currentColor"
          strokeWidth={mark === "dotted" ? 1.4 : 1.2}
          strokeLinecap="round"
          strokeDasharray={mark === "dotted" ? "0.01 3" : mark === "dashed" ? "3 2.5" : undefined}
        />
      )}
    </svg>
  );
}

export function Figure({
  title,
  readout,
  keys = [],
  caption,
  children,
}: {
  title: string;
  readout: string;
  keys?: LegendKey[];
  caption?: ReactNode;
  children: ReactNode;
}) {
  return (
    <figure className="relative mx-6 border border-border bg-bg max-md:mx-4" style={gridPaper()}>
      <span className="absolute top-0 left-0 z-1 bg-bg px-2.5 py-1.5 font-mono text-xs tracking-label text-muted uppercase">
        {title}
      </span>
      <div className="relative px-4 pt-11 pb-10 max-md:px-2 max-md:pt-10 max-md:pb-9">
        {children}
        <span className="pointer-events-none absolute right-3 bottom-2.5 font-mono text-xs tabular-nums text-fg">{readout}</span>
      </div>
      {keys.length ? (
        <div className="flex flex-wrap gap-x-5 gap-y-2 px-4 py-4 font-mono text-xs text-muted">
          {keys.map((key, index) => (
            <span key={index} className="inline-flex items-center gap-2">
              <Swatch mark={key.mark} />
              {key.label}
            </span>
          ))}
        </div>
      ) : null}
      {caption ? <figcaption className="px-4 py-4 text-xs leading-5 text-pretty text-muted">{caption}</figcaption> : null}
    </figure>
  );
}
