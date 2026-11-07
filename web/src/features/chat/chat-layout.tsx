import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Outlet } from "@tanstack/react-router";
import { RobotIcon, PlusIcon, SparkleIcon, TerminalIcon, XIcon } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import { Spinner } from "@/components/ui/spinner";
import { Content, Panel, PanelHeader } from "@/components/panel";
import { formatRelative } from "@/lib/format";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { MachineLink, useMachineNavigate, useSectionPath } from "@/app/machine-route";

export function useChatList() {
  const api = useApi();
  const machine = useMachine();
  return useQuery({
    queryKey: mq(machine, "chats"),
    queryFn: async () => (await api.GET("/api/chats")).data?.chats ?? [],
    refetchInterval: 15_000,
  });
}

export function ChatLayout() {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const navigate = useMachineNavigate();
  const queryClient = useQueryClient();
  // As the home writes it, on whichever machine this is (`/m/<machine>/chat/…`).
  const pathname = useSectionPath();
  const { data: chats, isPending } = useChatList();
  const detailOpen = pathname !== "/chat";

  const remove = useMutation({
    mutationFn: async (chat: { kind: "claude" | "codex" | "api"; id: string }) => {
      if (chat.kind === "api") {
        await api.DELETE("/api/api-chats/{id}", { params: { path: { id: chat.id } } });
      } else {
        await api.POST("/api/sessions/{source}/{id}/archive", {
          params: { path: { source: chat.kind, id: chat.id } },
        });
      }
      return chat;
    },
    onSuccess: (chat) => {
      void queryClient.invalidateQueries({ queryKey: mq(machine, "chats") });
      if (pathname.includes(chat.id)) void navigate({ to: "/chat" });
    },
  });

  return (
    <>
      <Panel collapseKey="chat" className={detailOpen ? "max-md:hidden" : ""}>
        <PanelHeader
          title={t("Instant Chat")}
          extra={
            <MachineLink
              to="/chat"
              title={t("new chat")}
              className="flex size-5 items-center justify-center rounded text-muted hover:bg-hover hover:text-fg"
            >
              <PlusIcon className="size-3.5" />
            </MachineLink>
          }
        />
        <div className="min-h-0 flex-1 overflow-y-auto">
          {isPending && <Spinner />}
          {chats?.map((chat) => (
            <div key={`${chat.kind}-${chat.id}`} className="group relative">
              <MachineLink
                to={
                  chat.kind === "claude"
                    ? "/chat/claude/$id"
                    : chat.kind === "codex"
                      ? "/chat/codex/$id"
                      : "/chat/api/$id"
                }
                params={{ id: chat.id }}
                className="block w-full border-b border-border/50 px-2.5 py-1.5 text-left hover:bg-hover"
                activeProps={{ className: "bg-hover" }}
              >
                <div className="flex items-center gap-1.5">
                  {chat.kind === "claude" ? (
                    <SparkleIcon className="size-3 shrink-0 text-accent" />
                  ) : chat.kind === "codex" ? (
                    <TerminalIcon className="size-3 shrink-0 text-muted" />
                  ) : (
                    <RobotIcon className="size-3 shrink-0 text-muted" />
                  )}
                  <span className="min-w-0 flex-1 truncate text-xs">{chat.title}</span>
                </div>
                <div className="mt-0.5 flex items-center gap-1.5 pl-4.5">
                  <span className="truncate font-mono text-2xs text-faint">
                    {chat.model ?? t("{{kind}} subscription", { kind: chat.kind })}
                  </span>
                  <span className="ml-auto shrink-0 font-mono text-2xs text-faint">
                    {formatRelative(chat.updatedAt)}
                  </span>
                </div>
              </MachineLink>
              <button
                type="button"
                title={chat.kind === "api" ? t("delete chat") : t("archive chat")}
                onClick={() => remove.mutate(chat)} data-signal="failed" className="absolute top-1 right-1 hidden size-4.5 items-center justify-center rounded bg-panel text-muted group-hover:flex hover:text-danger"
              >
                <XIcon className="size-3" />
              </button>
            </div>
          ))}
          {chats && chats.length === 0 && (
            <div className="p-3 font-mono text-2xs text-faint">{t("no chats yet — start one →")}</div>
          )}
        </div>
      </Panel>
      <Content className={detailOpen ? "" : "max-md:hidden"}>
        <Outlet />
      </Content>
    </>
  );
}
