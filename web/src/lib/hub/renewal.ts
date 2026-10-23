/*
 * Renewing the identity key of the account's devices that run AI, as an
 * admin page does it on its own (docs/host-identity-rotation.md §4): instead
 * of asking the person to remove and add again a computer a removed device
 * could control, every admin page renews it.
 *
 * - **What is due**: the chain says (`hostsToRotate`: the losses, and every
 *   host that let an exposed one control it); a host whose agent declared
 *   `rotate/1`, on a v3 account whose hub takes renewals, renews — any other
 *   keeps the re-enroll warning (`CloudAccount.reEnrollWarnings`).
 * - **Marked first**: each exposed host is registered `pending` at the hub
 *   (`PUT /v1/rotations/{host}` without a nonce) as soon as this page sees it:
 *   the hub suspends its device role meanwhile (§4.3) and every admin page
 *   sees it waiting.
 * - **One at a time**: the online ones now, an offline one when it comes back
 *   (presence); the hub's registration serialises pages (409
 *   `rotation_in_progress`: another page is on it). Controllers first, as
 *   correctness asks (§4.1 「次序先于收窄」): a host renews only after every
 *   exposed host it lets control it has, so that one's old key is dead by
 *   then. Where no order works — a cycle of exposed hosts letting each other
 *   in — the cycle's lists are narrowed against each other first, in one
 *   batch that a `RenewalPause` start opens (§3.4 rule 10), and once the whole
 *   cycle renewed the lists still the pause's own are restored in one batch
 *   that completes it. Nothing else is ever restored automatically. A host an
 *   exposed one that cannot renew controls is not renewed at all (`blocked`:
 *   narrow its list, or add that one again).
 * - **Failures** (§4.2): an old agent is not tried again (re-enrolling is the
 *   way); a channel that fails is tried again after 1, 5 and 30 minutes, then
 *   the person is asked; another holder of the old key (`not_acked`,
 *   `conflict`) means only a renewal confirmed on the device from then on.
 * - **Disputes** (§3.8): a retired key that signed in again, as the hub
 *   records it, is checked here against the chain (`keysAt`) — the hub
 *   cannot forge it, only withhold it. Retired by an automatic renewal, it
 *   isolates the host: no channel, no key, renewal only with the code its
 *   screen shows, until such a renewal (or its removal).
 *
 * The page reads it all through {@link RenewalRunner.status}; the account is
 * told what the chain does not say (`noteRenewals`).
 */
import { type Action, ASSURANCE_AUTO, ASSURANCE_ON_DEVICE, type Assurance, type ChainState, equal, hex, type LoginRole, MAX_PAUSE_ENTRY_LEN, type OpenPause, PAUSE_COMPLETED, PAUSE_START, type PauseRemoval, Serial, verifyHubLogin } from "@/lib/e2e";
import {
  assuredRenewalAfter,
  type CloudAccount,
  disputeIsolates,
  type Exposure,
  exposedControllers,
  edgeHeld,
  compareUtf8Ids,
  groupNarrowing,
  splitPauseRemovals,
  pauseReturns,
  hostsToRotate,
  type PauseEnding,
  type KeptDispute,
  RenewalError,
  renewalHelps,
  type RenewalReport,
  type RenewOptions,
} from "./account";
import { type AccountView, type HostDispute, type HostRotationView, HubApiError, disputePush, rotationPush, type RotationsView } from "./api";
import type { RenewalCourier } from "./deliver";
import type { HubLink } from "./link";

/** How long after a failed automatic attempt to try again: 1, 5 and 30 minutes; after the last, the person is asked (§4.2). */
export const RENEW_RETRY_MS = [60_000, 5 * 60_000, 30 * 60_000] as const;
/** How long a completed renewal is shown on its row (§4.4). */
export const RENEWED_SHOWN_MS = 24 * 60 * 60 * 1000;
/**
 * How long past its window a staging nobody wrote is taken for abandoned (the
 * page that made it was closed, or its withdrawal never reached the hub): it
 * is withdrawn and the renewal started again (§4.2 「下一个 admin 页面重来」).
 * The hub would refuse every new attempt meanwhile (409 `rotation_in_progress`).
 */
export const ABANDONED_MS = 10 * 60 * 1000;
/** How long clone evidence (`not_acked`, `conflict`) keeps a host on renewals confirmed on the device (§3.3). */
export const EVIDENCE_MS = 30 * 24 * 60 * 60 * 1000;

/**
 * When (by the hub's clock) `row` is an attempt nobody will finish — an
 * `attempt` past its expiry, or a staging {@link ABANDONED_MS} past its window
 * ({@link RenewalRunner.abandoned}) — as it stands; `null`: never.
 */
export function abandonsAt(row: HostRotationView): number | null {
  if (row.state === "attempt") return row.attemptExpiresMs;
  if (row.state !== "staged") return null;
  const ready = row.readyMs ?? row.stagedMs;
  return ready === null ? null : ready + ABANDONED_MS;
}

/** What the runner needs of the page's hub link. */
export type RenewalLink = Pick<HubLink, "presence" | "onPresence" | "onControl" | "onState">;

/** What the runner needs of `/v1/me` (`MeStore`): which hosts declared `rotate/1`. */
export interface RenewalMe {
  readonly view: AccountView | null;
  onChange(fn: () => void): () => void;
}

/** Where a host's identity key renewal stands, as its row in 「我的设备」 says it (§4.4). */
export type RenewalStatus =
  /** Nothing to renew (or it cannot renew here: the re-enroll warning speaks for it). */
  | { kind: "none" }
  /** Being renewed — by this page, or another one (`elsewhere`). About two minutes. */
  | { kind: "renewing"; elsewhere: boolean }
  /** Offline: renewed once it is back. */
  | { kind: "offline" }
  /**
   * Waiting for these exposed hosts it lets control it to renew first (§4.1),
   * and, for those that will not on their own now, why (`stuck`).
   */
  | { kind: "queued"; after: string[]; stuck: { host: string; why: QueuedWhy }[] }
  /** The last try failed; the next one is due at `atMs`. `cause`: the failure itself, to be said in the reader's language. */
  | { kind: "retrying"; atMs: number; error: string; cause?: unknown }
  /** Tried and failed as often as it is tried on its own (or in a way no try of its own mends): the person is asked. */
  | { kind: "stuck"; error: string; cause?: unknown }
  /**
   * It lets these exposed devices that cannot renew their identity key control
   * it (§4.1 case 3): renewing it would not help — narrow its list, or add
   * those again. Not renewed on its own. (Blocked only through a host that can
   * renew, itself blocked, it is `queued` after that one instead.)
   */
  | { kind: "blocked"; by: string[] }
  /** Its Droppen does not renew its identity key: update it (or re-enroll it, under "more"). */
  | { kind: "tooOld" }
  /** The hub waits for these other hosts to declare `rotate/1`. */
  | { kind: "waiting"; hosts: string[] }
  /** Only a renewal confirmed on the device goes: the account's policy on the chain, this device's own guard, or clone evidence. */
  | { kind: "needsCode"; why: "policy" | "device" | "evidence" }
  /** Its retired key signed in again: no channel, no key, until it is renewed with the code its screen shows. */
  | { kind: "isolated" }
  /** The account has not chosen how renewals are confirmed yet: nothing renews on its own until an admin does (§5.6). */
  | { kind: "policyUnset" }
  /** Renewed at `atMs` (shown for a day). */
  | { kind: "renewed"; atMs: number };

