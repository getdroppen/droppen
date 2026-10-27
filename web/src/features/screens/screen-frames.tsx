import { RefreshCw, X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { PaneTreeView } from "@/app/pane-tree-view";
import { useScreens } from "@/app/screens";
import { PaneLeafContent } from "@/app/secondary-pane";
import { cn } from "@/lib/cn";
import { useVisited } from "@/lib/use-visited";

/**
 * What a screen says about its saves, over the top of its panes: that another
 * device's newer layout just replaced this one (and the edit made here was
 * dropped), or that the last change has not reached the server yet. Small and
 * out of the way — neither blocks the panes, and the rail carries the unsaved
 * mark too where there is a rail.
 */
function ScreenSyncNotices({ id }: { id: string }) {
  const { t } = useTranslation();
  const { isScreenUnsaved, screenSaveError, conflictNoticeAt, dismissConflictNotice, machinesDropped, dismissMachinesDropped } = useScreens();
  const noticeAt = conflictNoticeAt(id);
  const unsaved = isScreenUnsaved(id);
  const error = screenSaveError(id);
  const dropped = machinesDropped(id);
  if (noticeAt === null && !unsaved && !dropped) return null;
  return (
    <div className="pointer-events-none absolute inset-x-0 top-1 z-20 flex flex-col items-center gap-1">
      {noticeAt !== null && (
        <div
          role="status"
          className="pointer-events-auto flex items-center gap-2 rounded border border-border bg-panel px-2 py-1 font-mono text-2xs text-muted shadow-sm"
        >
          <RefreshCw aria-hidden="true" className="size-3 text-accent" />
          {t("this screen was changed on another device and has been synced to the latest")}
          <button
            type="button"
            aria-label={t("dismiss")}
            onClick={() => dismissConflictNotice(id)}
            className="rounded text-faint hover:text-fg"
          >
            <X className="size-3" />
          </button>
        </div>
      )}
      {dropped && (
        <div
          role="status"
          data-signal="warn"
          className="pointer-events-auto flex max-w-[min(32rem,100%)] items-start gap-2 rounded border border-border bg-panel px-2 py-1 font-mono text-2xs text-warn-fg shadow-sm"
        >
          <span className="min-w-0 flex-1">
            {t("The device that keeps this screen runs an older Droppen and cannot keep panes of other devices: they went back to it. Update it, then choose them again.")}
          </span>
          <button
            type="button"
            aria-label={t("dismiss")}
            onClick={() => dismissMachinesDropped(id)}
            className="rounded text-faint hover:text-fg"
          >
            <X className="size-3" />
          </button>
        </div>
      )}
      {unsaved && (
        <div
          role="status"
          title={
            error
              ? t("this change could not be saved: {{error}}", { error })
              : t("this change has not been saved yet; retrying")
          }
          className={cn(
            "rounded px-1.5 py-0.5 font-mono text-2xs",
            error ? "bg-danger/15 text-danger" : "bg-warn/15 text-warn-fg",
          )}
        >
          {t("unsaved")}
        </div>
      )}
    </div>
  );
}

/**
 * Keep-alive layer for screens, rendered over the routed view like AppFrames.
 * A screen's pane tree mounts on first visit and stays mounted (just hidden)
 * while other routes show, so terminals keep their scroll position and
 * websockets stay attached across route switches. Frames unmount only when
 * their screen is deleted.
 */
export function ScreenFrames() {
  const { screens, activeScreenId, treeApiFor } = useScreens();

  const known = screens?.map((s) => s.id) ?? null;
  const next = useVisited(activeScreenId && known?.includes(activeScreenId) ? activeScreenId : null, known);

  return (
    <>
      {next.map((id) => {
        const treeApi = treeApiFor(id);
        if (!treeApi) return null;
        return (
          <div key={id} className={cn("absolute inset-0 z-10 flex bg-bg", id !== activeScreenId && "hidden")}>
            <PaneTreeView
              api={treeApi}
              active={id === activeScreenId}
              renderLeaf={(leaf) => (
                <PaneLeafContent
                  content={leaf.content}
                  onChange={(content) => treeApi.setPaneContent(leaf.id, content)}
                  onClose={() => treeApi.closePane(leaf.id)}
                />
              )}
            />
            <ScreenSyncNotices id={id} />
          </div>
        );
      })}
    </>
  );
}
