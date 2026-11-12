import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckIcon, CircleNotchIcon, PencilSimpleIcon, PlusIcon, ArrowsClockwiseIcon, TrashIcon } from "@phosphor-icons/react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CheckboxField } from "@/components/ui/checkbox-field";
import { Input } from "@/components/ui/input";
import { SelectField } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import { EnvBlockField } from "@/features/accounts/env-block-field";
import { parseEnvBlock } from "@/features/accounts/env-block";
import {
  accountsKey,
  accountEnvKeys,
  accountKindLabel,
  accountRotationToggleShown,
  accountsForProvider,
  accountUsageState,
  localLoginLabel,
  localLoginOf,
  localLoginSignedOut,
  quotaKey,
  useAccounts,
  usageNotice,
  useQuota,
  withAccount,
  type Account,
  type AccountKind,
  type AccountList,
  type AccountProvider,
  type AccountUsageState,
  type LocalLogin,
  type UsageNotice,
} from "@/features/accounts/use-accounts";
import { localLoginHint } from "@/features/accounts/account-picker";
import { useComposerStart } from "@/features/sessions/composer-controls";
import { activeRests, limitEventsKey } from "@/features/accounts/account-rest";
import { RestChip } from "@/features/accounts/rest-chip";
import { ReauthDialog, SetupTokenDialog, UsageLoginDialog } from "@/features/settings/account-wizards";
import { LimitEventsPanel } from "@/features/settings/limit-events-panel";
import { cn } from "@/lib/cn";
import { PROVIDER_NAME } from "@/lib/provider-name";
import { SettingsColumn, SettingField, SettingsHint, SettingsNote } from "@/features/settings/layout";
import { InlineEdit } from "@/features/settings/inline-edit";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";

type AccountConflict = components["schemas"]["AccountConflict"];

/** POST /api/accounts answered 409: the same secret is already stored. */
class DuplicateAccountError extends Error {
  readonly existingLabel: string;
  constructor(existingLabel: string) {
    super("duplicate_account");
    this.existingLabel = existingLabel;
  }
}

function AccountLabel({ account }: { account: Account }) {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const queryClient = useQueryClient();

  const rename = useMutation({
    mutationFn: async (label: string) => {
      const res = await api.PATCH("/api/accounts/{id}", {
        params: { path: { id: account.id } },
        body: { label },
      });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: accountsKey(machine) });
      void queryClient.invalidateQueries({ queryKey: quotaKey(machine) });
    },
  });

  return (
    <InlineEdit
      value={account.label}
      title={t("rename account")}
      onCommit={(label) => rename.mutateAsync(label)}
    />
  );
}

