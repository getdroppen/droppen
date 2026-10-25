/*
 * droppen.app's shell (`docs/local-aggregator.md` §6.2, `HubShell`): every
 * machine of the account on the page at once.
 *
 * Each active machine of the account's chain gets a runtime (`lib/machines`)
 * the moment the shell sees it, and loses it the moment the chain drops it:
 *
 * - its offline copy, followed from the hub (`MachineMirror` over a
 *   `HubCopySource`, kept on the device), which is what the page shows of it
 *   until it is wanted live — so listing every machine costs no channel;
 * - its end-to-end channel (`MachineChannel`), opened only once something
 *   asks for it ({@link HubMachine.ensureLive}: the machine the person is
 *   looking at, or one a pane holds), and its `SwitchingTransport` between
 *   the two, moved by its own `ModeController`; let go again once the
 *   machine has been out of view, and held by nothing, for {@link IDLE_MS};
 * - a {@link MachineInfo} store: online or not, live or on its copy, how it
 *   is reached, how old its copy is.
 *
 * What it follows from the hub is bounded: a machine out of view and held
 * by nothing follows only its index and status; the one in view and the
 * held ones also their warm sessions and the ones opened here; and a page-wide `StreamBudget` keeps the total well under
 * the hub's cap, giving way on sessions, never on an index or a status. The
 * device is swept of other accounts' copies once per shell, not per copy.
 *
 * One machine is in view at a time (the **focus**; `null`: 「我的设备」, at
 * `/devices` — `devices-route.ts`): the one the app's address is about
 * (`/m/<machine>/…`, `lib/machines/routes.ts`), followed as the router moves
 * ({@link HubShell.follow}). Others are on the page too, in panes beside it
 * (`docs/local-aggregator.md` §6.5): each is held while something shows it
 * ({@link HubMachine.hold}) — live, and never let go of — and a machine
 * neither in view nor held is not dialled at all. One machine is the **main
 * device** (§6.5 「主设备」), the registry's home: it keeps the screens, and
 * the plain addresses are its. It is the account's, kept on the hub
 * (`GET`/`PUT /v1/main-device`, `docs/hub-api/http.md`), so every browser and
 * phone of the account shows the same screens; this browser keeps a copy
 * for before the hub answers, or when it cannot. It does not follow the
 * machine in view; it moves only when the person makes another the main
 * device ({@link HubShell.setMain}) — here or on another of their devices —
 * or it leaves the account, and only then does the app's tree start over
 * (`app/root-layout.tsx`). Switching machines is
 * moving the address — no reload, the other machines' channels, copies and
 * query caches stay as they are. Everything that depends on a machine being
 * live or not is per machine: a switch between a machine and its copy
 * re-asks that machine's queries (`invalidateMachine`) and nothing else.
 *
 * This replaces `CloudContext`, which described the one machine the page had
 * open and needed a reload to open another.
 */
import type { QueryClient } from "@tanstack/react-query";
import i18next from "i18next";
import type { KeyStore } from "@/lib/e2e";
import type { CloudAccount } from "@/lib/hub/account";
import { type AccountMain, accountMain, type HubApi, HubApiError } from "@/lib/hub/api";
import { HubCopySource, StreamBudget } from "@/lib/hub/copy-source";
import { agentKeyEpoch, checkAgentKey, flushPendingReencrypts, linkCourier, noteAgentKeyState, pendingReencrypts, postMachineKey, type RenewalCourier } from "@/lib/hub/deliver";
import type { HubLink, PresenceState } from "@/lib/hub/link";
import { MachineChannel } from "@/lib/hub/machine";
import { deviceTransact, MachineMirror, mirrorSeal, sweepCopies, type Transact } from "@/lib/hub/mirror";
import type { RelayPool } from "@/lib/hub/relays";
import type { MachineKey } from "@/lib/machines/key";
import { invalidateMachine } from "@/lib/machines/query";
import { closeMachine, homeMachine, isOpen, type MachineDriver, runtimeFor, setHome } from "@/lib/machines/registry";
import { machinePath, splitMachinePath } from "@/lib/machines/routes";
import type { AppShell, MachineInfo, MachineRuntime } from "@/lib/machines/runtime";
import { enterHubShell, openHubMachine } from "@/lib/machines/shells/hub";
import { constantStore, createStore, type Store, useStore, type WritableStore } from "@/lib/machines/store";
import { onPageResume } from "@/lib/page-resume";
import { type Pipe, type PipeMessage, type PipeOpener, tunnelTransport } from "@/lib/transport/tunnel";
import { feedHubFor } from "@/app/global-feed";
import { copySessions, type SessionsStore } from "@/features/machines/copy-sessions";
import { agentVersionOf, systemLabel } from "@/features/machines/device-facts";
import { ModeController, SwitchingTransport } from "./cloud-transport";
import { DEVICES_PATH, isShellPagePath } from "./devices-route";
import { shownName } from "./machine-name";
import { offlineTransport, type OfflineOptions } from "./offline-transport";

/** What the offline copy says in the app, in the page's language at the moment it is said. */
const offlineMessages: OfflineOptions["messages"] = {
  get readOnly() {
    return i18next.t("the device is offline — this is a read-only copy");
  },
  get noCopy() {
    return i18next.t("the device is offline, and there is no copy of this session here yet");
  },
  get unavailable() {
    return i18next.t("the device is offline — this needs it to be online");
  },
};

/** How often the info's round trip may change; a change of path shows at once (as the bar's indicator). */
const PATH_EVERY_MS = 5_000;

/** How long a machine out of view keeps its channel (the local gateway's idle time too, §6.3). */
export const IDLE_MS = 5 * 60_000;
/** How often a machine whose channel is still carrying something (a download, an upload) looks again before letting it go. */
const RELEASE_RECHECK_MS = 15_000;