/**
 * Why a host another waits for does not renew on its own now: it needs the
 * code its screen shows, it is isolated, it is blocked by one that cannot
 * renew, it is offline, its tries ran out, its Droppen is too old, or the hub
 * waits for other hosts.
 */
export type QueuedWhy = "needsCode" | "isolated" | "blocked" | "offline" | "stuck" | "tooOld" | "waiting";

/** What the runner keeps of its own tries of one host. */
interface HostRun {
  attempts: number;
  nextMs: number | null;
  error: string | null;
  /** The failure {@link error} is the message of. */
  cause?: unknown;
  outcome: "tooOld" | "stuck" | "waiting" | "needsCode" | null;
  waiting: string[];
  /** For `waiting`: the `/v1/me` read when the hub said so — only a newer one may say the hosts it waits for updated. */
  waitingSeen?: AccountView | null;
  doneMs: number | null;
}

export interface RenewalRunnerOptions {
  account: CloudAccount;
  link: RenewalLink;
  /** The courier renewals go through (`shellCourier`). */
  courier: () => RenewalCourier;
  me?: RenewalMe | null;
  now?: () => number;
  /** {@link RENEW_RETRY_MS}, for tests. */
  retryMs?: readonly number[];
  /** Passed on to every `renewIdentity` (timers, for tests). */
  renewOptions?: Partial<Omit<RenewOptions, "assurance">>;
}

/**
 * A dispute statement this device can check (§3.8 「证据：退役钥签的争议声明」):
 * signed by the sign key the renewal at `seq` of `host` retired, over that
 * entry's hash and the key it put in — checked on this page's own chain
 * (`verifyRotateDispute`). A hub cannot forge one, only withhold it. `null`
 * when it does not check out, or is no statement (`retired_login`,
 * `not_acked`, `conflict`).
 */
export async function checkStatement(chain: ChainState, d: HostDispute): Promise<KeptDispute | null> {
  if (d.kind !== "disputed" || d.seq === null || !d.sig) return null;
  // Kept with its signature (R6 review): what reads it back verifies it again.
  return (await chain.verifyRotateDispute(d.host, d.seq, d.sig)) ? { host: d.host, seq: d.seq, sig: hex(d.sig) } : null;
}

/**
 * A sign-in with a key `host` renewed away from (`retired_login`, §3.8): its
 * signed `HubLogin` checked against the key the renewal at `retiredSeq`
 * retired (`keysAt`). Information only — it never isolates (two clocks cannot
 * order it against the renewal): `null` when it does not check out. Its `hub`
 * is checked as signed, not against a list: the hub keeps the login exactly as
 * it came, bound to the host it dialled — its origin (`wss://droppen.app`), or
 * for a browser device's roles its app host (`wss://my.droppen.app`; docs/hub-api
 * README "Hosts") — and a retired key's signature on either is the same evidence.
 */
export async function checkRetiredLogin(chain: ChainState, d: HostDispute): Promise<{ host: string; retiredSeq: number; timeMs: number } | null> {
  if (d.kind !== "retired_login" || !d.login || !d.sig || !d.signKey || d.retiredSeq === null) return null;
  if (d.login.account !== chain.account || d.login.id !== d.host) return null;
  const signKey = d.signKey;
  const retired = chain.retiredKeys(d.host).find((r) => r.untilSeq === d.retiredSeq && equal(r.signKey, signKey));
  const was = chain.keysAt(d.host, d.retiredSeq - 1);
  if (!retired || !was || !equal(was.signKey, signKey)) return null;
  const roles: LoginRole[] = d.role === "agent" ? ["agent"] : ["device", "deviceSession"];
  for (const role of roles) {
    const login = { role, account: d.login.account, id: d.login.id, hub: d.login.hub, nonce: d.login.nonce, timeMs: d.login.timeMs };
    if (await verifyHubLogin(login, signKey, d.sig)) return { host: d.host, retiredSeq: d.retiredSeq, timeMs: d.login.timeMs };
  }
  return null;
}

/** Whether `chain` has a renewal of `host` confirmed on the device (`assurance = 1`) after entry `seq`. */
export function assuredAfter(chain: ChainState, host: string, seq: number): boolean {
  return assuredRenewalAfter(chain, host, seq);
}

/**
 * The current hosts isolated (§3.8, §7 decision 2): only by a dispute
 * statement checked here, of an automatic renewal with no renewal confirmed
 * on the device after it (R6 review: `disputeIsolates`). The hub's `isolated`
 * flag alone is a note, never this (R6 review: it does not lead the person to
 * remove the device): {@link flaggedHosts}.
 */
export function isolatedHosts(chain: ChainState, statements: readonly KeptDispute[]): Set<string> {
  const out = new Set<string>();
  for (const d of statements) if (disputeIsolates(chain, d)) out.add(d.host);
  return out;
}

/**
 * The current hosts the hub flags isolated without evidence checked here (an
 * unverified note, R6 review): sealed no keys and renewed only confirmed on the
 * device — the stricter side — but not isolated on this page. Not once a
 * renewal confirmed on the device is its latest.
 */
export function flaggedHosts(chain: ChainState, hubIsolated: Iterable<string>, isolated: ReadonlySet<string>): Set<string> {
  const out = new Set<string>();
  for (const host of hubIsolated) {
    const record = chain.machine(host);
    if (!record || isolated.has(host)) continue;
    const latest = record.rotatedSeq === null ? undefined : chain.entryAt(record.rotatedSeq)?.draft.action;
    if (latest?.type === "rotateHostKey" && latest.rotation.assurance === ASSURANCE_ON_DEVICE) continue;
    out.add(host);
  }
  return out;
}

