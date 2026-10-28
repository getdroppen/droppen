/*
 * 「放进同一个屏幕」: a new screen with two devices side by side, made from
 * 「我的设备」 the moment a second device that runs AI has joined, and opened.
 *
 * Each pane opens on something of its own device, so the screen shows the
 * payoff at once rather than two empty pickers:
 *
 * - the device's newest session, as its index copy has it — or, for a device
 *   just added whose copy has not arrived yet, as the device itself lists
 *   them; opening it is what opening any transcript asks of that device;
 * - else a new terminal on it (its shell says which device it is), which
 *   asks that device for itself, as opening a terminal does;
 * - else, the device not reachable, the picker already on that device.
 *
 * A screen is the main device's (`docs/local-aggregator.md` §6.5 「主设备」):
 * it is made there with the screens API (`POST /api/screens`, then its
 * layout `PATCH`ed onto the revision it was made at), each pane naming its
 * device the way the cross-device panes do (`PaneContent.machine`, written
 * only for a device other than the main one: `withMachine`), so its header
 * names the device. All of it happens on the click; nothing is dialled
 * before it.
 */
import type { ApiClient } from "@/api/client";
import { newPaneId, type PaneContent, type PaneNode, withMachine } from "@/app/pane-tree";
import type { MachineKey } from "@/lib/machines/key";
import { mq } from "@/lib/machines/query";
import { recentSessions } from "@/features/machines/needs-you";
import type { SwitchingTransport } from "@/lib/transport/switching";
import type { HubShell } from "./hub-shell";
import { isSessionSource, type SessionSource } from "@/features/sessions/new-session-flow";

/** A session a device's copy knows. */
export interface SessionRefLite {
  source: SessionSource;
  id: string;
}

/** What a device's pane opens on: its newest session, else a new terminal of it, else the picker on it. */
export function paneFor(machine: MachineKey, choice: { session?: SessionRefLite | null; terminal?: string | null }): PaneContent {
  if (choice.session) return { kind: "session", source: choice.session.source, id: choice.session.id, machine };
  if (choice.terminal) return { kind: "terminal", id: choice.terminal, machine };
  return { kind: "picker", machine };
}

/** Two panes side by side, each on its device, stored as the main device (`home`) stores its screens. */
export function sideBySideLayout(left: PaneContent, right: PaneContent, home: MachineKey): PaneNode {
  return {
    type: "split",
    id: newPaneId("split"),
    direction: "row",
    ratio: 0.5,
    children: [
      { type: "leaf", id: newPaneId("pane"), content: withMachine(left, machineOf(left, home), home) },
      { type: "leaf", id: newPaneId("pane"), content: withMachine(right, machineOf(right, home), home) },
    ],
  };
}

function machineOf(content: PaneContent, home: MachineKey): MachineKey {
  return "machine" in content && typeof content.machine === "string" && content.machine ? content.machine : home;
}

/** What making the screen needs of the page (the shell's, or a test's). */
export interface ScreenMaker {
  /** The main device. */
  home: MachineKey;
  /** The main device's typed client. */
  api: Pick<ApiClient, "POST" | "PATCH">;
  /** Resolves once the main device can be asked (its channel is live); rejects if it cannot be reached. */
  live(): Promise<void>;
  /** The newest session `machine`'s index copy knows, once it has one (or `null` past a short wait). */
  newest(machine: MachineKey): Promise<SessionRefLite | null>;
  /** A new terminal on `machine`, asked of the device itself; `null` when it cannot be reached. */
  terminal(machine: MachineKey): Promise<string | null>;
  /** The screens are fetched afresh as the app opens on them. */
  invalidate(): void;
  /** Open the main device's page at `path`. */
  open(path: string): void;
  /** Let go of every device this held (the panes hold what they show from here). */
  release(): void;
}

/** Why a side-by-side screen could not be made. */
export class SideBySideError extends Error {
  constructor(readonly kind: "unreachable" | "refused", message: string) {
    super(message);
  }
}

/** What `machine`'s pane opens on, asked of the maker. */
export async function contentFor(maker: Pick<ScreenMaker, "newest" | "terminal">, machine: MachineKey): Promise<PaneContent> {
  const session = await maker.newest(machine).catch(() => null);
  if (session) return paneFor(machine, { session });
  const terminal = await maker.terminal(machine).catch(() => null);
  return paneFor(machine, { terminal });
}

