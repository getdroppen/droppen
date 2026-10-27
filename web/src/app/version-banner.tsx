import { RefreshCw, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useFeedSettings } from "./global-feed";
import { liveVersion, NO_DRIFT, observeVersion, type VersionDriftState } from "./version-drift";
import { checkForSwUpdate } from "@/lib/sw-update";

/**
 * True once the server has reported a build different from the one this page
 * loaded against. The baseline lives in component state, so a page load — and
 * only a page load — resets it. The feed re-sends `settings` on every reconnect,
 * which is how a self-restarted server announces its new build.
 */
export function useServerVersionDrift(): boolean {
  const version = liveVersion(useFeedSettings());
  const [state, setState] = useState<VersionDriftState>(NO_DRIFT);
  useEffect(() => {
    setState((s) => observeVersion(s, version));
  }, [version]);
  // The new build's worker precaches what the reload will need, while the
  // banner is still being read.
  useEffect(() => {
    if (state.drifted) checkForSwUpdate();
  }, [state.drifted]);
  return state.drifted;
}

/**
 * Slim notice strip: the server was updated under us. Never reloads on its own
 * — in-flight composer drafts and terminal state would be lost — so the reload
 * is a button, and dismissing hides the strip for the rest of this page load.
 */
export function VersionBanner() {
  const { t } = useTranslation();
  const drifted = useServerVersionDrift();
  const [dismissed, setDismissed] = useState(false);
  if (!drifted || dismissed) return null;
  return (
    <div data-signal="warn" className="flex shrink-0 items-center gap-2 border-b border-border/40 border-l-2 border-l-warn bg-warn/5 px-3 py-1.5">
      <RefreshCw className="size-3 shrink-0 text-warn-fg" />
      <span className="min-w-0 flex-1 truncate text-xs text-muted">
        {t("server updated — reload when convenient")}
      </span>
      <button
        type="button"
        onClick={() => location.reload()}
        data-variant="primary" className="shrink-0 bg-fg px-2 py-0.5 text-xs font-medium text-bg hover:bg-fg/85"
      >
        {t("reload")}
      </button>
      <button
        type="button"
        title={t("dismiss")}
        onClick={() => setDismissed(true)}
        className="shrink-0 rounded p-0.5 text-faint hover:bg-hover hover:text-fg"
      >
        <X className="size-3" />
      </button>
    </div>
  );
}
