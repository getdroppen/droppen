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
import { type Bytes, fromHex, hex, sha256, utf8 } from "@/lib/e2e";
import { hubHttpOrigin } from "./config";
import type { AuthenticationResponseJSON, CreationOptionsJSON, PasskeyChallenge, RegistrationResponseJSON, RequestOptionsJSON } from "./passkey";
import type { RelayDirectory } from "./relays";

export interface HubSession {
  account: string;
  token: string;
  expiresMs: number;
  /** The device this session belongs to; absent for a password sign-in's short-lived login token. */
  device?: string;
}

/** An entitlement the hub says is used up (409 `limit`: relay-and-plans §3.3). */
export interface LimitDetail {
  /** The entitlement, e.g. `devices.max`, `machines.max`, `storage.bytes`. */
  key: string;
  /** Its value (`null`: none — the hub would not refuse then, but the shape allows it). */
  limit: number | null;
  used: number;
}

export class HubApiError extends Error {
  /** How long to wait before trying again, when the hub said (a 429's `retry_after_ms`). */
  readonly retryAfterMs?: number;
  /** What a 409 `limit` says is used up. */
  readonly limit?: LimitDetail;
  /**
   * The hosts a 409 `chain_v3_not_ready` waits for (their agents do not read
   * chain v3 yet) — or a 409 `rotation_not_ready`'s `waiting` (hosts after
   * the batch that have not declared `rotate/1`).
   */
  readonly waitingHosts?: string[];
  /** A 409 `unlock_on`'s grant: the proof was taken; with the slot off, the same removal goes through with it (two minutes, once). */
  readonly grant?: string;
  /**
   * Why a 409 `rotation_not_ready` (docs/host-identity-rotation.md §3.3):
   * `disabled` (`DROPPEN_HUB_HOST_ROTATION` is off), `agent_too_old` (the
   * host never declared `rotate/1`), `needs_assurance`, `waiting`,
   * `not_staged`, `mismatch`, `window` (with {@link readyMs}).
   */
  readonly reason?: string;
  /** When a renewal's window closes, by the hub's clock (409 `rotation_not_ready` `window`). */
  readonly readyMs?: number;
  /** When a deleted account's address may sign up again, by the hub's clock (409 `recently_deleted`). */
  readonly untilMs?: number;

  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    extra: { retryAfterMs?: number; limit?: LimitDetail; waitingHosts?: string[]; grant?: string; reason?: string; readyMs?: number; untilMs?: number } = {},
  ) {
    super(message);
    this.name = "HubApiError";
    if (extra.retryAfterMs !== undefined) this.retryAfterMs = extra.retryAfterMs;
    if (extra.limit !== undefined) this.limit = extra.limit;
    if (extra.waitingHosts !== undefined) this.waitingHosts = extra.waitingHosts;
    if (extra.grant !== undefined) this.grant = extra.grant;
    if (extra.reason !== undefined) this.reason = extra.reason;
    if (extra.readyMs !== undefined) this.readyMs = extra.readyMs;
    if (extra.untilMs !== undefined) this.untilMs = extra.untilMs;
  }
}

/** A renewal's state at the hub (`GET /v1/rotations`, docs/host-identity-rotation.md §3.3). */
export type RotationState = "pending" | "attempt" | "staged" | "committed";

/** Why an attempt failed: the host still has to renew (its row goes back to `pending`). */
export interface RotationFailure {
  /** `not_acked` / `conflict`: another holder of the old key showed up (clone evidence: `assurance = 1` only from now on); `expired`; `refused`. */
  reason: string;
  atMs: number;
}

/** One host's renewal as the hub keeps it — a claim: what lands is checked on the chain. */
export interface HostRotationView {
  host: string;
  state: RotationState;
  assurance: 0 | 1;
  requestedMs: number | null;
  requestedBy: string | null;
  attemptExpiresMs: number | null;
  stagedMs: number | null;
  /** When the staging's window closes (the hub's clock): the renewal may be written then. */
  readyMs: number | null;
  signKey: Bytes | null;
  dhKey: Bytes | null;
  stagedSeq: number | null;
  committedMs: number | null;
  committedSeq: number | null;
  failure: RotationFailure | null;
  /** Clone evidence within 30 days: the hub takes `assurance = 1` only. */
  needsAssurance: boolean;
  /** The hub's view: a key retired by an `assurance = 0` renewal signed in again, and no `assurance = 1` renewal since. */
  isolated: boolean;
  /** While `attempt` or `staged`: the hash of that attempt's nonce (hex) — what names it to withdraw it ({@link HubApi.deleteRotation}). */
  nonceHash: string | null;
}

/**
 * What the hub recorded against a host (§3.8): `disputed` — a statement signed by the key a renewal retired,
 * disputing that renewal (`seq`, `entryHash`, `sig`: checked on this page's chain, it isolates); `retired_login` —
 * a sign-in with a retired key (the signed `HubLogin`: information only, never isolation); `not_acked` / `conflict`
 * — a staging another holder voided.
 */
export interface HostDispute {
  host: string;
  kind: "disputed" | "retired_login" | "not_acked" | "conflict";
  seenMs: number;
  /** The chain seq of the renewal that retired the key. */
  retiredSeq: number | null;
  role: "agent" | "device" | null;
  login: { account: string; id: string; hub: string; nonce: Bytes; timeMs: number } | null;
  sig: Bytes | null;
  signKey: Bytes | null;
  /** `disputed`: the disputed `rotateHostKey`'s seq, and its chain hash as the hub has it. */
  seq: number | null;
  entryHash: Bytes | null;
}

