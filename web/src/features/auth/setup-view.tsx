import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, Loader2 } from "lucide-react";
import { BrandMark } from "@/components/brand-mark";
import { ApiError } from "@/api/errors";
import { cn } from "@/lib/cn";
import { currentUser, isDesktopApp, isLocalOwner, localOpenFailure, LOGIN_PATH, pageDroppenCommand, setupAccount } from "@/lib/auth";

/** What the first-account form asks for. */
export interface SetupForm {
  /** The one-time setup code, as typed; not asked of this device's owner. */
  code: string;
  username: string;
  password: string;
  /** The password again. */
  confirm: string;
}

/**
 * Something to tell the person instead of signing them in: a translation key
 * (the English sentence) and what fills it, or the server's own words when it
 * said something this page has no translation for — and, where there is one,
 * the way on.
 */
export type SetupProblem =
  | { key: string; values?: Record<string, string | number>; action?: SetupAction }
  | { text: string; action?: undefined };

/** Where a refusal can send the person: the login page, or the app. */
export type SetupAction = "sign-in" | "home";

/** The server's rules for a name and a password (`server-rs/src/auth/users.rs`). */
export const MAX_USERNAME = 32;
export const MIN_PASSWORD_CHARS = 12;
export const MAX_PASSWORD_BYTES = 256;

/** Crockford base32, the setup code's alphabet (`server-rs/src/auth/setup.rs`). */
const CODE_ALPHABET = /^[0-9A-HJKMNP-TV-Z]{16}$/;

/**
 * The setup code the way the server reads it: case, spaces and dashes do not
 * matter, and O reads as 0 and I or L as 1.
 */
export function normalizeSetupCode(typed: string): string {
  return typed
    .replace(/[\s-]/g, "")
    .toUpperCase()
    .replace(/O/g, "0")
    .replace(/[IL]/g, "1");
}

/**
 * What is wrong with the form before it is sent, or null.
 *
 * The server's rules, checked here so that a mistake costs no round trip —
 * and, for the code, none of the few wrong guesses the server allows before
 * it makes the person wait. `withCode` false is this device's owner, who is
 * not asked for one. The sentences are the server's own `hint`s, so the one
 * rule only it knows (no common passwords) reads the same.
 */
export function setupFormProblem(form: SetupForm, withCode = true): SetupProblem | null {
  if (withCode && !CODE_ALPHABET.test(normalizeSetupCode(form.code))) {
    return { key: "a setup code is 16 letters and digits, like ABCD-EFGH-JKMN-PQRS" };
  }
  const name = form.username.trim();
  if (name.length === 0 || name.length > MAX_USERNAME || !/^[a-z0-9._-]+$/.test(name)) {
    return { key: "a user name is 1-32 characters of a-z, 0-9, '.', '_' or '-'" };
  }
  const password = form.password;
  if ([...password].length < MIN_PASSWORD_CHARS) return { key: "a password is at least 12 characters long" };
  if (new TextEncoder().encode(password).length > MAX_PASSWORD_BYTES) return { key: "a password is at most 256 bytes long" };
  if (password.trim() !== password) return { key: "a password may not start or end with whitespace" };
  // ASCII case only, as the server compares; a name is lower case ASCII.
  if (password.replace(/[A-Z]/g, (c) => c.toLowerCase()) === name) return { key: "a password may not be the user name" };
  if (form.confirm !== password) return { key: "the two do not match" };
  return null;
}