function AccountRow({
  account,
  provider,
  radioName,
  isDefault,
  usage,
  notice,
  onMakeDefault,
  onEnableUsage,
  onReauth,
}: {
  account: Account;
  /** The group this row is in, which is also the account's own CLI. */
  provider: AccountProvider;
  /** One radio group per CLI: each keeps its own default. */
  radioName: string;
  isDefault: boolean;
  /**
   * `usageEnabled` is the verified login (accounts start false and flip only
   * after usage-verify); the quota probe adds `not_logged_in` for a login that
   * has since expired, and `probe_failed` for a probe that broke, which says
   * nothing about the login and must not send the user back to the wizard.
   */
  usage: AccountUsageState;
  /**
   * The same state weighed against how fresh the account's reading is: an
   * expired /usage login behind numbers its sessions keep current is only the
   * idle-time fallback lost, and reads faint rather than as an alarm.
   */
  notice: UsageNotice;
  onMakeDefault: () => void;
  onEnableUsage: () => void;
  onReauth: () => void;
}) {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState(false);
  const envKeys = accountEnvKeys(account);
  const rests = activeRests(account);

  // Clears one rest by its scope; the reply is the whole list, rests and all.
  const clearRest = useMutation({
    mutationFn: async (scope: string) => {
      const res = await api.DELETE("/api/accounts/{id}/rest", {
        params: { path: { id: account.id }, query: { scope } },
      });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: (list) => {
      if (list) queryClient.setQueryData<AccountList>(accountsKey(machine), list);
      else void queryClient.invalidateQueries({ queryKey: accountsKey(machine) });
      void queryClient.invalidateQueries({ queryKey: limitEventsKey(machine) });
    },
  });

  // In or out of the rotation pool; the reply is this account alone, which
  // replaces its entry in the cached list.
  const rotate = useMutation({
    mutationFn: async (rotation: boolean) => {
      const res = await api.PATCH("/api/accounts/{id}", {
        params: { path: { id: account.id } },
        body: { rotation },
      });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: (updated) => {
      if (updated) {
        queryClient.setQueryData<AccountList>(accountsKey(machine), (list) => (list ? withAccount(list, updated) : list));
      } else {
        void queryClient.invalidateQueries({ queryKey: accountsKey(machine) });
      }
    },
  });
  // The pick shows while it is in flight, so the box does not flick back first.
  const rotating = rotate.isPending ? rotate.variables : account.rotation;

  const remove = useMutation({
    mutationFn: async () => {
      const res = await api.DELETE("/api/accounts/{id}", { params: { path: { id: account.id } } });
      if (res.error) throw new Error(res.error.error);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: accountsKey(machine) });
      void queryClient.invalidateQueries({ queryKey: quotaKey(machine) });
    },
  });

  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border py-1.5">
      <label className="flex shrink-0 items-center gap-1.5" title={t("new sessions use this account by default")}>
        <input
          type="radio"
          name={radioName}
          checked={isDefault}
          onChange={onMakeDefault}
          className="size-3 accent-accent"
        />
        <span className="text-xs text-muted">{t("default")}</span>
      </label>
      <AccountLabel account={account} />
      <Badge variant={provider}>{PROVIDER_NAME[provider]}</Badge>
      <Badge variant="outline">{accountKindLabel(account.kind)}</Badge>
      {/*
        Resting after a CLI turn under this account hit a limit. Shown only:
        nothing in phase 1 routes around a resting account.
      */}
      {rests.map((rest) => (
        <RestChip
          key={`${rest.scope} ${rest.class}`}
          rest={rest}
          className="max-w-72"
          clearing={clearRest.isPending && clearRest.variables === rest.scope}
          onClear={() => clearRest.mutate(rest.scope)}
        />
      ))}
      {/*
        The names only, so the row never carries a credential. A long block
        truncates rather than pushing the row's buttons onto another line; the
        whole list is on the tooltip, and `edit` reads the values back for the
        account it is editing.
      */}
      {envKeys.length > 0 && (
        <span className="max-w-56 truncate font-mono text-2xs text-faint" title={envKeys.join(", ")}>
          {envKeys.join(", ")}
        </span>
      )}
      {account.email && (
        <span className="max-w-48 truncate font-mono text-2xs text-faint" title={account.email}>
          {account.email}
        </span>
      )}
      {usage === "ready" && (
        <SettingsNote tone="ok" className="flex items-center gap-0.5">
          <CheckIcon className="size-3" />
          {t("usage enabled")}
        </SettingsNote>
      )}
      {usage === "unavailable" && (
        <SettingsNote tone="faint" title={t("the last quota probe failed; it will be retried")}>
          {t("usage unavailable")}
        </SettingsNote>
      )}
      {usage === "login_expired" &&
        (notice === "login_expired_idle" ? (
          <SettingsNote tone="faint">{t("usage login expired · only affects the idle-time fallback")}</SettingsNote>
        ) : (
          <SettingsNote tone="warn">{t("usage login expired")}</SettingsNote>
        ))}
      {(usage === "login_needed" || usage === "login_expired") && (
        <Button
          variant="ghost"
          onClick={onEnableUsage}
          title={t("log this account in to /usage so its quota shows up")}
          className="h-auto px-1.5 py-0.5 text-xs text-fg"
        >
          {usage === "login_expired" ? t("log in to usage again") : t("enable usage")}
        </Button>
      )}
      <div className="ml-auto flex max-w-full flex-wrap items-center justify-end gap-1">
        {accountRotationToggleShown(account) && (
          <CheckboxField
            checked={rotating ?? false}
            disabled={rotate.isPending}
            onCheckedChange={(checked) => rotate.mutate(checked)}
            className={cn("mr-1 shrink-0 items-center text-xs", rotate.isPending && "cursor-not-allowed opacity-60")}
            title={t("in the pool, a session moves to another pool account when its account hits a limit, and sessions from other pool accounts may move here; off keeps this account out in both directions")}
            labelClassName="text-xs text-muted"
            label={t("auto-rotation")}
          />
        )}
        {/*
          An env account's block is editable in place; an OAuth one has a
          credential to replace, not fields to correct.
        */}
        <Button
          variant="ghost"
          onClick={onReauth}
          title={
            account.kind === "env"
              ? t("edit this account's environment variables")
              : t("replace this account's credential, keeping its sessions")
          }
          className="h-auto gap-1 px-1.5 py-0.5 font-mono text-2xs"
        >
          {account.kind === "env" ? <PencilSimpleIcon className="size-3" /> : <ArrowsClockwiseIcon className="size-3" />}
          {account.kind === "env" ? t("edit") : t("re-auth")}
        </Button>
        {confirming ? (
          <>
            <SettingsNote tone="warn">{t("remove this account?")}</SettingsNote>
            <Button
              variant="ghost"
              disabled={remove.isPending}
              onClick={() => remove.mutate()}
              data-signal="failed"
              className="h-auto px-1.5 py-0.5 text-xs text-danger hover:bg-hover hover:text-danger"
            >
              {remove.isPending ? t("removing…") : t("remove")}
            </Button>
            <Button
              variant="ghost"
              onClick={() => setConfirming(false)}
              className="h-auto px-1.5 py-0.5 font-mono text-2xs"
            >
              {t("cancel")}
            </Button>
          </>
        ) : (
          <Button
            variant="ghost"
            title={t("delete account")}
            aria-label={t("delete account")}
            onClick={() => setConfirming(true)}
            data-signal="failed"
            className="size-6 p-0 hover:text-danger"
          >
            <TrashIcon className="size-3.5" />
          </Button>
        )}
      </div>
      {remove.isError && (
        <SettingsNote tone="danger" className="w-full">{remove.error.message}</SettingsNote>
      )}
      {clearRest.isError && (
        <SettingsNote tone="danger" className="w-full">{clearRest.error.message}</SettingsNote>
      )}
      {rotate.isError && (
        <SettingsNote tone="danger" className="w-full">{rotate.error.message}</SettingsNote>
      )}
    </div>
  );
}

