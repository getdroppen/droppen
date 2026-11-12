import { Fragment, type MouseEvent, type ReactNode, type RefObject, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { fetchLatestRelease, formatSize, type LatestRelease, type ReleaseFile } from "@/lib/hub/releases";
import { cn } from "@/lib/cn";
import { LINUX_CPUS } from "./detect";
import { RELEASES_URL } from "./get-droppen";
import { ButtonLink, TextLink } from "./page-kit";

let asked: Promise<LatestRelease | null> | null = null;

function loadLatestRelease(): Promise<LatestRelease | null> {
  if (!asked) {
    asked = fetchLatestRelease().then((release) => {
      if (!release) asked = null;
      return release;
    });
  }
  return asked;
}

export function useLatestRelease(given?: LatestRelease | null, ask = true): LatestRelease | null | undefined {
  const [release, setRelease] = useState<LatestRelease | null | undefined>(undefined);
  useEffect(() => {
    if (!ask || given !== undefined) return;
    let live = true;
    void loadLatestRelease().then((value) => live && setRelease(value));
    return () => {
      live = false;
    };
  }, [ask, given]);
  if (given !== undefined) return given;
  return ask ? release : null;
}

const releasePageLink = { href: RELEASES_URL, target: "_blank", rel: "noopener noreferrer" } as const;

export function fileLink(file: ReleaseFile | undefined): { href: string; target?: string; rel?: string } {
  if (!file) return releasePageLink;
  return file.href !== file.url ? { href: file.href, rel: "nofollow" } : { href: file.href };
}

export function fileFacts(release: LatestRelease, file: ReleaseFile): string {
  return `v${release.version} · ${formatSize(file.size)}`;
}

export function DownloadButton({ file, mark, label, testId, fallback, menu }: {
  file?: ReleaseFile;
  mark: ReactNode;
  label: string;
  testId: string;
  fallback?: { href: string; onClick?: (event: MouseEvent<HTMLAnchorElement>) => void };
  menu?: (anchor: RefObject<HTMLDivElement | null>) => ReactNode;
}) {
  const group = useRef<HTMLDivElement>(null);
  const link = file ? fileLink(file) : (fallback ?? releasePageLink);
  return (
    <div ref={group} className={cn("inline-flex w-fit", menu ? "relative" : "items-center")}>
      <ButtonLink {...link} variant="primary" data-testid={testId} className={cn("min-h-9", menu ? "border-fg border-r-0 bg-fg" : "border-fg")}>
        {mark}
        {label}
      </ButtonLink>
      {menu?.(group)}
    </div>
  );
}

export function LinuxTarballs({ release, cpu, testId }: {
  release: LatestRelease | null | undefined;
  cpu: string | null;
  testId: string;
}) {
  const { t } = useTranslation();
  if (!release) return null;
  const built = (LINUX_CPUS as readonly string[]).includes(cpu ?? "");
  const platforms: ("linux-x86_64" | "linux-aarch64")[] = cpu === "x86_64" ? ["linux-x86_64"] : cpu === "aarch64" ? ["linux-aarch64"] : ["linux-x86_64", "linux-aarch64"];
  const files = platforms.map((platform) => release.files[platform]).filter((file): file is ReleaseFile => !!file);
  if (files.length === 0) return null;
  return (
    <p data-testid={testId} className="font-mono text-xs font-semibold leading-5 text-faint">
      {cpu && !built && <>{t("Droppen's Linux builds are for x86_64 and aarch64.")} </>}
      {t("Or download it:")}{" "}
      {files.map((file, i) => (
        <Fragment key={file.name}>
          {i > 0 && <span className="max-[360px]:hidden"> · </span>}
          <span className="whitespace-nowrap max-[360px]:block max-[360px]:whitespace-normal">
            <TextLink {...fileLink(file)} quiet data-testid={`${testId}-file`}>{file.name}</TextLink>{" "}
            <span className="whitespace-nowrap">({formatSize(file.size)})</span>
          </span>
        </Fragment>
      ))}
    </p>
  );
}
