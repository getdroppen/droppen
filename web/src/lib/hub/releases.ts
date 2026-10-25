/*
 * The latest public release's files, as the hub names them
 * (`GET /v1/releases/latest`, docs/hub-api/http.md): the hub reads GitHub's
 * API itself and checks what it says, and the page asks the hub, on its own
 * origin. So the page asks GitHub for nothing (its CSP's `connect-src` stays
 * the hub and the relays), and its download buttons go straight to the files
 * themselves, which are links: a plain `<a href>`, where nothing is fetched
 * by the page. A hub that counts downloads names, for each file, its own
 * `download_path`, `/dl/<platform>`: a `302` to the file on GitHub that counts
 * one download of that platform for the day, and nothing about who asked.
 * Then the button links that, on this page's origin (the hub it asked);
 * without it (a hub before it, a self-hosted one), the file on GitHub.
 *
 * What the hub says is checked again here, as strictly: a tag `vX.Y.Z`, and
 * each file under its exact name at exactly
 * `https://github.com/getdroppen/droppen/releases/download/<tag>/<name>`; a
 * file that is not so is left out. Anything else (no route on this hub, a
 * 503 before it has read GitHub, a network failure, an answer that does not
 * check out) is `null`, and the buttons link the release page instead.
 */

/** The route. */
export const LATEST_RELEASE_PATH = "/v1/releases/latest";

/** Every file's address starts with this, then the tag, `/` and its name. */
export const RELEASE_DOWNLOAD_PREFIX = "https://github.com/getdroppen/droppen/releases/download/";

/** A release's page: this, then the tag. */
export const RELEASE_PAGE_PREFIX = "https://github.com/getdroppen/droppen/releases/tag/";

/** One file of the release. */
export interface ReleaseFile {
  name: string;
  /** Its address on GitHub. */
  url: string;
  /** Bytes. */
  size: number;
  /**
   * What a download button links: this hub's {@link downloadPath} when the
   * hub names it (a `302` to `url`, counted), else `url` itself.
   */
  href: string;
}

/** The files offered, by platform: the desktop app (universal) and the Linux server's tarballs. */
export type ReleasePlatform = "macos" | "linux-x86_64" | "linux-aarch64";

/** The latest release, as checked here. */
export interface LatestRelease {
  /** `X.Y.Z`. */
  version: string;
  /** `vX.Y.Z`. */
  tag: string;
  publishedAt: string;
  /** Its page on GitHub: the notes, `SHA256SUMS`, every other file. */
  releasesPage: string;
  /** A platform without its file is absent. */
  files: Partial<Record<ReleasePlatform, ReleaseFile>>;
}

/** The name each platform's file must have, for `tag` (`vX.Y.Z`). */
export function releaseFileName(platform: ReleasePlatform, tag: string): string {
  switch (platform) {
    case "macos":
      return `Droppen-${tag.slice(1)}.dmg`;
    case "linux-x86_64":
      return `droppen-${tag}-linux-x86_64.tar.gz`;
    case "linux-aarch64":
      return `droppen-${tag}-linux-aarch64.tar.gz`;
  }
}

/**
 * Where a hub that counts downloads sends a platform's on to its file:
 * `/dl/<platform>`, on the hub's own origin (docs/hub-api/http.md
 * `GET /dl/{platform}`). Taken only when the hub's answer names exactly this.
 */
export function downloadPath(platform: ReleasePlatform): string {
  return `/dl/${platform}`;
}

const PLATFORMS: ReleasePlatform[] = ["macos", "linux-x86_64", "linux-aarch64"];

const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/** The hub's answer, checked ({@link LatestRelease}); `null` when it does not check out or names no file. */
export function parseLatestRelease(body: unknown): LatestRelease | null {
  if (!isRecord(body)) return null;
  const { tag, version, published_at: publishedAt, platforms } = body;
  if (typeof tag !== "string" || !/^v\d{1,9}\.\d{1,9}\.\d{1,9}$/.test(tag)) return null;
  if (version !== tag.slice(1) || typeof publishedAt !== "string" || !isRecord(platforms)) return null;
  const files: LatestRelease["files"] = {};
  for (const platform of PLATFORMS) {
    const file = platforms[platform];
    if (!isRecord(file)) continue;
    const name = releaseFileName(platform, tag);
    const { url, size } = file;
    if (file.name !== name || url !== `${RELEASE_DOWNLOAD_PREFIX}${tag}/${name}`) continue;
    if (typeof size !== "number" || !Number.isSafeInteger(size) || size <= 0) continue;
    // The hub's counted link, only as this page would make it: never an address taken on trust.
    const counted = file.download_path === downloadPath(platform);
    files[platform] = { name, url, size, href: counted ? downloadPath(platform) : url };
  }
  if (Object.keys(files).length === 0) return null;
  // Made from the tag, as the hub makes it: never an address taken on trust.
  return { version, tag, publishedAt, releasesPage: `${RELEASE_PAGE_PREFIX}${tag}`, files };
}

/** Ask this page's hub, same-origin; `null` on any failure. */
export async function fetchLatestRelease(fetcher: typeof fetch = fetch): Promise<LatestRelease | null> {
  try {
    const response = await fetcher(LATEST_RELEASE_PATH, {
      credentials: "omit",
      headers: { accept: "application/json" },
      signal: typeof AbortSignal !== "undefined" && "timeout" in AbortSignal ? AbortSignal.timeout(10_000) : undefined,
    });
    if (!response.ok) return null;
    return parseLatestRelease(await response.json());
  } catch {
    return null;
  }
}

/** A file's size as people read it: `48 MB`, `9.5 MB`, `640 KB`. */
export function formatSize(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
  if (bytes >= 1e6) return `${bytes >= 1e7 ? Math.round(bytes / 1e6) : (bytes / 1e6).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1e3))} KB`;
}