/** A pipe that says when it is done, whichever end ends it. */
class CountedPipe implements Pipe {
  onmessage: ((message: PipeMessage) => void) | null = null;
  onclose: ((code: number) => void) | null = null;

  constructor(
    private readonly inner: Pipe,
    private readonly done: () => void,
  ) {
    inner.onmessage = (message) => this.onmessage?.(message);
    inner.onclose = (code) => {
      this.done();
      this.onclose?.(code);
    };
  }

  send(message: PipeMessage): void {
    this.inner.send(message);
  }

  close(): void {
    this.done();
    this.inner.close();
  }
}

/** `channel` as the transport's pipe opener, keeping `open` as the pipes still open on it. */
function countingOpener(channel: MachineChannel, open: Set<Pipe>): PipeOpener {
  return {
    open(kind, meta) {
      const pipe: Pipe = new CountedPipe(channel.open(kind, meta), () => open.delete(pipe));
      open.add(pipe);
      return pipe;
    },
    onOpen: (fn) => channel.onOpen(fn),
  };
}

export interface HubShellOptions {
  api: HubApi;
  account: CloudAccount;
  link: HubLink;
  relays: RelayPool;
  /** This browser's key store (the machines' keys, the copies' seal). */
  store: KeyStore;
  queryClient: QueryClient;
  /** The page's clock (tests). */
  now?: () => number;
  /** How long a machine out of view keeps its channel (default {@link IDLE_MS}). */
  idleMs?: number;
  /** Where the copies are kept (default this browser's device copy; tests pass a store of their own). */
  transact?: Transact;
  /** How long after a failed write of the person's main device it is tried again, doubling up to a minute (default {@link MAIN_RETRY_MS}). */
  mainRetryMs?: number;
}

/** A choice of main device the hub could not take (unreachable, unavailable) is sent again after this, doubling up to {@link MAIN_RETRY_MAX_MS}, and whenever the link opens. */
export const MAIN_RETRY_MS = 5_000;
const MAIN_RETRY_MAX_MS = 60_000;

/** What a machine is shown as, its info compared field by field (the path's too). */
function sameInfo(a: MachineInfo, b: MachineInfo): boolean {
  return (
    a.key === b.key &&
    a.name === b.name &&
    a.state === b.state &&
    a.since === b.since &&
    a.copyAsOf === b.copyAsOf &&
    a.agentVersion === b.agentVersion &&
    a.os === b.os &&
    a.canManage === b.canManage &&
    a.live === b.live &&
    a.path?.kind === b.path?.kind &&
    a.path?.relay === b.path?.relay &&
    a.path?.rttMs === b.path?.rttMs
  );
}

/**
 * One machine of the account on droppen.app: its copy, its channel, the
 * transport between them and what moves it, and what the page knows of it.
 * The registry's driver for it (`lib/machines/registry.ts`).
 */
export class HubMachine implements MachineDriver {
  readonly channel: MachineChannel;
  readonly source: HubCopySource;
  readonly mirror: MachineMirror;
  readonly transport: SwitchingTransport;
  readonly controller: ModeController;
  readonly info: WritableStore<MachineInfo>;
  /**
   * Its sessions as its copy lists them (the index it follows anyway): what
   * the devices dashboard and the cross-device "needs you" read of a machine
   * out of view (`features/machines`), without dialling it.
   */
  readonly sessions: SessionsStore;
  /** Somebody asked for it live ({@link ensureLive}): the channel is kept open from then on. */
  private wanted = false;
  /** It is the machine in view ({@link setInView}): never let go of. */
  private inView = false;
  /**
   * How many things on the page are using it ({@link hold}): a pane showing
   * one of its sessions or its terminal, one of its pages, the new-session
   * dialog on it. Held, it is never let go of either, and its sessions are
   * followed as the machine in view's are.
   */
  private holds = 0;
  /** Pipes still open on its channel through the transport: requests, downloads, uploads, sockets. */
  private readonly pipes = new Set<Pipe>();
  private readonly offs: (() => void)[] = [];
  private presenceSince: { state: MachineInfo["state"]; since: number };
  private path: { info: MachineInfo["path"]; at: number } = { info: undefined, at: 0 };
  private checking = false;
  private checkedEpoch: number | null | undefined;
  private flushing = false;
  private disposed = false;
  private idleTimer: ReturnType<typeof setTimeout> | undefined;

