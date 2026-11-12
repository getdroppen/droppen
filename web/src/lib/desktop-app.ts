/*
 * The Droppen desktop app as the web app's words about it need it: the system
 * it runs on, where the names differ (the Mac's menu bar, a Linux desktop's
 * system tray; the edition), whether it has Start at Login, and whether a tray
 * host shows its icon.
 *
 * The system is the server's (`GET /api/auth/status`, `platform`): the app runs
 * its server on its own device, and the words are about that device — on a
 * page opened from another device too, whose browser says nothing about it,
 * and on the setup page, before anyone is signed in. (The guide's steps 01
 * and 03 read the same from the tools report they draw, `os.platform`.)
 * Start at Login and the tray host are the app's own to say
 * (`desktop/src-tauri/src/login_item.rs`, `OFFERED`; `src/tray.rs`,
 * `host_present_at_start`), through its window's bridge
 * (`desktop/src-tauri/src/init.js`). A page without the bridge — a browser on
 * the device — takes both as yes, as before.
 */
import { serverPlatform } from "./auth";

/** The systems the app is built for. */
export type DesktopSystem = "mac" | "linux";

/**
 * A Node-style platform (`darwin`, `linux`) as one of the app's systems:
 * Linux as itself; anything else as the Mac — `darwin`, and a server from
 * before `platform`, when the Mac app was the only one.
 */
export function desktopSystemOf(platform: string | null | undefined): DesktopSystem {
  return platform === "linux" ? "linux" : "mac";
}

/** The system of the device this page's server runs on. Synchronous for the reason `isDesktopApp` is. */
export function desktopSystem(): DesktopSystem {
  return desktopSystemOf(serverPlatform());
}

/** What the app's window says of Start at Login, if this page is in its window and it says. */
function bridgeStartAtLogin(): boolean | undefined {
  if (typeof window === "undefined") return undefined;
  const said = (window as unknown as { __droppenDesktop?: { startAtLogin?: unknown } }).__droppenDesktop?.startAtLogin;
  return typeof said === "boolean" ? said : undefined;
}

/**
 * Whether the app has Start at Login, for the words that point at it: what
 * its window's bridge says; where nothing says — a browser on the device —
 * yes, as every app has it: the Mac app's login item, the Linux app's XDG
 * autostart entry.
 */
export function startAtLoginOffered(said: boolean | undefined = bridgeStartAtLogin()): boolean {
  return said ?? true;
}

/** What the app's window says of a tray host, if this page is in its window and it says. */
function bridgeTrayHost(): boolean | undefined {
  if (typeof window === "undefined") return undefined;
  const said = (window as unknown as { __droppenDesktop?: { trayHost?: unknown } }).__droppenDesktop?.trayHost;
  return typeof said === "boolean" ? said : undefined;
}

/**
 * Whether a tray host shows the app's icon, for the words that point at it:
 * what its window's bridge says; where nothing says — a browser on the device
 * — yes, as before. Only the app can tell: a page outside it has no session
 * bus to ask.
 */
export function trayHostPresent(said: boolean | undefined = bridgeTrayHost()): boolean {
  return said ?? true;
}
