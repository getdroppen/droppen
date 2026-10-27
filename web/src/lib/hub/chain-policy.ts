import {
  type Accept,
  type Action,
  ASSURANCE_ON_DEVICE,
  type Assurance,
  type Bytes,
  ChainClock,
  ChainError,
  ChainState,
  copy,
  type DeviceRecord,
  type Entry,
  equal,
  fromHex,
  hex,
  type HostRotation,
  isValidEd25519Key,
  isValidX25519,
  type OpenPause,
  PAUSE_START,
  type PauseRemoval,
  RULES,
  utf8,
  verifyHostRotation,
} from "@/lib/e2e";
import { AccountError, type FreshUserKey, RenewalError } from "./account";
import { HubApiError } from "./api";
import type { DeliveryResult, MachineKeyCourier } from "./deliver";
import type { ControllersRecord } from "./streams";

/*
 * What the account's chain says about control: who may control a host, which
 * devices it should be re-enrolled for, which hosts must renew their identity
 * key and in what order, and how a renewal's answer is checked — the policy
 * half of `account.ts`, over a `ChainState` and nothing of the page. Pure:
 * every function here reads the chain (and, where a caller says so, a host's
 * own record) and returns its answer; nothing writes anything.
 */
/**
 * At most this many RESOLVED disputes kept: a dispute that
 * still isolates its host on this page's chain is never evicted, however many
 * come — an alarm is logged instead — so copies of one statement, or many of
 * another host's, cannot push out the one that isolates a host.
 */
export const MAX_KEPT_DISPUTES = 512;

/**
 * Whether device `id` ever held the user key, by the chain's entries rather
 * than its flags now: a full v2 device, a v3 admin, a device given the
 * account by `setAdmin true`, or one that came back with the recovery words.
 * A device that held it once may still hold a copy, so removing or demoting
 * it is a user-key loss and the newest epoch is stale until a new one is
 * issued ({@link lastUserKeyLoss}, {@link userKeyStale}) — whatever the
 * device may do now.
 */
export function heldUserKey(chain: ChainState, id: string): boolean {
  for (const e of chain.allEntries()) {
    const a = e.draft.action;
    switch (a.type) {
      case "genesis":
      case "addDevice":
        if (a.device.id === id && a.device.full) return true;
        break;
      case "genesis3":
      case "addDevice3":
        if (a.device.id === id && a.device.flags.admin) return true;
        break;
      case "recoveryJoin":
      case "recoveryJoin3":
        if (a.device.id === id) return true;
        break;
      case "setAdmin":
        if (a.id === id && a.admin) return true;
        break;
      default:
        break;
    }
  }
  return false;
}

/**
 * The last entry that took the account from a device that held the user key
 * — the removal of a device that ever held it ({@link heldUserKey}: by the
 * chain's history, not its flags now — review 1: demoted, then removed, it
 * held it all the same), or a demotion (`setAdmin false`) — or `null`.
 */
export function lastUserKeyLoss(chain: ChainState): number | null {
  for (let seq = chain.seq; seq > 0; seq--) {
    const a = chain.entryAt(seq)?.draft.action;
    if ((a?.type === "removeDevice" || a?.type === "removeDevice3") && heldUserKey(chain, a.id)) return seq;
    if (a?.type === "setAdmin" && !a.admin) return seq;
  }
  return null;
}

/**
 * Whether the newest user key is still one a device that no longer manages
 * the account held: no new user key epoch (`keyEpochs`) since the chain's
 * last such loss ({@link lastUserKeyLoss}). Nothing new is wrapped under it:
 * a batch that issues machine keys meanwhile carries a new user key epoch.
 */
export function userKeyStale(chain: ChainState): boolean {
  const lost = lastUserKeyLoss(chain);
  if (lost === null) return false;
  for (let seq = lost + 1; seq <= chain.seq; seq++) {
    const a = chain.entryAt(seq)?.draft.action;
    if (a?.type === "keyEpochs" && a.user) return false;
  }
  return true;
}

/**
 * Whether a rotation is due ("rotate keys again"): since the chain's last
 * loss of a device that held the user key ({@link lastUserKeyLoss}), either
 * no new user key epoch came, or some current host has had no new machine
 * key epoch at or after the first one that did — it still seals under a key
 * the departed device could read. A batch that carried a new user key for
 * one host alone (an enrollment while the user key was stale) leaves it due
 * for the others.
 */
export function rotationDueIn(chain: ChainState): boolean {
  const lost = lastUserKeyLoss(chain);
  if (lost === null) return false;
  let fresh = false;
  const rotated = new Set<string>();
  for (let seq = lost + 1; seq <= chain.seq; seq++) {
    const a = chain.entryAt(seq)?.draft.action;
    if (a?.type !== "keyEpochs") continue;
    if (a.user) fresh = true;
    if (fresh) for (const m of a.machines) rotated.add(m.machine);
  }
  return !fresh || chain.hosts().some((h) => h.status.type === "active" && !rotated.has(h.info.id));
}

/**
 * The current hosts whose identity key a device that can no longer control
 * them could have taken (review D3: control is full access to that computer,
 * its identity key included): after the host joined, a device that could
 * control it (an admin, or one its accept list named) was removed, demoted
 * without being named, or shut out by a narrowed list. Rotating the keys
 * does not help against it: with a copied identity key it still signs in as
 * that computer, and each new machine key is handed to that identity.
 * Re-enrolling the computer (remove it, then add it again) gives it a new
 * identity (a new device id); nothing is done automatically.
 *
 * Kept for the hosts that cannot renew their identity key (an agent without
 * `rotate/1`, a v2 account: docs/host-identity-rotation.md §7 decision 7):
 * the others renew it instead ({@link hostsToRotate}).
 */