/** What the server's refusal of `POST /api/auth/setup` means for the person. */
export function setupRefusal(error: unknown): SetupProblem {
  if (!(error instanceof ApiError)) return { text: error instanceof Error ? error.message : String(error) };
  switch (error.code) {
    case "invalid_account":
      // The hint is one of the rule sentences above, translated by key; one
      // this page does not know reads in the server's English.
      return error.hint ? { key: error.hint } : { text: error.code };
    case "code_required":
      // The owner session this page had is gone (it expired, or another page
      // made the account): only the code is left.
      return {
        key: "The setup code is needed now. `{{command}} setup-code` on this device shows it, or run `{{command}} open` there again.",
        values: { command: pageDroppenCommand() },
      };
    case "wrong_setup_code":
      return {
        key: "That is not the setup code this device's server printed. `{{command}} setup-code` on this device shows it again.",
        values: { command: pageDroppenCommand() },
      };
    case "name_taken":
      return { key: "That name belongs to the Droppen app's own account on this device. Pick another." };
    case "not_from_cloud":
      return { key: "The first account is made on this device's own page, not through Droppen." };
    case "setup_done":
      return { key: "This device already has an account.", action: "sign-in" };
    case "auth_off":
      return { key: "Local login is switched off on this device (auth.enabled = false), so there is no account to create here.", action: "home" };
    case "no_setup_code":
      return {
        key: "This device's server has no setup code. Restart it to get one, or create the account on this device with `{{command}} user add <name>`.",
        values: { command: pageDroppenCommand() },
      };
    case "too_many_attempts": {
      const wait = error.body.retryAfterSeconds;
      return typeof wait === "number" && Number.isFinite(wait) && wait > 0
        ? { key: "Too many wrong setup codes. Try again in {{seconds}} s.", values: { seconds: Math.ceil(wait) } }
        : { key: "Too many wrong setup codes. Wait a little, then try again." };
    }
    default:
      return { text: error.hint ?? error.code };
  }
}

/** What a `#local-open=` link that did not sign the page in means for the person. */
export function localOpenRefusal(error: unknown): SetupProblem {
  if (!(error instanceof ApiError)) return { text: error instanceof Error ? error.message : String(error) };
  switch (error.code) {
    case "invalid_local_open":
      return {
        key: "That link was already used or has expired. Run `{{command}} open` on this device again.",
        values: { command: pageDroppenCommand() },
      };
    case "accounts_exist":
      return { key: "This device has an account now: sign in.", action: "sign-in" };
    case "auth_off":
      return { key: "Local login is switched off on this device (auth.enabled = false), so there is no account to create here.", action: "home" };
    case "not_from_cloud":
      return { key: "That link works only on the device itself, not through Droppen." };
    default:
      return { text: error.hint ?? error.code };
  }
}

/**
 * Check the form and make the account: null once it is made and this page is
 * signed in to it, else what to say. Nothing is sent for a form that breaks a
 * rule; no code is sent by this device's owner (`withCode` false). `create` is
 * the request, replaceable in a test.
 */
export async function submitSetup(
  form: SetupForm,
  withCode = true,
  create: (code: string | null, username: string, password: string) => Promise<unknown> = setupAccount,
): Promise<SetupProblem | null> {
  const problem = setupFormProblem(form, withCode);
  if (problem) return problem;
  try {
    await create(withCode ? form.code.trim() : null, form.username.trim(), form.password);
    return null;
  } catch (error) {
    return setupRefusal(error);
  }
}

const command = "rounded border border-border bg-raised px-2 py-1.5 font-mono text-2xs break-all whitespace-pre-wrap text-fg";

/** A refusal or a broken rule, and the way on when there is one. */
export function SetupProblemLine({ problem, testId = "setup-error" }: { problem: SetupProblem; testId?: string }) {
  const { t } = useTranslation();
  const text = "text" in problem ? problem.text : t(problem.key, problem.values);
  return (
    <div className="flex flex-col gap-2">
      <div data-signal="failed" data-testid={testId} className="font-mono text-2xs break-words text-danger">
        {text}
      </div>
      {problem.action && (
        // A document load, not a router navigation: the status this page
        // loaded with is out of date, and the guard of the page that loads
        // has to ask again.
        <a
          href={problem.action === "sign-in" ? LOGIN_PATH : "/"}
          data-testid={problem.action === "sign-in" ? "setup-sign-in" : "setup-home"}
          className="flex h-8 items-center justify-center border border-border-strong text-xs font-medium text-fg hover:bg-hover"
        >
          {problem.action === "sign-in" ? t("sign in") : t("continue")}
        </a>
      )}
    </div>
  );
}

