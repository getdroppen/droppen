// The hub's HTTP for the key-handling tests (docs/hub-api/http.md, enough of
// it): password sign-in to a short-lived login token, device sessions minted
// only by a device's signature over a challenge, devices, a chain the hub
// verifies as it appends (its own clock: the recovery delay), enrollments,
// the mailbox for machines and devices (sender = the session's full device,
// the recipient's own mail only, the caps), the write-once keyring (full
// devices only), and the numeric-code pairing rendezvous (who may do what).
// JSON both ways, binary as lower-case hex, as the real one.
import {
  type Action,
  ChainClock,
  ChainError,
  ChainState,
  decodeEntry,
  decodeHostKeyMail,
  encodeEntry,
  enrollProof,
  type EnrollCode,
  fromHex,
  generateDeviceKeys,
  hex,
  hostRotateBody,
  type KeyStore,
  MemoryKeyStore,
  type NoiseKeyPair,
  PASSKEY_DELAY_MS,
  passkeySlotProof,
  rotateConfirmFor,
  rotateSas,
  signDraft,
  signHostRotate,
  verifyHubLogin,
} from "../src/lib/e2e/index.js";
import { type AccountStorage, CloudAccount } from "../src/lib/hub/account.js";
import { HubApi } from "../src/lib/hub/api.js";
import type { MachineKeyCourier, RenewalCourier, RotateRequest } from "../src/lib/hub/deliver.js";
import { MemorySeenStore } from "../src/lib/hub/seen-store.js";

/** The hub origin the tests' pages dialled (what device-session signatures name). */
export const HUB_ORIGIN = "wss://hub.test";

export interface StoredMail {
  id: number;
  recipient: string;
  sender: string;
  purpose: number;
  epoch: number;
  sealed: string;
}

/** A browser's `localStorage`, one per key store (the same store is the same browser, across reloads). */
const storages = new WeakMap<KeyStore, AccountStorage & { values: Map<string, string> }>();

export function browserStorage(store: KeyStore): AccountStorage & { values: Map<string, string> } {
  let s = storages.get(store);
  if (!s) {
    const values = new Map<string, string>();
    s = { values, getItem: (k) => values.get(k) ?? null, setItem: (k, v) => void values.set(k, v), removeItem: (k) => void values.delete(k) };
    storages.set(store, s);
  }
  return s;
}

/** A browser's seen records (IndexedDB, in memory), one per key store as its `localStorage` is. */
const seenStores = new WeakMap<KeyStore, MemorySeenStore>();

export function browserSeen(store: KeyStore): MemorySeenStore {
  let s = seenStores.get(store);
  if (!s) {
    s = new MemorySeenStore();
    seenStores.set(store, s);
  }
  return s;
}

/** What a page of the browser of key store `store` opens the account with: its `localStorage` and its seen records. */
export function browserOf(store: KeyStore): { storage: AccountStorage; seen: MemorySeenStore } {
  return { storage: browserStorage(store), seen: browserSeen(store) };
}

type Who = { kind: "login" } | { kind: "device"; device: string };

export class FakeKeysHub {
  readonly account = "a-test";
  devices = new Map<string, { sign: string; dh: string }>();
  /** When each device registered (`POST /v1/devices`), as rh-hub keeps `created_ms`: its code-pairing window runs from here. */
  registeredMs = new Map<string, number>();
  /** Every `POST /v1/devices` taken, in order. */
  registrations: string[] = [];
  /** What each registration said it takes the place of (`replaces`), as rh-hub keeps it: another device of the account, or none. */
  replaces = new Map<string, string>();
  /** Devices signed in on `/v1/client` now, as `/v1/me` `online` says (set by the tests). */
  online = new Set<string>();
  /** Each device's browser by the hub's name for it (`/v1/me` `ua_label`; set by the tests). */
  uaLabels = new Map<string, string>();
  private deviceSeq = 0;
  /** A hub that lies: the id the next registration is given (one the chain may name already, with other keys). */
  nextDeviceId: string | null = null;
  machines = new Map<string, { sign: string; dh: string }>();
  chain: string[] = [];
  state: ChainState | null = null;
  clock = ChainClock.live(Date.now(), 0);
  /** The hub's own time (sessions and challenges). */
  now = () => Date.now();
  /** The hub chain's clock (the recovery delay runs on it). */
  chainNow = () => this.now();
  enrollments = new Map<string, { device: string; machine?: string; sign?: Uint8Array; dh?: Uint8Array; proof?: Uint8Array }>();
  mail: StoredMail[] = [];
  mailLog: StoredMail[] = [];
  private mailId = 0;
  keys = new Map<string, { blob: string; updated_ms: number; writer?: string }>();
  /** Answer keyring writes 507 `keyring_full`, as a hub at its cap does. */
  keyringFull = false;
  /** Accept keyring names as hubs before the M2 re-review did (any name, any epoch): for tests that plant blobs as a writer would have. */
  looseKeyNames = false;
  pairings = new Map<string, { device: string; created_ms: number; messages: { n: number; from: string; body: string }[] }>();
  private pairingId = 0;
  /** Live sessions: login tokens and device sessions. */
  sessions = new Map<string, Who & { expiresMs: number }>();
  private sessionId = 0;
  challenges = new Map<string, { device: string; expiresMs: number }>();
  /** How long a device session lives. */
  deviceSessionMs = 3_600_000;
  /** Refuse `GET /v1/keys` etc. like an older hub. */
  noKeyring = false;
  /** Awaited before each request is handled (`"<METHOD> <path>"`, its JSON body): where a test holds one back. */
  gate: ((request: string, body: unknown) => Promise<void>) | null = null;
  /** What `GET /v1/chain` serves instead of `chain`, when it says (a hub serving pages differently, a race). */
  chainServed: ((from: number) => Promise<string[] | undefined>) | null = null;
  /** Run after entries are appended and before the append is answered (the hub's push to subscribers lands first). */
  afterAppend: ((entries: string[]) => Promise<void>) | null = null;
  /** Keyring writes and mailbox posts, in the order they reached the hub. */
  requests: string[] = [];
  /** Refuse the next `n` mailbox posts (503). */
  failMail = 0;
  /** Session tokens refused outright (401), whoever they belong to. */
  refuseSessionsOf = new Set<string>();
  /** Refuse every bearer token (401), even fresh ones: a hub that will not have this device. */
  refuseAllSessions = false;
  /** `POST /v1/devices` refused with this 429 code (`device_rate`, `too_many_pending_devices`): the account's limits on new devices. */
  refuseRegistrations: string | null = null;
  /** Devices the hub says were removed, whatever the chain says (a lying hub). */
  claimRemoved = new Set<string>();
  /** Mail posts per sender, by time (the 120/min cap). */
  private posts = new Map<string, number[]>();
  mailRatePerMin = 120;
  /** `RH_HUB_CHAIN_V3` (docs/local-aggregator.md §3.5): v3 entries are taken only in `write`. */
  chainV3Mode: "off" | "read" | "write" = "write";
  /** Hosts whose agents have not declared `chain/3` (yet): a v3 batch while one is current, or added, is a 409. */
  undeclared = new Set<string>();
  /** Told of every 409 `chain_v3_not_ready` (the hosts it named), e.g. to have an agent declare `chain/3` a moment later. */
  onNotReady: ((hosts: string[]) => void) | null = null;
  /** Every 409 `chain_v3_not_ready` answered, by the hosts it named. */
  notReady: string[][] = [];
  /** Every `POST /v1/chain` that reached the hub (taken or not). */
  chainPosts = 0;
  /** Every `POST /v1/mailbox` that reached the hub, taken or refused: `sender recipient purpose`. */
  mailAttempts: string[] = [];
  /**
   * Hosts whose device role signed in (S1-4, as rh-hub's `ensure_host_device`):
   * only then does mail other than their own machine key (purpose 2) reach
   * them — before, it is a 400.
   */
  hostDevices = new Set<string>();
  /**
   * Identity key renewals as rh-hub keeps them (docs/host-identity-rotation.md
   * §3.3): `DROPPEN_HUB_HOST_ROTATION`, the window T, the
   * account's 「总在设备上确认」, the hosts whose agents declared `rotate/1`,
   * each host's row (as `GET /v1/rotations` answers it) and the disputes.
   */
  rotationEnabled = true;
  rotationWindowMs = 0;
  rotates = new Set<string>();
  rotationRows = new Map<string, Record<string, unknown>>();
  disputes: Record<string, unknown>[] = [];
  /** Answer the next renewal batches 409 `window` with these `ready_ms` (one each). */
  windowRefusals: number[] = [];
  /** Every `PUT /v1/rotations/{host}` taken or refused: `host attempt|pending assurance`. */
  rotationPuts: string[] = [];
  /** Every `DELETE /v1/rotations/{host}`. */
  rotationDeletes: string[] = [];
  /** Told of every `DELETE /v1/rotations/{host}` before it is decided: another page acting just before it lands. */
  beforeRotationDelete: ((host: string) => void) | null = null;
  /** How far the hub's `now_ms` is off this test's clock (its other clocks stay). */
  nowMsOffset = 0;
  /** Answer `GET /v1/rotations` 404 (a hub that withholds what it showed before). */
  rotationsGone = false;

