/*
 * How Droppen gets onto a computer — the interface agreed with the
 * distribution work (the public `getdroppen/droppen` repository), written
 * once here and shown by the download page (`download.tsx`) and by
 * droppen.app's 「添加可运行 AI 的设备」 guide (`features/cloud/add-computer.tsx`):
 *
 * - **the Droppen App for macOS** (Tauri, `feat/desktop-app`), what people
 *   are pointed to: `Droppen-<version>.dmg` (universal) attached to the
 *   release `v<version>`. Opening it starts Droppen with no login page.
 *   Until the first one is published ({@link APP_AVAILABLE}), every place that
 *   offers it says it is coming and offers the background install instead.
 *   Signing it in to an account from inside it (「登录 Droppen」, through the
 *   system browser: docs/local-aggregator.md §4.9) is not built yet
 *   ({@link APP_SIGN_IN_AVAILABLE}): until it is, the download page says it is
 *   coming, and the 「添加可运行 AI 的设备」 guide adds a Mac through the
 *   background install;
 * - **the install command**, for headless and background installs (a Mac
 *   running Droppen in the background, Linux servers): droppen.app answers
 *   `/install.sh` with a redirect to the script in that repository. A fresh
 *   install starts the local server at login, creates the first local login
 *   (printed or asked for) and opens the local page;
 * - **Linux servers** enrol without a browser, with a one-time code
 *   (`DROPPEN_CODE`) ({@link LINUX_AVAILABLE});
 * - the manual download: that repository's latest release, every file with
 *   its SHA-256 (`SHA256SUMS`, which the script checks before installing);
 *   and `--uninstall`, which removes the server and keeps its data
 *   (`scripts/install.sh`).
 */

/** Whether the first Droppen App for macOS is published: off, the App is "coming soon" everywhere and nothing links to a file that is not there. */
export const APP_AVAILABLE = true;

/**
 * Whether the Droppen App can sign itself in to Droppen (「登录 Droppen」 in
 * the App, through the system browser: docs/local-aggregator.md §4.9). Off —
 * the App hides its Droppen section today — nothing tells anyone to click a
 * button the App does not have: the download page says signing in from the
 * App is coming in an update, and the 「添加可运行 AI 的设备」 guide adds a
 * Mac through the background install, which signs in from its own page.
 */
export const APP_SIGN_IN_AVAILABLE = false;

/** Whether the install script runs on Linux and takes `DROPPEN_CODE`: off, Linux is "coming". */
export const LINUX_AVAILABLE = true;

/** What the page offers, as one switch each (tests and previews pass others). */
export interface Availability {
  app: boolean;
  /** Whether the App signs in to Droppen ({@link APP_SIGN_IN_AVAILABLE}); only read where {@link Availability.app} is on. */
  appSignIn: boolean;
  linux: boolean;
}

export const AVAILABILITY: Availability = { app: APP_AVAILABLE, appSignIn: APP_SIGN_IN_AVAILABLE, linux: LINUX_AVAILABLE };

/** The download page on droppen.app. */
export const DOWNLOAD_PATH = "/download";

/** The ways onto a device the download page knows, each a tab (`/download?os=…`). */
export const DOWNLOAD_OSES = ["macos-app", "macos-bg", "linux", "windows", "mobile"] as const;
export type DownloadOs = (typeof DOWNLOAD_OSES)[number];

/** The download page on `os`'s tab. */
export function downloadHref(os?: DownloadOs | null): string {
  return os ? `${DOWNLOAD_PATH}?os=${os}` : DOWNLOAD_PATH;
}

/** The tab an address names (`?os=`), if it names one. */
export function osOfSearch(search: string): DownloadOs | null {
  const os = new URLSearchParams(search).get("os");
  return (DOWNLOAD_OSES as readonly string[]).includes(os ?? "") ? (os as DownloadOs) : null;
}

/** Every release, with its files (the App's `Droppen-<version>.dmg` among them) and their SHA-256 checksums. */
export const RELEASES_URL = "https://github.com/getdroppen/droppen/releases/latest";

/** Where the App's download button goes: the latest release, whose universal `.dmg` is the one to take. */
export const APP_DOWNLOAD_URL = RELEASES_URL;

/** The one line that installs Droppen to run in the background (a Mac; Linux servers once {@link LINUX_AVAILABLE}). */
export const INSTALL_COMMAND = "curl -fsSL https://droppen.app/install.sh | sh";

/**
 * The same script on a device with no browser: it adds itself to the account
 * with a one-time code. The code goes in the environment of the shell that
 * runs the script, never on a command line, which other users of that device
 * can read; a hub other than droppen.app goes alongside it.
 */
export function codeInstallCommand(code: string, hub?: string): string {
  const where = hub && otherHub(hub) ? `DROPPEN_HUB=${hub} ` : "";
  return `curl -fsSL https://droppen.app/install.sh | ${where}DROPPEN_CODE=${code} sh`;
}

/**
 * Where a server keeps the binary it runs (its data directory's `bin`): the installer puts it on no `PATH`.
 *
 * Still the old name, on purpose. This command goes to a server of unknown
 * version (it is not in the account yet, so nothing has reported one), and only
 * servers from the release that ships `droppen` have `<data>/bin/droppen`
 * beside it; every server ever installed answers to this path.
 * TODO(droppen-name): say `~/.local/share/remote-hand/bin/droppen` once the
 * oldest agent version the hub reports for accounts has reached that release
 * (docs/local-aggregator.md §7.2, S5-1).
 */
export const SERVER_BIN_PATH = "~/.local/share/remote-hand/bin/remote-hand-server";

/** For a Droppen server already there, installed some other way: the code on standard input (`printf` is a shell builtin, so it is in no process's arguments). */
export function stdinEnrollCommand(code: string, hub: string): string {
  return `printf '%s\\n' '${code}' | ${SERVER_BIN_PATH} agent enroll - --hub ${hub}`;
}

/** The same script, removing what it installed. */
export const UNINSTALL_COMMAND = "curl -fsSL https://droppen.app/install.sh | sh -s -- --uninstall";

/** How to check a downloaded file against the checksum the release lists. */
export const CHECKSUM_COMMAND = "shasum -a 256 <file>";

/** The hub droppen.app is: a computer signed in elsewhere needs the hub address set under 「高级」. */
export const DROPPEN_ORIGIN = "https://droppen.app";

/** Whether the page is on a hub other than droppen.app (a self-hosted or a development one). */
export function otherHub(origin: string): boolean {
  return origin.replace(/\/+$/, "") !== DROPPEN_ORIGIN;
}
