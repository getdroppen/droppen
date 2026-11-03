import { Fragment, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { fetchLatestRelease, formatSize, type LatestRelease, type ReleaseFile } from "@/lib/hub/releases";
import { LINUX_CPUS } from "./detect";
import { RELEASES_URL } from "./get-droppen";

/* The latest release, read once per page load, and the links and facts its buttons show. */

let asked: Promise<LatestRelease | null> | null = null;

export function loadLatestRelease(): Promise<LatestRelease | null> {
  if (!asked) {
    asked = fetchLatestRelease().then((release) => {
      if (!release) asked = null;
      return release;
    });
  }
  return asked;
}

/** `undefined` while asked for, `null` when there is none; `given` skips the hub. */
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

export function countedLink(file: ReleaseFile): boolean {
  return file.href !== file.url;
}

/** The file itself, or the release page when there is none; counted links are `nofollow`. */
export function fileLink(file: ReleaseFile | undefined): { href: string; target?: string; rel?: string } {
  if (!file) return { href: RELEASES_URL, target: "_blank", rel: "noopener noreferrer" };
  return countedLink(file) ? { href: file.href, rel: "nofollow" } : { href: file.href };
}

export function fileFacts(release: LatestRelease, file: ReleaseFile): string {
  return `v${release.version} · ${formatSize(file.size)}`;
}

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
    <p data-testid={testId} className="font-mono text-xs font-semibold leading-5 text-faint">
      {sayUnbuilt && cpu && !built && <>{t("Droppen's Linux builds are for x86_64 and aarch64.")} </>}
      {t("Or download it:")}{" "}
      {files.map((file, i) => (
        <Fragment key={file.name}>
          {i > 0 && <span className="max-[360px]:hidden"> · </span>}
          {/* A file's name is one token where the line is wide; below 360px each file takes its own row, so a long name has the full card width. */}
          <span className="whitespace-nowrap max-[360px]:block max-[360px]:whitespace-normal">
            <a
              href={file.href}
              data-testid={`${testId}-file`}
              className="text-muted underline underline-offset-4 hover:text-fg"
              rel={countedLink(file) ? "nofollow" : undefined}
            >
              {file.name}
            </a>{" "}
            <span className="whitespace-nowrap">({formatSize(file.size)})</span>
          </span>
        </Fragment>
      ))}
    </p>
  );
}
