/*
 * A page's own chunk (`main.tsx`: what a hub serves, the local
 * `/link/return`), and what the page does when it does not come: the link
 * dropped, or a release replaced the build this shell is from and its chunks
 * are gone from the server. The first time, the page reloads, into the
 * current build — once per tab, so a reload that does not help is not tried
 * again. If the chunk does not come then either, the page says so and offers
 * to try again or to reload, rather than staying blank.
 */
import { StrictMode, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

/** Set in this tab's storage while a reload for a page's chunk is under way. */
export const RELOADED_KEY = "droppen.page-load.reloaded";

/** What the page reloads with, and where the tab remembers that it did (a test passes fakes). */
export interface PageLoadEnv {
  storage: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null;
  reload(): void;
}

function browserEnv(): PageLoadEnv {
  let storage: Storage | null = null;
  try {
    storage = window.sessionStorage;
  } catch {
    storage = null;
  }
  return { storage, reload: () => location.reload() };
}

/** Whether this is the tab's first failure since its last good load: then it is noted, for the reload. */
function firstFailure(env: PageLoadEnv): boolean {
  try {
    if (!env.storage || env.storage.getItem(RELOADED_KEY) !== null) return false;
    env.storage.setItem(RELOADED_KEY, "1");
    return true;
  } catch {
    // Nowhere to note it: no reload, which could otherwise repeat for good.
    return false;
  }
}

function loaded(env: PageLoadEnv): void {
  try {
    env.storage?.removeItem(RELOADED_KEY);
  } catch {
    // Nothing was noted.
  }
}

/**
 * Load the page's chunk (`load`) and `show` it. `beforeReload` runs before
 * any reload: what the page holds that the reload would lose goes back where
 * the reloaded page finds it.
 */
export function loadPage<T>(
  root: { render(node: ReactNode): void },
  load: () => Promise<T>,
  show: (page: T) => void,
  opts: { beforeReload?: () => void; env?: PageLoadEnv } = {},
): Promise<void> {
  const env = opts.env ?? browserEnv();
  const reload = () => {
    opts.beforeReload?.();
    env.reload();
  };
  const attempt = async (): Promise<void> => {
    let page: T;
    try {
      page = await load();
    } catch {
      if (firstFailure(env)) return reload();
      root.render(
        <StrictMode>
          <PageLoadFailed onRetry={() => void attempt()} onReload={reload} />
        </StrictMode>,
      );
      return;
    }
    loaded(env);
    show(page);
  };
  return attempt();
}

export function PageLoadFailed({ onRetry, onReload }: { onRetry: () => void; onReload: () => void }) {
  const { t } = useTranslation();
  const button = "rounded border border-border px-2 py-0.5 hover:bg-hover";
  return (
    <div data-testid="page-load-failed" className="flex h-full flex-col items-center justify-center gap-2 font-mono text-xs text-muted">
      <span>{t("the page could not be loaded")}</span>
      <div className="flex gap-2">
        <button type="button" className={button} onClick={onRetry}>
          {t("retry")}
        </button>
        <button type="button" className={button} onClick={onReload}>
          {t("reload")}
        </button>
      </div>
    </div>
  );
}
