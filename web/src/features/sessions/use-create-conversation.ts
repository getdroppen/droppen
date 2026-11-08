import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "@/api/errors";
import type { components } from "@/api/schema";
import { useGlobalFeedSelector, type GlobalFeedState } from "@/app/global-feed";
import { stashPendingMessage } from "@/features/chat/pending-message";
import { canCreateConversation, creationAttempt, creationRequest, resolveCreatedSession, withStartedPermission, type CreationAttempt, type NewConversation, type SessionSource } from "./new-session-flow";
import { newClientMessageId } from "./send-receipts";
import { useApi, useMachine } from "@/lib/machines/context";
import { useMachineNavigate } from "@/app/machine-route";
import { mq } from "@/lib/machines/query";

type ComposerPrefs = components["schemas"]["ComposerPrefs"];

/** What the server accepted: the request as it was sent, the id it answered, and the message id it went under. */
export interface AcceptedConversation {
  submitted: NewConversation;
  id: string;
  clientMessageId: string;
}

export interface CreateConversationOptions {
  /**
   * The attempt a start that may already have reached the server went under —
   * kept by the caller across a reload (the new-session draft), so pressing
   * send again after one is answered with the session it started.
   */
  attempt?: CreationAttempt | null;
  /** The attempt in force changed: a new request, or a start that went through (`null`). */
  onAttempt?: (attempt: CreationAttempt | null) => void;
}

/** Both CLI surfaces wait for discovery; API chats exist as soon as POST returns. */
export function useCreateConversation(
  input: NewConversation,
  onAccepted: (accepted: AcceptedConversation) => void,
  onCreated?: (source: SessionSource, id: string) => void,
  options: CreateConversationOptions = {},
) {
  const api = useApi();
  const machine = useMachine();
  const navigate = useMachineNavigate();
  const queryClient = useQueryClient();
  const [pending, setPending] = useState<{ kind: "session" | "chat"; source: SessionSource; id: string } | null>(null);
  const sessions = useGlobalFeedSelector(useCallback(
    (state: GlobalFeedState) => pending?.kind === "session" ? state.sessions : null,
    [pending?.kind],
  ));
  const { data: chats } = useQuery({
    queryKey: mq(machine, "chats"),
    queryFn: async () => {
      const res = await api.GET("/api/chats");
      if (!res.data) throw new Error("cannot load chats");
      return res.data.chats;
    },
    enabled: pending?.kind === "chat",
    refetchInterval: pending?.kind === "chat" ? 500 : false,
  });

  useEffect(() => {
    const available = pending?.kind === "chat"
      ? chats?.flatMap((chat) => chat.kind === "api" ? [] : [{ source: chat.kind, id: chat.id }]) ?? null
      : sessions;
    const found = resolveCreatedSession(pending, available);
    if (!found) return;
    setPending(null);
    if (pending?.kind === "chat") {
      void navigate({ to: found.source === "claude" ? "/chat/claude/$id" : "/chat/codex/$id", params: { id: found.id } });
    } else if (onCreated) {
      onCreated(found.source, found.id);
    } else {
      void navigate({ to: "/sessions/$source/$id", params: { source: found.source, id: found.id } });
    }
  }, [pending, sessions, chats, navigate, onCreated]);

  // Kept across retries of the same request, so a start whose answer was lost
  // is answered with the session it made instead of making a second one.
  const attempt = useRef<CreationAttempt | null>(options.attempt ?? null);
  const onAttempt = useRef(options.onAttempt);
  onAttempt.current = options.onAttempt;
  const mutation = useMutation({
    // `override`: the request as the caller assembled it at the moment of
    // sending (a composer whose attachments just finished uploading), rather
    // than whatever the last render handed this hook.
    mutationFn: async (override: NewConversation | void) => {
      // Capture the submitted settings/text, not whatever the form shows when POST finishes.
      const submitted = override ?? input;
      const next = creationAttempt(attempt.current, submitted, newClientMessageId);
      if (next !== attempt.current) onAttempt.current?.(next);
      attempt.current = next;
      const request = creationRequest(submitted, next.clientMessageId);
      const res = request.path === "/api/sessions/new"
        ? await api.POST(request.path, { body: request.body })
        : await api.POST(request.path, { body: request.body });
      // An ApiError, so a refusal with a reason of its own (an attachment no
      // longer on the device) reads as that reason, not as its bare token.
      if (res.error) throw new ApiError(res.error);
      if (!res.data.id) throw new Error("created conversation has no id");
      return { submitted, id: res.data.id, clientMessageId: next.clientMessageId };
    },
    onSuccess: (accepted) => {
      const { submitted, id } = accepted;
      // Started: the next start is a new session.
      attempt.current = null;
      onAttempt.current?.(null);
      // The server recorded the start's permission on the session's own row;
      // put the same row in this page's cache before the session opens, so its
      // composer shows — and sends — that mode rather than the CLI default
      // while its first prefs refetch is still in flight.
      if (submitted.kind === "session") {
        const { source, permission } = submitted;
        if (permission) {
          queryClient.setQueryData<ComposerPrefs>(mq(machine, "composer-prefs"), (old) =>
            withStartedPermission(old, source, id, permission),
          );
        }
      }
      if (submitted.kind === "chat") void queryClient.invalidateQueries({ queryKey: mq(machine, "chats") });
      if (submitted.source === "api") {
        stashPendingMessage(machine, id, submitted.text.trim());
        void navigate({ to: "/chat/api/$id", params: { id } });
      } else {
        setPending({ kind: submitted.kind, source: submitted.source, id });
      }
      onAccepted(accepted);
    },
  });
  const isPending = mutation.isPending || pending !== null;
  return { ...mutation, isPending, canStart: canCreateConversation(input, isPending) };
}
