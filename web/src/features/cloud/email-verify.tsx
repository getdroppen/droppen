import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Mail } from "lucide-react";
import { authErrorKind, type EmailStanding, type HubApi, HubApiError, type SignedIn, type TurnstileConfig } from "@/lib/hub/api";
import type { CloudAccount } from "@/lib/hub/account";
import { cn } from "@/lib/cn";
import { PrefsControls } from "@/components/prefs-controls";
import { publicLink, SIGN_IN_PATH } from "@/features/landing/route";
import { Turnstile } from "./turnstile";
import { authErrorText, Centered, ErrorLine, errorText, Note, Panel, primary, quiet, secondary } from "./ui";

/*
 * Verifying the account's e-mail (docs/relay-and-plans.md §4.1). Where the
 * hub requires it, a fresh account gets a link by mail and may do nothing
 * else with its login token until it is opened:
 *
 * - right after sign-up (or a sign-in that is not verified yet), "check your
 *   e-mail" asks the hub every 3 s and goes on to set up this device as soon
 *   as the link was opened — here or on any other device;
 * - the link opens `/verify#t=<token>`, the token in the fragment so no server
 *   log or Referer sees it; the page takes it out of the address at once and
 *   hands it to the hub, signed in or not;
 * - an account from before verification was required has a while to do it,
 *   and the machine list says so, quietly, until it is done.
 */

/** How often "check your e-mail" asks the hub whether the link was opened. */
const POLL_MS = 3000;

/** After a mail was sent: the hub takes one a minute. */
const RESEND_GAP_MS = 60_000;

/** `v1.<account>.<issued_ms>.<tag>`, as the hub mints it (at most 200 characters). */
const VERIFY_TOKEN = /^v1\.[\x21-\x2d\x2f-\x7e]{1,64}\.\d{1,20}\.[A-Za-z0-9_-]{1,128}$/;

/** The verification token in a link's fragment (`#t=…`), or `null` when there is none or it is not one. */
export function verifyTokenFromHash(hash: string): string | null {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  for (const part of raw.split("&")) {
    if (!part.startsWith("t=")) continue;
    let token: string;
    try {
      token = decodeURIComponent(part.slice(2)).trim();
    } catch {
      return null;
    }
    return token.length <= 200 && VERIFY_TOKEN.test(token) ? token : null;
  }
  return null;
}

/** A date as the page's language writes it (the day, not the time). */
function formatDay(ms: number, lang: string): string {
  try {
    return new Intl.DateTimeFormat(lang, { dateStyle: "medium" }).format(new Date(ms));
  } catch {
    return new Date(ms).toISOString().slice(0, 10);
  }
}

/**
 * "Send the link again": with the widget first when the hub asks for
 * Turnstile (the mail is sent as soon as it answers), then at most once a
 * minute — or after however long the hub says.
 */