export function reEnrollCulprits(chain: ChainState): Map<string, Map<string, number>> {
  const culprits = new Map<string, Map<string, number>>();
  const hosts = chain.hosts().filter((h) => h.status.type === "active");
  replayChain(chain, {
    // As this list has always read it: every recovery join is an admin.
    recoveryJoin3Admin: () => true,
    lost: (_replay, seq, device, who) => {
      for (const h of hosts) {
        if (seq <= h.addedSeq || !who(h.info.id)) continue;
        let devices = culprits.get(h.info.id);
        if (!devices) culprits.set(h.info.id, (devices = new Map()));
        devices.set(device, seq);
      }
    },
    afterEntry: (replay) => {
      // A device let back in (an accept list that names it again, made an admin again) is not lost any more.
      for (const [host, devices] of culprits) for (const d of [...devices.keys()]) if (replay.could(d, host)) devices.delete(d);
    },
  });
  for (const [host, devices] of [...culprits]) if (devices.size === 0) culprits.delete(host);
  return culprits;
}

/** One device as a replay over the chain ({@link replayChain}) follows it. */
export interface ReplayDevice {
  admin: boolean;
  active: boolean;
  /** Whether the chain says it is a host now. */
  host: boolean;
  /** The seq that added it. */
  addedSeq: number;
}

/** The device and accept state a replay keeps, as its hooks read it. */
export interface ChainReplay {
  devs: Map<string, ReplayDevice>;
  accept: Map<string, Accept>;
  /** Whether `device` may control `host` now. */
  could(device: string, host: string): boolean;
  /** `host`'s accept list now (the admins', when it narrowed none). */
  policy(host: string): Accept;
}

/** What a caller hears as {@link replayChain} goes. */
export interface ChainReplayHooks {
  /** Whether a `recoveryJoin3` device is an admin here. */
  recoveryJoin3Admin(flags: { admin: boolean }): boolean;
  /** `device` lost control of every host `who` names (the predicate was true at the loss). */
  lost(replay: ChainReplay, seq: number, device: string, who: (host: string) => boolean): void;
  /** After each entry: what a later loss or admission makes no longer lost. */
  afterEntry?(replay: ChainReplay, seq: number): void;
  /** A `setAccept` entry, before the list is replaced (an admission may start here). */
  setAccept?(replay: ChainReplay, seq: number, host: string, before: Accept, from: Accept): void;
  /** A `setHost` entry, after the accept list it drops when it stops being a host. */
  setHost?(seq: number, id: string, host: boolean): void;
  /** A `rotateHostKey` entry (a renewal clears what the new identity key cannot be a pass to). */
  rotate?(seq: number, action: Extract<Action, { type: "rotateHostKey" }>): void;
  /** A `renewalPause` entry, its start or its end. */
  pause?(replay: ChainReplay, seq: number, action: Extract<Action, { type: "renewalPause" }>): void;
  /** A device removed (its hosts keep what it may have taken; the device itself is gone). */
  removed?(seq: number, device: string): void;
}

/**
 * One pass over the chain's device and accept entries: who may control each
 * host, and every loss of that control — the state both the re-enroll list
 * ({@link reEnrollCulprits}) and the rotation list ({@link hostsToRotate})
 * are built from, so the two cannot disagree about what the chain says.
 */
export function replayChain(chain: ChainState, hooks: ChainReplayHooks): ChainReplay {
  const devs = new Map<string, ReplayDevice>();
  const accept = new Map<string, Accept>();
  const policy = (host: string) => accept.get(host) ?? ({ type: "admins" } as Accept);
  const replay: ChainReplay = {
    devs,
    accept,
    policy,
    could: (d, host) => {
      const r = devs.get(d);
      return !!r && r.active && d !== host && (r.admin || acceptNames(policy(host), d));
    },
  };
  for (const e of chain.allEntries()) {
    const seq = e.draft.seq;
    const signer = e.draft.signer;
    const a = e.draft.action;
    switch (a.type) {
      case "genesis":
      case "addDevice":
        devs.set(a.device.id, { admin: a.device.full, active: true, host: false, addedSeq: seq });
        break;
      case "genesis3":
        devs.set(a.device.id, { admin: a.device.flags.admin, active: true, host: a.device.flags.host, addedSeq: seq });
        break;
      case "addDevice3":
        devs.set(a.device.id, { admin: a.device.flags.admin, active: signer.type !== "passkey", host: a.device.flags.host, addedSeq: seq });
        break;
      case "addMachine":
        devs.set(a.machine.id, { admin: false, active: true, host: true, addedSeq: seq });
        break;
      case "recoveryJoin":
        devs.set(a.device.id, { admin: true, active: false, host: false, addedSeq: seq });
        break;
      case "recoveryJoin3":
        devs.set(a.device.id, { admin: hooks.recoveryJoin3Admin(a.device.flags), active: false, host: a.device.flags.host, addedSeq: seq });
        break;
      case "approveDevice":
      case "activateRecovered":
      case "passkeyActivate": {
        const r = devs.get(a.id);
        if (r) r.active = true;
        break;
      }
      case "setAdmin": {
        const r = devs.get(a.id);
        if (!r) break;
        if (!a.admin) hooks.lost(replay, seq, a.id, (h) => replay.could(a.id, h) && !acceptNames(policy(h), a.id));
        r.admin = a.admin;
        break;
      }
      case "setAccept": {
        const before = policy(a.host);
        for (const [d, r] of devs) {
          if (r.active && !r.admin && d !== a.host && acceptNames(before, d) && !acceptNames(a.from, d)) hooks.lost(replay, seq, d, (h) => h === a.host);
        }
        hooks.setAccept?.(replay, seq, a.host, before, a.from);
        if (a.from.type === "admins") accept.delete(a.host);
        else accept.set(a.host, a.from);
        break;
      }
      case "setHost": {
        const r = devs.get(a.id);
        if (r) r.host = a.host;
        if (!a.host) accept.delete(a.id);
        hooks.setHost?.(seq, a.id, a.host);
        break;
      }
      case "removeDevice":
      case "removeMachine":
      case "removeDevice3": {
        hooks.lost(replay, seq, a.id, (h) => replay.could(a.id, h));
        const r = devs.get(a.id);
        if (r) r.active = false;
        hooks.removed?.(seq, a.id);
        break;
      }
      case "rotateHostKey":
        hooks.rotate?.(seq, a);
        break;
      case "renewalPause":
        hooks.pause?.(replay, seq, a);
        break;
      default:
        break;
    }
    hooks.afterEntry?.(replay, seq);
  }
  return replay;
}

