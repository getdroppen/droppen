/*
 * Handing a machine its key (architecture doc §5 "钥匙投递", §6 step 3).
 *
 * Inside the end-to-end channel first: `POST /rh/machine-key {epoch, key,
 * reencrypt}` on an `rpc` pipe to the agent's own routes, from this (full)
 * device — the agent answers 200 `{mkEpoch, changed}`, 409 for an older epoch
 * or a different key for one it holds, 403 unless the channel's device is a
 * current full device. Only when the machine is not online within a few
 * seconds (or the channel cannot be opened) does the key go to the mailbox,
 * sealed from this device's static key to the agent's (purpose 2), which the
 * agent takes when it next connects. Either way the agent accepts it only from
 * a current full device of its own verified chain.
 */
import { type Bytes, equal, fromHex, hex, machineKeyCommit, type NoiseKeyPair, sealTo } from "@/lib/e2e";
import { type PipeOpener, tunnelTransport } from "@/lib/transport/tunnel";
import type { CloudAccount } from "./account";
import { type HubApi, MAIL_MACHINE_KEY } from "./api";
import type { HubLink } from "./link";
import { ChannelIsolated, MachineChannel } from "./machine";
import type { RelayPool } from "./relays";

/** How long to wait for a machine to come online before using the mailbox. */
export const ONLINE_WAIT_MS = 10_000;
/**
 * How long to keep trying a channel the agent refuses as stale: right after
 * a chain change (a removal) it opens none until another full device has
 * attested the new head, which they do within seconds of seeing it.
 */
export const STALE_GRACE_MS = 15_000;
/** How long one in-channel delivery may take, the stale grace and handshakes included. */
export const CHANNEL_TIMEOUT_MS = 40_000;

/**
 * Why a key could not go in the channel: the machine is `offline` (the hub
 * says so); the agent would not open a channel yet because no other device
 * has `unconfirmed` the chain's new head; or the channel `failed` otherwise.
 */
export class ChannelUnavailable extends Error {
  constructor(
    readonly reason: "offline" | "unconfirmed" | "failed",
    message: string,
  ) {
    super(message);
    this.name = "ChannelUnavailable";
  }
}

export interface MachineKeyCourier {
  /** `true` once `machine` is online (now, or within `timeoutMs`), else `false`. */
  waitOnline(machine: string, timeoutMs: number): Promise<boolean>;
  /** `POST /rh/machine-key` over the channel to `machine`: the agent's status and JSON answer. */
  postInChannel(machine: string, body: { epoch: number; key: string; reencrypt: boolean }): Promise<{ status: number; body: unknown }>;
  /** The sealed mailbox fallback. */
  mail(machine: string, epoch: number, mk: Uint8Array): Promise<void>;
}

/**
 * `POST /rh/rotate`'s body (docs/host-identity-rotation.md §3.2; the agent's
 * routes are camelCase, binary as hex): the request's 16-byte nonce, the
 * admin's ephemeral X25519 key the answer's MAC is made under, the chain view
 * the proof is about, and how the renewal is confirmed.
 */
export interface RotateRequest {
  nonce: string;
  eph: string;
  chainSeq: number;
  chainHead: string;
  assurance: 0 | 1;
}

/** A {@link MachineKeyCourier} that also asks a host to renew its identity key, and vouches for the chain head (`ATTEST`). */
export interface RenewalCourier extends MachineKeyCourier {
  /** `POST /rh/rotate` over the channel to `machine` (never by mailbox): its status and JSON answer. `ChannelUnavailable` when it cannot be reached. */
  rotate(machine: string, body: RotateRequest): Promise<{ status: number; body: unknown }>;
  /** Vouch for the head this device sees now (an agent's admin routes want a recent word on it). */
  attest(): Promise<void>;
}

export type DeliveryResult =
  | { via: "channel"; changed: boolean }
  /** `pending`: a re-encryption was asked for and is noted, to be asked again in the channel (`flushPendingReencrypts`). */
  | { via: "mailbox"; reason: "offline" | "unconfirmed" | "failed"; pending?: boolean }
  /** `code`: the agent's reason (`limited_device`: this device does not manage the account). */
  | { via: "refused"; status: number; message: string; code?: string };

function withTimeout<T>(work: Promise<T>, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("timed out")), ms);
    work.then(
      (v) => {
        clearTimeout(timer);
        resolve(v);
      },
      (e) => {
        clearTimeout(timer);
        reject(e);
      },
    );
  });
}

