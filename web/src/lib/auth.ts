/**
 * The browser half of the server's bearer-token auth.
 *
 * One token per browser, in `localStorage`, sent as `Authorization: Bearer` on
 * every request by the middleware in `api/client.ts`. Two surfaces cannot carry
 * a header — websockets and `<img src>`/`<a href>` downloads — and those spend a
 * short-lived `?ticket=` minted here instead.
 *
 * Everything degrades to nothing on a server whose authentication is switched
 * off (`auth.enabled = false`, for development): `authRequired` is false, no
 * ticket is ever minted and no login page is drawn. Otherwise every caller
 * signs in. A device with no account yet (`setupRequired`) answers nothing but
 * its status, the setup route and the local-open route, and the page draws the
 * setup page: open it signed in from the device itself (`remote-hand-server
 * open`, or the desktop app), which makes the page this device's owner
 * ({@link LOCAL_OWNER}), or make the first account with the one-time code the
 * server printed.
 */
import { cacheClear, cacheUnseal } from "@/lib/local-cache";
import { ApiError } from "@/api/errors";
import type { components } from "@/api/schema";
import type { MachineKey } from "@/lib/machines/key";
import { apiFor, homeMachine } from "@/lib/machines/registry";
import { droppenCommand } from "@/lib/droppen-command";
import { takeLocalOpen } from "@/lib/local-open";
import { forgetDownloadTicket, primeDownloadTicket } from "./download-ticket";

export {
  type CachedTicket,
  DOWNLOAD_TICKET_TTL_MS,
  fetchTicket,
  freshDownloadTicket,
  freshTicket,
  HEAL_COOLDOWN_MS,
  healDownloadTicket,
  invalidateDownloadTicket,
  primeDownloadTicket,
  serverRestarted,
  subscribeDownloadTicket,
  downloadTicketSnapshot,
} from "./download-ticket";

/**
 * The login is the page's own: its home machine's (`lib/machines/registry.ts`)
 * — the server that served the page in the local shell, the main device on
 * droppen.app — whichever machines a view is about.
 */
export const pageApi = () => apiFor(homeMachine());

/**
 * Whether `machine` is the one the page's login — its token, its tickets, the
 * auth status it asks — is with: the home. Another machine's server
 * restarting, or its sockets failing, says nothing about that login, and must
 * not spend it. (On droppen.app no machine needs a ticket: each is reached
 * through its own end-to-end channel.)
 */
export function holdsPageLogin(machine: MachineKey): boolean {
  return machine === homeMachine();
}

/**
 * `GET /api/auth/status`'s answer. `authRequired` is true unless
 * authentication is switched off (`authOff`); `setupRequired` says the device
 * has no account yet. Those two are optional here because a page on
 * droppen.app asks devices running older servers too, whose answers lack them.
 * Only the fields read here are named, so the shape holds whatever else the
 * generated schema carries.
 */
type WireAuthStatus = components["schemas"]["AuthStatus"];
export type AuthStatus = Pick<WireAuthStatus, "authRequired" | "user" | "desktopApp"> & {
  setupRequired?: boolean;
  authOff?: boolean;
  droppenPath?: string | null;
  droppenEnv?: CommandEnv[] | null;
};
type CommandEnv = components["schemas"]["CommandEnv"];
type LoginResult = components["schemas"]["LoginResult"];
export type TicketScope = components["schemas"]["TicketScope"];

/**
 * Where the token lives. No instance name in the key: `localStorage` is already
 * scoped to the origin, and one origin serves exactly one instance.
 */
export const TOKEN_KEY = "rh.auth.token";

/** The login page's route, and the one path a 401 must not bounce off. */
export const LOGIN_PATH = "/login";

/** The login endpoint. */
export const LOGIN_API_PATH = "/api/auth/login";

/**
 * The setup page's route: where every page of a server with no account yet
 * goes, and — like the login page — a path a 401 must not bounce off.
 */
export const SETUP_PATH = "/setup";

/** The endpoint that makes the first account with the one-time setup code. */
export const SETUP_API_PATH = "/api/auth/setup";

/** The endpoint that trades a `#local-open=` token for a session. */
export const LOCAL_OPEN_API_PATH = "/api/auth/local-open";

/**
 * Who a page opened signed in from the device itself is, while the device has
 * no account: its owner. The app is theirs; the setup page and the security
 * tab offer them the first account without the setup code.
 */
export const LOCAL_OWNER = "local:owner";

/** Whether `user` is this device's owner by a local-open session. */
export function isLocalOwner(user: string | null | undefined): boolean {
  return user === LOCAL_OWNER;
}