/** `GET /v1/rotations`: the account's renewals, disputes and policy. */
export interface RotationsView {
  /** `DROPPEN_HUB_HOST_ROTATION` is on: renewals are taken. */
  enabled: boolean;
  /** The confirmation window T. */
  windowMs: number;
  hosts: HostRotationView[];
  disputes: HostDispute[];
  /** The hub's clock when it answered (`now_ms`, else the Date header; `null`: neither): what the rows' ages are judged by, never the page's clock. */
  nowMs: number | null;
}

/** `/v1/me`'s `rotations` summary (`null`: a hub that predates renewals). */
export interface RotationsSummary {
  enabled: boolean;
  windowMs: number;
  active: { host: string; state: RotationState }[];
}

/** The hub's `rotation` push on `/v1/client`: a host's renewal changed state (`reason`: why an attempt failed). */
export interface RotationPush {
  host: string;
  /** `none`: withdrawn (`DELETE /v1/rotations/{host}`), or nothing to renew any more. */
  state: RotationState | "none";
  reason: string | null;
}

/** The hub's `host_dispute` push: evidence to fetch (`GET /v1/rotations`) and check. */
export interface DisputePush {
  host: string;
  kind: string;
  seenMs: number;
}

/** A plan someone may wait for on the waitlist. */
export type WaitlistPlan = "free" | "pro";

/** Who may sign up at this hub (`RH_HUB_SIGNUP`). */
export type SignupMode = "invite" | "open" | "closed";

/** Whether this hub requires accounts to verify their e-mail. */
export type Verification = "required" | "off";

/** The Turnstile widget's page, on its own origin (never the app's: its script must not load here). */
export interface TurnstileConfig {
  siteKey: string;
  /** e.g. `https://verify.droppen.app`, normalised to a bare origin. */
  origin: string;
}

/** Passkeys at this hub (docs/passkeys.md §3.1): the RP ID they are made for. */
export interface PasskeysConfig {
  rpId: string;
}

/** What sign-up asks for (`GET /v1/signup/config`). */
export interface SignupConfig {
  signup: SignupMode;
  inviteRequired: boolean;
  turnstile: TurnstileConfig | null;
  verification: Verification;
  /** `null`: this hub offers no passkeys (or predates them), and the page says nothing of them. */
  passkeys: PasskeysConfig | null;
  /** What deleting an account leaves behind (absent: the hub predates account deletion, or did not say). */
  deletion?: DeletionConfig | null;
}

/** What deleting an account leaves behind (`GET /v1/signup/config` `deletion`). */
export interface DeletionConfig {
  /** For how many days a deleted account's address cannot sign up again (0: it can at once). */
  emailHoldDays: number;
}

/** A password sign-in or sign-up: a LOGIN token, and whether the account's e-mail still needs verifying. */
export interface SignedIn extends HubSession {
  emailVerified: boolean;
  verification: Verification;
}

/** The passkey a sign-in used, as the hub names it. */
export interface UsedPasskey {
  id: string;
  label: string;
  /** Whether a new device can join with it (a current passkey slot names it, §4.4; a hint only, the chain decides). `none`: a hub from before slots. */
  unlock: "on" | "off" | "paused" | "none";
}

/** A passkey sign-in: the same LOGIN token as a password's, and the passkey it used (`null`: the hub did not say). */
export interface PasskeySignedIn extends SignedIn {
  passkey: UsedPasskey | null;
  /** The credential id the passkey answered with (base64url): which passkey this browser holds, for joining with it (§4.6). */
  credentialId?: string;
}

/** One of the account's passkeys, as `GET /v1/passkeys` lists it (docs/passkeys.md §3.6). */
export interface AccountPasskey {
  id: string;
  label: string;
  /** Its provider's name by AAGUID ("iCloud Keychain", "Google Password Manager", …), else "Passkey". */
  provider: string;
  /** base64url: what the page asks this very passkey for its PRF with. */
  credentialId: string;
  /** hex SHA-256 of the credential id: what a passkey slot on the chain names. */
  credHash: string;
  /** BE: it may sync (a passkey manager's); BS: it does, as of its last use. */
  backupEligible: boolean;
  backupState: boolean;
  /** What the browser said of PRF when it was made: a hint only. */
  prf: "yes" | "no" | "unknown";
  createdMs: number;
  /** The device that added it (`null`: made at sign-up). */
  createdBy: string | null;
  lastUsedMs: number | null;
  /** The hub's reading of the chain: a current slot names it (a new device can join with it). The page goes by the chain itself. */
  unlock: "on" | "off";
}

/** How the account signs in: its passkeys, whether it has a password, how many passkeys it may have. */
export interface SignInMethods {
  passkeys: AccountPasskey[];
  password: boolean;
  max: number;
}

/**
 * Proof it is still the person, for changing a way to sign in: the current
 * password, or a passkey's answer to a re-authentication challenge — or, for
 * a passkey's removal only, the grant a 409 `unlock_on` gave once the proof
 * was taken.
 */
export type Reauth = { currentPassword: string } | { passkey: { challenge: string; credential: AuthenticationResponseJSON } } | { grant: string };

/** Proof it is still the person, a grant aside: what deleting the account takes. */
export type Proof = Exclude<Reauth, { grant: string }>;

/** The account's e-mail as `/v1/me` tells it. */
export interface EmailStanding {
  address: string;
  verified: boolean;
  verification: Verification;
  /** Unverified: when adding devices and machines starts waiting for it (`null`: verified, or not required). */
  dueMs: number | null;
  /** The newest verification mail: queued, handed to the provider, or refused by it. */
  delivery: "pending" | "sent" | "failed" | null;
  deliveryError: string | null;
}

