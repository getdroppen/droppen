/*
 * The hub's HTTP API (docs/hub-api/http.md): accounts and sessions, device
 * registration, the device chain, enrollment and the mailbox. JSON both ways,
 * binary fields as lower-case hex (except WebAuthn's own JSON, base64url:
 * `passkey.ts`), errors as `{error, message}`.
 *
 * None of this decrypts anything: a hub session only says who may use the
 * service. What the page can read is decided by its device keys.
 *
 * Sessions are bound to a device. A password or passkey sign-in (`login`,
 * `signUp`, `loginWithPasskey`, `signUpWithPasskey`) gives a short-lived
 * LOGIN token that can only register a device, read `/v1/me` and
 * (read-only) the chain, and sign out. Everything else takes a
 * DEVICE session, minted only by a fresh signature of the device key over a
 * hub challenge (`sessionChallenge`, `deviceSession`) — never by a bearer
 * token alone. Only the device session is kept in this browser; it is renewed
 * before it expires and once on a 401 (`HubApi.renew`), and a removed
 * device's sessions are refused.
 */
import { createHubClient, type HubClient } from "@/api/hub";
import type { components } from "@/api/hub-schema";
import { type Bytes, fromHex, hex, MAX_PAGE_LEN, RecordError, sha256, utf8 } from "@/lib/e2e";
import { hubHttpOrigin } from "./config";
import type { AuthenticationResponseJSON, CreationOptionsJSON, PasskeyChallenge, RegistrationResponseJSON, RequestOptionsJSON } from "./passkey";
import type { RelayDirectory } from "./relays";
import {
  type AccountMain,
  type AccountPasskey,
  type AccountView,
  type EnrollmentStatus,
  type HostRotationView,
  type HubSession,
  type KeyringEntry,
  type LimitDetail,
  type Mail,
  type PairingMessage,
  type PairingRole,
  type PairingSummary,
  type PasskeySignedIn,
  type Proof,
  type Reauth,
  type RelayTickets,
  type RotationsView,
  type SessionRenewer,
  type SignInMethods,
  type SignedIn,
  type SignupConfig,
  type TouchEnv,
  type WaitlistPlan,
  HubApiError,
  MAIL_PAGE,
  SIGNUP_CONFIG,
  accountMain,
  accountPasskey,
  chainV3View,
  emailStanding,
  hostRotationView,
  machineCopy,
  num,
  passkeyChallenge,
  planView,
  reauthBody,
  registeredDevices,
  rotationsSummary,
  rotationsView,
  signInChange,
  signedIn,
  str,
  usedPasskey,
} from "./api-views";

export {
  CODE_PAIRING_WINDOW_MS,
  HubApiError,
  MAIL_HOST_KEY,
  MAIL_KEY_REQUEST,
  MAIL_MACHINE_KEY,
  MAIL_PAGE,
  MAIL_PAIRING,
  MAIL_USER_KEY,
  PASSWORD_MIN_CHARS,
  SIGNUP_CONFIG,
  accountMain,
  authErrorKind,
  chainV3View,
  chainV3Writable,
  disputePush,
  hostDispute,
  hostRotationView,
  hubErrorKind,
  knownMailPurpose,
  planView,
  rotationPush,
  rotationsSummary,
  rotationsView,
  signInChange,
} from "./api-views";

export type {
  AccountMain,
  AccountPasskey,
  AccountView,
  AuthErrorKind,
  ChainV3View,
  DeletionConfig,
  DisputePush,
  EmailStanding,
  EnrollmentStatus,
  HostDispute,
  HostRotationView,
  HubSession,
  KeyringEntry,
  LimitDetail,
  MachineCopy,
  MachineSummary,
  Mail,
  PairingMessage,
  PairingRole,
  PairingSummary,
  PasskeySignedIn,
  PasskeysConfig,
  PlanView,
  Proof,
  Reauth,
  RegisteredDevice,
  RelayTickets,
  RotationFailure,
  RotationPush,
  RotationState,
  RotationsSummary,
  RotationsView,
  SessionRenewer,
  SignInChange,
  SignInChangeKind,
  SignInMethods,
  SignedIn,
  SignupConfig,
  SignupMode,
  TouchEnv,
  UsedPasskey,
  Verification,
  WaitlistPlan,
} from "./api-views";


const SESSION_KEY = "rh.hub.session";
/**
 * Known-client tokens, one per e-mail signed in with (`known_client`).
 * Kept across logout on purpose: the token only says this
 * browser signed in to the account before, so its owner's next login is not
 * held back by others' failures; it lets nobody in without the password.
 * Clearing it on logout would hand that protection away exactly when the
 * owner comes back, and a stolen one is worth little: after three failures
 * in an hour the hub treats it as no token at all.
 */
const KNOWN_CLIENT_KEY = "rh.hub.knownClientHash:";
/**
 * Where they were kept before: under the e-mail itself, readable by anyone at
 * this browser (`rh.hub.knownClient:<e-mail>`). Moved to the hashed key as the
 * page starts ({@link migrateKnownClients}) or as the e-mail is next used.
 */
const LEGACY_KNOWN_CLIENT_KEY = "rh.hub.knownClient:";
/** What the e-mail is hashed with: a label of its own, so the hash is of no use elsewhere. */
const KNOWN_CLIENT_LABEL = "droppen/v1 known-client e-mail\0";

/** The storage the known-client tokens live in: reads and writes, and removals and a listing where it has them. */
export type KnownClientStorage = Pick<Storage, "getItem" | "setItem"> & Partial<Pick<Storage, "removeItem" | "key" | "length">>;

const normalEmail = (email: string) => email.trim().toLowerCase();

/**
 * The key `email`'s token is kept under: a hash of the e-mail, not the e-mail,
 * so the storage does not say who signed in here. `null` without WebCrypto
 * (an insecure context, where nothing of cloud mode works): no token is kept.
 */
export async function knownClientKey(email: string): Promise<string | null> {
  try {
    return `${KNOWN_CLIENT_KEY}${hex(await sha256(utf8(KNOWN_CLIENT_LABEL), utf8(normalEmail(email))))}`;
  } catch {
    return null;
  }
}

