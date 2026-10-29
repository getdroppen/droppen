/*
 * A recovered or passkey-joined device's self-activation that this device
 * holds back (it came before a day had passed since this device first saw the
 * join), with whatever the hub has past it: what it is, what approving takes
 * in, what a veto takes out, and looking at it again once it may be taken.
 * `CloudAccount` documents the methods it hands to these.
 */
import { ChainClock, type ChainError, ChainState, type Entry } from "@/lib/e2e";
import type { AccountBasics } from "./basics";
import { AccountError } from "./errors";
import type { HeldBack, HeldChange, VetoPlan, VetoTarget } from "./views";

/** What the hold reads of the account beyond its basics. */
export interface ActivationHoldDeps {
  /** Catch up with the hub's chain. */
  sync(): Promise<number>;
  /** Take `chain` (the hub's, past the hold) with this device's own `mine` on it: the person settled the hold. */
  adopt(chain: ChainState, mine: Entry[]): Promise<void>;
  /** Latch this page's view as a fork (`CloudAccount.forked`). */
  latchFork(message: string): AccountError;
  /** Whether the page keeps its session fresh: only then is the hold looked at again on its own. */
  keepFresh(): boolean;
}

export class ActivationHold {
  constructor(
    private readonly basics: AccountBasics,
    private readonly deps: ActivationHoldDeps,
  ) {}

  /** The person settled the hold (approved it or vetoed it: `CloudAccount.adopt`): none is held now. */
  settled(): void {
    this.held = null;
    // Settled by the person: no earlier plan is a veto any more.
    this.taken = null;
    this.scheduleHeld();
  }

  /** Look at the hold again no more on its own: the page stopped keeping its session (`CloudAccount.stopSession`). */
  stopTimer(): void {
    if (this.heldTimer) clearTimeout(this.heldTimer);
    this.heldTimer = null;
  }

  /**
   * The hold the veto is over: this view's, or — taken in by this device's
   * clock since the person read the plan — that one, but only while it is
   * the last one taken in and the chain here has not moved since: a stale
   * plan could take out a device trusted after it (the person looks again).
   * `sweepTo`: for a hold taken in, how far the plan could stand for.
   */
  vetoedHold(id: string, veto: Pick<VetoPlan, "seq"> | undefined): { held: HeldBack | null; sweepTo: number | null } {
    let held: HeldBack | null = this.held;
    let sweepTo: number | null = null;
    if (!held && veto) {
      const taken = this.taken;
      if (!taken || taken.seq !== veto.seq || taken.device !== id || this.basics.state?.seq !== taken.takenThroughSeq) {
        throw new AccountError("vetoStale", "{{id}}'s activation was taken in here, and the account's chain has moved on since: look at your devices again, and remove what is not yours there", undefined, undefined, { id });
      }
      held = { seq: veto.seq, device: id, readyMs: 0, entries: [] };
      // What the plan could stand for: up to where this view took the hold in.
      sweepTo = taken.takenThroughSeq;
    }
    return { held, sweepTo };
  }

  /** A veto over the hold at `seq` went through: the plan it was read on is no veto any more. */
  planDone(seq: number): void {
    if (this.taken?.seq === seq) this.taken = null;
  }

  get heldBack(): HeldBack | null {
    return this.held;
  }

  vetoScope(held: HeldBack, chain: ChainState, opts: { trusted?: true | ReadonlySet<string>; view?: ReadonlySet<string> } = {}): { devices: string[]; machines: string[] } {
    const tainted = new Set<string>(held.device ? [held.device] : []);
    const machines = new Set<string>();
    const trust = opts.trusted;
    for (const e of held.entries) {
      const { signer, action } = e.draft;
      const brought = signer.type === "recovery" || signer.type === "passkey" || (signer.type === "device" && tainted.has(signer.id));
      // Signed by another device this view trusted: what it brought in goes only if asked.
      const byTrusted = !brought && trust !== undefined && signer.type === "device" && signer.id !== this.basics.deviceId;
      if (!brought && !byTrusted) continue;
      const take = (id: string, into: Set<string>) => {
        if (brought || trust === true || (typeof trust === "object" && trust.has(id))) into.add(id);
      };
      if (action.type === "addDevice" || action.type === "recoveryJoin" || action.type === "recoveryJoin3") take(action.device.id, tainted);
      else if (action.type === "addDevice3") take(action.device.id, action.device.flags.host && !action.device.flags.admin ? machines : tainted);
      else if (action.type === "approveDevice" || action.type === "activateRecovered" || action.type === "passkeyActivate") take(action.id, tainted);
      else if (action.type === "addMachine") take(action.machine.id, machines);
    }
    // Neither this device nor one this view already trusted: only what came in while the activation was held back.
    const view = opts.view ?? this.trustedView();
    const trustedHere = (id: string) => id === this.basics.deviceId || view.has(id);
    const devices = [...tainted].filter((id) => !trustedHere(id) && chain.deviceRecord(id) && chain.deviceRecord(id)!.status.type !== "removed");
    devices.sort((x, y) => (x === held.device ? -1 : y === held.device ? 1 : 0));
    return { devices, machines: [...machines].filter((m) => !trustedHere(m) && chain.machine(m)) };
  }

