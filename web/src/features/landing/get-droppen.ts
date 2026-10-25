/*
 * How Droppen gets onto a computer — the interface agreed with the
 * distribution work (the public `getdroppen/droppen` repository), written
 * once here and shown by the download page (`download.tsx`) and by
 * droppen.app's 「添加可运行 AI 的设备」 guide (`features/cloud/add-computer.tsx`):
 *
 * - **the Droppen App for macOS** (Tauri, `feat/desktop-app`), what people
 *   are pointed to: `Droppen-<version>.dmg` (universal) attached to the
 *   release `v<version>`. The buttons download the latest one in place: the
 *   hub names it (`GET /v1/releases/latest`, `lib/hub/releases.ts`), the
 *   page links it; until it does, or when it cannot, they link the release
 *   page ({@link APP_DOWNLOAD_URL}). The page never asks GitHub for anything
 *   (its `connect-src` stays the hub and its relays). Opening it starts
 *   Droppen with no login page.
 *   Until the first one is published ({@link APP_AVAILABLE}), every place that
 *   offers it says it is coming and offers the background install instead.
 *   It signs itself in to an account from inside it (「登录 Droppen」, through
 *   the system browser: docs/local-aggregator.md §4.9;
 *   {@link APP_SIGN_IN_AVAILABLE}), so the download page and the
 *   「添加可运行 AI 的设备」 guide add a Mac through the App;
 * - **the install command**, for headless and background installs (a Mac
 *   running Droppen in the background, Linux servers): droppen.app answers
 *   `/install.sh` with the script of the release Droppen Hub was built from
 *   (that repository's `scripts/install.sh`, the release's file). A fresh
 *   install starts the local server at login, creates the first local login
 *   (printed or asked for) and opens the local page;
 * - **Linux servers** enrol without a browser, with a one-time code
 *   (`DROPPEN_CODE`) ({@link LINUX_AVAILABLE});
 * - the manual download: that repository's latest release, every file with
 *   its SHA-256 (`SHA256SUMS`, which the script checks before installing).
 *   The Linux tarballs (`droppen-v<version>-linux-<cpu>.tar.gz`) are linked
 *   after the install command when the hub names them;
 *   and `--uninstall`, which removes the server and keeps its data
 *   (`scripts/install.sh`).
 */

/** Whether the first Droppen App for macOS is published: off, the App is "coming soon" everywhere and nothing links to a file that is not there. */
export const APP_AVAILABLE = true;

/**
 * Whether the Droppen App can sign itself in to Droppen (「登录 Droppen」 in
 * the App, through the system browser: docs/local-aggregator.md §4.9). On:
 * every App published has it (the hand-off came in 0.2.125, the first public
 * `.dmg` is 0.2.127; the App's own first-run guide reads the same fact as
 * `DESKTOP_SIGN_IN_READY`, `features/setup/flags.ts`). The download page then
 * has the App's 「登录 Droppen」 as its last step, and the 「添加可运行 AI 的设备」
 * guide and 「我的设备」's first-run card add a Mac through the App. Off (a
 * preview), nothing tells anyone to click a button the App does not have: the
 * download page says signing in from the App is coming in an update, and a Mac
 * is added through the background install, which signs in from its own page.
 */
export const APP_SIGN_IN_AVAILABLE = true;

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

/** The latest release's page: every file (the App's `.dmg` among them) and their SHA-256 checksums. */
export const RELEASES_URL = "https://github.com/getdroppen/droppen/releases/latest";

/**
 * Where an App download button goes when the hub has not named the file
 * (`release-files.tsx`): the latest release, whose universal `.dmg` is the
 * one to take.
 */
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

/**
 * How to turn systemd on in WSL (Microsoft's documentation). The install
 * command runs Droppen as a systemd user service and stops where there is no
 * systemd user manager (`scripts/install.sh`), as in WSL until systemd is
 * turned on.
 */
export const WSL_SYSTEMD_URL = "https://learn.microsoft.com/windows/wsl/systemd";

/** The same script, removing what it installed. */
export const UNINSTALL_COMMAND = "curl -fsSL https://droppen.app/install.sh | sh -s -- --uninstall";

/** How to check a downloaded file against the checksum the release lists. */
export const CHECKSUM_COMMAND = "shasum -a 256 <file>";

/** The hub droppen.app is: a computer signed in elsewhere needs the hub address set under 「高级」. */
export const DROPPEN_ORIGIN = "https://droppen.app";

/**
 * droppen.app's other names: the app at `my.droppen.app` and the site at
 * `www.droppen.app` are the same hub, whose address devices dial stays
 * {@link DROPPEN_ORIGIN} (docs/hub-api/README.md, "Hosts").
 */
const DROPPEN_NAMES = new Set([DROPPEN_ORIGIN, "https://my.droppen.app", "https://www.droppen.app"]);

/** Whether the page is on a hub other than droppen.app (a self-hosted or a development one): droppen.app's own names are droppen.app. */
export function otherHub(origin: string): boolean {
  return !DROPPEN_NAMES.has(origin.replace(/\/+$/, ""));
}