  /**
   * The host's agent staging a renewal on its hub connection
   * (`rotation_stage`), checked as rh-hub checks the nonce: its `ready_ms`,
   * or `null` when no open attempt has that nonce.
   */
  async stageRotation(host: string, s: { nonce: Uint8Array; signKey: Uint8Array; dhKey: Uint8Array; stagedSeq: number }): Promise<number | null> {
    const row = this.rotationRows.get(host);
    if (!row || row.state !== "attempt" || row.nonce_hash !== hex(new Uint8Array(await crypto.subtle.digest("SHA-256", s.nonce)))) return null;
    const readyMs = this.now() + this.rotationWindowMs;
    Object.assign(row, { state: "staged", staged_ms: this.now(), ready_ms: readyMs, sign_key: hex(s.signKey), dh_key: hex(s.dhKey), staged_seq: s.stagedSeq });
    return readyMs;
  }

  private renewalNotReady(reason: string, extra: Record<string, unknown> = {}): Response {
    return this.json(409, { error: "rotation_not_ready", reason, message: `the renewal is not ready: ${reason}`, ...extra });
  }

  json(status: number, value: unknown): Response {
    return new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
  }

  private removed(device: string): boolean {
    return this.claimRemoved.has(device) || this.state?.deviceRecord(device)?.status.type === "removed";
  }

  /** Whether `device` is a current full (admin) device of the chain. */
  private full(device: string): boolean {
    return this.state?.admin(device) !== undefined;
  }

  /** The hub's push and every session of a removed device: gone (`rh-hub` revokes them as the removal lands). */
  private revokeRemoved(): void {
    for (const [token, who] of this.sessions) if (who.kind === "device" && this.removed(who.device)) this.sessions.delete(token);
  }

  /**
   * As `rh-hub`: a v3 batch only in `write`, and only while every current
   * host (and every host it adds) declared `chain/3`. On a chain that holds a
   * v3 entry already, only what adds a host or makes one a host again is
   * gated: removals, demotions, rotations, approvals, recovery joins (and the
   * rest) go whatever the mode.
   */
  private chainV3Refusal(decoded: ReturnType<typeof decodeEntry>[]): Response | null {
    if (!decoded.some((e) => e.draft.version === 3)) return null;
    const addsHost = decoded.some((e) => {
      const a = e.draft.action;
      return (a.type === "addDevice3" && a.device.flags.host) || (a.type === "setHost" && a.host);
    });
    if (this.state?.isV3 && !addsHost) return null;
    const hosts = new Set((this.state?.hosts() ?? []).filter((h) => h.status.type === "active").map((h) => h.info.id));
    for (const e of decoded) {
      const a = e.draft.action;
      if (a.type === "addDevice3" && a.device.flags.host) hosts.add(a.device.id);
      if (a.type === "setHost" && a.host) hosts.add(a.id);
    }
    const waiting = [...hosts].filter((h) => this.undeclared.has(h)).sort();
    if (this.chainV3Mode === "write" && waiting.length === 0) return null;
    this.notReady.push(waiting);
    this.onNotReady?.(waiting);
    return this.json(409, { error: "chain_v3_not_ready", mode: this.chainV3Mode, hosts: waiting, message: "not every computer reads chain v3 yet" });
  }

  /** `/v1/me`'s `chain_v3`, as `rh-hub` works it out: `writable` in `write` once no current host lags (or the chain is v3 already). */
  private chainV3View() {
    const inUse = this.state?.isV3 === true;
    const waiting = (this.state?.hosts() ?? [])
      .filter((h) => h.status.type === "active" && this.undeclared.has(h.info.id))
      .map((h) => h.info.id)
      .sort();
    return { mode: this.chainV3Mode, writable: this.chainV3Mode === "write" && (inUse || waiting.length === 0), waiting, in_use: inUse };
  }

