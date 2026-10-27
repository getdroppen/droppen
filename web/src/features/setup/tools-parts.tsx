/*
 * The small pieces step 01's cards are built of: the status chip, a command
 * shown verbatim with its copy button, the method picker, the embedded
 * terminal of a running task, and the last lines of a failed one.
 */
import { Check, Copy, SquareTerminal } from "lucide-react";
import { type KeyboardEvent, type ReactNode, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { LiveDot } from "@/components/live-dot";
import { Button } from "@/components/ui/button";
import { XtermPane } from "@/features/terminals/lazy-xterm-pane";
import { useCopy } from "@/lib/clipboard";
import { cn } from "@/lib/cn";
import { elapsed } from "./tools-model";

/** What a status chip says with its colour: only the state, never decoration. */
export type ChipTone = "ready" | "running" | "failed" | "warn" | "neutral";

export function StatusChip({ tone, children }: { tone: ChipTone; children: ReactNode }) {
  return (
    <span
      role="status"
      data-tone={tone}
      className={cn(
        "inline-flex h-4 shrink-0 items-center gap-1 px-1.5 font-mono text-[10px] font-medium whitespace-nowrap",
        tone === "ready" && "text-fg shadow-[inset_0_0_0_1px_var(--color-fg)]",
        tone === "running" && "text-ok shadow-[inset_0_0_0_1px_var(--color-ok)]",
        tone === "failed" && "text-danger shadow-[inset_0_0_0_1px_var(--color-danger)]",
        tone === "warn" && "text-warn-fg shadow-[inset_0_0_0_1px_var(--color-warn-fg)]",
        tone === "neutral" && "text-muted shadow-[inset_0_0_0_1px_var(--color-border-strong)]",
      )}
    >
      {tone === "ready" && <Check className="size-2.5" aria-hidden="true" />}
      {/* Blinks once a second; steady under reduced motion (LiveDot). */}
      {tone === "running" && <LiveDot className="size-1.5 bg-ok" />}
      {children}
    </span>
  );
}

/** A command, exactly as it will run, with a button that copies it. */
export function CommandWell({ command, testId }: { command: string; testId?: string }) {
  const { t } = useTranslation();
  const { copied, copy } = useCopy();
  return (
    <div data-testid={testId} className="flex min-h-9 items-center gap-2.5 border border-border bg-raised py-1.5 pr-1.5 pl-3 font-mono text-xs">
      <span aria-hidden="true" className="text-faint select-none">
        $
      </span>
      <code data-command="" className="min-w-0 flex-1 break-all text-fg">
        {command}
      </code>
      <button
        type="button"
        aria-label={t("Copy command")}
        title={copied ? t("Copied") : t("Copy command")}
        onClick={() => copy(command)}
        className="flex size-6 shrink-0 items-center justify-center text-muted hover:bg-hover hover:text-fg"
      >
        {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
      </button>
    </div>
  );
}

/** "Copy command" as a button of a card's action row. */
export function CopyCommandButton({ command, primary = false, disabled }: { command: string; primary?: boolean; disabled?: boolean }) {
  const { t } = useTranslation();
  const { copied, copy } = useCopy();
  return (
    <Button
      variant={primary ? "primary" : "secondary"}
      data-guide-primary={primary ? "" : undefined}
      disabled={disabled}
      onClick={() => copy(command)}
    >
      {copied && <Check className="size-3.5" aria-hidden="true" />}
      {copied ? t("Copied") : t("Copy command")}
    </Button>
  );
}

/** A row of radios drawn as one segmented control; the arrow keys move the choice. */
export function Segmented<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: { value: T; label: ReactNode }[]; onChange: (value: T) => void }) {
  const group = useRef<HTMLDivElement>(null);
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
    if (!step) return;
    event.preventDefault();
    const at = options.findIndex((o) => o.value === value);
    const next = options[(at + step + options.length) % options.length]!;
    onChange(next.value);
    group.current?.querySelector<HTMLButtonElement>(`[data-value="${next.value}"]`)?.focus();
  };
  return (
    <div ref={group} role="radiogroup" aria-label={label} onKeyDown={onKeyDown} className="inline-flex border border-border-strong">
      {options.map((option) => {
        const on = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={on ? 0 : -1}
            data-value={option.value}
            onClick={() => onChange(option.value)}
            className={cn("h-6 border-l border-border-strong px-2.5 font-mono text-2xs whitespace-nowrap first:border-l-0", on ? "bg-fg text-bg" : "text-muted hover:bg-hover hover:text-fg")}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}

/** The running time of a task, ticking once a second. */
function Elapsed({ since }: { since: string }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return <span className="font-mono text-2xs text-[#a4abb5] tabular-nums">{elapsed(since, now)}</span>;
}

/**
 * A running task's terminal, embedded (dark in both themes, like every
 * terminal here): its header with the time it has run and Stop, then the
 * shim-hosted terminal itself, which a reopened window attaches to again.
 * The focus never moves in by itself; a click or F6 takes it there.
 */
export function TaskTerminal({ terminalId, startedAt, onStop, stopLabel }: { terminalId: string; startedAt: string; onStop: (() => void) | null; stopLabel?: string }) {
  const { t } = useTranslation();
  return (
    <div role="region" aria-label={t("Terminal · on this device")} data-setup-terminal={terminalId} className="mt-2.5 border border-[#2b313a] bg-[#0f1115]">
      <div className="flex h-7 items-center gap-2 border-b border-[#2b313a] pr-1 pl-2.5 font-mono text-2xs text-[#a4abb5]">
        <SquareTerminal className="size-3" aria-hidden="true" />
        <span className="min-w-0 flex-1 truncate">{t("Terminal · on this device")}</span>
        <Elapsed since={startedAt} />
        {onStop && (
          <button type="button" data-testid="task-stop" onClick={onStop} className="h-5 border border-[#5c6470] px-2 text-[#e9ebee] hover:bg-[#1e222a]">
            {stopLabel ?? t("Stop")}
          </button>
        )}
      </div>
      <div className="h-[220px] max-[960px]:h-[160px]">
        <XtermPane id={terminalId} visible />
      </div>
    </div>
  );
}

/** The last lines a failed task printed (the server keeps them, redacted). */
export function TailBlock({ lines }: { lines: string[] }) {
  const { t } = useTranslation();
  if (lines.length === 0) return null;
  return (
    <pre
      aria-label={t("Terminal · on this device")}
      data-testid="task-tail"
      className="mt-2.5 max-h-40 overflow-auto border border-[#2b313a] bg-[#0f1115] px-2.5 py-2 font-mono text-2xs leading-4 whitespace-pre-wrap text-[#e9ebee]"
    >
      {lines.join("\n")}
    </pre>
  );
}
