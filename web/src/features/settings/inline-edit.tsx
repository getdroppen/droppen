import { useState } from "react";
import { Input } from "@/components/ui/input";
import { isImeKeydown } from "@/lib/ime";

/**
 * A name that turns into an input when clicked: Enter commits, Escape reverts,
 * a blur commits. Accounts and scheduled tasks both rename inline, and both
 * have to agree on what a key does. The input stays open if the save fails, so
 * the name can be corrected without typing it again.
 */
export function InlineEdit({
  value,
  title,
  onCommit,
  className,
}: {
  value: string;
  title: string;
  onCommit: (next: string) => Promise<unknown> | void;
  className?: string;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const commit = async () => {
    const next = (draft ?? "").trim();
    if (next === "" || next === value) {
      setDraft(null);
      return;
    }
    setSaving(true);
    try {
      await onCommit(next);
      setDraft(null);
    } catch {
      // The caller owns the error; the input stays open for a retry.
    } finally {
      setSaving(false);
    }
  };

  if (draft === null) {
    return (
      <button
        type="button"
        title={title}
        onClick={() => setDraft(value)}
        className={className ?? "max-w-40 truncate rounded px-1 text-left font-mono text-xs text-fg hover:bg-hover"}
      >
        {value}
      </button>
    );
  }
  return (
    <Input
      autoFocus
      value={draft}
      disabled={saving}
      aria-label={title}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => void commit()}
      onKeyDown={(e) => {
        if (isImeKeydown(e)) return;
        if (e.key === "Enter") {
          e.preventDefault();
          void commit();
        } else if (e.key === "Escape") {
          e.preventDefault();
          setDraft(null);
        }
      }}
      className="w-40"
    />
  );
}
