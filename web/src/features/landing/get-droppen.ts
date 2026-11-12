import { DOWNLOAD_PATH } from "./route";

export interface Availability {
  app: boolean;
  appSignIn: boolean;
  linux: boolean;
}

export const AVAILABILITY: Availability = { app: true, appSignIn: true, linux: true };
export const DOWNLOAD_OSES = ["macos-app", "macos-bg", "linux", "windows", "mobile"] as const;
export type DownloadOs = (typeof DOWNLOAD_OSES)[number];

export function downloadHref(os?: DownloadOs | null): string {
  return os ? `${DOWNLOAD_PATH}?os=${os}` : DOWNLOAD_PATH;
}

export function osOfSearch(search: string): DownloadOs | null {
  const os = new URLSearchParams(search).get("os");
  return (DOWNLOAD_OSES as readonly string[]).includes(os ?? "") ? (os as DownloadOs) : null;
}

export const RELEASES_URL = "https://github.com/getdroppen/droppen/releases/latest";

/** The same script every install, enroll and uninstall line starts with. */
const FETCH_INSTALL = "curl -fsSL https://droppen.app/install.sh";

export const INSTALL_COMMAND = `${FETCH_INSTALL} | sh`;

export function codeInstallCommand(code: string, hub?: string): string {
  const where = hub && otherHub(hub) ? `DROPPEN_HUB=${hub} ` : "";
  return `${FETCH_INSTALL} | ${where}DROPPEN_CODE=${code} sh`;
}

// Older installations only have this legacy path; updates do not create the newer bin/droppen link.
const SERVER_BIN_PATH = "~/.local/share/remote-hand/bin/remote-hand-server";

export function stdinEnrollCommand(code: string, hub: string): string {
  return `printf '%s\\n' '${code}' | ${SERVER_BIN_PATH} agent enroll - --hub ${hub}`;
}

export const WSL_SYSTEMD_URL = "https://learn.microsoft.com/windows/wsl/systemd";
export const UNINSTALL_COMMAND = `${FETCH_INSTALL} | sh -s -- --uninstall`;
export const CHECKSUM_COMMAND = "shasum -a 256 <file>";
export const ATTESTATION_COMMAND = "gh attestation verify <file> --repo getdroppen/droppen --signer-workflow getdroppen/droppen/.github/workflows/release.yml --source-ref refs/tags/<version>";
export const VERIFY_DOWNLOAD_URL = "https://github.com/getdroppen/droppen/blob/main/docs/install.md#verify-a-download";
export const LINGER_COMMAND = "sudo loginctl enable-linger";

const DROPPEN_ORIGIN = "https://api.droppen.app";
export const DROPPEN_SITE_ORIGIN = "https://www.droppen.app";
const DROPPEN_APP_ORIGIN = "https://my.droppen.app";
const DROPPEN_NAMES = new Set([DROPPEN_ORIGIN, DROPPEN_APP_ORIGIN, DROPPEN_SITE_ORIGIN, "https://droppen.app"]);

export function otherHub(origin: string): boolean {
  return !DROPPEN_NAMES.has(origin.replace(/\/+$/, ""));
}
