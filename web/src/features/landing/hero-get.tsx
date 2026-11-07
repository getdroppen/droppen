import { useTranslation } from "react-i18next";
import type { SignupMode } from "@/lib/hub/api";
import { AppleLogoIcon } from "@phosphor-icons/react";
import type { LatestRelease } from "@/lib/hub/releases";
import type { HeroOffer } from "./detect";
import { type Availability, downloadHref, type DownloadOs, INSTALL_COMMAND, RELEASES_URL } from "./get-droppen";
import { ArrowLink, ButtonLink, CommandLine, QUIET_NOWRAP_LINK } from "./page-kit";
import { DownloadButton, fileFacts, fileLink, LinuxTarballs } from "./release-files";
import { appHost } from "@/lib/hub/hosts";
import { APP_PATH, publicLink, SIGN_UP_PATH, WAITLIST_HREF } from "./route";
import { SIGNUP_COPY } from "./signup-copy";

function TabLink({ os, label, testId }: { os?: DownloadOs; label: string; testId: string }) {
  const href = downloadHref(os);
  return (
    <a {...publicLink(href)} data-testid={testId} className={QUIET_NOWRAP_LINK}>
      {label}
    </a>
  );
}

function ReleaseLink({ release }: { release: LatestRelease | null | undefined }) {
  const { t } = useTranslation();
  return (
    <a href={release?.releasesPage ?? RELEASES_URL} target="_blank" rel="noopener noreferrer" data-testid="hero-release-link" className={QUIET_NOWRAP_LINK}>
      {t("checksums and other files")}
    </a>
  );
}

function Account({ signedIn, lead, signup }: { signedIn: boolean; lead: boolean; signup: SignupMode }) {
  const { t } = useTranslation();
  const variant = lead ? "primary" : "secondary";
  const closed = signup === "closed";
  if (signedIn) {
    return (
      <>
        <ButtonLink variant={variant} {...publicLink(APP_PATH)}>
          {t("Open app")}
        </ButtonLink>
        {lead && (
          <ButtonLink variant="secondary" href="#tour">
            {t("How it works")}
          </ButtonLink>
        )}
      </>
    );
  }
  return (
    <ButtonLink variant={variant} {...publicLink(closed ? WAITLIST_HREF : SIGN_UP_PATH)}>
      {t(SIGNUP_COPY[signup].freeCta)}
    </ButtonLink>
  );
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
    <div data-testid="hero-get" data-offer={offer} className="mt-4 flex w-full flex-col items-center gap-3 [&_a]:shrink-0 [&_button]:shrink-0">
      {offer === "mac" && (
        <>
          <div className={row}>
            <DownloadButton
              file={dmg}
              mark={<AppleLogoIcon weight="fill" className="size-4 shrink-0" />}
              testId="hero-download"
              label={t("Download for Mac")}
              fallback={publicLink(downloadHref("macos-app"))}
            />
            <Account signedIn={signedIn} lead={false} signup={signup} />
          </div>
          <p className={small}>
            {release && dmg && <span data-testid="hero-file-facts">{fileFacts(release, dmg)} · </span>}
            {t("macOS 13 or later · Apple silicon and Intel")} · <TabLink label={t("Linux and other platforms")} testId="hero-other-systems" />
          </p>
        </>
      )}
      {offer === "linux" && (
        <>
          <div className="flex max-w-[36rem] flex-col gap-2">
            <CommandLine command={INSTALL_COMMAND} testId="hero-install-command" primaryCopy />
            <p className={small}>
              {t("Paste it into a terminal on this device: Droppen installs, starts and opens its page.")}{" "}
              <TabLink os="linux" label={t("A server with no browser?")} testId="hero-linux-tab" /> · <ReleaseLink release={release} />
            </p>
            <LinuxTarballs release={release} cpu={cpu} testId="hero-tarballs" />
          </div>
          <div className={row}>
            <Account signedIn={signedIn} lead={false} signup={signup} />
          </div>
        </>
      )}
      {offer === "windows" && (
        <div data-testid="hero-windows" className="flex flex-wrap items-center gap-3">
          <ArrowLink
            variant="primary"
            {...publicLink(signedIn ? APP_PATH : closed ? WAITLIST_HREF : SIGN_UP_PATH)}
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
            <Account signedIn={signedIn} lead signup={signup} />
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
            <Account signedIn={signedIn} lead signup={signup} />
          </div>
          <p data-testid="hero-all" className={small}>
            {t("Get Droppen:")}{" "}
            {availability.app ? (
              <a {...(dmg ? fileLink(dmg) : publicLink(downloadHref("macos-app")))} data-testid="hero-download" className={QUIET_NOWRAP_LINK}>
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
