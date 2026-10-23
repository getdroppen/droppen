import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, Loader2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { components } from "@/api/schema";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { EnvBlockField } from "@/features/accounts/env-block-field";
import { parseEnvBlock } from "@/features/accounts/env-block";
import {
  accountsKey,
  accountProvider,
  quotaKey,
  useAccounts,
  type Account,
} from "@/features/accounts/use-accounts";
import { XtermPane } from "@/features/terminals/lazy-xterm-pane";
import { cn } from "@/lib/cn";
import { useApi, useMachine } from "@/lib/machines/context";
import { mq } from "@/lib/machines/query";

type SetupTokenStatus = components["schemas"]["SetupTokenStatus"];
type SetupTokenStarted = components["schemas"]["SetupTokenStarted"];

const WIDE = "w-[min(48rem,calc(100vw-2rem))]";
const TERMINAL_BOX = "h-[min(50vh,22rem)] overflow-hidden rounded border border-border";

const field =
  "h-6.5 w-full rounded border border-border bg-bg px-2 font-mono text-2xs text-fg placeholder:text-faint focus:border-fg focus:outline-none";
const primary =
  "flex h-7 items-center justify-center gap-1.5 bg-fg px-3 text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40";
const secondary =
  "flex h-7 items-center justify-center gap-1.5 rounded border border-border px-3 font-mono text-2xs text-muted hover:bg-hover hover:text-fg disabled:opacity-40";

/** The URL to authorize, as text to copy. Nothing here opens a browser. */
function AuthUrlBlock({ url }: { url: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // No clipboard permission (or no secure context): the field below is
      // selectable, which is the fallback that always works.
    }
  };

  return (
    <div className="flex flex-col gap-1">
      <span className="text-2xs text-faint">{t("open this url yourself to authorize")}</span>
      <div className="flex items-center gap-1.5">
        <input readOnly value={url} onFocus={(e) => e.currentTarget.select()} className={cn(field, "flex-1")} />
        <button type="button" onClick={() => void copy()} className={cn(secondary, "shrink-0 px-2")}>
          {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
          {copied ? t("copied") : t("copy")}
        </button>
      </div>
    </div>
  );
}

/** The code the authorization page showed, typed into the wizard for the user. */
function CodeBox({ captureId }: { captureId: string }) {
  const api = useApi();
  const { t } = useTranslation();
  const [code, setCode] = useState("");
  const [sent, setSent] = useState(false);

  const submit = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/accounts/setup-token/{captureId}/code", {
        params: { path: { captureId } },
        body: { code: code.trim() },
      });
      if (res.error) throw new Error(res.error.error);
    },
    onSuccess: () => {
      // Never keep it around: it is single-use, and the server has it now.
      setCode("");
      setSent(true);
    },
  });

  return (
    <div className="flex flex-col gap-1">
      <span className="text-2xs text-faint">{t("paste the code the page gave you")}</span>
      <div className="flex items-center gap-1.5">
        <input
          value={code}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => {
            setCode(e.target.value);
            setSent(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && code.trim() !== "") submit.mutate();
          }}
          placeholder={t("authorization code")}
          className={cn(field, "flex-1")}
        />
        <button
          type="button"
          disabled={code.trim() === "" || submit.isPending}
          onClick={() => submit.mutate()}
          className={cn(primary, "shrink-0")}
        >
          {submit.isPending && <Loader2 className="size-3 animate-spin" />}
          {t("submit code")}
        </button>
      </div>
      {submit.isError && <span data-signal="failed" className="font-mono text-2xs text-danger">{submit.error.message}</span>}
      {sent && !submit.isError && <span className="font-mono text-2xs text-faint">{t("code sent — waiting for the token…")}</span>}
    </div>
  );
}

/**
 * A running capture: the URL to copy, the code to paste back, and what became
 * of the token.
 *
 * The wizard's terminal is still there and still attachable, but it is no longer
 * how the flow is driven — it runs at a fixed width so nothing it prints gets
 * wrapped, which makes it awkward to read in a narrow pane. So it is folded away
 * behind a disclosure, for when something has gone wrong and the output matters.
 */