/** A key from before e-mails were hashed. */
function legacyKnownClientKey(email: string): string {
  return `${LEGACY_KNOWN_CLIENT_KEY}${normalEmail(email)}`;
}

/**
 * Move every token kept under its e-mail (`rh.hub.knownClient:<e-mail>`) to
 * the hashed key; a token already there (a newer one) stays. Returns how many
 * were moved. Never throws: what cannot be moved now is moved when its e-mail
 * is next used.
 */
export async function migrateKnownClients(storage: KnownClientStorage | undefined = pageStorage()): Promise<number> {
  if (!storage || typeof storage.key !== "function" || typeof storage.length !== "number" || typeof storage.removeItem !== "function") return 0;
  let moved = 0;
  try {
    const legacy: string[] = [];
    for (let i = 0; i < storage.length; i++) {
      const name = storage.key(i);
      if (name?.startsWith(LEGACY_KNOWN_CLIENT_KEY)) legacy.push(name);
    }
    for (const name of legacy) {
      const token = storage.getItem(name);
      const key = await knownClientKey(name.slice(LEGACY_KNOWN_CLIENT_KEY.length));
      if (key === null) return moved;
      if (token && storage.getItem(key) === null) storage.setItem(key, token);
      storage.removeItem(name);
      moved++;
    }
  } catch {
    // the rest moves as each e-mail is next used
  }
  return moved;
}

/** This page's `localStorage`, where it may be had. */
function pageStorage(): Storage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

/**
 * The device session kept in this browser — even an expired one: it names the
 * account and device to renew a session for (with a new signature; its token
 * alone is worth nothing then). A login token, or a session from before
 * sessions were bound to devices, is not kept.
 */
export function loadSession(storage: Storage | undefined = globalThis.localStorage): HubSession | null {
  try {
    const raw = storage?.getItem(SESSION_KEY);
    if (!raw) return null;
    const s = JSON.parse(raw) as Partial<HubSession>;
    if (typeof s.account !== "string" || typeof s.token !== "string" || typeof s.expiresMs !== "number" || typeof s.device !== "string" || !s.device) return null;
    return { account: s.account, token: s.token, expiresMs: s.expiresMs, device: s.device };
  } catch {
    return null;
  }
}

/** Keep a device session (a login token is never kept). */
export function saveSession(session: HubSession, storage: Storage | undefined = globalThis.localStorage): void {
  if (!session.device) return;
  try {
    storage?.setItem(SESSION_KEY, JSON.stringify({ account: session.account, token: session.token, expiresMs: session.expiresMs, device: session.device }));
  } catch {
    // memory only
  }
}

export function clearSession(storage: Storage | undefined = globalThis.localStorage): void {
  try {
    storage?.removeItem(SESSION_KEY);
  } catch {
    // nothing stored
  }
}
type Fetch = (input: string, init?: RequestInit) => Promise<Response>;

/**
 * Whether this page says "Macintosh" yet has a touch screen: an iPad asking
 * for desktop sites (iPadOS Safari's default), since no Mac has one. Never
 * throws: no, when in doubt.
 */
export function touchScreenMac(env: TouchEnv = globalThis as unknown as TouchEnv): boolean {
  try {
    const nav = env.navigator;
    return !!nav && /Macintosh/.test(nav.userAgent ?? "") && (nav.maxTouchPoints ?? 0) > 1;
  } catch {
    return false;
  }
}
/**
 * The hub's HTTP API, at `base` — the page's own origin by default, or the
 * marker's `https://` form when the shell names another hub
 * (`hubHttpOrigin()`: the browser calls `https://api.droppen.app`
 * cross-origin from `my.droppen.app`).
 */
export class HubApi {
  /** Set by the account: re-establish the device session once on a 401. */
  renew: SessionRenewer | null = null;

  /** The typed client (`openapi/hub.json`): the migrated calls go through it. */
  private readonly client: HubClient;

  /** The move of the tokens kept under their e-mails, started as the page's API is made. */
  private readonly migrated: Promise<unknown>;

  constructor(
    private readonly base = hubHttpOrigin() ?? "",
    private readonly fetchImpl: Fetch = (input, init) => globalThis.fetch(input, init),
    private readonly storage: KnownClientStorage | undefined = pageStorage(),
  ) {
    this.migrated = migrateKnownClients(storage);
    this.client = createHubClient(this.base, this.fetchImpl);
  }

  /** The known-client token a sign-in to the account with `email` gave this browser, if any (one kept under the e-mail moves to its hash). */
  private async knownClient(email: string): Promise<string | null> {
    await this.migrated;
    const key = await knownClientKey(email);
    if (key === null || !this.storage) return null;
    try {
      const kept = this.storage.getItem(key);
      if (kept !== null) return kept;
      const legacy = this.storage.getItem(legacyKnownClientKey(email));
      if (legacy === null) return null;
      this.storage.setItem(key, legacy);
      this.storage.removeItem?.(legacyKnownClientKey(email));
      return legacy;
    } catch {
      return null;
    }
  }

  /** Keep the hub's known-client token for `email` (each sign-in gives a fresh one), under the e-mail's hash. */
  private async keepKnownClient(email: string, token: unknown): Promise<void> {
    if (typeof token !== "string" || !token) return;
    await this.migrated;
    const key = await knownClientKey(email);
    if (key === null || !this.storage) return;
    try {
      this.storage.setItem(key, token);
      this.storage.removeItem?.(legacyKnownClientKey(email));
    } catch {
      // memory only: the next login just goes without it
    }
  }

  /** Send with `token`, and once more with a renewed device session if the hub said 401. */
  private async send(method: string, path: string, opts: { token?: string; body?: unknown; noRenew?: boolean }): Promise<Response> {
    const go = (token: string | undefined) => {
      const headers: Record<string, string> = {};
      if (opts.body !== undefined) headers["content-type"] = "application/json";
      if (token) headers.authorization = `Bearer ${token}`;
      return this.fetchImpl(`${this.base}${path}`, { method, headers, body: opts.body === undefined ? undefined : JSON.stringify(opts.body) });
    };
    const res = await go(opts.token);
    if (res.status !== 401 || !opts.token || opts.noRenew || !this.renew) return res;
    let fresh: string | null = null;
    try {
      fresh = await this.renew(opts.token);
    } catch {
      // the 401 stands
    }
    return fresh ? go(fresh) : res;
  }

