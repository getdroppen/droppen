import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy, Loader2 } from "lucide-react";
import { isDesktopApp } from "@/lib/auth";
import type { Attempt, LinkOpener } from "./opener";

/*
 * Signing this device in to Droppen through the system browser
 * (docs/local-aggregator.md §4.9, the browser hand-off), as pieces any screen
 * can use — Settings → Access → Droppen, the first-run card, and the desktop app's
 * first-run guide (「在手机和其它设备上也能用 → 登录 Droppen」,
 * `features/setup/where-sign-in.tsx`):
 *
 * - {@link startDroppenSignIn}: start it the way this window can, from the
 *   click itself — the system browser inside the Droppen desktop app (its
 *   window has no popups: a new window goes to the browser), the droppen.app
 *   popup elsewhere;
 * - {@link BrowserSignInWaiting}: "continue in your browser" — the three
 *   steps, this device's fingerprint (droppen.app shows the same), how long
 *   the link still works, and open again / copy the link / cancel.
 *
 * The rest — that droppen.app answered, that the hub took the enrollment,
 * that this device is in the account — the opener follows at the local
 * server, and the Droppen app brings its window to the front once it is in.
 */

/** Whether this window signs in through the system browser rather than a popup. */
export function signsInThroughBrowser(): boolean {
  return isDesktopApp();
}

/** Start signing this device in, the way this window can. Call from the click itself. */
export function startDroppenSignIn(opener: LinkOpener, hubWeb: string, opts: { hub?: string } = {}): void {
  if (signsInThroughBrowser()) opener.openInBrowser({ hub: opts.hub, aggregate: true });
  else opener.open(hubWeb, { hub: opts.hub, aggregate: true });
}

/** `ms` as `m:ss`. */
export function formatLeft(ms: number): string {
  const seconds = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

function useNow(everyMs: number, initial?: number): number {
  const [now, setNow] = useState(() => initial ?? Date.now());
  useEffect(() => {
    if (initial !== undefined) return;
    const timer = setInterval(() => setNow(Date.now()), everyMs);
    return () => clearInterval(timer);
  }, [everyMs, initial]);
  return now;
}

const primary = "flex h-7 items-center justify-center bg-fg px-3 text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40";
const quiet =
  "flex h-7 shrink-0 items-center justify-center gap-1.5 rounded border border-border px-3 font-mono text-2xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40";

/** Copy `text`, saying so for a moment. */
function CopyLink({ text }: { text: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className={quiet}
      data-testid="droppen-copy-link"
      onClick={() => {
        void navigator.clipboard?.writeText(text).then(() => {
          setCopied(true);
          setTimeout(() => setCopied(false), 1500);
        });
      }}
    >
      {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
      {copied ? t("copied") : t("copy link")}
    </button>
  );
}

/**
 * "Continue in your browser": what the person does at droppen.app, this
 * device's fingerprint to compare, how long the link still works, and a way to
 * open it again, copy it or give up. `now` fixes the clock (render tests);
 * `reopenPrimary: false` where the page has a primary of its own (the
 * first-run guide's action bar).
 */
export function BrowserSignInWaiting({
  attempt,
  onReopen,
  onCancel,
  now,
  reopenPrimary = true,
}: {
  attempt: Attempt;
  onReopen: () => void;
  onCancel: () => void;
  now?: number;
  reopenPrimary?: boolean;
}) {
  const { t } = useTranslation();
  const clock = useNow(1000, now);
  const left = attempt.expiresMs - clock;
  return (
    <div className="flex flex-col gap-3" data-testid="droppen-browser-waiting">
      <p className="text-xs font-medium text-fg">{t("Continue in your browser")}</p>
      <ol className="flex list-decimal flex-col gap-1 pl-5 text-xs leading-5 text-fg">
        <li>{t("Sign in to droppen.app, or create an account.")}</li>
        <li>{t("Check that the fingerprint there matches this one, then add this device.")}</li>
        <li>{t("Come back here: this device joins your account by itself.")}</li>
      </ol>
      <div className="flex flex-col gap-1">
        <span className="text-2xs text-faint">{t("this device's fingerprint — droppen.app shows the same")}</span>
        <span data-testid="machine-fingerprint" className="font-mono text-sm tracking-wide text-fg">
          {attempt.machine.fingerprint}
        </span>
      </div>
      {/* Ticks every second: kept out of any surrounding live region, so a screen reader is not read the panel again each second. */}
      <p className="flex items-center gap-2 text-2xs text-faint" data-testid="droppen-browser-left" aria-live="off">
        <Loader2 className="size-3 animate-spin" />
        {left > 0 ? t("waiting for droppen.app… the link works for another {{left}}", { left: formatLeft(left) }) : t("waiting for droppen.app…")}
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          data-variant={reopenPrimary ? "primary" : undefined}
          className={reopenPrimary ? primary : quiet}
          data-testid="droppen-reopen-browser"
          onClick={onReopen}
        >
          {t("open the browser again")}
        </button>
        {attempt.handoffUrl && <CopyLink text={attempt.handoffUrl} />}
        <button type="button" className={quiet} data-testid="droppen-cancel" onClick={onCancel}>
          {t("cancel")}
        </button>
      </div>
      <p className="text-2xs leading-4 text-faint">{t("The wrong browser opened? Copy the link into the one you use droppen.app in, on this device.")}</p>
      {/* The link is the attempt's credential until it expires: whoever opens it can add this device to their own account. */}
      <p className="text-2xs leading-4 text-danger" data-testid="droppen-link-private">
        {t("Whoever opens this link can add this device to their own Droppen account: never send it to anyone, and open it only on this device.")}
      </p>
    </div>
  );
}
