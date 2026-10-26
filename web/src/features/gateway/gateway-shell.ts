/*
 * The local app's gateway shell (`docs/local-aggregator.md` §6.2,
 * `GatewayShell`, S1-7): the local app as it always was — `local`, the page's
 * own server, its home — and beside it every other device of the account the
 * local install's gateway shows this local account (`dir`).
 *
 * Each of those is a machine of its own on the page, read from its copy and
 * nothing else (S2 brings channels):
 *
 * - a `MachineMirror` over a `GatewayCopySource` — memory only (`persist:
 *   false`): nothing is written to IndexedDB, nothing sealed, so it works on a
 *   page that is not a secure context (a phone at the computer's LAN
 *   address); the install holds the copies and the keys;
 * - its session list off that copy (`copySessions`, at rest while the device
 *   is not online), which the unified list and 「需要你」 read
 *   (`features/machines`);
 * - the offline transport over the copy, with words for a copy that is
 *   read-only here rather than offline: every request is answered from the
 *   copy or refused (503), nothing is ever written to the device;
 * - a `MachineInfo` store: online or not (the hub's word, through the
 *   install), how current its copy is (「截至 …」), `readOnly`.
 *
 * Held (a page or a pane shows one of its sessions), a device follows its
 * warm and opened sessions through the gateway, as droppen.app's do.
 *
 * The shell exists while `GET /api/cloud` says this local account sees other
 * devices and the install's aggregator runs (`gateway-boot.tsx`); the
 * gateway sending the page away (`bye`) ends it too. Without other devices it
 * is the local shell — the same page, byte for byte.
 */
import i18next from "i18next";
import { GatewayCopySource } from "@/lib/gateway/copy-source";
import { type GatewayDevice, GatewayLink } from "@/lib/gateway/link";
import { MachineMirror, type MirrorSeal, type Transact } from "@/lib/hub/mirror";
import { machineDisplayName } from "@/lib/hub/streams";
import { LOCAL, type MachineKey } from "@/lib/machines/key";
import { type MachineDriver, runtimeFor } from "@/lib/machines/registry";
import type { AppShell, MachineInfo, MachineRuntime } from "@/lib/machines/runtime";
import { closeGatewayMachine, leaveGatewayShell, openGatewayMachine } from "@/lib/machines/shells/gateway";
import { gatewayShell, setGatewayShell } from "@/features/machines/shells";
import { createStore, type Store, type WritableStore } from "@/lib/machines/store";
import type { Transport } from "@/lib/transport/types";
import { copySessions, type SessionsStore } from "@/features/machines/copy-sessions";
import { offlineTransport, type OfflineOptions } from "@/features/cloud/offline-transport";

/** What the copy of another device says in the app, in the page's language when it is said. */
export const gatewayMessages: OfflineOptions["messages"] = {
  get readOnly() {
    return i18next.t("read-only here: this session is on another device");
  },
  get noCopy() {
    return i18next.t("there is no copy of this session on this device yet");
  },
  get unavailable() {
    return i18next.t("this needs that device itself — its sessions are read-only here");
  },
};

/** The copies are never kept on the device, so never sealed (and never opened: nothing is stored). */
const NO_SEAL: MirrorSeal = {
  seal: () => Promise.reject(new Error("the gateway's copies are not kept on this device")),
  open: () => Promise.resolve(null),
};

/** No store at all: every read finds nothing, every write goes nowhere. */
const NOWHERE: Transact = async () => undefined;

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
    a.readOnly === b.readOnly
  );
}

export interface GatewayShellOptions {
  /** Who `/api/auth/status` says is signed in, on a device's copy (the page's own local account). */
  user: string | null;
  /** The page's clock (tests). */
  now?: () => number;
}

/** Another device of the account, as the local gateway shows it: the registry's driver for it. */
export class GatewayMachine implements MachineDriver {
  readonly source: GatewayCopySource;
  readonly mirror: MachineMirror;
  /** Its sessions off its copy (at rest while it is not online). */
  readonly sessions: SessionsStore;
  readonly transport: Transport;
  readonly info: WritableStore<MachineInfo>;
  private device: GatewayDevice;
  private holds = 0;
  private disposed = false;
  private readonly offMirror: () => void;

  constructor(
    readonly key: MachineKey,
    device: GatewayDevice,
    link: GatewayLink,
    opts: GatewayShellOptions,
  ) {
    this.device = device;
    this.source = new GatewayCopySource(link, key);
    this.mirror = new MachineMirror({
      account: "gateway",
      machine: key,
      source: this.source,
      transact: NOWHERE,
      seal: NO_SEAL,
      persist: false,
      followSessions: false,
      sweep: false,
    });
    void this.mirror.start();
    const online = () => this.device.state === "online";
    this.sessions = copySessions(key, this.mirror, { online, now: opts.now });
    this.transport = offlineTransport(this.mirror, { user: opts.user, messages: gatewayMessages, machineOnline: online });
    this.info = createStore(this.read(), sameInfo);
    this.offMirror = this.mirror.onChange((change) => {
      if (change.kind !== "session") this.refresh();
    });
  }