  private static async fail(res: Response): Promise<never> {
    let d: unknown = {};
    try {
      d = (await res.json()) ?? {};
    } catch {
      // an empty or non-JSON body
    }
    return HubApi.failBody(res, d);
  }

  /** The error from a body the caller already read (openapi-fetch parses it). */
  private static failBody(res: Response, body: unknown): never {
    const d = (body !== null && typeof body === "object" ? body : {}) as {
      error?: string;
      message?: string;
      retry_after_ms?: unknown;
      key?: unknown;
      limit?: unknown;
      used?: unknown;
      hosts?: unknown;
      grant?: unknown;
      reason?: unknown;
      ready_ms?: unknown;
      waiting?: unknown;
      until_ms?: unknown;
    };
    let retryAfterMs: number | undefined;
    if (typeof d.retry_after_ms === "number" && Number.isFinite(d.retry_after_ms) && d.retry_after_ms >= 0) retryAfterMs = d.retry_after_ms;
    else {
      const seconds = Number(res.headers?.get("retry-after") ?? NaN);
      if (Number.isFinite(seconds) && seconds >= 0) retryAfterMs = seconds * 1000;
    }
    const limit: LimitDetail | undefined =
      d.error === "limit" && typeof d.key === "string" ? { key: d.key, limit: num(d.limit), used: num(d.used) ?? 0 } : undefined;
    const hostList = (v: unknown) => (Array.isArray(v) ? v.filter((h): h is string => typeof h === "string") : undefined);
    const waitingHosts = d.error === "chain_v3_not_ready" ? hostList(d.hosts) : d.error === "rotation_not_ready" ? hostList(d.waiting) : undefined;
    const grant = d.error === "unlock_on" && typeof d.grant === "string" && d.grant ? d.grant : undefined;
    const reason = (d.error === "rotation_not_ready" || d.error === "rotation_in_progress") && typeof d.reason === "string" ? d.reason : undefined;
    const readyMs = d.error === "rotation_not_ready" ? (num(d.ready_ms) ?? undefined) : undefined;
    const untilMs = d.error === "recently_deleted" ? (num(d.until_ms) ?? undefined) : undefined;
    throw new HubApiError(res.status, d.error ?? "error", d.message || res.statusText || `${d.error ?? "error"} (${res.status})`, {
      retryAfterMs,
      limit,
      waitingHosts,
      grant,
      reason,
      readyMs,
      untilMs,
    });
  }

  private async call<T>(method: string, path: string, opts: { token?: string; body?: unknown; noRenew?: boolean } = {}): Promise<T> {
    const res = await this.send(method, path, opts);
    if (res.status === 204) return undefined as T;
    if (!res.ok) return HubApi.fail(res);
    try {
      return (await res.json()) as T;
    } catch {
      return null as T;
    }
  }

  /**
   * One call through the typed client, with `send`'s 401 behaviour: once,
   * with a renewed device session.
   */
  private async typed<T>(
    call: (headers: Record<string, string>) => Promise<{ data?: T; error?: unknown; response: Response }>,
    opts: { token?: string; noRenew?: boolean } = {},
  ): Promise<T> {
    const send = (token: string | undefined) => call(token ? { authorization: `Bearer ${token}` } : {});
    const out = await send(opts.token);
    if (out.response.status !== 401 || !opts.token || opts.noRenew || !this.renew) return HubApi.done(out);
    let fresh: string | null = null;
    try {
      fresh = await this.renew(opts.token);
    } catch {
      // the 401 stands
    }
    return HubApi.done(fresh ? await send(fresh) : out);
  }

  private static done<T>(out: { data?: T; error?: unknown; response: Response }): T {
    if (out.response.status === 204) return undefined as T;
    if (!out.response.ok) return HubApi.failBody(out.response, out.error);
    return (out.data ?? null) as T;
  }

  /**
   * What sign-up asks for, as droppen.app answers it ([`SIGNUP_CONFIG`]). It
   * used to be asked of the hub (`GET /v1/signup/config`); one service, one
   * page, so the answers are written here and the request is gone. Sites
   * served by another hub show the same answers.
   */
  async signupConfig(): Promise<SignupConfig> {
    return { ...SIGNUP_CONFIG, passkeys: SIGNUP_CONFIG.passkeys && { ...SIGNUP_CONFIG.passkeys }, deletion: SIGNUP_CONFIG.deletion && { ...SIGNUP_CONFIG.deletion } };
  }

  /**
   * Join the waitlist (droppen.app/pricing): an address and the plan it waits
   * for. No sign-in, no confirmation mail: submitting is joining. The hub
   * answers the same whether the address is new or already on the list, so
   * nothing comes back. Refused: 400 `bad_request` (not an address, not a
   * plan), 400 `disposable_email`, 429 `rate` with how long to wait.
   */
  async joinWaitlist(input: { email: string; plan: WaitlistPlan; locale?: string }): Promise<void> {
    const body: components["schemas"]["Join"] = { email: input.email, plan: input.plan };
    if (input.locale) body.locale = input.locale;
    await this.typed<components["schemas"]["Waitlisted"]>((headers) => this.client.POST("/v1/waitlist", { body, headers }));
  }

  /**
   * Sign up: a short-lived LOGIN token (register a device, then get a device
   * session) — once the e-mail is verified, where the hub requires it.
   * `invite` only when given (an open hub takes none); `turnstile` is the
   * widget's token; `lang` the language the verification mail is written in.
   */
  async signUp(email: string, password: string, opts: { invite?: string; turnstile?: string | null; lang?: string } = {}): Promise<SignedIn> {
    const body: components["schemas"]["SignUp"] = { email, password };
    if (opts.invite) body.invite = opts.invite;
    if (opts.turnstile) body.turnstile = opts.turnstile;
    if (opts.lang) body.lang = opts.lang;
    const r = await this.typed<components["schemas"]["SignedIn"]>((headers) => this.client.POST("/v1/signup", { body, headers }));
    await this.keepKnownClient(email, r?.known_client);
    return signedIn(r);
  }