  constructor(
    readonly key: MachineKey,
    private readonly opts: HubShellOptions,
    budget: StreamBudget | null = null,
  ) {
    const { api, account, link, relays, store, queryClient } = opts;
    const now = opts.now ?? Date.now;
    // Its channel, through a relay (or the hub); it may also run over a direct path next to it.
    this.channel = new MachineChannel(link, account, key, { direct: true, relays });
    // Its copy on this device, from the hub, opened here with its keys as the account comes to hold them.
    this.source = HubCopySource.following({
      account,
      machine: key,
      keys: () => account.machineKeys(key, store),
      client: {
        link,
        fetchPage: (stream, page) => api.streamPage(account.session.token, stream, page),
        // A record under an epoch not held yet: look for the keys again
        // (the account's change then reaches the source).
        onMissingKey: () => void account.refreshKeys().catch((error: unknown) => console.warn("cloud: keys:", error)),
      },
      budget,
      // A machine just added from another page: its first key may reach the keyring after its chain entry.
      refresh: () => account.refreshKeys(),
    });
    // Its index and status only, until it is in view (`setInView`); the shell sweeps the device once for every copy.
    this.mirror = new MachineMirror({
      account: account.account,
      machine: key,
      source: this.source,
      seal: mirrorSeal(store),
      transact: opts.transact,
      followSessions: false,
      sweep: false,
    });
    void this.mirror.start();
    // Shown as the app shows a copy: while the machine is not online (or its
    // copy has gone stale), nothing on it is running.
    this.sessions = copySessions(key, this.mirror, { online: () => this.presence() === "online", now: opts.now });
    // Every request for it goes to the machine end to end — or, until the
    // channel is open and whenever the machine is away, to its offline copy
    // on this device, which draws at once.
    const offline = offlineTransport(this.mirror, {
      user: `cloud:${account.deviceId}`,
      messages: offlineMessages,
      machineOnline: () => this.presence() === "online",
    });
    this.transport = new SwitchingTransport({ live: tunnelTransport(countingOpener(this.channel, this.pipes)), offline }, "offline", {
      onOpened: (source, id) => this.mirror.touch(source, id),
    });
    this.controller = new ModeController({
      link,
      channel: this.channel,
      machine: key,
      transport: this.transport,
      relayOnline: () => relays.online(key),
      wanted: () => this.wanted,
      onSwitch: () => {
        // This machine's queries re-ask whichever side it is on now (and no
        // other machine's), and its feed socket, closed by the switch,
        // reconnects at once rather than after a backoff.
        void invalidateMachine(queryClient, key);
        setTimeout(() => {
          // Not for a machine closed meanwhile: that would make it a feed hub again.
          if (!this.disposed) feedHubFor(key).kick();
        }, 0);
      },
    });
    this.presenceSince = { state: this.presence(), since: now() };
    this.info = createStore(this.read(), sameInfo);
    const refresh = () => this.refresh();
    this.offs.push(
      link.onPresence(refresh),
      relays.onChange(refresh),
      this.channel.onStatus((status) => {
        if (status === "open") this.recheckKey(true);
        // Open while nobody wants it (something late dialled it): let it go again in a while.
        if (status === "open" && !this.wanted && !this.inUse && this.idleTimer === undefined) this.idleAfter(this.opts.idleMs ?? IDLE_MS);
        this.flushReencrypts();
        refresh();
      }),
      this.channel.onPaths(() => {
        const next = this.pathNow();
        const held = this.path;
        if (next?.kind !== held.info?.kind || next?.relay !== held.info?.relay || now() - held.at >= PATH_EVERY_MS) refresh();
      }),
      this.transport.onMode(refresh),
      this.mirror.onChange((change) => {
        if (change.kind !== "session") refresh();
      }),
      account.onChange(() => {
        this.recheckKey();
        this.flushReencrypts();
        refresh();
      }),
    );
    this.controller.start();
  }

  private presence(): PresenceState {
    if (this.opts.relays.online(this.key)) return "online";
    return this.opts.link.presence.get(this.key)?.state ?? "unknown";
  }

  private pathNow(): MachineInfo["path"] {
    const paths = this.channel.paths;
    const route = this.channel.route;
    if (this.channel.status !== "open" || (!paths && !route)) return undefined;
    if (paths?.direct) return { kind: "direct", rttMs: paths.directRttMs };
    if (route?.kind === "relay") return { kind: "relay", relay: route.relay.id, rttMs: paths?.relayRttMs ?? null };
    return { kind: "hub", rttMs: paths?.relayRttMs ?? null };
  }

  private read(): MachineInfo {
    const now = this.opts.now ?? Date.now;
    const state = this.presence();
    if (state !== this.presenceSince.state) this.presenceSince = { state, since: now() };
    const path = this.pathNow();
    this.path = { info: path, at: now() };
    const status = this.mirror.status?.state ?? null;
    const agentVersion = agentVersionOf(status);
    const os = systemLabel(status);
    const channel = this.channel.status;
    const live: MachineInfo["live"] =
      this.transport.current === "live"
        ? "live"
        : channel !== "open" && this.channel.refusal?.kind === "notAccepted"
          ? "refused"
          : channel !== "open" && this.channel.refusal?.kind === "unconfirmed"
            ? "stale"
            : this.wanted && state !== "offline" && (channel === "connecting" || channel === "open")
              ? "connecting"
              : "copy";
    return {
      key: this.key,
      name: shownName(this.key, status) ?? this.key,
      state,
      since: this.presenceSince.since,
      ...(path ? { path } : {}),
      ...(this.mirror.freshestMs !== null ? { copyAsOf: this.mirror.freshestMs } : {}),
      ...(agentVersion ? { agentVersion } : {}),
      ...(os ? { os } : {}),
      canManage: this.opts.account.isAdmin !== false,
      live,
    };
  }

  private refresh(): void {
    if (this.disposed) return;
    this.info.set(this.read());
    this.sessions.refresh();
  }

  /** Make the live connection, and keep it: something is about to use the machine. */
  ensureLive(): void {
    if (this.disposed) return;
    clearTimeout(this.idleTimer);
    this.idleTimer = undefined;
    // (After a release the page is already on the copy, so this dials at once.)
    if (!this.wanted) {
      this.wanted = true;
      this.refresh();
    }
    this.controller.poke();
  }

  /** In view or held: something on the page is about it. */
  private get inUse(): boolean {
    return this.inView || this.holds > 0;
  }

  /**
   * Something on the page uses it until the returned release: live, its
   * channel kept, its sessions followed. The last release on a machine out of
   * view starts the idle time ({@link setInView}).
   */
  hold(): () => void {
    if (this.disposed) return () => undefined;
    this.holds += 1;
    this.mirror.setFollowSessions(true);
    this.ensureLive();
    let released = false;
    return () => {
      if (released || this.disposed) return;
      released = true;
      this.holds -= 1;
      if (this.inUse) return;
      this.mirror.setFollowSessions(false);
      if (this.wanted || this.channel.status === "open") this.idleAfter(this.opts.idleMs ?? IDLE_MS);
    };
  }