/**
 * The hosts to renew in turn, from those `due` (exposed and free to go now):
 * first every one that lets no exposed, renewable host control it; when each
 * waits for another (they let each other control them), the longest exposed
 * first. Only an order among hosts already free to go: that a host never
 * renews before an exposed host it lets control it is the runner's `due`
 * rule (`exposedControllers`, §4.1).
 */
export function renewalOrder(due: readonly string[], exposed: ReadonlyMap<string, Exposure>, renewable: (host: string) => boolean): string[] {
  const blockers = (host: string) => [...(exposed.get(host)?.via.keys() ?? [])].filter((e) => e !== host && exposed.has(e) && renewable(e));
  const free = due.filter((h) => blockers(h).length === 0);
  const since = (h: string) => exposed.get(h)?.sinceSeq ?? 0;
  const byAge = (a: string, b: string) => since(a) - since(b) || (a < b ? -1 : a > b ? 1 : 0);
  if (free.length) return [...free].sort(byAge);
  // Every one waits for another: they control each other. Nothing but a renewal of each breaks it.
  return [...due].sort(byAge);
}

/**
 * Every admin page's renewal runner (§4.2), and what every page shows of
 * renewals (any device may read them: `GET /v1/rotations`).
 */
export class RenewalRunner {
  /** `GET /v1/rotations` as last read (`null`: not yet, or a hub from before renewals). */
  view: RotationsView | null = null;
  /** When {@link view} was read, by this page's clock (for {@link hubNow}). */
  private readAtMs = 0;
  /** Bumped on every change (for `useSyncExternalStore`). */
  version = 0;
  private readonly runs = new Map<string, HostRun>();
  private readonly listeners = new Set<() => void>();
  private readonly serial = new Serial();
  private readonly offs: (() => void)[] = [];
  /** Exposures registered `pending` at the hub from this page (`host@sinceSeq`). */
  private readonly marked = new Set<string>();
  /** Rows this page withdrew (abandoned ones, pending ones nothing exposes any more), by `host@requestedMs`: once each. */
  private readonly withdrawn = new Set<string>();
  /** This browser's attempts this page took up from a page that went away (`resume`), by `host@nonceHash`: once each. */
  private readonly resumed = new Set<string>();
  /** When {@link withdrawUnexposed} last read the hub. */
  private lastUnexposedCheck = Number.NEGATIVE_INFINITY;
  /** When this browser's records of attempts the hub no longer has were last looked for (`forgetLeftRenewals`). */
  private lastLeftCheck = Number.NEGATIVE_INFINITY;
  /** Disputes checked against the chain, by host. */
  private checked: KeptDispute[] = [];
  /** Sign-ins with a retired key checked here, by host: the latest (information only, never isolation). */
  private retiredLogins = new Map<string, number>();
  /** The host this page is renewing now, and how far it is. */
  private running: string | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private stopped = false;
  private ticking = false;
  private again = false;

  constructor(private readonly opts: RenewalRunnerOptions) {}

  private now(): number {
    return (this.opts.now ?? Date.now)();
  }

  /** The hub's clock now, as `GET /v1/rotations` last told it (`now_ms`, else its Date header); the page's clock only before any (R6 review). */
  hubNow(): number {
    const view = this.view;
    if (!view || view.nowMs === null) return this.now();
    return view.nowMs + (this.now() - this.readAtMs);
  }

