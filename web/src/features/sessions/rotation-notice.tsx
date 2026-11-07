import { XIcon } from "@phosphor-icons/react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  countdownTick,
  ROTATION_NOTICE_SECONDS,
  rotationDismissed,
  rotationDismissName,
  rotationIsRecent,
  rotationNotice,
  type SessionRotation,
} from "@/features/accounts/session-rotation";
import { cn } from "@/lib/cn";
import { useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { machineStorageKey, readMachineItem, writeMachineItem } from "@/lib/machines/storage";

function readDismissed(machine: MachineKey, name: string): string | null {
  try {
    return readMachineItem(localStorage, machine, name);
  } catch {
    return null;
  }
}

/**
 * The composer's line about the session's latest automatic account rotation,
 * for a day after it happened. Calm when the conversation carried on under
 * another account — including a move made ahead of a limit, before anything
 * failed; amber when it could not, because then the session is waiting on the
 * old one again.
 *
 * Closing it is remembered per session in this browser, against that rotation
 * only: the next one shows again. It also closes itself: the close button
 * counts down {@link ROTATION_NOTICE_SECONDS} while the page is on screen, and
 * holds (showing ×) while the pointer or focus is on it.
 */
export function RotationNotice({
  source,
  id,
  rotation,
}: {
  source: string;
  id: string;
  rotation: SessionRotation | null | undefined;
}) {
  const { t } = useTranslation();
  const machine = useMachine();
  const name = rotationDismissName(source, id);
  const key = machineStorageKey(machine, name);
  const [state, setState] = useState(() => ({ key, dismissed: readDismissed(machine, name) }));
  // Re-read when the view moves to another session without remounting.
  let dismissed = state.dismissed;
  if (state.key !== key) {
    dismissed = readDismissed(machine, name);
    setState({ key, dismissed });
  }

  const shown = !!rotation && rotationIsRecent(rotation) && !rotationDismissed(dismissed, rotation);
  const notice = shown ? rotationNotice(rotation, t) : null;
  const at = rotation?.at ?? null;

  // Each notice gets its own countdown: a new rotation, or another session's, starts it over.
  const showing = notice ? `${key}|${at}` : null;
  const [count, setCount] = useState({ for: showing, left: ROTATION_NOTICE_SECONDS });
  let left = count.left;
  if (count.for !== showing) {
    left = ROTATION_NOTICE_SECONDS;
    setCount({ for: showing, left });
  }
  const [held, setHeld] = useState(false);

  const dismiss = () => {
    if (!at) return;
    setState({ key, dismissed: at });
    try {
      writeMachineItem(localStorage, machine, name, at);
    } catch {
      // Storage unavailable: it stays closed for this page.
    }
  };

  useEffect(() => {
    if (!showing || held) return;
    const timer = window.setInterval(() => {
      const visible = document.visibilityState === "visible";
      setCount((c) => (c.for === showing ? { ...c, left: countdownTick(c.left, { visible, held: false }) } : c));
    }, 1000);
    return () => window.clearInterval(timer);
  }, [showing, held]);

  const expired = showing !== null && left === 0;
  useEffect(() => {
    if (expired) dismiss();
    // `dismiss` is rebuilt every render; the countdown reaching zero is the only trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [expired]);

  if (!notice) return null;

  return (
    <div
      className={cn(
        "flex items-start gap-1.5 border-b border-border px-3 py-1 font-mono text-2xs leading-4",
        notice.tone === "warn" ? "text-warn-fg" : "text-muted",
      )}
      title={notice.title}
    >
      <span aria-hidden className={cn("shrink-0", notice.tone === "info" && "text-accent")}>
        ↻
      </span>
      <span className="min-w-0 flex-1 break-words">{notice.text}</span>
      <button
        type="button"
        aria-label={t("dismiss")}
        title={held ? t("dismiss") : t("closes by itself in {{n}} s", { n: left })}
        onClick={dismiss}
        onPointerEnter={() => setHeld(true)}
        onPointerLeave={() => setHeld(false)}
        onFocus={() => setHeld(true)}
        onBlur={() => setHeld(false)}
        className="flex h-4 min-w-4 shrink-0 items-center justify-center rounded px-0.5 tabular-nums hover:bg-hover hover:text-fg"
      >
        {held ? <XIcon className="size-3" /> : <span data-countdown>{left}</span>}
      </button>
    </div>
  );
}
