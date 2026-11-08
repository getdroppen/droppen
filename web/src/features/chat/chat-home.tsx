import { useQuery } from "@tanstack/react-query";
import { Bot, ChevronLeft, Sparkles, Terminal } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { MachineLink } from "@/app/machine-route";
import { Notice } from "@/components/notice";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { AccountPicker, localLoginHint } from "@/features/accounts/account-picker";
import {
  accountsForProvider,
  knowsStartRule,
  localLoginOf,
  localLoginSignedOut,
  useAccounts,
  useStartRecheck,
  withoutAccount,
} from "@/features/accounts/use-accounts";
import {
  attachmentPaths,
  ComposerBox,
  MAX_ATTACHMENTS,
  useComposerAttachments,
  type ComposerAttachment,
} from "@/features/sessions/composer-box";
import {
  ComposerChoiceChips,
  ComposerSelect,
  useComposerChoice,
  useComposerStart,
} from "@/features/sessions/composer-controls";
import { SessionHeader } from "@/features/sessions/session-header";
import { useCreateConversation } from "@/features/sessions/use-create-conversation";
import type { NewConversation } from "@/features/sessions/new-session-flow";
import { cn } from "@/lib/cn";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";

type Backend = "claude" | "codex" | "api";

/** The column the view is laid out in: the transcript's 48em measure, centred (docs/ui-spec.md §3). */
const COLUMN = "mx-auto w-full max-w-[48rem]";