export function ResendControl({
  api,
  token,
  turnstile,
  noRenew,
  compact,
  onVerified,
  onExpired,
}: {
  api: HubApi;
  /** A login or device session. */
  token: () => string;
  turnstile: TurnstileConfig | null;
  noRenew?: boolean;
  compact?: boolean;
  /** The hub says there is nothing to verify (any more). */
  onVerified?: () => void;
  /** The session is gone (a login token's 15 minutes). */
  onExpired?: () => void;
}) {
  const { t } = useTranslation();
  const [stage, setStage] = useState<"idle" | "check" | "sending" | "sent">("idle");
  const [error, setError] = useState<string | null>(null);
  const [waitUntil, setWaitUntil] = useState(0);
  const [now, setNow] = useState(() => Date.now());
  const sending = useRef(false);

  useEffect(() => {
    if (waitUntil <= now) return;
    const id = setTimeout(() => setNow(Date.now()), Math.min(1000, waitUntil - now));
    return () => clearTimeout(id);
  }, [waitUntil, now]);
  const wait = Math.max(0, Math.ceil((waitUntil - now) / 1000));

  const holdFor = (ms: number) => {
    const at = Date.now();
    setNow(at);
    setWaitUntil(at + ms);
  };

  const send = async (check: string | null) => {
    if (sending.current) return;
    sending.current = true;
    setStage("sending");
    setError(null);
    try {
      await api.resendVerification(token(), check, { noRenew });
      setStage("sent");
      holdFor(RESEND_GAP_MS);
    } catch (err) {
      const kind = authErrorKind(err);
      if (kind === "alreadyVerified" || kind === "verificationOff") {
        setStage("idle");
        onVerified?.();
        return;
      }
      if (kind === "unauthorized" && onExpired) {
        setStage("idle");
        onExpired();
        return;
      }
      if (kind === "rate" && err instanceof HubApiError && err.retryAfterMs !== undefined) holdFor(err.retryAfterMs);
      setError(authErrorText(err, "resend"));
      // A refused check: show a fresh widget to answer again.
      const recheck = turnstile && (kind === "turnstileRetry" || kind === "turnstileFailed" || kind === "turnstileRequired");
      setStage(recheck ? "check" : "idle");
    } finally {
      sending.current = false;
    }
  };

  const start = () => {
    if (wait > 0 || stage === "sending") return;
    setError(null);
    if (turnstile) setStage("check");
    else void send(null);
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <button
          type="button"
          className={compact ? cn(secondary, "h-7 px-3 text-xs") : cn(quiet, "-ml-3 self-start")}
          disabled={stage === "sending" || stage === "check" || wait > 0}
          onClick={start}
        >
          {stage === "sending" && <Loader2 className="size-3.5 animate-spin" />}
          {wait > 0 ? t("send again in {{n}} s", { n: wait }) : t("send the link again")}
        </button>
        {stage === "sent" && (
          <span data-signal="ok" className="font-mono text-2xs text-ok">
            {t("sent: check your inbox, and the spam folder")}
          </span>
        )}
      </div>
      {stage === "check" && turnstile && (
        <Turnstile
          config={turnstile}
          action="resend"
          onToken={(check) => {
            if (check) void send(check);
          }}
        />
      )}
      <ErrorLine message={error} />
    </div>
  );
}

/**
 * Signed up (or in) with an e-mail still to verify: where the link went, and
 * on by itself once it was opened. A login token lasts 15 minutes; when it is
 * gone, back to the sign-in form.
 * Exported so the screen can be previewed on its own.
 */
