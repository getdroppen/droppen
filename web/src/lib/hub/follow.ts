/*
 * Keeping up with the account while the page is open (architecture doc §1
 * "吊销与轮换", §5 "冻结"):
 *
 * - follow the chain live: on every (re)connection ask the hub for the
 *   entries after the ones this device has (`chain_sub`), then merge each
 *   `chain` frame as it lands (verified, as every entry is);
 * - after every change — and every few hours while open — attest the head
 *   this device sees, so the account's agents always hold a recent
 *   attestation from an established full device (they open no channel
 *   without one);
 * - take keys mailed to this device as they arrive (the hub's `mail` notice,
 *   and on every connection for mail left while away);
 * - notice this device being removed — the chain says so, or the hub closes
 *   the connection with 4403 and the chain confirms it — and hand over to the
 *   page, which wipes its keys and copies. A hub that refuses this device's
 *   session (the account's `refusal`) without the chain showing the removal
 *   is only a claim: the page stops, but keeps the keys (`onRefused`) —
 *   unless it says the account itself was deleted (`error account_deleted`
 *   on the socket, `410 account_deleted` to a renewal: refusal `deleted`),
 *   when the page wipes them too: there is no account left for them to open.
 *   An account suspended (`account_suspended`) is a refusal like any other:
 *   nothing is wiped for it.
 */
import { Serial } from "@/lib/e2e";
import { AccountError, type CloudAccount } from "./account";
import { HubApiError } from "./api";
import type { HubLink } from "./link";

/** How often an open page re-attests the head even when nothing changed. */
export const ATTEST_EVERY_MS = 3 * 60 * 60 * 1000;
/** The hub's close for a removed device's connection. */
export const REMOVED_CLOSE = 4403;

export type FollowLink = Pick<HubLink, "onState" | "onControl" | "onChain" | "sendControl" | "attest" | "state" | "lastClose">;

/** Whether a control message on the device socket says the account was deleted (`error {code: "account_deleted"}`, before the 4403 close). */
export function accountDeletedMessage(message: Record<string, unknown>): boolean {
  return message.t === "error" && message.code === "account_deleted";
}

export interface FollowerOptions {
  link: FollowLink;
  account: CloudAccount;
  /** This device is no longer in the account (the chain says so). */
  onRemoved: () => void;
  /** The hub refuses this device, but the chain this device holds does not show it removed. */
  onRefused?: () => void;
  /** The hub served a chain that forks from this device's: an alarm, never merged. */
  onAlarm?: (message: string) => void;
  attestEveryMs?: number;
}

export class ChainFollower {
  private readonly offs: (() => void)[] = [];
  private timer: ReturnType<typeof setInterval> | null = null;
  private readonly serial = new Serial();
  private removed = false;
  private stopped = false;
  /** The head last vouched for: a change of this device's own (an entry it appended) is attested too. */
  private attestedSeq = -1;

  constructor(private readonly opts: FollowerOptions) {}

  start(): void {
    const { link } = this.opts;
    this.offs.push(
      link.onState((state) => {
        if (state === "open") this.onOpen();
        else if (state === "closed" && link.lastClose === REMOVED_CLOSE) void this.run(() => this.checkRemoved(true));
      }),
      link.onChain((entries) => void this.run(() => this.onEntries(entries))),
      link.onControl((message) => {
        if (message.t === "mail") void this.run(() => this.onMail());
        // The account itself is gone (deleted; the 4403 close follows): its keys here open nothing any more.
        else if (accountDeletedMessage(message)) this.opts.account.setRefusal("deleted");
      }),
      this.opts.account.onChange(() => {
        if (this.opts.account.refusal) {
          void this.run(() => this.checkRemoved(true));
          return;
        }
        const seq = this.opts.account.chain?.seq ?? -1;
        if (seq !== this.attestedSeq) void this.run(() => this.attest());
      }),
    );
    this.timer = setInterval(() => void this.run(() => this.attest()), this.opts.attestEveryMs ?? ATTEST_EVERY_MS);
    // Where timers can (node), this one alone does not keep the process up.
    (this.timer as { unref?: () => void }).unref?.();
    if (link.state === "open") this.onOpen();
  }

  /** Resolves once everything queued so far is done (and what that queued, `rounds` deep). */
  async idle(rounds = 3): Promise<void> {
    for (let i = 0; i < rounds; i++) await this.serial.run(async () => undefined);
  }

  stop(): void {
    this.stopped = true;
    for (const off of this.offs.splice(0)) off();
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
  }

  /** One thing at a time: entries merge in order, and a removal is acted on once. */
  private run(work: () => Promise<unknown>): Promise<unknown> {
    return this.serial.run(async () => {
      if (this.stopped) return;
      try {
        await work();
      } catch (error) {
        console.warn("chain follower:", error);
      }
    });
  }

  private onOpen(): void {
    const seq = this.opts.account.chain?.seq ?? -1;
    this.opts.link.sendControl({ t: "chain_sub", from: seq + 1 });
    void this.run(() => this.attest());
    void this.run(() => this.onMail());
  }

  private async onEntries(entries: Uint8Array[]): Promise<void> {
    let applied: number;
    try {
      applied = await this.opts.account.absorb(entries);
    } catch (error) {
      if (error instanceof AccountError && error.kind === "fork") {
        this.opts.onAlarm?.(error.message);
        return;
      }
      throw error;
    }
    if (!applied) return;
    if (await this.checkRemoved(false)) return;
    await this.attest();
    // A removal elsewhere means a new user key in the mail; a new machine
    // key in the keyring; a recovery key replaced, packages to keep.
    await this.opts.account.settle();
  }

  private async onMail(): Promise<void> {
    if (this.opts.account.standing !== "active") return;
    await this.opts.account.receiveMail();
  }

  /**
   * Whether this device was removed, or the hub refuses it; acts on it once.
   * `hubSaid`: the hub closed with 4403 or refused the session — confirm it
   * on the chain, which alone decides whether the keys go.
   */
  private async checkRemoved(hubSaid: boolean): Promise<boolean> {
    if (this.removed) return true;
    let refused = this.opts.account.refusal !== null;
    if (hubSaid && !refused) {
      try {
        await this.opts.account.sync();
      } catch (error) {
        refused = this.opts.account.refusal !== null || (error instanceof HubApiError && (error.status === 401 || error.status === 403));
        if (!refused) throw error;
      }
    }
    if (this.opts.account.standing === "removed") {
      this.removed = true;
      this.stop();
      this.opts.onRemoved();
      return true;
    }
    if (!refused) return false;
    // The hub's word only: stop, keep the keys.
    this.removed = true;
    this.stop();
    this.opts.onRefused?.();
    return true;
  }

  /** Vouch for the head this device sees (an active admin only: chain v3 counts no other device's word). */
  async attest(): Promise<void> {
    // A view that forked vouches for nothing; one behind what this browser verified catches up first (`attestation`).
    if (this.opts.account.forked !== null) return;
    if (this.opts.account.standing !== "active" || this.opts.account.isAdmin === false) return;
    const attestation = await this.opts.account.attestation();
    if (this.opts.link.attest(attestation)) this.attestedSeq = attestation.seq;
  }
}