  /**
   * Sign in with the password: a short-lived LOGIN token (register a device,
   * then get a device session). 403 `turnstile_required` after many failures:
   * sign in again with the widget's token. The known-client token an earlier
   * sign-in gave this browser goes along: a hub without Turnstile then does
   * not hold it back for others' failures (429 `rate` otherwise).
   */
  async login(email: string, password: string, turnstile?: string | null): Promise<SignedIn> {
    const body: components["schemas"]["Login"] = { email, password };
    if (turnstile) body.turnstile = turnstile;
    const known = await this.knownClient(email);
    if (known) body.known_client = known;
    const r = await this.typed<components["schemas"]["SignedIn"]>((headers) => this.client.POST("/v1/login", { body, headers }));
    await this.keepKnownClient(email, r?.known_client);
    return signedIn(r);
  }

  /**
   * Passkey sign-up, step 1 (docs/passkeys.md §3.4 A): every check a sign-up
   * gets (rate, the widget's token, the e-mail, the invite, whether the
   * e-mail is taken) before any passkey exists, then the options to make one
   * with. Refused (409 `email_taken`, 403 `bad_invite`, …): make none.
   */
  async passkeySignupOptions(input: { email: string; invite?: string; turnstile?: string | null; lang?: string }): Promise<PasskeyChallenge<CreationOptionsJSON>> {
    const body: components["schemas"]["Options"] = { purpose: "signup", email: input.email };
    if (input.invite) body.invite = input.invite;
    if (input.turnstile) body.turnstile = input.turnstile;
    if (input.lang) body.lang = input.lang;
    const r = await this.typed<components["schemas"]["ChallengeOptions"]>((headers) => this.client.POST("/v1/passkeys/options", { body, headers }));
    return passkeyChallenge(r, "create") as PasskeyChallenge<CreationOptionsJSON>;
  }

  /**
   * Passkey sign-up, step 2: the new passkey for the challenge it answered,
   * and the account. The same answer as a password sign-up (`signUp`).
   */
  async signUpWithPasskey(input: { challenge: string; email: string; invite?: string; lang?: string; credential: RegistrationResponseJSON }): Promise<SignedIn> {
    const body: components["schemas"]["PasskeySignUp"] = {
      challenge: input.challenge,
      email: input.email,
      // The schema types WebAuthn's own JSON as an empty record; the browser's answer goes as it is.
      credential: input.credential as unknown as components["schemas"]["PasskeySignUp"]["credential"],
    };
    if (input.invite) body.invite = input.invite;
    if (input.lang) body.lang = input.lang;
    const r = await this.typed<components["schemas"]["SignedIn"]>((headers) => this.client.POST("/v1/signup/passkey", { body, headers }));
    await this.keepKnownClient(input.email, r?.known_client);
    return signedIn(r);
  }

  /** A sign-in challenge for a passkey, and the options to ask for one with (no credential named: the person picks). */
  async passkeyLoginOptions(): Promise<PasskeyChallenge<RequestOptionsJSON>> {
    // The schema models no body here; the hub still takes an empty one.
    const r = await this.typed<components["schemas"]["ChallengeOptions"]>((headers) => this.client.POST("/v1/passkeys/login/options", { body: {} as never, headers }));
    return passkeyChallenge(r, "get") as PasskeyChallenge<RequestOptionsJSON>;
  }

  /**
   * Sign in with a passkey's answer: the same LOGIN token as a password
   * sign-in, and which passkey it was. No e-mail on this side, so no
   * known-client token is kept. 401 `unknown_passkey`: the hub knows no such
   * passkey; 401 `bad_passkey`: any other check failed.
   */
  async loginWithPasskey(challenge: string, credential: AuthenticationResponseJSON): Promise<PasskeySignedIn> {
    const body: components["schemas"]["PasskeyLogin"] = { challenge, credential: credential as unknown as components["schemas"]["PasskeyLogin"]["credential"] };
    const r = await this.typed<components["schemas"]["SignedIn"]>((headers) => this.client.POST("/v1/login/passkey", { body, headers }));
    return { ...signedIn(r), passkey: usedPasskey(r?.passkey) };
  }

  // ---------------------------------------------------------- the account's ways to sign in (K2)

  /** The account's passkeys and whether it has a password (any current device). */
  async signInMethods(token: string): Promise<SignInMethods> {
    const r = await this.typed<components["schemas"]["PasskeyList"]>((headers) => this.client.GET("/v1/passkeys", { headers }), { token });
    return {
      passkeys: Array.isArray(r?.passkeys) ? r.passkeys.map(accountPasskey).filter((p): p is AccountPasskey => p !== null) : [],
      password: r?.password === true,
      max: num(r?.max) ?? 10,
    };
  }

  /** Adding a passkey, step 1: with proof it is still the person, the options to make one with (a current full device). */
  async passkeyAddOptions(token: string, proof: Reauth): Promise<PasskeyChallenge<CreationOptionsJSON>> {
    const body: components["schemas"]["Proof"] = reauthBody(proof);
    const r = await this.typed<components["schemas"]["ChallengeOptions"]>((headers) => this.client.POST("/v1/passkeys/add/options", { body, headers }), { token });
    return passkeyChallenge(r, "create") as PasskeyChallenge<CreationOptionsJSON>;
  }

  /** Adding a passkey, step 2: the new passkey for the challenge it answered. */
  async addPasskey(token: string, input: { challenge: string; label?: string; credential: RegistrationResponseJSON }): Promise<AccountPasskey> {
    const body: components["schemas"]["Add"] = { challenge: input.challenge, credential: input.credential as unknown as components["schemas"]["Add"]["credential"] };
    if (input.label) body.label = input.label;
    const r = await this.typed<components["schemas"]["PasskeyView"]>((headers) => this.client.POST("/v1/passkeys", { body, headers }), { token });
    const passkey = accountPasskey(r);
    if (!passkey) throw new HubApiError(502, "bad_passkey_row", "the hub's answer names no passkey");
    return passkey;
  }

  async renamePasskey(token: string, id: string, label: string): Promise<AccountPasskey> {
    const body: components["schemas"]["Rename"] = { label };
    const r = await this.typed<components["schemas"]["PasskeyView"]>(
      (headers) => this.client.PATCH("/v1/passkeys/{id}", { params: { path: { id } }, body, headers }),
      { token },
    );
    const passkey = accountPasskey(r);
    if (!passkey) throw new HubApiError(502, "bad_passkey_row", "the hub's answer names no passkey");
    return passkey;
  }

