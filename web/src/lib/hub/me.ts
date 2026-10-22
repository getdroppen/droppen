/*
 * The page's live copy of `/v1/me`: the account's plan and usage, each
 * machine's offline copy on the hub, and the devices registered with the
 * account's password — kept current from the hub link: the `plan` control
 * message (after sign-in and on every change: its `detail` is `/v1/me`'s
 * `plan`), `device_registered`, `sign_in_changed` (a passkey or the
 * password added, renamed, changed or removed: every full device says so),
 * and a fresh read when the link opens or a machine's presence changes (the
 * hub notes when an agent signed out).
 *
 * One store per account (the page has one account at a time). Reads are
 * coalesced and spaced, so the many components that show a part of it ask
 * the hub once.
 */
import type { AccountView, PlanView, SignInChange } from "./api";
import { planView, signInChange } from "./api";
import { REGISTRATION_TTL_MS, type Registration } from "./plan";

export type MeLink = {
  onControl(fn: (message: Record<string, unknown>) => void): () => void;
  onPresence(fn: (presence: Map<string, { state: string; sinceMs: number }>) => void): () => void;
  onState(fn: (state: "connecting" | "open" | "closed") => void): () => void;
};

export interface MeStoreOptions {
  now?: () => number;
  /** At least this long between two reads that were not asked for outright. */
  minGapMs?: number;
  /** How long after a change (a presence, a plan) to read again. */
  settleMs?: number;
  /** Where dismissed registrations are kept (`localStorage`). */
  storage?: Pick<Storage, "getItem" | "setItem"> | null;
  /** The account, for the storage key. */
  account?: string;
}

const dismissedKey = (account: string) => `rh.hub.registered.dismissed.${account}`;
const MAX_DISMISSED = 64;
/** Which sign-in changes this browser has had said: those up to `baseline` (before it first looked) and the ones dismissed since. */
const changesKey = (account: string) => `rh.hub.sign-in-changes.${account}`;
type ChangesSeen = { baseline: number; dismissed: number[] };

export class MeStore {
  /** The last `/v1/me` read. */
  view: AccountView | null = null;
  /** The newest plan: from that read, or a `plan` message since. */
  plan: PlanView | null = null;
  /** Bumped on every change (for `useSyncExternalStore`). */
  version = 0;
  private readonly registered = new Map<string, number>();
  private readonly dismissedSet: Set<string>;
  private readonly changes = new Map<number, SignInChange>();
  /** `null` until a first read says where this browser starts (every change before is history). */
  private changesSeen: ChangesSeen | null;
  private readonly listeners = new Set<() => void>();
  private inflight: Promise<void> | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private lastReadMs = 0;
  private attached: MeLink | null = null;
  private readonly offs: (() => void)[] = [];

  constructor(
    private readonly read: () => Promise<AccountView>,
    private readonly opts: MeStoreOptions = {},
  ) {
    this.dismissedSet = new Set(this.loadDismissed());
    this.changesSeen = this.loadChangesSeen();
  }