  async vetoPlan(): Promise<VetoPlan | null> {
    const held = this.held;
    if (!held?.device || !this.basics.state) return null;
    const view = this.trustedView();
    const chain = await this.provisional(held);
    const scope = this.vetoScope(held, chain, { view });
    const wide = this.vetoScope(held, chain, { view, trusted: true });
    const inScope = new Set([...scope.devices, ...scope.machines]);
    // Who brought each one in: the first held-back entry that did.
    const by = new Map<string, string | null>();
    for (const e of held.entries) {
      const { signer, action } = e.draft;
      const subject =
        action.type === "addDevice" || action.type === "recoveryJoin" || action.type === "recoveryJoin3" || action.type === "addDevice3"
          ? action.device.id
          : action.type === "addMachine"
            ? action.machine.id
            : action.type === "approveDevice" || action.type === "activateRecovered" || action.type === "passkeyActivate"
              ? action.id
              : null;
      if (subject !== null && subject !== held.device && !by.has(subject)) by.set(subject, signer.type === "device" ? signer.id : signer.type === "passkey" ? signer.slot : null);
    }
    const target = (id: string): VetoTarget => ({ id, host: chain.deviceRecord(id)?.host === true, by: by.get(id) ?? null });
    const trusted = [...wide.devices, ...wide.machines].filter((id) => !inScope.has(id));
    const voids = (ids: Iterable<string>) => [...ids].some((id) => chain.voidsRecoveryWords(id, "removal"));
    const voidsWords = voids(inScope);
    // The passkey slots a removal turns off, as the chain rules: those its device registered, and the one it came in through.
    const slotsOf = (ids: Iterable<string>) => {
      const gone = new Set(ids);
      const through = new Set([...gone].flatMap((id) => chain.deviceRecord(id)?.viaPasskey ?? []));
      return new Set(chain.allSlots().flatMap((s) => (!s.removed && (gone.has(s.setBy) || through.has(s.slot.slot)) ? [s.slot.slot] : [])));
    };
    const slots = slotsOf(inScope);
    return {
      device: held.device,
      seq: held.seq,
      view: [...view],
      scope: [...scope.devices, ...scope.machines].map(target),
      trusted: trusted.map(target),
      voidsWords,
      trustedVoidsWords: !voidsWords && voids(trusted),
      voidsPasskeys: slots.size > 0,
      trustedVoidsPasskeys: [...slotsOf(trusted)].some((slot) => !slots.has(slot)),
    };
  }

  heldChanges(): HeldChange[] {
    const held = this.held;
    if (!held) return [];
    return held.entries.map((e) => {
      const { signer, action } = e.draft;
      const subject =
        action.type === "addDevice" || action.type === "recoveryJoin" || action.type === "addDevice3" || action.type === "recoveryJoin3"
          ? action.device.id
          : action.type === "addMachine"
            ? action.machine.id
            : action.type === "removeDevice" ||
                action.type === "removeMachine" ||
                action.type === "approveDevice" ||
                action.type === "activateRecovered" ||
                action.type === "removeDevice3" ||
                action.type === "setAdmin" ||
                action.type === "setHost" ||
                action.type === "passkeyActivate"
              ? action.id
              : action.type === "setAccept"
                ? action.host
                : null;
      // A passkey slot's entries are named by the slot (as the alerts name them).
      return { seq: e.draft.seq, kind: action.type, by: signer.type === "device" ? signer.id : signer.type === "passkey" ? signer.slot : null, subject };
    });
  }

  private held: HeldBack | null = null;

  /**
   * The last hold a sync took in on this device's own clock (its day passed
   * here): its activation, and how far this view went with it. A veto the
   * person confirmed on that hold's plan is still a veto, but only while
   * nothing newer has been taken in since (`removeDevice`'s `veto`).
   */
  private taken: { seq: number; device: string; takenThroughSeq: number } | null = null;

