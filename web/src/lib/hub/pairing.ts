/*
 * Adding a second device, both ends (architecture doc §1 "新设备如何拿到钥";
 * the constructions are `lib/e2e/pairing.ts` and `pake.ts`).
 *
 * The new device (`PairingResponder`) is signed in and registered at the hub
 * but not in the chain. It offers either
 *
 * - a QR code / pasteable text, `RHP1-<base32>` of its {@link Offer}: the
 *   account, its id, its two public keys and a one-time secret `s` that stays
 *   in this page's memory; or
 * - an 8-digit code: it opens a pairing at the hub (`POST /v1/pairings`),
 *   waits for the existing device's hello, runs CPace as the responder with
 *   `channelId(account, itself)`, and posts its answer — once: one run per
 *   code, and any failure means a fresh code ("start again"), never the same.
 *
 * Either way it then waits for a pairing delivery in its mailbox (purpose 3),
 * checks its MAC with `s` (or the run's pairing secret), fetches and verifies
 * the chain from genesis, opens the delivery against it (the account key's
 * fingerprint, a current full sender with the keys it names, this device
 * listed with its own keys, every user key the one the chain committed) and
 * keeps the `UK` epochs — and only then pins the account and says "paired".
 * A browser that has not pinned the account takes keys no other way.
 *
 * A device already in the chain that has no keys (or never pinned) pairs
 * again the same way: a new offer or code, and the existing device, seeing it
 * listed with these very keys, sends a delivery MAC'd with the NEW secret
 * without adding it twice.
 *
 * A browser that joined with a passkey and waits (`docs/passkeys.md` §4.6)
 * offers itself the same way, and this is how it is approved: the existing
 * device approves it (`approveDevice`, `setAdmin`) only when the keys the
 * offer or the code run authenticated are the ones its join named, and the
 * join still waits (`CloudAccount.pairDevice`) — then mails the delivery.
 *
 * The existing device (`candidateFromOffer` / `runCodeInitiator`, then
 * `completePairing`) reads the offer, or runs CPace as the initiator against
 * the pairing the user picked and checks the answer — a wrong code aborts the
 * run on both ends (it closes the pairing, which the new device sees) — shows
 * the new device's fingerprint to confirm, then signs "add device" for exactly
 * the offered or answered keys and mails the delivery.
 *
 * Messages on the hub's pairing rendezvous, as hex bodies: the initiator's
 * `encodePakeHello` (`sid ‖ share`), then the responder's `encodePakeAnswer`.
 * Only the new device, which opened the pairing, closes it (`DELETE`): once
 * its delivery arrived or it gave up waiting. It answers one hello only, so a
 * code is spent on its first run either way. Before it has answered, the
 * pairing going away means the code expired; after, only the delivery (or
 * its absence) tells.
 */
import {
  type Action,
  type Bytes,
  ChainState,
  channelId,
  checkDeliveryTag,
  Cpace,
  createDelivery,
  decodeDelivery,
  decodePakeAnswer,
  decodePakeHello,
  type Delivery,
  encodeDelivery,
  encodePakeAnswer,
  encodePakeHello,
  equal,
  type NoiseKeyPair,
  newPairingCode,
  newSid,
  type Offer,
  offerFromText,
  offerText,
  openDelivery,
  type PakeHello,
  pakeAnswer,
  PairingError,
  Serial,
  utf8,
  verifyPakeAnswer,
} from "@/lib/e2e";
import { deviceFingerprint, joinAction } from "./account";
import { type HubApi, HubApiError, hubErrorKind, isNetworkError, knownMailPurpose, MAIL_MACHINE_KEY, MAIL_PAIRING, type PairingSummary } from "./api";

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * What stopped a pairing, for the screen to say in words and to offer the
 * way on that can work (`features/cloud/pairing-screen.tsx`). The hub's own
 * message never reaches the screen: it stays in the state's `message`, for
 * the console.
 */
