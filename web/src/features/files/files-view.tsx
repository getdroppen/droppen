import { useQuery } from "@tanstack/react-query";
import { useSearch } from "@tanstack/react-router";
import { useMachineNavigate } from "@/app/machine-route";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  ChevronRight,
  Copy,
  Eye,
  EyeOff,
  FileText,
  Folder,
  Home,
  RefreshCw,
  Search,
  X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { SelectField } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { DownloadLink, startDownload } from "@/components/host-file";
import {
  clearLastFilesDir,
  readLastFilesDir,
  restoreFilesDir,
  shouldAbandonRestoredDir,
  visibleEntries,
  writeLastFilesDir,
} from "./files-prefs";
import { useShowHiddenFiles } from "./use-files-prefs";
import {
  arrangeFiles,
  directoryInput,
  fileBreadcrumbs,
  joinFilePath,
  type FilesLocation,
  type FileSort,
} from "./files-model";
import { FilePreview } from "./file-preview";
import { UploadButton } from "./files-operations";
import { useCopy } from "@/lib/clipboard";
import { formatBytes, formatTime } from "@/lib/format";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";

export type FilesSearch = FilesLocation;
const tool =
  "flex size-8 shrink-0 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg disabled:opacity-30";

/** Route adapter only. Panes own their location instead of changing the main URL. */
export function FilesView() {
  // The home's `/files` or another machine's: the same page.
  const location = useSearch({ strict: false }) as FilesSearch;
  const navigate = useMachineNavigate();
  return (
    <FilesBrowser
      location={location}
      onNavigate={(search, replace) =>
        void navigate({ to: "/files", search: { ...search }, replace })
      }
    />
  );
}

