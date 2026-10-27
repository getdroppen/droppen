import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { Loader2 } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/cn";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { useVisited } from "@/lib/use-visited";

function AppFrame({ id, title }: { id: string; title: string }) {
  const { t } = useTranslation();
  const [loaded, setLoaded] = useState(false);
  return (
    <div className="relative flex size-full">
      {!loaded && (
        <div className="absolute inset-0 flex items-center justify-center gap-2 bg-bg">
          <Loader2 className="size-4 animate-spin text-muted" />
          <span className="font-mono text-2xs text-muted">{t("starting {{name}}…", { name: title })}</span>
        </div>
      )}
      <iframe
        src={`/embed/${id}/`}
        title={title}
        onLoad={() => setLoaded(true)}
        className="size-full border-0"
        allow="clipboard-read; clipboard-write"
      />
    </div>
  );
}

/**
 * Keep-alive layer for embedded apps, rendered over the routed view. An app's
 * iframe mounts on first visit and stays mounted (just hidden) while other
 * routes show, so switching to a session and back does not reload VSCode.
 * Frames unmount only when their instance is closed.
 */
export function AppFrames() {
  const api = useApi();
  const machine = useMachine();
  const activeId = useRouterState({
    select: (s) => /^\/apps\/([\w-]+)/.exec(s.location.pathname)?.[1] ?? null,
  });
  const { data: apps } = useQuery({
    queryKey: mq(machine, "apps"),
    queryFn: async () => (await api.GET("/api/apps")).data?.apps ?? [],
    // Converge on instances closed elsewhere (other tab, crashed process).
    refetchInterval: 15_000,
  });

  // Mount on visit, prune instances the server no longer lists. A freshly
  // created app is already in the cache (AppsRail writes it on success), so
  // pruning strictly by the list is safe.
  const next = useVisited(activeId, apps ? apps.map((a) => a.id) : null);

  return (
    <>
      {next.map((id) => (
        <div key={id} className={cn("absolute inset-0 z-10 bg-bg", id !== activeId && "hidden")}>
          <AppFrame id={id} title={apps?.find((a) => a.id === id)?.title ?? "app"} />
        </div>
      ))}
    </>
  );
}
