import { useQuery } from "@tanstack/react-query";
import { ChevronLeft, Loader2 } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
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
  ComposerSelect,
  CliModelControls,
  useComposerChoice,
  useComposerStart,
} from "@/features/sessions/composer-controls";
import { isImeKeydown } from "@/lib/ime";
import { useCreateConversation } from "@/features/sessions/use-create-conversation";
import type { NewConversation } from "@/features/sessions/new-session-flow";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";
import { MachineLink } from "@/app/machine-route";

type Backend = "claude" | "codex" | "api";

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

  const chatInput: NewConversation = (
    { kind: "chat", source: backend, text, providerReady: !!providerReady,
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
  // The message as it was when send was pressed: what goes out once it can,
  // whatever is typed meanwhile. Emptying the box calls it off.
  const [pressed, setPressed] = useState<string | null>(null);
  const sendQueued = pressed !== null;
  const recheck = useStartRecheck(startQuery);
  const setBackend = (next: Backend) => {
    setBackendState(next);
    // A pick made for one CLI names an account the other cannot use.
    setAccountChoice(null);
    // And a chat pressed for one CLI is not sent to another once its answer lands.
    setPressed(null);
    recheck.cancel();
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
  const send = () => {
    if (canStart) setPressed(text);
  };
  useEffect(() => {
    if (pressed === null) return;
    // Anything that would not let it start now — the box emptied, above all —
    // calls it off.
    if (!start.canStart) {
      setPressed(null);
      recheck.cancel();
      return;
    }
    const request = { ...chatInput, text: pressed };
    if (!leftToServer) {
      setPressed(null);
      recheck.cancel();
      start.mutate(request);
      return;
    }
    const verdict = recheck.check();
    if (verdict === "wait") return;
    setPressed(null);
    start.mutate(verdict === "unplaced" ? withoutAccount(request) : request);
  });

  return (
    <div className="flex flex-1 items-start justify-center overflow-y-auto p-4 md:items-center">
      <div className="flex w-full max-w-md flex-col gap-2">
        {/* Below md the layout shows this form alone; the way back to the
            list is here, as on a chat's own page (`api-chat-view.tsx`). */}
        <MachineLink
          to="/chat"
          title={t("back")}
          className="flex size-6 shrink-0 items-center justify-center rounded text-muted hover:bg-hover md:hidden"
        >
          <ChevronLeft className="size-4" />
        </MachineLink>
        <div className="font-mono text-2xs font-semibold tracking-wider text-muted uppercase">
          {t("new chat")}
        </div>
        <Tabs value={backend} onValueChange={(v) => setBackend(v as Backend)}>
          <TabsList>
            <TabsTab value="claude">claude</TabsTab>
            <TabsTab value="codex">codex</TabsTab>
            <TabsTab value="api">api</TabsTab>
          </TabsList>
        </Tabs>
        <div className="font-mono text-2xs text-faint">
          {backend === "api"
            ? t("any openai-compatible endpoint (openrouter…)")
            : t("runs on your {{backend}} subscription", { backend })}
        </div>
        {backend !== "api" ? (
          <>
            <div className="flex flex-wrap items-center gap-1.5">
              <CliModelControls choice={choice} />
              {/* A server that predates `accountId` on chats would ignore the pick. */}
              {accounts.length > 0 && knowsStartRule(accountList, cliSource) && (
                <AccountPicker
                  accounts={accounts}
                  value={accountValue || null}
                  onSelect={(id) => setAccountChoice(id ?? "")}
                  allowLocal
                  provider={cliSource}
                  disabled={start.isPending}
                />
              )}
            </div>
            {localSignedOut && (
              <div data-signal="warn" data-local-signed-out="" className="font-mono text-2xs text-warn-fg">
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
              </div>
            )}
          </>
        ) : providerReady ? (
          <div className="flex items-center gap-1.5">
            {apiModels?.length ? (
              <ComposerSelect
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
                className="h-6.5 w-full rounded border border-border bg-bg px-2 font-mono text-2xs text-fg placeholder:text-faint focus:border-fg focus:outline-none"
              />
            )}
          </div>
        ) : (
          <div data-signal="warn" className="font-mono text-2xs text-warn-fg">
            {t("no API provider configured —")}{" "}
            <MachineLink to="/settings" className="text-accent hover:underline">
              {t("set base URL and key in settings")}
            </MachineLink>
          </div>
        )}
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !isImeKeydown(e) && !e.shiftKey && canStart) {
              e.preventDefault();
              send();
            }
          }}
          rows={4}
          placeholder={t("say something…")}
          className="w-full resize-y rounded border border-border bg-bg px-2 py-1.5 text-xs text-fg placeholder:text-faint focus:border-fg focus:outline-none"
        />
        {start.isError && <div data-signal="failed" className="font-mono text-2xs text-danger">{start.error.message}</div>}
        <button
          type="button"
          disabled={!canStart || sendQueued}
          onClick={send}
          data-variant="primary" className="flex h-7 items-center justify-center gap-1.5 bg-fg text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40"
        >
          {(start.isPending || sendQueued) && <Loader2 className="size-3 animate-spin" />}
          {leftToServer && (!startReady || sendQueued) && !start.isPending ? t("choosing account…") : t("start chat")}
        </button>
      </div>
    </div>
  );
}