export function FilesBrowser({
  location,
  onNavigate,
  actions,
}: {
  location: FilesLocation;
  onNavigate: (next: FilesLocation, replace?: boolean) => void;
  actions?: React.ReactNode;
}) {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const { path, file } = location;
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<FileSort>("name");
  const [showHidden, setShowHidden] = useShowHiddenFiles();
  const [pathInput, setPathInput] = useState(path ?? "~");
  const [notice, setNotice] = useState("");
  const pathRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [history, setHistory] = useState({ entries: [location], index: 0 });
  const [initialRestore] = useState(() =>
    file ? null : restoreFilesDir(path, readLastFilesDir(localStorage, machine)),
  );
  const restored = useRef(initialRestore);

  const { data: settings } = useQuery({
    queryKey: mq(machine, "settings"),
    queryFn: async () => (await api.GET("/api/settings")).data,
    staleTime: 60_000,
  });
  const home = settings?.homeDir ?? "";
  const { data, isPending, isError, error, isFetching, refetch } = useQuery({
    queryKey: mq(machine, "fs-list", path ?? "~"),
    queryFn: async ({ signal }) => {
      const res = await api.GET("/api/fs/list", {
        params: { query: path ? { path } : {} },
        signal,
      });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    retry: false,
  });
  const directory = data?.path ?? path ?? "~";
  const shown = useMemo(
    () => visibleEntries(data?.entries ?? [], showHidden),
    [data?.entries, showHidden],
  );
  const entries = useMemo(
    () => arrangeFiles(shown, query, sort),
    [shown, query, sort],
  );
  const hidden = (data?.entries.length ?? 0) - shown.length;
  const virtualizer = useVirtualizer({
    count: entries.length,
    getScrollElement: () => listRef.current,
    estimateSize: () => 40,
    overscan: 12,
    getItemKey: (i) => entries[i]?.name ?? i,
  });

  useEffect(() => {
    if (initialRestore !== null) {
      onNavigate({ path: initialRestore }, true);
    }
  }, []);
  useEffect(() => {
    if (!data?.path || (path === undefined && restored.current !== null))
      return;
    writeLastFilesDir(localStorage, machine, data.path);
    // A newly picked pane must persist its own location, not restore whatever
    // a different file browser happened to visit before the next reload.
    if (path === undefined && restored.current === null)
      onNavigate({ path: data.path, file }, true);
    if (data.path === restored.current) restored.current = null;
  }, [data?.path, path, file, onNavigate, machine]);
  useEffect(() => {
    if (
      shouldAbandonRestoredDir({
        isError,
        path,
        restoredPath: restored.current,
      })
    ) {
      restored.current = null;
      clearLastFilesDir(localStorage, machine);
      onNavigate({ path: "~" }, true);
    }
  }, [isError, path, onNavigate, machine]);
  useEffect(() => {
    setPathInput(directory);
  }, [directory]);
  useEffect(() => {
    setQuery("");
    setNotice("");
    listRef.current?.scrollTo(0, 0);
  }, [path]);
  useEffect(() => {
    listRef.current?.scrollTo(0, 0);
  }, [query, sort, showHidden]);
  useEffect(() => {
    setHistory((prev) => {
      const current = prev.entries[prev.index];
      if (current?.path === path && current?.file === file) return prev;
      return {
        entries: [...prev.entries.slice(0, prev.index + 1), { path, file }],
        index: prev.index + 1,
      };
    });
  }, [path, file]);

  const go = (next: FilesLocation) => onNavigate(next);
  const step = (delta: number) => {
    const index = history.index + delta;
    const target = history.entries[index];
    if (target) {
      setHistory({ ...history, index });
      go(target);
    }
  };
  const copy = useCopy({
    onSuccess: () => setNotice(t("path copied")),
    onError: () => {
      setNotice(t("cannot copy path · select it in the address bar"));
      pathRef.current?.focus();
      pathRef.current?.select();
    },
  }).copy;

  return (
    <section
      className="files-browser flex min-h-0 min-w-0 flex-1 flex-col bg-panel"
      aria-label={t("Files")}
    >
      <header className="flex h-9 shrink-0 items-center gap-2 border-b border-border px-3">
        <Folder className="size-4 text-accent" />
        <span className="min-w-0 flex-1 truncate text-xs font-semibold">
          {t("Files")}
        </span>
        {data && <UploadButton dir={data.path} />}
        {actions}
      </header>
      <form
        className="flex shrink-0 items-center gap-0.5 border-b border-border px-1 py-1"
        onSubmit={(e) => {
          e.preventDefault();
          const next = directoryInput(pathInput, directory);
          if (next) go({ path: next });
        }}
      >
        <button
          type="button"
          className={tool}
          title={t("back")}
          aria-label={t("back")}
          disabled={history.index === 0}
          onClick={() => step(-1)}
        >
          <ArrowLeft className="size-4" />
        </button>
        <button
          type="button"
          className={tool}
          title={t("forward")}
          aria-label={t("forward")}
          disabled={history.index >= history.entries.length - 1}
          onClick={() => step(1)}
        >
          <ArrowRight className="size-4" />
        </button>
        <button
          type="button"
          className={tool}
          title={t("parent directory")}
          aria-label={t("parent directory")}
          disabled={!data?.parent}
          onClick={() => go({ path: data?.parent ?? "~" })}
        >
          <ArrowUp className="size-4" />
        </button>
        <button
          type="button"
          className={tool}
          title={t("home directory")}
          aria-label={t("home directory")}
          onClick={() => go({ path: "~" })}
        >
          <Home className="size-4" />
        </button>
        <input
          ref={pathRef}
          aria-label={t("directory path")}
          title={directory}
          value={pathInput}
          onChange={(e) => setPathInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              setPathInput(directory);
              e.currentTarget.blur();
            }
          }}
          className="h-8 min-w-0 flex-1 rounded border border-border bg-bg px-2 font-mono text-xs outline-none focus:border-accent"
        />
        <button
          type="submit"
          className={tool}
          aria-label={t("open directory")}
          title={t("open directory")}
        >
          <ChevronRight className="size-4" />
        </button>
        <button
          type="button"
          className={tool}
          aria-label={t("refresh")}
          title={t("refresh")}
          disabled={isFetching}
          onClick={() => void refetch()}
        >
          <RefreshCw
            className={`size-3.5 ${isFetching ? "animate-spin" : ""}`}
          />
        </button>
      </form>
      <nav
        aria-label={t("directory breadcrumbs")}
        className="flex min-h-8 shrink-0 items-center gap-1 overflow-x-auto border-b border-border px-2 font-mono text-2xs"
      >
        {fileBreadcrumbs(directory, home).map((part, i) => (
          <span key={part.path} className="flex shrink-0 items-center gap-1">
            {i > 0 && <ChevronRight className="size-3 text-faint" />}
            <button
              type="button"
              className="max-w-64 truncate rounded px-1 py-1 text-muted hover:bg-hover hover:text-accent"
              title={part.path}
              onClick={() => go({ path: part.path })}
            >
              {part.label}
            </button>
          </span>
        ))}
        <button
          type="button"
          className={`${tool} ml-auto`}
          aria-label={t("copy absolute path")}
          title={t("copy absolute path")}
          onClick={() => void copy(directory)}
        >
          <Copy className="size-3.5" />
        </button>
      </nav>
      {notice && (
        <div
          role="status"
          className="flex items-center justify-between px-3 py-1 text-2xs text-muted"
        >
          {notice}
          <button
            type="button"
            aria-label={t("dismiss")}
            onClick={() => setNotice("")}
          >
            <X className="size-3" />
          </button>
        </div>
      )}
      <div
        className={`files-body flex min-h-0 flex-1 ${file ? "has-preview" : ""}`}
      >
        <div className="files-list flex min-h-0 min-w-0 flex-1 flex-col">
          <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-border p-2">
            <label className="flex h-8 min-w-24 flex-1 items-center gap-1.5 rounded border border-border bg-bg px-2">
              <Search className="size-3.5 shrink-0 text-faint" />
              <input
                aria-label={t("filter files")}
                placeholder={t("filter this directory…")}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                className="min-w-0 flex-1 bg-transparent text-xs outline-none"
              />
              {query && (
                <button
                  type="button"
                  aria-label={t("clear filter")}
                  onClick={() => setQuery("")}
                >
                  <X className="size-3" />
                </button>
              )}
            </label>
            <SelectField<FileSort>
              aria-label={t("sort files")}
              value={sort}
              onValueChange={setSort}
              options={[
                { value: "name", label: t("name A–Z") },
                { value: "modified", label: t("newest first") },
                { value: "size", label: t("largest first") },
              ]}
              className="h-8 w-auto max-w-36 font-sans text-2xs"
              popupClassName="font-sans"
            />
            <button
              type="button"
              className={tool}
              aria-pressed={showHidden}
              aria-label={t("show hidden files")}
              title={t("show hidden files")}
              onClick={() => setShowHidden(!showHidden)}
            >
              {showHidden ? (
                <Eye className="size-4 text-accent" />
              ) : (
                <EyeOff className="size-4" />
              )}
            </button>
          </div>
          <div
            ref={listRef}
            className="min-h-0 flex-1 overflow-auto"
            aria-label={t("file list")}
          >
            {isPending && <Spinner />}
            {isError && (
              <div role="alert" data-signal="failed" className="p-4 text-xs text-danger">
                {t("cannot list directory")}
                <p className="mt-1 break-words text-2xs">{error.message}</p>
                <button
                  type="button"
                  className="mt-3 text-accent underline"
                  onClick={() => void refetch()}
                >
                  {t("retry")}
                </button>
              </div>
            )}
            {data && entries.length === 0 && (
              <div className="p-6 text-center text-xs text-muted">
                {query
                  ? t("no matching files")
                  : hidden
                    ? t("only hidden files in this directory")
                    : t("this directory is empty")}
                {hidden > 0 && (
                  <button
                    type="button"
                    className="mt-2 block w-full text-accent"
                    onClick={() => setShowHidden(true)}
                  >
                    {t("show hidden files")}
                  </button>
                )}
              </div>
            )}
            <div
              style={{
                height: virtualizer.getTotalSize(),
                position: "relative",
              }}
            >
              {virtualizer.getVirtualItems().map((vi) => {
                const entry = entries[vi.index]!;
                const full = joinFilePath(directory, entry.name);
                const isDir = entry.kind === "dir";
                return (
                  <div
                    key={vi.key}
                    style={{
                      position: "absolute",
                      top: 0,
                      left: 0,
                      width: "100%",
                      height: 40,
                      transform: `translateY(${vi.start}px)`,
                    }}
                  >
                    <ContextMenu>
                      <ContextMenuTrigger
                        render={
                          <div
                            className={`group flex h-10 items-center gap-2 border-b border-border/40 px-3 hover:bg-hover ${file === full ? "bg-accent/10" : ""}`}
                          >
                            <button
                              type="button"
                              onClick={() =>
                                go(
                                  isDir
                                    ? { path: full }
                                    : { path: directory, file: full },
                                )
                              }
                              title={entry.name}
                              aria-current={file === full ? "true" : undefined}
                              className="flex min-w-0 flex-1 items-center gap-2 self-stretch text-left outline-offset-[-2px]"
                            >
                              {isDir ? (
                                <Folder className="size-4 shrink-0 fill-accent/10 text-accent" />
                              ) : (
                                <FileText className="size-4 shrink-0 text-faint" />
                              )}
                              <span
                                className={`min-w-0 flex-1 truncate text-xs ${isDir ? "font-medium" : "font-mono"}`}
                              >
                                {entry.name}
                              </span>
                              <span className="files-modified shrink-0 text-2xs text-faint">
                                {entry.mtime ? formatTime(entry.mtime) : ""}
                              </span>
                              <span className="w-16 shrink-0 text-right font-mono text-2xs text-faint">
                                {isDir ? (
                                  <ChevronRight className="ml-auto size-3.5" />
                                ) : (
                                  formatBytes(entry.sizeBytes)
                                )}
                              </span>
                            </button>
                            {!isDir && (
                              <DownloadLink
                                path={full}
                                name={entry.name}
                                title={t("download")}
                                className="shrink-0 rounded px-1 py-2 text-2xs text-accent hover:bg-accent/10"
                              >
                                {t("download")}
                              </DownloadLink>
                            )}
                          </div>
                        }
                      />
                      <ContextMenuContent>
                        <ContextMenuItem onClick={() => void copy(full)}>
                          {t("copy absolute path")}
                        </ContextMenuItem>
                        <ContextMenuItem
                          onClick={() =>
                            void copy(
                              home && full.startsWith(`${home}/`)
                                ? full.slice(home.length + 1)
                                : full,
                            )
                          }
                        >
                          {t("copy path relative to ~")}
                        </ContextMenuItem>
                        {!isDir && (
                          <ContextMenuItem
                            onClick={() =>
                              void startDownload(machine, full, { name: entry.name })
                            }
                          >
                            {t("download")}
                          </ContextMenuItem>
                        )}
                      </ContextMenuContent>
                    </ContextMenu>
                  </div>
                );
              })}
            </div>
          </div>
          <footer className="flex h-7 shrink-0 items-center gap-2 border-t border-border px-3 text-2xs text-faint">
            <span>{t("{{n}} items", { n: entries.length })}</span>
            {hidden > 0 && <span>{t("{{n}} hidden", { n: hidden })}</span>}
          </footer>
        </div>
        {file && (
          <FilePreview
            key={file}
            file={file}
            onClose={() => go({ path: directory })}
          />
        )}
      </div>
    </section>
  );
}
