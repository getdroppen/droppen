import { useTranslation } from "react-i18next";
import { Download } from "lucide-react";
import type { SignupMode } from "@/lib/hub/api";
import type { LatestRelease, ReleaseFile } from "@/lib/hub/releases";
import type { HeroOffer } from "./detect";
import { type Availability, downloadHref, type DownloadOs, INSTALL_COMMAND, RELEASES_URL } from "./get-droppen";
import { ArrowLink, CommandLine, landingPrimary, landingSecondary } from "./page-kit";
import { fileFacts, fileLink, LinuxTarballs } from "./release-files";
import { appHost } from "@/lib/hub/hosts";
import { APP_PATH, publicLink, SIGN_UP_PATH, WAITLIST_HREF } from "./route";

/*
 * The hero's actions for the device reading the page: the App's download on
 * a Mac, the install command on Linux, the web app on a phone or Windows, and
 * every way onto a device when it cannot be told. One primary action.
 */

const quietLink = "whitespace-nowrap text-muted underline underline-offset-4 hover:text-fg";

function TabLink({ os, label, testId }: { os?: DownloadOs; label: string; testId: string }) {
  const href = downloadHref(os);
  return (
    <a {...publicLink(href)} data-testid={testId} className={quietLink}>
      {label}
    </a>
  );
}

function ReleaseLink({ release }: { release: LatestRelease | null | undefined }) {
  const { t } = useTranslation();
  return (
    <a href={release?.releasesPage ?? RELEASES_URL} target="_blank" rel="noopener noreferrer" data-testid="hero-release-link" className={quietLink}>
      {t("checksums and other files")}
    </a>
  );
}

/** The account's way in; `lead` makes it the hero's primary. */
function Account({ signedIn, lead, closed }: { signedIn: boolean; lead: boolean; closed: boolean }) {
  const { t } = useTranslation();
  const first = lead ? { "data-variant": "primary", className: landingPrimary } : { className: landingSecondary };
  if (signedIn) {
    return (
      <>
        <a {...publicLink(APP_PATH)} {...first}>
          {t("Open app")}
        </a>
        {lead && (
          <a href="#tour" className={landingSecondary}>
            {t("How it works")}
          </a>
        )}
      </>
    );
  }
  return (
    <>
      <a {...publicLink(closed ? WAITLIST_HREF : SIGN_UP_PATH)} {...first}>
        {closed ? t("Join the waitlist") : t("Create a free account")}
      </a>
    </>
  );
}

function macDownload(dmg: ReleaseFile | undefined) {
  return dmg ? fileLink(dmg) : publicLink(downloadHref("macos-app"));
}

const row = "flex flex-wrap items-center justify-center gap-3";
const note = "max-w-[34em] text-sm leading-6 text-muted";
const small = "font-mono text-xs leading-5 text-muted";

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
    <div data-testid="hero-get" data-offer={offer} className="mt-8 flex w-full flex-col items-center gap-3">
      {offer === "mac" && (
        <>
          <div className={row}>
            {/* The file itself when the hub named it (a click downloads it: the page stays), else this site's download page, on the Mac's tab. */}
            <a {...macDownload(dmg)} data-variant="primary" data-testid="hero-download" className={landingPrimary}>
              <Download className="size-3.5" />
              {t("Download for Mac")}
            </a>
            <Account signedIn={signedIn} lead={false} closed={closed} />
          </div>
          <p className={small}>
            {release && dmg && <span data-testid="hero-file-facts">{fileFacts(release, dmg)} · </span>}
            {/* The checksums and the release's other files are the download page's, one click away. */}
            {t("macOS 13 or later · Apple silicon and Intel")} · <TabLink label={t("Linux and other platforms")} testId="hero-other-systems" />
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
        <div data-testid="hero-windows" className="flex flex-wrap items-center gap-3">
          <ArrowLink
            variant="primary"
            {...publicLink(signedIn ? APP_PATH : closed ? WAITLIST_HREF : SIGN_UP_PATH)}
            data-variant="primary"
            data-testid="hero-web-signup"
          >
            {signedIn ? t("Open web app") : closed ? t("Join the web waitlist") : t("Sign up on the web")}
          </ArrowLink>
          <ArrowLink {...publicLink(downloadHref("macos-app"))} data-testid="hero-mac-tab">
            {t("Droppen for Mac")}
          </ArrowLink>
          <ArrowLink {...publicLink(downloadHref("linux"))} data-testid="hero-linux-tab">
            {t("Run Droppen on a Linux server")}
          </ArrowLink>
        </div>
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
              <a {...macDownload(dmg)} data-testid="hero-download" className={quietLink}>
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