/**
 * The endpoints whose refusal belongs in the form that asked, not to a dead
 * token: logging in and re-typing the current password to change it, whose 401
 * means "wrong password", and making the first account, which refuses a wrong
 * code with a 403 but must not be read as a lost login whatever it answers.
 * Everything else that answers 401 is the middleware refusing the token itself.
 */
const PASSWORD_PATHS = [LOGIN_API_PATH, "/api/auth/password", SETUP_API_PATH, LOCAL_OPEN_API_PATH];

/**
 * What a server with authentication switched off answers, and what an
 * unreachable one is read as.
 */
const OPEN_SERVER: AuthStatus = {
  authRequired: false,
  setupRequired: false,
  user: null,
  desktopApp: false,
  authOff: true,
  droppenPath: "",
  droppenEnv: [],
};

// ---------------------------------------------------------------------------
// The token
// ---------------------------------------------------------------------------

/** `undefined` until `localStorage` has been read once; `null` means logged out. */
let token: string | null | undefined;

export function getToken(): string | null {
  if (token === undefined) {
    try {
      token = localStorage.getItem(TOKEN_KEY);
    } catch {
      // Storage blocked (private mode, third-party iframe): this tab keeps the
      // token in memory instead and loses it on reload.
      token = null;
    }
  }
  return token;
}

export function setToken(next: string): void {
  token = next;
  try {
    localStorage.setItem(TOKEN_KEY, next);
  } catch {
    // memory-only for this tab
  }
}

/** Who wants to know that this page has lost its login; see {@link onSignedOut}. */
const signedOutListeners = new Set<() => void>();

/**
 * Hear about the token going away — a logout, or a 401 on the way to the login
 * page — before the page itself does. For whatever keeps running under the
 * token until the document is replaced: the page's socket above all, whose
 * frames would otherwise go on feeding the device copy being cleared.
 */
export function onSignedOut(listener: () => void): () => void {
  signedOutListeners.add(listener);
  return () => {
    signedOutListeners.delete(listener);
  };
}

export function clearToken(): void {
  token = null;
  // Tickets were minted by the token that just died; none of them outlive it.
  forgetDownloadTicket();
  // Nor does the copy of the session list and transcripts kept for a fast
  // start: it is the logged-in user's, not the device's. Sealed at once, so a
  // save already scheduled cannot put any of it back.
  void cacheClear();
  for (const listener of [...signedOutListeners]) {
    try {
      listener();
    } catch {
      // one listener failing must not keep the others from hearing
    }
  }
  try {
    localStorage.removeItem(TOKEN_KEY);
  } catch {
    // nothing stored to remove
  }
}

/** The `Authorization` header for a raw `fetch`, or nothing when logged out. */
export function authHeader(): Record<string, string> {
  const value = getToken();
  return value ? { Authorization: `Bearer ${value}` } : {};
}

// ---------------------------------------------------------------------------
// Whether this server wants a login at all
// ---------------------------------------------------------------------------

let status: AuthStatus | null = null;
let statusInFlight: Promise<AuthStatus> | null = null;

/**
 * `GET /api/auth/status`, asked once per page load.
 *
 * The route needs no token, so this is also how a browser holding a dead one
 * finds out. Asked after a `#local-open=` link has been traded for a session
 * ({@link startLocalOpen}), so the answer is about that session. A network failure reads as "open server": there is no password to
 * type at a server that cannot be reached, and the requests that follow fail
 * visibly on their own.
 */
export async function authStatus(): Promise<AuthStatus> {
  if (status) return status;
  statusInFlight ??= (async () => {
    // Who the page is depends on the token a `#local-open=` link is being
    // traded for, so that trade lands first.
    await localOpenFlight;
    try {
      const res = await pageApi().GET("/api/auth/status");
      status = res.data ?? OPEN_SERVER;
    } catch {
      status = OPEN_SERVER;
    } finally {
      statusInFlight = null;
    }
    return status ?? OPEN_SERVER;
  })();
  return statusInFlight;
}

/**
 * Whether a login is required, from the cached answer.
 *
 * Synchronous on purpose: the router guard resolves `authStatus()` before
 * anything renders, so every caller downstream of a render can just read it.
 * Before that first answer it reads as "open", which only ever means one extra
 * unticketed request that the 401 handler picks up.
 */
export function authRequired(): boolean {
  return status?.authRequired ?? false;
}

/**
 * Whether the Droppen desktop app runs this page's server and owns its login
 * (`desktop/` in the repository; `RH_DESKTOP_APP` on the server). The app made
 * the password nobody knows and signs in again by itself, so a password change
 * and a sign-out are dead ends there; and its window has no popups, so it signs
 * in to Droppen through the system browser (`features/link/browser-sign-in.tsx`).
 * Synchronous for the reason {@link authRequired} is.
 */