/**
 * How a credential gets here, which is the first thing the form asks after the
 * CLI. The wizard is not a kind — it is the OAuth mode driven from this app
 * instead of from a token the user minted elsewhere.
 */
type AddMode = "oauth_wizard" | "oauth_paste" | "env";

const MODE_KEY: Record<AddMode, string> = {
  oauth_wizard: "OAuth authorization (setup-token wizard)",
  oauth_paste: "paste an OAuth token",
  env: "local token (env)",
};

/**
 * CLI, then mode, then the credential itself. The secret is write-only: it is
 * sent to the server and never read back.
 */
function AddAccountForm({
  provider,
  onProviderChange,
  onRunWizard,
  onDone,
}: {
  provider: AccountProvider;
  onProviderChange: (provider: AccountProvider) => void;
  /** The OAuth mode hands over to the setup-token dialog. */
  onRunWizard: () => void;
  onDone: () => void;
}) {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [label, setLabel] = useState("");
  const [mode, setMode] = useState<AddMode>("oauth_wizard");
  const [secret, setSecret] = useState("");
  const [env, setEnv] = useState("");
  const [unverified, setUnverified] = useState<string | null>(null);

  // Codex has no OAuth flow here, so its form has no mode question to ask.
  const effectiveMode: AddMode = provider === "codex" ? "env" : mode;
  const kind: AccountKind = effectiveMode === "oauth_paste" ? "oauth_token" : "env";
  // Parsed on every keystroke: the server applies the same rules, and a line
  // number is only useful while the line is still on screen.
  const block = parseEnvBlock(env, provider);

  const add = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/accounts", {
        body: {
          label: label.trim(),
          provider,
          kind,
          // One or the other, never both: the block is the credential for an
          // env account, the token is for an OAuth one.
          ...(kind === "env" ? { env } : { secret: secret.trim() }),
        },
      });
      if (res.error) {
        if (res.response.status === 409) {
          throw new DuplicateAccountError((res.error as AccountConflict).existingLabel);
        }
        throw new Error(res.error.error);
      }
      return res.data;
    },
    onSuccess: (data) => {
      // Never keep the secret around once the server has it.
      setSecret("");
      setEnv("");
      setLabel("");
      void queryClient.invalidateQueries({ queryKey: accountsKey(machine) });
      void queryClient.invalidateQueries({ queryKey: quotaKey(machine) });
      // The account is in the list above either way; the form stays open only
      // to say that nothing proved the credential works.
      if (data?.unverified) {
        setUnverified(data.unverified);
        return;
      }
      onDone();
    },
  });

  const credentialReady = kind === "env" ? block.issue === null : secret.trim() !== "";
  const ready = label.trim() !== "" && credentialReady && !add.isPending;

  return (
    <SettingsColumn className="mt-2 flex flex-col gap-2 border border-border bg-panel p-2">
      <div className="flex flex-col gap-1">
        <span className="text-xs text-muted">{t("cli")}</span>
        <Tabs
          value={provider}
          onValueChange={(value) => {
            onProviderChange(value as AccountProvider);
            setUnverified(null);
          }}
        >
          <TabsList className="w-full">
            <TabsTab value="claude">claude</TabsTab>
            <TabsTab value="codex">codex</TabsTab>
          </TabsList>
        </Tabs>
      </div>
      <SettingField label={t("how it authenticates")}>
        {provider === "claude" ? (
          <SelectField<AddMode>
            value={mode}
            onValueChange={setMode}
            options={(["oauth_wizard", "oauth_paste", "env"] as AddMode[]).map((m) => ({ value: m, label: t(MODE_KEY[m]) }))}
          />
        ) : (
          <span className="font-mono text-2xs text-muted">{t(MODE_KEY.env)}</span>
        )}
      </SettingField>

      {effectiveMode === "oauth_wizard" ? (
        <>
          <SettingsHint>
            {t(
              "Runs `claude setup-token` in a terminal here. Nothing opens a browser: copy the URL it prints, authorize it yourself, and paste the code back — the token is then stored as a new account.",
            )}
          </SettingsHint>
          <div className="flex items-center gap-1.5">
            <Button variant="primary" onClick={onRunWizard}>
              {t("run setup-token wizard")}
            </Button>
            <Button onClick={onDone}>{t("cancel")}</Button>
          </div>
        </>
      ) : (
        <>
          <SettingField label={t("label")}>
            <Input
              autoFocus
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder={t("work account")}
            />
          </SettingField>
          {kind === "env" ? (
            <EnvBlockField
              provider={provider}
              value={env}
              onChange={setEnv}
              issue={block.issue}
              warning={block.warning}
            />
          ) : (
            <SettingField label={t("oauth token")} hint={<>· {t("stored on the server, never shown again")}</>}>
              <Input
                type="password"
                autoComplete="off"
                value={secret}
                onChange={(e) => setSecret(e.target.value)}
                placeholder="sk-ant-oat…"
              />
            </SettingField>
          )}
          {unverified && (
            <SettingsNote tone="warn">
              {t("saved, but it could not be checked: {{why}}", { why: unverified })}
            </SettingsNote>
          )}
          {add.isError && (
            <SettingsNote tone="danger">
              {add.error instanceof DuplicateAccountError
                ? t("Already registered as {{label}}", { label: add.error.existingLabel })
                : add.error.message}
            </SettingsNote>
          )}
          <div className="flex items-center gap-1.5">
            <Button variant="primary" disabled={!ready} onClick={() => add.mutate()}>
              {add.isPending && <CircleNotchIcon className="size-3 animate-spin" />}
              {t("add account")}
            </Button>
            <Button onClick={onDone}>{t("cancel")}</Button>
          </div>
        </>
      )}
    </SettingsColumn>
  );
}