  /** How many things hold it now (diagnostics, tests). */
  get holders(): number {
    return this.holds;
  }

  /**
   * In view or not. In view: live, and its warm and opened sessions followed.
   * Out of view and held by nothing (a pane, a page of it: {@link hold}):
   * only its index and status followed, and its channel let go after
   * `idleMs` — nothing on the page uses it, and coming back opens it again.
   */
  setInView(on: boolean): void {
    if (this.disposed) return;
    this.inView = on;
    this.mirror.setFollowSessions(this.inUse);
    if (on) return this.ensureLive();
    if (this.inUse) return;
    if (this.wanted || this.channel.status === "open") this.idleAfter(this.opts.idleMs ?? IDLE_MS);
  }

  private idleAfter(ms: number): void {
    clearTimeout(this.idleTimer);
    this.idleTimer = setTimeout(() => this.release(), ms);
    (this.idleTimer as { unref?: () => void }).unref?.();
  }

  /**
   * Let the channel go: not wanted any more; the page shows the machine from
   * its copy at once, and dials nothing for it until it is wanted again —
   * which then dials straight away. Not while the channel is still being made
   * (it would install itself after a close), nor while it still carries
   * something (a download or an upload started before the switch): then it
   * looks again a little later.
   */
  private release(): void {
    this.idleTimer = undefined;
    if (this.disposed || this.inUse) return;
    if (this.channel.status === "connecting" || this.pipes.size > 0) {
      this.idleAfter(Math.min(RELEASE_RECHECK_MS, this.opts.idleMs ?? IDLE_MS));
      return;
    }
    this.wanted = false;
    this.controller.toCopy();
    this.channel.close();
    this.refresh();
  }

  /**
   * After every channel open: is the agent on the chain's newest key for it?
   * If not, hand it over here. Only a device that manages the account hands a
   * machine its key (it holds the user key that opens it).
   */
  recheckKey(force = false): void {
    const { account } = this.opts;
    if (!account.isAdmin || this.disposed) return;
    const latest = account.chain?.latestMachineEpoch(this.key) ?? null;
    if (this.channel.status !== "open" || this.checking || (!force && latest === this.checkedEpoch)) return;
    this.checking = true;
    this.checkedEpoch = latest;
    void checkAgentKey({
      latest,
      agentEpoch: () => agentKeyEpoch(this.channel),
      keyBytes: (epoch) => account.machineKeyBytes(this.key, epoch),
      // No key leaves a page whose view forked, however long the check took to get here.
      post: (body) => {
        account.assertNotForked();
        return postMachineKey(this.channel, body);
      },
    })
      .then((state) => noteAgentKeyState(this.key, state))
      .catch((error: unknown) => console.warn("cloud: agent key check:", error))
      .finally(() => {
        this.checking = false;
      });
  }

  /** A re-encryption asked for while the channel could not open (the key went by mailbox) is asked again once it does. */
  private flushReencrypts(): void {
    const { account } = this.opts;
    if (this.channel.status !== "open" || this.flushing || this.disposed || !pendingReencrypts(account.account).some((p) => p.machine === this.key)) return;
    this.flushing = true;
    void flushPendingReencrypts(account.account, this.key, (item) => account.pinnedMachineKey(item), (body) => {
      account.assertNotForked();
      return postMachineKey(this.channel, body);
    })
      .catch((error: unknown) => console.warn("cloud: re-encryption:", error))
      .finally(() => {
        this.flushing = false;
      });
  }

  /** Stop everything kept for it: the controller, the channel, the copy (what it still had to save goes with it). */
  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    clearTimeout(this.idleTimer);
    for (const off of this.offs.splice(0)) off();
    this.controller.stop();
    // Nothing late — a retry, a refetch of a view still unmounting — dials it again.
    this.transport.dispose();
    this.channel.close();
    this.sessions.close();
    this.mirror.close();
    this.source.close();
  }
}

/** The app's sections whose page is the same on any machine (a plugin's is `/p/<id>/`). */
const SECTIONS = ["sessions", "chat", "terminals", "files", "settings", "stats"];

/**
 * Where the app goes when it switches to another machine: the section it was
 * in — not the old machine's session, screen, terminal or folder, which the
 * new one does not have — written as the home writes it. An address that
 * names its machine (`/m/<machine>/…`) is read without it.
 */
export function switchPath(pathname: string): string {
  const [, first = "", second = ""] = splitMachinePath(pathname).path.split("/");
  if (first === "p" && second) return `/p/${second}/`;
  return SECTIONS.includes(first) ? `/${first}` : "/sessions";
}

/**
 * `address` (path and search) on `key`: as it is when it is already `key`'s
 * page, else `key`'s page of the same section (`/m/<key>/…`, or the plain
 * address on the home) — a screen, an app or a plugin, which only the home
 * has, become the sessions.
 */
export function addressOn(key: MachineKey, address: string, home: MachineKey): string {
  const [pathname = "/", search = ""] = address.split(/(?=\?)/);
  const { machine, path } = splitMachinePath(pathname);
  if ((machine ?? home) === key) return machine === null || key !== home ? address : `${path}${search}`;
  const section = switchPath(path);
  return machinePath(key, section.startsWith("/p/") ? "/sessions" : section, home);
}

/** The app's page of one session (`/sessions/$source/$id`). */
export function sessionPath(source: string, id: string): string {
  return `/sessions/${encodeURIComponent(source)}/${encodeURIComponent(id)}`;
}

const selectedKey = (account: string) => `rh.hub.machine.${account}`;
/** This browser's copy of the account's main device (the hub keeps the account's own: {@link HubShell}). */
const mainKey = (account: string) => `rh.hub.home.${account}`;