/** Why a current host should renew its identity key ({@link hostsToRotate}). */
export interface Exposure {
  /** The entry it became exposed at: the earliest cause still standing. */
  sinceSeq: number;
  /** Devices that could control it and no longer can (removed, demoted, shut out of its list), with the seq each lost that. */
  lost: Map<string, number>;
  /** Exposed hosts it let control it while they were exposed (transitive, §4.1), with the seq it first did. */
  via: Map<string, number>;
  /**
   * Exposed hosts that cannot renew their identity key (an older agent, a v2
   * account) — or hosts in that state themselves — it lets control it now:
   * renewing it would not help while it does (§4.1 case 3). Narrowing its
   * list, or adding that host again, does.
   */
  blockedBy: Map<string, number>;
}

/**
 * The exposed hosts `host` lets control it (§4.1 「次序先于收窄」): each must have
 * renewed before `host` does — its old key is dead by then. Not `host` itself.
 */
export function exposedControllers(chain: ChainState, exposed: ReadonlyMap<string, Exposure>, host: string): string[] {
  // As the chain admits now (`mayControl`): an edge a renewal pause holds is no controller of it.
  return [...exposed.keys()].filter((e) => e !== host && chain.machine(e) && admits(chain, host, e)).sort();
}

/**
 * The strongly connected sets of the exposed hosts' admission graph (an edge E → H
 * where H lets E control it), Tarjan's way: a set of two or more is a cycle, where
 * no order of renewals works (§4.1). Sorted, for a stable batch.
 */
export function exposedCycles(chain: ChainState, exposed: ReadonlyMap<string, Exposure>): string[][] {
  const ids = [...exposed.keys()].filter((h) => chain.machine(h)).sort();
  const next = (v: string) => ids.filter((h) => h !== v && admits(chain, h, v));
  const index = new Map<string, number>();
  const low = new Map<string, number>();
  const stack: string[] = [];
  const on = new Set<string>();
  const out: string[][] = [];
  let i = 0;
  const visit = (v: string) => {
    index.set(v, i);
    low.set(v, i);
    i++;
    stack.push(v);
    on.add(v);
    for (const w of next(v)) {
      if (!index.has(w)) {
        visit(w);
        low.set(v, Math.min(low.get(v)!, low.get(w)!));
      } else if (on.has(w)) low.set(v, Math.min(low.get(v)!, index.get(w)!));
    }
    if (low.get(v) === index.get(v)) {
      const set: string[] = [];
      for (let w = stack.pop()!; ; w = stack.pop()!) {
        on.delete(w);
        set.push(w);
        if (w === v) break;
      }
      if (set.length >= 2) out.push(set.sort());
    }
  };
  for (const v of ids) if (!index.has(v)) visit(v);
  return out.sort((x, y) => (x[0]! < y[0]! ? -1 : 1));
}

/**
 * The pauses §4.1 calls for, before any of them renews — only where no order
 * of renewals works: for each cycle of exposed hosts that let each other
 * control them (`exposedCycles`, two or more, one of them able to renew), the
 * removals of one `RenewalPause` start (§3.4 rule 10): every member, and the
 * other members it lets control it now. The lists themselves (the base) are
 * never touched. Acyclic parts are never paused: the order of renewals takes
 * care of them (`exposedControllers`). Empty: no such cycle.
 */
export function groupNarrowing(chain: ChainState, exposed: ReadonlyMap<string, Exposure>, renews: (host: string) => boolean): { hosts: string[]; removals: PauseRemoval[] }[] {
  const all: { hosts: string[]; removals: PauseRemoval[] }[] = [];
  for (const cycle of exposedCycles(chain, exposed)) {
    if (!cycle.some(renews)) continue;
    // Canonical, as the chain requires: hosts, and each host's ids, ascending by bytes.
    const removals = [...cycle].sort(compareUtf8Ids).flatMap((host) => {
      const ids = cycle.filter((e) => e !== host && admits(chain, host, e)).sort(compareUtf8Ids);
      return ids.length ? [{ host, ids }] : [];
    });
    if (removals.length) all.push({ hosts: cycle, removals });
  }
  return all;
}

/** Strings by their UTF-8 bytes: the chain's canonical order for a pause's hosts and ids. */
export const compareUtf8Ids = (x: string, y: string) => compareBytes(utf8(x), utf8(y));

/**
 * `removals` (canonical: hosts, and each host's ids, ascending by bytes) cut
 * into pauses each of which `fits` (M6: the whole encoded entry within
 * `MAX_PAUSE_ENTRY_LEN`) — in order, a host's ids split across pauses only
 * when they alone would not fit one. Each part stays canonical. A single
 * edge that does not fit is refused (it cannot be paused at all).
 */
export function splitPauseRemovals(removals: readonly PauseRemoval[], fits: (removals: PauseRemoval[]) => boolean): PauseRemoval[][] {
  const parts: PauseRemoval[][] = [];
  let part: PauseRemoval[] = [];
  const close = () => {
    if (part.length) parts.push(part);
    part = [];
  };
  for (const r of removals) {
    for (const id of r.ids) {
      const last = part.at(-1);
      const tried = last?.host === r.host ? [...part.slice(0, -1), { host: r.host, ids: [...last.ids, id] }] : [...part, { host: r.host, ids: [id] }];
      if (fits(tried)) {
        part = tried;
        continue;
      }
      close();
      const alone = [{ host: r.host, ids: [id] }];
      if (!fits(alone)) throw new AccountError("invalidEntry", "a renewal pause of one edge does not fit an entry", undefined, RULES.pauseTooLarge);
      part = alone;
    }
  }
  close();
  return parts;
}

/** What ending an open renewal pause now gives back, what must stay held and why, and whether its starter still manages the account. */
export interface PauseEnding {
  edges: { host: string; id: string }[];
  /** The hosts among them whose base lets every device in. */
  all: string[];
  held: { host: string; id: string; who: string; why: "exposed" | "cannotRenew" | "isolated" | "flagged" }[];
  /** Its starter no longer manages the account: it is never ended on its own, only by a person (M7). */
  orphan: boolean;
}

