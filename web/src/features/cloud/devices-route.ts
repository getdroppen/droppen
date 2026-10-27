/*
 * Where 「我的设备」 lives on droppen.app (docs/local-aggregator.md §3.7): one
 * page at `/devices`, shown whenever no device is in view (`HubShell.focus`
 * is `null`) — every device of the account in one list, the devices that run
 * AI among them, with what the plan uses above it.
 *
 * The two pages it replaces never had an address of their own: the machine
 * list was "no machine in view" at whatever path the app had last shown, and
 * the devices page was drawn over the app. Their ways in — the bar's
 * buttons, "change relay…", the no-copy cover, the machine that left the
 * chain — now all land here (`openMyDevices`), and an address the app had
 * (a deep link, the page it was on) is kept to go back to when a device is
 * opened (`HubShell.select`).
 *
 */
import { createStore } from "@/lib/machines/store";

export const DEVICES_PATH = "/devices";

/**
 * The account's own settings (「账户」: its e-mail, how it signs in, the
 * recovery words, signing out) — a page of the same kind as 「我的设备」: no
 * device in view, its own address, reached from the bar and from
 * 「我的设备」.
 */
export const ACCOUNT_PATH = "/account";

/** Whether `pathname` is 「我的设备」. */
export function isDevicesPath(pathname: string): boolean {
  return pathname.replace(/\/+$/, "") === DEVICES_PATH;
}

/** Whether `pathname` is the account's settings. */
export function isAccountPath(pathname: string): boolean {
  return pathname.replace(/\/+$/, "") === ACCOUNT_PATH;
}

/** Whether `pathname` is one of the shell's own pages, shown with no device in view: 「我的设备」 or the account's settings. */
export function isShellPagePath(pathname: string): boolean {
  return isDevicesPath(pathname) || isAccountPath(pathname);
}

/** A part of the page a way in asks for (the relay picker: `relays`), taken by the page as it opens — or, already open, scrolled to. */
export const askedPlace = createStore<{ anchor: string } | null>(null);

/**
 * A passkey the account's settings are asked to open the removal of, by its
 * credential hash (hex): a device that joined with it was refused as not the
 * person's (`passkey-join.tsx`), so the passkey itself — which still signs in
 * — should go. Taken once, by the settings as they open.
 */
let askedRemoval: string | null = null;

/** Ask the account's settings, about to open, to open the removal of the passkey whose credential hash is `credHash`. */
export function askRemoval(credHash: string): void {
  askedRemoval = credHash;
}

/** The removal asked for, once (`null`: none). */
export function takeAskedRemoval(): string | null {
  const asked = askedRemoval;
  askedRemoval = null;
  return asked;
}
