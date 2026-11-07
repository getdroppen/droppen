import { CircleNotchIcon, WarningIcon } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { restChipText, restTitle, restTone, type AccountRest } from "@/features/accounts/account-rest";
import { cn } from "@/lib/cn";

/**
 * One active rest, as a chip: class-coloured, the end time in words, the
 * server's reason on hover. `onClear` adds the small clear action the settings
 * page offers; the session header shows the same chip without it.
 */
export function RestChip({
  rest,
  warning = false,
  title,
  onClear,
  clearing = false,
  className,
}: {
  rest: AccountRest;
  /** Lead with a warning glyph, for places where the chip is a marker. */
  warning?: boolean;
  /** Replaces the default hover text (reason, session, observed time). */
  title?: string;
  onClear?: () => void;
  clearing?: boolean;
  className?: string;
}) {
  const { t } = useTranslation();
  return (
    <span
      className={cn(
        "inline-flex max-w-full min-w-0 items-center gap-1 rounded px-1.5 py-px font-mono text-2xs leading-4 whitespace-nowrap",
        restTone(rest.class),
        className,
      )}
      title={title ?? restTitle(rest, t)}
    >
      {warning && <WarningIcon className="size-3 shrink-0" />}
      <span className="min-w-0 truncate">{restChipText(rest, t)}</span>
      {onClear && (
        <button
          type="button"
          disabled={clearing}
          onClick={onClear}
          title={t("clear this rest so the account is used again")}
          className="-mr-1 shrink-0 rounded px-1 text-muted hover:bg-hover hover:text-fg disabled:opacity-40"
        >
          {clearing ? <CircleNotchIcon className="inline size-3 animate-spin align-[-2px]" /> : t("clear")}
        </button>
      )}
    </span>
  );
}