/**
 * The control edges an open renewal pause would give back if it ended now
 * (§4.1 「结束之前」): each `(host, id)` it holds that no other open pause holds,
 * whose host is current, whose id is a current device, and that the host's
 * base list still names — exactly what its completion or cancellation
 * re-enables. `all`: the hosts among them whose base lets every device in.
 */
export function pauseReturns(chain: ChainState, pause: OpenPause): { edges: { host: string; id: string }[]; all: string[] } {
  const others = chain.openPauses().filter((p) => !equal(p.op, pause.op));
  const heldElsewhere = (host: string, id: string) => others.some((p) => p.removals.some((r) => r.host === host && r.ids.includes(id)));
  const edges: { host: string; id: string }[] = [];
  const all = new Set<string>();
  for (const r of pause.removals) {
    if (!chain.machine(r.host)) continue;
    const base = chain.baseAccept(r.host);
    for (const id of r.ids) {
      if (!chain.device(id) || heldElsewhere(r.host, id) || !acceptNames(base, id)) continue;
      edges.push({ host: r.host, id });
      if (base.type === "all") all.add(r.host);
    }
  }
  return { edges, all: [...all].sort() };
}

/**
 * Why an edge a pause holds must stay held (§4.1 「结束之前」), judged on
 * `chain` alone — whatever an earlier renewal, or the hub's word on what can
 * renew, says: either end exposed on it, or held by isolation or the hub's
 * flag. `null`: it may come back.
 */
export function edgeHeld(
  chain: ChainState,
  edge: { host: string; id: string },
  held: { exposed: ReadonlyMap<string, Exposure>; isolated: (id: string) => boolean; flagged: (id: string) => boolean },
): { who: string; why: "exposed" | "isolated" | "flagged" } | null {
  for (const who of [edge.id, edge.host]) {
    if (held.isolated(who)) return { who, why: "isolated" };
    if (held.flagged(who)) return { who, why: "flagged" };
    if (held.exposed.has(who)) return { who, why: "exposed" };
  }
  return null;
}

/** Whether renewing a host's identity key ends its exposure: something renewing fixes, and nothing that it does not. */
export function renewalHelps(x: Exposure): boolean {
  return x.blockedBy.size === 0 && (x.lost.size > 0 || x.via.size > 0);
}

/**
 * The current hosts whose identity key may have been copied, each with why
 * (docs/host-identity-rotation.md §4.1, §7 decisions 3 and 8):
 *
 * - the losses {@link reEnrollCulprits} counts — after the host joined, a
 *   device that could control it (an admin, or one its list named) was
 *   removed, demoted without being named, or shut out of its list; a device
 *   let back in is not lost any more;
 * - transitively: while a host E that can renew (`renews`) is exposed, a host
 *   that lets E control it is exposed too — E's copy is a pass to it (X → H →
 *   H2: H2 as well, through H);
 * - cleared only by a later `rotateHostKey` of that very host. After it, only
 *   three things expose it again (§4.1 「换完之后什么会让它再暴露」): a new
 *   loss after that renewal (and what a new loss exposes transitively); an
 *   exposed host added to its list after it; and letting an exposed host that
 *   cannot renew control it (`blockedBy`: no renewal helps then — narrow the
 *   list or add that host again). An exposed host that can renew, and that it
 *   let in before its renewal, does not expose it again: every exposed host
 *   is marked to renew right after the loss, and the hub suspends its device
 *   role meanwhile (§4.3). So two hosts that let each other control one
 *   another renew once each.
 *
 * `renews` says which hosts can renew (every one when not given: the chain
 * alone). Nothing a host's own record says (its status's `controllers`) takes a
 * host out: a renewal is cheap, and a device that did connect could have
 * edited that record.
 */