/** The account's plan, its entitlements (`null`: unlimited) and what it uses. */
export interface PlanView {
  id: string;
  source: string;
  status: string;
  periodStartMs: number | null;
  periodEndMs: number | null;
  gracePlan: string | null;
  graceUntilMs: number | null;
  entitlements: Record<string, number | null>;
  usage: {
    storageBytes: number;
    /** Every current device of the chain, hosts included (`devices.max`). */
    devices: number;
    /** The devices that run AI (`hosts.max`); a hub from before chain v3 says only `machines`. */
    hosts: number;
    /** The old name of `hosts`, kept one version. */
    machines: number;
    relays: number;
    /**
     * What went through the relays this period (what goes straight between
     * two devices is not counted): `included`, the plan's share (`null`: no
     * limit); `throttled`, the account used it up and relayed connections are
     * slowed. `null` from a hub that does not say.
     */
    transfer: { periodStartMs: number | null; periodEndMs: number | null; bytes: number; included: number | null; overageBytes: number; throttled: boolean } | null;
  };
}

/**
 * How long after it registered — after the sign-in that registered it — a
 * device may open a numeric-code pairing when it is not a current admin of
 * the chain (rh-hub `pairings::UNCHAINED_WINDOW_MS`; past it the hub answers
 * 403 `pairing_window_closed`). A fresh sign-in registers it afresh
 * (`CloudAccount.open`). The QR code needs no pairing at the hub, and works
 * for as long as the hub keeps the device (a day).
 */
export const CODE_PAIRING_WINDOW_MS = 30 * 60 * 1000;

/**
 * The shortest password the hub takes at sign-up: rh-hub `auth::MIN_PASSWORD`
 * counts bytes of UTF-8, the form counts characters (as it says: "at least
 * 10 characters"), which are never fewer bytes. The longest is the hub's to
 * say (1024 bytes).
 */
export const PASSWORD_MIN_CHARS = 10;

/** A device registered to the account (with the password), in the chain or not. */
export interface RegisteredDevice {
  id: string;
  createdMs: number | null;
  lastSeenMs: number | null;
  inChain: boolean;
  /**
   * What it said it is when it registered (metadata only, never a reason to
   * allow anything): `host` for a computer that runs AI and signed in as a
   * device too (the same id as its machine row), `null` when it said nothing.
   */
  kind: string | null;
  /**
   * What the hub made of the User-Agent on its own requests (`ua_label`):
   * the kind of device and the browser, "iPhone · Chrome" — a name to know a
   * browser or phone by, never a reason to trust it. `null` when the hub
   * did not say (an older hub, or nothing it recognised); read through
   * `uaLabelOf`, which keeps only the words such a label is made of.
   */
  uaLabel: string | null;
  /**
   * The device of the account it registered to take the place of: its
   * browser lost that one's keys (`replaces`; metadata only, what a browser
   * says of itself). `null` when it said none, or an older hub.
   */
  replaces: string | null;
  /** Whether it is signed in on `/v1/client` now (`online`); `null` when the hub did not say (an older hub). */
  online: boolean | null;
}

/**
 * Whether the account takes chain v3 entries now (`/v1/me`'s `chain_v3`,
 * docs/local-aggregator.md §3.5): the hub must write them (`mode: "write"`)
 * and every current host's agent must read them (`waiting`: those that do
 * not yet). `writable` false means: write exactly what chain v2 can say.
 */
export interface ChainV3View {
  mode: "off" | "read" | "write";
  writable: boolean;
  /** The current hosts whose agents have not declared `chain/3` (update Droppen on them). */
  waiting: string[];
  /** The chain already holds a v3 entry. */
  inUse: boolean;
}

export interface AccountView {
  account: string;
  email: EmailStanding;
  /** `null` when the hub could not read it (or predates plans). */
  plan: PlanView | null;
  chainSeq: number | null;
  /** `null` from a hub that predates chain v3 (which takes no v3 entry). */
  chainV3: ChainV3View | null;
  machines: MachineSummary[];
  /** Every device registered to the account (`[]` from a hub that does not say). */
  devices: RegisteredDevice[];
  /**
   * Whether the account keeps a standby relay (「备用中继」, `relay_prefs.standby`):
   * the Hub keeps two relays in its set instead of one (relay-and-plans §2.7).
   * `null` from a hub that does not say.
   */
  relayStandby: boolean | null;
  /** The account's ways to sign in changed lately (the last 30 days, oldest first; `[]` from a hub that does not say). */
  signInChanges: SignInChange[];
  /** Identity key renewals (`null` from a hub that predates them: none are taken). */
  rotations?: RotationsSummary | null;
}

/** What changed in how the account signs in: a passkey added, renamed or removed, the password set, changed or removed. */
export type SignInChangeKind = "passkey_added" | "passkey_renamed" | "passkey_removed" | "password_set" | "password_changed" | "password_removed";

/** One change to the account's ways to sign in (`/v1/me` `sign_in_changes`, the `sign_in_changed` control message). */
export interface SignInChange {
  id: number;
  kind: SignInChangeKind;
  /** The device that made it (`null`: the hub does not say). */
  device: string | null;
  /** The passkey's name, for a passkey's change. */
  label: string | null;
  atMs: number;
}

const SIGN_IN_CHANGE_KINDS: ReadonlySet<string> = new Set(["passkey_added", "passkey_renamed", "passkey_removed", "password_set", "password_changed", "password_removed"]);