/**
 * One CLI's accounts and the one its new sessions start under.
 *
 * Each CLI keeps its own default, so each group carries its own radio set: the
 * local-login row and one row per account, all named after the provider so the
 * browser never treats the two groups as one choice.
 */
function ProviderGroup({
  provider,
  accounts,
  defaultId,
  local,
  localDir,
  usageFor,
  onMakeDefault,
  onEnableUsage,
  onReauth,
}: {
  provider: AccountProvider;
  accounts: Account[];
  defaultId: string | null;
  /** The CLI's own login: whether it is signed out, and whether somebody chose it as the default. */
  local: LocalLogin | null;
  /** The CLI's own config dir, when the server reported one. */
  localDir: string | undefined;
  /** Reads an account's usage state, and the line it earns, from the shared quota cache. */
  usageFor: (account: Account) => { usage: AccountUsageState; notice: UsageNotice };
  /** `null` with `automatic` clears the default; `null` alone chooses the local login. */
  onMakeDefault: (accountId: string | null, automatic?: boolean) => void;
  onEnableUsage: (account: Account) => void;
  onReauth: (account: Account) => void;
}) {
  const { t } = useTranslation();
  const radioName = `rh-default-account-${provider}`;
  const signedOut = localLoginSignedOut(local);
  // Three answers to "where do new sessions start", on a server that tells a
  // chosen local login from no choice (`isDefault`) and with accounts to stand
  // in: an account; always the local login; or automatic — the local login,
  // and while it is signed out an account in the auto-rotation pool. With no
  // accounts, or on an older server, the last two are the same thing.
  const automaticOffered = typeof local?.isDefault === "boolean" && accounts.length > 0;
  const automatic = automaticOffered && defaultId === null && !local?.isDefault;
  const localChosen = defaultId === null && !automatic;
  // Where a new composer of this CLI would start now: what the note names.
  const { start } = useComposerStart(provider);
  const standIn = automatic && start?.reason === "fallback" ? accounts.find((a) => a.id === start.accountId) : undefined;
  const noStandIn = automatic && start?.reason === "local_signed_out";

  return (
    <div className="mt-4">
      <div className="-mx-3 flex h-9 items-center gap-2 border-y border-border px-3">
        <span data-label="" className="label-caps text-2xs text-fg">{PROVIDER_NAME[provider]}</span>
        <span className="font-mono text-2xs text-faint">{t("accounts")}</span>
        <span className="font-mono text-2xs tabular-nums text-faint">{accounts.length + 1}</span>
      </div>
      {automaticOffered && (
        <label className="flex h-9 items-center gap-1.5 border-b border-border">
          <input
            type="radio"
            name={radioName}
            data-default-choice="automatic"
            checked={automatic}
            onChange={() => onMakeDefault(null, true)}
            className="size-3 accent-accent"
          />
          <span className="font-mono text-xs text-fg">{t("automatic")}</span>
          <span className="truncate font-mono text-2xs text-faint">
            {t("your own login; an account in auto-rotation while it is signed out")}
          </span>
        </label>
      )}
      {automatic && signedOut && (
        <p data-signal="warn" data-local-signed-out="" className="border-b border-border py-1.5 font-mono text-2xs text-warn-fg">
          {standIn ? (
            provider === "codex"
              ? t("your own codex login on this device was refused, so new sessions that name no account start on {{label}}; after `codex login`, run one chat or session on the local login to switch back", { label: standIn.label })
              : t("your own claude login on this device is signed out, so new sessions that name no account start on {{label}} until you sign in again", { label: standIn.label })
          ) : (
            <>
              {localLoginHint(provider, t)}
              {noStandIn && <>{" — "}{t("and no account in auto-rotation is free to take its place")}</>}
            </>
          )}
        </p>
      )}
      <label className="flex h-9 items-center gap-1.5 border-b border-border">
        <input
          type="radio"
          name={radioName}
          data-default-choice="local"
          checked={localChosen}
          onChange={() => onMakeDefault(null)}
          className="size-3 accent-accent"
        />
        <span className="font-mono text-xs text-fg">{localLoginLabel(local, t)}</span>
        <span className="truncate font-mono text-2xs text-faint">
          {localDir ?? (provider === "codex" ? t("your own codex login") : t("your own claude login"))}
        </span>
      </label>
      {localChosen && signedOut && (
        <p data-signal="warn" data-local-signed-out="" className="border-b border-border py-1.5 font-mono text-2xs text-warn-fg">
          {localLoginHint(provider, t)}
        </p>
      )}
      {accounts.map((account) => (
        <AccountRow
          key={account.id}
          account={account}
          provider={provider}
          radioName={radioName}
          {...usageFor(account)}
          isDefault={defaultId === account.id}
          onMakeDefault={() => onMakeDefault(account.id)}
          onEnableUsage={() => onEnableUsage(account)}
          onReauth={() => onReauth(account)}
        />
      ))}
      {accounts.length === 0 && <p className="py-2 text-xs text-muted">{t("no accounts yet")}</p>}
    </div>
  );
}