/**
 * Deliver `mk` (`MK` epoch `epoch` of `machine`): in the channel if the
 * machine is online within `onlineWaitMs`, else (or if the channel fails) by
 * mailbox. An agent that refuses it in the channel (409: it holds a newer
 * epoch, or another key for this one; 403: this device may not) would refuse
 * it from the mailbox too, so that is reported, not retried. One that will
 * not take it in the channel yet (412 `stale`: no device that manages the
 * account vouched for its view lately) gets it by mailbox (`unconfirmed`).
 */
export async function deliverMachineKey(
  courier: MachineKeyCourier,
  machine: string,
  epoch: number,
  mk: Uint8Array,
  reencrypt: boolean,
  opts: { onlineWaitMs?: number; channelTimeoutMs?: number } = {},
): Promise<DeliveryResult> {
  const online = await courier.waitOnline(machine, opts.onlineWaitMs ?? ONLINE_WAIT_MS);
  if (online) {
    let reason: "offline" | "unconfirmed" | "failed" = "failed";
    try {
      const res = await withTimeout(courier.postInChannel(machine, { epoch, key: hex(mk), reencrypt }), opts.channelTimeoutMs ?? CHANNEL_TIMEOUT_MS);
      if (res.status === 200) return { via: "channel", changed: (res.body as { changed?: unknown } | null)?.changed !== false };
      if (res.status === 409 || res.status === 403) {
        const body = res.body as { message?: unknown; error?: unknown } | null;
        const message = body?.message;
        const code = typeof body?.error === "string" ? { code: body.error } : {};
        return { via: "refused", status: res.status, message: typeof message === "string" ? message : `refused (${res.status})`, ...code };
      }
      // 412 `stale`: the agent will not take a key in the channel until a device that manages the
      // account vouches for its view (or its view catches up): the mailbox carries it meanwhile.
      if (res.status === 412) reason = "unconfirmed";
    } catch (error) {
      // the channel did not open, or broke: the mailbox it is
      if (error instanceof ChannelUnavailable) reason = error.reason;
    }
    await courier.mail(machine, epoch, mk);
    return { via: "mailbox", reason };
  }
  await courier.mail(machine, epoch, mk);
  return { via: "mailbox", reason: "offline" };
}

/** `POST /rh/machine-key` on an `rpc` pipe of the channel `opener` opens: the agent's status and JSON answer. */
export function postMachineKey(opener: PipeOpener, body: { epoch: number; key: string; reencrypt: boolean }): Promise<{ status: number; body: unknown }> {
  return postJson(opener, "/rh/machine-key", body);
}

/** `POST /rh/rotate` on an `rpc` pipe of the channel `opener` opens (docs/host-identity-rotation.md §3.2): the agent's status and JSON answer. */
export function postRotate(opener: PipeOpener, body: RotateRequest): Promise<{ status: number; body: unknown }> {
  return postJson(opener, "/rh/rotate", body);
}