  private async append(entries: string[]): Promise<Response> {
    this.chainPosts++;
    const decoded = entries.map((e) => decodeEntry(fromHex(e)));
    const notReady = this.chainV3Refusal(decoded);
    if (notReady) return notReady;
    // As rh-hub (§3.3 提交): a renewal only as staged, its window passed.
    const renewals = decoded.flatMap((e) => (e.draft.action.type === "rotateHostKey" ? [{ seq: e.draft.seq, r: e.draft.action.rotation }] : []));
    for (const { r } of renewals) {
      if (!this.rotationEnabled) return this.renewalNotReady("disabled");
      const row = this.rotationRows.get(r.host);
      if (!row || row.state !== "staged") return this.renewalNotReady("not_staged");
      if (row.sign_key !== hex(r.signKey) || row.dh_key !== hex(r.dhKey) || row.staged_seq !== r.stagedSeq) return this.renewalNotReady("mismatch");
      const early = this.windowRefusals.shift();
      if (early !== undefined) return this.renewalNotReady("window", { ready_ms: early });
      if (this.now() < (row.ready_ms as number)) return this.renewalNotReady("window", { ready_ms: row.ready_ms });
    }
    // As rh-hub: a batch never ends with a host still owed its next machine key epoch (a new or restarted host, a narrowed accept list).
    if (this.state && decoded.every((e) => e.draft.seq > this.state!.seq)) {
      const probe = this.state.clone();
      const clock = ChainClock.history();
      try {
        for (const e of decoded) await probe.apply(e, clock);
      } catch {
        // said below, entry by entry
      }
      if (probe.seq === this.state.seq + decoded.length && probe.hostKeyDue !== null) {
        return this.json(422, { error: "invalid_entry", message: "a host's next machine key epoch must come in the same batch" });
      }
    }
    for (const e of entries) {
      const entry = decodeEntry(fromHex(e));
      if (this.state && entry.draft.seq <= this.state.seq) {
        return this.json(409, { error: "chain_conflict", message: `the chain is at Some(${this.state.seq}); fetch and retry` });
      }
      const a = entry.draft.action;
      const v3Host = (a.type === "addDevice3" && a.device.flags.host) || (a.type === "recoveryJoin3" && a.device.flags.host);
      const device =
        a.type === "genesis" || a.type === "addDevice" || a.type === "recoveryJoin" || ((a.type === "addDevice3" || a.type === "recoveryJoin3") && !v3Host) ? a.device : null;
      if (device) {
        const reg = this.devices.get(device.id);
        if (!reg || reg.sign !== hex(device.signKey) || reg.dh !== hex(device.dhKey)) {
          return this.json(400, { error: "bad_request", message: `device ${device.id} is not registered to this account with these keys` });
        }
      }
      // A host it adds (or makes a host again) is an enrolled machine with the same keys.
      const host = v3Host && (a.type === "addDevice3" || a.type === "recoveryJoin3") ? a.device : a.type === "setHost" && a.host ? (this.state?.device(a.id)?.info ?? null) : null;
      if (host) {
        const reg = this.machines.get(host.id);
        if (!reg || reg.sign !== hex(host.signKey) || reg.dh !== hex(host.dhKey)) {
          return this.json(422, { error: "invalid_entry", message: `host ${host.id} is not an enrolled machine with these keys` });
        }
      }
      try {
        this.clock.nowMs = this.chainNow();
        if (!this.state) {
          this.state = await ChainState.genesis(entry);
          this.clock = ChainClock.live(this.chainNow(), 1);
        } else await this.state.apply(entry, this.clock);
      } catch (error) {
        const message = error instanceof ChainError ? error.message : String(error);
        return this.json(400, { error: "bad_request", message });
      }
      this.chain.push(e);
    }
    this.revokeRemoved();
    for (const { seq, r } of renewals) Object.assign(this.rotationRows.get(r.host)!, { state: "committed", committed_ms: this.now(), committed_seq: seq, failure: null });
    this.applyEffects(entries);
    await this.afterAppend?.(entries);
    return this.json(200, { seq: this.chain.length - 1 });
  }

  /**
   * What `rh-hub` does with a removal or a new recovery key in the same
   * transaction (M2 re-review): the removed device's mail (from or
   * to it) purged, and the keyring swept as `keyring::sweep` sweeps it —
   * packages for another recovery key (every one, with none); of a
   * writer no longer current (for a package, its sender) whatever names an
   * epoch or a machine the chain never committed, and all but one copy per
   * (writer, machine, `MK` epoch) — under the newest `UK` — and per (sender,
   * `UK` epoch) — the newest for the current key. Never a copy because
   * another writer holds the same wrap: readers try every one.
   */
  private applyEffects(entries: string[]): void {
    const actions = entries.map((e) => decodeEntry(fromHex(e)).draft.action);
    const removed = actions.flatMap((a) => (a.type === "removeDevice" || a.type === "removeDevice3" ? [a.id] : []));
    if (removed.length) this.mail = this.mail.filter((m) => !removed.includes(m.sender) && !removed.includes(m.recipient));
    if (!actions.some((a) => a.type === "removeDevice" || a.type === "removeDevice3" || a.type === "setRecoveryKey") || !this.state) return;
    const state = this.state;
    const current = (id: string) => state.admin(id) !== undefined;
    const rsk = state.currentRecoveryKey;
    const tag = rsk ? hex(rsk.slice(0, 8)) : null;
    const epoch = (s: string | undefined) => (s !== undefined && /^\d{1,10}$/.test(s) ? Number(s) : null);
    type Rank = [number, number, string];
    const above = (x: Rank, y: Rank) => x[0] - y[0] || x[1] - y[1] || (x[2] < y[2] ? -1 : x[2] > y[2] ? 1 : 0);
    const gone = new Set<string>();
    // The copies a writer no longer current may keep, per slot: the highest rank stays.
    const slots = new Map<string, Rank[]>();
    const compete = (slot: string, rank: Rank) => slots.set(slot, [...(slots.get(slot) ?? []), rank]);
    for (const [name, k] of this.keys) {
      const writer = k.writer ?? "";
      const parts = name.split("/");
      if (parts[0] === "rk") {
        const [, uk, sender, nameTag] = parts;
        if (tag === null || (nameTag !== undefined && nameTag !== tag) || !sender) gone.add(name);
        else if (current(sender) && (writer === "" || current(writer))) continue;
        else {
          const u = epoch(uk);
          if (u === null || u >= state.userKeyEpochs() || parts.length > 5) gone.add(name);
          else compete(`rk/${sender}/${u}`, [nameTag === undefined ? 0 : 1, k.updated_ms, name]);
        }
      } else if (writer === "" || current(writer)) continue;
      else {
        const [kind, machine, mk, uk] = parts;
        const [e, u] = [epoch(mk), epoch(uk)];
        const committed = kind === "mk" && parts.length === 5 && !!machine && e !== null && u !== null && e < state.machineKeyEpochs(machine) && u < state.userKeyEpochs();
        if (committed) compete(`mk/${writer}/${machine}/${e}`, [u, k.updated_ms, name]);
        else gone.add(name);
      }
    }
    for (const ranks of slots.values()) {
      const best = ranks.reduce((x, y) => (above(y, x) > 0 ? y : x));
      for (const r of ranks) if (r !== best) gone.add(r[2]);
    }
    for (const name of gone) this.keys.delete(name);
  }

