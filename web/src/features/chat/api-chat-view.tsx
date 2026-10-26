import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import { ChevronLeft, Loader2, SendHorizontal } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { DiagramRendering, drawsDiagrams, Markdown, MathRendering } from "@/components/markdown";
import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import { autoGrowTextarea, useEnterSendsModifier } from "@/features/sessions/composer-controls";
import { takePendingMessage } from "@/features/chat/pending-message";
import { authHeader, noteUnauthorized } from "@/lib/auth";
import { cn } from "@/lib/cn";
import { isImeKeydown } from "@/lib/ime";
import { useApi, useMachine } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";
import { transportFor } from "@/lib/machines/registry";
import { MachineLink } from "@/app/machine-route";

interface StreamEvent {
  type: "delta" | "done" | "error";
  text?: string;
  content?: string;
  error?: string;
}

/** POST the message to `machine`'s chat `id` and feed SSE deltas to the callback; resolves when done. */
async function streamTurn(
  machine: MachineKey,
  id: string,
  text: string,
  onDelta: (t: string) => void,
): Promise<{ error: string | null }> {
  const res = await transportFor(machine).fetch(new Request(`/api/api-chats/${id}/message`, {
    method: "POST",
    // A streaming response, so this cannot go through the typed client: the
    // bearer token and the 401 handling its middleware does are by hand.
    headers: { "content-type": "application/json", ...authHeader() },
    body: JSON.stringify({ text }),
  }));
  if (res.status === 401) noteUnauthorized(`/api/api-chats/${id}/message`);
  if (!res.ok || !res.body) {
    const body = (await res.text().catch(() => "")).slice(0, 200);
    return { error: `HTTP ${res.status}${body ? ` — ${body}` : ""}` };
  }
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let error: string | null = null;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split("\n");
    buffer = lines.pop() ?? "";
    for (const line of lines) {
      if (!line.startsWith("data:")) continue;
      try {
        const evt = JSON.parse(line.slice(5).trim()) as StreamEvent;
        if (evt.type === "delta" && evt.text) onDelta(evt.text);
        if (evt.type === "error") error = evt.error ?? "chat request failed";
      } catch {
        // partial line; ignored
      }
    }
  }
  return { error };
}

export function ApiChatView() {
  // The home's `/chat/api/$id` or another machine's: the same page.
  const { id } = useParams({ strict: false }) as { id: string };
  return <ApiChatViewInner id={id} />;
}