export type PairingCause =
  /** This device may no longer open a numeric code (403 `pairing_window_closed`): it signed in too long ago. */
  | "windowClosed"
  /** The account has as many codes open as it may (429 `too_many_pairings`); each lapses after {@link DELIVERY_TIMEOUT_MS}. */
  | "tooMany"
  /** The code lapsed before anyone typed it (its pairing is gone: 404 `not_found`). */
  | "expired"
  /** The code's pairing took all the messages it may (429 `pairing_full`). */
  | "full"
  /** The code was typed, but did not check out, or no device confirmed it in time. */
  | "wrongCode"
  /** Keys arrived, but do not verify against the account's chain. */
  | "badDelivery"
  /** The hub holds this device back for a while (429 `rate`). */
  | "rate"
  /** The hub could not be reached, for longer than a moment. */
  | "offline"
  /** The hub answered with an error of its own (5xx), for longer than a moment. */
  | "unavailable"
  | "other";

/** What a failed request to the hub means for the pairing. */
export function pairingCause(error: unknown): PairingCause {
  if (isNetworkError(error)) return "offline";
  if (!(error instanceof HubApiError)) return "other";
  if (error.status === 404 && error.code === "not_found") return "expired";
  switch (hubErrorKind(error)) {
    case "pairingWindowClosed":
      return "windowClosed";
    case "tooManyPairings":
      return "tooMany";
    case "pairingFull":
      return "full";
    case "rate":
      return "rate";
    case "unavailable":
      return "unavailable";
    default:
      return "other";
  }
}

/** A cause that may pass by itself: the loop keeps waiting through it for a while. */
const passing = (cause: PairingCause) => cause === "offline" || cause === "unavailable" || cause === "rate";

/** How long the hub may stay unreachable (or unwell) before a waiting pairing gives up. */
export const TRANSIENT_GRACE_MS = 60_000;

/** The new device's side of the screen. */
export type ResponderState =
  | { kind: "idle" }
  | { kind: "offer"; text: string; fingerprint: string }
  | { kind: "opening" }
  /** `expiresMs`: when the code lapses at the hub, in this page's clock ({@link DELIVERY_TIMEOUT_MS} after it opened). */
  | { kind: "code"; code: string; fingerprint: string; expiresMs: number }
  | { kind: "answered"; code: string; fingerprint: string }
  | { kind: "verifying" }
  | { kind: "paired"; epochs: number[] }
  /** `message`: the error as it came (the hub's words, or the page's), for the console only. */
  | { kind: "failed"; reason: "codeFailed" | "badDelivery" | "error"; cause: PairingCause; message: string; retryAfterMs?: number };

const messageOf = (error: unknown) => (error instanceof Error ? error.message : String(error));
const retryAfterOf = (error: unknown) => (error instanceof HubApiError ? error.retryAfterMs : undefined);

export interface ResponderDeps {
  api: Pick<HubApi, "mailbox" | "ackMail" | "openPairing" | "pairingMessages" | "postPairingMessage" | "closePairing">;
  token: string;
  account: string;
  device: string;
  signPublic: Bytes;
  noise: NoiseKeyPair;
  /** The chain fetched and verified from genesis (genesis pinned), as it stands now. */
  chain(): Promise<ChainState | null>;
  /** Keep the delivered `UK` epochs (encrypted at rest); the caller zeroes them after. */
  storeUserKeys(keys: { epoch: number; key: Bytes }[]): Promise<void>;
  pollMs?: number;
  /** How many times to re-fetch a chain that does not list this device yet. */
  chainRetries?: number;
  /** How long to wait for a delivery after answering a code (a wrong code never brings one). */
  deliveryTimeoutMs?: number;
  /**
   * Only for a device that pinned the account before (it missed a
   * rotation's mail): take the keys mailed to it (purpose 1) and say which
   * epochs it now holds, `null` if none. Never given to a browser that has not
   * pinned the account — it takes keys only from a delivery MAC'd with this
   * screen's secret.
   */
  adopted?: () => Promise<number[] | null>;
  now?: () => number;
}

/** The pairing rendezvous's own lifetime: no delivery can come later than this. */
export const DELIVERY_TIMEOUT_MS = 10 * 60 * 1000;

/** The new device: offers itself, answers one code run, waits for its delivery. */
export class PairingResponder {
  state: ResponderState = { kind: "idle" };
  private readonly listeners = new Set<(s: ResponderState) => void>();
  /** The secrets a delivery may be MAC'd with: the QR's `s`, a code run's pairing secret. */
  private secrets: Bytes[] = [];
  private pairing: string | null = null;
  private after = 0;
  private answered = false;
  private running = false;
  private stopped = false;
  private fingerprint = "";
  private qrText: string | null = null;
  private code: string | null = null;
  private codeExpiresMs = 0;
  private opening: Promise<string> | null = null;
  private answeredAt = 0;
  /** Since when the hub has been unreachable (or unwell) without a break: 0 while it answers. */
  private troubleSince = 0;
  private readonly serial = new Serial();