  /** With proof it is still the person. 409 `last_credential`: the account's last way to sign in; 409 `unlock_on`: a slot on the chain names it. */
  async deletePasskey(token: string, id: string, proof: Reauth): Promise<void> {
    const body: components["schemas"]["Proof"] = reauthBody(proof);
    await this.typed<components["schemas"]["PasskeyRemoved"]>(
      (headers) => this.client.DELETE("/v1/passkeys/{id}", { params: { path: { id } }, body, headers }),
      { token },
    );
  }

  /** A re-authentication challenge, and options naming the account's own passkeys (409 `no_passkey`: give the password). */
  async passkeyReauthOptions(token: string): Promise<PasskeyChallenge<RequestOptionsJSON>> {
    // The schema models no body here; the hub still takes an empty one.
    const r = await this.typed<components["schemas"]["ChallengeOptions"]>(
      (headers) => this.client.POST("/v1/passkeys/reauth/options", { body: {} as never, headers }),
      { token },
    );
    return passkeyChallenge(r, "get") as PasskeyChallenge<RequestOptionsJSON>;
  }

  /** Set or change the password, with proof it is still the person (403 `reauth_failed` / `reauth_required`). */
  async setPassword(token: string, password: string, proof: Reauth): Promise<void> {
    const body: components["schemas"]["NewPassword"] = { password, ...reauthBody(proof) };
    await this.typed<components["schemas"]["PasswordState"]>((headers) => this.client.PUT("/v1/password", { body, headers }), { token });
  }

  /** Remove the password (409 `last_credential` without a passkey), with proof it is still the person. */
  async deletePassword(token: string, proof: Reauth): Promise<void> {
    const body: components["schemas"]["Proof"] = reauthBody(proof);
    await this.typed<components["schemas"]["PasswordState"]>((headers) => this.client.DELETE("/v1/password", { body, headers }), { token });
  }

  /**
   * Delete the account, for good (a current full device): `confirm` is its
   * e-mail address as the person typed it (400 `confirm_mismatch` otherwise),
   * with proof it is still the person, as for the password. Every session
   * and socket of the account ends with it. `purged: false`: the hub is still
   * erasing what it stored, and finishes on its own — the account is gone
   * either way.
   */
  async deleteAccount(token: string, confirm: string, proof: Proof): Promise<{ purged: boolean }> {
    const body: components["schemas"]["DeleteAccount"] = { confirm, ...reauthBody(proof) };
    const r = await this.typed<components["schemas"]["AccountDeleted"]>((headers) => this.client.DELETE("/v1/account", { body, headers }), { token });
    return { purged: r?.purged !== false };
  }

  /** Forget the known-client token kept for `email` (its account is gone: the token is worth nothing, and the address need not stay here). */
  async forgetKnownClient(email: string): Promise<void> {
    try {
      const key = await knownClientKey(email);
      if (key !== null) this.storage?.removeItem?.(key);
      this.storage?.removeItem?.(legacyKnownClientKey(email));
    } catch {
      // nothing kept
    }
  }

  /** Send the verification mail again (a login or device session; with the widget's token when Turnstile is on). */
  async resendVerification(token: string, turnstile?: string | null, opts: { noRenew?: boolean } = {}): Promise<{ queued: unknown }> {
    const body: components["schemas"]["Resend"] = turnstile ? { turnstile } : {};
    const r = await this.typed<components["schemas"]["Queued"]>((headers) => this.client.POST("/v1/email/resend", { body, headers }), { token, noRenew: opts.noRenew });
    return { queued: r?.queued ?? null };
  }

  /** The verification link's token (no session: the token is the proof). 400 `bad_token`, 410 `expired`. */
  async verifyEmail(token: string): Promise<{ account: string; verified: boolean }> {
    const body: components["schemas"]["VerifyEmail"] = { token };
    const r = await this.typed<components["schemas"]["Verified"]>((headers) => this.client.POST("/v1/email/verify", { body, headers }));
    return { account: typeof r?.account === "string" ? r.account : "", verified: r?.verified === true };
  }

  /** A challenge to sign for a device session (no bearer: the signature is what counts). */
  async sessionChallenge(account: string, device: string): Promise<Bytes> {
    const body: components["schemas"]["ChallengeFor"] = { account, device };
    const r = await this.typed<components["schemas"]["Challenge"]>((headers) => this.client.POST("/v1/session/challenge", { body, headers }));
    const nonce = fromHex(r.nonce);
    if (nonce.length !== 32) throw new HubApiError(502, "bad_challenge", "the hub's challenge is not 32 bytes");
    return nonce;
  }

  /**
   * A device session for the device that signed the challenge (`HubLogin`,
   * role "device-session"). 403 `device_removed`: the hub says the chain
   * removed it (a claim to check on the chain); 401 `denied`: a bad signature,
   * nonce or device.
   */
  async deviceSession(body: { account: string; device: string; timeMs: number; nonce: Uint8Array; signature: Uint8Array }): Promise<HubSession> {
    const payload: components["schemas"]["NewDeviceSession"] = {
      account: body.account,
      device: body.device,
      time_ms: body.timeMs,
      nonce: hex(body.nonce),
      signature: hex(body.signature),
    };
    const r = await this.typed<components["schemas"]["SignedInDevice"]>((headers) => this.client.POST("/v1/session", { body: payload, headers }));
    if (r.account !== body.account || r.device !== body.device) throw new HubApiError(502, "bad_session", "the hub answered for another account or device");
    return { account: r.account, device: r.device, token: r.token, expiresMs: r.expires_ms };
  }

  logout(token: string): Promise<void> {
    return this.typed<void>((headers) => this.client.POST("/v1/logout", { headers }), { token, noRenew: true });
  }

