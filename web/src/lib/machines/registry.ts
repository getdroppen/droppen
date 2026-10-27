/*
 * The machines this page reaches, each by its own transport and typed client
 * (`docs/local-aggregator.md` §6.1, §6.6).
 *
 * The page used to have one module-level transport and one `api`, which was
 * "the server" by assumption. Here a machine is a {@link MachineRuntime},
 * registered under its {@link MachineKey}, and everything that talks to a
 * machine names which one: `apiFor(machine)`, `transportFor(machine)`, and the
 * per-machine singletons that used to be module-level (the feed hub, the
 * session feed pool) made with {@link perMachine}.
 *
 * Which machines there are is the shell's to say (`shells/`):
 *
 * - the local shell: only {@link LOCAL}, the page's own server, reached
 *   directly. The registry is that shell until one of the others boots
 *   ({@link setShell}), so a page that boots nothing is the local page;
 * - droppen.app (`features/cloud/hub-shell.ts`): every machine of the
 *   account at once, each through its own end-to-end channel (or its offline
 *   copy), and no `local`.
 *
 * One machine is the **home**: where the page keeps what belongs to the person
 * rather than a machine — the screens' layout, plugin documents — what the
 * plain addresses (no `/m/<machine>`, `routes.ts`) are about. On
 * droppen.app it is the main device the person chose (`HubShell.setMain`);
 * the machine they look at is the address's, and it moves without the home
 * ({@link setHome} is for a new main device: the app's tree is about the home,
 * `app/root-layout.tsx`, and starts over on the new one, without a reload).
 */
import { createApiClient, type ApiClient } from "@/api/client";
import { directTransport } from "@/lib/transport/direct";
import type { Transport } from "@/lib/transport/types";
import { LOCAL, type MachineKey } from "./key";
import type { MachineInfo, MachineRuntime, ShellKind } from "./runtime";
import { constantStore, createStore, type Store } from "./store";

export type { MachineInfo, MachineRuntime, ShellKind } from "./runtime";

/** What a shell adds to a machine beyond its transport: what it knows of it, how to go live, and what to stop when it goes. */
export interface MachineDriver {
  info: Store<MachineInfo>;
  ensureLive(): void;
  /**
   * Something on the page is using the machine until the returned release is
   * called (`MachineRuntime.hold`). Optional: a driver without it is asked
   * for {@link ensureLive} instead, and nothing is counted.
   */
  hold?(): () => void;
  /** Stop everything kept for the machine (its channel, its copy). Run once it is closed. */
  dispose(): void;
}

/** A machine reached directly, the page's own server: always there, always live. */
function directDriver(key: MachineKey): MachineDriver {
  return {
    info: constantStore<MachineInfo>({ key, name: key, state: "online", since: 0, canManage: true, live: "live" }),
    ensureLive: () => undefined,
    dispose: () => undefined,
  };
}

class Runtime implements MachineRuntime {
  /** Closed while it was the home: kept only for the views still on the page (see {@link closeMachine}). */
  retired = false;
  private current: Transport;
  private client: ApiClient | null = null;

  constructor(
    readonly key: MachineKey,
    transport: Transport,
    private driver: MachineDriver,
  ) {
    this.current = transport;
  }

  info(): Store<MachineInfo> {
    return this.driver.info;
  }

  ensureLive(): void {
    this.driver.ensureLive();
  }

  hold(): () => void {
    const driver = this.driver;
    if (driver.hold) return once(driver.hold());
    driver.ensureLive();
    return () => undefined;
  }

  /** @internal */
  setDriver(driver: MachineDriver): void {
    this.driver = driver;
  }

  /** @internal */
  dispose(): void {
    this.driver.dispose();
  }

  get transport(): Transport {
    return this.current;
  }

  get api(): ApiClient {
    this.client ??= createApiClient(() => this.current);
    return this.client;
  }

  retarget(next: Transport): void {
    this.current = next;
  }
}

/** `fn`, made safe to call more than once: only the first call does anything. */
function once(fn: () => void): () => void {
  let done = false;
  return () => {
    if (done) return;
    done = true;
    fn();
  };
}

const runtimes = new Map<MachineKey, Runtime>();
const homeStore = createStore<MachineKey>(LOCAL);
let shell: ShellKind = "local";
/** The machines open on the page, as {@link openMachines} lists them; re-read on every open, close and home move. */
const openStore = createStore<readonly MachineKey[]>([LOCAL], sameKeys);

function sameKeys(a: readonly MachineKey[], b: readonly MachineKey[]): boolean {
  return a.length === b.length && a.every((key, i) => key === b[i]);
}

/** Say what is open now to whoever listens ({@link openMachinesStore}). */
function published(): void {
  openStore.set(openMachines());
}

/** The shell the page is running as. */
export function shellKind(): ShellKind {
  return shell;
}

/** The machine the person's own things live on (see the module comment). */
export function homeMachine(): MachineKey {
  return homeStore.get();
}

/** The home, as it moves (`useHomeMachine()` in `context.tsx`). */
export function homeMachineStore(): Store<MachineKey> {
  return homeStore;
}

/**
 * The runtime for `machine`.
 *
 * In the local shell {@link LOCAL} always exists, on the direct transport,
 * whether or not anything registered it. Any other machine is only there once
 * its shell has opened it: asking for one that is not is a bug, and throws.
 */
