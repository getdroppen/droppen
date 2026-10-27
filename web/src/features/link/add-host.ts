/*
 * What droppen.app/link does once the person consents (docs/local-aggregator.md
 * §4.3 steps 5–7, S1-4), apart from the page: issue the code and hand it to
 * the computer's page, wait for its proof under exactly the keys it showed,
 * add it — `[addDevice3{host, admin}, keyEpochs]` on a v3 chain, v2's
 * `[addMachine, keyEpochs]` before — hand it its own content key, and then
 * the content keys of the other devices that accept its control, once its
 * device role signed in at the hub (S1-4: the local install controls the
 * others with its one identity; it asks for anything still missing itself).
 *
 * Every local install runs AI, so it is never a full device (review D1: its
 * key sits in a file every program of its user can read, §3.3, and its agent
 * neither vouches for the chain nor signs). The consent page offers no
 * choice; `admin: true` is refused here before any code is used.
 *
 * Two ways in, one sequence: the sign-in's browser hand-off knows the keys the
 * computer showed (`addHost`, which pins them on the proof), and the code
 * screen (`addHostByCode`) has no keys to pin — its computer proves under keys
 * this page never saw. Only `addHost` takes `keys`, so that pinning cannot be
 * skipped by accident.
 */
import type { EnrollCode } from "@/lib/e2e";
import { type CloudAccount, HOST_KEYS_WAIT, hostAdminRefused } from "@/lib/hub/account";
import type { DeliveryResult, MachineKeyCourier } from "@/lib/hub/deliver";

export type AddStep = "code" | "proof" | "key" | "hostKeys";

/** What adding needs besides the account, either way. */
interface AddHostCommon {
  /** The hub takes chain v3 entries now (`/v1/me`'s `chain_v3.writable`). */
  writable: boolean | (() => boolean);
  /** How its own content key reaches it. */
  courier: MachineKeyCourier;
  onStep?: (step: AddStep) => void;
  signal?: AbortSignal;
}

/** What adding the computer that asked (the local sign-in's browser hand-off) needs, besides {@link AddHostCommon}. */
export interface AddHostOptions extends AddHostCommon {
  writable: boolean;
  /** The keys the computer's page showed: a proof under any others is refused. */
  keys: { signKey: Uint8Array; dhKey: Uint8Array };
  /** Whether it is added as a full device: always `false` (refused for a host: D1). */
  admin: boolean;
  /**
   * The code, to the computer's page only: postMessage to `cb`, or — the
   * browser hand-off — sealed to its attempt, through a tab at `cb`.
   */
  sendCode: (code: EnrollCode) => void | Promise<void>;
  pollMs?: number;
  /** How long it waits for its device role before leaving the other keys to its own request. */
  hostKeysWait?: { forMs: number; everyMs: number };
}

/** What adding a computer from a code this page issues needs, besides {@link AddHostCommon}. */
export interface AddHostByCodeOptions extends AddHostCommon {
  /** Read when the proof is in (the hub may start taking v3 entries while the computer waits). */
  writable: () => boolean;
  /** The code the hub issued, and when it runs out, for the screen's countdown. */
  onCode: (code: EnrollCode, expiresMs: number | null) => void;
}

/** What was added. */
export interface Added {
  machine: string;
  /** The other devices' content keys sealed to it; `null`: its device role did not sign in in time (it asks itself). */
  hostKeys: number | null;
}

/** What the code screen added: the machine, and how its own content key reached it. */
export interface AddedByCode {
  machine: string;
  result: DeliveryResult;
}

/**
 * The sequence both ways share: issue the code, wait for the proof, add the
 * computer and hand it its own content key, then the other devices' keys.
 * `keys` absent is the code screen's way: the computer proves under keys this
 * page never saw, and the other keys are not waited for (it asks itself).
 */
async function addHostEnrollment(
  account: CloudAccount,
  opts: AddHostCommon & {
    issue: () => Promise<{ code: EnrollCode; expiresMs: number | null }>;
    onCode: (code: EnrollCode, expiresMs: number | null) => void | Promise<void>;
    keys?: { signKey: Uint8Array; dhKey: Uint8Array };
    admin: boolean;
    pollMs?: number;
    hostKeysWait?: { forMs: number; everyMs: number };
  },
): Promise<{ machine: string; result: DeliveryResult; hostKeys: number | null }> {
  opts.onStep?.("code");
  const { code, expiresMs } = await opts.issue();
  await opts.onCode(code, expiresMs);
  opts.onStep?.("proof");
  const writable = typeof opts.writable === "function" ? opts.writable() : opts.writable;
  const machine = await account.completeEnrollment(code, opts.signal, opts.pollMs, opts.keys, { writable, admin: opts.admin });
  opts.onStep?.("key");
  const { result } = await account.handOverMachineKey(machine.id, opts.courier);
  opts.onStep?.("hostKeys");
  if (opts.keys === undefined) {
    // The code screen's way: in the background, and the computer asks for them itself if it never signs in here.
    void account.handHostKeys(machine.id).catch((error: unknown) => console.warn(`could not hand ${machine.id} the other devices' keys (it asks for them itself):`, error));
    return { machine: machine.id, result, hostKeys: null };
  }
  let hostKeys: number | null = null;
  try {
    hostKeys = await account.handHostKeys(machine.id, opts.hostKeysWait ?? HOST_KEYS_WAIT, opts.signal);
  } catch (error) {
    // Added and keyed: the rest it asks for itself (purpose 4).
    console.warn(`link: could not hand ${machine.id} the other devices' keys (it asks for them itself):`, error);
  }
  return { machine: machine.id, result, hostKeys };
}

/** Add the computer that asked (see the module docs). */
export async function addHost(account: CloudAccount, opts: AddHostOptions): Promise<Added> {
  // Every local install runs AI (review D1): refused before a code is issued.
  if (opts.admin) throw hostAdminRefused();
  const { machine, hostKeys } = await addHostEnrollment(account, {
    writable: opts.writable,
    courier: opts.courier,
    onStep: opts.onStep,
    signal: opts.signal,
    hostKeysWait: opts.hostKeysWait,
    issue: async () => ({ code: await account.startEnrollment(), expiresMs: null }),
    onCode: opts.sendCode,
    keys: opts.keys,
    admin: opts.admin,
    pollMs: opts.pollMs ?? 500,
  });
  return { machine, hostKeys };
}

/**
 * Add a computer from a code this page shows (`agent enroll`): no keys were
 * shown to this page, and none are pinned on its proof.
 */
export async function addHostByCode(account: CloudAccount, opts: AddHostByCodeOptions): Promise<AddedByCode> {
  const { machine, result } = await addHostEnrollment(account, {
    writable: opts.writable,
    courier: opts.courier,
    onStep: opts.onStep,
    signal: opts.signal,
    issue: () => account.startTimedEnrollment(),
    onCode: opts.onCode,
    admin: false,
  });
  return { machine, result };
}