  private now(): number {
    return (this.opts.now ?? Date.now)();
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

  /** Read `/v1/me` now (one read at a time: a second ask joins the one running). */
  refresh(): Promise<void> {
    if (this.inflight) return this.inflight;
    this.lastReadMs = this.now();
    this.inflight = (async () => {
      try {
        const view = await this.read();
        this.view = view;
        if (view.plan) this.plan = view.plan;
        const now = this.now();
        for (const d of view.devices) {
          if (!d.inChain && d.createdMs !== null && now - d.createdMs <= REGISTRATION_TTL_MS) this.noteRegistered(d.id, d.createdMs);
        }
        // (A view made by hand — a test's, the preview's — may have none.)
        const changes = view.signInChanges ?? [];
        for (const c of changes) this.changes.set(c.id, c);
        // The first look of this browser: what changed before it is history, not news.
        if (!this.changesSeen) this.saveChangesSeen({ baseline: Math.max(0, ...changes.map((c) => c.id)), dismissed: [] });
        this.emit();
      } finally {
        this.inflight = null;
      }
    })();
    return this.inflight;
  }

  /** Read again unless a read is recent (`minGapMs`); errors are logged, not thrown. */
  refreshIfStale(): void {
    if (this.inflight || this.now() - this.lastReadMs < (this.opts.minGapMs ?? 10_000)) return;
    this.refresh().catch((error: unknown) => console.warn("cloud: /v1/me:", error));
  }

  /** Read again a moment after a change (several changes, one read). */
  soon(): void {
    if (this.timer) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.refresh().catch((error: unknown) => console.warn("cloud: /v1/me:", error));
    }, this.opts.settleMs ?? 1500);
    (this.timer as { unref?: () => void }).unref?.();
  }

  /** A control message from the hub link: `plan` and `device_registered` are taken; anything else is ignored. Whether it was taken. */
  control(message: Record<string, unknown>): boolean {
    if (message.t === "plan") {
      const plan = planView(message.detail);
      if (plan) {
        this.plan = plan;
        this.emit();
      }
      // The per-machine copies are not in the message: read them too.
      this.soon();
      return true;
    }
    if (message.t === "sign_in_changed") {
      const change = signInChange(message);
      if (change) {
        this.changes.set(change.id, change);
        this.emit();
      }
      return true;
    }
    if (message.t === "device_registered" && typeof message.device === "string") {
      const created = typeof message.created_ms === "number" && Number.isFinite(message.created_ms) ? message.created_ms : this.now();
      this.noteRegistered(message.device, created);
      this.emit();
      return true;
    }
    return false;
  }

  /** Follow `link` (once; a later link replaces it). */
  attach(link: MeLink): void {
    if (this.attached === link) return;
    for (const off of this.offs.splice(0)) off();
    this.attached = link;
    this.offs.push(
      link.onControl((message) => void this.control(message)),
      link.onPresence(() => this.soon()),
      link.onState((state) => {
        if (state === "open") this.soon();
      }),
    );
  }

  private noteRegistered(device: string, createdMs: number): void {
    const had = this.registered.get(device);
    if (had === undefined || createdMs > had) this.registered.set(device, createdMs);
  }

  /** Every registration heard of (from the link, or `/v1/me`). */
  registrations(): Registration[] {
    return [...this.registered.entries()].map(([device, createdMs]) => ({ device, createdMs }));
  }

  /** The changes to the account's ways to sign in this browser has not had said yet (none before its first read), oldest first. */
  signInChanges(): SignInChange[] {
    const seen = this.changesSeen;
    if (!seen) return [];
    const dismissed = new Set(seen.dismissed);
    return [...this.changes.values()].filter((c) => c.id > seen.baseline && !dismissed.has(c.id)).sort((a, b) => a.id - b.id);
  }

  /** The newest sign-in change heard of (its id; 0: none), said or not: what a page listing the ways to sign in reads again on. */
  get signInChangeHead(): number {
    return Math.max(0, ...this.changes.keys());
  }

  /** "That was me" for a sign-in change: not said again on this browser. */
  dismissSignInChange(id: number): void {
    const seen = this.changesSeen ?? { baseline: 0, dismissed: [] };
    this.saveChangesSeen({ baseline: seen.baseline, dismissed: [...seen.dismissed.filter((d) => d !== id), id].slice(-MAX_DISMISSED) });
    this.emit();
  }

  private saveChangesSeen(seen: ChangesSeen): void {
    this.changesSeen = seen;
    try {
      if (this.opts.account) this.opts.storage?.setItem(changesKey(this.opts.account), JSON.stringify(seen));
    } catch {
      // memory only
    }
  }

  private loadChangesSeen(): ChangesSeen | null {
    try {
      const raw = this.opts.account ? this.opts.storage?.getItem(changesKey(this.opts.account)) : null;
      if (!raw) return null;
      const r = JSON.parse(raw) as Partial<ChangesSeen>;
      if (typeof r.baseline !== "number" || !Number.isFinite(r.baseline)) return null;
      return { baseline: r.baseline, dismissed: Array.isArray(r.dismissed) ? r.dismissed.filter((x): x is number => typeof x === "number") : [] };
    } catch {
      return null;
    }
  }

  get dismissed(): ReadonlySet<string> {
    return this.dismissedSet;
  }

  /** "That was me": a registration not shown again on this device. */
  dismiss(device: string): void {
    this.dismissedSet.add(device);
    const list = [...this.dismissedSet].slice(-MAX_DISMISSED);
    try {
      if (this.opts.account) this.opts.storage?.setItem(dismissedKey(this.opts.account), JSON.stringify(list));
    } catch {
      // memory only
    }
    this.emit();
  }

  private loadDismissed(): string[] {
    try {
      const raw = this.opts.account ? this.opts.storage?.getItem(dismissedKey(this.opts.account)) : null;
      const list = raw ? (JSON.parse(raw) as unknown) : [];
      return Array.isArray(list) ? list.filter((x): x is string => typeof x === "string") : [];
    } catch {
      return [];
    }
  }

  stop(): void {
    for (const off of this.offs.splice(0)) off();
    this.attached = null;
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }
}

function defaultStorage(): Pick<Storage, "getItem" | "setItem"> | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

const stores = new WeakMap<object, MeStore>();

/**
 * The store for `account` (made on first use), following `link` once given.
 * `account` is anything with an api and a session: a `CloudAccount`.
 */
export function meStore(
  account: { account: string; api: { me(token: string): Promise<AccountView> }; session: { token: string } },
  link?: MeLink | null,
): MeStore {
  let store = stores.get(account);
  if (!store) {
    store = new MeStore(() => account.api.me(account.session.token), { account: account.account, storage: defaultStorage() });
    stores.set(account, store);
  }
  if (link) store.attach(link);
  return store;
}