  /**
   * The account, its e-mail's standing, its plan and its machines (a login or
   * device session). From an iPad asking for desktop sites it says so
   * (`?touch=1`, {@link touchScreenMac}), so the hub names this device an
   * iPad, not a Mac.
   */
  async me(token: string, opts: { noRenew?: boolean; env?: TouchEnv } = {}): Promise<AccountView> {
    const r = await this.typed<components["schemas"]["Me"]>(
      (headers) =>
        this.client.GET("/v1/me", {
          params: touchScreenMac(opts.env) ? { query: { touch: "1" } } : undefined,
          headers,
        }),
      { token, noRenew: opts.noRenew },
    );
    const email = emailStanding(r.email);
    // A device in use is a known client: keep each fresh token the hub gives,
    // so this browser's next password login is never held back for others'.
    if (email.address) await this.keepKnownClient(email.address, r.known_client);
    return {
      account: r.account,
      email,
      plan: planView(r.plan),
      chainSeq: r.chain_seq ?? null,
      chainV3: chainV3View(r.chain_v3),
      machines: (r.machines ?? []).map((m) => ({
        id: m.id,
        signKey: fromHex(m.sign_key),
        dhKey: fromHex(m.dh_key),
        inChain: m.in_chain,
        createdMs: num(m.created_ms),
        lastSeenMs: m.last_seen_ms ?? null,
        presence: m.presence,
        copy: machineCopy(m.copy),
        readsV3: m.reads_v3 === true,
        rotates: m.rotates === true,
      })),
      devices: registeredDevices(r.devices),
      relayStandby: typeof r.relay_prefs?.standby === "boolean" ? r.relay_prefs.standby : null,
      signInChanges: Array.isArray(r.sign_in_changes) ? r.sign_in_changes.flatMap((c) => signInChange(c) ?? []) : [],
      rotations: rotationsSummary(r.rotations),
    };
  }

  /**
   * Keep a standby relay for the whole account, or not (`PATCH
   * /v1/relays/prefs`, a current device of the chain): what the account says
   * after.
   */
  async setRelayStandby(token: string, on: boolean): Promise<boolean> {
    const r = await this.typed<components["schemas"]["RelayPrefs"]>(
      (headers) => this.client.PATCH("/v1/relays/prefs", { body: { standby: on }, headers }),
      { token },
    );
    return r.standby === true;
  }

  /** Register this browser's keys as a new device; `replaces`: the device of the account whose keys this browser lost (metadata). */
  async registerDevice(token: string, signKey: Uint8Array, dhKey: Uint8Array, opts: { replaces?: string | null } = {}): Promise<string> {
    const body: components["schemas"]["NewDevice"] = { sign_key: hex(signKey), dh_key: hex(dhKey) };
    if (opts.replaces) body.replaces = opts.replaces;
    const r = await this.typed<components["schemas"]["Registered"]>(
      (headers) => this.client.POST("/v1/devices", { body, headers }),
      { token },
    );
    return r.device;
  }

  /** The chain from `from` (a device session, or read-only with a login token). */
  async chain(token: string, from: number, opts: { noRenew?: boolean } = {}): Promise<{ entries: Uint8Array[]; seq: number | null }> {
    const r = await this.typed<components["schemas"]["ChainHead"]>(
      (headers) => this.client.GET("/v1/chain", { params: { query: { from } }, headers }),
      { token, noRenew: opts.noRenew },
    );
    return { entries: (r.entries ?? []).map((e) => fromHex(e)), seq: r.seq ?? null };
  }

  async appendChain(token: string, entries: Uint8Array[]): Promise<number> {
    const r = await this.typed<components["schemas"]["Appended"]>(
      (headers) => this.client.POST("/v1/chain", { body: { entries: entries.map((e) => hex(e)) }, headers }),
      { token },
    );
    return r.seq;
  }

  async createEnrollment(token: string, device: string, enrollToken: Uint8Array): Promise<number> {
    const r = await this.typed<components["schemas"]["EnrollmentMade"]>(
      (headers) => this.client.POST("/v1/enrollments", { body: { device, token: hex(enrollToken) }, headers }),
      { token },
    );
    return r.expires_ms;
  }

  async enrollment(token: string, enrollToken: Uint8Array): Promise<EnrollmentStatus> {
    const r = await this.typed<components["schemas"]["EnrollmentState"]>(
      (headers) => this.client.GET("/v1/enrollments/{token}", { params: { path: { token: hex(enrollToken) } }, headers }),
      { token },
    );
    return {
      state: r.state as EnrollmentStatus["state"],
      machine: r.machine ?? undefined,
      signKey: r.sign_key ? fromHex(r.sign_key) : undefined,
      dhKey: r.dh_key ? fromHex(r.dh_key) : undefined,
      proof: r.proof ? fromHex(r.proof) : undefined,
    };
  }

  /**
   * Mail from this session's device (`sender` must be it, and a current full
   * device). 429 `mail_rate` and 507 `mailbox_full` are the hub's caps
   * ({@link hubErrorKind}).
   */
  async mail(token: string, body: { recipient: string; sender: string; purpose: number; epoch: number; sealed: Uint8Array }): Promise<void> {
    const payload: components["schemas"]["NewMail"] = { ...body, sealed: hex(body.sealed) };
    await this.typed<components["schemas"]["MailQueued"]>((headers) => this.client.POST("/v1/mailbox", { body: payload, headers }), { token });
  }

  /** What waits in this session's device's mailbox (sealed to it, or MAC'd for it), every page of it, oldest first. */
  async mailbox(token: string): Promise<Mail[]> {
    const all: Mail[] = [];
    let after = 0;
    // A bound on pages: the hub keeps at most 256 per recipient.
    for (let page = 0; page < 64; page++) {
      const r = await this.typed<components["schemas"]["Mailbox"]>(
        (headers) => this.client.GET("/v1/mailbox", { params: { query: { after, limit: MAIL_PAGE } }, headers }),
        { token },
      );
      const list = Array.isArray(r?.mail) ? r.mail : [];
      for (const m of list) {
        all.push({ id: String(m.id), sender: m.sender, purpose: m.purpose, epoch: m.epoch, sealed: fromHex(m.sealed) });
        const n = Number(m.id);
        if (Number.isSafeInteger(n) && n > after) after = n;
      }
      if (list.length < MAIL_PAGE) break;
    }
    return all;
  }

