import { useMutation } from "@tanstack/react-query";
import { PushPinIcon, PushPinSlashIcon } from "@phosphor-icons/react";
import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import type { components } from "@/api/schema";
import { useGlobalFeedSelector, useGlobalFeedStore, type GlobalFeedState } from "@/app/global-feed";
import { useApi } from "@/lib/machines/context";

type Session = components["schemas"]["SessionSummary"];
type PinTarget = Pick<Session, "source" | "id" | "pinned">;

/** Shared by the row menu and the touch-accessible detail button. */
export function useSessionPin(onError: (error: Error) => void) {
  const api = useApi();
  const store = useGlobalFeedStore();
  return useMutation({
    mutationFn: async (target: PinTarget) => {
      const res = await api.PUT("/api/sessions/{source}/{id}/pin", {
        params: { path: { source: target.source, id: target.id } },
        body: { pinned: !target.pinned },
      });
      if (res.error) throw new ApiError(res.error);
      return { ...target, pinned: res.data.pinned };
    },
    onSuccess: (target) => {
      // Only after persistence succeeds. Other devices receive the same change
      // through the ordinary feed delta; no second preference cache is needed.
      const sessions = store.getSnapshot().sessions;
      if (sessions) store.publish({ sessions: sessions.map((s) => s.source === target.source && s.id === target.id ? { ...s, pinned: target.pinned } : s) });
    },
    onError,
  });
}

export function SessionPinButton({ session, onError }: { session: Session; onError: (message: string) => void }) {
  const { t } = useTranslation();
  const pinned = useGlobalFeedSelector(useCallback(
    (state: GlobalFeedState) => state.sessions?.find((s) => s.source === session.source && s.id === session.id)?.pinned ?? session.pinned,
    [session.source, session.id, session.pinned],
  ));
  const pin = useSessionPin((err) => onError(t("pin failed: {{msg}}", { msg: errorText(err) })));
  const label = t(pinned ? "unpin session" : "pin session");
  return (
    <button
      type="button"
      title={label}
      aria-label={label}
      aria-pressed={!!pinned}
      disabled={pin.isPending}
      onClick={() => pin.mutate({ source: session.source, id: session.id, pinned })}
      className={`flex size-6 shrink-0 items-center justify-center rounded hover:bg-hover disabled:opacity-40 ${pinned ? "text-accent" : "text-muted hover:text-fg"}`}
    >
      {pinned ? <PushPinSlashIcon className="size-3.5" /> : <PushPinIcon className="size-3.5" />}
    </button>
  );
}
