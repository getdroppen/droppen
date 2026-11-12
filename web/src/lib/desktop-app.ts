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
 *
 * The same bridge is what Settings → Desktop app talks through (part 9):
 * `openInBrowser`, `setStartAtLogin`, `openLoginItems` and
 * `refreshDesktopSettings` ask the app, and the state it pushes is what the
 * section draws (`desktop_settings.rs` on the app side).
 */
import { serverPlatform } from "./auth";

/** The systems the app is built for. */
export type DesktopSystem = "mac" | "linux";

/** What the app's init script puts on the window; a browser's pages have none. */
interface DesktopBridge {
  startAtLogin?: unknown;
  trayHost?: unknown;
  settingsState?: () => unknown;
  settings?: (bridge: string, state: unknown) => void;
  openInBrowser?: () => void;
  setStartAtLogin?: (on: boolean) => void;
  openLoginItems?: () => void;
  refreshDesktopSettings?: () => void;
}

function desktopBridge(): DesktopBridge | undefined {
  if (typeof window === "undefined") return undefined;
  return (window as unknown as { __droppenDesktop?: DesktopBridge }).__droppenDesktop;
}

/** Whether this page is the desktop app's own window (its init script is there). */
export function isDesktopWindow(): boolean {
  return desktopBridge() !== undefined;
}

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
  const said = desktopBridge()?.startAtLogin;
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
  const said = desktopBridge()?.trayHost;
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

/** The Start at Login state the app pushes to its Settings → Desktop app. */
export interface DesktopSettingsState {
  /** It is on: registered, approved or waiting for approval. */
  checked: boolean;
  /** The switch can change it: not from an App Translocation copy. */
  enabled: boolean;
  /** macOS needs the user in Login Items, or the last turn-on failed. */
  note: "needsApproval" | "turnOnFailed" | null;
}

/** The pushed state, checked down to the shape the section draws. */
export function parseDesktopSettings(raw: unknown): DesktopSettingsState | null {
  if (typeof raw !== "object" || raw === null) return null;
  const state = raw as Record<string, unknown>;
  if (typeof state.checked !== "boolean" || typeof state.enabled !== "boolean") return null;
  const note = state.note === "needsApproval" || state.note === "turnOnFailed" ? state.note : null;
  return { checked: state.checked, enabled: state.enabled, note };
}

/** What the app last pushed, or null before it has (and in a browser). */
export function desktopSettingsState(): DesktopSettingsState | null {
  return parseDesktopSettings(desktopBridge()?.settingsState?.());
}

/** Tell the app the section is open: it reads the platform and pushes the state back. */
export function refreshDesktopSettings(): void {
  desktopBridge()?.refreshDesktopSettings?.();
}

/** Open this device's page in its browser, signed in — the tray item's action. */
export function openInBrowser(): void {
  desktopBridge()?.openInBrowser?.();
}

/** Turn Start at Login on or off; the app pushes the real state back. */
export function setDesktopStartAtLogin(on: boolean): void {
  desktopBridge()?.setStartAtLogin?.(on);
}

/** macOS's Login Items page, for an item waiting for approval. */
export function openLoginItems(): void {
  desktopBridge()?.openLoginItems?.();
}
