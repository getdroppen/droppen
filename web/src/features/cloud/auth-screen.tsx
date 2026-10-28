/*
 * droppen.app's sign-in and sign-up screen (`cloud-app.tsx` renders it in
 * cloud mode before the account is open): a passkey first where the browser
 * and the hub can, the password otherwise, the e-mail check, and the terms
 * the person agrees to.
 */

import { type ReactNode, useEffect, useRef, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { Loader2 } from "lucide-react";
import { authErrorKind, HubApi, type HubSession, type PasskeySignedIn, PASSWORD_MIN_CHARS, SIGNUP_CONFIG, type SignedIn, type SignupConfig } from "@/lib/hub/api";
import { finishPasskeyLogin, type PasskeyAnswer, PasskeyAutofill, passkeyCancelled, passkeyLogin, passkeySignUp, pageCredentials, probePasskeys, webAuthnAvailable } from "@/lib/hub/passkey";
import { rememberPasskeySignIn } from "@/lib/hub/passkey-join";
import { pairingResume, takeAccountDeleted, takeSignOutFailures, takeStillListed } from "./sign-out";
import { StillListedNote } from "./sign-out-dialog";
import { authErrorText, CloudPage, ErrorLine, Field, field, Note, primary, quiet, Tabs } from "./ui";
import { CheckEmailScreen } from "./email-verify";
import { BrandMark } from "@/components/brand-mark";
import { PrefsControls } from "@/components/prefs-controls";
import { goPublic, isAuthPath, PRIVACY_PATH, publicHref, publicLink, SIGN_IN_PATH, SIGN_UP_PATH, TERMS_PATH } from "@/features/landing/route";

/** The languages the verification mail is written in (the page's, else English). */
const MAIL_LANGS = ["en", "zh-CN", "zh-TW", "ja"];

/** How the form asks who you are: a passkey first, the password, or not known yet (the caller has no answer). */
export type AuthMethod = "passkey" | "password" | "pending";

/**
 * A passkey first where both this browser and the hub can (docs/passkeys.md
 * §7.1, §7.2). The password where either cannot, as the form always was
 * and with no word of passkeys, or where the person asked for it.
 */
export function authMethod(webauthn: boolean, config: SignupConfig | "failed" | null, wantPassword: boolean): AuthMethod {
  if (!webauthn || config === "failed") return "password";
  if (config === null) return "pending";
  if (!config.passkeys) return "password";
  return wantPassword ? "password" : "passkey";
}

/**
 * Sign in, or sign up: sign-up is open, as `SIGNUP_CONFIG` says, so an invite
 * code is optional — it may give the account a plan — and waits behind "have
 * an invite code?". No Turnstile widget is drawn. An account whose e-mail must
 * be verified first waits on "check your e-mail" before going on.
 *
 * Where passkeys are offered and the browser does WebAuthn, a passkey comes
 * first (docs/passkeys.md §3.4 A, §3.5): sign-up makes one, sign-in asks for
 * one, modally or from the e-mail field's autofill, and the password is a step
 * away. A passkey sign-in goes on exactly as a password one does.
 * Exported so the screen can be previewed on its own.
 */
export function AuthScreen({
  api,
  onSignedIn,
  initialMode = "signin",
  above,
}: {
  api: HubApi;
  onSignedIn: (session: HubSession) => void;
  initialMode?: "signin" | "signup";
  /**
   * Shown above the sign-in form (not sign-up's: a new account has nothing to
   * bring over): on the app host, the offer to bring this browser's device over
   * from the hub's origin.
   */
  above?: ReactNode;
}) {
  const { t, i18n } = useTranslation();
  const [mode, setModeState] = useState<"signin" | "signup">(initialMode);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [invite, setInvite] = useState("");
  // Open sign-up: the person asked for the optional invite field (or the hub refused an invite).
  const [inviteShown, setInviteShown] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The browser's passkey prompt was closed: said, but not as an error.
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // What the last sign-out in this tab could not clear (read once).
  const [failures] = useState(() => takeSignOutFailures());
  // A browser just signed out and removed that the account still lists (read once).
  const [stillListed] = useState(() => takeStillListed());
  // This tab's account was just deleted, from its settings (read once).
  const [deleted] = useState(() => takeAccountDeleted());
  // Signed out by the pairing screen's "sign in again": it comes back once signed in.
  const [resumesPairing] = useState(() => pairingResume() !== null);
  // Signed in, the e-mail still to verify.
  const [waiting, setWaiting] = useState<{ session: SignedIn; address: string } | null>(null);
  // WebAuthn in this browser at all, and passkeys in the e-mail field's autofill.
  const [webauthn] = useState(() => webAuthnAvailable());
  const [autofillable, setAutofillable] = useState(false);
  // The person chose the password over a passkey.
  const [wantPassword, setWantPassword] = useState(false);
  const autofill = useRef<PasskeyAutofill | null>(null);

  useEffect(() => {
    if (!webauthn) return;
    let live = true;
    void probePasskeys().then((support) => live && setAutofillable(support.conditional));
    return () => {
      live = false;
    };
  }, [webauthn]);

  // On the sign-in / sign-up pages the address follows the tab (a deep link keeps its own).
  const setMode = (next: "signin" | "signup") => {
    setModeState(next);
    setNote(null);
    setWantPassword(false);
    if (isAuthPath(location.pathname)) goPublic(next === "signup" ? SIGN_UP_PATH : SIGN_IN_PATH, { replace: true });
  };

  // Sign-up's answers are constants (`SIGNUP_CONFIG`): open, an optional invite, no widget.
  const config = SIGNUP_CONFIG;
  const inviteOffered = true;
  const method = authMethod(webauthn, config, wantPassword);
  // Passkeys here at all: the e-mail field offers them, and the form offers both ways in.
  const passkeys = webauthn ? config.passkeys : null;
  const lang = MAIL_LANGS.includes(i18n.language) ? i18n.language : "en";

  /** Where a sign-in goes, however it was made: on, or "check your e-mail" first. */
  const signedIn = (session: SignedIn, address: string) => {
    if (session.verification === "required" && !session.emailVerified) return setWaiting({ session, address });
    onSignedIn(session);
  };

  /** A refusal in words (a closed passkey prompt as a note). */
  const failed = (err: unknown, context: "signup" | "login") => {
    if (passkeyCancelled(err)) {
      setNote(context === "signup" ? t("no passkey was created") : t("no passkey was used"));
    } else {
      // A refused invite opens the field: the code may be mistyped or used up.
      if (authErrorKind(err) === "badInvite") setInviteShown(true);
      setError(authErrorText(err, context));
    }
    setBusy(false);
  };

  /** A passkey sign-in, modal or from the autofill: `true` once signed in. */
  const withPasskey = async (run: () => Promise<PasskeySignedIn>): Promise<boolean> => {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const session = await run();
      // Which passkey this tab holds: a new browser joins the account with it next, with one gesture less.
      if (session.credentialId) rememberPasskeySignIn({ account: session.account, credentialId: session.credentialId, rpId: passkeys?.rpId ?? null });
      setBusy(false);
      // A passkey names no e-mail on this side: "check your e-mail" shows the hub's.
      signedIn(session, "");
      return true;
    } catch (err) {
      failed(err, "login");
      return false;
    }
  };

  // The autofill's answers, through the newest render's handlers.
  const answer = (a: PasskeyAnswer) => withPasskey(() => finishPasskeyLogin(api, {}, a));
  const handlers = useRef({ answer });
  useEffect(() => {
    handlers.current = { answer };
  });

  // Signing in: the e-mail field's autofill offers this hub's passkeys (§3.5 step 1), asked afresh before each challenge expires.
  const autofillOn = mode === "signin" && passkeys !== null && autofillable && waiting === null;
  useEffect(() => {
    const credentials = autofillOn ? pageCredentials() : null;
    if (!credentials) return;
    const fill = new PasskeyAutofill({
      options: () => api.passkeyLoginOptions(),
      credentials,
      onAnswer: (answer) => handlers.current.answer(answer),
      onError: (err) => console.warn("cloud: passkey autofill stopped:", err),
    });
    autofill.current = fill;
    fill.start();
    return () => {
      fill.close();
      if (autofill.current === fill) autofill.current = null;
    };
  }, [api, autofillOn]);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (busy || method === "pending") return;
    const address = email.trim();
    if (method === "passkey") {
      const credentials = pageCredentials();
      if (!credentials) return;
      if (mode === "signin") {
        // Modal, with no passkey named: the person picks one (the autofill's request withdrawn first).
        await withPasskey(() => passkeyLogin(api, { credentials }, { rpId: passkeys?.rpId, autofill: autofill.current }));
        return;
      }
      setBusy(true);
      setError(null);
      setNote(null);
      try {
        const session = await passkeySignUp(api, { credentials }, { email: address, invite: invite.trim() || undefined, lang, rpId: passkeys?.rpId });
        setBusy(false);
        signedIn(session, address);
      } catch (err) {
        failed(err, "signup");
      }
      return;
    }
    // Too short for the hub: said here, before a refusal.
    if (mode === "signup" && [...password].length < PASSWORD_MIN_CHARS) {
      setNote(null);
      setError(t("a password needs at least {{n}} characters", { n: PASSWORD_MIN_CHARS }));
      return;
    }
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const session =
        mode === "signup"
          ? await api.signUp(address, password, { invite: invite.trim() || undefined, lang })
          : await api.login(address, password);
      setBusy(false);
      signedIn(session, address);
    } catch (err) {
      failed(err, mode === "signup" ? "signup" : "login");
    }
  };

  if (waiting) {
    return (
      <CheckEmailScreen
        api={api}
        session={waiting.session}
        address={waiting.address}
        onVerified={(session) => onSignedIn(session)}
        onBack={(message) => {
          setWaiting(null);
          setPassword("");
          if (!message) setEmail("");
          setError(message ?? null);
          setMode("signin");
        }}
      />
    );
  }

  const choose = (usePassword: boolean) => {
    setWantPassword(usePassword);
    setError(null);
    setNote(null);
  };
  // One centred column: the mark and the name, then the form, and a way back to the landing page;
  // language and theme at the right of the header, as on the landing page.
  return (
    <CloudPage aside={<PrefsControls />}>
      <div className="flex min-h-[calc(100dvh-3.5rem)] flex-col items-center justify-center px-6 py-14 max-md:px-4 max-md:py-10">
        <div className="flex w-full max-w-[22rem] flex-col">
          <div className="mb-8 flex flex-col items-center gap-3">
            <BrandMark size={36} />
            <h1 className="font-mono text-lg leading-6 font-medium text-fg">Droppen</h1>
          </div>
          {deleted && (
            <div className="mb-6">
              <Note tone="ok">
                <p data-testid="account-deleted" className="text-xs leading-5 font-medium text-fg">
                  {t("Your account was deleted.")}
                </p>
                <p className="text-xs leading-5 text-muted">{t("What it kept at Droppen is erased for good, and this browser no longer holds its keys.")}</p>
              </Note>
            </div>
          )}
          {failures && (
            <div className="mb-6">
              <ClearFailures failures={failures} />
            </div>
          )}
          {stillListed && (
            <div className="mb-6">
              <StillListedNote why={stillListed} />
            </div>
          )}
          {resumesPairing && (
            <div className="mb-6">
              <Note tone="warn">
                <p data-testid="signin-to-pair" className="text-xs leading-5 text-muted">
                  {t("Sign in again to go on adding this browser: you come back to where you were.")}
                </p>
              </Note>
            </div>
          )}
          {above && mode === "signin" && (
            <div data-testid="auth-above" className="mb-6">
              {above}
            </div>
          )}
          <form onSubmit={submit} className="flex flex-col gap-3">
            <Tabs
              value={mode}
              onChange={setMode}
              options={[
                { id: "signin", label: t("sign in") },
                { id: "signup", label: t("sign up") },
              ]}
            />
            {mode === "signup" && inviteShown && (
              <Field label={t("invite code (optional)")}>
                <input
                  data-testid="auth-invite"
                  value={invite}
                  onChange={(e) => setInvite(e.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                  // Asked for: the person just opened it.
                  autoFocus
                  className={field}
                />
              </Field>
            )}
            <Field label={t("e-mail")}>
              <input
                data-testid="auth-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                // Signing in, this hub's passkeys show in the field's autofill too.
                autoComplete={mode === "signin" && passkeys ? "username webauthn" : "username"}
                autoFocus
                className={field}
              />
            </Field>
            {method === "password" && (
              <>
                <Field label={t("password")}>
                  <input
                    data-testid="auth-password"
                    type="password"
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    autoComplete={mode === "signup" ? "new-password" : "current-password"}
                    className={field}
                  />
                </Field>
                {mode === "signup" && <p className="text-xs text-faint">{t("At least 10 characters. Your password signs you in; it cannot decrypt anything.")}</p>}
              </>
            )}
            {method === "passkey" && mode === "signup" && (
              <p className="text-xs text-faint">{t("A passkey signs you in with your device's screen lock; it cannot decrypt anything.")}</p>
            )}
            {mode === "signup" && <SignUpTerms />}
            <ErrorLine message={error} />
            {note && (
              <p data-testid="passkey-cancelled" className="font-mono text-2xs text-muted">
                {note}
              </p>
            )}
            {method === "passkey" ? (
              mode === "signup" ? (
                <button
                  type="submit"
                  data-testid="passkey-signup"
                  data-variant="primary"
                  disabled={busy || !email.trim()}
                  className={primary}
                >
                  {busy && <Loader2 className="size-3.5 animate-spin" />}
                  {t("create a passkey and sign up")}
                </button>
              ) : (
                <button type="submit" data-testid="passkey-signin" data-variant="primary" disabled={busy} className={primary}>
                  {busy && <Loader2 className="size-3.5 animate-spin" />}
                  {t("sign in with a passkey")}
                </button>
              )
            ) : (
              <button
                type="submit"
                data-testid={method === "password" ? "password-submit" : "auth-pending"}
                data-variant="primary"
                disabled={method === "pending" || busy || !email.trim() || !password}
                className={primary}
              >
                {(busy || method === "pending") && <Loader2 className="size-3.5 animate-spin" />}
                {mode === "signup" ? t("create account") : t("sign in")}
              </button>
            )}
            {mode === "signup" && inviteOffered && !inviteShown && (
              <button type="button" data-testid="show-invite" className={quiet} onClick={() => setInviteShown(true)}>
                {t("have an invite code?")}
              </button>
            )}
            {passkeys && (
              method === "passkey" ? (
                <button type="button" data-testid="use-password" className={quiet} onClick={() => choose(true)}>
                  {mode === "signup" ? t("sign up with a password instead") : t("sign in with a password")}
                </button>
              ) : method === "password" ? (
                <button type="button" data-testid="use-passkey" className={quiet} onClick={() => choose(false)}>
                  {mode === "signup" ? t("sign up with a passkey instead") : t("sign in with a passkey instead")}
                </button>
              ) : null
            )}
          </form>
          <a {...publicLink("/")} className="mt-6 self-center text-xs text-muted underline-offset-4 hover:text-fg hover:underline">
            {t("What is Droppen?")}
          </a>
        </div>
      </div>
    </CloudPage>
  );
}

/**
 * What signing up agrees to, said above the button: the terms of service and
 * the privacy notice (on the site host where the hub has one: `publicHref`),
 * each opening in a new tab so the form keeps what was typed (and the human
 * check its token).
 */
function SignUpTerms() {
  const { t } = useTranslation();
  const link = "text-muted underline underline-offset-4 hover:text-fg";
  return (
    <p data-testid="signup-terms" className="text-xs leading-5 text-faint">
      <Trans
        t={t}
        i18nKey="By signing up you agree to the <terms>Terms of Service</terms> and have read the <privacy>Privacy notice</privacy>."
        components={{
          terms: <a href={publicHref(TERMS_PATH)} target="_blank" rel="noopener" data-testid="signup-terms-link" className={link} />,
          privacy: <a href={publicHref(PRIVACY_PATH)} target="_blank" rel="noopener" data-testid="signup-privacy-link" className={link} />,
        }}
      />
    </p>
  );
}

/** What could not be cleared, said plainly (a removed device's wipe, a sign-out). */
export function ClearFailures({ failures }: { failures: string[] | null | undefined }) {
  const { t } = useTranslation();
  if (!failures?.length) return null;
  return (
    <Note tone="danger">
      <span data-signal="failed" className="font-mono text-xs text-danger">{t("not everything could be cleared from this browser")}</span>
      <ul className="flex flex-col gap-0.5 font-mono text-2xs break-words text-muted">
        {failures.map((f, i) => (
          <li key={i}>{f}</li>
        ))}
      </ul>
      <p className="text-xs leading-5 text-muted">{t("Clear this site's data in the browser's settings to be sure.")}</p>
    </Note>
  );
}