export function hostsToRotate(chain: ChainState, renews: (host: string) => boolean = () => true): Map<string, Exposure> {
  /** Per host, since when its list has named each device (`*`: since it let every device in). */
  const admitted = new Map<string, Map<string, number>>();
  /** Per host, its latest renewal. */
  const renewedAt = new Map<string, number>();
  const exposed = new Map<string, { since: number; lost: Map<string, number>; via: Map<string, number> }>();
  /** The renewal pauses open at this point of the chain (§3.4 rule 10): by op, the edges each takes out of its hosts' lists. */
  const pauses = new Map<string, PauseRemoval[]>();
  const paused = (host: string, id: string) => [...pauses.values()].some((rs) => rs.some((r) => r.host === host && r.ids.includes(id)));
  /** Since when `host`'s list has let `id` in, as the overlay of open pauses leaves it (`undefined`: it does not now). */
  const admittedSince = (replay: ChainReplay, host: string, id: string): number | undefined => {
    if (!acceptNames(replay.policy(host), id) || paused(host, id)) return undefined;
    const since = admitted.get(host);
    const named = since?.get(id);
    if (named !== undefined) return named;
    const all = since?.get("*");
    return all === undefined ? undefined : Math.max(all, replay.devs.get(id)?.addedSeq ?? 0);
  };
  const entry = (host: string, seq: number) => {
    let x = exposed.get(host);
    if (!x) exposed.set(host, (x = { since: seq, lost: new Map(), via: new Map() }));
    return x;
  };
  replayChain(chain, {
    recoveryJoin3Admin: (flags) => flags.admin,
    lost: (replay, seq, device, who) => {
      for (const [h, r] of replay.devs) {
        if (!r.active || !r.host || h === device || !who(h)) continue;
        entry(h, seq).lost.set(device, seq);
      }
    },
    setAccept: (replay, seq, host, before, from) => {
      // Since when it lets each one in: kept for those it let in already, this entry for the rest.
      const was = admitted.get(host) ?? new Map<string, number>();
      const now = new Map<string, number>();
      if (from.type === "all") now.set("*", before.type === "all" ? (was.get("*") ?? seq) : seq);
      if (from.type === "devices") for (const id of from.ids) now.set(id, admittedSince(replay, host, id) ?? seq);
      admitted.set(host, now);
    },
    setHost: (seq, id, host) => {
      // Its list goes with it; what it may have been copied stays (its identity key is the same when it runs AI again).
      if (!host) admitted.delete(id);
    },
    rotate: (seq, action) => {
      // A new identity: nothing anyone copied up to the view its seed was staged on is a pass to it any more. A loss
      // after `staged_seq` is no remedy of this one (§4.1 「清除」): the seed was made before it.
      const upTo = Math.min(action.rotation.stagedSeq, seq);
      const x = exposed.get(action.rotation.host);
      if (x) {
        for (const [d, at] of [...x.lost]) if (at <= upTo) x.lost.delete(d);
        for (const [d, at] of [...x.via]) if (at <= upTo) x.via.delete(d);
        if (x.lost.size === 0 && x.via.size === 0) exposed.delete(action.rotation.host);
        else x.since = Math.min(...x.lost.values(), ...x.via.values());
      }
      renewedAt.set(action.rotation.host, upTo);
    },
    pause: (replay, seq, action) => {
      const op = hex(action.op);
      if (action.phase === PAUSE_START) {
        pauses.set(op, action.removals.map((r) => ({ host: r.host, ids: [...r.ids] })));
        return;
      }
      // Ended (completed or cancelled): each edge it held and no other open pause holds is let in again — now, as an
      // addition to the list after any renewal before it (§4.1: an exposed host let back in exposes again).
      const removals = pauses.get(op) ?? [];
      pauses.delete(op);
      for (const r of removals) {
        const since = admitted.get(r.host);
        if (!since) continue;
        for (const id of r.ids) if (!paused(r.host, id) && acceptNames(replay.policy(r.host), id)) since.set(id, seq);
      }
    },
    removed: (_seq, device) => {
      exposed.delete(device);
    },
    afterEntry: (replay, seq) => {
      // A device let back in is not lost any more.
      for (const [host, x] of exposed) for (const d of [...x.lost.keys()]) if (replay.could(d, host)) x.lost.delete(d);
      for (const [host, x] of [...exposed]) if (x.lost.size === 0 && x.via.size === 0) exposed.delete(host);
      // Transitively, until nothing changes: a host that lets an exposed one that can renew control it (X → H → H2 …) —
      // unless it renewed after that one was exposed and let in already (§4.1: the copy cannot reach its new key).
      for (let changed = true; changed; ) {
        changed = false;
        for (const [source, sx] of [...exposed]) {
          if (!renews(source)) continue;
          for (const [h, r] of replay.devs) {
            if (!r.active || !r.host || h === source) continue;
            const since = admittedSince(replay, h, source);
            if (since === undefined) continue;
            const renewed = renewedAt.get(h);
            if (renewed !== undefined && sx.since <= renewed && since <= renewed) continue;
            const x = exposed.get(h);
            if (x?.via.has(source)) continue;
            entry(h, seq).via.set(source, seq);
            changed = true;
          }
        }
      }
    },
  });
  // What no renewal helps: letting an exposed host that cannot renew control it — or a host in that state (§4.1 case 3).
  const blocked = new Map<string, Map<string, number>>();
  const stuck = new Set([...exposed.keys()].filter((h) => !renews(h)));
  for (let changed = true; changed; ) {
    changed = false;
    for (const h of chain.hosts()) {
      const id = h.info.id;
      if (h.status.type !== "active") continue;
      for (const source of stuck) {
        // As the chain admits now: an edge a renewal pause holds lets nothing in.
        if (source === id || !admits(chain, id, source)) continue;
        const by = blocked.get(id) ?? new Map<string, number>();
        if (by.has(source)) continue;
        by.set(source, exposed.get(source)?.since ?? chain.seq);
        blocked.set(id, by);
        stuck.add(id);
        changed = true;
      }
    }
  }
  const out = new Map<string, Exposure>();
  for (const h of chain.hosts()) {
    const id = h.info.id;
    if (!chain.machine(id)) continue;
    const x = exposed.get(id);
    const by = blocked.get(id) ?? new Map<string, number>();
    if (!x && by.size === 0) continue;
    const lostBy = x?.lost ?? new Map<string, number>();
    const via = x?.via ?? new Map<string, number>();
    out.set(id, { sinceSeq: Math.min(...lostBy.values(), ...via.values(), ...by.values()), lost: lostBy, via, blockedBy: by });
  }
  return out;
}

/**
 * The hosts that would renew their identity key after device `device` stops
 * controlling the hosts `from` (all it may control now when not given) — a
 * removal, a demotion, a narrowed list — for the confirmation to say so
 * (§4.4): those hosts, every host that lets one of them (or one already
 * exposed) control it, and so on. Hosts already exposed are not counted.
 */
export function hostsExposedBy(chain: ChainState, device: string, from?: readonly string[]): string[] {
  const already = hostsToRotate(chain);
  const hosts = chain.hosts().filter((h) => h.status.type === "active");
  const out = new Set<string>(from ?? hosts.filter((h) => h.info.id !== device && admits(chain, h.info.id, device)).map((h) => h.info.id));
  // A host with no host key yet is no host to renew (it is the device itself, for a host removed).
  out.delete(device);
  for (let changed = true; changed; ) {
    changed = false;
    for (const h of hosts) {
      const id = h.info.id;
      if (out.has(id) || id === device) continue;
      if ([...out].some((e) => e !== id && admits(chain, id, e))) {
        out.add(id);
        changed = true;
      }
    }
  }
  return [...out].filter((id) => !already.has(id)).sort();
}

/** `CloudAccount.writePause`: nothing left to write on the newest head. */
export class NothingToWrite extends Error {}

/** A renewal this page verified a dispute of: its host, and the disputed `rotateHostKey`'s seq (§3.8). */
export interface KeptDispute {
  host: string;
  seq: number;
  /**
   * The statement's signature (hex), by the key the renewal at `seq` retired
   * (R6 review): kept with it, so that whatever reads it back — a reload, another
   * tab — verifies it again on its own chain (`verifyRotateDispute`) before it
   * counts for anything but a wait.
   */
  sig: string;
}

