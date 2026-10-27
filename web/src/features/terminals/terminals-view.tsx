import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearch } from "@tanstack/react-router";
import { useMachineNavigate } from "@/app/machine-route";
import { ChevronLeft, Plus, SquareTerminal } from "lucide-react";
import { useTranslation } from "react-i18next";
import { ApiError, errorText } from "@/api/errors";
import type { components } from "@/api/schema";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { Spinner } from "@/components/ui/spinner";
import { Content, Panel, PanelHeader } from "@/components/panel";
import { XtermPane } from "@/features/terminals/lazy-xterm-pane";
import { basename, formatRelative } from "@/lib/format";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { useVisited } from "@/lib/use-visited";

export interface TerminalsSearch {
  id?: string;
}

type TerminalInfo = components["schemas"]["TerminalInfo"];

export function TerminalsView() {
  const api = useApi();
  const machine = useMachine();
  const { t: tr } = useTranslation();
  // The home's `/terminals` or another machine's: the same page.
  const { id: activeId } = useSearch({ strict: false }) as TerminalsSearch;
  const navigate = useMachineNavigate();
  const queryClient = useQueryClient();

  const { data, isPending, error } = useQuery({
    queryKey: mq(machine, "terminals"),
    queryFn: async () => {
      const res = await api.GET("/api/terminals");
      // Thrown, so the list can say why it is empty (in cloud mode: the
      // machine is offline, and terminals need it).
      if (!res.data) throw new ApiError((res as { error?: unknown }).error ?? { error: `HTTP ${res.response.status}` });
      return res.data;
    },
    refetchInterval: 15_000,
  });
  const terminals = data?.terminals ?? [];

  const create = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/terminals", { body: {} });
      if (res.error) throw new ApiError(res.error);
      return res.data;
    },
    onSuccess: (t) => {
      void queryClient.invalidateQueries({ queryKey: mq(machine, "terminals") });
      void navigate({ to: "/terminals", search: { id: t.id } });
    },
    // The server's own sentence when it wrote one (`hint`), else its code.
    onError: (err: Error) => window.alert(`create failed: ${errorText(err)}`),
  });

  const rename = useMutation({
    mutationFn: async ({ id, title }: { id: string; title: string }) => {
      const res = await api.PATCH("/api/terminals/{id}", {
        params: { path: { id } },
        body: { title },
      });
      if (res.error) throw new Error(res.error.error);
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: mq(machine, "terminals") }),
  });

  const kill = useMutation({
    mutationFn: async (id: string) => {
      const res = await api.DELETE("/api/terminals/{id}", { params: { path: { id } } });
      if (res.error) throw new Error(res.error.error);
      return id;
    },
    onSuccess: (id) => {
      void queryClient.invalidateQueries({ queryKey: mq(machine, "terminals") });
      if (id === activeId) void navigate({ to: "/terminals", search: {} });
    },
  });

  const doRename = (t: TerminalInfo) => {
    const title = window.prompt(tr("terminal name:"), t.title);
    if (title?.trim()) rename.mutate({ id: t.id, title: title.trim() });
  };

  const active = terminals.find((t) => t.id === activeId);

  // Keep-alive, on the same rule as the app iframes and the screen frames: a
  // terminal's pane mounts the first time it is selected and then stays mounted
  // (hidden) so its scrollback, its socket and its selection survive switching
  // away and back.
  //
  // What it replaces is a pane per *listed* terminal, which is the thing that
  // made this view expensive. Every XtermPane is a full `Terminal` with a
  // 5000-line scrollback, its own WebGL context and its own websocket attached
  // to a live pty — so merely opening this route built all of that for every
  // terminal on the server, including the ones the user has never looked at,
  // and Chrome silently drops the oldest WebGL context past its per-page cap
  // (~16), leaving already-mounted terminals to fall back to the DOM renderer.
  // Nothing was gained: the server replays a full repaint on every attach
  // (`server-rs/src/terminals/routes.rs`), so a first selection paints straight
  // away without having been attached in advance.
  // Prune killed terminals, but only once the list has actually answered — an
  // empty `data` is "not loaded yet" and would unmount every pane on a refetch.
  const known = data ? terminals.map((t) => t.id) : null;
  const mounted = useVisited(activeId && known?.includes(activeId) ? activeId : null, known);

  return (
    <>
      <Panel collapseKey="terminals" className={activeId ? "max-md:hidden" : ""}>
        <PanelHeader
          title={tr("Terminals")}
          extra={
            <button
              type="button"
              title={tr("new terminal")}
              onClick={() => create.mutate()}
              className="flex size-5 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg"
            >
              <Plus className="size-3.5" />
            </button>
          }
        />
        <div className="min-h-0 flex-1 overflow-y-auto">
          {isPending && <Spinner />}
          {terminals.map((t) => (
            <ContextMenu key={t.id}>
              <ContextMenuTrigger
                render={
                  <button
                    type="button"
                    onClick={() => void navigate({ to: "/terminals", search: { id: t.id } })}
                    className={`flex w-full items-center gap-1.5 border-b border-border/50 px-2.5 py-1.5 text-left hover:bg-hover data-[popup-open]:bg-hover data-[popup-open]:ring-1 data-[popup-open]:ring-accent/40 data-[popup-open]:ring-inset ${
                      t.id === activeId ? "bg-hover" : ""
                    }`}
                  >
                    <SquareTerminal className="size-3.5 shrink-0 text-ok/70" />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-xs">{t.title}</span>
                      <span className="block truncate font-mono text-2xs text-faint">{basename(t.cwd)}</span>
                    </span>
                    <span className="shrink-0 font-mono text-2xs text-faint">
                      {formatRelative(t.createdAt)}
                    </span>
                  </button>
                }
              />
              <ContextMenuContent>
                <ContextMenuItem onClick={() => doRename(t)}>{tr("rename")}</ContextMenuItem>
                <ContextMenuItem onClick={() => void navigator.clipboard.writeText(t.id)}>
                  copy terminal id
                </ContextMenuItem>
                <ContextMenuItem danger onClick={() => kill.mutate(t.id)}>
                  {tr("kill terminal")}
                </ContextMenuItem>
              </ContextMenuContent>
            </ContextMenu>
          ))}
          {!isPending && error && (
            <div data-testid="terminals-error" className="p-3 text-2xs text-warn">
              {errorText(error)}
            </div>
          )}
          {!isPending && !error && terminals.length === 0 && (
            <div className="p-3 text-2xs text-faint">no terminals — create one with +</div>
          )}
        </div>
      </Panel>
      <Content className={activeId ? "" : "max-md:hidden"}>
        {active && (
          <header className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-panel px-3 md:hidden">
            <button
              type="button"
              title={tr("back to terminal list")}
              onClick={() => void navigate({ to: "/terminals", search: {} })}
              className="flex size-6 shrink-0 items-center justify-center rounded text-muted hover:bg-hover"
            >
              <ChevronLeft className="size-4" />
            </button>
            <span className="truncate font-mono text-xs">{active.title}</span>
          </header>
        )}
        <div className="min-h-0 flex-1">
          {mounted.map((id) => (
            <XtermPane key={id} id={id} visible={id === activeId} />
          ))}
          {!activeId && (
            <div className="flex h-full items-center justify-center font-mono text-xs text-faint">
              select or create a terminal
            </div>
          )}
        </div>
      </Content>
    </>
  );
}
