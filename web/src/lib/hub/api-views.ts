/*
 * The hub's wire types, as this page reads them, and the pure checks that
 * turn what the hub said into them (docs/hub-api/http.md). The Hub is assumed
 * malicious: nothing here trusts a field's type or range, and every validator
 * answers `null` (or a safe default) rather than pass a shape it does not
 * recognise. `api.ts` keeps the client that fetches these and the session
 * storage; this file keeps the vocabulary and the checking.
 */
import type { components } from "@/api/hub-schema";
import { type Bytes, fromHex } from "@/lib/e2e";
import type { AuthenticationResponseJSON, PasskeyChallenge } from "./passkey";

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

/** Passkeys at this hub (docs/passkeys.md §3.1): the RP ID they are made for. */
export interface PasskeysConfig {
  rpId: string;
}

/** What sign-up asks for, as droppen.app answers it ([`SIGNUP_CONFIG`]). */
export interface SignupConfig {
  signup: SignupMode;
  inviteRequired: boolean;
  verification: Verification;
  /** `null`: this hub offers no passkeys (or predates them), and the page says nothing of them. */
  passkeys: PasskeysConfig | null;
  /** What deleting an account leaves behind (absent: the hub predates account deletion, or did not say). */
  deletion?: DeletionConfig | null;
}

/**
 * droppen.app's answers, written here instead of asked of the hub: sign-up is
 * open (an invite code is optional — it may give the account a plan), the
 * e-mail must be verified before a device joins, passkeys are made for
 * `droppen.app`, and a deleted account's address is held for seven days.
 * The hub no longer answers
 * `GET /v1/signup/config`, and a site served by another hub shows these same
 * answers.
 */
export const SIGNUP_CONFIG: SignupConfig = {
  signup: "open",
  inviteRequired: false,
  verification: "required",
  passkeys: { rpId: "droppen.app" },
  deletion: { emailHoldDays: 7 },
};

/** What deleting an account leaves behind ([`SIGNUP_CONFIG`]'s `deletion`). */
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
/**
 * Asked for a fresh device session when the hub refused `stale` with a 401:
 * the token to retry with once, or `null` (no retry; the 401 stands).
 */
export type SessionRenewer = (stale: string) => Promise<string | null>;

/** Where {@link touchScreenMac} looks: the page by default, a fake in tests. */
export interface TouchEnv {
  navigator?: { userAgent?: string; maxTouchPoints?: number };
}
/** `POST /v1/relays/tickets`. */
export interface RelayTickets {
  tickets: { relay: string; origin: string; ticket: string; expiresMs: number; emergency: boolean; throttled: boolean }[];
  /** `unknown`, `unavailable`, `limit` (the set is full: `accountSet` names it) or `forbidden`. */
  errors: { relay: string; error: string; limit: number | null; accountSet: string[] }[];
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
export function signedIn(r: components["schemas"]["SignedIn"]): SignedIn {
  const verification: Verification = r.verification === "required" ? "required" : "off";
  return { account: r.account, token: r.token, expiresMs: r.expires_ms, emailVerified: verification === "off" || r.email_verified !== false, verification };
}

/** `{challenge, options}` of the shape `create()` (or `get()`) needs, else a 502: nothing is asked of the browser. */
export function passkeyChallenge(raw: unknown, kind: "create" | "get"): PasskeyChallenge<unknown> {
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
export function usedPasskey(raw: unknown): UsedPasskey | null {
  if (!raw || typeof raw !== "object") return null;
  const p = raw as Record<string, unknown>;
  if (typeof p.id !== "string") return null;
  const unlock = p.unlock === "on" || p.unlock === "off" || p.unlock === "paused" ? p.unlock : "none";
  return { id: p.id, label: typeof p.label === "string" ? p.label : "", unlock };
}

/** One row of `GET /v1/passkeys` (and the answer to adding or renaming one); `null` when it names none. */
export function accountPasskey(raw: unknown): AccountPasskey | null {
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
export function reauthBody(proof: Reauth): components["schemas"]["Proof"] {
  if ("grant" in proof) return { grant: proof.grant };
  return "currentPassword" in proof
    ? { current_password: proof.currentPassword }
    : { passkey: proof.passkey as unknown as components["schemas"]["ReauthPasskey"] };
}

export const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
export const str = (v: unknown): string | null => (typeof v === "string" ? v : null);

/** `/v1/me`'s `email` (a bare address from a hub that predates verification: nothing to verify). */
export function emailStanding(raw: unknown): EmailStanding {
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
export function machineCopy(raw: unknown): MachineCopy | null {
  if (!raw || typeof raw !== "object") return null;
  const c = raw as Record<string, unknown>;
  return { streams: num(c.streams) ?? 0, bytes: num(c.bytes) ?? 0, lastWriteMs: num(c.last_write_ms) };
}

/** `/v1/me`'s `devices`: the account's registered devices. */
export function registeredDevices(raw: unknown): RegisteredDevice[] {
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
export function hexOf(v: unknown, bytes: number): Bytes | null {
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