/** Kept disputes as stored (anything else in the list is dropped). */
export function parseDisputes(list: unknown): KeptDispute[] {
  if (!Array.isArray(list)) return [];
  return mergeDisputes(
    [],
    list.flatMap((d) => {
      const x = (d && typeof d === "object" ? d : {}) as { host?: unknown; seq?: unknown; sig?: unknown };
      return typeof x.host === "string" && Number.isSafeInteger(x.seq) && (x.seq as number) > 0 && typeof x.sig === "string" && /^[0-9a-f]{128}$/.test(x.sig)
        ? [{ host: x.host, seq: x.seq as number, sig: x.sig }]
        : [];
    }),
  );
}

/** A dispute's identity: its host, its renewal and its signature (one each; a forged copy is another, dropped once checked). */
export const disputeId = (d: KeptDispute) => `${d.seq}@${d.host}@${d.sig}`;

/** Whether kept dispute `d` is signed by the key the renewal at `d.seq` on `chain` retired (`false` for anything that does not verify). */
export async function disputeChecksOut(chain: ChainState, d: KeptDispute): Promise<boolean> {
  try {
    return await chain.verifyRotateDispute(d.host, d.seq, fromHex(d.sig));
  } catch {
    return false;
  }
}

/** Those of `found` not in `kept`, one each. */
export function disputesNotIn(kept: readonly KeptDispute[], found: readonly KeptDispute[]): KeptDispute[] {
  const seen = new Set(kept.map(disputeId));
  const out: KeptDispute[] = [];
  for (const d of found) {
    const id = disputeId(d);
    if (seen.has(id)) continue;
    seen.add(id);
    out.push({ host: d.host, seq: d.seq, sig: d.sig });
  }
  return out;
}

/** `a` then what `b` adds, one each. */
export function mergeDisputes(a: readonly KeptDispute[], b: readonly KeptDispute[]): KeptDispute[] {
  return [...disputesNotIn([], a), ...disputesNotIn(a, b)];
}

/**
 * At most {@link MAX_KEPT_DISPUTES} resolved ones — the oldest resolved go
 * first — and every one that still isolates its host on `chain` (or that
 * `chain` cannot judge yet), whatever their number: those are never evicted.
 */
export function capDisputes(chain: Pick<ChainState, "seq" | "entryAt" | "machine"> | null, list: readonly KeptDispute[], max = MAX_KEPT_DISPUTES): KeptDispute[] {
  const open = (d: KeptDispute) => !chain || disputeIsolates(chain, d);
  const resolved = list.filter((d) => !open(d));
  const room = Math.max(0, max - (list.length - resolved.length));
  const drop = new Set(resolved.slice(0, Math.max(0, resolved.length - room)));
  if (list.length - resolved.length > max) console.warn(`cloud account: ${list.length - resolved.length} unresolved disputes kept — more than ${max}; none dropped`);
  return list.filter((d) => !drop.has(d));
}

/** Whether `chain` has a renewal of `host` confirmed on the device (`assurance = 1`) after entry `seq`. */
export function assuredRenewalAfter(chain: Pick<ChainState, "seq" | "entryAt">, host: string, seq: number): boolean {
  for (let s = chain.seq; s > seq; s--) {
    const a = chain.entryAt(s)?.draft.action;
    if (a?.type === "rotateHostKey" && a.rotation.host === host && a.rotation.assurance === ASSURANCE_ON_DEVICE) return true;
  }
  return false;
}

/**
 * Whether a verified dispute of the renewal at `d.seq` isolates `d.host` on
 * `chain` (§3.8, R6 review): that renewal was automatic, no renewal confirmed on
 * the device came after it, and the host is still one. A chain behind the one
 * it was verified on (no entry at `d.seq` yet) keeps it isolated: it cannot
 * know better.
 */
export function disputeIsolates(chain: Pick<ChainState, "seq" | "entryAt" | "machine">, d: KeptDispute): boolean {
  if (!chain.machine(d.host)) return false;
  if (d.seq > chain.seq) return true;
  const a = chain.entryAt(d.seq)?.draft.action;
  if (a?.type !== "rotateHostKey" || a.rotation.host !== d.host || a.rotation.assurance === ASSURANCE_ON_DEVICE) return false;
  return !assuredRenewalAfter(chain, d.host, d.seq);
}

/** What the renewal runner tells the account (`CloudAccount.noteRenewals`). */
export interface RenewalNote {
  /** The hosts that renew their identity key here (chain v3, the hub takes renewals, their agents declared `rotate/1`). */
  renews: ReadonlySet<string>;
  /** The hosts the hub has waiting to renew (`pending`, `attempt`, `staged`): their device role is suspended (§4.3). */
  pending: ReadonlySet<string>;
  /** The hosts isolated after their retired key showed up again (§3.8): by evidence this page checked only. */
  isolated: ReadonlySet<string>;
  /** The hosts the hub flags isolated without evidence checked here (R6 review): withheld keys, not isolated. */
  flagged?: ReadonlySet<string>;
}

/** A host's answer to `POST /rh/rotate` (camelCase, hex), parsed. */
export interface RotateAnswer {
  signKey: Bytes;
  dhKey: Bytes;
  oldSig: Bytes;
  newSig: Bytes;
  mac: Bytes;
  stagedSeq: number;
  stagedHead: Bytes;
  /** When its window closes by the hub's clock, if the host was told (`null`: ask the hub). */
  readyMs: number | null;
}

/** `answer` in the shape `POST /rh/rotate` answers it ({@link parseRotateAnswer} reads it back): what a renewal under way keeps of it. */
export function rotateAnswerJson(answer: RotateAnswer): Record<string, unknown> {
  return {
    signKey: hex(answer.signKey),
    dhKey: hex(answer.dhKey),
    oldSig: hex(answer.oldSig),
    newSig: hex(answer.newSig),
    mac: hex(answer.mac),
    stagedSeq: answer.stagedSeq,
    stagedHead: hex(answer.stagedHead),
    readyMs: answer.readyMs,
  };
}

