/*
 * What a host's identity key renewal asks of the page and what it reports
 * (`CloudAccount.renewIdentity`, docs/host-identity-rotation.md), and why
 * one did not go through.
 */
import type { Assurance, Bytes } from "@/lib/e2e";
import type { DeliveryResult } from "../deliver";
import type { FreshUserKey } from "./views";

/** A renewal superseded by a loss after it staged is started again at most this many times in all (§4.1 「清除」). */
export const RENEW_RESTARTS = 3;
/** What `holdRenewal` answers while another tab of this browser holds the renewal of that host. */
export const BUSY: unique symbol = Symbol("busy");

/**
 * Why a host's identity key renewal did not go through
 * (`CloudAccount.renewIdentity`; docs/host-identity-rotation.md §4.2):
 *
 * - `tooOld`: its Droppen has no `/rh/rotate` (404), or never declared
 *   `rotate/1` — re-enrolling it is the way; not tried again.
 * - `disabled`: the hub takes no renewal (`DROPPEN_HUB_HOST_ROTATION` off, or
 *   a hub from before renewals).
 * - `needsAssurance`: only a renewal confirmed on the device itself goes (clone
 *   evidence, an isolated host, the account's setting).
 * - `inProgress`: another page's attempt is running.
 * - `offline`: no channel to it.
 * - `hubNotReady`: its own hub connection cannot stage a renewal yet.
 * - `waiting`: the hub waits for other hosts to declare `rotate/1` (`hosts`).
 * - `renewFirst`: it is exposed and lets exposed hosts that have not renewed
 *   yet control it (`hosts`): those renew first (§4.1 「次序先于收窄」) — their
 *   old keys would still reach its new seed. Not tried; nothing to withdraw.
 * - `superseded`: a device that could control it was lost after the view its
 *   new key was staged on (§4.1 「清除」): that seed is no remedy for it. The
 *   attempt is withdrawn and started again with a fresh nonce and seed a few
 *   times; this is what is left when those ran out.
 * - `policyUnset`: the account has not chosen how renewals are confirmed yet
 *   (§5.6 「打开的顺序」): an admin writes a `SetRenewalPolicy` first.
 * - `refused`: it, or the hub, would not (`detail` says why).
 * - `badAnswer`: its answer does not check out (a signature, the DH
 *   confirmation, a key the chain named before, another chain view) —
 *   possibly not the computer itself answering.
 * - `wrongCode` / `cancelled`: the code typed is not the one the device shows,
 *   or none was typed.
 * - `voided`: the hub voided the staging (`detail`: `not_acked`, `conflict` —
 *   another holder of its old key showed up — or `expired`).
 * - `rate`: the hub's rate for renewals of this host.
 * - The host's own 412 `stale` (`agent/bridge.rs`: no admin's recent word on
 *   the account as it sees it), by its `reason`:
 *   - `unconfirmed` (`stale`): no other admin device vouched for its head
 *     (`hosts`: the account's other admins). This device's own word counts
 *     only while it is the sole admin (`ChainState.freshness`), so right after
 *     it changed the account — its renewal policy, say — another admin device
 *     has to see the change first; as the sole admin, its word did not reach
 *     the host (`detail`: why);
 *   - `behind`: an admin vouched for more of the chain than the host has: it
 *     catches up first;
 *   - `fork`: the host's view conflicts with what an admin vouched for — not
 *     tried again;
 *   - `lapsed`: the host takes no control from this device now (no admin's
 *     word in seven days, or this device may no longer control it).
 * - `failed`: anything else (`detail`).
 */
export type RenewalFailure =
  | "unconfirmed"
  | "behind"
  | "fork"
  | "lapsed"
  | "tooOld"
  | "disabled"
  | "needsAssurance"
  | "inProgress"
  | "offline"
  | "hubNotReady"
  | "waiting"
  | "renewFirst"
  | "superseded"
  | "policyUnset"
  | "refused"
  | "badAnswer"
  | "wrongCode"
  | "cancelled"
  | "voided"
  | "rate"
  | "failed";

export class RenewalError extends Error {
  constructor(
    readonly kind: RenewalFailure,
    message: string,
    /** The hub's or the agent's own reason, when it gave one. */
    readonly detail?: string,
    /** `waiting`: the hosts the hub waits for; `renewFirst`: the hosts to renew first; `unconfirmed`: the other admin devices, one of which has to vouch. */
    readonly hosts?: string[],
  ) {
    super(message);
    this.name = "RenewalError";
  }
}

/** What one completed identity key renewal did (`CloudAccount.renewIdentity`). */
export interface RenewalReport {
  host: string;
  /** The chain seq of its `rotateHostKey`. */
  seq: number;
  assurance: Assurance;
  signKey: Bytes;
  dhKey: Bytes;
  /** The new keys as people compare them ({@link deviceFingerprint}). */
  fingerprint: string;
  /** Its next machine key epoch, sealed to the new key only. */
  mkEpoch: number;
  result: DeliveryResult;
  /** How many of the hosts' keys its device role was sealed again (`null`: it has not signed in with the new key yet; it asks). */
  hostKeys: number | null;
  /** A new user key the batch carried (the newest was stale). */
  userKey?: FreshUserKey;
}

/** How long a renewal confirmed on the device waits for the person to type the code (`RenewOptions.codeWaitMs`). */
export const CODE_WAIT_MS = 10 * 60_000;

/** The code `answer` gives, or `null` (given up) once `ms` passed without one. */
export function answeredWithin(answer: Promise<string | null>, ms: number): Promise<string | null> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<null>((resolve) => {
    timer = setTimeout(() => resolve(null), ms);
  });
  return Promise.race([answer, late]).finally(() => clearTimeout(timer));
}

/** How `renewIdentity` goes (`assurance`), and what it may ask of the page. */
export interface RenewOptions {
  assurance: Assurance;
  /**
   * A renewal confirmed on the device (`ASSURANCE_ON_DEVICE`): ask the person
   * to type the code the device shows (`null`: they gave up). The page is
   * never told the code: it must be typed (§3.7).
   */
  askCode?: (host: string) => Promise<string | null>;
  /**
   * How long `askCode` is waited for ({@link CODE_WAIT_MS}): unanswered by
   * then, the renewal is cancelled as if the person gave up — a prompt left
   * open never holds the renewal runner's queue (and the disputes it checks).
   */
  codeWaitMs?: number;
  /** Where it is, for the page. */
  onStep?: (step: "requesting" | "confirming" | "waiting" | "writing" | "handing") => void;
  /** How often to look at the hub's staging, and for how long at most (the window and a margin). */
  pollMs?: number;
  maxWaitMs?: number;
  /** How long the host's device role is waited for to seal it the hosts' keys again (`handHostKeys`). */
  hostKeysWait?: { forMs: number; everyMs: number };
  /** Timers, for tests. */
  sleep?: (ms: number) => Promise<void>;
  now?: () => number;
}