/** The machine this browser last had in view, for `account`. */
export function selectedMachine(account: string): string | null {
  try {
    return localStorage.getItem(selectedKey(account));
  } catch {
    return null;
  }
}

function rememberSelected(account: string, machine: string | null): void {
  try {
    if (machine) localStorage.setItem(selectedKey(account), machine);
    else localStorage.removeItem(selectedKey(account));
  } catch {
    // nothing to remember it in: the next load lands on the list
  }
}

/**
 * The main device this browser keeps `account`'s screens on (§6.5 「主设备」),
 * once there is one: its copy of the account's, from the hub, or what it chose
 * itself while the hub could not say.
 */
export function mainMachine(account: string): string | null {
  try {
    return localStorage.getItem(mainKey(account));
  } catch {
    return null;
  }
}

function rememberMain(account: string, machine: string): void {
  try {
    localStorage.setItem(mainKey(account), machine);
  } catch {
    // nothing to remember it in: the next load derives it again
  }
}

/** The `location` of the page as a path and search, or `null` outside a page. */
function here(): string | null {
  return typeof location === "undefined" ? null : `${location.pathname}${location.search}`;
}

export class HubShell implements AppShell {
  readonly kind = "hub" as const;
  private readonly held = new Map<MachineKey, HubMachine>();
  /** Per machine, the subscription that keeps {@link machines} in step with its info. */
  private readonly listening = new Map<MachineKey, () => void>();
  private readonly list: WritableStore<readonly MachineInfo[]> = createStore<readonly MachineInfo[]>([]);
  private readonly focused = createStore<MachineKey | null>(null);
  /** The machine the app last showed (`null`: none yet on this page). */
  private shown: MachineKey | null = null;
  /** The app's address when the page last went to 「我的设备」 (or loaded there from a deep link): where opening a device goes back to. */
  private appAddress: string | null = null;
  /** {@link openAt} set the address itself: {@link select} leaves it be. */
  private pinned = false;
  /**
   * Nothing remembered on this browser for the account (no main device, no
   * device last in view) and no device opened yet: an address naming no
   * device waits on 「我的设备」 rather than opening on the default main device.
   */
  private fresh = false;
  /** {@link start} has run: the chain gaining the account's first device makes it the main device. */
  private started = false;
  /** The main device this page chose ({@link setMain}), whatever the browser's storage kept. */
  private main: MachineKey | null = null;
  /**
   * {@link main} is only this page's default (the chain's first that runs
   * AI, nothing remembered on this browser, nothing chosen or heard): not one
   * to seed the account with ({@link adopt}).
   */
  private derived = false;
  /** The account's main device as the hub last said it (`null`: nothing heard on this page — offline, or a hub from before). */
  private heard: AccountMain | null = null;
  /** A main device the person chose here that the hub has not taken yet (it could not be reached): asked again a little later, and when the link opens. */
  private choosing: MachineKey | null = null;
  /** The next try of {@link choosing}, and how many failed before it. */
  private retry: { timer: ReturnType<typeof setTimeout> | undefined; failed: number } = { timer: undefined, failed: 0 };
  private readonly offs: (() => void)[] = [];
  private stopped = false;
  /** Every machine's hub streams, counted against one budget: they share the page's one hub link. */
  readonly budget = new StreamBudget();

  constructor(readonly opts: HubShellOptions) {}

  get api(): HubApi {
    return this.opts.api;
  }

  get account(): CloudAccount {
    return this.opts.account;
  }

  get link(): HubLink {
    return this.opts.link;
  }

  get relays(): RelayPool {
    return this.opts.relays;
  }

  /**
   * Open every active machine of the account, follow the chain to open the
   * ones it gains and close the ones it loses, make the main device the home,
   * and bring into view the machine the address is about — the main device
   * for an address that names none, if there is one yet — or 「我的设备」.
   */
  start(): void {
    enterHubShell();
    // The page back (in view, online again): the hub link reconnects or is
    // checked now, and the relays' failures from before are forgiven, their
    // list fetched again if old — before the machines' own tries, which
    // listen after this.
    this.offs.push(
      onPageResume(() => {
        this.opts.link.wake();
        this.opts.relays.wake();
      }),
    );
    // The account's main device as the hub keeps it: said after every sign-in
    // on the link, and again whenever a device of the account changes it.
    this.offs.push(
      this.opts.link.onControl((message) => {
        if (message.t !== "main_device") return;
        const main = accountMain(message);
        if (main) this.hear(main, true);
      }),
      this.opts.link.onState((state) => {
        // The person's choice the hub could not take then: again, now it can.
        if (state === "open" && this.choosing !== null) void this.claim(this.choosing, false);
      }),
    );
    this.sync();
    this.offs.push(this.opts.account.onChange(() => this.sync()));
    // A save still trailing the state it saves goes out as the page does.
    const flush = () => {
      for (const m of this.held.values()) void m.mirror.flush();
    };
    globalThis.addEventListener?.("pagehide", flush);
    this.offs.push(() => globalThis.removeEventListener?.("pagehide", flush));
    // Back or forward moves the address; the page has to follow it here.
    // With the app on the page its router follows the address, and the bar
    // the router (`follow`); only `/devices` and `/account` are the shell's own: onto them,
    // its page, not the app's Not Found. Off them while one is
    // shown (the app's router is not mounted): into the app on the device
    // the address is about — the main device for an address naming none —
    // at the address the browser went to, which is then the app's address to
    // come back to. With no main device yet, 「我的设备」 keeps its own
    // address.
    const popped = () => {
      if (this.stopped) return;
      const atDevices = isShellPagePath(location.pathname);
      if (this.focused.get() !== null) {
        if (atDevices) this.select(null);
        return;
      }
      if (atDevices) return;
      const key = this.deviceOf(location.pathname);
      if (key) {
        this.appAddress = here();
        this.pinned = true;
        try {
          this.select(key);
        } finally {
          this.pinned = false;
        }
      } else globalThis.history?.replaceState(globalThis.history.state, "", DEVICES_PATH);
    };
    globalThis.addEventListener?.("popstate", popped);
    this.offs.push(() => globalThis.removeEventListener?.("popstate", popped));
    // The main device until the hub says the account's: the one this browser
    // remembers (its copy of the account's), else — upgrading from a build
    // where the home followed the device in view — the one last in view, so
    // the screens and plain addresses of before open where they did; else,
    // on a browser that remembers neither, the account's first device that
    // runs AI, in the chain's order (§6.5): the same on every browser, not
    // whichever happens to be opened first.
    const account = this.opts.account.account;
    const remembered = [mainMachine(account), selectedMachine(account)].find((key): key is string => !!key && this.held.has(key)) ?? null;
    this.fresh = remembered === null;
    const main = remembered ?? this.active().find((key) => this.held.has(key)) ?? null;
    if (main) this.moveMain(main, remembered === null);
    this.started = true;
    // At `/devices` or `/account`: that page. Elsewhere: the device the address is
    // about, or 「我的设备」 (the address kept) when it names none and this
    // browser remembers nothing yet.
    const atDevices = typeof location !== "undefined" && isShellPagePath(location.pathname);
    const key = atDevices || typeof location === "undefined" ? null : this.deviceOf(location.pathname);
    if (key) {
      this.pinned = true;
      try {
        this.select(key);
      } finally {
        this.pinned = false;
      }
    } else this.select(null);
    // Other accounts' copies, and plaintext from before copies were sealed:
    // swept once for every machine, after their copies have been read.
    void Promise.all([...this.held.values()].map((m) => m.mirror.whenLoaded())).then(() => {
      if (!this.stopped) return sweepCopies(this.opts.transact ?? deviceTransact, account);
    });
  }