/** `POST /rh/rotate`'s 200, or a {@link RenewalError} `badAnswer` if it is not of that shape. */
export function parseRotateAnswer(raw: unknown): RotateAnswer {
  const r = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const field = (name: string, bytes: number): Bytes => {
    const v = r[name];
    if (typeof v !== "string" || v.length !== bytes * 2 || !/^[0-9a-fA-F]+$/.test(v)) throw new RenewalError("badAnswer", `the device's answer has no valid ${name}`, name);
    return fromHex(v.toLowerCase());
  };
  const stagedSeq = r.stagedSeq;
  if (typeof stagedSeq !== "number" || !Number.isSafeInteger(stagedSeq) || stagedSeq < 0) throw new RenewalError("badAnswer", "the device's answer has no valid stagedSeq", "stagedSeq");
  const ready = r.readyMs;
  return {
    signKey: field("signKey", 32),
    dhKey: field("dhKey", 32),
    oldSig: field("oldSig", 64),
    newSig: field("newSig", 64),
    mac: field("mac", 32),
    stagedSeq,
    stagedHead: field("stagedHead", 32),
    readyMs: typeof ready === "number" && Number.isFinite(ready) && ready > 0 ? ready : null,
  };
}

/**
 * The renewal `answer` proves for `host` (whose keys on `chain` are `old`),
 * once it checks out (§3.2 "D 再检查"): it is about exactly the view asked
 * about, its new keys are valid and ones the chain never named — no host's
 * identity lands on a key some device held — and the host's current key and
 * its new one both signed the proof. Else a {@link RenewalError} `badAnswer`
 * (the MAC is checked by the caller, which holds the ephemeral key).
 */
export async function checkRotateAnswer(
  chain: ChainState,
  host: string,
  old: { signKey: Uint8Array; dhKey: Uint8Array },
  answer: RotateAnswer,
  view: { seq: number; head: Uint8Array },
  nonce: Uint8Array,
  assurance: Assurance,
): Promise<HostRotation> {
  if (answer.stagedSeq !== view.seq || !equal(answer.stagedHead, view.head)) throw new RenewalError("badAnswer", `${host} answered about another chain view`, "view");
  if (!(await isValidEd25519Key(answer.signKey)) || !(await isValidX25519(answer.dhKey))) throw new RenewalError("badAnswer", `${host}'s new key is not a valid one`, "key");
  if (chain.everNamed(answer.signKey) || chain.everNamed(answer.dhKey)) throw new RenewalError("badAnswer", `${host}'s new key is one the chain named before`, "reused");
  const rotation: HostRotation = {
    host,
    signKey: answer.signKey,
    dhKey: answer.dhKey,
    stagedSeq: answer.stagedSeq,
    stagedHead: answer.stagedHead,
    nonce: copy(nonce),
    assurance,
    oldSig: answer.oldSig,
    newSig: answer.newSig,
  };
  if (!(await verifyHostRotation(chain.account, rotation, old.signKey, old.dhKey))) {
    throw new RenewalError("badAnswer", `${host}'s renewal is not signed by both its current and its new identity key`, "signature");
  }
  return rotation;
}

/** A hub refusal of a renewal step, as a {@link RenewalError} (anything else is thrown as it is). */
export function renewalRefusal(error: unknown): Error {
  if (error instanceof RenewalError || !(error instanceof HubApiError)) return error instanceof Error ? error : new Error(String(error));
  if (error.status === 409 && error.code === "rotation_in_progress") return new RenewalError("inProgress", "another device is renewing it now", error.code);
  if (error.status === 409 && error.code === "rotation_not_ready") {
    switch (error.reason) {
      case "agent_too_old":
        return new RenewalError("tooOld", "its Droppen does not renew its identity key", error.reason);
      case "needs_assurance":
        return new RenewalError("needsAssurance", "only a renewal confirmed on the device goes now", error.reason);
      case "waiting":
        return new RenewalError("waiting", "waiting for Droppen to be updated on other devices that run AI", error.reason, error.waitingHosts ?? []);
      case "not_staged":
      case "mismatch":
        return new RenewalError("voided", "the hub has no matching staged renewal", error.reason);
      case "policy_unset":
        return new RenewalError("policyUnset", "choose how identity key renewals are confirmed first", error.reason);
      case "disabled":
      case undefined:
        return new RenewalError("disabled", "the hub takes no identity key renewal now", "disabled");
      case "v2":
        // A v2 account renews nothing: re-enrolling is the way (§5.6).
        return new RenewalError("refused", "this account is on chain v2: re-enroll the device instead", "v2");
      default:
        // `conflict`, `window` after the page's tries, and anything newer: refused, and tried again later.
        return new RenewalError("refused", `the hub did not take the renewal (${error.reason})`, error.reason);
    }
  }
  // A hub from before renewals has no such route.
  if (error.status === 404) return new RenewalError("disabled", "the hub takes no identity key renewal", "not_found");
  if (error.status === 429) return new RenewalError("rate", "too many renewals of this device lately: try again later", error.code);
  if (error.status === 409) return new RenewalError("refused", error.message, error.code);
  return new RenewalError("failed", error.message, error.code);
}

/** What a host's own record says of a device it should be re-enrolled for ({@link reEnrollEvidence}). */
export type ReEnrollEvidence =
  /** The record names it: it opened a channel to the host (first, by the host's clock). */
  | { kind: "connected"; firstMs: number }
  /** The record covers every moment it could have, and does not name it: no channel since `sinceMs` (the host's clock). */
  | { kind: "notSeen"; sinceMs: number }
  /** No record, or one that does not cover the time it could control the host (an older agent, a record begun later). */
  | { kind: "unknown" };

/** One device a host should be re-enrolled for, as 「我的设备」 says it ({@link CloudAccount.reEnrollWarnings}). */
export interface ReEnrollLine {
  device: string;
  /** The chain seq it last lost control of the host at. */
  lostSeq: number;
  evidence: ReEnrollEvidence;
  /** A person said here it never connected (and no record says it did). */
  dismissed: boolean;
}

/**
 * What host `host`'s record (`record`: its status's `controllers`) says of
 * `device`, which lost control of it at seq `lostSeq`; `keptMs`: when a
 * record seen before said it first connected. It names the device: it
 * connected. It does not, and covers every moment the device could have —
 * written after the loss (the agent refuses the device from then on) and
 * complete since before the device or the host joined the account, so no
 * policy is assumed — then no channel is on record. Anything else is not
 * known. Evidence only: whoever did connect could have edited the record.
 */
