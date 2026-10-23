import { useTranslation } from "react-i18next";
import { Download } from "lucide-react";
import { primary, secondary } from "@/features/cloud/ui";
import type { SignupMode } from "@/lib/hub/api";
import type { LatestRelease } from "@/lib/hub/releases";
import type { HeroOffer } from "./detect";
import { type Availability, downloadHref, type DownloadOs, INSTALL_COMMAND, RELEASES_URL } from "./get-droppen";
import { CommandLine } from "./page-kit";
import { fileFacts, fileLink, LinuxTarballs } from "./release-files";
import { appHost } from "@/lib/hub/hosts";
import { APP_PATH, publicLink, SIGN_IN_PATH, SIGN_UP_PATH, WAITLIST_HREF } from "./route";

/*
 * The home page hero's actions, for the device reading it (`detect.ts`,
 * `heroOffer`), with one primary action:
 *
 * - **a Mac**: 「下载 Mac 版」, the latest release's universal
 *   `Droppen-<version>.dmg` itself, downloaded in place, its version and
 *   size beside it (`release-files.tsx`: the hub names the file; until it
 *   does, or if it cannot, the button links the release page); the release
 *   page beside it, for its checksums and other files; the account's ways in
 *   after it;
 * - **Linux**: the install command, its copy button the primary, the
 *   tarball for this processor (both when it cannot be told), and the
 *   download page's Linux tab for a server with no browser;
 * - **Windows**: no app yet: droppen.app works in this browser (the account's
 *   way in is the primary), and the ways onto a Mac or a Linux server;
 * - **a phone or tablet**: droppen.app in the browser, on the home screen;
 *   the AI runs on a Mac or Linux;
 * - **a device it cannot tell** (or one the build has no way onto yet): the
 *   account's way in, and every way onto a device, each one click away —
 *   the Mac's the file itself once the App is out (`APP_AVAILABLE`), never a
 *   link to a file that is not there.
 *
 * The download page (`download.tsx`) opens on the same device's tab.
 */

const quietLink = "text-muted underline underline-offset-4 hover:text-fg";

/** The download page on `os`'s tab (every tab: no `os`), in place. */
function TabLink({ os, label, testId }: { os?: DownloadOs; label: string; testId: string }) {
  const href = downloadHref(os);
  return (
    <a {...publicLink(href)} data-testid={testId} className={quietLink}>
      {label}
    </a>
  );
}

/** The latest release's page, for its checksums and other files. */
function ReleaseLink({ release }: { release: LatestRelease | null | undefined }) {
  const { t } = useTranslation();
  return (
    <a href={release?.releasesPage ?? RELEASES_URL} target="_blank" rel="noopener noreferrer" data-testid="hero-release-link" className={quietLink}>
      {t("checksums and other files")}
    </a>
  );
}

/**
 * The account's ways in: signed out, sign up (where sign-up is closed, the
 * waitlist) and sign in; signed in, back into the app. `lead`: the hero's
 * primary action is the first.
 */
function Account({ signedIn, lead, closed }: { signedIn: boolean; lead: boolean; closed: boolean }) {
  const { t } = useTranslation();
  const first = lead ? { "data-variant": "primary", className: primary } : { className: secondary };
  if (signedIn) {
    return (
      <>
        <a {...publicLink(APP_PATH)} {...first}>
          {t("Open app")}
        </a>
        {lead && (
          <a href="#journey" className={secondary}>
            {t("How it works")}
          </a>
        )}
      </>
    );
  }
  return (
    <>
      <a {...publicLink(closed ? WAITLIST_HREF : SIGN_UP_PATH)} {...first}>
        {closed ? t("Join the waitlist") : t("Get started")}
      </a>
      <a {...publicLink(SIGN_IN_PATH)} className={secondary}>
        {t("Sign in")}
      </a>
    </>
  );
}

const row = "flex flex-wrap items-center gap-3";
const note = "max-w-[34em] text-xs leading-5 text-muted";
const small = "font-mono text-2xs leading-5 text-faint";