export function isDesktopApp(): boolean {
  return status?.desktopApp ?? false;
}

/**
 * The command a person types on this page's device to run its server
 * (`lib/droppen-command.ts`), from the cached status. Synchronous for the
 * reason {@link authRequired} is.
 */
export function pageDroppenCommand(): string {
  return droppenCommand(status);
}

/**
 * Who the cached status says is signed in: a name, `null` on an open server or
 * when nobody is, `undefined` while the status has not been asked yet.
 */
export function currentUser(): string | null | undefined {
  return status ? status.user : undefined;
}

/** Forget the cached status, so the next `authStatus()` asks again. */
export function forgetAuthStatus(): void {
  status = null;
  statusInFlight = null;
}

/**
 * Re-ask who we are, ignoring the cache, and send the browser to the login page
 * when the answer is "nobody".
 *
 * This is the escape hatch for the one refusal a script cannot read: a
 * websocket handshake rejected for want of a ticket closes with no status code,
 * so the only way to tell "the server is down" from "the token is dead" is to
 * ask a route that answers without one.
 */
export async function verifyStillAuthorized(): Promise<boolean> {
  const res = await pageApi().GET("/api/auth/status");
  if (!res.data) return true; // no answer is not an answer
  status = res.data;
  if (res.data.authRequired && !res.data.user) {
    noteUnauthorized("/api/auth/status");
    return false;
  }
  return true;
}

// ---------------------------------------------------------------------------
// Logging in and out
// ---------------------------------------------------------------------------

/** Keep the token a login (or the first account's setup) handed back. */
function signedIn(result: LoginResult): void {
  // A download ticket belongs to the session that minted it, which this one
  // replaces — and, after the setup, the server has just revoked.
  forgetDownloadTicket();
  setToken(result.token);
  status = {
    authRequired: true,
    setupRequired: false,
    user: result.username,
    desktopApp: status?.desktopApp ?? false,
    authOff: false,
    droppenPath: status?.droppenPath ?? "",
    droppenEnv: status?.droppenEnv ?? [],
  };
  // The device copy is this user's to fill from here on.
  cacheUnseal();
  void primeDownloadTicket();
}

/** Exchange a password for a token, and keep it. Throws `ApiError` on refusal. */
export async function login(username: string, password: string): Promise<LoginResult> {
  const res = await pageApi().POST(LOGIN_API_PATH, { body: { username, password } });
  if (!res.data) throw new ApiError(res.error);
  signedIn(res.data);
  return res.data;
}

/**
 * Make the first account of a server in its setup state, and keep the token
 * that comes back: the page that made the account is signed in to it, exactly
 * as after {@link login}.
 *
 * `code` is the one-time setup code the server printed, sent as typed (the
 * server ignores case, spaces and dashes, and reads O as 0 and I or L as 1).
 * `null` when the page is this device's owner ({@link LOCAL_OWNER}), whose
 * session is what the code would otherwise prove: it is not sent at all. The
 * server then revokes every owner session, so the new token is the page's only
 * one from here on.
 *
 * Throws `ApiError` on refusal — its `code` is one of `invalid_account`,
 * `code_required`, `wrong_setup_code`, `not_from_cloud`, `setup_done`,
 * `name_taken`, `auth_off`, `no_setup_code` or `too_many_attempts` (with
 * `retryAfterSeconds` in its body).
 */
export async function setupAccount(code: string | null, username: string, password: string): Promise<LoginResult> {
  const body: components["schemas"]["SetupRequest"] = code === null ? { username, password } : { code, username, password };
  const res = await pageApi().POST(SETUP_API_PATH, { body });
  if (!res.data) throw new ApiError(res.error);
  signedIn(res.data);
  return res.data;
}

// ---------------------------------------------------------------------------
// Opened signed in from the device itself
// ---------------------------------------------------------------------------

/**
 * The trade of a `#local-open=` token for a session, while it is under way —
 * {@link authStatus} waits for it — and how it ended when it did not work.
 */
let localOpenFlight: Promise<void> | null = null;
let localOpenError: unknown = null;

/**
 * Trade a local-open token for a session, and keep the session.
 *
 * `remote-hand-server open` on the device (or the desktop app's "open in
 * browser") opens this page with `#local-open=<token>`: single use, alive for
 * a minute, and good only while the device has no account. The token goes in
 * the body, never in a URL, and is never written anywhere: the fragment it came
 * in is gone from the address before anything else runs (`lib/local-open.ts`).
 * A refusal is kept for the page to show ({@link localOpenFailure}) and leaves
 * whatever token the browser already held alone.
 */
