/*
 * Why the account refuses something (`AccountError`): every message is an
 * i18n key, the English with `{{name}}` where its values go.
 */
import { RULES, type UnreadableReason } from "@/lib/e2e";
import { type AccountView, chainV3Writable } from "../api";
import type { HeldBack } from "./views";

export type AccountErrorKind =
  | "notInChain"
  | "fork"
  | "proof"
  | "expired"
  | "noUserKey"
  | "noRecoveryKey"
  | "wrongWords"
  | "noPackages"
  | "tooEarly"
  | "notPending"
  | "self"
  | "unpinned"
  | "refused"
  | "heldBack"
  | "missingKey"
  /** The hub does not take v3 entries for this account now (`chain_v3.writable`): to upgrade it, or (on a v3 chain) to add a host or make one a host again. */
  | "notWritable"
  /** The chain holds no v3 entry yet: this needs the account upgraded first (`upgradeToV3`, review D4). */
  | "notUpgraded"
  /** The batch would not verify: the chain's verifier refused it here, before it was signed away (`rule` says why). */
  | "invalidEntry"
  /** Only a device that manages the account may do this. */
  | "notAdmin"
  /** The last device that manages the account stays one. */
  | "lastAdmin"
  /** Another page of this browser set the account up a moment ago: this one reloads. */
  | "setUpElsewhere"
  /** This browser's device keys are stored but would not open just now: they were left as they are (try again). */
  | "keysUnreadable"
  /** No current passkey slot names the passkey a new device tried to join with (docs/passkeys.md §4.6). */
  | "noPasskeySlot"
  /** The passkey gave no PRF output here: it cannot let devices join from this browser (§5.2). */
  | "noPrf"
  /** What the passkey's PRF gave is not the slot's `PAK`, or the slot's proof does not hold: nothing is trusted (§4.6 step 3). */
  | "wrongPasskey"
  /** The device being approved no longer waits to join: a newer join with the same passkey replaced it, or a device removed it. */
  | "joinReplaced"
  /** The keys the pairing checked are not the ones the device waiting to join was added with. */
  | "joinMismatch"
  /** The account has as many passkey slots as it may. */
  | "tooManySlots"
  /** The batch is more entries than the hub takes in one append ({@link MAX_APPEND_ENTRIES}). */
  | "tooManyEntries"
  /** A veto would remove nothing: what it was for is out of the account already. Nothing is signed. */
  | "nothingToRemove"
  /** A veto confirmed on a hold this device has since taken in, with the chain moved on after that: the person looks again. Nothing is signed. */
  | "vetoStale"
  /**
   * A host that may have had its identity key copied (`hostsToRotate`) or is
   * isolated would be let control another one: renew its identity key first
   * (docs/host-identity-rotation.md §4.1, §7 decision 8).
   */
  | "exposed"
  /** A pause's end would give back other control than the person was shown (M7): ask again. */
  | "pauseChanged";

export class AccountError extends Error {
  /** The message as an i18n key (the English, with `{{name}}` where `vars` go). */
  readonly key: string;

  constructor(
    readonly kind: AccountErrorKind,
    message: string,
    readonly readyMs?: number,
    /** `invalidEntry`: the chain's rule that refused it (`RULES`, or the verifier's error kind). */
    readonly rule?: string,
    /** What the message says to a person: its i18n key (the English) and the values it names, when it has them. */
    readonly vars?: Record<string, string>,
  ) {
    super(vars ? message.replace(/\{\{(\w+)\}\}/g, (_, name: string) => vars[name] ?? "") : message);
    this.name = "AccountError";
    this.key = message;
  }
}

/** `keysUnreadable`: why (`UnreadableReason`), for the screen to say it in the person's words. */
export class KeysUnreadableError extends AccountError {
  constructor(
    readonly reason: UnreadableReason,
    message: string,
  ) {
    super("keysUnreadable", message);
  }
}

/**
 * A served chain only behind the seen record, agreeing with it where they
 * overlap: a checkpoint another page is still moving, looked at again (a
 * page's first load), not a fork — until it stays so.
 */
export class MovingCheckpoint extends AccountError {
  constructor(message: string) {
    super("fork", message);
  }
}

/**
 * Why the chain does not take a device that runs AI as a full device (review
 * D1: `RULES.hostNotAdmin`), as an error whose message is an i18n key.
 */
export function hostAdminRefused(): AccountError {
  return new AccountError("invalidEntry", "a device that runs AI cannot be a full device (yet)", undefined, RULES.hostNotAdmin);
}

/**
 * Why a new host cannot be added yet on a chain that holds a v3 entry: the
 * hub does not take v3 entries now (`chain_v3.writable`). `null` when it does,
 * or the chain is not v3. `namedWaiting`: name the devices that must update
 * first (the browser hand-off's page does; the code screen says it plainly).
 */
export function v3NotWritableError(
  chainV3: boolean | undefined,
  view: Pick<AccountView, "chainV3"> | null | undefined,
  namedWaiting = false,
): AccountError | null {
  if (!chainV3 || chainV3Writable(view)) return null;
  const waiting = namedWaiting ? (view?.chainV3?.waiting ?? []) : [];
  return waiting.length
    ? new AccountError("notWritable", "Not available yet: update Droppen on {{hosts}} first.", undefined, undefined, { hosts: waiting.join(", ") })
    : new AccountError("notWritable", "Not available yet: Droppen turns this on once it is updated on every device of the account that runs AI.");
}

/** D1 extended: a device that ran AI (and stopped) is never a full device either (`RULES.ranAiNotAdmin`), as an error whose message is an i18n key. */
export function ranAiAdminRefused(id: string): AccountError {
  return new AccountError(
    "invalidEntry",
    "{{id}} ran AI before, and its agent kept its key in a file: it cannot be a full device. Add a new browser or phone instead.",
    undefined,
    RULES.ranAiNotAdmin,
    { id },
  );
}

/**
 * The hub takes no chain v3 entries now, and a new account starts on v3 only
 * (`createGenesis`): none is made, and the person is told so.
 */
export function noNewAccounts(): AccountError {
  return new AccountError("notWritable", "New accounts cannot be set up right now: the hub does not take chain v3 changes yet — that is on Droppen's side. Try again later.");
}

/** The chain holds no v3 entry yet (review D4). */
export function notUpgraded(): AccountError {
  return new AccountError("notUpgraded", "upgrade the account to chain v3 first (「我的设备」)");
}

/** A held-back activation, in an error's words: who, and how it came in. */
export function heldWho(held: HeldBack): string {
  const how = held.entries[0]?.draft.action.type === "passkeyActivate" ? "joined through a passkey" : "came back with the recovery words";
  return `${held.device ?? "a device"} ${how} and activated itself sooner than this device allows`;
}