  private read(): MachineInfo {
    const d = this.device;
    const copyAsOf = this.mirror.freshestMs ?? d.copyAsOfMs;
    const status = this.mirror.status?.state ?? null;
    return {
      key: this.key,
      name: d.name ?? machineDisplayName(status) ?? this.key,
      state: d.state,
      since: d.sinceMs ?? 0,
      ...(copyAsOf !== null ? { copyAsOf } : {}),
      ...(d.agentVersion ? { agentVersion: d.agentVersion } : {}),
      ...(d.os ? { os: d.os } : {}),
      canManage: false,
      live: "copy",
      readOnly: true,
    };
  }

  /** What the gateway says of it now. */
  update(device: GatewayDevice): void {
    this.device = device;
    this.refresh();
  }

  private refresh(): void {
    if (this.disposed) return;
    this.info.set(this.read());
    this.sessions.refresh();
  }

  /** Nothing to dial: the local app reaches other devices only through their copies here (S2 adds channels). */
  ensureLive(): void {}

  /** Something on the page shows it: its warm and opened sessions are followed while it does. */
  hold(): () => void {
    if (this.disposed) return () => undefined;
    this.holds += 1;
    this.mirror.setFollowSessions(true);
    let released = false;
    return () => {
      if (released || this.disposed) return;
      released = true;
      this.holds -= 1;
      if (this.holds === 0) this.mirror.setFollowSessions(false);
    };
  }

  dispose(): void {
    if (this.disposed) return;
    this.disposed = true;
    this.offMirror();
    this.sessions.close();
    this.mirror.close();
    this.source.close();
  }
}

export class GatewayShell implements AppShell {
  readonly kind = "gateway" as const;
  readonly home: MachineKey = LOCAL;
  private readonly devices = new Map<MachineKey, { machine: GatewayMachine; off: () => void }>();
  private readonly list: WritableStore<readonly MachineInfo[]>;
  private readonly offs: (() => void)[] = [];
  private stopped = false;

  constructor(
    readonly link: GatewayLink,
    private readonly opts: GatewayShellOptions,
  ) {
    this.list = createStore<readonly MachineInfo[]>([], (a, b) => a.length === b.length && a.every((x, i) => x === b[i]));
  }

  start(): void {
    this.offs.push(this.link.dir.subscribe(() => this.reconcile()));
    this.link.start();
    this.reconcile();
  }

  /** Open the devices the gateway lists, update the ones there, close the ones gone. */
  private reconcile(): void {
    if (this.stopped) return;
    const dir = this.link.dir.get() ?? [];
    for (const [key, held] of [...this.devices]) {
      if (dir.some((d) => d.id === key)) continue;
      this.devices.delete(key);
      held.off();
      closeGatewayMachine(key);
    }
    for (const device of dir) {
      if (device.id === LOCAL) continue;
      const held = this.devices.get(device.id);
      if (held) {
        held.machine.update(device);
        continue;
      }
      const machine = new GatewayMachine(device.id, device, this.link, this.opts);
      openGatewayMachine(device.id, machine.transport, machine);
      this.devices.set(device.id, { machine, off: machine.info.subscribe(() => this.publish()) });
    }
    if (this.devices.size === 0) leaveGatewayShell();
    this.publish();
  }

  private publish(): void {
    const local = runtimeFor(LOCAL).info().get();
    this.list.set(this.devices.size === 0 ? [] : [local, ...[...this.devices.values()].map((d) => d.machine.info.get())]);
  }

  /** This device first, then the others as the gateway lists them; empty while there are none. */
  machines(): Store<readonly MachineInfo[]> {
    return this.list;
  }

  runtime(key: MachineKey): MachineRuntime {
    return runtimeFor(key);
  }

  /** Another device's driver (`null`: `local`, or not shown). */
  machine(key: MachineKey | null | undefined): GatewayMachine | null {
    return key ? (this.devices.get(key)?.machine ?? null) : null;
  }

  /** Back to the local app alone: every other device closed, the socket too. */
  stop(): void {
    if (this.stopped) return;
    this.stopped = true;
    for (const off of this.offs.splice(0)) off();
    for (const [key, held] of [...this.devices]) {
      held.off();
      closeGatewayMachine(key);
    }
    this.devices.clear();
    leaveGatewayShell();
    this.link.close();
    this.list.set([]);
  }
}

/** Start the gateway shell over `link` (a running one is stopped first). */
export function startGatewayShell(link: GatewayLink, opts: GatewayShellOptions): GatewayShell {
  gatewayShell()?.stop();
  const shell = new GatewayShell(link, opts);
  setGatewayShell(shell);
  shell.start();
  return shell;
}