  /** As the hub's keyring rule (docs/hub-api/http.md, `PUT /v1/keys/{name}`): the reason `writer` may not write `name` now, or `null`. */
  private keyNameRefusal(name: string, writer: string): string | null {
    if (this.looseKeyNames) return null;
    const state = this.state;
    if (!state) return "no chain";
    const parts = name.split("/");
    const epoch = (t: string | undefined) => (t && /^\d{1,10}$/.test(t) ? Number(t) : null);
    const last = parts[parts.length - 1]?.split(".")[0];
    if (parts.length !== 5 || last !== writer) return "a keyring name ends with the device that writes it";
    if (parts[0] === "mk") {
      const mk = epoch(parts[2]);
      const uk = epoch(parts[3]);
      if (!parts[1] || !state.machine(parts[1])) return "no such device that runs AI in the account's chain";
      if (mk === null || uk === null || mk >= state.machineKeyEpochs(parts[1]) || uk >= state.userKeyEpochs()) return "the chain has not committed those key epochs";
      return null;
    }
    if (parts[0] === "rk") {
      const uk = epoch(parts[1]);
      if (parts[2] !== writer) return "a recovery package is written by its sender";
      if (uk === null || uk >= state.userKeyEpochs()) return "the chain has not committed that user key epoch";
      const rsk = state.currentRecoveryKey;
      if (!rsk || parts[3] !== hex(rsk.slice(0, 8))) return "not the account's current recovery key";
      return null;
    }
    return "not a keyring name";
  }

  private newToken(who: Who, ttlMs: number): { token: string; expiresMs: number } {
    const token = `${who.kind === "login" ? "login" : `ds-${who.device}`}-${++this.sessionId}`;
    const expiresMs = this.now() + ttlMs;
    this.sessions.set(token, { ...who, expiresMs });
    return { token, expiresMs };
  }

