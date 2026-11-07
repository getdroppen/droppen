import { SquaresFourIcon } from "@phosphor-icons/react";
import { useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useScreens, type ScreenInfo } from "@/app/screens";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { isImeKeydown } from "@/lib/ime";
import { isScreenEmoji, SCREEN_EMOJIS } from "./screen-emoji";

export function ScreenEmojiDialog({ screen, onClose }: { screen: ScreenInfo; onClose: () => void }) {
  const { t } = useTranslation();
  const { setScreenEmoji } = useScreens();
  const inputId = useId();
  const [value, setValue] = useState(screen.emoji ?? "");
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const pending = useRef(false);

  const save = async (input: string) => {
    if (pending.current) return;
    const emoji = input.trim();
    if (emoji && !isScreenEmoji(emoji)) {
      setError(t("enter a single emoji"));
      return;
    }
    pending.current = true;
    setSaving(true);
    setError(null);
    try {
      await setScreenEmoji(screen.id, emoji);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      pending.current = false;
      setSaving(false);
    }
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent title={t("custom emoji")} className="max-w-xs">
        <div className="flex flex-col gap-3">
          <p className="truncate text-xs text-muted" title={screen.title}>{screen.title}</p>
          <div className="grid grid-cols-8 gap-1" role="group" aria-label={t("suggested emoji")}>
            {SCREEN_EMOJIS.map((emoji) => (
              <button
                key={emoji}
                type="button"
                aria-label={emoji}
                aria-pressed={screen.emoji === emoji}
                disabled={saving}
                onClick={() => void save(emoji)}
                className="flex h-8 items-center justify-center rounded text-xl leading-none hover:bg-hover focus-visible:outline focus-visible:outline-accent aria-pressed:bg-accent/15 disabled:opacity-40"
              >
                {emoji}
              </button>
            ))}
          </div>
          <div className="flex flex-col gap-1.5">
            <label htmlFor={inputId} className="text-xs text-muted">{t("or type or paste an emoji")}</label>
            <input
              id={inputId}
              value={value}
              disabled={saving}
              placeholder="👩🏽‍💻"
              aria-invalid={!!error}
              aria-describedby={error ? `${inputId}-error` : undefined}
              onChange={(e) => { setValue(e.target.value); setError(null); }}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !isImeKeydown(e)) {
                  e.preventDefault();
                  void save(value);
                }
              }}
              className="h-9 w-full rounded border border-border bg-bg px-2 text-xl text-fg placeholder:text-faint focus:border-fg focus:outline-none"
            />
            {error && <p id={`${inputId}-error`} role="alert" className="text-xs text-danger">{error}</p>}
          </div>
          <div className="flex items-center justify-between gap-2">
            <button
              type="button"
              disabled={saving}
              onClick={() => void save("")}
              className="flex h-7 items-center gap-1.5 rounded px-2 text-xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40"
            >
              <SquaresFourIcon className="size-3.5" />
              {t("restore default icon")}
            </button>
            <button
              type="button"
              disabled={saving}
              onClick={() => void save(value)}
              data-variant="primary" className="flex h-7 items-center bg-fg px-3 text-xs text-bg hover:bg-fg/85 disabled:opacity-40"
            >
              {saving ? t("saving…") : t("save")}
            </button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