/** The two places the first-account form is drawn, at their own sizes. */
const SIZES = {
  page: {
    form: "flex flex-col gap-3",
    field:
      "h-8 w-full rounded border border-border bg-bg px-2 font-mono text-xs text-fg placeholder:text-faint focus:border-fg focus:outline-none",
    button: "flex h-8 items-center justify-center gap-1.5 bg-fg text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40",
  },
  settings: {
    form: "flex max-w-xs flex-col gap-2",
    field:
      "h-6.5 w-full rounded border border-border bg-bg px-2 font-mono text-2xs text-fg placeholder:text-faint focus:border-fg focus:outline-none",
    button:
      "flex h-7 items-center justify-center gap-1.5 self-start bg-fg px-3 text-xs font-medium text-bg hover:bg-fg/85 disabled:opacity-40",
  },
} as const;

/**
 * Name, password and the password again — and the setup code, unless the page
 * is this device's owner. Once the account is made the page is signed in to
 * it and loads again from the top: the owner session it may have had is
 * revoked, and everything that runs under a token starts over with the new one.
 */
export function FirstAccountForm({
  withCode,
  size = "page",
  autoFocus = false,
}: {
  withCode: boolean;
  size?: keyof typeof SIZES;
  autoFocus?: boolean;
}) {
  const { t } = useTranslation();
  const [form, setForm] = useState<SetupForm>({ code: "", username: "", password: "", confirm: "" });
  const [problem, setProblem] = useState<SetupProblem | null>(null);
  const [busy, setBusy] = useState(false);
  const look = SIZES[size];

  const edit = (key: keyof SetupForm) => (event: React.ChangeEvent<HTMLInputElement>) => {
    const value = event.target.value;
    setForm((current) => ({ ...current, [key]: value }));
  };

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setProblem(null);
    const outcome = await submitSetup(form, withCode);
    if (outcome === null) {
      // A full document load, as after a login: the feed socket, every query
      // and the ticket prewarm start at boot, and this way they start with the
      // token the setup just handed back.
      location.replace("/");
      return;
    }
    setProblem(outcome);
    setBusy(false);
  };

  const filled =
    (!withCode || form.code.trim() !== "") && form.username.trim() !== "" && form.password !== "" && form.confirm !== "";

  return (
    <form onSubmit={submit} className={look.form} data-testid="setup-form" data-with-code={withCode ? "true" : "false"}>
      {withCode && (
        <label className="flex flex-col gap-1">
          <span className="text-2xs text-faint">{t("setup code")}</span>
          <input
            value={form.code}
            onChange={edit("code")}
            data-testid="setup-code"
            autoComplete="one-time-code"
            autoCapitalize="characters"
            autoCorrect="off"
            spellCheck={false}
            autoFocus={autoFocus}
            placeholder="ABCD-EFGH-JKMN-PQRS"
            className={cn(look.field, "uppercase")}
          />
        </label>
      )}
      <label className="flex flex-col gap-1">
        <span className="text-2xs text-faint">{t("username")}</span>
        <input
          value={form.username}
          onChange={edit("username")}
          data-testid="setup-username"
          autoComplete="username"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          autoFocus={autoFocus && !withCode}
          className={look.field}
        />
      </label>
      <label className="flex flex-col gap-1">
        <span className="text-2xs text-faint">{t("password")}</span>
        <input
          type="password"
          value={form.password}
          onChange={edit("password")}
          data-testid="setup-password"
          autoComplete="new-password"
          className={look.field}
        />
      </label>
      <label className="flex flex-col gap-1">
        <span className="text-2xs text-faint">{t("confirm password")}</span>
        <input
          type="password"
          value={form.confirm}
          onChange={edit("confirm")}
          data-testid="setup-confirm"
          autoComplete="new-password"
          className={look.field}
        />
      </label>
      {problem && <SetupProblemLine problem={problem} />}
      <button type="submit" disabled={busy || !filled} data-testid="setup-submit" data-variant="primary" className={look.button}>
        {busy && <Loader2 className="size-3.5 animate-spin" />}
        {busy ? t("creating account…") : t("create account and sign in")}
      </button>
    </form>
  );
}