/**
 * One CLI's accounts remote-hand can run sessions under — the Claude Code or
 * the Codex tab of Settings › Agents & accounts, its rows as they always were.
 * Secrets are write-only: they go to the server on add and are never read
 * back into the browser.
 */
export function AccountsSection({ provider }: { provider: AccountProvider }) {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data, isPending } = useAccounts();
  const { data: settings } = useQuery({
    queryKey: mq(machine, "settings"),
    queryFn: async () => (await api.GET("/api/settings")).data,
    staleTime: 5 * 60_000,
  });
  // Shares the composer's cache; it is the only place that knows whether an
  // account's /usage login actually works.
  const { data: quota } = useQuota();
  const [adding, setAdding] = useState(false);
  // Which CLI the open form is about: the tab's, unless switched in the form.
  const [addProvider, setAddProvider] = useState<AccountProvider>(provider);
  const [wizardOpen, setWizardOpen] = useState(false);
  const [usageFor, setUsageFor] = useState<Account | null>(null);
  const [reauthFor, setReauthFor] = useState<Account | null>(null);

  const setDefault = useMutation({
    mutationFn: async ({ accountId, provider, automatic }: { accountId: string | null; provider: AccountProvider; automatic?: boolean }) => {
      const res = await api.PUT("/api/accounts/default", { body: { accountId, provider, ...(automatic ? { automatic } : {}) } });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: (list) => {
      if (list) queryClient.setQueryData<AccountList>(accountsKey(machine), list);
    },
  });

  const localDir: Record<AccountProvider, string | undefined> = {
    claude: settings?.claudeDir,
    codex: settings?.codexDir,
  };

  return (
    <div data-tab-panel={provider} className="p-3">
      {provider === "claude" && (
        <SettingsColumn>
          <SettingsHint>
            {t(
              "Sessions started under an account use Droppen's own Claude config dir, so they never touch your local login. Sessions you start from a terminal keep using the local login.",
            )}
          </SettingsHint>
          <SettingsHint className="mt-1">
            {t("A local token is handed to the CLI through its environment instead, for the session it starts.")}
          </SettingsHint>
          {settings?.claudeManagedDir && (
            <p className="mt-1 font-mono text-xs text-muted">
              {t("managed config dir")}: <span className="break-all text-muted">{settings.claudeManagedDir}</span>
            </p>
          )}
        </SettingsColumn>
      )}

      <div>
        {isPending && <Spinner />}
        {data && (
          <ProviderGroup
            provider={provider}
            accounts={accountsForProvider(data.accounts, provider)}
            defaultId={
              // `?? null` for the refresh where a new web build is talking
              // to a server that has no Codex default yet.
              provider === "codex" ? (data.defaultCodexAccountId ?? null) : data.defaultAccountId
            }
            local={localLoginOf(data, provider)}
            localDir={localDir[provider]}
            usageFor={(account) => {
              const q = quota?.accounts.find((a) => a.id === account.id);
              const usage = accountUsageState(account.kind, account.usageEnabled, q?.error);
              return { usage, notice: usageNotice(usage, q?.updatedAt, (q?.windows.length ?? 0) > 0) };
            }}
            onMakeDefault={(accountId, automatic) => setDefault.mutate({ accountId, provider, automatic })}
            onEnableUsage={setUsageFor}
            onReauth={setReauthFor}
          />
        )}
        {setDefault.isError && (
          <SettingsNote as="p" tone="danger" className="mt-1">{setDefault.error.message}</SettingsNote>
        )}
      </div>

      <SettingsColumn className="mt-3">
        {adding ? (
          <AddAccountForm
            provider={addProvider}
            onProviderChange={setAddProvider}
            onRunWizard={() => setWizardOpen(true)}
            onDone={() => setAdding(false)}
          />
        ) : (
          <Button variant="primary" onClick={() => setAdding(true)}>
            <PlusIcon className="size-3" />
            {t("add account")}
          </Button>
        )}
      </SettingsColumn>

      {/* The raw limit messages the server recorded, for whoever wants the CLIs' exact wording. */}
      {data && <LimitEventsPanel accounts={data.accounts} />}

      <SetupTokenDialog
        open={wizardOpen}
        onOpenChange={(open) => {
          setWizardOpen(open);
          // The wizard stores the account itself, so the form behind it has
          // nothing left to ask once it closes.
          if (!open) setAdding(false);
        }}
      />
      <UsageLoginDialog account={usageFor} onOpenChange={(open) => !open && setUsageFor(null)} />
      <ReauthDialog account={reauthFor} onOpenChange={(open) => !open && setReauthFor(null)} />
    </div>
  );
}
