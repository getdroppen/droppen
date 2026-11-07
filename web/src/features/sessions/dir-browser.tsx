import { useQuery } from "@tanstack/react-query";
import { ArrowUpIcon, FolderIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Spinner } from "@/components/ui/spinner";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";

/**
 * Server-side folder navigator so a working directory can be picked without typing.
 *
 * Exported because the scheduled-task form asks the same question about the same
 * filesystem, and a second copy of a directory lister is a second place for the
 * dot-directory ordering and the unreadable-directory fallback to drift.
 */
export function DirBrowser({ start, onPick }: { start: string; onPick: (path: string) => void }) {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const [path, setPath] = useState<string | undefined>(start.startsWith("/") ? start : undefined);

  const { data, isPending, isError } = useQuery({
    queryKey: mq(machine, "fs-list", path ?? "~"),
    queryFn: async () => {
      const res = await api.GET("/api/fs/list", { params: { query: { path } } });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
  });

  const dirs = (data?.entries ?? []).filter((e) => e.kind === "dir");
  // Dot-directories are rarely project roots; keep them reachable but last.
  const ordered = [...dirs.filter((d) => !d.name.startsWith(".")), ...dirs.filter((d) => d.name.startsWith("."))];

  return (
    <div className="mt-1 overflow-hidden rounded border border-border">
      <div className="flex h-6.5 items-center gap-1 border-b border-border bg-raised px-1.5">
        <button
          type="button"
          title={t("parent directory")}
          disabled={!data || data.parent === null}
          onClick={() => data?.parent && setPath(data.parent)}
          className="flex size-5 shrink-0 items-center justify-center rounded text-muted hover:bg-hover disabled:opacity-30"
        >
          <ArrowUpIcon className="size-3" />
        </button>
        <span className="min-w-0 flex-1 truncate font-mono text-2xs text-muted" title={data?.path}>
          {data?.path ?? "…"}
        </span>
        <button
          type="button"
          disabled={!data}
          onClick={() => data && onPick(data.path)}
          className="shrink-0 rounded px-1.5 py-0.5 font-mono text-2xs text-accent hover:bg-accent/15 disabled:opacity-30"
        >
          {t("use this folder")}
        </button>
      </div>
      <div className="max-h-40 overflow-y-auto">
        {isPending && <Spinner className="py-3" />}
        {isError && (
          <div className="flex items-center justify-between px-2 py-1.5">
            <span data-signal="failed" className="font-mono text-2xs text-danger">{t("cannot open this directory")}</span>
            <button
              type="button"
              onClick={() => setPath(undefined)}
              className="rounded px-1.5 py-0.5 font-mono text-2xs text-accent hover:bg-accent/15"
            >
              {t("go to home")}
            </button>
          </div>
        )}
        {data && ordered.length === 0 && (
          <div className="px-2 py-1.5 font-mono text-2xs text-faint">{t("no subfolders")}</div>
        )}
        {data &&
          ordered.map((d) => (
            <button
              key={d.name}
              type="button"
              onClick={() => setPath(`${data.path === "/" ? "" : data.path}/${d.name}`)}
              className="flex w-full items-center gap-1.5 px-2 py-1 text-left hover:bg-hover"
            >
              <FolderIcon className="size-3 shrink-0 text-muted" />
              <span className="truncate font-mono text-2xs text-fg">{d.name}</span>
            </button>
          ))}
      </div>
    </div>
  );
}