export function redeemLocalOpen(token: string): Promise<void> {
  localOpenError = null;
  localOpenFlight = (async () => {
    try {
      // Not a token at all (`local-open.ts` read it as ""): what the server
      // would say, without asking it.
      if (!token) throw new ApiError({ error: "invalid_local_open" });
      const res = await pageApi().POST(LOCAL_OPEN_API_PATH, { body: { token } });
      if (!res.data) throw new ApiError(res.error);
      // Whoever the old token was (a dead one, an earlier owner session), the
      // device copy and the tickets were theirs.
      if (getToken() !== null) clearToken();
      setToken(res.data.token);
      cacheUnseal();
      forgetAuthStatus();
    } catch (error) {
      localOpenError = error;
    }
  })();
  return localOpenFlight;
}

/**
 * At boot, before the router asks who the page is: trade the token a
 * `#local-open=` link brought, if it brought one. The local shell's only —
 * droppen.app reaches a device through Droppen, which a local-open never is.
 */
export function startLocalOpen(): void {
  const token = takeLocalOpen();
  if (token !== null) void redeemLocalOpen(token);
}

/** Why the last `#local-open=` link did not sign the page in, or null. */
export function localOpenFailure(): unknown {
  return localOpenError;
}

/**
 * Revoke this token server-side and drop it locally.
 *
 * The local half happens whatever the server said: a token the user asked to be
 * rid of must not survive a failed request.
 */
export async function logout(): Promise<void> {
  try {
    await pageApi().POST("/api/auth/logout");
  } catch {
    // revoked locally regardless
  }
  clearToken();
  forgetAuthStatus();
  // The caller navigates away next; the copy is gone before it does.
  await cacheClear();
}

// ---------------------------------------------------------------------------
// What a 401 means
// ---------------------------------------------------------------------------

/**
 * Whether a 401 from `requestPath`, seen while the browser is on `currentPath`,
 * should send it to the login page.
 *
 * Two exceptions. A 401 from a password endpoint is a wrong password and belongs
 * in the form that asked for it — throwing the user out of the settings page for
 * mistyping their current password would be absurd, and on the login page itself
 * it is the loop. And a 401 that arrives while the login page is already open
 * has nowhere to go: the page it would navigate to is the page it is on.
 */
export function shouldRedirectToLogin(requestPath: string, currentPath: string): boolean {
  if (PASSWORD_PATHS.includes(requestPath)) return false;
  // The setup page is the other way in, and as much a dead end for a 401: in
  // the setup state everything but its own endpoint answers one.
  return currentPath !== LOGIN_PATH && currentPath !== SETUP_PATH;
}

type AuthAnswer = Pick<AuthStatus, "authRequired" | "setupRequired" | "user" | "authOff">;

/**
 * Whether this caller is one the setup page is for: it has to sign in, nobody
 * is signed in, and the server has no account yet to sign in to.
 */
export function needsSetup(answer: AuthAnswer): boolean {
  return answer.authRequired && !answer.user && answer.setupRequired === true;
}

/** Whether this caller has to sign in, and there is an account to sign in to. */
export function needsLogin(answer: AuthAnswer): boolean {
  return answer.authRequired && !answer.user && !answer.setupRequired;
}

/**
 * Whether authentication is switched off altogether. A server that does not
 * say (`authOff` absent) meant exactly this by `authRequired: false`.
 */
export function authIsOff(answer: AuthAnswer): boolean {
  return answer.authOff ?? !answer.authRequired;
}

/**
 * Whether the setup page has something for this caller: the device has no
 * account yet, and nobody is signed in — or the device's owner is, who makes
 * the first account there without the code.
 */
export function setupOpenTo(answer: AuthAnswer): boolean {
  return answer.setupRequired === true && !authIsOff(answer) && (!answer.user || isLocalOwner(answer.user));
}

/** The page the router guard sends a load to instead of the one asked for. */
export interface AuthRedirect {
  to: "/" | typeof LOGIN_PATH | typeof SETUP_PATH;
  /** For the login page: where to come back to afterwards. */
  next?: string;
}

/**
 * Where a page load goes, given the server's answer to `GET /api/auth/status`
 * and the path (`pathname`, and the whole `href` for coming back) it asked
 * for: nowhere (`null`, draw what was asked for) or one of the entrances.
 *
 *  * {@link needsSetup}: the setup page, from everywhere;
 *  * {@link needsLogin}: the login page, coming back to where it was asked
 *    from — but from the setup page, which is over once an account exists,
 *    with nowhere to come back to;
 *  * anybody else — signed in (`user`: an account, this device's owner by a
 *    local-open session, or a device reached through Droppen), or
 *    authentication off — the app. The login page sends them home; so does
 *    the setup page, unless it still has something for them
 *    ({@link setupOpenTo}: the owner, while the device has no account).
 */