function CapturePanel({
  capture,
  onCaptured,
  onRestart,
  reusedLabel,
  capturedText,
}: {
  capture: SetupTokenStarted;
  /** The capture stored a token; `accountId` names where it landed. */
  onCaptured: (status: SetupTokenStatus) => void;
  /** Start over after the CLI gave up. */
  onRestart: () => void;
  /** Names the account a token already on file belongs to. */
  reusedLabel?: (accountId: string | undefined) => string | undefined;
  /** What to say when a token lands: an account was added, or replaced. */
  capturedText: string;
}) {
  const api = useApi();
  const machine = useMachine();
  const { t } = useTranslation();
  const [showTerminal, setShowTerminal] = useState(false);
  const handled = useRef(false);

  const { data: status } = useQuery({
    queryKey: mq(machine, "setup-token", capture.captureId),
    queryFn: async () => {
      const res = await api.GET("/api/accounts/setup-token/{captureId}", {
        params: { path: { captureId: capture.captureId } },
      });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    // Polling stops the moment the capture settles, which also keeps the
    // "captured" result stable for the effect below.
    refetchInterval: (query) => (query.state.data?.status === "pending" ? 1500 : false),
  });

  useEffect(() => {
    if (status?.status !== "captured" || handled.current) return;
    handled.current = true;
    onCaptured(status);
  }, [status, onCaptured]);

  const pending = status?.status === "pending" || status === undefined;
  // A reused token minted nothing new: the CLI handed back one an account
  // already holds, so name that account rather than claim an add.
  const already = status?.reused ? reusedLabel?.(status.accountId) : undefined;

  return (
    <div className="flex flex-col gap-2">
      {status?.authUrl && <AuthUrlBlock url={status.authUrl} />}
      {pending && status?.authUrl && <CodeBox captureId={capture.captureId} />}
      {pending && !status?.authUrl && (
        <div className="flex items-center gap-1.5 font-mono text-2xs text-faint">
          <Loader2 className="size-3 animate-spin" />
          {t("waiting for the authorization url…")}
        </div>
      )}
      {status?.status === "captured" && (
        <div className={cn("flex items-center gap-1.5 font-mono text-2xs", status.reused ? "text-muted" : "text-ok")}>
          <Check className="size-3.5" />
          {!status.reused
            ? capturedText
            : already
              ? t("Token already registered as {{label}}", { label: already })
              : t("Token already registered")}
        </div>
      )}
      {status?.unverified && (
        <span data-signal="warn" className="font-mono text-2xs text-warn-fg">
          {t("saved, but it could not be checked: {{why}}", { why: status.unverified })}
        </span>
      )}
      {(status?.status === "exited" || status?.status === "error") && (
        <div className="flex flex-wrap items-center gap-2">
          <span data-signal="warn" className="font-mono text-2xs text-warn-fg">
            {status.error ?? t("the wizard exited before a token appeared — try again")}
            {/* -1 is the wizard being killed rather than exiting itself. */}
            {status.exitCode !== undefined && ` (exit ${status.exitCode})`}
          </span>
          <button type="button" onClick={onRestart} className={secondary}>
            {t("try again")}
          </button>
        </div>
      )}
      <button
        type="button"
        onClick={() => setShowTerminal((on) => !on)}
        className="self-start font-mono text-2xs text-faint hover:text-muted"
      >
        {showTerminal ? t("hide the wizard's terminal") : t("show the wizard's terminal")}
      </button>
      {showTerminal && (
        <div className={TERMINAL_BOX}>
          <XtermPane id={capture.terminalId} visible />
        </div>
      )}
    </div>
  );
}

/**
 * Both wizard flows end the same way: re-read the account list and the quota
 * that hangs off it, then let the result stand for a moment before closing.
 */
function useCaptureSettled(onDone: () => void) {
  const machine = useMachine();
  const queryClient = useQueryClient();
  // `null` re-reads the caches but leaves the dialog open, for the one outcome
  // the user has to actually read: stored without anything vouching for it.
  return (delay: number | null = 1500) => {
    void queryClient.invalidateQueries({ queryKey: accountsKey(machine) });
    void queryClient.invalidateQueries({ queryKey: quotaKey(machine) });
    if (delay !== null) setTimeout(onDone, delay);
  };
}

/**
 * `claude setup-token` in a terminal here: the CLI prints an authorization URL,
 * which is shown as text to copy — nothing opens a browser on the machine
 * running remote-hand, which for a phone on the other end of it would be a
 * window nobody is looking at. The code comes back through this dialog, and the
 * server lifts the token off the stream. The browser only ever sees the
 * capture's status.
 */
export function SetupTokenDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const api = useApi();
  const { t } = useTranslation();
  const [label, setLabel] = useState("");
  const [capture, setCapture] = useState<SetupTokenStarted | null>(null);
  // Refetched by the invalidation below, so a reused token can be named.
  const { data: accountList } = useAccounts();
  const settled = useCaptureSettled(() => onOpenChange(false));

  useEffect(() => {
    if (!open) return;
    setLabel("");
    setCapture(null);
  }, [open]);

  const start = useMutation({
    mutationFn: async () => {
      const res = await api.POST("/api/accounts/setup-token", { body: { label: label.trim() } });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: (data) => setCapture(data),
  });

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={t("run setup-token wizard")} className={capture ? WIDE : undefined}>
        {!capture ? (
          <div className="flex flex-col gap-2">
            <p className="text-2xs text-faint">
              {t(
                "Runs `claude setup-token` in a terminal here. Nothing opens a browser: copy the URL it prints, authorize it yourself, and paste the code back — the token is then stored as a new account.",
              )}
            </p>
            <label className="flex flex-col gap-1">
              <span className="text-2xs text-faint">{t("label")}</span>
              <input
                autoFocus
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                placeholder={t("work account")}
                className={field}
              />
            </label>
            {start.isError && <span data-signal="failed" className="font-mono text-2xs text-danger">{start.error.message}</span>}
            <button
              type="button"
              disabled={label.trim() === "" || start.isPending}
              onClick={() => start.mutate()}
              className={primary}
            >
              {start.isPending && <Loader2 className="size-3 animate-spin" />}
              {t("start wizard")}
            </button>
          </div>
        ) : (
          <CapturePanel
            capture={capture}
            // A reused token minted nothing new: the CLI handed back a token an
            // account already holds, so name that account instead of claiming an
            // add. The list is refetched by the invalidation, so by the time this
            // renders the label is there.
            reusedLabel={(id) => accountList?.accounts.find((a) => a.id === id)?.label}
            capturedText={t("token captured · account added")}
            // A token nothing could vouch for is the one outcome worth reading,
            // so the dialog stays open on it instead of closing on a tick.
            onCaptured={(status) => settled(status.unverified ? null : 1500)}
            onRestart={() => setCapture(null)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * A new credential for an account that already exists.
 *
 * The account keeps its id, and with it every session bound to it — which is
 * the point: an account whose quota ran out is re-authorized and the sessions
 * running under it carry on, rather than being re-pointed one at a time.
 *
 * Two ways in, because there are two ways to hold a credential: run the OAuth
 * wizard here, or paste one in. An env account — either CLI's — has no OAuth
 * flow at all, so for those the block is the only offer. The block is read
 * back from the server first, so this opens on what is stored: a value can be
 * corrected without every other line being retyped. Saving still replaces the
 * whole block, which is what the box says.
 */
export function ReauthDialog({
  account,
  onOpenChange,
}: {
  /** The account to re-authorize or edit; null keeps the dialog closed. */
  account: Account | null;
  onOpenChange: (open: boolean) => void;
}) {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [capture, setCapture] = useState<SetupTokenStarted | null>(null);
  const [secret, setSecret] = useState("");
  // `null` until the stored block arrives: the box must not open empty and
  // then fill under somebody who has started typing.
  const [env, setEnv] = useState<string | null>(null);
  const [unverified, setUnverified] = useState<string | null>(null);
  const settled = useCaptureSettled(() => onOpenChange(false));
  const provider = account ? accountProvider(account) : "claude";
  const kind = account?.kind ?? "oauth_token";
  const oauth = kind === "oauth_token";
  // The list carries the names only; this is the one read that brings the
  // values back, and only for the account being edited.
  const stored = useQuery({
    queryKey: mq(machine, "account-env", account?.id ?? ""),
    queryFn: async () => {
      const res = await api.GET("/api/accounts/{id}/env", { params: { path: { id: account!.id } } });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    enabled: account !== null && !oauth,
  });

  useEffect(() => {
    setCapture(null);
    setSecret("");
    setEnv(null);
    setUnverified(null);
  }, [account]);

  useEffect(() => {
    // Filling it again when the same account is reopened is wanted; filling it
    // over edits in progress is not, so this follows the account object and
    // the data it read, not every refetch.
    if (!oauth && stored.data) setEnv(stored.data.env ?? "");
  }, [account, oauth, stored.data]);

  const block = parseEnvBlock(env ?? "", provider);

  const start = useMutation({
    mutationFn: async (id: string) => {
      const res = await api.POST("/api/accounts/{id}/setup-token", { params: { path: { id } } });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: (data) => setCapture(data),
  });

  const paste = useMutation({
    mutationFn: async (id: string) => {
      const res = await api.PATCH("/api/accounts/{id}", {
        params: { path: { id } },
        // The block replaces the whole previous one; an OAuth account takes a
        // token instead. Never both.
        body: oauth ? { secret: secret.trim() } : { env: env ?? "" },
      });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: (data) => {
      // Never keep the token around once the server has it. The block stays
      // on screen: it is what was just saved, and the dialog may stay open to
      // report an unverified save.
      setSecret("");
      // Opening the editor again shows the saved text, not the pre-edit one.
      if (!oauth && env !== null) queryClient.setQueryData(mq(machine, "account-env", account?.id ?? ""), { env });
      // Stored, but no check ran: say so and stay open rather than close on a
      // green tick the server never earned.
      setUnverified(data?.unverified ?? null);
      settled(data?.unverified ? null : 600);
    },
  });

  return (
    <Dialog open={account !== null} onOpenChange={onOpenChange}>
      <DialogContent title={t(oauth ? "re-authorize account" : "edit environment variables")} className={capture ? WIDE : undefined}>
        {capture ? (
          <CapturePanel
            capture={capture}
            capturedText={t("token captured · account updated")}
            onCaptured={(status) => settled(status.unverified ? null : 1500)}
            onRestart={() => setCapture(null)}
          />
        ) : (
          <div className="flex flex-col gap-2">
            <p className="text-2xs text-faint">
              {t(
                oauth
                  ? "Replaces this account's credential in place. The account keeps its id, so every session already running under it carries on."
                  : "Shown as stored. Saving replaces the whole block; the account keeps its id, so every session already running under it carries on.",
              )}
            </p>
            {oauth && (
              <>
                {start.isError && <span data-signal="failed" className="font-mono text-2xs text-danger">{start.error.message}</span>}
                <button
                  type="button"
                  disabled={!account || start.isPending}
                  onClick={() => account && start.mutate(account.id)}
                  className={primary}
                >
                  {start.isPending && <Loader2 className="size-3 animate-spin" />}
                  {t("run setup-token wizard")}
                </button>
                <div className="text-center font-mono text-2xs text-faint">{t("or")}</div>
              </>
            )}
            {oauth ? (
              <label className="flex flex-col gap-1">
                <span className="text-2xs text-faint">
                  {t("oauth token")}
                  {" · "}
                  {t("checked against Anthropic before it is stored")}
                </span>
                <input
                  type="password"
                  autoComplete="off"
                  value={secret}
                  onChange={(e) => setSecret(e.target.value)}
                  placeholder="sk-ant-oat…"
                  className={field}
                />
              </label>
            ) : stored.isPending ? (
              <div className="flex items-center gap-1.5 font-mono text-2xs text-faint">
                <Loader2 className="size-3 animate-spin" />
                {t("reading this account's environment variables…")}
              </div>
            ) : stored.isError ? (
              <div className="flex flex-wrap items-center gap-2">
                <span data-signal="failed" className="font-mono text-2xs text-danger">{stored.error.message}</span>
                <button type="button" onClick={() => void stored.refetch()} className={secondary}>
                  {t("try again")}
                </button>
              </div>
            ) : (
              <EnvBlockField
                provider={provider}
                value={env ?? ""}
                onChange={setEnv}
                issue={block.issue}
                warning={block.warning}
                hint={t("this is the block as stored; saving replaces it whole")}
              />
            )}
            {paste.isError && <span data-signal="failed" className="font-mono text-2xs text-danger">{paste.error.message}</span>}
            {unverified && (
              <span data-signal="warn" className="font-mono text-2xs text-warn-fg">
                {t("saved, but it could not be checked: {{why}}", { why: unverified })}
              </span>
            )}
            <button
              type="button"
              disabled={!account || (oauth ? secret.trim() === "" : env === null || block.issue !== null) || stored.isError || paste.isPending}
              onClick={() => account && paste.mutate(account.id)}
              className={oauth ? secondary : primary}
            >
              {paste.isPending && <Loader2 className="size-3 animate-spin" />}
              {paste.isPending ? t("checking the token…") : t(oauth ? "save token" : "save environment variables")}
            </button>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * `claude auth login` into the account's own probe config dir, which is what
 * the quota probe later reads. The token from setup-token cannot do this: its
 * scope covers inference only.
 */
export function UsageLoginDialog({
  account,
  onOpenChange,
}: {
  /** The account to log in; null keeps the dialog closed. */
  account: Account | null;
  onOpenChange: (open: boolean) => void;
}) {
  const machine = useMachine();
  const api = useApi();
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [terminalId, setTerminalId] = useState<string | null>(null);
  const [notYet, setNotYet] = useState(false);
  const startedFor = useRef<string | null>(null);

  const start = useMutation({
    mutationFn: async (id: string) => {
      const res = await api.POST("/api/accounts/{id}/usage-login", { params: { path: { id } } });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: (data) => setTerminalId(data.terminalId),
  });

  const verify = useMutation({
    mutationFn: async (id: string) => {
      const res = await api.POST("/api/accounts/{id}/usage-verify", { params: { path: { id } } });
      if (res.error) throw new Error(res.error.error);
      return res.data;
    },
    onSuccess: (data) => {
      if (!data.loggedIn) {
        setNotYet(true);
        return;
      }
      void queryClient.invalidateQueries({ queryKey: accountsKey(machine) });
      void queryClient.invalidateQueries({ queryKey: quotaKey(machine) });
      onOpenChange(false);
    },
  });

  // One wizard per account; reopening for another account restarts it.
  useEffect(() => {
    if (!account) {
      startedFor.current = null;
      setTerminalId(null);
      setNotYet(false);
      return;
    }
    if (startedFor.current === account.id) return;
    startedFor.current = account.id;
    setTerminalId(null);
    setNotYet(false);
    start.mutate(account.id);
  }, [account]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <Dialog open={account !== null} onOpenChange={onOpenChange}>
      <DialogContent title={t("enable usage")} className={terminalId ? WIDE : undefined}>
        <div className="flex flex-col gap-2">
          <p className="text-2xs text-faint">
            {t(
              "Runs `claude auth login` for this account in its own config dir, so Droppen can read its quota. Press verify once the CLI reports you are logged in.",
            )}
          </p>
          {start.isError && <span data-signal="failed" className="font-mono text-2xs text-danger">{start.error.message}</span>}
          {terminalId ? (
            <div className={TERMINAL_BOX}>
              <XtermPane id={terminalId} visible />
            </div>
          ) : (
            !start.isError && (
              <div className="flex items-center gap-1.5 font-mono text-2xs text-faint">
                <Loader2 className="size-3 animate-spin" />
                {t("starting the login wizard…")}
              </div>
            )
          )}
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              disabled={!account || !terminalId || verify.isPending}
              onClick={() => account && verify.mutate(account.id)}
              className={cn(primary, "px-3")}
            >
              {verify.isPending && <Loader2 className="size-3 animate-spin" />}
              {t("verify")}
            </button>
            {notYet && (
              <span data-signal="warn" className="font-mono text-2xs text-warn-fg">
                {t("not logged in yet — finish the login in the terminal")}
              </span>
            )}
            {verify.isError && (
              <span data-signal="failed" className="font-mono text-2xs text-danger">{verify.error.message}</span>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
