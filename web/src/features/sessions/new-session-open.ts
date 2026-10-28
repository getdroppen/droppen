/*
 * Opening the new-session view (`new-session-view.tsx`): the chord, the hook
 * that navigates to it, and its route's search. Apart from the view because
 * the layout and the session list use these on every page, and the view is
 * loaded only when it is first shown (`app/lazy-views.ts`).
 */
import { useCallback, useEffect } from "react";
import { useMachineNavigate } from "@/app/machine-route";
import { canStartOn, defaultDevice } from "@/features/machines/device-picker";
import { isDesktopApp } from "@/lib/auth";
import { useHomeMachine, useMachine, useOpenMachines } from "@/lib/machines/context";
import type { MachineKey } from "@/lib/machines/key";
import { runtimeFor } from "@/lib/machines/registry";
import { isNewSessionChord, lastDevice, newSessionShortcut } from "./new-session";
import { agentKey, parseAgentKey, type SessionSource } from "./new-session-flow";

export const IS_MAC = typeof navigator !== "undefined" && /Mac|iPhone|iPad|iPod/.test(navigator.platform || navigator.userAgent);

/** The new-session chord as this window takes it (⌘N in the desktop app, ⇧⌘O in a browser), for hints. */
export function useNewSessionShortcut(): string {
  return newSessionShortcut(IS_MAC, isDesktopApp());
}

/** What {@link useOpenNewSession} opens the view with; all optional. */
export interface OpenNewSession {
  /** The device to start on; by default the view's, else the one last started on, else the home. */
  machine?: MachineKey;
  /** The directory to start in, absolute ("new session here", a folder picked elsewhere). */
  cwd?: string;
  /** A first message, put in the composer in place of what was typed there; nothing is sent until the person sends it. */
  draft?: string;
  /** The agent: claude, codex or an ACP agent (acp, with `acpAgent`). */
  source?: SessionSource;
  /** Which configured ACP agent, by id, when `source` is `acp`. */
  acpAgent?: string;
}

/**
 * Open the new-session view: on `machine`, or — with none named — on the
 * device a new session defaults to (the view's, else the one last started on,
 * else the home; one that can start it), optionally with a directory, a first
 * message and an agent already filled in (`?cwd=…&draft=…&agent=…`, which the
 * view takes into its draft and drops from the address).
 */
export function useOpenNewSession(here?: MachineKey): (options?: OpenNewSession) => void {
  const view = useMachine();
  const home = useHomeMachine();
  const machines = useOpenMachines();
  const navigate = useMachineNavigate();
  return useCallback(
    (options = {}) => {
      const machine =
        options.machine ??
        defaultDevice({
          open: machines,
          here: here ?? view,
          last: machines.length > 1 ? lastDevice(localStorage) : null,
          home,
          usable: (m) => canStartOn(runtimeFor(m).info().get()),
        });
      const search: NewSessionSearch = {
        ...(options.cwd ? { cwd: options.cwd } : {}),
        ...(options.draft !== undefined ? { draft: options.draft } : {}),
        ...(options.source ? { agent: agentKey({ source: options.source, acpAgent: options.acpAgent ?? null }) } : {}),
      };
      void navigate({ to: "/sessions/new", machine, ...(Object.keys(search).length ? { search: { ...search } } : {}) });
    },
    [view, here, home, machines, navigate],
  );
}

/**
 * Cmd+Shift+O (Ctrl+Shift+O elsewhere) opens a new session — and Cmd+N
 * (Ctrl+N) in the desktop app, whose window is handed it — on `here` (the
 * routed view's device) by default.
 */
export function NewSessionHotkey({ here, desktop = isDesktopApp() }: { here: MachineKey; desktop?: boolean }) {
  const open = useOpenNewSession(here);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.defaultPrevented || !isNewSessionChord(event, IS_MAC, desktop)) return;
      // A terminal's Ctrl+N is the shell's (the next history line), not ours.
      if (event.target instanceof Element && event.target.closest(".xterm")) return;
      event.preventDefault();
      open();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, desktop]);
  return null;
}

export interface NewSessionSearch {
  /** The directory to start in ("new session here"). */
  cwd?: string;
  /** A first message to fill in (not sent). */
  draft?: string;
  /** The agent: `claude`, `codex`, or `acp:<id>` for a configured ACP agent. */
  agent?: string;
}

export function validateNewSessionSearch(search: Record<string, unknown>): NewSessionSearch {
  const agent = parseAgentKey(search.agent);
  return {
    cwd: typeof search.cwd === "string" && search.cwd ? search.cwd : undefined,
    draft: typeof search.draft === "string" ? search.draft : undefined,
    agent: agent ? agentKey(agent) : undefined,
  };
}