  /**
   * The device an app address is about, if the shell has it: the one it
   * names (`/m/<machine>/…`), else the main device — unless this browser
   * remembers nothing yet ({@link fresh}).
   */
  private deviceOf(pathname: string): MachineKey | null {
    const named = splitMachinePath(pathname).machine;
    if (named !== null) return this.held.has(named) ? named : null;
    return !this.fresh && this.held.has(homeMachine()) ? homeMachine() : null;
  }

  /** The account's active machines, in the chain's order. */
  private active(): MachineKey[] {
    return (this.opts.account.chain?.hosts() ?? []).filter((m) => m.status.type === "active").map((m) => m.info.id);
  }

  private sync(): void {
    if (this.stopped) return;
    const want = this.active();
    for (const key of want) {
      if (this.held.has(key)) continue;
      const machine = new HubMachine(key, this.opts, this.budget);
      this.held.set(key, machine);
      openHubMachine(key, machine.transport, machine);
      this.listening.set(key, machine.info.subscribe(() => this.publish()));
    }
    for (const key of [...this.held.keys()]) if (!want.includes(key)) this.remove(key);
    if (this.started) {
      // The account's main device, once this page's chain has it too.
      this.adopt(false);
      // An account that had no device that runs AI when the page started: its
      // first one is the main device. Asked of this page's memory (`main`), not
      // the browser's storage, which may keep nothing: a main device chosen on
      // this page stays chosen as the chain changes.
      if (this.main === null || !this.held.has(this.main)) {
        const first = want.find((key) => this.held.has(key));
        if (first) this.moveMain(first, true);
      }
    }
    this.publish();
  }

  /**
   * A machine the chain no longer has: out of view (to 「我的设备」) if it was
   * in it, closed, and — the main device — replaced as the main by the
   * device in view, else the first left in the chain's order. The account
   * takes that one unless another page of it was first (the hub cleared the
   * one that left; `if_unset`): then that one, when the hub answers.
   */
  private remove(key: MachineKey): void {
    const inView = this.focused.get();
    if (inView === key) this.select(null);
    if (homeMachine() === key) {
      const other =
        inView !== null && inView !== key && this.held.has(inView) ? inView : this.active().find((k) => k !== key && this.held.has(k));
      // With none left, it stays the (closed) home until another machine is opened.
      if (other) {
        this.moveMain(other);
        void this.claim(other, true);
      }
    }
    this.held.delete(key);
    this.listening.get(key)?.();
    this.listening.delete(key);
    // The home only when it was the last machine: its feeds and copy go with it all the same.
    closeMachine(key, { evenTheHome: true });
  }

  private publish(): void {
    const order = this.active();
    this.list.set(order.filter((k) => this.held.has(k)).map((k) => this.held.get(k)!.info.get()));
  }

  machines(): Store<readonly MachineInfo[]> {
    return this.list;
  }

  runtime(key: MachineKey): MachineRuntime {
    if (!this.held.has(key)) throw new Error(`no machine "${key}" in this account`);
    return runtimeFor(key);
  }

  /** The cloud half of `key` (its channel, its copy), or `null` for a machine the shell does not have. */
  machine(key: MachineKey | null | undefined): HubMachine | null {
    return key ? (this.held.get(key) ?? null) : null;
  }

  /** The machine in view — the one the app's address is about (`null`: 「我的设备」), as it changes. */
  get focus(): Store<MachineKey | null> {
    return this.focused;
  }

  /** The machine the app last showed, while the account still has it: 「我的设备」's way back to it. */
  get lastShown(): MachineKey | null {
    return this.shown !== null && this.held.has(this.shown) ? this.shown : null;
  }

  /** The main device (§6.5 「主设备」): where the screens are kept, and what an address naming no device is about. */
  get home(): MachineKey {
    return homeMachine();
  }

