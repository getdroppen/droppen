import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

type Mark = "block" | "lit" | "dim" | "key" | "payload";

interface LegendKey {
  mark: Mark;
  label: ReactNode;
}

function Swatch({ mark }: { mark: Mark }) {
  const ink = mark === "lit" || mark === "payload" ? "text-fg" : mark === "key" ? "text-ok" : "text-muted";
  return (
    <svg aria-hidden="true" focusable="false" viewBox="0 0 16 10" className={cn("h-2.5 w-4 shrink-0 overflow-visible", ink)}>
      {mark === "block" ? (
        <path d="M2 3.5 8 1 14 3.5 8 6ZM2 3.5v3L8 9l6-2.5v-3M8 6v3" fill="none" stroke="currentColor" strokeWidth={0.9} strokeLinejoin="round" />
      ) : mark === "payload" ? (
        <path d="M8 1.5 13 4 8 6.5 3 4ZM3 4v3l5 2 5-2V4M8 6.5V9" fill="none" stroke="currentColor" strokeWidth={0.9} strokeLinejoin="round" />
      ) : mark === "key" ? (
        <path d="M3.4 5a1.6 1.6 0 1 0 0 .01M5 5h8M11.4 5v2M9 5v1.4" fill="none" stroke="currentColor" strokeWidth={1} strokeLinecap="round" strokeLinejoin="round" />
      ) : (
        <path d="M1.5 5h13" fill="none" stroke="currentColor" strokeWidth={1.2} strokeLinecap="round" />
      )}
    </svg>
  );
}

export function Figure({ title, keys = [], caption, children }: { title: string; keys?: LegendKey[]; caption?: ReactNode; children: ReactNode }) {
  return (
    <figure className="relative mx-6 border border-border bg-bg max-md:mx-4">
      <span className="absolute top-0 left-0 z-1 bg-bg px-2.5 py-1.5 font-mono text-xs tracking-label text-muted uppercase">{title}</span>
      <div className="relative px-4 pt-11 pb-10 max-md:px-2 max-md:pt-10 max-md:pb-9">{children}</div>
      {keys.length ? (
        <div className="flex flex-wrap gap-x-5 gap-y-2 px-4 py-4 font-mono text-xs text-muted">
          {keys.map((key, index) => (
            <span key={index} className="inline-flex items-center gap-2"><Swatch mark={key.mark} />{key.label}</span>
          ))}
        </div>
      ) : null}
      {caption ? <figcaption className="px-4 py-4 text-xs leading-5 text-pretty text-muted">{caption}</figcaption> : null}
    </figure>
  );
}