/** Make the screen on the main device with `left` and `right` side by side, each on something of its own, and open it: its id. */
export async function makeSideBySide(maker: ScreenMaker, input: { title: string; left: MachineKey; right: MachineKey }): Promise<string> {
  try {
    const [left, right] = await Promise.all([contentFor(maker, input.left), contentFor(maker, input.right)]);
    const layout = sideBySideLayout(left, right, maker.home);
    await maker.live();
    const made = await maker.api.POST("/api/screens", { body: { title: input.title } });
    if (!made.data) throw new SideBySideError("refused", (made.error as { error?: string } | undefined)?.error ?? `HTTP ${made.response.status}`);
    const laid = await maker.api.PATCH("/api/screens/{id}", {
      params: { path: { id: made.data.id } },
      body: { layout: layout as never, baseRevision: made.data.revision },
    });
    if (!laid.data) throw new SideBySideError("refused", (laid.error as { error?: string } | undefined)?.error ?? `HTTP ${laid.response.status}`);
    maker.invalidate();
    maker.open(`/screens/${encodeURIComponent(made.data.id)}`);
    return made.data.id;
  } finally {
    maker.release();
  }
}

/** Resolves once `transport` is live, or rejects after `ms`. */
export function whenLive(transport: Pick<SwitchingTransport, "current" | "onMode">, ms = 30_000): Promise<void> {
  if (transport.current === "live") return Promise.resolve();
  return new Promise((resolve, reject) => {
    const off = transport.onMode((mode) => {
      if (mode !== "live") return;
      clearTimeout(timer);
      off();
      resolve();
    });
    const timer = setTimeout(() => {
      off();
      reject(new SideBySideError("unreachable", "the device could not be reached"));
    }, ms);
  });
}

/** A store's value once `ready` says so, or `null` after `ms`. */
export function whenReady<T>(store: { get(): T; subscribe(fn: () => void): () => void }, ready: (value: T) => boolean, ms: number): Promise<T | null> {
  if (ready(store.get())) return Promise.resolve(store.get());
  return new Promise((resolve) => {
    const off = store.subscribe(() => {
      if (!ready(store.get())) return;
      clearTimeout(timer);
      off();
      resolve(store.get());
    });
    const timer = setTimeout(() => {
      off();
      resolve(null);
    }, ms);
  });
}

/** How long a device's index copy may take to arrive before the device itself is asked for its sessions. */
const COPY_WAIT_MS = 3_000;
/** How long a device may take to come live (for its sessions, or a terminal) before its pane gets the picker instead. */
const TERMINAL_WAIT_MS = 15_000;

/** The shell's way to make a screen: on its main device, each device held live only while something of it is asked for. */
export function shellScreenMaker(shell: HubShell): ScreenMaker {
  const home = shell.home;
  if (!shell.machine(home)) throw new SideBySideError("unreachable", "the main device is not on this page");
  const releases: (() => void)[] = [];
  const hold = (key: MachineKey) => {
    const machine = shell.machine(key);
    if (!machine) return null;
    releases.push(machine.hold());
    return machine;
  };
  return {
    home,
    api: shell.runtime(home).api,
    live: async () => {
      const main = hold(home);
      if (!main) throw new SideBySideError("unreachable", "the main device is not on this page");
      await whenLive(main.transport);
    },
    newest: async (key) => {
      const pick = (rows: Parameters<typeof recentSessions>[0]): SessionRefLite | null => {
        const row = recentSessions(rows, 1)[0];
        return row && isSessionSource(row.source) ? { source: row.source, id: row.id } : null;
      };
      // Its index copy, when it has one (a device just added may not have it here yet)…
      const sessions = shell.machine(key)?.sessions;
      const copy = sessions ? await whenReady(sessions, (value) => value.rows !== null, COPY_WAIT_MS) : null;
      if (copy?.rows) return pick(copy.rows);
      // …else the device itself, which the pane is about to ask for anyway.
      const machine = hold(key);
      if (!machine) return null;
      await whenLive(machine.transport, TERMINAL_WAIT_MS);
      const res = await shell.runtime(key).api.GET("/api/sessions");
      return pick(res.data?.sessions ?? null);
    },
    terminal: async (key) => {
      const machine = hold(key);
      if (!machine) return null;
      await whenLive(machine.transport, TERMINAL_WAIT_MS);
      const res = await shell.runtime(key).api.POST("/api/terminals", { body: {} });
      return res.data?.id ?? null;
    },
    invalidate: () => void shell.opts.queryClient.invalidateQueries({ queryKey: mq(home, "screens") }),
    open: (path) => shell.openAt(home, path),
    release: () => {
      for (const release of releases.splice(0)) release();
    },
  };
}