  fetch = async (input: string, init?: RequestInit): Promise<Response> => {
    const url = new URL(input, "https://hub.test");
    const method = init?.method ?? "GET";
    const body = init?.body ? JSON.parse(init.body as string) : null;
    const path = url.pathname;
    await this.gate?.(`${method} ${path}`, body);
    const token = new Headers(init?.headers).get("authorization")?.replace(/^Bearer /, "") ?? "";
    const denied = () => this.json(401, { error: "unauthorized", message: "sign in first" });

    // No bearer needed: password sign-in, and a device's signature for a device session.
    if ((path === "/v1/login" || path === "/v1/signup") && method === "POST") {
      const { token: t, expiresMs } = this.newToken({ kind: "login" }, 15 * 60_000);
      return this.json(200, { account: this.account, token: t, expires_ms: expiresMs });
    }
    if (path === "/v1/session/challenge" && method === "POST") {
      if (body.account !== this.account || !this.devices.has(body.device)) return this.json(401, { error: "denied", message: "no such device" });
      const nonce = hex(crypto.getRandomValues(new Uint8Array(32)));
      this.challenges.set(nonce, { device: body.device, expiresMs: this.now() + 120_000 });
      return this.json(200, { nonce });
    }
    if (path === "/v1/session" && method === "POST") {
      const challenge = this.challenges.get(body.nonce);
      this.challenges.delete(body.nonce);
      const reg = this.devices.get(body.device);
      if (!challenge || challenge.device !== body.device || challenge.expiresMs < this.now() || !reg || body.account !== this.account) {
        return this.json(401, { error: "denied", message: "bad nonce or device" });
      }
      if (Math.abs(this.now() - body.time_ms) > 5 * 60_000) return this.json(401, { error: "denied", message: "clock" });
      const ok = await verifyHubLogin(
        { role: "deviceSession", account: body.account, id: body.device, hub: HUB_ORIGIN, nonce: fromHex(body.nonce), timeMs: body.time_ms },
        fromHex(reg.sign),
        fromHex(body.signature),
      );
      if (!ok) return this.json(401, { error: "denied", message: "bad signature" });
      if (this.removed(body.device)) return this.json(403, { error: "device_removed", message: "this device was removed from the account" });
      const s = this.newToken({ kind: "device", device: body.device }, this.deviceSessionMs);
      return this.json(200, { account: this.account, device: body.device, token: s.token, expires_ms: s.expiresMs });
    }

    if (this.refuseSessionsOf.has(token) || (this.refuseAllSessions && token)) return denied();
    const who = this.sessions.get(token);
    if (!who || who.expiresMs <= this.now()) return denied();
    if (who.kind === "device" && this.removed(who.device)) {
      this.sessions.delete(token);
      return denied();
    }
    // A login token: register a device, /v1/me, sign out, and read the chain.
    if (path === "/v1/devices" && method === "POST") {
      if (who.kind !== "login") return this.json(403, { error: "forbidden", message: "register with a login session" });
      if (this.refuseRegistrations) return this.json(429, { error: this.refuseRegistrations, message: "no new devices for now" });
      // Never an id used before, even once a device was forgotten (as rh-hub's random ids).
      const id = this.nextDeviceId ?? `d-${++this.deviceSeq}`;
      this.nextDeviceId = null;
      this.devices.set(id, { sign: body.sign_key, dh: body.dh_key });
      this.registeredMs.set(id, this.now());
      this.registrations.push(id);
      if (typeof body.replaces === "string" && this.devices.has(body.replaces)) this.replaces.set(id, body.replaces);
      return this.json(200, { device: id });
    }
    // `/v1/me`, as much of it as the pairing screen and the sign-in read: the devices registered, and when.
    if (path === "/v1/me" && method === "GET") {
      return this.json(200, {
        account: this.account,
        email: { address: "someone@example.test", verified: true, verification: "off" },
        plan: null,
        chain_seq: this.chain.length ? this.chain.length - 1 : null,
        chain_v3: this.chainV3View(),
        devices: [...this.devices.entries()].map(([id, d]) => ({
          id,
          sign_key: d.sign,
          dh_key: d.dh,
          created_ms: this.registeredMs.get(id) ?? null,
          last_seen_ms: null,
          in_chain: this.state?.device(id) !== undefined,
          replaces: this.replaces.get(id) ?? null,
          online: this.online.has(id),
          ua_label: this.uaLabels.get(id) ?? null,
        })),
        machines: [],
      });
    }
    if (path === "/v1/logout" && method === "POST") {
      this.sessions.delete(token);
      return new Response(null, { status: 204 });
    }
    if (path === "/v1/chain" && method === "GET") {
      const from = Number(url.searchParams.get("from") ?? 0);
      const chain = (await this.chainServed?.(from)) ?? this.chain;
      return this.json(200, { entries: chain.slice(from), seq: chain.length ? chain.length - 1 : null });
    }
    if (who.kind !== "device") return this.json(401, { error: "device_session_required", message: "this needs a device session" });
    const me = who.device;

    if (path === "/v1/chain" && method === "POST") return this.append(body.entries as string[]);
    if (path === "/v1/rotations" && method === "GET") {
      // A hub that stopped saying (or never said): 404, as one from before renewals.
      if (this.rotationsGone) return this.json(404, { error: "not_found" });
      return this.json(200, { enabled: this.rotationEnabled, window_ms: this.rotationWindowMs, hosts: [...this.rotationRows.values()], disputes: this.disputes, now_ms: this.now() + this.nowMsOffset });
    }
    if (path.startsWith("/v1/rotations/")) {
      const host = decodeURIComponent(path.slice("/v1/rotations/".length));
      if (!this.full(me)) return this.json(403, { error: "not_a_full_device", message: "only a full device renews" });
      const row = this.rotationRows.get(host);
      if (method === "DELETE") {
        this.rotationDeletes.push(host);
        this.beforeRotationDelete?.(host);
        // As rh-hub: `nonce_hash` names the attempt (or staging) withdrawn, `requested_ms` the pending mark; anything else
        // the host has by then stays — 409 `rotation_in_progress` while an attempt or staging is open, `{}` otherwise.
        const now = this.rotationRows.get(host);
        if (now && now.state !== "committed") {
          const live = now.state === "attempt" || now.state === "staged";
          const named = body?.nonce_hash !== undefined ? live && now.nonce_hash === body.nonce_hash : body?.requested_ms !== undefined ? now.state === "pending" && now.requested_ms === body.requested_ms : true;
          if (!named) return live ? this.json(409, { error: "rotation_in_progress", message: "another renewal of it is running" }) : this.json(200, {});
          // An isolated host stays `pending` (only a renewal confirmed on the device, or its removal, clears it).
          if (now.isolated === true) {
            const kept: Record<string, unknown> = { ...now, state: "pending", assurance: 1, failure: null };
            for (const k of ["nonce_hash", "attempt_expires_ms", "staged_ms", "ready_ms", "sign_key", "dh_key", "staged_seq"]) delete kept[k];
            this.rotationRows.set(host, kept);
            return this.json(200, {});
          }
          this.rotationRows.delete(host);
        }
        return this.json(200, {});
      }
      if (method !== "PUT") return this.json(405, { error: "method" });
      this.rotationPuts.push(`${host} ${body.nonce_hash ? "attempt" : "pending"} ${body.assurance}`);
      if (!this.state?.machine(host)) return this.json(409, { error: "not_a_host", message: "not a current host" });
      if (!this.rotationEnabled) return this.renewalNotReady("disabled");
      if (!this.rotates.has(host)) return this.renewalNotReady("agent_too_old");
      // As rh-hub (`rotations.rs`): a live attempt or staging — whoever made it — is never replaced by another nonce; only withdrawn (DELETE).
      const live = row && ((row.state === "attempt" && (row.attempt_expires_ms as number) > this.now()) || row.state === "staged");
      if (body.nonce_hash && live && row.nonce_hash !== body.nonce_hash) return this.json(409, { error: "rotation_in_progress", message: "another renewal of it is running" });
      const kept = { needs_assurance: row?.needs_assurance === true, isolated: row?.isolated === true, failure: row?.failure ?? null };
      const next: Record<string, unknown> = body.nonce_hash
        ? { host, state: "attempt", assurance: body.assurance, requested_ms: this.now(), requested_by: me, nonce_hash: body.nonce_hash, attempt_expires_ms: this.now() + 600_000, ...kept }
        : row && row.state !== "committed"
          ? row
          : { host, state: "pending", assurance: body.assurance, requested_ms: this.now(), requested_by: me, ...kept };
      this.rotationRows.set(host, next);
      return this.json(200, next);
    }
    if (path === "/v1/enrollments") {
      this.enrollments.set(body.token, { device: body.device });
      return this.json(200, { expires_ms: Date.now() + 600_000 });
    }
    if (path.startsWith("/v1/enrollments/")) {
      const e = this.enrollments.get(path.split("/").pop()!);
      if (!e) return this.json(404, { error: "not_found" });
      if (!e.proof) return this.json(200, { state: "waiting" });
      return this.json(200, { state: "proved", machine: e.machine, sign_key: hex(e.sign!), dh_key: hex(e.dh!), proof: hex(e.proof) });
    }
    if (path === "/v1/mailbox" && method === "POST") this.mailAttempts.push(`${body.sender} ${body.recipient} ${body.purpose}`);
    if (path === "/v1/mailbox" && method === "POST" && this.failMail > 0) {
      this.failMail--;
      return this.json(503, { error: "unavailable", message: "try again" });
    }
    if (path === "/v1/mailbox" && method === "POST") {
      if (body.sender !== me) return this.json(403, { error: "forbidden", message: "mail is sent as this session's device" });
      // As rh-hub (chain v3, §3.3, review D2): an admin sends anything; any
      // other current device only asks an admin for keys (purpose 4); a user
      // key goes only to an active admin (not a pending one); a host key only
      // to a device that is not an admin and that the host named in it
      // accepts control from.
      const recipientRecord = this.state?.deviceRecord(body.recipient);
      const recipientAdmin = !!recipientRecord && recipientRecord.status.type === "active" && recipientRecord.admin;
      if (body.purpose === 4 && !this.full(me)) {
        if (!this.state?.device(me)) return this.json(403, { error: "not_in_chain", message: "only a current device of the account may ask for keys" });
        if (!recipientAdmin) return this.json(403, { error: "not_an_admin", message: "a device that is not an admin asks only an admin device for keys" });
      } else if (!this.full(me)) return this.json(403, { error: "not_a_full_device", message: "only a current full device sends mail" });
      if (body.purpose === 1 && !recipientAdmin) return this.json(403, { error: "not_an_admin", message: "a user key goes only to a device that manages the account" });
      if (body.purpose === 5) {
        let host = "";
        try {
          host = decodeHostKeyMail(fromHex(body.sealed)).host;
        } catch {
          return this.json(400, { error: "bad_request", message: "a host key names its host" });
        }
        let admitted = false;
        try {
          this.state?.mayControl(body.recipient, host);
          admitted = !!this.state && !recipientAdmin;
        } catch {
          admitted = false;
        }
        if (!admitted) return this.json(403, { error: "not_accepted", message: "a host key goes only to a device that is not an admin and that the host accepts control from" });
      }
      if (!(this.devices.has(body.recipient) || this.machines.has(body.recipient))) {
        return this.json(400, { error: "bad_request", message: "recipient and sender must belong to this account" });
      }
      // As rh-hub (`mail_role_for`): a host's own machine key only to it as a
      // machine; anything else only to a device role — a host's once it
      // signed in as a device.
      if (body.purpose === 2 ? !this.machines.has(body.recipient) : !(this.devices.has(body.recipient) || this.hostDevices.has(body.recipient))) {
        return this.json(400, { error: "bad_request", message: "this mail goes only to a device of this account (a computer gets it once it signs in as a device)" });
      }
      const sealed = fromHex(body.sealed);
      if (sealed.length > 8192) return this.json(400, { error: "bad_request", message: "sealed is ≤ 8 KiB of hex" });
      const recent = (this.posts.get(me) ?? []).filter((t) => t > this.now() - 60_000);
      if (recent.length >= this.mailRatePerMin) return this.json(429, { error: "mail_rate", message: "too much mail from this device: wait a minute" });
      const held = this.mail.filter((m) => m.recipient === body.recipient);
      const bytes = held.reduce((n, m) => n + m.sealed.length / 2, 0);
      // As rh-hub (M2 re-review): one sender's share, and room kept for key mail.
      const fromSender = held.filter((m) => m.sender === me).length;
      const other = held.filter((m) => m.purpose !== 1 && m.purpose !== 2).length;
      const keyMail = body.purpose === 1 || body.purpose === 2;
      if (held.length >= 256 || bytes + sealed.length > 1 << 20 || fromSender >= 128 || (!keyMail && other >= 32)) {
        return this.json(507, { error: "mailbox_full", message: "the recipient's mailbox is full" });
      }
      recent.push(this.now());
      this.posts.set(me, recent);
      const mail = { id: ++this.mailId, recipient: body.recipient, sender: body.sender, purpose: body.purpose, epoch: body.epoch, sealed: body.sealed };
      this.requests.push(`mail ${body.recipient} ${body.purpose} ${body.epoch}`);
      this.mail.push(mail);
      this.mailLog.push(mail);
      return this.json(200, { id: mail.id });
    }
    if (path === "/v1/mailbox") {
      const after = Number(url.searchParams.get("after") ?? 0);
      const limit = Math.min(64, Math.max(1, Number(url.searchParams.get("limit") ?? 64)));
      // A host's device role never sees its agent's own machine key (purpose 2).
      const mine = this.mail.filter((m) => m.recipient === me && m.id > after && !(m.purpose === 2 && this.machines.has(me))).slice(0, limit);
      return this.json(200, { mail: mine.map(({ recipient: _, ...m }) => m) });
    }
    if (path.startsWith("/v1/mailbox/") && method === "DELETE") {
      const id = Number(path.split("/").pop());
      this.mail = this.mail.filter((m) => !(m.id === id && m.recipient === me));
      return new Response(null, { status: 204 });
    }
    if (path === "/v1/keys") {
      if (this.noKeyring) return this.json(404, { error: "not_found" });
      return this.json(200, { keys: [...this.keys.entries()].map(([name, k]) => ({ name, blob: k.blob, updated_ms: k.updated_ms, writer: k.writer ?? "" })) });
    }
    if (path.startsWith("/v1/keys/") && method === "PUT") {
      if (this.noKeyring) return this.json(404, { error: "not_found" });
      if (!this.full(me)) return this.json(403, { error: "not_a_full_device", message: "only a current full device writes the keyring" });
      const name = path.slice("/v1/keys/".length);
      if (!/^[a-z0-9._/-]{1,128}$/.test(name) || name.includes("..")) return this.json(400, { error: "bad_request", message: "a key name is [a-z0-9._/-]{1,128}" });
      if (fromHex(body.blob).length > 4096) return this.json(400, { error: "bad_request", message: "blob is ≤ 4 KiB of hex" });
      if (this.keyringFull) return this.json(507, { error: "keyring_full", message: "the account's keyring is full" });
      const refusal = this.keyNameRefusal(name, me);
      if (refusal) return this.json(403, { error: "bad_key_name", message: refusal });
      const existing = this.keys.get(name);
      if (existing) {
        if (existing.blob === body.blob) return this.json(200, { name, stored: false });
        return this.json(409, { error: "exists", message: "a keyring name is written once" });
      }
      this.keys.set(name, { blob: body.blob, updated_ms: this.now(), writer: me });
      this.requests.push(`put ${name}`);
      return this.json(200, { name, stored: true });
    }
    if (path === "/v1/pairings" && method === "POST") {
      if (body.device !== me) return this.json(403, { error: "forbidden", message: "a pairing is opened for this session's own device" });
      // As rh-hub (M2 re-review): a device outside the chain holds one at a time.
      // …and only within 30 minutes of registering (rh-hub `pairings::UNCHAINED_WINDOW_MS`).
      if (!this.full(me) && this.now() - (this.registeredMs.get(me) ?? this.now()) >= 30 * 60_000) {
        return this.json(403, { error: "pairing_window_closed", message: "this device registered too long ago to open a pairing: sign in again on it" });
      }
      if (!this.full(me)) for (const [id, p] of this.pairings) if (p.device === me) this.pairings.delete(id);
      if (this.pairings.size >= 4) return this.json(429, { error: "too_many_pairings", message: "too many pairings" });
      const id = `p-${++this.pairingId}`;
      this.pairings.set(id, { device: me, created_ms: this.now(), messages: [] });
      return this.json(200, { id });
    }
    if (path === "/v1/pairings") {
      if (!this.full(me)) return this.json(403, { error: "not_a_full_device", message: "only a full device lists pairings" });
      return this.json(200, { pairings: [...this.pairings.entries()].map(([id, p]) => ({ id, device: p.device, created_ms: p.created_ms })) });
    }
    const pm = /^\/v1\/pairings\/([^/]+)(\/messages)?$/.exec(path);
    if (pm) {
      const id = decodeURIComponent(pm[1]!);
      const p = this.pairings.get(id);
      if (!p) return this.json(404, { error: "not_found", message: "no such pairing (or it expired)" });
      if (method === "DELETE" && !pm[2]) {
        // Its opener, or any current full device (M2 re-review).
        if (p.device !== me && !this.full(me)) return new Response(null, { status: 204 });
        this.pairings.delete(id);
        return new Response(null, { status: 204 });
      }
      if (method === "POST") {
        if (body.from !== "initiator" && body.from !== "responder") return this.json(400, { error: "bad_request", message: "from is initiator or responder" });
        if (body.from === "initiator" && !this.full(me)) return this.json(403, { error: "not_a_full_device", message: "only a full device runs a code" });
        if (body.from === "responder" && p.device !== me) return this.json(403, { error: "forbidden", message: "only the device that opened a pairing answers" });
        if (p.messages.length >= 8) return this.json(429, { error: "pairing_full", message: "full" });
        const n = p.messages.length + 1;
        p.messages.push({ n, from: body.from, body: body.body });
        return this.json(200, { n });
      }
      if (p.device !== me && !this.full(me)) return this.json(403, { error: "forbidden", message: "not your pairing" });
      const after = Number(url.searchParams.get("after") ?? 0);
      return this.json(200, { messages: p.messages.filter((m) => m.n > after) });
    }
    return this.json(404, { error: "not_found" });
  };