/** What this device's owner is told about the first account, here and in Settings › Access. */
export const OWNER_EXPLANATION =
  "You opened this page as this device's owner. This device has no account yet: create one to sign in from other devices; from then on every browser signs in, this one too.";

/**
 * The first account of a device whose Droppen server has none yet.
 *
 * Nobody signed in — on the device itself or from another one, which the
 * server cannot tell apart — is shown the two ways in: open the page signed in
 * from the device itself, or the one-time setup code. This device's owner (a
 * page opened that way) is asked only for a name and a password.
 */
export function SetupView() {
  // The router guard resolved the status before this renders.
  return <SetupPage owner={isLocalOwner(currentUser())} localOpenError={localOpenFailure()} />;
}

/** The setup page itself; see {@link SetupView}. */
export function SetupPage({ owner, localOpenError = null }: { owner: boolean; localOpenError?: unknown }) {
  const { t } = useTranslation();
  return (
    <div className="flex h-full w-full overflow-y-auto p-6" data-testid="setup-view" data-owner={owner ? "true" : "false"}>
      <div className="m-auto flex w-full max-w-80 flex-col gap-3">
        <div className="mb-3 flex items-center gap-2.5">
          <BrandMark size={20} title="Droppen" />
          <span className="font-mono text-sm font-medium text-fg">Droppen</span>
        </div>
        <div className="flex items-center gap-2 border-b border-border pb-2">
          <h1 data-label="" className="label-caps text-2xs text-muted">{t("set up this device")}</h1>
        </div>
        {owner ? (
          <>
            <p className="text-2xs text-faint" data-testid="setup-owner">
              {t(OWNER_EXPLANATION)}
            </p>
            <FirstAccountForm withCode={false} autoFocus />
          </>
        ) : (
          <>
            <p className="text-xs text-fg" data-testid="setup-explanation">
              {t("This device has no account yet.")}
            </p>
            {localOpenError !== null && localOpenError !== undefined && (
              <SetupProblemLine problem={localOpenRefusal(localOpenError)} testId="setup-local-open-error" />
            )}
            {isDesktopApp() ? (
              // The desktop app's server keeps no setup code and its control
              // socket opens nothing (any process of the user could reach
              // either): the app's own Open in Browser is the way in.
              <div className="flex flex-col gap-1.5 text-2xs text-faint" data-testid="setup-desktop-app">
                <p>{t("This is the Droppen app's own server. In the app's menu-bar menu, choose Open in Browser: it opens this page signed in, and the first account is made there.")}</p>
                <p>{t("Or create the account on this device, without a code:")}</p>
                <pre className={command}>{`${pageDroppenCommand()} user add <name>`}</pre>
              </div>
            ) : (
              <>
                <div className="flex flex-col gap-1.5 text-2xs text-faint" data-testid="setup-local-open">
                  <p>
                    {t("On this device: open Droppen from the app, or run `{{command}} open` in a terminal — it opens this page signed in.", {
                      command: pageDroppenCommand(),
                    })}
                  </p>
                  <pre className={command}>{`${pageDroppenCommand()} open`}</pre>
                </div>
                {/* Collapsed: the code is the way in from another device, and the way of last resort on this one. */}
                <details className="group border-t border-border pt-3" data-testid="setup-with-code">
                  <summary className="flex cursor-pointer list-none items-start gap-1 text-2xs text-muted hover:text-fg [&::-webkit-details-marker]:hidden">
                    <ChevronRight className="mt-0.5 size-3 shrink-0 transition-transform group-open:rotate-90" />
                    <span>
                      {t(
                        "Or create the first account with the one-time setup code this device's server keeps (`{{command}} setup-code` on this device shows it)",
                        { command: pageDroppenCommand() },
                      )}
                    </span>
                  </summary>
                  <div className="mt-3 flex flex-col gap-3">
                    <FirstAccountForm withCode />
                    <div className="flex flex-col gap-1.5 text-2xs text-faint" data-testid="setup-help">
                      <p>{t("Or create the account on this device, without a code:")}</p>
                      <pre className={command}>{`${pageDroppenCommand()} user add <name>`}</pre>
                    </div>
                  </div>
                </details>
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}
