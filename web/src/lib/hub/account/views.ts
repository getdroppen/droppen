/*
 * What the account hands the pages: what a rotation did, the alerts, a
 * held-back activation, and what vetoing it would take out.
 */
import type { Accept, Action, Entry } from "@/lib/e2e";
import type { DeliveryResult } from "../deliver";
import type { Rewrap } from "../keyring";

export interface RotationReport {
  /** The new user key epoch; `null` when only machine keys were rotated (a device that held no user key left). */
  ukEpoch: number | null;
  /** Devices the new `UK` epoch was sealed to (mailbox, purpose 1). */
  sealedTo: string[];
  /** Devices that are not admins the new machine keys were sealed to (mailbox, purpose 5). */
  hostKeysTo?: string[];
  /** Those it could not reach, and why. */
  hostKeysFailed?: Record<string, string>;
  failedTo: string[];
  /** Why each of `failedTo` failed (the hub's error code, e.g. `mailbox_full`, or a message). */
  failedWhy: Record<string, string>;
  /** Whether it went into the recovery package too. */
  recovery: boolean;
  machines: {
    machine: string;
    mkEpoch?: number;
    result?: DeliveryResult;
    error?: string;
    /**
     * Why the keyring did not take the new key's blob (e.g. `keyring_full`):
     * the key was handed over all the same — the agent must stop sealing under
     * the retired one — but other devices cannot read what the machine writes
     * from now on until the keys are rotated again with room in the keyring.
     */
    keyring?: string;
  }[];
  /** A veto over a held-back activation: the devices and machines it took out besides (what the recovered device brought in). */
  vetoed?: { devices: string[]; machines: string[] };
  /**
   * The older machine-key wraps rewritten under the new user key, in this
   * device's own name (relay-and-plans §6: the hub then reclaims the wraps
   * they supersede, the removed device's among them), and any that failed —
   * those stay readable through their old wraps.
   */
  rewrapped?: { done: Rewrap[]; failed: (Rewrap & { error: string })[] };
}

/**
 * A new user key epoch a batch that issues machine keys carried because the
 * newest one was held by a device that no longer manages the account
 * ({@link userKeyStale}): shared as a rotation shares it — kept here, sealed
 * for the recovery words, mailed to the other admins.
 */
export interface FreshUserKey {
  epoch: number;
  sealedTo: string[];
  failedTo: string[];
  failedWhy: Record<string, string>;
  recovery: boolean;
}

/** A device a veto would take out ({@link VetoPlan}), and what brought it in. */
export interface VetoTarget {
  id: string;
  /** It runs AI (a host): taken out, it must be added to the account again. */
  host: boolean;
  /** Who brought it in: the device that signed it, a passkey slot, or `null` for the recovery words (`null` too for the held device itself). */
  by: string | null;
}

/**
 * What vetoing a held-back activation would take out, for the page to show
 * before the person confirms (issue #17): `scope`, which the veto always
 * takes, and `trusted`, which it takes only if asked.
 */
export interface VetoPlan {
  /** The device whose activation is held back. */
  device: string;
  /** Where the hold starts: the activation's seq. */
  seq: number;
  /** The devices this view trusted when the plan was read: what the veto never removes (handed back to `removeDevice`'s `veto`). */
  view: string[];
  /** The held device first, then what it, the recovery words or a passkey slot brought in (`vetoScope`). */
  scope: VetoTarget[];
  /**
   * What the devices this view already trusted (other than this one) brought
   * in while the activation was held back here, and what those brought in:
   * not in `scope`, kept unless the person asks (`removeDevice`'s
   * `alsoTrusted`). Never this device, nor a device this view already trusted.
   */
  trusted: VetoTarget[];
  /** Whether the veto voids the account's current recovery words (a device it takes out set them, or came back with them). */
  voidsWords: boolean;
  /** Whether also taking out `trusted` voids them where `scope` alone does not. */
  trustedVoidsWords: boolean;
  /** Whether the veto turns off passkey slots (one a device it takes out registered, or came in through): joining with them stops. */
  voidsPasskeys: boolean;
  /** Whether also taking out `trusted` turns off slots `scope` alone does not. */
  trustedVoidsPasskeys: boolean;
}

/** One entry the hub's chain has past a held-back activation, as the page shows it. */
export interface HeldChange {
  seq: number;
  kind: Action["type"];
  /** The device that signed it (a passkey slot: the slot), or `null` for the recovery words. */
  by: string | null;
  /** The device or machine it is about, if any. */
  subject: string | null;
}

/** A change to who is in the account that this device has not acknowledged yet. */
export interface ChainAlert {
  seq: number;
  /** `upgrade`: the account's first chain v3 entry (a new user key written as v3: review D4). */
  kind: "addDevice" | "removeDevice" | "recoveryJoin" | "setRecoveryKey" | "keyEpochs" | "upgrade" | "setAdmin" | "setHost" | "setAccept" | "passkey" | "rotateHostKey" | "renewalPolicy" | "renewalPause";
  /**
   * `renewalPause` (host identity rotation §3.4 rule 10): which marker, and the control its start paused (per host, the
   * ids it stopped accepting) — for `completed`, only what comes back (empty: none of it).
   */
  pause?: { phase: "start" | "completed" | "cancelled"; removals: { host: string; ids: string[] }[] };
  /** The device that signed it, or `null` for the recovery words (a passkey slot's entries: the slot). */
  by: string | null;
  /** Chain v3's `addDevice3`: what the added device may do (「可运行 AI」, 「可管理账号」). */
  flags?: { host: boolean; admin: boolean };
  /** `setAdmin` / `setHost`: what it was set to; `renewalPolicy`: whether every renewal must now be confirmed on the device. */
  on?: boolean;
  /** `setAccept`: whom the host accepts control from now. */
  accept?: Accept;
  /** A passkey entry: which (a slot registered or removed, a device it added, one activating itself). */
  passkey?: "slotSet" | "slotRemove" | "join" | "activate";
  /** `rotateHostKey` (`docs/host-identity-rotation.md` §3.4 "告警"): the host's new keys (said by their `deviceFingerprint`), and whether the renewal was confirmed on the device. */
  renewed?: { signKey: Uint8Array; dhKey: Uint8Array; onDevice: boolean };
  /** The device added, removed or joined (`null` for new recovery words). */
  subject: string | null;
  /** When this device first saw it (else when it says it was made). */
  atMs: number;
  /** A device this device may remove about it (the one added or joined, or the one that set the words), if still there. */
  remove: string | null;
}

/** Entries the hub has past this view that this device does not take yet (see `CloudAccount.heldBack`). */
export interface HeldBack {
  /** The entry it stopped at: a recovered device's self-activation, or a passkey-joined one's (`passkeyActivate`). */
  seq: number;
  /** The device that activated itself. */
  device: string | null;
  /** When this device's own clock takes it: a day after it first saw the join. */
  readyMs: number;
  /** What the hub served from `seq` on. */
  entries: Entry[];
}
