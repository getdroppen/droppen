/*
 * What a device that runs AI says of itself, as people read it: its system
 * and its agent's version (its info, `MachineInfo.os` / `agentVersion`). Shown
 * on its row in 「我的设备」 and in its details.
 *
 * On droppen.app both come off the machine's status stream (`systemLabel`,
 * `agentVersionOf`); the local page has them from its server's directory,
 * which reads the same fields the same way (`server-rs/src/aggregator/live.rs`,
 * `os_of`): the two pages say a system in the same words.
 */
import type { StatusState } from "@/lib/hub/streams";

const text = (v: unknown): string | null => (typeof v === "string" && v.trim() ? v.trim() : null);

/**
 * A machine's system from its status: `macOS 15.3.1 aarch64`, `ubuntu 24.04
 * x86_64` (a Linux by its distribution, else `Linux`), whatever of the version
 * and the architecture the agent knew. An agent from before it said gives its
 * settings' platform (`darwin`, `linux`; {@link osLabel} names it).
 */
export function systemLabel(status: Pick<StatusState, "os" | "settings"> | null | undefined): string | null {
  const os = status?.os;
  if (os) {
    const name = os.name === "macos" ? "macOS" : os.name === "linux" ? (os.distro ?? "Linux") : os.name === "windows" ? "Windows" : os.name;
    return [name, os.version, os.arch].filter((p): p is string => !!p).join(" ");
  }
  return text(status?.settings?.platform);
}

/** The version of a machine's agent from its status: the agent's own word, else what its settings say. */
export function agentVersionOf(status: Pick<StatusState, "agentVersion" | "settings"> | null | undefined): string | null {
  return text(status?.agentVersion) ?? text(status?.settings?.serverVersion);
}

/** A system as people call it, from the name the agent reports (Node's `process.platform`). */
export function osLabel(platform: string | null | undefined): string | null {
  if (!platform) return null;
  switch (platform) {
    case "darwin":
      return "macOS";
    case "linux":
      return "Linux";
    case "win32":
      return "Windows";
    case "freebsd":
      return "FreeBSD";
    default:
      return platform;
  }
}

/** The agent's version as it reads: `v0.2.112`, without the build's commit (kept for the tooltip). */
export function versionLabel(version: string | null | undefined): string | null {
  if (!version) return null;
  const bare = version.split("+")[0]!.trim();
  return bare ? `v${bare.replace(/^v/, "")}` : null;
}