export function authRedirect(answer: AuthAnswer, pathname: string, href: string): AuthRedirect | null {
  if (needsSetup(answer)) return pathname === SETUP_PATH ? null : { to: SETUP_PATH };
  if (needsLogin(answer)) {
    if (pathname === LOGIN_PATH) return null;
    if (pathname === SETUP_PATH) return { to: LOGIN_PATH };
    const next = safeNext(href);
    return next === "/" ? { to: LOGIN_PATH } : { to: LOGIN_PATH, next };
  }
  if (pathname === SETUP_PATH) return setupOpenTo(answer) ? null : { to: "/" };
  if (pathname === LOGIN_PATH) return { to: "/" };
  return null;
}

/** The login URL that comes back to `next` afterwards. */
export function loginHref(next: string): string {
  const target = safeNext(next);
  return target === "/" ? LOGIN_PATH : `${LOGIN_PATH}?next=${encodeURIComponent(target)}`;
}

/**
 * A `next=` turned into somewhere it is safe to go.
 *
 * Only a path on this origin: anything absolute, protocol-relative (`//host`,
 * and `/\host`, which browsers read the same way) or plain nonsense becomes the
 * home page, so a crafted link cannot use the login page as an open redirect.
 *
 * The strip on the first line is the part that is easy to miss and is the whole
 * defence. A URL parser removes ASCII tab, CR and LF from *anywhere* in a URL
 * before it parses it (WHATWG URL, "basic URL parser" step 2), so the string
 * this function inspects is not the string `location.replace` will act on:
 * `/<tab>//evil.example` passes a naive `startsWith("//")` test and is then
 * parsed as `///evil.example`, which is protocol-relative and leaves the origin.
 * Stripping first makes the two agree, and the cleaned string — not the
 * original — is what is handed back, so the caller navigates to what was
 * checked.
 */
export function safeNext(next: string | null | undefined): string {
  if (!next) return "/";
  const cleaned = next.replace(/[\t\n\r]/g, "");
  if (!cleaned.startsWith("/")) return "/";
  // `\` is `/` to a URL parser once it is looking for an authority, so `/\host`
  // and `//host` are the same protocol-relative URL and both are refused.
  if (cleaned[1] === "/" || cleaned[1] === "\\") return "/";
  return cleaned;
}

/**
 * Where a 401 sends this page, given the last status it had and where it is:
 * the setup page when the server has no account to sign in to, otherwise the
 * login page, coming back here afterwards.
 *
 * Under the desktop app always the login page: its window signs in with the
 * app's own account, which does not end the setup state, so the server says
 * `setupRequired` to the window too — and the app answers a move to the
 * login page with a fresh login (`desktop/core/src/web.rs`), never one to the
 * setup page, which asks for a code its user does not have. A browser on the
 * same device that heads for the login page is sent on to the setup page by
 * the guard of the page that loads, which asks the server again.
 */
export function unauthorizedHref(last: Pick<AuthStatus, "authRequired" | "setupRequired" | "user" | "authOff" | "desktopApp"> | null, here: string): string {
  if (last && needsSetup(last) && !last.desktopApp) return SETUP_PATH;
  return loginHref(here);
}

/** One navigation per page, however many requests fail at once. */
let redirecting = false;

/**
 * Act on a 401: drop the dead token and go and get a new one.
 *
 * A hard navigation rather than a router push, because everything running under
 * the dead token — websockets, their reconnect timers, react-query retries —
 * has to stop, and a document swap is the one thing that stops all of it.
 *
 * To the setup page rather than the login page when the last status said the
 * server has no account (as {@link verifyStillAuthorized} finds a server whose
 * accounts are gone): there is nobody to sign in as — but never under the
 * desktop app ({@link unauthorizedHref}). Either way the guard of the page
 * that loads asks again and has the last word.
 */
export function noteUnauthorized(requestPath: string): void {
  if (redirecting) return;
  if (!shouldRedirectToLogin(requestPath, location.pathname)) return;
  clearToken();
  redirecting = true;
  const href = unauthorizedHref(status, location.pathname + location.search);
  // After the device copy is gone, not before: an unload interrupts the
  // clear. Bounded, so a hung database cannot keep the page here.
  void cacheClear().finally(() => location.replace(href));
}
