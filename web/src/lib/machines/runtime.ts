/*
 * One machine as the page reaches it, and the shell that says which machines
 * there are (`docs/local-aggregator.md` §6.1).
 *
 * A {@link MachineRuntime} is everything the app needs to talk to one
 * machine — its transport and typed client — plus what a page with several of
 * them needs to know about each without opening anything: a
 * {@link MachineInfo} store (is it online, is the page on it or on its copy,
 * how is it reached) and {@link MachineRuntime.ensureLive}, which asks for the
 * live connection only when something is about to use it. A shell
 * (`shells/`) makes the runtimes and registers them (`registry.ts`).
 */
import type { ApiClient } from "@/api/client";
import type { MachineMirror } from "@/lib/hub/mirror";
import type { Transport } from "@/lib/transport/types";
import type { DeviceSessions } from "@/features/machines/needs-you";
import type { MachineKey } from "./key";
import { shallowEqual, type Store } from "./store";

/** What the page knows about a machine, without asking it anything. */
export interface MachineInfo {
  readonly key: MachineKey;
  /** Its name to show (the machine's own, else its id). */
  readonly name: string;
  /** Whether it is up, as the page can tell. */
  readonly state: "online" | "offline" | "unknown";
  /** Since when it has been in `state` (ms; the page's clock). */
  readonly since: number;
  /** How the live connection reaches it, once there is one that says (`hub`: no relay could carry it). */
  readonly path?: { readonly kind: "direct" | "relay" | "hub"; readonly relay?: string; readonly rttMs: number | null };
  /** How current its offline copy is (the newest time the machine signed, ms); absent with no copy. */
  readonly copyAsOf?: number;
  readonly agentVersion?: string;
  readonly os?: string;
  /** Whether what this device does on it carries the account's management rights. */
  readonly canManage: boolean;
  /**
   * What the page shows of it: the machine itself (`live`), its offline copy
   * (`copy`), the copy while the live connection is being made
   * (`connecting`), the copy because the machine will not take this device
   * until another vouches for the account's newest devices (`stale`), or the
   * copy because the machine does not accept control from this device at all
   * — its "who can control it", or this is a limited device (`refused`).
   */
  readonly live: "live" | "copy" | "connecting" | "stale" | "refused";
  /**
   * The page can only ever show its copy, read-only: another device in the
   * local app, through the local gateway, before that can reach devices live
   * (S1-7; S2 opens channels). Nothing new can be started on it here.
   */
  readonly readOnly?: boolean;
}

/**
 * Whether two {@link MachineInfo} say the same thing of a machine: every field
 * shallow-compared, the path by its own fields (a fresh object on every read).
 */
export function sameMachineInfo(a: MachineInfo, b: MachineInfo): boolean {
  if (a === b) return true;
  const { path: pathA, ...restA } = a;
  const { path: pathB, ...restB } = b;
  if (!shallowEqual(restA, restB)) return false;
  return pathA?.kind === pathB?.kind && pathA?.relay === pathB?.relay && pathA?.rttMs === pathB?.rttMs;
}

/** One machine as the page reaches it. */
export interface MachineRuntime {
  readonly key: MachineKey;
  /**
   * How the machine is reached. Read at each request and each connection
   * attempt, never captured: a socket stays on the transport that opened it
   * until it next reconnects.
   */
  readonly transport: Transport;
  /** Its typed `/api/*` client, sent through {@link transport}. */
  readonly api: ApiClient;
  /** What the page knows about it, as it changes. */
  info(): Store<MachineInfo>;
  /**
   * Something is about to use the machine itself — a session opened, a
   * terminal, a message sent: make the live connection if there is none (a
   * machine is otherwise shown from its copy, and nothing is dialled for it).
   */
  ensureLive(): void;
  /**
   * Something on the page is using the machine for as long as it is there —
   * a pane showing one of its sessions or its terminal, one of its pages:
   * make the live connection ({@link ensureLive}) and keep it while held.
   * Returns the release; held twice, it is released twice. A shell that does
   * not count holds treats one as {@link ensureLive}.
   */
  hold(): () => void;
}

/**
 * Which shell the page is: the local app (`local`), the local app showing
 * the account's other devices through its gateway too (`gateway`, S1-7), or
 * droppen.app (`hub`). Only droppen.app has no {@link LOCAL}.
 */
export type ShellKind = "local" | "gateway" | "hub";

/** What a shell holds of one machine that the views every mode shares read: its sessions and its copy. */
export interface ShellMachine {
  readonly sessions: Store<DeviceSessions>;
  readonly mirror: MachineMirror;
}

/** The machines a page has, and which one is its home (§6.1). */
export interface AppShell {
  readonly kind: ShellKind;
  /** Every machine's info, as the list changes. */
  machines(): Store<readonly MachineInfo[]>;
  runtime(key: MachineKey): MachineRuntime;
  /** Where the person's own things are kept (the screens' layout, plugin documents). */
  readonly home: MachineKey;
  /** The shell's own machine `key` (its sessions and copy), or `null` when it does not hold it. */
  machine(key: MachineKey | null | undefined): ShellMachine | null;
}