/** One `sign_in_changes` item, or a `sign_in_changed` message (its `t` aside): `null` when it is not one this page knows. */
export function signInChange(raw: unknown): SignInChange | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const id = num(r.id);
  const atMs = num(r.at_ms);
  if (id === null || atMs === null || typeof r.kind !== "string" || !SIGN_IN_CHANGE_KINDS.has(r.kind)) return null;
  return { id, kind: r.kind as SignInChangeKind, device: str(r.device), label: str(r.label), atMs };
}

/** Whether a v3 entry may be written now: only when the hub says so (`chain_v3.writable`). */
export function chainV3Writable(view: Pick<AccountView, "chainV3"> | null | undefined): boolean {
  return view?.chainV3?.writable === true;
}

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

export interface EnrollmentStatus {
  state: "waiting" | "expired" | "proved";
  machine?: string;
  signKey?: Bytes;
  dhKey?: Bytes;
  proof?: Bytes;
}

/** A machine's offline copy on the hub: how many streams, their bytes, and the newest content write. */
export interface MachineCopy {
  streams: number;
  bytes: number;
  lastWriteMs: number | null;
}

export interface MachineSummary {
  id: string;
  signKey: Bytes;
  dhKey: Bytes;
  inChain: boolean;
  createdMs: number | null;
  lastSeenMs: number | null;
  presence: { state: "online" | "offline" | "unknown"; since_ms: number };
  /** Its offline copy on the hub (`null` from a hub that does not say). */
  copy: MachineCopy | null;
  /** Its agent declared `chain/3` and `policy/1` (`false` from a hub that does not say). */
  readsV3: boolean;
  /** Its agent declared `rotate/1`: it renews its identity key on request (`false` from a hub that does not say). */
  rotates: boolean;
}

/** Mailbox purposes (`POST /v1/mailbox`). */
export const MAIL_USER_KEY = 1;
export const MAIL_MACHINE_KEY = 2;
export const MAIL_PAIRING = 3;
/** A device asking the account's other full devices for a user key epoch it lacks (no secret: only `epoch`). */
export const MAIL_KEY_REQUEST = 4;
/**
 * Chain v3: a host's `MK` epoch sealed by an admin for a device that is not
 * an admin (`encodeHostKeyMail`: `str16 host | sealed`, the row's epoch the
 * host's). Such a device holds no user key: these are how it reads the
 * hosts' offline copies (docs/local-aggregator.md §3.3).
 */
export const MAIL_HOST_KEY = 5;

/**
 * Whether a device page knows what to do with mail of `purpose`. Anything
 * else (a newer kind) is acknowledged unread rather than left to fill the
 * mailbox (relay-and-plans §6).
 */
export function knownMailPurpose(purpose: number): boolean {
  return purpose === MAIL_USER_KEY || purpose === MAIL_MACHINE_KEY || purpose === MAIL_PAIRING || purpose === MAIL_KEY_REQUEST || purpose === MAIL_HOST_KEY;
}

export interface Mail {
  id: string;
  sender: string;
  purpose: number;
  epoch: number;
  sealed: Bytes;
}

/** At most this many mails per `GET /v1/mailbox` page (the hub's default and maximum). */
export const MAIL_PAGE = 64;

export interface KeyringEntry {
  name: string;
  blob: Bytes;
  updatedMs: number;
}

export type PairingRole = "initiator" | "responder";

/**
 * The account's main device as the hub says (`GET`/`PUT /v1/main-device`,
 * and the `main_device` message on `/v1/client`): one of its hosts, or
 * `null` — none chosen yet, or the one chosen left the account. A claim: the
 * page uses it only if its own verified chain has that host.
 */
export interface AccountMain {
  machine: string | null;
  /** When the hub last set or cleared it (`null`: never): the newer of two answers wins. */
  setMs: number | null;
}

/** `{machine, set_ms}` (an HTTP answer or a `main_device` message) as an {@link AccountMain}, or `null` if it is not one. */
export function accountMain(v: unknown): AccountMain | null {
  if (!v || typeof v !== "object") return null;
  const r = v as { machine?: unknown; set_ms?: unknown };
  if (r.machine !== null && (typeof r.machine !== "string" || !r.machine)) return null;
  const setMs = r.set_ms === null || r.set_ms === undefined ? null : num(r.set_ms);
  if (r.set_ms !== null && r.set_ms !== undefined && setMs === null) return null;
  return { machine: r.machine, setMs };
}

export interface PairingSummary {
  id: string;
  device: string;
  createdMs: number;
}

export interface PairingMessage {
  n: number;
  from: PairingRole;
  body: Bytes;
}

type Fetch = (input: string, init?: RequestInit) => Promise<Response>;

/**
 * Asked for a fresh device session when the hub refused `stale` with a 401:
 * the token to retry with once, or `null` (no retry; the 401 stands).
 */
export type SessionRenewer = (stale: string) => Promise<string | null>;

