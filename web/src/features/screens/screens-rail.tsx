import { Link } from "@tanstack/react-router";
import { SquaresFourIcon } from "@phosphor-icons/react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useFeedSessions } from "@/app/global-feed";
import type { PaneNode } from "@/app/pane-tree";
import { useScreens, type ScreenInfo } from "@/app/screens";
import { LiveDot } from "@/components/live-dot";
import { formatUnread, screenAttention, sessionsInLayout } from "@/features/sessions/session-attention";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { cn } from "@/lib/cn";
import { isImeKeydown } from "@/lib/ime";
import { useMachine } from "@/lib/machines/context";
import { sessionKey } from "@/lib/machines/key";
import { ScreenEmojiDialog } from "./screen-emoji-dialog";

function RenameScreenDialog({
  target,
  onOpenChange,
}: {
  target: ScreenInfo | null;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement>(null);
  const { renameScreen } = useScreens();
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // Focus + select once the dialog opens (after Base UI settles initial focus).
  useEffect(() => {
    if (!target) return;
    setError(null);
    setSaving(false);
    const raf = requestAnimationFrame(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
    return () => cancelAnimationFrame(raf);
  }, [target]);

  if (!target) return null;

  const submit = () => {
    if (saving) return;
    setSaving(true);
    renameScreen(target.id, inputRef.current?.value ?? "")
      .then(() => onOpenChange(false))
      .catch((err: Error) => {
        setError(err.message);
        setSaving(false);
      });
  };

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onOpenChange(false);
      }}
    >
      <DialogContent title={t("rename screen")}>
        <div className="flex flex-col gap-2">
          <input
            ref={inputRef}
            defaultValue={target.title}
            autoFocus
            placeholder={t("screen title")}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !isImeKeydown(e)) {
                e.preventDefault();
                submit();
              }
            }}
            className="h-7 w-full border border-border bg-bg px-2 text-xs text-fg placeholder:text-faint focus:border-fg focus:outline-none"
          />
          {error && <p data-signal="failed" className="font-mono text-2xs text-danger">{error}</p>}
          <div className="flex justify-end gap-2 pt-1">
            <button
              type="button"
              onClick={() => onOpenChange(false)}
              className="flex h-7 items-center px-3 text-xs font-medium text-muted hover:bg-hover hover:text-fg"
            >
              {t("cancel")}
            </button>
            <button
              type="button"
              disabled={saving}
              onClick={submit}
              data-variant="primary" className="flex h-7 items-center bg-fg px-3 text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40"
            >
              {saving ? t("saving…") : t("save")}
            </button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}

/**
 * Persistent screens live in the rail below the main nav: each screen gets an
 * icon with emoji/rename/delete on right-click; creation happens via the shared
 * rail plus button. Deleting a screen leaves the resources shown in it
 * untouched.
 */
export function ScreensRail() {
  const { t } = useTranslation();
  const { screens, removeScreen, isScreenUnsaved, screenSaveError, treeApiFor } = useScreens();
  const [renameTarget, setRenameTarget] = useState<ScreenInfo | null>(null);
  const [emojiTarget, setEmojiTarget] = useState<ScreenInfo | null>(null);
  // Every row by key, once per feed update, so each icon's lookups are cheap.
  const machine = useMachine();
  const sessions = useFeedSessions();
  const byKey = useMemo(
    () => new Map((sessions ?? []).map((s) => [sessionKey({ machine, source: s.source, id: s.id }), s])),
    [sessions, machine],
  );
  /** What the panes on a screen are doing, from its working copy when there is one. */
  const attentionOf = (screen: ScreenInfo) => {
    const root = treeApiFor(screen.id)?.root ?? (screen.layout as PaneNode | null | undefined);
    return screenAttention(sessionsInLayout(root, machine).map((ref) => byKey.get(sessionKey(ref))));
  };

  return (
    <div className="flex flex-col items-center gap-1 max-md:hidden">
      {screens && screens.length > 0 && <div className="my-1 h-px w-5 bg-border" />}
      {screens?.map((screen) => {
        const unsaved = isScreenUnsaved(screen.id);
        const error = screenSaveError(screen.id);
        const attention = attentionOf(screen);
        const notes = [
          attention.asking > 0 ? t("{{n}} asking you", { n: attention.asking }) : null,
          attention.working ? t("a session on this screen is working") : null,
          attention.unread > 0 ? t("{{n}} unread messages", { n: formatUnread(attention.unread) }) : null,
          unsaved ? (error ? t("this change could not be saved: {{error}}", { error }) : t("unsaved")) : null,
        ].filter((note): note is string => note !== null);
        const label = [screen.title, ...notes].join(" · ");
        return (
          <ContextMenu key={screen.id}>
            <ContextMenuTrigger render={<div />}>
              <Link
                to="/screens/$id"
                params={{ id: screen.id }}
                title={label}
                aria-label={label}
                className="relative flex size-8 items-center justify-center text-muted hover:bg-hover hover:text-fg"
                activeProps={{ className: "shadow-[inset_0_0_0_1px_var(--color-fg)] !text-fg" }}
              >
                {screen.emoji
                  ? <span aria-hidden="true" className="text-xl leading-none">{screen.emoji}</span>
                  : <SquaresFourIcon aria-hidden="true" className="size-4" />}
                {/* The last layout change has not reached the server; it is being retried. */}
                {unsaved && (
                  <span
                    aria-hidden="true"
                    data-signal={error ? "failed" : "unsaved"} className={cn("absolute left-0.5 top-0.5 size-1.5", error ? "bg-danger" : "bg-warn")}
                  />
                )}
                {/* What the panes on this screen are doing: unread total top
                    right, sessions asking you bottom left, live work bottom
                    right. */}
                {attention.unread > 0 && (
                  <span
                    aria-hidden="true"
                    data-unread={attention.unread}
                    className="absolute -top-1 -right-1 flex h-3.5 min-w-3.5 items-center justify-center bg-unread px-0.5 font-mono text-[9px] leading-none font-semibold tabular-nums text-bg ring-2 ring-panel"
                  >
                    {formatUnread(attention.unread)}
                  </span>
                )}
                {attention.asking > 0 && (
                  <span
                    aria-hidden="true"
                    data-asking={attention.asking}
                    data-signal="asking" className="absolute -bottom-1 -left-1 flex h-3.5 min-w-3.5 items-center justify-center bg-warn px-0.5 font-mono text-[9px] leading-none font-semibold tabular-nums text-on-signal ring-2 ring-panel"
                  >
                    {attention.asking}
                  </span>
                )}
                {attention.working && (
                  <span aria-hidden="true" className="absolute right-0.5 bottom-0.5 flex ring-2 ring-panel">
                    <LiveDot className="bg-ok" />
                  </span>
                )}
              </Link>
            </ContextMenuTrigger>
            <ContextMenuContent>
              <ContextMenuItem onClick={() => setEmojiTarget(screen)}>
                {t("custom emoji")}
              </ContextMenuItem>
              <ContextMenuItem onClick={() => setRenameTarget(screen)}>
                {t("rename screen")}
              </ContextMenuItem>
              <ContextMenuItem danger onClick={() => removeScreen(screen.id)}>
                {t("delete {{name}}", { name: screen.title })}
              </ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
        );
      })}
      <RenameScreenDialog target={renameTarget} onOpenChange={(open) => !open && setRenameTarget(null)} />
      {emojiTarget && <ScreenEmojiDialog screen={emojiTarget} onClose={() => setEmojiTarget(null)} />}
    </div>
  );
}
