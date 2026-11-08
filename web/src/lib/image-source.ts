import { localFilePath } from "@/lib/fs-url";

/**
 * Where a picture an agent wrote into its reply is actually drawn from.
 *
 *  * `host` — a path on this machine, served by `/api/fs/download` under the
 *    same home-directory fence as every other file (`components/host-file.tsx`).
 *  * `github` — a file in a GitHub repository, which may be private: the
 *    browser has no GitHub login, so `/api/media/github` fetches it with the
 *    host's `gh` instead. The server's `media::parse_github_url` is the
 *    authority on what it accepts; {@link isGithubFileUrl} only has to agree
 *    with it closely enough that a URL it sends there is one the server takes.
 *  * `direct` — any other `http(s)` image, loaded by the browser itself and
 *    only when the reader asks for it: the URL is the agent's choice, and
 *    fetching it on sight would hand the server it names whatever the URL
 *    carries as soon as the reply is on screen (`components/image-lightbox.tsx`).
 *    It loads with no referrer, so the third party is not told which
 *    transcript it is in either.
 */
export type ImageSource =
  | { kind: "host"; path: string }
  | { kind: "github"; url: string }
  | { kind: "direct"; url: string };

/** The source for a markdown image's `src`, or null when nothing can draw it. */
export function imageSource(src: string | undefined): ImageSource | null {
  if (!src) return null;
  const path = localFilePath(src);
  if (path) return { kind: "host", path };
  if (isGithubFileUrl(src)) return { kind: "github", url: src };
  if (/^https?:\/\//i.test(src)) return { kind: "direct", url: src };
  // Relative paths, `data:` and anything else markdown let through: there is
  // no base to resolve them against that would mean what the agent meant.
  return null;
}

/**
 * Whether a URL names a file in a GitHub repository in one of the shapes the
 * proxy fetches: `github.com/<owner>/<repo>/(blob|raw)/<ref>/<path>` with no
 * query beyond `raw=true|1`, or `raw.githubusercontent.com/<owner>/<repo>/<ref>/<path>`
 * with at most a `token`. `user-attachments` and other GitHub pages are not
 * files in a repository, so they load directly like any other image.
 */
export function isGithubFileUrl(src: string): boolean {
  let url: URL;
  try {
    url = new URL(src);
  } catch {
    return false;
  }
  if (url.protocol !== "https:" || url.port || url.username || url.password) return false;
  const segments = url.pathname.split("/").slice(1);
  if (segments.some((segment) => segment === "")) return false;
  const params = [...url.searchParams];
  const only = (name: string, accept: (value: string) => boolean) =>
    params.length === 0 || (params.length === 1 && params[0]?.[0] === name && accept(params[0][1]));
  if (url.hostname === "github.com") {
    const raw = only("raw", (value) => value === "true" || value === "1");
    return raw && segments.length >= 5 && (segments[2] === "blob" || segments[2] === "raw");
  }
  if (url.hostname === "raw.githubusercontent.com") {
    return only("token", () => true) && segments.length >= 4;
  }
  return false;
}

/** A file name to save the picture under: the last path segment, decoded. */
export function imageFileName(source: ImageSource): string {
  const path = source.kind === "host" ? source.path : safePathname(source.url);
  const last = path.split("/").filter(Boolean).pop() ?? "image";
  try {
    return decodeURIComponent(last);
  } catch {
    return last;
  }
}

function safePathname(url: string): string {
  try {
    return new URL(url).pathname;
  } catch {
    return url;
  }
}