  /** Delete mail `id` (only this session's device's own). */
  async ackMail(token: string, id: string): Promise<void> {
    // The schema types the id as a number; the string goes as it is (the hub's ids are numbers).
    await this.typed<void>((headers) => this.client.DELETE("/v1/mailbox/{id}", { params: { path: { id: id as unknown as number } }, headers }), { token });
  }

  /** The account's keyring: opaque wrapped-key blobs the hub cannot open. */
  async keys(token: string): Promise<KeyringEntry[]> {
    const r = await this.typed<components["schemas"]["Keyring"]>((headers) => this.client.GET("/v1/keys", { headers }), { token });
    return r.keys.map((k) => ({ name: k.name, blob: fromHex(k.blob), updatedMs: k.updated_ms }));
  }

  /**
   * Put a keyring blob (a current full device only). Names are written once:
   * `stored: false` when the name holds these very bytes already (a retry);
   * other bytes under it are a 409 `exists`.
   */
  async putKey(token: string, name: string, blob: Uint8Array): Promise<{ stored: boolean }> {
    const body: components["schemas"]["NewKeyWrap"] = { blob: hex(blob) };
    // Left on `call`: a name is several path segments (`mk/m-1/0/0/dev`), and openapi-fetch would
    // percent-encode its `/` — the hub's route is `{*name}` and takes the name as written.
    const r = await this.typed<components["schemas"]["KeyPut"]>(
      (headers) => this.client.PUT("/v1/keys/{name}", { params: { path: { name } }, body, headers }),
      { token },
    );
    return { stored: r?.stored !== false };
  }

  /** Open a numeric-code pairing for `device` (this session's own device, registered but not in the chain). */
  async openPairing(token: string, device: string): Promise<string> {
    const body: components["schemas"]["NewPairing"] = { device };
    const r = await this.typed<components["schemas"]["PairingOpened"]>((headers) => this.client.POST("/v1/pairings", { body, headers }), { token });
    return r.id;
  }

  async pairings(token: string): Promise<PairingSummary[]> {
    const r = await this.typed<components["schemas"]["Pairings"]>((headers) => this.client.GET("/v1/pairings", { headers }), { token });
    return r.pairings.map((p) => ({ id: p.id, device: p.device, createdMs: p.created_ms }));
  }

  async postPairingMessage(token: string, id: string, from: PairingRole, body: Uint8Array): Promise<number> {
    const payload: components["schemas"]["PairingMessage"] = { from, body: hex(body) };
    const r = await this.typed<components["schemas"]["PairingPosted"]>(
      (headers) => this.client.POST("/v1/pairings/{id}/messages", { params: { path: { id } }, body: payload, headers }),
      { token },
    );
    return r.n;
  }

  async pairingMessages(token: string, id: string, after: number): Promise<PairingMessage[]> {
    const r = await this.typed<components["schemas"]["PairingMessages"]>(
      (headers) => this.client.GET("/v1/pairings/{id}/messages", { params: { path: { id }, query: { after } }, headers }),
      { token },
    );
    return r.messages.map((m) => ({ n: m.n, from: m.from as PairingRole, body: fromHex(m.body) }));
  }

  async closePairing(token: string, id: string): Promise<void> {
    await this.typed<void>((headers) => this.client.DELETE("/v1/pairings/{id}", { params: { path: { id } }, headers }), { token });
  }

  /**
   * Make `machine` the account's main device (`PUT /v1/main-device`, a
   * current device of the chain only): every device of the account hears it.
   * With `ifUnset`, only while the account has none. The one that stands
   * after — with `ifUnset`, maybe another page's.
   */
  async setMainDevice(token: string, machine: string, opts: { ifUnset?: boolean } = {}): Promise<AccountMain> {
    const body: components["schemas"]["SetMain"] = { machine };
    if (opts.ifUnset) body.if_unset = true;
    const main = accountMain(await this.typed<components["schemas"]["MainDevice"]>((headers) => this.client.PUT("/v1/main-device", { body, headers }), { token }));
    if (!main) throw new HubApiError(502, "bad_main_device", "the hub's main device is not one");
    return main;
  }

  /**
   * The account's identity key renewals (`GET /v1/rotations`, any device
   * session of the account; docs/host-identity-rotation.md §3.3, §3.8): each
   * host's state, the disputes with their evidence, and the account's
   * policy. A hub from before renewals answers 404.
   */
  async rotations(token: string): Promise<RotationsView> {
    const res = await this.send("GET", "/v1/rotations", { token });
    if (!res.ok) return HubApi.fail(res);
    let raw: unknown = null;
    try {
      raw = await res.json();
    } catch {
      // an empty body: nothing
    }
    const view = rotationsView(raw);
    // The hub's clock, for judging the rows' ages (R6 review): `now_ms`, else the answer's Date header.
    if (view.nowMs === null) {
      const date = Date.parse(res.headers?.get("date") ?? "");
      if (Number.isFinite(date)) view.nowMs = date;
    }
    return view;
  }

  /**
   * Register a renewal of host `host` (`PUT /v1/rotations/{host}`, a current
   * admin's device session): without `nonceHash`, only mark it `pending` (it
   * renews when it can: an offline host); with one, open an attempt (valid
   * 10 min) for the request whose nonce hashes to it. 409
   * `rotation_in_progress` (another page's attempt is running), 409
   * `rotation_not_ready` (`reason`: `disabled`, `agent_too_old`,
   * `needs_assurance`), 409 `not_a_host`, 429 `rate`.
   */
  async putRotation(token: string, host: string, opts: { nonceHash?: Uint8Array; assurance: 0 | 1 }): Promise<HostRotationView> {
    const body: components["schemas"]["PutRotation"] = { assurance: opts.assurance };
    if (opts.nonceHash) body.nonce_hash = hex(opts.nonceHash);
    const r = await this.typed<components["schemas"]["RotationPut"]>(
      (headers) => this.client.PUT("/v1/rotations/{host}", { params: { path: { host } }, body, headers }),
      { token },
    );
    const row = hostRotationView(r);
    if (!row || row.host !== host) throw new HubApiError(502, "bad_rotation", "the hub answered for another renewal");
    return row;
  }