export function reEnrollEvidence(chain: ChainState, host: string, device: string, lostSeq: number, record?: ControllersRecord, keptMs?: number): ReEnrollEvidence {
  const named = record?.devices.find((d) => d.id === device)?.firstMs;
  const firstMs = named === undefined ? keptMs : keptMs === undefined ? named : Math.min(named, keptMs);
  if (firstMs !== undefined) return { kind: "connected", firstMs };
  const h = chain.deviceRecord(host);
  const d = chain.deviceRecord(device);
  if (!record || !h || !d) return { kind: "unknown" };
  const couldFrom = Math.max(h.addedSeq, d.addedSeq);
  return record.seq >= lostSeq && record.from < couldFrom ? { kind: "notSeen", sinceMs: record.fromMs } : { kind: "unknown" };
}

/** `{host: {device: number}}` as stored, keeping only what has that shape. */
export function seqMaps(text: string | null): Record<string, Record<string, number>> {
  const out: Record<string, Record<string, number>> = {};
  if (!text) return out;
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return out;
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) return out;
  for (const [host, devices] of Object.entries(value as Record<string, unknown>)) {
    if (!devices || typeof devices !== "object" || Array.isArray(devices)) continue;
    const kept: Record<string, number> = {};
    for (const [device, n] of Object.entries(devices as Record<string, unknown>)) if (Number.isSafeInteger(n) && (n as number) >= 0) kept[device] = n as number;
    if (Object.keys(kept).length) out[host] = kept;
  }
  return out;
}

/** What a `setAccept` changed: who it let in, who it shut out — and, for a narrowing, the host's new key epoch. */
export interface AcceptChange {
  narrowed: boolean;
  /** Current devices that are not admins it lets control the host now and did not before. */
  admitted: string[];
  /** Current devices that are not admins it no longer lets control the host. */
  shutOut: string[];
  mkEpoch?: number;
  result?: DeliveryResult;
  /** The devices still let in that the new key was sealed to. */
  hostKeysTo?: string[];
  /** A new user key the narrowing carried (the newest was stale: `userKeyStale`). */
  userKey?: FreshUserKey;
}

/** Whether `from` lets device `id` control a host (besides admins, which every host accepts). */
export function acceptNames(from: Accept, id: string): boolean {
  return from.type === "all" || (from.type === "devices" && from.ids.includes(id));
}

/**
 * Whether a mail to `to` failed only because it is a host whose device role
 * has not signed in at the hub yet (S1-4): the hub takes mail for a host's
 * device role once it did, and answers 400 before — an agent from before
 * S1-4 never does, one with the aggregator off has not. Not a failure: the
 * host asks for what it lacks (purpose 4) once it signs in.
 */
export function noDeviceRoleYet(to: Pick<DeviceRecord, "host">, error: unknown): boolean {
  // Exactly the hub's refusal for it (docs/hub-api/http.md, `POST /v1/mailbox`), not any 400 (security review of S1-4, N2).
  return to.host && error instanceof HubApiError && error.status === 400 && error.code === "bad_request" && error.message.includes(NO_DEVICE_ROLE_HINT);
}

/** What the hub's refusal of mail for a host's device role that never signed in says (docs/hub-api/http.md, `POST /v1/mailbox`). */
export const NO_DEVICE_ROLE_HINT = "gets it once it signs in as a device";

/** Whether host `host` accepts control from device `id` on `chain` (`mayControl`). */
export function admits(chain: ChainState, host: string, id: string): boolean {
  try {
    chain.mayControl(id, host);
    return true;
  } catch {
    return false;
  }
}

/** Who a new accept list `to` for `host` lets in and shuts out, among the current devices that are not admins (as the chain judges a narrowing). */
export function acceptChange(chain: ChainState, host: string, to: Accept): { admitted: string[]; shutOut: string[] } {
  const was = chain.acceptPolicy(host);
  const admitted: string[] = [];
  const shutOut: string[] = [];
  for (const d of chain.allDevices()) {
    if (d.status.type !== "active" || d.admin || d.info.id === host) continue;
    const before = acceptNames(was, d.info.id);
    const after = acceptNames(to, d.info.id);
    if (after && !before) admitted.push(d.info.id);
    if (before && !after) shutOut.push(d.info.id);
  }
  return { admitted, shutOut };
}

/** A courier for a rotation that hands no agent a key (the upgrade's): never called. */
export const NO_COURIER: MachineKeyCourier = {
  waitOnline: () => Promise.reject(new Error("no machine key to hand over")),
  postInChannel: () => Promise.reject(new Error("no machine key to hand over")),
  mail: () => Promise.reject(new Error("no machine key to hand over")),
};

/**
 * Apply `entries` to a copy of `chain` with the chain's own verifier (history
 * clock: the delay of a recovered device's self-activation is the caller's
 * to check) — or, with no chain yet (`null`: a genesis and what comes with
 * it), verify them as a chain of their own: a batch it refuses — or one that
 * ends with a host still owed its machine key epoch, which the hub refuses
 * too — is an `invalidEntry`, by the rule that refused it.
 */
export async function verifyBatch(chain: ChainState | null, entries: Entry[]): Promise<void> {
  const refused = (error: unknown): never => {
    if (!(error instanceof ChainError)) throw error;
    throw new AccountError("invalidEntry", `the chain would not take this change: ${error.message}`, undefined, error.detail.reason ?? error.kind);
  };
  const clock = ChainClock.history();
  let probe: ChainState;
  if (chain) {
    probe = chain.clone();
    for (const entry of entries) {
      try {
        await probe.apply(entry, clock);
      } catch (error) {
        refused(error);
      }
    }
  } else {
    try {
      probe = await ChainState.verify(entries, undefined, clock);
    } catch (error) {
      return refused(error);
    }
  }
  if (probe.hostKeyDue !== null) {
    throw new AccountError("invalidEntry", `the chain would not take this change: ${RULES.hostKeyNext}`, undefined, RULES.hostKeyNext);
  }
}

export function compareBytes(a: Uint8Array, b: Uint8Array): number {
  for (let i = 0; i < Math.min(a.length, b.length); i++) if (a[i] !== b[i]) return a[i]! - b[i]!;
  return a.length - b.length;
}