  /** The names held as `base` (`<base>/<writer>[.<suffix>]`), and `base` itself if planted so. */
  keysAt(base: string): string[] {
    return [...this.keys.keys()].filter((n) => n === base || n.startsWith(`${base}/`)).sort();
  }

  /** The (first) blob held as `base`. */
  keyAt(base: string): { blob: string; updated_ms: number; writer?: string } | undefined {
    const name = this.keysAt(base)[0];
    return name === undefined ? undefined : this.keys.get(name);
  }

  api(): HubApi {
    return new HubApi("https://hub.test", this.fetch);
  }

  /**
   * A browser of the account: its own key store (and so its own storage),
   * signed in with the password — a login token, a device registered if the
   * browser has none, then a device session for it.
   */
  async device(_login = "tok", store: KeyStore = new MemoryKeyStore()): Promise<CloudAccount> {
    const api = this.api();
    const login = await api.login("someone@example.test", "a password");
    return CloudAccount.open(api, login, store, { origin: HUB_ORIGIN, ...browserOf(store) });
  }

  /**
   * Host `id`'s device role signs in (S1-4): a device session for it, as
   * `POST /v1/session` mints one for a current host (the signature is
   * rh-hub's to check, and its tests'). Its token.
   */
  signInHost(id: string): string {
    if (!this.state?.device(id)?.host) throw new Error(`${id} is not a current host`);
    this.hostDevices.add(id);
    return this.newToken({ kind: "device", device: id }, this.deviceSessionMs).token;
  }