  /**
   * The person makes `key` the main device (「设为主设备」): here at once
   * ({@link moveMain}), and for the whole account — the hub keeps it, and
   * every other page of the account follows (`PUT /v1/main-device`). Offline,
   * it is this page's (and this browser's) until the hub can be told: tried
   * again a little later, and whenever the link opens.
   */
  setMain(key: MachineKey): void {
    if (this.stopped || !this.held.has(key)) return;
    this.moveMain(key);
    void this.claim(key, false);
  }

  /**
   * Tell the hub `key` is the account's main device — or, `ifUnset`, that it
   * is unless the account has one already — and follow its answer, the one
   * that stands. The person's choice (`!ifUnset`) that could not reach the
   * hub (or that it could not take now: a 5xx, a 429) is sent again after
   * {@link MAIN_RETRY_MS}, doubling up to a minute, and whenever the link
   * opens; one it refuses (a hub from before, the device gone meanwhile)
   * stays this page's own. A claim
   * `ifUnset` that fails is not repeated here: the hub says the account has
   * none again with the next connection.
   */
  private async claim(key: MachineKey, ifUnset: boolean): Promise<void> {
    if (!ifUnset) {
      if (this.choosing !== key) this.retry.failed = 0;
      this.choosing = key;
      clearTimeout(this.retry.timer);
      this.retry.timer = undefined;
    }
    let main: AccountMain;
    try {
      main = await this.opts.api.setMainDevice(this.opts.account.session.token, key, { ifUnset });
    } catch (error) {
      console.warn("cloud: main device:", error);
      if (ifUnset || this.stopped || this.choosing !== key) return;
      const refused = error instanceof HubApiError && error.status >= 400 && error.status < 500 && error.status !== 408 && error.status !== 429;
      if (refused) {
        this.choosing = null;
        return;
      }
      // Not reached, or not now: again in a while (and as soon as the link opens).
      const wait = Math.min(MAIN_RETRY_MAX_MS, (this.opts.mainRetryMs ?? MAIN_RETRY_MS) * 2 ** this.retry.failed);
      this.retry.failed += 1;
      clearTimeout(this.retry.timer);
      this.retry.timer = setTimeout(() => {
        this.retry.timer = undefined;
        if (!this.stopped && this.choosing === key) void this.claim(key, false);
      }, wait);
      (this.retry.timer as { unref?: () => void }).unref?.();
      return;
    }
    if (this.stopped) return;
    if (!ifUnset) {
      // A later choice is on its way: its answer is the one to follow.
      if (this.choosing !== key) return;
      this.choosing = null;
      this.retry.failed = 0;
    }
    this.hear(main, false);
  }

  /**
   * What the hub says the account's main device is — after each sign-in on
   * the link, when a device of the account changes it, or as a write's answer
   * — unless it is older than what this page heard last (`set_ms`).
   * `fromHub`: said by the hub of itself, not answering this page.
   */
  private hear(main: AccountMain, fromHub: boolean): void {
    if (this.stopped) return;
    const before = this.heard;
    if (before?.setMs != null && main.setMs !== null && main.setMs < before.setMs) return;
    this.heard = main;
    this.adopt(fromHub);
  }

  /**
   * Follow the account's main device as the hub last said it. A host this
   * page has (its own verified chain) becomes the main device — the app's
   * tree starts over only if it is another one — and one it does not have
   * yet waits for the chain ({@link sync}); the hub's word alone opens
   * nothing. None (never chosen, upgrading from a build that kept it per
   * browser, or it left the account): `seed` — the hub said so of itself — has
   * the account take this page's main device unless another page was first
   * (`if_unset`), but not one this page only derived (nothing remembered or
   * chosen here: a browser that never chose does not decide for the others).
   * Nothing while the person's own choice is on its way to the hub.
   */
  private adopt(seed: boolean): void {
    const heard = this.heard;
    if (this.stopped || !this.started || heard === null || this.choosing !== null) return;
    if (heard.machine !== null) {
      if (this.held.has(heard.machine)) this.moveMain(heard.machine);
      return;
    }
    if (seed && this.main !== null && this.held.has(this.main) && !this.derived) void this.claim(this.main, true);
  }

  /**
   * Make `key` the main device on this page: the screens, the layouts and the
   * plain addresses are its from here on, and the app's tree starts over on
   * it (`app/root-layout.tsx` is keyed by the home) — only if it is another
   * one. This browser keeps it as its copy for the next load. The page in
   * view stays the page it was: a plain address of the old main device is
   * rewritten to name it (`/m/<old>/…`) — or, a screen or an app of it, which
   * only the main device has, becomes the sessions — and an address naming
   * `key` becomes plain. `derived`: only this page's default ({@link derived}).
   */
  private moveMain(key: MachineKey, derived = false): void {
    if (this.stopped || !this.held.has(key)) return;
    this.main = key;
    this.derived = derived;
    rememberMain(this.opts.account.account, key);
    const old = homeMachine();
    if (old === key && isOpen(old)) return;
    const address = here();
    setHome(key);
    if (address === null || this.focused.get() === null || isShellPagePath(location.pathname)) return;
    const [pathname = "/", search = ""] = address.split(/(?=\?)/);
    const { machine, path } = splitMachinePath(pathname);
    const owner = machine ?? old;
    const next =
      owner === key
        ? `${path}${search}`
        : SECTIONS.includes(path.split("/")[1] ?? "")
          ? `${machinePath(owner, path, key)}${search}`
          : "/sessions";
    if (next !== address) globalThis.history?.replaceState(globalThis.history.state, "", next);
  }