/**
 * The hero's actions for `offer`. `release`: the latest release as the hub
 * names it (`undefined` while asked, `null`: none); `cpu`: the processor,
 * for Linux; `signup`: the hub's sign-up mode (`signup-mode.ts`).
 */
export function HeroGet({
  offer,
  signedIn,
  availability,
  release,
  cpu,
  signup = "open",
}: {
  offer: HeroOffer;
  signedIn: boolean;
  availability: Availability;
  release: LatestRelease | null | undefined;
  cpu: string | null;
  signup?: SignupMode;
}) {
  const { t } = useTranslation();
  const closed = signup === "closed";
  const dmg = release?.files.macos;
  return (
    <div data-testid="hero-get" data-offer={offer} className="mt-8 flex flex-col gap-3">
      {offer === "mac" && (
        <>
          <div className={row}>
            {/* The file itself when the hub named it (GitHub serves it as a download: the page stays), else the release page. */}
            <a {...fileLink(dmg)} data-variant="primary" data-testid="hero-download" className={primary}>
              <Download className="size-3.5" />
              {t("Download for Mac")}
            </a>
            <Account signedIn={signedIn} lead={false} closed={closed} />
          </div>
          <p className={small}>
            {release && dmg && <span data-testid="hero-file-facts">{fileFacts(release, dmg)} · </span>}
            {t("macOS 13 or later · Apple silicon and Intel")} · <TabLink label={t("other systems")} testId="hero-other-systems" /> · <ReleaseLink release={release} />
          </p>
        </>
      )}
      {offer === "linux" && (
        <>
          <div className="flex max-w-[36rem] flex-col gap-2">
            <CommandLine command={INSTALL_COMMAND} label={t("copy the command")} testId="hero-install-command" primaryCopy />
            <p className={small}>
              {t("Paste it into a terminal on this device: Droppen installs, starts and opens its page.")}{" "}
              <TabLink os="linux" label={t("A server with no browser?")} testId="hero-linux-tab" /> · <ReleaseLink release={release} />
            </p>
            <LinuxTarballs release={release} cpu={cpu} testId="hero-tarballs" />
          </div>
          <div className={row}>
            <Account signedIn={signedIn} lead={false} closed={closed} />
          </div>
        </>
      )}
      {offer === "windows" && (
        <>
          <div className={row}>
            <Account signedIn={signedIn} lead closed={closed} />
          </div>
          <p data-testid="hero-windows" className={note}>
            {t("No Windows app yet. Droppen works in this browser: watch and answer your AI from here, while it runs on a Mac or a Linux server.")}{" "}
            <TabLink os="macos-app" label={t("Droppen for Mac")} testId="hero-mac-tab" /> · <TabLink os="linux" label={t("Droppen on a Linux server")} testId="hero-linux-tab" />
          </p>
        </>
      )}
      {offer === "phone" && (
        <>
          <div className={row}>
            <Account signedIn={signedIn} lead closed={closed} />
          </div>
          <p data-testid="hero-phone" className={note}>
            {t("On your phone, Droppen works in the browser: open {{app}} and add it to your home screen. Your AI runs on a Mac or on Linux, with Droppen installed there.", { app: appHost() })}{" "}
            <TabLink os="mobile" label={t("how to set it up")} testId="hero-mobile-tab" />
          </p>
        </>
      )}
      {offer === "all" && (
        <>
          <div className={row}>
            <Account signedIn={signedIn} lead closed={closed} />
          </div>
          <p data-testid="hero-all" className={small}>
            {t("Get Droppen:")}{" "}
            {availability.app ? (
              <a {...fileLink(dmg)} data-testid="hero-download" className={quietLink}>
                {t("Download for Mac")}
              </a>
            ) : (
              <TabLink os="macos-app" label={t("Droppen for Mac")} testId="hero-mac-tab" />
            )}{" "}
            · <TabLink os="linux" label={t("Linux server")} testId="hero-linux-tab" /> · <TabLink os="windows" label={t("Windows")} testId="hero-windows-tab" /> ·{" "}
            <TabLink os="mobile" label={t("Phone / tablet")} testId="hero-mobile-tab" />
          </p>
        </>
      )}
    </div>
  );
}