  /** The same browser again with its device session kept from before (a reload). */
  async reload(account: CloudAccount): Promise<CloudAccount> {
    return CloudAccount.open(this.api(), { ...account.session }, account.store, { origin: HUB_ORIGIN, ...browserOf(account.store) });
  }

  /** Enroll a machine through `account`: the agent's keys, its proof checked, "add machine" signed (`opts`: the hub's say, `writable`). */
  async enroll(
    account: CloudAccount,
    id = "m-1",
    opts?: Parameters<CloudAccount["completeEnrollment"]>[4],
  ): Promise<{ id: string; dh: NoiseKeyPair; sign: Uint8Array; signPrivate: CryptoKey }> {
    const keys = await generateDeviceKeys();
    this.machines.set(id, { sign: hex(keys.signPublic), dh: hex(keys.dhPublic) });
    const code: EnrollCode = await account.startEnrollment();
    const token = [...this.enrollments.keys()].at(-1)!;
    const proof = await enrollProof(code, account.account, id, keys.signPublic, keys.dhPublic);
    this.enrollments.set(token, { device: account.deviceId, machine: id, sign: keys.signPublic, dh: keys.dhPublic, proof });
    await account.completeEnrollment(code, undefined, 1, undefined, opts);
    return { id, dh: { privateKey: keys.dh.privateKey, publicKey: keys.dhPublic }, sign: keys.signPublic, signPrivate: keys.sign.privateKey };
  }
}

/**
 * `account`'s chain made as accounts were before chain v3: a v2 genesis and
 * `UK` epoch 0 as v2. The page makes none any more (`createGenesis` is v3
 * from its genesis); this stands for an account from before, for the tests
 * of what such an account does — its v2 entries, its one-time upgrade.
 */
export function createV2Genesis(account: CloudAccount, withRecovery: boolean): Promise<string[] | null> {
  return (account as unknown as { foundAccount(withRecovery: boolean, version: 2): Promise<string[] | null> }).foundAccount(withRecovery, 2);
}

/** A passkey slot on the chain (`PasskeySlotSet`, self-activation on) and the `PAK` that signs as it. */
export interface TestSlot {
  slot: string;
  pak: CryptoKeyPair;
}