export function CheckEmailScreen({
  api,
  session,
  address,
  turnstile,
  onVerified,
  onBack,
}: {
  api: HubApi;
  session: SignedIn;
  /** The address as it was typed (until the hub says it). */
  address: string;
  turnstile: TurnstileConfig | null;
  onVerified: (session: SignedIn) => void;
  /** Back to the sign-in form, with what to say there. */
  onBack: (message?: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const [email, setEmail] = useState<EmailStanding | null>(null);
  const [trouble, setTrouble] = useState<string | null>(null);
  const done = useRef(false);
  const handlers = useRef({ onVerified, onBack });
  useEffect(() => {
    handlers.current = { onVerified, onBack };
  });

  const verified = useCallback(() => {
    if (done.current) return;
    done.current = true;
    handlers.current.onVerified(session);
  }, [session]);
  const expired = useCallback(() => {
    if (done.current) return;
    done.current = true;
    handlers.current.onBack(t("your sign-in expired: sign in again"));
  }, [t]);

  useEffect(() => {
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const poll = async () => {
      if (!live || done.current) return;
      if (Date.now() >= session.expiresMs) return expired();
      try {
        const me = await api.me(session.token, { noRenew: true });
        if (!live) return;
        setTrouble(null);
        setEmail(me.email);
        if (me.email.verified || me.email.verification === "off") return verified();
      } catch (err) {
        if (!live) return;
        if (err instanceof HubApiError && err.status === 401) return expired();
        setTrouble(errorText(err));
      }
      timer = setTimeout(() => void poll(), POLL_MS);
    };
    void poll();
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [api, session, verified, expired]);

  const shown = email?.address || address;
  // An account from before verification was required: it may go on, and verify by then.
  const later = email?.dueMs != null && email.dueMs > Date.now() ? email.dueMs : null;
  return (
    <Centered aside={<PrefsControls />}>
      <Panel title={t("check your e-mail")}>
        <p className="text-xs leading-5 text-muted">{t("Open the link we sent to this address; this page continues by itself.")}</p>
        <div data-testid="verify-address" className="border border-border bg-raised px-2.5 py-2 font-mono text-xs break-all text-fg">
          {shown}
        </div>
        {email?.delivery === "failed" ? (
          <Note tone="danger">
            <span data-signal="failed" className="font-mono text-xs text-danger">
              {t("the mail could not be delivered")}
            </span>
            {email.deliveryError && <span className="font-mono text-2xs break-words text-muted">{email.deliveryError}</span>}
            <p className="text-xs leading-5 text-muted">{t("Check the address, or send it again.")}</p>
          </Note>
        ) : (
          <p className="flex items-center gap-2 text-xs text-faint">
            <Loader2 className="size-3 shrink-0 animate-spin" />
            {email?.delivery === "pending" ? t("sending the mail…") : t("waiting for the link to be opened…")}
          </p>
        )}
        <ErrorLine message={trouble} />
        {later !== null && (
          <Note>
            <p className="text-xs leading-5 text-muted">{t("This account may verify its e-mail later, until {{date}}.", { date: formatDay(later, i18n.language) })}</p>
            <button type="button" className={cn(quiet, "-ml-3 self-start")} onClick={verified}>
              {t("verify later")}
            </button>
          </Note>
        )}
        <ResendControl api={api} token={() => session.token} turnstile={turnstile} noRenew onVerified={verified} onExpired={expired} />
        <div className="border-t border-border pt-3">
          <button type="button" className={cn(quiet, "-ml-3")} onClick={() => handlers.current.onBack()}>
            {t("use another account")}
          </button>
        </div>
      </Panel>
    </Centered>
  );
}

type VerifyState = { kind: "checking" } | { kind: "verified" } | { kind: "expired" } | { kind: "invalid" } | { kind: "error"; message: string };

/** What the link opened in this page load said (shown again if the page comes back without the token). */
let remembered: VerifyState | null = null;

/**
 * `/verify#t=<token>`, the link in the mail, on any device and signed in or
 * not: the token out of the address at once, then to the hub. `onContinue`
 * (signed in here): on into the app; else a way to sign in.
 */
export function VerifyEmailPage({ api, onContinue }: { api: HubApi; onContinue?: () => void }) {
  const { t } = useTranslation();
  const [state, setState] = useState<VerifyState>(() => remembered ?? { kind: "checking" });
  const token = useRef<string | null | undefined>(undefined);

  const settle = useCallback((next: VerifyState) => {
    if (next.kind !== "checking") remembered = next;
    setState(next);
  }, []);

  const run = useCallback(
    async (value: string) => {
      settle({ kind: "checking" });
      try {
        await api.verifyEmail(value);
        settle({ kind: "verified" });
      } catch (err) {
        const kind = authErrorKind(err);
        if (kind === "expired") settle({ kind: "expired" });
        else if (kind === "badToken") settle({ kind: "invalid" });
        else settle({ kind: "error", message: authErrorText(err) });
      }
    },
    [api, settle],
  );

  useEffect(() => {
    if (token.current !== undefined) return;
    token.current = verifyTokenFromHash(location.hash);
    // The token must not linger: out of the address bar (and this history entry) before anything else.
    if (location.hash) history.replaceState(history.state, "", `${location.pathname}${location.search}`);
    if (token.current) void run(token.current);
    else if (!remembered) settle({ kind: "invalid" });
  }, [run, settle]);

  const next = onContinue ? (
    <button type="button" data-variant="primary" className={cn(primary, "self-start")} onClick={onContinue}>
      {t("open the app")}
    </button>
  ) : (
    <a {...publicLink(SIGN_IN_PATH)} data-variant="primary" className={cn(primary, "self-start")}>
      {t("sign in")}
    </a>
  );

  const title =
    state.kind === "checking"
      ? t("verifying your e-mail…")
      : state.kind === "verified"
        ? t("e-mail verified")
        : state.kind === "expired"
          ? t("this link expired")
          : state.kind === "invalid"
            ? t("this link is not valid")
            : t("could not verify your e-mail");

  return (
    <Centered aside={<PrefsControls />}>
      <Panel title={title}>
        {state.kind === "checking" && (
          <p className="flex items-center gap-2 text-xs text-faint">
            <Loader2 className="size-3 shrink-0 animate-spin" />
            {t("checking the link…")}
          </p>
        )}
        {state.kind === "verified" && (
          <>
            <Note tone="ok">
              <span data-testid="email-verified" data-signal="ok" className="font-mono text-xs text-ok">
                {t("Your e-mail is verified.")}
              </span>
              <p className="text-xs leading-5 text-muted">
                {onContinue
                  ? t("Adding devices no longer waits for it.")
                  : t("Go back to the tab where you signed up — it continues by itself — or sign in here.")}
              </p>
            </Note>
            {next}
          </>
        )}
        {state.kind === "expired" && (
          <>
            <p className="text-xs leading-5 text-muted">
              {onContinue ? t("This link expired: open the app and ask for a new one.") : t("This link expired: sign in and ask for a new one.")}
            </p>
            {next}
          </>
        )}
        {state.kind === "invalid" && (
          <>
            <p className="text-xs leading-5 text-muted">{t("This link is not valid. Open the newest link we sent, or sign in and ask for a new one.")}</p>
            {next}
          </>
        )}
        {state.kind === "error" && (
          <>
            <ErrorLine message={state.message} />
            <div className="flex gap-2">
              {token.current && (
                <button type="button" data-variant="primary" className={primary} onClick={() => void run(token.current!)}>
                  {t("retry")}
                </button>
              )}
              {onContinue ? (
                <button type="button" className={quiet} onClick={onContinue}>
                  {t("open the app")}
                </button>
              ) : (
                <a {...publicLink(SIGN_IN_PATH)} className={quiet}>
                  {t("sign in")}
                </a>
              )}
            </div>
          </>
        )}
      </Panel>
    </Centered>
  );
}

/**
 * The machine list's quiet line for an account that still has to verify its
 * e-mail (one from before verification was required, with days left — or
 * none, when adding devices and machines already waits for it), with a way to
 * have the link sent again. Gone once the hub says it is verified (asked
 * again whenever the page comes back into view). No way to dismiss it.
 */
export function VerifyEmailBanner({ account }: { account: CloudAccount }) {
  const { t, i18n } = useTranslation();
  const [email, setEmail] = useState<EmailStanding | null>(null);
  const [turnstile, setTurnstile] = useState<TurnstileConfig | null>(null);

  useEffect(() => {
    let live = true;
    let configured = false;
    const look = async () => {
      try {
        const me = await account.api.me(account.session.token);
        if (!live) return;
        setEmail(me.email);
        if (me.email.verification === "required" && !me.email.verified && !configured) {
          configured = true;
          const config = await account.api.signupConfig();
          if (live) setTurnstile(config.turnstile);
        }
      } catch (error) {
        console.warn("cloud: the account's e-mail standing:", error);
      }
    };
    void look();
    const onVisible = () => {
      if (document.visibilityState === "visible") void look();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      live = false;
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [account]);

  if (!email || email.verification !== "required" || email.verified) return null;
  const due = email.dueMs !== null && email.dueMs > Date.now() ? email.dueMs : null;
  return (
    <div
      data-testid="verify-email"
      data-signal="warn"
      className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-border px-4 py-2.5 shadow-[inset_2px_0_0_var(--color-warn)]"
    >
      <span className="flex min-w-[min(100%,16rem)] flex-1 items-start gap-3">
        <Mail className="mt-1 size-3.5 shrink-0 text-faint" />
        <span className="min-w-0 flex-1 text-xs leading-5 text-muted">
          <span className="text-fg">
            {due !== null
              ? t("Verify your e-mail by {{date}}: after that, adding devices waits for it.", { date: formatDay(due, i18n.language) })
              : t("Verify your e-mail: adding devices waits for it.")}
          </span>
          <span className="text-faint"> · </span>
          <span className="font-mono text-2xs break-words">{email.address}</span>
          {email.delivery === "failed" && (
            <>
              <span className="text-faint"> · </span>
              <span className="text-danger">{t("the mail could not be delivered")}</span>
            </>
          )}
        </span>
      </span>
      <span className="flex shrink-0">
        <ResendControl
          api={account.api}
          token={() => account.session.token}
          turnstile={turnstile}
          compact
          onVerified={() => setEmail((e) => (e ? { ...e, verified: true } : e))}
        />
      </span>
    </div>
  );
}