async function postJson(opener: PipeOpener, path: string, body: unknown): Promise<{ status: number; body: unknown }> {
  const res = await tunnelTransport(opener).fetch(
    new Request(`http://machine${path}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
  );
  let json: unknown = null;
  try {
    json = await res.json();
  } catch {
    // no body
  }
  return { status: res.status, body: json };
}

/**
 * Re-encryptions asked for that did not reach the agent in the channel (the
 * key went by mailbox, which carries no such request), pinned to exactly the
 * key they are for: the machine, its `MK` epoch, the `UK` epoch its keyring
 * blob is wrapped under, and the chain's commitment to the key (hex). Nothing
 * secret — the key itself is in the keyring. Kept in this browser until the
 * next channel to that machine asks again; at that flush the key read back
 * must match both the pinned commitment and the chain's.
 */
export interface PendingReencrypt {
  machine: string;
  mkEpoch: number;
  ukEpoch: number;
  /** `machineKeyCommit(account, machine, mkEpoch, MK)`, hex. */
  mkHash: string;
}

const pendingKey = (account: string) => `rh.hub.reencrypt.v2.${account}`;
const pendingMemory = new Map<string, PendingReencrypt[]>();

function storage(): Pick<Storage, "getItem" | "setItem" | "removeItem"> | null {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

function isPending(p: unknown): p is PendingReencrypt {
  const x = p as Partial<PendingReencrypt> | null;
  return (
    !!x &&
    typeof x.machine === "string" &&
    Number.isSafeInteger(x.mkEpoch) &&
    x.mkEpoch! >= 0 &&
    Number.isSafeInteger(x.ukEpoch) &&
    x.ukEpoch! >= 0 &&
    typeof x.mkHash === "string" &&
    /^[0-9a-f]{64}$/.test(x.mkHash)
  );
}

export function pendingReencrypts(account: string): PendingReencrypt[] {
  const s = storage();
  if (!s) return [...(pendingMemory.get(account) ?? [])];
  try {
    const list = JSON.parse(s.getItem(pendingKey(account)) ?? "[]") as unknown;
    return Array.isArray(list) ? list.filter(isPending) : [];
  } catch {
    return [];
  }
}

function savePending(account: string, list: PendingReencrypt[]): void {
  const s = storage();
  if (!s) {
    pendingMemory.set(account, list);
    return;
  }
  try {
    if (list.length) s.setItem(pendingKey(account), JSON.stringify(list));
    else s.removeItem(pendingKey(account));
  } catch {
    pendingMemory.set(account, list);
  }
}

/** Note a re-encryption to ask for again; only the newest epoch per machine matters. */
export function notePendingReencrypt(account: string, item: PendingReencrypt): void {
  savePending(account, [...pendingReencrypts(account).filter((p) => p.machine !== item.machine), item]);
}

export function clearPendingReencrypt(account: string, item: Pick<PendingReencrypt, "machine" | "mkEpoch">): void {
  savePending(
    account,
    pendingReencrypts(account).filter((p) => !(p.machine === item.machine && p.mkEpoch === item.mkEpoch)),
  );
}

/** Forget every noted re-encryption of `account` (a removed device's wipe). */
export function forgetPendingReencrypts(account: string): void {
  savePending(account, []);
}

/**
 * Ask machine `machine` again, in a channel that is open now, for the
 * re-encryptions noted for it: `POST /rh/machine-key {epoch, key,
 * reencrypt: true}` with the key read back from the keyring under the pinned
 * `UK` epoch (`keyBytes`, which also checks it against the chain's
 * commitment) — and only if it hashes to the pinned commitment too. A key
 * that does not is never posted. Cleared once the agent answers 200, or a
 * 409 saying it already holds a newer epoch (re-encrypting under this one is
 * moot) — and only then (M2 re-review): a 409 `uncommitted` means the
 * agent's chain is behind (a hub can hold back the commitment to make it say
 * so), and `key_mismatch` that its chain disagrees; either way the request
 * stays, to be asked again in the next channel. Returns how many were asked.
 */
export async function flushPendingReencrypts(
  account: string,
  machine: string,
  keyBytes: (item: PendingReencrypt) => Promise<Bytes | null>,
  post: (body: { epoch: number; key: string; reencrypt: boolean }) => Promise<{ status: number; body: unknown }>,
): Promise<number> {
  let asked = 0;
  for (const item of pendingReencrypts(account).filter((p) => p.machine === machine)) {
    const key = await keyBytes(item);
    if (!key) continue;
    try {
      if (!equal(await machineKeyCommit(account, machine, item.mkEpoch, key), fromHex(item.mkHash))) {
        console.warn(`cloud: the key for ${machine} epoch ${item.mkEpoch} is not the one noted: not asking for a re-encryption under it`);
        continue;
      }
      const res = await post({ epoch: item.mkEpoch, key: hex(key), reencrypt: true });
      if (reencryptSettled(res, item.mkEpoch)) clearPendingReencrypt(account, item);
      else if (res.status === 409) console.warn(`cloud: ${machine} did not take the re-encryption under epoch ${item.mkEpoch} yet (${conflictCode(res.body) ?? "conflict"}): asking again later`);
      asked++;
    } finally {
      key.fill(0);
    }
  }
  return asked;
}

/** The error code of an agent's JSON answer, if it has one. */
function conflictCode(body: unknown): string | null {
  const code = (body as { error?: unknown } | null)?.error;
  return typeof code === "string" ? code : null;
}

/**
 * Whether the agent's answer to a re-encryption request settles it: it took
 * it (200), or it refused it only because it holds a newer key epoch than
 * `mkEpoch` already (409 with `mkEpoch` past it). Any other 409 — `uncommitted`
 * (its chain is behind), `key_mismatch` (its chain disagrees) — does not.
 */
export function reencryptSettled(res: { status: number; body: unknown }, mkEpoch: number): boolean {
  if (res.status === 200) return true;
  if (res.status !== 409) return false;
  const held = (res.body as { mkEpoch?: unknown } | null)?.mkEpoch;
  return typeof held === "number" && held > mkEpoch;
}

/**
 * Where a machine's agent stands against the chain's newest committed key
 * epoch for it: `current`; `updated` (it was behind and took the committed
 * key in the channel just now); `behind` (it still uses an old key and could
 * not be given the new one — "open it to update"); `unknown` (an agent that
 * does not say).
 */
export type AgentKeyState = "current" | "updated" | "behind" | "unknown";

/** `GET /rh/info` over the channel: the agent's own `mkEpoch` (`undefined`: it does not say). */
export async function agentKeyEpoch(opener: PipeOpener): Promise<number | null | undefined> {
  const res = await tunnelTransport(opener).fetch(new Request("http://machine/rh/info", { method: "GET" }));
  if (!res.ok) return undefined;
  const body = (await res.json().catch(() => null)) as { mkEpoch?: unknown } | null;
  if (!body || !("mkEpoch" in body)) return undefined;
  const e = body.mkEpoch;
  return e === null ? null : typeof e === "number" && Number.isSafeInteger(e) && e >= 0 ? e : undefined;
}

/**
 * After a channel opens: compare the agent's key epoch with the chain's
 * newest committed one for it (`latest`), and if the agent is behind, hand
 * it the committed key in the channel (`keyBytes` reads it back from the
 * keyring, checked against the chain).
 */
export async function checkAgentKey(opts: {
  latest: number | null;
  agentEpoch: () => Promise<number | null | undefined>;
  keyBytes: (mkEpoch: number) => Promise<Bytes | null>;
  post: (body: { epoch: number; key: string; reencrypt: boolean }) => Promise<{ status: number; body: unknown }>;
}): Promise<AgentKeyState> {
  if (opts.latest === null) return "current";
  let have: number | null | undefined;
  try {
    have = await opts.agentEpoch();
  } catch {
    return "unknown";
  }
  if (have === undefined) return "unknown";
  if (have !== null && have >= opts.latest) return "current";
  const key = await opts.keyBytes(opts.latest).catch(() => null);
  if (!key) return "behind";
  try {
    const res = await opts.post({ epoch: opts.latest, key: hex(key), reencrypt: false });
    return res.status === 200 ? "updated" : "behind";
  } catch {
    return "behind";
  } finally {
    key.fill(0);
  }
}

const agentKeys = new Map<string, AgentKeyState>();
const agentKeyListeners = new Set<() => void>();

/** What the last check said about `machine`'s agent (`undefined`: not checked on this page). */
export function agentKeyState(machine: string): AgentKeyState | undefined {
  return agentKeys.get(machine);
}

export function noteAgentKeyState(machine: string, state: AgentKeyState): void {
  if (agentKeys.get(machine) === state) return;
  agentKeys.set(machine, state);
  for (const fn of [...agentKeyListeners]) fn();
}

export function onAgentKeyState(fn: () => void): () => void {
  agentKeyListeners.add(fn);
  return () => agentKeyListeners.delete(fn);
}

/** Seal `mk` from this device to `machine`'s agent and leave it in the machine's mailbox (purpose 2). */
export async function mailMachineKey(
  api: Pick<HubApi, "mail">,
  token: string,
  account: string,
  sender: string,
  senderKeys: NoiseKeyPair,
  machine: string,
  machineDh: Uint8Array,
  epoch: number,
  mk: Uint8Array,
): Promise<void> {
  const sealed: Bytes = await sealTo(senderKeys, machineDh, { purpose: "machineKey", account, sender, recipient: machine, epoch }, mk);
  await api.mail(token, { recipient: machine, sender, purpose: MAIL_MACHINE_KEY, epoch, sealed });
}

/**
 * The real courier: presence from the page's hub link, the page's own open
 * channel to the machine if it has one (`open(machine)`) and it goes to the
 * key the chain names now, else a channel opened for the delivery and closed
 * after it, and the account's mailbox. It asks a host to renew its identity
 * key the same way (never by mailbox), and vouches for the head on the link.
 */
export function linkCourier(
  link: HubLink,
  account: CloudAccount,
  open?: (machine: string) => MachineChannel | null | undefined,
  relays?: RelayPool | null,
): RenewalCourier {
  /**
   * `post` over a channel to `machine`: the page's own if one is open to the
   * DH key the chain has for it now, else a new one. A stale refusal right
   * after a chain change passes once another device attests the new head:
   * sync, attest again (the channel does), and retry for a while before
   * giving up on the channel. A channel already open was authorised before
   * the change and still is (the agent closes only a removed device's): it
   * needs no new handshake, so no fresh attestation — unless the host
   * renewed its identity key, whose old channels lead nowhere.
   */
  async function inChannel(machine: string, post: (opener: PipeOpener) => Promise<{ status: number; body: unknown }>, opts: { isolatedRenewal?: boolean } = {}): Promise<{ status: number; body: unknown }> {
    const current = open?.(machine);
    const now = account.machine(machine)?.dhKey;
    const to = current?.responderKey;
    if (!opts.isolatedRenewal && current && current.machine === machine && current.status === "open" && (!to || !now || equal(to, now))) {
      try {
        // No key leaves a page whose view forked (`CloudAccount.forked`), however long it waited to get here.
        account.assertNotForked();
        return await post(current);
      } catch (error) {
        if (account.forked !== null) throw error;
        // it went away meanwhile: a new one, below
      }
    }
    const deadline = Date.now() + STALE_GRACE_MS;
    for (;;) {
      const channel = new MachineChannel(link, account, machine, { relays: relays ?? undefined, isolatedRenewal: opts.isolatedRenewal });
      try {
        account.assertNotForked();
        return await post(channel);
      } catch (error) {
        if (account.forked !== null) throw error;
        if (channel.status === "offline") throw new ChannelUnavailable("offline", channel.error ?? "the device is offline");
        // Isolated (§3.8): no channel goes to it but a renewal confirmed on the device's, and waiting does not change that.
        if (error instanceof ChannelIsolated) throw new ChannelUnavailable("failed", error.message);
        const unconfirmed = channel.lastRefusal === "stale";
        if (Date.now() >= deadline) {
          throw new ChannelUnavailable(unconfirmed ? "unconfirmed" : "failed", channel.error ?? (error instanceof Error ? error.message : String(error)));
        }
      } finally {
        channel.close();
      }
      await new Promise((resolve) => setTimeout(resolve, 2000));
      try {
        await account.sync();
      } catch {
        // the next try
      }
    }
  }
  return {
    waitOnline(machine, timeoutMs) {
      if (link.presence.get(machine)?.state === "online") return Promise.resolve(true);
      return new Promise((resolve) => {
        const timer = setTimeout(() => {
          off();
          resolve(false);
        }, timeoutMs);
        const off = link.onPresence((p) => {
          if (p.get(machine)?.state !== "online") return;
          clearTimeout(timer);
          off();
          resolve(true);
        });
      });
    },
    postInChannel(machine, body) {
      return inChannel(machine, (opener) => postMachineKey(opener, body));
    },
    rotate(machine, body) {
      if (link.presence.get(machine)?.state === "offline") return Promise.reject(new ChannelUnavailable("offline", "the device is offline"));
      // An isolated host is reached for a renewal confirmed on the device only — on a channel of its own, for this one call (§3.8).
      if (account.isolated?.(machine)) {
        if (body.assurance !== 1) return Promise.reject(new ChannelUnavailable("failed", `${machine} is isolated: only a renewal confirmed on the device reaches it`));
        return inChannel(machine, (opener) => postRotate(opener, body), { isolatedRenewal: true });
      }
      return inChannel(machine, (opener) => postRotate(opener, body));
    },
    async attest() {
      if (account.isAdmin) link.attest(await account.attestation());
    },
    async mail(machine, epoch, mk) {
      const info = account.machine(machine);
      if (!info) throw new Error(`no device ${machine} that runs AI in the chain`);
      account.assertNotForked();
      const api = {
        mail: (token: string, body: Parameters<HubApi["mail"]>[1]) => {
          account.assertNotForked();
          return account.api.mail(token, body);
        },
      };
      await mailMachineKey(api, account.session.token, account.account, account.deviceId, account.noiseKeys, machine, info.dhKey, epoch, mk);
    },
  };
}