export function runtimeFor(machine: MachineKey): MachineRuntime {
  let runtime = runtimes.get(machine);
  if (!runtime) {
    if (machine !== LOCAL || shell === "hub") throw new Error(`no machine "${machine}" on this page`);
    runtime = new Runtime(LOCAL, directTransport, directDriver(LOCAL));
    runtimes.set(LOCAL, runtime);
  }
  return runtime;
}

/** `machine`'s typed client. */
export function apiFor(machine: MachineKey): ApiClient {
  return runtimeFor(machine).api;
}

/** `machine`'s transport, as it is now. */
export function transportFor(machine: MachineKey): Transport {
  return runtimeFor(machine).transport;
}

/** The machines open on this page, home first. */
export function openMachines(): MachineKey[] {
  const keys = [...runtimes.keys()].filter((key) => !runtimes.get(key)!.retired);
  if (shell !== "hub" && !runtimes.has(LOCAL)) keys.unshift(LOCAL);
  const home = homeMachine();
  return keys.sort((a, b) => Number(b === home) - Number(a === home));
}

/** The machines open on the page, home first, as they open, close and the home moves. */
export function openMachinesStore(): Store<readonly MachineKey[]> {
  return openStore;
}

/** Whether `machine` is open on the page (the local app's `local` always is). */
export function isOpen(machine: MachineKey): boolean {
  const runtime = runtimes.get(machine);
  return runtime ? !runtime.retired : machine === LOCAL && shell !== "hub";
}

/**
 * Open `machine` on the page, reached through `transport` (or point an open
 * one at a new transport), with what its shell knows of it (`driver`; a
 * machine reached directly when there is none). Meant for a shell before
 * anything has connected to the machine.
 */
export function openMachine(machine: MachineKey, transport: Transport, driver?: MachineDriver): MachineRuntime {
  const known = runtimes.get(machine);
  if (known) {
    known.retired = false;
    known.retarget(transport);
    if (driver) known.setDriver(driver);
    published();
    return known;
  }
  const runtime = new Runtime(machine, transport, driver ?? directDriver(machine));
  runtimes.set(machine, runtime);
  published();
  return runtime;
}

/**
 * Make the page a `kind` shell, and `machine` (which must be open, or `local`
 * in the local app) its home. On droppen.app there is no `local`: a runtime
 * made for it before the shell said otherwise (a stray early ask) goes, and a
 * hub shell with no machine yet has no home to speak of. The local app's
 * gateway shell keeps `local` as it is — its home — and adds the others.
 */
export function setShell(kind: ShellKind, machine?: MachineKey): void {
  shell = kind;
  if (kind === "hub") forget(LOCAL);
  if (machine !== undefined) setHome(machine);
  published();
}

/**
 * Move the home to `machine`, which must be open: everything about the home
 * (the app's tree, plugins' documents) is about it from here on. A no-op
 * when it already is.
 */
export function setHome(machine: MachineKey): void {
  runtimeFor(machine);
  homeStore.set(machine);
  published();
}

// ---------------------------------------------------------------------------
// Per-machine singletons
// ---------------------------------------------------------------------------

interface Slots {
  drop(machine: MachineKey): void;
}
const slots = new Set<Slots>();

/**
 * One `T` per machine, made on first ask: what used to be a module-level
 * singleton of the page — a feed hub, a session feed pool, a cache of pictures
 * — keyed by the machine it is about. `dispose` (if given) runs when the
 * machine is closed.
 */
export function perMachine<T>(make: (machine: MachineKey) => T, dispose?: (value: T) => void): (machine: MachineKey) => T {
  const made = new Map<MachineKey, T>();
  slots.add({
    drop(machine) {
      if (!made.has(machine)) return;
      const value = made.get(machine) as T;
      made.delete(machine);
      dispose?.(value);
    },
  });
  return (machine) => {
    if (made.has(machine)) return made.get(machine) as T;
    const value = make(machine);
    made.set(machine, value);
    return value;
  };
}

/**
 * Close `machine`: every per-machine singleton made for it (its feed socket,
 * its session feeds), then whatever its shell kept for it (`dispose`), and
 * its runtime. The home cannot be closed — move it first ({@link setHome}) —
 * unless the shell says so (`evenTheHome`: its last machine went, or the
 * page is being left). Then everything made for it goes all the same, but
 * its runtime is kept, retired — no longer open, but still there for the
 * views of it that are on the page until they go, so a last render cannot
 * fail to find it — and the next machine opened becomes the home.
 */
export function closeMachine(machine: MachineKey, opts: { evenTheHome?: boolean } = {}): void {
  const home = machine === homeMachine();
  if (home && isOpen(machine) && !opts.evenTheHome) throw new Error("the home machine cannot be closed");
  forget(machine, home);
}

/** Drop everything made for `machine`, and its runtime (`retire`: keep it, retired). */
function forget(machine: MachineKey, retire = false): void {
  for (const slot of slots) slot.drop(machine);
  const runtime = runtimes.get(machine);
  if (runtime && retire) runtime.retired = true;
  else runtimes.delete(machine);
  runtime?.dispose();
  published();
}