  /**
   * Withdraw host `host`'s renewal (`DELETE /v1/rotations/{host}`): back to
   * none, unless a staging is committed. `only` names what is withdrawn: the
   * attempt (or staging) opened with that nonce's hash, or the `pending` mark
   * made at `requestedMs` — anything else the host has by then (another
   * page's newer attempt) stays: 409 `rotation_in_progress`, or `{}` when the
   * one named is gone. A hub from before that withdraws whatever it has.
   */
  async deleteRotation(token: string, host: string, only?: { nonceHash: Uint8Array | string } | { requestedMs: number }): Promise<void> {
    const body = !only ? undefined : "nonceHash" in only ? { nonce_hash: typeof only.nonceHash === "string" ? only.nonceHash : hex(only.nonceHash) } : { requested_ms: only.requestedMs };
    // The response is the documented `{}`; the withdrawal body stays a raw
    // bytes compatibility path (`#[api(skip)]` in the hub), so it is sent as it is.
    await this.call<components["schemas"]["Empty"]>("DELETE", `/v1/rotations/${encodeURIComponent(host)}`, body ? { token, body } : { token });
  }

  /**
   * One snapshot page of a stream, as the hub stores it: sealed bytes it
   * cannot open. Unchecked here — the reader checks it against the signed
   * manifest that names it (`openManifestPage`) before using a byte of it.
   *
   * At most {@link MAX_PAGE_LEN} bytes, whatever the hub sends (the same cap
   * as the Rust device's `read_capped`): a `Content-Length` over it is refused
   * before the body is touched, and a larger body is refused and its reader
   * cancelled as the bytes come in — never buffered whole first. Refused as a
   * `tooLarge` page, which a stream reader handles like any other page it
   * cannot use.
   */
  async streamPage(token: string, stream: Uint8Array, page: Uint8Array): Promise<Bytes> {
    const res = await this.send("GET", `/v1/streams/${hex(stream)}/pages/${hex(page)}`, { token });
    if (!res.ok) return HubApi.fail(res);
    const tooLarge = () => new RecordError("tooLarge", `a page larger than ${MAX_PAGE_LEN} bytes`);
    const declared = res.headers.get("content-length");
    if (declared !== null && Number(declared) > MAX_PAGE_LEN) throw tooLarge();
    if (!res.body) return new Uint8Array();
    const reader = res.body.getReader();
    const chunks: Uint8Array[] = [];
    let total = 0;
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        total += value.length;
        if (total > MAX_PAGE_LEN) {
          await reader.cancel().catch(() => undefined);
          throw tooLarge();
        }
        chunks.push(value);
      }
    } finally {
      reader.releaseLock();
    }
    const out = new Uint8Array(total);
    let at = 0;
    for (const chunk of chunks) {
      out.set(chunk, at);
      at += chunk.length;
    }
    return out;
  }

  /** The relays this account may use, its set and `relay.max`, and its machines' round trips (`GET /v1/relays`). */
  async relays(token: string): Promise<RelayDirectory> {
    const r = await this.typed<components["schemas"]["RelayList"]>((headers) => this.client.GET("/v1/relays", { headers }), { token });
    const list = Array.isArray(r?.relays) ? (r.relays as Record<string, unknown>[]) : [];
    const set = Array.isArray(r?.account_set) ? (r.account_set as unknown as Record<string, unknown>[]) : [];
    const agents: Record<string, Record<string, number>> = {};
    if (r?.agents && typeof r.agents === "object") {
      for (const [machine, rtts] of Object.entries(r.agents as Record<string, unknown>)) {
        if (!rtts || typeof rtts !== "object") continue;
        agents[machine] = {};
        for (const [relay, ms] of Object.entries(rtts as Record<string, unknown>)) {
          if (typeof ms === "number" && Number.isFinite(ms)) agents[machine][relay] = ms;
        }
      }
    }
    return {
      relays: list
        .filter((x) => typeof x.id === "string" && typeof x.origin === "string")
        .map((x) => ({
          id: x.id as string,
          city: str(x.city) ?? (x.id as string),
          region: str(x.region) ?? "",
          origin: x.origin as string,
          state: str(x.state) ?? "up",
          load: num(x.load),
        })),
      limit: num(r?.limit) ?? 0,
      accountSet: set.filter((x) => typeof x.id === "string").map((x) => ({ id: x.id as string, lastUsedMs: num(x.last_used_ms) ?? 0 })),
      emergency: Array.isArray(r?.emergency) ? (r.emergency as unknown[]).filter((x): x is string => typeof x === "string") : [],
      agents,
      throttled: r?.throttled === true,
    };
  }

  /** Tickets for `relays` (`POST /v1/relays/tickets`): each, or why not. */
  async relayTickets(token: string, relays: string[]): Promise<RelayTickets> {
    const body: components["schemas"]["TicketRequest"] = { relays };
    const r = await this.typed<components["schemas"]["Tickets"]>((headers) => this.client.POST("/v1/relays/tickets", { body, headers }), { token });
    const tickets = Array.isArray(r?.tickets) ? (r.tickets as unknown as Record<string, unknown>[]) : [];
    const errors = Array.isArray(r?.errors) ? (r.errors as unknown as Record<string, unknown>[]) : [];
    return {
      tickets: tickets
        .filter((t) => typeof t.relay === "string" && typeof t.ticket === "string" && typeof t.origin === "string")
        .map((t) => ({
          relay: t.relay as string,
          origin: t.origin as string,
          ticket: t.ticket as string,
          expiresMs: num(t.expires_ms) ?? 0,
          emergency: t.emergency === true,
          throttled: t.throttled === true,
        })),
      errors: errors
        .filter((e) => typeof e.relay === "string")
        .map((e) => ({
          relay: e.relay as string,
          error: str(e.error) ?? "error",
          limit: num(e.limit),
          accountSet: Array.isArray(e.account_set) ? (e.account_set as unknown[]).filter((x): x is string => typeof x === "string") : [],
        })),
    };
  }
}

/**
 * The hub could not be reached at all (no answer: offline, a dropped
 * connection, DNS): `fetch`'s own `TypeError`, whose words differ by browser
 * (Chrome "Failed to fetch", Safari "Load failed", Firefox "NetworkError when
 * attempting to fetch resource.").
 */
export function isNetworkError(error: unknown): boolean {
  return error instanceof TypeError && /failed to fetch|load failed|networkerror|network request failed|network connection was lost/i.test(error.message);
}
