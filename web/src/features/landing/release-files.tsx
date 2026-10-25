import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { fetchLatestRelease, formatSize, type LatestRelease, type ReleaseFile } from "@/lib/hub/releases";
import { LINUX_CPUS } from "./detect";
import { RELEASES_URL } from "./get-droppen";

/*
 * The download buttons' files (`lib/hub/releases.ts`): the latest release as
 * this page's hub names it, read once per page load. A button with its file
 * is a plain link to it, which downloads in place, with the version and the
 * size beside it — through the hub's counted `/dl/<platform>` when the hub
 * names one (`ReleaseFile.href`), else straight to GitHub; without it (no
 * answer yet, or none: a hub without the route, a 503, a network failure)
 * it is a link to the release page, in a new tab. Shared by the home page's
 * hero (`hero-get.tsx`), the download page (`download.tsx`) and the
 * add-device guide.
 */

let asked: Promise<LatestRelease | null> | null = null;

/** The latest release, asked of the hub once per page load (asked again on the next use after a failure). */
export function loadLatestRelease(): Promise<LatestRelease | null> {
  if (!asked) {
    asked = fetchLatestRelease().then((release) => {
      if (!release) asked = null;
      return release;
    });
  }
  return asked;
}

/**
 * The latest release: `undefined` while it is asked for, `null` when there
 * is none to be had. `given` (tests, previews) stands in for the hub: then
 * nothing is asked.
 */
export function useLatestRelease(given?: LatestRelease | null): LatestRelease | null | undefined {
  const [release, setRelease] = useState<LatestRelease | null | undefined>(undefined);
  useEffect(() => {
    if (given !== undefined) return;
    let live = true;
    void loadLatestRelease().then((value) => live && setRelease(value));
    return () => {
      live = false;
    };
  }, [given]);
  return given !== undefined ? given : release;
}

/** Whether a file's link is the hub's counted `/dl/<platform>`, not the file on GitHub. */
export function countedLink(file: ReleaseFile): boolean {
  return file.href !== file.url;
}

/**
 * A download button's link: the file itself (in place; through the hub's
 * `/dl/` when it counts, `nofollow` so that crawlers leave the count alone,
 * as `/robots.txt` asks too), else the release page (a new tab).
 */
export function fileLink(file: ReleaseFile | undefined): { href: string; target?: string; rel?: string } {
  if (!file) return { href: RELEASES_URL, target: "_blank", rel: "noopener noreferrer" };
  return countedLink(file) ? { href: file.href, rel: "nofollow" } : { href: file.href };
}

/** `v0.2.129 · 48 MB`: the version and size of the file a button downloads. */
export function fileFacts(release: LatestRelease, file: ReleaseFile): string {
  return `v${release.version} · ${formatSize(file.size)}`;
}

/**
 * The Linux tarballs, after the install command: the one for this
 * processor when it is known and built for, else both (with a word when the
 * processor has no build). Nothing until the hub names them: the release
 * page is linked beside the command already.
 */
export function LinuxTarballs({
  release,
  cpu,
  testId,
  sayUnbuilt = true,
}: {
  release: LatestRelease | null | undefined;
  cpu: string | null;
  testId: string;
  /** Say when the processor has no build (off where the page says it already). */
  sayUnbuilt?: boolean;
}) {
  const { t } = useTranslation();
  if (!release) return null;
  const built = (LINUX_CPUS as readonly string[]).includes(cpu ?? "");
  const platforms: ("linux-x86_64" | "linux-aarch64")[] = cpu === "x86_64" ? ["linux-x86_64"] : cpu === "aarch64" ? ["linux-aarch64"] : ["linux-x86_64", "linux-aarch64"];
  const files = platforms.map((platform) => release.files[platform]).filter((file): file is ReleaseFile => !!file);
  if (files.length === 0) return null;
  return (
    <p data-testid={testId} className="font-mono text-2xs leading-5 text-faint">
      {sayUnbuilt && cpu && !built && <>{t("Droppen's Linux builds are for x86_64 and aarch64.")} </>}
      {t("Or download it:")}{" "}
      {files.map((file, i) => (
        <span key={file.name}>
          {i > 0 && " · "}
          <a
            href={file.href}
            data-testid={`${testId}-file`}
            className="text-muted underline underline-offset-4 [overflow-wrap:anywhere] hover:text-fg"
            rel={countedLink(file) ? "nofollow" : undefined}
          >
            {file.name}
          </a>{" "}
          ({formatSize(file.size)})
        </span>
      ))}
    </p>
  );
}
