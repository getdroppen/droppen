/*
 * Getting from one device that runs AI to two on one screen, on droppen.app,
 * without anyone to walk you through it:
 *
 * - the hints that say it can be done, while the account has a single device
 *   that runs AI — on 「我的设备」, in the device switcher and in the pane
 *   picker (`add-device`) — and the one-time tip, once it has two, that every
 *   pane can show a different device (`side-by-side`); each put away for good
 *   on this browser by dismissing it, and the same everywhere it shows;
 * - the way to 「我的设备」's 「添加可运行 AI 的设备」 guide from any of them
 *   ({@link openAddDevice});
 * - what the guide's last step watches for (`watchNewHost`): a device that
 *   runs AI the account did not have when the guide opened, read off the
 *   shell's machine list (the account's chain, followed live) and each
 *   machine's presence — nothing is dialled for it.
 */
import { useSyncExternalStore } from "react";
import type { MachineKey } from "@/lib/machines/key";
import type { MachineInfo } from "@/lib/machines/runtime";
import { createStore } from "@/lib/machines/store";
import { hubShell } from "@/features/machines/shells";

/** The hints a browser can put away: adding a second device, and the one-time side-by-side tip. */
export type Hint = "add-device" | "side-by-side";

const hintKey = (hint: Hint, account: string) => `rh.hub.hint.${hint}.${account}`;

/** Bumped as a hint is put away, so every place it shows hears of it. */
const hintsVersion = createStore(0);

/** Whether `hint` was put away on this browser for `account`. */
export function hintDismissed(hint: Hint, account: string): boolean {
  try {
    return localStorage.getItem(hintKey(hint, account)) === "1";
  } catch {
    return false;
  }
}

/** Put `hint` away for `account` on this browser, everywhere it shows. */
export function dismissHint(hint: Hint, account: string): void {
  try {
    localStorage.setItem(hintKey(hint, account), "1");
  } catch {
    // nothing to remember it in: put away for this page only
  }
  dismissedHere.add(hintKey(hint, account));
  hintsVersion.set(hintsVersion.get() + 1);
}

/** Put away on this page even where the browser keeps nothing. */
const dismissedHere = new Set<string>();

/** Whether `hint` shows for `account` (`null`: no account, nothing shows), as it is put away. */
export function useHint(hint: Hint, account: string | null): boolean {
  useSyncExternalStore(hintsVersion.subscribe, hintsVersion.get, hintsVersion.get);
  if (!account) return false;
  return !dismissedHere.has(hintKey(hint, account)) && !hintDismissed(hint, account);
}

/** The guide asked for by a way in (a hint), taken by 「我的设备」 as it opens. */
export const addAsked = createStore(false);

/** Open 「我的设备」 with the 「添加可运行 AI 的设备」 guide open, from anywhere on the page. */
export function openAddDevice(): void {
  addAsked.set(true);
  hubShell()?.select(null);
}

/** Where the guide's last step stands: nothing new yet, a new device in the account, or that device ready (online, or given up waiting for its presence). */
export type NewHost =
  | { kind: "waiting" }
  | { kind: "joined"; info: MachineInfo }
  | { kind: "ready"; info: MachineInfo };

/**
 * The newest device that runs AI and was not in the account when the guide
 * opened (`before`), and whether it is up: `ready` once its presence says
 * it is online — or, past `patienceMs` since it joined (`joinedAt`), as it
 * stands, so a device whose presence is slow to arrive does not hold the
 * guide forever.
 */
export function watchNewHost(input: {
  before: ReadonlySet<MachineKey>;
  machines: readonly MachineInfo[];
  joinedAt?: number | null;
  now?: number;
  patienceMs?: number;
}): NewHost {
  const fresh = input.machines.filter((m) => !input.before.has(m.key));
  const info = fresh.find((m) => m.state === "online") ?? fresh.at(-1) ?? null;
  if (!info) return { kind: "waiting" };
  if (info.state === "online") return { kind: "ready", info };
  const waited = input.joinedAt != null && input.now != null ? input.now - input.joinedAt : 0;
  return waited >= (input.patienceMs ?? 20_000) ? { kind: "ready", info } : { kind: "joined", info };
}

/**
 * The device to put beside a new one (`added`): the one the app last showed,
 * else the main device, else the first other device that runs AI — `null`
 * while the account has no other.
 */
export function partnerFor(added: MachineKey, input: { lastShown: MachineKey | null; home: MachineKey | null; machines: readonly MachineKey[] }): MachineKey | null {
  const others = input.machines.filter((key) => key !== added);
  for (const candidate of [input.lastShown, input.home]) if (candidate && others.includes(candidate)) return candidate;
  return others[0] ?? null;
}