export function ChatHome() {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const [backend, setBackendState] = useState<Backend>("claude");
  const [text, setText] = useState("");
  const [apiModel, setApiModel] = useState("");

  // CLI backends reuse the synced composer defaults (model + effort).
  const cliSource = backend === "codex" ? "codex" : "claude";
  // Untouched, `POST /api/chats/new` names no account and the spawn follows the
  // server's start rule — the default (moved off a rest), else the local login,
  // else, while that is signed out and nobody chose it, an account in the
  // auto-rotation pool. The picker shows the server's own answer for the model
  // this chat will send, and with nothing stored the model is the one the
  // server paired with that login (`useComposerStart`): sent with it, the chat
  // lands where it is shown. A pick here is sent as the chat's own: "" is the
  // local login, an id an account. The model list follows whichever it is.
  const { data: accountList } = useAccounts();
  const accounts = accountsForProvider(accountList?.accounts ?? [], cliSource);
  const startQuery = useComposerStart(cliSource);
  const { start: startAccount, ready: startReady } = startQuery;
  const [accountChoice, setAccountChoice] = useState<string | null>(null);
  const accountValue = accountChoice ?? startAccount?.accountId ?? "";
  const choice = useComposerChoice(
    cliSource,
    undefined,
    null,
    accountValue || null,
    accountChoice === null ? (startAccount?.model ?? null) : null,
  );
  // Running on a local login known to be signed out — picked by hand, chosen
  // as the default, or the only login there is: allowed, and said, with how to
  // fix it.
  const localSignedOut = backend !== "api" && !accountValue && localLoginSignedOut(localLoginOf(accountList, cliSource));
  // Accounts there are, but none in the pool free to stand in for it.
  const noStandIn = accountChoice === null && startAccount?.reason === "local_signed_out" && accounts.length > 0;

  const { data: provider } = useQuery({
    queryKey: mq(machine, "chat-provider"),
    queryFn: async () => (await api.GET("/api/chat-provider")).data,
  });
  const providerReady = !!provider?.baseUrl && provider.hasKey;
  const { data: apiModels } = useQuery({
    enabled: backend === "api" && providerReady,
    queryKey: mq(machine, "chat-provider-models"),
    queryFn: async () => (await api.GET("/api/chat-provider/models")).data?.models ?? [],
    staleTime: 5 * 60_000,
  });
  const effectiveApiModel = apiModel || provider?.model || "";

  // The same composer state a session's new-session view uses: each file is
  // uploaded the moment it is pasted, dropped or picked, and send waits for
  // the uploads rather than sending without them.
  const attachments = useComposerAttachments();
  // The api backend's provider call takes text parts only, so it has no attach
  // control at all; a switch to it clears what is in the box and says so.
  const [apiCleared, setApiCleared] = useState<string[]>([]);
  // Attachments put back after a refused start that did not fit beside the
  // ones added since, named until the next press (as a session's are).
  const [noRoom, setNoRoom] = useState<string[]>([]);
  // A message with attachments and no words sends the same sentence a
  // session's composer sends, rather than refusing an empty prompt.
  const attachedMessage = t("Please look at the attached file(s).");
  const message = text.trim() || (attachments.attachments.length > 0 ? attachedMessage : "");

  const chatInput: NewConversation = (
    { kind: "chat", source: backend, text: backend === "api" ? text.trim() : message, providerReady: !!providerReady,
      model: backend === "api" ? effectiveApiModel : choice.activeModel.id, effort: choice.activeEffort,
      ...(backend === "api"
        ? {}
        : accountChoice !== null
          ? { accountId: accountChoice || null }
          // A pair no model-less start would reach: the login has to be named.
          : startAccount?.placed === false
            ? { accountId: startAccount.accountId ?? null, accountFrom: "start" as const }
            : {}) });
  // What went out leaves the box; what was typed after the press stays.
  const start = useCreateConversation(chatInput, ({ submitted }) => setText((now) => (now === submitted.text ? "" : now)));
  // The message as it was when send was pressed — its words and the keys of
  // the attachments it carries: what goes out once it can, whatever is typed
  // or added meanwhile. Emptying the box calls it off.
  const [pressed, setPressed] = useState<{ text: string; keys: string[] } | null>(null);
  // What a refused start put back, kept until it is no longer needed.
  const heldBack = useRef<ComposerAttachment[]>([]);
  const sendQueued = pressed !== null;
  const recheck = useStartRecheck(startQuery);
  const setBackend = (next: Backend) => {
    setBackendState(next);
    // A pick made for one CLI names an account the other cannot use.
    setAccountChoice(null);
    // And a chat pressed for one CLI is not sent to another once its answer lands.
    setPressed(null);
    recheck.cancel();
    // The api backend takes text only: nothing can be attached to it, so what
    // is attached is taken out, visibly rather than silently.
    if (next === "api") {
      const names = attachments.attachments.map((a) => a.name);
      if (names.length) {
        attachments.clear();
        setApiCleared(names);
      }
    } else {
      setApiCleared([]);
    }
  };
  // Left to the server, a chat waits for its answer to what is asked now: the
  // login and model shown are that answer's.
  const leftToServer = backend !== "api" && accountChoice === null;
  const canStart = start.canStart && (!leftToServer || startReady);
  // And it goes out on that answer asked again at the press, not as it sat on
  // screen: the rule may have moved on since. A different answer is shown
  // first, and the chat goes out on it — within a few re-asks and a few
  // seconds; past those, with no account, for the server to place
  // (`useStartRecheck`). A pick made meanwhile is sent as the pick.
  //
  // Attachments are the session composer's contract: the press takes the keys
  // it carried, waits for their uploads, and goes out with their paths. A
  // refusal puts them back; a failed upload stops the send until it is removed.
  const send = () => {
    setNoRoom([]);
    if (canStart && !attachments.anyFailed) setPressed({ text, keys: attachments.attachments.map((a) => a.key) });
  };
  useEffect(() => {
    if (pressed === null) return;
    const keys = pressed.keys;
    const held = attachments.attachments.filter((a) => keys.includes(a.key));
    // Anything that would not let it start now — the box emptied of the words
    // it was pressed with, an attachment taken out or failed, no answer yet —
    // calls it off.
    const textCleared = !!pressed.text.trim() && !text.trim();
    if (!start.canStart || textCleared || held.length < keys.length || held.some((a) => a.status === "error")) {
      setPressed(null);
      recheck.cancel();
      return;
    }
    // Uploads are already racing; it goes out the moment they land.
    if (held.some((a) => a.status === "uploading")) return;
    let unplaced = false;
    if (leftToServer) {
      const verdict = recheck.check();
      if (verdict === "wait") return;
      unplaced = verdict === "unplaced";
    } else {
      recheck.cancel();
    }
    setPressed(null);
    const { images, files } = attachmentPaths(held);
    const request = {
      ...chatInput,
      text: pressed.text.trim() || attachedMessage,
      ...(images.length ? { images } : {}),
      ...(files.length ? { files } : {}),
    };
    // What went out leaves the composer; what was typed or added after the
    // press stays.
    heldBack.current = held;
    attachments.take(keys);
    start.mutate(unplaced ? withoutAccount(request) : request, {
      // Refused: nothing started, so the attachments go back where they were,
      // in front of anything added since.
      onError: () => {
        const dropped = attachments.putBack(heldBack.current);
        heldBack.current = [];
        if (dropped.length) setNoRoom(dropped.map((a) => a.name));
      },
    });
  });

  // The explanation belongs to the backend switch, so it sits right beside it
  // rather than apart as a stray line.
  const where = backend === "api"
    ? t("any openai-compatible endpoint (openrouter…)")
    : t("runs on your {{backend}} subscription", { backend });
  const waitingForAccount = leftToServer && (!startReady || sendQueued) && !start.isPending;
  const title = (
    <h2 className="text-[1.25rem] leading-[1.625rem] font-semibold tracking-[-0.02em] text-fg max-md:text-[1.125rem] max-md:leading-6">
      {t("What do you want to chat about?")}
    </h2>
  );

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SessionHeader
        titleRow={<>
          <MachineLink
            to="/chat"
            title={t("back")}
            className="flex size-9 shrink-0 items-center justify-center text-muted hover:bg-hover md:hidden"
          >
            <ChevronLeft className="size-4" />
          </MachineLink>
          <span className="min-w-0 flex-1 truncate text-[13px] font-semibold max-md:text-[15px]">{t("new chat")}</span>
        </>}
      />
      {/* Desktop: the question, about the middle of the view, as a new session's. */}
      <div aria-hidden="true" className="shrink-0 max-md:hidden" style={{ height: "max(1rem, calc(50% - 12rem))" }} />
      <div data-chat-new-title="" className="shrink-0 px-4 pb-3 max-md:hidden">
        <div className={COLUMN}>{title}</div>
      </div>
      {/* The composer: under the question on a desktop, pinned to the bottom on a phone. */}
      <div
        data-chat-new-composer=""
        className="pb-safe shrink-0 bg-panel px-4 max-md:order-last max-md:border-t max-md:border-border max-md:px-2 max-md:pt-2"
      >
        <div data-chat-new-column="" className={cn(COLUMN, "pb-3 max-md:pb-2")}>
          <div className="mb-2 md:hidden">{title}</div>
          {start.isError && <Notice signal="failed">{start.error.message}</Notice>}
          {noRoom.length > 0 && (
            <Notice signal="warn">
              <span data-attachments-dropped="">
                {t("no room beside the returned message's attachments (at most {{max}}), so these were taken out: {{names}}", {
                  max: MAX_ATTACHMENTS,
                  names: noRoom.join(", "),
                })}
              </span>
            </Notice>
          )}
          {attachments.anyFailed && (
            <Notice signal="failed">
              {attachments.attachments.find((a) => a.error)?.error ?? t("upload failed")} — {t("remove the failed attachment to send")}
            </Notice>
          )}
          {apiCleared.length > 0 && (
            <Notice signal="warn">{t("api chats take text only — the attachments were removed")}</Notice>
          )}
          {localSignedOut && (
            <Notice signal="warn" local>
              {localLoginHint(cliSource, t)}
              {accounts.length === 0 && (
                <>
                  {" — "}
                  {t("or add an account")}
                  {" · "}
                  <MachineLink to="/settings" className="underline hover:text-fg">
                    {t("Settings")}
                  </MachineLink>
                </>
              )}
              {noStandIn && <>{" — "}{t("and no account in auto-rotation is free to take its place")}</>}
            </Notice>
          )}
          {backend === "api" && !providerReady && (
            <Notice signal="warn">
              {t("no API provider configured —")}{" "}
              <MachineLink to="/settings" className="text-accent hover:underline">
                {t("set base URL and key in settings")}
              </MachineLink>
            </Notice>
          )}
          <div className="mb-2 flex flex-wrap items-center gap-x-2 gap-y-1">
            <Tabs value={backend} onValueChange={(v) => setBackend(v as Backend)}>
              <TabsList className="w-72 max-w-full">
                <TabsTab value="claude" className="gap-1.5">
                  <Sparkles className="size-3 shrink-0" />
                  claude
                </TabsTab>
                <TabsTab value="codex" className="gap-1.5">
                  <Terminal className="size-3 shrink-0" />
                  codex
                </TabsTab>
                <TabsTab value="api" className="gap-1.5">
                  <Bot className="size-3 shrink-0" />
                  api
                </TabsTab>
              </TabsList>
            </Tabs>
            <span className="font-mono text-2xs text-muted">{where}</span>
          </div>
          {waitingForAccount && (
            <div data-choosing-account="" className="mb-1.5 px-2.5 py-1 font-mono text-2xs text-faint">
              {t("choosing account…")}
            </div>
          )}
          {backend === "claude" && (
            <p data-attachment-hint="" className="mb-1 px-2.5 font-mono text-2xs text-faint">
              {t("claude chats take PNG/JPEG/GIF/WebP images up to 5 MB and UTF-8 text files up to 100 KB — other files are refused")}
            </p>
          )}
          <ComposerBox
            attachments={attachments}
            allowAttach={backend !== "api"}
            text={text}
            onText={setText}
            onSubmit={send}
            enterSends
            placeholder={`${t("say something…")} (${t("Enter to send, Shift+Enter for newline")})`}
            chips={
              backend === "api" ? (
                providerReady ? (
                  apiModels?.length ? (
                    <ComposerSelect
                      ghost
                      label={t("model")}
                      value={effectiveApiModel}
                      options={apiModels}
                      onChange={setApiModel}
                    />
                  ) : (
                    <input
                      value={effectiveApiModel}
                      onChange={(e) => setApiModel(e.target.value)}
                      placeholder={t("model id (e.g. openai/gpt-5.5)")}
                      aria-label={t("model")}
                      className="h-6 w-44 max-w-full min-w-0 border border-border bg-bg px-1.5 font-mono text-2xs text-fg placeholder:text-faint focus:border-fg focus:outline-none"
                    />
                  )
                ) : null
              ) : (
                <ComposerChoiceChips
                  source={cliSource}
                  choice={choice}
                  permission={false}
                  account={
                    accounts.length > 0 && knowsStartRule(accountList, cliSource) ? (
                      <AccountPicker
                        ghost
                        accounts={accounts}
                        value={accountValue || null}
                        onSelect={(id) => setAccountChoice(id ?? "")}
                        allowLocal
                        provider={cliSource}
                        disabled={start.isPending}
                      />
                    ) : undefined
                  }
                />
              )
            }
            send={{
              disabled: !canStart || sendQueued || attachments.anyFailed,
              label: waitingForAccount ? t("choosing account…") : t("start chat"),
              busy: start.isPending || sendQueued,
            }}
          />
        </div>
      </div>
      {/* Below the composer on a desktop, above it on a phone: the view's own space. */}
      <div className="min-h-0 flex-1" />
    </div>
  );
}