/** Admin `admin` registers a passkey slot whose devices may activate themselves after a day (docs/passkeys.md §4.4). */
export async function addPasskeySlot(admin: CloudAccount, slot = "p-testslot0001"): Promise<TestSlot> {
  const pak = (await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
  const key = new Uint8Array(await crypto.subtle.exportKey("raw", pak.publicKey));
  const credHash = crypto.getRandomValues(new Uint8Array(32));
  const saltNonce = crypto.getRandomValues(new Uint8Array(32));
  const proof = await passkeySlotProof(pak.privateKey, key, admin.account, admin.chain!.accountKey, slot, credHash, saltNonce);
  await admin.append({ type: "passkeySlotSet", slot: { slot, key, proof, credHash, saltNonce, grants: 0, selfActivate: true } });
  return { slot, pak };
}

/** `action` signed as passkey slot `slot` on the hub's head, posted with `who`'s session (a browser registered, maybe not in the chain). */
export async function postAsPasskey(who: CloudAccount, slot: TestSlot, action: Action): Promise<void> {
  await who.sync();
  const entry = await signDraft(who.chain!.draft({ type: "passkey", slot: slot.slot }, action, Date.now()), slot.pak.privateKey);
  await who.api.appendChain(who.session.token, [encodeEntry(entry)]);
}

/** `who` joins through `slot`: a pending device that does not manage the account (flags 0). */
export async function passkeyJoin(who: CloudAccount, slot: TestSlot): Promise<void> {
  await postAsPasskey(who, slot, { type: "addDevice3", device: { id: who.deviceId, signKey: who.keys.signPublic, dhKey: who.keys.dhPublic, flags: { host: false, admin: false } } });
}

/**
 * `who`'s self-activation through `slot`, taken by `hub` at once — its chain
 * clock says the day has passed (a hub that lies, or one far ahead). A device
 * that first saw the join less than a day ago holds it back (`heldBack`).
 */
export async function passkeyActivateEarly(hub: FakeKeysHub, who: CloudAccount, slot: TestSlot): Promise<void> {
  const was = hub.chainNow;
  hub.chainNow = () => Date.now() + PASSKEY_DELAY_MS + 60_000;
  try {
    await postAsPasskey(who, slot, { type: "passkeyActivate", id: who.deviceId });
  } finally {
    hub.chainNow = was;
  }
}

/** A courier that records what it was given: the machine "online" (in-channel) or not (mailbox). */
export class FakeCourier implements MachineKeyCourier {
  online = true;
  channelStatus = 200;
  channelThrows = false;
  /** Throw this instead of a plain error (why the channel would not open). */
  channelError: Error | null = null;
  posted: { machine: string; epoch: number; key: string; reencrypt: boolean }[] = [];
  mailed: { machine: string; epoch: number; key: string }[] = [];
  waited: number[] = [];

  async waitOnline(_machine: string, timeoutMs: number): Promise<boolean> {
    this.waited.push(timeoutMs);
    return this.online;
  }

  async postInChannel(machine: string, body: { epoch: number; key: string; reencrypt: boolean }): Promise<{ status: number; body: unknown }> {
    if (this.channelError) throw this.channelError;
    if (this.channelThrows) throw new Error("the machine did not answer");
    this.posted.push({ machine, ...body });
    if (this.channelStatus === 200) return { status: 200, body: { mkEpoch: body.epoch, changed: true } };
    return { status: this.channelStatus, body: { error: "stale_epoch", message: "an older or different key" } };
  }

  async mail(machine: string, epoch: number, mk: Uint8Array): Promise<void> {
    this.mailed.push({ machine, epoch, key: hex(mk) });
  }
}

/** Every byte string anywhere in `value` (typed arrays, hex and base64 strings decoded), for "no raw key at rest" checks. */
export function containsBytes(value: unknown, needle: Uint8Array, seen = new Set<unknown>()): boolean {
  const hay = (b: Uint8Array) => {
    outer: for (let i = 0; i + needle.length <= b.length; i++) {
      for (let j = 0; j < needle.length; j++) if (b[i + j] !== needle[j]) continue outer;
      return true;
    }
    return false;
  };
  if (value === null || value === undefined) return false;
  if (value instanceof Uint8Array) return hay(value);
  if (ArrayBuffer.isView(value)) return hay(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
  if (value instanceof ArrayBuffer) return hay(new Uint8Array(value));
  if (typeof value === "string") {
    if (value.toLowerCase().includes(hex(needle))) return true;
    if (value.includes(Buffer.from(needle).toString("base64").slice(0, 40))) return true;
    return false;
  }
  if (typeof value !== "object" || seen.has(value)) return false;
  seen.add(value);
  return Object.values(value as Record<string, unknown>).some((v) => containsBytes(v, needle, seen));
}

/** A key store that keeps every value ever put (and is otherwise a memory store). */
export class RecordingKeyStore extends MemoryKeyStore {
  readonly puts: { name: string; value: unknown }[] = [];
  override async put(name: string, value: unknown): Promise<void> {
    this.puts.push({ name, value });
    return super.put(name, value);
  }
}

/** A host's identity as its agent holds it: the sign key that signs, and both public keys. */
export interface AgentIdentity {
  sign: CryptoKey;
  signPublic: Uint8Array;
  dhPublic: Uint8Array;
}

/**
 * The account's agents as a courier sees them (`RenewalCourier`): machine
 * keys as {@link FakeCourier} takes them, and `POST /rh/rotate` answered as
 * R3's agent answers it (docs/host-identity-rotation.md §3.2) —
 * a new seed's keys, both keys' signatures over the proof, the MAC under the
 * new DH key and the admin's ephemeral one, the staging at the hub — or
 * however a test makes it misbehave. A staged identity becomes the host's
 * own once the chain names it.
 */
export class FakeAgents extends FakeCourier implements RenewalCourier {
  /** Each host's identity now. */
  readonly identities = new Map<string, AgentIdentity>();
  /** What each host staged last, until the chain names it. */
  readonly staged = new Map<string, AgentIdentity>();
  /** The code each host's screen shows for its last renewal confirmed on the device. */
  readonly sas = new Map<string, string>();
  /** Every `POST /rh/rotate`, by host. */
  readonly rotateCalls: string[] = [];
  attests = 0;
  /** At most this many `POST /rh/rotate` were running at once. */
  maxActive = 0;
  private active = 0;
  /** Answer with this status (and `{error}`) instead (404: an older agent). */
  rotateStatus: { status: number; error?: string; reason?: string } | null = null;
  /** Throw this instead (the channel failed). */
  rotateError: Error | null = null;
  /** Spoil the answer: a signature, the MAC, or the view. */
  tamper: "oldSig" | "newSig" | "mac" | "view" | null = null;
  /** Answer with these keys as the "new" ones (a key the chain named before). */
  reuse: { signPublic: Uint8Array; dhPublic: Uint8Array } | null = null;
  /** Stage at the hub (`rotation_stage`) before answering; tell `readyMs`. */
  stageAtHub = true;
  tellReady = true;

  constructor(
    private readonly hub: FakeKeysHub,
    private readonly chainOf: () => ChainState | null,
  ) {
    super();
  }

  /** Host `id`'s identity, as its enrollment made it. */
  hold(id: string, identity: AgentIdentity): void {
    this.identities.set(id, identity);
  }

  /** What host `id` signs with now: its staged identity once the chain names it (the agent's promotion, §3.5). */
  current(id: string): AgentIdentity | undefined {
    const staged = this.staged.get(id);
    const onChain = this.chainOf()?.machine(id)?.info.signKey;
    if (staged && onChain && hex(onChain) === hex(staged.signPublic)) {
      this.identities.set(id, staged);
      this.staged.delete(id);
    }
    return this.identities.get(id);
  }

  async attest(): Promise<void> {
    this.attests++;
  }

  async rotate(machine: string, body: RotateRequest): Promise<{ status: number; body: unknown }> {
    this.rotateCalls.push(machine);
    this.active++;
    this.maxActive = Math.max(this.maxActive, this.active);
    try {
      await new Promise((resolve) => setTimeout(resolve, 1));
      if (this.rotateError) throw this.rotateError;
      if (this.rotateStatus) return { status: this.rotateStatus.status, body: { error: this.rotateStatus.error ?? "error", reason: this.rotateStatus.reason, message: "refused" } };
      const old = this.current(machine);
      const chain = this.chainOf();
      if (!old || !chain) return { status: 409, body: { error: "not_current_host", message: "not a current host" } };
      const fresh = await generateDeviceKeys();
      const next: AgentIdentity = { sign: fresh.sign.privateKey, signPublic: this.reuse?.signPublic ?? fresh.signPublic, dhPublic: this.reuse?.dhPublic ?? fresh.dhPublic };
      const nonce = fromHex(body.nonce);
      const head = fromHex(body.chainHead);
      const proof = hostRotateBody(chain.account, machine, body.chainSeq, head, old.signPublic, old.dhPublic, next.signPublic, next.dhPublic, nonce, body.assurance);
      const flip = (b: Uint8Array) => {
        const c = new Uint8Array(b);
        c[0] = c[0]! ^ 1;
        return c;
      };
      let oldSig = await signHostRotate(old.sign, proof);
      let newSig = await signHostRotate(fresh.sign.privateKey, proof);
      let mac = await rotateConfirmFor(fresh.dh.privateKey, fromHex(body.eph), proof);
      if (this.tamper === "oldSig") oldSig = flip(oldSig);
      if (this.tamper === "newSig") newSig = flip(newSig);
      if (this.tamper === "mac") mac = flip(mac);
      this.sas.set(machine, await rotateSas(proof));
      const readyMs = this.stageAtHub ? await this.hub.stageRotation(machine, { nonce, signKey: next.signPublic, dhKey: next.dhPublic, stagedSeq: body.chainSeq }) : null;
      this.staged.set(machine, next);
      return {
        status: 200,
        body: {
          signKey: hex(next.signPublic),
          dhKey: hex(next.dhPublic),
          oldSig: hex(oldSig),
          newSig: hex(newSig),
          mac: hex(mac),
          stagedSeq: this.tamper === "view" ? Math.max(0, body.chainSeq - 1) : body.chainSeq,
          stagedHead: body.chainHead,
          ...(this.tellReady && readyMs !== null ? { readyMs } : {}),
        },
      };
    } finally {
      this.active--;
    }
  }
}