export function ApiChatViewInner({ id }: { id: string }) {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const [text, setText] = useState("");
  const [streaming, setStreaming] = useState<string | null>(null);
  const [pendingUser, setPendingUser] = useState<string | null>(null);
  const [sendError, setSendError] = useState<string | null>(null);
  const enterSendsModifier = useEnterSendsModifier();
  const queryClient = useQueryClient();
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  useEffect(() => autoGrowTextarea(textareaRef.current), [text]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const stickToBottom = useRef(true);
  const busyRef = useRef(false);

  const { data: chat, isPending } = useQuery({
    queryKey: mq(machine, "api-chat", id),
    queryFn: async () => {
      const res = await api.GET("/api/api-chats/{id}", { params: { path: { id } } });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
  });
  const { data: prefs } = useQuery({
    queryKey: mq(machine, "prefs"),
    queryFn: async () => (await api.GET("/api/prefs")).data,
  });
  const bubble = prefs?.transcriptStyle === "bubble";
  const renderMath = prefs?.math !== "source";
  const renderDiagrams = drawsDiagrams(prefs);

  const send = async (message: string) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setSendError(null);
    setPendingUser(message);
    setStreaming("");
    let acc = "";
    const { error } = await streamTurn(machine, id, message, (delta) => {
      acc += delta;
      setStreaming(acc);
    }).catch((e: Error) => ({ error: e.message }));
    await queryClient.invalidateQueries({ queryKey: mq(machine, "api-chat", id) });
    await queryClient.invalidateQueries({ queryKey: mq(machine, "chats") });
    setStreaming(null);
    setPendingUser(null);
    if (error) setSendError(error);
    busyRef.current = false;
  };

  // First message stashed by the new-chat form arrives here after navigation.
  useEffect(() => {
    const pending = takePendingMessage(machine, id);
    if (pending) void send(pending);
  }, [machine, id]); // eslint-disable-line react-hooks/exhaustive-deps

  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [chat, streaming, pendingUser]);

  const submit = () => {
    const message = text.trim();
    if (!message || busyRef.current) return;
    setText("");
    void send(message);
  };

  if (isPending) return <Spinner className="flex-1" />;
  if (!chat) {
    return (
      <div className="flex flex-1 flex-col items-center justify-center gap-2">
        <div className="font-mono text-xs text-muted">{t("this chat is gone")}</div>
        <MachineLink to="/chat" className="font-mono text-2xs text-accent hover:underline">
          {t("back to chats")}
        </MachineLink>
      </div>
    );
  }

  const rows = [
    ...chat.messages,
    ...(pendingUser !== null ? [{ role: "user" as const, content: pendingUser, at: "" }] : []),
    ...(streaming !== null ? [{ role: "assistant" as const, content: streaming, at: "" }] : []),
  ];

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="flex shrink-0 items-center gap-2 border-b border-border bg-panel px-3 py-1.5">
        <MachineLink
          to="/chat"
          title={t("back")}
          className="flex size-6 shrink-0 items-center justify-center rounded text-muted hover:bg-hover md:hidden"
        >
          <ChevronLeft className="size-4" />
        </MachineLink>
        <span className="min-w-0 flex-1 truncate text-xs font-medium">{chat.title}</span>
        <Badge variant="outline">{chat.model}</Badge>
      </header>
      <div
        ref={scrollRef}
        onScroll={() => {
          const el = scrollRef.current;
          if (el) stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
        }}
        className="min-h-0 flex-1 overflow-y-auto px-3 py-2"
      >
        <MathRendering.Provider value={renderMath}>
        <DiagramRendering.Provider value={renderDiagrams}>
        {rows.map((m, i) => {
          const isUser = m.role === "user";
          const spinner = streaming !== null && i === rows.length - 1 && (
            <Loader2 className="mt-1 size-3 animate-spin text-muted" />
          );
          const inner = isUser ? (
            <div className="whitespace-pre-wrap text-xs">{m.content}</div>
          ) : (
            <div className="text-xs">
              <Markdown text={m.content} />
              {spinner}
            </div>
          );
          return (
            <div key={i} className={cn("mb-3 flex flex-col", bubble && (isUser ? "items-end" : "items-start"))}>
              <div className="mb-0.5 font-mono text-2xs text-faint">{isUser ? t("you") : chat.model}</div>
              {bubble ? (
                <div
                  className={cn(
                    "max-w-[85%] rounded-lg border px-3 py-2",
                    isUser ? "border-accent/25 bg-accent/10" : "border-border bg-panel",
                  )}
                >
                  {inner}
                </div>
              ) : isUser ? (
                <div className="whitespace-pre-wrap rounded border border-accent/20 bg-accent/5 px-2.5 py-1.5 text-xs">
                  {m.content}
                </div>
              ) : (
                inner
              )}
            </div>
          );
        })}
        </DiagramRendering.Provider>
        </MathRendering.Provider>
        {rows.length === 0 && (
          <div className="p-3 font-mono text-2xs text-faint">{t("no messages yet")}</div>
        )}
      </div>
      <div className="shrink-0 border-t border-border bg-panel">
        {sendError && (
          <div data-signal="failed" className="border-b border-border/50 px-3 py-1 font-mono text-2xs text-danger">
            {sendError}
          </div>
        )}
        <div className="flex items-end gap-2 p-2">
          <textarea
            ref={textareaRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key !== "Enter" || isImeKeydown(e)) return;
              const sendKey = enterSendsModifier ? e.metaKey || e.ctrlKey : !e.shiftKey;
              if (sendKey) {
                e.preventDefault();
                submit();
              }
            }}
            rows={2}
            placeholder={t("message {{model}}…", { model: chat.model })}
            className="max-h-40 min-h-8 flex-1 resize-none rounded border border-border bg-bg px-2 py-1.5 text-xs text-fg placeholder:text-faint focus:border-fg focus:outline-none md:max-h-72"
          />
          <button
            type="button"
            onClick={submit}
            disabled={streaming !== null || !text.trim()}
            title={t("send")}
            data-variant="primary" className="flex size-7 shrink-0 items-center justify-center bg-fg text-bg hover:bg-fg/85 disabled:opacity-40"
          >
            {streaming !== null ? (
              <Loader2 className="size-3.5 animate-spin" />
            ) : (
              <SendHorizontal className="size-3.5" />
            )}
          </button>
        </div>
      </div>
    </div>
  );
}