  private heldTimer: ReturnType<typeof setTimeout> | null = null;

  /**
   * Why a merge of `served` stopped, as what this device holds back: `null`
   * when it did not stop; a self-activation too early here (and everything
   * the hub served from it) — with the recovery words, or through a passkey
   * slot (`passkeyActivate`: any admin may veto it, docs/passkeys.md §4.4,
   * so it names its device as a recovered one does); anything else is
   * thrown — a fork as the alarm it is (latched), an entry that does not
   * verify as itself.
   */
  heldFrom(error: ChainError | null, served: Entry[]): HeldBack | null {
    if (!error) return null;
    if (error.kind === "fork") throw this.deps.latchFork(error.message);
    if (error.kind !== "tooEarly" || error.detail.seq === undefined) throw error;
    const seq = error.detail.seq;
    const entries = served.filter((e) => e.draft.seq >= seq);
    const action = entries[0]?.draft.action;
    const device = action?.type === "activateRecovered" || action?.type === "passkeyActivate" ? action.id : null;
    return { seq, device, readyMs: error.detail.readyMs ?? 0, entries };
  }

  setHeld(held: HeldBack | null): void {
    const was = this.held;
    // A hold taken in by the clock (not by the person's approve or veto, `adopt`): kept, so a veto confirmed on it a moment ago still is one.
    if (was?.device && was.seq !== held?.seq && this.basics.state && this.basics.state.seq >= was.seq) {
      const at = this.basics.state.entryAt(was.seq)?.draft.action;
      if ((at?.type === "activateRecovered" || at?.type === "passkeyActivate") && at.id === was.device) this.taken = { seq: was.seq, device: was.device, takenThroughSeq: this.basics.state.seq };
    }
    this.held = held;
    this.scheduleHeld();
    if ((was?.seq ?? -1) !== (held?.seq ?? -1) || (was?.entries.length ?? 0) !== (held?.entries.length ?? 0)) this.basics.emit();
  }

  /** While the page is open, look again once a held-back activation may be taken. */
  scheduleHeld(): void {
    if (this.heldTimer) clearTimeout(this.heldTimer);
    this.heldTimer = null;
    if (!this.held || !this.deps.keepFresh()) return;
    const wait = Math.max(1000, this.held.readyMs - Date.now() + 1000);
    this.heldTimer = setTimeout(
      () => {
        this.heldTimer = null;
        this.deps.sync().catch((error: unknown) => console.warn("cloud account: sync after a held-back activation:", error));
      },
      Math.min(wait, 6 * 60 * 60 * 1000),
    );
    (this.heldTimer as { unref?: () => void }).unref?.();
  }

  /**
   * The hub's chain past a held-back activation, verified with no clock:
   * only ever to sign this device's veto on (a removal must go on the hub's
   * head) or to take on the user's word ("approve") — never trusted before.
   */
  async provisional(held: HeldBack): Promise<ChainState> {
    return ChainState.verify([...this.basics.state!.allEntries(), ...held.entries], undefined, ChainClock.history());
  }

  /**
   * While this device holds back a self-activation ({@link heldBack}), it
   * hands no device its keys by pairing — not even one already listed, which
   * needs no new entry, and so would not meet the refusal every entry but
   * the veto meets (`appendBatch`): that device, opening the account afresh,
   * may take the held activation as history. It waits, as they do, until the
   * person approves or removes the held device.
   */
  refuseWhileHeld(): void {
    const held = this.held;
    if (!held) return;
    throw new AccountError(
      "heldBack",
      "{{id}} activated itself sooner than this device allows: approve it or remove it first. Until then this device adds no device and pairs none again.",
      held.readyMs,
      undefined,
      { id: held.device ?? "?" },
    );
  }

  /** The devices this view trusts now (active in it), and this device: what a veto never removes. */
  trustedView(): Set<string> {
    const ids = new Set((this.basics.state?.allDevices() ?? []).filter((d) => d.status.type === "active").map((d) => d.info.id));
    ids.add(this.basics.deviceId);
    return ids;
  }

  /**
   * "That was me" for a held-back self-activation: take the hub's chain past
   * it now, on the user's word — unless that chain no longer has this device
   * (then approving would only be a way to wipe it).
   */
  async vouchForHeld(held: HeldBack): Promise<void> {
    const chain = await this.provisional(held);
    if (chain.device(this.basics.deviceId) === undefined) {
      throw new AccountError("heldBack", "the hub's chain goes on to remove this device: remove the recovered device from another device instead");
    }
    await this.deps.adopt(chain, []);
  }
}