/** Where {@link touchScreenMac} looks: the page by default, a fake in tests. */
export interface TouchEnv {
  navigator?: { userAgent?: string; maxTouchPoints?: number };
}

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

  /** Who may sign up, the Turnstile widget's page, whether e-mail must be verified, and passkeys (public). */
  async signupConfig(): Promise<SignupConfig> {
    const r = await this.typed<components["schemas"]["SignupConfig"]>((headers) => this.client.GET("/v1/signup/config", { headers }));
    const signup: SignupMode = r?.signup === "open" || r?.signup === "closed" ? (r.signup as SignupMode) : "invite";
    const deletion = deletionConfig(r?.deletion);
    return {
      signup,
      inviteRequired: typeof r?.invite_required === "boolean" ? r.invite_required : signup === "invite",
      turnstile: turnstileConfig(r?.turnstile),
      verification: r?.verification === "required" ? "required" : "off",
      passkeys: passkeysConfig(r?.passkeys),
      // Only from a hub that says it (one from before account deletion says nothing).
      ...(deletion ? { deletion } : {}),
    };
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

  /** The account's main device (`GET /v1/main-device`; a hub from before it answers 404). */
  async mainDevice(token: string): Promise<AccountMain> {
    const main = accountMain(await this.typed<components["schemas"]["MainDevice"]>((headers) => this.client.GET("/v1/main-device", { headers }), { token }));
    if (!main) throw new HubApiError(502, "bad_main_device", "the hub's main device is not one");
    return main;
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
   */
  async streamPage(token: string, stream: Uint8Array, page: Uint8Array): Promise<Bytes> {
    const res = await this.send("GET", `/v1/streams/${hex(stream)}/pages/${hex(page)}`, { token });
    if (!res.ok) return HubApi.fail(res);
    return new Uint8Array(await res.arrayBuffer());
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

/** `POST /v1/relays/tickets`. */
export interface RelayTickets {
  tickets: { relay: string; origin: string; ticket: string; expiresMs: number; emergency: boolean; throttled: boolean }[];
  /** `unknown`, `unavailable`, `limit` (the set is full: `accountSet` names it) or `forbidden`. */
  errors: { relay: string; error: string; limit: number | null; accountSet: string[] }[];
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

/** The hub's caps and refusals a person should be told about, by kind (`null`: another error). */
export function hubErrorKind(
  error: unknown,
):
  | "mailRate"
  | "mailboxFull"
  | "keyringFull"
  | "notFullDevice"
  | "notAnAdmin"
  | "chainV3NotReady"
  | "rotationNotReady"
  | "rotationInProgress"
  | "exists"
  | "removed"
  | "emailUnverified"
  | "rate"
  | "deviceRate"
  | "tooManyPendingDevices"
  | "tooManyEnrollments"
  | "pairingWindowClosed"
  | "tooManyPairings"
  | "pairingFull"
  | "unavailable"
  | "reauthFailed"
  | "reauthRequired"
  | "lastCredential"
  | "unlockOn"
  | "tooManyPasskeys"
  | "passkeyExists"
  | "noPasskey"
  | "passkeyRefused"
  | "confirmMismatch"
  | "accountSuspended"
  | "accountDeleted"
  | "recentlyDeleted"
  | null {
  if (!(error instanceof HubApiError)) return null;
  // Deleting the account (`DELETE /v1/account`), an account gone from the hub
  // (`410 account_deleted`: what a device session renewal answers for it — this
  // browser's keys for it open nothing any more), and one the hub's operator
  // suspended (`403 account_suspended`: nothing is wiped for that).
  if (error.status === 410 && error.code === "account_deleted") return "accountDeleted";
  if (error.status === 400 && error.code === "confirm_mismatch") return "confirmMismatch";
  if (error.status === 403 && error.code === "account_suspended") return "accountSuspended";
  if (error.status === 409 && error.code === "recently_deleted") return "recentlyDeleted";
  // The account's ways to sign in (`docs/hub-api/http.md` "The account's ways to sign in").
  if (error.status === 403 && error.code === "reauth_failed") return "reauthFailed";
  if (error.status === 403 && error.code === "reauth_required") return "reauthRequired";
  if (error.status === 409 && error.code === "last_credential") return "lastCredential";
  if (error.status === 409 && error.code === "unlock_on") return "unlockOn";
  if (error.status === 409 && error.code === "too_many_passkeys") return "tooManyPasskeys";
  if (error.status === 409 && error.code === "passkey_exists") return "passkeyExists";
  if (error.status === 409 && error.code === "no_passkey") return "noPasskey";
  if (error.status === 400 && error.code === "bad_passkey") return "passkeyRefused";
  // Registering this browser as a device (a sign-in), and adding one (§4.3, §4.4).
  if (error.status === 429 && error.code === "device_rate") return "deviceRate";
  if (error.status === 429 && error.code === "too_many_pending_devices") return "tooManyPendingDevices";
  if (error.status === 429 && error.code === "too_many_enrollments") return "tooManyEnrollments";
  // The numeric-code pairing's rendezvous (`docs/hub-api/http.md` `POST /v1/pairings`).
  if (error.status === 403 && error.code === "pairing_window_closed") return "pairingWindowClosed";
  if (error.status === 429 && error.code === "too_many_pairings") return "tooManyPairings";
  if (error.status === 429 && error.code === "pairing_full") return "pairingFull";
  if (error.status === 403 && error.code === "not_an_admin") return "notAnAdmin";
  if (error.status === 409 && error.code === "chain_v3_not_ready") return "chainV3NotReady";
  // Identity key renewals (docs/host-identity-rotation.md §3.3).
  if (error.status === 409 && error.code === "rotation_not_ready") return "rotationNotReady";
  if (error.status === 409 && error.code === "rotation_in_progress") return "rotationInProgress";
  if (error.status === 429 && error.code === "mail_rate") return "mailRate";
  if (error.status === 507 && error.code === "mailbox_full") return "mailboxFull";
  if (error.status === 507 && error.code === "keyring_full") return "keyringFull";
  if (error.status === 403 && error.code === "not_a_full_device") return "notFullDevice";
  if (error.status === 409 && error.code === "exists") return "exists";
  if (error.status === 403 && error.code === "device_removed") return "removed";
  if (error.status === 403 && error.code === "email_unverified") return "emailUnverified";
  if (error.status === 429 && error.code === "rate") return "rate";
  // The hub (or what is in front of it) not answering properly: try again later.
  if (error.status >= 500 || error.status === 408) return "unavailable";
  return null;
}

export type AuthErrorKind =
  | "badLogin"
  | "signupClosed"
  | "badInvite"
  | "turnstileRequired"
  | "turnstileRetry"
  | "turnstileFailed"
  | "turnstileUnavailable"
  | "disposableEmail"
  | "emailTaken"
  | "alreadyVerified"
  | "verificationOff"
  | "badToken"
  | "expired"
  | "unauthorized"
  | "badPasskey"
  | "unknownPasskey"
  | "passkeysOff"
  | "accountSuspended"
  | "recentlyDeleted"
  | "rate";

/** Sign-up, sign-in and e-mail verification's refusals, by kind (`null`: another error). */
export function authErrorKind(error: unknown): AuthErrorKind | null {
  if (!(error instanceof HubApiError)) return null;
  const { status, code } = error;
  if (status === 429) return "rate";
  if (status === 401) {
    if (code === "bad_login") return "badLogin";
    // A passkey refused is not a session expired.
    if (code === "bad_passkey") return "badPasskey";
    if (code === "unknown_passkey") return "unknownPasskey";
    return "unauthorized";
  }
  if (status === 403 && code === "passkeys_off") return "passkeysOff";
  if (status === 403 && code === "account_suspended") return "accountSuspended";
  if (status === 409 && code === "recently_deleted") return "recentlyDeleted";
  if (status === 403 && code === "signup_closed") return "signupClosed";
  if (status === 403 && code === "bad_invite") return "badInvite";
  if (status === 403 && code === "turnstile_required") return "turnstileRequired";
  if (status === 403 && code === "turnstile_retry") return "turnstileRetry";
  if (status === 403 && code === "turnstile_failed") return "turnstileFailed";
  if (status === 503 && code === "turnstile_unavailable") return "turnstileUnavailable";
  if (status === 400 && code === "disposable_email") return "disposableEmail";
  if (status === 409 && code === "email_taken") return "emailTaken";
  if (status === 409 && code === "already_verified") return "alreadyVerified";
  if (status === 409 && code === "verification_off") return "verificationOff";
  if (status === 400 && code === "bad_token") return "badToken";
  if (status === 410 && code === "expired") return "expired";
  return null;
}

/** A sign-in's answer; a hub from before verification says neither field, and verifies nothing. */
function signedIn(r: components["schemas"]["SignedIn"]): SignedIn {
  const verification: Verification = r.verification === "required" ? "required" : "off";
  return { account: r.account, token: r.token, expiresMs: r.expires_ms, emailVerified: verification === "off" || r.email_verified !== false, verification };
}

/** `signup/config`'s `passkeys` (absent, from an older hub, or not of its shape: none). */
function passkeysConfig(raw: unknown): PasskeysConfig | null {
  if (!raw || typeof raw !== "object") return null;
  const rpId = (raw as { rp_id?: unknown }).rp_id;
  return typeof rpId === "string" && rpId ? { rpId } : null;
}

/** `signup/config`'s `deletion` (absent, from an older hub, or not of its shape: `null`). */
function deletionConfig(raw: unknown): DeletionConfig | null {
  if (!raw || typeof raw !== "object") return null;
  const days = (raw as { email_hold_days?: unknown }).email_hold_days;
  return typeof days === "number" && Number.isInteger(days) && days >= 0 ? { emailHoldDays: days } : null;
}

/** `{challenge, options}` of the shape `create()` (or `get()`) needs, else a 502: nothing is asked of the browser. */
function passkeyChallenge(raw: unknown, kind: "create" | "get"): PasskeyChallenge<unknown> {
  const bad = () => new HubApiError(502, "bad_options", "the hub's passkey options are not usable");
  if (!raw || typeof raw !== "object") throw bad();
  const r = raw as { challenge?: unknown; options?: unknown };
  if (typeof r.challenge !== "string" || !r.challenge || !r.options || typeof r.options !== "object") throw bad();
  const o = r.options as Record<string, unknown>;
  if (typeof o.challenge !== "string") throw bad();
  if (kind === "create") {
    const user = o.user as Record<string, unknown> | undefined;
    const rp = o.rp as Record<string, unknown> | undefined;
    if (!user || typeof user.id !== "string" || typeof user.name !== "string" || !rp || typeof rp.name !== "string" || !Array.isArray(o.pubKeyCredParams)) throw bad();
  }
  return { challenge: r.challenge, options: o };
}

/** `login/passkey`'s `passkey`. */
function usedPasskey(raw: unknown): UsedPasskey | null {
  if (!raw || typeof raw !== "object") return null;
  const p = raw as Record<string, unknown>;
  if (typeof p.id !== "string") return null;
  const unlock = p.unlock === "on" || p.unlock === "off" || p.unlock === "paused" ? p.unlock : "none";
  return { id: p.id, label: typeof p.label === "string" ? p.label : "", unlock };
}

/** One row of `GET /v1/passkeys` (and the answer to adding or renaming one); `null` when it names none. */
function accountPasskey(raw: unknown): AccountPasskey | null {
  if (!raw || typeof raw !== "object") return null;
  const p = raw as Record<string, unknown>;
  if (typeof p.id !== "string" || typeof p.credential_id !== "string") return null;
  return {
    id: p.id,
    label: str(p.label) ?? "",
    provider: str(p.provider) ?? "Passkey",
    credentialId: p.credential_id,
    credHash: str(p.cred_hash) ?? "",
    backupEligible: p.backup_eligible === true,
    backupState: p.backup_state === true,
    prf: p.prf === "yes" || p.prf === "no" ? p.prf : "unknown",
    createdMs: num(p.created_ms) ?? 0,
    createdBy: str(p.created_by),
    lastUsedMs: num(p.last_used_ms),
    unlock: p.unlock === "on" ? "on" : "off",
  };
}

/** A proof's fields as `PUT`/`DELETE /v1/password` (and `DELETE /v1/account`) take them. */
function reauthBody(proof: Reauth): components["schemas"]["Proof"] {
  if ("grant" in proof) return { grant: proof.grant };
  return "currentPassword" in proof
    ? { current_password: proof.currentPassword }
    : { passkey: proof.passkey as unknown as components["schemas"]["ReauthPasskey"] };
}

/** The widget's page, if the hub names a usable one: an http(s) origin, nothing more. */
function turnstileConfig(raw: { site_key?: unknown; origin?: unknown } | null | undefined): TurnstileConfig | null {
  if (!raw || typeof raw.site_key !== "string" || !raw.site_key || typeof raw.origin !== "string") return null;
  try {
    const url = new URL(raw.origin);
    if (url.protocol !== "https:" && url.protocol !== "http:") return null;
    return { siteKey: raw.site_key, origin: url.origin };
  } catch {
    return null;
  }
}

const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const str = (v: unknown): string | null => (typeof v === "string" ? v : null);

/** `/v1/me`'s `email` (a bare address from a hub that predates verification: nothing to verify). */
function emailStanding(raw: unknown): EmailStanding {
  if (typeof raw === "string" || raw === null || raw === undefined) {
    return { address: raw ?? "", verified: false, verification: "off", dueMs: null, delivery: null, deliveryError: null };
  }
  const e = raw as Record<string, unknown>;
  const delivery = e.delivery === "pending" || e.delivery === "sent" || e.delivery === "failed" ? e.delivery : null;
  return {
    address: str(e.address) ?? "",
    verified: e.verified === true,
    verification: e.verification === "required" ? "required" : "off",
    dueMs: num(e.due_ms),
    delivery,
    deliveryError: str(e.delivery_error),
  };
}

/** A machine's `copy` in `/v1/me`. */
function machineCopy(raw: unknown): MachineCopy | null {
  if (!raw || typeof raw !== "object") return null;
  const c = raw as Record<string, unknown>;
  return { streams: num(c.streams) ?? 0, bytes: num(c.bytes) ?? 0, lastWriteMs: num(c.last_write_ms) };
}

/** `/v1/me`'s `devices`: the account's registered devices. */
function registeredDevices(raw: unknown): RegisteredDevice[] {
  if (!Array.isArray(raw)) return [];
  return raw.flatMap((d) => {
    if (!d || typeof d !== "object" || typeof (d as { id?: unknown }).id !== "string") return [];
    const r = d as Record<string, unknown>;
    return [
      {
        id: r.id as string,
        createdMs: num(r.created_ms),
        lastSeenMs: num(r.last_seen_ms),
        inChain: r.in_chain === true,
        kind: str(r.kind),
        uaLabel: str(r.ua_label),
        replaces: str(r.replaces),
        online: typeof r.online === "boolean" ? r.online : null,
      },
    ];
  });
}

/** `/v1/me`'s `chain_v3` (`null`: a hub that predates it). Only an explicit `writable: true` lets v3 entries be written. */
export function chainV3View(raw: unknown): ChainV3View | null {
  if (!raw || typeof raw !== "object") return null;
  const c = raw as Record<string, unknown>;
  const mode = c.mode === "write" || c.mode === "read" ? c.mode : "off";
  return {
    mode,
    writable: c.writable === true,
    waiting: Array.isArray(c.waiting) ? c.waiting.filter((h): h is string => typeof h === "string") : [],
    inUse: c.in_use === true,
  };
}

const ROTATION_STATES: readonly RotationState[] = ["pending", "attempt", "staged", "committed"];

/** A lower-case hex field of `bytes` bytes, or `null`. */
function hexOf(v: unknown, bytes: number): Bytes | null {
  if (typeof v !== "string" || v.length !== bytes * 2 || !/^[0-9a-f]+$/.test(v)) return null;
  return fromHex(v);
}

/** One `Rotation` row (`GET /v1/rotations`, `PUT /v1/rotations/{host}`), or `null` if it is not one. */
export function hostRotationView(raw: unknown): HostRotationView | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  if (typeof r.host !== "string" || !r.host || !ROTATION_STATES.includes(r.state as RotationState)) return null;
  const f = r.failure && typeof r.failure === "object" ? (r.failure as Record<string, unknown>) : null;
  return {
    host: r.host,
    state: r.state as RotationState,
    assurance: r.assurance === 1 ? 1 : 0,
    requestedMs: num(r.requested_ms),
    requestedBy: str(r.requested_by),
    attemptExpiresMs: num(r.attempt_expires_ms),
    stagedMs: num(r.staged_ms),
    readyMs: num(r.ready_ms),
    signKey: hexOf(r.sign_key, 32),
    dhKey: hexOf(r.dh_key, 32),
    stagedSeq: num(r.staged_seq),
    committedMs: num(r.committed_ms),
    committedSeq: num(r.committed_seq),
    failure: f && typeof f.reason === "string" ? { reason: f.reason, atMs: num(f.at_ms) ?? 0 } : null,
    needsAssurance: r.needs_assurance === true,
    isolated: r.isolated === true,
    nonceHash: typeof r.nonce_hash === "string" && /^[0-9a-f]{64}$/.test(r.nonce_hash) ? r.nonce_hash : null,
  };
}

/** One `Dispute` (`GET /v1/rotations`), or `null` if it is not one. */
export function hostDispute(raw: unknown): HostDispute | null {
  if (!raw || typeof raw !== "object") return null;
  const d = raw as Record<string, unknown>;
  if (typeof d.host !== "string" || !d.host) return null;
  if (d.kind !== "disputed" && d.kind !== "retired_login" && d.kind !== "not_acked" && d.kind !== "conflict") return null;
  const l = d.login && typeof d.login === "object" ? (d.login as Record<string, unknown>) : null;
  const nonce = l ? hexOf(l.nonce, 32) : null;
  const timeMs = l ? num(l.time_ms) : null;
  const login =
    l && nonce && timeMs !== null && typeof l.account === "string" && typeof l.id === "string" && typeof l.hub === "string"
      ? { account: l.account, id: l.id, hub: l.hub, nonce, timeMs }
      : null;
  return {
    host: d.host,
    kind: d.kind,
    seenMs: num(d.seen_ms) ?? 0,
    retiredSeq: num(d.retired_seq),
    role: d.role === "agent" || d.role === "device" ? d.role : null,
    login,
    sig: hexOf(d.sig, 64),
    signKey: hexOf(d.sign_key, 32),
    seq: num(d.seq),
    entryHash: hexOf(d.entry_hash, 32),
  };
}

/** `GET /v1/rotations`' answer; anything not of its shape is left out. */
export function rotationsView(raw: unknown): RotationsView {
  const r = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  return {
    enabled: r.enabled === true,
    windowMs: num(r.window_ms) ?? 120_000,
    nowMs: num(r.now_ms),
    hosts: Array.isArray(r.hosts) ? r.hosts.flatMap((h) => hostRotationView(h) ?? []) : [],
    disputes: Array.isArray(r.disputes) ? r.disputes.flatMap((d) => hostDispute(d) ?? []) : [],
  };
}

/** `/v1/me`'s `rotations` (`null`: a hub that predates renewals). */
export function rotationsSummary(raw: unknown): RotationsSummary | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  const active = Array.isArray(r.active)
    ? r.active.flatMap((a) => {
        const x = (a && typeof a === "object" ? a : {}) as Record<string, unknown>;
        return typeof x.host === "string" && ROTATION_STATES.includes(x.state as RotationState) ? [{ host: x.host, state: x.state as RotationState }] : [];
      })
    : [];
  return { enabled: r.enabled === true, windowMs: num(r.window_ms) ?? 120_000, active };
}

/** A `rotation` push (`{t: "rotation", host, state, reason?}`), or `null` if it is not one. */
export function rotationPush(message: Record<string, unknown>): RotationPush | null {
  const state = message.state === "none" ? "none" : ROTATION_STATES.includes(message.state as RotationState) ? (message.state as RotationState) : null;
  if (message.t !== "rotation" || typeof message.host !== "string" || state === null) return null;
  return { host: message.host, state, reason: typeof message.reason === "string" ? message.reason : null };
}

/** A `host_dispute` push (`{t: "host_dispute", host, kind, seen_ms}`), or `null` if it is not one. */
export function disputePush(message: Record<string, unknown>): DisputePush | null {
  if (message.t !== "host_dispute" || typeof message.host !== "string") return null;
  return { host: message.host, kind: typeof message.kind === "string" ? message.kind : "", seenMs: num(message.seen_ms) ?? 0 };
}

/** `/v1/me`'s `plan`, also what a device's `plan` control message carries as `detail` (`null`: not one). */
export function planView(raw: unknown): PlanView | null {
  if (!raw || typeof raw !== "object") return null;
  const p = raw as Record<string, unknown>;
  const entitlements: Record<string, number | null> = {};
  if (p.entitlements && typeof p.entitlements === "object") {
    for (const [key, value] of Object.entries(p.entitlements as Record<string, unknown>)) entitlements[key] = num(value);
  }
  const u = (p.usage && typeof p.usage === "object" ? p.usage : {}) as Record<string, unknown>;
  const t = u.transfer && typeof u.transfer === "object" ? (u.transfer as Record<string, unknown>) : null;
  return {
    id: str(p.id) ?? "",
    source: str(p.source) ?? "",
    status: str(p.status) ?? "",
    periodStartMs: num(p.period_start_ms),
    periodEndMs: num(p.period_end_ms),
    gracePlan: str(p.grace_plan),
    graceUntilMs: num(p.grace_until_ms),
    entitlements,
    usage: {
      storageBytes: num(u.storage_bytes) ?? 0,
      devices: num(u.devices) ?? 0,
      hosts: num(u.hosts) ?? num(u.machines) ?? 0,
      machines: num(u.machines) ?? num(u.hosts) ?? 0,
      relays: num(u.relays) ?? 0,
      transfer: t
        ? {
            periodStartMs: num(t.period_start_ms),
            periodEndMs: num(t.period_end_ms),
            bytes: num(t.bytes) ?? 0,
            included: num(t.included),
            overageBytes: num(t.overage_bytes) ?? 0,
            throttled: t.throttled === true,
          }
        : null,
    },
  };
}