  onChange(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private emit(): void {
    this.version++;
    for (const fn of [...this.listeners]) {
      try {
        fn();
      } catch {
        // a listener's problem
      }
    }
  }

  /** Read the hub's renewals and follow them: its pushes, the link coming back, presence, the chain, `/v1/me`. */
  start(): void {
    const { link, account, me } = this.opts;
    this.offs.push(
      link.onControl((message) => {
        // Any word of a renewal or a dispute (`state: "none"` after a withdrawal included): read them again.
        if (rotationPush(message) || disputePush(message) || message.t === "rotation" || message.t === "host_dispute") void this.refresh();
      }),
      link.onState((state) => {
        if (state === "open") void this.refresh();
      }),
      link.onPresence(() => this.poke()),
      account.onChange(() => this.poke()),
    );
    if (me) this.offs.push(me.onChange(() => this.poke()));
    void this.refresh();
  }

  stop(): void {
    this.stopped = true;
    for (const off of this.offs.splice(0)) off();
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  /** Resolves once what is queued now is done. */
  async idle(): Promise<void> {
    for (let i = 0; i < 4; i++) await this.serial.run(async () => undefined);
  }

  /** Read `GET /v1/rotations` again, check its disputes, tell the account, and see what is due. */
  refresh(): Promise<void> {
    return this.serial.run(async () => {
      if (this.stopped) return;
      const { account } = this.opts;
      try {
        this.view = await account.api.rotations(account.session.token);
        this.readAtMs = this.now();
      } catch (error) {
        // A hub from before renewals (404), or not reachable now: nothing is renewed until it says.
        if (!(error instanceof HubApiError && error.status === 404)) console.warn("cloud: renewals:", error);
        if (error instanceof HubApiError && error.status === 404) this.view = null;
      }
      const disputes = this.view?.disputes ?? [];
      // A statement about a renewal this page's chain has not taken yet: the chain first.
      if (account.chain && disputes.some((d) => d.kind === "disputed" && d.seq !== null && d.seq > account.chain!.seq)) {
        await account.sync().catch((error: unknown) => console.warn("cloud: syncing for a dispute:", error));
      }
      const chain = account.chain;
      const checked: KeptDispute[] = [];
      const logins = new Map<string, number>();
      if (chain) {
        for (const d of disputes) {
          try {
            const statement = await checkStatement(chain, d);
            if (statement) checked.push(statement);
            const login = await checkRetiredLogin(chain, d);
            if (login) logins.set(login.host, Math.max(logins.get(login.host) ?? 0, login.timeMs));
          } catch (error) {
            console.warn("cloud: checking a dispute:", error);
          }
        }
      }
      this.checked = checked;
      this.retiredLogins = logins;
      // Kept in this browser, only ever added to: the hub omitting them later (or a 404) undoes nothing.
      account.keepDisputes(checked);
      this.note();
      this.emit();
    }).then(() => this.poke());
  }

  /** The account told what it cannot read off the chain ({@link CloudAccount.noteRenewals}). */
  private note(): void {
    const { account } = this.opts;
    const chain = account.chain;
    const enabled = this.view?.enabled === true;
    const renews = new Set<string>();
    if (enabled && chain?.isV3) for (const m of this.opts.me?.view?.machines ?? []) if (m.rotates && chain.machine(m.id)) renews.add(m.id);
    const pending = new Set((this.view?.hosts ?? []).filter((r) => r.state !== "committed").map((r) => r.host));
    // What this page kept (and other tabs did) as well as what the hub hands on now: the hub omitting one undoes nothing.
    const isolated = chain ? isolatedHosts(chain, [...account.keptDisputes(), ...this.checked]) : new Set<string>();
    const flagged = chain ? flaggedHosts(chain, (this.view?.hosts ?? []).filter((r) => r.isolated).map((r) => r.host), isolated) : new Set<string>();
    account.noteRenewals({ renews, pending, isolated, flagged });
  }

  /**
   * Pause, with ONE `RenewalPause` start (§3.4 rule 10, §4.1 覆盖层), the
   * control inside a cycle of exposed hosts that let each other control them,
   * before any of them renews — one cycle a time, read off the newest chain,
   * so a page that finds it paused already writes nothing. No list is
   * written: the overlay takes the edges out of what the chain admits.
   * `written`: the chain changed (the next look goes on); `failed`: not
   * written, and nothing of that cycle renews until it is; `none`: nothing
   * to pause.
   */
  private async narrowGroup(): Promise<"none" | "written" | "failed"> {
    const { account } = this.opts;
    const chain = account.chain;
    const renews = (h: string) => account.renews(h);
    if (!chain || !groupNarrowing(chain, account.hostsToRotate(), renews).length) return "none";
    const ops: Uint8Array[] = [];
    try {
      const wrote = await account.writePause((c) => {
        const cycle = groupNarrowing(c, hostsToRotate(c, renews), renews)[0];
        if (!cycle) return [];
        // M6: a cycle too large for one entry goes in several pauses, each within `MAX_PAUSE_ENTRY_LEN` — all in ONE batch,
        // so no host of it renews before every one of its edges is paused.
        const parts = splitPauseRemovals(cycle.removals, (r) => account.pauseEntryLen(c, r) <= MAX_PAUSE_ENTRY_LEN);
        while (ops.length < parts.length) ops.push(crypto.getRandomValues(new Uint8Array(16)));
        return parts.map((removals, i) => ({ type: "renewalPause", op: ops[i]!, phase: PAUSE_START, removals }) as Action);
      });
      return wrote ? "written" : "none";
    } catch (error) {
      console.warn("cloud: pausing control between devices renewing together:", error);
      return "failed";
    }
  }

  /**
   * End what may end of each open renewal pause (§4.1 「结束之前」), judged on
   * this page's chain alone — exposure as the chain has it now, whatever an
   * earlier renewal or the hub's word on what can renew says:
   *
   * - no edge it gives back has an end that is exposed, isolated or flagged:
   *   `completed`;
   * - some must stay held and others may come back: in ONE batch, a new
   *   start holding just those, then `completed` — the held ones never come
   *   back between the two;
   * - every one must stay held: nothing (the rows say why).
   *
   * Whether it wrote.
   */
  private async restoreGroups(): Promise<boolean> {
    const { account } = this.opts;
    const chain = account.chain;
    if (!chain) return false;
    for (const pause of chain.openPauses()) {
      // Started by a device that no longer manages the account: in force, and ended only by a person (M7) — never here.
      if (!chain.admin(pause.signer)) continue;
      if (!this.ending(chain, pause)) continue;
      try {
        // Judged again on the chain each batch is built on, and right before publication (H1).
        const back = await account.endPauseWith(pause.op, PAUSE_COMPLETED, (c, p) => (c.admin(p.signer) ? this.ending(c, p) : null));
        if (back) return true;
      } catch (error) {
        console.warn("cloud: ending a pause of control between devices renewed together:", error);
      }
    }
    return false;
  }

  /**
   * What ending `pause` on `chain` would write (see {@link restoreGroups}):
   * the edges to keep held in a new pause (empty: none), or `null` when
   * nothing of it may come back yet.
   */
  private ending(chain: ChainState, pause: OpenPause): { keep: PauseRemoval[] } | null {
    const { account } = this.opts;
    const { edges } = pauseReturns(chain, pause);
    // A host of it that will still renew on its own (it can, a renewal helps, nothing holds it): waited for — no churn of
    // pauses while the cycle renews. Only what may come back is judged on the chain alone, below.
    const pending = account.hostsToRotate();
    const ends = new Set(edges.flatMap((e) => [e.host, e.id]));
    const renewing = (x: string) => {
      const ex = pending.get(x);
      return !!ex && account.renews(x) && renewalHelps(ex) && !account.isolated(x) && !account.flagged(x) && !this.runs.get(x)?.outcome;
    };
    if ([...ends].some(renewing)) return null;
    const exposed = hostsToRotate(chain);
    const held = edges.filter((e) => edgeHeld(chain, e, { exposed, isolated: (id) => account.isolated(id), flagged: (id) => account.flagged(id) }));
    if (edges.length && held.length === edges.length) return null;
    // Switched off at the hub, which refuses a new start then: only an end that keeps nothing held.
    if (held.length && this.view?.enabled === false) return null;
    const byHost = new Map<string, string[]>();
    for (const e of held) byHost.set(e.host, [...(byHost.get(e.host) ?? []), e.id]);
    // Canonical: hosts, and each host's ids, ascending by bytes.
    return { keep: [...byHost].map(([host, ids]) => ({ host, ids: ids.sort(compareUtf8Ids) })).sort((x, y) => compareUtf8Ids(x.host, y.host)) };
  }

  /**
   * The devices whose control of each other is paused while their identity
   * keys are renewed (§4.1), when `host` is one of them: the hosts of the open
   * renewal pauses that take an edge to or from it. Empty otherwise.
   */
  pausedWith(host: string): string[] {
    const chain = this.opts.account.chain;
    if (!chain) return [];
    const out = new Set<string>();
    for (const pause of chain.openPauses()) {
      const touches = pause.removals.some((r) => r.host === host || r.ids.includes(host));
      if (!touches) continue;
      for (const r of pause.removals) {
        out.add(r.host);
        for (const id of r.ids) if (chain.machine(id)) out.add(id);
      }
    }
    return [...out].filter((m) => chain.device(m)).sort();
  }

  /**
   * The open renewal pauses that take an edge out of `host`'s list, each with
   * what ending it now would give back and what must stay held, and why
   * (`CloudAccount.pauseEnding`): what its row says, and its 「结束暂停」.
   */
  pausesOf(host: string): { op: Uint8Array; ending: PauseEnding }[] {
    const { account } = this.opts;
    const chain = account.chain;
    if (!chain) return [];
    return chain.openPauses().flatMap((p) => {
      if (!p.removals.some((r) => r.host === host)) return [];
      const ending = account.pauseEnding(p.op);
      return ending ? [{ op: p.op, ending }] : [];
    });
  }

  /** When a retired key of `host` was last used to sign in, as checked here (information only: §3.8). */
  retiredLoginAt(host: string): number | null {
    return this.retiredLogins.get(host) ?? null;
  }

  /** The hub's row for `host`, if it has one. */
  row(host: string): HostRotationView | undefined {
    return this.view?.hosts.find((r) => r.host === host);
  }

  private run(host: string): HostRun {
    let r = this.runs.get(host);
    if (!r) this.runs.set(host, (r = { attempts: 0, nextMs: null, error: null, outcome: null, waiting: [], doneMs: null }));
    return r;
  }

  /** Whether only a renewal confirmed on the device goes for `host` now, and why. */
  needsCode(host: string): "policy" | "device" | "evidence" | null {
    const row = this.row(host);
    const now = this.hubNow();
    const evidence =
      row?.needsAssurance === true ||
      // The hub's isolation flag with no evidence checked here: the stricter side, without isolating (R6 review).
      this.opts.account.flagged?.(host) === true ||
      (row?.failure && (row.failure.reason === "not_acked" || row.failure.reason === "conflict")) ||
      this.runs.get(host)?.outcome === "needsCode" ||
      (this.view?.disputes ?? []).some((d) => d.host === host && (d.kind === "not_acked" || d.kind === "conflict") && now - d.seenMs < EVIDENCE_MS);
    if (evidence) return "evidence";
    // The account's policy, read off the verified chain (§3.4 rule 9); this device's own guard (§4.4).
    if (this.opts.account.alwaysAssured) return "policy";
    return this.opts.account.localAssured?.() === true ? "device" : null;
  }

  /**
   * Whether `row` is an attempt nobody will finish: an `attempt` past its
   * expiry, or a staging whose window closed more than {@link ABANDONED_MS}
   * ago and that was never written (the page that made it went away).
   */
  abandoned(row: HostRotationView): boolean {
    const now = this.hubNow();
    if (row.state === "attempt") return row.attemptExpiresMs !== null && row.attemptExpiresMs <= now;
    if (row.state === "staged") {
      const ready = row.readyMs ?? row.stagedMs;
      return ready !== null && ready + ABANDONED_MS < now;
    }
    return false;
  }

  /** Whether another page has an attempt of `host` running at the hub (an abandoned one is none). */
  private elsewhere(host: string): boolean {
    const row = this.row(host);
    if (!row || this.running === host) return false;
    return (row.state === "staged" || row.state === "attempt") && !this.abandoned(row);
  }

  /**
   * Withdraw the `pending` rows of `rows` whose hosts nothing exposes any more:
   * after reading the hub's chain head (`/v1/me`) and rows again, and only
   * when this page's chain is at least that head and still says so.
   */
  private async withdrawUnexposed(rows: HostRotationView[]): Promise<void> {
    const { account } = this.opts;
    // At most every 10 s: a page whose chain lags the hub would otherwise read it on every look.
    if (this.now() - this.lastUnexposedCheck < 10_000) return;
    this.lastUnexposedCheck = this.now();
    let head: number | null;
    let fresh: RotationsView;
    try {
      head = (await account.api.me(account.session.token)).chainSeq;
      fresh = await account.api.rotations(account.session.token);
    } catch (error) {
      console.warn("cloud: reading the hub before withdrawing a renewal:", error);
      return;
    }
    const chain = account.chain;
    if (!chain || head === null || chain.seq < head) return;
    const exposed = account.hostsToRotate();
    for (const row of rows) {
      const now = fresh.hosts.find((r) => r.host === row.host);
      if (now?.state === "pending" && now.requestedMs === row.requestedMs && chain.machine(row.host) && !exposed.has(row.host)) await this.withdraw(now);
    }
  }

  /**
   * Withdraw `row` at the hub (once per row) and forget it here: that row only
   * — its attempt, by its nonce's hash, or its mark, by when it was made — so
   * that what another page started on the host since this page read it (a
   * new attempt over an expired one, or over the mark) stays.
   */
  private async withdraw(row: HostRotationView): Promise<void> {
    const { account } = this.opts;
    const key = `${row.host}@${row.requestedMs ?? ""}@${row.state}`;
    if (this.withdrawn.has(key)) return;
    this.withdrawn.add(key);
    const only =
      row.state === "pending" && row.requestedMs !== null
        ? { requestedMs: row.requestedMs }
        : (row.state === "attempt" || row.state === "staged") && row.nonceHash
          ? { nonceHash: row.nonceHash }
          : undefined;
    let withdrew = false;
    try {
      await account.api.deleteRotation(account.session.token, row.host, only);
      withdrew = true;
      for (const k of [...this.marked]) if (k.startsWith(`${row.host}@`)) this.marked.delete(k);
    } catch (error) {
      console.warn(`cloud: withdrawing ${row.host}'s renewal:`, error);
    }
    // What the hub has now, read again: a named withdrawal leaves what another page started meanwhile, and an
    // isolated host stays `pending` — its row (and so its flag) must not be forgotten here.
    try {
      this.view = await account.api.rotations(account.session.token);
      this.readAtMs = this.now();
    } catch {
      const view: RotationsView | null = this.view;
      if (withdrew && view) this.view = { ...view, hosts: view.hosts.filter((r: HostRotationView) => r.host !== row.host || r.isolated) };
    }
  }

  /** Where `host`'s renewal stands, for its row. */
  status(host: string): RenewalStatus {
    return this.standing(host, true);
  }

  /** {@link status}; `why`: for a queued host, why the ones it waits for are stuck (one level: no loops). */
  private standing(host: string, why: boolean): RenewalStatus {
    const { account } = this.opts;
    const chain = account.chain;
    if (!chain?.machine(host)) return { kind: "none" };
    if (account.isolated(host)) return this.running === host ? { kind: "renewing", elsewhere: false } : { kind: "isolated" };
    if (this.running === host) return { kind: "renewing", elsewhere: false };
    const exposed = account.hostsToRotate();
    const x = exposed.get(host);
    const row = this.row(host);
    if (x || (row && row.state !== "committed")) {
      if (!account.renews(host)) {
        // It cannot renew here: an agent without `rotate/1` on a hub that takes renewals is too old; otherwise the re-enroll warning speaks.
        const declared = this.opts.me?.view?.machines.find((m) => m.id === host);
        return this.view?.enabled && chain.isV3 && declared && !declared.rotates ? { kind: "tooOld" } : { kind: "none" };
      }
      if (this.elsewhere(host)) return { kind: "renewing", elsewhere: true };
      // Controllers first (§4.1 「次序先于收窄」): every exposed host it lets control it renews before it does — nothing of its own goes before.
      const after = x ? exposedControllers(chain, exposed, host) : [];
      // Blocked: it lets an exposed host that cannot renew control it. Only through one that can — itself blocked — it waits for that one (queued: "… is blocked").
      const by = x ? [...x.blockedBy.keys()].sort() : [];
      if (by.length && (by.some((e) => !account.renews(e)) || after.length === 0)) return { kind: "blocked", by };
      if (x && chain.renewalPolicySeq === null) return { kind: "policyUnset" };
      const run = this.runs.get(host);
      if (run?.outcome === "tooOld") return { kind: "tooOld" };
      if (after.length) {
        const stuck: { host: string; why: QueuedWhy }[] = [];
        for (const e of why ? after : []) {
          const kind = this.standing(e, false).kind;
          if (kind === "needsCode" || kind === "isolated" || kind === "blocked" || kind === "offline" || kind === "stuck" || kind === "tooOld" || kind === "waiting") stuck.push({ host: e, why: kind });
        }
        return { kind: "queued", after, stuck };
      }
      const code = this.needsCode(host);
      if (code) return { kind: "needsCode", why: code };
      if (run?.outcome === "stuck") return { kind: "stuck", error: run.error ?? "", cause: run.cause };
      if (run?.outcome === "waiting") return { kind: "waiting", hosts: run.waiting };
      if (run?.nextMs !== null && run?.nextMs !== undefined && run.nextMs > this.now()) return { kind: "retrying", atMs: run.nextMs, error: run.error ?? "", cause: run.cause };
      if (this.opts.link.presence.get(host)?.state !== "online") return { kind: "offline" };
      return { kind: "renewing", elsewhere: false };
    }
    const at = this.renewedAt(host);
    if (at !== null && this.hubNow() - at < RENEWED_SHOWN_MS) return { kind: "renewed", atMs: at };
    return { kind: "none" };
  }

  /**
   * The isolated hosts `host` lets control it (R6 review): until they are
   * removed, whoever holds their current key may drive it — remove them first.
   */
  isolatedControllers(host: string): string[] {
    const { account } = this.opts;
    const chain = account.chain;
    if (!chain?.machine(host)) return [];
    const policy = chain.acceptPolicy(host);
    return (chain.hosts() ?? [])
      .map((h) => h.info.id)
      .filter((id) => id !== host && account.isolated(id) && (policy.type === "all" || (policy.type === "devices" && policy.ids.includes(id))))
      .sort();
  }

  /** When `host` last renewed its identity key, if it did: its entry's time, or the hub's commit (the later). */
  renewedAt(host: string): number | null {
    const chain = this.opts.account.chain;
    const record = chain?.machine(host);
    if (!chain || record?.rotatedSeq === null || record?.rotatedSeq === undefined) return null;
    const entry = chain.entryAt(record.rotatedSeq);
    const row = this.row(host);
    const committed = row?.state === "committed" && row.committedSeq === record.rotatedSeq ? row.committedMs : null;
    const at = Math.max(entry?.draft.createdMs ?? 0, committed ?? 0, this.runs.get(host)?.doneMs ?? 0);
    return at > 0 ? at : null;
  }

  /** What the account's notice says (§4.4): hosts being renewed (or waiting their turn), and hosts that need the person. */
  notice(): { renewing: string[]; needYou: string[] } {
    const renewing: string[] = [];
    const needYou: string[] = [];
    for (const h of this.opts.account.chain?.hosts() ?? []) {
      if (h.status.type !== "active") continue;
      const s = this.status(h.info.id);
      if (s.kind === "renewing" || s.kind === "offline" || s.kind === "queued" || s.kind === "retrying" || s.kind === "waiting") renewing.push(h.info.id);
      else if (s.kind === "needsCode" || s.kind === "isolated" || s.kind === "stuck" || s.kind === "blocked") needYou.push(h.info.id);
    }
    return { renewing, needYou };
  }

  /** See what is due again, soon (one look at a time). */
  private poke(): void {
    if (this.stopped) return;
    if (this.ticking) {
      this.again = true;
      return;
    }
    this.ticking = true;
    void this.serial
      .run(async () => {
        do {
          this.again = false;
          await this.tick();
        } while (this.again && !this.stopped);
      })
      .finally(() => {
        this.ticking = false;
      });
  }

  /** Register what is exposed at the hub, and renew the next host that can go now. */
  private async tick(): Promise<void> {
    const { account } = this.opts;
    let chain = account.chain;
    this.note();
    if (this.stopped || !account.isAdmin || !chain?.isV3 || !this.view) return;
    // Switched off at the hub: nothing renews, so nothing kept for a renewal stays.
    if (!this.view.enabled) return this.switchedOff();
    // A renewal the hub committed that this page's chain has not taken yet: the chain first. Marking that host on
    // the old view would set it back to `pending` at the hub and suspend it for nothing.
    if ((this.view.hosts ?? []).some((r) => r.state === "committed" && r.committedSeq !== null && r.committedSeq > chain!.seq)) {
      try {
        await account.sync();
      } catch (error) {
        console.warn("cloud: syncing before renewing:", error);
      }
      chain = account.chain;
      if (!chain || this.view.hosts.some((r) => r.state === "committed" && r.committedSeq !== null && r.committedSeq > chain!.seq)) return;
    }
    // This browser's own attempt, left by a page of it that went away before writing it (a reload, a crash, a closed
    // tab in the window): only the page that asked could check the host's answer, so nobody else will write it. Taken
    // up here — written, or (left before its answer checked out) withdrawn at once and started again (§4.2).
    let resumed = false;
    for (const row of [...this.view.hosts]) {
      if (this.stopped || this.running === row.host || !row.nonceHash || (row.state !== "staged" && row.state !== "attempt")) continue;
      const key = `${row.host}@${row.nonceHash}`;
      if (this.resumed.has(key) || !(await account.keptRenewal(row.host, row.nonceHash))) continue;
      const outcome = await this.resume(row.host);
      // Another tab of this browser on it (alive): looked at again later.
      if (outcome !== "busy") this.resumed.add(key);
      resumed ||= outcome !== "busy";
    }
    // What it changed (the chain, the hub's rows) is looked at afresh: the next look.
    if (resumed) {
      this.again = true;
      return;
    }
    // What this browser kept of attempts the hub no longer has (written, withdrawn or run out by any page): forgotten,
    // at most every 10 s.
    if (this.now() - this.lastLeftCheck >= 10_000) {
      this.lastLeftCheck = this.now();
      await account.forgetLeftRenewals(this.view.hosts);
    }
    // Attempts nobody will finish: withdrawn, so the renewal can start again (§4.2).
    for (const row of [...this.view.hosts]) if (this.running !== row.host && this.abandoned(row)) await this.withdraw(row);
    const exposed = account.hostsToRotate();
    // Pending at the hub but exposed no more: withdrawn, its device role back — judged on the hub's head and rows read
    // just now, and only when this page's chain is at least that new (a mark another page made for an exposure this
    // page has not seen yet stays).
    // (Not a host the hub keeps isolated: that is no mark of a page's, and a withdrawal leaves it `pending`.)
    const unexposed = (this.view?.hosts ?? []).filter((row) => row.state === "pending" && !row.isolated && chain!.machine(row.host) && !exposed.has(row.host));
    if (unexposed.length) await this.withdrawUnexposed(unexposed);
    // Nothing renews before an admin chose how renewals are confirmed (§5.6 「打开的顺序」): the rows say so, and the setting asks.
    if (chain.renewalPolicySeq === null) return;
    // §4.1 「互相放行的一组：先收窄，再换，最后恢复」: no exposed host renews while it still lets another exposed one control it.
    const narrowed = await this.narrowGroup();
    if (narrowed !== "none") return;
    if (await this.restoreGroups()) return;
    for (const [host, x] of exposed) {
      // Only what a renewal fixes is marked (§4.1 case 3: a host an exposed one that cannot renew controls waits for the person).
      if (!account.renews(host) || account.isolated(host) || !renewalHelps(x)) continue;
      const key = `${host}@${x.sinceSeq}`;
      const row = this.row(host);
      if (this.marked.has(key) || (row && row.state !== "committed")) continue;
      this.marked.add(key);
      try {
        const marked = await account.api.putRotation(account.session.token, host, { assurance: this.needsCode(host) ? ASSURANCE_ON_DEVICE : ASSURANCE_AUTO });
        const view: RotationsView | null = this.view;
        if (view) this.view = { ...view, hosts: [...view.hosts.filter((r: HostRotationView) => r.host !== host), marked] };
      } catch (error) {
        const why = error instanceof HubApiError ? error : null;
        if (why?.code === "rotation_not_ready" && why.reason === "agent_too_old") this.run(host).outcome = "tooOld";
        else if (!(why?.code === "rotation_in_progress")) console.warn(`cloud: marking ${host} to renew its identity key:`, error);
      }
    }
    this.note();
    this.emit();
    this.retryWaiting(chain);
    const now = this.now();
    const due = [...exposed.keys()].filter((host) => {
      const run = this.runs.get(host);
      return (
        account.renews(host) &&
        renewalHelps(exposed.get(host)!) &&
        // Never before an exposed host it lets control it (§4.1): that one's old key would still reach its new seed.
        exposedControllers(chain, exposed, host).length === 0 &&
        !account.isolated(host) &&
        !this.needsCode(host) &&
        !run?.outcome &&
        (run?.nextMs === null || run?.nextMs === undefined || run.nextMs <= now) &&
        this.opts.link.presence.get(host)?.state === "online" &&
        !this.elsewhere(host)
      );
    });
    this.schedule();
    const next = renewalOrder(due, exposed, (h) => account.renews(h))[0];
    if (!next || !(await this.attempt(next, { assurance: ASSURANCE_AUTO }, true))) return;
    // Renewed: what it changed may make the next one due.
    this.again = true;
  }

  /**
   * Renewals switched off at the hub (`enabled: false`), maybe while some were
   * under way: nothing renews now, so nothing kept for one stays. The marks
   * are withdrawn — a `pending` host has its keys withheld here (`note`) and
   * its device role suspended at the hub — and so are attempts nobody will
   * finish — but not a host the hub keeps isolated: no page's mark, it stays
   * `pending` at the hub whatever is withdrawn, and its flag holds the pauses
   * over it. The open pauses end where nothing they hold must stay held, judged
   * on the chain alone as ever ({@link restoreGroups}); one that must keep some
   * held would need a new start, which the hub refuses while off: it stays,
   * and its rows say why.
   */
  private async switchedOff(): Promise<void> {
    for (const row of [...(this.view?.hosts ?? [])]) {
      if (this.running !== row.host && !row.isolated && (row.state === "pending" || this.abandoned(row))) await this.withdraw(row);
    }
    await this.restoreGroups();
    this.note();
    this.emit();
  }

  /**
   * A host the hub would not renew while other hosts ran a Droppen too old to
   * follow a renewal (`waiting`) is due again once `/v1/me`, read since, says
   * each of them declared `rotate/1` (or is no host of the account any more) —
   * never by the clock alone: every try costs one of the hub's attempts.
   */
  private retryWaiting(chain: ChainState): void {
    const me = this.opts.me?.view;
    if (!me) return;
    const updated = (host: string) => !chain.machine(host) || me.machines.some((m) => m.id === host && m.rotates);
    for (const run of this.runs.values()) {
      if (run.outcome !== "waiting" || run.waitingSeen === me || !run.waiting.every(updated)) continue;
      run.outcome = null;
      run.waiting = [];
      run.waitingSeen = null;
      run.nextMs = null;
    }
  }

  /**
   * Wake for the earliest retry that is due later — and for when another
   * page's attempt or staging is to be taken for abandoned ({@link abandoned}):
   * a staging is committed only by the page that asked for it (or, should that
   * one go away, the next page of its browser: `CloudAccount.resumeRenewal`),
   * and once that browser is gone nothing else need wake a quiet page to
   * withdraw it and start again, while the host waits with its device role
   * suspended.
   */
  private schedule(): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
    const now = this.now();
    const retries = [...this.runs.values()].map((r) => (r.outcome === null && r.nextMs !== null ? r.nextMs : Infinity));
    // By the hub's clock, as `abandoned` reads it: on this page's, a moment past.
    const skew = this.hubNow() - now;
    const abandons = (this.view?.hosts ?? []).flatMap((row) => {
      const at = this.running === row.host ? null : abandonsAt(row);
      return at === null || at - skew + 1000 <= now ? [] : [at - skew + 1000];
    });
    const next = Math.min(...retries, ...abandons);
    if (!Number.isFinite(next) || this.stopped) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.poke();
    }, Math.max(0, next - now));
    (this.timer as { unref?: () => void }).unref?.();
  }

  /**
   * One renewal of `host`: on its own (`auto`: failures are counted and tried
   * again later), or as the person asked (their failure is theirs to see).
   * Whether it went through.
   */
  private async attempt(host: string, how: Pick<RenewOptions, "assurance" | "askCode" | "onStep">, auto: boolean): Promise<boolean> {
    const { account } = this.opts;
    return (await this.go(host, auto, () => account.renewIdentity(host, this.opts.courier(), { ...this.opts.renewOptions, ...how }))) === "renewed";
  }

  /**
   * Take up this browser's attempt of `host` that a page of it left
   * (`CloudAccount.resumeRenewal`), as an automatic attempt: `renewed`,
   * `busy` (another tab of this browser is on it), or `none` (nothing to
   * finish, withdrawn, or failed — counted as {@link failed} says).
   */
  private resume(host: string): Promise<"renewed" | "busy" | "none"> {
    const { account } = this.opts;
    return this.go(host, true, () => account.resumeRenewal(host, this.opts.courier(), this.opts.renewOptions));
  }

  /** `work` on `host` as this page's renewal in progress (its row says so), and the hub's renewals read again after. */
  private async go(host: string, auto: boolean, work: () => Promise<RenewalReport | "busy" | null>): Promise<"renewed" | "busy" | "none"> {
    const { account } = this.opts;
    const run = this.run(host);
    this.running = host;
    this.emit();
    try {
      const done = await work();
      if (done === "busy" || done === null) return done === "busy" ? "busy" : "none";
      this.runs.set(host, { attempts: 0, nextMs: null, error: null, outcome: null, waiting: [], doneMs: this.now() });
      return "renewed";
    } catch (error) {
      // A failed attempt was withdrawn at the hub: mark it waiting again (its device role stays suspended).
      for (const key of [...this.marked]) if (key.startsWith(`${host}@`)) this.marked.delete(key);
      if (!auto) throw error;
      this.failed(host, run, error);
      return "none";
    } finally {
      this.running = null;
      try {
        this.view = await account.api.rotations(account.session.token);
        this.readAtMs = this.now();
      } catch {
        // as it was
      }
      this.note();
      this.emit();
    }
  }

  /** What an automatic attempt's failure means for the next one (§4.2's table). */
  private failed(host: string, run: HostRun, error: unknown): void {
    const why = error instanceof RenewalError ? error : null;
    run.error = error instanceof Error ? error.message : String(error);
    run.cause = error;
    switch (why?.kind) {
      case "tooOld":
        run.outcome = "tooOld";
        return;
      case "needsAssurance":
        run.outcome = "needsCode";
        return;
      case "voided":
        // Another holder of its old key answered or signed in: only the device's own screen tells them apart now.
        if (why.detail === "not_acked" || why.detail === "conflict") {
          run.outcome = "needsCode";
          return;
        }
        // A connection lost, or replaced, in the window (graded evidence, §3.3): no clone evidence — tried again with a
        // fresh nonce soon (within the hub's 6 an hour), not counted towards asking the person.
        if (why.detail === "interrupted" || why.detail === "replaced") {
          run.nextMs = this.now() + (this.opts.retryMs ?? RENEW_RETRY_MS)[0]!;
          return;
        }
        break;
      case "waiting":
        // Not its failure: tried again once the hosts it waits for updated (`retryWaiting`).
        run.outcome = "waiting";
        run.waiting = why.hosts ?? [];
        run.waitingSeen = this.opts.me?.view ?? null;
        run.nextMs = null;
        return;
      case "renewFirst":
      case "policyUnset":
        // Out of order on a chain that changed meanwhile, or the policy not chosen: not its failure — the order (`due`), or the admin, says when.
        return;
      case "offline":
      case "inProgress":
        // Not a failure of its own — presence, or the other page, says when — but not at once either.
        run.nextMs = this.now() + (this.opts.retryMs ?? RENEW_RETRY_MS)[0]!;
        return;
      case "fork":
        // The host's view of the account conflicts with an admin's word: no try of its own mends that — the person is asked.
        run.outcome = "stuck";
        run.nextMs = null;
        return;
      case "refused":
        // A v2 account renews nothing: the re-enroll path (no retry).
        if (why.detail === "v2") {
          run.outcome = "stuck";
          run.nextMs = null;
          return;
        }
        break;
      default:
        break;
    }
    const delays = this.opts.retryMs ?? RENEW_RETRY_MS;
    run.attempts++;
    if (run.attempts > delays.length) {
      run.outcome = "stuck";
      run.nextMs = null;
      return;
    }
    run.nextMs = this.now() + delays[run.attempts - 1]!;
  }

  /**
   * Renew `host` now, as the person asked (the row's "renew identity key",
   * "enter the code shown on the device", "try again"): `ASSURANCE_ON_DEVICE`
   * asks them for the code (`askCode`). One at a time with the automatic
   * ones; its failure is thrown for the page to say.
   */
  renew(host: string, how: { assurance: Assurance; askCode?: (host: string) => Promise<string | null>; onStep?: RenewOptions["onStep"] }): Promise<void> {
    // Never ahead of an exposed host it lets control it (§4.1): the row says to wait for those; it renews after them on its own.
    const first = this.opts.account.renewFirst(host);
    if (first.length) {
      return Promise.reject(new RenewalError("renewFirst", `renew the identity key of ${first.join(", ")} first`, undefined, first));
    }
    // A person's try starts the automatic count afresh.
    this.runs.set(host, { attempts: 0, nextMs: null, error: null, outcome: null, waiting: [], doneMs: this.runs.get(host)?.doneMs ?? null });
    return this.serial.run(async () => {
      await this.attempt(host, how, false);
    }).finally(() => this.poke());
  }

  /** The account's 「更换身份密钥时总在设备上确认」: a `setRenewalPolicy` entry on the chain (§4.4), then a look at what is due. */
  async setAlwaysAssured(on: boolean): Promise<void> {
    await this.opts.account.setRenewalPolicy(on);
    this.emit();
    await this.refresh();
  }
}

let current: RenewalRunner | null = null;

/** The page's renewal runner (`null`: none, e.g. local mode or before sign-in). */
export function renewalRunner(): RenewalRunner | null {
  return current;
}

export function setRenewalRunner(next: RenewalRunner | null): void {
  current?.stop();
  current = next;
}