  /**
   * Bring `key` into view — it goes live, and the app shows one of its pages
   * — or go to 「我的设备」 (`null`). Remembered for the next load. No
   * reload: every other machine stays as it is, and so does the main device.
   *
   * From 「我的设备」 the app comes back at the address it had, when that is
   * `key`'s page, else at `key`'s page of the same section. With the app on
   * the page, the address moves to `key`'s page of the section it is in;
   * the bar's switch navigates itself, and the shell follows the address
   * ({@link follow}).
   */
  select(key: MachineKey | null): void {
    if (this.stopped) return;
    if (key !== null && !this.held.has(key)) return;
    rememberSelected(this.opts.account.account, key);
    if (key !== null) this.fresh = false;
    this.moveAddress(key);
    this.show(key);
  }

  /** Put `key` in view (and the one before out of it), the address left as it is. */
  private show(key: MachineKey | null): void {
    const before = this.focused.get();
    if (before !== null && before !== key) this.held.get(before)?.setInView(false);
    if (key !== null) {
      this.shown = key;
      this.held.get(key)!.setInView(true);
    }
    this.focused.set(key);
  }

  /**
   * The app's address is about `key` now (the router moved: a link, the
   * bar's switch, Back): bring it into view, the address as it is. Nothing
   * while 「我的设备」 is shown — the app is not on the page — or for a
   * machine the shell does not have. (The router's own state is what says
   * so: the browser's address may trail it by a moment, as its history
   * writes are queued. A router mounted again under an address it has not
   * loaded yet renders nothing until it has: `app/root-layout.tsx`.)
   */
  follow(key: MachineKey): void {
    if (this.stopped || this.focused.get() === null || this.focused.get() === key || !this.held.has(key)) return;
    rememberSelected(this.opts.account.account, key);
    this.show(key);
  }

  /**
   * Bring `key` into view on one of its own pages, `path` — a session of it,
   * its terminals, written as the home's (`/sessions/…`) — at its address
   * (`/m/<key>/…`, plain on the main device). Opening the session asks for
   * the machine live, as switching to it does.
   */
  openAt(key: MachineKey, path: string): void {
    if (this.stopped || !this.held.has(key)) return;
    this.fresh = false;
    const address = machinePath(key, path, homeMachine());
    if (here() !== address) globalThis.history?.replaceState(globalThis.history.state, "", address);
    if (this.focused.get() !== null) {
      rememberSelected(this.opts.account.account, key);
      return this.show(key);
    }
    this.pinned = true;
    try {
      this.select(key);
    } finally {
      this.pinned = false;
    }
  }

  /**
   * Keep the address in step with what is in view (`devices-route.ts`):
   * 「我的设备」 is `/devices` (the account's settings `/account`, kept as it is), and the app's address is kept for when a
   * device is opened from it ({@link addressOn}). Before the app's router
   * mounts again, so it starts at the app's page and never at `/devices`.
   */
  private moveAddress(key: MachineKey | null): void {
    const history = globalThis.history;
    const address = here();
    if (address === null || !history) return;
    const atDevices = isShellPagePath(location.pathname);
    if (key === null) {
      if (atDevices) return;
      this.appAddress = address;
      history.replaceState(history.state, "", DEVICES_PATH);
      return;
    }
    if (this.pinned) return;
    const to = addressOn(key, atDevices ? (this.appAddress ?? "/sessions") : address, homeMachine());
    if (to !== address) history.replaceState(history.state, "", to);
  }

  /** Whether showing `key` leaves another machine the app was showing (its page — a session, a folder — is not `key`'s: {@link switchPath}). */
  movesAway(key: MachineKey): boolean {
    return this.shown !== null && this.shown !== key;
  }

  /** Stop every machine (sign-out, a removed device): nothing of them is saved after this. */
  stop(): void {
    if (this.stopped) return;
    this.stopped = true;
    clearTimeout(this.retry.timer);
    for (const off of this.offs.splice(0)) off();
    for (const off of this.listening.values()) off();
    this.listening.clear();
    for (const [key, machine] of this.held) {
      if (isOpen(key)) closeMachine(key, { evenTheHome: true });
      else machine.dispose();
    }
    this.held.clear();
    this.list.set([]);
  }

  /** For diagnostics (and the acceptance harness's latency probe): the machine in view's channel, copy and transport. */
  diagnostics(): Record<string, unknown> {
    const machine = this.machine(this.focused.get());
    return {
      shell: this,
      link: this.opts.link,
      relays: this.opts.relays,
      transport: machine?.transport,
      channel: machine?.channel,
      mirror: machine?.mirror,
      controller: machine?.controller,
      // Any machine's feed socket (and the session feeds on it), by key.
      feedHub: (key: MachineKey) => (this.held.has(key) ? feedHubFor(key) : null),
    };
  }
}

let current: HubShell | null = null;

/** droppen.app's shell, once the page is past sign-in and device setup; `null` in local mode. */
export function hubShell(): HubShell | null {
  return current;
}

export function setHubShell(next: HubShell | null): void {
  current = next;
}

const NO_FOCUS = constantStore<MachineKey | null>(null);

/** The machine droppen.app has in view (`null`: 「我的设备」, or local mode), as it changes. */
export function useFocusedMachine(): MachineKey | null {
  return useStore(current?.focus ?? NO_FOCUS);
}

/** Every machine droppen.app has, as each changes (empty in local mode). */
export function useShellMachines(): readonly MachineInfo[] {
  return useStore(current?.machines() ?? NO_MACHINES);
}
const NO_MACHINES = constantStore<readonly MachineInfo[]>([]);

/**
 * A courier for machine keys (`linkCourier`) that hands a key over in the
 * channel the page already has open to that machine, when it has one.
 */
export function shellCourier(link: HubLink, account: CloudAccount): RenewalCourier {
  return linkCourier(link, account, (machine) => current?.machine(machine)?.channel, current?.relays);
}