  constructor(private readonly deps: ResponderDeps) {}

  onState(fn: (s: ResponderState) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private set(next: ResponderState): void {
    this.state = next;
    for (const fn of [...this.listeners]) fn(next);
  }

  private done(): boolean {
    return this.stopped || this.state.kind === "paired" || this.state.kind === "failed";
  }

  /** Show the QR code / text: a fresh `s`, kept in memory only. */
  async startOffer(): Promise<string> {
    this.fingerprint ||= await deviceFingerprint(this.deps.signPublic, this.deps.noise.publicKey);
    if (!this.qrText) {
      const secret = crypto.getRandomValues(new Uint8Array(32));
      this.secrets.push(secret);
      const offer: Offer = { account: this.deps.account, device: this.deps.device, signKey: this.deps.signPublic, dhKey: this.deps.noise.publicKey, secret };
      this.qrText = offerText(offer);
    }
    if (!this.answered) this.set({ kind: "offer", text: this.qrText, fingerprint: this.fingerprint });
    this.loop();
    return this.qrText;
  }

  /** Whether a code is open at the hub for this responder, on screen or not (behind the QR code tab): `startCode` shows it again. */
  get holdsCode(): boolean {
    return this.pairing !== null && this.code !== null && !this.done();
  }

  /** Show a numeric code instead: one pairing, one run (the one already open — or being opened — shown again, if there is one). */
  startCode(): Promise<string> {
    if (this.pairing && this.code) {
      if (!this.done()) this.showCode();
      return Promise.resolve(this.code);
    }
    this.opening ??= this.openCode().finally(() => {
      this.opening = null;
    });
    return this.opening;
  }

  private async openCode(): Promise<string> {
    this.fingerprint ||= await deviceFingerprint(this.deps.signPublic, this.deps.noise.publicKey);
    this.set({ kind: "opening" });
    const code = newPairingCode();
    try {
      this.pairing = await this.deps.api.openPairing(this.deps.token, this.deps.device);
    } catch (error) {
      this.fail("error", pairingCause(error), messageOf(error), retryAfterOf(error));
      throw error;
    }
    if (this.stopped) {
      // The screen went away while the hub opened it: nobody will show this code.
      const pairing = this.pairing;
      this.pairing = null;
      void this.deps.api.closePairing(this.deps.token, pairing).catch(() => undefined);
      return code;
    }
    this.code = code;
    this.codeExpiresMs = (this.deps.now ?? Date.now)() + DELIVERY_TIMEOUT_MS;
    this.showCode();
    this.loop();
    return code;
  }

  private showCode(): void {
    const code = this.code!;
    this.set(this.answered ? { kind: "answered", code, fingerprint: this.fingerprint } : { kind: "code", code, fingerprint: this.fingerprint, expiresMs: this.codeExpiresMs });
  }

  stop(): void {
    this.stopped = true;
    const pairing = this.pairing;
    if (pairing && this.state.kind !== "paired") void this.deps.api.closePairing(this.deps.token, pairing).catch(() => undefined);
    for (const s of this.secrets) s.fill(0);
    this.secrets = [];
  }

  private fail(reason: "codeFailed" | "badDelivery" | "error", cause: PairingCause, message: string, retryAfterMs?: number): void {
    if (this.done()) return;
    console.warn(`pairing: ${cause}: ${message}`);
    this.set(retryAfterMs === undefined ? { kind: "failed", reason, cause, message } : { kind: "failed", reason, cause, message, retryAfterMs });
    this.stop();
  }

  private loop(): void {
    if (this.running) return;
    this.running = true;
    void (async () => {
      while (!this.done()) {
        let wait = this.deps.pollMs ?? 2000;
        try {
          await this.tick();
          this.troubleSince = 0;
        } catch (error) {
          // A moment offline (or the hub unwell, or holding this device
          // back) is waited through: what was shown still works, and a QR
          // code already read on the other device must not be thrown away.
          const cause = pairingCause(error);
          const now = (this.deps.now ?? Date.now)();
          this.troubleSince ||= now;
          if (passing(cause) && now - this.troubleSince < TRANSIENT_GRACE_MS) {
            console.warn(`pairing: ${cause}, still waiting: ${messageOf(error)}`);
            wait = Math.max(wait, retryAfterOf(error) ?? 0);
          } else this.fail("error", cause, messageOf(error), retryAfterOf(error));
        }
        if (this.done()) break;
        await sleep(wait);
      }
      this.running = false;
    })();
  }

  /** One round: the mailbox first (a delivery ends it all), then the code run. Serialised. */
  tick(): Promise<void> {
    return this.serial.run(() => this.round());
  }

  private async round(): Promise<void> {
    if (this.done()) return;
    if (await this.checkMail()) return;
    if (this.deps.adopted) {
      const epochs = await this.deps.adopted();
      if (epochs?.length) {
        const pairing = this.pairing;
        this.pairing = null;
        if (pairing) await this.deps.api.closePairing(this.deps.token, pairing).catch(() => undefined);
        this.set({ kind: "paired", epochs });
        for (const s of this.secrets) s.fill(0);
        this.secrets = [];
        return;
      }
    }
    const now = (this.deps.now ?? Date.now)();
    if (this.answered && now > this.answeredAt + (this.deps.deliveryTimeoutMs ?? DELIVERY_TIMEOUT_MS)) {
      return this.fail("codeFailed", "wrongCode", "no device sent this device its keys: if the code was wrong, start again");
    }
    if (!this.pairing || this.answered || this.state.kind === "verifying") return;
    let messages;
    try {
      messages = await this.deps.api.pairingMessages(this.deps.token, this.pairing, this.after);
    } catch (error) {
      if (error instanceof HubApiError && error.status === 404) {
        // Expired before anyone typed the code.
        this.pairing = null;
        return this.fail("codeFailed", "expired", "the code expired or was not accepted");
      }
      throw error;
    }
    for (const m of messages) {
      this.after = Math.max(this.after, m.n);
      if (m.from !== "initiator" || this.answered) continue;
      // One run per code: whatever happens next, this code is spent.
      this.answered = true;
      this.answeredAt = (this.deps.now ?? Date.now)();
      let hello: PakeHello;
      try {
        hello = decodePakeHello(m.body);
        const ci = channelId(this.deps.account, this.deps.device);
        const run = await Cpace.start("responder", utf8(this.code!), ci, hello.sid);
        const keys = await run.finish(hello.share);
        const answer = await pakeAnswer(hello, run.share, this.deps.device, this.deps.signPublic, this.deps.noise.publicKey, keys);
        await this.deps.api.postPairingMessage(this.deps.token, this.pairing, "responder", encodePakeAnswer(answer));
        this.secrets.push(keys.pairSecret);
        keys.confirmInitiator.fill(0);
        keys.confirmResponder.fill(0);
      } catch (error) {
        // The hub's refusal of the answer (the code lapsed, the pairing took all it may, offline), or the run itself.
        const cause = error instanceof HubApiError || isNetworkError(error) ? pairingCause(error) : "wrongCode";
        return this.fail("codeFailed", cause, messageOf(error), retryAfterOf(error));
      }
      this.set({ kind: "answered", code: this.code!, fingerprint: this.fingerprint });
    }
  }

  /** A delivery whose MAC checks with one of this page's secrets: verify it all, keep the keys, say "paired". */
  private async checkMail(): Promise<boolean> {
    const mail = await this.deps.api.mailbox(this.deps.token);
    for (const m of mail) {
      if (m.purpose !== MAIL_PAIRING) {
        // Keys mailed to a device pairing again, and other devices' requests,
        // are for once it is in (`receiveMail`); a purpose no page knows —
        // or a machine's key, never a device's — is acknowledged, not left
        // to fill the mailbox (relay-and-plans §6).
        if (m.purpose === MAIL_MACHINE_KEY || !knownMailPurpose(m.purpose)) await this.deps.api.ackMail(this.deps.token, m.id).catch(() => undefined);
        continue;
      }
      let delivery: Delivery;
      try {
        delivery = decodeDelivery(m.sealed);
      } catch {
        await this.deps.api.ackMail(this.deps.token, m.id);
        continue;
      }
      let secret: Bytes | null = null;
      for (const s of this.secrets) {
        try {
          await checkDeliveryTag(delivery, s);
          secret = s;
          break;
        } catch {
          // another secret's
        }
      }
      if (!secret) {
        // Made for an earlier screen (or by nobody who saw this one): of no use.
        await this.deps.api.ackMail(this.deps.token, m.id);
        continue;
      }
      this.set({ kind: "verifying" });
      let keys: { epoch: number; key: Bytes }[] | null = null;
      try {
        // The device that signed "add device" mailed after appending it; a
        // hub slow to serve that entry gets a few more tries.
        const retries = this.deps.chainRetries ?? 5;
        for (let attempt = 0; ; attempt++) {
          const chain = await this.deps.chain();
          if (!chain) throw new PairingError("notListed", "the account has no chain");
          try {
            keys = await openDelivery(delivery, secret, chain, this.deps.device, this.deps.signPublic, this.deps.noise);
            break;
          } catch (error) {
            if (!(error instanceof PairingError && error.kind === "notListed") || attempt >= retries) throw error;
            await sleep(this.deps.pollMs ?? 2000);
          }
        }
        await this.deps.storeUserKeys(keys);
      } catch (error) {
        await this.deps.api.ackMail(this.deps.token, m.id).catch(() => undefined);
        this.fail("badDelivery", "badDelivery", messageOf(error));
        return true;
      } finally {
        for (const k of keys ?? []) k.key.fill(0);
      }
      await this.deps.api.ackMail(this.deps.token, m.id);
      const pairing = this.pairing;
      this.pairing = null;
      if (pairing) await this.deps.api.closePairing(this.deps.token, pairing).catch(() => undefined);
      this.set({ kind: "paired", epochs: keys.map((k) => k.epoch) });
      for (const s of this.secrets) s.fill(0);
      this.secrets = [];
      return true;
    }
    return false;
  }
}

/** A device to add, as the existing device has it: from an offer, or from a code run. */
export interface Candidate {
  device: string;
  signKey: Bytes;
  dhKey: Bytes;
  /** What the delivery is MAC'd with: the offer's `s`, or the run's pairing secret. */
  secret: Bytes;
  fingerprint: string;
}

/** The pasted or scanned text as a device to add to `chain`'s account. */
export async function candidateFromOffer(text: string, chain: ChainState): Promise<Candidate> {
  const offer = offerFromText(text);
  if (offer.account !== chain.account) throw new PairingError("wrongAccount", "this pairing code is for another account");
  // Already added with these very keys: it is pairing again (its keys never arrived, or it never pinned the account).
  // (Chain v3: a device that does not manage the account pairs again too; a host never pairs.)
  const listed = chain.deviceRecord(offer.device);
  const same = !!listed && equal(listed.info.signKey, offer.signKey) && equal(listed.info.dhKey, offer.dhKey);
  const again = listed?.status.type === "active" && !listed.host && same;
  // Waiting after it joined with a passkey: approved by this pairing — only with the keys it joined with (`CloudAccount.pairDevice`).
  const joining = listed?.status.type === "pending" && listed.viaPasskey !== null;
  if (joining && !same) throw new PairingError("badOffer", "the device that showed this code is not the one that asked to join with a passkey: nothing was approved");
  if ((listed && !again && !joining) || chain.machine(offer.device)) throw new PairingError("badOffer", `${offer.device} is already in the chain`);
  return { device: offer.device, signKey: offer.signKey, dhKey: offer.dhKey, secret: offer.secret, fingerprint: await deviceFingerprint(offer.signKey, offer.dhKey) };
}

export interface InitiatorDeps {
  api: Pick<HubApi, "postPairingMessage" | "pairingMessages">;
  token: string;
  account: string;
  pollMs?: number;
  timeoutMs?: number;
  signal?: AbortSignal;
}

/**
 * Run CPace as the initiator against `pairing` with the code the user typed,
 * and check the answer. One guess per code: the new device answers only the
 * first hello, so a wrong code ends this run here, and the new device, which
 * never gets a delivery, gives up and shows a fresh code. The pairing is the
 * new device's to close.
 */
export async function runCodeInitiator(deps: InitiatorDeps, pairing: PairingSummary, code: string): Promise<Candidate> {
  if (!/^\d{8}$/.test(code)) throw new PairingError("badAnswer", "a pairing code is 8 digits");
  const sid = newSid();
  const run = await Cpace.start("initiator", utf8(code), channelId(deps.account, pairing.device), sid);
  const hello: PakeHello = { sid, share: run.share };
  await deps.api.postPairingMessage(deps.token, pairing.id, "initiator", encodePakeHello(hello));
  const deadline = Date.now() + (deps.timeoutMs ?? 120_000);
  let after = 0;
  for (;;) {
    if (deps.signal?.aborted) throw deps.signal.reason ?? new DOMException("aborted", "AbortError");
    const messages = await deps.api.pairingMessages(deps.token, pairing.id, after);
    const reply = messages.find((m) => m.from === "responder");
    for (const m of messages) after = Math.max(after, m.n);
    if (reply) {
      // The answer is read: the run is over either way.
      const answer = decodePakeAnswer(reply.body);
      const keys = await run.finish(answer.share);
      await verifyPakeAnswer(answer, hello, keys);
      if (answer.device !== pairing.device) throw new PairingError("badAnswer", "the answer names another device than the pairing");
      keys.confirmInitiator.fill(0);
      keys.confirmResponder.fill(0);
      return {
        device: answer.device,
        signKey: answer.signKey,
        dhKey: answer.dhKey,
        secret: keys.pairSecret,
        fingerprint: await deviceFingerprint(answer.signKey, answer.dhKey),
      };
    }
    if (Date.now() > deadline) throw new PairingError("badAnswer", "the new device did not answer");
    await sleep(deps.pollMs ?? 1000);
  }
}

export interface CompleteDeps {
  api: Pick<HubApi, "mail">;
  token: string;
  /** This (full) device. */
  device: string;
  signPublic: Bytes;
  noise: NoiseKeyPair;
  chain(): ChainState;
  /** Sign and append one entry as this device (the addition: `joinAction`'s). */
  append(action: Action): Promise<void>;
  /** Every `UK` epoch held, as bytes for the length of `use`. */
  withUserKeys<T>(use: (keys: { epoch: number; key: Bytes }[]) => Promise<T>): Promise<T>;
}

/**
 * Sign "add device" for exactly the candidate's keys, then mail it every
 * `UK` epoch in a pairing delivery, MAC'd with its secret. For a device the
 * chain lists already with these keys (one pairing again: its keys never
 * arrived, or it never pinned the account) nothing is added twice: the
 * delivery goes again, MAC'd with the secret of the screen it shows now.
 * A delivery that does not go out (the hub's mailbox caps among other
 * things) rejects; the new device can pair again.
 *
 * `opts.admin` (`joinAction`, the default): whether the device manages the
 * account — one that does not exists only on a chain that holds a v3 entry.
 * One that does not gets a delivery with no user key at all (it pins the
 * account all the same) — the machine keys of the hosts that accept it
 * follow by mail (purpose 5).
 */
export async function completePairing(deps: CompleteDeps, c: Candidate, opts: { admin?: boolean } = {}): Promise<"delivered" | "redelivered"> {
  try {
    const listed = deps.chain().deviceRecord(c.device);
    // Pairing again: the device it is (never a host), with these very keys.
    if (listed && (listed.status.type !== "active" || listed.host || !equal(listed.info.signKey, c.signKey) || !equal(listed.info.dhKey, c.dhKey))) {
      throw new PairingError("badOffer", `${c.device} is in the chain already, but not as this device`);
    }
    if (!listed) await deps.append(joinAction({ id: c.device, signKey: c.signKey, dhKey: c.dhKey }, { host: false, admin: opts.admin ?? true }, deps.chain()));
    // User keys only to a device that manages the account (§3.3).
    const admin = deps.chain().admin(c.device) !== undefined;
    const delivery = admin
      ? await deps.withUserKeys((keys) => createDelivery(deps.chain(), deps.device, deps.signPublic, deps.noise, c.device, c.dhKey, keys, c.secret))
      : await createDelivery(deps.chain(), deps.device, deps.signPublic, deps.noise, c.device, c.dhKey, [], c.secret);
    await deps.api.mail(deps.token, { recipient: c.device, sender: deps.device, purpose: MAIL_PAIRING, epoch: 0, sealed: encodeDelivery(delivery) });
    return listed ? "redelivered" : "delivered";
  } finally {
    c.secret.fill(0);
  }
}

/** Give up on a candidate without adding it (its secret forgotten; the pairing is the new device's to close). */
export function abandonCandidate(c: Candidate): void {
  c.secret.fill(0);
}
